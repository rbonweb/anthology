"""Tests for scripts/vendor.py.

The unit tests run in Docker without git or network, so they exercise the
transforms and the build/check/sync/diff pipeline on local fixture trees. Fetching
is abstracted behind a `fetch(entry) -> Path` function that returns a pristine repo
checkout; the tests stub it. Real fetches run on the host and in the CI
`vendor-check` job.
"""
import ast
import hashlib
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
            {"superpowers", "design-skills", "frontend-design", "web-design-guidelines",
             "ui-taste", "caveman", "design-taste-frontend"}
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
                    "plugins/anthology/knowledge/.git", "plugins/anthology/knowledge/Upper",
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


def sha(data):
    return hashlib.sha256(data).hexdigest()


def checksums_bytes(files):
    return "".join(f"{sha(data)}  {rel}\n" for rel, data in sorted(files.items())).encode()


URL_FILES = {
    "SKILL.md": b"---\nname: ui\ndescription: d\n---\nbody\n",
    "LICENSE": b"Apache License\nVersion 2.0\n",
    "ref/a.md": b"a\n",
    "scripts/x.mjs": b"export {};\n",
}


def url_base(name):
    return f"https://example.test/.well-known/skills/{name}/"


def url_entry(name, files=URL_FILES, **kw):
    e = {
        "name": name,
        "url": url_base(name),
        "path": "",
        "ref": "v1",
        "checksum": sha(checksums_bytes(files)),
        "license": "Apache-2.0",
        "license_file": "LICENSE",
        "target": f"plugins/anthology/knowledge/{name}",
        "transform": [],
    }
    e.update(kw)
    return e


def write_snapshot(root, name, files=URL_FILES):
    base = Path(root) / "vendor" / "snapshots" / name
    for rel, data in files.items():
        write(base, rel, data)
    write(base, "CHECKSUMS.sha256", checksums_bytes(files))
    return base


class FakeWeb:
    """A getter serving one skill (files, CHECKSUMS.sha256, index.json) from memory."""

    def __init__(self, name, files=URL_FILES, sums=None, index_files=None):
        self.base = url_base(name)
        self.index_url = self.base.rsplit(name + "/", 1)[0] + "index.json"
        self.pages = {self.base + rel: data for rel, data in files.items()}
        self.pages[self.base + "CHECKSUMS.sha256"] = checksums_bytes(files) if sums is None else sums
        listed = index_files if index_files is not None else sorted(files) + ["CHECKSUMS.sha256"]
        self.pages[self.index_url] = json.dumps({"skills": [
            {"name": "other", "files": ["x"]}, {"name": name, "files": listed}]}).encode()
        self.calls = []

    def __call__(self, url):
        self.calls.append(url)
        if url not in self.pages:
            raise vendor.VendorError(f"HTTP 404: {url}")
        return self.pages[url]


class UrlSourceValidationTest(unittest.TestCase):
    def test_valid_url_entry_has_no_errors(self):
        self.assertEqual(vendor.validate_sources([url_entry("ui")]), [])

    def test_url_must_be_https_and_end_with_a_slash(self):
        for bad in ("http://example.test/s/ui/", "file:///tmp/ui/", "ftp://example.test/ui/",
                    "https://", "https:///ui/", "https://example.test/ui", "//example.test/ui/",
                    "https://example.test/u i/", "https://user@example.test/ui/", ""):
            self.assertTrue(vendor.validate_sources([url_entry("ui", url=bad)]), bad)

    def test_a_url_entry_needs_a_checksum_and_no_repo_or_commit(self):
        e = url_entry("ui")
        del e["checksum"]
        self.assertTrue(vendor.validate_sources([e]))
        for bad in ("abc", "C" * 64, "c" * 63, ""):
            self.assertTrue(vendor.validate_sources([url_entry("ui", checksum=bad)]), bad)
        self.assertTrue(vendor.validate_sources([url_entry("ui", repo="https://github.com/a/b")]))
        self.assertTrue(vendor.validate_sources([url_entry("ui", commit=SHA_A)]))

    def test_a_url_entry_has_an_empty_path(self):
        self.assertTrue(vendor.validate_sources([url_entry("ui", path="skills/ui")]))

    def test_a_url_entry_needs_a_license_file_inside_the_snapshot(self):
        e = url_entry("ui")
        del e["license_file"]
        self.assertTrue(vendor.validate_sources([e]))
        self.assertTrue(vendor.validate_sources([url_entry("ui", license_file=None)]))
        for bad in ("/etc/passwd", "../LICENSE", "a/../../x"):
            self.assertTrue(vendor.validate_sources([url_entry("ui", license_file=bad)]), bad)

    def test_the_license_slug_of_a_url_entry_is_its_name(self):
        self.assertEqual(vendor.source_slug(url_entry("ui")), "ui")


