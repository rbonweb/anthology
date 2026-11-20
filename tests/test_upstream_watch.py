"""Tests for scripts/upstream_watch.py.

Everything runs against fixtures (tests/fixtures/upstream_watch) and injected fakes: the Docker
test images have neither git nor network. The one test that needs git is skipped there.
"""
from __future__ import annotations

import hashlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import upstream_watch as uw  # noqa: E402

FIX = ROOT / "tests" / "fixtures" / "upstream_watch"
SP_REPO = "https://github.com/obra/superpowers"
SKILLS_REPO = "https://github.com/anthropics/skills"
RTK_REPO = "https://github.com/rtk-ai/rtk"
NPM_URL = "https://registry.npmjs.org/@anthropic-ai/claude-code/latest"
S = {c: c * 40 for c in "123456789abcdef"}  # 40-hex fillers
OLD_SUM = hashlib.sha256((FIX / "manifest_old.txt").read_bytes()).hexdigest()
NEW_SUM = hashlib.sha256((FIX / "manifest_new.txt").read_bytes()).hexdigest()
UI_URL = "https://uizze.sh/.well-known/skills/ui-taste"
UI_MANIFEST = UI_URL + "/CHECKSUMS.sha256"


def fixture(name):
    return (FIX / name).read_bytes()


def refs_of(name):
    return uw.parse_ls_remote(fixture(name).decode("utf-8"))


def compare_url(repo, old, new):
    owner, name = repo.split("github.com/")[1].split("/")
    return f"https://api.github.com/repos/{owner}/{name}/compare/{old}...{new}"


def compare_json(files, **extra):
    data = {"status": "ahead", "ahead_by": 1, "total_commits": 1, "commits": [],
            "files": [{"filename": f, "status": "modified"} for f in files]}
    data.update(extra)
    return json.dumps(data).encode()


class FakeNet:
    """Serves ls-remote results and HTTP bodies from dicts; records the calls."""

    def __init__(self, refs=None, http=None):
        self.refs = dict(refs or {})
        self.http = dict(http or {})
        self.ls_calls = []
        self.http_calls = []

    def ls_remote(self, repo):
        self.ls_calls.append(repo)
        value = self.refs[repo]
        if isinstance(value, Exception):
            raise value
        return value

    def http_get(self, url, headers=None):
        self.http_calls.append(url)
        value = self.http[url]
        if isinstance(value, Exception):
            raise value
        return value


class FakeGitHub:
    """Records every call; `issues` is what the 'open issues' listing returns."""

    def __init__(self, issues=()):
        self.issues = list(issues)
        self.calls = []

    def list_open_issues(self):
        self.calls.append(("list",))
        return list(self.issues)

    def create_issue(self, title, body):
        self.calls.append(("create", title, body))
        return {"number": 900 + len(self.calls)}

    def update_issue(self, number, title, body):
        self.calls.append(("update", number, title, body))
        return {"number": number}

    def writes(self):
        return [c for c in self.calls if c[0] in ("create", "update")]


def git_entry(**kw):
    entry = {"name": "frontend-design", "repo": SKILLS_REPO, "path": "skills/frontend-design",
             "ref": "main", "commit": S["1"], "license": "Apache-2.0", "target": "t", "transform": []}
    entry.update(kw)
    return entry


def tag_entry(**kw):
    entry = git_entry(name="superpowers", repo=SP_REPO, path="", ref="v7.0.2", commit=S["5"])
    entry.update(kw)
    return entry


def url_entry(**kw):
    entry = {"name": "ui-taste", "url": UI_URL, "checksum": OLD_SUM, "license": "MIT", "target": "t"}
    entry.update(kw)
    return entry


def rtk_entry(ref="v1.2.1", commit=S["3"]):
    return {"name": "rtk", "managed_by": "subtree", "target": "rtk", "license": "Apache-2.0",
            "fork": {"repo": "https://github.com/rbonweb/rtk", "ref": "develop", "commit": S["a"]},
            "upstream": {"repo": RTK_REPO, "ref": ref, "commit": commit}}


def make_root(sources=None, claude="2.1.329", snapshot=True):
    """A throwaway repo root: vendor/sources.json, ci/claude-code/package.json, a snapshot."""
    tmp = tempfile.TemporaryDirectory()
    root = Path(tmp.name)
    (root / "vendor").mkdir()
    (root / "ci" / "claude-code").mkdir(parents=True)
    data = sources if sources is not None else json.loads((FIX / "sources.json").read_text())["sources"]
    (root / "vendor" / "sources.json").write_text(json.dumps({"sources": data}))
    (root / "ci" / "claude-code" / "package.json").write_text(json.dumps(
        {"dependencies": {"@anthropic-ai/claude-code": claude}}))
    if snapshot:
        shutil.copytree(FIX / "snapshots", root / "vendor" / "snapshots")
    return tmp, root


def check(entry, net, root=None):
    return uw.Checker(root or ROOT, net).check_entry(entry)


