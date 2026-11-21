//! Port of `tests/test_memory_index.py`: the SQLite graph and search (`anthology_memory::index`)
//! and the `anthology mem` CLI (MI1).
//!
//! - The CLI cases run the built `anthology mem …` (Python ran `bin/anthology-mem`, which
//!   `exec`s the script); the two wrapper cases run the staged `bin/anthology-mem` shim, the
//!   symlinked one with a pinned binary installed in the sandbox.
//! - `SCHEMA_VERSION` patches are `Limits.schema_version`; `os.getcwd` and the privacy-scan
//!   patch go through `MemDeps` (GC-19).
//! - The LIKE fallback (`test_like_fallback_when_fts5_is_unavailable`) is gone: SQLite is
//!   bundled with FTS5 (F-11). The python3-missing case is BS1's wrapper suite (F-7).
#![forbid(unsafe_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anthology_core::Env;
use anthology_core::clock::Date;
use anthology_core::git::RealGit;
use anthology_memory::index::{self, Error, Limits, SCHEMA_VERSION, Source, Stats, notes, repo};
use anthology_pycompat::json::{self, Int, Value};
use anthology_testkit::mem::{MemCase, Note, git, needs_git, note_text, snapshot, write};
use anthology_testkit::run::Output;
use anthology_testkit::sandbox::repo_root;

fn add(dir: &Path, fname: &str, note: Note) -> PathBuf {
    write(&dir.join(fname), &note_text(&note), None)
}

fn note(fname: &str) -> Note {
    Note::new(fname.strip_suffix(".md").unwrap_or(fname))
}

fn user_version(db: &Path) -> i64 {
    let conn = rusqlite::Connection::open(db).expect("open");
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("user_version")
}

fn fts_sql(db: &Path) -> String {
    let conn = rusqlite::Connection::open(db).expect("open");
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE name='notes_fts'",
        [],
        |r| r.get(0),
    )
    .expect("sql")
}

mod parse_note_test {
    use super::*;

    /// The note type as the index stores it (`other` when the note names none).
    fn ty(n: &notes::NoteInfo) -> &str {
        n.kind
            .as_deref()
            .filter(|k| !k.is_empty())
            .unwrap_or("other")
    }

    fn upd(n: &notes::NoteInfo) -> &str {
        n.updated.as_deref().unwrap_or("")
    }

    fn parse(case: &MemCase, text: &str, name: &str) -> notes::NoteInfo {
        notes::parse_note(&write(&case.tmp.join(name), text, None)).expect("parse")
    }

    #[test]
    fn test_real_claude_code_shape() {
        let case = MemCase::new();
        let text = note_text(
            &Note::new("Pretty")
                .type_("feedback")
                .desc("Do the thing")
                .updated("2026-03-04")
                .body("Hello [[other]] and [[third|alias]].\n")
                .relation("supersedes", &["old-a"])
                .relation("depends_on", &["x", "y"]),
        );
        let n = parse(&case, &text, "stem.md");
        assert_eq!(
            (n.name.as_str(), ty(&n), upd(&n), n.description.as_str()),
            ("Pretty", "feedback", "2026-03-04", "Do the thing")
        );
        assert_eq!(n.relations.supersedes, ["old-a"]);
        assert_eq!(n.relations.depends_on, ["x", "y"]);
        assert!(n.relations.relates_to.is_empty());
        assert!(n.body.contains("Hello [[other]]"));
        assert!(!n.body.contains("name: Pretty"));
        // `n["stem"] == "stem"`: the stem is what a note without a name is listed by.
        let bare = parse(&case, "---\ndescription: d\n---\n", "stem.md");
        assert_eq!(bare.name, "stem");
    }

    #[test]
    fn test_quotes_stripped_and_block_lists() {
        let case = MemCase::new();
        let n = parse(
            &case,
            "---\nname: \"Quoted Name\"\ndescription: 'single'\nmetadata:\n  type: \"reference\"\n  \
             modified: \"2026-01-02\"\n  relations:\n    supersedes:\n      - old-a\n      - \"old-b\"\n    \
             relates_to: [z]\n---\nbody\n",
            "stem.md",
        );
        assert_eq!(
            (n.name.as_str(), n.description.as_str(), ty(&n), upd(&n)),
            ("Quoted Name", "single", "reference", "2026-01-02")
        );
        assert_eq!(n.relations.supersedes, ["old-a", "old-b"]);
        assert_eq!(n.relations.relates_to, ["z"]);
        assert!(n.relations.depends_on.is_empty());
    }

    #[test]
    fn test_fallbacks() {
        let case = MemCase::new();
        let n = parse(
            &case,
            "---\ndescription: d\ntype: user\nupdated: 2025-05-05\n---\nbody\n",
            "my-note.md",
        );
        assert_eq!(
            (n.name.as_str(), ty(&n), upd(&n)),
            ("my-note", "user", "2025-05-05")
        );
        let n = parse(
            &case,
            "---\nname: n\nmetadata:\n  type: project\n  updated: 2025-01-01\nmodified: 2026-01-01\n---\n",
            "stem.md",
        );
        assert_eq!((ty(&n), upd(&n)), ("project", "2025-01-01"));
        let n = parse(
            &case,
            "---\nname: n\nmodified: 2026-01-01\n---\n",
            "stem.md",
        );
        assert_eq!((ty(&n), upd(&n)), ("other", "2026-01-01"));
        let n = parse(
            &case,
            "---\nname: n\ntype: user\nmetadata:\n  type: feedback\n---\n",
            "stem.md",
        );
        assert_eq!(ty(&n), "feedback");
    }

