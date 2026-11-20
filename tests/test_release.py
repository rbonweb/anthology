"""Tests for scripts/release_edit.py and scripts/release.sh.

The pure-logic tests need nothing but Python. The integration tests copy the release
scripts into a throwaway git repository and run them with a fake `claude` binary, so they
are skipped where git or bash is missing (the Docker test images) and run in the CI
`version-bump` job.
"""
from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import release_edit as re_  # noqa: E402

CHANGELOG = """\
# Changelog

Intro text.

## anthology

### [Unreleased]

#### Added

- Anthology thing.

### [0.0.9] - 2026-01-01

#### Added

- Older anthology thing.

## anthology-guardrails

### [Unreleased]

#### Added

- Guardrails thing.

## anthology-superpowers

### [Unreleased]

"""


def section(text, plugin):
    """The `## plugin` section of a changelog, for before/after comparison."""
    out, on = [], False
    for line in text.splitlines(keepends=True):
        if line.startswith("## "):
            on = line.strip() == f"## {plugin}"
        if on:
            out.append(line)
    return "".join(out)


class ChangelogTests(unittest.TestCase):
    def test_move_inserts_version_section_below_unreleased(self):
        out = re_.changelog_move(CHANGELOG, "anthology", "0.1.0", "2026-11-20")
        self.assertIn("### [Unreleased]\n\n### [0.1.0] - 2026-11-20\n\n#### Added\n\n"
                      "- Anthology thing.\n\n### [0.0.9] - 2026-01-01\n", out)
        self.assertEqual(out.count("- Anthology thing."), 1)

    def test_move_leaves_other_plugins_and_intro_untouched(self):
        out = re_.changelog_move(CHANGELOG, "anthology", "0.1.0", "2026-11-20")
        for other in ("anthology-guardrails", "anthology-superpowers"):
            self.assertEqual(section(out, other), section(CHANGELOG, other))
        self.assertTrue(out.startswith("# Changelog\n\nIntro text.\n\n## anthology\n"))

    def test_move_does_not_confuse_plugin_with_shared_prefix(self):
        out = re_.changelog_move(CHANGELOG, "anthology-guardrails", "0.2.0", "2026-11-20")
        self.assertEqual(section(out, "anthology"), section(CHANGELOG, "anthology"))
        self.assertIn("### [0.2.0] - 2026-11-20\n\n#### Added\n\n- Guardrails thing.", out)

    def test_move_last_section_in_file(self):
        text = CHANGELOG.rstrip("\n") + "\n- Superpowers thing.\n"
        out = re_.changelog_move(text, "anthology-superpowers", "0.1.0", "2026-11-20")
        self.assertTrue(out.endswith("### [Unreleased]\n\n### [0.1.0] - 2026-11-20\n\n"
                                     "- Superpowers thing.\n"))

    def test_empty_unreleased_is_refused(self):
        with self.assertRaises(re_.ReleaseError) as ctx:
            re_.changelog_move(CHANGELOG, "anthology-superpowers", "0.1.0", "2026-11-20")
        self.assertIn("Unreleased", str(ctx.exception))
        with self.assertRaises(re_.ReleaseError):
            re_.changelog_check(CHANGELOG, "anthology-superpowers")

    def test_heading_only_unreleased_is_refused(self):
        text = CHANGELOG.replace("- Guardrails thing.\n", "")
        with self.assertRaises(re_.ReleaseError):
            re_.changelog_check(text, "anthology-guardrails")

    def test_missing_plugin_or_unreleased_heading(self):
        with self.assertRaises(re_.ReleaseError):
            re_.changelog_check(CHANGELOG, "nope")
        with self.assertRaises(re_.ReleaseError):
            re_.changelog_check("## p\n\n### [1.0.0] - 2026-01-01\n\n- x\n", "p")

    def test_existing_version_section_is_refused(self):
        with self.assertRaises(re_.ReleaseError):
            re_.changelog_move(CHANGELOG, "anthology", "0.0.9", "2026-11-20")

    def test_check_passes_with_content(self):
        re_.changelog_check(CHANGELOG, "anthology")


