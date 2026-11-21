//! MS1's own tests: the frontmatter parser (`memory_index.parse_text`), the user-note rule, and
//! the repo helpers that have no Python test of their own (`legacy_dirs`, `note_sources`,
//! `_unsafe_dir`, `_main_root`). The `notes.parse` difftest compares the parser with the Python
//! on generated input; these pin the behaviour it was checked against.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use anthology_core::git::RealGit;
use anthology_memory::notes::{
    NoteInfo, REPO_EXCLUDE, Relations, USER_REASON, is_user_note, normalize_type, note_info,
    parse_note, parse_text, parse_text_ok,
};
use anthology_memory::repo::{
    REASON_NOT_A_FOLDER, REASON_SYMLINK, encode_project_path, legacy_dirs, main_root, note_sources,
    note_sources_opt, unsafe_dir,
};
use anthology_testkit::mem::{MemCase, Note, needs_git, write};

fn parse(text: &str) -> NoteInfo {
    parse_text(text, "stem")
}

#[test]
fn constants_are_pythons() {
    assert_eq!(
        USER_REASON,
        "type: user notes are personal; they never go into a repo"
    );
    assert_eq!(REPO_EXCLUDE, ["user"]);
}

#[test]
fn a_claude_code_note_parses() {
    let text = Note::new("Deploy")
        .type_("feedback")
        .desc("How we deploy")
        .updated("2026-01-02")
        .relation("supersedes", &["a", "\"b\""])
        .relation("relates_to", &["c"])
        .body("Text.\n")
        .text();
    let (info, ok) = parse_text_ok(&text, "deploy");
    assert!(ok);
    assert_eq!(
        info,
        NoteInfo {
            name: "Deploy".into(),
            description: "How we deploy".into(),
            kind: Some("feedback".into()),
            updated: Some("2026-01-02".into()),
            relations: Relations {
                supersedes: vec!["a".into(), "b".into()],
                depends_on: vec![],
                relates_to: vec!["c".into()],
            },
            body: "Text.\n".into(),
        }
    );
    assert_eq!(info.type_str(), "feedback");
}

#[test]
fn no_frontmatter_is_listed_by_stem() {
    for text in ["plain\n", "---\nname: x\n", "--- \nname: x\n---\n", ""] {
        let (info, ok) = parse_text_ok(text, "stem");
        assert!(!ok, "{text:?}");
        assert_eq!(info.name, "stem");
        assert_eq!(info.kind.as_deref(), Some("other"));
        assert_eq!(info.updated, None);
        assert_eq!(info.body, text);
    }
}

#[test]
fn bom_and_crlf_are_normalised() {
    let info = parse("\u{feff}---\r\nname: n\r\n---\r\n\r\nbody\r\n");
    assert_eq!(info.name, "n");
    assert_eq!(info.body, "body\n");
    // Only a CRLF pair is folded; a lone CR stays.
    assert_eq!(parse("x\ry").body, "x\ry");
}

#[test]
fn the_closing_line_may_carry_spaces_and_tabs() {
    let info = parse("---\nname: n\n--- \t\n\n\nbody");
    assert_eq!(info.name, "n");
    assert_eq!(info.body, "body");
    assert_eq!(parse("---\n---\nb").body, "b");
    assert_eq!(parse("---\nname: n\n----\n").name, "stem");
}

#[test]
fn scalars_lose_one_pair_of_quotes() {
    let info = parse(
        "---\nname: '  it''s  '\ndescription: \"a\\nb \\\"q\\\" c\\\\d\"\n\
         metadata:\n  type: \"'\"\n---\n",
    );
    assert_eq!(info.name, "it's");
    assert_eq!(info.description, "a\nb \"q\" c\\d");
    assert_eq!(info.kind.as_deref(), Some("'"));
    assert_eq!(parse("---\nname: 'x\"\n---\n").name, "'x\"");
}

#[test]
fn top_level_and_metadata_fields_have_an_order() {
    let info = parse("---\ntype: top\nupdated: u1\nmodified: m1\nmetadata:\n  updated: u2\n---\n");
    assert_eq!(info.kind.as_deref(), Some("top"));
    assert_eq!(info.updated.as_deref(), Some("u2"));
    let info = parse("---\ntype: top\nupdated: u1\nmodified: m1\n---\n");
    assert_eq!(info.updated.as_deref(), Some("m1"));
    let info = parse("---\nmetadata:\n  type: ' '\ntype: t\n---\n");
    assert_eq!(info.kind.as_deref(), Some("t"));
}

#[test]
fn block_scalars_and_lists() {
    let info = parse(
        "---\ndescription: >-\n  one\n    two\n\n  three\nname: n\n\
         metadata:\n  relations:\n    depends_on:\n    - x\n    -  'y'\n    - \n  type: t\n---\n",
    );
    assert_eq!(info.description, "one two three");
    assert_eq!(info.name, "n");
    assert_eq!(info.relations.depends_on, ["x", "y"]);
    assert_eq!(info.kind.as_deref(), Some("t"));
    // `>x` is not a block scalar; an unclosed flow list stays a string.
    let info = parse("---\nname: >x\ndescription: [a, b\n---\n");
    assert_eq!(info.name, ">x");
    assert_eq!(info.description, "[a, b");
    // A list where a map should be gives no fields.
    let info = parse("---\n- a\n- b\n---\n");
    assert_eq!(info.name, "stem");
}

