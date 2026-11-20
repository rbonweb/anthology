"""Checks on the repo's public documents and on what the repo is allowed to contain.

- LICENSE is the Apache License 2.0 text, unmodified.
- No tracked file holds a real home path (outside rtk/ and the generated vendored trees),
  and no file anywhere holds the owner's home path.
- No `.env*` file is tracked.
- SECURITY.md states the `Bash(rtk *)` allow and the memory trust model.
- CONTRIBUTING.md links the maintainer tools and the evals.
- The README carries the sentences the design requires.
- Every relative link in the public docs points at a file or folder that exists (and at a
  heading that exists, for `#fragment` links), and every `scripts/...` and
  `.github/workflows/...` path the docs name in code spans exists.

"Tracked files" is `git ls-files` when git is on PATH and the repo is a work tree. The Docker
test images have no git, so there the whole tree is walked instead.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Directories the walk fallback never enters (not repo content).
PRUNED = (".git", ".claude/worktrees", ".superpowers", ".vendor-cache", "__pycache__",
          "node_modules")

# The home-path check skips the upstream rtk tree and the generated vendored trees.
HOME_PATH_SKIPS = ("rtk/", "plugins/anthology-superpowers/", "plugins/anthology/knowledge/",
                   "vendor/snapshots/")
HOME_PATH = re.compile(rb"/(?:Users|home)/([A-Za-z0-9._-]+)/")
# Fixture placeholders, not real people.
PLACEHOLDER_NAMES = {"someone", "tester", "user", "username", "example", "runner"}

# Built by concatenation so this file never contains the strings it looks for.
OWNER_HOMES = ("/Users/" + "rbonweb", "/home/" + "rbonweb")

MARKDOWN_LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
HEADING = re.compile(r"^#{1,6}\s+(.*)$", re.MULTILINE)


PUBLIC_DOCS = ("README.md", "SECURITY.md", "CONTRIBUTING.md", "DECISIONS.md")
CODE_SPAN_PATH = re.compile(r"`((?:scripts|\.github/workflows)/[A-Za-z0-9._-]+)`")


def _is_git_work_tree() -> bool:
    if not shutil.which("git"):
        return False
    result = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--is-inside-work-tree"],
                            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                            universal_newlines=True)
    return result.returncode == 0 and result.stdout.strip() == "true"


def tracked_files() -> list:
    """Relative POSIX paths of the repo's files."""
    if _is_git_work_tree():
        out = subprocess.run(["git", "-C", str(ROOT), "ls-files", "-z"],
                             stdout=subprocess.PIPE, check=True).stdout
        names = [os.fsdecode(raw) for raw in out.split(b"\0") if raw]
        return sorted(n for n in names if (ROOT / n).is_file())
    found = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        rel_dir = Path(dirpath).relative_to(ROOT).as_posix()
        kept = []
        for d in dirnames:
            rel = d if rel_dir == "." else rel_dir + "/" + d
            if d in PRUNED or rel in PRUNED:
                continue
            kept.append(d)
        dirnames[:] = kept
        for f in filenames:
            rel = f if rel_dir == "." else rel_dir + "/" + f
            if rel == ".git":  # a linked worktree has a `.git` file, not a directory
                continue
            found.append(rel)
    return sorted(found)


def read(name: str) -> str:
    return (ROOT / name).read_text(encoding="utf-8")


def _clean(target: str) -> str:
    """A link target without its #fragment and without one leading `./`."""
    target = target.split("#")[0]
    return target[2:] if target.startswith("./") else target


def headings(text: str) -> list:
    return [m.group(1).strip() for m in HEADING.finditer(text)]


class LicenseTest(unittest.TestCase):
    def test_license_is_apache_2(self):
        text = read("LICENSE")
        self.assertTrue(text.lstrip().startswith("Apache License"))
        self.assertIn("Version 2.0, January 2004", text)

    def test_license_is_unmodified(self):
        # The appendix keeps its template line: nobody's name is filled in.
        self.assertIn("Copyright [yyyy] [name of copyright owner]", read("LICENSE"))


class RepoContentTest(unittest.TestCase):
    def test_file_listing_is_not_empty(self):
        self.assertIn("LICENSE", tracked_files())

    def test_no_real_home_paths(self):
        hits = []
        for rel in tracked_files():
            if rel.startswith(HOME_PATH_SKIPS):
                continue
            data = (ROOT / rel).read_bytes()
            for m in HOME_PATH.finditer(data):
                name = m.group(1).decode("ascii")
                if name.lower() not in PLACEHOLDER_NAMES:
                    hits.append(f"{rel}: {m.group(0).decode('ascii')}")
        self.assertEqual(hits, [], "real home path in a public file")

    def test_no_owner_home_path_anywhere(self):
        needles = [h.lower().encode("ascii") for h in OWNER_HOMES]
        hits = []
        for rel in tracked_files():
            data = (ROOT / rel).read_bytes().lower()
            if any(n in data for n in needles):
                hits.append(rel)
        self.assertEqual(hits, [], "the owner's home path is in a public file")

    def test_no_env_files(self):
        bad = [rel for rel in tracked_files() if os.path.basename(rel).startswith(".env")]
        self.assertEqual(bad, [])

    def test_no_codeowners_file(self):
        # The README recommends CODEOWNERS for users' own memory folders; this repo has none.
        bad = [rel for rel in tracked_files() if os.path.basename(rel) == "CODEOWNERS"]
        self.assertEqual(bad, [])


