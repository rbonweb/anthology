"""Static checks on the behaviour evals: case files, the BrowserOS Neo mock, the runner's grants
and the evals workflow. No git, no network, no model calls."""
from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

import yaml

from _repo import EXPECTED_ROSTER, ROOT

EVALS = ROOT / "evals"
NAME_RE = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
NON_CASE_DIRS = {"mocks", "results", "smoke"}

TOP_KEYS = {"schema_version", "name", "description", "tags", "plugins", "execution", "runs", "graders"}
EXECUTION_KEYS = {"prompt", "max_turns", "timeout_seconds", "model", "allowed_tools"}
GRADER_KEYS = {
    "tool_used": {"name", "type", "tool", "input_match", "min", "max"},
    "tool_order": {"name", "type", "before", "after"},
    "regex": {"name", "type", "target", "pattern", "match", "flags"},
    "file_exists": {"name", "type", "path", "exists"},
    "llm": {"name", "type", "criteria"},
}
REGEX_TARGETS = {"last_message", "trace", "mock_calls"}
REGEX_MATCH = {"contains", "not_contains"}
PLUGINS = ["../../plugins/anthology", "../../plugins/anthology-guardrails", "../../plugins/anthology-superpowers"]
MCP_PREFIX = "mcp__browseros-neo__"

# Startup reading per agent (caveman first) and the agents that preload skills: the roster.
EXPECTED_STARTUP = {agent: ["caveman"] + spec["knowledge"] for agent, spec in EXPECTED_ROSTER.items()}
PRELOAD_AGENTS = {agent for agent, spec in EXPECTED_ROSTER.items() if spec["preloads"]}
# `plugin eval` refuses worktree isolation ("worktree isolation is unavailable in this session"),
# so an agent with `isolation: worktree` can never run as a knowledge-read eval case. Those
# agents are checked by the knowledge-read-writers smoke (evals/smoke/smoke.py) instead.
WORKTREE_AGENTS = {agent for agent, spec in EXPECTED_ROSTER.items() if spec["worktree"] is True}
KNOWLEDGE_AGENTS = sorted(set(EXPECTED_ROSTER) - WORKTREE_AGENTS)
GLOB_ANCHOR = {"tool": "Glob", "input_match": r"\*\.md"}

REQUIRED_CASES = [
    "guard-g1", "guard-g4", "guard-g6", "guard-beats-rtk", "hook-reach-subagent",
    "browser-browseros-only", "harness-sanity", "guide-present", "guide-main-only",
    "skills-hidden", "agent-fields",
]
# These two cannot pass inside `claude plugin eval` (its sandbox has no rtk state and no plugin bin/ on
# the Bash PATH), so they are smokes in evals/smoke/smoke.py.
MOVED_TO_SMOKES = ["rtk-rewrite", "agent-memory-path"]
# Bash is visible in this case only so that a host-browser launch attempt is recorded and denied.
BASH_PROMPT_EXEMPT = {"browser-browseros-only"}
# This case holds Write (and Edit) only so that its `max: 0` checks are real: the prompt must
# not ask for a write.
WRITE_PROMPT_EXEMPT = {"agent-fields"}
FORBIDDEN_LITERALS = ["password:", "password =", "AKIA", "ghp_", "BEGIN" + " PRIVATE KEY"]
ABSOLUTE_PATHS = ["/Users/", "/home/", "~/"]
KEY_GATE = "steps.key.outputs.present == 'true'"


def slug_of(ref):
    return "ds-" + ref[3:] if ref.startswith("ds/") else ref


def path_regex_of(ref):
    if ref.startswith("ds/"):
        return r"knowledge/design-skills/skills/%s/SKILL\.md" % ref[3:]
    return r"knowledge/%s/SKILL\.md" % ref


def case_dirs():
    return sorted(p for p in EVALS.iterdir() if p.is_dir() and p.name not in NON_CASE_DIRS)


def load_case(path):
    return yaml.safe_load((path / "case.yaml").read_text(encoding="utf-8"))


def all_cases():
    return {p.name: load_case(p) for p in case_dirs()}