#[test]
fn relations_keep_the_last_non_empty_key_per_kind() {
    let info = parse(
        "---\nmetadata:\n  relations:\n    supersedes: [a]\n    ' supersedes ': []\n    \
         relates_to: one\n    depends_on: [b]\n    \"depends_on\": [c, ' ', d]\n    other: [z]\n---\n",
    );
    assert_eq!(info.relations.supersedes, ["a"]);
    assert_eq!(info.relations.relates_to, ["one"]);
    assert_eq!(info.relations.depends_on, ["c", "d"]);
    assert_eq!(info.relations.get("other"), &[] as &[String]);
}

#[test]
fn comments_blank_lines_and_odd_indents() {
    let info = parse(
        "---\n# a comment\n\n   \nname: n\n  stray: deeper\n- item\nname2\n\tdescription: tab\n---\n",
    );
    assert_eq!(info.name, "n");
    // A tab does not count as indentation, so the line is a top-level key.
    assert_eq!(info.description, "tab");
}

#[test]
fn user_notes_in_any_case_with_comments() {
    for (t, user) in [
        ("user", true),
        ("User", true),
        ("USER # me", true),
        (" user#x", true),
        ("users", false),
        ("project", false),
    ] {
        let info = parse(&format!("---\nmetadata:\n  type: {t}\n---\n"));
        assert_eq!(is_user_note(&info), user, "{t:?}");
    }
    assert_eq!(normalize_type(Some(" User # x ")), "user");
    assert_eq!(normalize_type(None), "");
    let hand = NoteInfo::default();
    assert!(!is_user_note(&hand));
    assert_eq!(hand.type_str(), "other");
}

#[test]
fn note_info_decodes_with_replacement() {
    let info = note_info("n.md", b"---\nname: a\xffb\n---\nbody \xe2\x82\n");
    assert_eq!(info.name, "a\u{fffd}b");
    assert_eq!(info.body, "body \u{fffd}\n");
    assert_eq!(note_info("x.md", b"no frontmatter").name, "x");
}

#[test]
fn parse_note_reads_a_file_by_its_stem() {
    let case = MemCase::new();
    let p = write(&case.tmp.join("a.b.md"), "plain\n", None);
    assert_eq!(parse_note(&p).expect("note").name, "a.b");
    assert!(parse_note(&case.tmp.join("missing.md")).is_none());
    assert!(parse_note(&case.tmp).is_none());
}

#[test]
fn unsafe_dir_names_symlinks_and_non_folders() {
    let case = MemCase::new();
    let dir = case.tmp.join("d");
    fs::create_dir(&dir).expect("mkdir");
    assert_eq!(unsafe_dir(&dir), None);
    assert_eq!(unsafe_dir(&case.tmp.join("missing")), None);
    assert_eq!(unsafe_dir(&case.tmp.join("missing/below")), None);
    let file = write(&case.tmp.join("f"), "x", None);
    assert_eq!(
        unsafe_dir(&file).expect("a file").reason,
        REASON_NOT_A_FOLDER
    );
    assert_eq!(unsafe_dir(&file.join("below")), None, "ENOTDIR is absent");
    let link = case.tmp.join("link");
    symlink(&dir, &link).expect("symlink");
    let t = unsafe_dir(&link).expect("a symlink");
    assert_eq!(t.reason, REASON_SYMLINK);
    assert_eq!(
        t.to_string(),
        format!("refusing to use {}: it is a symlink", link.display())
    );
}

#[test]
fn note_sources_skip_symlinked_folders() {
    let case = MemCase::new();
    let root = case.tmp.join("root");
    let home = case.tmp.join("anth");
    fs::create_dir_all(root.join(".claude/memory")).expect("mkdir");
    for agent in ["anthology-scout", "anthology-architect"] {
        fs::create_dir_all(root.join(".claude/agent-memory").join(agent)).expect("mkdir");
    }
    write(&root.join(".claude/agent-memory/file.md"), "x", None);
    symlink(
        root.join(".claude/memory"),
        root.join(".claude/agent-memory/linked"),
    )
    .expect("symlink");
    let elsewhere = case.tmp.join("elsewhere");
    fs::create_dir(&elsewhere).expect("mkdir");
    fs::create_dir(&home).expect("mkdir");
    symlink(&elsewhere, home.join("personal-memory")).expect("symlink");
    let got: Vec<(String, String, Option<String>)> = note_sources(&root, &home)
        .into_iter()
        .map(|s| {
            let rel = s.dir.strip_prefix(&case.tmp).expect("below tmp");
            (s.scope, rel.display().to_string(), s.agent)
        })
        .collect();
    let agent = |n: &str| {
        (
            "agent".to_string(),
            format!("root/.claude/agent-memory/{n}"),
            Some(n.to_string()),
        )
    };
    assert_eq!(
        got,
        [
            ("repo".to_string(), "root/.claude/memory".to_string(), None),
            agent("anthology-architect"),
            agent("anthology-scout"),
        ]
    );
    fs::remove_file(home.join("personal-memory")).expect("unlink");
    fs::create_dir(home.join("personal-memory")).expect("mkdir");
    let personal = note_sources_opt(None, &home);
    assert_eq!(personal.len(), 1);
    assert_eq!(personal[0].scope, "personal");
    assert_eq!(personal[0].dir, home.join("personal-memory"));
}

