"""Tests for scripts/settings_patch.py: reversible, byte-stable edits of the user's settings.json."""
from __future__ import annotations

import difflib
import json
import os
import stat
import sys
import tempfile
import unittest
from unittest import mock
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
sys.path.insert(0, str(SCRIPTS))

import settings_patch  # noqa: E402

PLUGIN_ROOT = ROOT / "plugins" / "anthology"
PROFILE_ROOTS = ("Library/Application Support/Google", ".config/chromium")

BASE = {
    "model": "opus",
    "permissions": {"allow": ["Bash(ls)"], "deny": ["Read(.env)"]},
    "env": {"A": "1"},
}


def ops():
    return settings_patch.setup_ops(PLUGIN_ROOT, profile_roots=PROFILE_ROOTS)


def make_plugin_root(parent, name="anthology"):
    """A minimal valid plugin folder `<parent>/anthology` (F21 needs a manifest named anthology)."""
    root = Path(parent) / "anthology"
    (root / ".claude-plugin").mkdir(parents=True)
    (root / ".claude-plugin" / "plugin.json").write_text(json.dumps({"name": name}), encoding="utf-8")
    return root


def fixtures():
    """name -> settings text, covering the formats the property tests must keep byte-stable."""
    base2 = json.dumps(BASE, indent=2) + "\n"
    out = {
        "two-space": base2,
        "two-space-no-newline": json.dumps(BASE, indent=2),
        "four-space": json.dumps(BASE, indent=4) + "\n",
        "tab": json.dumps(BASE, indent="\t") + "\n",
        "compact": json.dumps(BASE, separators=(",", ":")) + "\n",
        "compact-spaces": json.dumps(BASE),
        "crlf": base2.replace("\n", "\r\n"),
        "missing": "",
        "blank-newline": "\n",
        "empty-object": "{}",
        "empty-object-newline": "{}\n",
        "empty-object-spaced": "{\n}\n",
        "other-keys-only": json.dumps({"model": "x"}, indent=2) + "\n",
        "empty-permissions": json.dumps({"permissions": {}}, indent=2) + "\n",
        "empty-arrays": json.dumps({"permissions": {"allow": [], "deny": []}}, indent=2) + "\n",
        "empty-arrays-compact": '{"permissions":{"allow":[],"deny":[]}}',
        "unicode": json.dumps({"note": "héllo ✓", "permissions": {"allow": ["Bash(echo é)"]}},
                              indent=2, ensure_ascii=False) + "\n",
        "empty-worktree": json.dumps({"worktree": {}}, indent=2) + "\n",
        "baseref-fresh": json.dumps({"worktree": {"baseRef": "fresh"}, "x": 1}, indent=2) + "\n",
        "allow-one-item-compact": '{"permissions":{"allow":["Bash(ls)"]}}',
        "nested-values": json.dumps({"hooks": {"A": [{"x": [1, 2, {"y": None}]}]}, "z": 1.5e3},
                                    indent=2) + "\n",
    }
    return out