    #[test]
    fn test_no_or_broken_frontmatter_is_listed_by_stem() {
        let case = MemCase::new();
        let n = parse(&case, "just text\n", "plain.md");
        assert_eq!((n.name.as_str(), ty(&n), upd(&n)), ("plain", "other", ""));
        assert_eq!(n.relations, notes::Relations::default());
        assert_eq!(n.body, "just text\n");
        let n = parse(&case, "---\nname: [unclosed\n  : :\n", "broken.md");
        assert_eq!((n.name.as_str(), ty(&n)), ("broken", "other"));
        let n = parse(&case, "---\r\nname: crlf\r\n---\r\nbody\r\n", "stem.md");
        assert_eq!(n.name, "crlf");
    }

    #[test]
    fn test_folded_description() {
        let case = MemCase::new();
        let n = parse(
            &case,
            "---\nname: n\ndescription: >\n  first line\n  second line\n---\nb\n",
            "stem.md",
        );
        assert_eq!(n.description, "first line second line");
    }
}

/// `IndexCase`: a repo folder, an agent folder and personal memory, and a database in the data
/// folder.
struct IndexCase {
    case: MemCase,
    repo: PathBuf,
    agent: PathBuf,
    personal: PathBuf,
    db: PathBuf,
}

impl IndexCase {
    fn new() -> IndexCase {
        let case = MemCase::new();
        let repo = case.tmp.join("work/.claude/memory");
        let agent = case.tmp.join("work/.claude/agent-memory/anthology-writer");
        let personal = case.anth.join("personal-memory");
        let db = case.data.join("memory-index/test.db");
        IndexCase {
            case,
            repo,
            agent,
            personal,
            db,
        }
    }

    fn sources(&self) -> Vec<Source> {
        vec![
            Source::new("repo", &self.repo),
            Source::new("agent", &self.agent),
            Source::new("personal", &self.personal),
        ]
    }

    fn build(&self) -> Stats {
        index::build(&self.sources(), &self.db, &Limits::default()).expect("build")
    }

    fn seed_graph(&self) {
        add(
            &self.repo,
            "a.md",
            note("a").body("Links to [[b]]. Contains the word zebra.\n"),
        );
        add(
            &self.repo,
            "b.md",
            note("b").relation("depends_on", &["c"]).body("Plain b.\n"),
        );
        add(
            &self.repo,
            "c.md",
            note("c").relation("relates_to", &["d"]).body("Plain c.\n"),
        );
        add(
            &self.repo,
            "d.md",
            note("d").body("Plain d, a zebra crossing.\n"),
        );
        add(&self.repo, "e.md", note("e").body("Nobody links here.\n"));
    }

    fn search(&self, q: &str) -> Vec<index::Hit> {
        index::search(q, &self.db, 20).expect("search")
    }

    fn related(&self, name: &str, depth: i64) -> Vec<index::Related> {
        index::related(name, depth, &self.db).expect("related")
    }

    fn orphans(&self) -> Vec<String> {
        index::orphans(&self.db)
            .expect("orphans")
            .into_iter()
            .map(|r| r.name)
            .collect()
    }
}

fn counts(s: &Stats) -> (u64, u64, u64, bool) {
    (s.added, s.updated, s.removed, s.rebuilt)
}

mod build_test {
    use super::*;