class UrlSourceTest(FixtureCase):
    def setUp(self):
        super().setUp()
        self.e = url_entry("ui")
        self.set_sources([self.e])
        self.snap = write_snapshot(self.root, "ui")

    def fetch_offline(self):
        return vendor.snapshot_fetcher(self.root)(self.e)

    def test_offline_fetch_returns_the_verified_snapshot(self):
        self.assertEqual(self.fetch_offline(), self.snap)

    def test_the_default_fetcher_routes_url_entries_to_the_snapshot(self):
        self.assertEqual(vendor._fetcher(self.root, None)(self.e), self.snap)

    def test_a_tampered_file_is_rejected(self):
        write(self.snap, "ref/a.md", b"changed\n")
        with self.assertRaisesRegex(vendor.VendorError, "ref/a.md"):
            self.fetch_offline()

    def test_an_extra_file_is_rejected(self):
        write(self.snap, "ref/extra.md", b"x\n")
        with self.assertRaisesRegex(vendor.VendorError, "extra.md"):
            self.fetch_offline()

    def test_a_missing_file_is_rejected(self):
        (self.snap / "ref/a.md").unlink()
        with self.assertRaisesRegex(vendor.VendorError, "ref/a.md"):
            self.fetch_offline()

    def test_a_wrong_pin_is_rejected(self):
        self.e["checksum"] = "0" * 64
        with self.assertRaisesRegex(vendor.VendorError, "checksum"):
            self.fetch_offline()

    def test_a_rewritten_checksums_file_is_rejected_by_the_pin(self):
        files = dict(URL_FILES, **{"ref/a.md": b"changed\n"})
        write_snapshot(self.root, "ui", files)
        with self.assertRaisesRegex(vendor.VendorError, "checksum"):
            self.fetch_offline()

    def test_a_missing_snapshot_is_rejected(self):
        shutil.rmtree(self.snap)
        with self.assertRaisesRegex(vendor.VendorError, "fetch ui"):
            self.fetch_offline()

    def test_a_symlink_in_the_snapshot_is_rejected(self):
        (self.snap / "ref/a.md").unlink()
        os.symlink("../SKILL.md", self.snap / "ref/a.md")
        with self.assertRaises(vendor.VendorError):
            self.fetch_offline()

    def test_unsafe_paths_in_checksums_are_rejected(self):
        for bad in ("../x", "/etc/passwd", "a/../../b", "a//b", "./a", "a\\b", "CHECKSUMS.sha256"):
            with self.assertRaises(vendor.VendorError, msg=bad):
                vendor.parse_checksums(f"{'1' * 64}  {bad}\n".encode())

    def test_malformed_checksums_are_rejected(self):
        line = ("1" * 64 + "  a\n").encode()
        for bad in (b"", b"nothex  a\n", b"1" * 64 + b" a\n", line + b"\n", line * 2, b"\xff\xfe"):
            with self.assertRaises(vendor.VendorError, msg=bad):
                vendor.parse_checksums(bad)

    def test_well_formed_checksums_parse(self):
        self.assertEqual(vendor.parse_checksums(checksums_bytes(URL_FILES)),
                         {rel: sha(data) for rel, data in URL_FILES.items()})

    def test_build_check_and_license_work_without_git_or_network(self):
        vendor.build(self.root, "ui")
        target = self.root / "plugins/anthology/knowledge/ui"
        self.assertEqual(sorted(p.relative_to(target).as_posix() for p in target.rglob("*") if p.is_file()),
                         sorted(list(URL_FILES) + ["CHECKSUMS.sha256"]))
        vendor.fetch_licenses(self.root)
        self.assertEqual((self.root / "vendor/licenses/ui.txt").read_bytes(), URL_FILES["LICENSE"])
        self.assertEqual(vendor.check(self.root), [])

    def test_check_reports_a_tampered_snapshot(self):
        vendor.build(self.root, "ui")
        vendor.fetch_licenses(self.root)
        write(self.snap, "SKILL.md", b"changed\n")
        self.assertTrue(any("SKILL.md" in p for p in vendor.check(self.root)))

    def test_check_reports_a_drifted_license(self):
        vendor.build(self.root, "ui")
        vendor.fetch_licenses(self.root)
        write(self.root, "vendor/licenses/ui.txt", "other\n")
        self.assertTrue(any("ui.txt" in p for p in vendor.check(self.root)))

    def test_diff_works_on_a_url_entry(self):
        vendor.build(self.root, "ui")
        write(self.root / "plugins/anthology/knowledge/ui", "SKILL.md", b"---\nname: ui\n---\nnew\n")
        self.assertIn("-body", vendor.diff_entry(self.root, "ui"))

    @unittest.skipUnless(shutil.which("git"), "git is not installed")
    def test_a_patch_on_a_url_entry_can_drop_the_shipped_checksums(self):
        vendor.build(self.root, "ui")
        target = self.root / "plugins/anthology/knowledge/ui"
        (target / "CHECKSUMS.sha256").unlink()
        write(self.root, "vendor/patches/ui.patch", vendor.diff_entry(self.root, "ui"))
        self.e["transform"] = [{"patch": "vendor/patches/ui.patch"}]
        self.set_sources([self.e])
        vendor.build(self.root, "ui")
        self.assertFalse((target / "CHECKSUMS.sha256").exists())
        vendor.fetch_licenses(self.root)
        self.assertEqual(vendor.check(self.root), [])

    def test_sync_on_a_url_entry_is_an_error_with_a_hint(self):
        with self.assertRaisesRegex(vendor.VendorError, r"checksum.*vendor\.py fetch ui"):
            vendor.sync(self.root, "ui", "v2")


