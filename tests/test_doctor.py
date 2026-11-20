"""Tests for scripts/doctor.py (environment checks, duplicate-plugin detection)."""
from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
PLUGIN = ROOT / "plugins" / "anthology"
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
sys.path.insert(0, str(SCRIPTS))

import _rtk  # noqa: E402
import doctor  # noqa: E402

INSTALLED = {
    "version": 2,
    "plugins": {
        "anthology@anthology": [{"scope": "user", "installPath": "/x/a", "version": "0.1.0"}],
        "superpowers@claude-plugins-official": [
            {"scope": "user", "installPath": "/x/s", "version": "7.0.2"}],
        "superpowers@other-market": [{"scope": "project", "installPath": "/x/s2"},
                                     {"scope": "user", "installPath": "/x/s3"}],
        "design-skills@design-skills": [{"scope": "local", "installPath": "/x/d"}],
        "anthology-superpowers@anthology": [{"scope": "user", "installPath": "/x/as"}],
        "superpowers-extras@m": [{"scope": "user", "installPath": "/x/e"}],
    },
}


class FindDuplicatesTest(unittest.TestCase):
    def test_finds_superpowers_and_design_skills_installs_with_uninstall_commands(self):
        found = doctor.find_duplicates(INSTALLED)
        self.assertEqual(
            sorted((d["plugin"], d["scope"]) for d in found),
            [("design-skills@design-skills", "local"),
             ("superpowers@claude-plugins-official", "user"),
             ("superpowers@other-market", "project"),
             ("superpowers@other-market", "user")])
        by = {(d["plugin"], d["scope"]): d["command"] for d in found}
        self.assertEqual(by[("superpowers@claude-plugins-official", "user")],
                         "claude plugin uninstall superpowers@claude-plugins-official -s user")
        self.assertEqual(by[("design-skills@design-skills", "local")],
                         "claude plugin uninstall design-skills@design-skills -s local")

    def test_ours_are_not_duplicates(self):
        names = {d["plugin"] for d in doctor.find_duplicates(INSTALLED)}
        self.assertNotIn("anthology-superpowers@anthology", names)
        self.assertNotIn("superpowers-extras@m", names)

    def test_odd_shapes_give_no_duplicates(self):
        for installed in (None, [], {}, {"plugins": []}, {"plugins": {"superpowers@x": "bad"}},
                          {"plugins": {"superpowers@x": [None, 3]}}):
            self.assertEqual(doctor.find_duplicates(installed), [], installed)

    def test_missing_scope_defaults_to_user(self):
        found = doctor.find_duplicates({"plugins": {"superpowers@m": [{"installPath": "/p"}]}})
        self.assertEqual(found[0]["scope"], "user")
        self.assertTrue(found[0]["command"].endswith("-s user"))