class SecurityDocTest(unittest.TestCase):
    def setUp(self):
        self.text = read("SECURITY.md")

    def test_names_the_rtk_allow(self):
        self.assertIn("Bash(rtk *)", self.text)

    def test_has_memory_trust_model_heading(self):
        self.assertTrue(
            any(re.search(r"memory trust model", h, re.IGNORECASE) for h in headings(self.text)),
            "SECURITY.md needs a heading about the memory trust model")

    def test_reporting_route_has_no_email(self):
        self.assertIsNone(re.search(r"[\w.+-]+@[\w-]+\.[\w.-]+", self.text))
        self.assertIn("private vulnerability reporting", self.text.lower())


class ContributingDocTest(unittest.TestCase):
    def setUp(self):
        self.targets = [m.group(1) for m in MARKDOWN_LINK.finditer(read("CONTRIBUTING.md"))]

    def test_links_the_maintainer_tools(self):
        for want in ("scripts/vendor.py", "scripts/sync-rtk.sh", "scripts/release.sh"):
            with self.subTest(want=want):
                self.assertIn(want, [_clean(t) for t in self.targets])

    def test_links_the_evals(self):
        paths = [_clean(t) for t in self.targets]
        self.assertTrue(any(p == "evals" or p.startswith("evals/") for p in paths),
                        "CONTRIBUTING.md must link evals/")


class ReadmeTest(unittest.TestCase):
    def setUp(self):
        self.text = read("README.md")

    def test_install_commands(self):
        for cmd in ("/plugin marketplace add rbonweb/anthology",
                    "/plugin install anthology@anthology",
                    "/anthology:setup"):
            with self.subTest(cmd=cmd):
                self.assertIn(cmd, self.text)

    def test_states_the_limits(self):
        self.assertIn("repo memory is visible to everyone who can see the repo",
                      self.text.lower())
        self.assertIn("Bash(rtk *)", self.text)

    def test_no_credit_sections(self):
        for name in ("README.md", "SECURITY.md", "CONTRIBUTING.md", "DECISIONS.md"):
            with self.subTest(name=name):
                for h in headings(read(name)):
                    self.assertNotRegex(h, re.compile(r"credit|acknowledg|thanks", re.I))

    def test_codeowners_example_is_a_placeholder(self):
        self.assertIn("@your-org/reviewers", self.text)


def github_anchor(heading: str) -> str:
    """The anchor GitHub gives a heading: lower case, punctuation dropped, spaces to hyphens."""
    text = re.sub(r"[`*_]", "", heading.strip().lower())
    text = re.sub(r"[^\w\- ]", "", text)
    return text.replace(" ", "-")


class LinkTest(unittest.TestCase):
    def test_relative_links_resolve(self):
        problems = []
        for doc in PUBLIC_DOCS:
            text = read(doc)
            for m in MARKDOWN_LINK.finditer(text):
                target = m.group(1)
                if re.match(r"[a-z][a-z0-9+.-]*:", target):  # https:, mailto:
                    continue
                path, _, fragment = target.partition("#")
                rel = _clean(path) if path else doc
                dest = ROOT / rel
                if not dest.exists():
                    problems.append(f"{doc}: {target} does not exist")
                elif fragment and dest.is_file() and dest.suffix == ".md":
                    anchors = {github_anchor(h) for h in headings(dest.read_text(encoding="utf-8"))}
                    if fragment not in anchors:
                        problems.append(f"{doc}: {target} has no such heading")
        self.assertEqual(problems, [])

    def test_named_scripts_and_workflows_exist(self):
        problems = []
        for doc in ("README.md", "SECURITY.md", "CONTRIBUTING.md"):  # DECISIONS names paths in upstream trees
            for m in CODE_SPAN_PATH.finditer(read(doc)):
                rel = m.group(1)
                if not (ROOT / rel).is_file():
                    problems.append(f"{doc}: `{rel}` does not exist")
        self.assertEqual(problems, [])


class DecisionsDocTest(unittest.TestCase):
    def test_entries_are_adr_style(self):
        text = read("DECISIONS.md")
        adrs = re.findall(r"^## ADR-(\d{3}): .+$", text, re.MULTILINE)
        self.assertGreaterEqual(len(adrs), 6)
        self.assertEqual(adrs, sorted(adrs))
        for field in ("**Date:**", "**Task:**", "**Status:** Accepted", "**Context**",
                      "**Decision**", "**Consequences**"):
            with self.subTest(field=field):
                self.assertEqual(text.count(field), len(adrs))


if __name__ == "__main__":
    unittest.main()
