import json
import os
import re
import sys
import unittest
from pathlib import Path

from _repo import (EXPECTED_ROSTER, ROOT, SUPERPOWERS, knowledge_path, load_frontmatter,
                   superpowers_skill_path)

sys.path.insert(0, str(ROOT / "scripts"))
import vendor  # noqa: E402

SEMVER = re.compile(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$")
PLUGINS = {"anthology", "anthology-guardrails", "anthology-superpowers"}


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


class MarketplaceTest(unittest.TestCase):
    def setUp(self):
        self.marketplace = read_json(ROOT / ".claude-plugin" / "marketplace.json")

    def test_marketplace_name_and_plugins(self):
        self.assertEqual(self.marketplace["name"], "anthology")
        self.assertEqual({p["name"] for p in self.marketplace["plugins"]}, PLUGINS)
        for p in self.marketplace["plugins"]:
            self.assertEqual(p["source"], f"./plugins/{p['name']}")

    def read_manifest(self, name):
        manifest = read_json(ROOT / "plugins" / name / ".claude-plugin" / "plugin.json")
        self.assertEqual(manifest["name"], name)
        return manifest

    def check_first_party(self, name):
        manifest = self.read_manifest(name)
        self.assertRegex(manifest["version"], SEMVER)
        self.assertEqual(manifest["license"], "Apache-2.0")
        return manifest

    def test_anthology_manifest(self):
        manifest = self.check_first_party("anthology")
        deps = {d["name"]: d["version"] for d in manifest["dependencies"]}
        self.assertEqual(set(deps), {"anthology-guardrails", "anthology-superpowers"})
        for version in deps.values():
            self.assertTrue(version, "dependency needs a version range")
        self.assertTrue(manifest["engines"]["claude-code"])

    def test_guardrails_manifest(self):
        self.check_first_party("anthology-guardrails")

    # anthology-superpowers is vendored: the upstream license stays MIT and it has its
    # own version, so it is not held to the first-party license check.
    def test_superpowers_manifest(self):
        manifest = self.read_manifest("anthology-superpowers")
        self.assertEqual(manifest["license"], "MIT")
        self.assertRegex(manifest["version"], SEMVER)
        sources = {s["name"]: s for s in read_json(ROOT / "vendor" / "sources.json")["sources"]}
        self.assertEqual(manifest["version"], sources["superpowers"]["plugin_version"])
        deps = {d["name"]: d["version"]
                for d in read_json(ROOT / "plugins" / "anthology" / ".claude-plugin"
                                   / "plugin.json")["dependencies"]}
        major, minor, _ = manifest["version"].split(".")
        self.assertTrue(deps["anthology-superpowers"].lstrip("~^").startswith(f"{major}.{minor}"),
                        "the dependency range must resolve to the vendored version")


class PinnedImagesTest(unittest.TestCase):
    """Each pinned toolchain image is a one-line Dockerfile, so Dependabot can bump it."""

    FROM = re.compile(r"^FROM \S+:\S+@sha256:[0-9a-f]{64}$")

    def test_dockerfiles_pinned_by_digest(self):
        files = sorted((ROOT / "ci" / "images").glob("*.Dockerfile"))
        self.assertEqual({f.name for f in files},
                         {"python.Dockerfile", "python39.Dockerfile", "rust.Dockerfile",
                          "gitleaks.Dockerfile"})
        for f in files:
            lines = [l for l in f.read_text(encoding="utf-8").splitlines() if l.strip()]
            self.assertEqual(len(lines), 1, f"{f.name}: expected a single FROM line")
            self.assertRegex(lines[0], self.FROM, f"{f.name}: FROM must be tag@sha256:digest")


PRELOADS = sorted({s for a in EXPECTED_ROSTER.values() for s in a["preloads"]})
DESIGN_SKILLS = ["accessibility-audit", "design-critique", "design-elevation", "design-ops",
                 "design-systems", "interaction-design", "journey-mapping", "ux-research",
                 "ux-strategy", "ux-writing"]
# A reference is `superpowers:<skill>` not preceded by another name character.
UNPREFIXED_REF = re.compile(rb"(?<![\w./-])superpowers:(?=[a-z0-9-])")


def files_under(root):
    return [p for p in sorted(Path(root).rglob("*")) if p.is_file() and not p.is_symlink()]


class UnchangedCopiesTest(unittest.TestCase):
    """The vendored trees exist; `vendor.py check` (CI) proves they match upstream."""

    def test_copies_exist_with_their_skill_files(self):
        for ref in ("frontend-design", "web-design-guidelines"):
            path = knowledge_path(ref)
            self.assertTrue(path.is_file(), path)
            meta, body = load_frontmatter(path)
            self.assertEqual(meta["name"], ref)
            self.assertTrue(meta["description"])
            self.assertTrue(body.strip())

    def test_design_skills_resolve(self):
        for name in DESIGN_SKILLS:
            meta, _ = load_frontmatter(knowledge_path(f"ds/{name}"))
            self.assertEqual(meta["name"], name)

    def test_every_source_has_a_target_and_a_license(self):
        sources = read_json(ROOT / "vendor" / "sources.json")["sources"]
        self.assertTrue(sources)
        for s in sources:
            self.assertTrue((ROOT / s["target"]).is_dir(), s["target"])
            self.assertTrue(files_under(ROOT / s["target"]), s["target"])
        slugs = {vendor.source_slug(s) for s in sources}
        self.assertEqual({p.stem for p in (ROOT / "vendor" / "licenses").glob("*.txt")}, slugs)
        for slug in slugs:
            text = (ROOT / "vendor" / "licenses" / f"{slug}.txt").read_text(encoding="utf-8")
            self.assertTrue(text.strip(), slug)
            self.assertRegex(text, r"MIT License|Apache License")

    def test_vercel_license_is_the_fixed_mit_text(self):
        text = (ROOT / "vendor" / "licenses" / "vercel-labs-agent-skills.txt").read_text(encoding="utf-8")
        self.assertIn("README", text.splitlines()[0])
        self.assertIn("Copyright (c) Vercel", text)
        self.assertIn("MIT License", text)


class AnthologySuperpowersTest(unittest.TestCase):
    def test_manifest_name(self):
        manifest = read_json(SUPERPOWERS / ".claude-plugin" / "plugin.json")
        self.assertEqual(manifest["name"], "anthology-superpowers")

    def test_no_unprefixed_skill_references(self):
        offenders = [str(p.relative_to(SUPERPOWERS)) for p in files_under(SUPERPOWERS)
                     if UNPREFIXED_REF.search(p.read_bytes())]
        self.assertEqual(offenders, [])

    def test_prefixed_references_exist(self):
        hook = (SUPERPOWERS / "hooks" / "session-start").read_text(encoding="utf-8")
        self.assertIn("anthology-superpowers:using-superpowers", hook)

    def test_session_hook_is_executable_and_wired(self):
        self.assertTrue(os.access(SUPERPOWERS / "hooks" / "session-start", os.X_OK))
        hooks = read_json(SUPERPOWERS / "hooks" / "hooks.json")
        self.assertIn("SessionStart", hooks["hooks"])

    def test_preloads_resolve(self):
        for name in PRELOADS:
            meta, _ = load_frontmatter(superpowers_skill_path(name))
            self.assertEqual(meta["name"], name)
            self.assertTrue(meta["description"])


if __name__ == "__main__":
    unittest.main()