class EnvCase(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(os.path.realpath(tmp.name))
        self.config = self.tmp / "config"
        self.config.mkdir()
        self.env = {"HOME": str(self.tmp), "CLAUDE_CONFIG_DIR": str(self.config), "PATH": "/bin"}
        for patcher in (mock.patch.dict(os.environ, self.env), mock.patch("platform.system", return_value="Linux")):
            patcher.start()
            self.addCleanup(patcher.stop)
        for var in ("CLAUDECODE", "CLAUDE_CODE_SUBAGENT_MODEL", "CLAUDE_PLUGIN_DATA", "CLAUDE_PLUGIN_ROOT"):
            os.environ.pop(var, None)  # the host may be Claude Code itself
        self.tools = {"python3", "node", "docker"}
        which = mock.patch.object(doctor.shutil, "which",
                                  side_effect=lambda n, path=None: "/usr/bin/" + n if n in self.tools else None)
        which.start()
        self.addCleanup(which.stop)
        self.docker_exit = 0

        def fake_run(argv, **kw):
            if argv[:2] == ["docker", "info"] or argv[:1] == ["/usr/bin/docker"]:
                return subprocess.CompletedProcess(argv, self.docker_exit, b"", b"")
            raise AssertionError("unexpected command %r" % (argv,))

        run = mock.patch.object(doctor.subprocess, "run", side_effect=fake_run)
        run.start()
        self.addCleanup(run.stop)
        self.apps = self.tmp / "SysApplications"
        self.managed = self.tmp / "managed-mcp.json"
        self.cwd = self.tmp / "proj"
        self.cwd.mkdir()
        for patcher in (mock.patch.object(doctor, "SYSTEM_APPLICATIONS", self.apps),
                        mock.patch.object(doctor, "MANAGED_MCP_PATHS", (self.managed,))):
            patcher.start()
            self.addCleanup(patcher.stop)

    def checks(self):
        return {c["name"]: c for c in doctor.check_env(dict(os.environ), cwd=self.cwd)}



class CheckEnvTest(EnvCase):
    def test_shape_and_order(self):
        checks = doctor.check_env(dict(os.environ), cwd=self.cwd)
        self.assertEqual([c["name"] for c in checks],
                         ["python3", "node", "docker", "browseros", "rtk", "platform_supported",
                          "plugin_read_rule", "browseros_tools", "plugin_data", "path_wrappers",
                          "agent_memory_path", "subagent_model"])
        for c in checks:
            self.assertEqual(set(c), {"name", "ok", "detail"})
            self.assertIn(c["ok"], (True, False, None))
            self.assertIsInstance(c["detail"], str)

    def test_present_tools_are_ok(self):
        c = self.checks()
        for name in ("python3", "node", "docker", "platform_supported"):
            self.assertTrue(c[name]["ok"], name)

    def test_missing_tools_are_not_ok(self):
        self.tools = set()
        c = self.checks()
        for name in ("python3", "node", "docker"):
            self.assertFalse(c[name]["ok"], name)
        self.assertIn("Node", c["node"]["detail"])

    def test_docker_on_path_but_not_running_says_start_it(self):
        self.docker_exit = 1
        c = self.checks()["docker"]
        self.assertFalse(c["ok"])
        self.assertIn("start", c["detail"].lower())

    def test_docker_timeout_is_not_ok(self):
        doctor.subprocess.run.side_effect = subprocess.TimeoutExpired("docker", 10)
        self.assertFalse(self.checks()["docker"]["ok"])

    def test_browseros_via_mcp_server_name_without_leaking_urls_or_tokens(self):
        (self.config / ".claude.json").write_text(json.dumps({
            "mcpServers": {"browseros-neo": {"type": "http", "url": "http://127.0.0.1:1/mcp?token=SECRET",
                                             "headers": {"Authorization": "Bearer SECRET"}}}}), encoding="utf-8")
        c = self.checks()["browseros"]
        self.assertTrue(c["ok"])
        self.assertNotIn("SECRET", json.dumps(c))
        self.assertNotIn("127.0.0.1", json.dumps(c))

    def test_browseros_via_app_or_path(self):
        self.assertFalse(self.checks()["browseros"]["ok"])
        (self.apps / "BrowserOS.app").mkdir(parents=True)
        self.assertTrue(self.checks()["browseros"]["ok"])
        (self.apps / "BrowserOS.app").rmdir()
        self.tools.add("browseros")
        self.assertTrue(self.checks()["browseros"]["ok"])

    def test_browseros_is_never_offered_as_an_install(self):
        detail = self.checks()["browseros"]["detail"]
        self.assertNotRegex(detail.lower(), r"brew|install chrome|download")

    def test_claude_json_in_home_is_used_without_config_dir(self):
        env = {"HOME": str(self.tmp), "PATH": "/bin"}
        (self.tmp / ".claude.json").write_text(json.dumps({"mcpServers": {"my-browseros": {}}}), encoding="utf-8")
        checks = {c["name"]: c for c in doctor.check_env(env, cwd=self.cwd)}
        self.assertTrue(checks["browseros"]["ok"])

    def test_corrupt_claude_json_is_not_an_error(self):
        (self.config / ".claude.json").write_text("{nope", encoding="utf-8")
        self.assertFalse(self.checks()["browseros"]["ok"])

    def test_windows_is_unsupported(self):
        with mock.patch("platform.system", return_value="Windows"):
            c = self.checks()["platform_supported"]
        self.assertFalse(c["ok"])
        self.assertIn("macOS and Linux", c["detail"])

    def test_rtk_not_wired_yet_is_none(self):
        with mock.patch.dict(sys.modules, {"rtk_install": None}):
            self.assertIsNone(self.checks()["rtk"]["ok"])
            self.assertIn("not wired yet", self.checks()["rtk"]["detail"])

    def write_state(self, **kv):
        state_dir = self.config / "anthology"
        state_dir.mkdir(exist_ok=True)
        (state_dir / "state.json").write_text(json.dumps(kv), encoding="utf-8")

    def rtk_check(self, lock):
        return {x["name"]: x for x in doctor.check_env(dict(os.environ), lock=lock, cwd=self.cwd)}["rtk"]

    def test_rtk_is_checked_through_the_lock_and_the_installed_marker(self):
        data = self.tmp / "data"
        binary = data / "rtk" / "1.0.0" / "rtk"
        binary.parent.mkdir(parents=True)
        binary.write_text("x", encoding="utf-8")
        self.write_state(rtk_path=str(binary), data_dir=str(data))
        lock = self.tmp / "rtk.lock"
        lock.write_text("{}", encoding="utf-8")
        parsed = {"version": "1.0.0", "sha256": {"linux-x86": "ab" * 32}}
        calls = []

        def check_installed(data_dir, lock_data, target=None):
            calls.append((str(data_dir), lock_data, target))
            return True

        fake = types.SimpleNamespace(load_lock=lambda path: parsed,
                                     target=lambda system, machine: "linux-x86",
                                     check_installed=check_installed)
        with mock.patch.dict(sys.modules, {"rtk_install": fake}):
            c = self.rtk_check(lock)
            self.assertTrue(c["ok"], c)
            self.assertEqual(calls, [(str(data), parsed, "linux-x86")])
            fake.check_installed = lambda *a: False
            c = self.rtk_check(lock)
            self.assertFalse(c["ok"])
            self.assertIn("setup", c["detail"])

    def test_rtk_outside_the_locked_version_dir_is_not_ok(self):
        data = self.tmp / "data"
        binary = data / "rtk" / "0.9.0" / "rtk"  # an older version than the lock's
        binary.parent.mkdir(parents=True)
        binary.write_text("x", encoding="utf-8")
        self.write_state(rtk_path=str(binary), data_dir=str(data))
        fake = types.SimpleNamespace(load_lock=lambda path: {"version": "1.0.0"},
                                     target=lambda s, m: "t", check_installed=lambda *a: True)
        with mock.patch.dict(sys.modules, {"rtk_install": fake}):
            c = self.rtk_check(self.tmp / "rtk.lock")
        self.assertFalse(c["ok"])

    def test_rtk_missing_binary_is_not_ok(self):
        fake = types.SimpleNamespace(load_lock=lambda path: {"version": "1.0.0"},
                                     target=lambda s, m: "t", check_installed=lambda *a: True)
        with mock.patch.dict(sys.modules, {"rtk_install": fake}):
            c = self.rtk_check(self.tmp / "rtk.lock")
            self.assertFalse(c["ok"])
            self.write_state(rtk_path=str(self.tmp / "gone" / "rtk"))
            self.assertFalse(self.rtk_check(self.tmp / "rtk.lock")["ok"])

    def test_rtk_unreadable_lock_is_not_wired_yet(self):
        def load_lock(path):
            raise RuntimeError("cannot read")

        fake = types.SimpleNamespace(load_lock=load_lock, target=lambda s, m: "t",
                                     check_installed=lambda *a: True)
        with mock.patch.dict(sys.modules, {"rtk_install": fake}):
            c = self.rtk_check(self.tmp / "rtk.lock")
        self.assertIsNone(c["ok"])
        self.assertIn("not wired yet", c["detail"])

    def test_real_rtk_install_installed_binary_passes_and_tampering_fails(self):
        import rtk_install
        blob = _rtk.tarball()
        lock = self.tmp / "rtk.lock"
        _rtk.write_lock(lock, blob)
        data = self.tmp / "plugin-data"
        with mock.patch("platform.machine", return_value=_rtk.LINUX[1]):
            self.assertFalse(self.rtk_check(lock)["ok"])  # nothing installed yet
            result = rtk_install.reconcile(data, rtk_install.load_lock(lock), _rtk.FakeFetch(blob), 5)
            self.assertIsNone(result["warning"], result)
            c = self.rtk_check(lock)
            self.assertTrue(c["ok"], c)
            os.chmod(result["rtk_path"], 0o600)  # no longer executable
            self.assertFalse(self.rtk_check(lock)["ok"])
            os.chmod(result["rtk_path"], 0o755)
            self.assertTrue(self.rtk_check(lock)["ok"])
            # a lock bumped to a version that is not installed yet
            _rtk.write_lock(lock, blob, version="1.3.0-anthology.1")
            self.assertFalse(self.rtk_check(lock)["ok"])

    def test_real_rtk_install_missing_lock_is_not_wired_yet(self):
        c = self.rtk_check(self.tmp / "absent.lock")
        self.assertIsNone(c["ok"])

    def plugin_rule(self, **kw):
        return {c["name"]: c for c in doctor.check_env(dict(os.environ), plugin_root=PLUGIN, cwd=self.cwd, **kw)}["plugin_read_rule"]

    def test_plugin_read_rule_is_unknown_before_setup(self):
        self.assertIsNone(self.plugin_rule()["ok"])

    def test_plugin_read_rule_after_setup(self):
        import settings_patch
        state_dir = self.config / "anthology"
        state_dir.mkdir()
        (state_dir / "state.json").write_text(json.dumps({"setup_done": "0.1.0"}), encoding="utf-8")
        self.assertFalse(self.plugin_rule()["ok"])  # no settings.json yet
        settings = self.config / "settings.json"
        settings.write_text(json.dumps({"permissions": {"allow": ["Read(//old/**)"]}}), encoding="utf-8")
        c = self.plugin_rule()
        self.assertFalse(c["ok"])
        self.assertIn("setup", c["detail"])
        settings.write_text(json.dumps({"permissions": {"allow": [settings_patch.plugin_read_rule(PLUGIN)]}}),
                            encoding="utf-8")
        self.assertTrue(self.plugin_rule()["ok"])
        settings.write_text("{nope", encoding="utf-8")
        self.assertIn("cannot be read", self.plugin_rule()["detail"])

    def test_check_env_never_raises(self):
        with mock.patch.object(doctor.shutil, "which", side_effect=RuntimeError("boom")):
            checks = doctor.check_env(dict(os.environ), cwd=self.cwd)
        self.assertEqual(len(checks), 12)
        self.assertFalse(checks[0]["ok"])


def _write(path, text):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text if isinstance(text, str) else json.dumps(text), encoding="utf-8")
    return path