class UrlDownloadTest(FixtureCase):
    def setUp(self):
        super().setUp()
        self.e = url_entry("ui")
        self.set_sources([self.e])
        self.snap = self.root / "vendor" / "snapshots" / "ui"

    def download(self, web):
        return vendor.download_snapshot(self.root, self.e, get=web)

    def tree(self, base):
        return {p.relative_to(base).as_posix(): p.read_bytes() for p in base.rglob("*") if p.is_file()}

    def test_download_writes_a_verified_snapshot(self):
        web = FakeWeb("ui")
        self.assertEqual(self.download(web), self.snap)
        self.assertEqual(self.tree(self.snap),
                         dict(URL_FILES, **{"CHECKSUMS.sha256": checksums_bytes(URL_FILES)}))
        self.assertEqual(vendor.snapshot_fetcher(self.root)(self.e), self.snap)
        self.assertEqual(sorted(p.name for p in self.snap.parent.iterdir()), ["ui"])
        self.assertEqual(web.calls[0], web.base + "CHECKSUMS.sha256")

    def test_download_replaces_an_older_snapshot(self):
        write_snapshot(self.root, "ui", {"SKILL.md": b"old\n", "LICENSE": b"old\n", "stale.md": b"s\n"})
        self.download(FakeWeb("ui"))
        self.assertFalse((self.snap / "stale.md").exists())
        self.assertEqual((self.snap / "SKILL.md").read_bytes(), URL_FILES["SKILL.md"])

    def assert_refused_and_untouched(self, web, pattern):
        write_snapshot(self.root, "ui", {"SKILL.md": b"keep\n", "LICENSE": b"keep\n"})
        before = self.tree(self.snap)
        with self.assertRaisesRegex(vendor.VendorError, pattern):
            self.download(web)
        self.assertEqual(self.tree(self.snap), before)
        self.assertEqual(sorted(p.name for p in self.snap.parent.iterdir()), ["ui"])

    def test_a_wrong_pin_refuses_before_downloading_any_file(self):
        self.e["checksum"] = "0" * 64
        web = FakeWeb("ui")
        self.assert_refused_and_untouched(web, "checksum")
        self.assertEqual(web.calls, [web.base + "CHECKSUMS.sha256"])

    def test_a_file_that_does_not_match_its_checksum_refuses(self):
        web = FakeWeb("ui")
        web.pages[web.base + "ref/a.md"] = b"tampered\n"
        self.assert_refused_and_untouched(web, "ref/a.md")

    def test_a_missing_file_refuses(self):
        web = FakeWeb("ui")
        del web.pages[web.base + "ref/a.md"]
        self.assert_refused_and_untouched(web, "404")

    def test_an_index_that_lists_another_file_set_refuses(self):
        everything = sorted(URL_FILES) + ["CHECKSUMS.sha256"]
        self.assert_refused_and_untouched(FakeWeb("ui", index_files=everything + ["more.md"]), "more.md")
        self.assert_refused_and_untouched(
            FakeWeb("ui", index_files=[f for f in everything if f != "ref/a.md"]), "ref/a.md")

    def test_an_index_without_the_skill_refuses(self):
        web = FakeWeb("ui")
        web.pages[web.index_url] = b'{"skills": []}'
        self.assert_refused_and_untouched(web, "index")

    def test_a_malformed_index_refuses(self):
        web = FakeWeb("ui")
        web.pages[web.index_url] = b"not json"
        self.assert_refused_and_untouched(web, "index")

    def test_path_traversal_in_checksums_refuses_and_requests_nothing_else(self):
        for bad in ("../evil.md", "/etc/passwd", "a/../../b.md"):
            sums = checksums_bytes(URL_FILES) + f"{sha(b'x')}  {bad}\n".encode()
            self.e["checksum"] = sha(sums)
            web = FakeWeb("ui", sums=sums)
            self.assert_refused_and_untouched(web, "unsafe")
            self.assertEqual(web.calls, [web.base + "CHECKSUMS.sha256"], bad)
        self.assertFalse((self.root / "vendor" / "evil.md").exists())
        self.assertFalse((self.tmp / "evil.md").exists())

    def test_the_getter_is_https_only(self):
        for bad in ("http://example.test/x", "file:///etc/passwd", "ftp://example.test/x", "//x/y"):
            with self.assertRaises(vendor.VendorError, msg=bad):
                vendor.http_get(bad)

    def test_an_invalid_entry_is_refused_before_any_request(self):
        self.e["url"] = "http://example.test/.well-known/skills/ui/"
        self.set_sources([self.e])
        web = FakeWeb("ui")
        with self.assertRaises(vendor.VendorError):
            self.download(web)
        self.assertEqual(web.calls, [])

    def test_fetch_entry_downloads_url_entries_and_delegates_the_rest(self):
        web = FakeWeb("ui")
        self.assertEqual(vendor.fetch_entry(self.root, self.e, None, get=web), self.snap)
        self.assertEqual(vendor.fetch_entry(self.root, entry("g"), lambda e: Path("/x/" + e["name"])),
                         Path("/x/g"))


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

    def test_drops_the_upstream_dev_marketplace(self):
        # Its entry version would disagree with plugin.json and `claude plugin validate`
        # checks the marketplace file instead of the plugin.
        tree = self.make_tree()
        write(tree, ".claude-plugin/marketplace.json", '{"plugins": [{"version": "7.0.2"}]}\n')
        vendor.rename_plugin(tree, self.OLD, self.NEW, plugin_version="0.1.0")
        self.assertFalse((tree / ".claude-plugin/marketplace.json").exists())
        self.assertTrue((tree / ".claude-plugin/plugin.json").is_file())

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

