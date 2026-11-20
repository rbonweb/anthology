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
            self.assertEqual(env["PATH"], "/usr/bin:/bin")
            allowed = {"HOME", "CLAUDE_CONFIG_DIR", "PATH", "TMPDIR", "LANG", "TERM", "DISABLE_AUTOUPDATER",
                       "ANTHROPIC_API_KEY"}
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
            out = subprocess.run([sys.executable, str(script)], env={"CAP_N": "10001"}, check=True,
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
        self.assertIn("Executable not found", smoke.hook_event_text(events))

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

    def test_load_roster_knows_the_worktree_agents(self):
        roster = smoke.load_roster()
        if roster is None:
            self.skipTest("PyYAML is not installed")
        self.assertEqual(sorted(a for a, v in roster.items() if v["worktree"]),
                         ["debugger", "docs-writer", "implementer", "ui-designer"])

    def test_agent_models_covers_the_roster_with_known_families(self):
        roster = smoke.agent_models(ROOT / "plugins" / "anthology" / "agents")
        self.assertEqual(len(roster), 11)
        self.assertTrue(set(roster.values()) <= {"opus", "sonnet", "haiku"}, roster)


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
        self.assertEqual(len(out.strip().splitlines()), len(MODEL_SMOKES))
        self.assertTrue(all(l.startswith("SKIP ") for l in out.splitlines()))

    def test_a_failing_smoke_exits_1_and_is_summarised(self):
        out_path = self.tmp / "smoke.json"
        with no_auth_environ():  # the stub knows no subcommand flags, so cli-syntax FAILs
            code, out = run_main(["--claude", str(self.stub), "--out", str(out_path), "cli-syntax"])
        self.assertEqual(code, 1)
        result = json.loads(out_path.read_text(encoding="utf-8"))["smokes"]["cli-syntax"]
        self.assertEqual(result["status"], "FAIL")
        self.assertIn("lacks", result["reason"])
        self.assertTrue(out.startswith("FAIL cli-syntax: "))

    def test_a_missing_binary_skips(self):
        with no_auth_environ():
            code, out = run_main(["--claude", str(self.tmp / "absent"), "cli-syntax"])
        self.assertEqual(code, 0)
        self.assertTrue(out.startswith("SKIP cli-syntax: claude binary not found"))

    def test_python_missing_never_fails_the_run_while_the_wrapper_is_absent(self):
        with no_auth_environ():
            code, out = run_main(["--claude", str(self.stub), "python-missing"])
        self.assertRegex(out, r"^(SKIP|PASS) python-missing: ")
        self.assertEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
