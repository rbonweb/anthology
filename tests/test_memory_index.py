"""Tests for scripts/memory_index.py (SQLite graph + search) and the bin/anthology-mem CLI."""
import datetime
import json
import os
import sqlite3
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

from _mem import BIN, MemCase, git, needs_git, note_text, snapshot, write

import memory_index  # noqa: E402  (path set up by _mem)
import memory_sync  # noqa: E402

try:
    _probe = sqlite3.connect(":memory:")
    _probe.execute("CREATE VIRTUAL TABLE t USING fts5(a)")
    HAVE_FTS5 = True
except sqlite3.Error:
    HAVE_FTS5 = False
needs_fts5 = unittest.skipUnless(HAVE_FTS5, "this sqlite has no FTS5")


class ParseNoteTest(MemCase):
    def parse(self, text, name="stem.md"):
        return memory_index.parse_note(write(self.tmp / name, text))

    def test_real_claude_code_shape(self):
        n = self.parse(note_text("Pretty", "feedback", "Do the thing", "2026-03-04",
                                 body="Hello [[other]] and [[third|alias]].\n",
                                 relations={"supersedes": ["old-a"], "depends_on": ["x", "y"]}))
        self.assertEqual((n["name"], n["type"], n["updated"], n["description"]),
                         ("Pretty", "feedback", "2026-03-04", "Do the thing"))
        self.assertEqual(n["relations"], {"supersedes": ["old-a"], "depends_on": ["x", "y"]})
        self.assertIn("Hello [[other]]", n["body"])
        self.assertNotIn("name: Pretty", n["body"])
        self.assertEqual(n["stem"], "stem")

    def test_quotes_stripped_and_block_lists(self):
        n = self.parse('---\nname: "Quoted Name"\ndescription: \'single\'\nmetadata:\n'
                       '  type: "reference"\n  modified: "2026-01-02"\n  relations:\n'
                       '    supersedes:\n      - old-a\n      - "old-b"\n'
                       '    relates_to: [z]\n---\nbody\n')
        self.assertEqual((n["name"], n["description"], n["type"], n["updated"]),
                         ("Quoted Name", "single", "reference", "2026-01-02"))
        self.assertEqual(n["relations"], {"supersedes": ["old-a", "old-b"], "relates_to": ["z"]})

    def test_fallbacks(self):
        n = self.parse("---\ndescription: d\ntype: user\nupdated: 2025-05-05\n---\nbody\n", "my-note.md")
        self.assertEqual((n["name"], n["type"], n["updated"]), ("my-note", "user", "2025-05-05"))
        n = self.parse("---\nname: n\nmetadata:\n  type: project\n  updated: 2025-01-01\n"
                       "modified: 2026-01-01\n---\n")
        self.assertEqual((n["type"], n["updated"]), ("project", "2025-01-01"))
        n = self.parse("---\nname: n\nmodified: 2026-01-01\n---\n")
        self.assertEqual((n["type"], n["updated"]), ("other", "2026-01-01"))
        n = self.parse("---\nname: n\ntype: user\nmetadata:\n  type: feedback\n---\n")
        self.assertEqual(n["type"], "feedback")

    def test_no_or_broken_frontmatter_is_listed_by_stem(self):
        n = self.parse("just text\n", "plain.md")
        self.assertEqual((n["name"], n["type"], n["updated"], n["relations"]),
                         ("plain", "other", "", {}))
        self.assertEqual(n["body"], "just text\n")
        n = self.parse("---\nname: [unclosed\n  : :\n", "broken.md")
        self.assertEqual((n["name"], n["type"]), ("broken", "other"))
        n = self.parse("---\r\nname: crlf\r\n---\r\nbody\r\n")
        self.assertEqual(n["name"], "crlf")

    def test_folded_description(self):
        n = self.parse("---\nname: n\ndescription: >\n  first line\n  second line\n---\nb\n")
        self.assertEqual(n["description"], "first line second line")