class CompatBase(EnvCase):
    """CheckEnvTest's isolated HOME/config/PATH plus helpers for the PLAN section 14 checks."""

    def check(self, name, env_extra=None, plugin_root=PLUGIN):
        env = dict(os.environ)
        env.update(env_extra or {})
        found = {c["name"]: c for c in doctor.check_env(env, plugin_root=plugin_root, cwd=self.cwd)}
        return found[name]

    def user_json(self, data):
        return _write(self.config / ".claude.json", data)


class BrowserosAppTest(CompatBase):
    def test_neo_app_in_system_applications(self):
        (self.apps / "BrowserOS neo.app").mkdir(parents=True)
        c = self.checks()["browseros"]
        self.assertTrue(c["ok"], c)

    def test_plain_app_in_user_applications(self):
        (self.tmp / "Applications").mkdir()
        self.assertFalse(self.checks()["browseros"]["ok"])
        (self.tmp / "Applications" / "BrowserOS.app").mkdir()
        self.assertTrue(self.checks()["browseros"]["ok"])

    def test_unrelated_app_is_not_found(self):
        (self.apps / "Foo.app").mkdir(parents=True)
        (self.apps / "browseros.app").mkdir()  # the prefix is case-sensitive
        self.assertFalse(self.checks()["browseros"]["ok"])

    def test_a_file_is_not_an_app(self):
        _write(self.apps / "BrowserOS neo.app", "x")
        self.assertFalse(self.checks()["browseros"]["ok"])

    def test_linux_binaries_on_path(self):
        for name in ("browseros", "browseros-neo", "BrowserOS"):
            self.tools = {"python3", name}
            self.assertTrue(self.checks()["browseros"]["ok"], name)

    def test_linux_appimage_in_user_applications_or_local_bin(self):
        self.assertFalse(self.checks()["browseros"]["ok"])
        _write(self.tmp / ".local" / "bin" / "BrowserOS-1.2.AppImage", "x")
        self.assertTrue(self.checks()["browseros"]["ok"])
        (self.tmp / ".local" / "bin" / "BrowserOS-1.2.AppImage").unlink()
        _write(self.tmp / "Applications" / "BrowserOS.AppImage", "x")
        self.assertTrue(self.checks()["browseros"]["ok"])

    def test_not_found_message_still_says_install_it_yourself(self):
        c = self.checks()["browseros"]
        self.assertFalse(c["ok"])
        self.assertIn("agents never install", c["detail"])


