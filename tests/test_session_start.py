"""Tests for scripts/session_start.py, session-start.sh, the routing guide, hooks.json and the
skills folder (Task 13).

Cases that need git skip when it is missing (the Docker test images have none).
"""
from __future__ import annotations

import io
import json
import os
import re
import shutil
import subprocess
import sys
import types
import unittest
from pathlib import Path
from unittest import mock

from _mem import ROOT, SCRIPTS, MemCase, needs_git, note_text, write

import _rtk  # noqa: E402
import memory_sync  # noqa: E402
import session_start  # noqa: E402
import settings_patch  # noqa: E402
import state  # noqa: E402

PLUGIN = ROOT / "plugins" / "anthology"
GUIDE = PLUGIN / "templates" / "routing-guide.md"
AGENTS = ("researcher", "architect", "implementer", "debugger", "reviewer", "scout", "docs-writer",
          "ui-designer", "ui-reviewer", "product-planner", "marketer")
NAG = "anthology: run /anthology:setup to finish installing (rtk, permissions, memory)"
MOVED = "anthology: the plugin moved; re-run /anthology:setup so agents can read their knowledge"


BLOCKS = ('import sys\nsys.stdin.read()\nsys.stderr.write("Blocked by anthology-guardrails (G1): x\\n")\n'
          'sys.exit(2)\n')


class FakeRtk:
    """Stands in for rtk_install with its real interface (load_lock, reconcile returning the
    {"rtk_path", "warning", "installed"} dict); records the calls. tests/test_session_start.py
    RealRtkTest runs the real module, so this fake cannot drift from it unnoticed."""

    LOCK = {"version": "9.9.9", "sha256": {}}

    def __init__(self, result=None, exc=None, lock_exc=None):
        self.calls = []
        self.lock_paths = []
        self.result = result or {"rtk_path": None, "warning": None, "installed": False}
        self.exc, self.lock_exc = exc, lock_exc

    def load_lock(self, path):
        self.lock_paths.append(path)
        if self.lock_exc:
            raise self.lock_exc
        return self.LOCK

    def reconcile(self, data_dir, lock, fetch=None, timeout=30, system=None, machine=None):
        self.calls.append((data_dir, lock, fetch, timeout))
        if self.exc:
            raise self.exc
        return self.result


class SessionCase(MemCase):
    def setUp(self):
        super().setUp()
        self.plugin_root = PLUGIN
        os.environ["CLAUDE_PLUGIN_ROOT"] = str(PLUGIN)
        os.environ["CLAUDE_PLUGIN_DATA"] = str(self.data)
        self.rtk = FakeRtk()
        patcher = mock.patch.dict(sys.modules, {"rtk_install": self.rtk})
        patcher.start()
        self.addCleanup(patcher.stop)

    def mark_setup(self, plugin_root=None):
        """Pretend /anthology:setup ran: state.setup_done plus the U1 allow rule in settings.json."""
        state.write(setup_done="0.1.0")
        rule = settings_patch.plugin_read_rule(plugin_root or PLUGIN)
        write(self.config / "settings.json", json.dumps({"permissions": {"allow": [rule]}}))

    def go(self, cwd=None, env_extra=None, hook_input=None):
        env = dict(os.environ)
        env.update(env_extra or {})
        data = {"hook_event_name": "SessionStart", "source": "startup", "cwd": str(cwd or self.tmp)}
        data.update(hook_input or {})
        return json.loads(session_start.run(env, json.dumps(data)))

    @staticmethod
    def context(out):
        return out["hookSpecificOutput"]["additionalContext"]

    def fake_plugins(self, guard_body, rules=True):
        """A plugins/ folder with a copy of anthology's guide and a fake anthology-guardrails."""
        base = self.tmp / "fake-plugins"
        root = base / "anthology"
        (root / "templates").mkdir(parents=True)
        shutil.copy(str(GUIDE), str(root / "templates" / "routing-guide.md"))
        guard = base / "anthology-guardrails" / "scripts"
        guard.mkdir(parents=True)
        (guard / "guard.py").write_text(guard_body, encoding="utf-8")
        if rules:
            (guard / "guard_rules.py").write_text(
                'SELF_TEST_SAMPLE = {"tool_name": "Bash", "tool_input": {"command": "x"}}\n'
                'PROFILE_ROOTS = (".config/chromium",)\n', encoding="utf-8")
        return root


