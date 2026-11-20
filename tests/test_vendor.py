"""Tests for scripts/vendor.py.

The unit tests run in Docker without git or network, so they exercise the
transforms and the build/check/sync/diff pipeline on local fixture trees. Fetching
is abstracted behind a `fetch(entry) -> Path` function that returns a pristine repo
checkout; the tests stub it. Real fetches run on the host and in the CI
`vendor-check` job.
"""
import ast
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from _repo import ROOT, STRIPPED_KNOWLEDGE

sys.path.insert(0, str(ROOT / "scripts"))
import vendor  # noqa: E402

SHA_A = "a" * 40
SHA_B = "b" * 40
MIT_TEXT = "MIT License\n\nCopyright (c) Someone\n"
FIXED_MIT = "Note: the license is declared in the upstream README.\n\nMIT License\n\nCopyright (c) Vercel\n"


def write(base, rel, text="", exec_bit=False):
    p = Path(base) / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(text, bytes):
        p.write_bytes(text)
    else:
        p.write_text(text, encoding="utf-8")
    p.chmod(0o755 if exec_bit else 0o644)
    return p


def entry(name, **kw):
    e = {
        "name": name,
        "repo": f"https://github.com/acme/{name}",
        "path": "",
        "ref": "main",
        "commit": SHA_A,
        "license": "MIT",
        "license_file": "LICENSE",
        "target": f"plugins/anthology/knowledge/{name}",
        "transform": [],
    }
    e.update(kw)
    return e