# ---------------------------------------------------------------------------------------------

class ParseLsRemoteTests(unittest.TestCase):
    def test_parses_refs_and_peeled_entries(self):
        refs = refs_of("ls_remote_tags.txt")
        self.assertEqual(refs["refs/heads/main"], S["1"])
        self.assertEqual(refs["refs/tags/v7.1.0"], S["6"])
        self.assertEqual(refs["refs/tags/v7.1.0^{}"], S["7"])

    def test_ignores_malformed_lines(self):
        text = f"nothex\trefs/tags/v1.0.0\n{S['1']}\n\n{S['2']}\trefs/tags/v2.0.0\n{S['3']}  refs/tags/x\n"
        self.assertEqual(uw.parse_ls_remote(text), {"refs/tags/v2.0.0": S["2"]})


class TagPinnedTests(unittest.TestCase):
    def net(self, name="ls_remote_tags.txt", drop=(), http=None):
        refs = refs_of(name)
        for key in drop:
            refs.pop(key)
        return FakeNet({SP_REPO: refs}, http)

    def test_newer_tag_found_with_peeled_commit(self):
        net = self.net(drop=("refs/tags/v7.10.0",), http={
            compare_url(SP_REPO, S["5"], S["7"]): compare_json(["skills/a/SKILL.md", "README.md"])})
        change = check(tag_entry(), net)
        self.assertEqual((change.key, change.old, change.new), ("superpowers", "v7.0.2", "v7.1.0"))
        self.assertEqual(change.new_target, "v7.1.0")
        self.assertIn(S["7"], uw.render_body(change))  # peeled commit, not the tag object
        self.assertNotIn(S["6"], uw.render_body(change))
        self.assertEqual(change.command, "python3 scripts/vendor.py sync superpowers --ref v7.1.0")
        self.assertEqual(change.files, ["skills/a/SKILL.md", "README.md"])

    def test_semver_order_not_string_order_and_lightweight_tag(self):
        net = self.net(http={compare_url(SP_REPO, S["5"], S["a"]): compare_json(["x.md"])})
        change = check(tag_entry(), net)
        self.assertEqual(change.new, "v7.10.0")  # 7.10 > 7.1
        self.assertIn(S["a"], uw.render_body(change))

    def test_prereleases_and_non_version_tags_ignored(self):
        net = self.net(drop=("refs/tags/v7.10.0", "refs/tags/v7.1.0", "refs/tags/v7.1.0^{}"))
        self.assertIsNone(check(tag_entry(), net))  # only rc/beta/nightly are newer

    def test_up_to_date(self):
        net = self.net("ls_remote_up_to_date.txt")
        self.assertIsNone(check(tag_entry(), net))
        self.assertEqual(net.http_calls, [])

    def test_keeps_the_pin_prefix_convention(self):
        net = FakeNet({SP_REPO: refs_of("ls_remote_no_prefix.txt")},
                      {compare_url(SP_REPO, S["5"], S["2"]): compare_json([])})
        change = check(tag_entry(ref="1.2.0"), net)
        self.assertEqual(change.new, "1.3")  # not v1.9.0
        self.assertIn("--ref 1.3", change.command)

    def test_unprefixed_pin_two_components(self):
        net = FakeNet({SP_REPO: refs_of("ls_remote_tags.txt")},
                      {compare_url(SP_REPO, S["5"], S["a"]): compare_json([])})
        self.assertEqual(check(tag_entry(ref="v7.0"), net).new, "v7.10.0")

    def test_compare_failure_degrades_to_a_note(self):
        net = self.net(drop=("refs/tags/v7.10.0",))  # no compare response registered
        change = check(tag_entry(), net)
        self.assertEqual(change.new, "v7.1.0")
        self.assertTrue(any("unavailable" in n for n in change.notes))

    def test_path_filter_applies_to_tag_diffs(self):
        net = self.net(drop=("refs/tags/v7.10.0",), http={
            compare_url(SP_REPO, S["5"], S["7"]): fixture("compare_path_changed.json")})
        change = check(tag_entry(path="skills/frontend-design"), net)
        self.assertEqual(change.files, ["skills/frontend-design/SKILL.md",
                                        "skills/frontend-design/examples/hero.md"])

    def test_missing_tag_list_is_an_error(self):
        net = FakeNet({SP_REPO: {"refs/heads/main": S["1"]}})
        with self.assertRaises(uw.WatchError):
            check(tag_entry(), net)