class FindMcpServersTest(CompatBase):
    def find(self, cwd=None):
        return doctor.find_mcp_servers(dict(os.environ), cwd or self.cwd)

    def by_name(self, cwd=None):
        return {s["name"]: s for s in self.find(cwd)}

    def test_user_scope(self):
        self.user_json({"mcpServers": {"browseros-neo": {"url": "http://127.0.0.1:9/mcp?token=SECRET"}}})
        s = self.by_name()["browseros-neo"]
        self.assertEqual((s["scope"], s["tool_prefix"]), ("user", "mcp__browseros-neo__"))
        self.assertEqual(set(s), {"name", "scope", "tool_prefix"})

    def test_user_scope_from_home_without_config_dir(self):
        _write(self.tmp / ".claude.json", {"mcpServers": {"a": {}}})
        env = {"HOME": str(self.tmp), "PATH": "/bin"}
        self.assertEqual([s["name"] for s in doctor.find_mcp_servers(env, self.cwd)], ["a"])

    def test_local_scope_for_cwd_and_ancestors(self):
        deep = self.cwd / "a" / "b"
        deep.mkdir(parents=True)
        self.user_json({"projects": {str(self.cwd): {"mcpServers": {"loc": {"command": "SECRETCMD"}}},
                                     str(self.tmp / "other"): {"mcpServers": {"nope": {}}}}})
        found = self.by_name(deep)
        self.assertEqual(found["loc"]["scope"], "local")
        self.assertNotIn("nope", found)

    def test_project_scope_walks_up_to_the_first_mcp_json(self):
        deep = self.cwd / "a" / "b"
        deep.mkdir(parents=True)
        _write(self.cwd / ".mcp.json", {"mcpServers": {"proj": {"args": ["SECRETARG"]}}})
        s = self.by_name(deep)["proj"]
        self.assertEqual((s["scope"], s["tool_prefix"]), ("project", "mcp__proj__"))

    def test_project_walk_stops_at_the_git_toplevel(self):
        _write(self.tmp / ".mcp.json", {"mcpServers": {"outside": {}}})
        (self.cwd / ".git").mkdir()
        (self.cwd / "sub").mkdir()
        self.assertNotIn("outside", self.by_name(self.cwd / "sub"))

    def test_plugin_scope_from_mcp_json_and_plugin_json(self):
        one, two = self.tmp / "p1", self.tmp / "p2"
        _write(one / ".mcp.json", {"mcpServers": {"srv": {"url": "http://SECRETHOST"}}})
        _write(two / ".mcp.json", {"flat": {"command": "x"}})  # top-level map form
        _write(two / ".claude-plugin" / "plugin.json", {"mcpServers": {"inline": {"command": "y"}}})
        _write(self.config / "plugins" / "installed_plugins.json", {"version": 2, "plugins": {
            "one@market": [{"scope": "user", "installPath": str(one)}],
            "two@market": [{"scope": "user", "installPath": str(two)}]}})
        found = self.by_name()
        self.assertEqual(found["srv"]["scope"], "plugin")
        self.assertEqual(found["srv"]["tool_prefix"], "mcp__plugin_one_srv__")
        self.assertEqual(found["flat"]["tool_prefix"], "mcp__plugin_two_flat__")
        self.assertEqual(found["inline"]["tool_prefix"], "mcp__plugin_two_inline__")

    def test_managed_scope(self):
        _write(self.managed, {"mcpServers": {"corp": {"url": "http://SECRETHOST"}}})
        s = self.by_name()["corp"]
        self.assertEqual((s["scope"], s["tool_prefix"]), ("managed", "mcp__corp__"))

    def test_malformed_files_are_skipped(self):
        self.user_json("{nope")
        _write(self.cwd / ".mcp.json", "[1, 2")
        _write(self.managed, "")
        _write(self.config / "plugins" / "installed_plugins.json",
               {"plugins": {"x@m": "bad", "y@m": [None, {"installPath": 3}]}})
        self.assertEqual(self.find(), [])
        self.user_json({"mcpServers": ["a"], "projects": {str(self.cwd): "bad"}})
        self.assertEqual(self.find(), [])

    def test_no_value_from_any_config_reaches_any_check(self):
        secret = "SECRETVALUE123"
        self.user_json({"mcpServers": {"my-browseros": {"url": "http://%s/mcp" % secret,
                                                        "headers": {"A": secret}}}})
        _write(self.cwd / ".mcp.json", {"mcpServers": {"p": {"command": secret, "args": [secret]}}})
        _write(self.managed, {"mcpServers": {"m": {"env": {"T": secret}}}})
        blob = json.dumps(doctor.check_env(dict(os.environ), plugin_root=PLUGIN, cwd=self.cwd))
        self.assertNotIn(secret, blob)
        self.assertIn("my-browseros", blob)