class OutputTest(SessionCase):
    def test_output_shape_and_guide(self):
        self.mark_setup()
        out = self.go()
        hook = out["hookSpecificOutput"]
        self.assertEqual(hook["hookEventName"], "SessionStart")
        guide = GUIDE.read_text(encoding="utf-8")
        self.assertTrue(self.context(out).startswith(guide.rstrip("\n")))
        self.assertNotIn("systemMessage", out)

    def test_every_session_source_gets_the_guide(self):
        self.mark_setup()
        for source in ("startup", "resume", "clear", "compact"):
            out = self.go(hook_input={"source": source})
            self.assertIn("# Anthology routing guide", self.context(out), source)

    def test_setup_not_run_shows_the_one_line_nag(self):
        out = self.go()
        self.assertIn(NAG, out["systemMessage"].splitlines())
        self.assertIn("# Anthology routing guide", self.context(out))

    def test_stale_u1_rule_warns(self):
        state.write(setup_done="0.1.0")
        write(self.config / "settings.json", json.dumps(
            {"permissions": {"allow": ["Read(//old/place/plugins/**)"]}}))
        out = self.go()
        self.assertIn(MOVED, out["systemMessage"].splitlines())

    def test_missing_settings_file_after_setup_warns(self):
        state.write(setup_done="0.1.0")
        self.assertIn(MOVED, self.go()["systemMessage"].splitlines())

    def test_unreadable_settings_after_setup_has_its_own_wording(self):
        state.write(setup_done="0.1.0")
        write(self.config / "settings.json", "{nope")
        msg = self.go()["systemMessage"]
        self.assertNotIn(MOVED, msg.splitlines())
        self.assertIn("could not read settings.json", msg)

    def test_settings_with_a_bom_and_the_rule_are_quiet(self):
        state.write(setup_done="0.1.0")
        rule = settings_patch.plugin_read_rule(PLUGIN)
        write(self.config / "settings.json", "\ufeff" + json.dumps({"permissions": {"allow": [rule]}}))
        self.assertNotIn("systemMessage", self.go())

    def test_allow_that_is_a_string_is_not_a_match(self):
        state.write(setup_done="0.1.0")
        rule = settings_patch.plugin_read_rule(PLUGIN)
        write(self.config / "settings.json", json.dumps({"permissions": {"allow": "x" + rule + "y"}}))
        self.assertIn(MOVED, self.go()["systemMessage"].splitlines())

    def test_current_u1_rule_is_quiet(self):
        self.mark_setup()
        self.assertNotIn("systemMessage", self.go())

    def test_total_context_stays_within_the_budget_with_a_300_note_personal_index(self):
        self.mark_setup()
        personal = self.anth / "personal-memory"
        for i in range(300):
            write(personal / ("project-topic-%03d.md" % i),
                  note_text("project-topic-%03d" % i, "project", "A fairly descriptive note number %d" % i))
        out = self.go()
        ctx = self.context(out)
        self.assertLessEqual(len(ctx), 9000)
        self.assertIn("# Anthology routing guide", ctx)
        self.assertIn("## Personal memory", ctx)
        self.assertRegex(ctx, r"\d+ more .*anthology-mem search")
        self.assertIn(str(personal / "MEMORY.md"), ctx)

    def test_oversized_memory_text_is_truncated_at_a_line_with_a_pointer(self):
        self.mark_setup()
        lines = "".join("- note line %04d with some padding text here\n" % i for i in range(400))
        with mock.patch.object(session_start.memory_sync, "run",
                               return_value=("## Personal memory (x)\n\n" + lines, [])):
            out = self.go()
        ctx = self.context(out)
        guide = GUIDE.read_text(encoding="utf-8").rstrip("\n")
        self.assertLessEqual(len(ctx), 9000)
        self.assertTrue(ctx.startswith(guide))
        self.assertIn("anthology-mem search", ctx.splitlines()[-1])
        self.assertIn("MEMORY.md", ctx.splitlines()[-1])
        body = ctx.splitlines()[:-1]
        self.assertTrue(body[-1].startswith("- note line "))
        self.assertTrue(body[-1].endswith("padding text here"))  # cut on a line boundary

    def test_the_guide_is_never_truncated_even_if_it_alone_is_over_budget(self):
        self.mark_setup()
        root = self.fake_plugins(BLOCKS)
        (root / "templates" / "routing-guide.md").write_text("# Anthology routing guide\n" + "x" * 9500,
                                                           encoding="utf-8")
        out = self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})
        self.assertGreaterEqual(len(self.context(out)), 9500)

    def test_memory_messages_reach_the_system_message(self):
        self.mark_setup()
        with mock.patch.object(session_start.memory_sync, "run", return_value=("", ["memory said hi"])):
            out = self.go()
        self.assertIn("memory said hi", out["systemMessage"].splitlines())

    def test_system_message_is_capped(self):
        self.mark_setup()
        with mock.patch.object(session_start.memory_sync, "run",
                               return_value=("", ["m%03d %s" % (i, "y" * 80) for i in range(100)])):
            out = self.go()
        self.assertLessEqual(len(out["systemMessage"]), 2000)

    def test_no_claude_md_is_written_and_no_guide_text_lands_on_disk(self):
        self.mark_setup()
        repo = self.tmp / "work"
        repo.mkdir()
        self.go(cwd=repo)
        names = [p.name for p in self.tmp.rglob("*") if p.is_file()]
        self.assertNotIn("CLAUDE.md", names)
        for p in self.tmp.rglob("*"):
            if p.is_file() and ".git" not in p.parts:
                self.assertNotIn("Anthology routing guide", p.read_text(encoding="utf-8", errors="replace"), p)

    def test_session_start_only_writes_state(self):
        self.mark_setup()
        def files():  # Apple's python writes .pyc files under HOME; they are not ours
            return {p for p in self.tmp.rglob("*") if p.is_file() and p.suffix != ".pyc"}

        before = files()
        self.go()
        after = files()
        self.assertEqual({p.name for p in after - before} - {"state.json"}, set())

    def test_missing_guide_file_is_a_warning_not_a_crash(self):
        self.mark_setup()
        root = self.fake_plugins(BLOCKS)
        (root / "templates" / "routing-guide.md").unlink()
        out = self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})
        self.assertIn("routing guide", out["systemMessage"])

    def test_stdin_that_is_not_json_still_works(self):
        self.mark_setup()
        out = json.loads(session_start.run(dict(os.environ), "not json"))
        self.assertIn("# Anthology routing guide", self.context(out))

    @needs_git
    def test_repo_cover_only_when_memory_sync_repaired(self):
        self.mark_setup()
        repo = self.make_repo(remote="https://example.invalid/acme/app.git")
        write(repo / ".claude" / "memory" / "project-a.md", note_text("project-a", "project", "First"))
        memory_sync.approve(repo)
        first = self.context(self.go(cwd=repo))
        self.assertIn("## Repo memory", first)
        self.assertIn("project-a", first)
        second = self.context(self.go(cwd=repo))
        self.assertNotIn("## Repo memory", second)

    @needs_git
    def test_unapproved_repo_with_memory_gets_the_hint(self):
        self.mark_setup()
        repo = self.make_repo(remote="https://example.invalid/acme/other.git")
        write(repo / ".claude" / "memory" / "project-a.md", note_text("project-a", "project", "First"))
        out = self.go(cwd=repo)
        self.assertIn(memory_sync.HINT, out["systemMessage"].splitlines())


