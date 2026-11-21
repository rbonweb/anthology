"""Tests for guard.py (the hook entry point) and hooks.json."""
from __future__ import annotations

import json
import os
import subprocess
import sys
import unittest

from _repo import ROOT

PLUGIN = ROOT / "plugins" / "anthology-guardrails"
GUARD = PLUGIN / "scripts" / "guard.py"
HOOKS = PLUGIN / "hooks" / "hooks.json"
sys.path.insert(0, str(PLUGIN / "scripts"))
import guard_rules  # noqa: E402

HOME = "/home/tester"


def run_guard(stdin, home=HOME):
    """Run guard.py with `stdin` (str or bytes); return the CompletedProcess (bytes streams)."""
    if isinstance(stdin, str):
        stdin = stdin.encode("utf-8")
    env = {k: v for k, v in os.environ.items() if k != "HOME"}
    if home is not None:
        env["HOME"] = home
    return subprocess.run(
        [sys.executable, str(GUARD)], input=stdin, capture_output=True, env=env, timeout=60)


def hook_input(tool_name, tool_input):
    return json.dumps({
        "hook_event_name": "PreToolUse", "tool_name": tool_name, "tool_input": tool_input})


class GuardCliTest(unittest.TestCase):
    def test_blocked_payloads_exit_2_with_the_message_on_stderr(self):
        cases = [
            ("G1", "Bash", {"command": "npx playwright install"}),
            ("G2", "Bash", {"command": "open -a \"Google Chrome\" https://x.dev"}),
            ("G3", "Read", {"file_path": "~/.config/google-chrome/Default/Cookies"}),
            ("G4", "Bash", {"command": "agent-browser open http://localhost:3000"}),
            ("G5", "mcp__playwright__browser_navigate", {"url": "https://x.dev"}),
            ("G6", "Write", {"file_path": ".claude/memory/a.md",
                             "content": "ghp" + "_abc123"}),
        ]
        for rule, tool, tool_input in cases:
            with self.subTest(rule=rule):
                proc = run_guard(hook_input(tool, tool_input))
                self.assertEqual(proc.returncode, 2)
                self.assertEqual(
                    proc.stderr.decode("utf-8"),
                    f"Blocked by anthology-guardrails ({rule}): {guard_rules.MESSAGES[rule]}\n")
                self.assertEqual(proc.stdout, b"")

    def test_too_complex_command_exits_2_naming_the_way_out(self):
        command = ("$(cat <<EOF\n'" * 8000)[:100_000]           # over the work budget
        proc = run_guard(hook_input("Bash", {"command": command}))
        self.assertEqual(proc.returncode, 2)
        self.assertEqual(proc.stderr.decode("utf-8"),
                         f"Blocked by anthology-guardrails (G0): {guard_rules.TOO_COMPLEX}\n")
        self.assertIn("/plugin disable anthology-guardrails@anthology", guard_rules.TOO_COMPLEX)

    def test_g1_message_wording(self):
        proc = run_guard(hook_input("Bash", {"command": "npx playwright install"}))
        self.assertEqual(proc.returncode, 2)
        self.assertIn(
            "run it inside Docker (OrbStack); for browser work use BrowserOS Neo",
            proc.stderr.decode("utf-8"))

    def test_allowed_payloads_exit_0_silently(self):
        cases = [
            ("Bash", {"command": "git status"}),
            ("Bash", {"command": "docker compose run e2e npx playwright test"}),
            ("Read", {"file_path": "src/chrome-extension/manifest.json"}),
            ("mcp__browseros-neo__navigate", {"url": "https://x.dev"}),
            ("WebFetch", {"url": "https://x.dev"}),
            ("Bash", {}),
            ("Bash", None),
        ]
        for tool, tool_input in cases:
            with self.subTest(tool=tool, tool_input=tool_input):
                proc = run_guard(hook_input(tool, tool_input))
                self.assertEqual(proc.returncode, 0)
                self.assertEqual(proc.stderr, b"")
                self.assertEqual(proc.stdout, b"")

    def test_malformed_input_exits_0(self):
        cases = [
            "not json", "", "   ", "{", "[", "null", "[]", "[1, 2]", "42", '"str"', "true",
            "{}", '{"tool_name": 5}', '{"tool_name": null}', '{"tool_name": ["Bash"]}',
            '{"tool_input": {"command": "npx playwright install"}}',
            '{"tool_name": "Bash"}', '{"tool_name": "Bash", "tool_input": "npx playwright install"}',
            '{"tool_name": "Bash", "tool_input": null}',
            '{"tool_name": "Bash", "tool_input": []}',
            '{"tool_name": "Bash", "tool_input": {"command": 5}}',
            '{"tool_name": "Bash", "tool_input": {"command": null}}',
            "[" * 100_000, b"\xff\xfe\x00 not utf-8", b'{"tool_name": "Bash", "tool_input": {"command": "ls \xff"}}',
        ]
        for stdin in cases:
            with self.subTest(stdin=repr(stdin)[:50]):
                proc = run_guard(stdin)
                self.assertEqual(proc.returncode, 0)
                self.assertEqual(proc.stdout, b"")

    def test_invalid_utf8_inside_a_blocked_command_still_blocks(self):
        raw = (b'{"tool_name": "Bash", "tool_input": {"command": "echo \xff; '
               b'npx playwright install"}}')
        proc = run_guard(raw)
        self.assertEqual(proc.returncode, 2)
        self.assertIn("(G1)", proc.stderr.decode("utf-8"))

    def test_self_test_sample_exits_2(self):
        proc = run_guard(json.dumps(guard_rules.SELF_TEST_SAMPLE))
        self.assertEqual(proc.returncode, 2)
        self.assertIn("(G1)", proc.stderr.decode("utf-8"))

    def test_home_comes_from_the_environment(self):
        payload = hook_input("Read", {"file_path": "~/.config/chromium/x"})
        self.assertEqual(run_guard(payload, home="/home/someone").returncode, 2)
        # With no HOME at all the guard falls back to expanduser and still works.
        self.assertEqual(run_guard(payload, home=None).returncode, 2)
        payload = hook_input("Bash", {"command": "cd ~ && ls .config/chromium"})
        self.assertEqual(run_guard(payload, home="/home/someone").returncode, 2)

    def test_internal_errors_fail_open(self):
        # A rules module that raises must not brick the session: shadow guard_rules.
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            scripts = os.path.join(tmp, "scripts")
            os.mkdir(scripts)
            with open(GUARD, encoding="utf-8") as src:
                code = src.read()
            with open(os.path.join(scripts, "guard.py"), "w", encoding="utf-8") as dst:
                dst.write(code)
            with open(os.path.join(scripts, "guard_rules.py"), "w", encoding="utf-8") as dst:
                dst.write("raise RuntimeError('boom')\n")
            proc = subprocess.run(
                [sys.executable, os.path.join(scripts, "guard.py")],
                input=hook_input("Bash", {"command": "npx playwright install"}).encode(),
                capture_output=True, timeout=60)
        self.assertEqual(proc.returncode, 0)

    def test_runs_from_any_working_directory(self):
        import tempfile
        with tempfile.TemporaryDirectory() as tmp:
            proc = subprocess.run(
                [sys.executable, str(GUARD)],
                input=hook_input("Bash", {"command": "npx playwright install"}).encode(),
                capture_output=True, cwd=tmp, timeout=60)
        self.assertEqual(proc.returncode, 2)


