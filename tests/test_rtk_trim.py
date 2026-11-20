"""Unit tests for the primitives of scripts/rtk_trim.py (the engine of scripts/rtk-trim.sh).

Every primitive applies once, is a no-op once applied, and fails loudly (TrimError, naming
the file and the anchor) when its anchor is missing or a signature changed."""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from _repo import ROOT

sys.path.insert(0, str(ROOT / "scripts"))
import rtk_trim  # noqa: E402
from rtk_trim import Trim, TrimError  # noqa: E402

RUST = """use std::io;

/// Docs of keep.
fn keep() {
    one();
}

/// Docs of gone.
#[inline]
fn gone(x: u8) -> u8 {
    if x > 0 {
        x
    } else {
        0
    }
}

mod tests {
    #[test]
    fn test_gone() {
        assert_eq!(gone(1), 1);
    }

    #[test]
    fn test_keep() {
        keep();
    }
}
"""


class TrimTestCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.t = Trim(self.root)

    def tearDown(self):
        self._tmp.cleanup()

    def write(self, rel: str, text: str) -> Path:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def read(self, rel: str) -> str:
        return (self.root / rel).read_text(encoding="utf-8")

    def fresh(self) -> Trim:
        """A second run of the trim over the same tree."""
        return Trim(self.root)


class RmTest(TrimTestCase):
    def test_removes_files_and_directories_once(self):
        self.write("a.txt", "a")
        self.write("d/sub/b.txt", "b")
        self.t.rm("a.txt", "d", "never-there")
        self.assertFalse((self.root / "a.txt").exists())
        self.assertFalse((self.root / "d").exists())
        self.assertEqual(self.t.changed, ["rm a.txt", "rm d"])
        again = self.fresh()
        again.rm("a.txt", "d", "never-there")
        self.assertEqual(again.changed, [])


class SubTest(TrimTestCase):
    def test_applies_once_then_is_a_no_op(self):
        self.write("f.rs", "a\nold line\nb\n")
        self.t.sub("f.rs", "old line\n", "new line\n")
        self.assertEqual(self.read("f.rs"), "a\nnew line\nb\n")
        self.assertEqual(self.t.changed, ["edit f.rs"])
        again = self.fresh()
        again.sub("f.rs", "old line\n", "new line\n")
        self.assertEqual(again.changed, [])
        self.assertEqual(self.read("f.rs"), "a\nnew line\nb\n")

    def test_new_that_extends_old_is_applied_once(self):
        self.write("Cargo.toml", "[lints.rust]\nwarnings = \"deny\"\n")
        old, new = "[lints.rust]\nwarnings = \"deny\"\n", "[lints.rust]\nwarnings = \"deny\"\nx = 1\n"
        self.t.sub("Cargo.toml", old, new)
        self.fresh().sub("Cargo.toml", old, new)
        self.assertEqual(self.read("Cargo.toml"), new)

    def test_missing_anchor_raises_with_file_and_anchor(self):
        self.write("f.rs", "something else\n")
        with self.assertRaises(TrimError) as cm:
            self.t.sub("f.rs", "old line\nmore\n", "new line\n")
        self.assertIn("f.rs", str(cm.exception))
        self.assertIn("anchor missing", str(cm.exception))
        self.assertIn("old line", str(cm.exception))
        self.assertEqual(self.read("f.rs"), "something else\n")

    def test_changed_anchor_is_not_taken_for_applied(self):
        # The anchor block changed upstream (a new line in it): the replacement is not
        # present either, so the trim stops instead of skipping.
        self.write("f.rs", "enum E {\n    A,\n    New,\n    B,\n}\n")
        with self.assertRaises(TrimError):
            self.t.sub("f.rs", "enum E {\n    A,\n    B,\n}\n", "enum E {\n    A,\n}\n// trimmed\n")

    def test_ambiguous_anchor_raises(self):
        self.write("f.rs", "x\nx\n")
        with self.assertRaises(TrimError) as cm:
            self.t.sub("f.rs", "x\n", "y\n")
        self.assertIn("2 times", str(cm.exception))

    def test_new_inside_old_is_rejected_without_done(self):
        self.write("f.rs", "a\nb\n")
        with self.assertRaises(TrimError) as cm:
            self.t.sub("f.rs", "a\nb\n", "a\n")
        self.assertIn("bad anchor design", str(cm.exception))

    def test_done_marker_decides_applied_for_chained_edits(self):
        self.write("f.rs", "if x {\n    old();\n} else {\n")
        self.t.sub("f.rs", "if x {\n    old();\n", "if x {\n    mid();\n", done="mid();")
        # A later edit rewrites the replacement; the done marker still says "applied".
        self.t.sub("f.rs", "    mid();\n} else {\n", "    final_call();\n} else {\n")
        again = self.fresh()
        with self.assertRaises(TrimError):
            again.sub("f.rs", "if x {\n    old();\n", "if x {\n    mid();\n", done="mid();")
        again.sub("f.rs", "if x {\n    old();\n", "if x {\n    mid();\n", done="final_call();")
        self.assertEqual(again.changed, [])

    def test_missing_file_raises(self):
        with self.assertRaises(TrimError) as cm:
            self.t.sub("nope.rs", "a", "b")
        self.assertIn("nope.rs", str(cm.exception))
        self.assertIn("file missing", str(cm.exception))