class IndexCase(MemCase):
    def setUp(self):
        super().setUp()
        self.repo = self.tmp / "work" / ".claude" / "memory"
        self.agent = self.tmp / "work" / ".claude" / "agent-memory" / "anthology-writer"
        self.personal = self.anth / "personal-memory"
        self.db = self.data / "memory-index" / "test.db"

    def sources(self):
        return [("repo", self.repo), ("agent", self.agent), ("personal", self.personal)]

    def add(self, directory, fname, **kw):
        name = kw.pop("name", Path(fname).stem)
        return write(directory / fname, note_text(name, **kw))

    def build(self):
        return memory_index.build(self.sources(), self.db)

    def seed_graph(self):
        self.add(self.repo, "a.md", body="Links to [[b]]. Contains the word zebra.\n")
        self.add(self.repo, "b.md", relations={"depends_on": ["c"]}, body="Plain b.\n")
        self.add(self.repo, "c.md", relations={"relates_to": ["d"]}, body="Plain c.\n")
        self.add(self.repo, "d.md", body="Plain d, a zebra crossing.\n")
        self.add(self.repo, "e.md", body="Nobody links here.\n")


class BuildTest(IndexCase):
    def test_stats_and_incremental(self):
        self.seed_graph()
        self.add(self.agent, "agent-note.md")
        self.add(self.personal, "user-me.md", type_="user")
        s = self.build()
        self.assertEqual((s["added"], s["updated"], s["removed"], s["rebuilt"]), (7, 0, 0, False))
        s = self.build()
        self.assertEqual((s["added"], s["updated"], s["removed"], s["rebuilt"]), (0, 0, 0, False))
        # touching without changing content is not an update
        p = self.repo / "a.md"
        st = p.stat()
        os.utime(p, ns=(st.st_atime_ns, st.st_mtime_ns + 10_000_000_000))
        self.assertEqual(self.build()["updated"], 0)
        # a real edit is
        p.write_text(p.read_text() + "Extra gnu content.\n")
        s = self.build()
        self.assertEqual((s["added"], s["updated"], s["removed"]), (0, 1, 0))
        self.assertEqual([r["name"] for r in memory_index.search("gnu", self.db)], ["a"])
        # deletion removes nodes, edges and search hits
        (self.repo / "a.md").unlink()
        s = self.build()
        self.assertEqual((s["added"], s["updated"], s["removed"]), (0, 0, 1))
        self.assertEqual(memory_index.search("gnu", self.db), [])
        self.assertEqual(memory_index.search("zebra", self.db)[0]["name"], "d")

    def test_missing_source_dirs_are_fine(self):
        self.add(self.repo, "a.md")
        s = self.build()
        self.assertEqual(s["added"], 1)

    def test_schema_version_bump_forces_full_rebuild(self):
        self.seed_graph()
        self.build()
        self.assertEqual(sqlite3.connect(str(self.db)).execute("PRAGMA user_version").fetchone()[0],
                         memory_index.SCHEMA_VERSION)
        with mock.patch.object(memory_index, "SCHEMA_VERSION", memory_index.SCHEMA_VERSION + 1):
            s = self.build()
            self.assertTrue(s["rebuilt"])
            self.assertEqual(s["added"], 5)
            self.assertEqual(sqlite3.connect(str(self.db)).execute("PRAGMA user_version").fetchone()[0],
                             memory_index.SCHEMA_VERSION)
            self.assertFalse(self.build()["rebuilt"])
        self.assertEqual(sorted(r["name"] for r in memory_index.search("zebra", self.db)), ["a", "d"])

    def test_foreign_or_corrupt_db_is_rebuilt(self):
        self.seed_graph()
        self.db.parent.mkdir(parents=True)
        self.db.write_bytes(b"this is not a database at all" * 50)
        s = self.build()
        self.assertTrue(s["rebuilt"])
        self.assertEqual(s["added"], 5)

    def test_db_never_lands_in_a_source_or_its_git_tree(self):
        self.seed_graph()
        with self.assertRaises(ValueError):
            memory_index.build(self.sources(), self.repo / "index.db")
        # a repo work tree (a .git entry above the source) is off limits as well
        (self.tmp / "work" / ".git").mkdir()
        with self.assertRaises(ValueError):
            memory_index.build(self.sources(), self.tmp / "work" / "elsewhere" / "index.db")
        self.assertFalse((self.tmp / "work" / "elsewhere").exists())
        before = snapshot(self.tmp / "work")
        memory_index.build(self.sources(), self.db)
        self.assertTrue(self.db.exists())
        self.assertEqual(snapshot(self.tmp / "work"), before)

    def test_personal_source_may_sit_in_a_git_work_tree(self):
        """R23a: a HOME that is a dotfiles git repo must not break the personal index."""
        (self.tmp / ".git").mkdir()  # self.tmp holds home, data and everything else
        self.add(self.personal, "user-me.md", type_="user")
        s = memory_index.build([("personal", self.personal)], self.db)
        self.assertEqual(s["added"], 1)
        with self.assertRaises(ValueError):  # still never inside the notes folder itself
            memory_index.build([("personal", self.personal)], self.personal / "index.db")
        # repo and agent sources keep the work-tree refusal
        self.add(self.repo, "a.md")
        with self.assertRaises(ValueError):
            memory_index.build([("repo", self.repo)], self.data / "other.db")

    def test_db_path_for(self):
        p = memory_index.db_path_for(self.data, "github.com/acme/repo")
        self.assertEqual(p.parent, self.data / "memory-index")
        self.assertRegex(p.name, r"^acme-repo-[0-9a-f]{12}\.db$")
        self.assertEqual(p, memory_index.db_path_for(self.data, "github.com/acme/repo"))
        self.assertNotEqual(p, memory_index.db_path_for(self.data, "github.com/acme/repo2"))
        evil = memory_index.db_path_for(self.data, "../../../etc/pass wd;rm -rf")
        self.assertEqual(evil.parent, self.data / "memory-index")
        self.assertRegex(evil.name, r"^[a-z0-9._-]+\.db$")
        self.assertFalse(evil.name.startswith((".", "-")))
        long_key = "host/" + "x" * 200 + "/" + "y" * 200
        self.assertLessEqual(len(memory_index.db_path_for(self.data, long_key).name), 40 + 1 + 12 + 3)
        self.assertRegex(memory_index.db_path_for(self.data, "path:/tmp/a b/c").name,
                         r"^[a-z0-9._-]+-[0-9a-f]{12}\.db$")