class FailureTest(SessionCase):
    def test_injected_exception_gives_a_short_system_message(self):
        with mock.patch.object(session_start, "_plugin_root", side_effect=RuntimeError("boom")):
            out = json.loads(session_start.run(dict(os.environ), "{}"))
        self.assertIn("anthology", out["systemMessage"])
        self.assertLessEqual(len(out["systemMessage"]), 300)
        self.assertNotIn("did not run", out["systemMessage"])

    def test_every_step_is_isolated_and_the_guide_is_always_delivered(self):
        self.mark_setup()
        for name, exc in (("_guard_selftest", RuntimeError("boom")), ("_setup_warning", RecursionError()),
                          ("_reconcile_rtk", OSError("x")), ("_read_guide", UnicodeDecodeError("utf-8", b"x", 0, 1, "bad")),
                          ("_parse_stdin", ValueError("x"))):
            with self.subTest(name), mock.patch.object(session_start, name, side_effect=exc):
                out = self.go()
                if name != "_read_guide":
                    self.assertIn("# Anthology routing guide", self.context(out))
                self.assertIn("anthology:", out["systemMessage"])
                # the steps that did not fail are not blamed
                self.assertNotIn("memory sync failed", out["systemMessage"])

    def test_a_failing_guard_does_not_claim_the_other_steps_failed(self):
        self.mark_setup()
        with mock.patch.object(session_start, "_guard_selftest", side_effect=RuntimeError("boom")):
            out = self.go()
        lines = out["systemMessage"].splitlines()
        self.assertEqual(lines, ["anthology: the guardrails self-test failed (RuntimeError)"])

    def test_pathologically_nested_stdin_and_settings_still_deliver_the_guide(self):
        self.mark_setup()
        write(self.config / "settings.json", "[" * 200000)
        out = json.loads(session_start.run(dict(os.environ), "[" * 200000))
        self.assertIn("# Anthology routing guide", self.context(out))

    def test_non_utf8_guide_is_a_warning_not_a_crash(self):
        self.mark_setup()
        root = self.fake_plugins(BLOCKS)
        (root / "templates" / "routing-guide.md").write_bytes(b"\xff\xfe\x00bad")
        self.mark_setup(root)
        out = self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})
        self.assertIn("routing guide", out["systemMessage"])

    def test_main_exits_zero_even_when_run_raises(self):
        buf = io.StringIO()
        with mock.patch.object(session_start, "run", side_effect=RuntimeError("boom")), \
                mock.patch.object(sys, "stdin", io.StringIO("{}")), mock.patch.object(sys, "stdout", buf):
            code = session_start.main()
        self.assertEqual(code, 0)
        out = json.loads(buf.getvalue())
        self.assertIn("systemMessage", out)
        self.assertLessEqual(len(out["systemMessage"]), 300)

    def test_main_prints_one_json_object_on_success(self):
        self.mark_setup()
        buf = io.StringIO()
        stdin = io.StringIO(json.dumps({"cwd": str(self.tmp)}))
        with mock.patch.object(sys, "stdin", stdin), mock.patch.object(sys, "stdout", buf):
            self.assertEqual(session_start.main(), 0)
        self.assertIn("hookSpecificOutput", json.loads(buf.getvalue()))

    def test_memory_failure_never_fails_the_session(self):
        self.mark_setup()
        with mock.patch.object(session_start.memory_sync, "run", side_effect=OSError("disk")):
            out = self.go()
        self.assertIn("# Anthology routing guide", self.context(out))
        self.assertIn("memory", out["systemMessage"])