class HooksJsonTest(unittest.TestCase):
    def test_hooks_json_is_the_exec_form_pretooluse_hook(self):
        data = json.loads(HOOKS.read_text(encoding="utf-8"))
        self.assertEqual(data, {"hooks": {"PreToolUse": [{
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": "python3",
                "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/guard.py"],
            }],
        }]}})

    def test_hooks_json_points_at_an_existing_script(self):
        data = json.loads(HOOKS.read_text(encoding="utf-8"))
        hook = data["hooks"]["PreToolUse"][0]["hooks"][0]
        script = hook["args"][0].replace("${CLAUDE_PLUGIN_ROOT}", str(PLUGIN))
        self.assertTrue(os.path.isfile(script), script)

    def test_only_pretooluse_is_registered(self):
        data = json.loads(HOOKS.read_text(encoding="utf-8"))
        self.assertEqual(list(data["hooks"]), ["PreToolUse"])

    def test_hooks_json_is_pretty_printed(self):
        text = HOOKS.read_text(encoding="utf-8")
        self.assertTrue(text.endswith("\n"))
        self.assertGreater(len(text.splitlines()), 5)


class SourceHygieneTest(unittest.TestCase):
    def test_python_files_declare_future_annotations(self):
        for path in (GUARD, PLUGIN / "scripts" / "guard_rules.py"):
            with self.subTest(path=path.name):
                text = path.read_text(encoding="utf-8")
                self.assertIn("from __future__ import annotations", text)

    def test_no_machine_specific_paths(self):
        needle = "/" + "Us" + "ers/"
        for path in (GUARD, PLUGIN / "scripts" / "guard_rules.py", HOOKS,
                     ROOT / "tests" / "test_guard_rules.py", ROOT / "tests" / "test_guard_cli.py"):
            with self.subTest(path=path.name):
                self.assertNotIn(needle, path.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