class QueryTest(IndexCase):
    def setUp(self):
        super().setUp()
        self.seed_graph()
        self.build()

    def names(self, rows):
        return [r["name"] for r in rows]

    def test_search_finds_a_body_term(self):
        rows = memory_index.search("zebra", self.db)
        self.assertEqual(sorted(self.names(rows)), ["a", "d"])
        self.assertTrue(all(r["scope"] == "repo" for r in rows))
        self.assertTrue(all(r["file"].endswith(".md") for r in rows))
        self.assertTrue(all("zebra" in r["snippet"] for r in rows))

    def test_search_terms_are_anded_and_case_insensitive(self):
        self.assertEqual(self.names(memory_index.search("ZEBRA crossing", self.db)), ["d"])
        self.assertEqual(memory_index.search("zebra nonexistentword", self.db), [])
        self.assertEqual(memory_index.search("", self.db), [])
        self.assertEqual(memory_index.search("   ", self.db), [])

    def test_search_matches_names_and_descriptions(self):
        self.add(self.repo, "feedback-tabs.md", type_="feedback", desc="Prefer tabs everywhere")
        self.build()
        self.assertEqual(self.names(memory_index.search("tabs", self.db)), ["feedback-tabs"])

    def test_fts_syntax_in_user_input_cannot_break_search(self):
        for q in ('"', '"unbalanced', "zebra AND", "OR", "NEAR(", "a*", "-zebra", "col:zebra",
                  "zebra)", "'; DROP TABLE nodes; --", "zebra^", "{x}"):
            memory_index.search(q, self.db)  # must not raise
        self.assertEqual(memory_index.search("'; DROP TABLE nodes; --", self.db), [])
        self.assertTrue(memory_index.search("zebra", self.db))

    def test_related_follows_links_and_typed_relations_to_depth_two(self):
        rows = memory_index.related("a", db_path=self.db)
        got = {r["name"]: (r["kind"], r["distance"]) for r in rows}
        self.assertEqual(got, {"b": ("links", 1), "c": ("depends_on", 2)})
        got3 = {r["name"]: r["distance"] for r in memory_index.related("a", depth=3, db_path=self.db)}
        self.assertEqual(got3, {"b": 1, "c": 2, "d": 3})
        self.assertEqual(self.names(memory_index.related("a", depth=1, db_path=self.db)), ["b"])
        self.assertEqual(memory_index.related("e", db_path=self.db), [])

    def test_related_walks_edges_in_both_directions(self):
        got = {r["name"]: r["distance"] for r in memory_index.related("c", depth=1, db_path=self.db)}
        self.assertEqual(got, {"b": 1, "d": 1})
        got = {r["name"]: r["distance"] for r in memory_index.related("d", depth=2, db_path=self.db)}
        self.assertEqual(got, {"c": 1, "b": 2})

    def test_related_unknown_note_is_empty(self):
        self.assertEqual(memory_index.related("no-such-note", db_path=self.db), [])

    def test_related_resolves_by_file_stem(self):
        self.add(self.repo, "file-stem.md", name="Pretty Name")
        self.add(self.repo, "linker.md", body="See [[file-stem]] and [[Pretty Name]].\n")
        self.build()
        rows = memory_index.related("linker", depth=1, db_path=self.db)
        self.assertEqual(self.names(rows), ["Pretty Name"])
        self.assertEqual(self.names(memory_index.related("file-stem", depth=1, db_path=self.db)),
                         ["linker"])

    def test_dangling_links_are_ignored(self):
        self.add(self.repo, "ghosty.md", body="Points at [[ghost]] and [[ghost]].\n",
                 relations={"supersedes": ["phantom"]})
        self.build()
        self.assertEqual(memory_index.related("ghosty", db_path=self.db), [])
        self.assertIn("ghosty", self.names(memory_index.orphans(self.db)))
        self.assertNotIn("ghost", self.names(memory_index.orphans(self.db)))
        self.assertEqual(memory_index.stale(self.db), [])

    def test_unknown_relation_kinds_are_ignored(self):
        self.add(self.repo, "odd.md", relations={"likes": ["a"]})
        self.build()
        self.assertEqual(memory_index.related("odd", db_path=self.db), [])

    def test_prefers_same_scope_when_names_collide(self):
        self.add(self.repo, "dup.md", desc="repo dup")
        self.add(self.personal, "dup.md", desc="personal dup", type_="user")
        self.add(self.repo, "src.md", body="[[dup]]\n")
        self.add(self.personal, "psrc.md", body="[[dup]]\n", type_="user")
        self.build()
        r = memory_index.related("src", depth=1, db_path=self.db)
        self.assertEqual([(x["name"], x["scope"]) for x in r], [("dup", "repo")])
        r = memory_index.related("psrc", depth=1, db_path=self.db)
        self.assertEqual([(x["name"], x["scope"]) for x in r], [("dup", "personal")])

    def test_orphans_have_no_incoming_edge(self):
        # a is linked by nobody, b by a, c by b, d by c, e by nobody
        self.assertEqual(self.names(memory_index.orphans(self.db)), ["a", "e"])
        self.add(self.repo, "f.md", body="[[e]]\n")
        self.build()
        self.assertEqual(self.names(memory_index.orphans(self.db)), ["a", "f"])

    def test_stale_superseded_and_old(self):
        self.add(self.repo, "old-rule.md", updated="2026-05-01")
        self.add(self.repo, "new-rule.md", updated="2026-05-02", relations={"supersedes": ["old-rule"]})
        self.add(self.repo, "ancient.md", updated="2020-01-01")
        self.add(self.repo, "fresh.md", updated="2026-05-01")
        self.build()
        today = datetime.date(2026, 6, 1)
        rows = memory_index.stale(self.db, days=180, today=today)
        got = {r["name"]: r["reason"] for r in rows}
        self.assertEqual(set(got), {"old-rule", "ancient"})
        self.assertEqual(got["old-rule"], "superseded")
        self.assertIn("old", got["ancient"])
        # undated notes are not stale; a tighter window catches more
        rows = memory_index.stale(self.db, days=10, today=today)
        self.assertIn("fresh", self.names(rows))
        self.assertNotIn("a", self.names(rows))
        self.assertEqual(self.names(memory_index.stale(self.db, days=180, today="2026-06-01")),
                         self.names(memory_index.stale(self.db, days=180, today=today)))