class PlatformTest(SessionCase):
    def test_windows_gets_the_refusal_and_nothing_else(self):
        with mock.patch("platform.system", return_value="Windows"), \
                mock.patch.object(session_start.memory_sync, "run") as mem, \
                mock.patch.object(session_start, "_guard_selftest") as guard:
            out = json.loads(session_start.run(dict(os.environ), "{}"))
        self.assertEqual(out, {"systemMessage": "Anthology supports macOS and Linux only; it is inactive on Windows."})
        mem.assert_not_called()
        guard.assert_not_called()
        self.assertEqual(self.rtk.calls, [])
        self.assertFalse(state.state_path().exists())


class StateTest(SessionCase):
    def test_data_dir_is_recorded_only_when_it_changes(self):
        self.mark_setup()
        self.go()
        self.assertEqual(state.read()["data_dir"], str(self.data))
        with mock.patch.object(state, "write", wraps=state.write) as w:
            self.go()
            self.assertFalse([c for c in w.call_args_list if "data_dir" in c.kwargs])
            self.go(env_extra={"CLAUDE_PLUGIN_DATA": str(self.tmp / "data2")})
            self.assertTrue([c for c in w.call_args_list if c.kwargs.get("data_dir") == str(self.tmp / "data2")])

    def test_no_data_dir_env_means_no_state_write(self):
        self.mark_setup()
        env = dict(os.environ)
        env.pop("CLAUDE_PLUGIN_DATA")
        before = dict(state.read())
        session_start.run(env, "{}")
        self.assertEqual(state.read(), before)


class RtkTest(SessionCase):
    def test_not_reconciled_before_setup_ran(self):
        self.go()
        self.assertEqual(self.rtk.calls, [])

    def test_reconcile_runs_after_setup_with_the_data_dir_and_lock(self):
        self.mark_setup()
        self.go()
        self.assertEqual(len(self.rtk.calls), 1)
        data_dir, lock, fetch, timeout = self.rtk.calls[0]
        self.assertEqual(data_dir, self.data)
        self.assertEqual(self.rtk.lock_paths, [PLUGIN / "rtk.lock"])
        self.assertEqual(lock, FakeRtk.LOCK)  # the parsed lock, not a path
        self.assertIsNone(fetch)
        self.assertEqual(timeout, session_start.RTK_TIMEOUT)

    def test_worst_case_reconcile_fits_the_hook_timeout(self):
        # rtk_install downloads for at most 4 x timeout plus one socket timeout; it only starts
        # when RTK_MIN_REMAINING is left of the DEADLINE, which is itself under the 60 s hook timeout.
        self.assertLessEqual(5 * session_start.RTK_TIMEOUT, session_start.RTK_MIN_REMAINING)
        self.assertLess(session_start.DEADLINE, 60)

    def test_data_dir_falls_back_to_state(self):
        self.mark_setup()
        state.write(data_dir=str(self.tmp / "remembered"))
        env = dict(os.environ)
        env.pop("CLAUDE_PLUGIN_DATA")
        session_start.run(env, "{}")
        self.assertEqual(self.rtk.calls[0][0], self.tmp / "remembered")

    def test_returned_warning_is_shown(self):
        self.mark_setup()
        self.rtk.result = {"rtk_path": None, "warning": "rtk download failed: offline", "installed": False}
        out = self.go()
        self.assertIn("rtk download failed: offline", out["systemMessage"].splitlines())

    def test_a_clean_result_adds_no_warning(self):
        self.mark_setup()
        self.rtk.result = {"rtk_path": str(self.data / "rtk" / "x" / "rtk"), "warning": None, "installed": True}
        self.assertNotIn("systemMessage", self.go())

    def test_a_missing_lock_is_the_not_wired_note(self):
        self.mark_setup()
        self.rtk.lock_exc = RuntimeError("cannot read")
        with mock.patch.object(Path, "exists", return_value=False):
            warning = session_start._reconcile_rtk(PLUGIN, self.data)
        self.assertEqual(warning, "anthology: rtk wiring not present yet; skipped")
        self.assertEqual(self.rtk.calls, [])

    def test_an_invalid_lock_is_a_warning_and_nothing_is_reconciled(self):
        self.mark_setup()
        self.rtk.lock_exc = RuntimeError("rtk.lock is not valid JSON")
        out = self.go()
        self.assertIn("rtk.lock is not valid", out["systemMessage"])
        self.assertEqual(self.rtk.calls, [])
        self.assertIn("# Anthology routing guide", self.context(out))

    def test_exception_becomes_a_warning(self):
        self.mark_setup()
        self.rtk.exc = RuntimeError("kaboom")
        out = self.go()
        self.assertIn("rtk", out["systemMessage"])
        self.assertIn("kaboom", out["systemMessage"])
        self.assertIn("# Anthology routing guide", self.context(out))

    def test_absent_wiring_is_one_short_note(self):
        self.mark_setup()
        with mock.patch.dict(sys.modules, {"rtk_install": None}):
            out = self.go()
        self.assertIn("anthology: rtk wiring not present yet; skipped", out["systemMessage"].splitlines())


