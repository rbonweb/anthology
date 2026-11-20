"""Tests for scripts/check_version_bump.py.

The pure-logic tests need nothing but Python. The integration tests build throwaway
repositories with git, so they are skipped where git is missing (the Docker test
images) and run in the CI `version-bump` job.
"""
from __future__ import annotations

import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import check_version_bump as cvb  # noqa: E402

GATE = "scripts/check_version_bump.py"
ZEROS = "0" * 40


class SemverTests(unittest.TestCase):
    def test_parse_valid(self):
        for v in ("0.1.0", "1.2.3", "10.20.30", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0+build.5",
                  "1.0.0-rc.1+build.5", "1.0.0-0.3.7", "1.0.0-x-y-z"):
            self.assertIsNotNone(cvb.parse_semver(v), v)

    def test_parse_invalid(self):
        for v in ("", "1", "1.2", "1.2.3.4", "01.2.3", "1.02.3", "1.2.3-", "1.2.3-01", "v1.2.3",
                  "1.2.3+", "1.2.3-a..b", "a.b.c", " 1.2.3", None, 3, "1.2.3-alpha_1",
                  "1.2.3\n", "1.2.3-rc.1\n", "1.2.3+b\n"):
            self.assertIsNone(cvb.parse_semver(v), repr(v))

    def test_precedence(self):
        ordered = ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta",
                   "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0",
                   "2.0.0", "10.0.0"]
        for i, a in enumerate(ordered):
            for j, b in enumerate(ordered):
                got = cvb.compare_semver(a, b)
                want = (i > j) - (i < j)
                self.assertEqual(got, want, f"{a} vs {b}")

    def test_numeric_not_lexicographic(self):
        self.assertGreater(cvb.compare_semver("0.10.0", "0.9.0"), 0)

    def test_build_metadata_ignored(self):
        self.assertEqual(cvb.compare_semver("1.0.0+a", "1.0.0+b"), 0)


class ChangedPluginsTests(unittest.TestCase):
    def test_groups_by_plugin(self):
        paths = ["plugins/a/x.md", "plugins/a/sub/y.md", "plugins/b/z", "README.md",
                 "scripts/vendor.py", "plugins/top-level-file"]
        self.assertEqual(cvb.changed_plugins(paths), ["a", "b"])

    def test_prefix_is_a_directory_boundary(self):
        self.assertEqual(cvb.changed_plugins(["plugins/anthology-guardrails/x"]),
                         ["anthology-guardrails"])

    def test_nothing(self):
        self.assertEqual(cvb.changed_plugins([]), [])
        self.assertEqual(cvb.changed_plugins(["tests/x.py", ".github/workflows/ci.yml"]), [])


class VerdictTests(unittest.TestCase):
    def test_bumped(self):
        ok, msg = cvb.verdict("p", "0.1.0", "0.1.1")
        self.assertTrue(ok)

    def test_equal_fails(self):
        ok, msg = cvb.verdict("p", "0.1.0", "0.1.0")
        self.assertFalse(ok)
        self.assertEqual(msg, "plugins/p changed without a version bump (still 0.1.0)")

    def test_lower_fails(self):
        ok, msg = cvb.verdict("p", "0.2.0", "0.1.9")
        self.assertFalse(ok)
        self.assertIn("0.2.0", msg)
        self.assertIn("0.1.9", msg)

    def test_prerelease_below_release(self):
        self.assertFalse(cvb.verdict("p", "1.0.0", "1.0.0-rc.1")[0])
        self.assertTrue(cvb.verdict("p", "1.0.0-rc.1", "1.0.0")[0])

    def test_invalid_head_fails(self):
        for bad in ("banana", "1.2", None, 7):
            ok, msg = cvb.verdict("p", "0.1.0", bad)
            self.assertFalse(ok, bad)
            self.assertIn("not valid semver", msg)

    def test_new_or_removed_plugin_passes(self):
        self.assertTrue(cvb.verdict("p", None, "0.1.0", base_exists=False)[0])
        self.assertTrue(cvb.verdict("p", "0.1.0", None, head_exists=False)[0])

    def test_invalid_base_version_passes_if_head_valid(self):
        # A broken base version must not lock the plugin out of ever fixing it.
        self.assertTrue(cvb.verdict("p", "garbage", "0.1.0")[0])


