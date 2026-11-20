"""Tests for the rtk subtree: its sources.json pins, its license, the fork's named
tests and scripts/sync-rtk.sh."""
from __future__ import annotations

import json
import os
import re
import unittest

from _repo import ROOT

FORK_COMMIT = "9b790c414de16f5fb688126b3945c73aa0e0218e"
UPSTREAM_COMMIT = "d0f0399a5ff8cd1d81325a033f0b2d8864d665f6"
FORK_TESTS = [
    "fork_1_auto_allow_hook.rs",
    "fork_2_read_minimal_default.rs",
    "fork_3_node_filter.rs",
    "fork_4_python_filter.rs",
    "fork_5_cd_chain_rewrite.rs",
    "fork_6_biome_lint.rs",
    "fork_7_opencode_run.rs",
    "fork_8_upstream_headers.rs",
    "fork_9_awareness_exact.rs",
]
SYNC = ROOT / "scripts" / "sync-rtk.sh"
RTK_TEST = ROOT / "scripts" / "rtk-test.sh"


def rtk_entry() -> dict:
    sources = json.loads((ROOT / "vendor" / "sources.json").read_text(encoding="utf-8"))["sources"]
    entries = [s for s in sources if s.get("name") == "rtk"]
    assert len(entries) == 1, entries
    return entries[0]


class RtkSourceEntryTest(unittest.TestCase):
    def test_entry_is_a_subtree_with_its_target_and_license(self):
        entry = rtk_entry()
        self.assertEqual(entry["managed_by"], "subtree")
        self.assertEqual(entry["target"], "rtk")
        self.assertEqual(entry["license"], "Apache-2.0")
        self.assertNotIn("repo", entry)

    def test_fork_pin(self):
        self.assertEqual(rtk_entry()["fork"], {
            "repo": "https://github.com/rbonweb/rtk",
            "ref": "develop",
            "commit": FORK_COMMIT,
        })

    def test_upstream_pin(self):
        self.assertEqual(rtk_entry()["upstream"], {
            "repo": "https://github.com/rtk-ai/rtk",
            "ref": "v1.2.1",
            "commit": UPSTREAM_COMMIT,
        })


class RtkTreeTest(unittest.TestCase):
    def test_license_is_apache_2(self):
        text = (ROOT / "rtk" / "LICENSE").read_text(encoding="utf-8")
        self.assertIn("Apache License", text)
        self.assertIn("Version 2.0, January 2004", text)

    def test_cargo_manifest_exists(self):
        self.assertTrue((ROOT / "rtk" / "Cargo.toml").is_file())

    def test_every_fork_change_has_its_named_test(self):
        tests = ROOT / "rtk" / "tests"
        self.assertEqual(sorted(p.name for p in tests.glob("fork_*.rs")), FORK_TESTS)
        for name in FORK_TESTS:
            self.assertIn("#[test]", (tests / name).read_text(encoding="utf-8"), name)


class SyncScriptTest(unittest.TestCase):
    def setUp(self):
        self.text = SYNC.read_text(encoding="utf-8")

    def test_is_executable_with_strict_mode(self):
        self.assertTrue(os.access(SYNC, os.X_OK))
        self.assertTrue(self.text.startswith("#!"))
        self.assertRegex(self.text, r"(?m)^set -eu$")

    def test_never_pushes_or_merges(self):
        code = "\n".join(line for line in self.text.splitlines()
                         if not line.lstrip().startswith("#"))
        self.assertNotRegex(code, r"\bgit\s+push\b")
        self.assertNotRegex(code, r"\bgit\s+merge\b")
        self.assertNotRegex(code, r"\bgit\s+(checkout|switch)\s+(build|main)\b")

    def test_pulls_upstream_into_the_subtree_on_a_review_branch(self):
        self.assertIn("UPSTREAM=https://github.com/rtk-ai/rtk", self.text)
        self.assertIn('git subtree pull --prefix="$PREFIX" "$UPSTREAM" "$tag"', self.text)
        self.assertIn('branch="rtk-sync/$tag"', self.text)
        self.assertIn("git status --porcelain", self.text)
        self.assertIn("scripts/rtk-trim.sh", self.text)
        self.assertIn('"$root/scripts/rtk-test.sh"', self.text)

    def test_validates_the_tag(self):
        match = re.search(r"(?m)^tag_pattern='([^']+)'$", self.text)
        self.assertIsNotNone(match)
        self.assertIn("[[ $tag =~ $tag_pattern ]]", self.text)
        pattern = re.compile(match.group(1))
        self.assertTrue(pattern.fullmatch("v1.2.1"))
        for bad in ("1.2.1", "v1.2", "v1.2.1; rm -rf /", "main", "v1.2.1-rc1"):
            self.assertIsNone(pattern.fullmatch(bad), bad)


class RtkTestScriptTest(unittest.TestCase):
    """$RTK_TEST is scripts/rtk-test.sh, used locally, by CI and by sync-rtk.sh."""

    def setUp(self):
        self.text = RTK_TEST.read_text(encoding="utf-8")

    def test_is_executable_with_strict_mode(self):
        self.assertTrue(os.access(RTK_TEST, os.X_OK))
        self.assertTrue(self.text.startswith("#!"))
        self.assertRegex(self.text, r"(?m)^set -eu$")

    def test_reads_the_image_from_the_rust_dockerfile(self):
        self.assertIn('awk \'$1 == "FROM" {print $2}\' "$root/ci/images/rust.Dockerfile"', self.text)
        self.assertNotRegex(self.text, r"rust:[0-9.]+-slim")

    def test_runs_locked_cargo_test_as_a_non_root_user(self):
        self.assertIn('cargo test --locked "$@"', self.text)
        self.assertIn("runuser -u", self.text)
        self.assertIn("useradd", self.text)
        self.assertIn("git curl jq procps", self.text)
        self.assertNotIn("--skip", self.text)

    def test_ci_and_sync_use_it(self):
        ci = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
        self.assertRegex(ci, r"(?m)^  rtk-test:$")
        self.assertIn("run: scripts/rtk-test.sh", ci)
        self.assertIn('"$root/scripts/rtk-test.sh"', SYNC.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
