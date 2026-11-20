"""design-taste-frontend on BrowserOS Neo (Task 5).

Runs on the committed tree and the port map only: no network, no git, no browser.
The pristine upstream is not in the repo, so the checks that need it (every upstream
script is mapped, a keep script is byte-identical) live in `vendor.py check`; the
fixture tests for them are in test_vendor.py.
"""
import json
import re
import sys
import unittest

from _repo import KNOWLEDGE, ROOT, load_frontmatter

sys.path.insert(0, str(ROOT / "scripts"))
import vendor  # noqa: E402

FOLDER = KNOWLEDGE / "design-taste-frontend"
PORTMAP = ROOT / "vendor" / "patches" / "design-taste-frontend.portmap.json"
BRIEF_REGEX = (r"remote-debugging|user-data-dir|CHROME_PATH|Google Chrome|chrome\.exe"
               r"|/usr/bin/(google-chrome|chromium)|playwright|puppeteer")
ACTIONS = {"keep", "port", "drop"}


def portmap():
    return json.loads(PORTMAP.read_text(encoding="utf-8"))


def shipped_files():
    return sorted(p for p in FOLDER.rglob("*") if p.is_file())


class NoChromeLaunchTest(unittest.TestCase):
    def test_the_folder_is_there(self):
        self.assertTrue((FOLDER / "SKILL.md").is_file())

    def test_the_port_map_carries_the_briefs_regex(self):
        self.assertEqual(portmap()["chrome_regex"], BRIEF_REGEX)

    def test_no_shipped_file_or_path_matches_the_chrome_pattern(self):
        pattern = re.compile(BRIEF_REGEX.encode(), re.IGNORECASE)
        files = shipped_files()
        self.assertGreater(len(files), 1000)
        for path in files:
            rel = path.relative_to(FOLDER).as_posix()
            self.assertIsNone(pattern.search(rel.encode()), rel)
            self.assertIsNone(pattern.search(path.read_bytes()), rel)

    def test_the_pattern_catches_what_it_should(self):
        pattern = re.compile(BRIEF_REGEX.encode(), re.IGNORECASE)
        for yes in ("--remote-debugging-port=0", "--USER-DATA-DIR=/tmp/x", "CHROME_PATH", "google chrome",
                    "Chrome.exe", "/usr/bin/chromium", "Playwright", "require('puppeteer')"):
            self.assertTrue(pattern.search(yes.encode()), yes)
        for no in ("HeadlessChrome/", "chrome://", "page.cdp('Page.captureScreenshot')"):
            self.assertIsNone(pattern.search(no.encode()), no)

    def test_the_two_automation_brand_icons_are_gone(self):
        for name in ("playwright", "puppeteer"):
            self.assertFalse((FOLDER / "assets" / "icons" / "demo" / f"{name}.svg").exists())


class SkillFileTest(unittest.TestCase):
    def test_frontmatter_has_no_hooks(self):
        meta, _ = load_frontmatter(FOLDER / "SKILL.md")
        self.assertEqual(meta["name"], "design-taste-frontend")
        self.assertNotIn("hooks", meta)
        head = (FOLDER / "SKILL.md").read_text(encoding="utf-8").split("\n---", 1)[0]
        self.assertNotRegex(head, r"(?m)^\s*hooks\s*:")
        self.assertNotIn("--hook", head)

    def test_skill_mentions_browseros_neo(self):
        text = (FOLDER / "SKILL.md").read_text(encoding="utf-8")
        self.assertIn("BrowserOS Neo", text)
        self.assertIn("mcp__browseros-neo__", text)

    def test_nothing_tells_the_agent_to_run_the_removed_tools(self):
        stale = re.compile(r"scripts/shoot\.mjs|shoot\.mjs --|gate\.mjs --hook|--profile\b|moments\.mjs shoot|backgrounds/check\.mjs")
        for path in shipped_files():
            if path.suffix in (".md", ".mjs") and path.stat().st_size < 600_000:
                text = path.read_text(encoding="utf-8", errors="replace")
                rel = path.relative_to(FOLDER).as_posix()
                self.assertIsNone(stale.search(text), f"{rel}: {stale.search(text)}")

    def test_the_gate_reference_documents_the_bridge_and_the_eyes_step(self):
        text = (FOLDER / "references" / "gate.md").read_text(encoding="utf-8")
        self.assertIn("## The bridge", text)
        self.assertIn("## The eyes", text)
        self.assertIn("## Before you finish", text)
        self.assertNotIn("## As a hook", text)