class VersionEditTests(unittest.TestCase):
    FIRST_PARTY = ('{\n  "name": "p",\n  "version": "0.1.0",\n'
                   '  "author": { "name": "x" },\n  "engines": { "claude-code": ">=2.1.311" }\n}\n')
    GENERATED = ('{\n  "name": "q",\n  "author": {\n    "name": "A",\n    "version": "9.9.9"\n'
                 '  },\n  "version": "0.1.0",\n  "keywords": [\n    "version"\n  ]\n}\n')

    def test_replaces_only_the_version_value(self):
        out = re_.set_plugin_version(self.FIRST_PARTY, "0.2.0")
        self.assertEqual(out, self.FIRST_PARTY.replace('"version": "0.1.0"',
                                                       '"version": "0.2.0"'))

    def test_ignores_nested_version_keys_and_preserves_layout(self):
        out = re_.set_plugin_version(self.GENERATED, "0.3.0")
        self.assertEqual(out, self.GENERATED.replace('  "version": "0.1.0"',
                                                     '  "version": "0.3.0"'))
        self.assertIn('"version": "9.9.9"', out)

    def test_missing_or_non_string_version(self):
        with self.assertRaises(re_.ReleaseError):
            re_.set_plugin_version('{"name": "p"}\n', "0.2.0")
        with self.assertRaises(re_.ReleaseError):
            re_.set_plugin_version('{"version": 3}\n', "0.2.0")

    def test_plugin_version_reads_top_level(self):
        self.assertEqual(re_.plugin_version(self.GENERATED), "0.1.0")

    SOURCES = ('{\n  "sources": [\n    {\n      "name": "other",\n'
               '      "target": "plugins/anthology/knowledge/x",\n      "plugin_version": "7.7.7"\n'
               '    },\n    {\n      "name": "superpowers",\n      "ref": "v7.0.2",\n'
               '      "target": "plugins/anthology-superpowers",\n'
               '      "transform": [{"name": "rename-plugin", "new": "z"}],\n'
               '      "plugin_version": "0.1.0",\n      "upstream_version": "7.0.2"\n    }\n  ]\n}\n')

    def test_sources_edit_is_byte_minimal(self):
        out = re_.set_source_plugin_version(self.SOURCES, "plugins/anthology-superpowers",
                                            "0.2.0")
        self.assertEqual(out, self.SOURCES.replace('"plugin_version": "0.1.0"',
                                                   '"plugin_version": "0.2.0"'))

    def test_sources_unknown_target_or_no_plugin_version(self):
        with self.assertRaises(re_.ReleaseError):
            re_.set_source_plugin_version(self.SOURCES, "plugins/nope", "0.2.0")
        with self.assertRaises(re_.ReleaseError):
            re_.set_source_plugin_version('{"sources": [{"target": "t"}]}', "t", "0.2.0")


class CheckVersionTests(unittest.TestCase):
    def test_higher_is_a_bump(self):
        self.assertEqual(re_.check_version("0.1.0", "0.1.1", True), "bump")
        self.assertEqual(re_.check_version("0.1.0", "0.2.0", False), "bump")

    def test_equal_without_tag_is_first_release(self):
        self.assertEqual(re_.check_version("0.1.0", "0.1.0", False), "same")

    def test_equal_with_tag_is_refused(self):
        with self.assertRaises(re_.ReleaseError):
            re_.check_version("0.1.0", "0.1.0", True)

    def test_lower_is_refused(self):
        for tag_exists in (True, False):
            with self.assertRaises(re_.ReleaseError):
                re_.check_version("0.2.0", "0.1.9", tag_exists)

    def test_not_semver_is_refused(self):
        for bad in ("1.2", "v1.2.3", "", "1.2.3.4"):
            with self.assertRaises(re_.ReleaseError):
                re_.check_version("0.1.0", bad, False)


