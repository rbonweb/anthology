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

# Each pattern is a launch of an OS browser (or the opt-in that triggers one), with sample
# strings it must match: the self-test below proves no pattern is vacuous.
_CALL = r"\b(exec|execFile|spawn)(Sync)?\(\s*['\"`](/usr/bin/|/bin/)?"
_CMD = r"(open|xdg-open|start|wslview)(?![\w.-])"
LAUNCH_SAMPLES = {
    r"xdg-open": ["xdg-open $URL"],
    r"wslview": ["wslview $URL"],
    r"sensible-browser": ["sensible-browser $URL"],
    r"x-www-browser": ["x-www-browser $URL"],
    r"gio\s+open": ["gio open $URL"],
    r"rundll32": ["rundll32.exe url.dll,FileProtocolHandler x"],
    r"FileProtocolHandler": ["url.dll,FileProtocolHandler x"],
    r"BRAINSTORM_OPEN": ["BRAINSTORM_OPEN=1", "BRAINSTORM_OPEN_CMD=x"],
    r"open\s+-a\b": ["open -a Safari"],
    r"cmd(\.exe)?\s+/c\s+start": ["cmd /c start x", "cmd.exe /c start x"],
    r"start\s+\"\"": ["start \"\" http://x"],
    r"\bopen\s+[\"']?https?://": ["open https://example.com", "open 'http://x'"],
    r"\bopen\s+[\"']?\$": ["open \"$URL\"", "open '$URL'", "open '${URL}'", "open $URL"],
    _CALL + _CMD: [
        "execFile('open', [url])", "cp.execFile( \"xdg-open\", args)", "spawn('start', [url])",
        "cp.exec(\"open \" + url)", "exec(`xdg-open ${url}`)", "execSync('wslview x')",
        "spawn(\"/usr/bin/open\", [url])",
    ],
    r"bin:\s*['\"](/usr/bin/)?(open|xdg-open|start)['\"]": [
        "{ bin: 'open', args: [url] }", "{ bin: \"xdg-open\" }",
    ],
    r"\bwebbrowser\b": ["import webbrowser", "python -m webbrowser -t x", "webbrowser.open(u)"],
    r"Start-Process": ["Start-Process $url"],
    r"explorer(\.exe)?\s+[\"']?https?://": ["explorer.exe http://x", "explorer \"https://x\""],
    r"open\s+location": ["osascript -e 'open location \"http://x\"'"],
    r"require\(\s*['\"]open['\"]\s*\)": ["require('open')", "const o = require( \"open\" )"],
    r"import\s+\w+\s+from\s+['\"]open['\"]": ["import open from 'open'"],
    r"/usr/bin/open\b": ["/usr/bin/open -g x"],
    r"\bstart\s+[\"']?https?://": ["start http://x", "start \"https://x\""],
    r"start-server\.sh[^\n]*--open": ["bash scripts/start-server.sh --project-dir p --open"],
}
LAUNCH_PATTERNS = list(LAUNCH_SAMPLES)
COMPILED = {p: re.compile(p.encode(), re.IGNORECASE) for p in LAUNCH_PATTERNS}

# Things that must never trip a pattern: ordinary prose and non-launch code.
NEGATIVE_SAMPLES = [
    "ws.on('open', () => {})", "open the URL in BrowserOS Neo", "fs.open(file, 'r', cb)",
    "an open question", "keep the tab open", "the open source license",
    "WebSocket.OPEN", "exec('start-server.sh')", "spawn('node', [server])",
    "start the server", "start the server at the url", "execFile('git', ['open'])",
    "import open_things from './x'", "require('openai')", "/usr/bin/env node",
]

# (relative path, pattern, reason). Historical prose that no skill tells the agent to
# follow. Only that pattern is waived for that file, so a later sync that adds a different
# launch form there still fails; and an exemption that stops matching fails too.
EXEMPT = [
    ("docs/plans/2026-01-17-visual-brainstorming.md",
     r"\bopen\s+[\"']?https?://",
     "upstream design plan from Jan 2026; line 510 tells the human to open the local URL, "
     "no skill sends the agent here"),
]

COMPANION = SUPERPOWERS / "skills" / "brainstorming" / "visual-companion.md"


def shipped_files():
    return sorted(p for p in SUPERPOWERS.rglob("*") if p.is_file())


def matches(path):
    data = path.read_bytes()
    return [p for p, rx in COMPILED.items() if rx.search(data)]


class NoOsBrowserLaunch(unittest.TestCase):
    def test_no_shipped_file_launches_an_os_browser(self):
        files = shipped_files()
        self.assertGreater(len(files), 100, "the plugin tree is missing")
        waived = {(rel, pattern) for rel, pattern, _ in EXEMPT}
        bad = []
        for path in files:
            rel = path.relative_to(SUPERPOWERS).as_posix()
            for pattern in matches(path):
                if (rel, pattern) not in waived:
                    bad.append(f"{rel}: {pattern}")
        self.assertEqual(bad, [], "OS browser launch in the shipped superpowers plugin")

    def test_every_exemption_still_matches_and_has_a_reason(self):
        for rel, pattern, reason in EXEMPT:
            with self.subTest(rel=rel, pattern=pattern):
                self.assertIn(pattern, COMPILED, "exemption names an unknown pattern")
                self.assertTrue(reason.strip())
                path = SUPERPOWERS / rel
                self.assertTrue(path.is_file(), f"{rel} no longer exists: drop the exemption")
                self.assertIn(pattern, matches(path),
                              f"{rel} no longer matches: drop the exemption")

    def test_every_pattern_matches_its_samples(self):
        for pattern, samples in LAUNCH_SAMPLES.items():
            self.assertTrue(samples, f"{pattern}: needs at least one sample")
            for text in samples:
                with self.subTest(pattern=pattern, text=text):
                    self.assertTrue(COMPILED[pattern].search(text.encode()))

    def test_the_patterns_ignore_ordinary_text(self):
        for text in NEGATIVE_SAMPLES:
            with self.subTest(text=text):
                hit = [p for p, rx in COMPILED.items() if rx.search(text.encode())]
                self.assertEqual(hit, [])


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
