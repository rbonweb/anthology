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


class CheckEnvTest(unittest.TestCase):
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
        exists = mock.patch.object(doctor, "BROWSEROS_APP", self.tmp / "Applications" / "BrowserOS.app")
        exists.start()
        self.addCleanup(exists.stop)

    def checks(self):
        return {c["name"]: c for c in doctor.check_env(dict(os.environ))}

    def test_shape_and_order(self):
        checks = doctor.check_env(dict(os.environ))
        self.assertEqual([c["name"] for c in checks],
                         ["python3", "node", "docker", "browseros", "rtk", "platform_supported",
                          "plugin_read_rule"])
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
        (self.tmp / "Applications" / "BrowserOS.app").mkdir(parents=True)
        self.assertTrue(self.checks()["browseros"]["ok"])
        (self.tmp / "Applications" / "BrowserOS.app").rmdir()
        self.tools.add("browseros")
        self.assertTrue(self.checks()["browseros"]["ok"])

    def test_browseros_is_never_offered_as_an_install(self):
        detail = self.checks()["browseros"]["detail"]
        self.assertNotRegex(detail.lower(), r"brew|install chrome|download")

    def test_claude_json_in_home_is_used_without_config_dir(self):
        env = {"HOME": str(self.tmp), "PATH": "/bin"}
        (self.tmp / ".claude.json").write_text(json.dumps({"mcpServers": {"my-browseros": {}}}), encoding="utf-8")
        checks = {c["name"]: c for c in doctor.check_env(env)}
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

    def test_rtk_verified_against_the_lock(self):
        binary = self.tmp / "rtk"
        binary.write_text("x", encoding="utf-8")
        state = self.config / "anthology"
        state.mkdir()
        (state / "state.json").write_text(json.dumps({"rtk_path": str(binary)}), encoding="utf-8")
        lock = self.tmp / "rtk.lock"
        lock.write_text(json.dumps({"version": "1", "targets": {"linux-x86": {"sha256": "ab" * 32}}}),
                        encoding="utf-8")
        fake = types.SimpleNamespace(target=lambda system, machine: "linux-x86",
                                     verify=lambda path, sha: sha == "ab" * 32)
        with mock.patch.dict(sys.modules, {"rtk_install": fake}):
            c = {x["name"]: x for x in doctor.check_env(dict(os.environ), lock=lock)}["rtk"]
            self.assertTrue(c["ok"])
            fake.verify = lambda path, sha: False
            c = {x["name"]: x for x in doctor.check_env(dict(os.environ), lock=lock)}["rtk"]
            self.assertFalse(c["ok"])
            self.assertIn("setup", c["detail"])

    def test_rtk_missing_binary_is_not_ok(self):
        lock = self.tmp / "rtk.lock"
        lock.write_text("{}", encoding="utf-8")
        fake = types.SimpleNamespace(target=lambda s, m: "t", verify=lambda p, h: True)
        with mock.patch.dict(sys.modules, {"rtk_install": fake}):
            c = {x["name"]: x for x in doctor.check_env(dict(os.environ), lock=lock)}["rtk"]
        self.assertFalse(c["ok"])

    def plugin_rule(self, **kw):
        return {c["name"]: c for c in doctor.check_env(dict(os.environ), plugin_root=PLUGIN, **kw)}["plugin_read_rule"]

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
            checks = doctor.check_env(dict(os.environ))
        self.assertEqual(len(checks), 7)
        self.assertFalse(checks[0]["ok"])


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
                          "plugin_read_rule"})
        self.assertEqual(len(out["duplicates"]), 4)


if __name__ == "__main__":
    unittest.main()