class RealRtkTest(MemCase):
    """The real rtk_install behind session start's adapter, with a fake download."""

    def test_installs_records_the_path_and_is_quiet_the_second_time(self):
        import rtk_install
        blob = _rtk.tarball()
        root = self.tmp / "plugin"
        _rtk.write_lock(root / "rtk.lock", blob)
        fetch = _rtk.FakeFetch(blob)
        want = str(self.data / "rtk" / _rtk.VERSION / "rtk")
        with mock.patch.object(rtk_install, "default_fetch", fetch), \
                mock.patch("platform.system", return_value=_rtk.LINUX[0]), \
                mock.patch("platform.machine", return_value=_rtk.LINUX[1]):
            self.assertIsNone(session_start._reconcile_rtk(root, self.data))
            self.assertEqual(state.read()["rtk_path"], want)
            self.assertTrue(os.access(want, os.X_OK))
            self.assertIsNone(session_start._reconcile_rtk(root, self.data))
        self.assertEqual(len(fetch.calls), 1)
        self.assertEqual(fetch.calls[0][2], session_start.RTK_TIMEOUT)

    def test_a_failed_download_is_a_warning_and_state_is_untouched(self):
        import rtk_install
        blob = _rtk.tarball()
        root = self.tmp / "plugin"
        _rtk.write_lock(root / "rtk.lock", blob)
        with mock.patch.object(rtk_install, "default_fetch", _rtk.FakeFetch(b"not the pinned tarball")), \
                mock.patch("platform.system", return_value=_rtk.LINUX[0]), \
                mock.patch("platform.machine", return_value=_rtk.LINUX[1]):
            warning = session_start._reconcile_rtk(root, self.data)
        self.assertIn("rtk is not installed or up to date", warning)
        self.assertNotIn("rtk_path", state.read())

    def test_a_missing_lock_is_the_not_wired_note(self):
        root = self.tmp / "no-lock"
        root.mkdir()
        self.assertEqual(session_start._reconcile_rtk(root, self.data),
                         "anthology: rtk wiring not present yet; skipped")


class GuardSelfTestTest(SessionCase):
    def test_real_guard_blocks_the_sample_quietly(self):
        self.mark_setup()
        self.assertNotIn("systemMessage", self.go())

    def test_sample_is_piped_as_json_and_blocked_with_exit_2(self):
        import guardrails_loc
        rules = guardrails_loc.load_rules(PLUGIN, {})
        guard = PLUGIN.parent / "anthology-guardrails" / "scripts" / "guard.py"
        proc = subprocess.run([sys.executable, str(guard)], input=json.dumps(rules.SELF_TEST_SAMPLE).encode(),
                              capture_output=True, timeout=30)
        self.assertEqual(proc.returncode, 2)

    def test_missing_python3_on_the_hook_path_warns_loudly(self):
        self.mark_setup()
        out = self.go(env_extra={"PATH": "/nonexistent"})
        msg = out["systemMessage"]
        self.assertIn("python3", msg)
        self.assertRegex(msg.lower(), r"guardrails")
        self.assertIn("# Anthology routing guide", self.context(out))

    def test_guard_that_does_not_block_warns(self):
        self.mark_setup()
        root = self.fake_plugins("import sys\nsys.exit(0)\n")
        self.mark_setup(root)
        out = self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})
        self.assertRegex(out["systemMessage"].lower(), r"did not block")

    def test_guard_that_crashes_warns(self):
        root = self.fake_plugins("raise SystemExit(1)\n")
        self.mark_setup(root)
        out = self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})
        self.assertRegex(out["systemMessage"].lower(), r"did not block")

    def test_guard_that_blocks_is_quiet(self):
        root = self.fake_plugins(BLOCKS)
        self.mark_setup(root)
        self.assertNotIn("systemMessage", self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)}))

    def test_slow_guard_times_out_into_a_warning(self):
        root = self.fake_plugins("import time\ntime.sleep(30)\n")
        self.mark_setup(root)
        with mock.patch.object(session_start, "GUARD_TIMEOUT", 1):
            out = self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})
        self.assertRegex(out["systemMessage"].lower(), r"timed out|did not finish")

    def test_guardrails_not_found_warns(self):
        root = self.fake_plugins(BLOCKS)
        shutil.rmtree(str(root.parent / "anthology-guardrails"))
        self.mark_setup(root)
        out = self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})
        self.assertRegex(out["systemMessage"].lower(), r"guardrails.*(not found|missing)")

    def test_guardrails_without_the_sample_warns(self):
        root = self.fake_plugins(BLOCKS, rules=False)
        self.mark_setup(root)
        out = self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})
        self.assertRegex(out["systemMessage"].lower(), r"guardrails")

    def test_guardrails_found_through_installed_plugins_json(self):
        base = self.tmp / "cache-style"
        root = base / "anthology" / "0.1.0"
        (root / "templates").mkdir(parents=True)
        shutil.copy(str(GUIDE), str(root / "templates" / "routing-guide.md"))
        guard = self.tmp / "installs" / "guard-new"
        guard_old = self.tmp / "installs" / "guard-old"
        for d, body in ((guard, BLOCKS),
                        (guard_old, "import sys\nsys.exit(0)\n")):
            (d / "scripts").mkdir(parents=True)
            (d / "scripts" / "guard.py").write_text(body, encoding="utf-8")
            (d / "scripts" / "guard_rules.py").write_text(
                'SELF_TEST_SAMPLE = {"tool_name": "Bash", "tool_input": {"command": "x"}}\n'
                'PROFILE_ROOTS = ()\n', encoding="utf-8")
        write(self.config / "plugins" / "installed_plugins.json", json.dumps({"version": 2, "plugins": {
            "anthology-guardrails@anthology": [
                {"scope": "user", "installPath": str(guard_old), "lastUpdated": "2026-01-01T00:00:00Z"},
                {"scope": "user", "installPath": str(guard), "lastUpdated": "2026-06-01T00:00:00Z"},
                {"scope": "user", "installPath": str(self.tmp / "gone"), "lastUpdated": "2027-01-01T00:00:00Z"}]}}))
        self.mark_setup(root)
        self.assertNotIn("systemMessage", self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)}))