class CutTest(TrimTestCase):
    def test_applies_once_then_is_a_no_op(self):
        self.write("f.rs", "a\npub mod telemetry;\nb\n")
        self.t.cut("f.rs", "pub mod telemetry;\n", "mod telemetry")
        self.assertEqual(self.read("f.rs"), "a\nb\n")
        again = self.fresh()
        again.cut("f.rs", "pub mod telemetry;\n", "mod telemetry")
        self.assertEqual(again.changed, [])

    def test_missing_anchor_with_the_token_still_present_raises(self):
        self.write("f.rs", "a\npub(crate) mod telemetry;\nb\n")
        with self.assertRaises(TrimError) as cm:
            self.t.cut("f.rs", "pub mod telemetry;\n", "mod telemetry")
        self.assertIn("f.rs", str(cm.exception))
        self.assertIn("still present", str(cm.exception))
        self.assertIn("pub mod telemetry;", str(cm.exception))

    def test_ambiguous_anchor_raises(self):
        self.write("f.rs", "x\nx\n")
        with self.assertRaises(TrimError):
            self.t.cut("f.rs", "x\n", "x")


class DropTest(TrimTestCase):
    def test_drops_item_with_attributes_and_docs_once(self):
        self.write("f.rs", RUST)
        self.t.drop("f.rs", "fn gone(x: u8) -> u8 {")
        text = self.read("f.rs")
        self.assertNotIn("fn gone", text)
        self.assertNotIn("Docs of gone", text)
        self.assertNotIn("#[inline]", text)
        self.assertIn("/// Docs of keep.\nfn keep() {\n    one();\n}\n\nmod tests {", text)
        again = self.fresh()
        again.drop("f.rs", "fn gone(x: u8) -> u8 {")
        self.assertEqual(again.changed, [])

    def test_drops_an_indented_test_with_its_attribute(self):
        self.write("f.rs", RUST)
        self.t.drop("f.rs", "    fn test_gone() {")
        text = self.read("f.rs")
        self.assertNotIn("test_gone", text)
        self.assertEqual(text.count("#[test]"), 1)
        self.assertIn("    #[test]\n    fn test_keep() {\n        keep();\n    }\n}\n", text)

    def test_body_changes_are_tolerated(self):
        self.write("f.rs", RUST.replace("        x\n    } else {", "        x + 0\n    } else {"))
        self.t.drop("f.rs", "fn gone(x: u8) -> u8 {")
        self.assertNotIn("fn gone", self.read("f.rs"))

    def test_changed_signature_raises(self):
        self.write("f.rs", RUST.replace("fn gone(x: u8) -> u8 {", "fn gone(x: u16) -> u16 {"))
        with self.assertRaises(TrimError) as cm:
            self.t.drop("f.rs", "fn gone(x: u8) -> u8 {")
        self.assertIn("f.rs", str(cm.exception))
        self.assertIn("signature changed?", str(cm.exception))
        self.assertIn("fn gone", str(cm.exception))

    def test_duplicate_signature_raises(self):
        self.write("f.rs", "fn a() {\n}\nfn a() {\n}\n")
        with self.assertRaises(TrimError):
            self.t.drop("f.rs", "fn a() {")

    def test_item_without_closing_brace_raises(self):
        self.write("f.rs", "    fn a() {\n        x();\nfn b() {}\n")
        with self.assertRaises(TrimError):
            self.t.drop("f.rs", "    fn a() {")