class BrowserosToolsTest(CompatBase):
    def test_exact_name_in_user_scope_is_ok(self):
        self.user_json({"mcpServers": {"browseros-neo": {}}})
        self.assertTrue(self.checks()["browseros_tools"]["ok"])

    def test_exact_name_in_project_and_managed_scope_is_ok(self):
        _write(self.cwd / ".mcp.json", {"mcpServers": {"browseros-neo": {}}})
        self.assertTrue(self.checks()["browseros_tools"]["ok"])
        (self.cwd / ".mcp.json").unlink()
        _write(self.managed, {"mcpServers": {"browseros-neo": {}}})
        self.assertTrue(self.checks()["browseros_tools"]["ok"])

    def test_wrong_name_is_loud(self):
        self.user_json({"mcpServers": {"browseros": {"url": "http://SECRETHOST"}}})
        c = self.checks()["browseros_tools"]
        self.assertFalse(c["ok"])
        self.assertIn("mcp__browseros-neo__", c["detail"])
        self.assertIn("browseros", c["detail"])
        self.assertIn("user", c["detail"])
        self.assertNotIn("SECRETHOST", c["detail"])

    def test_plugin_provided_server_is_loud(self):
        plug = self.tmp / "bp"
        _write(plug / ".mcp.json", {"mcpServers": {"browseros-neo": {}}})
        _write(self.config / "plugins" / "installed_plugins.json",
               {"plugins": {"bp@m": [{"installPath": str(plug)}]}})
        c = self.checks()["browseros_tools"]
        self.assertFalse(c["ok"])
        self.assertIn("mcp__browseros-neo__", c["detail"])
        self.assertIn("mcp__plugin_bp_browseros-neo__", c["detail"])

    def test_no_server_is_none(self):
        self.assertIsNone(self.checks()["browseros_tools"]["ok"])

    def test_the_browseros_check_accepts_any_scope(self):
        _write(self.cwd / ".mcp.json", {"mcpServers": {"x-browseros": {}}})
        self.assertTrue(self.checks()["browseros"]["ok"])