    #[test]
    fn test_stats_and_incremental() {
        let ic = IndexCase::new();
        ic.seed_graph();
        add(&ic.agent, "agent-note.md", note("agent-note"));
        add(&ic.personal, "user-me.md", note("user-me").type_("user"));
        assert_eq!(counts(&ic.build()), (7, 0, 0, false));
        assert_eq!(counts(&ic.build()), (0, 0, 0, false));
        // touching without changing content is not an update
        let p = ic.repo.join("a.md");
        let mtime = fs::metadata(&p).expect("stat").modified().expect("mtime");
        fs::File::options()
            .write(true)
            .open(&p)
            .expect("open")
            .set_modified(mtime + Duration::from_secs(10))
            .expect("utime");
        assert_eq!(ic.build().updated, 0);
        // a real edit is
        let text = fs::read_to_string(&p).expect("read");
        fs::write(&p, format!("{text}Extra gnu content.\n")).expect("write");
        let s = ic.build();
        assert_eq!((s.added, s.updated, s.removed), (0, 1, 0));
        let names: Vec<String> = ic.search("gnu").into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["a"]);
        // deletion removes nodes, edges and search hits
        fs::remove_file(&p).expect("rm");
        let s = ic.build();
        assert_eq!((s.added, s.updated, s.removed), (0, 0, 1));
        assert!(ic.search("gnu").is_empty());
        assert_eq!(ic.search("zebra")[0].name, "d");
    }

    #[test]
    fn test_missing_source_dirs_are_fine() {
        let ic = IndexCase::new();
        add(&ic.repo, "a.md", note("a"));
        assert_eq!(ic.build().added, 1);
    }

    #[test]
    fn test_schema_version_bump_forces_full_rebuild() {
        let ic = IndexCase::new();
        ic.seed_graph();
        ic.build();
        assert_eq!(user_version(&ic.db), SCHEMA_VERSION);
        let bumped = Limits {
            schema_version: SCHEMA_VERSION + 1,
        };
        let s = index::build(&ic.sources(), &ic.db, &bumped).expect("build");
        assert!(s.rebuilt);
        assert_eq!(s.added, 5);
        assert_eq!(user_version(&ic.db), SCHEMA_VERSION + 1);
        assert!(
            !index::build(&ic.sources(), &ic.db, &bumped)
                .expect("build")
                .rebuilt
        );
        let mut names: Vec<String> = ic.search("zebra").into_iter().map(|r| r.name).collect();
        names.sort();
        assert_eq!(names, ["a", "d"]);
    }

    #[test]
    fn test_foreign_or_corrupt_db_is_rebuilt() {
        let ic = IndexCase::new();
        ic.seed_graph();
        fs::create_dir_all(ic.db.parent().expect("parent")).expect("mkdir");
        fs::write(&ic.db, b"this is not a database at all".repeat(50)).expect("write");
        let s = ic.build();
        assert!(s.rebuilt);
        assert_eq!(s.added, 5);
    }

    /// F-11: an index Python built (`user_version` 1, Python's schema) is rebuilt once.
    #[test]
    fn python_built_db_with_user_version_1_is_rebuilt() {
        let ic = IndexCase::new();
        ic.seed_graph();
        fs::create_dir_all(ic.db.parent().expect("parent")).expect("mkdir");
        {
            let conn = rusqlite::Connection::open(&ic.db).expect("open");
            conn.execute_batch(
                "CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER, size INTEGER, sha256 TEXT);
                 CREATE TABLE nodes(id TEXT PRIMARY KEY, name TEXT, type TEXT, scope TEXT, file TEXT,
                                    description TEXT, updated TEXT);
                 CREATE TABLE refs(src TEXT, target TEXT, kind TEXT);
                 CREATE TABLE edges(src TEXT, dst TEXT, kind TEXT);
                 CREATE TABLE notes_fts(name, description, body, id);
                 INSERT INTO files VALUES ('/gone.md', 1, 1, 'x');
                 PRAGMA user_version = 1;",
            )
            .expect("python schema");
        }
        let s = ic.build();
        assert_eq!(counts(&s), (5, 0, 0, true));
        assert_eq!(user_version(&ic.db), 2);
        assert!(fts_sql(&ic.db).to_uppercase().contains("VIRTUAL"));
        assert!(!ic.build().rebuilt);
    }

    #[test]
    fn test_db_never_lands_in_a_source_or_its_git_tree() {
        let ic = IndexCase::new();
        ic.seed_graph();
        let r = index::build(&ic.sources(), &ic.repo.join("index.db"), &Limits::default());
        assert!(matches!(r, Err(Error::Value(_))), "{r:?}");
        // a repo work tree (a .git entry above the source) is off limits as well
        fs::create_dir(ic.case.tmp.join("work/.git")).expect("mkdir");
        let elsewhere = ic.case.tmp.join("work/elsewhere");
        let r = index::build(
            &ic.sources(),
            &elsewhere.join("index.db"),
            &Limits::default(),
        );
        assert!(matches!(r, Err(Error::Value(_))), "{r:?}");
        assert!(!elsewhere.exists());
        let before = snapshot(&ic.case.tmp.join("work"));
        ic.build();
        assert!(ic.db.exists());
        assert_eq!(snapshot(&ic.case.tmp.join("work")), before);
    }

    #[test]
    fn test_personal_source_may_sit_in_a_git_work_tree() {
        // R23a: a HOME that is a dotfiles git repo must not break the personal index.
        let ic = IndexCase::new();
        fs::create_dir(ic.case.tmp.join(".git")).expect("mkdir");
        add(&ic.personal, "user-me.md", note("user-me").type_("user"));
        let personal = [Source::new("personal", &ic.personal)];
        let s = index::build(&personal, &ic.db, &Limits::default()).expect("build");
        assert_eq!(s.added, 1);
        // still never inside the notes folder itself
        let r = index::build(&personal, &ic.personal.join("index.db"), &Limits::default());
        assert!(matches!(r, Err(Error::Value(_))), "{r:?}");
        // repo and agent sources keep the work-tree refusal
        add(&ic.repo, "a.md", note("a"));
        let r = index::build(
            &[Source::new("repo", &ic.repo)],
            &ic.case.data.join("other.db"),
            &Limits::default(),
        );
        assert!(matches!(r, Err(Error::Value(_))), "{r:?}");
    }

    #[test]
    fn test_db_path_for() {
        let ic = IndexCase::new();
        let data = &ic.case.data;
        let p = index::db_path_for(data, "github.com/acme/repo");
        assert_eq!(p.parent(), Some(data.join("memory-index").as_path()));
        let name = |p: &Path| p.file_name().expect("name").to_string_lossy().into_owned();
        let named = regex::Regex::new(r"^acme-repo-[0-9a-f]{12}\.db$").expect("re");
        assert!(named.is_match(&name(&p)), "{p:?}");
        assert_eq!(p, index::db_path_for(data, "github.com/acme/repo"));
        assert_ne!(p, index::db_path_for(data, "github.com/acme/repo2"));
        let evil = index::db_path_for(data, "../../../etc/pass wd;rm -rf");
        assert_eq!(evil.parent(), Some(data.join("memory-index").as_path()));
        let safe = regex::Regex::new(r"^[a-z0-9._-]+\.db$").expect("re");
        assert!(safe.is_match(&name(&evil)), "{evil:?}");
        assert!(!name(&evil).starts_with(['.', '-']));
        let long_key = format!("host/{}/{}", "x".repeat(200), "y".repeat(200));
        assert!(name(&index::db_path_for(data, &long_key)).chars().count() <= 40 + 1 + 12 + 3);
        let spaced = regex::Regex::new(r"^[a-z0-9._-]+-[0-9a-f]{12}\.db$").expect("re");
        assert!(spaced.is_match(&name(&index::db_path_for(data, "path:/tmp/a b/c"))));
    }
}

fn names<T>(rows: &[T], f: impl Fn(&T) -> &str) -> Vec<String> {
    rows.iter().map(|r| f(r).to_string()).collect()
}

fn seeded() -> IndexCase {
    let ic = IndexCase::new();
    ic.seed_graph();
    ic.build();
    ic
}

mod query_test {
    use super::*;

    fn hit_names(rows: &[index::Hit]) -> Vec<String> {
        names(rows, |r| &r.name)
    }

    fn rel_names(rows: &[index::Related]) -> Vec<String> {
        names(rows, |r| &r.name)
    }

    #[test]
    fn test_search_finds_a_body_term() {
        let ic = seeded();
        let rows = ic.search("zebra");
        let mut n = hit_names(&rows);
        n.sort();
        assert_eq!(n, ["a", "d"]);
        assert!(rows.iter().all(|r| r.scope == "repo"));
        assert!(rows.iter().all(|r| r.file.ends_with(".md")));
        assert!(rows.iter().all(|r| r.snippet.contains("zebra")));
    }

