"""Tests for scripts/memory_sync.py (repo memory: trust boundary, repairs, derived indexes).

Cases that need git skip when it is missing (the Docker test images have none); the CI
`memory-git` job and the host run them. Everything else (URL normalisation, index
generation, migration, approvals) runs everywhere.
"""
import json
import os
import re
import stat
import subprocess
import sys
import threading
import unittest
from pathlib import Path
from unittest import mock

from _mem import ROOT, MemCase, git, needs_git, note_text, snapshot, write

import memory_index  # noqa: E402  (path set up by _mem)
import memory_privacy  # noqa: E402
import memory_sync  # noqa: E402

sys.path.insert(0, str(ROOT / "plugins" / "anthology-guardrails" / "scripts"))
import guard_rules  # noqa: E402

HINT = ("this repo has memory notes; repo notes load only after /anthology:setup, "
        "but notes in .claude/agent-memory still reach their agents, so review them")
UNION_LINES = [".claude/memory/MEMORY.md merge=union",
               ".claude/agent-memory/*/MEMORY.md merge=union"]


def encode(path):
    return re.sub(r"[^A-Za-z0-9]", "-", os.path.realpath(str(path)))


def env():
    return dict(os.environ)


def personal_only(anth):
    """The context when personal memory has no notes: its section is still there, with its path
    and how to save user notes there (C1)."""
    personal = anth / "personal-memory"
    return ("## Personal memory (%s)\nSave notes about the user (type: user: role, preferences) here "
            "as %s/user-<slug>.md, never in the repo.\n\n(no notes yet)\n" % (personal, personal))


class NormalizeRemoteTest(unittest.TestCase):
    """repo_key's URL normalisation (R4); pure strings, no git."""

    def test_forms_that_name_the_same_repo(self):
        for url in ("git@github.com:Acme/Repo.git",
                    "https://github.com/acme/repo",
                    "ssh://git@github.com/acme/repo.git/",
                    "https://user:tok@github.com/acme/repo",
                    "http://github.com/ACME/REPO.git",
                    "git://github.com/acme/repo",
                    "ssh://git@github.com:22/acme/repo.git",
                    "https://github.com:443/acme/repo/",
                    "github.com:acme/repo"):
            self.assertEqual(memory_sync.normalize_remote(url), "github.com/acme/repo", url)

    def test_credentials_never_survive(self):
        key = memory_sync.normalize_remote("https://me:s3cret-token@git.example.invalid/a/b.git")
        self.assertEqual(key, "git.example.invalid/a/b")
        self.assertNotIn("s3cret", key)
        self.assertNotIn("me", key.split("/")[0])

    def test_non_default_port_is_kept(self):
        self.assertEqual(memory_sync.normalize_remote("ssh://git@host.example.invalid:2222/a/b.git"),
                         "host.example.invalid:2222/a/b")

    def test_local_paths_become_path_keys(self):
        tmp = os.path.realpath(os.environ.get("TMPDIR", "/tmp"))
        self.assertEqual(memory_sync.normalize_remote("/srv/x.git"),
                         "path:" + os.path.realpath("/srv/x"))
        self.assertEqual(memory_sync.normalize_remote("file:///srv/x.git"),
                         "path:" + os.path.realpath("/srv/x"))
        self.assertTrue(memory_sync.normalize_remote(tmp + "/y.git").startswith("path:"))

    def test_scp_form_with_absolute_path_is_not_a_local_path(self):
        self.assertEqual(memory_sync.normalize_remote("git@host.example.invalid:/srv/x.git"),
                         "host.example.invalid/srv/x")


@needs_git
class RepoKeyTest(MemCase):
    def test_origin_url_is_normalised(self):
        root = self.make_repo(remote="https://user:tok@github.com/Acme/Repo.git")
        self.assertEqual(memory_sync.repo_key(root), "github.com/acme/repo")

    def test_origin_wins_over_other_remotes(self):
        root = self.make_repo(remote="https://github.com/acme/repo")
        git(root, "remote", "add", "aaa", "https://github.com/other/thing")
        self.assertEqual(memory_sync.repo_key(root), "github.com/acme/repo")

    def test_alphabetically_first_remote_without_origin(self):
        root = self.make_repo()
        git(root, "remote", "add", "zeta", "https://github.com/z/z")
        git(root, "remote", "add", "alpha", "https://github.com/a/a")
        self.assertEqual(memory_sync.repo_key(root), "github.com/a/a")

    def test_no_remote_is_the_realpath(self):
        root = self.make_repo()
        self.assertEqual(memory_sync.repo_key(root), "path:" + str(root))

    def test_linked_worktree_shares_the_key(self):
        root = self.make_repo()
        write(root / "f", "x")
        self.commit_all(root)
        wt = self.tmp / "wt"
        git(root, "worktree", "add", "-q", str(wt), "-b", "wt")
        self.assertEqual(memory_sync.repo_key(wt), memory_sync.repo_key(root))
        self.assertEqual(memory_sync.repo_key(wt), "path:" + str(root))

    def test_not_a_repo_falls_back_to_the_path(self):
        plain = self.tmp / "plain"
        plain.mkdir()
        self.assertEqual(memory_sync.repo_key(plain), "path:" + str(plain))


class ApprovalTest(MemCase):
    def test_approve_records_key_and_adds_union_attrs(self):
        root = self.tmp / "r"
        root.mkdir()
        key = memory_sync.approve(root)
        self.assertEqual(key, "path:" + str(root))
        self.assertTrue(memory_sync.is_approved(key))
        data = json.loads((self.anth / "approved-repos.json").read_text())
        self.assertEqual(data["version"], 1)
        self.assertRegex(data["repos"][key]["approved"], r"^\d{4}-\d{2}-\d{2}$")
        attrs = (root / ".gitattributes").read_text().splitlines()
        for line in UNION_LINES:
            self.assertIn(line, attrs)

    def test_unapprove(self):
        root = self.tmp / "r"
        root.mkdir()
        key = memory_sync.approve(root)
        self.assertTrue(memory_sync.unapprove(key))
        self.assertFalse(memory_sync.is_approved(key))
        self.assertFalse(memory_sync.unapprove(key))

    def test_invalid_file_means_nothing_approved_and_approve_recovers(self):
        write(self.anth / "approved-repos.json", "{not json")
        self.assertFalse(memory_sync.is_approved("anything"))
        write(self.anth / "approved-repos.json", json.dumps(["list"]))
        self.assertFalse(memory_sync.is_approved("anything"))
        root = self.tmp / "r"
        root.mkdir()
        key = memory_sync.approve(root)
        self.assertTrue(memory_sync.is_approved(key))

    def test_no_temp_files_left_behind(self):
        root = self.tmp / "r"
        root.mkdir()
        memory_sync.approve(root)
        self.assertEqual([p.name for p in self.anth.iterdir()], ["approved-repos.json"])

    @needs_git
    def test_credentials_are_never_stored(self):
        root = self.make_repo(remote="https://user:tok3n@github.com/Acme/Repo.git")
        key = memory_sync.approve(root)
        self.assertEqual(key, "github.com/acme/repo")
        self.assertNotIn("tok3n", (self.anth / "approved-repos.json").read_text())


class UnionAttrsTest(MemCase):
    def test_creates_and_is_idempotent(self):
        root = self.tmp / "r"
        root.mkdir()
        self.assertTrue(memory_sync.ensure_union_attrs(root))
        first = (root / ".gitattributes").read_text()
        self.assertEqual(first.splitlines(), UNION_LINES)
        self.assertTrue(first.endswith("\n"))
        self.assertFalse(memory_sync.ensure_union_attrs(root))
        self.assertEqual((root / ".gitattributes").read_text(), first)

    def test_keeps_existing_content_and_adds_missing_newline(self):
        root = self.tmp / "r"
        root.mkdir()
        write(root / ".gitattributes", "*.png binary")  # no trailing newline
        self.assertTrue(memory_sync.ensure_union_attrs(root))
        self.assertEqual((root / ".gitattributes").read_text().splitlines(),
                         ["*.png binary"] + UNION_LINES)

    def test_adds_only_the_missing_line(self):
        root = self.tmp / "r"
        root.mkdir()
        write(root / ".gitattributes", UNION_LINES[0] + "\n")
        self.assertTrue(memory_sync.ensure_union_attrs(root))
        self.assertEqual((root / ".gitattributes").read_text().splitlines(), UNION_LINES)