class GuardLocationTest(SessionCase):
    """F20: the self-test proves the guardrails are installed and enabled, not just that a file blocks."""

    def installed(self, plugins):
        write(self.config / "plugins" / "installed_plugins.json", json.dumps({"version": 2, "plugins": plugins}))

    def guard_install(self, name="guard"):
        d = self.tmp / "installs" / name
        (d / "scripts").mkdir(parents=True)
        (d / "scripts" / "guard.py").write_text(BLOCKS, encoding="utf-8")
        (d / "scripts" / "guard_rules.py").write_text(
            'SELF_TEST_SAMPLE = {"tool_name": "Bash", "tool_input": {"command": "x"}}\nPROFILE_ROOTS = ()\n',
            encoding="utf-8")
        return d

    def test_anthology_installed_without_guardrails_warns_even_if_a_sibling_folder_exists(self):
        self.mark_setup()
        self.installed({"anthology@anthology": [{"scope": "user", "installPath": str(PLUGIN)}]})
        msg = self.go()["systemMessage"]
        self.assertRegex(msg.lower(), r"anthology-guardrails is not installed")

    def test_disabled_in_user_settings_warns(self):
        self.mark_setup()
        data = json.loads((self.config / "settings.json").read_text(encoding="utf-8"))
        data["enabledPlugins"] = {"anthology-guardrails@anthology": False}
        write(self.config / "settings.json", json.dumps(data))
        self.assertRegex(self.go()["systemMessage"].lower(), r"guardrails is disabled")

    def test_disabled_in_project_and_local_settings_warns(self):
        self.mark_setup()
        for name in ("settings.json", "settings.local.json"):
            proj = self.tmp / ("proj-" + name)
            write(proj / ".claude" / name, json.dumps({"enabledPlugins": {"anthology-guardrails@x": False}}))
            self.assertRegex(self.go(cwd=proj)["systemMessage"].lower(), r"guardrails is disabled")

    def test_enabled_true_is_fine(self):
        self.mark_setup()
        data = json.loads((self.config / "settings.json").read_text(encoding="utf-8"))
        data["enabledPlugins"] = {"anthology-guardrails@anthology": True}
        write(self.config / "settings.json", json.dumps(data))
        self.assertNotIn("systemMessage", self.go())

    def cached_root(self):
        """anthology as a marketplace install runs it: from <config>/plugins/cache (no sibling fallback)."""
        root = self.config / "plugins" / "cache" / "anthology" / "anthology" / "0.1.0"
        (root / "templates").mkdir(parents=True)
        shutil.copy(str(GUIDE), str(root / "templates" / "routing-guide.md"))
        return root

    def test_registry_entry_wins_over_the_sibling_folder(self):
        root = self.cached_root()
        guard = root.parent.parent / "anthology-guardrails"  # a sibling that would NOT block
        (guard / "scripts").mkdir(parents=True)
        (guard / "scripts" / "guard.py").write_text("import sys\nsys.exit(0)\n", encoding="utf-8")
        self.mark_setup(root)
        self.installed({"anthology-guardrails@anthology": [
            {"scope": "user", "installPath": str(self.guard_install())}]})
        self.assertNotIn("systemMessage", self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)}))

    def test_project_scoped_install_of_another_project_is_ignored(self):
        self.mark_setup()
        other, mine = self.tmp / "other-proj", self.tmp / "my-proj"
        other.mkdir()
        mine.mkdir()
        self.installed({
            "anthology@anthology": [{"scope": "user", "installPath": str(PLUGIN)}],
            "anthology-guardrails@anthology": [
                {"scope": "project", "projectPath": str(other), "installPath": str(self.guard_install("a"))}]})
        self.assertIn("not installed", self.go(cwd=mine)["systemMessage"])
        (other / "sub").mkdir()
        self.assertNotIn("systemMessage", self.go(cwd=other / "sub"))

    def test_newest_applicable_scope_wins(self):
        root = self.cached_root()
        self.mark_setup(root)
        proj = self.tmp / "proj"
        proj.mkdir()
        bad = self.tmp / "installs" / "bad"
        (bad / "scripts").mkdir(parents=True)
        (bad / "scripts" / "guard.py").write_text("import sys\nsys.exit(0)\n", encoding="utf-8")
        (bad / "scripts" / "guard_rules.py").write_text(
            'SELF_TEST_SAMPLE = {"tool_name": "Bash", "tool_input": {"command": "x"}}\nPROFILE_ROOTS = ()\n',
            encoding="utf-8")
        self.installed({"anthology-guardrails@anthology": [
            {"scope": "user", "installPath": str(bad), "lastUpdated": "2026-01-01T00:00:00Z"},
            {"scope": "project", "projectPath": str(proj), "installPath": str(self.guard_install()),
             "lastUpdated": "2026-06-01T00:00:00Z"}]})
        env = {"CLAUDE_PLUGIN_ROOT": str(root)}
        self.assertNotIn("systemMessage", self.go(cwd=proj, env_extra=env))
        self.assertIn("did not block", self.go(cwd=self.tmp, env_extra=env)["systemMessage"].lower())

    def test_exit_2_without_the_guards_message_is_not_a_pass(self):
        root = self.fake_plugins("import sys\nsys.stdin.read()\nsys.exit(2)\n")
        self.mark_setup(root)
        self.assertRegex(self.go(env_extra={"CLAUDE_PLUGIN_ROOT": str(root)})["systemMessage"].lower(),
                         r"did not block")