class RoundTripTest(unittest.TestCase):
    def test_apply_then_revert_restores_the_original_bytes(self):
        for name, text in fixtures().items():
            with self.subTest(name):
                planned = settings_patch.plan(text, ops())
                new, diff = settings_patch.apply(text, planned)
                self.assertNotEqual(new, text)
                self.assertTrue(diff)
                data = json.loads(new)
                self.assertIn("Bash(rtk *)", data["permissions"]["allow"])
                self.assertEqual(data["worktree"]["baseRef"], "head")
                back, _ = settings_patch.revert(new, planned)
                self.assertEqual(back, text)

    def test_apply_twice_equals_apply_once(self):
        for name, text in fixtures().items():
            with self.subTest(name):
                once, _ = settings_patch.apply(text, ops())
                twice, diff = settings_patch.apply(once, ops())
                self.assertEqual(twice, once)
                self.assertEqual(diff, "")
                self.assertEqual(settings_patch.plan(once, ops()), [])

    def test_everything_outside_the_edit_is_untouched(self):
        for name, text in fixtures().items():
            if name in ("missing", "blank-newline", "baseref-fresh"):
                continue
            with self.subTest(name):
                new, _ = settings_patch.apply(text, ops())
                matcher = difflib.SequenceMatcher(None, text, new, autojunk=False)
                kinds = {tag for tag, *_ in matcher.get_opcodes()}
                self.assertLessEqual(kinds, {"equal", "insert"})

    def test_inserted_lines_copy_the_files_style(self):
        two, _ = settings_patch.apply(json.dumps(BASE, indent=2) + "\n", ops())
        self.assertIn('\n      "Bash(ls)",\n      "Bash(rtk *)"', two)
        four, _ = settings_patch.apply(json.dumps(BASE, indent=4) + "\n", ops())
        self.assertIn('\n            "Bash(ls)",\n            "Bash(rtk *)"', four)
        tab, _ = settings_patch.apply(json.dumps(BASE, indent="\t") + "\n", ops())
        self.assertIn('\n\t\t\t"Bash(ls)",\n\t\t\t"Bash(rtk *)"', tab)
        crlf, _ = settings_patch.apply((json.dumps(BASE, indent=2) + "\n").replace("\n", "\r\n"), ops())
        self.assertNotRegex(crlf, r"(?<!\r)\n")
        compact, _ = settings_patch.apply(json.dumps(BASE, separators=(",", ":")), ops())
        self.assertNotIn("\n", compact)

    def test_missing_file_gets_a_formatted_document(self):
        new, _ = settings_patch.apply("", ops())
        self.assertTrue(new.endswith("}\n"))
        self.assertIn('\n  "permissions": {', new)
        self.assertEqual(json.loads(new)["worktree"], {"baseRef": "head"})