class RegenerateIndexTest(MemCase):
    def mem(self, notes):
        d = self.tmp / "mem"
        for fname, text in notes.items():
            write(d / fname, text)
        return d

    def test_groups_by_type_newest_first(self):
        d = self.mem({
            "project-old.md": note_text("project-old", "project", "Old thing", "2026-01-01"),
            "project-new.md": note_text("project-new", "project", "New thing", "2026-03-01"),
            "user-me.md": note_text("user-me", "user", "About me", "2026-02-01"),
            "feedback-x.md": note_text("feedback-x", "feedback", "Do x", "2026-02-02"),
            "ref-a.md": note_text("ref-a", "reference", "Where a", None),
            "zed.md": note_text("zed", "zeta", "Custom type", None),
            "alpha.md": note_text("alpha", "aardvark", "Other custom", None),
        })
        self.assertTrue(memory_sync.regenerate_index(d))
        lines = (d / "MEMORY.md").read_text().splitlines()
        self.assertTrue(lines[0].startswith("<!-- Generated by anthology"))
        self.assertEqual([l for l in lines if l.startswith("## ")],
                         ["## user", "## feedback", "## project", "## reference",
                          "## aardvark", "## zeta"])
        self.assertIn("- [project-new](project-new.md) — New thing", lines)
        self.assertLess(lines.index("- [project-new](project-new.md) — New thing"),
                        lines.index("- [project-old](project-old.md) — Old thing"))

    def test_dated_before_undated_ties_by_name(self):
        d = self.mem({
            "b.md": note_text("b", "project", "b", None),
            "a.md": note_text("a", "project", "a", None),
            "c.md": note_text("c", "project", "c", "2026-01-01"),
            "d.md": note_text("d", "project", "d", "2026-01-01"),
        })
        memory_sync.regenerate_index(d)
        entries = [l for l in (d / "MEMORY.md").read_text().splitlines() if l.startswith("- [")]
        self.assertEqual([e.split("]")[0][3:] for e in entries], ["c", "d", "a", "b"])

    def test_uses_name_over_stem_and_top_level_fields(self):
        d = self.mem({"file-stem.md": note_text("Pretty Name", "feedback", "d", "2026-05-05",
                                                 top_level_type=True)})
        memory_sync.regenerate_index(d)
        text = (d / "MEMORY.md").read_text()
        self.assertIn("## feedback", text)
        self.assertIn("- [Pretty Name](file-stem.md) — d", text)

    def test_unparseable_frontmatter_is_listed_by_stem(self):
        d = self.mem({"broken.md": "---\nname: [unclosed\n  : :\n", "plain.md": "no frontmatter\n"})
        memory_sync.regenerate_index(d)
        text = (d / "MEMORY.md").read_text()
        self.assertIn("## other", text)
        self.assertIn("[broken](broken.md)", text)
        self.assertIn("[plain](plain.md)", text)

    def test_skips_memory_md_symlinks_and_non_md(self):
        d = self.mem({"a.md": note_text("a")})
        write(d / "notes.txt", "x")
        (d / "link.md").symlink_to(d / "a.md")
        write(d / "MEMORY.md", "stale generated content that is not a note\n")
        memory_sync.regenerate_index(d)
        text = (d / "MEMORY.md").read_text()
        self.assertEqual(text.count("](a.md)"), 1)
        self.assertNotIn("link", text)
        self.assertNotIn("notes.txt", text)
        self.assertNotIn("stale generated", text)

    def test_descriptions_are_single_lined_and_lines_capped(self):
        d = self.mem({"a.md": "---\nname: a\ndescription: \"first\\nsecond   spaced\"\n---\nx\n",
                      "b.md": note_text("b", desc="w" * 500)})
        memory_sync.regenerate_index(d)
        lines = (d / "MEMORY.md").read_text().splitlines()
        self.assertTrue(all(len(l) <= 200 for l in lines))
        self.assertFalse(any(l.startswith("second") for l in lines))

    def test_deterministic_across_creation_order_and_mtimes(self):
        notes = {f"n{i:02d}.md": note_text(f"n{i:02d}", ("project", "feedback", "user")[i % 3],
                                          f"desc {i}", f"2026-01-{(i % 28) + 1:02d}")
                 for i in range(30)}
        a = self.tmp / "a"
        b = self.tmp / "b"
        for k in sorted(notes):
            write(a / k, notes[k])
        for n, k in enumerate(sorted(notes, reverse=True)):
            write(b / k, notes[k])
            os.utime(b / k, (1_000_000 + n * 1000, 1_000_000 + n * 1000))
        memory_sync.regenerate_index(a)
        memory_sync.regenerate_index(b)
        self.assertEqual((a / "MEMORY.md").read_bytes(), (b / "MEMORY.md").read_bytes())

    def test_generated_text_has_no_paths_or_times(self):
        d = self.mem({"a.md": note_text("a")})
        memory_sync.regenerate_index(d)
        text = (d / "MEMORY.md").read_text()
        self.assertNotIn(str(self.tmp), text)

    def test_unchanged_run_writes_nothing(self):
        d = self.mem({"a.md": note_text("a"), "b.md": note_text("b", "user")})
        self.assertTrue(memory_sync.regenerate_index(d))
        before = (d / "MEMORY.md").stat().st_mtime_ns
        os.utime(d / "MEMORY.md", ns=(before - 5_000_000_000, before - 5_000_000_000))
        before = (d / "MEMORY.md").stat().st_mtime_ns
        self.assertFalse(memory_sync.regenerate_index(d))
        self.assertEqual((d / "MEMORY.md").stat().st_mtime_ns, before)
        write(d / "c.md", note_text("c"))
        self.assertTrue(memory_sync.regenerate_index(d))
        self.assertIn("](c.md)", (d / "MEMORY.md").read_text())

    def test_no_notes_writes_nothing(self):
        d = self.tmp / "empty"
        d.mkdir()
        self.assertFalse(memory_sync.regenerate_index(d))
        self.assertFalse((d / "MEMORY.md").exists())
        write(d / "MEMORY.md", "keep me\n")
        self.assertFalse(memory_sync.regenerate_index(d))
        self.assertEqual((d / "MEMORY.md").read_text(), "keep me\n")
        self.assertFalse(memory_sync.regenerate_index(self.tmp / "missing"))

    def test_300_notes_stay_within_budget_with_more_pointers(self):
        notes = {}
        types = ("feedback", "project", "reference")
        for i in range(300):
            t = types[i % 3]
            notes[f"{t}-{i:03d}.md"] = note_text(f"{t}-{i:03d}", t, f"Description number {i}",
                                                 f"2026-{(i // 28) % 12 + 1:02d}-{i % 28 + 1:02d}")
        d = self.mem(notes)
        memory_sync.regenerate_index(d)
        raw = (d / "MEMORY.md").read_bytes()
        lines = raw.decode().splitlines()
        budget = memory_sync.DEFAULT_BUDGET
        self.assertEqual((budget.lines, budget.bytes), (150, 20000))
        self.assertLessEqual(len(lines), budget.lines)
        self.assertLessEqual(len(raw), budget.bytes)
        pointers = [l for l in lines if re.match(r"^- \d+ more — use `anthology-mem search`$", l)]
        self.assertEqual(len(pointers), 3)
        # every type keeps entries, and shown + hidden == 100 per type
        shown = {}
        current = None
        for l in lines:
            if l.startswith("## "):
                current = l[3:]
                shown[current] = 0
            elif l.startswith("- ["):
                shown[current] += 1
        hidden = {}
        current = None
        for l in lines:
            if l.startswith("## "):
                current = l[3:]
            m = re.match(r"^- (\d+) more", l)
            if m:
                hidden[current] = int(m.group(1))
        for t in types:
            self.assertGreater(shown[t], 0)
            self.assertEqual(shown[t] + hidden[t], 100)
        self.assertLess(max(shown.values()) - min(shown.values()), 2)

    def test_most_recent_notes_survive_truncation(self):
        notes = {f"p{i:03d}.md": note_text(f"p{i:03d}", "project", "d", f"2026-01-{i % 28 + 1:02d}")
                 for i in range(60)}
        notes["newest.md"] = note_text("newest", "project", "d", "2030-01-01")
        d = self.mem(notes)
        memory_sync.regenerate_index(d, memory_sync.Budget(10, 20000))
        lines = (d / "MEMORY.md").read_text().splitlines()
        self.assertLessEqual(len(lines), 10)
        self.assertIn("[newest](newest.md)", lines[2])
        self.assertTrue(lines[-1].endswith("more — use `anthology-mem search`"))

    def test_byte_budget_is_enforced(self):
        notes = {f"n{i:03d}.md": note_text(f"n{i:03d}", "project", "x" * 170, f"2026-01-{i % 28 + 1:02d}")
                 for i in range(120)}
        d = self.mem(notes)
        memory_sync.regenerate_index(d)
        raw = (d / "MEMORY.md").read_bytes()
        self.assertLessEqual(len(raw), 20000)
        self.assertLessEqual(len(raw.decode().splitlines()), 150)
        self.assertIn("more — use `anthology-mem search`", raw.decode())


class MigrateTest(MemCase):
    """B3: legacy notes go to personal memory; only an explicit, clean promotion reaches the repo."""

    def setUp(self):
        super().setUp()
        self.old = self.tmp / "old"
        self.repo_mem = self.tmp / "repo" / ".claude" / "memory"
        self.personal = self.anth / "personal-memory"
        self.ledger = self.anth / "migrated.json"
        write(self.old / "user-me.md", note_text("user-me", "user", "me"))
        write(self.old / "feedback-a.md", note_text("feedback-a", "feedback", "a"))
        write(self.old / "project-b.md", note_text("project-b", "project", "b"))
        write(self.old / "MEMORY.md", "- index\n")

    def migrate(self, **kw):
        kw.setdefault("hostname", "alice-laptop")
        return memory_sync.migrate(self.old, self.repo_mem, self.personal, ledger=self.ledger,
                                   today="2026-04-05", **kw)

    def test_every_note_goes_to_personal_memory(self):
        result = self.migrate()
        self.assertEqual(sorted(result["copied"]), ["feedback-a.md", "project-b.md", "user-me.md"])
        self.assertEqual(sorted(p.name for p in self.personal.iterdir()),
                         ["feedback-a.md", "project-b.md", "user-me.md"])
        self.assertFalse(self.repo_mem.exists())
        self.assertEqual((result["promoted"], result["blocked"]), ([], []))
        self.assertEqual((self.personal / "project-b.md").read_bytes(),
                         (self.old / "project-b.md").read_bytes())
        self.assertEqual(stat.S_IMODE(self.personal.stat().st_mode), 0o700)

    def test_memory_md_and_symlinks_are_not_copied(self):
        outside = self.tmp / "secret.md"
        write(outside, "---\nname: s\n---\nsecret\n")
        (self.old / "link.md").symlink_to(outside)
        self.migrate(promote={"link.md"})
        names = {p.name for p in self.personal.iterdir()}
        self.assertNotIn("MEMORY.md", names)
        self.assertNotIn("link.md", names)
        self.assertFalse(self.repo_mem.exists())

    def test_second_run_copies_nothing(self):
        self.migrate()
        again = self.migrate()
        self.assertEqual(again["copied"], [])
        self.assertEqual(again["conflicts"], [])
        self.assertEqual(sorted(again["skipped"]), ["feedback-a.md", "project-b.md", "user-me.md"])

    def test_never_overwrites_differing_files(self):
        write(self.personal / "project-b.md", "mine, edited since\n")
        result = self.migrate()
        self.assertEqual((self.personal / "project-b.md").read_text(), "mine, edited since\n")
        self.assertEqual(result["conflicts"], ["project-b.md"])
        copy = self.personal / "project-b.migrated-2026-04-05.md"
        self.assertEqual(copy.read_bytes(), (self.old / "project-b.md").read_bytes())

    def test_conflict_name_is_made_unique(self):
        write(self.personal / "project-b.md", "mine\n")
        write(self.personal / "project-b.migrated-2026-04-05.md", "taken\n")
        self.migrate()
        self.assertEqual((self.personal / "project-b.migrated-2026-04-05.md").read_text(), "taken\n")
        self.assertEqual((self.personal / "project-b.migrated-2026-04-05-2.md").read_bytes(),
                         (self.old / "project-b.md").read_bytes())

    def test_identical_existing_file_is_skipped(self):
        write(self.personal / "project-b.md", (self.old / "project-b.md").read_text())
        result = self.migrate()
        self.assertIn("project-b.md", result["skipped"])
        self.assertEqual(result["conflicts"], [])

    def test_ledger_stops_recopying_deleted_notes(self):
        self.migrate()
        (self.personal / "project-b.md").unlink()
        again = self.migrate()
        self.assertNotIn("project-b.md", again["copied"])
        self.assertFalse((self.personal / "project-b.md").exists())
        data = json.loads(self.ledger.read_text())
        self.assertEqual(data["version"], 1)
        self.assertEqual(len(data["done"]), 3)
        self.assertTrue(all(re.fullmatch(r"[0-9a-f]{64}", h) for h in data["done"]))

    def test_without_a_ledger_a_deleted_note_is_copied_again_but_never_duplicated(self):
        memory_sync.migrate(self.old, self.repo_mem, self.personal, today="2026-04-05")
        again = memory_sync.migrate(self.old, self.repo_mem, self.personal, today="2026-04-05")
        self.assertEqual(again["copied"], [])
        self.assertEqual(again["conflicts"], [])
        self.assertEqual(len(list(self.personal.iterdir())), 3)

    def test_missing_old_dir_is_a_noop(self):
        result = memory_sync.migrate(self.tmp / "nope", self.repo_mem, self.personal,
                                     ledger=self.ledger)
        self.assertEqual(result, {"copied": [], "skipped": [], "conflicts": [], "promoted": [],
                                  "blocked": []})
        self.assertFalse(self.repo_mem.exists())
        self.assertFalse(self.personal.exists())

    def test_a_clean_promoted_note_goes_to_the_repo(self):
        result = self.migrate(promote={"project-b.md"})
        self.assertEqual(result["promoted"], ["project-b.md"])
        self.assertEqual(sorted(p.name for p in self.repo_mem.iterdir()), ["project-b.md"])
        self.assertEqual((self.repo_mem / "project-b.md").read_bytes(),
                         (self.old / "project-b.md").read_bytes())
        self.assertEqual(sorted(p.name for p in self.personal.iterdir()), ["feedback-a.md", "user-me.md"])
        self.assertEqual(len(json.loads(self.ledger.read_text())["done"]), 3)

    def test_promotion_ignores_the_ledger_but_never_overwrites(self):
        self.migrate()  # the automatic copy: everything personal, all in the ledger
        write(self.repo_mem / "project-b.md", "the repo's own\n")
        result = self.migrate(promote={"project-b.md"})
        self.assertEqual(result["promoted"], ["project-b.md"])
        self.assertEqual(result["conflicts"], ["project-b.md"])
        self.assertEqual((self.repo_mem / "project-b.md").read_text(), "the repo's own\n")
        self.assertTrue((self.repo_mem / "project-b.migrated-2026-04-05.md").is_file())
        self.assertTrue((self.personal / "project-b.md").is_file())  # never deleted

    def test_promoted_notes_with_findings_or_type_user_stay_personal(self):
        write(self.old / "reference-host.md", note_text("reference-host", "reference", "Host",
                                                        body="Runs on build-box.lan\n"))
        result = self.migrate(promote={"reference-host.md", "user-me.md"})
        self.assertFalse(self.repo_mem.exists())
        blocked = {b["file"]: b for b in result["blocked"]}
        self.assertEqual(sorted(blocked), ["reference-host.md", "user-me.md"])
        self.assertEqual(blocked["reference-host.md"]["findings"], [{"kind": "private-hostname", "line": 8}])
        self.assertIn("private hostname (line 8)", blocked["reference-host.md"]["reason"])
        self.assertNotIn("build-box", json.dumps(result))
        self.assertIn("type: user", blocked["user-me.md"]["reason"])
        self.assertTrue((self.personal / "reference-host.md").is_file())
        self.assertTrue((self.personal / "user-me.md").is_file())

    def test_promotion_refuses_a_symlinked_dot_claude_before_any_mkdir(self):
        outside = self.tmp / "outside"
        outside.mkdir()
        repo = self.tmp / "repo"
        repo.mkdir()
        (repo / ".claude").symlink_to(outside)
        result = self.migrate(promote={"project-b.md"})
        self.assertEqual(list(outside.iterdir()), [])
        self.assertEqual([b["file"] for b in result["blocked"]], ["project-b.md"])
        self.assertIn(".claude", result["blocked"][0]["reason"])
        self.assertTrue((self.personal / "project-b.md").is_file())

    def test_a_user_type_in_any_case_or_with_a_comment_is_never_promoted(self):
        write(self.old / "user-odd.md", note_text("user-odd", "User  # mine", "odd"))
        result = self.migrate(promote={"user-odd.md"})
        self.assertEqual(result["blocked"][0]["reason"], memory_sync.USER_REASON)
        self.assertFalse(self.repo_mem.exists())

    def test_promotion_never_writes_through_a_symlinked_repo_memory(self):
        outside = self.tmp / "outside"
        outside.mkdir()
        self.repo_mem.parent.mkdir(parents=True)
        self.repo_mem.symlink_to(outside)
        result = self.migrate(promote={"project-b.md"})
        self.assertEqual(list(outside.iterdir()), [])
        self.assertEqual([b["file"] for b in result["blocked"]], ["project-b.md"])
        self.assertTrue((self.personal / "project-b.md").is_file())


