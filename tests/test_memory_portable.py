"""Tests for scripts/memory_portable.py (export/import) and the export/import skills.

Import is the security-sensitive half: every malicious archive below must be rejected as a
whole, with nothing written anywhere (the whole temp tree is compared before and after).
Git-dependent cases skip when git is missing.
"""
import gzip
import hashlib
import io
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from _mem import MemCase, ROOT, SCRIPTS, needs_git, note_text, write

import memory_portable  # noqa: E402  (path set up by _mem)
import memory_sync  # noqa: E402

MANIFEST = json.dumps({"format": 1, "date": "2026-01-01", "counts": {}}).encode()
MIB = 1024 * 1024


class ZeroFile(object):
    """A read-only file of `size` zero bytes (so huge archives cost nothing to build)."""

    def __init__(self, size):
        self.left = size

    def read(self, n=-1):
        n = self.left if n is None or n < 0 else min(n, self.left)
        self.left -= n
        return b"\0" * n


def add(tar, name, data=b"", type_=tarfile.REGTYPE, linkname="", size=None):
    info = tarfile.TarInfo(name)
    info.type = type_
    info.linkname = linkname
    info.mode = 0o644
    if type_ == tarfile.REGTYPE:
        if hasattr(data, "read"):
            info.size = size
            tar.addfile(info, data)
        else:
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    else:
        tar.addfile(info)


def make_archive(path, members, manifest=True, level=6):
    """members: [(name, bytes)] or [(name, dict(type_=..., linkname=..., data=..., size=...))]."""
    with tarfile.open(str(path), "w:gz", compresslevel=level) as tar:
        if manifest:
            add(tar, "manifest.json", MANIFEST)
        for name, spec in members:
            if isinstance(spec, dict):
                add(tar, name, **spec)
            else:
                add(tar, name, spec)
    return Path(path)


def tree(root):
    """{relative path: content digest, or None for folders, or "link:<target>"} of everything under root
    (lexical, symlinks not followed, `.git` skipped)."""
    out = {}
    for base, dirs, files in os.walk(str(root), followlinks=False):
        dirs[:] = [d for d in dirs if d != ".git"]
        for name in dirs + files:
            p = os.path.join(base, name)
            rel = os.path.relpath(p, str(root))
            if os.path.islink(p):
                out[rel] = "link:" + os.readlink(p)
            elif os.path.isfile(p):
                with open(p, "rb") as fh:
                    out[rel] = "sha256:" + hashlib.sha256(fh.read()).hexdigest()
            else:
                out[rel] = None
    return out


def cli_env(env):
    """Keep the CLI subprocess from writing bytecode caches (macOS puts them under HOME)."""
    return dict(env, PYTHONDONTWRITEBYTECODE="1",
                PYTHONPYCACHEPREFIX=os.path.join(tempfile.gettempdir(), "anthology-test-pycache"))


class PortableCase(MemCase):
    def setUp(self):
        super().setUp()
        self.cwd = self.tmp / "plain"
        self.cwd.mkdir()
        self.out = self.tmp / "out"
        self.out.mkdir()
        self.personal = self.anth / "personal-memory"
        self.env = dict(os.environ)

    def second_machine(self, name="b"):
        """An environment dict with its own empty HOME and config dir."""
        home = self.tmp / name / "home"
        home.mkdir(parents=True, exist_ok=True)
        env = dict(os.environ, HOME=str(home), CLAUDE_CONFIG_DIR=str(home / ".claude"))
        return env, home / ".claude" / "anthology"

    def seed_personal(self):
        write(self.personal / "user-me.md", note_text("user-me", "user", "About me", "2026-01-02"))
        write(self.personal / "feedback-x.md", note_text("feedback-x", "feedback", "Do x", "2026-01-03"))

    def archive(self, **kw):
        self.seed_personal()
        res = memory_portable.export(self.cwd, self.out, today="2026-04-05", env=self.env, **kw)
        return Path(res["path"]), res