class OpsTest(unittest.TestCase):
    def test_setup_ops_contents(self):
        planned = settings_patch.plan("", ops())
        text, _ = settings_patch.apply("", planned)
        data = json.loads(text)
        allow, deny = data["permissions"]["allow"], data["permissions"]["deny"]
        self.assertIn("Bash(rtk *)", allow)
        self.assertIn(settings_patch.ALLOW_MEM_SAVE, allow)
        self.assertTrue(settings_patch.ALLOW_MEM_SAVE.startswith("Bash(anthology-mem save"))
        for root in PROFILE_ROOTS:
            self.assertIn("Read(~/%s)" % root, deny)
            self.assertIn("Read(~/%s/**)" % root, deny)
        parent = os.path.dirname(os.path.realpath(str(PLUGIN_ROOT)))
        self.assertIn("Read(//%s/**)" % parent.lstrip("/"), allow)
        self.assertEqual(settings_patch.plugin_read_rule(PLUGIN_ROOT), "Read(//%s/**)" % parent.lstrip("/"))
        self.assertEqual(data["worktree"]["baseRef"], "head")

    def test_default_profile_roots_come_from_the_guardrails_plugin(self):
        import guardrails_loc
        rules = guardrails_loc.load_rules(PLUGIN_ROOT, {})
        deny = [o["value"] for o in settings_patch.setup_ops(PLUGIN_ROOT) if o["path"] == ["permissions", "deny"]]
        variants = getattr(rules, "_PREFIX_VARIANTS", ("",))
        expected = []
        for root in rules.PROFILE_ROOTS:
            for v in (variants if root.startswith(".config/") else ("",)):
                expected += ["Read(~/%s%s)" % (root, v), "Read(~/%s%s/**)" % (root, v)]
        self.assertEqual(deny, expected)
        self.assertIn("Read(~/%s)" % rules.PROFILE_ROOTS[0], deny)

    def test_deny_rules_cover_the_guards_linux_channel_variants(self):
        deny = [o["value"] for o in settings_patch.setup_ops(PLUGIN_ROOT) if o["path"] == ["permissions", "deny"]]
        for rule in ("Read(~/.config/google-chrome-beta)", "Read(~/.config/google-chrome-unstable/**)",
                     "Read(~/.config/chromium-dev)", "Read(~/.config/chromium-canary/**)"):
            self.assertIn(rule, deny)
        # the macOS roots have no channel variants
        self.assertNotIn("Read(~/Library/Application Support/Google-beta)", deny)

    def test_injected_profile_roots_get_no_variants(self):
        deny = [o["value"] for o in settings_patch.setup_ops(PLUGIN_ROOT, profile_roots=(".config/x",))
                if o["path"] == ["permissions", "deny"]]
        self.assertEqual(deny, ["Read(~/.config/x)", "Read(~/.config/x/**)"])

    def test_ops_are_json_serialisable(self):
        planned = settings_patch.plan(json.dumps(BASE), ops())
        self.assertEqual(json.loads(json.dumps(planned)), planned)

    def test_users_own_rule_is_never_ours(self):
        text = json.dumps({"permissions": {"allow": ["Bash(rtk *)", "Bash(ls)"]}}, indent=2) + "\n"
        planned = settings_patch.plan(text, ops())
        self.assertFalse([o for o in planned if o.get("value") == "Bash(rtk *)"])
        new, _ = settings_patch.apply(text, planned)
        back, _ = settings_patch.revert(new, planned)
        self.assertEqual(back, text)
        self.assertIn("Bash(rtk *)", json.loads(back)["permissions"]["allow"])

    def test_base_ref_previous_value_is_remembered_and_restored(self):
        text = json.dumps({"worktree": {"baseRef": "fresh"}}, indent=2) + "\n"
        planned = settings_patch.plan(text, ops())
        base_op = [o for o in planned if o["path"] == ["worktree", "baseRef"]][0]
        self.assertEqual(base_op["previous"], {"present": True, "raw": '"fresh"'})
        new, _ = settings_patch.apply(text, planned)
        self.assertEqual(json.loads(new)["worktree"]["baseRef"], "head")
        back, _ = settings_patch.revert(new, planned)
        self.assertEqual(json.loads(back)["worktree"]["baseRef"], "fresh")

    def test_base_ref_changed_by_the_user_is_left_alone_on_revert(self):
        text = json.dumps({"worktree": {"baseRef": "fresh"}}, indent=2) + "\n"
        planned = settings_patch.plan(text, ops())
        new, _ = settings_patch.apply(text, planned)
        new = new.replace('"head"', '"fresh-ish"')
        back, _ = settings_patch.revert(new, planned)
        self.assertEqual(json.loads(back)["worktree"]["baseRef"], "fresh-ish")

    def test_user_edits_between_apply_and_revert_survive(self):
        text = json.dumps(BASE, indent=2) + "\n"
        planned = settings_patch.plan(text, ops())
        new, _ = settings_patch.apply(text, planned)
        data = json.loads(new)
        data["model"] = "sonnet"
        data["permissions"]["allow"].append("Bash(make *)")
        data["permissions"]["allow"].insert(0, "Bash(pwd)")
        data["extra"] = {"k": [1]}
        edited = json.dumps(data, indent=2) + "\n"
        back, _ = settings_patch.revert(edited, planned)
        got = json.loads(back)
        self.assertEqual(got["model"], "sonnet")
        self.assertEqual(got["permissions"]["allow"], ["Bash(pwd)", "Bash(ls)", "Bash(make *)"])
        self.assertEqual(got["permissions"]["deny"], ["Read(.env)"])
        self.assertEqual(got["extra"], {"k": [1]})
        self.assertNotIn("worktree", got)

    def test_created_containers_survive_when_the_user_added_to_them(self):
        planned = settings_patch.plan("", ops())
        new, _ = settings_patch.apply("", planned)
        data = json.loads(new)
        data["permissions"]["ask"] = ["Bash(git push *)"]
        back, _ = settings_patch.revert(json.dumps(data, indent=2) + "\n", planned)
        self.assertEqual(json.loads(back), {"permissions": {"ask": ["Bash(git push *)"]}})

    def test_revert_of_a_text_without_our_entries_changes_nothing(self):
        text = json.dumps(BASE, indent=2) + "\n"
        planned = settings_patch.plan(text, ops())
        back, diff = settings_patch.revert(text, planned)
        self.assertEqual(back, text)
        self.assertEqual(diff, "")