class BranchPinnedTests(unittest.TestCase):
    def net(self, head, compare=None, repo=SKILLS_REPO, old=S["1"]):
        http = {compare_url(repo, old, head): compare} if compare is not None else {}
        return FakeNet({repo: {"refs/heads/main": head}}, http)

    def test_moved_with_changes_under_path(self):
        net = self.net(S["2"], fixture("compare_path_changed.json"))
        change = check(git_entry(), net)
        self.assertEqual((change.old, change.new), (S["1"][:12], S["2"][:12]))
        self.assertEqual(change.files, ["skills/frontend-design/SKILL.md",
                                        "skills/frontend-design/examples/hero.md"])
        self.assertEqual(change.command, "python3 scripts/vendor.py sync frontend-design --ref main")
        self.assertIn(S["2"], uw.render_body(change))

    def test_prefix_match_is_by_directory(self):
        net = self.net(S["2"], fixture("compare_path_changed.json"))
        change = check(git_entry(), net)
        self.assertNotIn("skills/frontend-designer/SKILL.md", change.files)

    def test_unchanged_head_makes_no_api_call(self):
        net = self.net(S["1"])
        self.assertIsNone(check(git_entry(), net))
        self.assertEqual(net.http_calls, [])

    def test_moved_elsewhere_is_no_change(self):
        net = self.net(S["2"], fixture("compare_other_paths.json"))
        self.assertIsNone(check(git_entry(), net))

    def test_empty_path_means_whole_repo(self):
        net = self.net(S["2"], fixture("compare_other_paths.json"))
        change = check(git_entry(path=""), net)
        self.assertEqual(change.files, ["skills/other-skill/SKILL.md", "README.md"])

    def test_truncated_compare_is_a_change_with_a_note(self):
        files = ["elsewhere/f%d.md" % i for i in range(300)]
        net = self.net(S["2"], compare_json(files))
        change = check(git_entry(), net)  # nothing under path is visible, but 300 means truncated
        self.assertIsNotNone(change)
        self.assertIn("diff too large to summarise", uw.render_body(change))

    def test_non_github_repo_is_a_change_with_commit_range_only(self):
        repo = "https://git.example.test/team/skills"
        net = FakeNet({repo: {"refs/heads/main": S["2"]}})
        change = check(git_entry(repo=repo), net)
        self.assertEqual(net.http_calls, [])
        self.assertIn(S["1"], uw.render_body(change))
        self.assertIn(S["2"], uw.render_body(change))
        self.assertEqual(change.files, [])

    def test_missing_branch_is_an_error(self):
        net = FakeNet({SKILLS_REPO: {"refs/heads/other": S["2"]}})
        with self.assertRaises(uw.WatchError):
            check(git_entry(), net)

    def test_compare_is_cached_per_repo_and_commits(self):
        net = self.net(S["2"], fixture("compare_path_changed.json"))
        checker = uw.Checker(ROOT, net)
        a = checker.check_entry(git_entry(name="a-one", path="skills/frontend-design"))
        b = checker.check_entry(git_entry(name="a-two", path="skills/other-skill"))
        self.assertEqual(len(net.http_calls), 1)
        self.assertEqual(len(net.ls_calls), 1)
        self.assertEqual(a.files[0], "skills/frontend-design/SKILL.md")
        self.assertEqual(b.files, ["skills/other-skill/SKILL.md"])

    def test_dot_git_suffix_is_stripped_for_the_api(self):
        net = FakeNet({SKILLS_REPO + ".git": {"refs/heads/main": S["2"]}},
                      {compare_url(SKILLS_REPO, S["1"], S["2"]): compare_json(["skills/frontend-design/a.md"])})
        self.assertIsNotNone(check(git_entry(repo=SKILLS_REPO + ".git"), net))

    def test_compare_error_is_an_error_when_it_decides_the_verdict(self):
        net = FakeNet({SKILLS_REPO: {"refs/heads/main": S["2"]}})
        with self.assertRaises(uw.WatchError):
            check(git_entry(), net)

    def test_commit_messages_and_authors_never_appear(self):
        net = self.net(S["2"], fixture("compare_path_changed.json"))
        change = check(git_entry(), net)
        text = uw.render_title(change) + uw.render_body(change) + json.dumps(change.to_json())
        for needle in ("SECRET-COMMIT-MESSAGE", "mallory", "Mallory"):
            self.assertNotIn(needle, text)