    #[test]
    fn test_search_terms_are_anded_and_case_insensitive() {
        let ic = seeded();
        assert_eq!(hit_names(&ic.search("ZEBRA crossing")), ["d"]);
        assert!(ic.search("zebra nonexistentword").is_empty());
        assert!(ic.search("").is_empty());
        assert!(ic.search("   ").is_empty());
    }

    #[test]
    fn test_search_matches_names_and_descriptions() {
        let ic = seeded();
        add(
            &ic.repo,
            "feedback-tabs.md",
            note("feedback-tabs")
                .type_("feedback")
                .desc("Prefer tabs everywhere"),
        );
        ic.build();
        assert_eq!(hit_names(&ic.search("tabs")), ["feedback-tabs"]);
    }

    #[test]
    fn test_fts_syntax_in_user_input_cannot_break_search() {
        let ic = seeded();
        for q in [
            "\"",
            "\"unbalanced",
            "zebra AND",
            "OR",
            "NEAR(",
            "a*",
            "-zebra",
            "col:zebra",
            "zebra)",
            "'; DROP TABLE nodes; --",
            "zebra^",
            "{x}",
        ] {
            index::search(q, &ic.db, 20).unwrap_or_else(|e| panic!("{q:?}: {e:?}"));
        }
        assert!(ic.search("'; DROP TABLE nodes; --").is_empty());
        assert!(!ic.search("zebra").is_empty());
    }

    #[test]
    fn test_related_follows_links_and_typed_relations_to_depth_two() {
        let ic = seeded();
        let got: Vec<(String, String, i64)> = ic
            .related("a", 2)
            .into_iter()
            .map(|r| (r.name, r.kind, r.distance))
            .collect();
        assert_eq!(
            got,
            [
                ("b".to_string(), "links".to_string(), 1),
                ("c".to_string(), "depends_on".to_string(), 2)
            ]
        );
        let got3: Vec<(String, i64)> = ic
            .related("a", 3)
            .into_iter()
            .map(|r| (r.name, r.distance))
            .collect();
        assert_eq!(got3, [("b".into(), 1), ("c".into(), 2), ("d".into(), 3)]);
        assert_eq!(rel_names(&ic.related("a", 1)), ["b"]);
        assert!(ic.related("e", 2).is_empty());
    }

    #[test]
    fn test_related_walks_edges_in_both_directions() {
        let ic = seeded();
        let got: Vec<(String, i64)> = ic
            .related("c", 1)
            .into_iter()
            .map(|r| (r.name, r.distance))
            .collect();
        assert_eq!(got, [("b".into(), 1), ("d".into(), 1)]);
        let got: Vec<(String, i64)> = ic
            .related("d", 2)
            .into_iter()
            .map(|r| (r.name, r.distance))
            .collect();
        assert_eq!(got, [("c".into(), 1), ("b".into(), 2)]);
    }

    #[test]
    fn test_related_unknown_note_is_empty() {
        let ic = seeded();
        assert!(ic.related("no-such-note", 2).is_empty());
    }

    #[test]
    fn test_related_resolves_by_file_stem() {
        let ic = seeded();
        add(&ic.repo, "file-stem.md", Note::new("Pretty Name"));
        add(
            &ic.repo,
            "linker.md",
            note("linker").body("See [[file-stem]] and [[Pretty Name]].\n"),
        );
        ic.build();
        assert_eq!(rel_names(&ic.related("linker", 1)), ["Pretty Name"]);
        assert_eq!(rel_names(&ic.related("file-stem", 1)), ["linker"]);
    }

    #[test]
    fn test_dangling_links_are_ignored() {
        let ic = seeded();
        add(
            &ic.repo,
            "ghosty.md",
            note("ghosty")
                .body("Points at [[ghost]] and [[ghost]].\n")
                .relation("supersedes", &["phantom"]),
        );
        ic.build();
        assert!(ic.related("ghosty", 2).is_empty());
        assert!(ic.orphans().contains(&"ghosty".to_string()));
        assert!(!ic.orphans().contains(&"ghost".to_string()));
        let today = anthology_core::clock::today_local();
        assert!(
            index::stale(&ic.db, &Int::Small(180), today)
                .expect("stale")
                .is_empty()
        );
    }

    #[test]
    fn test_unknown_relation_kinds_are_ignored() {
        let ic = seeded();
        add(&ic.repo, "odd.md", note("odd").relation("likes", &["a"]));
        ic.build();
        assert!(ic.related("odd", 2).is_empty());
    }

    #[test]
    fn test_prefers_same_scope_when_names_collide() {
        let ic = seeded();
        add(&ic.repo, "dup.md", note("dup").desc("repo dup"));
        add(
            &ic.personal,
            "dup.md",
            note("dup").desc("personal dup").type_("user"),
        );
        add(&ic.repo, "src.md", note("src").body("[[dup]]\n"));
        add(
            &ic.personal,
            "psrc.md",
            note("psrc").body("[[dup]]\n").type_("user"),
        );
        ic.build();
        let pairs = |rows: Vec<index::Related>| -> Vec<(String, String)> {
            rows.into_iter().map(|r| (r.name, r.scope)).collect()
        };
        assert_eq!(
            pairs(ic.related("src", 1)),
            [("dup".to_string(), "repo".to_string())]
        );
        assert_eq!(
            pairs(ic.related("psrc", 1)),
            [("dup".to_string(), "personal".to_string())]
        );
    }

    #[test]
    fn test_orphans_have_no_incoming_edge() {
        // a is linked by nobody, b by a, c by b, d by c, e by nobody
        let ic = seeded();
        assert_eq!(ic.orphans(), ["a", "e"]);
        add(&ic.repo, "f.md", note("f").body("[[e]]\n"));
        ic.build();
        assert_eq!(ic.orphans(), ["a", "f"]);
    }