class RangeTests(unittest.TestCase):
    TABLE = [
        ("~0.1", "0.1.0", True), ("~0.1", "0.1.5", True), ("~0.1", "0.2.0", False),
        ("~0.1", "0.0.9", False), ("~1.2", "1.2.9", True), ("~1.2", "1.3.0", False),
        ("~0.1.2", "0.1.1", False), ("~0.1.2", "0.1.9", True), ("~0.1.2", "0.2.0", False),
        ("^0.1.2", "0.1.2", True), ("^0.1.2", "0.1.9", True), ("^0.1.2", "0.2.0", False),
        ("^0.1.2", "0.1.1", False), ("^0.0.3", "0.0.3", True), ("^0.0.3", "0.0.4", False),
        ("^1.2.3", "1.9.0", True), ("^1.2.3", "2.0.0", False), ("^1.2.3", "1.2.2", False),
        ("1.2.3", "1.2.3", True), ("1.2.3", "1.2.4", False),
        (">=1.2.3", "1.2.3", True), (">=1.2.3", "3.0.0", True), (">=1.2.3", "1.2.2", False),
        # npm: a prerelease only satisfies a range that names a prerelease of that version
        ("~0.1", "0.1.5-beta.1", False), ("^0.1.2", "0.1.5-rc.1", False),
        ("^0.1.2-beta.1", "0.1.2-beta.2", True), ("^0.1.2-beta.1", "0.1.2", True),
        ("^0.1.2-beta.2", "0.1.2-beta.1", False), (">=1.0.0", "2.0.0-rc.1", False),
        # npm partial ranges: ^X.Y, ^X, ~X
        ("^1.0", "1.0.0", True), ("^1.0", "1.9.3", True), ("^1.0", "2.0.0", False),
        ("^1.0", "0.9.9", False), ("^1.0", "1.0.1-rc.1", False), ("^1.0", "1.1.0-rc.1", False),
        ("^1.2", "1.2.0", True), ("^1.2", "1.1.9", False), ("^1.2", "1.9.0", True),
        ("^0.1", "0.1.0", True), ("^0.1", "0.1.9", True), ("^0.1", "0.2.0", False),
        ("^0.1", "0.0.9", False), ("^0.1", "0.1.1-beta.1", False),
        ("^0.0", "0.0.0", True), ("^0.0", "0.0.9", True), ("^0.0", "0.1.0", False),
        ("^0.0", "1.0.0", False),
        ("^1", "1.0.0", True), ("^1", "1.9.9", True), ("^1", "2.0.0", False),
        ("^1", "0.9.9", False), ("^1", "1.0.0-rc.1", False),
        ("^0", "0.0.0", True), ("^0", "0.9.9", True), ("^0", "1.0.0", False),
        ("^0", "0.1.0-rc.1", False),
        ("~1", "1.0.0", True), ("~1", "1.9.9", True), ("~1", "2.0.0", False),
        ("~1", "0.9.9", False), ("~1", "1.0.0-rc.1", False),
        ("~0", "0.0.0", True), ("~0", "0.9.9", True), ("~0", "1.0.0", False),
        ("~0", "0.1.0-rc.1", False),
        ("^10", "10.4.0", True), ("^10", "11.0.0", False),
    ]

    def test_acceptance_table(self):
        for rng, version, want in self.TABLE:
            self.assertEqual(re_.range_accepts(rng, version), want, f"{rng} vs {version}")

    def test_unsupported_forms_raise(self):
        for rng in ("*", "latest", "1.x", ">1.2.3", "1.2 - 2.0", "", "~", "^", ">=1.2", ">=1",
                    "||", "~0.1 || ~0.2", "v1.2.3", "=1.2.3", "<2.0.0", None,
                    "1.2", "1", "^1.x", "~1.x", "^1.2.3+build", "^1.2+build", " ^1.0",
                    "^1.0 ", "^01", "^1.02", "^1.2.", "^-1"):
            with self.assertRaises(re_.UnsupportedRange, msg=repr(rng)):
                re_.range_accepts(rng, "1.2.3")

    def _tree(self, deps):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        for name, d in deps.items():
            p = root / "plugins" / name / ".claude-plugin"
            p.mkdir(parents=True)
            manifest = {"name": name, "version": "0.1.0"}
            if d is not None:
                manifest["dependencies"] = d
            (p / "plugin.json").write_text(json.dumps(manifest), encoding="utf-8")
        return root

    def test_dependents_accepting(self):
        root = self._tree({"lib": None, "app": [{"name": "lib", "version": "~0.1"}],
                           "other": [{"name": "unrelated", "version": "9.9.9"}]})
        self.assertEqual(re_.check_dependents(root, "lib", "0.1.7"), [])

    def test_dependents_rejecting_names_the_dependent(self):
        root = self._tree({"lib": None, "app": [{"name": "lib", "version": "~0.1"}]})
        problems = re_.check_dependents(root, "lib", "0.2.0")
        self.assertEqual(len(problems), 1)
        self.assertIn("plugins/app/.claude-plugin/plugin.json", problems[0])
        self.assertIn("~0.1", problems[0])
        self.assertIn("widen", problems[0])

    def test_dependents_unsupported_range_fails_closed(self):
        root = self._tree({"lib": None, "app": [{"name": "lib", "version": "*"}]})
        problems = re_.check_dependents(root, "lib", "0.1.0")
        self.assertEqual(len(problems), 1)
        self.assertIn("can't check range", problems[0])

    def test_dependents_caret_partial_range(self):
        root = self._tree({"lib": None, "app": [{"name": "lib", "version": "^1.0"}]})
        self.assertEqual(re_.check_dependents(root, "lib", "1.0.0"), [])
        problems = re_.check_dependents(root, "lib", "2.0.0")
        self.assertEqual(len(problems), 1)
        self.assertIn("plugins/app/.claude-plugin/plugin.json", problems[0])
        self.assertIn("^1.0", problems[0])
        self.assertIn("does not accept 2.0.0", problems[0])

    def test_dependency_without_range_or_as_string_accepts_anything(self):
        root = self._tree({"lib": None, "a": [{"name": "lib"}], "b": ["lib"]})
        self.assertEqual(re_.check_dependents(root, "lib", "5.0.0"), [])