class UrlSourceTests(unittest.TestCase):
    def test_changed_checksum_with_file_level_diff(self):
        tmp, root = make_root()
        self.addCleanup(tmp.cleanup)
        net = FakeNet(http={UI_MANIFEST: fixture("manifest_new.txt")})
        change = check(url_entry(), net, root)
        self.assertEqual((change.key, change.old, change.new), ("ui-taste", OLD_SUM[:12], NEW_SUM[:12]))
        self.assertEqual(net.http_calls, [UI_MANIFEST])
        self.assertEqual(change.files, ["+ references/added.md", "- references/removed.md",
                                        "~ references/changed.md"])
        self.assertIn(NEW_SUM, uw.render_body(change))
        self.assertEqual(change.command, (
            f'set "checksum" of ui-taste in vendor/sources.json to {NEW_SUM}, then run: '
            "python3 scripts/vendor.py fetch ui-taste && python3 scripts/vendor.py build ui-taste"))

    def test_unchanged(self):
        net = FakeNet(http={UI_MANIFEST: fixture("manifest_old.txt")})
        self.assertIsNone(check(url_entry(), net))

    def test_url_already_naming_the_manifest_is_used_as_is(self):
        url = "https://example.test/skills/ui-taste/CHECKSUMS.sha256"
        net = FakeNet(http={url: fixture("manifest_old.txt")})
        self.assertIsNone(check(url_entry(url=url), net))
        self.assertEqual(net.http_calls, [url])

    def test_trailing_slash(self):
        net = FakeNet(http={UI_MANIFEST: fixture("manifest_old.txt")})
        self.assertIsNone(check(url_entry(url=UI_URL + "/"), net))

    def test_missing_snapshot_is_noted(self):
        tmp, root = make_root(snapshot=False)
        self.addCleanup(tmp.cleanup)
        net = FakeNet(http={UI_MANIFEST: fixture("manifest_new.txt")})
        change = check(url_entry(), net, root)
        self.assertEqual(change.files, [])
        self.assertTrue(any("file-level diff unavailable" in n for n in change.notes))

    def test_fetch_failure_is_an_error(self):
        net = FakeNet(http={UI_MANIFEST: OSError("boom")})
        with self.assertRaises(uw.WatchError):
            check(url_entry(), net)

    def test_non_https_url_is_an_error(self):
        with self.assertRaises(uw.WatchError):
            check(url_entry(url="file:///etc/passwd"), FakeNet())

    def test_bad_checksum_in_sources_is_an_error(self):
        with self.assertRaises(uw.WatchError):
            check(url_entry(checksum="zz"), FakeNet(http={UI_MANIFEST: b""}))


class SubtreeTests(unittest.TestCase):
    def test_newer_upstream_tag(self):
        refs = {"refs/tags/v1.2.1": S["3"], "refs/tags/v1.3.0": S["8"], "refs/tags/v1.3.0^{}": S["9"],
                "refs/tags/v1.4.0-rc.1": S["a"], "refs/heads/develop": S["b"]}
        net = FakeNet({RTK_REPO: refs}, {compare_url(RTK_REPO, S["3"], S["9"]): compare_json(["src/main.rs"])})
        change = check(rtk_entry(), net)
        self.assertEqual((change.key, change.old, change.new), ("rtk", "v1.2.1", "v1.3.0"))
        self.assertEqual(change.command, "scripts/sync-rtk.sh v1.3.0")
        self.assertEqual(net.ls_calls, [RTK_REPO])  # the fork is ours: never queried
        self.assertEqual(change.files, ["src/main.rs"])

    def test_up_to_date(self):
        net = FakeNet({RTK_REPO: {"refs/tags/v1.2.1": S["3"]}})
        self.assertIsNone(check(rtk_entry(), net))


class ClaudeCodeTests(unittest.TestCase):
    def check(self, npm, pinned="2.1.329"):
        tmp, root = make_root(claude=pinned)
        self.addCleanup(tmp.cleanup)
        return uw.Checker(root, FakeNet(http={NPM_URL: npm})).check_claude_code()

    def test_newer(self):
        change = self.check(fixture("npm_latest.json"))
        self.assertEqual((change.key, change.old, change.new), ("claude-code", "2.1.329", "2.1.400"))
        self.assertEqual(change.command, (
            'set "@anthropic-ai/claude-code" in ci/claude-code/package.json to 2.1.400, run '
            "npm install --package-lock-only in ci/claude-code, and let CI (plugin-validate) and "
            "evals run on the PR"))
        self.assertIn("https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md",
                      uw.render_body(change))

    def test_equal_and_older(self):
        self.assertIsNone(self.check(fixture("npm_same.json")))
        self.assertIsNone(self.check(fixture("npm_older.json")))

    def test_invalid_version_from_registry_is_an_error(self):
        with self.assertRaises(uw.WatchError):
            self.check(b'{"version": "2.1.400`x`"}')
        with self.assertRaises(uw.WatchError):
            self.check(b"not json")

    def test_prerelease_latest_is_not_newer_than_its_release(self):
        self.assertIsNone(self.check(b'{"version": "2.1.329-beta.1"}'))