def grader_regexes(grader):
    """Every regex a grader carries."""
    found = []
    if "input_match" in grader:
        found.append(grader["input_match"])
    if "pattern" in grader:
        found.append(grader["pattern"])
    for side in ("before", "after"):
        if isinstance(grader.get(side), dict) and "input_match" in grader[side]:
            found.append(grader[side]["input_match"])
    return found


def grader_tools(grader):
    tools = []
    if "tool" in grader:
        tools.append(grader["tool"])
    for side in ("before", "after"):
        value = grader.get(side)
        if isinstance(value, str):
            tools.append(value)
        elif isinstance(value, dict):
            tools.append(value.get("tool"))
    return tools


def run_script_grants():
    """The `--allow-tools` grants listed in scripts/run-evals.sh, one per line."""
    text = (ROOT / "scripts" / "run-evals.sh").read_text(encoding="utf-8")
    m = re.search(r"^GRANTS='([^']*)'", text, re.M)
    assert m, "scripts/run-evals.sh has no GRANTS='...' block"
    return [line for line in m.group(1).splitlines() if line.strip()]


class CaseLayoutTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = all_cases()

    def test_there_are_cases(self):
        self.assertTrue(self.cases)

    def test_every_case_directory_has_a_case_file(self):
        for d in case_dirs():
            self.assertTrue((d / "case.yaml").is_file(), d.name)

    def test_names_and_schema(self):
        for dirname, case in self.cases.items():
            with self.subTest(case=dirname):
                self.assertRegex(dirname, NAME_RE)
                self.assertEqual(case["name"], dirname)
                self.assertEqual(case["schema_version"], "1.0")
                self.assertEqual(case["runs"], 1)
                self.assertIsInstance(case["description"], str)
                self.assertTrue(case["description"])

    def test_keys_are_the_allowed_ones(self):
        for dirname, case in self.cases.items():
            with self.subTest(case=dirname):
                self.assertTrue(set(case) <= TOP_KEYS, set(case) - TOP_KEYS)
                self.assertTrue({"schema_version", "name", "execution", "graders"} <= set(case))
                self.assertTrue(set(case["execution"]) <= EXECUTION_KEYS, set(case["execution"]) - EXECUTION_KEYS)

    def test_execution_values(self):
        for dirname, case in self.cases.items():
            ex = case["execution"]
            with self.subTest(case=dirname):
                self.assertIsInstance(ex["prompt"], str)
                self.assertTrue(ex["prompt"].strip())
                self.assertIn(ex["model"], {"opus", "sonnet", "haiku"})
                self.assertIsInstance(ex["max_turns"], int)
                self.assertIsInstance(ex["timeout_seconds"], int)
                self.assertIsInstance(ex["allowed_tools"], list)
                self.assertTrue(case["tags"])
                self.assertTrue(all(isinstance(t, str) for t in case["tags"]))

    def test_plugins_are_the_three_in_order_and_resolve(self):
        for dirname, case in self.cases.items():
            with self.subTest(case=dirname):
                self.assertEqual(case["plugins"], PLUGINS)
                for rel in case["plugins"]:
                    manifest = (EVALS / dirname / rel).resolve() / ".claude-plugin" / "plugin.json"
                    self.assertTrue(manifest.is_file(), str(manifest))

    def test_no_evals_directory_inside_a_plugin(self):
        # U9: a plugin must never carry its evals (they would ship to users).
        for plugin in (ROOT / "plugins").iterdir():
            self.assertFalse((plugin / "evals").exists(), plugin.name)

    def test_graders(self):
        for dirname, case in self.cases.items():
            graders = case["graders"]
            with self.subTest(case=dirname):
                self.assertTrue(graders)
                names = [g["name"] for g in graders]
                self.assertEqual(len(names), len(set(names)), "grader names must be unique")
                for g in graders:
                    self.assertIn(g["type"], GRADER_KEYS, g["name"])
                    self.assertTrue(set(g) <= GRADER_KEYS[g["type"]], (g["name"], set(g) - GRADER_KEYS[g["type"]]))
                    for pattern in grader_regexes(g):
                        self.assertIsInstance(pattern, str)
                        re.compile(pattern)
                    self.check_grader_shape(g)

    def check_grader_shape(self, g):
        kind = g["type"]
        if kind == "tool_used":
            self.assertIsInstance(g["tool"], str, g["name"])
            for key in ("min", "max"):
                if key in g:
                    self.assertIsInstance(g[key], int, g["name"])
            # The harness defaults `min` to 1 (`min ?? 1`): a grader with only `max: 0` reports
            # "called 0x (expected 1..0)" and can never pass (verified live on Claude Code 2.1.311).
            if g.get("max") == 0:
                self.assertEqual(g.get("min"), 0, g["name"] + ": max: 0 needs min: 0")
        elif kind == "tool_order":
            for side in ("before", "after"):
                value = g[side]
                if isinstance(value, dict):
                    self.assertTrue(set(value) <= {"tool", "input_match"} and "tool" in value, g["name"])
                else:
                    self.assertIsInstance(value, str, g["name"])
        elif kind == "regex":
            target = g["target"]
            if isinstance(target, dict):
                self.assertEqual(set(target), {"source", "path"}, g["name"])
                self.assertEqual(target["source"], "file", g["name"])
                self.assertIsInstance(target["path"], str, g["name"])
            else:
                self.assertIn(target, REGEX_TARGETS, g["name"])
            if "flags" in g:
                self.assertRegex(g["flags"], r"^[dgimsuvy]+$", g["name"])
            self.assertIn(g.get("match", "contains"), REGEX_MATCH, g["name"])
            self.assertIsInstance(g["pattern"], str, g["name"])
        elif kind == "file_exists":
            self.assertIsInstance(g["path"], str, g["name"])
            self.assertIsInstance(g["exists"], bool, g["name"])
        elif kind == "llm":
            self.assertTrue(g["criteria"].strip(), g["name"])

    def test_prompts_have_no_absolute_paths(self):
        for dirname, case in self.cases.items():
            for bad in ABSOLUTE_PATHS:
                self.assertNotIn(bad, case["execution"]["prompt"], dirname)

    def test_no_secret_looking_literals(self):
        for d in case_dirs():
            text = (d / "case.yaml").read_text(encoding="utf-8")
            for bad in FORBIDDEN_LITERALS:
                self.assertNotIn(bad, text, d.name)
            self.assertIsNone(re.search(r"\bsk-", text), d.name)

    def test_gitignore_excludes_the_results(self):
        lines = (ROOT / ".gitignore").read_text(encoding="utf-8").splitlines()
        self.assertIn("evals/results/", [line.strip() for line in lines])


class RosterCaseTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = all_cases()

    def test_a_routing_case_per_agent_and_a_knowledge_case_per_non_worktree_agent(self):
        for agent in EXPECTED_STARTUP:
            self.assertIn("route-" + agent, self.cases)
            if agent in WORKTREE_AGENTS:
                self.assertNotIn("knowledge-" + agent, self.cases, "plugin eval cannot run a worktree agent")
            else:
                self.assertIn("knowledge-" + agent, self.cases)
        self.assertEqual(len(EXPECTED_STARTUP), 11)
        self.assertEqual(sorted(n[len("knowledge-"):] for n in self.cases if n.startswith("knowledge-")),
                         KNOWLEDGE_AGENTS)
        self.assertTrue(WORKTREE_AGENTS)

    def test_routing_cases_dispatch_the_agent(self):
        for agent in EXPECTED_STARTUP:
            with self.subTest(agent=agent):
                case = self.cases["route-" + agent]
                self.assertIn("routing", case["tags"])
                self.assertIn({"name": "dispatched", "type": "tool_used", "tool": "Agent",
                               "input_match": '"anthology:%s"' % agent}, case["graders"])

    def test_knowledge_cases_read_the_startup_files_in_order(self):
        for agent in KNOWLEDGE_AGENTS:
            refs = EXPECTED_STARTUP[agent]
            with self.subTest(agent=agent):
                case = self.cases["knowledge-" + agent]
                graders = {g["name"]: g for g in case["graders"]}
                reads = [g["name"] for g in case["graders"] if g["name"].startswith("read-")]
                self.assertEqual(reads, ["read-" + slug_of(r) for r in refs])
                for ref in refs:
                    slug, pattern = slug_of(ref), path_regex_of(ref)
                    read, order = graders["read-" + slug], graders.get("order-" + slug)
                    self.assertEqual((read["type"], read["tool"], read["input_match"]), ("tool_used", "Read", pattern))
                    self.assertIsNotNone(order, "order-" + slug)
                    self.assertEqual(order["type"], "tool_order")
                    self.assertEqual(order["before"], {"tool": "Read", "input_match": pattern})
                    # the first Glob of the *.md listing, not any discovery Glob before it
                    self.assertEqual(order["after"], GLOB_ANCHOR)
                self.assertEqual(sum(1 for n in graders if n.startswith("order-")), len(refs))
                # consecutive Startup files are read in the documented order
                inorder = [g["name"] for g in case["graders"] if g["name"].startswith("inorder-")]
                self.assertEqual(inorder, ["inorder-%s-%s" % (slug_of(a), slug_of(b)) for a, b in zip(refs, refs[1:])])
                for a, b in zip(refs, refs[1:]):
                    g = graders["inorder-%s-%s" % (slug_of(a), slug_of(b))]
                    self.assertEqual(g["type"], "tool_order")
                    self.assertEqual(g["before"], {"tool": "Read", "input_match": path_regex_of(a)})
                    self.assertEqual(g["after"], {"tool": "Read", "input_match": path_regex_of(b)})
                self.assertEqual((graders["main-dispatches-first"]["before"], graders["main-dispatches-first"]["after"]),
                                 ("Agent", GLOB_ANCHOR))
                tools = case["execution"]["allowed_tools"]
                if agent in PRELOAD_AGENTS:
                    # Skill is granted, so "no Skill call" means something, and the agent must quote
                    # the first heading of each preloaded skill (U1: the preload arrived).
                    self.assertIn("Skill", tools)
                    self.assertIn("preload-not-invoked", graders)
                    self.assertEqual((graders["preload-not-invoked"]["tool"], graders["preload-not-invoked"]["min"],
                                      graders["preload-not-invoked"]["max"]), ("Skill", 0, 0))
                    quoted = graders["preload-quoted"]
                    self.assertEqual((quoted["type"], quoted["target"]), ("regex", "last_message"))
                    for skill in EXPECTED_ROSTER[agent]["preloads"]:
                        self.assertIn(skill_heading(skill), quoted["pattern"], skill)
                else:
                    self.assertNotIn("Skill", tools)
                    self.assertNotIn("preload-not-invoked", graders)

    def test_agent_files_agree_with_the_cases(self):
        # Runs once the agents exist (stream A); until then each agent is skipped.
        for agent, refs in EXPECTED_STARTUP.items():
            path = ROOT / "plugins" / "anthology" / "agents" / (agent + ".md")
            with self.subTest(agent=agent):
                if not path.is_file():
                    print("skipped: %s does not exist yet" % path.relative_to(ROOT))
                    continue
                self.assertEqual(startup_refs(path.read_text(encoding="utf-8")), refs)