class FallbackTest(IndexCase):
    def test_like_fallback_when_fts5_is_unavailable(self):
        self.seed_graph()
        with mock.patch.object(memory_index, "_fts5_available", return_value=False):
            self.build()
            sql = sqlite3.connect(str(self.db)).execute(
                "SELECT sql FROM sqlite_master WHERE name='notes_fts'").fetchone()[0]
            self.assertNotIn("VIRTUAL", sql.upper())
            rows = memory_index.search("zebra crossing", self.db)
            self.assertEqual([r["name"] for r in rows], ["d"])
            self.assertIn("zebra", rows[0]["snippet"])
            self.assertEqual(sorted(r["name"] for r in memory_index.search("ZEBRA", self.db)), ["a", "d"])
            for q in ('"', "100%", "a_b", "zebra AND", "'; --"):
                memory_index.search(q, self.db)
            self.assertEqual(memory_index.search("100%", self.db), [])
            # graph queries do not depend on FTS
            self.assertEqual([r["name"] for r in memory_index.related("a", depth=1, db_path=self.db)], ["b"])
            self.assertEqual([r["name"] for r in memory_index.orphans(self.db)], ["a", "e"])

    @needs_fts5
    def test_fts_table_is_used_when_available(self):
        self.seed_graph()
        self.build()
        sql = sqlite3.connect(str(self.db)).execute(
            "SELECT sql FROM sqlite_master WHERE name='notes_fts'").fetchone()[0]
        self.assertIn("VIRTUAL", sql.upper())
        self.assertIn("fts5", sql.lower())
        # simple stemming: "crossing" also finds "crossings"
        self.add(self.repo, "plural.md", body="Many crossings here.\n")
        self.build()
        self.assertIn("plural", [r["name"] for r in memory_index.search("crossing", self.db)])