class FixtureCase(unittest.TestCase):
    """A scratch repo root plus pristine upstream trees, with fetch stubbed."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.root = self.tmp / "repo"
        (self.root / "vendor").mkdir(parents=True)
        self.pristine = {}

    def add_upstream(self, name, files, **entry_kw):
        """Create a pristine checkout (with a .git dir that must never be copied)."""
        repo = self.tmp / "upstream" / name
        write(repo, ".git/HEAD", "ref: refs/heads/main\n")
        for rel, text in files.items():
            exec_bit = rel.endswith(".sh") or rel.startswith("hooks/")
            write(repo, rel, text, exec_bit=exec_bit)
        self.pristine[name] = repo
        return entry(name, **entry_kw)

    def fetch(self, e):
        return self.pristine[e["name"]]

    def set_sources(self, entries):
        (self.root / "vendor" / "sources.json").write_text(
            json.dumps({"sources": entries}, indent=2) + "\n", encoding="utf-8")

    def build_all(self, entries):
        self.set_sources(entries)
        for e in entries:
            if e.get("managed_by") == "subtree":
                continue
            vendor.build(self.root, e["name"], fetch=self.fetch)
        vendor.fetch_licenses(self.root, fetch=self.fetch)

    def check(self):
        return vendor.check(self.root, fetch=self.fetch)


class SourcesJsonTest(unittest.TestCase):
    def test_real_sources_validate(self):
        entries = vendor.load_sources(ROOT)
        self.assertEqual(vendor.validate_sources(entries), [])
        self.assertEqual(
            {e["name"] for e in entries},
            {"superpowers", "design-skills", "frontend-design", "web-design-guidelines"}
            | set(STRIPPED_KNOWLEDGE))

    def test_real_pins_match_the_plan(self):
        pins = {e["name"]: e for e in vendor.load_sources(ROOT)}
        self.assertEqual(pins["superpowers"]["commit"], "22f0b463e7a5e579904dd6211a520321035526f7")
        self.assertEqual(pins["superpowers"]["ref"], "v7.0.2")
        self.assertEqual(pins["superpowers"]["plugin_version"], "0.1.0")
        self.assertEqual(pins["superpowers"]["upstream_version"], "7.0.2")
        self.assertEqual(pins["design-skills"]["commit"], "7e25e66e87f64f9d3dea22198f9e29937e7c48f5")
        self.assertEqual(pins["frontend-design"]["commit"], "e4bb5d960d87126ff3b3a8af1a104b7db67560ba")
        self.assertEqual(pins["frontend-design"]["path"], "skills/frontend-design")
        self.assertEqual(pins["web-design-guidelines"]["commit"], "1a2c79d4e6c4a2283dc2c614b6538cbe1ed5ca62")
        self.assertEqual(pins["web-design-guidelines"]["path"], "skills/web-design-guidelines")
        self.assertIsNone(pins["web-design-guidelines"]["license_file"])
        self.assertEqual(pins["web-design-guidelines"]["license_note"], "README")

    def test_the_22_knowledge_entries(self):
        pins = {e["name"]: e for e in vendor.load_sources(ROOT)}
        self.assertEqual(len(STRIPPED_KNOWLEDGE), 22)
        for name, (slug, path) in STRIPPED_KNOWLEDGE.items():
            e = pins[name]
            self.assertEqual(vendor.source_slug(e), slug, name)
            self.assertEqual(e["path"], path, name)
            self.assertEqual(e["target"], f"plugins/anthology/knowledge/{name}", name)
            self.assertEqual(e["license"], "Apache-2.0" if slug.startswith("anthropics") else "MIT", name)
            self.assertEqual(e["license_file"], "LICENSE", name)
            self.assertEqual(e["transform"][0], {"name": "strip-connectors"}, name)
            expected = [{"name": "strip-connectors"}]
            if name == "code-review-checklist":
                expected.append({"name": "rename-skill", "old": "code-review", "new": name})
            self.assertEqual(e["transform"], expected, name)
        kwp = {e["commit"] for e in pins.values() if vendor.source_slug(e) == "anthropics-knowledge-work-plugins"}
        self.assertEqual(kwp, {"0e0fcacddcc892eb92a1b7873ff9d68159c47571"})
        ux = {e["commit"] for e in pins.values() if vendor.source_slug(e) == "uxkin-agent"}
        self.assertEqual(ux, {"07e8f5f61421c7268fddc1511878fb09945da854"})
        self.assertEqual({e["ref"] for e in pins.values() if vendor.source_slug(e) == "uxkin-agent"},
                         {"website-design"})

    def test_valid_entry_has_no_errors(self):
        self.assertEqual(vendor.validate_sources([entry("x")]), [])

    def test_checksum_instead_of_commit_is_accepted(self):
        e = entry("x", checksum="c" * 64)
        del e["commit"]
        self.assertEqual(vendor.validate_sources([e]), [])

    def test_commit_must_be_40_hex(self):
        for bad in ("abc123", "g" * 40, "A" * 40, "a" * 39, ""):
            errors = vendor.validate_sources([entry("x", commit=bad)])
            self.assertTrue(any("commit" in m for m in errors), bad)

    def test_commit_or_checksum_required(self):
        e = entry("x")
        del e["commit"]
        self.assertTrue(vendor.validate_sources([e]))

    def test_license_restricted_to_mit_and_apache(self):
        for bad in ("GPL-3.0-only", "CC-BY-4.0", "", None, "NOASSERTION"):
            errors = vendor.validate_sources([entry("x", license=bad)])
            self.assertTrue(any("license" in m for m in errors), bad)
        self.assertEqual(vendor.validate_sources([entry("x", license="Apache-2.0")]), [])

    def test_required_fields(self):
        for field in ("name", "path", "ref", "license", "target", "transform"):
            e = entry("x")
            del e[field]
            self.assertTrue(vendor.validate_sources([e]), field)

    def test_duplicate_names_and_targets_rejected(self):
        self.assertTrue(vendor.validate_sources([entry("x"), entry("x", target="plugins/anthology/knowledge/y")]))
        self.assertTrue(vendor.validate_sources([entry("x"), entry("y", target="plugins/anthology/knowledge/x")]))

    def test_target_must_stay_inside_the_repo(self):
        for bad in ("/etc", "../x", "plugins/../../x", ""):
            self.assertTrue(vendor.validate_sources([entry("x", target=bad)]), bad)

    def test_target_cannot_name_the_root_or_a_critical_directory(self):
        # build/sync rmtree the target, so these would delete the checkout, .git or vendor/
        for bad in (".", "./", "plugins/.", "./plugins/x", "plugins/./x", "plugins//x", "plugins/x/",
                    ".git", "vendor", "scripts", "tests", "plugins", "plugins/", ".github",
                    "pluginsx/y", "other/plugins/x"):
            self.assertTrue(vendor.validate_sources([entry("x", target=bad)]), bad)

    def test_target_is_on_the_allowlist(self):
        # build/sync rmtree the target: only the vendored plugin and one folder per knowledge
        # entry may be replaced, never first-party plugin directories or their parents.
        for bad in ("plugins/anthology", "plugins/anthology/", "plugins/anthology/knowledge",
                    "plugins/anthology/knowledge/a/b", "plugins/anthology/skills/x",
                    "plugins/anthology/.claude-plugin", "plugins/anthology-guardrails",
                    "plugins/anthology-superpowers/skills", "plugins/x", "plugins/anthology/knowledgex/a"):
            self.assertTrue(vendor.validate_sources([entry("x", target=bad)]), bad)
        for good in ("plugins/anthology-superpowers", "plugins/anthology/knowledge/x",
                     "plugins/anthology/knowledge/code-review-checklist"):
            self.assertEqual(vendor.validate_sources([entry("x", target=good)]), [], good)

    def test_subtree_target_is_still_a_clean_relative_path(self):
        e = {"name": "rtk", "managed_by": "subtree", "license": "Apache-2.0"}
        for bad in (".", "../x", "/x", "a/./b"):
            self.assertTrue(vendor.validate_sources([dict(e, target=bad)]), bad)
        self.assertEqual(vendor.validate_sources([dict(e, target="rtk")]), [])

    def test_path_must_stay_inside_the_upstream_repo(self):
        for bad in ("/etc", "../x", "a/../../x"):
            self.assertTrue(vendor.validate_sources([entry("x", path=bad)]), bad)

    def test_repo_must_be_https(self):
        for bad in ("git://github.com/a/b", "file:///tmp/x", "http://github.com/a/b", "-oProxy=x"):
            self.assertTrue(vendor.validate_sources([entry("x", repo=bad)]), bad)

    def test_unknown_transform_rejected(self):
        self.assertTrue(vendor.validate_sources([entry("x", transform=["nonsense"])]))
        self.assertTrue(vendor.validate_sources([entry("x", transform=[{"name": "nonsense"}])]))

    def test_rename_plugin_needs_plugin_version(self):
        t = [{"name": "rename-plugin", "old": "a", "new": "b"}]
        self.assertTrue(vendor.validate_sources([entry("x", transform=t)]))
        self.assertEqual(vendor.validate_sources([entry("x", transform=t, plugin_version="0.1.0")]), [])

    def test_strip_connectors_and_rename_skill_are_known_transforms(self):
        self.assertEqual(vendor.validate_sources([entry("x", transform=["strip-connectors"])]), [])
        t = [{"name": "rename-skill", "old": "a", "new": "x"}]
        self.assertEqual(vendor.validate_sources([entry("x", transform=t)]), [])
        for bad in ({"name": "rename-skill", "old": "a"}, {"name": "rename-skill", "new": "b"},
                    {"name": "rename-skill", "old": "a", "new": "../b"},
                    {"name": "rename-skill", "old": "", "new": "b"}):
            self.assertTrue(vendor.validate_sources([entry("x", transform=[bad])]), bad)

    def test_missing_license_file_requires_a_note(self):
        self.assertTrue(vendor.validate_sources([entry("x", license_file=None)]))
        self.assertEqual(
            vendor.validate_sources([entry("x", license_file=None, license_note="README")]), [])

    def test_same_repo_entries_must_agree_on_the_license_file(self):
        a = entry("a", repo="https://github.com/o/r", license_file="LICENSE")
        b = entry("b", repo="https://github.com/o/r", license_file="OTHER")
        self.assertTrue(vendor.validate_sources([a, b]))

    def test_subtree_entries_are_not_checked_for_pins(self):
        e = {"name": "rtk", "managed_by": "subtree", "license": "Apache-2.0", "target": "rtk"}
        self.assertEqual(vendor.validate_sources([e]), [])

    def test_slug(self):
        self.assertEqual(vendor.source_slug(entry("x", repo="https://github.com/obra/superpowers")),
                         "obra-superpowers")
        self.assertEqual(vendor.source_slug(entry("x", repo="https://github.com/o/r.git/")), "o-r")


class RenamePluginTest(FixtureCase):
    OLD, NEW = "superpowers", "anthology-superpowers"

    def make_tree(self):
        tree = self.tmp / "tree"
        write(tree, ".claude-plugin/plugin.json", json.dumps({
            "name": "superpowers", "description": "d", "version": "7.0.2",
            "homepage": "https://github.com/obra/superpowers", "license": "MIT",
            "author": {"name": "Jesse"}, "keywords": ["a", "b"]}, indent=2) + "\n")
        write(tree, "skills/a/SKILL.md", "\n".join([
            "Use superpowers:brainstorming first.",
            "Already anthology-superpowers:writing-plans stays.",
            "See https://github.com/obra/superpowers and github.com/obra/superpowers:main.",
            "Superpowers: prose is untouched, as is using-superpowers: here.",
            "Drop the `superpowers:` prefix (`superpowers:test-driven-development`).",
            '"superpowers:code-reviewer", superpowers:skill-name, superpowers:',
            "path/superpowers:x and Xsuperpowers:y and _superpowers:z stay.",
            "",
        ]))
        write(tree, "hooks/session-start", "echo 'superpowers:using-superpowers'\n", exec_bit=True)
        write(tree, "assets/blob.bin", b"\x00superpowers:brainstorming\x00")
        return tree

    def test_rewrites_only_skill_reference_tokens(self):
        tree = self.make_tree()
        stats = vendor.rename_plugin(tree, self.OLD, self.NEW, plugin_version="0.1.0")
        text = (tree / "skills/a/SKILL.md").read_text(encoding="utf-8")
        self.assertEqual(text.splitlines(), [
            "Use anthology-superpowers:brainstorming first.",
            "Already anthology-superpowers:writing-plans stays.",
            "See https://github.com/obra/superpowers and github.com/obra/superpowers:main.",
            "Superpowers: prose is untouched, as is using-superpowers: here.",
            "Drop the `superpowers:` prefix (`anthology-superpowers:test-driven-development`).",
            '"anthology-superpowers:code-reviewer", anthology-superpowers:skill-name, superpowers:',
            "path/superpowers:x and Xsuperpowers:y and _superpowers:z stay.",
        ])
        self.assertEqual((tree / "hooks/session-start").read_text(),
                         "echo 'anthology-superpowers:using-superpowers'\n")
        self.assertEqual((tree / "assets/blob.bin").read_bytes(), b"\x00superpowers:brainstorming\x00")
        self.assertEqual(stats["references"], 5)
        self.assertEqual(stats["files"], 2)

    def test_manifest_name_and_version(self):
        tree = self.make_tree()
        vendor.rename_plugin(tree, self.OLD, self.NEW, plugin_version="0.1.0")
        raw = (tree / ".claude-plugin/plugin.json").read_text(encoding="utf-8")
        manifest = json.loads(raw)
        self.assertEqual(manifest["name"], "anthology-superpowers")
        self.assertEqual(manifest["version"], "0.1.0")
        self.assertEqual(manifest["license"], "MIT")
        self.assertEqual(manifest["homepage"], "https://github.com/obra/superpowers")
        self.assertEqual(list(manifest), ["name", "description", "version", "homepage", "license",
                                          "author", "keywords"])
        self.assertTrue(raw.endswith("}\n"))

    def test_preserves_file_modes(self):
        tree = self.make_tree()
        vendor.rename_plugin(tree, self.OLD, self.NEW, plugin_version="0.1.0")
        self.assertTrue(os.access(tree / "hooks/session-start", os.X_OK))
        self.assertFalse(os.access(tree / "skills/a/SKILL.md", os.X_OK))

    def test_refuses_a_manifest_with_another_name(self):
        tree = self.make_tree()
        vendor.rename_plugin(tree, self.OLD, self.NEW, plugin_version="0.1.0")
        with self.assertRaises(vendor.VendorError):
            vendor.rename_plugin(tree, self.OLD, self.NEW, plugin_version="0.1.0")

    def test_missing_manifest_is_an_error(self):
        tree = self.tmp / "empty"
        tree.mkdir()
        with self.assertRaises(vendor.VendorError):
            vendor.rename_plugin(tree, self.OLD, self.NEW, plugin_version="0.1.0")


CONNECTOR_DOC = """\
---
name: demo
description: Demo.
---