class CheckAllTests(unittest.TestCase):
    def test_unknown_shape_errors_and_the_rest_continue(self):
        tmp, root = make_root()
        self.addCleanup(tmp.cleanup)
        refs = refs_of("ls_remote_tags.txt")
        net = FakeNet(
            {SP_REPO: refs, SKILLS_REPO: {"refs/heads/main": S["1"]}.copy(),
             RTK_REPO: {"refs/tags/v1.2.1": S["3"]}},
            {compare_url(SP_REPO, S["5"], S["a"]): compare_json(["x.md"]),
             UI_MANIFEST: fixture("manifest_new.txt"), NPM_URL: fixture("npm_latest.json")})
        # frontend-design's pinned commit in the fixture differs from S["1"]; keep it unmoved
        sources = json.loads((root / "vendor" / "sources.json").read_text())
        net.refs[SKILLS_REPO] = {"refs/heads/main": sources["sources"][1]["commit"]}
        changes, errors = uw.check_all(root, net)
        self.assertEqual([c.key for c in changes], ["superpowers", "ui-taste", "claude-code"])
        self.assertEqual([e["key"] for e in errors], ["mystery"])
        self.assertIn("unrecognised source shape", errors[0]["error"])

    def test_one_failing_network_entry_does_not_stop_the_others(self):
        tmp, root = make_root([tag_entry(), url_entry()])
        self.addCleanup(tmp.cleanup)
        net = FakeNet({SP_REPO: OSError("network down")},
                      {UI_MANIFEST: fixture("manifest_new.txt"), NPM_URL: fixture("npm_same.json")})
        changes, errors = uw.check_all(root, net)
        self.assertEqual([c.key for c in changes], ["ui-taste"])
        self.assertEqual([e["key"] for e in errors], ["superpowers"])

    def test_claude_code_failure_is_recorded_under_its_key(self):
        tmp, root = make_root([])
        self.addCleanup(tmp.cleanup)
        changes, errors = uw.check_all(root, FakeNet(http={NPM_URL: OSError("down")}))
        self.assertEqual((changes, [e["key"] for e in errors]), ([], ["claude-code"]))

    def test_the_real_sources_file_is_readable_generically(self):
        entries = json.loads((ROOT / "vendor" / "sources.json").read_text())["sources"]
        for entry in entries:
            self.assertIn(uw.classify(entry), ("git", "url", "subtree"), entry["name"])


class HostileInputTests(unittest.TestCase):
    def test_hostile_tags_are_never_selected_or_rendered(self):
        net = FakeNet({SP_REPO: refs_of("ls_remote_tags.txt")},
                      {compare_url(SP_REPO, S["5"], S["a"]): compare_json(["x.md"])})
        change = check(tag_entry(), net)
        text = uw.render_title(change) + uw.render_body(change) + json.dumps(change.to_json())
        for needle in ("touch-x", "$(id)", "octocat", "`touch"):
            self.assertNotIn(needle, text)

    def test_hostile_pin_ref_errors(self):
        for ref in ("v1.0.0`x`", "main\nINJECT", "$(id)", "a b", "@octocat", "-rf", ""):
            with self.assertRaises(uw.WatchError, msg=repr(ref)):
                check(git_entry(ref=ref), FakeNet({SKILLS_REPO: {"refs/heads/main": S["1"]}}))

    def test_hostile_name_errors(self):
        for name in ("a`b", "a b", "x\ny", "../x", "a$(id)", ""):
            with self.assertRaises(uw.WatchError, msg=repr(name)):
                check(git_entry(name=name), FakeNet())

    def test_hostile_commit_sha_errors(self):
        with self.assertRaises(uw.WatchError):
            check(git_entry(commit="not-a-sha"), FakeNet({SKILLS_REPO: {"refs/heads/main": S["2"]}}))

    def test_hostile_head_sha_from_upstream_errors(self):
        net = FakeNet({SKILLS_REPO: {"refs/heads/main": "zzz"}})
        with self.assertRaises(uw.WatchError):
            check(git_entry(), net)

    def test_hostile_repo_url_errors(self):
        for repo in ("--upload-pack=touch x", "file:///etc", "https://x/y z", "ssh://h/r"):
            with self.assertRaises(uw.WatchError, msg=repo):
                check(git_entry(repo=repo), FakeNet())

    def test_hostile_file_paths_are_sanitised_inside_the_fence(self):
        net = FakeNet({SKILLS_REPO: {"refs/heads/main": S["2"]}},
                      {compare_url(SKILLS_REPO, S["1"], S["2"]): fixture("compare_hostile_paths.json")})
        change = check(git_entry(path="docs"), net)
        body = uw.render_body(change)
        fence = body.split("```text\n")[1].split("\n```")[0]
        self.assertEqual(fence.count("\n"), 1)  # two paths, no injected newline
        self.assertNotIn("`", fence)
        self.assertNotIn("\x07", body)
        self.assertEqual(fence.splitlines()[1], "docs/ok.md")

    def test_sanitise_helpers(self):
        self.assertEqual(uw.sanitise_path("a`b\nc\x00d‮e"), "abcde")
        self.assertEqual(len(uw.sanitise_path("x" * 1000)), uw.MAX_PATH_LEN)