class PluginDataTest(CompatBase):
    def state(self, **kv):
        _write(self.config / "anthology" / "state.json", kv)

    def test_not_recorded_is_none(self):
        self.assertIsNone(self.checks()["plugin_data"]["ok"])

    def test_recorded_and_present_is_ok(self):
        (self.tmp / "d").mkdir()
        self.state(data_dir=str(self.tmp / "d"))
        self.assertTrue(self.checks()["plugin_data"]["ok"])

    def test_recorded_but_gone_is_loud(self):
        self.state(data_dir=str(self.tmp / "gone"))
        c = self.checks()["plugin_data"]
        self.assertFalse(c["ok"])
        self.assertIn("gone", c["detail"])

    def test_a_file_is_not_a_folder(self):
        _write(self.tmp / "f", "x")
        self.state(data_dir=str(self.tmp / "f"))
        self.assertFalse(self.checks()["plugin_data"]["ok"])

    def test_env_wins_over_state(self):
        (self.tmp / "d").mkdir()
        self.state(data_dir=str(self.tmp / "gone"))
        self.assertTrue(self.check("plugin_data", {"CLAUDE_PLUGIN_DATA": str(self.tmp / "d")})["ok"])


class PathWrappersTest(CompatBase):
    def fake_plugin(self):
        root = self.tmp / "plug"
        for name in ("anthology-mem", "rtk", "rtk-hook"):
            _write(root / "bin" / name, "#!/bin/sh\n")
            os.chmod(root / "bin" / name, 0o755)
        return root

    def test_all_present_and_executable_outside_claude_code(self):
        root = self.fake_plugin()
        self.assertTrue(self.check("path_wrappers", plugin_root=root)["ok"])

    def test_missing_and_non_executable_are_named(self):
        root = self.fake_plugin()
        (root / "bin" / "rtk").unlink()
        os.chmod(root / "bin" / "rtk-hook", 0o644)
        c = self.check("path_wrappers", plugin_root=root)
        self.assertFalse(c["ok"])
        self.assertIn("bin/rtk", c["detail"])
        self.assertIn("rtk-hook", c["detail"])
        self.assertNotIn("anthology-mem", c["detail"])

    def test_the_real_plugin_has_its_wrappers(self):
        self.assertTrue(self.check("path_wrappers")["ok"])

    def test_inside_claude_code_the_bin_must_be_on_path(self):
        root = self.fake_plugin()
        bad = self.check("path_wrappers", {"CLAUDECODE": "1"}, plugin_root=root)
        self.assertFalse(bad["ok"])
        self.assertIn("bin/ is not on PATH", bad["detail"])
        which = mock.patch.object(doctor.shutil, "which",
                                  side_effect=lambda n, path=None: str(root / "bin" / n))
        which.start()
        self.addCleanup(which.stop)
        self.assertTrue(self.check("path_wrappers", {"CLAUDECODE": "1"}, plugin_root=root)["ok"])

    def test_inside_claude_code_another_anthology_mem_on_path_is_not_ours(self):
        root = self.fake_plugin()
        other = self.tmp / "other" / "anthology-mem"
        _write(other, "x")
        which = mock.patch.object(doctor.shutil, "which", side_effect=lambda n, path=None: str(other))
        which.start()
        self.addCleanup(which.stop)
        self.assertFalse(self.check("path_wrappers", {"CLAUDECODE": "1"}, plugin_root=root)["ok"])