def skill_heading(skill):
    """The first `# ` heading of a superpowers skill, without the marker."""
    text = (ROOT / "plugins" / "anthology-superpowers" / "skills" / skill / "SKILL.md").read_text(encoding="utf-8")
    body = text.split("\n---\n", 1)[1] if text.startswith("---\n") else text
    m = re.search(r"^# (.+)$", body, re.M)
    assert m, skill
    return m.group(1).strip()


def startup_refs(text):
    """The knowledge references of an agent file's `Startup` section, in order, as `x` / `ds/x`."""
    m = re.search(r"^(#+)[ \t]*Startup\b.*$", text, re.M)
    if not m:
        return None
    rest = text[m.end():]
    nxt = re.search(r"^#{1,%d}[ \t]+\S" % len(m.group(1)), rest, re.M)
    section = rest[:nxt.start()] if nxt else rest
    refs = []
    prefix = "design-skills/skills/"
    for ref in re.findall(r"knowledge/([A-Za-z0-9_./-]+?)/SKILL\.md", section):
        refs.append("ds/" + ref[len(prefix):] if ref.startswith(prefix) else ref)
    return refs


class StartupParserTests(unittest.TestCase):
    def test_parses_plain_and_design_skills_references(self):
        text = ("# Agent\n\n## Startup\n\n1. Read `${CLAUDE_PLUGIN_ROOT}/knowledge/caveman/SKILL.md`\n"
                "2. Read knowledge/design-skills/skills/ux-research/SKILL.md\n\n## Other\nknowledge/x/SKILL.md\n")
        self.assertEqual(startup_refs(text), ["caveman", "ds/ux-research"])

    def test_no_startup_section(self):
        self.assertIsNone(startup_refs("# Agent\n"))


class RequiredCaseTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = all_cases()

    def test_required_cases_exist(self):
        for name in REQUIRED_CASES:
            self.assertIn(name, self.cases)

    def test_moved_cases_are_smokes_not_cases(self):
        for name in MOVED_TO_SMOKES:
            self.assertNotIn(name, self.cases)
            self.assertFalse((EVALS / name).exists(), name)
            self.assertIn('"%s": (smoke_' % name,
                          (EVALS / "smoke" / "smoke.py").read_text(encoding="utf-8"))

    def test_subagent_bash_cases_check_the_agent_ran_first(self):
        # The main session also holds Bash, so the Bash call must come after the Agent dispatch.
        for name, command in (("hook-reach-subagent", "agent-browser"),):
            order = [g for g in self.cases[name]["graders"] if g["type"] == "tool_order"]
            self.assertEqual([(g["before"], g["after"]) for g in order],
                             [("Agent", {"tool": "Bash", "input_match": command})], name)

    def test_the_guard_cases_say_what_they_cover(self):
        self.assertIn("rtk-rewrite smoke", self.cases["guard-beats-rtk"]["description"])
        prompt = self.cases["guard-g6"]["execution"]["prompt"]
        self.assertIn("rule G6", prompt)  # the model must not refuse the probe write
        self.assertIn("Do not refuse", prompt)
        self.assertIn("Step 1 word", self.cases["guide-main-only"]["execution"]["prompt"])

    def test_skills_hidden_has_a_canary(self):
        patterns = [g.get("pattern") for g in self.cases["skills-hidden"]["graders"]]
        self.assertIn(r'"skills":\[', patterns)

    def test_guard_cases_name_their_guard(self):
        for name, guard in (("guard-g1", "G1"), ("guard-g4", "G4"), ("guard-g6", "G6"),
                            ("guard-beats-rtk", "G1"), ("hook-reach-subagent", "G4")):
            patterns = [g["pattern"] for g in self.cases[name]["graders"] if g["name"] == "blocked"]
            self.assertEqual(patterns, [r"anthology-guardrails \(%s\)" % guard], name)


class MockTests(unittest.TestCase):
    MOCKS = EVALS / "mocks" / "browseros-neo"

    def tools(self):
        data = json.loads((self.MOCKS / "_tools.json").read_text(encoding="utf-8"))
        return {t["name"]: t for t in data["tools"]}

    def test_tools_json_is_well_formed(self):
        data = json.loads((self.MOCKS / "_tools.json").read_text(encoding="utf-8"))
        self.assertEqual(set(data), {"tools"})
        for tool in data["tools"]:
            self.assertTrue(tool["name"] and tool["description"], tool)
            self.assertEqual(tool["inputSchema"]["type"], "object", tool["name"])

    def test_every_mocked_tool_has_a_responder_and_the_reverse(self):
        tools = self.tools()
        responders = {p.stem for p in self.MOCKS.glob("*.md")}
        self.assertEqual(set(tools), responders)

    def test_every_tool_a_grader_names_is_mocked(self):
        tools = self.tools()
        for name, case in all_cases().items():
            for g in case["graders"]:
                for tool in grader_tools(g):
                    if isinstance(tool, str) and tool.startswith(MCP_PREFIX):
                        short = tool[len(MCP_PREFIX):]
                        self.assertIn(short, tools, "%s: %s" % (name, tool))
                        self.assertTrue((self.MOCKS / (short + ".md")).is_file(), "%s: %s" % (name, tool))

    def test_the_cases_that_use_the_mock_grant_tool_search(self):
        for name in ("browser-browseros-only", "harness-sanity"):
            self.assertIn("ToolSearch", all_cases()[name]["execution"]["allowed_tools"], name)


class GrantTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.cases = all_cases()
        cls.grants = run_script_grants()

    def bash_grants(self):
        return [g[len("Bash("):-1] for g in self.grants if g.startswith("Bash(") and g.endswith(")")]

    def test_grants_are_read(self):
        self.assertTrue(self.bash_grants())
        for grant in self.grants:
            self.assertRegex(grant, r"^(Bash|Write|Edit|NotebookEdit)\(.+\)$")

    def test_every_literal_bash_grant_is_used_by_a_prompt(self):
        prompts = [c["execution"]["prompt"] for c in self.cases.values()]
        for command in self.bash_grants():
            if "*" in command:  # a wildcard grant is not a literal command a prompt can name
                continue
            self.assertTrue(any(command in p for p in prompts), "no case prompt runs: " + command)

    def test_every_bash_case_prompt_names_a_granted_command(self):
        for name, case in self.cases.items():
            if "Bash" not in case["execution"]["allowed_tools"] or name in BASH_PROMPT_EXEMPT:
                continue
            with self.subTest(case=name):
                prompt = case["execution"]["prompt"]
                self.assertTrue(any(c in prompt for c in self.bash_grants() if "*" not in c), prompt)

    def test_pattern_entries_in_allowed_tools_equal_a_grant(self):
        # A case may list `Bash(cmd)` / `Write(path)` itself, but only for a command the runner grants.
        # Today no case lists one, so this fails the moment one appears without a matching grant.
        entries = [(name, tool) for name, case in self.cases.items()
                   for tool in case["execution"]["allowed_tools"] if "(" in tool]
        self.assertEqual([(n, t) for n, t in entries if t not in self.grants], [])

    def test_every_write_style_grant_is_held_by_a_case(self):
        # A `Write(...)`, `Edit(...)` or `NotebookEdit(...)` grant only matters for a case that lists the tool.
        held = {t for c in self.cases.values() for t in c["execution"]["allowed_tools"]}
        for grant in self.grants:
            tool = grant.split("(", 1)[0]
            if tool != "Bash":
                self.assertIn(tool, held, grant)

    def test_the_grants_name_no_command_of_a_moved_case(self):
        text = "\n".join(self.grants)
        for bad in ("rtkrepo", "anthology-mem"):
            self.assertNotIn(bad, text)

    def test_the_runner_keeps_npx_offline(self):
        text = (ROOT / "scripts" / "run-evals.sh").read_text(encoding="utf-8")
        self.assertRegex(text, r"(?m)^export NPM_CONFIG_OFFLINE=true$")

    def test_plain_write_tools_have_a_narrow_operator_grant(self):
        # A gated tool needs the case entry AND an operator grant; the grant is path-limited.
        for name, case in self.cases.items():
            for tool in case["execution"]["allowed_tools"]:
                if tool in ("Write", "Edit", "NotebookEdit"):
                    self.assertTrue(any(g.startswith(tool + "(.claude/memory/") for g in self.grants), (name, tool))

    def test_the_tool_field_cases_hold_the_tools_they_check(self):
        # agent-fields asserts `max: 0` on these tools; that only means something when the main
        # session has them.
        cases = all_cases()
        for tool in ("Edit", "Write", "NotebookEdit"):
            self.assertIn(tool, cases["agent-fields"]["execution"]["allowed_tools"])

    def test_every_write_case_prompt_names_a_granted_path(self):
        prefixes = [g[len("Write("):-1].rstrip("*") for g in self.grants if g.startswith("Write(")]
        self.assertTrue(prefixes)
        for name, case in self.cases.items():
            if "Write" in case["execution"]["allowed_tools"] and name not in WRITE_PROMPT_EXEMPT:
                prompt = case["execution"]["prompt"]
                self.assertTrue(any(p in prompt for p in prefixes), name)


class WorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        text = (ROOT / ".github" / "workflows" / "evals.yml").read_text(encoding="utf-8")
        cls.text = text
        cls.wf = yaml.safe_load(text)
        cls.job = cls.wf["jobs"]["evals"]
        cls.steps = cls.job["steps"]

    def test_triggers_are_schedule_and_dispatch_only(self):
        triggers = self.wf.get("on", self.wf.get(True))  # YAML 1.1 reads a bare `on` as True
        self.assertEqual(set(triggers), {"schedule", "workflow_dispatch"})
        self.assertEqual(len(triggers["schedule"]), 1)
        self.assertEqual(set(triggers["workflow_dispatch"]["inputs"]), {"max_cost_usd"})
        self.assertEqual(triggers["workflow_dispatch"]["inputs"]["max_cost_usd"]["default"], "20")

    def test_permissions(self):
        self.assertEqual(self.wf["permissions"], {})
        self.assertEqual(self.job["permissions"], {"contents": "read"})

    def test_one_job_guarded_by_repository_with_a_timeout(self):
        self.assertEqual(list(self.wf["jobs"]), ["evals"])
        self.assertEqual(self.job["if"], "github.repository == 'rbonweb/anthology'")
        self.assertEqual(self.job["runs-on"], "ubuntu-latest")
        self.assertEqual(self.job["timeout-minutes"], 180)  # evals (up to 120 min of cases) plus the smokes
        self.assertEqual(self.wf["concurrency"], {"group": "evals", "cancel-in-progress": False})

    def test_secret_is_read_only_through_env(self):
        for step in self.steps:
            for field in ("run", "uses", "with", "if", "name"):
                self.assertNotIn("secrets.", json.dumps(step.get(field, "")), "secret outside env: %r" % step)
        key = self.steps[0]
        self.assertEqual(key["id"], "key")
        self.assertEqual(key["env"], {"ANTHROPIC_API_KEY": "${{ secrets.ANTHROPIC_API_KEY }}"})
        self.assertIn("present=true", key["run"])
        self.assertIn("present=false", key["run"])
        self.assertIn("::notice::ANTHROPIC_API_KEY is not set; the behaviour evals are skipped", key["run"])

    def test_every_other_step_is_gated_on_the_key(self):
        for step in self.steps[1:]:
            self.assertIn(KEY_GATE, step.get("if", ""), step)

    def test_installs_the_pinned_claude_code_and_runs_the_script(self):
        runs = [s.get("run", "") for s in self.steps]
        self.assertTrue(any("npm ci --prefix ci/claude-code" in r for r in runs))
        self.assertTrue(any("scripts/run-evals.sh --ci" in r for r in runs))
        run_step = next(s for s in self.steps if "scripts/run-evals.sh --ci" in s.get("run", ""))
        self.assertEqual(run_step["env"]["CLAUDE_BIN"], "ci/claude-code/node_modules/.bin/claude")
        self.assertEqual(run_step["env"]["DISABLE_AUTOUPDATER"], "1")
        # a regressed guard must not let a granted npx fetch packages
        self.assertEqual(run_step["env"]["NPM_CONFIG_OFFLINE"], "true")
        # `plugin eval` needs the credential in its children (scrubbing fails every case "Not logged in")
        # and hides it from their tools itself; the smokes build their own environment.
        self.assertEqual(run_step["env"]["CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"], "0")
        self.assertIn("inputs.max_cost_usd", run_step["env"]["MAX_COST_USD"])
        self.assertIn('--max-cost-usd "$MAX_COST_USD"', run_step["run"])

    def test_results_are_uploaded_even_after_a_failure(self):
        upload = next(s for s in self.steps if s.get("uses", "").startswith("actions/upload-artifact@"))
        self.assertEqual(upload["if"], "always() && " + KEY_GATE)
        self.assertEqual(upload["with"]["path"], "evals/results/")
        self.assertEqual(upload["with"]["retention-days"], 30)
        self.assertEqual(upload["with"]["if-no-files-found"], "ignore")


if __name__ == "__main__":
    unittest.main()