## Tool Integration

### If no tools are connected
- Paste the copy

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

    def strip(self, doc):
        tree, _ = self.run_strip({"SKILL.md": doc})
        return (tree / "SKILL.md").read_text(encoding="utf-8")

    def test_tools_connected_blocks_without_a_placeholder(self):
        # a one-line list item, a paragraph, a lead-in with a list after a blank line, a "weren't"
        doc = ("# T\n\n## A\n\n- Offer drafts\n- If project management tool is connected, offer updates\n\n"
               "## B\n\nIf the UXKIN tools are connected and the direction is open, call\n"
               "`find_ui_materials` for the feeling to see\nreal sites.\n\nNext paragraph.\n\n"
               "## C\n\nIf the UXKIN tools are connected:\n\n1. Call `find_ui_materials`\n   with words.\n"
               "2. Try again.\n\nWithout the tools, rely on the project.\n\n"
               "## D\n\nIf the UXKIN tools weren't connected and it would help, say once\n"
               "(setup: https://uxkin.com/ui-design-mcp).\n\nEnd.\n")
        self.assertEqual(self.strip(doc), (
            "# T\n\n## A\n\n- Offer drafts\n\n## B\n\nNext paragraph.\n\n"
            "## C\n\nWithout the tools, rely on the project.\n\n## D\n\nEnd.\n"))

    EMAIL_SEQUENCE = ("# Email Sequence\n\n## Sequence types\n\n**Educational Drip:**\nLesson 1 -- Lesson 2\n\n"
                      "## Tool Integration\n\n"
                      "### If ~~email marketing is connected (e.g., Klaviyo, Mailchimp)\n"
                      "- Reference how to set up the flow\n- Map the branching logic\n\n"
                      "### If ~~marketing automation or ~~CRM is connected (e.g., HubSpot)\n"
                      "- Reference lead scoring data\n\n"
                      "### If no tools are connected\n"
                      "- Deliver all email content in copy-paste-ready format\n"
                      "- Include a setup checklist the user can follow in any email platform:\n"
                      "  1. Create the automation or flow\n  2. Set the enrollment trigger\n\n"
                      "## Output\n\nPresent the complete sequence.\n")

    def test_fallback_subsections_survive_inside_a_removed_connector_section(self):
        # the email-sequence shape: the container and the connector children go; the
        # "If no tools are connected" subsection stays verbatim under the container heading
        self.assertEqual(self.strip(self.EMAIL_SEQUENCE), (
            "# Email Sequence\n\n## Sequence types\n\n**Educational Drip:**\nLesson 1 -- Lesson 2\n\n"
            "## Tool Integration\n\n"
            "### If no tools are connected\n"
            "- Deliver all email content in copy-paste-ready format\n"
            "- Include a setup checklist the user can follow in any email platform:\n"
            "  1. Create the automation or flow\n  2. Set the enrollment trigger\n\n"
            "## Output\n\nPresent the complete sequence.\n"))

    def test_fallback_paragraphs_survive_inside_a_removed_connector_section(self):
        # the write-spec shape
        doc = ("# T\n\n### 2. Gather\n\nAsk.\n\n### 3. Pull Context from Connected Tools\n\n"
               "If **~~project tracker** is connected:\n- Search tickets\n\n"
               "If **~~design** is connected:\n- Pull mockups\n\n"
               "If these tools are not connected, work entirely from what the user provides. "
               "Do not ask the user to connect tools.\n\n### 4. Generate\n\nkept\n")
        self.assertEqual(self.strip(doc), (
            "# T\n\n### 2. Gather\n\nAsk.\n\n### 3. Pull Context from Connected Tools\n\n"
            "If these tools are not connected, work entirely from what the user provides. "
            "Do not ask the user to connect tools.\n\n### 4. Generate\n\nkept\n"))

    def test_a_removed_section_without_fallbacks_leaves_nothing_and_a_fallback_at_the_end_stays(self):
        self.assertEqual(self.strip("# T\n\n## Connectors\n\nIf ~~a is connected:\n- b\n\n## Next\n\nx\n"),
                         "# T\n\n## Next\n\nx\n")
        self.assertEqual(self.strip("# T\n\n## Connectors\n\nIf ~~a is connected:\n- b\n\n"
                                    "### If no tools are connected\n- c\n"),
                         "# T\n\n## Connectors\n\n### If no tools are connected\n- c\n")

    def test_a_fallback_heading_is_not_a_connector_section_by_itself(self):
        doc = "# T\n\n### If no connected tools\n- Ask the user\n\n### Next\n\nx\n"
        self.assertEqual(self.strip(doc), doc)

    def test_a_longer_fence_is_not_closed_by_a_shorter_one(self):
        doc = "# T\n\n## Connectors\n\n````\n```\n## Not a heading\n```\n````\n\n## Tips\n\nkept\n"
        self.assertEqual(self.strip(doc), "# T\n\n## Tips\n\nkept\n")

    def test_no_tool_fallbacks_stay(self):
        doc = ("# T\n\nIf no project management tool is connected:\n- Ask the user\n\n"
               "**If tools are not connected:**\n- Use web search\n\n"
               "If these tools are not connected, work from what the user provides.\n")
        self.assertEqual(self.strip(doc), doc)

    def test_a_list_item_telling_the_user_to_connect_via_mcp_goes(self):
        doc = ("# T\n\n**If tools are not connected:**\n- Use web search\n"
               "- Note: \"For precise data, connect an SEO tool like Ahrefs via MCP.\"\n\nNext.\n")
        self.assertEqual(self.strip(doc), "# T\n\n**If tools are not connected:**\n- Use web search\n\nNext.\n")

    SUPERCHARGED = ("```\n+----+\n| STANDALONE (always works) |\n| SUPERCHARGED (when you connect your tools) |\n"
                    "| + Tracker: link tickets |\n+----+\n```\n")

    def test_supercharged_box_goes_with_a_heading_it_leaves_empty(self):
        doc = f"# T\n\nIntro.\n\n## How It Works\n\n{self.SUPERCHARGED}\n## Next\n\nkept\n"
        self.assertEqual(self.strip(doc), "# T\n\nIntro.\n\n## Next\n\nkept\n")

    def test_supercharged_box_keeps_a_heading_that_has_other_content(self):
        doc = f"# T\n\n## How It Works\n\nSome words.\n\n{self.SUPERCHARGED}\nAfter.\n"
        self.assertEqual(self.strip(doc), "# T\n\n## How It Works\n\nSome words.\n\nAfter.\n")
        doc = f"# T\n\n## How It Works\n\n{self.SUPERCHARGED}\n### Sub\n\nkept\n"
        self.assertEqual(self.strip(doc), "# T\n\n## How It Works\n\n### Sub\n\nkept\n")

    def test_supercharged_box_takes_a_line_that_only_introduces_it(self):
        doc = f"# T\n\nText.\n\nHere is how it works:\n\n{self.SUPERCHARGED}\nAfter.\n"
        self.assertEqual(self.strip(doc), "# T\n\nText.\n\nAfter.\n")

    def test_other_fenced_blocks_are_untouched(self):
        doc = "# T\n\n```\n+--+\n| STANDALONE |\n+--+\n```\n\nIf the tool is fine.\n"
        self.assertEqual(self.strip(doc), doc)

    def test_idempotent(self):
        extra = CONNECTOR_DOC + "\n## Box\n\n" + self.SUPERCHARGED + "\n## Z\n\nIf the UXKIN tools are connected:\n\n1. a\n\nEnd.\n"
        tree, _ = self.run_strip({"SKILL.md": extra, "references/a.md": CONNECTOR_DOC})
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

    def test_renames_the_frontmatter_name_and_the_slash_references(self):
        tree = self.make("---\nname: code-review\ndescription: Use /code-review here.\n---\n\n"
                         "# /code-review\n\n```\n/code-review <PR URL>\n```\n\n"
                         "Also /code-review-checklist, a/code-review and code-review stay.\n")
        vendor.rename_skill(tree, "code-review", "code-review-checklist")
        self.assertEqual((tree / "SKILL.md").read_text(encoding="utf-8"),
                         "---\nname: code-review-checklist\ndescription: Use /code-review here.\n---\n\n"
                         "# code-review-checklist\n\n```\ncode-review-checklist <PR URL>\n```\n\n"
                         "Also /code-review-checklist, a/code-review and code-review stay.\n")

    def test_rename_is_idempotent_on_the_body(self):
        tree = self.make("---\nname: code-review\n---\n\n# /code-review\n")
        vendor.rename_skill(tree, "code-review", "code-review-checklist")
        once = (tree / "SKILL.md").read_bytes()
        (tree / "SKILL.md").write_bytes(once.replace(b"name: code-review-checklist", b"name: code-review"))
        vendor.rename_skill(tree, "code-review", "code-review-checklist")
        self.assertEqual((tree / "SKILL.md").read_bytes(), once)

    def test_a_different_name_is_an_error(self):
        tree = self.make("---\nname: other\n---\nbody\n")
        with self.assertRaises(vendor.VendorError):
            vendor.rename_skill(tree, "code-review", "x")

    def test_frontmatter_without_a_name_is_an_error(self):
        with self.assertRaises(vendor.VendorError):
            vendor.rename_skill(self.make("---\ndescription: x\n---\n\n/a\n"), "a", "b")

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


