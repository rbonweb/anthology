"""The 11-agent roster (Part 1 section 5): frontmatter, Startup, rules and startup budget."""
import re
import sys
import unittest

from _repo import (EXPECTED_ROSTER, KNOWLEDGE, ROOT, knowledge_path, load_frontmatter,
                   superpowers_skill_path)
from test_knowledge import EXPECTED_KNOWLEDGE

sys.path.insert(0, str(ROOT / "scripts"))
import startup_budget  # noqa: E402  (the budget calculation is shared with upstream_watch)
from startup_budget import BUDGET, sections  # noqa: E402

PLUGIN = ROOT / "plugins" / "anthology"
AGENTS = PLUGIN / "agents"
SECTIONS = ["Startup", "Role", "Do", "Don't", "Memory", "When blocked", "Report"]
MAX_BODY_LINES = 60
BROWSER_AGENTS = {"ui-designer", "ui-reviewer", "researcher", "marketer"}
FRONTMATTER_KEYS = {"name", "description", "model", "disallowedTools", "isolation", "memory",
                    "skills"}
CAVEMAN = "knowledge/caveman/SKILL.md"


def disallowed(meta):
    value = meta.get("disallowedTools", "")
    items = value.split(",") if isinstance(value, str) else value
    return {i.strip() for i in items if i.strip()}


def startup_files(name):
    return startup_budget.startup_files(ROOT, name)


def startup_bytes(name):
    return startup_budget.startup_bytes(ROOT, name)


