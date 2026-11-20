"""scripts/startup_budget.py: the shared per-agent startup-size calculation and its CLI."""
from __future__ import annotations

import io
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from _agents import make_repo, write
from _repo import EXPECTED_ROSTER, ROOT, load_frontmatter

sys.path.insert(0, str(ROOT / "scripts"))
import startup_budget as sb  # noqa: E402


class StartupBudgetTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        make_repo(self.root)

    def body_bytes(self, name):
        _, body = sb.split_agent(self.root / "plugins/anthology/agents" / f"{name}.md")
        return len(body.encode("utf-8"))

    def test_split_agent_reads_the_skills_list_without_yaml(self):
        skills, body = sb.split_agent(self.root / "plugins/anthology/agents/skilled.md")
        self.assertEqual(skills, ["anthology-superpowers:plan"])
        self.assertTrue(body.startswith("## Startup"))
        self.assertEqual(sb.split_agent(self.root / "plugins/anthology/agents/reader.md")[0], [])

    def test_startup_files_and_paths(self):
        self.assertEqual(sb.startup_files(self.root, "reader"),
                         ["knowledge/alpha/SKILL.md", "knowledge/beta/SKILL.md"])
        self.assertEqual(sb.startup_paths(self.root, "skilled"),
                         ["plugins/anthology/knowledge/beta/SKILL.md",
                          "plugins/anthology-superpowers/skills/plan/SKILL.md"])
        self.assertEqual(sb.startup_paths(self.root, "bare"), [])

    def test_startup_bytes_is_body_plus_startup_files_plus_preloads(self):
        self.assertEqual(sb.startup_bytes(self.root, "reader"), self.body_bytes("reader") + 300)
        self.assertEqual(sb.startup_bytes(self.root, "skilled"), self.body_bytes("skilled") + 600)
        self.assertEqual(sb.startup_bytes(self.root, "bare"), self.body_bytes("bare"))

    def test_agents_reading_a_target(self):
        read = sb.agents_reading
        self.assertEqual(read(self.root, "plugins/anthology/knowledge/alpha"), ["reader"])
        self.assertEqual(read(self.root, "plugins/anthology/knowledge/beta"), ["reader", "skilled"])
        self.assertEqual(read(self.root, "plugins/anthology-superpowers"), ["skilled"])
        self.assertEqual(read(self.root, "plugins/anthology/knowledge"), ["reader", "skilled"])
        self.assertEqual(read(self.root, "plugins/anthology/knowledge/alph"), [])  # not a prefix match
        self.assertEqual(read(self.root, "rtk"), [])
        self.assertEqual(read(self.root, ""), [])

    def test_agent_names_skip_names_that_are_not_plain_identifiers(self):
        write(self.root, "plugins/anthology/agents/evil@octocat.md", "x")
        write(self.root, "plugins/anthology/agents/a b.md", "x")
        self.assertEqual(sb.agent_names(self.root), ["bare", "reader", "skilled"])

    def test_cli_prints_size_against_budget_and_fails_when_over(self):
        budgets = {"reader": 10_000, "skilled": 10_000, "bare": 1}
        out = io.StringIO()
        with mock.patch.dict(sb.BUDGET, budgets, clear=True):
            code = sb.main(["--root", str(self.root)], stdout=out)
        text = out.getvalue()
        self.assertEqual(code, 1)
        self.assertIn(f"reader: {self.body_bytes('reader') + 300} bytes of 10000 budget (ok)", text)
        self.assertIn("bare: ", text)
        self.assertIn(f"OVER; if intended set BUDGET to {sb.suggested_budget(self.body_bytes('bare'))}", text)

    def test_cli_passes_and_selects_agents(self):
        out = io.StringIO()
        with mock.patch.dict(sb.BUDGET, {"reader": 10_000}, clear=True):
            self.assertEqual(sb.main(["--root", str(self.root), "reader"], stdout=out), 0)
        self.assertEqual(out.getvalue().count("bytes of"), 1)

    def test_cli_rejects_unknown_agents_and_flags_a_missing_budget(self):
        out = io.StringIO()
        self.assertEqual(sb.main(["--root", str(self.root), "nope"], stdout=out), 2)
        self.assertIn("unknown agent(s): nope", out.getvalue())
        out = io.StringIO()
        with mock.patch.dict(sb.BUDGET, {}, clear=True):
            self.assertEqual(sb.main(["--root", str(self.root), "reader"], stdout=out), 1)
        self.assertIn("no budget set", out.getvalue())

    def test_suggested_budget_rounds_up(self):
        self.assertEqual(sb.suggested_budget(100), 115)
        self.assertEqual(sb.suggested_budget(101), 117)  # 116.15 -> 117


class RealRepoTest(unittest.TestCase):
    """The stdlib parser agrees with the YAML-based one the other tests use."""

    def test_skills_match_the_yaml_frontmatter(self):
        for name in EXPECTED_ROSTER:
            with self.subTest(agent=name):
                meta, body = load_frontmatter(ROOT / "plugins/anthology/agents" / f"{name}.md")
                skills, plain_body = sb.split_agent(ROOT / "plugins/anthology/agents" / f"{name}.md")
                self.assertEqual(skills, meta.get("skills", []))
                self.assertEqual(plain_body, body)

    def test_budget_covers_the_roster_and_the_repo_is_within_it(self):
        self.assertEqual(set(sb.BUDGET), set(EXPECTED_ROSTER))
        self.assertEqual(set(sb.agent_names(ROOT)), set(EXPECTED_ROSTER))
        out = io.StringIO()
        self.assertEqual(sb.main(["--root", str(ROOT)], stdout=out), 0, out.getvalue())


if __name__ == "__main__":
    unittest.main()