class AgentMemoryPathTest(CompatBase):
    def setUp(self):
        super().setUp()
        (self.cwd / ".git").mkdir()
        self.mem = self.cwd / ".claude" / "agent-memory"

    def test_none_is_none(self):
        self.assertIsNone(self.checks()["agent_memory_path"]["ok"])
        self.mem.mkdir(parents=True)
        self.assertIsNone(self.checks()["agent_memory_path"]["ok"])

    def test_expected_folders_are_ok(self):
        (self.mem / "anthology-implementer").mkdir(parents=True)
        self.assertTrue(self.checks()["agent_memory_path"]["ok"])

    def test_bare_agent_name_alone_is_a_soft_warning_not_a_failure(self):
        (self.mem / "implementer").mkdir(parents=True)
        c = self.checks()["agent_memory_path"]
        self.assertIsNone(c["ok"])
        self.assertIn("agent-memory/implementer/", c["detail"])
        self.assertIn("no `anthology-implementer/`", c["detail"])
        self.assertIn("agent of your own", c["detail"])

    def test_a_user_agent_next_to_anthology_folders_is_fine(self):
        (self.mem / "reviewer").mkdir(parents=True)
        (self.mem / "anthology-implementer").mkdir()
        c = self.checks()["agent_memory_path"]
        self.assertTrue(c["ok"])
        self.assertNotIn("reviewer", c["detail"])

    def test_colon_and_underscore_forms_alone_are_soft(self):
        for name in ("anthology:implementer", "anthology_debugger"):
            (self.mem / name).mkdir(parents=True)
            self.assertIsNone(self.checks()["agent_memory_path"]["ok"], name)
            (self.mem / name).rmdir()

    def test_repo_top_survives_an_unreadable_parent(self):
        with mock.patch("pathlib.Path.exists", side_effect=PermissionError("denied")):
            self.assertIsNone(doctor.compat.repo_top(self.cwd))

    def test_found_from_a_subfolder_and_unrelated_folders_ignored(self):
        sub = self.cwd / "pkg" / "x"
        sub.mkdir(parents=True)
        (self.mem / "notes").mkdir(parents=True)
        (self.mem / "anthology-implementer").mkdir()
        c = {x["name"]: x for x in doctor.check_env(dict(os.environ), cwd=sub)}["agent_memory_path"]
        self.assertTrue(c["ok"])


