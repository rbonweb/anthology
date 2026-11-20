import json
import re
import unittest

from _repo import ROOT, SUPERPOWERS

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

    # Task 2 generates anthology-superpowers. It is vendored (upstream license
    # stays MIT, own version), so only existence and name are checked here.
    @unittest.expectedFailure
    def test_superpowers_manifest(self):
        self.assertTrue(SUPERPOWERS.is_dir())
        self.read_manifest("anthology-superpowers")


class PinnedImagesTest(unittest.TestCase):
    """Each pinned toolchain image is a one-line Dockerfile, so Dependabot can bump it."""

    FROM = re.compile(r"^FROM \S+:\S+@sha256:[0-9a-f]{64}$")

    def test_dockerfiles_pinned_by_digest(self):
        files = sorted((ROOT / "ci" / "images").glob("*.Dockerfile"))
        self.assertEqual({f.name for f in files},
                         {"python.Dockerfile", "rust.Dockerfile", "gitleaks.Dockerfile"})
        for f in files:
            lines = [l for l in f.read_text(encoding="utf-8").splitlines() if l.strip()]
            self.assertEqual(len(lines), 1, f"{f.name}: expected a single FROM line")
            self.assertRegex(lines[0], self.FROM, f"{f.name}: FROM must be tag@sha256:digest")


if __name__ == "__main__":
    unittest.main()