class SweepUserNotesTest(MemCase):
    """C1: `type: user` notes are moved out of repo and agent memory into personal memory."""

    def setUp(self):
        super().setUp()
        self.src = self.tmp / "repo" / ".claude" / "memory"
        self.personal = self.anth / "personal-memory"
        write(self.src / "user-role.md", note_text("user-role", "user", "Role"))
        write(self.src / "project-a.md", note_text("project-a", "project", "A"))

    def test_type_is_compared_case_insensitively_without_comments(self):
        write(self.src / "user-caps.md", note_text("user-caps", "User", "Caps"))
        write(self.src / "user-comment.md", note_text("user-comment", "user # about me", "Comment"))
        res = memory_sync.sweep_user_notes(self.src, self.personal)
        self.assertEqual(sorted(res["moved"]), ["user-caps.md", "user-comment.md", "user-role.md"])
        self.assertEqual(memory_sync.normalize_type(" User # x "), "user")

    def test_moves_user_notes_only(self):
        data = (self.src / "user-role.md").read_bytes()
        res = memory_sync.sweep_user_notes(self.src, self.personal, today="2026-04-05")
        self.assertEqual(res, {"moved": ["user-role.md"], "conflicts": []})
        self.assertFalse((self.src / "user-role.md").exists())
        self.assertTrue((self.src / "project-a.md").exists())
        self.assertEqual((self.personal / "user-role.md").read_bytes(), data)

    def test_identical_personal_copy_counts_as_done(self):
        write(self.personal / "user-role.md", (self.src / "user-role.md").read_text())
        res = memory_sync.sweep_user_notes(self.src, self.personal, today="2026-04-05")
        self.assertEqual(res["moved"], ["user-role.md"])
        self.assertEqual(sorted(p.name for p in self.personal.iterdir()), ["user-role.md"])
        self.assertFalse((self.src / "user-role.md").exists())

    def test_conflict_is_kept_side_by_side(self):
        write(self.personal / "user-role.md", "mine\n")
        data = (self.src / "user-role.md").read_bytes()
        res = memory_sync.sweep_user_notes(self.src, self.personal, today="2026-04-05")
        self.assertEqual(res, {"moved": ["user-role.md"], "conflicts": ["user-role.md"]})
        self.assertEqual((self.personal / "user-role.md").read_text(), "mine\n")
        self.assertEqual((self.personal / "user-role.migrated-2026-04-05.md").read_bytes(), data)
        self.assertFalse((self.src / "user-role.md").exists())

    def test_symlinks_and_unsafe_folders_are_never_touched(self):
        outside = self.tmp / "outside.md"
        write(outside, note_text("user-out", "user", "Out"))
        (self.src / "user-link.md").symlink_to(outside)
        memory_sync.sweep_user_notes(self.src, self.personal)
        self.assertTrue((self.src / "user-link.md").is_symlink())
        self.assertTrue(outside.exists())
        self.assertFalse((self.personal / "user-link.md").exists())
        # a symlinked personal folder: nothing is written or deleted
        other = self.tmp / "elsewhere"
        other.mkdir()
        link = self.tmp / "personal-link"
        link.symlink_to(other)
        write(self.src / "user-two.md", note_text("user-two", "user", "Two"))
        self.assertEqual(memory_sync.sweep_user_notes(self.src, link), {"moved": [], "conflicts": []})
        self.assertTrue((self.src / "user-two.md").exists())
        self.assertEqual(list(other.iterdir()), [])
        # a symlinked source folder is not followed
        linked_src = self.tmp / "src-link"
        linked_src.symlink_to(self.src)
        self.assertEqual(memory_sync.sweep_user_notes(linked_src, self.personal)["moved"], [])

    def test_repo_file_is_kept_when_the_copy_cannot_be_verified(self):
        real = memory_sync._read_regular
        calls = []

        def flaky(path):
            calls.append(path)
            data = real(path)
            return data + b"changed" if str(path).startswith(str(self.personal)) else data

        with mock.patch.object(memory_sync, "_read_regular", flaky):
            res = memory_sync.sweep_user_notes(self.src, self.personal)
        self.assertEqual(res["moved"], [])
        self.assertTrue((self.src / "user-role.md").exists())


class AutoMemoryOffTest(MemCase):
    """R8: env var or setting turns run into a no-op."""

    def test_env_values(self):
        write(self.anth / "personal-memory" / "user-me.md", note_text("user-me", "user", "me"))
        for value in ("1", "true", "TRUE", "yes", "on", "anything"):
            e = dict(os.environ, CLAUDE_CODE_DISABLE_AUTO_MEMORY=value)
            self.assertEqual(memory_sync.run(self.tmp, e), ("", []), value)
        self.assertFalse((self.anth / "personal-memory" / "MEMORY.md").exists())
        for value in ("", "0", "false", "No", "FALSE"):
            e = dict(os.environ, CLAUDE_CODE_DISABLE_AUTO_MEMORY=value)
            ctx, _ = memory_sync.run(self.tmp, e)
            self.assertIn("user-me", ctx, value)

    def test_user_setting(self):
        write(self.anth / "personal-memory" / "user-me.md", note_text("user-me", "user", "me"))
        write(self.config / "settings.json", json.dumps({"autoMemoryEnabled": False}))
        self.assertEqual(memory_sync.run(self.tmp, env()), ("", []))
        self.assertFalse((self.anth / "personal-memory" / "MEMORY.md").exists())

    @needs_git
    def test_project_setting_and_precedence(self):
        root = self.make_repo()
        memory_sync.approve(root)
        write(self.config / "settings.json", json.dumps({"autoMemoryEnabled": False}))
        # a project settings file that re-enables beats the user file
        write(root / ".claude" / "settings.json", json.dumps({"autoMemoryEnabled": True}))
        memory_sync.run(root, env())
        self.assertTrue((root / ".claude" / "settings.local.json").exists())
        (root / ".claude" / "settings.local.json").unlink()
        # and settings.local.json beats settings.json
        write(root / ".claude" / "settings.json", json.dumps({"autoMemoryEnabled": True}))
        write(root / ".claude" / "settings.local.json", json.dumps({"autoMemoryEnabled": False}))
        before = snapshot(root)
        self.assertEqual(memory_sync.run(root, env()), ("", []))
        self.assertEqual(snapshot(root), before)

    @needs_git
    def test_disabled_repo_run_writes_nothing(self):
        root = self.make_repo()
        write(root / ".claude" / "memory" / "a.md", note_text("a"))
        memory_sync.approve(root)
        before = snapshot(root)
        e = dict(os.environ, CLAUDE_CODE_DISABLE_AUTO_MEMORY="1")
        self.assertEqual(memory_sync.run(root, e), ("", []))
        self.assertEqual(snapshot(root), before)


class RunOutsideRepoTest(MemCase):
    def test_personal_index_is_injected_anywhere(self):
        write(self.anth / "personal-memory" / "user-me.md",
              note_text("user-me", "user", "Likes terse answers"))
        folder = self.tmp / "plain"
        folder.mkdir()
        ctx, msgs = memory_sync.run(folder, env())
        self.assertEqual(msgs, [])
        self.assertTrue(ctx.startswith("## "))
        self.assertIn(str(self.anth / "personal-memory"), ctx.splitlines()[0])
        self.assertIn("[user-me](user-me.md) — Likes terse answers", ctx)
        self.assertEqual(list(folder.iterdir()), [])
        self.assertTrue((self.anth / "personal-memory" / "MEMORY.md").exists())

    def test_never_creates_the_personal_dir(self):
        folder = self.tmp / "plain"
        folder.mkdir()
        self.assertEqual(memory_sync.run(folder, env()), (personal_only(self.anth), []))
        self.assertFalse((self.anth / "personal-memory").exists())

    def test_swallows_every_exception(self):
        with mock.patch.object(memory_sync, "_run", side_effect=RuntimeError("boom")):
            ctx, msgs = memory_sync.run(self.tmp, env())
        self.assertEqual(ctx, "")
        self.assertEqual(len(msgs), 1)
        self.assertTrue(msgs[0].startswith("anthology memory: "))
        self.assertIn("boom", msgs[0])

    def test_unreadable_cwd_does_not_raise(self):
        ctx, msgs = memory_sync.run(self.tmp / "does-not-exist", env())
        self.assertEqual(ctx, personal_only(self.anth))


@needs_git
class TrustBoundaryTest(MemCase):
    def setUp(self):
        super().setUp()
        self.root = self.make_repo()
        write(self.root / ".claude" / "memory" / "feedback-x.md",
              note_text("feedback-x", "feedback", "Prefer small commits", "2026-02-02"))
        self.commit_all(self.root)

    def test_unapproved_repo_gets_one_hint_and_no_writes(self):
        old = self.config / "projects" / encode(self.root) / "memory"
        write(old / "project-old.md", note_text("project-old"))
        before = snapshot(self.root)
        exclude = (self.root / ".git" / "info" / "exclude").read_bytes()
        status = git(self.root, "status", "--porcelain").stdout
        ctx, msgs = memory_sync.run(self.root, env())
        self.assertEqual(ctx, personal_only(self.anth))
        self.assertEqual(msgs, [HINT])
        self.assertEqual(snapshot(self.root), before)
        self.assertEqual((self.root / ".git" / "info" / "exclude").read_bytes(), exclude)
        self.assertEqual(git(self.root, "status", "--porcelain").stdout, status)
        self.assertFalse((self.root / ".claude" / "settings.local.json").exists())
        self.assertFalse((self.root / ".gitattributes").exists())
        self.assertFalse((self.anth / "approved-repos.json").exists())
        self.assertFalse((self.anth / "personal-memory").exists())

    def test_hint_for_agent_memory_only(self):
        other = self.make_repo("other")
        write(other / ".claude" / "agent-memory" / "anthology-x" / "a.md", note_text("a"))
        self.assertEqual(memory_sync.run(other, env()), (personal_only(self.anth), [HINT]))

    def test_no_hint_without_committed_memory(self):
        other = self.make_repo("other")
        self.assertEqual(memory_sync.run(other, env()), (personal_only(self.anth), []))

    def test_unapproved_notes_are_never_injected(self):
        write(self.anth / "personal-memory" / "user-me.md", note_text("user-me", "user", "me"))
        ctx, msgs = memory_sync.run(self.root, env())
        self.assertNotIn("feedback-x", ctx)
        self.assertIn("user-me", ctx)
        self.assertEqual(msgs, [HINT])

    def test_after_approve_the_repair_happens(self):
        memory_sync.approve(self.root)
        ctx, msgs = memory_sync.run(self.root, env())
        settings = json.loads((self.root / ".claude" / "settings.local.json").read_text())
        self.assertEqual(settings["autoMemoryDirectory"], str(self.root / ".claude" / "memory"))
        index = (self.root / ".claude" / "memory" / "MEMORY.md").read_text()
        self.assertIn("[feedback-x](feedback-x.md) — Prefer small commits", index)
        self.assertIn(index, ctx)
        self.assertTrue(ctx.startswith("## "))
        self.assertTrue(ctx.startswith("## Personal memory"))  # always first, even when empty
        repo_head = [l for l in ctx.splitlines() if l.startswith("## Repo memory")][0]
        self.assertIn(str(self.root / ".claude" / "memory"), repo_head)
        self.assertNotIn(HINT, msgs)
        self.assertTrue(msgs)
        # the generated index never names a clone path
        self.assertNotIn(str(self.root), index)