class SubagentModelTest(CompatBase):
    def test_unset_is_ok(self):
        self.assertTrue(self.checks()["subagent_model"]["ok"])

    def test_set_in_process_env(self):
        c = self.check("subagent_model", {"CLAUDE_CODE_SUBAGENT_MODEL": "SECRETMODEL"})
        self.assertFalse(c["ok"])
        self.assertIn("process environment", c["detail"])
        self.assertNotIn("SECRETMODEL", c["detail"])
        self.assertIn("Anthology never changes it", c["detail"])
        self.assertIn("opus/sonnet/haiku", c["detail"])

    def test_empty_env_value_is_unset(self):
        self.assertTrue(self.check("subagent_model", {"CLAUDE_CODE_SUBAGENT_MODEL": ""})["ok"])

    def test_set_only_in_user_settings(self):
        _write(self.config / "settings.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "SECRETMODEL"}})
        c = self.checks()["subagent_model"]
        self.assertFalse(c["ok"])
        self.assertIn("settings.json", c["detail"])
        self.assertNotIn("SECRETMODEL", c["detail"])

    def test_set_in_project_settings_and_local(self):
        _write(self.cwd / ".claude" / "settings.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}})
        self.assertFalse(self.checks()["subagent_model"]["ok"])
        (self.cwd / ".claude" / "settings.json").unlink()
        _write(self.cwd / ".claude" / "settings.local.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}})
        c = self.checks()["subagent_model"]
        self.assertFalse(c["ok"])
        self.assertIn("settings.local.json", c["detail"])

    def test_unrelated_settings_and_garbage_are_ok(self):
        _write(self.config / "settings.json", {"env": {"OTHER": "1"}})
        _write(self.cwd / ".claude" / "settings.json", "{nope")
        _write(self.cwd / ".claude" / "settings.local.json", {"env": "bad"})
        self.assertTrue(self.checks()["subagent_model"]["ok"])

    def test_the_owners_settings_are_never_written(self):
        path = _write(self.config / "settings.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}})
        before = path.read_bytes()
        self.checks()
        self.assertEqual(path.read_bytes(), before)


class CliTest(unittest.TestCase):
    def test_json_output(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        config = Path(tmp.name) / "c"
        (config / "plugins").mkdir(parents=True)
        (config / "plugins" / "installed_plugins.json").write_text(json.dumps(INSTALLED), encoding="utf-8")
        env = dict(os.environ, CLAUDE_CONFIG_DIR=str(config), HOME=tmp.name)
        proc = subprocess.run([sys.executable, str(SCRIPTS / "doctor.py"), "--json"], env=env,
                              capture_output=True, text=True, timeout=60)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        out = json.loads(proc.stdout)
        self.assertEqual({c["name"] for c in out["checks"]},
                         {"python3", "node", "docker", "browseros", "rtk", "platform_supported",
                          "plugin_read_rule", "browseros_tools", "plugin_data", "path_wrappers",
                          "agent_memory_path", "subagent_model"})
        self.assertEqual(len(out["duplicates"]), 4)


if __name__ == "__main__":
    unittest.main()