class LockGuardTest(TrimTestCase):
    LOCK = ('version = 4\n\n[[package]]\nname = "a"\nversion = "1.0.0"\n\n'
            '[[package]]\nname = "b"\nversion = "2.0.0"\ndependencies = [\n "a",\n]\n')

    def test_packages(self):
        path = self.write("Cargo.lock", self.LOCK)
        self.assertEqual(rtk_trim.lock_packages(path), {("a", "1.0.0"), ("b", "2.0.0")})

    def test_pruning_passes_and_reports_removals(self):
        before = self.write("before.lock", self.LOCK)
        after = self.write("after.lock", 'version = 4\n\n[[package]]\nname = "a"\nversion = "1.0.0"\n')
        self.assertEqual(rtk_trim.lock_guard(before, after), [("b", "2.0.0")])

    def test_changed_or_added_version_raises(self):
        before = self.write("before.lock", self.LOCK)
        after = self.write("after.lock", self.LOCK.replace('"2.0.0"', '"2.0.1"'))
        with self.assertRaises(TrimError) as cm:
            rtk_trim.lock_guard(before, after)
        self.assertIn("b 2.0.1", str(cm.exception))


class TrimmedConflictsTest(unittest.TestCase):
    def test_only_deleted_by_us_paths_inside_removed_paths(self):
        status = "\n".join([
            "DU rtk/engine/src/lib.rs",          # upstream edited a deleted crate
            "DU rtk/src/core/telemetry.rs",      # upstream edited a deleted file
            "UU rtk/src/lib.rs",                 # a real content conflict: never resolved here
            "DU rtk/src/core/telemetry_x.rs",    # not a removed path (prefix only)
            "DU other/engine/x.rs",              # outside the subtree
            'DU "rtk/engine/odd name.rs"',       # quoted: left for a human
            " M rtk/Cargo.toml",
        ])
        self.assertEqual(
            rtk_trim.trimmed_conflicts(status, ["engine", "src/core/telemetry.rs"]),
            ["rtk/engine/src/lib.rs", "rtk/src/core/telemetry.rs"])

    def test_removed_paths_come_from_the_operations(self):
        removed = rtk_trim.removed_paths()
        for path in ("engine", "plugin", "proxy", "src/core/telemetry.rs",
                     "tests/fork_8_upstream_headers.rs", "src/hooks/init/codex.rs"):
            self.assertIn(path, removed)
        self.assertNotIn("src/hooks/init/claude.rs", removed)


class OperationsTest(TrimTestCase):
    def test_an_unexpected_tree_stops_on_the_first_missing_file(self):
        with self.assertRaises(TrimError) as cm:
            rtk_trim.operations(self.t)
        self.assertIn("Cargo.toml: file missing", str(cm.exception))

    def test_cli_exits_1_with_a_named_message(self):
        result = subprocess.run(
            [sys.executable, str(ROOT / "scripts" / "rtk_trim.py"), "--root", str(self.root)],
            capture_output=True, text=True)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(result.stderr.startswith("rtk-trim: Cargo.toml: file missing"), result.stderr)

    def test_source_is_python39_compatible(self):
        src = (ROOT / "scripts" / "rtk_trim.py").read_text(encoding="utf-8")
        self.assertIn("from __future__ import annotations", src)
        self.assertNotRegex(src, r"(?m)^\s*match\s+\S.*:\s*$")
        self.assertTrue(os.access(ROOT / "scripts" / "rtk_trim.py", os.R_OK))


if __name__ == "__main__":
    unittest.main()