@needs_git
class RepairTest(MemCase):
    def approved_repo(self, name="repo"):
        root = self.make_repo(name)
        memory_sync.approve(root)
        return root

    def test_ensure_memory_dir_preserves_other_keys_and_order(self):
        root = self.approved_repo()
        sl = root / ".claude" / "settings.local.json"
        write(sl, json.dumps({"zeta": 1, "permissions": {"allow": ["Bash(ls)"]}, "alpha": [1]}))
        data, changed = memory_sync.ensure_memory_dir(root, sl)
        self.assertTrue(changed)
        self.assertEqual(list(data), ["zeta", "permissions", "alpha", "autoMemoryDirectory"])
        on_disk = json.loads(sl.read_text())
        self.assertEqual(list(on_disk), list(data))
        self.assertEqual(on_disk["permissions"], {"allow": ["Bash(ls)"]})
        self.assertTrue(sl.read_text().endswith("}\n"))
        self.assertTrue((root / ".claude" / "memory").is_dir())
        mtime = sl.stat().st_mtime_ns
        data2, changed2 = memory_sync.ensure_memory_dir(root, sl)
        self.assertFalse(changed2)
        self.assertEqual(data2, data)
        self.assertEqual(sl.stat().st_mtime_ns, mtime)

    def test_ensure_memory_dir_missing_file_creates_it(self):
        root = self.approved_repo()
        sl = root / ".claude" / "settings.local.json"
        data, changed = memory_sync.ensure_memory_dir(root, sl)
        self.assertTrue(changed)
        self.assertEqual(data, {"autoMemoryDirectory": str(root / ".claude" / "memory")})
        self.assertEqual(json.loads(sl.read_text()), data)

    def test_ensure_memory_dir_corrects_a_stale_path(self):
        root = self.approved_repo()
        sl = root / ".claude" / "settings.local.json"
        write(sl, json.dumps({"autoMemoryDirectory": "/some/old/clone/.claude/memory"}))
        data, changed = memory_sync.ensure_memory_dir(root, sl)
        self.assertTrue(changed)
        self.assertEqual(data["autoMemoryDirectory"], str(root / ".claude" / "memory"))

    def test_invalid_settings_are_left_alone(self):
        root = self.approved_repo()
        sl = root / ".claude" / "settings.local.json"
        for text in ("{ not json", "[1, 2]", ""):
            write(sl, text)
            data, changed = memory_sync.ensure_memory_dir(root, sl)
            self.assertFalse(changed)
            self.assertEqual(sl.read_text(), text)
            self.assertEqual(data, {})
        write(sl, "{ not json")
        ctx, msgs = memory_sync.run(root, env())
        self.assertEqual(sl.read_text(), "{ not json")
        self.assertTrue(any("settings.local.json" in m for m in msgs), msgs)

    def test_ensure_ignored(self):
        root = self.approved_repo()
        exclude = root / ".git" / "info" / "exclude"
        self.assertTrue(memory_sync.ensure_ignored(root))
        self.assertEqual(exclude.read_text().splitlines().count("/.claude/settings.local.json"), 1)
        write(root / ".claude" / "settings.local.json", "{}")
        self.assertEqual(git(root, "check-ignore", "-q", ".claude/settings.local.json",
                             check=False).returncode, 0)
        self.assertFalse(memory_sync.ensure_ignored(root))

    def test_ensure_ignored_noop_when_gitignore_already_covers_it(self):
        root = self.approved_repo()
        write(root / ".gitignore", ".claude/settings.local.json\n")
        exclude = root / ".git" / "info" / "exclude"
        before = exclude.read_bytes() if exclude.exists() else b""
        self.assertFalse(memory_sync.ensure_ignored(root))
        self.assertEqual(exclude.read_bytes() if exclude.exists() else b"", before)

    def test_ensure_ignored_handles_missing_trailing_newline(self):
        root = self.approved_repo()
        exclude = root / ".git" / "info" / "exclude"
        exclude.parent.mkdir(exist_ok=True)
        exclude.write_text("# keep\nfoo")
        self.assertTrue(memory_sync.ensure_ignored(root))
        self.assertEqual(exclude.read_text().splitlines(),
                         ["# keep", "foo", "/.claude/settings.local.json"])

    def test_linked_worktree_writes_the_right_exclude_file(self):
        root = self.approved_repo()
        write(root / "f", "x")
        self.commit_all(root)
        wt = Path(os.path.realpath(self.tmp / "wt"))
        git(root, "worktree", "add", "-q", str(wt), "-b", "wt")
        self.assertTrue(memory_sync.ensure_ignored(wt))
        shared = git(wt, "rev-parse", "--path-format=absolute", "--git-path", "info/exclude").stdout.strip()
        self.assertEqual(os.path.realpath(shared), os.path.realpath(str(root / ".git" / "info" / "exclude")))
        self.assertIn("/.claude/settings.local.json", Path(shared).read_text().splitlines())
        self.assertEqual(git(wt, "check-ignore", "-q", ".claude/settings.local.json",
                             check=False).returncode, 0)
        self.assertEqual(list((root / ".git").glob("worktrees/*/info/exclude")), [])
        self.assertFalse(memory_sync.ensure_ignored(wt))

    def test_run_in_a_linked_worktree_points_at_the_worktree(self):
        root = self.approved_repo()
        write(root / ".claude" / "memory" / "project-a.md", note_text("project-a", desc="A"))
        self.commit_all(root)
        wt = Path(os.path.realpath(self.tmp / "wt"))
        git(root, "worktree", "add", "-q", str(wt), "-b", "wt")
        ctx, msgs = memory_sync.run(wt, env())
        settings = json.loads((wt / ".claude" / "settings.local.json").read_text())
        self.assertEqual(settings["autoMemoryDirectory"], str(wt / ".claude" / "memory"))
        self.assertIn("project-a", ctx)
        self.assertFalse((root / ".claude" / "settings.local.json").exists())


@needs_git
class ReCloneTest(MemCase):
    def test_new_clone_of_an_approved_repo(self):
        bare = self.tmp / "origin.git"
        bare.mkdir()
        git(bare, "init", "-q", "--bare")
        a = self.make_repo("clone-a", remote=str(bare))
        write(a / ".claude" / "memory" / "feedback-x.md",
              note_text("feedback-x", "feedback", "Prefer small commits", "2026-02-02"))
        self.commit_all(a)
        git(a, "push", "-q", "origin", "main")
        memory_sync.approve(a)
        memory_sync.run(a, env())

        b = Path(os.path.realpath(self.tmp / "elsewhere" / "clone-b"))
        b.parent.mkdir()
        git(self.tmp, "clone", "-q", str(bare), str(b))
        self.assertEqual(memory_sync.repo_key(a), memory_sync.repo_key(b))
        self.assertTrue(memory_sync.is_approved(memory_sync.repo_key(b)))

        ctx, msgs = memory_sync.run(b, env())
        settings = json.loads((b / ".claude" / "settings.local.json").read_text())
        self.assertEqual(settings["autoMemoryDirectory"], str(b / ".claude" / "memory"))
        self.assertIn("[feedback-x](feedback-x.md)", ctx)
        self.assertEqual(git(b, "check-ignore", "-q", ".claude/settings.local.json",
                             check=False).returncode, 0)
        self.assertNotIn(".claude/settings.local.json", git(b, "status", "--porcelain").stdout)

        # a second run is a complete no-op
        before = snapshot(b)
        exclude = (b / ".git" / "info" / "exclude").read_bytes()
        self.assertEqual(memory_sync.run(b, env()), (personal_only(self.anth), []))
        self.assertEqual(snapshot(b), before)
        self.assertEqual((b / ".git" / "info" / "exclude").read_bytes(), exclude)

    def test_empty_repo_memory_cover_has_a_placeholder(self):
        root = self.make_repo()
        memory_sync.approve(root)
        ctx, msgs = memory_sync.run(root, env())
        self.assertTrue(ctx.startswith("## "))
        self.assertTrue(ctx.strip().splitlines()[-1].strip())
        self.assertEqual(memory_sync.run(root, env()), (personal_only(self.anth), []))


@needs_git
class RunFlowTest(MemCase):
    def test_personal_comes_before_the_repo_cover(self):
        root = self.make_repo()
        memory_sync.approve(root)
        write(root / ".claude" / "memory" / "project-a.md", note_text("project-a", desc="Repo thing"))
        write(self.anth / "personal-memory" / "user-me.md", note_text("user-me", "user", "Me"))
        ctx, _ = memory_sync.run(root, env())
        heads = [l for l in ctx.splitlines() if l.startswith("## ") and ("memory" in l.lower())]
        self.assertIn(str(self.anth / "personal-memory"), heads[0])
        self.assertIn(str(root / ".claude" / "memory"), heads[1])
        self.assertLess(ctx.index("user-me"), ctx.index("project-a"))

    def test_agent_memory_indexes_are_regenerated(self):
        root = self.make_repo()
        memory_sync.approve(root)
        agent = root / ".claude" / "agent-memory" / "anthology-writer"
        write(agent / "feedback-q.md", note_text("feedback-q", "feedback", "Q"))
        memory_sync.run(root, env())
        self.assertIn("[feedback-q](feedback-q.md)", (agent / "MEMORY.md").read_text())

    def test_migration_runs_in_approved_repos_only_and_includes_main_worktree(self):
        root = self.make_repo()
        write(root / "f", "x")
        self.commit_all(root)
        memory_sync.approve(root)
        wt = Path(os.path.realpath(self.tmp / "wt"))
        git(root, "worktree", "add", "-q", str(wt), "-b", "wt")
        old_main = self.config / "projects" / encode(root) / "memory"
        old_wt = self.config / "projects" / encode(wt) / "memory"
        write(old_main / "project-m.md", note_text("project-m", "project", "from main"))
        write(old_main / "user-u.md", note_text("user-u", "user", "personal one"))
        write(old_wt / "feedback-w.md", note_text("feedback-w", "feedback", "from wt"))
        ctx, msgs = memory_sync.run(wt, env())
        repo_mem = wt / ".claude" / "memory"
        personal = self.anth / "personal-memory"
        # B3: nothing is copied into the repo; every legacy note goes to personal memory
        self.assertEqual(sorted(p.name for p in repo_mem.glob("*.md")), [])
        self.assertEqual(sorted(p.name for p in personal.glob("*.md") if p.name != "MEMORY.md"),
                         ["feedback-w.md", "project-m.md", "user-u.md"])
        self.assertIn("personal one", ctx)
        self.assertIn("from main", ctx)
        self.assertTrue((self.anth / "migrated.json").exists())
        self.assertTrue(any("3 note(s)" in m and str(personal) in m and "/anthology:setup" in m
                            for m in msgs), msgs)
        # old notes are still there (copied, never moved)
        self.assertTrue((old_main / "project-m.md").exists())
        # a later session does not duplicate
        memory_sync.run(wt, env())
        self.assertEqual(sorted(p.name for p in personal.glob("*.md") if p.name != "MEMORY.md"),
                         ["feedback-w.md", "project-m.md", "user-u.md"])


