"""The vendored superpowers plugin never launches an OS browser (final-review I9).

BrowserOS Neo is the only browser Anthology ships against. Upstream superpowers' visual
companion opened the user's default browser from a Node child process; `vendor/patches/
superpowers.patch` removes that. These tests walk every shipped file so a sync that brings
a launcher back fails here. No network, no git, no browser.
"""
import json
import re
import unittest

from _repo import ROOT, SUPERPOWERS

# Each pattern is a launch of an OS browser (or the opt-in that triggers one).
LAUNCH_PATTERNS = [
    r"xdg-open",
    r"wslview",
    r"sensible-browser",
    r"x-www-browser",
    r"gio\s+open",
    r"rundll32",
    r"FileProtocolHandler",
    r"BRAINSTORM_OPEN",
    r"open\s+-a\b",
    r"cmd(\.exe)?\s+/c\s+start",
    r"start\s+\"\"",
    r"\bopen\s+[\"']?https?://",
    r"\bopen\s+\"?\$",
    r"execFile(Sync)?\(\s*['\"](open|xdg-open|start)['\"]",
    r"bin:\s*['\"](open|xdg-open|start)['\"]",
    r"spawn(Sync)?\(\s*['\"](open|xdg-open|start)['\"]",
    r"start-server\.sh[^\n]*--open",
]
COMPILED = [(p, re.compile(p.encode(), re.IGNORECASE)) for p in LAUNCH_PATTERNS]

# (relative path, reason). Historical prose that no skill tells the agent to follow. An
# exemption is only legal while the file really matches, so a stale one fails too.
EXEMPT = [
    ("docs/plans/2026-01-17-visual-brainstorming.md",
     "upstream design plan from Jan 2026; line 510 tells the human to open the local URL, "
     "no skill sends the agent here"),
]

COMPANION = SUPERPOWERS / "skills" / "brainstorming" / "visual-companion.md"


def shipped_files():
    return sorted(p for p in SUPERPOWERS.rglob("*") if p.is_file())


def matches(path):
    data = path.read_bytes()
    return [p for p, rx in COMPILED if rx.search(data)]


class NoOsBrowserLaunch(unittest.TestCase):
    def test_no_shipped_file_launches_an_os_browser(self):
        files = shipped_files()
        self.assertGreater(len(files), 100, "the plugin tree is missing")
        exempt = {rel for rel, _ in EXEMPT}
        bad = []
        for path in files:
            rel = path.relative_to(SUPERPOWERS).as_posix()
            if rel in exempt:
                continue
            for pattern in matches(path):
                bad.append(f"{rel}: {pattern}")
        self.assertEqual(bad, [], "OS browser launch in the shipped superpowers plugin")

    def test_every_exemption_still_matches_and_has_a_reason(self):
        for rel, reason in EXEMPT:
            with self.subTest(rel=rel):
                self.assertTrue(reason.strip())
                path = SUPERPOWERS / rel
                self.assertTrue(path.is_file(), f"{rel} no longer exists: drop the exemption")
                self.assertTrue(matches(path), f"{rel} no longer matches: drop the exemption")

    def test_the_patterns_catch_what_they_are_meant_to(self):
        samples = [
            "execFile('open', [url])", "cp.execFile( \"xdg-open\", args)",
            "{ bin: 'open', args: [url] }", "spawn('start', [url])",
            "open \"$URL\"", "open https://example.com", "cmd.exe /c start http://x",
            "rundll32.exe url.dll,FileProtocolHandler", "gio open x", "open -a Safari",
            "bash scripts/start-server.sh --project-dir p --open", "BRAINSTORM_OPEN=1",
            "wslview x", "sensible-browser x", "x-www-browser x", "start \"\" http://x",
        ]
        for text in samples:
            with self.subTest(text=text):
                self.assertTrue(any(rx.search(text.encode()) for _, rx in COMPILED))
        for text in ["an open question", "keep the tab open", "the open source license"]:
            with self.subTest(text=text):
                self.assertFalse(any(rx.search(text.encode()) for _, rx in COMPILED))


class VisualCompanionUsesBrowserOs(unittest.TestCase):
    def test_visual_companion_points_at_browseros_neo_and_has_no_open_flag(self):
        text = COMPANION.read_text(encoding="utf-8")
        self.assertIn("mcp__browseros-neo__", text)
        self.assertNotIn("--open", text)

    def test_start_server_rejects_open_like_any_unknown_option(self):
        script = (SUPERPOWERS / "skills" / "brainstorming" / "scripts" / "start-server.sh")
        text = script.read_text(encoding="utf-8")
        self.assertNotIn("--open", text)
        self.assertIn("Unknown argument", text)

    def test_server_exports_no_launcher(self):
        server = (SUPERPOWERS / "skills" / "brainstorming" / "scripts" / "server.cjs")
        text = server.read_text(encoding="utf-8")
        self.assertNotIn("browserLauncherForPlatform", text)
        self.assertNotIn("child_process", text)

    def test_readme_has_the_marketplace_install_line(self):
        text = (SUPERPOWERS / "README.md").read_text(encoding="utf-8")
        self.assertIn("/plugin install anthology-superpowers@anthology", text)


class PatchIsWired(unittest.TestCase):
    def test_sources_lists_the_patch_after_rename_plugin(self):
        sources = json.loads((ROOT / "vendor" / "sources.json").read_text(encoding="utf-8"))
        entry = next(s for s in sources["sources"] if s["name"] == "superpowers")
        steps = entry["transform"]
        names = [s.get("name") or s.get("patch") for s in steps]
        self.assertIn("rename-plugin", names)
        self.assertIn("vendor/patches/superpowers.patch", names)
        self.assertLess(names.index("rename-plugin"),
                        names.index("vendor/patches/superpowers.patch"))
        self.assertTrue((ROOT / "vendor" / "patches" / "superpowers.patch").is_file())


if __name__ == "__main__":
    unittest.main()
