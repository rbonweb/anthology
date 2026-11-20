"""Tests for evals/smoke/smoke.py: no model, no network, Docker-safe.

The smokes themselves need a real Claude Code and credentials; these tests cover the pieces that do
not: the name list, the isolated environment, SKIP without credentials, the helpers and the summary.
"""
from __future__ import annotations

import contextlib
import io
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "evals" / "smoke"))
import smoke  # noqa: E402

NAMES = ["cli-syntax", "eval-ceiling", "context-cap", "missing-interpreter", "python-missing",
         "worktree-base", "model-check", "knowledge-read-writers", "auto-memory-dir",
         "writer-memory-commit"]
MODEL_SMOKES = ["eval-ceiling", "context-cap", "missing-interpreter", "worktree-base", "model-check",
                "knowledge-read-writers", "auto-memory-dir", "writer-memory-commit"]


def no_auth_environ():
    env = {k: v for k, v in os.environ.items() if k not in smoke.AUTH_VARS}
    return mock.patch.dict(os.environ, env, clear=True)


def run_main(argv):
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = smoke.main(argv)
    return code, out.getvalue()


class ListAndEnvTests(unittest.TestCase):
    def test_list_prints_exactly_the_ten_names(self):
        code, out = run_main(["--list"])
        self.assertEqual(code, 0)
        self.assertEqual(out.split(), NAMES)

    def test_unknown_name_is_a_usage_error(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as cm:
            smoke.main(["no-such-smoke"])
        self.assertEqual(cm.exception.code, 2)

    def test_env_passes_only_the_allowed_variables(self):
        fake = {"SECRET_X": "hunter2", "CLAUDE_CODE_SUBAGENT_MODEL": "opus", "ANTHROPIC_API_KEY": "sk-fake-key",
                "ANTHROPIC_AUTH_TOKEN": "", "ANTHROPIC_BASE_URL": "", "PATH": "/usr/bin:/bin"}
        with tempfile.TemporaryDirectory() as d, mock.patch.dict(os.environ, fake, clear=True):
            env = smoke.build_env(Path(d))
            self.assertEqual(env["ANTHROPIC_API_KEY"], "sk-fake-key")
            self.assertNotIn("SECRET_X", env)
            self.assertNotIn("CLAUDE_CODE_SUBAGENT_MODEL", env)
            self.assertNotIn("ANTHROPIC_AUTH_TOKEN", env)  # empty means unset
            self.assertEqual(Path(env["HOME"]), Path(d) / "home")
            self.assertEqual(Path(env["CLAUDE_CONFIG_DIR"]), Path(d) / "cfg")
            self.assertEqual(env["DISABLE_AUTOUPDATER"], "1")
            self.assertEqual(env["CLAUDE_CODE_SUBPROCESS_ENV_SCRUB"], "1")  # tools and hooks never see the key
            self.assertEqual(env["NPM_CONFIG_OFFLINE"], "true")
            self.assertEqual(env["PATH"], "/usr/bin:/bin")
            allowed = {"HOME", "CLAUDE_CONFIG_DIR", "PATH", "TMPDIR", "LANG", "TERM", "DISABLE_AUTOUPDATER",
                       "ANTHROPIC_API_KEY", "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "NPM_CONFIG_OFFLINE"}
            self.assertEqual(set(env), allowed)
            self.assertTrue((Path(d) / "home").is_dir() and (Path(d) / "cfg").is_dir())

    def test_redact_hides_auth_values(self):
        with mock.patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-secret-value"}):
            self.assertEqual(smoke.redact("key=sk-secret-value!"), "key=***!")


class HelperTests(unittest.TestCase):
    def test_cap_padding_is_exactly_n_chars_ending_in_the_marker(self):
        for n in (10000, 10001, 9000, 100, 20):
            pad = smoke.cap_padding(n)
            self.assertEqual(len(pad), n)
            self.assertTrue(pad.endswith(f" END-{n}"), n)

    def test_the_hook_script_prints_the_same_padding(self):
        with tempfile.TemporaryDirectory() as d:
            script = Path(d) / "ss.py"
            script.write_text(smoke.cap_hook_script(), encoding="utf-8")
            out = subprocess.run([sys.executable, str(script)], env={"CAP_N": "10001"}, check=True, timeout=60,
                                 stdout=subprocess.PIPE, text=True).stdout
        payload = json.loads(out)["hookSpecificOutput"]
        self.assertEqual(payload["hookEventName"], "SessionStart")
        self.assertEqual(payload["additionalContext"], smoke.cap_padding(10001))

    def test_has_flag_is_not_a_prefix_match(self):
        self.assertTrue(smoke.has_flag("  -s, --scope <scope>  Scope", "-s"))
        self.assertTrue(smoke.has_flag("--keep-data  Keep", "--keep-data"))
        self.assertFalse(smoke.has_flag("--scope only", "-s"))
        self.assertFalse(smoke.has_flag("--runs-per-case", "--runs"))

    def write_hooks(self, d, data):
        path = Path(d) / "hooks.json"
        path.write_text(data if isinstance(data, str) else json.dumps(data), encoding="utf-8")
        return path

    def test_scanner_finds_sh_session_start_commands(self):
        hooks = {"hooks": {
            "SessionStart": [
                {"hooks": [{"type": "command", "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/session-start.sh"]},
                           {"type": "command", "command": "python3", "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/x.py"]}]},
                {"hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/scripts/other.sh --quick"}]}],
            "PreToolUse": [{"hooks": [{"type": "command", "command": "sh", "args": ["guard.sh"]}]}]}}
        with tempfile.TemporaryDirectory() as d:
            found = smoke.sh_session_start_commands(self.write_hooks(d, hooks))
        self.assertEqual(found, [["sh", "${CLAUDE_PLUGIN_ROOT}/scripts/session-start.sh"],
                                 ["${CLAUDE_PLUGIN_ROOT}/scripts/other.sh", "--quick"]])

    def test_scanner_tolerates_missing_and_odd_files(self):
        with tempfile.TemporaryDirectory() as d:
            self.assertEqual(smoke.sh_session_start_commands(Path(d) / "absent.json"), [])
            self.assertEqual(smoke.sh_session_start_commands(self.write_hooks(d, "not json")), [])
            self.assertEqual(smoke.sh_session_start_commands(self.write_hooks(d, {"hooks": {"PreToolUse": []}})), [])
            self.assertEqual(smoke.sh_session_start_commands(self.write_hooks(d, [1, 2])), [])
            self.assertFalse(smoke.has_session_start(self.write_hooks(d, {"hooks": {}})))
            self.assertTrue(smoke.has_session_start(self.write_hooks(d, {"hooks": {"SessionStart": [{}]}})))

    def test_stream_json_helpers_separate_asked_from_returned(self):
        lines = [
            json.dumps({"type": "system", "subtype": "hook_response", "output": "Executable not found: python9"}),
            json.dumps({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "Bash", "input": {"command": "echo RAN-OK"}}]}}),
            "not json at all",
            json.dumps({"type": "user", "message": {"content": [
                {"type": "tool_result", "content": "blocked by onFailure"},
                {"type": "tool_result", "content": [{"type": "text", "text": "part one"}, {"type": "text", "text": "two"}]}]}}),
        ]
        events = smoke.stream_events("\n".join(lines))
        self.assertEqual(len(events), 3)
        self.assertEqual(smoke.tool_result_texts(events), ["blocked by onFailure", "part one two"])  # not the asked command
        self.assertIn("Executable not found", smoke.hook_failure_text(events))
        self.assertEqual(smoke.tool_uses(events), ["Bash"])

    def test_transcript_models_reads_both_layouts(self):
        with tempfile.TemporaryDirectory() as d:
            cfg = Path(d)
            for rel in ("projects/p/sess/subagents/agent-1.jsonl", "projects/p/subagents/agent-2.jsonl"):
                f = cfg / rel
                f.parent.mkdir(parents=True)
                f.write_text(json.dumps({"message": {"model": "claude-haiku-9"}}) + "\nbad\n", encoding="utf-8")
            models, files = smoke.transcript_models(cfg)
        self.assertEqual(models, ["claude-haiku-9"] * 2)
        self.assertEqual(len(files), 2)
        self.assertEqual(smoke.transcript_models(Path("/nonexistent-dir"))[1], [])

    def transcript(self, *calls):
        """A synthetic subagent transcript: one assistant message per (tool, input) pair."""
        lines = [json.dumps({"type": "assistant", "message": {"content": [
            {"type": "text", "text": "working"}, {"type": "tool_use", "name": name, "input": inp}]}})
            for name, inp in calls]
        return "\n".join(lines + ["not json"])

    def test_startup_order_helper_accepts_reads_in_order_before_the_glob(self):
        text = self.transcript(
            ("Read", {"file_path": "/x/plugins/anthology/knowledge/caveman/SKILL.md"}),
            ("Read", {"file_path": "/x/plugins/anthology/knowledge/documentation/SKILL.md"}),
            ("Read", {"file_path": "/x/plugins/anthology/knowledge/design-skills/skills/ux-writing/SKILL.md"}),
            ("Glob", {"pattern": "*.md"}))
        calls = smoke.transcript_calls(text)
        self.assertEqual(len(calls), 4)
        self.assertEqual(smoke.startup_problems(calls, ["caveman", "documentation", "ds/ux-writing"]), [])

    def test_startup_order_helper_reports_missing_late_and_out_of_order_reads(self):
        text = self.transcript(
            ("Glob", {"pattern": "**/knowledge/**"}),  # a discovery Glob is not the *.md anchor
            ("Read", {"file_path": "/x/knowledge/documentation/SKILL.md"}),
            ("Read", {"file_path": "/x/knowledge/caveman/SKILL.md"}),
            ("Glob", {"pattern": "*.md"}),
            ("Read", {"file_path": "/x/knowledge/design-skills/skills/ux-writing/SKILL.md"}))
        problems = smoke.startup_problems(smoke.transcript_calls(text),
                                          ["caveman", "documentation", "ds/ux-writing", "tech-debt"])
        self.assertEqual(problems, ["documentation read out of order", "ds/ux-writing read after the Glob",
                                    "tech-debt never read"])
        self.assertEqual(smoke.startup_problems([("Read", "{}")], ["caveman"]), ["no Glob of *.md"])

    def test_startup_regex_does_not_confuse_a_design_skill_with_a_plain_one(self):
        self.assertRegex("knowledge/design-skills/skills/ux-writing/SKILL.md", smoke.startup_regex("ds/ux-writing"))
        self.assertNotRegex("knowledge/ux-writing/SKILL.md", smoke.startup_regex("ds/ux-writing"))
        self.assertNotRegex("knowledge/tech-debt/SKILLxmd", smoke.startup_regex("tech-debt"))

    def test_skill_heading_reads_the_first_h1_after_the_frontmatter(self):
        self.assertEqual(smoke.skill_heading("writing-plans"), "Writing Plans")

    def test_parse_agent_on_a_fixture(self):
        text = ("---\nname: x\nmodel: opus\nisolation: worktree\nskills:\n  - anthology-superpowers:writing-plans\n"
                "  - anthology-superpowers:test-driven-development\n---\n\n## Startup\nRead:\n"
                "- `${CLAUDE_PLUGIN_ROOT}/knowledge/caveman/SKILL.md`\n"
                "- `${CLAUDE_PLUGIN_ROOT}/knowledge/design-skills/skills/ux-writing/SKILL.md`\n"
                "Prose with `${CLAUDE_PLUGIN_ROOT}/knowledge/ignored/SKILL.md` inline.\n\n"
                "## Role\n- `${CLAUDE_PLUGIN_ROOT}/knowledge/after-section/SKILL.md`\n")
        self.assertEqual(smoke.parse_agent(text), {"model": "opus", "worktree": True,
                                                   "preloads": ["writing-plans", "test-driven-development"],
                                                   "startup": ["caveman", "ds/ux-writing"]})
        self.assertEqual(smoke.parse_agent("no frontmatter\n"), {"model": "?", "worktree": False, "preloads": [], "startup": []})

    def test_parse_agent_on_a_real_agent_file(self):
        agent = smoke.parse_agent((ROOT / "plugins/anthology/agents/implementer.md").read_text(encoding="utf-8"))
        self.assertEqual(agent["model"], "sonnet")
        self.assertIs(agent["worktree"], True)
        self.assertEqual(agent["preloads"], ["test-driven-development", "verification-before-completion"])
        self.assertEqual(agent["startup"][0], "caveman")

    def test_the_parser_agrees_with_the_expected_roster(self):
        try:
            from _repo import EXPECTED_ROSTER
        except ImportError:
            self.skipTest("PyYAML is not installed")
        agents = smoke.load_agents(ROOT / "plugins" / "anthology" / "agents")
        self.assertEqual(set(agents), set(EXPECTED_ROSTER))
        self.assertEqual(len(agents), smoke.EXPECTED_AGENTS)
        for name, spec in EXPECTED_ROSTER.items():
            self.assertEqual(agents[name]["model"], spec["model"], name)
            self.assertEqual(agents[name]["worktree"], spec["worktree"], name)
            self.assertEqual(agents[name]["preloads"], spec["preloads"], name)
            self.assertEqual(agents[name]["startup"], ["caveman"] + spec["knowledge"], name)

    def test_cost_and_text_from_json_and_stream_json(self):
        one = json.dumps({"type": "result", "result": "hi", "total_cost_usd": 0.25})
        self.assertEqual(smoke.result_cost_and_text(one), (0.25, "hi"))
        self.assertEqual(smoke.result_cost_and_text(json.dumps([{"type": "system"}, json.loads(one)])), (0.25, "hi"))
        self.assertEqual(smoke.result_cost_and_text("noise\n" + json.dumps({"type": "system"}) + "\n" + one), (0.25, "hi"))
        self.assertEqual(smoke.result_cost_and_text("not json"), (None, None))
        self.assertEqual(smoke.result_cost_and_text(json.dumps({"type": "result"})), (None, None))

    def test_hook_failure_text_ignores_ids_and_the_not_found_patterns_are_specific(self):
        events = [{"type": "system", "subtype": "hook_response", "uuid": "1277-abc", "session_id": "x127y",
                   "output": "ok", "exit_code": 0}]
        self.assertFalse(smoke.NOT_FOUND_RE.search(smoke.hook_failure_text(events)))
        events = [{"type": "system", "subtype": "hook_response", "exit_code": 127}]
        self.assertTrue(smoke.NOT_FOUND_RE.search(smoke.hook_failure_text(events)))
        self.assertTrue(smoke.NOT_FOUND_RE.search("python9: command not found"))
        self.assertTrue(smoke.BLOCKED_RE.search('Blocking because onFailure is "block"'))
        self.assertFalse(smoke.BLOCKED_RE.search("warning: unknown key onFailure"))

    def test_transcript_cwds_reads_the_cwd_field_only(self):
        text = "\n".join([json.dumps({"cwd": "/w/.claude/worktrees/agent-1", "message": {}}),
                          json.dumps({"message": {"content": "error: /w/.claude/worktrees/agent-2 missing"}}), "bad"])
        self.assertEqual(smoke.transcript_cwds(text), ["/w/.claude/worktrees/agent-1"])

    def test_read_allow_uses_the_absolute_path_form(self):
        rule = smoke.READ_ALLOW["permissions"]["allow"][0]
        self.assertTrue(rule.startswith("Read(//") and rule.endswith("/plugins/**)"), rule)


