"""Tests for scripts/guardrails_loc.py: the guardrails folder is the one the hook really runs (P2/B2).

A `directory` marketplace loads its plugins in place; `installed_plugins.json` still names an
install-time snapshot in the plugin cache that never runs. A GitHub marketplace runs the cache.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import unittest

from _mem import ROOT, SCRIPTS as SCRIPTS_DIR, MemCase, write

import guardrails_loc  # noqa: E402
from test_session_start import PLUGIN, SessionCase  # noqa: E402

REAL_GUARDRAILS = ROOT / "plugins" / "anthology-guardrails"
EXIT_0 = "import sys\nsys.exit(0)\n"
MARKER = '\nPROFILE_ROOTS = tuple(PROFILE_ROOTS) + (".config/%s",)\n'


class LiveRootCase(MemCase):
    def setUp(self):
        super().setUp()
        self.cache = self.config / "plugins" / "cache" / "anthology" / "anthology-guardrails" / "0.1.0"
        self.market = self.tmp / "market"
        self.live = self.market / "plugins" / "anthology-guardrails"

    def copy_guardrails(self, dest, guard=None, marker=None):
        shutil.copytree(str(REAL_GUARDRAILS), str(dest))
        if guard is not None:
            write(dest / "scripts" / "guard.py", guard)
        if marker:
            with open(str(dest / "scripts" / "guard_rules.py"), "a", encoding="utf-8") as fh:
                fh.write(MARKER % marker)
        return dest

    def installed(self, install_path=None, extra=None):
        entry = {"scope": "user", "installPath": str(install_path or self.cache),
                 "version": "0.1.0", "lastUpdated": "2026-01-01T00:00:00Z"}
        plugins = {"anthology-guardrails@anthology": [entry]}
        plugins.update(extra or {})
        write(self.config / "plugins" / "installed_plugins.json", json.dumps({"version": 2, "plugins": plugins}))

    def known(self, source, location=None):
        write(self.config / "plugins" / "known_marketplaces.json", json.dumps({"anthology": {
            "source": source, "installLocation": str(location or self.market), "lastUpdated": "x"}}))

    def directory_known(self):
        self.known({"source": "directory", "path": str(self.market)})

    def manifest(self, source="./plugins/anthology-guardrails"):
        write(self.market / ".claude-plugin" / "marketplace.json", json.dumps({
            "name": "anthology", "plugins": [{"name": "anthology-guardrails", "source": source}]}))

    def inspect(self, plugin_root=PLUGIN):
        return guardrails_loc.inspect(plugin_root, {"CLAUDE_CONFIG_DIR": str(self.config)}, str(self.tmp))

    def diverging(self, live_guard=None, cache_guard=EXIT_0):
        """The cache copy and the in-place marketplace copy differ (guard.py and PROFILE_ROOTS)."""
        self.copy_guardrails(self.cache, guard=cache_guard, marker="cache-marker")
        self.copy_guardrails(self.live, guard=live_guard, marker="live-marker")
        self.manifest()
        self.directory_known()
        self.installed()


class ResolutionTest(LiveRootCase):
    def test_directory_marketplace_resolves_to_the_marketplace_copy(self):
        self.diverging()
        self.assertEqual(self.inspect(), (self.live, None))

    def test_github_marketplace_uses_install_path(self):
        self.diverging()
        self.known({"source": "github", "repo": "owner/anthology", "ref": "build"},
                   location=self.config / "plugins" / "marketplaces" / "anthology")
        self.assertEqual(self.inspect(), (self.cache, None))

    def test_other_source_types_use_install_path(self):
        self.diverging()
        for source in ({"source": "git", "url": "https://example.invalid/x.git"},
                       {"source": "url", "url": "https://example.invalid/m.json"}):
            self.known(source)
            self.assertEqual(self.inspect(), (self.cache, None), source)

    def test_directory_marketplace_with_a_symlinked_install_location(self):
        self.diverging()
        link = self.tmp / "link"
        os.symlink(str(self.market), str(link))
        self.known({"source": "directory", "path": str(link)}, location=link)
        found, problem = self.inspect()
        self.assertIsNone(problem)
        self.assertEqual(os.path.realpath(str(found)), os.path.realpath(str(self.live)))


class FallbackTest(LiveRootCase):
    def sibling_case(self):
        base = self.tmp / "plugins-dir"
        root = base / "anthology"
        root.mkdir(parents=True)
        sibling = self.copy_guardrails(base / "anthology-guardrails", marker="sibling")
        self.copy_guardrails(self.cache, guard=EXIT_0)
        self.installed()
        return root, sibling

    def test_missing_known_marketplaces_outside_the_cache_uses_the_sibling(self):
        root, sibling = self.sibling_case()
        self.assertEqual(self.inspect(root), (sibling, None))

    def test_garbage_known_marketplaces_outside_the_cache_uses_the_sibling(self):
        root, sibling = self.sibling_case()
        for text in ("{not json", "[]", '{"anthology": 3}', '{"anthology": {"source": "directory"}}', ""):
            write(self.config / "plugins" / "known_marketplaces.json", text)
            self.assertEqual(self.inspect(root), (sibling, None), text)

    def test_directory_marketplace_with_an_unusable_manifest_is_a_problem_not_the_sibling(self):
        root, sibling = self.sibling_case()
        self.directory_known()
        # a directory marketplace never runs the cache copy: an unusable manifest is a problem, not a fallback
        self.assertEqual(self.inspect(root), (None, "live_missing"))  # no marketplace.json
        write(self.market / ".claude-plugin" / "marketplace.json", "{broken")
        self.assertEqual(self.inspect(root), (None, "live_missing"))
        write(self.market / ".claude-plugin" / "marketplace.json", '{"plugins": "x"}')
        self.assertEqual(self.inspect(root), (None, "live_missing"))
        write(self.market / ".claude-plugin" / "marketplace.json", '{"plugins": []}')  # not listed
        self.assertEqual(self.inspect(root), (None, "live_missing"))

    def test_plugin_root_inside_the_cache_uses_install_path(self):
        self.copy_guardrails(self.cache, guard=EXIT_0)
        in_cache = self.config / "plugins" / "cache" / "anthology" / "anthology" / "0.1.0"
        in_cache.mkdir(parents=True)
        # a sibling-looking folder exists next to it, but a cached plugin is not loaded in place
        self.copy_guardrails(in_cache.parent / "anthology-guardrails", marker="decoy")
        self.installed()
        self.assertEqual(self.inspect(in_cache), (self.cache, None))

    def test_outside_the_cache_without_a_sibling_uses_install_path(self):
        self.copy_guardrails(self.cache, guard=EXIT_0)
        self.installed()
        lone = self.tmp / "lone" / "anthology"
        lone.mkdir(parents=True)
        self.assertEqual(self.inspect(lone), (self.cache, None))

    def test_dev_session_without_registry_entries_uses_the_sibling(self):
        base = self.tmp / "dev"
        (base / "anthology").mkdir(parents=True)
        sibling = self.copy_guardrails(base / "anthology-guardrails")
        self.assertEqual(self.inspect(base / "anthology"), (sibling, None))

    def test_problems_keep_their_meaning(self):
        self.assertEqual(self.inspect(self.tmp / "nowhere" / "anthology"), (None, "not_found"))
        write(self.config / "plugins" / "installed_plugins.json", json.dumps({"version": 2, "plugins": {
            "anthology@anthology": [{"scope": "user", "installPath": str(PLUGIN)}]}}))
        self.assertEqual(self.inspect(), (None, "not_installed"))
        self.copy_guardrails(self.cache)
        self.installed()
        write(self.config / "settings.json", json.dumps({"enabledPlugins": {"anthology-guardrails@anthology": False}}))
        self.assertEqual(self.inspect(self.tmp / "lone" / "anthology"), (self.cache, "disabled"))


class EscapingSourceTest(LiveRootCase):
    def assert_not_accepted(self, source):
        if not self.cache.exists():
            self.copy_guardrails(self.cache, guard=EXIT_0)
        self.directory_known()
        self.manifest(source)
        self.installed()
        # the cache copy is never what a directory marketplace runs: a refused source is a problem
        self.assertEqual(self.inspect(self.tmp / "lone" / "anthology"), (None, "live_missing"), source)

    def test_parent_escape_is_refused(self):
        # a real guard.py sits at each escape target, so only the containment check can refuse it
        self.copy_guardrails(self.tmp / "outside")
        self.assert_not_accepted("../outside")
        self.assert_not_accepted("./plugins/../../outside")
        self.assert_not_accepted("../../etc")

    def test_absolute_source_is_refused(self):
        outside = self.copy_guardrails(self.tmp / "abs-outside")
        self.assert_not_accepted(str(outside))

    def test_symlink_escape_is_refused(self):
        outside = self.copy_guardrails(self.tmp / "sym-outside")
        (self.market / "plugins").mkdir(parents=True)
        os.symlink(str(outside), str(self.market / "plugins" / "anthology-guardrails"))
        self.assert_not_accepted("./plugins/anthology-guardrails")

    def test_symlink_escape_is_not_undone_by_the_sibling_fallback(self):
        # the in-place layout: <market>/plugins/anthology next to an escaping anthology-guardrails link
        outside = self.copy_guardrails(self.tmp / "sym-outside")
        plugins = self.market / "plugins"
        (plugins / "anthology").mkdir(parents=True)
        os.symlink(str(outside), str(plugins / "anthology-guardrails"))
        self.copy_guardrails(self.cache, guard=EXIT_0)
        self.directory_known()
        self.manifest("./plugins/anthology-guardrails")
        self.installed()
        self.assertEqual(self.inspect(plugins / "anthology"), (None, "live_missing"))
        os.remove(str(self.config / "plugins" / "known_marketplaces.json"))  # the plain sibling fallback
        self.assertEqual(self.inspect(plugins / "anthology"), (self.cache, None))

    def test_sibling_link_to_the_folder_itself_is_refused(self):
        # <plugins>/anthology-guardrails -> <plugins>, which itself has scripts/guard.py
        plugins = self.tmp / "dev"
        (plugins / "anthology").mkdir(parents=True)
        write(plugins / "scripts" / "guard.py", EXIT_0)
        os.symlink(str(plugins), str(plugins / "anthology-guardrails"))
        self.assertEqual(self.inspect(plugins / "anthology"), (None, "not_found"))

    def test_dev_session_does_not_follow_an_escaping_sibling_link(self):
        outside = self.copy_guardrails(self.tmp / "sym-outside")
        (self.tmp / "dev" / "anthology").mkdir(parents=True)
        os.symlink(str(outside), str(self.tmp / "dev" / "anthology-guardrails"))
        self.assertEqual(self.inspect(self.tmp / "dev" / "anthology"), (None, "not_found"))

    def test_source_of_the_wrong_type_is_refused(self):
        for source in ({"source": "github", "repo": "a/b"}, 5, "", None):
            self.assert_not_accepted(source)


class LiveMissingTest(LiveRootCase):
    """A listed in-place folder without guard.py is what the hook runs: never the cache copy instead."""

    def test_live_folder_without_guard_py_is_live_missing_not_the_cache(self):
        self.copy_guardrails(self.cache, guard=EXIT_0)
        (self.live / "scripts").mkdir(parents=True)
        self.manifest()
        self.directory_known()
        self.installed()
        self.assertEqual(self.inspect(), (None, "live_missing"))
        self.assertEqual(self.inspect(self.market / "plugins" / "anthology"), (None, "live_missing"))

    def test_live_folder_that_does_not_exist_is_live_missing(self):
        self.copy_guardrails(self.cache, guard=EXIT_0)
        self.manifest()
        self.directory_known()
        self.installed()
        self.assertEqual(self.inspect(), (None, "live_missing"))

    def test_disabled_takes_priority_over_live_missing(self):
        self.copy_guardrails(self.cache, guard=EXIT_0)
        (self.live / "scripts").mkdir(parents=True)
        self.manifest()
        self.directory_known()
        self.installed()
        write(self.config / "settings.json",
              json.dumps({"enabledPlugins": {"anthology-guardrails@anthology": False}}))
        self.assertEqual(self.inspect(), (None, "disabled"))

    def test_unlisted_plugin_and_missing_manifest_are_live_missing(self):
        self.copy_guardrails(self.cache, guard=EXIT_0)
        self.directory_known()
        self.installed()
        self.assertEqual(self.inspect(), (None, "live_missing"))  # no marketplace.json
        write(self.market / ".claude-plugin" / "marketplace.json", json.dumps({
            "name": "anthology", "plugins": [{"name": "other", "source": "./plugins/other"}]}))
        self.assertEqual(self.inspect(), (None, "live_missing"))

    def test_load_rules_and_setup_never_take_rules_from_the_cache(self):
        import settings_patch
        self.copy_guardrails(self.cache, marker="cache-marker")
        (self.live / "scripts").mkdir(parents=True)
        self.manifest()
        self.directory_known()
        self.installed()
        env = {"CLAUDE_CONFIG_DIR": str(self.config)}
        with self.assertRaises(FileNotFoundError):
            guardrails_loc.load_rules(PLUGIN, env, str(self.tmp))
        with self.assertRaises(FileNotFoundError):
            settings_patch.setup_ops(PLUGIN, env=env)

    def test_live_folder_with_guard_py_is_unaffected(self):
        self.diverging()
        self.assertEqual(self.inspect(), (self.live, None))


class CacheSnapshotGoneTest(LiveRootCase):
    """m13: a directory marketplace runs the live folder, so a missing cache snapshot is irrelevant."""

    def installed(self, install_path=None, extra=None):
        # anthology itself is installed too, so "no usable guardrails entry" is not a dev session
        extra = dict(extra or {}, **{"anthology@anthology": [{
            "scope": "user", "installPath": str(self.tmp / "anthology"), "version": "0.1.0"}]})
        super().installed(install_path, extra)

    def live_only(self, cache_dir=False):
        self.copy_guardrails(self.live, marker="live-marker")
        if cache_dir:
            (self.cache / "scripts").mkdir(parents=True)  # the snapshot exists but has no guard.py
        self.manifest()
        self.directory_known()
        self.installed()

    def test_missing_cache_folder_still_resolves_to_the_live_folder(self):
        self.live_only()
        self.assertEqual(self.inspect(), (self.live, None))

    def test_cache_folder_without_guard_py_still_resolves_to_the_live_folder(self):
        self.live_only(cache_dir=True)
        self.assertEqual(self.inspect(), (self.live, None))

    def test_neither_folder_has_guard_py_is_not_installed(self):
        (self.live / "scripts").mkdir(parents=True)
        self.manifest()
        self.directory_known()
        self.installed()
        self.assertEqual(self.inspect(), (None, "not_installed"))

    def test_cache_gone_with_a_non_directory_marketplace_is_not_installed(self):
        self.copy_guardrails(self.live)
        self.manifest()
        self.known({"source": "github", "repo": "owner/anthology"})
        self.installed()
        self.assertEqual(self.inspect(), (None, "not_installed"))

    def test_cache_gone_and_live_source_escaping_is_not_installed_not_the_cache(self):
        outside = self.tmp / "outside" / "anthology-guardrails"
        self.copy_guardrails(outside)
        self.manifest(source="../outside/anthology-guardrails")
        self.directory_known()
        self.installed()
        self.assertEqual(self.inspect(), (None, "not_installed"))

    def test_disabled_still_wins_when_the_cache_is_gone(self):
        self.live_only()
        write(self.config / "settings.json",
              json.dumps({"enabledPlugins": {"anthology-guardrails@anthology": False}}))
        self.assertEqual(self.inspect(), (self.live, "disabled"))

    def test_project_scoped_entry_for_another_project_is_still_skipped(self):
        self.live_only()
        write(self.config / "plugins" / "installed_plugins.json", json.dumps({"version": 2, "plugins": {
            "anthology@anthology": [{"scope": "user", "installPath": str(self.tmp / "anthology")}],
            "anthology-guardrails@anthology": [{
                "scope": "project", "projectPath": str(self.tmp / "elsewhere"),
                "installPath": str(self.cache), "lastUpdated": "2026-01-01T00:00:00Z"}]}}))
        self.assertEqual(self.inspect(), (None, "not_installed"))


class BoundedReadTest(LiveRootCase):
    """m15 (same class): settings files in the working folder are read only when small and regular."""

    def test_oversized_project_settings_are_not_read(self):
        self.diverging()
        pad = "A" * (guardrails_loc.READ_LIMIT + 10)
        write(self.tmp / ".claude" / "settings.json", json.dumps({
            "enabledPlugins": {"anthology-guardrails@anthology": False}, "pad": pad}))
        self.assertEqual(self.inspect(), (self.live, None))

    def test_small_project_settings_still_disable(self):
        self.diverging()
        write(self.tmp / ".claude" / "settings.json", json.dumps({
            "enabledPlugins": {"anthology-guardrails@anthology": False}}))
        self.assertEqual(self.inspect(), (self.live, "disabled"))

    @unittest.skipUnless(os.path.exists("/dev/zero"), "needs /dev/zero")
    def test_device_symlink_is_not_read(self):
        self.diverging()
        (self.tmp / ".claude").mkdir()
        os.symlink("/dev/zero", str(self.tmp / ".claude" / "settings.json"))
        probe = ("import resource, sys\n"
                 "try:\n    resource.setrlimit(resource.RLIMIT_AS, (1 << 30, 1 << 30))\n"
                 "except (ValueError, OSError):\n    sys.exit(77)\n"
                 "sys.path.insert(0, %r)\nimport guardrails_loc\n"
                 "print(guardrails_loc.inspect(%r, {'CLAUDE_CONFIG_DIR': %r}, %r)[1])\n"
                 % (str(SCRIPTS_DIR), str(PLUGIN), str(self.config), str(self.tmp)))
        proc = subprocess.run([sys.executable, "-c", probe], capture_output=True, text=True, timeout=60)
        if proc.returncode == 77:
            self.skipTest("cannot limit memory here")
        self.assertEqual(proc.returncode, 0, proc.stderr[-300:])
        self.assertEqual(proc.stdout.strip(), "None")


class LoadRulesFollowTest(LiveRootCase):
    def test_load_rules_reads_profile_roots_from_the_live_copy(self):
        self.diverging()
        env = {"CLAUDE_CONFIG_DIR": str(self.config)}
        rules = guardrails_loc.load_rules(PLUGIN, env, str(self.tmp))
        self.assertIn(".config/live-marker", rules.PROFILE_ROOTS)
        self.assertNotIn(".config/cache-marker", rules.PROFILE_ROOTS)

    def test_setup_ops_deny_rules_come_from_the_live_copy(self):
        import settings_patch
        self.diverging()
        env = {"CLAUDE_CONFIG_DIR": str(self.config)}
        deny = [o["value"] for o in settings_patch.setup_ops(PLUGIN, env=env)
                if o["path"] == ["permissions", "deny"]]
        self.assertIn("Read(~/.config/live-marker)", deny)
        self.assertFalse([d for d in deny if "cache-marker" in d])


class SelfTestFollowsTest(SessionCase, LiveRootCase):
    """session_start's self-test pipes the sample into the guard that actually runs."""

    def test_live_copy_blocks_while_the_cache_copy_does_not_is_quiet(self):
        self.diverging(live_guard=None, cache_guard=EXIT_0)
        self.mark_setup()  # after the layout exists: setup records the op set it gives
        self.assertNotIn("systemMessage", self.go())

    def test_live_copy_that_does_not_block_warns_though_the_cache_copy_does(self):
        self.mark_setup()
        self.diverging(live_guard=EXIT_0, cache_guard=None)
        self.assertIn("did not block", self.go()["systemMessage"].lower())

    def test_live_folder_without_guard_py_warns_and_never_runs_the_cache_copy(self):
        self.mark_setup()
        self.copy_guardrails(self.cache)  # the real guard: it would block and pass the self-test
        (self.live / "scripts").mkdir(parents=True)
        self.manifest()
        self.directory_known()
        self.installed()
        msg = self.go()["systemMessage"]
        self.assertIn("anthology-guardrails folder this session runs has no guard.py", msg)
        self.assertIn("guardrails may be OFF", msg)
        self.assertRegex(msg, r"reinstall or repair")
        self.assertLess(len(msg), 400)

    def test_github_marketplace_checks_the_cache_copy(self):
        self.diverging(live_guard=EXIT_0, cache_guard=None)
        self.known({"source": "github", "repo": "owner/anthology"}, location=self.tmp / "gh")
        self.mark_setup()  # after the layout exists: setup records the op set it gives
        self.assertNotIn("systemMessage", self.go())


if __name__ == "__main__":
    unittest.main()