# ---- git + bash integration ----------------------------------------------------------

FAKE_CLAUDE = r"""#!/usr/bin/env bash
# Fake `claude`: records its arguments; for `plugin tag` creates the tag with git.
printf '%s\n' "$*" >> "$FAKE_CLAUDE_LOG"
[ "$1" = plugin ] && [ "$2" = tag ] || exit 0
shift 2
path=""; dry=0; push=0; remote=origin; msg=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry=1 ;;
    --push) push=1 ;;
    --remote) shift; remote="$1" ;;
    -m) shift; msg="$1" ;;
    *) path="$1" ;;
  esac
  shift
done
name=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["name"])' "$path/.claude-plugin/plugin.json")
ver=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$path/.claude-plugin/plugin.json")
tag="$name--v$ver"
if [ "$dry" = 1 ]; then echo "$tag"; exit 0; fi
git tag -a -m "${msg//%s/$ver}" "$tag"
[ "$push" = 1 ] && git push "$remote" "refs/tags/$tag"
exit 0
"""

SOURCES = json.dumps({"sources": [
    {"name": "superpowers", "target": "plugins/anthology-superpowers",
     "plugin_version": "0.1.0", "upstream_version": "7.0.2"},
    {"name": "x", "target": "plugins/anthology/knowledge/x"},
]}, indent=2) + "\n"

MANIFESTS = {
    "anthology": '{\n  "name": "anthology",\n  "version": "0.1.0",\n  "dependencies": [\n'
                 '    { "name": "anthology-guardrails", "version": "~0.1" },\n'
                 '    { "name": "anthology-superpowers", "version": "~0.1" }\n  ]\n}\n',
    "anthology-guardrails": '{\n  "name": "anthology-guardrails",\n  "version": "0.1.0"\n}\n',
    "anthology-superpowers": '{\n  "name": "anthology-superpowers",\n  "version": "0.1.0"\n}\n',
}

FAKE_CHANGELOG = """\
# Changelog

## anthology

### [Unreleased]

#### Added

- A.

## anthology-guardrails

### [Unreleased]

#### Added

- G.

## anthology-superpowers

### [Unreleased]

#### Added

- S.
"""

HAVE_TOOLS = bool(shutil.which("git") and shutil.which("bash"))