class ExportTest(PortableCase):
    def test_archive_shape_and_manifest(self):
        path, res = self.archive()
        self.assertEqual(path.name, "anthology-memory-2026-04-05.tar.gz")
        self.assertEqual(res["counts"], {"personal": 2, "repo": 0, "agent": 0})
        self.assertFalse(res["repo_included"])
        with tarfile.open(str(path), "r:gz") as tar:
            members = tar.getmembers()
            self.assertEqual([m.name for m in members],
                             ["manifest.json", "personal/feedback-x.md", "personal/user-me.md"])
            for m in members:
                self.assertTrue(m.isreg())
                self.assertEqual((m.mode, m.uid, m.gid, m.uname, m.gname, m.mtime),
                                 (0o644, 0, 0, "", "", 0))
            raw = tar.extractfile("manifest.json").read().decode()
        manifest = json.loads(raw)
        self.assertEqual(manifest, {"format": 1, "date": "2026-04-05",
                                    "counts": {"personal": 2, "repo": 0, "agent": 0}})
        for private in (str(self.tmp), str(self.home), os.path.basename(str(self.tmp)), "home"):
            self.assertNotIn(private, raw)

    def test_deterministic(self):
        self.seed_personal()
        a = memory_portable.export(self.cwd, self.tmp / "o1", today="2026-04-05", env=self.env)
        b = memory_portable.export(self.cwd, self.tmp / "o2", today="2026-04-05", env=self.env)
        self.assertEqual(Path(a["path"]).read_bytes(), Path(b["path"]).read_bytes())

    def test_never_overwrites_an_existing_archive(self):
        first, _ = self.archive()
        before = first.read_bytes()
        second = Path(memory_portable.export(self.cwd, self.out, today="2026-04-05", env=self.env)["path"])
        self.assertEqual(second.name, "anthology-memory-2026-04-05-2.tar.gz")
        self.assertEqual(first.read_bytes(), before)
        third = Path(memory_portable.export(self.cwd, self.out, today="2026-04-05", env=self.env)["path"])
        self.assertEqual(third.name, "anthology-memory-2026-04-05-3.tar.gz")

    def test_only_plain_notes_are_exported(self):
        self.seed_personal()
        write(self.personal / "MEMORY.md", "- generated\n")
        write(self.personal / "notes.txt", "x")
        write(self.personal / ".hidden.md", "x")
        write(self.personal / "has space.md", "x")
        (self.personal / "bad.md").write_bytes(b"\xff\xfe not utf-8")
        outside = self.tmp / "outside.md"
        write(outside, "secret outside\n")
        (self.personal / "link.md").symlink_to(outside)
        (self.personal / "sub").mkdir()
        write(self.personal / "sub" / "deep.md", "x")
        res = memory_portable.export(self.cwd, self.out, today="2026-04-05", env=self.env)
        with tarfile.open(res["path"], "r:gz") as tar:
            names = tar.getnames()
        self.assertEqual(names, ["manifest.json", "personal/feedback-x.md", "personal/user-me.md"])

    def test_default_folder_is_under_the_anthology_home_not_the_cwd(self):
        self.seed_personal()
        res = memory_portable.export(self.cwd, None, today="2026-04-05", env=self.env)
        exports = self.anth / "exports"
        self.assertEqual(Path(res["path"]).parent, exports)
        self.assertEqual(stat.S_IMODE(exports.stat().st_mode), 0o700)
        self.assertEqual(list(self.cwd.iterdir()), [])

    def test_explicit_folder_inside_a_git_work_tree_is_refused(self):
        self.seed_personal()
        work = self.tmp / "work"
        (work / ".git").mkdir(parents=True)  # a marker is enough: no git needed
        for target in (work, work / "sub" / "deeper"):
            before = tree(self.tmp)
            with self.assertRaisesRegex(memory_portable.ExportRefused,
                                        "export refuses to write personal notes inside a git repo"):
                memory_portable.export(self.cwd, target, today="2026-04-05", env=self.env)
            self.assertEqual(tree(self.tmp), before)

    def test_cli_export_refuses_a_folder_in_a_git_repo_with_exit_2(self):
        self.seed_personal()
        work = self.tmp / "work"
        (work / ".git").mkdir(parents=True)
        r = subprocess.run([sys.executable, str(SCRIPTS / "memory_portable.py"), "export", "--out", str(work)],
                           cwd=str(self.cwd), env=cli_env(self.env), stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, text=True)
        self.assertEqual(r.returncode, 2, r.stdout)
        self.assertIn("export refuses to write personal notes inside a git repo", r.stderr)
        self.assertEqual([p.name for p in work.iterdir()], [".git"])

    @needs_git
    def test_exporting_from_inside_a_repo_does_not_write_into_it(self):
        root = self.make_repo()
        memory_sync.approve(root)
        write(root / ".claude" / "memory" / "project-r.md", note_text("project-r"))
        before = tree(root)
        res = memory_portable.export(root, None, today="2026-04-05", env=self.env)
        self.assertEqual(tree(root), before)
        self.assertEqual(Path(res["path"]).parent, self.anth / "exports")
        self.assertEqual(res["counts"]["repo"], 1)

    def test_creates_the_output_folder(self):
        self.seed_personal()
        res = memory_portable.export(self.cwd, self.tmp / "new" / "dir", today="2026-04-05", env=self.env)
        self.assertTrue(Path(res["path"]).is_file())

    @needs_git
    def test_repo_and_agent_notes_only_from_an_approved_repo(self):
        root = self.make_repo()
        write(root / ".claude" / "memory" / "project-r.md", note_text("project-r"))
        write(root / ".claude" / "memory" / "MEMORY.md", "generated\n")
        write(root / ".claude" / "agent-memory" / "anthology-w" / "feedback-w.md",
              note_text("feedback-w", "feedback"))
        write(root / ".claude" / "agent-memory" / "bad dir" / "x.md", "x")
        res = memory_portable.export(root, self.out, today="2026-04-05", env=self.env)
        self.assertFalse(res["repo_included"])
        self.assertEqual(res["counts"], {"personal": 0, "repo": 0, "agent": 0})
        memory_sync.approve(root)
        res = memory_portable.export(root, self.out, today="2026-04-06", env=self.env)
        self.assertTrue(res["repo_included"])
        self.assertEqual(res["counts"], {"personal": 0, "repo": 1, "agent": 1})
        with tarfile.open(res["path"], "r:gz") as tar:
            self.assertEqual(tar.getnames(), ["manifest.json",
                                              "repo/agent-memory/anthology-w/feedback-w.md",
                                              "repo/memory/project-r.md"])


def secret_samples():
    """Secret-looking strings assembled at run time so no literal sits in the repo."""
    return {
        "private-key": "-----BEGIN " + "RSA PRIVATE KEY-----\nabc\n",
        "aws-access-key": "id " + "AKIA" + "ABCDEFGHIJKLMNOP",
        "github-token": "ghp" + "_" + "a1" * 20,
        "slack-token": "xox" + "b-" + "1234567890-abcdef",
        "api-key": "sk" + "-ant-" + "abcdefghij0123456789",
        "credential-assignment": "pass" + "word: hunter22hunter",
    }