class RenderTests(unittest.TestCase):
    def change(self, **kw):
        base = dict(key="demo", kind="git", old="v1.0.0", new="v1.1.0", new_target="v1.1.0",
                    details=[("Source", "https://github.com/o/r"), ("New commit", S["7"])],
                    command="python3 scripts/vendor.py sync demo --ref v1.1.0")
        base.update(kw)
        return uw.Change(**base)

    def test_title_and_marker(self):
        change = self.change()
        self.assertEqual(uw.render_title(change), "Upstream: demo v1.0.0 → v1.1.0")
        self.assertTrue(uw.render_body(change).startswith("<!-- upstream-watch:demo -->\n"))

    def test_body_has_command_and_gate_text(self):
        body = uw.render_body(self.change())
        self.assertIn("python3 scripts/vendor.py sync demo --ref v1.1.0", body)
        self.assertIn("on a branch", body)
        self.assertIn("CI and the evals are the gate", body)

    def test_body_is_deterministic(self):
        self.assertEqual(uw.render_body(self.change()), uw.render_body(self.change()))

    def test_extra_sections_come_before_the_command(self):
        body = uw.render_body(self.change(), extra_sections=[("Startup budget", "+3 tokens")])
        self.assertIn("### Startup budget\n+3 tokens", body)
        self.assertLess(body.index("### Startup budget"), body.index("scripts/vendor.py sync"))
        self.assertNotIn("Startup budget", uw.render_body(self.change()))

    def test_at_most_50_files_and_a_remainder_note(self):
        files = [f"f{i:03}.md" for i in range(80)]
        body = uw.render_body(self.change(files=files, files_total=80))
        self.assertIn("f049.md", body)
        self.assertNotIn("f050.md", body)
        self.assertIn("30 more", body)

    def test_json_shape(self):
        data = self.change().to_json()
        for key in ("key", "kind", "old", "new", "new_target", "command", "files", "notes", "title"):
            self.assertIn(key, data)
        json.dumps(data)


class SyncIssuesTests(unittest.TestCase):
    def change(self, key="demo", new="v1.1.0"):
        return uw.Change(key=key, kind="git", old="v1.0.0", new=new, new_target=new,
                         details=[("Source", "https://github.com/o/r")],
                         command=f"python3 scripts/vendor.py sync {key} --ref {new}")

    def issue(self, change, number=7, **kw):
        issue = {"number": number, "title": uw.render_title(change), "body": uw.render_body(change)}
        issue.update(kw)
        return issue

    def test_creates_when_absent(self):
        gh = FakeGitHub()
        actions = uw.sync_issues([self.change()], gh)
        self.assertEqual([c[0] for c in gh.calls], ["list", "create"])
        self.assertEqual(gh.calls[1][1], uw.render_title(self.change()))
        self.assertEqual(gh.calls[1][2], uw.render_body(self.change()))
        self.assertEqual([(a["key"], a["action"]) for a in actions], [("demo", "create")])

    def test_noop_when_identical(self):
        gh = FakeGitHub([self.issue(self.change())])
        actions = uw.sync_issues([self.change()], gh)
        self.assertEqual(gh.writes(), [])
        self.assertEqual(actions[0]["action"], "unchanged")
        self.assertEqual(actions[0]["number"], 7)

    def test_noop_tolerates_crlf_and_trailing_whitespace(self):
        body = uw.render_body(self.change()).replace("\n", "\r\n") + "\r\n"
        gh = FakeGitHub([self.issue(self.change(), body=body)])
        uw.sync_issues([self.change()], gh)
        self.assertEqual(gh.writes(), [])

    def test_patches_when_upstream_moved(self):
        gh = FakeGitHub([self.issue(self.change(new="v1.0.5"), number=12)])
        actions = uw.sync_issues([self.change()], gh)
        self.assertEqual(gh.writes(), [("update", 12, uw.render_title(self.change()),
                                        uw.render_body(self.change()))])
        self.assertEqual(actions[0]["action"], "update")

    def test_ignores_other_keys_and_pull_requests(self):
        other = self.issue(self.change(key="other"), number=3)
        pull = self.issue(self.change(), number=4, pull_request={"url": "x"})
        unmarked = {"number": 5, "title": "Upstream: demo", "body": "no marker here"}
        gh = FakeGitHub([other, pull, unmarked])
        actions = uw.sync_issues([self.change()], gh)
        self.assertEqual([c[0] for c in gh.writes()], ["create"])
        self.assertEqual(actions[0]["action"], "create")

    def test_marker_must_start_the_body(self):
        quoted = {"number": 6, "title": "t", "body": "see <!-- upstream-watch:demo --> in the text"}
        gh = FakeGitHub([quoted])
        uw.sync_issues([self.change()], gh)
        self.assertEqual([c[0] for c in gh.writes()], ["create"])

    def test_dry_run_reads_but_never_writes(self):
        gh = FakeGitHub([self.issue(self.change(new="v1.0.5"), number=12)])
        actions = uw.sync_issues([self.change(), self.change(key="fresh")], gh, dry_run=True)
        self.assertEqual(gh.writes(), [])
        self.assertEqual([a["action"] for a in actions], ["update", "create"])

    def test_dry_run_without_a_client(self):
        actions = uw.sync_issues([self.change()], None, dry_run=True)
        self.assertEqual(actions[0]["action"], "create-or-update")

    def test_write_failure_is_recorded_and_the_rest_continue(self):
        class Flaky(FakeGitHub):
            def create_issue(self, title, body):
                if "bad" in title:
                    raise OSError("422")
                return super().create_issue(title, body)

        gh = Flaky()
        actions = uw.sync_issues([self.change(key="bad"), self.change(key="good")], gh)
        self.assertEqual([(a["key"], a["action"]) for a in actions], [("bad", "error"), ("good", "create")])