class PortMapTest(unittest.TestCase):
    def test_the_map_shape(self):
        data = portmap()
        self.assertEqual(data["upstream"]["commit"], "f6b76575f949707b4e3c37cd86cb44b0b98643e8")
        self.assertEqual(len(data["scripts"]), 20)
        for rel, spec in data["scripts"].items():
            self.assertIn(spec["action"], ACTIONS, rel)
            self.assertIsInstance(spec["chrome"], bool, rel)
            self.assertTrue(spec["why"].strip(), rel)
            if spec["action"] == "port":
                self.assertIn(spec["how"], {"bridge", "trim"}, rel)
                self.assertTrue(spec["browseros"].strip(), rel)
        counts = {a: sum(1 for s in data["scripts"].values() if s["action"] == a) for a in ACTIONS}
        self.assertEqual(counts, {"keep": 9, "port": 9, "drop": 2})
        self.assertEqual(sorted(data["added"]), ["scripts/neo.mjs"])
        self.assertFalse(set(data["added"]) & set(data["scripts"]))

    def test_the_shipped_scripts_are_exactly_keep_port_and_added(self):
        data = portmap()
        wanted = {rel for rel, s in data["scripts"].items() if s["action"] in ("keep", "port")}
        wanted |= set(data["added"])
        self.assertEqual(vendor.portmap_scripts(FOLDER, data["script_rule"]), wanted)

    def test_no_drop_script_ships(self):
        data = portmap()
        for rel, spec in data["scripts"].items():
            if spec["action"] == "drop":
                self.assertFalse((FOLDER / rel).exists(), rel)
            else:
                self.assertTrue((FOLDER / rel).is_file(), rel)
        for rel in data["added"]:
            self.assertTrue((FOLDER / rel).is_file(), rel)

    def test_every_pristine_script_has_a_valid_action(self):
        data = portmap()
        self.assertEqual({s["action"] for s in data["scripts"].values()} - ACTIONS, set())
        for rel in ("scripts/shoot.mjs", "assets/backgrounds/check.mjs"):
            self.assertEqual(data["scripts"][rel]["action"], "drop")

    def test_no_script_is_executable(self):
        # agents run `node scripts/neo.mjs`; a patch cannot add an executable file
        for rel in portmap()["added"]:
            self.assertFalse((FOLDER / rel).stat().st_mode & 0o111, rel)

    def test_the_sources_entry_points_at_the_map(self):
        entry = next(e for e in vendor.load_sources(ROOT) if e["name"] == "design-taste-frontend")
        self.assertEqual(entry["portmap"], "vendor/patches/design-taste-frontend.portmap.json")
        self.assertEqual(entry["commit"], "f6b76575f949707b4e3c37cd86cb44b0b98643e8")
        self.assertEqual((entry["repo"], entry["path"], entry["ref"], entry["license"]),
                         ("https://github.com/leonxlnx/taste-skill", "skills/taste-skill", "v2.5.1", "MIT"))
        self.assertEqual(entry["transform"], [{"patch": "vendor/patches/design-taste-frontend.patch"}])
        self.assertTrue((ROOT / "vendor" / "licenses" / "leonxlnx-taste-skill.txt").is_file())

    def test_the_not_checked_paths_are_explicit_in_the_ported_checkers(self):
        text = (FOLDER / "scripts" / "consistency.mjs").read_text(encoding="utf-8")
        self.assertIn("BrowserOS Neo: no screenshots", text)
        self.assertIn("notChecked", text)
        layout = (FOLDER / "assets" / "layout" / "check.mjs").read_text(encoding="utf-8")
        self.assertIn("--shots is not available on BrowserOS Neo", layout)


if __name__ == "__main__":
    unittest.main()