# /demo

> If you see unfamiliar placeholders or need to check which tools are connected, see [CONNECTORS.md](../../CONNECTORS.md).

Intro paragraph.

## Steps

### 1. Gather

If **~~project tracker** is connected:
- Pull the tickets
- Link them

If no tracker is connected:
- Ask the user

**If ~~SEO tools are connected:**
- Pull keywords

**If ~~product analytics are connected:**
- Cross-reference traffic

Keep this paragraph.

- **~~knowledge base** (if connected): Search the wiki
- **Pasted text**: Notes

If ~~product analytics is connected, reference benchmarks.

Last sentence of the step.

```
## If Connectors Available
kept inside a fence
```

## If Connectors Available

If **~~knowledge base** is connected:
- Search prior docs

### Nested heading inside the section

Still connector text.

## Tool Integration

### If ~~email marketing is connected (e.g., Klaviyo)
- Set up the flow

### If no tools are connected
- Paste the copy

## Tips

1. Keep this tip.
"""

CONNECTOR_DOC_STRIPPED = """\
---
name: demo
description: Demo.
---

# /demo

Intro paragraph.

## Steps

### 1. Gather

If no tracker is connected:
- Ask the user

Keep this paragraph.

- **Pasted text**: Notes

Last sentence of the step.

```
## If Connectors Available
kept inside a fence
```