    #[test]
    fn test_stale_superseded_and_old() {
        let ic = seeded();
        add(
            &ic.repo,
            "old-rule.md",
            note("old-rule").updated("2026-05-01"),
        );
        add(
            &ic.repo,
            "new-rule.md",
            note("new-rule")
                .updated("2026-05-02")
                .relation("supersedes", &["old-rule"]),
        );
        add(
            &ic.repo,
            "ancient.md",
            note("ancient").updated("2020-01-01"),
        );
        add(&ic.repo, "fresh.md", note("fresh").updated("2026-05-01"));
        ic.build();
        let today = Date {
            y: 2026,
            m: 6,
            d: 1,
        };
        let rows = index::stale(&ic.db, &Int::Small(180), today).expect("stale");
        let mut got: Vec<(String, String)> = rows.into_iter().map(|r| (r.name, r.reason)).collect();
        got.sort();
        assert_eq!(
            got.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(),
            ["ancient", "old-rule"]
        );
        assert_eq!(got[1].1, "superseded");
        assert!(got[0].1.contains("old"));
        // undated notes are not stale; a tighter window catches more
        let rows = index::stale(&ic.db, &Int::Small(10), today).expect("stale");
        let n = names(&rows, |r| &r.name);
        assert!(n.contains(&"fresh".to_string()));
        assert!(!n.contains(&"a".to_string()));
        // `today="2026-06-01"`: Python's `_as_date` reads the string with `date.fromisoformat`.
        let from_text = index::fromisoformat("2026-06-01").expect("date");
        assert_eq!(
            index::stale(&ic.db, &Int::Small(180), from_text).expect("stale"),
            index::stale(&ic.db, &Int::Small(180), today).expect("stale")
        );
    }
}

mod fallback_test {
    use super::*;

    #[test]
    fn test_fts_table_is_used_when_available() {
        let ic = IndexCase::new();
        ic.seed_graph();
        ic.build();
        let sql = fts_sql(&ic.db);
        assert!(sql.to_uppercase().contains("VIRTUAL"));
        assert!(sql.to_lowercase().contains("fts5"));
        // simple stemming: "crossing" also finds "crossings"
        add(
            &ic.repo,
            "plural.md",
            note("plural").body("Many crossings here.\n"),
        );
        ic.build();
        assert!(ic.search("crossing").iter().any(|r| r.name == "plural"));
    }
}

// ---- the CLI ---------------------------------------------------------------------------------

fn staged_wrapper() -> PathBuf {
    anthology_testkit::staged("plugins/anthology/bin/anthology-mem")
}

fn run_mem(env: &Env, cwd: &Path, args: &[&str]) -> Output {
    let mut argv = vec!["mem"];
    argv.extend_from_slice(args);
    anthology_testkit::run_bin(&argv, b"", env, cwd)
}

fn ok(out: Output) -> Output {
    assert_eq!(out.code, 0, "{}", out.stderr_text());
    out
}

fn json_names(out: &Output) -> Vec<String> {
    let v = json::loads(&out.stdout_text()).expect("json");
    v.as_array()
        .expect("a list")
        .iter()
        .map(|r| {
            r.as_object()
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                .expect("name")
                .to_string()
        })
        .collect()
}

fn db_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "db"))
        .collect()
}

/// `CliTest`: state.json names the data folder; two personal notes.
struct CliCase {
    case: MemCase,
    cwd: PathBuf,
    personal: PathBuf,
}

impl CliCase {
    fn new() -> CliCase {
        let case = MemCase::new();
        let cwd = case.tmp.join("plain");
        fs::create_dir(&cwd).expect("mkdir");
        let state = format!(
            "{{\"data_dir\": {}}}",
            json::dumps(
                &Value::Str(case.data.to_string_lossy().into_owned()),
                &json::PY_DEFAULT
            )
        );
        write(&case.anth.join("state.json"), &state, None);
        let personal = case.anth.join("personal-memory");
        add(
            &personal,
            "user-me.md",
            Note::new("user-me")
                .type_("user")
                .desc("About me")
                .body("Likes the quokka. See [[user-pref]].\n"),
        );
        add(
            &personal,
            "user-pref.md",
            Note::new("user-pref")
                .type_("user")
                .desc("Prefs")
                .updated("2019-01-01")
                .body("Terse.\n"),
        );
        CliCase {
            case,
            cwd,
            personal,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        ok(run_mem(&self.case.env, &self.cwd, args))
    }

    fn index_dir(&self) -> PathBuf {
        self.case.data.join("memory-index")
    }
}

/// `memory_sync.approve(root)`: records the repo's key in approved-repos.json.
fn approve(case: &MemCase, root: &Path) {
    let key = repo::repo_key(root, &case.env, &RealGit);
    let text = format!(
        "{{\"repos\": {{{}: {{\"approved\": \"2026-01-01\"}}}}, \"version\": 1}}\n",
        json::dumps(&Value::Str(key), &json::PY_DEFAULT)
    );
    write(&case.anth.join("approved-repos.json"), &text, None);
}

mod cli_test {
    use super::*;

    #[test]
    fn test_wrapper_is_an_executable_posix_sh_script() {
        let w = staged_wrapper();
        let mode = fs::metadata(&w).expect("stat").permissions().mode();
        assert_ne!(mode & 0o111, 0, "{w:?} is not executable");
        let text = fs::read_to_string(&w).expect("read");
        assert_eq!(text.lines().next(), Some("#!/bin/sh"));
    }

    #[test]
    fn test_search_json_builds_the_index_in_the_data_dir() {
        let c = CliCase::new();
        let p = c.run(&["search", "quokka", "--json"]);
        let rows = json::loads(&p.stdout_text()).expect("json");
        assert_eq!(json_names(&p), ["user-me"]);
        let scope = rows.as_array().expect("list")[0]
            .as_object()
            .and_then(|m| m.get("scope"))
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(scope.as_deref(), Some("personal"));
        assert_eq!(db_files(&c.index_dir()).len(), 1);
        assert_eq!(fs::read_dir(&c.cwd).expect("ls").count(), 0);
    }