@unittest.skipUnless(os.path.exists("/bin/sh"), "no /bin/sh for the stub claude")
class MainWithStubTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.tmp = Path(self._tmp.name)
        self.stub = self.tmp / "claude"
        self.stub.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")  # writes nothing
        self.stub.chmod(self.stub.stat().st_mode | stat.S_IXUSR)

    def test_model_smokes_skip_without_credentials(self):
        out_path = self.tmp / "out" / "smoke.json"
        with no_auth_environ():
            code, out = run_main(["--claude", str(self.stub), "--out", str(out_path), *MODEL_SMOKES])
        self.assertEqual(code, 0)
        summary = json.loads(out_path.read_text(encoding="utf-8"))
        self.assertEqual(list(summary), ["smokes"])
        self.assertEqual(list(summary["smokes"]), MODEL_SMOKES)
        for name, result in summary["smokes"].items():
            self.assertEqual(result, {"status": "SKIP", "reason": smoke.NO_AUTH}, name)
        lines = out.strip().splitlines()
        self.assertEqual(len(lines), len(MODEL_SMOKES) + 1)
        self.assertTrue(all(l.startswith("SKIP ") for l in lines[:-1]))
        self.assertEqual(lines[-1], "smoke spend: $0.00 of $10.00")

    def test_a_failing_smoke_exits_1_and_is_summarised(self):
        out_path = self.tmp / "smoke.json"
        with no_auth_environ():  # the stub knows no subcommand flags, so cli-syntax FAILs
            code, out = run_main(["--claude", str(self.stub), "--out", str(out_path), "cli-syntax"])
        self.assertEqual(code, 1)
        result = json.loads(out_path.read_text(encoding="utf-8"))["smokes"]["cli-syntax"]
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("lacks", result["reason"])
        self.assertTrue(out.startswith("FAIL cli-syntax: "))

    def test_a_missing_binary_fails_when_named_and_skips_only_by_default(self):
        absent = str(self.tmp / "absent")
        with no_auth_environ():
            code, out = run_main(["--claude", absent, "cli-syntax"])
            self.assertEqual(code, 1)
            self.assertTrue(out.startswith("FAIL cli-syntax: claude binary not found"))
            with mock.patch.dict(os.environ, {"CLAUDE_BIN": absent}):
                self.assertEqual(run_main(["cli-syntax"])[0], 1)
            os.environ.pop("CLAUDE_BIN", None)
            with mock.patch.object(smoke, "default_claude", return_value=absent):
                code, out = run_main(["cli-syntax"])
        self.assertEqual(code, 0)
        self.assertTrue(out.startswith("SKIP cli-syntax: claude binary not found"))

    def test_budgets_are_validated(self):
        for argv in (["--total-budget-usd", "0"], ["--total-budget-usd", "abc"], ["--budget-usd", "abc"],
                     ["--budget-usd", "-1"], ["--budget-usd", "nan"]):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as cm:
                smoke.main([*argv, "cli-syntax"])
            self.assertEqual(cm.exception.code, 2, argv)

    def reporting_stub(self, cost):
        stub = self.tmp / "reporter"
        body = {"type": "result", "result": "hi", "total_cost_usd": cost} if cost is not None else {"type": "result"}
        stub.write_text("#!/bin/sh\necho '%s'\n" % json.dumps(body), encoding="utf-8")
        stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
        return str(stub)

    def test_reported_costs_are_summed_and_stop_the_launches(self):
        ctx = smoke.Ctx(self.reporting_stub(0.4), "1.00", total=1.0)
        for _ in range(3):
            self.assertEqual(ctx.prompt_run(self.tmp, "x", []).out, "hi")
        self.assertAlmostEqual(ctx.spent, 1.2)
        with self.assertRaises(smoke.BudgetExhausted):
            ctx.prompt_run(self.tmp, "x", [])
        status, reason = smoke.run_smoke("cli-syntax", ctx)
        self.assertEqual(status, smoke.FAIL)
        self.assertIn("smoke budget exhausted", reason)

    def test_an_unreported_cost_counts_as_the_per_run_budget(self):
        ctx = smoke.Ctx(self.reporting_stub(None), "0.75", total=10.0)
        ctx.prompt_run(self.tmp, "x", [])
        self.assertAlmostEqual(ctx.spent, 0.75)

    def test_a_smoke_that_hits_the_budget_mid_way_fails(self):
        ctx = smoke.Ctx(self.reporting_stub(2.0), "1.00", total=1.0)
        with mock.patch.dict(os.environ, {"ANTHROPIC_API_KEY": "sk-fake-key"}):
            status, reason = smoke.run_smoke("context-cap", ctx)  # first run costs 2.0, the second cannot start
        self.assertEqual(status, smoke.FAIL)
        self.assertIn("smoke budget exhausted", reason)

    def test_python_missing_passes_against_the_real_session_start_wrapper(self):
        self.assertTrue((smoke.ROOT / "plugins" / "anthology" / "scripts" / "session-start.sh").is_file())
        with no_auth_environ():
            code, out = run_main(["--claude", str(self.stub), "python-missing"])
        self.assertRegex(out, r"^PASS python-missing: 1 wrapper\(s\) warn about the missing python3")
        self.assertEqual(code, 0)

    def make_tree(self, tmp, script):
        scripts = Path(tmp) / "plugins" / "anthology" / "scripts"
        hooks = Path(tmp) / "plugins" / "anthology" / "hooks"
        scripts.mkdir(parents=True)
        hooks.mkdir(parents=True)
        (hooks / "hooks.json").write_text(json.dumps({"hooks": {"SessionStart": [{"hooks": [
            {"type": "command", "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/session-start.sh"]}]}]}}))
        if script is not None:
            (scripts / "session-start.sh").write_text(script)

    def run_python_missing_in(self, script):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(smoke, "ROOT", Path(tmp)), no_auth_environ():
            self.make_tree(tmp, script)
            return run_main(["--claude", str(self.stub), "python-missing"])

    def test_python_missing_skips_when_the_wrapper_is_absent(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(smoke, "ROOT", Path(tmp)), no_auth_environ():
            code, out = run_main(["--claude", str(self.stub), "python-missing"])
        self.assertRegex(out, r"^SKIP python-missing: ")
        self.assertEqual(code, 0)

    def test_python_missing_fails_when_the_wrapper_prints_nothing(self):
        code, out = self.run_python_missing_in("#!/bin/sh\nexit 0\n")
        self.assertRegex(out, r"^FAIL python-missing: ")
        self.assertNotEqual(code, 0)

    def test_python_missing_fails_when_the_wrapper_exits_nonzero(self):
        warn = """printf '%s\\n' '{"systemMessage":"python3 is missing"}'"""
        code, out = self.run_python_missing_in("#!/bin/sh\n" + warn + "\nexit 1\n")
        self.assertRegex(out, r"^FAIL python-missing: .*exit 1")
        self.assertNotEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
