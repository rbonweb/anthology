"""Tests for scripts/memory_diff.py (the memory-diff listing of PLAN section 13).

Stdlib only. The parsing and escaping tests need nothing but Python; the integration tests
build throwaway repositories with git, so they are skipped where git is missing (the Docker
test images) and run in the CI `memory-git` job (this file matches `test_memory_*.py`).
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts" / "memory_diff.py"
sys.path.insert(0, str(ROOT / "scripts"))
import memory_diff as md  # noqa: E402

HOSTILE = "evil\n|`::warning::x.md"


class ParseTests(unittest.TestCase):
    def test_splits_on_nul_not_newline(self):
        raw = b"A\0.claude/memory/a\nb.md\0R100\0.claude/memory/old.md\0.claude/memory/new.md\0"
        self.assertEqual(md.parse_name_status(raw), [
            ("A", [".claude/memory/a\nb.md"]),
            ("R", [".claude/memory/old.md", ".claude/memory/new.md"]),
        ])

    def test_empty(self):
        self.assertEqual(md.parse_name_status(b""), [])

    def test_all_statuses(self):
        raw = b"M\0m\0D\0d\0T\0t\0C75\0src\0dst\0"
        self.assertEqual([e[0] for e in md.parse_name_status(raw)], ["M", "D", "T", "C"])


class EscapeTests(unittest.TestCase):
    def test_control_backtick_and_pipe(self):
        self.assertEqual(md.escape_name("a\nb\rc\td\x00e\x1b"), "a?b?c?d?e?")
        self.assertEqual(md.escape_name("a`b|c"), "a'b\\|c")

    def test_unicode_line_separators(self):
        self.assertEqual(md.escape_name("a\u2028b\u0085c"), "a?b?c")

    def test_capped_at_200_characters(self):
        self.assertEqual(len(md.escape_name("x" * 500)), 200)

    def test_render_empty(self):
        self.assertIn("No memory changes.", md.render([]))


@unittest.skipUnless(shutil.which("git"), "git is not installed")
class GitIntegrationTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = Path(self._tmp.name) / "repo"
        self.repo.mkdir()
        self.summary = Path(self._tmp.name) / "summary.md"
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@users.noreply.github.com")
        self.git("config", "commit.gpgsign", "false")

    def env(self, summary=False):
        env = {k: v for k, v in os.environ.items()
               if not k.startswith("GIT_") and k != "GITHUB_STEP_SUMMARY"}
        if summary:
            env["GITHUB_STEP_SUMMARY"] = str(self.summary)
        return env

    def git(self, *args):
        out = subprocess.run(["git", "-C", str(self.repo), *args], check=True, env=self.env(),
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        return out.stdout.strip()

    def write(self, rel, text="note\n"):
        p = self.repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")

    def commit(self, msg="c"):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", msg)
        return self.git("rev-parse", "HEAD")

    def run_diff(self, base, head, summary=False):
        self.summary.unlink() if self.summary.exists() else None
        proc = subprocess.run([sys.executable, str(SCRIPT), base, head], cwd=str(self.repo),
                              env=self.env(summary), stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True)
        text = self.summary.read_text(encoding="utf-8") if self.summary.exists() else ""
        return proc.returncode, proc.stdout, proc.stderr, text

    def test_add_modify_delete_rename_under_both_roots(self):
        long_text = "a long enough body so rename detection sees identical content\n" * 5
        self.write("README.md", "hi\n")
        self.write(".claude/memory/mod.md", "one\n")
        self.write(".claude/memory/del.md", "gone\n")
        self.write(".claude/agent-memory/anthology-scout/old.md", long_text)
        base = self.commit("base")
        self.write(".claude/memory/new.md")
        self.write(".claude/memory/mod.md", "two\n")
        (self.repo / ".claude/memory/del.md").unlink()
        self.git("mv", ".claude/agent-memory/anthology-scout/old.md",
                 ".claude/agent-memory/anthology-scout/renamed.md")
        self.write(".claude/agent-memory/anthology-implementer/added.md")
        head = self.commit("head")
        code, out, _, text = self.run_diff(base, head, summary=True)
        self.assertEqual(code, 0)
        self.assertEqual(out.strip(), "memory-diff: 5 changed memory file(s)")
        self.assertIn("### Memory changes in this PR", text)
        self.assertIn("| A | `.claude/memory/new.md` |", text)
        self.assertIn("| M | `.claude/memory/mod.md` |", text)
        self.assertIn("| D | `.claude/memory/del.md` |", text)
        self.assertIn("| A | `.claude/agent-memory/anthology-implementer/added.md` |", text)
        self.assertIn("| R | `.claude/agent-memory/anthology-scout/old.md` → "
                      "`.claude/agent-memory/anthology-scout/renamed.md` |", text)

    def test_files_outside_the_roots_and_nested_roots_are_ignored(self):
        self.write("README.md", "hi\n")
        base = self.commit("base")
        self.write("README.md", "changed\n")
        self.write("docs/note.md")
        self.write("sub/.claude/memory/x.md")
        self.write("sub/.claude/agent-memory/a/y.md")
        self.write(".claude/memory-other/z.md")
        head = self.commit("head")
        code, out, _, text = self.run_diff(base, head, summary=True)
        self.assertEqual(code, 0)
        self.assertEqual(out.strip(), "memory-diff: 0 changed memory file(s)")
        self.assertIn("No memory changes.", text)
        self.assertNotIn("x.md", text)

    def test_merge_base_form_ignores_changes_only_on_base(self):
        self.write("README.md", "hi\n")
        fork = self.commit("fork point")
        self.git("checkout", "-q", "-b", "topic")
        self.write(".claude/memory/topic.md")
        topic = self.commit("topic")
        self.git("checkout", "-q", "main")
        self.write(".claude/memory/only-on-base.md")
        base = self.commit("base moves on")
        self.assertNotEqual(fork, base)
        code, _, _, text = self.run_diff(base, topic, summary=True)
        self.assertEqual(code, 0)
        self.assertIn("`.claude/memory/topic.md`", text)
        self.assertNotIn("only-on-base.md", text)

    def test_hostile_file_name_is_escaped_and_never_on_stdout(self):
        self.write("README.md", "hi\n")
        base = self.commit("base")
        self.write(f".claude/memory/{HOSTILE}")
        head = self.commit("head")
        code, out, err, text = self.run_diff(base, head, summary=True)
        self.assertEqual(code, 0)
        self.assertEqual(out.strip(), "memory-diff: 1 changed memory file(s)")
        self.assertNotIn("::warning::", out + err)
        self.assertNotIn("evil", out + err)
        self.assertIn("| A | `.claude/memory/evil?\\|'::warning::x.md` |", text)
        self.assertEqual(len([l for l in text.splitlines() if l.startswith("| A")]), 1)
        self.assertFalse(any(l.startswith("::") for l in text.splitlines()))

    def test_without_a_summary_file_the_section_goes_to_stdout_escaped(self):
        self.write("README.md", "hi\n")
        base = self.commit("base")
        self.write(f".claude/memory/{HOSTILE}")
        head = self.commit("head")
        code, out, _, _ = self.run_diff(base, head)
        self.assertEqual(code, 0)
        self.assertFalse(any(l.startswith("::") for l in out.splitlines()))
        self.assertIn("evil?\\|'::warning::x.md", out)
        self.assertTrue(out.rstrip().endswith("memory-diff: 1 changed memory file(s)"))

    def test_long_names_are_capped(self):
        self.write("README.md", "hi\n")
        base = self.commit("base")
        self.write(".claude/memory/" + "n" * 240 + ".md")
        head = self.commit("head")
        _, _, _, text = self.run_diff(base, head, summary=True)
        line = next(l for l in text.splitlines() if l.startswith("| A"))
        self.assertLessEqual(len(line.split("`")[1]), 200)

    def test_no_changes(self):
        self.write("README.md", "hi\n")
        base = self.commit("base")
        code, out, _, text = self.run_diff(base, base, summary=True)
        self.assertEqual(code, 0)
        self.assertIn("No memory changes.", text)
        self.assertEqual(out.strip(), "memory-diff: 0 changed memory file(s)")

    def test_bad_ref_exits_1(self):
        self.write("README.md", "hi\n")
        base = self.commit("base")
        code, out, err, _ = self.run_diff(base, "no-such-ref", summary=True)
        self.assertEqual(code, 1)
        self.assertIn("memory-diff:", err)

    def test_usage_error_exits_2(self):
        proc = subprocess.run([sys.executable, str(SCRIPT), "only-one"], stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True)
        self.assertEqual(proc.returncode, 2)
        proc = subprocess.run([sys.executable, str(SCRIPT), "--output=x", "HEAD"],
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.assertEqual(proc.returncode, 2)


if __name__ == "__main__":
    unittest.main()