class MainTests(unittest.TestCase):
    def run_main(self, argv, env=None, net=None, gh=None, root=None):
        out = io.StringIO()
        code = uw.main(argv, env=env or {}, net=net, gh=gh, stdout=out)
        return code, out.getvalue()

    def fake_world(self, claude_new=True, sources=None):
        tmp, root = make_root(sources if sources is not None else [tag_entry(), url_entry()])
        self.addCleanup(tmp.cleanup)
        refs = refs_of("ls_remote_tags.txt")
        net = FakeNet({SP_REPO: refs}, {
            compare_url(SP_REPO, S["5"], S["a"]): compare_json(["x.md"]),
            UI_MANIFEST: fixture("manifest_new.txt"),
            NPM_URL: fixture("npm_latest.json" if claude_new else "npm_same.json")})
        return root, net

    def test_dry_run_prints_issues_and_makes_no_writes(self):
        root, net = self.fake_world()
        gh = FakeGitHub()
        code, out = self.run_main(["--dry-run", "--root", str(root)], net=net, gh=gh)
        self.assertEqual(code, 0)
        self.assertEqual(gh.writes(), [])
        for needle in ("Upstream: superpowers v7.0.2", "Upstream: ui-taste", "Upstream: claude-code 2.1.329",
                       "<!-- upstream-watch:superpowers -->"):
            self.assertIn(needle, out)

    def test_dry_run_needs_no_token_or_repo(self):
        root, net = self.fake_world()
        code, _ = self.run_main(["--dry-run", "--root", str(root)], env={}, net=net)
        self.assertEqual(code, 0)

    def test_dry_run_does_not_write_github_output(self):
        root, net = self.fake_world()
        with tempfile.TemporaryDirectory() as d:
            out_file = Path(d) / "out"
            self.run_main(["--dry-run", "--root", str(root)], env={"GITHUB_OUTPUT": str(out_file)}, net=net)
            self.assertFalse(out_file.exists())

    def test_real_mode_requires_token_and_repo(self):
        root, net = self.fake_world()
        code, out = self.run_main(["--root", str(root)], env={}, net=net)
        self.assertEqual(code, 2)
        self.assertIn("GITHUB_TOKEN", out)

    def test_real_mode_syncs_and_flags_claude_code(self):
        root, net = self.fake_world()
        gh = FakeGitHub()
        with tempfile.TemporaryDirectory() as d:
            out_file = Path(d) / "out"
            env = {"GITHUB_TOKEN": "t", "GITHUB_REPOSITORY": "o/r", "GITHUB_OUTPUT": str(out_file)}
            code, _ = self.run_main(["--root", str(root)], env=env, net=net, gh=gh)
            self.assertEqual(code, 0)
            self.assertEqual(len(gh.writes()), 3)
            self.assertEqual(out_file.read_text(), "claude_code_changed=true\n")

    def test_output_false_when_the_claude_code_issue_already_matches(self):
        root, net = self.fake_world()
        tmp = uw.Checker(root, net).check_claude_code()
        gh = FakeGitHub([{"number": 9, "title": uw.render_title(tmp), "body": uw.render_body(tmp)}])
        with tempfile.TemporaryDirectory() as d:
            out_file = Path(d) / "out"
            env = {"GITHUB_TOKEN": "t", "GITHUB_REPOSITORY": "o/r", "GITHUB_OUTPUT": str(out_file)}
            self.run_main(["--root", str(root)], env=env, net=net, gh=gh)
            self.assertEqual(out_file.read_text(), "claude_code_changed=false\n")
            self.assertEqual(len(gh.writes()), 2)  # the other two were created

    def test_output_true_when_the_claude_code_issue_moved(self):
        root, net = self.fake_world()
        old = uw.Checker(root, FakeNet(http={NPM_URL: b'{"version": "2.1.350"}'})).check_claude_code()
        gh = FakeGitHub([{"number": 9, "title": uw.render_title(old), "body": uw.render_body(old)}])
        with tempfile.TemporaryDirectory() as d:
            out_file = Path(d) / "out"
            env = {"GITHUB_TOKEN": "t", "GITHUB_REPOSITORY": "o/r", "GITHUB_OUTPUT": str(out_file)}
            self.run_main(["--root", str(root)], env=env, net=net, gh=gh)
            self.assertEqual(out_file.read_text(), "claude_code_changed=true\n")
            self.assertIn("update", [c[0] for c in gh.calls])

    def test_output_false_when_no_new_release(self):
        root, net = self.fake_world(claude_new=False)
        with tempfile.TemporaryDirectory() as d:
            out_file = Path(d) / "out"
            env = {"GITHUB_TOKEN": "t", "GITHUB_REPOSITORY": "o/r", "GITHUB_OUTPUT": str(out_file)}
            self.run_main(["--root", str(root)], env=env, net=net, gh=FakeGitHub())
            self.assertEqual(out_file.read_text(), "claude_code_changed=false\n")

    def test_errors_exit_1_after_issues_are_synced(self):
        root, net = self.fake_world(sources=[tag_entry(), {"name": "mystery", "target": "t"}])
        gh = FakeGitHub()
        env = {"GITHUB_TOKEN": "t", "GITHUB_REPOSITORY": "o/r"}
        code, out = self.run_main(["--root", str(root)], env=env, net=net, gh=gh)
        self.assertEqual(code, 1)
        self.assertEqual(len(gh.writes()), 2)  # superpowers + claude-code were still synced
        self.assertIn("mystery", out)

    def test_json_output(self):
        root, net = self.fake_world(sources=[tag_entry(), {"name": "mystery", "target": "t"}])
        with tempfile.TemporaryDirectory() as d:
            target = Path(d) / "watch.json"
            code, _ = self.run_main(["--dry-run", "--json", str(target), "--root", str(root)], net=net)
            data = json.loads(target.read_text())
        self.assertEqual(code, 1)
        self.assertEqual([c["key"] for c in data["changes"]], ["superpowers", "claude-code"])
        self.assertEqual([e["key"] for e in data["errors"]], ["mystery"])

    def test_no_changes_prints_so_and_exits_0(self):
        root, net = self.fake_world(claude_new=False, sources=[])
        code, out = self.run_main(["--dry-run", "--root", str(root)], net=net)
        self.assertEqual(code, 0)
        self.assertIn("no upstream changes", out.lower())


