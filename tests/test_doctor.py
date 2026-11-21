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
        self.managed_settings = self.tmp / "managed-settings.json"
        for patcher in (mock.patch.object(doctor, "SYSTEM_APPLICATIONS", self.apps),
                        mock.patch.object(doctor, "MANAGED_MCP_PATHS", (self.managed,)),
                        mock.patch.object(doctor.compat, "MANAGED_SETTINGS_PATHS", (self.managed_settings,))):
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


def _run_probe(code, timeout=60):
    """Run `code` in a fresh interpreter (EnvCase mocks subprocess.run); `(returncode, stdout, stderr)`."""
    proc = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            universal_newlines=True)
    try:
        out, err = proc.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.communicate()
        raise AssertionError("the probe hung (blocked on a file it should not open)")
    return proc.returncode, out, err


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
        _write(self.config / "settings.json", {"enabledMcpjsonServers": ["browseros-neo"]})
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

    def test_the_browseros_check_accepts_any_loaded_scope(self):
        _write(self.cwd / ".mcp.json", {"mcpServers": {"x-browseros": {}}})
        _write(self.config / "settings.json", {"enableAllProjectMcpServers": True})
        self.assertTrue(self.checks()["browseros"]["ok"])
        self.assertIn("configured", self.checks()["browseros"]["detail"])


HOSTILE = "browseros\x1b[31m\nFAKE  ok   forged line\r\u202e"