class DeadlineTest(SessionCase):
    def test_rtk_reconcile_is_skipped_with_a_note_when_time_is_short(self):
        self.mark_setup()
        with mock.patch.object(session_start, "_elapsed", return_value=session_start.DEADLINE - 10):
            out = self.go()
        self.assertEqual(self.rtk.calls, [])
        self.assertIn("anthology: skipped the rtk check this session (not enough time left)",
                      out["systemMessage"].splitlines())
        self.assertIn("# Anthology routing guide", self.context(out))

    def test_rtk_reconcile_runs_when_there_is_time(self):
        self.mark_setup()
        with mock.patch.object(session_start, "_elapsed", return_value=1):
            self.go()
        self.assertEqual(len(self.rtk.calls), 1)


class ShellWrapperTest(MemCase):
    SH = PLUGIN / "scripts" / "session-start.sh"

    def run_sh(self, path, stdin=""):
        env = {"HOME": str(self.home), "CLAUDE_CONFIG_DIR": str(self.config), "PATH": path,
               "CLAUDE_PLUGIN_ROOT": str(PLUGIN), "CLAUDE_PLUGIN_DATA": str(self.data)}
        return subprocess.run(["/bin/sh", str(self.SH)], input=stdin.encode(), env=env,
                              capture_output=True, timeout=120)

    def test_python3_missing_prints_one_json_system_message_and_exits_zero(self):
        proc = self.run_sh("/nonexistent")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout.decode("utf-8"))
        self.assertEqual(list(out), ["systemMessage"])
        self.assertIn("python3", out["systemMessage"])
        self.assertIn("guardrails", out["systemMessage"])
        self.assertIn("routing guide", out["systemMessage"])

    def test_python3_present_emits_valid_hook_json(self):
        py = shutil.which("python3")
        self.assertIsNotNone(py)
        proc = self.run_sh(os.path.dirname(py) + os.pathsep + "/usr/bin" + os.pathsep + "/bin",
                           json.dumps({"cwd": str(self.tmp)}))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout.decode("utf-8"))
        self.assertIn("# Anthology routing guide", out["hookSpecificOutput"]["additionalContext"])

    def shim_dir(self, body):
        d = self.tmp / "shim"
        d.mkdir(exist_ok=True)
        shim = d / "python3"
        shim.write_text("#!/bin/sh\n" + body + "\n", encoding="utf-8")
        shim.chmod(0o755)
        return str(d)

    def test_python3_that_does_not_run_gets_the_same_loud_message(self):
        proc = self.run_sh(self.shim_dir("exit 1"))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout.decode("utf-8"))
        self.assertEqual(list(out), ["systemMessage"])
        self.assertIn("python3", out["systemMessage"])
        self.assertIn("3.9", out["systemMessage"])

    def test_broken_sibling_module_still_exits_zero_with_json(self):
        # A half-written plugin update: a sibling module that fails to compile.
        copy = self.tmp / "plugin-copy"
        shutil.copytree(str(PLUGIN / "scripts"), str(copy / "scripts"))
        (copy / "scripts" / "memory_sync.py").write_text("def broken(:\n", encoding="utf-8")
        env = {"HOME": str(self.home), "CLAUDE_CONFIG_DIR": str(self.config), "PATH": os.environ.get("PATH", ""),
               "CLAUDE_PLUGIN_ROOT": str(copy), "CLAUDE_PLUGIN_DATA": str(self.data)}
        proc = subprocess.run([sys.executable, str(copy / "scripts" / "session_start.py")], input=b"{}",
                              env=env, capture_output=True, timeout=120)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout.decode("utf-8"))
        self.assertIn("anthology", out["systemMessage"])

    def test_script_uses_only_sh_builtins_on_the_missing_path(self):
        text = self.SH.read_text(encoding="utf-8")
        self.assertIn("command -v python3", text)
        self.assertIn("exec python3", text)
        self.assertTrue(text.startswith("#!/bin/sh"))