    #[test]
    fn test_search_text_output() {
        let c = CliCase::new();
        let p = c.run(&["search", "quokka"]);
        assert!(p.stdout_text().contains("user-me"));
        assert!(p.stdout_text().contains("quokka"));
    }

    #[test]
    fn test_related_orphans_stale_rebuild() {
        let c = CliCase::new();
        let rel = c.run(&["related", "user-me", "--depth", "1", "--json"]);
        assert_eq!(json_names(&rel), ["user-pref"]);
        assert_eq!(json_names(&c.run(&["orphans", "--json"])), ["user-me"]);
        assert_eq!(json_names(&c.run(&["stale", "--json"])), ["user-pref"]);
        c.run(&["rebuild"]);
        assert_eq!(db_files(&c.index_dir()).len(), 1);
        assert_eq!(
            json_names(&c.run(&["search", "quokka", "--json"])),
            ["user-me"]
        );
    }

    #[test]
    fn test_every_query_sees_fresh_notes() {
        let c = CliCase::new();
        c.run(&["search", "quokka"]);
        add(
            &c.personal,
            "user-new.md",
            Note::new("user-new")
                .type_("user")
                .desc("New")
                .body("A wombat.\n"),
        );
        assert_eq!(
            json_names(&c.run(&["search", "wombat", "--json"])),
            ["user-new"]
        );
    }

    #[test]
    fn test_data_dir_falls_back_to_plugin_data_env() {
        let c = CliCase::new();
        fs::remove_file(c.case.anth.join("state.json")).expect("rm");
        let alt = c.case.tmp.join("plugin-data");
        let env = c.case.env.clone().with_os("CLAUDE_PLUGIN_DATA", &alt);
        ok(run_mem(&env, &c.cwd, &["search", "quokka"]));
        assert_eq!(db_files(&alt.join("memory-index")).len(), 1);
    }

    #[test]
    fn test_without_a_data_dir_it_asks_for_setup() {
        let c = CliCase::new();
        fs::remove_file(c.case.anth.join("state.json")).expect("rm");
        let p = run_mem(&c.case.env, &c.cwd, &["search", "quokka"]);
        assert_eq!(p.code, 1);
        assert!(
            format!("{}{}", p.stdout_text(), p.stderr_text())
                .contains("anthology-mem: run /anthology:setup first")
        );
    }

    /// The staged shim, reached through a symlink, finds its plugin root and runs the pinned
    /// binary (installed here by hand: `<ah>/bin/<version>/anthology` and its `.sha256`).
    #[test]
    fn test_wrapper_resolves_a_symlinked_path() {
        use anthology_core::lock::{Lock, render_anthology};
        use anthology_core::platform::Target;
        let c = CliCase::new();
        let plugin = c.case.tmp.join("plugin");
        anthology_testkit::sandbox::copy_tree(&repo_root().join("plugins/anthology"), &plugin);
        anthology_testkit::sandbox::copy_tree(
            &anthology_testkit::staged("plugins/anthology"),
            &plugin,
        );
        let lock = Lock {
            version: "0.0.1".into(),
            tag: "anthology-bin--v0.0.1".into(),
            repo: "example/anthology".into(),
            sha256: Target::ALL.iter().map(|t| (*t, "0".repeat(64))).collect(),
        };
        fs::write(plugin.join("anthology.lock"), render_anthology(&lock)).expect("lock");
        let vdir = c.case.anth.join("bin/0.0.1");
        fs::create_dir_all(&vdir).expect("mkdir");
        fs::copy(
            anthology_testkit::run::anthology_bin(),
            vdir.join("anthology"),
        )
        .expect("cp");
        fs::write(vdir.join(".sha256"), "0".repeat(64)).expect("sha");
        let link = c.case.tmp.join("linkbin/anthology-mem");
        fs::create_dir(link.parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(plugin.join("bin/anthology-mem"), &link).expect("symlink");
        let p = anthology_testkit::run::run_program(
            &link,
            &["orphans", "--json"],
            b"",
            &c.case.env,
            &c.cwd,
        );
        assert_eq!(p.code, 0, "{}", p.stderr_text());
        assert_eq!(json_names(&p), ["user-me"]);
    }

    #[test]
    fn test_unknown_subcommand_fails_cleanly() {
        let c = CliCase::new();
        let p = run_mem(&c.case.env, &c.cwd, &["frobnicate"]);
        assert_ne!(p.code, 0);
    }

    /// Python prints sqlite3's message alone (rusqlite appends the path to it).
    #[test]
    fn an_unopenable_database_prints_sqlites_message_alone() {
        if anthology_testkit::bs::is_root() {
            return;
        }
        let c = CliCase::new();
        let dir = c.index_dir();
        fs::create_dir_all(&dir).expect("mkdir");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).expect("chmod");
        let p = run_mem(&c.case.env, &c.cwd, &["orphans"]);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(
            (p.code, p.stderr_text()),
            (
                1,
                "anthology-mem: unable to open database file\n".to_string()
            )
        );
    }

