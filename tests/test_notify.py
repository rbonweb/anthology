"""Tests for scripts/notify.sh (the Notification hook) and its hooks.json entry.

The script is run for real with fake `osascript`, `notify-send`, `canberra-gtk-play`, `paplay`
and `uname` first on PATH. Each fake writes its arguments, one per line, to a log file, so the
tests see exactly what the desktop tool would have received."""
from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

from _repo import ROOT

PLUGIN = ROOT / "plugins" / "anthology"
SCRIPT = PLUGIN / "scripts" / "notify.sh"
HOOKS = PLUGIN / "hooks" / "hooks.json"
DEFAULT_MESSAGE = "Needs your attention"
MAC_ARGS = [
    "on run argv",
    'display notification (item 1 of argv) with title "Claude Code" sound name "Glass"',
    "end run",
]
HOSTILE = '"; do shell script "touch PWNED" -- $(touch PWNED) `touch PWNED` \'q\' "dq" \\ $HOME'


def hook_input(message, notification_type="permission_prompt"):
    return json.dumps({"hook_event_name": "Notification", "message": message,
                       "notification_type": notification_type})


@unittest.skipUnless(shutil.which("sh"), "sh is not installed")
class NotifyScriptTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.bin = self.tmp / "bin"
        self.log = self.tmp / "log"
        self.work = self.tmp / "work"
        for d in (self.bin, self.log, self.work):
            d.mkdir()
        self.fake("uname", 'echo "$FAKE_UNAME"')
        for name in ("osascript", "notify-send", "canberra-gtk-play", "paplay"):
            self.fake(name, 'printf "%s\\n" "$@" > "$FAKE_LOG/' + name + '"')

    def fake(self, name, body):
        path = self.bin / name
        path.write_text("#!/bin/sh\n" + body + "\n", encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)

    def run_script(self, stdin, uname="Darwin", env=None, path=None):
        full = {k: v for k, v in os.environ.items() if k != "ANTHOLOGY_NOTIFY"}
        full.update({"PATH": path or f"{self.bin}{os.pathsep}{os.environ['PATH']}",
                     "FAKE_LOG": str(self.log), "FAKE_UNAME": uname})
        full.update(env or {})
        return subprocess.run(["/bin/sh", str(SCRIPT)], input=stdin.encode(),
                              capture_output=True, cwd=self.work, env=full, timeout=30)

    def logged(self, name):
        path = self.log / name
        return path.read_text(encoding="utf-8").splitlines() if path.exists() else None

    def test_macos_passes_the_message_to_osascript_as_an_argument(self):
        proc = self.run_script(hook_input("Claude needs your permission"))
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(self.logged("osascript"),
                         ["-e", MAC_ARGS[0], "-e", MAC_ARGS[1], "-e", MAC_ARGS[2],
                          "Claude needs your permission"])

    def test_quotes_and_command_substitution_in_the_message_are_not_executed(self):
        proc = self.run_script(hook_input(HOSTILE))
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(self.logged("osascript")[-1], HOSTILE)
        self.assertFalse((self.work / "PWNED").exists())
        self.assertFalse((self.tmp / "PWNED").exists())

    def test_a_message_that_looks_like_an_option_stays_an_argument(self):
        self.run_script(hook_input("-e evil"))
        last = self.logged("osascript")[-1]
        self.assertTrue(last.startswith(" "), last)
        self.assertEqual(last.strip(), "-e evil")

    def test_newlines_collapse_and_long_messages_are_cut(self):
        self.run_script(hook_input("one\ntwo\tthree " + "x" * 500))
        sent = self.logged("osascript")[-1]
        self.assertTrue(sent.startswith("one two three xxx"), sent)
        self.assertLessEqual(len(sent), 200)

    def test_a_missing_or_non_string_message_gets_the_default_text(self):
        for stdin in ('{"hook_event_name": "Notification"}', '{"message": 7}', '{"message": ""}',
                      "not json", "", "[1, 2]"):
            with self.subTest(stdin=stdin):
                (self.log / "osascript").unlink(missing_ok=True)
                proc = self.run_script(stdin)
                self.assertEqual(proc.returncode, 0)
                self.assertEqual(self.logged("osascript")[-1], DEFAULT_MESSAGE)

    def test_anthology_notify_0_turns_the_alert_off(self):
        proc = self.run_script(hook_input("hello"), env={"ANTHOLOGY_NOTIFY": "0"})
        self.assertEqual(proc.returncode, 0)
        self.assertEqual((proc.stdout, proc.stderr), (b"", b""))
        self.assertEqual(list(self.log.iterdir()), [])

    def test_other_values_of_anthology_notify_keep_the_alert_on(self):
        self.run_script(hook_input("hello"), env={"ANTHOLOGY_NOTIFY": "1"})
        self.assertIsNotNone(self.logged("osascript"))

    def test_linux_uses_notify_send_and_a_sound_player(self):
        proc = self.run_script(hook_input(HOSTILE), uname="Linux")
        self.assertEqual(proc.returncode, 0)
        self.assertIsNone(self.logged("osascript"))
        self.assertEqual(self.logged("notify-send"), ["Claude Code", HOSTILE])
        self.assertEqual(self.logged("canberra-gtk-play"), ["-i", "message"])
        self.assertFalse((self.work / "PWNED").exists())

    def test_linux_without_notify_send_still_plays_the_sound_and_exits_0(self):
        (self.bin / "notify-send").unlink()
        path = os.pathsep.join([str(self.bin), os.defpath])
        proc = self.run_script(hook_input("hi"), uname="Linux", path=path)
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(self.logged("canberra-gtk-play"), ["-i", "message"])

    def test_with_no_notifier_at_all_it_exits_0_quietly(self):
        only = self.tmp / "only"
        only.mkdir()
        shutil.copy(self.bin / "uname", only / "uname")
        python3 = shutil.which("python3")
        if python3:
            os.symlink(python3, only / "python3")
        proc = self.run_script(hook_input("hi"), uname="Linux", path=str(only))
        self.assertEqual(proc.returncode, 0)
        self.assertEqual((proc.stdout, proc.stderr), (b"", b""))

    def test_a_failing_notifier_never_fails_the_hook(self):
        self.fake("osascript", "exit 9")
        proc = self.run_script(hook_input("hi"))
        self.assertEqual(proc.returncode, 0)
        self.assertEqual((proc.stdout, proc.stderr), (b"", b""))

    def test_script_is_posix_sh_and_executable(self):
        self.assertEqual(SCRIPT.read_text(encoding="utf-8").splitlines()[0], "#!/bin/sh")
        self.assertTrue(os.access(SCRIPT, os.X_OK))

    def test_script_never_evals_or_builds_applescript_from_the_message(self):
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertNotRegex(text, r"(?m)^\s*(eval|source)\b")
        self.assertNotRegex(text, r"-e '[^']*\$")


class NotifyHookEntryTest(unittest.TestCase):
    def test_hooks_json_registers_the_notification_hook(self):
        data = json.loads(HOOKS.read_text(encoding="utf-8"))
        self.assertEqual(data["hooks"]["Notification"], [{
            "matcher": "permission_prompt|idle_prompt",
            "hooks": [{"type": "command", "command": "sh",
                       "args": ["${CLAUDE_PLUGIN_ROOT}/scripts/notify.sh"], "timeout": 5}]}])

    def test_the_hook_script_exists(self):
        self.assertTrue(SCRIPT.is_file())


if __name__ == "__main__":
    unittest.main()