class HooksJsonTest(unittest.TestCase):
    def test_hooks_json_has_the_session_start_entry_and_the_rtk_pretooluse_entry(self):
        data = json.loads((PLUGIN / "hooks" / "hooks.json").read_text(encoding="utf-8"))
        self.assertEqual(set(data), {"hooks"})
        # SessionStart is this plugin's; PreToolUse is the rtk rewrite hook (tests/test_rtk_install.py).
        self.assertEqual(set(data["hooks"]), {"SessionStart", "PreToolUse"})
        entries = data["hooks"]["SessionStart"]
        self.assertEqual(len(entries), 1)
        self.assertNotIn("matcher", entries[0])
        self.assertEqual(entries[0]["hooks"], [{
            "type": "command", "command": "sh",
            "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/session-start.sh"], "timeout": 60}])


class SkillsFolderTest(unittest.TestCase):
    def test_skills_folder_is_exactly_setup_export_import(self):
        names = {p.name for p in (PLUGIN / "skills").iterdir()}
        self.assertEqual(names, {"setup", "export", "import"})

    def test_every_skill_is_manual_only(self):
        for name in ("setup", "export", "import"):
            head = (PLUGIN / "skills" / name / "SKILL.md").read_text(encoding="utf-8").split("\n---")[0]
            self.assertRegex(head, r"(?m)^name: %s$" % name)
            self.assertRegex(head, r"(?m)^description: \S")
            self.assertRegex(head, r"(?m)^disable-model-invocation: true$")

    def test_setup_skill_content(self):
        text = (PLUGIN / "skills" / "setup" / "SKILL.md").read_text(encoding="utf-8")
        self.assertIn('--plugin-root "${CLAUDE_PLUGIN_ROOT}"', text)
        self.assertIn("${CLAUDE_PLUGIN_ROOT}/scripts/anthology_setup.py", text)
        self.assertIn("/anthology:setup remove", text)
        self.assertIn("rtk bypass <cmd>", text)
        self.assertIn("--expect-sha256", text)
        self.assertIn("claude plugin uninstall", text)
        self.assertNotRegex(text, r"/U" + r"sers/|@[a-z0-9.-]+\.(com|org|dev)")


class GuideTest(unittest.TestCase):
    def setUp(self):
        self.text = GUIDE.read_text(encoding="utf-8")

    def test_size_and_shape(self):
        self.assertLessEqual(len(self.text), 3800)
        self.assertFalse(self.text.startswith("---"))
        for n in range(1, 9):
            self.assertRegex(self.text, r"(?m)^## %d\. " % n)

    def test_uses_anthology_ids_for_every_roster_agent(self):
        for agent in AGENTS:
            self.assertIn("anthology:%s" % agent, self.text)
        self.assertIn("claude-security", self.text)
        self.assertRegex(self.text, r"ui-designer builds, then ui-reviewer checks")

    def test_browser_rules(self):
        self.assertIn("BrowserOS Neo", self.text)
        self.assertRegex(self.text, r"BrowserOS Neo \(MCP\), the only browser")
        self.assertRegex(self.text, r"BrowserOS Neo unavailable: stop and ask\. Never install a browser")
        self.assertRegex(self.text, r"Never launch Chrome")
        for tool in ("agent-browser", "Playwright", "chrome-devtools", "Claude in Chrome"):
            self.assertIn(tool, self.text)
        for line in self.text.splitlines():
            if re.search(r"agent-browser|chrome-devtools|Claude in Chrome", line):
                self.assertRegex(line, r"Never ", line)

    def test_worktree_docker_rtk_and_memory_rules(self):
        self.assertIn('worktree.baseRef: "head"', self.text)
        self.assertIn("never `git worktree add`", self.text)
        self.assertIn("chore(memory)", self.text)
        self.assertRegex(self.text, r"commit before finishing")
        self.assertIn("Docker unavailable: stop and tell the user", self.text)
        self.assertIn("rtk bypass <cmd>", self.text)
        self.assertIn("anthology-mem search", self.text)
        self.assertIn("[[note-name]]", self.text)
        self.assertRegex(self.text, r"Never save secrets")

    def test_personal_index_is_below_the_guide(self):
        self.assertIn("personal memory, indexed below", self.text)
        self.assertNotIn("indexed above", self.text)

    def test_no_personal_details(self):
        self.assertNotRegex(self.text, r"/U" + r"sers/|/home/\w|@\w+\.\w+")


if __name__ == "__main__":
    unittest.main()