    #[test]
    fn test_approved_repo_contributes_repo_and_agent_notes() {
        if !needs_git() {
            return;
        }
        let c = CliCase::new();
        let root = c.case.make_repo("repo", None);
        add(
            &root.join(".claude/memory"),
            "project-r.md",
            Note::new("project-r").body("Armadillo.\n"),
        );
        add(
            &root.join(".claude/agent-memory/anthology-w"),
            "feedback-w.md",
            Note::new("feedback-w")
                .type_("feedback")
                .body("Armadillo too.\n"),
        );
        // unapproved: personal only
        let p = ok(run_mem(
            &c.case.env,
            &root,
            &["search", "armadillo", "--json"],
        ));
        assert!(json_names(&p).is_empty());
        approve(&c.case, &root);
        let p = ok(run_mem(
            &c.case.env,
            &root,
            &["search", "armadillo", "--json"],
        ));
        let rows = json::loads(&p.stdout_text()).expect("json");
        let mut got: Vec<(String, String)> = rows
            .as_array()
            .expect("list")
            .iter()
            .map(|r| {
                let m = r.as_object().expect("row");
                let f = |k: &str| m.get(k).and_then(Value::as_str).expect(k).to_string();
                (f("name"), f("scope"))
            })
            .collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("feedback-w".to_string(), "agent".to_string()),
                ("project-r".to_string(), "repo".to_string())
            ]
        );
        let p = ok(run_mem(&c.case.env, &root, &["search", "quokka", "--json"]));
        assert_eq!(json_names(&p), ["user-me"]);
        // the db is in the data dir, never in the repo
        let repo_dbs: Vec<_> = snapshot(&root)
            .into_keys()
            .filter(|k| k.ends_with(".db"))
            .collect();
        assert!(repo_dbs.is_empty());
        assert!(!db_files(&c.index_dir()).is_empty());
        // a subdirectory of the repo resolves to the same repo
        let sub = root.join("src");
        fs::create_dir(&sub).expect("mkdir");
        let p = ok(run_mem(
            &c.case.env,
            &sub,
            &["search", "armadillo", "--json"],
        ));
        assert_eq!(json_names(&p).len(), 2);
    }
}

// ---- `anthology mem save` --------------------------------------------------------------------

const REL: &str = ".claude/agent-memory/anthology-scout/project-eval-probe.md";

struct SaveCase {
    case: MemCase,
    repo: PathBuf,
}

impl SaveCase {
    fn new() -> SaveCase {
        let case = MemCase::new();
        let repo = case.make_repo("repo", None);
        SaveCase { case, repo }
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> Output {
        run_mem(&self.case.env, cwd, args)
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_in(&self.repo, args)
    }

    /// `save(*extra, **kw)`: the default flags with `kw` replacing some, then `extra`.
    fn save(&self, extra: &[&str], kw: &[(&str, &str)]) -> Output {
        let mut flags = vec![
            ("agent", "scout"),
            ("type", "project"),
            ("slug", "eval-probe"),
            ("name", "Eval probe"),
            ("description", "How the probe works"),
            ("body", "Body."),
        ];
        for (k, v) in kw {
            match flags.iter_mut().find(|f| f.0 == *k) {
                Some(f) => f.1 = v,
                None => flags.push((k, v)),
            }
        }
        let mut argv: Vec<String> = vec!["save".into()];
        for (k, v) in flags {
            argv.push(format!("--{k}"));
            argv.push(v.to_string());
        }
        argv.extend(extra.iter().map(|s| s.to_string()));
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        self.run(&argv)
    }

    fn repo_text(&self) -> String {
        self.repo.display().to_string()
    }
}

mod save_cli_test {
    use super::*;

    #[test]
    fn test_saves_without_any_setup() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        assert!(!s.case.anth.exists());
        let p = s.save(&[], &[]);
        assert_eq!(
            (p.code, p.stdout_text(), p.stderr_text()),
            (
                0,
                format!("created {REL} in {}\n", s.repo_text()),
                String::new()
            )
        );
        let n = notes::parse_note(&s.repo.join(REL)).expect("note");
        assert_eq!(
            (n.name.as_str(), n.kind.as_deref(), n.description.as_str()),
            ("Eval probe", Some("project"), "How the probe works")
        );
        assert!(
            s.repo
                .join(".claude/agent-memory/anthology-scout/MEMORY.md")
                .is_file()
        );
        assert!(!s.case.anth.exists());
        assert_eq!(fs::read_dir(&s.case.data).expect("ls").count(), 0);
    }