## Tips

1. Keep this tip.
"""


class StripConnectorsTest(FixtureCase):
    def run_strip(self, files):
        tree = self.tmp / "tree"
        for rel, text in files.items():
            write(tree, rel, text)
        stats = vendor.strip_connectors(tree)
        return tree, stats

    def test_removes_connector_sections_blocks_and_references_only(self):
        tree, stats = self.run_strip({"SKILL.md": CONNECTOR_DOC})
        self.assertEqual((tree / "SKILL.md").read_text(encoding="utf-8"),
                         CONNECTOR_DOC_STRIPPED)
        self.assertEqual(stats["files"], 1)

    def test_headings_and_fences_are_respected(self):
        # a heading-looking line inside a code fence neither starts nor ends a section
        doc = ("# T\n\n## If Connectors Available\n\ntext\n\n```\n## Not a heading\n```\n\nmore\n\n"
               "## Tips\n\nkept\n")
        tree, _ = self.run_strip({"SKILL.md": doc})
        self.assertEqual((tree / "SKILL.md").read_text(encoding="utf-8"), "# T\n\n## Tips\n\nkept\n")

    def test_fenced_connector_text_is_not_stripped_but_still_fails_the_guard(self):
        with self.assertRaises(vendor.VendorError):
            self.run_strip({"SKILL.md": "# T\n\n```\nIf ~~x is connected:\n- y\n```\n"})

    def test_section_at_the_end_of_a_file_leaves_one_final_newline(self):
        tree, _ = self.run_strip({"SKILL.md": "# T\n\ntext\n\n## Connectors\n\nIf ~~a is connected:\n- b\n"})
        self.assertEqual((tree / "SKILL.md").read_text(encoding="utf-8"), "# T\n\ntext\n")

    def test_connect_setup_heading_with_a_url(self):
        doc = ("# T\n\n## Connect UXKIN (optional)\n\nadd `https://uxkin.com/mcp`\n\n"
               "## 1. Start\n\nkept\n")
        tree, _ = self.run_strip({"SKILL.md": doc})
        self.assertEqual((tree / "SKILL.md").read_text(encoding="utf-8"), "# T\n\n## 1. Start\n\nkept\n")

    def test_connected_tools_heading_is_a_connector_section(self):
        doc = "# T\n\n### 3. Pull Context from Connected Tools\n\nIf **~~a** is connected:\n- b\n\n### 4. Next\n\nkept\n"
        tree, _ = self.run_strip({"SKILL.md": doc})
        self.assertEqual((tree / "SKILL.md").read_text(encoding="utf-8"), "# T\n\n### 4. Next\n\nkept\n")

    def test_idempotent(self):
        tree, _ = self.run_strip({"SKILL.md": CONNECTOR_DOC, "references/a.md": CONNECTOR_DOC})
        once = {p.name + str(p.parent): p.read_bytes() for p in tree.rglob("*.md")}
        stats = vendor.strip_connectors(tree)
        self.assertEqual(once, {p.name + str(p.parent): p.read_bytes() for p in tree.rglob("*.md")})
        self.assertEqual(stats["files"], 0)

    def test_files_without_connectors_are_byte_identical(self):
        raw = b"# Plain\r\n\r\nNo connectors here.\r\n"
        tree, stats = self.run_strip({"SKILL.md": raw, "scripts/run.sh": "#!/bin/sh\n~~ not markdown\n"})
        self.assertEqual((tree / "SKILL.md").read_bytes(), raw)
        self.assertEqual((tree / "scripts/run.sh").read_text(), "#!/bin/sh\n~~ not markdown\n")
        self.assertEqual(stats["files"], 0)

    def test_deletes_a_connectors_file_in_the_tree(self):
        tree, stats = self.run_strip({"SKILL.md": "# T\n", "CONNECTORS.md": "# Connectors\n",
                                      "references/CONNECTORS.md": "x\n"})
        self.assertFalse((tree / "CONNECTORS.md").exists())
        self.assertFalse((tree / "references/CONNECTORS.md").exists())
        self.assertEqual(stats["deleted"], 2)

    def test_unhandled_placeholder_fails_loudly_instead_of_guessing(self):
        for doc in ("Open the ~~project tracker and look.\n", "See [x](CONNECTORS.md) for details.\n"):
            with self.assertRaises(vendor.VendorError) as ctx:
                self.run_strip({"SKILL.md": doc})
            self.assertIn("SKILL.md:1", str(ctx.exception))

    def test_preserves_the_executable_bit(self):
        tree = self.tmp / "tree"
        write(tree, "x.md", "# T\n\n## Connectors\n\nIf ~~a is connected:\n- b\n", exec_bit=True)
        vendor.strip_connectors(tree)
        self.assertTrue(os.access(tree / "x.md", os.X_OK))

    def test_pipeline_applies_it_from_sources_json(self):
        e = self.add_upstream("conn", {"LICENSE": MIT_TEXT, "SKILL.md": CONNECTOR_DOC},
            transform=["strip-connectors"])
        self.build_all([e])
        self.assertEqual(self.check(), [])
        shipped = (self.root / e["target"] / "SKILL.md").read_text(encoding="utf-8")
        self.assertNotIn("~~", shipped)
        self.assertNotIn("CONNECTORS.md", shipped)


class RenameSkillTest(FixtureCase):
    def make(self, text):
        tree = self.tmp / "tree"
        write(tree, "SKILL.md", text)
        return tree

    def test_renames_the_frontmatter_name_only(self):
        tree = self.make("---\nname: code-review\ndescription: Review code.\n---\n\n# /code-review\n")
        vendor.rename_skill(tree, "code-review", "code-review-checklist")
        self.assertEqual((tree / "SKILL.md").read_text(encoding="utf-8"),
                         "---\nname: code-review-checklist\ndescription: Review code.\n---\n\n# /code-review\n")

    def test_a_different_name_is_an_error(self):
        tree = self.make("---\nname: other\n---\nbody\n")
        with self.assertRaises(vendor.VendorError):
            vendor.rename_skill(tree, "code-review", "x")

    def test_missing_skill_file_or_frontmatter_is_an_error(self):
        with self.assertRaises(vendor.VendorError):
            vendor.rename_skill(self.tmp / "nowhere", "a", "b")
        with self.assertRaises(vendor.VendorError):
            vendor.rename_skill(self.make("no frontmatter\n"), "a", "b")

    def test_is_not_confused_by_a_name_line_in_the_body(self):
        tree = self.make("---\nname: a\n---\n\nname: a\n")
        vendor.rename_skill(tree, "a", "b")
        self.assertEqual((tree / "SKILL.md").read_text(encoding="utf-8"), "---\nname: b\n---\n\nname: a\n")


class PipelineTest(FixtureCase):
    def setUp(self):
        super().setUp()
        self.plain = self.add_upstream("plain", {
            "LICENSE": MIT_TEXT,
            "skills/a/SKILL.md": "---\nname: a\n---\nbody\n",
            "scripts/run.sh": "#!/bin/sh\necho hi\n",
            "README.md": "readme\n",
        })
        self.sub = self.add_upstream("sub", {
            "LICENSE.txt": "Apache License\n",
            "skills/pick/SKILL.md": "pick\n",
            "skills/pick/LICENSE.txt": "Apache License\n",
            "skills/other/SKILL.md": "other\n",
        }, path="skills/pick", license="Apache-2.0", license_file="skills/pick/LICENSE.txt")
        self.renamed = self.add_upstream("ren", {
            ".claude-plugin/plugin.json": json.dumps({"name": "old", "version": "9.9.9"}, indent=2) + "\n",
            "skills/x/SKILL.md": "Use old:thing\n",
            "LICENSE": MIT_TEXT,
        }, transform=[{"name": "rename-plugin", "old": "old", "new": "new"}],
            plugin_version="0.1.0", upstream_version="9.9.9")
        self.entries = [self.plain, self.sub, self.renamed]

    def test_build_copies_whole_repo_without_git_dir(self):
        self.build_all(self.entries)
        target = self.root / "plugins/anthology/knowledge/plain"
        self.assertEqual(sorted(str(p.relative_to(target)) for p in target.rglob("*") if p.is_file()),
                         ["LICENSE", "README.md", "scripts/run.sh", "skills/a/SKILL.md"])
        self.assertFalse((target / ".git").exists())

    def test_build_preserves_exec_bit(self):
        self.build_all(self.entries)
        self.assertTrue(os.access(self.root / "plugins/anthology/knowledge/plain/scripts/run.sh", os.X_OK))
        self.assertFalse(os.access(self.root / "plugins/anthology/knowledge/plain/README.md", os.X_OK))

    def test_build_uses_only_the_pinned_subpath(self):
        self.build_all(self.entries)
        target = self.root / "plugins/anthology/knowledge/sub"
        self.assertEqual(sorted(str(p.relative_to(target)) for p in target.rglob("*") if p.is_file()),
                         ["LICENSE.txt", "SKILL.md"])

    def test_build_applies_rename(self):
        self.build_all(self.entries)
        target = self.root / "plugins/anthology/knowledge/ren"
        self.assertEqual((target / "skills/x/SKILL.md").read_text(), "Use new:thing\n")
        manifest = json.loads((target / ".claude-plugin/plugin.json").read_text())
        self.assertEqual((manifest["name"], manifest["version"]), ("new", "0.1.0"))

    def test_build_replaces_existing_target(self):
        self.build_all(self.entries)
        write(self.root, "plugins/anthology/knowledge/plain/stale.txt", "stale")
        vendor.build(self.root, "plain", fetch=self.fetch)
        self.assertFalse((self.root / "plugins/anthology/knowledge/plain/stale.txt").exists())

    def test_build_out_dir_leaves_the_repo_alone(self):
        self.set_sources(self.entries)
        out = self.tmp / "out"
        vendor.build(self.root, "plain", out=out, fetch=self.fetch)
        self.assertTrue((out / "plugins/anthology/knowledge/plain/README.md").is_file())
        self.assertFalse((self.root / "plugins").exists())

    def test_unknown_name(self):
        self.set_sources(self.entries)
        with self.assertRaises(vendor.VendorError):
            vendor.build(self.root, "nope", fetch=self.fetch)

    def test_check_passes_when_clean(self):
        self.build_all(self.entries)
        self.assertEqual(self.check(), [])

    def test_check_flags_modified_file(self):
        self.build_all(self.entries)
        write(self.root, "plugins/anthology/knowledge/plain/README.md", "edited\n")
        problems = self.check()
        self.assertTrue(any("README.md" in p and "differs" in p for p in problems), problems)

    def test_check_flags_extra_and_missing_files(self):
        self.build_all(self.entries)
        write(self.root, "plugins/anthology/knowledge/plain/extra.md", "x")
        (self.root / "plugins/anthology/knowledge/plain/README.md").unlink()
        problems = "\n".join(self.check())
        self.assertIn("extra.md", problems)
        self.assertIn("README.md", problems)

    def test_check_flags_lost_exec_bit(self):
        self.build_all(self.entries)
        (self.root / "plugins/anthology/knowledge/plain/scripts/run.sh").chmod(0o644)
        self.assertTrue(any("run.sh" in p and "mode" in p for p in self.check()))

    def test_check_flags_missing_target(self):
        self.build_all(self.entries)
        shutil.rmtree(self.root / "plugins/anthology/knowledge/plain")
        self.assertTrue(any("plain" in p for p in self.check()))

    def test_directory_symlinks_fail_loudly_in_pristine_trees(self):
        os.symlink("skills", self.pristine["plain"] / "linkdir")
        self.set_sources([self.plain])
        with self.assertRaises(vendor.VendorError) as ctx:
            vendor.build(self.root, "plain", fetch=self.fetch)
        self.assertIn("linkdir", str(ctx.exception))
        self.assertIn("symlinks to directories", str(ctx.exception))
        self.assertFalse((self.root / "plugins/anthology/knowledge/plain").exists())
        self.assertTrue(any("linkdir" in p for p in self.check()))

    def test_check_flags_a_directory_symlink_in_the_committed_tree(self):
        self.build_all(self.entries)
        os.symlink("skills", self.root / "plugins/anthology/knowledge/plain/linkdir")
        problems = self.check()
        self.assertTrue(any("linkdir" in p and "symlinks to directories" in p for p in problems), problems)

    def test_file_symlinks_are_copied_and_compared(self):
        os.symlink("README.md", self.pristine["plain"] / "link.md")
        self.build_all(self.entries)
        self.assertEqual(os.readlink(self.root / "plugins/anthology/knowledge/plain/link.md"), "README.md")
        self.assertEqual(self.check(), [])
        (self.root / "plugins/anthology/knowledge/plain/link.md").unlink()
        os.symlink("LICENSE", self.root / "plugins/anthology/knowledge/plain/link.md")
        self.assertTrue(any("link.md" in p for p in self.check()))

    def test_check_ignores_ds_store_but_not_other_extras(self):
        self.build_all(self.entries)
        write(self.root, "plugins/anthology/knowledge/plain/.DS_Store", "x")
        self.assertEqual(self.check(), [])

    def test_check_flags_upstream_change_at_same_pin(self):
        self.build_all(self.entries)
        write(self.pristine["plain"], "README.md", "upstream moved\n")
        self.assertTrue(self.check())

    def test_check_does_not_touch_the_repo(self):
        self.build_all(self.entries)
        before = sorted(str(p) for p in self.root.rglob("*"))
        self.check()
        self.assertEqual(before, sorted(str(p) for p in self.root.rglob("*")))

    def test_check_validates_sources_first(self):
        bad = dict(self.plain, license="GPL-3.0-only")
        self.set_sources([bad])
        self.assertTrue(any("license" in p for p in self.check()))

    def test_check_skips_subtree_entries(self):
        calls = []

        def fetch(e):
            calls.append(e["name"])
            return self.fetch(e)

        subtree = {"name": "rtk", "managed_by": "subtree", "license": "Apache-2.0", "target": "rtk"}
        self.build_all(self.entries)
        self.set_sources(self.entries + [subtree])
        self.assertEqual(vendor.check(self.root, fetch=fetch), [])
        self.assertNotIn("rtk", calls)
        vendor.build(self.root, "plain", fetch=fetch)  # others still build
        with self.assertRaises(vendor.VendorError):
            vendor.build(self.root, "rtk", fetch=fetch)


class LicenseTest(FixtureCase):
    def setUp(self):
        super().setUp()
        self.a = self.add_upstream("a", {"LICENSE": MIT_TEXT, "x.md": "x\n"})
        self.b = self.add_upstream("b", {"README.md": "License: MIT\n"},
                                   license_file=None, license_note="README")

    def test_fetch_licenses_copies_unchanged_one_per_repo(self):
        a2 = entry("a2", repo=self.a["repo"], target="plugins/anthology/knowledge/a2")
        self.pristine["a2"] = self.pristine["a"]
        self.build_all([self.a, a2])
        licenses = sorted(p.name for p in (self.root / "vendor/licenses").iterdir())
        self.assertEqual(licenses, ["acme-a.txt"])
        self.assertEqual((self.root / "vendor/licenses/acme-a.txt").read_text(), MIT_TEXT)

    def test_fetch_licenses_is_byte_exact(self):
        raw = b"MIT License\r\n\r\nCopyright \xc2\xa9 x\r\n"
        write(self.pristine["a"], "LICENSE", raw)
        self.set_sources([self.a])
        vendor.fetch_licenses(self.root, fetch=self.fetch)
        self.assertEqual((self.root / "vendor/licenses/acme-a.txt").read_bytes(), raw)

    def test_check_flags_license_drift(self):
        self.build_all([self.a])
        (self.root / "vendor/licenses/acme-a.txt").write_text("tampered\n")
        self.assertTrue(any("acme-a.txt" in p for p in self.check()))

    def test_check_flags_missing_license(self):
        self.build_all([self.a])
        (self.root / "vendor/licenses/acme-a.txt").unlink()
        self.assertTrue(any("acme-a.txt" in p for p in self.check()))

    def test_check_flags_empty_license(self):
        write(self.pristine["a"], "LICENSE", "")
        self.set_sources([self.a])
        with self.assertRaises(vendor.VendorError):
            vendor.fetch_licenses(self.root, fetch=self.fetch)
        vendor.build(self.root, "a", fetch=self.fetch)
        write(self.root, "vendor/licenses/acme-a.txt", "")
        self.assertTrue(any("acme-a.txt" in p for p in self.check()))

    def test_missing_upstream_license_file_is_an_error(self):
        (self.pristine["a"] / "LICENSE").unlink()
        self.set_sources([self.a])
        with self.assertRaises(vendor.VendorError):
            vendor.fetch_licenses(self.root, fetch=self.fetch)

    def test_fixed_license_is_not_overwritten_or_compared_to_upstream(self):
        self.set_sources([self.b])
        vendor.build(self.root, "b", fetch=self.fetch)
        vendor.fetch_licenses(self.root, fetch=self.fetch)  # must not invent a file
        self.assertFalse((self.root / "vendor/licenses/acme-b.txt").exists())
        self.assertTrue(any("acme-b.txt" in p for p in self.check()))  # missing fixed file
        write(self.root, "vendor/licenses/acme-b.txt", FIXED_MIT)
        self.assertEqual(self.check(), [])
        write(self.root, "vendor/licenses/acme-b.txt", "MIT License\n\nCopyright (c) Vercel\n")
        self.assertTrue(any("README" in p for p in self.check()))  # note line is required


class SyncAndDiffTest(FixtureCase):
    def setUp(self):
        super().setUp()
        self.e = self.add_upstream("p", {"LICENSE": MIT_TEXT, "skills/a/SKILL.md": "v1\n"})
        self.build_all([self.e])

    def resolve(self, e, ref):
        self.assertEqual(ref, "v2")
        return SHA_B

    def test_sync_repins_rebuilds_and_never_commits(self):
        write(self.pristine["p"], "skills/a/SKILL.md", "v2\n")
        report = vendor.sync(self.root, "p", "v2", fetch=self.fetch, resolve=self.resolve)
        pinned = vendor.load_sources(self.root)[0]
        self.assertEqual((pinned["ref"], pinned["commit"]), ("v2", SHA_B))
        self.assertEqual((self.root / "plugins/anthology/knowledge/p/skills/a/SKILL.md").read_text(), "v2\n")
        self.assertEqual(report["rejects"], [])
        self.assertEqual(self.check(), [])
        self.assertFalse((self.root / ".git").exists())

    def test_sync_keeps_other_fields_and_order(self):
        before = json.loads((self.root / "vendor/sources.json").read_text())["sources"][0]
        vendor.sync(self.root, "p", "v2", fetch=self.fetch, resolve=self.resolve)
        after = vendor.load_sources(self.root)[0]
        self.assertEqual(list(before), list(after))
        for k in before:
            if k not in ("ref", "commit"):
                self.assertEqual(before[k], after[k], k)

    def test_sync_accepts_a_full_sha_without_resolving(self):
        def boom(e, ref):
            raise AssertionError("must not resolve a 40-hex ref")
        vendor.sync(self.root, "p", SHA_B, fetch=self.fetch, resolve=boom)
        pinned = vendor.load_sources(self.root)[0]
        self.assertEqual((pinned["ref"], pinned["commit"]), (SHA_B, SHA_B))

    def test_sync_refreshes_the_license(self):
        write(self.pristine["p"], "LICENSE", "MIT License\n\nCopyright (c) New\n")
        vendor.sync(self.root, "p", "v2", fetch=self.fetch, resolve=self.resolve)
        self.assertIn("New", (self.root / "vendor/licenses/acme-p.txt").read_text())

    def test_diff_reports_nothing_for_a_pristine_copy(self):
        self.assertEqual(vendor.diff_entry(self.root, "p", fetch=self.fetch), "")

    def test_diff_shows_shipped_changes_as_a_patch(self):
        write(self.root, "plugins/anthology/knowledge/p/skills/a/SKILL.md", "v1\nlocal edit\n")
        write(self.root, "plugins/anthology/knowledge/p/new.md", "new\n")
        text = vendor.diff_entry(self.root, "p", fetch=self.fetch)
        self.assertIn("--- a/skills/a/SKILL.md", text)
        self.assertIn("+++ b/skills/a/SKILL.md", text)
        self.assertIn("+local edit", text)
        self.assertIn("--- /dev/null", text)
        self.assertIn("+++ b/new.md", text)

    @unittest.skipUnless(shutil.which("git"), "git is not installed")
    def test_diff_output_round_trips_through_a_patch(self):
        write(self.root, "plugins/anthology/knowledge/p/skills/a/SKILL.md", "v1\nlocal edit\n")
        write(self.root, "plugins/anthology/knowledge/p/new.md", "new\n")
        (self.root / "plugins/anthology/knowledge/p/LICENSE").unlink()
        patch = self.root / "vendor/patches/p.patch"
        patch.parent.mkdir()
        patch.write_text(vendor.diff_entry(self.root, "p", fetch=self.fetch), encoding="utf-8")
        shipped = {p: p.read_bytes() for p in (self.root / "plugins/anthology/knowledge/p").rglob("*") if p.is_file()}
        sources = json.loads((self.root / "vendor/sources.json").read_text())
        sources["sources"][0]["transform"] = [{"patch": "vendor/patches/p.patch"}]
        (self.root / "vendor/sources.json").write_text(json.dumps(sources, indent=2) + "\n")
        vendor.build(self.root, "p", fetch=self.fetch)
        self.assertEqual(shipped, {p: p.read_bytes() for p in (self.root / "plugins/anthology/knowledge/p").rglob("*")
                                   if p.is_file()})
        self.assertEqual(self.check(), [])

    @unittest.skipUnless(shutil.which("git"), "git is not installed")
    def test_sync_reports_patch_rejects(self):
        write(self.root, "plugins/anthology/knowledge/p/skills/a/SKILL.md", "v1\nlocal edit\n")
        patch = self.root / "vendor/patches/p.patch"
        patch.parent.mkdir()
        patch.write_text(vendor.diff_entry(self.root, "p", fetch=self.fetch), encoding="utf-8")
        sources = json.loads((self.root / "vendor/sources.json").read_text())
        sources["sources"][0]["transform"] = [{"patch": "vendor/patches/p.patch"}]
        (self.root / "vendor/sources.json").write_text(json.dumps(sources, indent=2) + "\n")
        write(self.pristine["p"], "skills/a/SKILL.md", "completely different\n")
        report = vendor.sync(self.root, "p", "v2", fetch=self.fetch, resolve=self.resolve)
        self.assertEqual(len(report["rejects"]), 1)
        self.assertIn("SKILL.md", report["rejects"][0])


class CompatibilityTest(unittest.TestCase):
    def test_vendor_py_parses_as_python_39(self):
        src = (ROOT / "scripts" / "vendor.py").read_text(encoding="utf-8")
        ast.parse(src, feature_version=(3, 9))
        self.assertIn("from __future__ import annotations", src)

    def test_cli_is_runnable_and_lists_commands(self):
        out = subprocess.run([sys.executable, str(ROOT / "scripts" / "vendor.py"), "--help"],
                             capture_output=True, text=True, check=True).stdout
        for cmd in ("fetch", "build", "check", "sync", "diff"):
            self.assertRegex(out, rf"\b{cmd}\b")

    def test_cache_dir_is_git_ignored(self):
        ignore = (ROOT / ".gitignore").read_text(encoding="utf-8").splitlines()
        self.assertIn(".vendor-cache/", ignore)
        self.assertEqual(vendor.CACHE_DIRNAME, ".vendor-cache")


if __name__ == "__main__":
    unittest.main()