class PortmapTest(FixtureCase):
    """The `portmap` entry field: the script rule applied to the pristine and the shipped tree."""

    PORTMAP = "vendor/patches/taste.portmap.json"
    NEO = "// bridge\nexport const x = 1;\n"
    PATCH_ADD_DROP = (
        "--- a/scripts/drop.mjs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-// drop\n-import fs from 'node:fs';\n"
        "--- /dev/null\n+++ b/scripts/neo.mjs\n@@ -0,0 +1,2 @@\n+// bridge\n+export const x = 1;\n")

    def setUp(self):
        super().setUp()
        real = json.loads((ROOT / "vendor/patches/design-taste-frontend.portmap.json").read_text(encoding="utf-8"))
        self.rule, self.chrome = real["script_rule"], real["chrome_regex"]
        self.upstream = {
            "LICENSE": MIT_TEXT,
            "SKILL.md": "---\nname: taste\n---\nbody\n",
            "scripts/keep.mjs": "// keep\nimport fs from 'node:fs';\n",
            "scripts/port.mjs": "// port\nimport fs from 'node:fs';\n",
            "scripts/drop.mjs": "// drop\nimport fs from 'node:fs';\n",
            "assets/lib.js": "window.x = 1;\n",
        }
        self.write_map()

    def write_map(self, scripts=None, added=None):
        scripts = scripts or {
            "scripts/keep.mjs": {"action": "keep", "chrome": False, "why": "k"},
            "scripts/port.mjs": {"action": "port", "how": "trim", "chrome": True, "why": "p", "browseros": "b"},
            "scripts/drop.mjs": {"action": "drop", "chrome": True, "why": "d", "browseros": "b"},
        }
        data = {"script_rule": self.rule, "chrome_regex": self.chrome, "scripts": scripts,
                "added": {"scripts/neo.mjs": {"chrome": False, "why": "n"}} if added is None else added}
        write(self.root, self.PORTMAP, json.dumps(data, indent=2) + "\n")

    def entry(self, files=None, patch=True):
        return self.add_upstream("taste", files or self.upstream,
                                 transform=[{"patch": "vendor/patches/taste.patch"}] if patch else [],
                                 portmap=self.PORTMAP)

    def data(self):
        return json.loads((self.root / self.PORTMAP).read_text(encoding="utf-8"))

    def trees(self, shipped_edit):
        pristine, shipped = self.tmp / "p", self.tmp / "s"
        for base in (pristine, shipped):
            shutil.rmtree(base, ignore_errors=True)
            for rel, text in self.upstream.items():
                write(base, rel, text)
        shipped_edit(shipped)
        return pristine, shipped

    def problems(self, shipped_edit):
        pristine, shipped = self.trees(shipped_edit)
        return vendor.portmap_problems(self.data(), pristine, shipped)

    @staticmethod
    def good(shipped):
        (shipped / "scripts/drop.mjs").unlink()
        write(shipped, "scripts/neo.mjs", PortmapTest.NEO)

    def test_the_rule_finds_extensions_shebangs_and_node_js_only(self):
        tree = self.tmp / "t"
        for rel, text in {
                "a.mjs": "", "B.MJS": "", "c.py": "", "d.sh": "", "e.ts": "", "f.txt": "#!/bin/sh\n",
                "g.txt": "# !\n", "h.md": "# title\n", "browser.js": "window.x = 1;\n",
                "node1.js": "const fs = require('fs');\n", "node2.js": "import fs from 'node:fs';\n",
                "node3.js": "process.exit(1)\n", "node4.js": "const m = await import('node:fs');\n",
                "node5.js": "import { dirname } from 'path';\n", "page.js": "const required = 1; // from 'react'\n",
                "sub/x.cjs": ""}.items():
            write(tree, rel, text)
        write(tree, "bin.dat", b"\xff\xfe\x00 process.exit(1)")
        self.assertEqual(vendor.portmap_scripts(tree, self.rule), {
            "a.mjs", "B.MJS", "c.py", "d.sh", "e.ts", "f.txt", "node1.js", "node2.js", "node3.js", "node4.js",
            "node5.js", "sub/x.cjs"})

    def test_a_clean_shipped_tree_has_no_problems(self):
        self.assertEqual(self.problems(self.good), [])

    def test_an_upstream_script_missing_from_the_map_is_a_problem(self):
        pristine, shipped = self.trees(self.good)
        write(pristine, "scripts/new.mjs", "import fs from 'node:fs';\n")
        write(shipped, "scripts/new.mjs", "import fs from 'node:fs';\n")
        found = vendor.portmap_problems(self.data(), pristine, shipped)
        self.assertTrue(any("scripts/new.mjs" in p and "not in the port map" in p for p in found), found)

    def test_a_mapped_script_that_is_not_upstream_is_a_problem(self):
        pristine, shipped = self.trees(self.good)
        (pristine / "scripts/keep.mjs").unlink()
        found = vendor.portmap_problems(self.data(), pristine, shipped)
        self.assertTrue(any("scripts/keep.mjs" in p and "not upstream" in p for p in found), found)

    def test_a_shipped_drop_script_is_a_problem(self):
        found = self.problems(lambda s: write(s, "scripts/neo.mjs", self.NEO))
        self.assertEqual(len(found), 1, found)
        self.assertIn("scripts/drop.mjs", found[0])
        self.assertIn("drop", found[0])

    def test_a_modified_keep_script_is_a_problem(self):
        def edit(shipped):
            self.good(shipped)
            write(shipped, "scripts/keep.mjs", "// keep\nimport fs from 'node:fs';\n// edited\n")
        found = self.problems(edit)
        self.assertEqual(len(found), 1, found)
        self.assertIn("scripts/keep.mjs", found[0])
        self.assertIn("byte-identical", found[0])

    def test_a_missing_keep_or_port_script_is_a_problem(self):
        for rel in ("scripts/keep.mjs", "scripts/port.mjs"):
            def edit(shipped, rel=rel):
                self.good(shipped)
                (shipped / rel).unlink()
            found = self.problems(edit)
            self.assertTrue(any(rel in p and "not shipped" in p for p in found), (rel, found))

    def test_a_ported_script_may_differ(self):
        def edit(shipped):
            self.good(shipped)
            write(shipped, "scripts/port.mjs", "// ported\nimport fs from 'node:fs';\n")
        self.assertEqual(self.problems(edit), [])

    def test_the_added_script_must_ship_and_must_not_be_upstream(self):
        found = self.problems(lambda s: (s / "scripts/drop.mjs").unlink())
        self.assertTrue(any("scripts/neo.mjs" in p and "not shipped" in p for p in found), found)
        pristine, shipped = self.trees(self.good)
        write(pristine, "scripts/neo.mjs", "import fs from 'node:fs';\n")
        found = vendor.portmap_problems(self.data(), pristine, shipped)
        self.assertTrue(any("scripts/neo.mjs" in p and "upstream" in p for p in found), found)

    def test_an_unlisted_shipped_script_is_a_problem(self):
        def edit(shipped):
            self.good(shipped)
            write(shipped, "scripts/extra.py", "print(1)\n")
            write(shipped, "assets/extra.js", "const fs = require('fs');\n")
        found = "\n".join(self.problems(edit))
        self.assertIn("scripts/extra.py", found)
        self.assertIn("assets/extra.js", found)

    def test_a_chrome_string_in_a_file_or_a_path_is_a_problem(self):
        for rel, text in (("notes/a.md", "launch with --Remote-Debugging-Port=0"),
                          ("notes/b.md", "uses Playwright"),
                          ("notes/c.bin", b"\x00\x01 google chrome \x02"),
                          ("notes/puppeteer.svg", "<svg/>")):
            def edit(shipped, rel=rel, text=text):
                self.good(shipped)
                write(shipped, rel, text)
            found = self.problems(edit)
            self.assertEqual(len(found), 1, (rel, found))
            self.assertIn(rel, found[0])

    def test_headless_chrome_alone_is_not_a_chrome_launch(self):
        self.assertEqual(self.problems(lambda s: (self.good(s), write(s, "a.md", "HeadlessChrome/ UA"))), [])

    def test_build_refuses_an_unmapped_upstream_script(self):
        files = dict(self.upstream, **{"scripts/new.mjs": "import fs from 'node:fs';\n"})
        e = self.entry(files, patch=False)
        self.set_sources([e])
        with self.assertRaises(vendor.VendorError) as ctx:
            vendor.build(self.root, "taste", fetch=self.fetch)
        self.assertIn("scripts/new.mjs", str(ctx.exception))
        self.assertFalse((self.root / e["target"]).exists())

    def test_build_refuses_a_shipped_drop_script(self):
        e = self.entry(patch=False)
        self.set_sources([e])
        with self.assertRaises(vendor.VendorError) as ctx:
            vendor.build(self.root, "taste", fetch=self.fetch)
        self.assertIn("scripts/drop.mjs", str(ctx.exception))

    def test_a_missing_portmap_file_is_an_error(self):
        e = self.entry(patch=False)
        (self.root / self.PORTMAP).unlink()
        self.set_sources([e])
        with self.assertRaises(vendor.VendorError) as ctx:
            vendor.build(self.root, "taste", fetch=self.fetch)
        self.assertIn("taste.portmap.json", str(ctx.exception))

    def test_the_portmap_path_must_be_a_safe_json_file_under_vendor_patches(self):
        for bad in ("../x.json", "/etc/x.json", "vendor/other/x.json", "vendor/patches/x.txt",
                    "vendor/patches/", "vendor/patches/../x.json", 3):
            errors = vendor.validate_sources([entry("p", portmap=bad)])
            self.assertTrue(any("portmap" in m for m in errors), (bad, errors))
        self.assertEqual(vendor.validate_sources([entry("p", portmap="vendor/patches/p.portmap.json")]), [])

    @unittest.skipUnless(shutil.which("git"), "git is not installed")
    def test_check_is_clean_with_a_good_patch_and_lists_every_problem_without_one(self):
        e = self.entry()
        write(self.root, "vendor/patches/taste.patch", self.PATCH_ADD_DROP)
        self.build_all([e])
        self.assertEqual(self.check(), [])
        e["transform"] = []
        self.set_sources([e])
        found = self.check()
        self.assertTrue(any("scripts/drop.mjs" in p for p in found), found)
        self.assertTrue(any("scripts/neo.mjs" in p for p in found), found)
        self.assertGreaterEqual(len([p for p in found if p.startswith("taste: portmap: ")]), 2, found)

    @unittest.skipUnless(shutil.which("git"), "git is not installed")
    def test_sync_reports_portmap_problems_without_failing_the_rebuild(self):
        e = self.entry()
        write(self.root, "vendor/patches/taste.patch", self.PATCH_ADD_DROP)
        self.build_all([e])
        write(self.pristine["taste"], "scripts/new.mjs", "import fs from 'node:fs';\n")
        report = vendor.sync(self.root, "taste", "v2", fetch=self.fetch, resolve=lambda *_: SHA_B)
        self.assertTrue(any("scripts/new.mjs" in p for p in report["portmap"]), report["portmap"])


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