@needs_git
class TeamMergeTest(MemCase):
    def scenario(self, with_attrs):
        root = self.make_repo()
        mem = root / ".claude" / "memory"
        write(mem / "project-base.md", note_text("project-base", "project", "Base", "2026-01-01"))
        memory_sync.regenerate_index(mem)
        if with_attrs:
            memory_sync.ensure_union_attrs(root)
        self.commit_all(root, "base")
        git(root, "checkout", "-q", "-b", "one")
        write(mem / "project-one.md", note_text("project-one", "project", "One", "2026-02-01"))
        memory_sync.regenerate_index(mem)
        self.commit_all(root, "one")
        git(root, "checkout", "-q", "main")
        git(root, "checkout", "-q", "-b", "two")
        write(mem / "project-two.md", note_text("project-two", "project", "Two", "2026-02-02"))
        memory_sync.regenerate_index(mem)
        self.commit_all(root, "two")
        git(root, "checkout", "-q", "one")
        return root, mem, git(root, "merge", "--no-edit", "two", check=False)

    def test_union_attrs_make_the_merge_succeed(self):
        root, mem, merge = self.scenario(with_attrs=True)
        self.assertEqual(merge.returncode, 0, merge.stdout + merge.stderr)
        raw = (mem / "MEMORY.md").read_text()
        self.assertIn("](project-one.md)", raw)
        self.assertIn("](project-two.md)", raw)
        self.assertNotIn("<<<<<<<", raw)
        memory_sync.regenerate_index(mem)
        text = (mem / "MEMORY.md").read_text()
        for name in ("project-base", "project-one", "project-two"):
            self.assertEqual(text.count(f"]({name}.md)"), 1, name)

    def test_without_the_attrs_the_same_merge_conflicts(self):
        root, mem, merge = self.scenario(with_attrs=False)
        self.assertNotEqual(merge.returncode, 0)
        self.assertIn("<<<<<<<", (mem / "MEMORY.md").read_text())

    def test_regenerate_cleans_up_what_union_leaves_behind(self):
        root = self.make_repo()
        mem = root / ".claude" / "memory"
        write(mem / "project-a.md", note_text("project-a", "project", "A", "2026-01-01"))
        write(mem / "project-b.md", note_text("project-b", "project", "B", "2026-01-02"))
        write(mem / "MEMORY.md", "<!-- old -->\n## project\n- [project-a](project-a.md) — A\n"
                                 "- [project-b](project-b.md) — B\n- [project-a](project-a.md) — A\n")
        self.assertTrue(memory_sync.regenerate_index(mem))
        text = (mem / "MEMORY.md").read_text()
        self.assertEqual(text.count("](project-a.md)"), 1)
        self.assertEqual(text.count("](project-b.md)"), 1)


@needs_git
class TrackedSettingsTest(MemCase):
    """Review fix: a committed settings.local.json must never receive this clone's path."""

    def test_tracked_settings_file_is_left_alone(self):
        root = self.make_repo()
        write(root / ".claude" / "settings.local.json", "{}\n")
        self.commit_all(root)
        memory_sync.approve(root)
        exclude = root / ".git" / "info" / "exclude"
        exclude_before = exclude.read_bytes() if exclude.exists() else b""
        before = snapshot(root)
        status = git(root, "status", "--porcelain").stdout
        for _ in range(3):
            ctx, msgs = memory_sync.run(root, env())
            self.assertEqual(ctx, personal_only(self.anth))
            self.assertTrue(any("tracked" in m and "git rm --cached" in m for m in msgs), msgs)
        self.assertEqual((root / ".claude" / "settings.local.json").read_text(), "{}\n")
        self.assertEqual(exclude.read_bytes() if exclude.exists() else b"", exclude_before)
        self.assertEqual(git(root, "status", "--porcelain").stdout, status)
        self.assertEqual(snapshot(root), before)
        self.assertNotIn(str(root), (root / ".claude" / "settings.local.json").read_text())

    def test_exclude_line_is_never_appended_twice(self):
        root = self.make_repo()
        write(root / ".gitignore", "!.claude/settings.local.json\n")  # beats the exclude file
        exclude = root / ".git" / "info" / "exclude"
        exclude.parent.mkdir(exist_ok=True)
        exclude.write_text("/.claude/settings.local.json\n")
        for _ in range(3):
            self.assertFalse(memory_sync.ensure_ignored(root))
        self.assertEqual(exclude.read_text(), "/.claude/settings.local.json\n")
        # and a fresh repo gets the line exactly once however often it runs
        other = self.make_repo("other")
        for _ in range(3):
            memory_sync.ensure_ignored(other)
        lines = (other / ".git" / "info" / "exclude").read_text().splitlines()
        self.assertEqual(lines.count("/.claude/settings.local.json"), 1)


@needs_git
class SymlinkSafetyTest(MemCase):
    """Review fixes: a repo's own symlinks must not redirect anthology's writes."""

    def test_symlinked_gitattributes_is_not_appended_through(self):
        root = self.make_repo()
        victim = self.tmp / "victim.rc"
        write(victim, "export X=1\n")
        (root / ".gitattributes").symlink_to(victim)
        with self.assertRaises(memory_sync.UnsafeTarget):
            memory_sync.approve(root)
        self.assertEqual(victim.read_text(), "export X=1\n")
        self.assertTrue(memory_sync.is_approved(memory_sync.repo_key(root)))  # recorded first
        with self.assertRaises(memory_sync.UnsafeTarget):
            memory_sync.ensure_union_attrs(root)
        self.assertEqual(victim.read_text(), "export X=1\n")

    def test_fifos_never_hang_a_session(self):
        """A FIFO named like a file anthology reads or writes must be refused, not opened."""
        root = self.make_repo()
        os.mkfifo(str(root / ".gitattributes"))
        (root / ".claude").mkdir()
        os.mkfifo(str(root / ".claude" / "settings.local.json"))
        code = (
            "import sys, memory_sync\n"
            "root = sys.argv[1]\n"
            "try:\n"
            "    memory_sync.approve(root)\n"
            "except memory_sync.UnsafeTarget:\n"
            "    print('refused attrs')\n"
            "ctx, msgs = memory_sync.run(root, None)\n"
            "print('settings', any('settings.local.json' in m for m in msgs))\n"
        )
        from _mem import SCRIPTS
        r = subprocess.run([sys.executable, "-c", code, str(root)], env=dict(os.environ, PYTHONPATH=str(SCRIPTS),
                           PYTHONDONTWRITEBYTECODE="1"), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           text=True, timeout=30)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("refused attrs", r.stdout)
        self.assertIn("settings True", r.stdout)

    def test_symlinked_settings_local_is_not_written_through(self):
        root = self.make_repo()
        memory_sync.approve(root)
        victim = self.tmp / "victim.json"
        write(victim, "{}\n")
        (root / ".claude").mkdir()
        (root / ".claude" / "settings.local.json").symlink_to(victim)
        ctx, msgs = memory_sync.run(root, env())
        self.assertEqual(victim.read_text(), "{}\n")
        self.assertTrue((root / ".claude" / "settings.local.json").is_symlink())

    def test_symlinked_info_exclude_is_refused(self):
        root = self.make_repo()
        victim = self.tmp / "victim.txt"
        write(victim, "keep\n")
        exclude = root / ".git" / "info" / "exclude"
        exclude.parent.mkdir(exist_ok=True)
        if exclude.exists():
            exclude.unlink()
        exclude.symlink_to(victim)
        with self.assertRaises(memory_sync.UnsafeTarget):
            memory_sync.ensure_ignored(root)
        self.assertEqual(victim.read_text(), "keep\n")

    def _setup_outside(self):
        root = self.make_repo()
        memory_sync.approve(root)
        outside = self.tmp / "outside"
        write(outside / "MEMORY.md", "precious\n")
        write(outside / "project-x.md", note_text("project-x"))
        (root / ".claude").mkdir()
        old = self.config / "projects" / encode(root) / "memory"
        write(old / "project-old.md", note_text("project-old"))
        return root, outside

    def test_symlinked_repo_memory_dir_skips_all_repo_memory_work(self):
        root, outside = self._setup_outside()
        (root / ".claude" / "memory").symlink_to(outside)
        before = snapshot(outside)
        ctx, msgs = memory_sync.run(root, env())
        self.assertEqual(snapshot(outside), before)
        self.assertEqual(ctx, personal_only(self.anth))
        self.assertTrue(any("symlink" in m and ".claude/memory" in m for m in msgs), msgs)
        self.assertFalse((root / ".claude" / "settings.local.json").exists())
        self.assertFalse((self.anth / "migrated.json").exists())

    def test_ensure_memory_dir_refuses_symlinked_or_non_folder_memory(self):
        root, outside = self._setup_outside()
        settings = root / ".claude" / "settings.local.json"
        (root / ".claude" / "memory").symlink_to(outside)
        self.assertEqual(memory_sync.ensure_memory_dir(root, settings), ({}, False))
        self.assertFalse(settings.exists())
        (root / ".claude" / "memory").unlink()
        write(root / ".claude" / "memory", "a file, not a folder")
        self.assertEqual(memory_sync.ensure_memory_dir(root, settings), ({}, False))
        ctx, msgs = memory_sync.run(root, env())
        self.assertTrue(any("not a folder" in m or "symlink" in m for m in msgs), msgs)
        self.assertFalse(settings.exists())

    def test_migrate_does_not_write_through_symlinked_destinations(self):
        old = self.tmp / "old"
        write(old / "project-a.md", note_text("project-a"))
        write(old / "user-u.md", note_text("user-u", "user"))
        outside = self.tmp / "outside"
        outside.mkdir()
        (self.tmp / "repo-mem").symlink_to(outside)
        (self.tmp / "personal-link").symlink_to(outside)
        res = memory_sync.migrate(old, self.tmp / "repo-mem", self.tmp / "personal-link")
        self.assertEqual(list(outside.iterdir()), [])
        self.assertEqual(res["copied"], [])


class ManyTypesBudgetTest(MemCase):
    def test_sixty_types_stay_within_the_budget(self):
        d = self.tmp / "mem"
        for t in range(60):
            for n in range(2):
                write(d / ("t%02d-%d.md" % (t, n)),
                      note_text("t%02d-%d" % (t, n), "type%02d" % t, "d", "2026-01-%02d" % (n + 1)))
        budget = memory_sync.Budget(30, 20000)
        memory_sync.regenerate_index(d, budget)
        lines = (d / "MEMORY.md").read_text().splitlines()
        self.assertLessEqual(len(lines), 30)
        self.assertEqual(lines[0], memory_sync.GENERATED)
        self.assertEqual(lines[1], "## type00")
        tail = lines[-1]
        self.assertRegex(tail, r"^- \d+ more in other types — use `anthology-mem search`$")
        hidden = int(tail.split()[1])
        shown_types = len([l for l in lines if l.startswith("## ")])
        pointers = sum(int(l.split()[1]) for l in lines[:-1] if re.match(r"^- \d+ more — use", l))
        entries = len([l for l in lines if l.startswith("- [")])
        self.assertEqual(hidden + pointers + entries, 120)
        self.assertEqual(hidden, 2 * (60 - shown_types))
        # a byte budget too
        memory_sync.regenerate_index(d, memory_sync.Budget(150, 600))
        self.assertLessEqual(len((d / "MEMORY.md").read_bytes()), 600)

    def test_all_types_fit_means_no_collapsed_pointer(self):
        d = self.tmp / "mem"
        for t in range(5):
            write(d / ("t%d.md" % t), note_text("t%d" % t, "type%d" % t))
        memory_sync.regenerate_index(d)
        self.assertNotIn("other types", (d / "MEMORY.md").read_text())


class MigrateGlobTest(MemCase):
    def test_note_names_with_glob_characters_are_matched_literally(self):
        old = self.tmp / "old"
        personal = self.tmp / "personal"
        write(old / "weird[1].md", "from old\n")
        write(personal / "weird[1].md", "mine\n")
        for _ in range(3):
            memory_sync.migrate(old, None, personal, today="2026-04-05")
        self.assertEqual(sorted(p.name for p in personal.iterdir()),
                         ["weird[1].md", "weird[1].migrated-2026-04-05.md"])