class ProjectApprovalTest(CompatBase):
    """m14: a server in the repo's .mcp.json only counts when Claude Code would load it for this user."""

    def project(self, name="browseros"):
        _write(self.cwd / ".mcp.json", {"mcpServers": {name: {"url": "http://SECRETHOST/mcp"}}})

    def loaded(self):
        return {s["name"]: s.get("approved", True) for s in doctor.find_mcp_servers(dict(os.environ), self.cwd)}

    def test_unapproved_project_server_is_not_configured(self):
        self.project()
        self.assertEqual(self.loaded(), {"browseros": False})
        c = self.checks()["browseros"]
        self.assertFalse(c["ok"])
        self.assertIn("unapproved project", c["detail"])
        self.assertIn("browseros", c["detail"])
        self.assertNotIn("SECRETHOST", c["detail"])
        self.assertIn("install it", c["detail"])

    def test_unapproved_project_server_does_not_pass_browseros_tools(self):
        self.project("browseros-neo")
        c = self.checks()["browseros_tools"]
        self.assertIsNone(c["ok"])
        self.assertIn("unapproved project", c["detail"])

    def test_unapproved_server_next_to_an_installed_app_says_so(self):
        self.project()
        (self.apps / "BrowserOS neo.app").mkdir(parents=True)
        c = self.checks()["browseros"]
        self.assertTrue(c["ok"])
        self.assertIn("installed", c["detail"])
        self.assertIn("unapproved project", c["detail"])

    def test_approval_in_user_settings_by_name_or_wholesale(self):
        self.project()
        _write(self.config / "settings.json", {"enabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded(), {"browseros": True})
        self.assertTrue(self.checks()["browseros"]["ok"])
        self.assertIn("configured", self.checks()["browseros"]["detail"])
        _write(self.config / "settings.json", {"enabledMcpjsonServers": ["other"]})
        self.assertEqual(self.loaded(), {"browseros": False})
        _write(self.config / "settings.json", {"enableAllProjectMcpServers": True})
        self.assertEqual(self.loaded(), {"browseros": True})

    def trust(self, folder=None, value=True):
        """Record the trust dialog the way Claude Code does: .claude.json projects[<folder>]."""
        path = self.config / ".claude.json"
        data = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
        data.setdefault("projects", {}).setdefault(str(folder or self.cwd), {})["hasTrustDialogAccepted"] = value
        self.user_json(data)

    def test_approval_in_the_users_claude_json_needs_no_trust(self):
        self.project()
        self.user_json({"projects": {str(self.cwd): {"enabledMcpjsonServers": ["browseros"]}}})
        self.assertEqual(self.loaded(), {"browseros": True})
        self.user_json({"projects": {str(self.cwd): {"enableAllProjectMcpServers": True}}})
        self.assertEqual(self.loaded(), {"browseros": True})

    def test_local_settings_approve_only_in_a_trusted_workspace(self):
        self.project()
        _write(self.cwd / ".claude" / "settings.local.json", {"enabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded(), {"browseros": False})  # a clone may commit this file
        self.trust()
        self.assertEqual(self.loaded(), {"browseros": True})

    def test_shared_settings_approve_only_in_a_trusted_workspace(self):
        # Claude Code honours the shared .claude/settings.json once the folder is trusted, not before
        self.project()
        _write(self.cwd / ".claude" / "settings.json",
               {"enableAllProjectMcpServers": True, "enabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded(), {"browseros": False})
        self.trust()
        self.assertEqual(self.loaded(), {"browseros": True})
        _write(self.cwd / ".claude" / "settings.json", {"enabledMcpjsonServers": ["other"]})
        self.assertEqual(self.loaded(), {"browseros": False})
        _write(self.cwd / ".claude" / "settings.json", {"enableAllProjectMcpServers": True})
        self.assertEqual(self.loaded(), {"browseros": True})

    def test_trust_must_be_recorded_for_this_project_and_be_true(self):
        self.project()
        _write(self.cwd / ".claude" / "settings.json", {"enableAllProjectMcpServers": True})
        self.trust(self.tmp / "elsewhere")
        self.assertEqual(self.loaded(), {"browseros": False})
        self.trust(self.tmp)  # an ancestor's trust is not enough: undeterminable means untrusted
        self.assertEqual(self.loaded(), {"browseros": False})
        for odd in ("true", 1, "yes", None, [True]):
            self.trust(self.cwd, odd)
            self.assertEqual(self.loaded(), {"browseros": False}, odd)
        self.user_json("[]")
        self.assertEqual(self.loaded(), {"browseros": False})

    def git_project(self):
        """.mcp.json at the git root; returns a working folder below it."""
        self.project()
        (self.cwd / ".git").mkdir()
        sub = self.cwd / "pkg" / "app"
        sub.mkdir(parents=True)
        return sub

    def loaded_from(self, folder):
        return {s["name"]: s.get("approved", True)
                for s in doctor.find_mcp_servers(dict(os.environ), folder)}

    def test_repo_settings_are_read_from_the_working_folder_not_the_mcp_json_folder(self):
        sub = self.git_project()
        self.trust()  # the git root
        _write(self.config / "settings.json", {"enableAllProjectMcpServers": True})
        self.assertEqual(self.loaded_from(sub), {"browseros": True})
        _write(sub / ".claude" / "settings.json", {"disabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded_from(sub), {"browseros": False})
        (sub / ".claude" / "settings.json").unlink()
        _write(sub / ".claude" / "settings.local.json", {"disabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded_from(sub), {"browseros": False})

    def test_approval_in_the_working_folders_settings_not_the_mcp_json_folders(self):
        sub = self.git_project()
        self.trust()
        _write(self.cwd / ".claude" / "settings.local.json", {"enabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded_from(sub), {"browseros": False})  # Claude Code reads <cwd>/.claude
        _write(sub / ".claude" / "settings.local.json", {"enabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded_from(sub), {"browseros": True})

    def test_trust_is_recorded_for_the_git_root_of_the_working_folder(self):
        sub = self.git_project()
        _write(sub / ".claude" / "settings.json", {"enableAllProjectMcpServers": True})
        self.trust(sub)  # a subfolder key is not how Claude Code records trust
        self.assertEqual(self.loaded_from(sub), {"browseros": False})
        self.trust(self.cwd)  # the git root is
        self.assertEqual(self.loaded_from(sub), {"browseros": True})

    def test_trust_at_the_mcp_json_folder_below_the_git_root_does_not_count(self):
        (self.cwd / ".git").mkdir()
        mid = self.cwd / "pkg"
        sub = mid / "app"
        sub.mkdir(parents=True)
        _write(mid / ".mcp.json", {"mcpServers": {"browseros": {}}})
        _write(sub / ".claude" / "settings.json", {"enableAllProjectMcpServers": True})
        self.trust(mid)
        self.assertEqual(self.loaded_from(sub), {"browseros": False})
        self.trust(self.cwd)
        self.assertEqual(self.loaded_from(sub), {"browseros": True})

    def test_without_a_git_root_the_working_folder_is_the_trust_key(self):
        sub = self.cwd / "a"
        sub.mkdir()
        self.project()  # at self.cwd, no .git anywhere below tmp
        _write(sub / ".claude" / "settings.json", {"enableAllProjectMcpServers": True})
        self.trust(self.cwd)  # the folder holding .mcp.json is not the working folder
        self.assertEqual(self.loaded_from(sub), {"browseros": False})
        self.trust(sub)
        self.assertEqual(self.loaded_from(sub), {"browseros": True})

    def test_repo_settings_in_utf16_are_read(self):
        self.project()
        self.trust()
        path = self.cwd / ".claude" / "settings.json"
        path.parent.mkdir()
        path.write_bytes(b"\xff\xfe" + '{"disabledMcpjsonServers": ["browseros"]}'.encode("utf-16-le"))
        _write(self.config / "settings.json", {"enableAllProjectMcpServers": True})
        self.assertEqual(self.loaded(), {"browseros": False})

    def test_the_users_own_files_are_read_like_claude_code_does(self):
        # doctor's reader must be the lenient one: UTF-16 BOM and bad bytes in the user's own files
        self.project()
        flag = '{"enableAllProjectMcpServers": true}'
        path = self.config / "settings.json"
        path.write_bytes(b"\xff\xfe" + flag.encode("utf-16-le"))
        self.assertEqual(self.loaded(), {"browseros": True})
        path.write_bytes(b'{"x": "a\xff", "enableAllProjectMcpServers": true}')
        self.assertEqual(self.loaded(), {"browseros": True})
        path.unlink()
        (self.config / ".claude.json").write_bytes(b"\xff\xfe" + json.dumps(
            {"mcpServers": {"browseros-neo": {}}}).encode("utf-16-le"))
        self.assertEqual(self.loaded(), {"browseros-neo": True, "browseros": False})

    def test_user_settings_approve_without_trust(self):
        self.project()
        _write(self.config / "settings.json", {"enableAllProjectMcpServers": True})
        self.assertEqual(self.loaded(), {"browseros": True})

    def test_a_server_disabled_in_any_source_is_never_approved(self):
        self.project()
        self.trust()
        _write(self.config / "settings.json", {"enableAllProjectMcpServers": True})
        for where in (self.cwd / ".claude" / "settings.json", self.cwd / ".claude" / "settings.local.json"):
            self.assertEqual(self.loaded(), {"browseros": True})
            _write(where, {"disabledMcpjsonServers": ["browseros"]})
            self.assertEqual(self.loaded(), {"browseros": False}, where.name)
            where.unlink()
        self.user_json({"projects": {str(self.cwd): {"hasTrustDialogAccepted": True,
                                                     "disabledMcpjsonServers": ["browseros"]}}})
        self.assertEqual(self.loaded(), {"browseros": False})

    def test_disabled_in_shared_settings_wins_even_without_trust(self):
        self.project()
        _write(self.config / "settings.json", {"enabledMcpjsonServers": ["browseros"]})
        _write(self.cwd / ".claude" / "settings.json", {"disabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded(), {"browseros": False})

    def test_oversized_repo_settings_approve_and_disable_nothing(self):
        self.project()
        self.trust()
        pad = "A" * (doctor.compat.REPO_READ_LIMIT + 10)
        _write(self.cwd / ".claude" / "settings.json", {"enableAllProjectMcpServers": True, "pad": pad})
        self.assertEqual(self.loaded(), {"browseros": False})

    def test_disabled_wins_over_enabled(self):
        self.project()
        _write(self.config / "settings.json", {"enableAllProjectMcpServers": True,
                                               "disabledMcpjsonServers": ["browseros"]})
        self.assertEqual(self.loaded(), {"browseros": False})

    def test_non_project_scopes_are_not_gated(self):
        self.user_json({"mcpServers": {"browseros-neo": {}}})
        _write(self.managed, {"mcpServers": {"corp-browseros": {}}})
        self.assertEqual(self.loaded(), {"browseros-neo": True, "corp-browseros": True})
        self.assertTrue(self.checks()["browseros"]["ok"])

    def test_garbage_approval_lists_are_unapproved(self):
        self.project()
        for bad in ("browseros", {"browseros": True}, [3, None], 7):
            _write(self.config / "settings.json", {"enabledMcpjsonServers": bad})
            self.assertEqual(self.loaded(), {"browseros": False}, bad)


class HostileNamesTest(CompatBase):
    """m14: names from repo files never reach the terminal raw."""

    def test_wrong_name_detail_is_escaped(self):
        self.user_json({"mcpServers": {HOSTILE: {"url": "http://SECRETHOST"}}})
        c = self.checks()["browseros_tools"]
        self.assertFalse(c["ok"])
        for bad in ("\x1b", "\n", "\r", "\u202e"):
            self.assertNotIn(bad, c["detail"])
        self.assertIn("\\x1b[31m", c["detail"])
        self.assertIn("\\n", c["detail"])
        self.assertTrue(c["detail"].isprintable())
        self.assertNotIn("SECRETHOST", c["detail"])

    def test_unapproved_name_is_escaped_too(self):
        _write(self.cwd / ".mcp.json", {"mcpServers": {HOSTILE: {}}})
        for name in ("browseros", "browseros_tools"):
            self.assertTrue(self.checks()[name]["detail"].isprintable(), name)

    def test_only_the_first_few_servers_are_listed(self):
        self.user_json({"mcpServers": {"browseros-%d" % i: {} for i in range(9)}})
        c = self.checks()["browseros_tools"]
        self.assertIn("and 4 more", c["detail"])
        self.assertEqual(c["detail"].count("scope, tools"), doctor.SHOWN_SERVERS)

    def test_only_the_first_few_unapproved_servers_are_listed(self):
        _write(self.cwd / ".mcp.json", {"mcpServers": {"browseros-%d" % i: {} for i in range(9)}})
        c = self.checks()["browseros_tools"]
        self.assertIn("and 4 more", c["detail"])
        self.assertNotIn("browseros-8", c["detail"])

    def test_a_very_long_name_is_cut(self):
        self.user_json({"mcpServers": {"browseros" + "x" * 5000: {}}})
        self.assertLess(len(self.checks()["browseros_tools"]["detail"]), 600)

    def test_text_mode_prints_one_line_per_check_and_no_control_characters(self):
        import contextlib
        import io
        self.user_json({"mcpServers": {HOSTILE: {}}})
        _write(self.cwd / ".mcp.json", {"mcpServers": {HOSTILE + "2": {}}})
        buf = io.StringIO()
        with mock.patch.object(doctor.os, "getcwd", return_value=str(self.cwd)):
            with contextlib.redirect_stdout(buf):
                doctor.main([])
        lines = buf.getvalue().splitlines()
        self.assertNotIn("\x1b", buf.getvalue())
        self.assertNotIn("\u202e", buf.getvalue())
        names = [l.split()[0] for l in lines if not l.startswith("duplicate:")]
        self.assertEqual(names, list(doctor.CHECKS))


class PercentInNamesTest(CompatBase):
    """Pre-existing: a `%` in a plugin key or server name must not break the tool-prefix format."""

    def test_plugin_key_with_a_percent_sign(self):
        plug = self.tmp / "bp"
        _write(plug / ".mcp.json", {"mcpServers": {"browseros-neo": {}}})
        _write(self.config / "plugins" / "installed_plugins.json",
               {"plugins": {"b%s%d@m": [{"installPath": str(plug)}]}})
        found = [s for s in doctor.find_mcp_servers(dict(os.environ), self.cwd) if s["scope"] == "plugin"]
        self.assertEqual([s["tool_prefix"] for s in found], ["mcp__plugin_b%s%d_browseros-neo__"])
        c = self.checks()
        self.assertFalse(c["browseros_tools"]["detail"].startswith("check failed"), c["browseros_tools"])
        self.assertFalse(c["browseros"]["detail"].startswith("check failed"), c["browseros"])

    def test_server_name_with_a_percent_sign(self):
        self.user_json({"mcpServers": {"browseros%s%": {}}})
        found = doctor.find_mcp_servers(dict(os.environ), self.cwd)
        self.assertEqual(found[0]["tool_prefix"], "mcp__browseros%s%__")
        self.assertFalse(self.checks()["browseros_tools"]["detail"].startswith("check failed"))


class NestedInstalledPluginsTest(CompatBase):
    """Pre-existing: a deeply nested installed_plugins.json must not crash doctor."""

    def nest(self):
        _write(self.config / "plugins" / "installed_plugins.json", "[" * 200000 + "]" * 200000)

    def run_main(self, *argv):
        import contextlib
        import io
        buf = io.StringIO()
        with mock.patch.object(doctor.os, "getcwd", return_value=str(self.cwd)):
            with contextlib.redirect_stdout(buf):
                code = doctor.main(list(argv))
        return code, buf.getvalue()

    def test_text_mode_reports_unreadable(self):
        self.nest()
        code, out = self.run_main()
        self.assertEqual(code, 0)
        self.assertIn("installed_plugins.json is unreadable", out)

    def test_json_mode_reports_unreadable_and_no_duplicates(self):
        self.nest()
        code, out = self.run_main("--json")
        data = json.loads(out)
        self.assertEqual(data["duplicates"], [])
        self.assertEqual(data["installed_plugins"], "unreadable")

    def test_malformed_and_missing_files(self):
        _write(self.config / "plugins" / "installed_plugins.json", "{nope")
        self.assertEqual(json.loads(self.run_main("--json")[1])["installed_plugins"], "unreadable")
        (self.config / "plugins" / "installed_plugins.json").unlink()
        data = json.loads(self.run_main("--json")[1])
        self.assertEqual((data["installed_plugins"], data["duplicates"]), ("missing", []))
        self.assertNotIn("unreadable", self.run_main()[1])

    def test_readable_file_is_ok(self):
        _write(self.config / "plugins" / "installed_plugins.json", INSTALLED)
        data = json.loads(self.run_main("--json")[1])
        self.assertEqual((data["installed_plugins"], len(data["duplicates"])), ("ok", 4))

    def test_a_nested_user_settings_file_does_not_crash_the_approval_check(self):
        # a valid project .mcp.json makes doctor read the (nested) user settings.json for approvals
        _write(self.cwd / ".mcp.json", {"mcpServers": {"browseros": {}}})
        _write(self.config / "settings.json", "[" * 200000 + "]" * 200000)
        c = self.checks()
        self.assertFalse(c["browseros"]["detail"].startswith("check failed"), c["browseros"])
        self.assertFalse(c["browseros_tools"]["detail"].startswith("check failed"))
        self.assertIn("unapproved", c["browseros"]["detail"])

    def test_nested_mcp_and_settings_files_do_not_crash_the_checks(self):
        self.nest()
        _write(self.cwd / ".mcp.json", "[" * 200000)
        _write(self.cwd / ".claude" / "settings.local.json", "[" * 200000)
        _write(self.config / "settings.json", "[" * 200000)
        self.assertFalse([c for c in self.checks().values() if c["detail"].startswith("check failed")])


class DuplicateScopeTest(unittest.TestCase):
    """Pre-existing: an unvalidated scope must not become part of a runnable command."""

    def found(self, scope, name="superpowers@m"):
        return doctor.find_duplicates({"plugins": {name: [{"scope": scope, "installPath": "/p"}]}})

    def test_known_scopes_get_the_command(self):
        for scope in ("user", "project", "local"):
            self.assertEqual(self.found(scope)[0]["command"],
                             "claude plugin uninstall superpowers@m -s " + scope)

    def test_unknown_scope_gets_no_command_and_is_escaped(self):
        for scope in ("user; touch x", "x\x1b[31m\nfake", "", " user", "USER", "managed", 3, ["user"]):
            (d,) = self.found(scope)
            self.assertIsNone(d["command"], scope)
            self.assertTrue(str(d["scope"]).isprintable(), scope)

    def test_the_escaped_scope_shows_the_odd_value(self):
        (d,) = self.found("x\x1b\nz")
        self.assertIn("\\x1b", d["scope"])
        self.assertIn("\\n", d["scope"])

    def test_a_plugin_name_with_shell_characters_gets_no_command(self):
        for name in ("superpowers@m; touch x", "superpowers@m$(id)", "superpowers@m x", "superpowers@m\nz"):
            (d,) = self.found("user", name)
            self.assertIsNone(d["command"], name)
            self.assertTrue(d["plugin"].isprintable(), name)

    def test_text_mode_prints_no_runnable_command_for_an_odd_scope(self):
        import contextlib
        import io
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        config = Path(tmp.name)
        _write(config / "plugins" / "installed_plugins.json", {"plugins": {
            "superpowers@m": [{"scope": "user; touch x"}, {"scope": "user"}]}})
        env = dict(os.environ, CLAUDE_CONFIG_DIR=str(config), HOME=tmp.name)
        buf = io.StringIO()
        with mock.patch.dict(os.environ, env), mock.patch.object(doctor, "check_env", return_value=[
                {"name": "platform_supported", "ok": True, "detail": "x"}]):
            with contextlib.redirect_stdout(buf):
                doctor.main([])
        out = buf.getvalue()
        self.assertEqual(out.count("claude plugin uninstall"), 1)
        self.assertIn("touch x", out)  # shown, escaped, with no command


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
        self.assertIn("/anthology:setup", c["detail"])
        self.assertNotIn("never changes it", c["detail"])
        self.assertIn("opus/sonnet/haiku", c["detail"])

    def test_empty_env_value_is_unset(self):
        self.assertTrue(self.check("subagent_model", {"CLAUDE_CODE_SUBAGENT_MODEL": ""})["ok"])

    def test_set_only_in_user_settings(self):
        _write(self.config / "settings.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "SECRETMODEL"}})
        c = self.checks()["subagent_model"]
        self.assertFalse(c["ok"])
        self.assertIn("settings.json", c["detail"])
        self.assertNotIn("SECRETMODEL", c["detail"])

    def test_process_env_copied_from_a_settings_file_names_only_the_file(self):
        # Claude Code copies a settings file's env into the commands it runs
        _write(self.config / "settings.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}})
        sources = doctor.compat.subagent_model_sources({**os.environ, "CLAUDE_CODE_SUBAGENT_MODEL": "m"}, self.cwd)
        self.assertEqual(sources, ["user settings.json"])
        sources = doctor.compat.subagent_model_sources({**os.environ, "CLAUDE_CODE_SUBAGENT_MODEL": "x"}, self.cwd)
        self.assertEqual(sources, ["user settings.json", "the process environment"])

    def test_every_place_is_listed_in_order(self):
        for path in (self.config / "settings.json", self.cwd / ".claude" / "settings.local.json",
                     self.managed_settings):
            _write(path, {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}})
        self.assertEqual(doctor.compat.subagent_model_sources(dict(os.environ), self.cwd),
                         ["user settings.json", "the project's .claude/settings.local.json", "managed settings"])
        c = self.checks()["subagent_model"]
        self.assertIn("(user settings.json, the project's .claude/settings.local.json, managed settings)",
                      c["detail"])
        for label in doctor.compat.subagent_model_sources(dict(os.environ), self.cwd):
            self.assertIn("CLAUDE_CODE_SUBAGENT_MODEL", doctor.compat.SUBAGENT_MODEL_HINTS[label])

    def test_run_in_the_home_folder_the_user_file_is_listed_once(self):
        env = {**os.environ, "CLAUDE_CONFIG_DIR": str(self.cwd / ".claude")}
        _write(self.cwd / ".claude" / "settings.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}})
        self.assertEqual(doctor.compat.subagent_model_sources(env, self.cwd), ["user settings.json"])

    def test_set_in_managed_settings(self):
        _write(self.managed_settings, {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "SECRETMODEL"}})
        c = self.checks()["subagent_model"]
        self.assertFalse(c["ok"])
        self.assertIn("managed settings", c["detail"])
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

    def test_oversized_repo_settings_are_skipped_like_an_absent_file(self):
        pad = "A" * (doctor.compat.REPO_READ_LIMIT + 10)
        for name in ("settings.json", "settings.local.json"):
            _write(self.cwd / ".claude" / name, {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}, "pad": pad})
        self.assertTrue(self.checks()["subagent_model"]["ok"])

    def test_repo_settings_just_under_the_cap_are_read(self):
        text = json.dumps({"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}, "pad": ""})
        pad = "A" * (doctor.compat.REPO_READ_LIMIT - len(text.encode("utf-8")))
        _write(self.cwd / ".claude" / "settings.json", json.dumps(
            {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}, "pad": pad}))
        self.assertEqual(os.path.getsize(str(self.cwd / ".claude" / "settings.json")),
                         doctor.compat.REPO_READ_LIMIT)
        self.assertFalse(self.checks()["subagent_model"]["ok"])

    def test_the_users_own_settings_are_not_capped(self):
        pad = "A" * (doctor.compat.REPO_READ_LIMIT + 10)
        _write(self.config / "settings.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}, "pad": pad})
        self.assertFalse(self.checks()["subagent_model"]["ok"])

    def test_the_repo_limit_is_claude_codes_own_two_mib(self):
        self.assertEqual(doctor.compat.REPO_READ_LIMIT, 2 * 1024 * 1024)

    @unittest.skipUnless(hasattr(os, "mkfifo"), "needs mkfifo")
    def test_a_fifo_with_valid_json_is_refused_though_it_reads_fine(self):
        # only the regular-file check can refuse this: the data is small and valid
        fifo = str(self.tmp / "settings.json")
        os.mkfifo(fifo)
        fd = os.open(fifo, os.O_RDWR | os.O_NONBLOCK)
        self.addCleanup(os.close, fd)
        os.write(fd, b'{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}')
        self.assertIsNone(doctor.compat.read_json(fifo, doctor.compat.REPO_READ_LIMIT))

    def test_settings_are_decoded_like_claude_code(self):
        flag = '{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}'
        (self.cwd / ".claude").mkdir()
        path = self.cwd / ".claude" / "settings.json"
        for raw in (b"\xff\xfe" + flag.encode("utf-16-le"), b"\xfe\xff" + flag.encode("utf-16-be"),
                    b"\xef\xbb\xbf" + flag.encode("utf-8"),
                    flag[:-2].encode("utf-8") + b', "x": "a\xff"}}'):
            path.write_bytes(raw)
            self.assertFalse(self.checks()["subagent_model"]["ok"], raw[:6])

    def test_a_symlink_to_a_regular_file_is_still_read(self):
        real = _write(self.tmp / "real.json", {"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}})
        (self.cwd / ".claude").mkdir()
        os.symlink(str(real), str(self.cwd / ".claude" / "settings.json"))
        self.assertFalse(self.checks()["subagent_model"]["ok"])

    @unittest.skipUnless(os.path.exists("/dev/zero"), "needs /dev/zero")
    def test_a_device_symlink_is_skipped_without_reading_it(self):
        (self.cwd / ".claude").mkdir()
        os.symlink("/dev/zero", str(self.cwd / ".claude" / "settings.json"))
        os.symlink("/dev/zero", str(self.cwd / ".mcp.json"))
        probe = ("import resource, sys\n"
                 "try:\n    resource.setrlimit(resource.RLIMIT_AS, (1 << 30, 1 << 30))\n"
                 "except (ValueError, OSError):\n    sys.exit(77)\n"
                 "sys.path.insert(0, %r)\nimport compat, doctor\n"
                 "print(compat.subagent_model_source({}, %r), doctor.find_mcp_servers({'HOME': %r}, %r))\n"
                 % (str(SCRIPTS), str(self.cwd), str(self.tmp), str(self.cwd)))
        code, out, err = _run_probe(probe)
        if code == 77:
            self.skipTest("cannot limit memory here")
        self.assertEqual(code, 0, err[-300:])
        self.assertEqual(out.strip(), "None []")

    @unittest.skipUnless(hasattr(os, "mkfifo"), "needs mkfifo")
    def test_a_fifo_is_skipped_without_blocking(self):
        (self.cwd / ".claude").mkdir()
        os.mkfifo(str(self.cwd / ".claude" / "settings.json"))
        probe = ("import sys\nsys.path.insert(0, %r)\nimport compat\n"
                 "print(compat.subagent_model_source({}, %r))\n" % (str(SCRIPTS), str(self.cwd)))
        code, out, err = _run_probe(probe, timeout=20)
        self.assertEqual(out.strip(), "None", err[-300:])

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