class PluginRootGuardTest(unittest.TestCase):
    """F21: the U1 allow rule must never be broader than the plugin folder's own parent."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(os.path.realpath(tmp.name))
        self.home = self.tmp / "home"
        self.home.mkdir()
        self.env = {"HOME": str(self.home)}

    def refuses(self, root, fragment=""):
        with self.assertRaises(ValueError) as ctx:
            settings_patch.setup_ops(root, profile_roots=PROFILE_ROOTS, env=self.env)
        self.assertIn(fragment, str(ctx.exception))

    def test_a_proper_root_is_accepted(self):
        root = make_plugin_root(self.tmp / "plugins")
        ops_ = settings_patch.setup_ops(root, profile_roots=PROFILE_ROOTS, env=self.env)
        self.assertIn(settings_patch.plugin_read_rule(root), [o["value"] for o in ops_])

    def test_empty_relative_missing_and_non_folder_roots(self):
        self.refuses("", "absolute")
        self.refuses("plugins/anthology", "absolute")
        self.refuses(str(self.tmp / "missing" / "anthology"), "not a folder")
        (self.tmp / "file").write_text("x", encoding="utf-8")
        self.refuses(str(self.tmp / "file"), "not a folder")

    def test_root_without_an_anthology_manifest(self):
        bare = self.tmp / "plugins" / "anthology"
        bare.mkdir(parents=True)
        self.refuses(bare, "not the anthology plugin")
        self.refuses(make_plugin_root(self.tmp / "other", name="something-else"), "not the anthology plugin")

    def test_filesystem_root_and_top_level_folders(self):
        self.refuses("/", "not")
        top = self.tmp  # make the parent an ancestor of HOME
        root = make_plugin_root(top)
        self.refuses(root, "home folder")

    def test_parent_that_is_the_home_folder(self):
        root = make_plugin_root(self.home)
        self.refuses(root, "home folder")

    def test_parent_that_is_an_ancestor_of_home(self):
        deep_home = self.tmp / "a" / "b" / "home"
        deep_home.mkdir(parents=True)
        root = make_plugin_root(self.tmp / "a")
        with self.assertRaises(ValueError):
            settings_patch.setup_ops(root, profile_roots=PROFILE_ROOTS, env={"HOME": str(deep_home)})

    def test_glob_and_rule_metacharacters_in_the_path(self):
        for ch in ("*", "?", "[", "]", "{", "}", "(", ")", "\\"):
            parent = self.tmp / ("plug" + ch + "ins")
            try:
                parent.mkdir()
            except OSError:
                continue
            self.refuses(make_plugin_root(parent), "pattern")

    def test_a_symlinked_root_is_judged_by_its_real_location(self):
        real = make_plugin_root(self.home)  # unsafe place
        link = self.tmp / "safe" / "anthology"
        link.parent.mkdir()
        os.symlink(str(real), str(link))
        self.refuses(link, "home folder")


class PluginRuleStatusTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(os.path.realpath(tmp.name))
        self.env = {"HOME": str(self.tmp), "CLAUDE_CONFIG_DIR": str(self.tmp / "c")}
        (self.tmp / "c").mkdir()
        self.path = self.tmp / "c" / "settings.json"
        self.rule = settings_patch.plugin_read_rule(PLUGIN_ROOT)

    def status(self):
        return settings_patch.plugin_rule_status(PLUGIN_ROOT, self.env)

    def test_states(self):
        self.assertEqual(self.status(), "missing")  # no file
        self.path.write_text(json.dumps({"permissions": {"allow": [self.rule]}}), encoding="utf-8")
        self.assertEqual(self.status(), "ok")
        self.path.write_text(json.dumps({"permissions": {"allow": ["Read(//old/**)"]}}), encoding="utf-8")
        self.assertEqual(self.status(), "missing")
        self.path.write_text("{nope", encoding="utf-8")
        self.assertEqual(self.status(), "unreadable")

    def test_bom_is_stripped(self):
        self.path.write_text("\ufeff" + json.dumps({"permissions": {"allow": [self.rule]}}), encoding="utf-8")
        self.assertEqual(self.status(), "ok")

    def test_allow_must_be_a_list_not_a_string(self):
        self.path.write_text(json.dumps({"permissions": {"allow": "xx" + self.rule + "yy"}}), encoding="utf-8")
        self.assertEqual(self.status(), "missing")

    def test_a_pathologically_deep_file_is_unreadable_not_a_crash(self):
        self.path.write_text("[" * 100000, encoding="utf-8")
        self.assertEqual(self.status(), "unreadable")


class StyleTest(unittest.TestCase):
    def test_inline_one_item_array_keeps_the_files_comma_space_style(self):
        text = '{"permissions": {"allow": ["Bash(ls)"]}}'
        new, _ = settings_patch.apply(text, ops())
        self.assertIn('["Bash(ls)", "Bash(rtk *)", ', new)
        self.assertIn('}, "worktree"', new)
        planned = settings_patch.plan(text, ops())
        self.assertEqual(settings_patch.revert(new, planned)[0], text)

    def test_diff_has_no_context_lines(self):
        text = json.dumps({"env": {"SECRET_TOKEN": "hunter2"}, "permissions": {"allow": ["Bash(ls)"]}}, indent=2) + "\n"
        _, diff = settings_patch.apply(text, ops())
        self.assertNotIn("hunter2", diff)
        self.assertNotIn("SECRET_TOKEN", diff)
        self.assertIn('+      "Bash(rtk *)"', diff)


class ReconcileTest(unittest.TestCase):
    def test_plugin_moved_replaces_only_the_stale_rule_we_added(self):
        text = json.dumps({"permissions": {"allow": ["Read(//somewhere/else/**)"]}}, indent=2) + "\n"
        old_root = make_plugin_root(Path(os.path.realpath(tempfile.mkdtemp())) / "plugins")
        first_ops = settings_patch.setup_ops(old_root, profile_roots=PROFILE_ROOTS)
        t1, _, stored1 = settings_patch.reconcile(text, first_ops, [])
        self.assertIn(settings_patch.plugin_read_rule(old_root), json.loads(t1)["permissions"]["allow"])
        t2, diff, stored2 = settings_patch.reconcile(t1, ops(), stored1)
        allow = json.loads(t2)["permissions"]["allow"]
        self.assertNotIn(settings_patch.plugin_read_rule(old_root), allow)
        self.assertIn(settings_patch.plugin_read_rule(PLUGIN_ROOT), allow)
        self.assertIn("Read(//somewhere/else/**)", allow)  # the user's own rule stays
        self.assertTrue(diff)
        back, _ = settings_patch.revert(t2, stored2)
        self.assertEqual(back, text)

    def test_changed_mem_save_rule_replaces_only_the_one_we_added(self):
        text = json.dumps(BASE, indent=2) + "\n"
        old_ops = [dict(o, value="Bash(anthology-mem save:*)") if o.get("tag") == settings_patch.TAG_MEM_SAVE
                   else o for o in ops()]
        t1, _, stored1 = settings_patch.reconcile(text, old_ops, [])
        self.assertIn("Bash(anthology-mem save:*)", json.loads(t1)["permissions"]["allow"])
        t2, _, stored2 = settings_patch.reconcile(t1, ops(), stored1)
        allow = json.loads(t2)["permissions"]["allow"]
        self.assertNotIn("Bash(anthology-mem save:*)", allow)
        self.assertIn(settings_patch.ALLOW_MEM_SAVE, allow)
        back, _ = settings_patch.revert(t2, stored2)
        self.assertEqual(back, text)

    def test_rerun_keeps_the_original_ops_so_remove_still_works(self):
        text = json.dumps(BASE, indent=2) + "\n"
        t1, _, stored1 = settings_patch.reconcile(text, ops(), [])
        t2, diff, stored2 = settings_patch.reconcile(t1, ops(), stored1)
        self.assertEqual(t2, t1)
        self.assertEqual(diff, "")
        self.assertEqual(stored2, stored1)
        back, _ = settings_patch.revert(t2, stored2)
        self.assertEqual(back, text)

    def test_rule_the_user_deleted_is_added_again_without_duplicating_the_record(self):
        text = json.dumps(BASE, indent=2) + "\n"
        t1, _, stored1 = settings_patch.reconcile(text, ops(), [])
        data = json.loads(t1)
        data["permissions"]["allow"].remove("Bash(rtk *)")
        t2, _, stored2 = settings_patch.reconcile(json.dumps(data, indent=2) + "\n", ops(), stored1)
        self.assertIn("Bash(rtk *)", json.loads(t2)["permissions"]["allow"])
        keys = [(o["op"], tuple(o["path"]), o["value"]) for o in stored2]
        self.assertEqual(len(keys), len(set(keys)))


class InvalidInputTest(unittest.TestCase):
    def test_unparseable_or_non_object_text_is_refused(self):
        for text in ('{"a": 1,}', "{", "[1, 2]", '"x"', "null", '{"a": 1} trailing', "// c\n{}"):
            with self.subTest(text):
                with self.assertRaises(ValueError):
                    settings_patch.apply(text, ops())
                with self.assertRaises(ValueError):
                    settings_patch.revert(text, ops())
                with self.assertRaises(ValueError):
                    settings_patch.plan(text, ops())

    def test_wrong_container_types_are_refused(self):
        for data in ({"permissions": []}, {"permissions": {"allow": "x"}}, {"worktree": 3},
                     {"permissions": {"deny": {}}}):
            with self.subTest(data):
                with self.assertRaises(ValueError):
                    settings_patch.apply(json.dumps(data), ops())

    def test_utf8_bom_is_kept(self):
        text = "﻿" + json.dumps(BASE, indent=2) + "\n"
        planned = settings_patch.plan(text, ops())
        new, _ = settings_patch.apply(text, planned)
        self.assertTrue(new.startswith("﻿{"))
        self.assertEqual(settings_patch.revert(new, planned)[0], text)


class DiffTest(unittest.TestCase):
    def test_diff_is_a_unified_diff_of_the_change(self):
        text = json.dumps(BASE, indent=2) + "\n"
        _, diff = settings_patch.apply(text, ops())
        self.assertIn("--- settings.json", diff)
        self.assertIn("+++ settings.json", diff)
        self.assertIn('+      "Bash(rtk *)"', diff)


SECRETS = ("hunter2", "sk-live-123", "s3cr3t-cookie", "Bearer abc.def", "XYZSECRET", "opus")


class RedactTest(unittest.TestCase):
    """Setup and remove diffs show values only where setup changes them: never one under `env`,
    a secret-looking key or any other key."""

    def check(self, text, diff_lines=(), undo_lines=()):
        """The setup diff and the remove diff of `text`; checks both carry the change, no secret
        and the expected lines."""
        planned = settings_patch.plan(text, ops())
        new, diff = settings_patch.apply(text, planned)
        back, undo = settings_patch.revert(new, planned)
        self.assertEqual(back, text)
        for shown, lines in ((diff, diff_lines), (undo, undo_lines)):
            self.assertIn("Bash(rtk *)", shown)
            self.assertIn("baseRef", shown)
            for secret in SECRETS:
                self.assertNotIn(secret, shown)
            for line in lines:
                self.assertIn(line, shown.splitlines())
        return diff, undo

    def test_minified_file(self):
        text = json.dumps({"env": {"API_TOKEN": "hunter2", "PLAIN": "sk-live-123"},
                           "model": "opus",
                           "permissions": {"allow": ["Bash(ls)"]},
                           "mcp": {"headers": {"Authorization": "Bearer abc.def"}},
                           "hooks": {"Stop": [{"command": "curl https://hooks.example/XYZSECRET"}]},
                           "proxyUrl": "http://u:s3cr3t-cookie@proxy"},
                          separators=(",", ":"))
        diff, undo = self.check(text)
        for shown in (diff, undo):
            for part in ('"API_TOKEN":"<redacted>"', '"PLAIN":"<redacted>"', '"model":"<redacted>"',
                         '"Authorization":"<redacted>"', '"command":"<redacted>"',
                         '"proxyUrl":"<redacted>"', '"Bash(ls)","Bash(rtk *)"'):
                self.assertIn(part, shown)

    def test_one_line_env_as_the_last_key(self):
        text = ('{\n  "model": "opus",\n  "permissions": {"allow": ["Bash(ls)"]},\n'
                '  "env": {"API_TOKEN": "hunter2", "PLAIN": "sk-live-123", "N": 3}\n}\n')
        env = '  "env": {"API_TOKEN": "<redacted>", "PLAIN": "<redacted>", "N": "<redacted>"}'
        self.check(text, diff_lines=["-" + env, "+" + env + ","],
                    undo_lines=["-" + env + ",", "+" + env])

    def test_pretty_printed_file(self):
        data = {"permissions": {"allow": ["Bash(ls)"]},
                "awsAuthRefresh": "login --password hunter2",
                "servers": [{"name": "x", "cookie": "s3cr3t-cookie", "env": {"K": ["sk-live-123"]}}],
                "env": {"SECRET": "hunter2"}}
        text = json.dumps(data, indent=2) + "\n"
        # env is the last key: its closing brace gets the comma, its values stay out of the diff
        self.check(text, diff_lines=["+  },"], undo_lines=["-  },"])
        self.assertIn('    "SECRET": "<redacted>"', settings_patch.redact(text).splitlines())

    def test_pretty_printed_file_with_a_one_line_secret_as_the_last_key(self):
        text = json.dumps({"permissions": {"allow": ["Bash(ls)"]},
                           "apiKeyHelper": "/bin/echo sk-live-123"}, indent=2) + "\n"
        self.check(text, diff_lines=['-  "apiKeyHelper": "<redacted>"', '+  "apiKeyHelper": "<redacted>",'],
                   undo_lines=['-  "apiKeyHelper": "<redacted>",', '+  "apiKeyHelper": "<redacted>"'])

    def test_redaction_keeps_keys_layout_and_line_count(self):
        text = json.dumps({"env": {"A": "1", "B": {"c": [1, "x"]}}, "model": "opus",
                           "githubToken": "hunter2"}, indent=2) + "\n"
        out = settings_patch.redact(text)
        self.assertEqual(len(out.splitlines()), len(text.splitlines()))
        self.assertEqual(json.loads(out), {"env": {"A": "<redacted>", "B": {"c": ["<redacted>", "<redacted>"]}},
                                           "model": "<redacted>", "githubToken": "<redacted>"})
        self.assertEqual(settings_patch.redact(""), "")

    def test_diff_without_secrets_is_unchanged_by_redaction(self):
        text = json.dumps({"model": "opus", "permissions": {"allow": ["Bash(ls)"]}}, indent=2) + "\n"
        new, diff = settings_patch.apply(text, settings_patch.plan(text, ops()))
        raw = "\n".join(difflib.unified_diff(text.splitlines(), new.splitlines(), "settings.json",
                                             "settings.json", lineterm="", n=0))
        self.assertEqual(diff, raw)

    def test_shown_paths_stay_visible(self):
        text = json.dumps({"permissions": {"allow": ["Bash(ls)"], "deny": ["Read(x)"], "ask": ["y"]},
                           "worktree": {"baseRef": "fresh", "other": "z"},
                           "list": [{"permissions": {"allow": ["hidden"]}}]}, indent=2)
        self.assertEqual(json.loads(settings_patch.redact(text)),
                         {"permissions": {"allow": ["Bash(ls)"], "deny": ["Read(x)"], "ask": ["<redacted>"]},
                          "worktree": {"baseRef": "fresh", "other": "<redacted>"},
                          "list": [{"permissions": {"allow": ["<redacted>"]}}]})

    def test_every_setup_op_is_on_a_shown_path(self):
        """Otherwise the diff would hide a change setup makes."""
        for op in settings_patch.setup_ops(PLUGIN_ROOT):
            self.assertIn(tuple(op["path"]), settings_patch.SHOWN_PATHS, op)


class OpsVersionTest(unittest.TestCase):
    def test_same_ops_same_version(self):
        self.assertEqual(settings_patch.ops_version(ops()), settings_patch.ops_version(ops()))
        self.assertTrue(settings_patch.ops_version(ops()).startswith("%d-" % settings_patch.OPS_VERSION))

    def test_any_change_to_the_op_list_changes_it(self):
        base = settings_patch.ops_version(ops())
        more = ops() + [{"op": "add", "path": ["permissions", "allow"], "value": "Bash(x *)"}]
        edited = [dict(o, value="Bash(rtk status *)") if o["value"] == "Bash(rtk *)" else o for o in ops()]
        roots = settings_patch.setup_ops(PLUGIN_ROOT, profile_roots=PROFILE_ROOTS + (".config/new",))
        for other in (more, edited, ops()[:-1], roots):
            self.assertNotEqual(settings_patch.ops_version(other), base)

    def test_bumping_ops_version_changes_it(self):
        base = settings_patch.ops_version(ops())
        with mock.patch.object(settings_patch, "OPS_VERSION", settings_patch.OPS_VERSION + 1):
            self.assertNotEqual(settings_patch.ops_version(ops()), base)

    def test_the_plugin_location_is_not_part_of_it(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            one = settings_patch.setup_ops(make_plugin_root(a), profile_roots=PROFILE_ROOTS, env={"HOME": "/nonexistent-home"})
            two = settings_patch.setup_ops(make_plugin_root(b), profile_roots=PROFILE_ROOTS, env={"HOME": "/nonexistent-home"})
        self.assertNotEqual(one, two)
        self.assertEqual(settings_patch.ops_version(one), settings_patch.ops_version(two))

    def test_planned_undo_info_is_not_part_of_it(self):
        planned = settings_patch.plan("{}", ops())
        self.assertEqual(settings_patch.ops_version(planned), settings_patch.ops_version(ops()))


class FilesTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(os.path.realpath(tmp.name))

    def test_settings_path_honours_claude_config_dir_then_home(self):
        self.assertEqual(settings_patch.settings_path({"CLAUDE_CONFIG_DIR": "/c", "HOME": "/h"}),
                         Path("/c/settings.json"))
        self.assertEqual(settings_patch.settings_path({"HOME": "/h"}), Path("/h/.claude/settings.json"))

    def test_read_text_keeps_crlf_and_missing_is_empty(self):
        path = self.tmp / "settings.json"
        self.assertEqual(settings_patch.read_text(path), "")
        path.write_bytes(b'{\r\n  "a": 1\r\n}\r\n')
        self.assertEqual(settings_patch.read_text(path), '{\r\n  "a": 1\r\n}\r\n')

    def test_write_is_atomic_keeps_mode_and_bytes(self):
        path = self.tmp / "settings.json"
        path.write_bytes(b"{}\r\n")
        path.chmod(0o640)
        settings_patch.write_text(path, '{\r\n  "a": "é"\r\n}\r\n')
        self.assertEqual(path.read_bytes(), '{\r\n  "a": "é"\r\n}\r\n'.encode("utf-8"))
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o640)
        self.assertEqual([p.name for p in self.tmp.iterdir()], ["settings.json"])

    def test_write_follows_a_symlink_to_its_target(self):
        target = self.tmp / "dotfiles" / "settings.json"
        target.parent.mkdir()
        target.write_text("{}", encoding="utf-8")
        link = self.tmp / "settings.json"
        os.symlink(str(target), str(link))
        settings_patch.write_text(link, '{"a": 1}')
        self.assertTrue(link.is_symlink())
        self.assertEqual(target.read_text(encoding="utf-8"), '{"a": 1}')

    def test_write_creates_the_folder_for_a_new_file(self):
        path = self.tmp / "cfg" / "settings.json"
        settings_patch.write_text(path, "{}")
        self.assertEqual(path.read_text(encoding="utf-8"), "{}")


if __name__ == "__main__":
    unittest.main()