def cli_env(home, config, extra=None):
    env = {k: v for k, v in os.environ.items()
           if k not in ("CLAUDE_PLUGIN_DATA", "CLAUDE_CODE_DISABLE_AUTO_MEMORY")}
    env.update({"HOME": str(home), "CLAUDE_CONFIG_DIR": str(config),
                "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull})
    env.update(extra or {})
    return env


class CliTest(MemCase):
    def setUp(self):
        super().setUp()
        self.wrapper = BIN / "anthology-mem"
        self.cwd = self.tmp / "plain"
        self.cwd.mkdir()
        write(self.anth / "state.json", json.dumps({"data_dir": str(self.data)}))
        self.personal = self.anth / "personal-memory"
        write(self.personal / "user-me.md",
              note_text("user-me", "user", "About me", body="Likes the quokka. See [[user-pref]].\n"))
        write(self.personal / "user-pref.md", note_text("user-pref", "user", "Prefs",
                                                        updated="2019-01-01", body="Terse.\n"))

    def run_cli(self, *args, cwd=None, env=None, check=True):
        p = subprocess.run([str(self.wrapper)] + list(args), cwd=str(cwd or self.cwd),
                           env=env or cli_env(self.home, self.config), stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, text=True)
        if check:
            self.assertEqual(p.returncode, 0, p.stderr)
        return p

    def test_wrapper_is_an_executable_posix_sh_script(self):
        self.assertTrue(os.access(str(self.wrapper), os.X_OK))
        self.assertEqual(self.wrapper.read_text().splitlines()[0], "#!/bin/sh")

    def test_search_json_builds_the_index_in_the_data_dir(self):
        p = self.run_cli("search", "quokka", "--json")
        rows = json.loads(p.stdout)
        self.assertEqual([r["name"] for r in rows], ["user-me"])
        self.assertEqual(rows[0]["scope"], "personal")
        dbs = list((self.data / "memory-index").glob("*.db"))
        self.assertEqual(len(dbs), 1)
        self.assertEqual(list(self.cwd.iterdir()), [])

    def test_search_text_output(self):
        p = self.run_cli("search", "quokka")
        self.assertIn("user-me", p.stdout)
        self.assertIn("quokka", p.stdout)

    def test_related_orphans_stale_rebuild(self):
        rel = json.loads(self.run_cli("related", "user-me", "--depth", "1", "--json").stdout)
        self.assertEqual([r["name"] for r in rel], ["user-pref"])
        orphans = json.loads(self.run_cli("orphans", "--json").stdout)
        self.assertEqual([r["name"] for r in orphans], ["user-me"])
        stale = json.loads(self.run_cli("stale", "--json").stdout)
        self.assertEqual([r["name"] for r in stale], ["user-pref"])
        self.run_cli("rebuild")
        self.assertEqual(len(list((self.data / "memory-index").glob("*.db"))), 1)
        self.assertEqual([r["name"] for r in json.loads(self.run_cli("search", "quokka", "--json").stdout)],
                         ["user-me"])

    def test_every_query_sees_fresh_notes(self):
        self.run_cli("search", "quokka")
        write(self.personal / "user-new.md", note_text("user-new", "user", "New", body="A wombat.\n"))
        rows = json.loads(self.run_cli("search", "wombat", "--json").stdout)
        self.assertEqual([r["name"] for r in rows], ["user-new"])

    def test_data_dir_falls_back_to_plugin_data_env(self):
        (self.anth / "state.json").unlink()
        alt = self.tmp / "plugin-data"
        env = cli_env(self.home, self.config, {"CLAUDE_PLUGIN_DATA": str(alt)})
        self.run_cli("search", "quokka", env=env)
        self.assertEqual(len(list((alt / "memory-index").glob("*.db"))), 1)

    def test_without_a_data_dir_it_asks_for_setup(self):
        (self.anth / "state.json").unlink()
        p = self.run_cli("search", "quokka", check=False)
        self.assertEqual(p.returncode, 1)
        self.assertIn("anthology-mem: run /anthology:setup first", p.stdout + p.stderr)

    def test_missing_python3_message_and_exit_code(self):
        empty = self.tmp / "emptybin"
        empty.mkdir()
        p = subprocess.run(["/bin/sh", str(self.wrapper), "orphans"],
                           env={"PATH": str(empty)}, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, text=True)
        self.assertEqual(p.returncode, 127)
        self.assertIn("anthology-mem: python3 not found", p.stderr)

    def test_wrapper_resolves_a_symlinked_path(self):
        link = self.tmp / "linkbin" / "anthology-mem"
        link.parent.mkdir()
        link.symlink_to(self.wrapper)
        p = subprocess.run([str(link), "orphans", "--json"], cwd=str(self.cwd),
                           env=cli_env(self.home, self.config), stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, text=True)
        self.assertEqual(p.returncode, 0, p.stderr)

    def test_unknown_subcommand_fails_cleanly(self):
        p = self.run_cli("frobnicate", check=False)
        self.assertNotEqual(p.returncode, 0)

    @needs_git
    def test_approved_repo_contributes_repo_and_agent_notes(self):
        root = self.make_repo()
        write(root / ".claude" / "memory" / "project-r.md", note_text("project-r", body="Armadillo.\n"))
        write(root / ".claude" / "agent-memory" / "anthology-w" / "feedback-w.md",
              note_text("feedback-w", "feedback", body="Armadillo too.\n"))
        # unapproved: personal only
        self.assertEqual(json.loads(self.run_cli("search", "armadillo", "--json", cwd=root).stdout), [])
        memory_sync.approve(root)
        rows = json.loads(self.run_cli("search", "armadillo", "--json", cwd=root).stdout)
        self.assertEqual({(r["name"], r["scope"]) for r in rows},
                         {("project-r", "repo"), ("feedback-w", "agent")})
        self.assertEqual({r["name"] for r in json.loads(self.run_cli("search", "quokka", "--json",
                                                                       cwd=root).stdout)}, {"user-me"})
        # the db is in the data dir, never in the repo
        self.assertEqual(list(root.rglob("*.db")), [])
        self.assertTrue(list((self.data / "memory-index").glob("*.db")))
        # a subdirectory of the repo resolves to the same repo
        sub = root / "src"
        sub.mkdir()
        rows = json.loads(self.run_cli("search", "armadillo", "--json", cwd=sub).stdout)
        self.assertEqual(len(rows), 2)


@needs_git
class SaveCliTest(MemCase):
    """`anthology-mem save` through the real wrapper; no setup, state file or data dir exists."""

    REL = ".claude/agent-memory/anthology-scout/project-eval-probe.md"

    def setUp(self):
        super().setUp()
        self.wrapper = BIN / "anthology-mem"
        self.repo = self.make_repo()

    def run_cli(self, *args, cwd=None):
        return subprocess.run([str(self.wrapper)] + list(args), cwd=str(cwd or self.repo),
                              env=cli_env(self.home, self.config), stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, text=True)

    def save(self, *extra, **kw):
        flags = {"agent": "scout", "type": "project", "slug": "eval-probe", "name": "Eval probe",
                 "description": "How the probe works", "body": "Body."}
        flags.update(kw)
        argv = ["save"]
        for key, value in flags.items():
            argv += ["--" + key, value]
        return self.run_cli(*argv, *extra)

    def test_saves_without_any_setup(self):
        self.assertFalse(self.anth.exists())
        p = self.save()
        self.assertEqual((p.returncode, p.stdout, p.stderr), (0, "created " + self.REL + "\n", ""))
        note = memory_index.parse_note(self.repo / self.REL)
        self.assertEqual((note["name"], note["type"], note["description"]),
                         ("Eval probe", "project", "How the probe works"))
        self.assertTrue((self.repo / ".claude/agent-memory/anthology-scout/MEMORY.md").is_file())
        self.assertFalse(self.anth.exists())
        self.assertEqual(list(self.data.iterdir()), [])

    def test_saves_to_the_repo_root_from_a_subfolder(self):
        sub = self.repo / "src" / "deep"
        sub.mkdir(parents=True)
        p = self.run_cli("save", "--agent", "scout", "--type", "feedback", "--slug", "x",
                         "--name", "n", "--description", "d", "--body", "b", cwd=sub)
        self.assertEqual(p.stdout, "created .claude/agent-memory/anthology-scout/feedback-x.md\n")
        self.assertTrue((self.repo / ".claude/agent-memory/anthology-scout/feedback-x.md").is_file())
        self.assertFalse((sub / ".claude").exists())

    def test_second_identical_call_fails_and_replace_succeeds(self):
        self.assertEqual(self.save().returncode, 0)
        before = (self.repo / self.REL).read_bytes()
        p = self.save()
        self.assertEqual(p.returncode, 1)
        self.assertEqual(p.stdout, "")
        self.assertTrue(p.stderr.startswith("anthology-mem: "))
        self.assertIn("--replace", p.stderr)
        self.assertEqual((self.repo / self.REL).read_bytes(), before)
        p = self.save("--replace", body="Newer body.")
        self.assertEqual((p.returncode, p.stdout), (0, "replaced " + self.REL + "\n"))
        self.assertIn("Newer body.", (self.repo / self.REL).read_text())

    def test_json_output(self):
        p = self.save("--json")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(json.loads(p.stdout), {"action": "created", "path": self.REL})
        self.assertEqual(list(json.loads(p.stdout)), ["action", "path"])
        p = self.save("--json", "--replace")
        self.assertEqual(json.loads(p.stdout), {"action": "replaced", "path": self.REL})

    def test_outside_a_git_repo_exits_1(self):
        plain = self.tmp / "plain"
        plain.mkdir()
        p = self.run_cli("save", "--agent", "scout", "--type", "project", "--slug", "x",
                         "--name", "n", "--description", "d", "--body", "b", cwd=plain)
        self.assertEqual(p.returncode, 1)
        self.assertEqual(p.stderr, "anthology-mem: not inside a git work tree; memory notes "
                                   "live in the repo\n")
        self.assertEqual(list(plain.iterdir()), [])

    def test_abbreviated_flags_are_rejected(self):
        p = self.save("--rep")
        self.assertEqual(p.returncode, 2)
        self.assertIn("unrecognized arguments", p.stderr)
        self.assertFalse((self.repo / ".claude").exists())

    def test_missing_required_flags_exit_2(self):
        p = self.run_cli("save", "--agent", "scout")
        self.assertEqual(p.returncode, 2)
        self.assertFalse((self.repo / ".claude").exists())

    def test_refusals_exit_1_with_a_clear_message(self):
        p = self.save(type="user")
        self.assertEqual(p.returncode, 1)
        self.assertIn("project", p.stderr)
        p = self.save(agent="nobody")
        self.assertEqual(p.returncode, 1)
        p = self.save(body="see " + "ghp" + "_abc123")
        self.assertEqual(p.returncode, 1)
        self.assertIn("secret", p.stderr)
        self.assertNotIn("ghp" + "_abc123", p.stderr)
        self.assertFalse((self.repo / ".claude").exists())

    def test_a_body_with_literal_newlines_in_one_argument(self):
        body = "First line.\nSecond; with | pipes > and && stuff.\n\n- item $HOME `x` \\n kept"
        p = self.save(body=body)
        self.assertEqual(p.returncode, 0, p.stderr)
        text = (self.repo / self.REL).read_text()
        self.assertTrue(text.endswith("---\n\n" + body + "\n"))

    def test_a_value_starting_with_dashes_is_taken_as_a_value(self):
        p = self.save(body="--not-a-flag text")
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertTrue((self.repo / self.REL).read_text().endswith("\n--not-a-flag text\n"))

    def test_the_save_does_not_touch_other_files(self):
        write(self.repo / "keep.txt", "keep")
        before = snapshot(self.repo)
        self.assertEqual(self.save().returncode, 0)
        after = snapshot(self.repo)
        self.assertEqual({k for k in after if k not in before},
                         {".claude/agent-memory/anthology-scout/MEMORY.md", self.REL})
        self.assertEqual({k: v for k, v in before.items()}, {k: after[k] for k in before})

    def test_save_is_listed_in_the_help_and_wrapper_header(self):
        p = self.run_cli("--help")
        self.assertIn("save", p.stdout)
        self.assertIn("save", self.wrapper.read_text().splitlines()[1])


if __name__ == "__main__":
    unittest.main()