class RosterTest(unittest.TestCase):
    def agent(self, name):
        return load_frontmatter(AGENTS / f"{name}.md")

    def test_file_set_matches_the_roster(self):
        self.assertEqual({p.stem for p in AGENTS.glob("*.md")}, set(EXPECTED_ROSTER))
        self.assertEqual(len(EXPECTED_ROSTER), 11)

    def test_frontmatter(self):
        for name, spec in EXPECTED_ROSTER.items():
            with self.subTest(agent=name):
                meta, _ = self.agent(name)
                self.assertEqual(set(meta) - FRONTMATTER_KEYS, set())
                self.assertEqual(meta["name"], name)
                self.assertEqual(meta["model"], spec["model"])
                self.assertEqual(meta["memory"], "project")
                self.assertIsInstance(meta["description"], str)
                self.assertTrue(meta["description"].startswith("Use when "))
                self.assertNotIn("\n", meta["description"].strip())

    def test_disallowed_tools(self):
        for name, spec in EXPECTED_ROSTER.items():
            with self.subTest(agent=name):
                meta, _ = self.agent(name)
                want = {"Agent"}
                if spec["edits"] == "no":
                    want |= {"Edit", "Write", "NotebookEdit"}
                self.assertEqual(disallowed(meta), want)

    def test_isolation_exactly_for_writers(self):
        for name, spec in EXPECTED_ROSTER.items():
            with self.subTest(agent=name):
                meta, _ = self.agent(name)
                self.assertEqual(meta.get("isolation"), "worktree" if spec["worktree"] else None)
                self.assertEqual(spec["worktree"], spec["edits"] == "yes")

    def test_skills_are_the_expected_preloads(self):
        for name, spec in EXPECTED_ROSTER.items():
            with self.subTest(agent=name):
                meta, _ = self.agent(name)
                want = [f"anthology-superpowers:{s}" for s in spec["preloads"]]
                self.assertEqual(meta.get("skills", []), want)
                self.assertEqual("skills" in meta, bool(want))
                for skill in spec["preloads"]:
                    self.assertTrue(superpowers_skill_path(skill).is_file(), skill)

    def test_startup_lists_caveman_then_the_expected_knowledge(self):
        for name, spec in EXPECTED_ROSTER.items():
            with self.subTest(agent=name):
                want = [CAVEMAN] + [str(knowledge_path(r).relative_to(PLUGIN))
                                    for r in spec["knowledge"]]
                self.assertEqual(startup_files(name), want)
                for rel in want:
                    self.assertTrue((PLUGIN / rel).is_file(), rel)

    def test_startup_text(self):
        for name in EXPECTED_ROSTER:
            with self.subTest(agent=name):
                _, body = self.agent(name)
                startup = sections(body)[1]["Startup"]
                self.assertIn("Before any other work, Read every file below, in order:", startup)
                self.assertIn("relative to that file's folder", startup)
                self.assertIn("design-skills:<x>", startup)
                self.assertIn("${CLAUDE_PLUGIN_ROOT}/knowledge/design-skills/skills/<x>/SKILL.md",
                              startup)
                self.assertIn("Caveman governs chat and reports only", startup)

    def test_no_loose_knowledge(self):
        used = {rel.split("/")[1] for name in EXPECTED_ROSTER for rel in startup_files(name)}
        self.assertEqual(EXPECTED_KNOWLEDGE - used, set())
        self.assertEqual(used - EXPECTED_KNOWLEDGE, set())
        used_ds = {rel.split("/")[3] for name in EXPECTED_ROSTER for rel in startup_files(name)
                   if rel.startswith("knowledge/design-skills/")}
        on_disk = {p.parent.name for p in (KNOWLEDGE / "design-skills" / "skills").glob(
            "*/SKILL.md")}
        self.assertEqual(len(on_disk), 10)
        self.assertEqual(on_disk - used_ds, set())

    def test_body_shape(self):
        for name in EXPECTED_ROSTER:
            with self.subTest(agent=name):
                _, body = self.agent(name)
                self.assertEqual(sections(body)[0], SECTIONS)
                self.assertLessEqual(len(body.rstrip("\n").splitlines()), MAX_BODY_LINES)

    def test_memory_section_names_the_agent_memory_dir(self):
        for name, spec in EXPECTED_ROSTER.items():
            with self.subTest(agent=name):
                _, body = self.agent(name)
                memory = sections(body)[1]["Memory"]
                self.assertIn(f".claude/agent-memory/anthology-{name}/", memory)
                if spec["edits"] == "yes":
                    self.assertIn("chore(memory)", memory)
                elif spec["edits"] == "docs":
                    self.assertIn("uncommitted", memory)
                else:
                    self.assertIn("heredoc", memory)
                    self.assertIn(f"mkdir -p .claude/agent-memory/anthology-{name} && cat >",
                                  memory)

    def test_memory_notes_are_one_per_file_with_the_indexed_frontmatter(self):
        # memory_sync regenerates MEMORY.md from each note's name, description, metadata.type.
        for name in EXPECTED_ROSTER:
            with self.subTest(agent=name):
                _, body = self.agent(name)
                memory = sections(body)[1]["Memory"]
                self.assertIn("one note per file", memory)
                self.assertIn("`<type>-<topic-slug>.md`", memory)
                self.assertIn("`name`, `description` and `metadata.type`", memory)
                self.assertIn("Never hand-edit MEMORY.md", memory)

    def test_no_edit_agents_may_only_edit_their_memory_dir(self):
        for name, spec in EXPECTED_ROSTER.items():
            if spec["edits"] != "no":
                continue
            with self.subTest(agent=name):
                meta, body = self.agent(name)
                self.assertIn("Do not edit files outside your memory dir",
                              sections(body)[1]["Don't"])
                self.assertNotRegex(meta["description"], r"never edits files\b(?! outside)")
                self.assertNotRegex(body, r"[Dd]o not edit files[.,]")

    def test_every_agent_carries_the_docker_rule(self):
        # Section 7.5 is global and the routing guide does not reach subagents (U7), so each
        # body must say it, whether the agent builds, runs tests or serves a page.
        for name in EXPECTED_ROSTER:
            with self.subTest(agent=name):
                _, body = self.agent(name)
                self.assertIn("Docker: run tests, builds and E2E suites in containers mounting "
                              "only the current worktree root", body)
                self.assertIn("run from that root", body)
                self.assertIn("No host installs", body)
                self.assertIn("Docker unavailable: stop and ask", body)

    def test_ui_agents_serve_pages_from_docker_with_a_published_port(self):
        for name in ("ui-designer", "ui-reviewer"):
            with self.subTest(agent=name):
                _, body = self.agent(name)
                self.assertIn("from Docker with a published port", body)

    def test_writers_commit_and_never_add_worktrees(self):
        for name, spec in EXPECTED_ROSTER.items():
            if spec["edits"] != "yes":
                continue
            with self.subTest(agent=name):
                _, body = self.agent(name)
                self.assertRegex(body, r"[Nn]ever `git worktree add`")
                self.assertIn("Commit before finishing", body)

    def test_docs_only_agents_limit_edits_to_markdown_in_the_main_clone(self):
        for name, spec in EXPECTED_ROSTER.items():
            if spec["edits"] != "docs":
                continue
            with self.subTest(agent=name):
                _, body = self.agent(name)
                self.assertIn("Markdown", body)
                self.assertIn("main clone", body)
                self.assertIn("do not commit", body.lower())

    def test_browser_agents_get_the_web_rule(self):
        for name in EXPECTED_ROSTER:
            with self.subTest(agent=name):
                _, body = self.agent(name)
                if name in BROWSER_AGENTS:
                    for needle in ("BrowserOS Neo", "mcp__browseros-neo__", "WebFetch",
                                   "own tab", "any other browser or browser automation",
                                   "Never launch Chrome", "never install a browser"):
                        self.assertIn(needle, body)
                else:
                    self.assertNotIn("BrowserOS", body)

    def test_ui_designer_has_the_which_knowledge_leads_rule(self):
        _, body = self.agent("ui-designer")
        self.assertRegex(body, r"(?i)which knowledge leads")
        self.assertRegex(body, r"design-taste-frontend leads")
        self.assertRegex(body, r"ui-taste leads")
        self.assertRegex(body, r"frontend-design is the baseline")

    def test_ui_reviewer_looks_at_the_rendered_page(self):
        _, body = self.agent("ui-reviewer")
        self.assertIn("rendered page", body)

    def test_no_forbidden_text(self):
        for name in EXPECTED_ROSTER:
            with self.subTest(agent=name):
                text = (AGENTS / f"{name}.md").read_text(encoding="utf-8")
                self.assertNotIn("agent-browser", text)
                self.assertNotIn("/Users/", text)
                self.assertNotRegex(text, r"(?i)\bcredits?\b")

    def test_startup_budget(self):
        self.assertEqual(set(BUDGET), set(EXPECTED_ROSTER))
        for name in EXPECTED_ROSTER:
            with self.subTest(agent=name):
                measured = startup_bytes(name)
                self.assertLessEqual(
                    measured, BUDGET[name],
                    f"{name}: measured {measured} bytes > budget {BUDGET[name]}. If the growth is "
                    f"intended, set BUDGET in scripts/startup_budget.py to ceil({measured} * 1.15) = "
                    f"{startup_budget.suggested_budget(measured)} and say why in the commit.")


if __name__ == "__main__":
    unittest.main()