class SecretsTest(PortableCase):
    def test_each_pattern_has_a_label(self):
        for label, text in secret_samples().items():
            self.assertEqual(memory_portable.scan_secrets(text), [label], label)
        self.assertEqual(memory_portable.scan_secrets("github_pat" + "_" + "A" * 25), ["github-token"])
        self.assertEqual(memory_portable.scan_secrets("Just a normal note about passwords in general."), [])
        self.assertEqual(memory_portable.scan_secrets("token: short"), [])

    def test_export_reports_labels_not_values(self):
        samples = secret_samples()
        write(self.personal / "user-keys.md", note_text("user-keys", "user", body=samples["aws-access-key"] + "\n"))
        write(self.personal / "user-ok.md", note_text("user-ok", "user"))
        res = memory_portable.export(self.cwd, self.out, today="2026-04-05", env=self.env)
        self.assertEqual(res["secrets"], [{"file": "personal/user-keys.md", "pattern": "aws-access-key"}])
        self.assertNotIn("ABCDEFGHIJKLMNOP", json.dumps(res))

    def test_import_plan_reports_labels_not_values(self):
        samples = secret_samples()
        path = make_archive(self.tmp / "s.tar.gz", [
            ("personal/user-keys.md", ("x\n" + samples["github-token"] + "\n").encode())])
        plan = memory_portable.plan_import(path, self.cwd, env=self.env)
        self.assertEqual(plan["secrets"], [{"file": "personal/user-keys.md", "pattern": "github-token"}])
        self.assertNotIn(samples["github-token"], json.dumps(plan))