class _Handler(BaseHTTPRequestHandler):
    log = []
    pages = {}

    def _reply(self, status, payload):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        type(self).log.append(("GET", self.path, dict(self.headers), None))
        query = parse_qs(urlparse(self.path).query)
        page = int(query.get("page", ["1"])[0])
        self._reply(200, type(self).pages.get(page, []))

    def _write(self, method):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        type(self).log.append((method, self.path, dict(self.headers), body))
        self._reply(201 if method == "POST" else 200, {"number": 42})

    def do_POST(self):
        self._write("POST")

    def do_PATCH(self):
        self._write("PATCH")

    def log_message(self, *args):
        pass


class GitHubClientTests(unittest.TestCase):
    def setUp(self):
        _Handler.log = []
        _Handler.pages = {}
        self.server = HTTPServer(("127.0.0.1", 0), _Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.client = uw.GitHubClient("o/r", "tok", "http://127.0.0.1:%d" % self.server.server_port)

    def test_paginates_open_issues_with_the_label_filter(self):
        _Handler.pages = {1: [{"number": i} for i in range(100)], 2: [{"number": 100}], 3: []}
        issues = self.client.list_open_issues()
        self.assertEqual(len(issues), 101)
        paths = [entry[1] for entry in _Handler.log]
        self.assertTrue(all("/repos/o/r/issues?labels=upstream-watch&state=open&per_page=100" in p for p in paths))
        self.assertEqual(len(paths), 2)  # a short second page ends it
        headers = {k.lower(): v for k, v in _Handler.log[0][2].items()}
        self.assertEqual(headers["authorization"], "Bearer tok")
        self.assertEqual(headers["accept"], "application/vnd.github+json")
        self.assertEqual(headers["x-github-api-version"], "2022-11-28")
        self.assertEqual(headers["user-agent"], "anthology-upstream-watch")

    def test_create_and_update_send_json(self):
        self.client.create_issue("T", "B")
        self.client.update_issue(7, "T2", "B2")
        (m1, p1, _, b1), (m2, p2, _, b2) = _Handler.log
        self.assertEqual((m1, p1, b1), ("POST", "/repos/o/r/issues",
                                        {"title": "T", "body": "B", "labels": ["upstream-watch"]}))
        self.assertEqual((m2, p2, b2), ("PATCH", "/repos/o/r/issues/7", {"title": "T2", "body": "B2"}))


class RealGitTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("git"), "git is not installed")
    def test_ls_remote_against_a_local_repository(self):
        with tempfile.TemporaryDirectory() as d:
            env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
                       GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")

            def git(*args):
                subprocess.run(["git", "-C", d, *args], check=True, capture_output=True, env=env)

            git("init", "-q", "-b", "main")
            git("commit", "-q", "--allow-empty", "-m", "one")
            git("tag", "-a", "v1.0.0", "-m", "annotated")
            git("tag", "v1.1.0")
            refs = uw.RealNet().ls_remote(d)
        self.assertIn("refs/heads/main", refs)
        self.assertIn("refs/tags/v1.0.0^{}", refs)  # annotated tag: peeled entry present
        self.assertIn("refs/tags/v1.1.0", refs)
        self.assertNotIn("refs/tags/v1.1.0^{}", refs)


if __name__ == "__main__":
    unittest.main()
