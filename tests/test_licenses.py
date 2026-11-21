"""Every installed plugin directory carries its license text.

Claude Code installs each plugin directory into its cache on its own, so the texts in the
repo-root vendor/licenses/ do not travel with it. scripts/vendor.py build ships each
source's license (and upstream NOTICE files) inside the knowledge folder; the first-party
plugins carry a copy of the repo LICENSE. `vendor.py check` proves the vendored copies
byte for byte; these tests guard the result in the committed tree.
"""
import posixpath
import sys
import unittest
from pathlib import Path

from _repo import ROOT

sys.path.insert(0, str(ROOT / "scripts"))
import vendor  # noqa: E402

KNOWLEDGE = ROOT / "plugins" / "anthology" / "knowledge"
LICENSE_NAMES = ("LICENSE", "LICENSE.txt", "LICENSE.md")


def knowledge_sources():
    return [e for e in vendor.vendored(vendor.load_sources(ROOT))
            if e["target"].startswith("plugins/anthology/knowledge/")]


class KnowledgeLicenseTest(unittest.TestCase):
    def test_every_knowledge_folder_has_a_license(self):
        folders = sorted(p for p in KNOWLEDGE.iterdir() if p.is_dir())
        self.assertEqual(len(folders), 29)
        for folder in folders:
            self.assertTrue(any((folder / n).is_file() for n in LICENSE_NAMES),
                            f"{folder.name} ships no LICENSE, LICENSE.txt or LICENSE.md")

    def test_every_knowledge_source_ships_the_license_it_names(self):
        sources = knowledge_sources()
        self.assertEqual(len(sources), 29)
        for e in sources:
            name = e["license_file"] and posixpath.basename(e["license_file"]) or "LICENSE"
            shipped = ROOT / e["target"] / name
            self.assertTrue(shipped.is_file(), f"{e['name']}: {name} is not shipped")
            self.assertTrue(shipped.read_bytes().strip(), f"{e['name']}: {name} is empty")

    def test_licenses_from_vendor_licenses_match_the_fixed_text(self):
        checked = 0
        for e in knowledge_sources():
            if e.get("url"):
                continue
            name = e["license_file"] and posixpath.basename(e["license_file"]) or "LICENSE"
            expected = (ROOT / "vendor" / "licenses" / f"{vendor.source_slug(e)}.txt").read_bytes()
            self.assertEqual((ROOT / e["target"] / name).read_bytes(), expected, e["name"])
            checked += 1
        self.assertEqual(checked, 28)

    def test_the_fixed_text_ships_as_license_for_web_design_guidelines(self):
        e = {s["name"]: s for s in knowledge_sources()}["web-design-guidelines"]
        self.assertIsNone(e["license_file"])
        self.assertEqual((ROOT / e["target"] / "LICENSE").read_bytes(),
                         (ROOT / "vendor" / "licenses" / "vercel-labs-agent-skills.txt").read_bytes())

    def test_the_fixed_text_ships_as_license_for_karpathy_guidelines(self):
        e = {s["name"]: s for s in knowledge_sources()}["karpathy-guidelines"]
        self.assertIsNone(e["license_file"])
        text = (ROOT / e["target"] / "LICENSE").read_text(encoding="utf-8")
        self.assertIn("MIT License", text)
        self.assertIn("forrestchang", text)
        self.assertIn("multica-ai/andrej-karpathy-skills", text.splitlines()[0])

    def test_caveman_ships_its_notice_and_the_mit_text(self):
        caveman = KNOWLEDGE / "caveman"
        self.assertTrue((caveman / "NOTICE").read_text(encoding="utf-8").strip())
        self.assertIn("MIT License", (caveman / "LICENSE-MIT").read_text(encoding="utf-8"))
        entry = {s["name"]: s for s in knowledge_sources()}["caveman"]
        self.assertEqual(entry["license_extra"], ["NOTICE", "LICENSE-MIT"])


class PluginRootLicenseTest(unittest.TestCase):
    def test_every_plugin_root_has_a_license(self):
        plugins = sorted(p for p in (ROOT / "plugins").iterdir() if p.is_dir())
        self.assertEqual([p.name for p in plugins],
                         ["anthology", "anthology-guardrails", "anthology-superpowers"])
        for plugin in plugins:
            self.assertTrue(any((plugin / n).is_file() for n in LICENSE_NAMES),
                            f"{plugin.name} ships no LICENSE")

    def test_first_party_plugins_carry_the_repo_license(self):
        text = (ROOT / "LICENSE").read_bytes()
        self.assertIn(b"Apache License", text)
        for name in ("anthology", "anthology-guardrails"):
            self.assertEqual((ROOT / "plugins" / name / "LICENSE").read_bytes(), text, name)


if __name__ == "__main__":
    unittest.main()
