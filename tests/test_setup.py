"""Tests for scripts/anthology_setup.py (/anthology:setup preview, apply and remove).

Cases that need git skip when it is missing (the Docker test images have none).
"""
from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import stat
import subprocess
import sys
import types
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest import mock

from _mem import ROOT, SCRIPTS, MemCase, needs_git, note_text, snapshot, write

import _rtk  # noqa: E402
import anthology_setup  # noqa: E402
import memory_sync  # noqa: E402
import settings_patch  # noqa: E402
import state  # noqa: E402

PLUGIN = ROOT / "plugins" / "anthology"
BYPASS = ("Note: `Bash(rtk *)` also auto-approves `rtk bypass <cmd>`, which runs any command; "
          "the guardrails and deny rules still apply to the wrapped command.")
SETTINGS = {"model": "opus", "permissions": {"allow": ["Bash(ls)"]}}


def sha(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


class FakeRtk:
    """Stands in for rtk_install with its real interface (load_lock returns the parsed lock,
    reconcile returns the {"rtk_path", "warning", "installed"} dict). RealRtkTest runs the real
    module, so this fake cannot drift from it unnoticed."""

    def __init__(self):
        self.calls = []
        self.result = {"rtk_path": None, "warning": None, "installed": True}

    def load_lock(self, path):
        return json.loads(Path(path).read_text(encoding="utf-8"))

    def target(self, system, machine):
        return "test-target"

    def reconcile(self, data_dir, lock, fetch=None, timeout=30, system=None, machine=None):
        self.calls.append((data_dir, lock, fetch, timeout))
        return self.result


class SetupCase(MemCase):
    def setUp(self):
        super().setUp()
        os.environ["CLAUDE_PLUGIN_DATA"] = str(self.config / "plugins" / "data" / "anthology-anthology")
        self.data_dir = Path(os.environ["CLAUDE_PLUGIN_DATA"])
        self.settings = self.config / "settings.json"
        self.rtk = FakeRtk()
        patcher = mock.patch.dict(sys.modules, {"rtk_install": self.rtk})
        patcher.start()
        self.addCleanup(patcher.stop)

    def write_settings(self, data=SETTINGS, indent=2):
        text = json.dumps(data, indent=indent) + "\n"
        write(self.settings, text)
        return text

    def preview(self, cwd=None):
        return anthology_setup.preview(PLUGIN, cwd=cwd or self.tmp)

    def apply(self, cwd=None, expect=None, **kw):
        if expect is None:
            expect = self.preview(cwd)["settings"]["sha256"]
        return anthology_setup.apply(PLUGIN, expect, cwd=cwd or self.tmp, **kw)

    def remove(self, cwd=None, **kw):
        return anthology_setup.remove(PLUGIN, cwd=cwd or self.tmp, **kw)


class PreviewTest(SetupCase):
    def test_preview_writes_nothing(self):
        self.write_settings()
        (self.data_dir).mkdir(parents=True)
        before = snapshot(self.tmp)
        out = self.preview()
        self.assertEqual(snapshot(self.tmp), before)
        self.assertEqual(self.rtk.calls, [])
        json.dumps(out)  # JSON-serialisable

    def test_preview_reports_every_step(self):
        text = self.write_settings()
        out = self.preview()
        self.assertTrue(out["supported"])
        self.assertEqual({c["name"] for c in out["doctor"]["checks"]},
                         {"python3", "node", "docker", "browseros", "rtk", "platform_supported",
                          "plugin_read_rule", "browseros_tools", "plugin_data", "path_wrappers",
                          "agent_memory_path", "subagent_model"})
        self.assertEqual(out["duplicates"], [])
        s = out["settings"]
        self.assertEqual(s["sha256"], sha(text))
        self.assertTrue(s["changed"])
        self.assertIn("+++ settings.json", s["diff"])
        self.assertIn("Bash(rtk *)", s["diff"])
        self.assertIn("Bash(anthology-mem save", s["diff"])
        self.assertIn('"head"', s["diff"])
        self.assertEqual(s["bypass_note"], BYPASS)
        self.assertEqual(out["memory"]["personal_dir"], str(self.anth / "personal-memory"))
        self.assertFalse(out["memory"]["personal_exists"])

    def test_preview_runs_the_compat_checks_in_its_cwd(self):
        os.environ.pop("CLAUDE_CODE_SUBAGENT_MODEL", None)
        proj = self.tmp / "proj"
        write(proj / ".claude" / "settings.local.json", json.dumps({"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}))
        checks = {c["name"]: c for c in self.preview(cwd=proj)["doctor"]["checks"]}
        self.assertFalse(checks["subagent_model"]["ok"])
        self.assertNotIn('"m"', json.dumps(checks["subagent_model"]))
        checks = {c["name"]: c for c in self.preview(cwd=self.tmp)["doctor"]["checks"]}
        self.assertTrue(checks["subagent_model"]["ok"])

    def test_missing_settings_file_previews_a_new_file(self):
        out = self.preview()
        self.assertEqual(out["settings"]["sha256"], sha(""))
        self.assertIn("worktree", out["settings"]["diff"])

    def test_rtk_step_comes_from_the_lock(self):
        lock = self.tmp / "p" / "anthology" / "rtk.lock"
        root = lock.parent
        (root / ".claude-plugin").mkdir(parents=True)
        shutil.copy(str(PLUGIN / ".claude-plugin" / "plugin.json"), str(root / ".claude-plugin" / "plugin.json"))
        guard = self.tmp / "p" / "anthology-guardrails"
        shutil.copytree(str(PLUGIN.parent / "anthology-guardrails" / "scripts"), str(guard / "scripts"))
        lock.write_text(json.dumps({"version": "1.2.1-anthology.1", "tag": "t", "targets": {}}), encoding="utf-8")
        out = anthology_setup.preview(root, cwd=self.tmp)
        self.assertEqual(out["rtk"]["version"], "1.2.1-anthology.1")
        self.assertEqual(out["rtk"]["target"], "test-target")
        self.assertEqual(out["rtk"]["dest"], str(self.data_dir / "rtk" / "1.2.1-anthology.1" / "rtk"))

    def test_rtk_step_skipped_when_wiring_is_absent(self):
        with mock.patch.dict(sys.modules, {"rtk_install": None}):
            out = self.preview()
        self.assertIn("rtk wiring not present", out["rtk"]["skipped"])

    def test_duplicates_are_listed_with_uninstall_commands(self):
        write(self.config / "plugins" / "installed_plugins.json", json.dumps({"version": 2, "plugins": {
            "superpowers@claude-plugins-official": [{"scope": "user", "installPath": "/p"}]}}))
        out = self.preview()
        self.assertEqual(out["duplicates"][0]["command"],
                         "claude plugin uninstall superpowers@claude-plugins-official -s user")

    def test_not_a_git_repo_is_reported(self):
        out = self.preview(cwd=self.tmp)
        self.assertEqual(out["memory"]["repo"], {"skipped": "not a git repo"})

    @needs_git
    def test_repo_step_names_the_key(self):
        repo = self.make_repo(remote="https://example.invalid/acme/app.git")
        out = self.preview(cwd=repo)
        self.assertEqual(out["memory"]["repo"]["key"], "example.invalid/acme/app")
        self.assertFalse(out["memory"]["repo"]["approved"])

    def test_unparseable_settings_are_reported_not_raised(self):
        write(self.settings, "{nope")
        out = self.preview()
        self.assertIn("not valid JSON", out["settings"]["error"])

    def test_windows_is_refused(self):
        with mock.patch("platform.system", return_value="Windows"):
            out = self.preview()
        self.assertFalse(out["supported"])
        self.assertIn("macOS and Linux", out["message"])
        self.assertNotIn("settings", out)


class RealRtkTest(MemCase):
    """The real rtk_install behind the setup adapters, with a fake download."""

    def setUp(self):
        super().setUp()
        self.data_dir = self.tmp / "plugin-data"
        self.blob = _rtk.tarball()
        self.root = self.tmp / "plugin"
        self.lock = _rtk.write_lock(self.root / "rtk.lock", self.blob)
        for patcher in (mock.patch("platform.system", return_value=_rtk.LINUX[0]),
                        mock.patch("platform.machine", return_value=_rtk.LINUX[1])):
            patcher.start()
            self.addCleanup(patcher.stop)

    def test_preview_names_the_version_target_and_destination(self):
        out = anthology_setup._rtk_preview(self.root, str(self.data_dir))
        self.assertEqual(out, {"version": _rtk.VERSION, "target": _rtk.TARGET,
                               "dest": str(self.data_dir / "rtk" / _rtk.VERSION / "rtk")})

    def test_apply_installs_then_reports_up_to_date(self):
        import rtk_install
        fetch = _rtk.FakeFetch(self.blob)
        with mock.patch.object(rtk_install, "default_fetch", fetch):
            self.assertEqual(anthology_setup._rtk_apply(self.root, str(self.data_dir)), ("installed", None))
            self.assertEqual(anthology_setup._rtk_apply(self.root, str(self.data_dir)), ("up to date", None))
        want = str(self.data_dir / "rtk" / _rtk.VERSION / "rtk")
        self.assertEqual(state.read()["rtk_path"], want)
        self.assertTrue(os.access(want, os.X_OK))
        self.assertEqual(len(fetch.calls), 1)
        self.assertEqual(fetch.calls[0][2], anthology_setup.RTK_TIMEOUT)

    def test_a_bad_download_is_a_warning(self):
        import rtk_install
        with mock.patch.object(rtk_install, "default_fetch", _rtk.FakeFetch(b"wrong")):
            status, warning = anthology_setup._rtk_apply(self.root, str(self.data_dir))
        self.assertEqual(status, "warning")
        self.assertIn("rtk is not installed or up to date", warning)
        self.assertNotIn("rtk_path", state.read())

    def test_an_invalid_lock_is_a_warning(self):
        (self.root / "rtk.lock").write_text('{"version": "1"}', encoding="utf-8")
        status, warning = anthology_setup._rtk_apply(self.root, str(self.data_dir))
        self.assertEqual(status, "failed")
        self.assertIn("rtk.lock", warning)
        self.assertEqual(anthology_setup._rtk_preview(self.root, str(self.data_dir)),
                         {"skipped": "rtk wiring not present (no rtk.lock)"})


class ApplyRemoveTest(SetupCase):
    def test_round_trip_leaves_settings_byte_identical_and_state_gone(self):
        text = self.write_settings()
        res = self.apply()
        self.assertTrue(res["ok"], res)
        data = json.loads(self.settings.read_text(encoding="utf-8"))
        self.assertIn("Bash(rtk *)", data["permissions"]["allow"])
        self.assertEqual(data["worktree"]["baseRef"], "head")
        self.assertEqual(data["model"], "opus")
        st = state.read()
        self.assertEqual(st["setup_done"], "0.1.0")
        self.assertTrue(st["settings_ops"])
        res = self.remove()
        self.assertTrue(res["ok"], res)
        self.assertEqual(self.settings.read_bytes(), text.encode("utf-8"))
        self.assertFalse(state.state_path().exists())

    def test_round_trip_with_crlf_and_no_settings_file(self):
        text = (json.dumps(SETTINGS, indent=2) + "\n").replace("\n", "\r\n")
        self.settings.parent.mkdir(parents=True, exist_ok=True)
        self.settings.write_bytes(text.encode("utf-8"))
        self.apply()
        self.remove()
        self.assertEqual(self.settings.read_bytes(), text.encode("utf-8"))
        self.settings.unlink()
        self.apply()
        self.assertTrue(self.settings.exists())
        self.remove()
        self.assertFalse(self.settings.exists())
        self.assertFalse(state.state_path().exists())

    def test_apply_twice_then_remove_still_restores_everything(self):
        text = self.write_settings()
        self.apply()
        self.apply()
        self.remove()
        self.assertEqual(self.settings.read_text(encoding="utf-8"), text)

    def test_apply_refuses_when_settings_changed_since_the_preview(self):
        self.write_settings()
        digest = self.preview()["settings"]["sha256"]
        changed = self.write_settings(dict(SETTINGS, model="sonnet"))
        res = anthology_setup.apply(PLUGIN, digest, cwd=self.tmp)
        self.assertFalse(res["ok"])
        self.assertIn("changed since the preview", res["error"])
        self.assertEqual(self.settings.read_text(encoding="utf-8"), changed)
        self.assertFalse(state.state_path().exists())
        self.assertEqual(self.rtk.calls, [])

    def test_unparseable_settings_stop_apply_before_anything_else(self):
        write(self.settings, "{nope")
        res = self.apply()
        self.assertFalse(res["ok"])
        self.assertIn("not valid JSON", res["error"])
        self.assertEqual(self.settings.read_text(encoding="utf-8"), "{nope")
        self.assertEqual(self.rtk.calls, [])
        self.assertFalse((self.anth / "personal-memory").exists())

    def test_apply_creates_the_personal_folder_with_mode_0700(self):
        self.write_settings()
        res = self.apply()
        personal = self.anth / "personal-memory"
        self.assertTrue(personal.is_dir())
        self.assertEqual(stat.S_IMODE(personal.stat().st_mode), 0o700)
        self.assertEqual(res["memory"]["personal_dir"], str(personal))

    def test_apply_runs_the_rtk_reconcile_and_surfaces_its_warning(self):
        self.write_settings()
        self.rtk.result = {"rtk_path": None, "warning": "rtk download failed: offline", "installed": False}
        res = self.apply()
        self.assertEqual(len(self.rtk.calls), 1)
        data_dir, lock, fetch, timeout = self.rtk.calls[0]
        self.assertEqual(data_dir, self.data_dir)
        self.assertEqual(lock, json.loads((PLUGIN / "rtk.lock").read_text(encoding="utf-8")))  # the parsed lock
        self.assertIsNone(fetch)
        self.assertGreaterEqual(timeout, 60)
        self.assertEqual(res["rtk"]["status"], "warning")
        self.assertIn("rtk download failed: offline", res["warnings"])
        self.assertTrue(res["ok"])
        self.assertEqual(state.read()["setup_done"], "0.1.0")

    def test_apply_reports_installed_and_up_to_date(self):
        self.write_settings()
        self.assertEqual(self.apply()["rtk"]["status"], "installed")
        self.rtk.result = {"rtk_path": "x", "warning": None, "installed": False}
        self.assertEqual(self.apply()["rtk"]["status"], "up to date")

    def test_missing_lock_is_a_warning_not_an_exception(self):
        root = self.tmp / "p" / "anthology"
        self.assertEqual(anthology_setup._rtk_preview(root, str(self.data_dir)),
                         {"skipped": "rtk wiring not present (no rtk.lock)"})
        status, warning = anthology_setup._rtk_apply(root, str(self.data_dir))
        self.assertEqual(status, "failed")
        self.assertIn("rtk install failed", warning)
        self.assertEqual(self.rtk.calls, [])

    def test_rtk_exception_is_a_warning(self):
        self.write_settings()
        self.rtk.reconcile = mock.Mock(side_effect=RuntimeError("kaboom"))
        res = self.apply()
        self.assertTrue(res["ok"])
        self.assertTrue([w for w in res["warnings"] if "kaboom" in w])

    def test_absent_rtk_wiring_is_skipped(self):
        self.write_settings()
        with mock.patch.dict(sys.modules, {"rtk_install": None}):
            res = self.apply()
        self.assertTrue(res["ok"])
        self.assertIn("skipped", res["rtk"]["status"])

    def test_data_dir_falls_back_to_state_when_env_is_unset(self):
        self.write_settings()
        state.write(data_dir=str(self.data_dir))
        env = dict(os.environ)
        env.pop("CLAUDE_PLUGIN_DATA")
        digest = anthology_setup.preview(PLUGIN, cwd=self.tmp, env=env)["settings"]["sha256"]
        anthology_setup.apply(PLUGIN, digest, cwd=self.tmp, env=env)
        self.assertEqual(self.rtk.calls[0][0], self.data_dir)

    def test_symlinked_settings_are_written_through(self):
        target = self.tmp / "dotfiles" / "settings.json"
        target.parent.mkdir()
        text = json.dumps(SETTINGS, indent=2) + "\n"
        target.write_text(text, encoding="utf-8")
        self.config.mkdir(parents=True, exist_ok=True)
        os.symlink(str(target), str(self.settings))
        self.apply()
        self.assertTrue(self.settings.is_symlink())
        self.assertIn("Bash(rtk *)", target.read_text(encoding="utf-8"))
        self.remove()
        self.assertEqual(target.read_text(encoding="utf-8"), text)

    def test_plugin_moved_replaces_the_stale_plugin_read_rule(self):
        text = self.write_settings()
        old = self.tmp / "old" / "anthology"
        (old / ".claude-plugin").mkdir(parents=True)
        shutil.copy(str(PLUGIN / ".claude-plugin" / "plugin.json"), str(old / ".claude-plugin" / "plugin.json"))
        shutil.copytree(str(PLUGIN.parent / "anthology-guardrails" / "scripts"),
                        str(old.parent / "anthology-guardrails" / "scripts"))
        digest = anthology_setup.preview(old, cwd=self.tmp)["settings"]["sha256"]
        self.assertTrue(anthology_setup.apply(old, digest, cwd=self.tmp)["ok"])
        self.assertIn(settings_patch.plugin_read_rule(old),
                      json.loads(self.settings.read_text(encoding="utf-8"))["permissions"]["allow"])
        self.apply()
        allow = json.loads(self.settings.read_text(encoding="utf-8"))["permissions"]["allow"]
        self.assertNotIn(settings_patch.plugin_read_rule(old), allow)
        self.assertIn(settings_patch.plugin_read_rule(PLUGIN), allow)
        self.remove()
        self.assertEqual(self.settings.read_text(encoding="utf-8"), text)

    def test_guardrails_not_found_stops_with_a_message(self):
        self.write_settings()
        lone = self.tmp / "lone" / "anthology"
        (lone / ".claude-plugin").mkdir(parents=True)
        shutil.copy(str(PLUGIN / ".claude-plugin" / "plugin.json"), str(lone / ".claude-plugin" / "plugin.json"))
        res = anthology_setup.apply(lone, sha(self.settings.read_text(encoding="utf-8")), cwd=self.tmp)
        self.assertFalse(res["ok"])
        self.assertIn("guardrails", res["error"])

    def test_windows_is_refused_without_touching_anything(self):
        text = self.write_settings()
        before = snapshot(self.tmp)
        with mock.patch("platform.system", return_value="Windows"):
            for res in (anthology_setup.apply(PLUGIN, sha(text), cwd=self.tmp), self.remove()):
                self.assertFalse(res["ok"])
                self.assertIn("macOS and Linux", res["error"])
        self.assertEqual(snapshot(self.tmp), before)

    @needs_git
    def test_apply_approves_the_repo_and_remove_keeps_it_unless_asked(self):
        self.write_settings()
        repo = self.make_repo(remote="https://example.invalid/acme/app.git")
        res = self.apply(cwd=repo)
        key = "example.invalid/acme/app"
        self.assertEqual(res["memory"]["approved_key"], key)
        self.assertTrue(memory_sync.is_approved(key))
        self.assertIn(".claude/memory/MEMORY.md merge=union",
                      (repo / ".gitattributes").read_text(encoding="utf-8"))
        self.remove(cwd=repo)
        self.assertTrue(memory_sync.is_approved(key))
        self.apply(cwd=repo)
        self.remove(cwd=repo, unapprove=True)
        self.assertFalse(memory_sync.is_approved(key))

    @needs_git
    def test_skip_approve(self):
        self.write_settings()
        repo = self.make_repo(remote="https://example.invalid/acme/app.git")
        res = self.apply(cwd=repo, skip_approve=True)
        self.assertIsNone(res["memory"]["approved_key"])
        self.assertFalse(memory_sync.is_approved("example.invalid/acme/app"))

    @needs_git
    def test_unsafe_target_is_reported_not_raised_and_setup_carries_on(self):
        self.write_settings()
        repo = self.make_repo(remote="https://example.invalid/acme/app.git")
        (repo / "elsewhere").write_text("x", encoding="utf-8")
        os.symlink(str(repo / "elsewhere"), str(repo / ".gitattributes"))
        res = self.apply(cwd=repo)
        self.assertTrue(res["ok"], res)
        self.assertIn(".gitattributes", res["memory"]["approve_error"])
        self.assertTrue(memory_sync.is_approved("example.invalid/acme/app"))  # approval is already recorded
        self.assertEqual(state.read()["setup_done"], "0.1.0")
        self.assertTrue((self.anth / "personal-memory").is_dir())
        self.assertEqual((repo / "elsewhere").read_text(encoding="utf-8"), "x")

    def test_unsafe_target_with_a_stubbed_approve(self):
        self.write_settings()
        with mock.patch.object(anthology_setup.memory_sync, "repo_root", return_value=self.tmp), \
                mock.patch.object(anthology_setup.memory_sync, "repo_key", return_value="k"), \
                mock.patch.object(anthology_setup.memory_sync, "approve",
                                  side_effect=memory_sync.UnsafeTarget("refusing to use .gitattributes")):
            res = self.apply()
        self.assertTrue(res["ok"])
        self.assertIn("refusing", res["memory"]["approve_error"])
        self.assertEqual(res["memory"]["approved_key"], "k")


class FixRound1Test(SetupCase):
    """Review round 1: plugin-root guard (F21), concurrent edits, remove preview, containment."""

    def unsafe_root(self):
        """A plugin folder directly inside the (temp) home folder: its parent is HOME."""
        root = self.home / "anthology"
        (root / ".claude-plugin").mkdir(parents=True)
        shutil.copy(str(PLUGIN / ".claude-plugin" / "plugin.json"), str(root / ".claude-plugin" / "plugin.json"))
        shutil.copytree(str(PLUGIN.parent / "anthology-guardrails" / "scripts"),
                        str(self.home / "anthology-guardrails" / "scripts"))
        return root

    def test_unsafe_plugin_root_is_refused_in_preview_and_apply_with_nothing_written(self):
        self.write_settings()
        root = self.unsafe_root()
        out = anthology_setup.preview(root, cwd=self.tmp)
        self.assertIn("home folder", out["settings"]["error"])
        self.assertFalse(out["settings"]["changed"])
        res = anthology_setup.apply(root, sha(self.settings.read_text(encoding="utf-8")), cwd=self.tmp)
        self.assertFalse(res["ok"])
        self.assertIn("home folder", res["error"])
        self.assertEqual(self.rtk.calls, [])
        self.assertFalse(state.state_path().exists())
        self.assertNotIn("Read(//", self.settings.read_text(encoding="utf-8"))

    def test_empty_plugin_root_is_refused(self):
        self.write_settings()
        res = anthology_setup.apply("", sha(self.settings.read_text(encoding="utf-8")), cwd=self.tmp)
        self.assertFalse(res["ok"])
        self.assertIn("absolute", res["error"])

    def test_edit_during_the_rtk_download_is_never_overwritten(self):
        self.write_settings()
        digest = self.preview()["settings"]["sha256"]
        edited = json.dumps(dict(SETTINGS, model="sonnet"), indent=2) + "\n"

        def reconcile(data_dir, lock, fetch=None, timeout=30, system=None, machine=None):
            self.settings.write_text(edited, encoding="utf-8")  # the user edits while we "download"
            return {"rtk_path": None, "warning": None, "installed": False}

        self.rtk.reconcile = reconcile
        res = anthology_setup.apply(PLUGIN, digest, cwd=self.tmp)
        self.assertFalse(res["ok"])
        self.assertIn("changed during setup", res["error"])
        self.assertEqual(self.settings.read_text(encoding="utf-8"), edited)
        self.assertFalse(state.read().get("settings_ops"))
        self.assertNotIn("setup_done", state.read())

    def test_edit_right_before_the_write_is_never_overwritten(self):
        self.write_settings()
        digest = self.preview()["settings"]["sha256"]
        edited = json.dumps(dict(SETTINGS, model="sonnet"), indent=2) + "\n"
        real_write = state.write

        def write_then_edit(**kw):
            out = real_write(**kw)
            if "settings_ops" in kw and kw["settings_ops"]:
                self.settings.write_text(edited, encoding="utf-8")
            return out

        with mock.patch.object(anthology_setup.state, "write", side_effect=write_then_edit):
            res = anthology_setup.apply(PLUGIN, digest, cwd=self.tmp)
        self.assertFalse(res["ok"])
        self.assertEqual(self.settings.read_text(encoding="utf-8"), edited)
        self.assertEqual(state.read().get("settings_ops"), [])

    def test_ops_are_recorded_before_settings_json_is_written(self):
        text = self.write_settings()
        digest = self.preview()["settings"]["sha256"]
        with mock.patch.object(anthology_setup.settings_patch, "write_text", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                anthology_setup.apply(PLUGIN, digest, cwd=self.tmp)
        self.assertTrue(state.read()["settings_ops"])
        self.assertEqual(self.settings.read_text(encoding="utf-8"), text)

    def test_diff_has_no_neighbouring_secret_lines(self):
        self.write_settings(dict(SETTINGS, env={"SECRET_TOKEN": "hunter2"}))
        out = self.preview()
        self.assertNotIn("hunter2", json.dumps(out))
        res = self.apply()
        self.assertNotIn("hunter2", json.dumps(res))

    def test_env_and_secret_values_never_reach_any_diff_in_any_layout(self):
        data = dict(SETTINGS, apiKeyHelper="/bin/echo sk-live-123", env={"SECRET_TOKEN": "hunter2"})
        layouts = {
            "minified": json.dumps(data, separators=(",", ":")),
            "one-line env last": ('{\n  "model": "opus",\n  "permissions": {"allow": ["Bash(ls)"]},\n'
                                  '  "env": {"SECRET_TOKEN": "hunter2", "OTHER": "sk-live-123"}\n}\n'),
            "pretty": json.dumps(data, indent=2) + "\n",
        }
        for name, text in layouts.items():
            with self.subTest(name):
                write(self.settings, text)
                for path in (self.data_dir, state.state_path()):
                    if path.is_dir():
                        shutil.rmtree(str(path))
                    elif path.exists():
                        path.unlink()
                outs = [self.preview(), self.apply(),
                        anthology_setup.remove(PLUGIN, cwd=self.tmp, preview=True), self.remove()]
                for out in outs:
                    self.assertTrue(out["settings"]["diff"], name)
                    self.assertIn("Bash(rtk *)", out["settings"]["diff"])
                    dumped = json.dumps(out)
                    self.assertNotIn("hunter2", dumped)
                    self.assertNotIn("sk-live-123", dumped)
                if name != "pretty":  # the env line itself is in the diff, with its key kept
                    self.assertRegex(outs[0]["settings"]["diff"], r'"SECRET_TOKEN": ?"<redacted>"')
                self.assertEqual(self.settings.read_text(encoding="utf-8"), text)

    def test_remove_preview_writes_nothing_and_shows_the_diff(self):
        text = self.write_settings()
        self.apply()
        write(self.data_dir / "rtk" / "1.0" / "rtk", "bin")
        applied = self.settings.read_text(encoding="utf-8")
        before = snapshot(self.tmp)
        out = anthology_setup.remove(PLUGIN, cwd=self.tmp, preview=True)
        self.assertEqual(snapshot(self.tmp), before)
        self.assertTrue(out["ok"])
        self.assertTrue(out["preview"])
        self.assertTrue(out["settings"]["changed"])
        self.assertIn('-      "Bash(rtk *)"', out["settings"]["diff"])
        self.assertIn('"baseRef"', out["settings"]["diff"])
        self.assertEqual(out["rtk"]["dir"], str(self.data_dir / "rtk"))
        self.assertEqual(out["state"]["path"], str(state.state_path()))
        self.assertTrue(out["state"]["exists"])
        self.assertEqual(self.settings.read_text(encoding="utf-8"), applied)
        self.assertNotEqual(applied, text)

    def test_remove_returns_the_same_diff(self):
        self.write_settings()
        self.apply()
        preview = anthology_setup.remove(PLUGIN, cwd=self.tmp, preview=True)
        res = self.remove()
        self.assertEqual(res["settings"]["diff"], preview["settings"]["diff"])

    @needs_git
    def test_remove_preview_lists_the_approvals_each_flag_would_drop(self):
        self.write_settings()
        repo = self.make_repo(remote="https://example.invalid/acme/app.git")
        memory_sync._save_approved({"version": 1, "repos": {"a/b": {}}})
        self.apply(cwd=repo)
        out = anthology_setup.remove(PLUGIN, cwd=repo, preview=True)
        self.assertEqual(out["memory"]["unapprove"], ["example.invalid/acme/app"])
        self.assertEqual(out["memory"]["unapprove_all"], ["a/b", "example.invalid/acme/app"])
        self.assertTrue(memory_sync.is_approved("a/b"))

    def test_remove_preview_error_for_unparseable_settings(self):
        self.write_settings()
        self.apply()
        write(self.settings, "{nope")
        out = anthology_setup.remove(PLUGIN, cwd=self.tmp, preview=True)
        self.assertFalse(out["ok"])
        self.assertIn("not valid JSON", out["error"])

    def test_remove_preview_cli_flag(self):
        self.write_settings()
        self.apply()
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = anthology_setup.main(["remove", "--preview", "--plugin-root", str(PLUGIN), "--cwd", str(self.tmp)])
        self.assertEqual(code, 0)
        self.assertTrue(json.loads(buf.getvalue())["preview"])
        self.assertTrue(state.state_path().exists())

    def test_rtk_is_never_deleted_when_data_dir_is_the_config_folder_or_above_plugins_data(self):
        self.write_settings()
        self.apply()
        for data in (self.config, self.config / "plugins", self.config / "plugins" / "data", self.home):
            write(data / "rtk" / "precious.txt", "keep")
            state.write(data_dir=str(data))
            env = dict(os.environ)
            env.pop("CLAUDE_PLUGIN_DATA")
            res = anthology_setup.remove(PLUGIN, cwd=self.tmp, env=env)
            self.assertTrue((data / "rtk" / "precious.txt").exists(), data)
            self.assertTrue([w for w in res["warnings"] if "rtk" in w], data)
            self.apply()

    def test_rtk_dir_that_resolves_elsewhere_is_left_alone(self):
        self.write_settings()
        self.apply()
        elsewhere = self.config / "elsewhere" / "rtk"
        write(elsewhere / "precious.txt", "keep")
        self.data_dir.mkdir(parents=True, exist_ok=True)
        os.symlink(str(elsewhere.parent), str(self.data_dir / "sub"))
        state.write(data_dir=str(self.data_dir / "sub"))
        env = dict(os.environ)
        env.pop("CLAUDE_PLUGIN_DATA")
        anthology_setup.remove(PLUGIN, cwd=self.tmp, env=env)
        self.assertTrue((elsewhere / "precious.txt").exists())


class RemoveTest(SetupCase):
    def test_remove_never_deletes_memory_notes_or_the_personal_folder(self):
        self.write_settings()
        personal = self.anth / "personal-memory"
        write(personal / "user-me.md", note_text("user-me", "user", "Me"))
        repo_note = None
        self.apply()
        self.remove(unapprove_all=True)
        self.assertTrue((personal / "user-me.md").exists())
        self.assertTrue(personal.is_dir())
        self.assertFalse(state.state_path().exists())

    @needs_git
    def test_remove_keeps_repo_notes_and_gitattributes(self):
        self.write_settings()
        repo = self.make_repo(remote="https://example.invalid/acme/app.git")
        write(repo / ".claude" / "memory" / "project-a.md", note_text("project-a", "project", "A"))
        write(repo / ".claude" / "agent-memory" / "anthology-scout" / "project-b.md",
              note_text("project-b", "project", "B"))
        self.apply(cwd=repo)
        self.remove(cwd=repo, unapprove=True, unapprove_all=True)
        self.assertTrue((repo / ".claude" / "memory" / "project-a.md").exists())
        self.assertTrue((repo / ".claude" / "agent-memory" / "anthology-scout" / "project-b.md").exists())
        self.assertTrue((repo / ".gitattributes").exists())
        self.assertFalse(memory_sync.is_approved("example.invalid/acme/app"))

    def test_unapprove_all_drops_every_approval(self):
        self.write_settings()
        memory_sync._save_approved({"version": 1, "repos": {"a/b": {}, "c/d": {}}})
        self.apply()
        self.remove(unapprove_all=True)
        self.assertFalse(memory_sync.is_approved("a/b"))
        self.assertFalse(memory_sync.is_approved("c/d"))

    def test_remove_deletes_only_the_rtk_folder_under_the_data_dir(self):
        self.write_settings()
        self.apply()
        write(self.data_dir / "rtk" / "1.0" / "rtk", "bin")
        write(self.data_dir / "other" / "keep.txt", "keep")
        res = self.remove()
        self.assertTrue(res["ok"], res)
        self.assertFalse((self.data_dir / "rtk").exists())
        self.assertTrue((self.data_dir / "other" / "keep.txt").exists())

    def test_remove_refuses_a_data_dir_outside_the_config_folder(self):
        self.write_settings()
        self.apply()
        outside = self.tmp / "outside"
        write(outside / "rtk" / "precious.txt", "keep")
        state.write(data_dir=str(outside))
        env = dict(os.environ)
        env.pop("CLAUDE_PLUGIN_DATA")
        res = anthology_setup.remove(PLUGIN, cwd=self.tmp, env=env)
        self.assertTrue((outside / "rtk" / "precious.txt").exists())
        self.assertTrue([w for w in res["warnings"] if "rtk" in w])

    def test_remove_does_not_follow_a_symlinked_rtk_folder(self):
        self.write_settings()
        self.apply()
        elsewhere = self.config / "elsewhere"
        write(elsewhere / "precious.txt", "keep")
        self.data_dir.mkdir(parents=True, exist_ok=True)
        os.symlink(str(elsewhere), str(self.data_dir / "rtk"))
        self.remove()
        self.assertTrue((elsewhere / "precious.txt").exists())

    def test_remove_with_unparseable_settings_keeps_state_for_a_retry(self):
        self.write_settings()
        self.apply()
        write(self.settings, "{nope")
        res = self.remove()
        self.assertFalse(res["ok"])
        self.assertIn("not valid JSON", res["error"])
        self.assertTrue(state.state_path().exists())
        self.assertEqual(self.settings.read_text(encoding="utf-8"), "{nope")

    def test_remove_without_a_prior_setup_is_harmless(self):
        text = self.write_settings()
        res = self.remove()
        self.assertTrue(res["ok"])
        self.assertEqual(self.settings.read_text(encoding="utf-8"), text)

    def test_users_own_rules_survive_remove(self):
        self.write_settings(dict(SETTINGS, permissions={"allow": ["Bash(rtk *)", "Bash(ls)"]}))
        self.apply()
        data = json.loads(self.settings.read_text(encoding="utf-8"))
        data["permissions"]["allow"].append("Bash(make *)")
        write(self.settings, json.dumps(data, indent=2) + "\n")
        self.remove()
        got = json.loads(self.settings.read_text(encoding="utf-8"))
        self.assertEqual(got["permissions"]["allow"], ["Bash(rtk *)", "Bash(ls)", "Bash(make *)"])
        self.assertNotIn("worktree", got)


class CliTest(SetupCase):
    def run_main(self, *argv):
        buf = io.StringIO()
        with redirect_stdout(buf):
            code = anthology_setup.main(list(argv))
        return code, json.loads(buf.getvalue())

    def test_preview_json(self):
        self.write_settings()
        code, out = self.run_main("preview", "--plugin-root", str(PLUGIN), "--cwd", str(self.tmp), "--json")
        self.assertEqual(code, 0)
        self.assertEqual(out["settings"]["sha256"], sha(self.settings.read_text(encoding="utf-8")))

    def test_apply_and_remove_flags(self):
        text = self.write_settings()
        digest = sha(text)
        code, out = self.run_main("apply", "--plugin-root", str(PLUGIN), "--expect-sha256", digest,
                                  "--cwd", str(self.tmp), "--skip-approve")
        self.assertEqual(code, 0, out)
        code, out = self.run_main("remove", "--plugin-root", str(PLUGIN), "--cwd", str(self.tmp),
                                  "--unapprove", "--unapprove-all")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.settings.read_text(encoding="utf-8"), text)

    def test_stale_digest_exits_1(self):
        self.write_settings()
        code, out = self.run_main("apply", "--plugin-root", str(PLUGIN), "--expect-sha256", "0" * 64,
                                  "--cwd", str(self.tmp))
        self.assertEqual(code, 1)
        self.assertFalse(out["ok"])

    def test_windows_exits_2_with_the_refusal(self):
        with mock.patch("platform.system", return_value="Windows"):
            for argv in (["preview"], ["apply", "--expect-sha256", "0" * 64], ["remove"]):
                code, out = self.run_main(*(argv + ["--plugin-root", str(PLUGIN)]))
                self.assertEqual(code, 2, argv)
                self.assertIn("macOS and Linux", json.dumps(out))

    def test_subprocess_preview_runs_as_a_script(self):
        env = dict(os.environ)
        proc = subprocess.run([sys.executable, str(SCRIPTS / "anthology_setup.py"), "preview",
                               "--plugin-root", str(PLUGIN), "--cwd", str(self.tmp), "--json"],
                              env=env, capture_output=True, text=True, timeout=120)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("settings", json.loads(proc.stdout))


if __name__ == "__main__":
    unittest.main()
