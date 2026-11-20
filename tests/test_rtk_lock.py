"""plugins/anthology/rtk.lock pins the released rtk binaries.

The lock is read strictly by rtk_install.load_lock: exactly the keys version, tag, repo,
sha256, and a sha256 for each of the four targets. The version must be the one in
rtk/Cargo.toml and the tag the one rtk-release.yml publishes under.
"""
from __future__ import annotations

import json
import re
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCK = ROOT / "plugins" / "anthology" / "rtk.lock"
SCRIPTS = ROOT / "plugins" / "anthology" / "scripts"
TARGETS = {
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
}


class RtkLockTest(unittest.TestCase):
    def setUp(self):
        self.lock = json.loads(LOCK.read_text(encoding="utf-8"))

    def test_exact_keys(self):
        self.assertEqual(set(self.lock), {"version", "tag", "repo", "sha256"})

    def test_version_matches_cargo_toml(self):
        cargo = (ROOT / "rtk" / "Cargo.toml").read_text(encoding="utf-8")
        version = re.search(r'^version = "([^"]+)"', cargo, re.M).group(1)
        self.assertEqual(self.lock["version"], version)

    def test_tag_and_repo(self):
        self.assertEqual(self.lock["tag"], "rtk--v" + self.lock["version"])
        self.assertEqual(self.lock["repo"], "rbonweb/anthology")

    def test_sha256_for_exactly_the_four_targets(self):
        self.assertEqual(set(self.lock["sha256"]), TARGETS)
        for target, digest in self.lock["sha256"].items():
            self.assertRegex(digest, r"^[0-9a-f]{64}$", target)

    def test_load_lock_accepts_it(self):
        sys.path.insert(0, str(SCRIPTS))
        try:
            import rtk_install
            loaded = rtk_install.load_lock(LOCK)
        finally:
            sys.path.remove(str(SCRIPTS))
        self.assertEqual(loaded["tag"], self.lock["tag"])


if __name__ == "__main__":
    unittest.main()