@unittest.skipUnless(shutil.which("git"), "git is not installed")
class GitIntegrationTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = Path(self._tmp.name)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@users.noreply.github.com")
        self.git("config", "commit.gpgsign", "false")

    def git(self, *args):
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        out = subprocess.run(["git", "-C", str(self.repo), *args], check=True, env=env,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        return out.stdout.strip()

    def write(self, rel, text):
        p = self.repo / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text, encoding="utf-8")

    def set_version(self, plugin, version):
        self.write(f"plugins/{plugin}/.claude-plugin/plugin.json",
                   json.dumps({"name": plugin, "version": version}) + "\n")

    def install_gate(self):
        self.write(GATE, "# gate marker\n")

    def commit(self, msg):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", msg)
        return self.git("rev-parse", "HEAD")

    def run_gate(self, base, head):
        buf, err = io.StringIO(), io.StringIO()
        with redirect_stdout(buf), redirect_stderr(err):
            code = cvb.main([base, head, "--repo", str(self.repo)])
        self.stderr = err.getvalue()
        return code, buf.getvalue()

    def base_with_plugin(self, version="0.1.0"):
        self.install_gate()
        self.set_version("p", version)
        self.write("plugins/p/skill.md", "one\n")
        return self.commit("base")

    def test_a_change_without_bump_fails(self):
        base = self.base_with_plugin()
        self.write("plugins/p/skill.md", "two\n")
        head = self.commit("edit")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 1, out)
        self.assertIn("FAIL", out)
        self.assertIn("plugins/p changed without a version bump (still 0.1.0)", out)

    def test_b_change_with_bump_passes(self):
        base = self.base_with_plugin()
        self.write("plugins/p/skill.md", "two\n")
        self.set_version("p", "0.1.1")
        head = self.commit("edit+bump")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 0, out)
        self.assertIn("ok", out)

    def test_c_no_plugin_change_passes(self):
        base = self.base_with_plugin()
        self.write("docs.md", "x\n")
        head = self.commit("docs")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 0, out)

    def test_d_base_without_script_is_noop(self):
        self.set_version("p", "0.1.0")
        base = self.commit("base without gate")
        self.install_gate()
        self.write("plugins/p/skill.md", "two\n")
        head = self.commit("add gate and edit plugin")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 0, out)
        self.assertIn("version gate: base predates the gate; skipping", out)

    def test_e_new_plugin_passes(self):
        base = self.base_with_plugin()
        self.set_version("q", "0.1.0")
        head = self.commit("new plugin")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 0, out)

    def test_removed_plugin_passes(self):
        base = self.base_with_plugin()
        shutil.rmtree(self.repo / "plugins" / "p")
        head = self.commit("remove plugin")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 0, out)

    def test_f_version_decrease_fails(self):
        base = self.base_with_plugin("0.2.0")
        self.set_version("p", "0.1.0")
        head = self.commit("downgrade")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 1, out)
        self.assertIn("FAIL", out)

    def test_invalid_head_version_fails(self):
        base = self.base_with_plugin()
        self.set_version("p", "banana")
        head = self.commit("bad version")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 1, out)

    def test_g_merge_base_semantics(self):
        self.base_with_plugin("0.1.0")
        # The base branch advances with its own bump after the branch point.
        self.git("checkout", "-q", "-b", "feature")
        self.write("plugins/p/skill.md", "feature edit\n")
        self.set_version("p", "0.1.1")
        feature_head = self.commit("feature edit+bump")
        self.git("checkout", "-q", "main")
        self.write("plugins/p/other.md", "main edit\n")
        self.set_version("p", "0.2.0")
        main_tip = self.commit("main edit+bump")
        # 0.1.1 < 0.2.0 on main, but the feature branch is judged against the merge base.
        code, out = self.run_gate(main_tip, feature_head)
        self.assertEqual(code, 0, out)
        # And a feature branch without a bump still fails whatever main did.
        self.git("checkout", "-q", "feature")
        self.set_version("p", "0.1.0")
        self.write("plugins/p/skill.md", "feature edit 2\n")
        unbumped = self.commit("feature without bump")
        code, out = self.run_gate(main_tip, unbumped)
        self.assertEqual(code, 1, out)

    def test_zero_base_is_noop(self):
        base = self.base_with_plugin()
        code, out = self.run_gate(ZEROS, base)
        self.assertEqual(code, 0, out)
        self.assertIn("new branch", out)

    def test_unresolvable_ref_exits_2(self):
        base = self.base_with_plugin()
        code, out = self.run_gate("f" * 40, base)
        self.assertEqual(code, 2, out)
        self.assertIn("version gate: cannot resolve", self.stderr)

    def test_no_merge_base_exits_2(self):
        base = self.base_with_plugin()
        self.git("checkout", "-q", "--orphan", "other")
        self.git("rm", "-rqf", "--cached", ".")
        self.write("unrelated.txt", "x\n")
        other = self.commit("unrelated history")
        code, out = self.run_gate(base, other)
        self.assertEqual(code, 2, out)
        self.assertIn("version gate: no merge base", self.stderr)

    def test_non_ascii_path_under_a_plugin_still_needs_a_bump(self):
        # git quotes non-ASCII paths ("plugins/p/caf\303\251.md") unless told not to; the
        # gate must still see them as plugin files.
        base = self.base_with_plugin()
        self.write("plugins/p/café.md", "one\n")
        head = self.commit("add a non-ascii file")
        code, out = self.run_gate(base, head)
        self.assertEqual(code, 1, out)
        self.assertIn("plugins/p changed without a version bump", out)


if __name__ == "__main__":
    unittest.main()