#[test]
fn legacy_dirs_cover_the_worktree_and_the_main_clone() {
    if !needs_git() {
        return;
    }
    let case = MemCase::new();
    let root = case.make_repo("repo", None);
    write(&root.join("f"), "x", None);
    case.commit_all(&root, "c");
    let wt = case.tmp.join("wt");
    case.git(
        &root,
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().expect("utf-8"),
            "-b",
            "wt",
        ],
    );
    assert_eq!(main_root(&wt, &case.env, &RealGit), root);
    let config = case.config.clone();
    let enc = |p: &Path| {
        config
            .join("projects")
            .join(encode_project_path(p))
            .join("memory")
    };
    assert_eq!(
        legacy_dirs(&wt, &config, &case.env, &RealGit),
        [enc(&wt), enc(&root)]
    );
    assert_eq!(
        legacy_dirs(&root, &config, &case.env, &RealGit),
        [enc(&root)]
    );
    // Not a repo: the folder itself, once.
    let plain = case.tmp.join("plain");
    fs::create_dir(&plain).expect("mkdir");
    assert_eq!(main_root(&plain, &case.env, &RealGit), plain);
    assert_eq!(
        legacy_dirs(&plain, &config, &case.env, &RealGit),
        [enc(&plain)]
    );
}

/// `SymlinkSafetyTest.test_symlinked_gitattributes_is_not_appended_through`'s MS1 half (the
/// id itself is MS2's): the approval is recorded before the union attributes are refused,
/// and nothing is written through the link.
#[test]
fn approval_is_recorded_before_a_symlinked_gitattributes_is_refused() {
    use anthology_memory::approve::{approve, ensure_union_attrs, is_approved};
    use anthology_memory::repo::repo_key;
    use anthology_pycompat::pyerr::PyExc;
    let case = MemCase::new();
    let root = case.tmp.join("r");
    fs::create_dir(&root).expect("mkdir");
    let victim = write(&case.tmp.join("victim.rc"), "export X=1\n", None);
    symlink(&victim, root.join(".gitattributes")).expect("symlink");
    let refused = |r: Result<_, PyExc>| matches!(r, Err(PyExc::Other("UnsafeTarget", _)));
    assert!(refused(approve(&root, &case.env, &RealGit).map(|_| ())));
    assert_eq!(fs::read_to_string(&victim).expect("victim"), "export X=1\n");
    assert!(is_approved(
        &repo_key(&root, &case.env, &RealGit),
        &case.env
    ));
    assert!(refused(ensure_union_attrs(&root).map(|_| ())));
    assert_eq!(fs::read_to_string(&victim).expect("victim"), "export X=1\n");
}

/// Time of the fastest of three parses of `text`.
fn best_parse_time(text: &str) -> std::time::Duration {
    (0..3)
        .map(|_| {
            let t = std::time::Instant::now();
            std::hint::black_box(parse(text));
            t.elapsed()
        })
        .min()
        .expect("three runs")
}

/// Many distinct keys cost linear time, as Python's dict does (a hostile repo note): 4x the
/// keys must cost less than 6x, with a floor so a very fast parse cannot flake.
#[test]
fn many_keys_parse_in_linear_time() {
    let note = |n: usize| {
        let keys: String = (0..n).map(|i| format!("k{i}: v\n")).collect();
        format!("---\n{keys}---\nbody\n")
    };
    let small = best_parse_time(&note(10_000));
    let large = best_parse_time(&note(40_000));
    let bound = (small * 6).max(std::time::Duration::from_millis(200));
    assert!(large < bound, "10k keys {small:?}, 40k keys {large:?}");
}

/// A note nested thousands of levels deep parses and is dropped on a small stack: neither the
/// parse nor the parsed tree recurses per level.
#[test]
fn deep_nesting_needs_no_deep_stack() {
    let mut text = String::from("---\nmetadata:\n  relations:\n");
    for d in 3..4_000 {
        text.push_str(&" ".repeat(d));
        text.push_str("k:\n");
    }
    text.push_str("---\nbody\n");
    let info = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || parse(&text))
        .expect("thread")
        .join()
        .expect("no stack overflow");
    assert_eq!(info.body, "body\n");
    assert_eq!(info.relations, Relations::default());
}