class ContextBudgetTest(MemCase):
    def seed(self, n=300):
        for i in range(n):
            write(self.anth / "personal-memory" / ("user-%03d.md" % i),
                  note_text("user-%03d" % i, "user", "Personal note number %d" % i,
                            "2026-%02d-%02d" % (i // 28 % 12 + 1, i % 28 + 1)))

    def sections(self, ctx):
        parts = re.split(r"(?m)^(?=## (?:Personal|Repo) memory \()", ctx)
        return [p for p in parts if p.startswith("## ")]

    def test_personal_section_is_capped_but_on_disk_index_is_not(self):
        self.seed()
        folder = self.tmp / "plain"
        folder.mkdir()
        ctx, msgs = memory_sync.run(folder, env())
        (section,) = self.sections(ctx)
        heading, _, body = section.partition("\n\n")
        self.assertTrue(heading.startswith("## Personal memory"))
        self.assertLessEqual(len(body.encode()), 2500)
        self.assertLessEqual(len(body.splitlines()), 40)
        disk = self.anth / "personal-memory" / "MEMORY.md"
        self.assertRegex(body, r"more — use `anthology-mem search` \(full index: .*MEMORY\.md\)")
        self.assertIn(str(disk), body)
        self.assertGreater(len(disk.read_text().splitlines()), 100)  # the file keeps the full budget
        self.assertLessEqual(len(disk.read_bytes()), memory_sync.DEFAULT_BUDGET.bytes)
        self.assertEqual(memory_sync.CONTEXT_SECTION_BUDGET, memory_sync.Budget(40, 2500))

    @needs_git
    def test_repo_cover_still_present_after_a_repair(self):
        self.seed()
        root = self.make_repo()
        write(root / ".claude" / "memory" / "project-a.md", note_text("project-a", desc="Repo thing"))
        memory_sync.approve(root)
        ctx, msgs = memory_sync.run(root, env())
        secs = self.sections(ctx)
        self.assertEqual(len(secs), 2)
        self.assertTrue(secs[0].startswith("## Personal memory"))
        self.assertTrue(secs[1].startswith("## Repo memory"))
        self.assertIn("[project-a](project-a.md)", secs[1])
        self.assertLessEqual(len(secs[0].partition("\n\n")[2].encode()), 2500)


class LegacyNotesStayOutOfTheRepoTest(MemCase):
    """B3 reproduced: the first session after setup copied every non-user legacy note into the
    (possibly public) repo. Now they all go to personal memory. Runs without git (fake_repo)."""

    def setUp(self):
        super().setUp()
        self.root = self.fake_repo()
        memory_sync.approve(self.root)
        self.old = self.config / "projects" / encode(self.root) / "memory"
        write(self.old / "reference-build-host.md",
              note_text("reference-build-host", "reference", "The build host",
                        body="Builds run on build-" + "box.lan as deploy.\n"))
        write(self.old / "project-plan.md", note_text("project-plan", "project", "The plan"))

    def test_session_start_copies_legacy_notes_to_personal_memory_only(self):
        ctx, msgs = memory_sync.run(self.root, env())
        repo_mem = self.root / ".claude" / "memory"
        self.assertEqual(sorted(p.name for p in repo_mem.iterdir()), [])
        personal = self.anth / "personal-memory"
        for name in ("reference-build-host.md", "project-plan.md"):
            self.assertEqual((personal / name).read_bytes(), (self.old / name).read_bytes())
        self.assertEqual(len(json.loads((self.anth / "migrated.json").read_text())["done"]), 2)
        self.assertTrue(any("2 note(s)" in m and str(personal) in m and "/anthology:setup" in m
                            for m in msgs), msgs)
        self.assertNotIn("build-" + "box", "".join(l for l in ctx.splitlines() if "Repo memory" in l))
        # a second session copies nothing more and still writes nothing into the repo
        _ctx, msgs = memory_sync.run(self.root, env())
        self.assertFalse(any("note(s)" in m for m in msgs), msgs)
        self.assertEqual(sorted(p.name for p in repo_mem.iterdir()), [])


class UserNotesLeaveTheRepoTest(MemCase):
    """C1: `type: user` notes never stay in an approved repo. Runs without git (fake_repo)."""

    def setUp(self):
        super().setUp()
        self.root = self.fake_repo()
        memory_sync.approve(self.root)
        self.repo_mem = self.root / ".claude" / "memory"
        self.agent = self.root / ".claude" / "agent-memory" / "anthology-scout"
        self.personal = self.anth / "personal-memory"
        write(self.repo_mem / "user-role.md", note_text("user-role", "user", "Senior engineer"))
        write(self.repo_mem / "project-a.md", note_text("project-a", "project", "Repo thing"))
        write(self.agent / "user-tz.md", note_text("user-tz", "user", "Works in CET"))
        write(self.agent / "feedback-b.md", note_text("feedback-b", "feedback", "Agent thing"))

    def test_user_notes_move_to_personal_memory_at_session_start(self):
        role = (self.repo_mem / "user-role.md").read_bytes()
        tz = (self.agent / "user-tz.md").read_bytes()
        ctx, msgs = memory_sync.run(self.root, env())
        self.assertFalse((self.repo_mem / "user-role.md").exists())
        self.assertFalse((self.agent / "user-tz.md").exists())
        self.assertEqual((self.personal / "user-role.md").read_bytes(), role)
        self.assertEqual((self.personal / "user-tz.md").read_bytes(), tz)
        self.assertTrue((self.repo_mem / "project-a.md").exists())
        self.assertTrue((self.agent / "feedback-b.md").exists())
        moved = [m for m in msgs if "moved 2 user note(s)" in m]
        self.assertEqual(len(moved), 1, msgs)
        self.assertIn(str(self.personal), moved[0])
        self.assertIn("commit their removal", moved[0])
        for index in (self.repo_mem / "MEMORY.md", self.agent / "MEMORY.md"):
            text = index.read_text()
            self.assertNotIn("## user", text)
            self.assertNotIn("user-", text)
        self.assertIn("[user-role](user-role.md)", (self.personal / "MEMORY.md").read_text())
        personal_part, _, repo_part = ctx.partition("## Repo memory")
        self.assertIn("user-role", personal_part)
        self.assertIn("project-a", repo_part)
        self.assertNotIn("user-", repo_part)

    def test_a_symlinked_personal_folder_is_a_visible_message_not_a_silent_skip(self):
        # m8: fail closed (nothing moved, copied or read through the link) but say so, once
        target = self.tmp / "dotfiles-memory"
        write(target / "feedback-mine.md", note_text("feedback-mine", "feedback", "Mine"))
        self.personal.parent.mkdir(parents=True, exist_ok=True)
        self.personal.symlink_to(target)
        old = self.config / "projects" / encode(self.root) / "memory"
        write(old / "project-old.md", note_text("project-old", "project", "Old"))
        before = snapshot(target)
        ctx, msgs = memory_sync.run(self.root, env())
        self.assertEqual(snapshot(target), before)
        self.assertTrue((self.repo_mem / "user-role.md").exists())  # not swept
        self.assertFalse((self.anth / "migrated.json").exists())  # not migrated
        warned = [m for m in msgs if "personal-memory" in m and "symlink" in m]
        self.assertEqual(len(warned), 1, msgs)
        self.assertIn("not moved", warned[0])
        self.assertNotIn("feedback-mine", ctx)  # not read through the link either
        self.assertIn("Personal memory is unavailable", ctx)
        self.assertNotIn("Save notes about the user", ctx)  # no instruction to write through the link
        self.assertEqual(memory_sync.note_sources(None, self.anth), [])  # `anthology-mem search` skips it
        self.assertNotIn("personal", [s for s, _ in memory_sync.note_sources(self.root, self.anth)])

    def test_a_symlinked_personal_folder_is_reported_outside_an_approved_repo_too(self):
        target = self.tmp / "dotfiles-memory"
        target.mkdir()
        self.personal.parent.mkdir(parents=True, exist_ok=True)
        self.personal.symlink_to(target)
        _ctx, msgs = memory_sync.run(self.tmp, env())
        self.assertEqual(len([m for m in msgs if "personal-memory" in m and "symlink" in m]), 1, msgs)

    def test_a_conflicting_personal_note_is_kept_side_by_side(self):
        write(self.personal / "user-role.md", "my own\n")
        role = (self.repo_mem / "user-role.md").read_bytes()
        memory_sync.run(self.root, env())
        self.assertEqual((self.personal / "user-role.md").read_text(), "my own\n")
        copies = [p for p in self.personal.glob("user-role.migrated-*.md")]
        self.assertEqual(len(copies), 1)
        self.assertEqual(copies[0].read_bytes(), role)
        self.assertFalse((self.repo_mem / "user-role.md").exists())

    def test_a_symlinked_user_note_is_untouched(self):
        outside = self.tmp / "outside.md"
        write(outside, note_text("user-out", "user", "Outside"))
        (self.repo_mem / "user-link.md").symlink_to(outside)
        memory_sync.run(self.root, env())
        self.assertTrue((self.repo_mem / "user-link.md").is_symlink())
        self.assertTrue(outside.is_file())
        self.assertFalse((self.personal / "user-link.md").exists())
        self.assertNotIn("user-link", (self.repo_mem / "MEMORY.md").read_text())

    def test_repo_indexes_never_list_user_notes_even_if_the_move_fails(self):
        write(self.repo_mem / "MEMORY.md", "## user\n- [user-role](user-role.md)\n")
        write(self.agent / "MEMORY.md", "## user\n- [user-tz](user-tz.md)\n")
        (self.agent / "feedback-b.md").unlink()  # the agent folder holds only a user note now
        stuck = {"moved": [], "conflicts": []}
        with mock.patch.object(memory_sync, "sweep_user_notes", return_value=stuck):
            ctx, _ = memory_sync.run(self.root, env())
        self.assertTrue((self.repo_mem / "user-role.md").exists())
        for index in (self.repo_mem / "MEMORY.md", self.agent / "MEMORY.md"):
            text = index.read_text()
            self.assertNotIn("user-", text)
            self.assertTrue(text.startswith(memory_sync.GENERATED))
        self.assertIn("## Repo memory", ctx)  # the same-session cover
        self.assertNotIn("user-role", ctx)

    def test_an_index_left_with_no_notes_drops_the_moved_ones(self):
        (self.agent / "feedback-b.md").unlink()
        memory_sync.regenerate_index(self.agent)  # an older index that lists the user note
        self.assertIn("user-tz", (self.agent / "MEMORY.md").read_text())
        memory_sync.run(self.root, env())
        self.assertEqual((self.agent / "MEMORY.md").read_text(), memory_sync.GENERATED + "\n")

    def test_render_index_exclude_types_keyword(self):
        self.assertIn("[user-role]", memory_sync.render_index(self.repo_mem))
        text = memory_sync.render_index(self.repo_mem, exclude_types=("user",))
        self.assertNotIn("user-role", text)
        self.assertIn("[project-a]", text)
        self.assertTrue(memory_sync.regenerate_index(self.repo_mem, memory_sync.DEFAULT_BUDGET, ("user",)))
        self.assertNotIn("user-role", (self.repo_mem / "MEMORY.md").read_text())

    def test_a_failing_repo_step_keeps_the_personal_section(self):
        write(self.personal / "user-me.md", note_text("user-me", "user", "Me"))
        with mock.patch.object(memory_sync, "sweep_user_notes",
                               side_effect=PermissionError(13, "denied")):
            ctx, msgs = memory_sync.run(self.root, env())
        self.assertTrue(ctx.startswith("## Personal memory (%s)" % self.personal))
        self.assertIn("[user-me](user-me.md)", ctx)
        self.assertTrue(any("repo memory step failed" in m and "denied" in m for m in msgs), msgs)

    def test_odd_user_types_are_left_out_of_repo_indexes(self):
        write(self.repo_mem / "user-caps.md", note_text("user-caps", "USER", "Caps"))
        text = memory_sync.render_index(self.repo_mem, exclude_types=("user",))
        self.assertNotIn("user-caps", text)
        self.assertNotIn("user-role", text)

    def test_the_personal_section_is_always_there_with_its_path(self):
        root = self.fake_repo("plain-repo", key="example.invalid/acme/other")  # not approved
        ctx, _ = memory_sync.run(root, env())
        self.assertEqual(ctx, personal_only(self.anth))
        self.assertFalse(self.personal.exists())  # never created just to show it
        self.personal.mkdir(parents=True)
        ctx, _ = memory_sync.run(root, env())
        self.assertEqual(ctx, personal_only(self.anth))
        self.assertIn(str(self.personal) + "/user-<slug>.md, never in the repo", ctx)


def _secret_samples():
    """One sample per G6 pattern, built by concatenation so no secret-looking literal is
    committed (gitleaks scans this repo)."""
    return {
        "aws": "AKIA" + "IOSFODNN7EXAMPLE",
        "github token": "ghp" + "_abc123",
        "github pat": "github" + "_pat_" + "abcdef",
        "openai style": "sk" + "-" + "a1b2c3d4e5f6g7h8i9",
        "slack": "xox" + "b-123",
        "private key": "-----BEGIN OPENSSH " + "PRIVATE KEY-----",
        "password": "pass" + "word: hunter2",
        "api key": "API" + "_KEY = abc",
    }


TODAY = "2026-05-06"


class SaveNoteTest(MemCase):
    """`save_note`: the helper behind `anthology-mem save` (plain temp dir, no git needed)."""

    def setUp(self):
        super().setUp()
        self.root = self.tmp / "work"
        self.root.mkdir()
        self.outside = self.tmp / "outside"
        self.outside.mkdir()

    def save(self, **kw):
        args = {"agent": "scout", "type_": "project", "slug": "eval-probe", "name": "Eval probe",
                "description": "How the probe works", "body": "Body text.", "today": TODAY,
                "env": {}}
        args.update(kw)
        return memory_sync.save_note(self.root, **args)

    def agent_dir(self, agent="scout"):
        return self.root / ".claude" / "agent-memory" / f"anthology-{agent}"

    def assertRefused(self, fragment=None, **kw):
        before = snapshot(self.tmp)
        with self.assertRaises(memory_sync.SaveError) as cm:
            self.save(**kw)
        if fragment:
            self.assertIn(fragment, str(cm.exception))
        self.assertEqual(snapshot(self.tmp), before)
        return str(cm.exception)

    def test_save_never_lists_user_notes_in_the_agent_index(self):
        # I1: an agent folder may hold a `user` note (Claude Code's agent memory, or a sweep that
        # has not run in this repo); `save` regenerates the committed index without it.
        write(self.agent_dir() / "user-role.md", note_text("user-role", "user", "Senior engineer"))
        write(self.agent_dir() / "user-tz.md", note_text("user-tz", "User # me", "Works in CET"))
        self.save()
        index = (self.agent_dir() / "MEMORY.md").read_text()
        self.assertIn("[Eval probe](project-eval-probe.md)", index)
        self.assertNotIn("## user", index.lower())
        self.assertNotIn("user-", index)

    def test_privacy_check_is_the_single_wiring_point(self):
        # save runs memory_sync.privacy_findings on every field and refuses on any finding,
        # naming the kind and line but never echoing the text.
        home = "/" + "Users" + "/alice/notes"
        msg = self.assertRefused("personal or private data", body="Line one.\nsee " + home + " here")
        self.assertIn("home path (line 2)", msg)
        self.assertNotIn("alice", msg)
        msg = self.assertRefused("email address", description="ping alice" + "@" + "corp-mail.io")
        self.assertNotIn("corp-mail", msg)
        self.assertRefused("personal or private data", name="Box at 192.168.1.20")
        self.save(slug="clean", body="Nothing private; see https://example.com/docs.")
        self.assertTrue((self.agent_dir() / "project-clean.md").is_file())

    def test_save_calls_privacy_findings_for_each_field(self):
        seen = []

        def findings(text, home=None, hostname=None):
            seen.append(text)
            return []

        with mock.patch.object(memory_sync, "privacy_findings", findings):
            self.save(slug="spy", body="Body here.")
        self.assertEqual(seen, ["spy", "Eval probe", "How the probe works", "Body here.\n"])

    def test_creates_the_note_with_the_exact_shape(self):
        old_umask = os.umask(0o022)  # the mode below is only exact under the usual umask
        try:
            out = self.save()
        finally:
            os.umask(old_umask)
        path = self.agent_dir() / "project-eval-probe.md"
        self.assertEqual(out["path"], path)
        self.assertEqual(out["root"], self.root)
        self.assertEqual(out["relative"],
                         ".claude/agent-memory/anthology-scout/project-eval-probe.md")
        self.assertEqual(out["action"], "created")
        self.assertIsNone(out["index_warning"])
        self.assertEqual(path.read_text(encoding="utf-8"),
                         "---\nname: 'Eval probe'\ndescription: 'How the probe works'\n"
                         "metadata:\n  type: project\n  modified: 2026-05-06\n---\n\nBody text.\n")
        self.assertEqual(path.stat().st_mode & 0o777, 0o644)

    def test_modified_defaults_to_today(self):
        import datetime
        self.save(today=None)
        text = (self.agent_dir() / "project-eval-probe.md").read_text()
        self.assertIn("  modified: " + datetime.date.today().isoformat() + "\n", text)

    def test_every_type_and_agent_is_accepted(self):
        for type_ in memory_sync.SAVE_TYPES:
            self.save(type_=type_, slug="t")
        for agent in memory_sync.ROSTER:
            if agent != "scout":
                self.save(agent=agent)
            self.assertTrue(list(self.agent_dir(agent).glob("*-*.md")), agent)
        self.assertEqual(len(list(self.agent_dir("scout").glob("*-t.md"))), 3)
        self.assertEqual(memory_sync.SAVE_TYPES, ("project", "feedback", "reference"))

    def test_roster_matches_the_agent_files(self):
        stems = {p.stem for p in (ROOT / "plugins" / "anthology" / "agents").glob("*.md")}
        self.assertEqual(set(memory_sync.ROSTER), stems)
        self.assertEqual(len(memory_sync.ROSTER), len(stems))

    def test_save_uses_the_shared_secret_list(self):
        # One anthology-side list (memory_privacy); test_memory_privacy keeps it a superset of G6.
        self.assertFalse(hasattr(memory_sync, "_SECRET_PATTERNS"))
        shared = {(rx.pattern, rx.flags) for _label, rx in memory_privacy.SECRET_PATTERNS}
        for rx in guard_rules._SECRET_PATTERNS:
            self.assertIn((rx.pattern, rx.flags), shared)

    def test_tricky_values_round_trip_through_the_real_indexer(self):
        values = ["It's a test", "a: b", "# not a comment", 'say "hi"', "a\\nb literal",
                  "back\\slash\\", "[x] and [[y]]", "- leading dash", "'quoted'", "''",
                  "key: 'v' # c", "---", "ünïcode ✓"]
        for i, value in enumerate(values):
            with self.subTest(value=value):
                out = self.save(slug="tricky-%d" % i, name=value, description=value + " d")
                info = memory_index.parse_note(out["path"])
                self.assertEqual(info["type"], "project")
                self.assertEqual(info["name"], value.strip())
                self.assertEqual(info["description"], (value + " d").strip())
                self.assertEqual(info["updated"], TODAY)
                self.assertTrue(info["ok"])

    def test_indexer_build_search_and_memory_md(self):
        for type_ in ("project", "feedback", "reference"):
            self.save(type_=type_, slug="x-" + type_, name="Note " + type_,
                      description="desc " + type_, body="The quokka lives here, " + type_ + ".")
        db = self.tmp / "idx.db"
        memory_index.build([("agent", self.agent_dir())], db)
        rows = memory_index.search("quokka", db)
        self.assertEqual(len(rows), 3)
        self.assertEqual({r["scope"] for r in rows}, {"agent"})
        import sqlite3
        conn = sqlite3.connect(str(db))
        types = {r[0]: r[1] for r in conn.execute("SELECT name, type FROM nodes")}
        conn.close()
        self.assertEqual(types, {"Note project": "project", "Note feedback": "feedback",
                                 "Note reference": "reference"})
        lines = (self.agent_dir() / "MEMORY.md").read_text(encoding="utf-8").splitlines()
        self.assertEqual([l for l in lines if l.startswith("## ")],
                         ["## feedback", "## project", "## reference"])
        self.assertIn("- [Note project](project-x-project.md) — desc project", lines)

    def test_save_regenerates_memory_md_each_time(self):
        self.save(slug="one", name="First")
        self.save(slug="two", name="Second")
        index = (self.agent_dir() / "MEMORY.md").read_text(encoding="utf-8")
        self.assertIn("[First](project-one.md)", index)
        self.assertIn("[Second](project-two.md)", index)

    def test_body_multiline_crlf_and_escapes(self):
        self.save(slug="b1", body="Line one.\nLine two\twith tab.\n\n- item; see [[link]]\n")
        text = (self.agent_dir() / "project-b1.md").read_text()
        self.assertTrue(text.endswith(
            "---\n\nLine one.\nLine two\twith tab.\n\n- item; see [[link]]\n"))
        self.save(slug="b2", body="\n\n  lead kept\r\nsecond\rthird   \n\n\n")
        text = (self.agent_dir() / "project-b2.md").read_bytes().decode()
        self.assertTrue(text.endswith("---\n\n  lead kept\nsecond\nthird\n"))
        self.assertNotIn("\r", text)
        self.save(slug="b3", body=r'printf "a\n" and \t stay literal')
        text = (self.agent_dir() / "project-b3.md").read_text()
        self.assertTrue(text.endswith('printf "a\\n" and \\t stay literal\n'))
        info = memory_index.parse_note(self.agent_dir() / "project-b1.md")
        self.assertIn("Line one.\nLine two", info["body"])

    def test_name_and_description_are_stored_stripped(self):
        out = self.save(name="  Padded name  ", description="padded  ")
        info = memory_index.parse_note(out["path"])
        self.assertEqual((info["name"], info["description"]), ("Padded name", "padded"))

    def test_boundary_lengths_are_accepted(self):
        self.save(slug="a" * 64, name="n" * 120, description="d" * 300, body="x" * 16384)

    def test_unknown_agent_and_type(self):
        msg = self.assertRefused(agent="nobody")
        self.assertIn("scout", msg)
        for agent in ("", "Scout", "../scout", "anthology-scout", "scout/x"):
            self.assertRefused(agent=agent)
        msg = self.assertRefused(type_="user")
        for t in memory_sync.SAVE_TYPES:
            self.assertIn(t, msg)
        for t in ("other", "", "Project", "project/../x"):
            self.assertRefused(type_=t)

    def test_bad_slugs(self):
        for slug in ("../x", "a/b", "A", "-a", "a-", "a--b", "a b", ".", "..", "a.b", "a_b", "",
                     "a" * 65, "x\n", "é"):
            with self.subTest(slug=slug):
                self.assertRefused(slug=slug)
        self.assertFalse((self.root / ".claude").exists())

    def test_bad_name_and_description(self):
        for field in ("name", "description"):
            limit = {"name": 120, "description": 300}[field]
            for value in ("", "   ", "two\nlines", "cr\rhere", "tab\there", "nul\x00",
                          "esc\x1b[0m", "del\x7f", "x" * (limit + 1)):
                with self.subTest(field=field, value=value):
                    self.assertRefused(**{field: value})

    def test_name_and_description_refuse_format_and_separator_characters(self):
        # Cf (bidi overrides, zero-width), U+2028/2029 and U+0085 would hide or reshape the
        # text in MEMORY.md and in a diff.
        for field in ("name", "description"):
            for char in ("\u202e", "\u200b", "\u200d", "\ufeff", "\u2066", "\u2028", "\u2029",
                         "\u0085", "\x9f"):
                with self.subTest(field=field, char=hex(ord(char))):
                    self.assertRefused(**{field: "a" + char + "b"})
            self.save(slug="ok-" + field, **{field: "plain ünïcode ✓ text"})

    def test_body_allows_format_characters_but_not_controls(self):
        self.save(slug="zwj", body="family \U0001F468\u200d\U0001F469 ok\u200b")
        text = (self.agent_dir() / "project-zwj.md").read_text(encoding="utf-8")
        self.assertIn("\u200d", text)
        for value in ("a\x85b", "a\x9fb"):
            with self.subTest(value=repr(value)):
                self.assertRefused(body=value)

    def test_bad_body(self):
        for value in ("", "  \n\n \t ", "nul\x00", "bell\x07", "esc\x1b[0m", "del\x7f",
                      "x" * 16385, "é" * 8193):
            with self.subTest(value=value[:12]):
                self.assertRefused(body=value)

    def test_a_slug_that_repeats_the_type_is_not_doubled(self):
        out = self.save(slug="project-eval-probe")
        self.assertEqual(out["path"], self.agent_dir() / "project-eval-probe.md")
        self.assertEqual(out["relative"],
                         ".claude/agent-memory/anthology-scout/project-eval-probe.md")
        info = memory_index.parse_note(out["path"])
        self.assertEqual(info["type"], "project")
        self.assertFalse((self.agent_dir() / "project-project-eval-probe.md").exists())
        # the same note as slug `eval-probe`: a second save collides
        self.assertRefused("already exists")
        # only the matching type is dropped, and only once
        out = self.save(type_="feedback", slug="project-x")
        self.assertEqual(out["path"].name, "feedback-project-x.md")
        out = self.save(type_="project", slug="project-project")
        self.assertEqual(out["path"].name, "project-project.md")
        out = self.save(type_="reference", slug="reference-reference-x")
        self.assertEqual(out["path"].name, "reference-reference-x.md")
        # a slug that is the bare type, or only starts with the type's letters, stays
        for type_, slug in (("feedback", "feedback"), ("project", "projectile"),
                            ("project", "project2")):
            out = self.save(type_=type_, slug=slug)
            self.assertEqual(out["path"].name, "%s-%s.md" % (type_, slug))

    def test_the_slug_is_checked_for_secrets(self):
        value = "abcdef0123456789abcd"
        msg = self.assertRefused(env={"X_API_TOKEN": value}, slug=value)
        self.assertIn("$X_API_TOKEN", msg)
        self.assertNotIn(value, msg)
        key_like = "sk" + "-" + "a1b2c3d4e5f6g7h8i9"
        msg = self.assertRefused(slug=key_like)
        self.assertIn("secret", msg)
        self.assertNotIn(key_like, msg)
        # the check runs on what the caller passed, before a type prefix is dropped
        self.assertRefused(env={"X_API_TOKEN": "project-" + value}, slug="project-" + value)

    def test_each_secret_pattern_is_refused_everywhere(self):
        samples = _secret_samples()
        self.assertEqual(len(samples), len(guard_rules._SECRET_PATTERNS))
        for label, secret in samples.items():
            for field in ("name", "description", "body"):
                with self.subTest(label=label, field=field):
                    msg = self.assertRefused(**{field: "see " + secret + " here"})
                    self.assertIn("looks like it contains a secret", msg)
                    self.assertNotIn(secret, msg)
        self.assertFalse((self.root / ".claude").exists())

    def test_env_secret_values_are_refused_without_echoing_them(self):
        value = "vA1ue" + "-of-the-thing"
        env = {"MY_API_TOKEN": value, "HOME": "/home/x"}
        for field in ("name", "description", "body"):
            with self.subTest(field=field):
                msg = self.assertRefused(env=env, **{field: "contains " + value + "!"})
                self.assertIn("$MY_API_TOKEN", msg)
                self.assertNotIn(value, msg)
        for var in ("DB_PASSWORD", "x_secret", "SSH_KEY", "THE_PASSWD", "AWS_CREDENTIALS"):
            with self.subTest(var=var):
                self.assertRefused(env={var: value}, body=value)

    def test_env_values_that_are_short_or_not_secret_named_do_not_trigger(self):
        self.save(slug="e1", env={"MY_API_TOKEN": "short"}, body="short appears: short")
        self.save(slug="e2", env={"EDITOR": "vim-editor-long"}, body="I use vim-editor-long")
        self.save(slug="e3", env={"MY_API_TOKEN": "x" * 7}, body="x" * 7)
        self.assertRefused(env={"MY_API_TOKEN": "x" * 8}, body="x" * 8)

    def test_env_defaults_to_the_process_environment(self):
        value = "proc" + "-env-value-123"
        with mock.patch.dict(os.environ, {"SOME_TOKEN": value}):
            with self.assertRaises(memory_sync.SaveError):
                memory_sync.save_note(self.root, "scout", "project", "x", "n", "d", value)

    def test_existing_note_needs_replace(self):
        self.save(body="first")
        path = self.agent_dir() / "project-eval-probe.md"
        before = path.read_bytes()
        msg = self.assertRefused(body="second")
        self.assertIn("--replace", msg)
        self.assertIn(".claude/agent-memory/anthology-scout/project-eval-probe.md", msg)
        self.assertEqual(path.read_bytes(), before)
        out = self.save(body="second", replace=True)
        self.assertEqual(out["action"], "replaced")
        self.assertTrue(path.read_text().endswith("\n\nsecond\n"))
        self.assertEqual(list(self.agent_dir().glob(".tmp-*")), [])

    def test_replace_of_a_missing_note_creates_it(self):
        out = self.save(replace=True)
        self.assertEqual(out["action"], "created")
        self.assertTrue(out["path"].is_file())

    def test_root_must_be_a_directory(self):
        with self.assertRaises(memory_sync.SaveError):
            memory_sync.save_note(self.tmp / "missing", "scout", "project", "x", "n", "d", "b",
                                  env={})
        f = write(self.tmp / "afile", "x")
        with self.assertRaises(memory_sync.SaveError):
            memory_sync.save_note(f, "scout", "project", "x", "n", "d", "b", env={})

    def test_a_symlinked_root_is_resolved_and_paths_below_are_checked(self):
        link = self.tmp / "rootlink"
        os.symlink(str(self.root), str(link))
        out = memory_sync.save_note(link, "scout", "project", "x", "n", "d", "b", today=TODAY,
                                    env={})
        self.assertTrue((self.agent_dir() / "project-x.md").is_file())
        self.assertEqual(out["path"], self.agent_dir() / "project-x.md")

    def test_a_symlinked_or_non_directory_component_is_refused(self):
        places = {
            "claude": lambda r: r / ".claude",
            "memory": lambda r: r / ".claude" / "agent-memory",
            "agent": lambda r: r / ".claude" / "agent-memory" / "anthology-scout",
        }
        for label, where in places.items():
            for replace in (False, True):
                with self.subTest(component=label, replace=replace):
                    root = self.tmp / ("r-%s-%s" % (label, replace))
                    root.mkdir()
                    self.root = root
                    path = where(root)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    outside = self.outside / ("o-%s-%s" % (label, replace))
                    outside.mkdir()
                    os.symlink(str(outside), str(path))
                    before = snapshot(self.tmp)
                    with self.assertRaises(memory_sync.SaveError) as cm:
                        self.save(replace=replace)
                    self.assertIn("symlink or not a folder", str(cm.exception))
                    self.assertEqual(snapshot(self.tmp), before)
                    self.assertEqual(list(outside.iterdir()), [])

    def test_a_dangling_symlink_component_is_refused(self):
        (self.root / ".claude").mkdir()
        os.symlink(str(self.outside / "nope"), str(self.root / ".claude" / "agent-memory"))
        self.assertRefused("symlink or not a folder")
        self.assertFalse((self.outside / "nope").exists())

    def test_a_file_where_a_folder_belongs_is_refused(self):
        write(self.root / ".claude", "not a dir")
        self.assertRefused("symlink or not a folder")
        self.assertEqual((self.root / ".claude").read_text(), "not a dir")

    def test_a_symlinked_target_is_refused_with_and_without_replace(self):
        victim = write(self.outside / "victim.txt", "precious")
        self.agent_dir().mkdir(parents=True)
        target = self.agent_dir() / "project-eval-probe.md"
        os.symlink(str(victim), str(target))
        for replace in (False, True):
            with self.subTest(replace=replace):
                self.assertRefused(replace=replace)
                self.assertEqual(victim.read_text(), "precious")
        # a dangling link at the target is refused too, and nothing is created through it
        os.unlink(str(target))
        os.symlink(str(self.outside / "new.txt"), str(target))
        for replace in (False, True):
            with self.subTest(dangling=replace):
                self.assertRefused(replace=replace)
                self.assertFalse((self.outside / "new.txt").exists())

    def test_a_directory_at_the_target_is_refused(self):
        (self.agent_dir() / "project-eval-probe.md").mkdir(parents=True)
        self.assertRefused(replace=True)
        self.assertRefused(replace=False)

    def test_other_agents_dirs_are_untouched(self):
        self.save(agent="reviewer", body="theirs")
        before = snapshot(self.agent_dir("reviewer"))
        self.save(agent="scout")
        self.assertEqual(snapshot(self.agent_dir("reviewer")), before)

    def test_a_failing_index_is_a_warning_not_a_failure(self):
        with mock.patch.object(memory_sync, "regenerate_index",
                               side_effect=OSError("disk says no")):
            out = self.save()
        self.assertTrue(out["path"].is_file())
        self.assertIn("disk says no", out["index_warning"])
        with mock.patch.object(memory_sync, "regenerate_index",
                               side_effect=memory_sync.UnsafeTarget("MEMORY.md is a link")):
            out = self.save(slug="two")
        self.assertIn("MEMORY.md is a link", out["index_warning"])


def run_with_timeout(fn, seconds=5):
    """Run fn in a daemon thread; returns (finished, result, error). A regression that blocks
    (for example reading a FIFO) fails the test instead of hanging the suite."""
    box = {}

    def target():
        try:
            box["result"] = fn()
        except BaseException as exc:  # noqa: BLE001 - reported to the test
            box["error"] = exc

    t = threading.Thread(target=target, daemon=True)
    t.start()
    t.join(seconds)
    return (not t.is_alive()), box.get("result"), box.get("error")


class SpecialMemoryMdTest(MemCase):
    """A MEMORY.md that is a symlink or a FIFO is replaced, never read or written through."""

    def setUp(self):
        super().setUp()
        self.agent = self.tmp / "agent"
        write(self.agent / "project-a.md", note_text("project-a", desc="Thing A"))
        self.outside = self.tmp / "outside"
        self.outside.mkdir()

    def unblock(self, fifo):
        try:  # let a stuck reader finish so the thread does not outlive the test
            fd = os.open(str(fifo), os.O_WRONLY | os.O_NONBLOCK)
            os.close(fd)
        except OSError:
            pass

    def assertPromptlyRegular(self, call):
        done, result, error = run_with_timeout(call)
        self.assertTrue(done, "regenerating MEMORY.md blocked on a special file")
        self.assertIsNone(error)
        target = self.agent / "MEMORY.md"
        self.assertTrue(target.is_file() and not target.is_symlink())
        self.assertIn("[project-a](project-a.md)", target.read_text())
        return result

    @unittest.skipUnless(hasattr(os, "mkfifo"), "no mkfifo")
    def test_a_fifo_at_memory_md_is_replaced_without_blocking(self):
        fifo = self.agent / "MEMORY.md"
        os.mkfifo(str(fifo))
        self.addCleanup(self.unblock, fifo)
        self.assertTrue(self.assertPromptlyRegular(lambda: memory_sync.regenerate_index(self.agent)))

    @unittest.skipUnless(hasattr(os, "mkfifo"), "no mkfifo")
    def test_a_symlink_to_a_fifo_is_replaced_without_blocking(self):
        fifo = self.outside / "pipe"
        os.mkfifo(str(fifo))
        self.addCleanup(self.unblock, fifo)
        os.symlink(str(fifo), str(self.agent / "MEMORY.md"))
        self.assertTrue(self.assertPromptlyRegular(lambda: memory_sync.regenerate_index(self.agent)))

    def test_a_symlink_to_a_file_with_the_same_text_is_still_replaced(self):
        memory_sync.regenerate_index(self.agent)
        text = (self.agent / "MEMORY.md").read_bytes()
        victim = write(self.outside / "victim.md", "")
        victim.write_bytes(text)
        (self.agent / "MEMORY.md").unlink()
        os.symlink(str(victim), str(self.agent / "MEMORY.md"))
        self.assertTrue(self.assertPromptlyRegular(lambda: memory_sync.regenerate_index(self.agent)))
        self.assertEqual(victim.read_bytes(), text)

    def test_a_symlink_to_a_different_file_leaves_the_target_alone(self):
        victim = write(self.outside / "victim.md", "precious")
        os.symlink(str(victim), str(self.agent / "MEMORY.md"))
        self.assertPromptlyRegular(lambda: memory_sync.regenerate_index(self.agent))
        self.assertEqual(victim.read_text(), "precious")

    @unittest.skipUnless(hasattr(os, "mkfifo"), "no mkfifo")
    def test_save_note_returns_promptly_with_a_fifo_memory_md(self):
        root = self.tmp / "work"
        agent_dir = root / ".claude" / "agent-memory" / "anthology-scout"
        agent_dir.mkdir(parents=True)
        fifo = agent_dir / "MEMORY.md"
        os.mkfifo(str(fifo))
        self.addCleanup(self.unblock, fifo)
        done, out, error = run_with_timeout(lambda: memory_sync.save_note(
            root, "scout", "project", "x", "n", "d", "b", env={}))
        self.assertTrue(done, "save_note blocked on a FIFO MEMORY.md")
        self.assertIsNone(error)
        self.assertIsNone(out["index_warning"])
        self.assertTrue(fifo.is_file() and not fifo.is_symlink())


if __name__ == "__main__":
    unittest.main()