    #[test]
    fn test_saves_to_the_repo_root_from_a_subfolder() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let sub = s.repo.join("src/deep");
        fs::create_dir_all(&sub).expect("mkdir");
        let p = s.run_in(
            &sub,
            &[
                "save",
                "--agent",
                "scout",
                "--type",
                "feedback",
                "--slug",
                "x",
                "--name",
                "n",
                "--description",
                "d",
                "--body",
                "b",
            ],
        );
        assert_eq!(
            p.stdout_text(),
            format!(
                "created .claude/agent-memory/anthology-scout/feedback-x.md in {}\n",
                s.repo_text()
            )
        );
        assert!(
            s.repo
                .join(".claude/agent-memory/anthology-scout/feedback-x.md")
                .is_file()
        );
        assert!(!sub.join(".claude").exists());
    }

    #[test]
    fn test_second_identical_call_fails_and_replace_succeeds() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        assert_eq!(s.save(&[], &[]).code, 0);
        let before = fs::read(s.repo.join(REL)).expect("read");
        let p = s.save(&[], &[]);
        assert_eq!(p.code, 1);
        assert_eq!(p.stdout_text(), "");
        assert!(p.stderr_text().starts_with("anthology-mem: "));
        assert!(p.stderr_text().contains("--replace"));
        assert_eq!(fs::read(s.repo.join(REL)).expect("read"), before);
        let p = s.save(&["--replace"], &[("body", "Newer body.")]);
        assert_eq!(
            (p.code, p.stdout_text()),
            (0, format!("replaced {REL} in {}\n", s.repo_text()))
        );
        assert!(
            fs::read_to_string(s.repo.join(REL))
                .expect("read")
                .contains("Newer body.")
        );
    }

    #[test]
    fn test_json_output() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let p = s.save(&["--json"], &[]);
        assert_eq!(p.code, 0, "{}", p.stderr_text());
        let expect = |action: &str| {
            format!(
                "{{\"action\": \"{action}\", \"path\": \"{REL}\", \"root\": {}}}\n",
                json::dumps(&Value::Str(s.repo_text()), &json::PY_DEFAULT)
            )
        };
        // The keys in this order: action, path, root.
        assert_eq!(p.stdout_text(), expect("created"));
        let p = s.save(&["--json", "--replace"], &[]);
        assert_eq!(p.stdout_text(), expect("replaced"));
    }

    #[test]
    fn test_outside_a_git_repo_exits_1() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let plain = s.case.tmp.join("plain");
        fs::create_dir(&plain).expect("mkdir");
        let p = s.run_in(
            &plain,
            &[
                "save",
                "--agent",
                "scout",
                "--type",
                "project",
                "--slug",
                "x",
                "--name",
                "n",
                "--description",
                "d",
                "--body",
                "b",
            ],
        );
        assert_eq!(p.code, 1);
        assert_eq!(
            p.stderr_text(),
            "anthology-mem: not inside a git work tree; memory notes live in the repo\n"
        );
        assert_eq!(fs::read_dir(&plain).expect("ls").count(), 0);
    }

    #[test]
    fn test_abbreviated_flags_are_rejected() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let p = s.save(&["--rep"], &[]);
        assert_eq!(p.code, 2);
        assert!(p.stderr_text().contains("unrecognized arguments"));
        assert!(!s.repo.join(".claude").exists());
    }

    #[test]
    fn test_missing_required_flags_exit_2() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let p = s.run(&["save", "--agent", "scout"]);
        assert_eq!(p.code, 2);
        assert!(!s.repo.join(".claude").exists());
    }

    #[test]
    fn test_refusals_exit_1_with_a_clear_message() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let p = s.save(&[], &[("type", "user")]);
        assert_eq!(p.code, 1);
        assert!(p.stderr_text().contains("project"));
        let p = s.save(&[], &[("agent", "nobody")]);
        assert_eq!(p.code, 1);
        // Assembled at run time: the repository holds no token-shaped literal.
        let token = format!("{}_{}", "ghp", "abc123");
        let p = s.save(&[], &[("body", &format!("see {token}"))]);
        assert_eq!(p.code, 1);
        assert!(p.stderr_text().contains("secret"));
        assert!(!p.stderr_text().contains(&token));
        assert!(!s.repo.join(".claude").exists());
    }

    #[test]
    fn test_a_body_with_literal_newlines_in_one_argument() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let body = "First line.\nSecond; with | pipes > and && stuff.\n\n- item $HOME `x` \\n kept";
        let p = s.save(&[], &[("body", body)]);
        assert_eq!(p.code, 0, "{}", p.stderr_text());
        let text = fs::read_to_string(s.repo.join(REL)).expect("read");
        assert!(text.ends_with(&format!("---\n\n{body}\n")), "{text}");
    }

    #[test]
    fn test_a_value_starting_with_dashes_is_taken_as_a_value() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let p = s.save(&[], &[("body", "--not-a-flag text")]);
        assert_eq!(p.code, 0, "{}", p.stderr_text());
        assert!(
            fs::read_to_string(s.repo.join(REL))
                .expect("read")
                .ends_with("\n--not-a-flag text\n")
        );
    }

    #[test]
    fn test_a_type_prefixed_slug_is_not_doubled() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let p = s.save(&[], &[("slug", "project-eval-probe")]);
        assert_eq!(
            (p.code, p.stdout_text()),
            (0, format!("created {REL} in {}\n", s.repo_text()))
        );
        assert!(s.repo.join(REL).is_file());
        assert!(
            !s.repo
                .join(".claude/agent-memory/anthology-scout/project-project-eval-probe.md")
                .exists()
        );
    }

    #[test]
    fn test_the_root_names_a_nested_repo() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let nested = s.repo.join("vendor/inner");
        fs::create_dir_all(&nested).expect("mkdir");
        git(&s.case.env, &nested, &["init", "-q"]);
        let p = s.run_in(
            &nested,
            &[
                "save",
                "--agent",
                "scout",
                "--type",
                "project",
                "--slug",
                "x",
                "--name",
                "n",
                "--description",
                "d",
                "--body",
                "b",
            ],
        );
        assert_eq!(p.code, 0, "{}", p.stderr_text());
        let real = fs::canonicalize(&nested).expect("realpath");
        assert!(
            p.stdout_text()
                .trim_end_matches('\n')
                .ends_with(&format!(" in {}", real.display()))
        );
    }

    #[test]
    fn test_a_single_token_value_starting_with_a_dash_is_a_flag_unless_joined_with_equals() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let p = s.save(&[], &[("body", "-x")]);
        assert_eq!(p.code, 2);
        assert_eq!(p.stdout_text(), "");
        assert!(!s.repo.join(".claude").exists());
        let p = s.run(&[
            "save",
            "--agent",
            "scout",
            "--type",
            "project",
            "--slug",
            "eval-probe",
            "--name",
            "n",
            "--description",
            "d",
            "--body=-x",
        ]);
        assert_eq!(p.code, 0, "{}", p.stderr_text());
        assert!(
            fs::read_to_string(s.repo.join(REL))
                .expect("read")
                .ends_with("\n-x\n")
        );
    }

    #[test]
    fn test_the_save_does_not_touch_other_files() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        write(&s.repo.join("keep.txt"), "keep", None);
        let before = snapshot(&s.repo);
        assert_eq!(s.save(&[], &[]).code, 0);
        let after = snapshot(&s.repo);
        let new: Vec<&String> = after.keys().filter(|k| !before.contains_key(*k)).collect();
        let mut new: Vec<&str> = new.iter().map(|k| k.as_str()).collect();
        new.sort();
        assert_eq!(new, [".claude/agent-memory/anthology-scout/MEMORY.md", REL]);
        for (k, v) in &before {
            assert_eq!(after.get(k), Some(v), "{k}");
        }
    }

    #[test]
    fn test_save_is_listed_in_the_help_and_wrapper_header() {
        if !needs_git() {
            return;
        }
        let s = SaveCase::new();
        let p = s.run(&["--help"]);
        assert!(p.stdout_text().contains("save"));
        let header = fs::read_to_string(staged_wrapper()).expect("read");
        assert!(header.lines().nth(1).expect("line 2").contains("save"));
    }
}