class ImportTest(PortableCase):
    def test_round_trip_into_an_empty_home_is_byte_identical(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        plan = memory_portable.plan_import(path, self.cwd, today="2026-04-06", env=env_b)
        self.assertEqual([e["member"] for e in plan["new"]],
                         ["personal/feedback-x.md", "personal/user-me.md"])
        self.assertEqual((plan["identical"], plan["conflicts"], plan["skipped_repo"]), ([], [], []))
        before = tree(anth_b)
        self.assertEqual(before, {})  # planning wrote nothing
        result = memory_portable.apply_import(plan)
        self.assertEqual(len(result["written"]), 2)
        for name in ("user-me.md", "feedback-x.md"):
            self.assertEqual((anth_b / "personal-memory" / name).read_bytes(),
                             (self.personal / name).read_bytes())
        index = (anth_b / "personal-memory" / "MEMORY.md").read_text()
        self.assertIn("[user-me](user-me.md)", index)

    def test_identical_skipped_conflict_kept_side_by_side(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        mem_b = anth_b / "personal-memory"
        write(mem_b / "user-me.md", (self.personal / "user-me.md").read_text())          # identical
        write(mem_b / "feedback-x.md", "my own different note\n")                        # conflict
        plan = memory_portable.plan_import(path, self.cwd, today="2026-04-06", env=env_b)
        self.assertEqual([e["member"] for e in plan["identical"]], ["personal/user-me.md"])
        self.assertEqual([e["member"] for e in plan["conflicts"]], ["personal/feedback-x.md"])
        self.assertEqual(plan["new"], [])
        result = memory_portable.apply_import(plan)
        self.assertEqual((mem_b / "feedback-x.md").read_text(), "my own different note\n")
        imported = mem_b / "feedback-x.imported-2026-04-06.md"
        self.assertEqual(imported.read_bytes(), (self.personal / "feedback-x.md").read_bytes())
        self.assertEqual(result["conflicts"], [str(imported)])
        self.assertEqual(result["written"], [])
        # importing again does not pile up copies: the imported copy counts as identical
        plan2 = memory_portable.plan_import(path, self.cwd, today="2026-04-07", env=env_b)
        self.assertEqual((len(plan2["new"]), len(plan2["conflicts"]), len(plan2["identical"])), (0, 0, 2))
        memory_portable.apply_import(plan2)
        self.assertEqual(sorted(p.name for p in mem_b.glob("feedback-x*")),
                         ["feedback-x.imported-2026-04-06.md", "feedback-x.md"])

    def test_imported_name_is_made_unique(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        mem_b = anth_b / "personal-memory"
        write(mem_b / "feedback-x.md", "mine\n")
        write(mem_b / "feedback-x.imported-2026-04-06.md", "an older different import\n")
        plan = memory_portable.plan_import(path, self.cwd, today="2026-04-06", env=env_b)
        memory_portable.apply_import(plan)
        self.assertEqual((mem_b / "feedback-x.imported-2026-04-06.md").read_text(),
                         "an older different import\n")
        self.assertTrue((mem_b / "feedback-x.imported-2026-04-06-2.md").is_file())

    def test_plan_and_dry_run_write_nothing(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        write(anth_b / "personal-memory" / "feedback-x.md", "mine\n")
        before = tree(self.tmp)
        memory_portable.plan_import(path, self.cwd, env=env_b)
        self.assertEqual(tree(self.tmp), before)
        r = self.run_cli(env_b, "import", str(path), "--dry-run")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(tree(self.tmp), before)
        r = self.run_cli(env_b, "import", str(path))  # no --yes: also only the plan
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("Nothing written", r.stdout)
        self.assertEqual(tree(self.tmp), before)
        r = self.run_cli(env_b, "import", str(path), "--yes", "--dry-run")  # dry-run wins
        self.assertEqual(tree(self.tmp), before)

    def test_changed_after_planning_becomes_a_conflict_not_an_overwrite(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        plan = memory_portable.plan_import(path, self.cwd, today="2026-04-06", env=env_b)
        write(anth_b / "personal-memory" / "user-me.md", "appeared meanwhile\n")
        result = memory_portable.apply_import(plan)
        self.assertEqual((anth_b / "personal-memory" / "user-me.md").read_text(), "appeared meanwhile\n")
        self.assertTrue((anth_b / "personal-memory" / "user-me.imported-2026-04-06.md").is_file())
        self.assertEqual(len(result["conflicts"]), 1)

    def test_archive_swapped_after_planning_is_refused(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        plan = memory_portable.plan_import(path, self.cwd, env=env_b)
        # same member names, different content: only the checksum can tell
        make_archive(path, [("personal/feedback-x.md", b"swapped\n"), ("personal/user-me.md", b"swapped\n")])
        with self.assertRaisesRegex(memory_portable.ImportRejected, "changed after"):
            memory_portable.apply_import(plan)
        self.assertEqual(tree(anth_b), {})

    def test_the_archive_is_opened_once_and_the_plan_hash_is_of_those_bytes(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        real_open = os.open
        opened = []

        def spy(file, *a, **kw):
            if str(file) == str(path):
                opened.append(file)
            return real_open(file, *a, **kw)

        with mock.patch.object(os, "open", side_effect=spy):
            plan = memory_portable.plan_import(path, self.cwd, env=env_b)
        self.assertEqual(len(opened), 1)
        self.assertEqual(plan["sha256"], hashlib.sha256(path.read_bytes()).hexdigest())
        opened[:] = []
        with mock.patch.object(os, "open", side_effect=spy):
            memory_portable.apply_import(plan)
        self.assertEqual(len(opened), 1)

    def test_cli_yes_can_be_bound_to_the_dry_run_hash(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        dry = self.run_cli(env_b, "import", str(path), "--dry-run")
        sha = hashlib.sha256(path.read_bytes()).hexdigest()
        self.assertIn(sha, dry.stdout)
        before = tree(self.tmp)
        r = self.run_cli(env_b, "import", str(path), "--yes", "--sha256", "0" * 64)
        self.assertEqual(r.returncode, 2, r.stdout)
        self.assertEqual(tree(self.tmp), before)
        r = self.run_cli(env_b, "import", str(path), "--yes", "--sha256", sha)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue((anth_b / "personal-memory" / "user-me.md").exists())

    def test_forged_plan_entries_are_refused(self):
        path, _ = self.archive()
        env_b, anth_b = self.second_machine()
        before = tree(self.tmp)
        for mutate in (
            lambda p: p["new"][0].update(name="../escape.md"),
            lambda p: p["new"][0].update(name="MEMORY.md"),
            lambda p: p["new"][0].update(scope="elsewhere"),
            lambda p: p["new"][0].update(member="personal/never-in-archive.md", name="never-in-archive.md"),
            lambda p: p.update(home=str(self.tmp / "not-anthology")),
            lambda p: p.update(archive=str(self.tmp / "missing.tar.gz")),
            lambda p: p.update(repo_root=str(self.tmp), new=[
                dict(member="repo/memory/a.md", scope="repo", agent=None, name="a.md")]),
        ):
            plan = memory_portable.plan_import(path, self.cwd, env=env_b)
            mutate(plan)
            with self.assertRaises(memory_portable.ImportRejected):
                memory_portable.apply_import(plan)
        self.assertEqual(tree(self.tmp), before)

    def test_repo_parts_are_skipped_outside_an_approved_repo(self):
        path = make_archive(self.tmp / "m.tar.gz", [
            ("personal/user-p.md", b"personal\n"),
            ("repo/memory/project-r.md", b"repo\n"),
            ("repo/agent-memory/anthology-w/feedback-w.md", b"agent\n")])
        env_b, anth_b = self.second_machine()
        plan = memory_portable.plan_import(path, self.cwd, env=env_b)
        self.assertEqual(sorted(plan["skipped_repo"]),
                         ["repo/agent-memory/anthology-w/feedback-w.md", "repo/memory/project-r.md"])
        self.assertTrue(plan["messages"])
        memory_portable.apply_import(plan)
        self.assertEqual(sorted(p.name for p in (anth_b / "personal-memory").iterdir()),
                         ["MEMORY.md", "user-p.md"])
        self.assertEqual(list(self.cwd.iterdir()), [])

    def test_folder_members_are_allowed(self):
        with tarfile.open(str(self.tmp / "d.tar.gz"), "w:gz") as tar:
            add(tar, "manifest.json", MANIFEST)
            add(tar, "personal", type_=tarfile.DIRTYPE)
            add(tar, "personal/a.md", b"a\n")
        env_b, anth_b = self.second_machine()
        plan = memory_portable.plan_import(self.tmp / "d.tar.gz", self.cwd, env=env_b)
        memory_portable.apply_import(plan)
        self.assertEqual((anth_b / "personal-memory" / "a.md").read_bytes(), b"a\n")

    def run_cli(self, env, *args):
        return subprocess.run([sys.executable, str(SCRIPTS / "memory_portable.py")] + list(args),
                              cwd=str(self.cwd), env=cli_env(env), stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True)


class MaliciousArchiveTest(PortableCase):
    """Every case: rejected as a whole, and not one byte written anywhere."""

    GOOD = ("personal/good.md", b"good\n")

    def bad(self, kind):
        p = self.tmp / ("bad-%s.tar.gz" % kind)
        good = [self.GOOD]
        link = tarfile.SYMTYPE
        builders = {
            "dotdot": lambda: make_archive(p, good + [("../evil.md", b"x")]),
            "dotdot-inside": lambda: make_archive(p, good + [("personal/../../evil.md", b"x")]),
            "absolute": lambda: make_archive(p, good + [("/etc/evil.md", b"x")]),
            "symlink": lambda: make_archive(p, good + [("personal/link.md", dict(
                type_=link, linkname="/etc/passwd"))]),
            "hardlink": lambda: make_archive(p, good + [("personal/hard.md", dict(
                type_=tarfile.LNKTYPE, linkname="personal/good.md"))]),
            "chardev": lambda: make_archive(p, good + [("personal/dev.md", dict(type_=tarfile.CHRTYPE))]),
            "blockdev": lambda: make_archive(p, good + [("personal/blk.md", dict(type_=tarfile.BLKTYPE))]),
            "fifo": lambda: make_archive(p, good + [("personal/fifo.md", dict(type_=tarfile.FIFOTYPE))]),
            "symlink-folder": lambda: make_archive(p, good + [("personal", dict(
                type_=link, linkname="/etc"))]),
            "outside-settings": lambda: make_archive(p, good + [("repo/settings.json", b"{}")]),
            "outside-subdir": lambda: make_archive(p, good + [("personal/sub/x.md", b"x")]),
            "outside-agent-dotdot": lambda: make_archive(p, good + [("repo/agent-memory/../x.md", b"x")]),
            "outside-agent-no-folder": lambda: make_archive(p, good + [("repo/agent-memory/x.md", b"x")]),
            "outside-toplevel": lambda: make_archive(p, good + [("other/x.md", b"x")]),
            "outside-claude": lambda: make_archive(p, good + [(".claude/settings.json", b"{}")]),
            "outside-memory-md": lambda: make_archive(p, good + [("personal/MEMORY.md", b"x")]),
            "outside-memory-md-lower": lambda: make_archive(p, good + [("personal/memory.md", b"x")]),
            "outside-repo-memory-md": lambda: make_archive(p, good + [("repo/memory/MEMORY.md", b"x")]),
            "outside-not-md": lambda: make_archive(p, good + [("personal/x.txt", b"x")]),
            "outside-hidden": lambda: make_archive(p, good + [("personal/.x.md", b"x")]),
            "outside-bad-agent-name": lambda: make_archive(p, good + [("repo/agent-memory/.hid/x.md", b"x")]),
            "dot-component": lambda: make_archive(p, good + [("personal/./x.md", b"x")]),
            "empty-component": lambda: make_archive(p, good + [("personal//x.md", b"x")]),
            "newline-name": lambda: make_archive(p, good + [("personal/foo\n.md", b"x")]),
            "newline-agent": lambda: make_archive(p, good + [("repo/agent-memory/ag\n/bar.md", b"x")]),
            "newline-trailing": lambda: make_archive(p, good + [("personal/foo.md\n", b"x")]),
            "backslash": lambda: make_archive(p, good + [("personal\\x.md", b"x")]),
            "backslash-traversal": lambda: make_archive(p, good + [("personal\\..\\x.md", b"x")]),
            "duplicate": lambda: make_archive(p, good + [self.GOOD]),
            "non-utf8": lambda: make_archive(p, good + [("personal/bin.md", b"\xff\xfe\xfa")]),
            "no-manifest": lambda: make_archive(p, good, manifest=False),
            "bad-manifest-format": lambda: self.manifest_variant(p, b'{"format": 99}'),
            "bad-manifest-json": lambda: self.manifest_variant(p, b"not json"),
            "not-gzip": lambda: self.write_bytes(p, b"this is not an archive at all"),
            "empty-file": lambda: self.write_bytes(p, b""),
            "truncated": lambda: self.truncate(make_archive(p, good + [("personal/big.md", b"y" * 5000)])),
        }
        return builders[kind]()

    def manifest_variant(self, path, manifest):
        with tarfile.open(str(path), "w:gz") as tar:
            add(tar, "manifest.json", manifest)
            add(tar, *self.GOOD)
        return path

    def write_bytes(self, path, data):
        path.write_bytes(data)
        return path

    def truncate(self, path):
        data = path.read_bytes()
        path.write_bytes(data[:len(data) // 2])
        return path

    KINDS = ("dotdot dotdot-inside absolute symlink hardlink chardev blockdev fifo symlink-folder "
             "outside-settings outside-subdir outside-agent-dotdot outside-agent-no-folder "
             "outside-toplevel outside-claude outside-memory-md outside-memory-md-lower "
             "outside-repo-memory-md outside-not-md outside-hidden outside-bad-agent-name "
             "dot-component empty-component newline-name newline-agent newline-trailing backslash backslash-traversal duplicate non-utf8 "
             "no-manifest bad-manifest-format bad-manifest-json not-gzip empty-file truncated").split()

    def assert_rejected(self, path, label="", reason=None):
        env_b, anth_b = self.second_machine(os.path.basename(str(path)).replace(".", "_"))
        before = tree(self.tmp)
        with self.assertRaises(memory_portable.ImportRejected, msg=label) as ctx:
            memory_portable.plan_import(path, self.cwd, env=env_b)
        if reason:
            self.assertRegex(str(ctx.exception), reason, label)
        self.assertEqual(tree(self.tmp), before, label)
        self.assertEqual(tree(anth_b), {}, label)

    def test_every_malicious_archive_is_rejected_and_nothing_is_written(self):
        for kind in self.KINDS:
            with self.subTest(kind=kind):
                path = self.bad(kind)
                self.assert_rejected(path, kind)

    def test_rejected_through_the_cli_with_exit_code_2(self):
        for kind in ("dotdot", "symlink", "outside-settings", "non-utf8"):
            with self.subTest(kind=kind):
                path = self.bad(kind)
                env_b, anth_b = self.second_machine(kind)
                before = tree(self.tmp)
                r = subprocess.run([sys.executable, str(SCRIPTS / "memory_portable.py"), "import",
                                    str(path), "--yes"], cwd=str(self.cwd), env=cli_env(env_b),
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                self.assertIn("rejected", r.stderr)
                self.assertEqual(tree(self.tmp), before)

    def test_a_hand_forged_plan_pointing_at_a_malicious_archive_is_rejected_by_apply(self):
        """apply_import re-validates the archive itself; it never trusts a plan."""
        env_b, anth_b = self.second_machine("forged")
        good_entry = {"member": "personal/good.md", "scope": "personal", "agent": None, "name": "good.md"}
        for kind in ("dotdot", "symlink", "hardlink", "fifo", "outside-settings", "duplicate",
                     "non-utf8", "newline-name", "newline-agent", "backslash"):
            with self.subTest(kind=kind):
                path = self.bad(kind)
                before = tree(self.tmp)
                plan = {"archive": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                        "date": "2026-04-06", "home": str(anth_b), "repo_root": None,
                        "new": [dict(good_entry)], "identical": [], "conflicts": []}
                with self.assertRaises(memory_portable.ImportRejected):
                    memory_portable.apply_import(plan)
                self.assertEqual(tree(self.tmp), before)

    def test_names_with_a_trailing_newline_are_not_note_names(self):
        for bad in ("foo\n.md", "foo.md\n", "\nfoo.md"):
            self.assertFalse(memory_portable._ok_note_name(bad), repr(bad))
        self.assertIsNone(memory_portable.NAME.match("ag\n"))
        self.assertTrue(memory_portable._ok_note_name("foo.md"))

    def test_still_rejected_when_tarfile_has_no_data_filter(self):
        """macOS system Python 3.9.6 has no tarfile.data_filter: our own checks are the protection."""
        for kind in self.KINDS:
            with self.subTest(kind=kind):
                path = self.bad(kind)
                with mock.patch.object(tarfile, "data_filter", None, create=True):
                    self.assert_rejected(path, kind)

    def test_data_filter_is_applied_when_present(self):
        path = make_archive(self.tmp / "ok.tar.gz", [self.GOOD])
        env_b, anth_b = self.second_machine()
        seen = []

        def refuse(member, dest):
            seen.append((member.name, os.path.isabs(dest)))
            raise ValueError("filter says no")

        with mock.patch.object(tarfile, "data_filter", refuse, create=True):
            with self.assertRaises(memory_portable.ImportRejected):
                memory_portable.plan_import(path, self.cwd, env=env_b)
        self.assertTrue(seen)
        self.assertTrue(all(absolute for _name, absolute in seen))
        # and a filter that accepts leaves valid archives working
        with mock.patch.object(tarfile, "data_filter", lambda m, d: m, create=True):
            memory_portable.plan_import(path, self.cwd, env=env_b)

    def test_archive_file_over_50_mib_is_rejected_without_reading_it(self):
        path = self.tmp / "huge.tar.gz"
        with open(str(path), "wb") as fh:
            fh.truncate(50 * MIB + 1)  # sparse: costs no disk
        self.assert_rejected(path, reason="larger than 50 MiB")

    def test_total_size_over_50_mib_single_member(self):
        path = self.tmp / "inflate.tar.gz"
        with tarfile.open(str(path), "w:gz", compresslevel=1) as tar:
            add(tar, "manifest.json", MANIFEST)
            add(tar, "personal/big.md", ZeroFile(51 * MIB), size=51 * MIB)
        self.assertLess(path.stat().st_size, 5 * MIB)  # a small gzip that inflates big
        self.assert_rejected(path, reason="more than 50 MiB")

    def test_total_size_over_50_mib_many_members(self):
        path = self.tmp / "sum.tar.gz"
        with tarfile.open(str(path), "w:gz", compresslevel=1) as tar:
            add(tar, "manifest.json", MANIFEST)
            for i in range(9):
                add(tar, "personal/part%d.md" % i, ZeroFile(6 * MIB), size=6 * MIB)
        self.assertLess(path.stat().st_size, 5 * MIB)
        self.assert_rejected(path, reason="more than 50 MiB")

    def test_declared_size_is_checked_before_any_data_is_read(self):
        info = tarfile.TarInfo("personal/big.md")
        info.size = 5 * 1024 * MIB
        raw = io.BytesIO()
        tar = tarfile.open(fileobj=raw, mode="w")  # left open on purpose: no end-of-archive blocks
        add(tar, "manifest.json", MANIFEST)
        header = raw.getvalue() + info.tobuf(tarfile.GNU_FORMAT, "utf-8", "surrogateescape")
        path = self.tmp / "declared.tar.gz"
        with gzip.open(str(path), "wb") as gz:
            gz.write(header)
        self.assert_rejected(path, reason="more than 50 MiB")

    def test_oversized_extended_header_is_not_read_into_memory(self):
        info = tarfile.TarInfo("PaxHeader/x")
        info.type = tarfile.XHDTYPE
        info.size = 80 * MIB
        path = self.tmp / "paxbomb.tar.gz"
        with gzip.GzipFile(str(path), "wb", compresslevel=1) as gz:
            gz.write(info.tobuf(tarfile.GNU_FORMAT, "utf-8", "surrogateescape"))
            gz.write(b"\0" * (80 * MIB))
        self.assertLess(path.stat().st_size, 5 * MIB)
        self.assert_rejected(path, reason="size limit")

    def test_capped_reader_never_reads_far_past_its_limit(self):
        source = io.BytesIO(b"\0" * 1000)
        capped = memory_portable._CappedReader(source, 50)
        with self.assertRaises(memory_portable.ImportRejected):
            capped.read(10 ** 9)  # a hostile header asking for a huge read
        self.assertLessEqual(source.tell(), 51)
        source = io.BytesIO(b"\0" * 1000)
        capped = memory_portable._CappedReader(source, 50)
        with self.assertRaises(memory_portable.ImportRejected):
            capped.read()  # read-everything
        self.assertLessEqual(source.tell(), 51)
        with self.assertRaises(memory_portable.ImportRejected):
            memory_portable._CappedReader(io.BytesIO(b"\0" * 1000), 50).seek(51)
        capped = memory_portable._CappedReader(io.BytesIO(b"abc" * 30), 50)
        self.assertEqual(capped.read(10), b"abcabcabca")  # normal reads are untouched

    def test_headers_that_the_member_count_does_not_see_are_capped_too(self):
        """Extended headers are consumed by tarfile, not listed as members: the stream cap bounds them."""
        path = self.tmp / "paxflood.tar.gz"
        with tarfile.open(str(path), "w:gz", compresslevel=1, format=tarfile.PAX_FORMAT) as tar:
            add(tar, "manifest.json", MANIFEST)
            for i in range(8000):
                info = tarfile.TarInfo("personal/n%04d.md" % i)
                info.size = 1
                info.pax_headers = {"comment": "x" * 8192}
                tar.addfile(info, io.BytesIO(b"x"))
        self.assertLess(path.stat().st_size, 20 * MIB)
        self.assert_rejected(path, reason="size limit")

    def test_more_than_10000_members(self):
        path = self.tmp / "many.tar.gz"
        with tarfile.open(str(path), "w:gz") as tar:
            add(tar, "manifest.json", MANIFEST)
            for i in range(10001):
                add(tar, "personal/n%05d.md" % i, b"x")
        self.assert_rejected(path, reason="more than 10000 members")

    def test_exactly_10000_members_is_fine(self):
        path = self.tmp / "ok-many.tar.gz"
        with tarfile.open(str(path), "w:gz") as tar:
            add(tar, "manifest.json", MANIFEST)
            for i in range(9999):
                add(tar, "personal/n%05d.md" % i, b"x")
        env_b, _ = self.second_machine()
        plan = memory_portable.plan_import(path, self.cwd, env=env_b)
        self.assertEqual(len(plan["new"]), 9999)


@needs_git
class DestinationTest(PortableCase):
    def setUp(self):
        super().setUp()
        self.repo = self.make_repo()
        memory_sync.approve(self.repo)
        self.archive_path = make_archive(self.tmp / "m.tar.gz", [
            ("personal/user-p.md", b"personal\n"),
            ("repo/memory/project-r.md", b"repo\n"),
            ("repo/agent-memory/anthology-w/feedback-w.md", b"agent\n")])
        self.outside = self.tmp / "outside"
        self.outside.mkdir()

    def plan(self):
        return memory_portable.plan_import(self.archive_path, self.repo, today="2026-04-06", env=self.env)

    def test_round_trip_into_an_approved_repo(self):
        plan = self.plan()
        self.assertEqual(len(plan["new"]), 3)
        self.assertEqual(plan["skipped_repo"], [])
        result = memory_portable.apply_import(plan)
        self.assertEqual(len(result["written"]), 3)
        self.assertEqual((self.repo / ".claude" / "memory" / "project-r.md").read_bytes(), b"repo\n")
        agent = self.repo / ".claude" / "agent-memory" / "anthology-w"
        self.assertEqual((agent / "feedback-w.md").read_bytes(), b"agent\n")
        self.assertEqual((self.personal / "user-p.md").read_bytes(), b"personal\n")
        for d in (self.repo / ".claude" / "memory", agent, self.personal):
            self.assertTrue((d / "MEMORY.md").is_file(), d)
        self.assertIn("[project-r](project-r.md)", (self.repo / ".claude" / "memory" / "MEMORY.md").read_text())

    def test_full_export_import_round_trip_between_two_repos(self):
        write(self.repo / ".claude" / "memory" / "project-r.md", note_text("project-r", "project", "R", "2026-02-02"))
        write(self.repo / ".claude" / "agent-memory" / "anthology-w" / "feedback-w.md",
              note_text("feedback-w", "feedback", "W"))
        write(self.personal / "user-p.md", note_text("user-p", "user", "P"))
        exported = Path(memory_portable.export(self.repo, self.out, today="2026-04-05", env=self.env)["path"])
        env_b, anth_b = self.second_machine()
        repo_b = self.make_repo("repo-b")
        memory_sync.approve(repo_b, env=env_b)
        plan = memory_portable.plan_import(exported, repo_b, env=env_b)
        memory_portable.apply_import(plan)
        pairs = [(self.repo / ".claude" / "memory" / "project-r.md", repo_b / ".claude" / "memory" / "project-r.md"),
                 (self.repo / ".claude" / "agent-memory" / "anthology-w" / "feedback-w.md",
                  repo_b / ".claude" / "agent-memory" / "anthology-w" / "feedback-w.md"),
                 (self.personal / "user-p.md", anth_b / "personal-memory" / "user-p.md")]
        for a, b in pairs:
            self.assertEqual(a.read_bytes(), b.read_bytes())

    def snapshot_everything(self):
        return tree(self.tmp)

    def test_symlinked_destinations_are_refused_and_nothing_is_written(self):
        cases = {
            "memory": lambda: (self.repo / ".claude").mkdir() or
                              (self.repo / ".claude" / "memory").symlink_to(self.outside),
            "dot-claude": lambda: (self.repo / ".claude").symlink_to(self.outside),
            "agent-memory": lambda: (self.repo / ".claude").mkdir() or
                                    (self.repo / ".claude" / "agent-memory").symlink_to(self.outside),
            "agent-folder": lambda: (self.repo / ".claude" / "agent-memory").mkdir(parents=True) or
                                    (self.repo / ".claude" / "agent-memory" / "anthology-w").symlink_to(self.outside),
            "personal": lambda: (self.anth.mkdir(parents=True, exist_ok=True) or
                                 self.personal.symlink_to(self.outside)),
        }
        for name, setup in cases.items():
            with self.subTest(case=name):
                setup()
                try:
                    before = self.snapshot_everything()
                    plan = self.plan()
                    with self.assertRaises(memory_portable.ImportRejected):
                        memory_portable.apply_import(plan)
                    self.assertEqual(self.snapshot_everything(), before)
                    self.assertEqual(list(self.outside.iterdir()), [])
                finally:  # undo for the next case
                    if self.personal.is_symlink():
                        self.personal.unlink()
                    if (self.repo / ".claude").is_symlink():
                        (self.repo / ".claude").unlink()
                    shutil.rmtree(str(self.repo / ".claude"), ignore_errors=True)

    def test_destination_note_that_is_a_symlink_is_never_followed(self):
        target = self.tmp / "target.md"
        write(target, "precious\n")
        mem = self.repo / ".claude" / "memory"
        mem.mkdir(parents=True)
        (mem / "project-r.md").symlink_to(target)
        plan = self.plan()
        self.assertEqual([e["member"] for e in plan["conflicts"]], ["repo/memory/project-r.md"])
        memory_portable.apply_import(plan)
        self.assertEqual(target.read_text(), "precious\n")
        self.assertTrue((mem / "project-r.imported-2026-04-06.md").is_file())

    def test_repo_must_still_be_approved_at_apply_time(self):
        plan = self.plan()
        memory_sync.unapprove(memory_sync.repo_key(self.repo))
        before = self.snapshot_everything()
        with self.assertRaises(memory_portable.ImportRejected):
            memory_portable.apply_import(plan)
        self.assertEqual(self.snapshot_everything(), before)

    def test_unapproved_repo_gets_personal_notes_only(self):
        other = self.make_repo("other")
        plan = memory_portable.plan_import(self.archive_path, other, env=self.env)
        self.assertEqual(len(plan["skipped_repo"]), 2)
        memory_portable.apply_import(plan)
        self.assertFalse((other / ".claude").exists())
        self.assertTrue((self.personal / "user-p.md").exists())


class CliTest(PortableCase):
    def run_cli(self, *args, env=None):
        return subprocess.run([sys.executable, str(SCRIPTS / "memory_portable.py")] + list(args),
                              cwd=str(self.cwd), env=cli_env(env or self.env), stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True)

    def test_export_then_import_through_the_cli(self):
        self.seed_personal()
        samples = secret_samples()
        write(self.personal / "user-keys.md", note_text("user-keys", "user", body=samples["slack-token"] + "\n"))
        r = self.run_cli("export", "--out", str(self.out))
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("personal: 3", r.stdout)
        self.assertIn("slack-token", r.stdout)
        self.assertNotIn(samples["slack-token"], r.stdout + r.stderr)
        self.assertIn("personal notes", r.stdout)
        archive = next(self.out.glob("anthology-memory-*.tar.gz"))
        r = self.run_cli("export", "--json")  # default out: <anthology home>/exports, never the cwd
        data = json.loads(r.stdout)
        self.assertEqual(Path(data["path"]).parent, self.anth / "exports")
        self.assertEqual(list(self.cwd.iterdir()), [])
        self.assertEqual(data["counts"]["personal"], 3)
        env_b, anth_b = self.second_machine()
        r = self.run_cli("import", str(archive), "--json", env=env_b)
        plan = json.loads(r.stdout)["plan"]
        self.assertEqual(len(plan["new"]), 3)
        self.assertNotIn(samples["slack-token"], r.stdout)
        self.assertFalse((anth_b / "personal-memory").exists())
        r = self.run_cli("import", str(archive), "--yes", env=env_b)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual((anth_b / "personal-memory" / "user-me.md").read_bytes(),
                         (self.personal / "user-me.md").read_bytes())

    def test_missing_file_is_an_error_not_a_traceback(self):
        r = self.run_cli("import", str(self.tmp / "nope.tar.gz"))
        self.assertEqual(r.returncode, 2)
        self.assertNotIn("Traceback", r.stderr)

    def second_machine(self, name="b"):
        home = self.tmp / name / "home"
        home.mkdir(parents=True, exist_ok=True)
        return (dict(os.environ, HOME=str(home), CLAUDE_CONFIG_DIR=str(home / ".claude")),
                home / ".claude" / "anthology")


class SkillsTest(unittest.TestCase):
    SKILLS = ROOT / "plugins" / "anthology" / "skills"

    def frontmatter(self, name):
        text = (self.SKILLS / name / "SKILL.md").read_text(encoding="utf-8")
        match = re.match(r"^---\n(.*?)\n---\n", text, re.S)
        self.assertIsNotNone(match, name)
        fields = {}
        for line in match.group(1).splitlines():
            key, sep, value = line.partition(":")
            if sep:
                fields[key.strip()] = value.strip()
        return fields, text[match.end():]

    def test_export_and_import_skills(self):
        for name in ("export", "import"):
            fields, body = self.frontmatter(name)
            self.assertEqual(fields.get("name"), name)
            self.assertTrue(fields.get("description"))
            self.assertEqual(fields.get("disable-model-invocation"), "true", name)
            self.assertIn('python3 "${CLAUDE_PLUGIN_ROOT}/scripts/memory_portable.py"', body)

    def test_skill_bodies_follow_the_flow(self):
        _, export_body = self.frontmatter("export")
        self.assertIn("personal notes", export_body)
        self.assertIn("counts", export_body)
        self.assertIn("exports", export_body)
        self.assertIn("never", export_body.lower())
        self.assertNotIn("add `--out <folder>`", export_body)
        _, import_body = self.frontmatter("import")
        for needle in ("--dry-run", "--yes", "--sha256", ".imported-", "confirmation", "delete"):
            self.assertIn(needle, import_body)
        self.assertLess(import_body.index("--dry-run"), import_body.index("--yes"))


if __name__ == "__main__":
    unittest.main()