@unittest.skipUnless(HAVE_TOOLS, "git and bash are required")
class ReleaseScriptTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        base = Path(self._tmp.name)
        self.repo = base / "repo"
        self.remote = base / "remote.git"
        self.log = base / "claude.log"
        self.fake = base / "fake-claude"
        self.fake.write_text(FAKE_CLAUDE, encoding="utf-8")
        self.fake.chmod(0o755)
        self.log.write_text("", encoding="utf-8")
        self.repo.mkdir()
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@users.noreply.github.com")
        self.git("config", "commit.gpgsign", "false")
        self.git("config", "tag.gpgsign", "false")
        (self.repo / "scripts").mkdir()
        for name in ("release.sh", "release_edit.py", "check_version_bump.py"):
            shutil.copy(ROOT / "scripts" / name, self.repo / "scripts" / name)
        for plugin, text in MANIFESTS.items():
            self.write(f"plugins/{plugin}/.claude-plugin/plugin.json", text)
        self.write("vendor/sources.json", SOURCES)
        self.write("CHANGELOG.md", FAKE_CHANGELOG)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "initial")
        subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(self.remote)], check=True)
        self.git("remote", "add", "origin", str(self.remote))
        self.git("push", "-q", "origin", "main")

    def env(self):
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env["CLAUDE_BIN"] = str(self.fake)
        env["FAKE_CLAUDE_LOG"] = str(self.log)
        return env

    def git(self, *args, cwd=None):
        out = subprocess.run(["git", "-C", str(cwd or self.repo), *args], check=True,
                             env=self.env(), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                             universal_newlines=True)
        return out.stdout.strip()

    def write(self, rel, text):
        p = self.repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")

    def read(self, rel):
        return (self.repo / rel).read_text(encoding="utf-8")

    def release(self, *args):
        return subprocess.run(["bash", str(self.repo / "scripts" / "release.sh"), *args],
                              cwd=str(self.repo), env=self.env(), stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, universal_newlines=True)

    def state(self):
        return (self.git("rev-parse", "HEAD"), self.git("status", "--porcelain"),
                self.git("tag", "-l"), self.git("worktree", "list"))

    def remote_refs(self):
        return subprocess.run(["git", "-C", str(self.remote), "for-each-ref"], check=True,
                              stdout=subprocess.PIPE, universal_newlines=True).stdout

    def test_script_is_executable_in_the_real_repo(self):
        self.assertTrue((ROOT / "scripts" / "release.sh").stat().st_mode & stat.S_IXUSR)

    def test_normal_release(self):
        remote_before = self.remote_refs()
        r = self.release("anthology-guardrails", "0.1.1")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.git("log", "-1", "--format=%s"), "release: anthology-guardrails 0.1.1")
        self.assertNotIn("Co-Authored-By", self.git("log", "-1", "--format=%B"))
        self.assertEqual(self.git("show", "--name-only", "--format=", "HEAD").splitlines(),
                         ["CHANGELOG.md",
                          "plugins/anthology-guardrails/.claude-plugin/plugin.json"])
        self.assertEqual(self.read("plugins/anthology-guardrails/.claude-plugin/plugin.json"),
                         MANIFESTS["anthology-guardrails"].replace("0.1.0", "0.1.1"))
        log = self.read("CHANGELOG.md")
        self.assertIn("### [Unreleased]\n\n### [0.1.1] - ", log)
        self.assertEqual(log.count("- G."), 1)
        self.assertIn("anthology-guardrails--v0.1.1", self.git("tag", "-l"))
        self.assertEqual(self.git("tag", "-l", "-n1", "anthology-guardrails--v0.1.1"),
                         "anthology-guardrails--v0.1.1 anthology-guardrails 0.1.1")
        self.assertEqual(self.git("status", "--porcelain"), "")
        self.assertEqual(self.remote_refs(), remote_before)
        self.assertIn("plugin tag plugins/anthology-guardrails -m anthology-guardrails %s",
                      self.log.read_text())
        # prints how to publish
        self.assertIn("git push origin HEAD", r.stdout)
        self.assertIn("git push origin refs/tags/anthology-guardrails--v0.1.1", r.stdout)

    def test_dry_run_leaves_the_checkout_untouched(self):
        before = self.state()
        remote_before = self.remote_refs()
        r = self.release("anthology-guardrails", "0.1.1", "--dry-run")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertIn("anthology-guardrails--v0.1.1", r.stdout)
        self.assertIn("release: anthology-guardrails 0.1.1", r.stdout)
        self.assertIn("CHANGELOG.md", r.stdout)
        self.assertIn("+### [0.1.1] - ", r.stdout)
        self.assertEqual(self.state(), before)
        self.assertEqual(len(self.git("worktree", "list").splitlines()), 1)
        self.assertEqual(self.remote_refs(), remote_before)
        self.assertIn("--dry-run", self.log.read_text())

    def test_dry_run_with_push_is_refused(self):
        before = self.state()
        r = self.release("anthology-guardrails", "0.1.1", "--dry-run", "--push")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("--dry-run", r.stdout)
        self.assertEqual(self.state(), before)

    def test_dirty_tree_is_refused(self):
        self.write("plugins/anthology/note.txt", "x\n")
        before = self.state()
        r = self.release("anthology-guardrails", "0.1.1")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("not clean", r.stdout)
        self.assertEqual(self.state(), before)
        self.assertEqual(self.log.read_text(), "")

    def test_equal_version_without_tag_is_the_first_release(self):
        r = self.release("anthology-guardrails", "0.1.0")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.read("plugins/anthology-guardrails/.claude-plugin/plugin.json"),
                         MANIFESTS["anthology-guardrails"])
        self.assertEqual(self.git("show", "--name-only", "--format=", "HEAD").splitlines(),
                         ["CHANGELOG.md"])
        self.assertIn("### [0.1.0] - ", self.read("CHANGELOG.md"))
        self.assertIn("anthology-guardrails--v0.1.0", self.git("tag", "-l"))

    def test_equal_version_with_existing_tag_is_refused(self):
        self.git("tag", "anthology-guardrails--v0.1.0")
        before = self.state()
        r = self.release("anthology-guardrails", "0.1.0")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("already exists", r.stdout)
        self.assertEqual(self.state(), before)

    def test_lower_version_is_refused(self):
        before = self.state()
        r = self.release("anthology-guardrails", "0.0.9")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("0.0.9", r.stdout)
        self.assertEqual(self.state(), before)

    def test_invalid_semver_and_unknown_plugin_are_refused(self):
        before = self.state()
        self.assertNotEqual(self.release("anthology-guardrails", "0.2").returncode, 0)
        r = self.release("nonesuch", "0.1.0")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("plugins/nonesuch", r.stdout)
        self.assertNotEqual(self.release("../x", "0.1.0").returncode, 0)
        self.assertNotEqual(self.release("anthology-guardrails").returncode, 0)
        self.assertEqual(self.state(), before)

    def test_empty_unreleased_is_refused(self):
        self.write("CHANGELOG.md", FAKE_CHANGELOG.replace("#### Added\n\n- S.\n", ""))
        self.git("commit", "-q", "-a", "-m", "empty")
        before = self.state()
        r = self.release("anthology-superpowers", "0.1.1")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("Unreleased", r.stdout)
        self.assertEqual(self.state(), before)

    def test_dependent_range_rejecting_the_version_is_refused(self):
        before = self.state()
        r = self.release("anthology-guardrails", "0.2.0")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("plugins/anthology/.claude-plugin/plugin.json", r.stdout)
        self.assertIn("~0.1", r.stdout)
        self.assertEqual(self.state(), before)
        self.assertEqual(self.log.read_text(), "")

    def test_push_from_detached_head_is_refused_with_nothing_changed(self):
        self.git("checkout", "-q", "--detach")
        before = self.state()
        remote_before = self.remote_refs()
        r = self.release("anthology-guardrails", "0.1.1", "--push")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("checked-out branch", r.stdout)
        self.assertEqual(self.state(), before)
        self.assertEqual(self.remote_refs(), remote_before)
        self.assertEqual(self.log.read_text(), "")

    def test_rejected_branch_push_gives_a_recovery_hint_and_no_tag(self):
        other = Path(self._tmp.name) / "other"
        subprocess.run(["git", "clone", "-q", "-b", "main", str(self.remote), str(other)], check=True)
        for key, value in (("user.name", "Test"), ("user.email", "test@users.noreply.github.com"),
                           ("commit.gpgsign", "false")):
            self.git("config", key, value, cwd=other)
        (other / "diverge.txt").write_text("x\n", encoding="utf-8")
        self.git("add", "diverge.txt", cwd=other)
        self.git("commit", "-q", "-m", "diverge", cwd=other)
        self.git("push", "-q", "origin", "main", cwd=other)
        remote_before = self.remote_refs()
        r = self.release("anthology-guardrails", "0.1.1", "--push")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("release commit exists locally", r.stdout)
        self.assertIn("git push origin HEAD", r.stdout)
        self.assertIn("git reset --hard HEAD~1", r.stdout)
        self.assertEqual(self.git("tag", "-l"), "")
        self.assertEqual(self.remote_refs(), remote_before)
        self.assertEqual(self.git("log", "-1", "--format=%s"), "release: anthology-guardrails 0.1.1")

    def test_dry_run_failure_after_the_worktree_exists_cleans_up(self):
        self.write("CHANGELOG.md", FAKE_CHANGELOG.replace(
            "- G.\n", "- G.\n\n### [0.1.1] - 2026-01-01\n\n- old\n"))
        self.git("commit", "-q", "-a", "-m", "changelog")
        before = self.state()
        r = self.release("anthology-guardrails", "0.1.1", "--dry-run")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("already has a 0.1.1 section", r.stdout)
        self.assertNotIn("restored", r.stdout)
        self.assertEqual(self.state(), before)

    def test_dry_run_needs_no_identity_and_skips_hooks(self):
        # No user.name/user.email anywhere, and a pre-commit hook that would leave a marker.
        home = Path(self._tmp.name) / "home"
        home.mkdir()
        self.git("config", "--unset", "user.name")
        self.git("config", "--unset", "user.email")
        marker = Path(self._tmp.name) / "hook-ran"
        hook = self.repo / ".git" / "hooks" / "pre-commit"
        hook.write_text(f"#!/bin/sh\ntouch '{marker}'\n", encoding="utf-8")
        hook.chmod(0o755)
        env = self.env()
        env.update({"HOME": str(home), "XDG_CONFIG_HOME": str(home),
                    "GIT_CONFIG_NOSYSTEM": "1"})
        r = subprocess.run(["bash", str(self.repo / "scripts" / "release.sh"),
                            "anthology-guardrails", "0.1.1", "--dry-run"],
                           cwd=str(self.repo), env=env, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, universal_newlines=True)
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertFalse(marker.exists())
        local = subprocess.run(["git", "-C", str(self.repo), "config", "--local", "--get-regexp",
                                "^user[.]"], stdout=subprocess.PIPE, universal_newlines=True)
        self.assertEqual(local.stdout, "")  # the placeholder identity was never written

    def test_push_publishes_branch_and_tag(self):
        r = self.release("anthology-guardrails", "0.1.1", "--push")
        self.assertEqual(r.returncode, 0, r.stdout)
        head = self.git("rev-parse", "HEAD")
        refs = self.remote_refs()
        self.assertIn(f"{head} commit\trefs/heads/main", refs)
        self.assertIn("refs/tags/anthology-guardrails--v0.1.1", refs)
        self.assertIn("--push --remote origin", self.log.read_text())

    def test_superpowers_release_also_updates_sources_json(self):
        r = self.release("anthology-superpowers", "0.1.1")
        self.assertEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.read("vendor/sources.json"),
                         SOURCES.replace('"plugin_version": "0.1.0"',
                                         '"plugin_version": "0.1.1"'))
        self.assertEqual(self.git("show", "--name-only", "--format=", "HEAD").splitlines(),
                         ["CHANGELOG.md",
                          "plugins/anthology-superpowers/.claude-plugin/plugin.json",
                          "vendor/sources.json"])
        self.assertEqual(self.read("plugins/anthology-superpowers/.claude-plugin/plugin.json"),
                         MANIFESTS["anthology-superpowers"].replace("0.1.0", "0.1.1"))


if __name__ == "__main__":
    unittest.main()
