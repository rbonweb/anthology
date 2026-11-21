//! Port of `tests/test_memory_sync.py`, part 2 (MS2): migration, the `type: user` sweep, the
//! SessionStart flow (`run`), the per-clone repairs, symlink and FIFO safety, and `save_note`.
//!
//! Python's `mock.patch` sites go through the GC-19 seams: `fake_repo` is a fake `Git`
//! (`ms2_support::FakeGit`), `_read_regular` / `sweep_user_notes` / `_run` through `SyncDeps`,
//! and `privacy_findings` / `regenerate_index` through `SaveDeps`. Cases that need git return
//! early when it is missing, as Python's `needs_git` skips them.

mod ms2_support;

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::clock::Date;
use anthology_memory::sync::{self, SyncDeps};
use anthology_pycompat::json::{self, Value};
use anthology_testkit::mem::{MemCase, Note, needs_git, snapshot, write};

use ms2_support::*;

const HINT: &str = "this repo has memory notes; repo notes load only after /anthology:setup, \
                    but notes in .claude/agent-memory still reach their agents, so review them";

fn date(y: i32, m: u8, d: u8) -> Date {
    Date { y, m, d }
}

fn note(name: &str, type_: &str, desc: &str) -> String {
    Note::new(name).type_(type_).desc(desc).text()
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn load(p: &Path) -> Value {
    json::loads(&read(p)).expect("json")
}

/// MigrateTest: B3, legacy notes go to personal memory; only an explicit, clean promotion
/// reaches the repo.
mod migrate_test {
    use super::*;
    use anthology_memory::sync::MigrateResult;

    struct T {
        c: MemCase,
        old: PathBuf,
        repo_mem: PathBuf,
        personal: PathBuf,
        ledger: PathBuf,
    }

    fn alice() -> Option<String> {
        Some(["alice-", "laptop"].concat())
    }

    fn set_up() -> T {
        let c = MemCase::new();
        let old = c.tmp.join("old");
        let repo_mem = c.tmp.join("repo/.claude/memory");
        let personal = c.anth.join("personal-memory");
        let ledger = c.anth.join("migrated.json");
        write(
            &old.join("user-me.md"),
            &note("user-me", "user", "me"),
            None,
        );
        write(
            &old.join("feedback-a.md"),
            &note("feedback-a", "feedback", "a"),
            None,
        );
        write(
            &old.join("project-b.md"),
            &note("project-b", "project", "b"),
            None,
        );
        write(&old.join("MEMORY.md"), "- index\n", None);
        T {
            c,
            old,
            repo_mem,
            personal,
            ledger,
        }
    }

    impl T {
        fn migrate(&self, promote: &[&str]) -> MigrateResult {
            let mut deps = SyncDeps::default();
            deps.privacy.hostname = alice;
            let promote: Vec<String> = promote.iter().map(|s| s.to_string()).collect();
            sync::migrate(
                &self.old,
                Some(&self.repo_mem),
                &self.personal,
                Some(&self.ledger),
                Some(date(2026, 4, 5)),
                &promote,
                None,
                &deps,
            )
        }
    }

    #[test]
    fn test_every_note_goes_to_personal_memory() {
        let t = set_up();
        let mut r = t.migrate(&[]);
        r.copied.sort();
        assert_eq!(r.copied, ["feedback-a.md", "project-b.md", "user-me.md"]);
        assert_eq!(
            names(&t.personal),
            ["feedback-a.md", "project-b.md", "user-me.md"]
        );
        assert!(!t.repo_mem.exists());
        assert!(r.promoted.is_empty() && r.blocked.is_empty());
        assert_eq!(
            fs::read(t.personal.join("project-b.md")).unwrap(),
            fs::read(t.old.join("project-b.md")).unwrap()
        );
        assert_eq!(
            fs::metadata(&t.personal).unwrap().permissions().mode() & 0o7777,
            0o700
        );
    }

    #[test]
    fn test_memory_md_and_symlinks_are_not_copied() {
        let t = set_up();
        let outside = t.c.tmp.join("secret.md");
        write(&outside, "---\nname: s\n---\nsecret\n", None);
        symlink(&outside, t.old.join("link.md")).unwrap();
        t.migrate(&["link.md"]);
        let n = names(&t.personal);
        assert!(!n.contains(&"MEMORY.md".to_string()));
        assert!(!n.contains(&"link.md".to_string()));
        assert!(!t.repo_mem.exists());
    }

    #[test]
    fn test_second_run_copies_nothing() {
        let t = set_up();
        t.migrate(&[]);
        let mut again = t.migrate(&[]);
        assert!(again.copied.is_empty());
        assert!(again.side_by_side.is_empty());
        again.skipped.sort();
        assert_eq!(
            again.skipped,
            ["feedback-a.md", "project-b.md", "user-me.md"]
        );
    }

    #[test]
    fn test_never_overwrites_differing_files() {
        let t = set_up();
        write(
            &t.personal.join("project-b.md"),
            "mine, edited since\n",
            None,
        );
        let r = t.migrate(&[]);
        assert_eq!(
            read(&t.personal.join("project-b.md")),
            "mine, edited since\n"
        );
        assert_eq!(r.side_by_side, ["project-b.md"]);
        let copy = t.personal.join("project-b.migrated-2026-04-05.md");
        assert_eq!(
            fs::read(copy).unwrap(),
            fs::read(t.old.join("project-b.md")).unwrap()
        );
    }

    #[test]
    fn test_conflict_name_is_made_unique() {
        let t = set_up();
        write(&t.personal.join("project-b.md"), "mine\n", None);
        write(
            &t.personal.join("project-b.migrated-2026-04-05.md"),
            "taken\n",
            None,
        );
        t.migrate(&[]);
        assert_eq!(
            read(&t.personal.join("project-b.migrated-2026-04-05.md")),
            "taken\n"
        );
        assert_eq!(
            fs::read(t.personal.join("project-b.migrated-2026-04-05-2.md")).unwrap(),
            fs::read(t.old.join("project-b.md")).unwrap()
        );
    }

    #[test]
    fn test_identical_existing_file_is_skipped() {
        let t = set_up();
        write(
            &t.personal.join("project-b.md"),
            &read(&t.old.join("project-b.md")),
            None,
        );
        let r = t.migrate(&[]);
        assert!(r.skipped.contains(&"project-b.md".to_string()));
        assert!(r.side_by_side.is_empty());
    }

    #[test]
    fn test_ledger_stops_recopying_deleted_notes() {
        let t = set_up();
        t.migrate(&[]);
        fs::remove_file(t.personal.join("project-b.md")).unwrap();
        let again = t.migrate(&[]);
        assert!(!again.copied.contains(&"project-b.md".to_string()));
        assert!(!t.personal.join("project-b.md").exists());
        let data = load(&t.ledger);
        let m = data.as_object().unwrap();
        assert_eq!(m.get("version").and_then(Value::as_i64), Some(1));
        let done = m.get("done").and_then(Value::as_array).unwrap();
        assert_eq!(done.len(), 3);
        assert!(done.iter().all(|h| {
            let h = h.as_str().unwrap();
            h.len() == 64
                && h.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }));
    }

    #[test]
    fn test_without_a_ledger_a_deleted_note_is_copied_again_but_never_duplicated() {
        let t = set_up();
        let d = SyncDeps::default();
        sync::migrate(
            &t.old,
            Some(&t.repo_mem),
            &t.personal,
            None,
            Some(date(2026, 4, 5)),
            &[],
            None,
            &d,
        );
        let again = sync::migrate(
            &t.old,
            Some(&t.repo_mem),
            &t.personal,
            None,
            Some(date(2026, 4, 5)),
            &[],
            None,
            &d,
        );
        assert!(again.copied.is_empty());
        assert!(again.side_by_side.is_empty());
        assert_eq!(names(&t.personal).len(), 3);
    }

    #[test]
    fn test_missing_old_dir_is_a_noop() {
        let t = set_up();
        let r = sync::migrate(
            &t.c.tmp.join("nope"),
            Some(&t.repo_mem),
            &t.personal,
            Some(&t.ledger),
            None,
            &[],
            None,
            &SyncDeps::default(),
        );
        assert_eq!(r, MigrateResult::default());
        assert!(!t.repo_mem.exists());
        assert!(!t.personal.exists());
    }

    #[test]
    fn test_a_clean_promoted_note_goes_to_the_repo() {
        let t = set_up();
        let r = t.migrate(&["project-b.md"]);
        assert_eq!(r.promoted, ["project-b.md"]);
        assert_eq!(names(&t.repo_mem), ["project-b.md"]);
        assert_eq!(
            fs::read(t.repo_mem.join("project-b.md")).unwrap(),
            fs::read(t.old.join("project-b.md")).unwrap()
        );
        assert_eq!(names(&t.personal), ["feedback-a.md", "user-me.md"]);
        let done = load(&t.ledger);
        assert_eq!(
            done.as_object()
                .unwrap()
                .get("done")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn test_promotion_ignores_the_ledger_but_never_overwrites() {
        let t = set_up();
        t.migrate(&[]);
        write(&t.repo_mem.join("project-b.md"), "the repo's own\n", None);
        let r = t.migrate(&["project-b.md"]);
        assert_eq!(r.promoted, ["project-b.md"]);
        assert_eq!(r.side_by_side, ["project-b.md"]);
        assert_eq!(read(&t.repo_mem.join("project-b.md")), "the repo's own\n");
        assert!(
            t.repo_mem
                .join("project-b.migrated-2026-04-05.md")
                .is_file()
        );
        assert!(t.personal.join("project-b.md").is_file());
    }

    #[test]
    fn test_promoted_notes_with_findings_or_type_user_stay_personal() {
        let t = set_up();
        write(
            &t.old.join("reference-host.md"),
            &Note::new("reference-host")
                .type_("reference")
                .desc("Host")
                .body(&["Runs on build-", "box.lan\n"].concat())
                .text(),
            None,
        );
        let r = t.migrate(&["reference-host.md", "user-me.md"]);
        assert!(!t.repo_mem.exists());
        let mut files: Vec<&str> = r.blocked.iter().map(|(f, _)| f.as_str()).collect();
        files.sort();
        assert_eq!(files, ["reference-host.md", "user-me.md"]);
        let i = r
            .blocked
            .iter()
            .position(|(f, _)| f == "reference-host.md")
            .unwrap();
        let f = &r.blocked_findings[i];
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].kind.as_str(), f[0].line), ("private-hostname", 8));
        assert!(r.blocked[i].1.contains("private hostname (line 8)"));
        assert!(!format!("{r:?}").contains("build-box"));
        let u = r
            .blocked
            .iter()
            .position(|(f, _)| f == "user-me.md")
            .unwrap();
        assert!(r.blocked[u].1.contains("type: user"));
        assert!(t.personal.join("reference-host.md").is_file());
        assert!(t.personal.join("user-me.md").is_file());
    }

    #[test]
    fn test_promotion_refuses_a_symlinked_dot_claude_before_any_mkdir() {
        let t = set_up();
        let outside = t.c.tmp.join("outside");
        fs::create_dir(&outside).unwrap();
        let repo = t.c.tmp.join("repo");
        fs::create_dir(&repo).unwrap();
        symlink(&outside, repo.join(".claude")).unwrap();
        let r = t.migrate(&["project-b.md"]);
        assert!(names(&outside).is_empty());
        assert_eq!(
            r.blocked.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(),
            ["project-b.md"]
        );
        assert!(r.blocked[0].1.contains(".claude"));
        assert!(t.personal.join("project-b.md").is_file());
    }

    #[test]
    fn test_a_user_type_in_any_case_or_with_a_comment_is_never_promoted() {
        let t = set_up();
        write(
            &t.old.join("user-odd.md"),
            &note("user-odd", "User  # mine", "odd"),
            None,
        );
        let r = t.migrate(&["user-odd.md"]);
        assert_eq!(
            r.blocked[0].1,
            "type: user notes are personal; they never go into a repo"
        );
        assert!(!t.repo_mem.exists());
    }

    #[test]
    fn test_promotion_never_writes_through_a_symlinked_repo_memory() {
        let t = set_up();
        let outside = t.c.tmp.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::create_dir_all(t.repo_mem.parent().unwrap()).unwrap();
        symlink(&outside, &t.repo_mem).unwrap();
        let r = t.migrate(&["project-b.md"]);
        assert!(names(&outside).is_empty());
        assert_eq!(
            r.blocked.iter().map(|b| b.0.as_str()).collect::<Vec<_>>(),
            ["project-b.md"]
        );
        assert!(t.personal.join("project-b.md").is_file());
    }
}

/// MigrateGlobTest.
mod migrate_glob_test {
    use super::*;

    #[test]
    fn test_note_names_with_glob_characters_are_matched_literally() {
        let c = MemCase::new();
        let old = c.tmp.join("old");
        let personal = c.tmp.join("personal");
        write(&old.join("weird[1].md"), "from old\n", None);
        write(&personal.join("weird[1].md"), "mine\n", None);
        for _ in 0..3 {
            sync::migrate(
                &old,
                None,
                &personal,
                None,
                Some(date(2026, 4, 5)),
                &[],
                None,
                &SyncDeps::default(),
            );
        }
        assert_eq!(
            names(&personal),
            ["weird[1].md", "weird[1].migrated-2026-04-05.md"]
        );
    }
}

/// SweepUserNotesTest: C1, `type: user` notes move out of repo and agent memory.
mod sweep_user_notes_test {
    use super::*;
    use anthology_memory::sync::SweepResult;

    struct T {
        c: MemCase,
        src: PathBuf,
        personal: PathBuf,
    }

    fn set_up() -> T {
        let c = MemCase::new();
        let src = c.tmp.join("repo/.claude/memory");
        let personal = c.anth.join("personal-memory");
        write(
            &src.join("user-role.md"),
            &note("user-role", "user", "Role"),
            None,
        );
        write(
            &src.join("project-a.md"),
            &note("project-a", "project", "A"),
            None,
        );
        T { c, src, personal }
    }

    fn sweep(src: &Path, personal: &Path, today: Option<Date>) -> SweepResult {
        sync::sweep_user_notes(src, personal, today, &SyncDeps::default())
    }

    fn sr(moved: &[&str], conflicts: &[&str]) -> SweepResult {
        SweepResult {
            moved: moved.iter().map(|s| s.to_string()).collect(),
            conflicts: conflicts.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_type_is_compared_case_insensitively_without_comments() {
        let t = set_up();
        write(
            &t.src.join("user-caps.md"),
            &note("user-caps", "User", "Caps"),
            None,
        );
        write(
            &t.src.join("user-comment.md"),
            &note("user-comment", "user # about me", "Comment"),
            None,
        );
        let mut r = sweep(&t.src, &t.personal, None);
        r.moved.sort();
        assert_eq!(r.moved, ["user-caps.md", "user-comment.md", "user-role.md"]);
        // `normalize_type(" User # x ")` is "user": a note typed so is swept too.
        write(
            &t.src.join("user-x.md"),
            &note("user-x", " User # x ", "X"),
            None,
        );
        assert_eq!(sweep(&t.src, &t.personal, None).moved, ["user-x.md"]);
    }

    #[test]
    fn test_moves_user_notes_only() {
        let t = set_up();
        let data = fs::read(t.src.join("user-role.md")).unwrap();
        let r = sweep(&t.src, &t.personal, Some(date(2026, 4, 5)));
        assert_eq!(r, sr(&["user-role.md"], &[]));
        assert!(!t.src.join("user-role.md").exists());
        assert!(t.src.join("project-a.md").exists());
        assert_eq!(fs::read(t.personal.join("user-role.md")).unwrap(), data);
    }

    #[test]
    fn test_identical_personal_copy_counts_as_done() {
        let t = set_up();
        write(
            &t.personal.join("user-role.md"),
            &read(&t.src.join("user-role.md")),
            None,
        );
        let r = sweep(&t.src, &t.personal, Some(date(2026, 4, 5)));
        assert_eq!(r.moved, ["user-role.md"]);
        assert_eq!(names(&t.personal), ["user-role.md"]);
        assert!(!t.src.join("user-role.md").exists());
    }

    #[test]
    fn test_conflict_is_kept_side_by_side() {
        let t = set_up();
        write(&t.personal.join("user-role.md"), "mine\n", None);
        let data = fs::read(t.src.join("user-role.md")).unwrap();
        let r = sweep(&t.src, &t.personal, Some(date(2026, 4, 5)));
        assert_eq!(r, sr(&["user-role.md"], &["user-role.md"]));
        assert_eq!(read(&t.personal.join("user-role.md")), "mine\n");
        assert_eq!(
            fs::read(t.personal.join("user-role.migrated-2026-04-05.md")).unwrap(),
            data
        );
        assert!(!t.src.join("user-role.md").exists());
    }

    #[test]
    fn test_symlinks_and_unsafe_folders_are_never_touched() {
        let t = set_up();
        let outside = t.c.tmp.join("outside.md");
        write(&outside, &note("user-out", "user", "Out"), None);
        symlink(&outside, t.src.join("user-link.md")).unwrap();
        sweep(&t.src, &t.personal, None);
        assert!(
            fs::symlink_metadata(t.src.join("user-link.md"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(outside.exists());
        assert!(!t.personal.join("user-link.md").exists());
        let other = t.c.tmp.join("elsewhere");
        fs::create_dir(&other).unwrap();
        let link = t.c.tmp.join("personal-link");
        symlink(&other, &link).unwrap();
        write(
            &t.src.join("user-two.md"),
            &note("user-two", "user", "Two"),
            None,
        );
        assert_eq!(sweep(&t.src, &link, None), SweepResult::default());
        assert!(t.src.join("user-two.md").exists());
        assert!(names(&other).is_empty());
        let linked_src = t.c.tmp.join("src-link");
        symlink(&t.src, &linked_src).unwrap();
        assert!(sweep(&linked_src, &t.personal, None).moved.is_empty());
    }

    fn flaky(p: &Path) -> Result<Vec<u8>, anthology_pycompat::pyerr::PyExc> {
        let mut data = anthology_core::fsx::read_regular(p, None)?;
        if p.to_string_lossy().contains("/personal-memory/") {
            data.extend_from_slice(b"changed");
        }
        Ok(data)
    }

    #[test]
    fn test_repo_file_is_kept_when_the_copy_cannot_be_verified() {
        let t = set_up();
        let deps = SyncDeps {
            read_regular: flaky,
            ..SyncDeps::default()
        };
        let r = sync::sweep_user_notes(&t.src, &t.personal, None, &deps);
        assert!(r.moved.is_empty());
        assert!(t.src.join("user-role.md").exists());
    }
}

/// AutoMemoryOffTest: R8, the env var or a setting turns `run` into a no-op.
mod auto_memory_off_test {
    use super::*;

    #[test]
    fn test_env_values() {
        let c = MemCase::new();
        write(
            &c.anth.join("personal-memory/user-me.md"),
            &note("user-me", "user", "me"),
            None,
        );
        for v in ["1", "true", "TRUE", "yes", "on", "anything"] {
            let env = c.env.clone().with("CLAUDE_CODE_DISABLE_AUTO_MEMORY", v);
            assert_eq!(
                sync::run(&c.tmp, &env, &SyncDeps::default()),
                (String::new(), vec![]),
                "{v}"
            );
        }
        assert!(!c.anth.join("personal-memory/MEMORY.md").exists());
        for v in ["", "0", "false", "No", "FALSE"] {
            let env = c.env.clone().with("CLAUDE_CODE_DISABLE_AUTO_MEMORY", v);
            let (ctx, _) = sync::run(&c.tmp, &env, &SyncDeps::default());
            assert!(ctx.contains("user-me"), "{v}");
        }
    }

    #[test]
    fn test_user_setting() {
        let c = MemCase::new();
        write(
            &c.anth.join("personal-memory/user-me.md"),
            &note("user-me", "user", "me"),
            None,
        );
        write(
            &c.config.join("settings.json"),
            "{\"autoMemoryEnabled\": false}",
            None,
        );
        assert_eq!(run(&c, &c.tmp), (String::new(), vec![]));
        assert!(!c.anth.join("personal-memory/MEMORY.md").exists());
    }

    #[test]
    fn test_project_setting_and_precedence() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        approve(&c, &root);
        write(
            &c.config.join("settings.json"),
            "{\"autoMemoryEnabled\": false}",
            None,
        );
        write(
            &root.join(".claude/settings.json"),
            "{\"autoMemoryEnabled\": true}",
            None,
        );
        run(&c, &root);
        assert!(root.join(".claude/settings.local.json").exists());
        fs::remove_file(root.join(".claude/settings.local.json")).unwrap();
        write(
            &root.join(".claude/settings.json"),
            "{\"autoMemoryEnabled\": true}",
            None,
        );
        write(
            &root.join(".claude/settings.local.json"),
            "{\"autoMemoryEnabled\": false}",
            None,
        );
        let before = snapshot(&root);
        assert_eq!(run(&c, &root), (String::new(), vec![]));
        assert_eq!(snapshot(&root), before);
    }

    #[test]
    fn test_disabled_repo_run_writes_nothing() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        write(
            &root.join(".claude/memory/a.md"),
            &Note::new("a").text(),
            None,
        );
        approve(&c, &root);
        let before = snapshot(&root);
        let env = c.env.clone().with("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
        assert_eq!(
            sync::run(&root, &env, &SyncDeps::default()),
            (String::new(), vec![])
        );
        assert_eq!(snapshot(&root), before);
    }
}

/// RunOutsideRepoTest.
mod run_outside_repo_test {
    use super::*;

    #[test]
    fn test_personal_index_is_injected_anywhere() {
        let c = MemCase::new();
        write(
            &c.anth.join("personal-memory/user-me.md"),
            &note("user-me", "user", "Likes terse answers"),
            None,
        );
        let folder = c.tmp.join("plain");
        fs::create_dir(&folder).unwrap();
        let (ctx, msgs) = run(&c, &folder);
        assert!(msgs.is_empty());
        assert!(ctx.starts_with("## "));
        assert!(
            ctx.lines()
                .next()
                .unwrap()
                .contains(&*c.anth.join("personal-memory").to_string_lossy())
        );
        assert!(ctx.contains("[user-me](user-me.md) — Likes terse answers"));
        assert!(names(&folder).is_empty());
        assert!(c.anth.join("personal-memory/MEMORY.md").exists());
    }

    #[test]
    fn test_never_creates_the_personal_dir() {
        let c = MemCase::new();
        let folder = c.tmp.join("plain");
        fs::create_dir(&folder).unwrap();
        assert_eq!(run(&c, &folder), (personal_only(&c.anth), vec![]));
        assert!(!c.anth.join("personal-memory").exists());
    }

    fn boom(_: &Path, _: &Env, _: &dyn anthology_core::git::Git) -> Option<PathBuf> {
        panic!("boom")
    }

    #[test]
    fn test_swallows_every_exception() {
        let c = MemCase::new();
        let deps = SyncDeps {
            repo_root: boom,
            ..SyncDeps::default()
        };
        let (ctx, msgs) = sync::run(&c.tmp, &c.env, &deps);
        assert_eq!(ctx, "");
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].starts_with("anthology memory: "));
        assert!(msgs[0].contains("boom"));
    }

    #[test]
    fn test_unreadable_cwd_does_not_raise() {
        let c = MemCase::new();
        let (ctx, _) = run(&c, &c.tmp.join("does-not-exist"));
        assert_eq!(ctx, personal_only(&c.anth));
    }
}

/// TrustBoundaryTest (needs git).
mod trust_boundary_test {
    use super::*;

    fn set_up() -> Option<(MemCase, PathBuf)> {
        if !needs_git() {
            return None;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        write(
            &root.join(".claude/memory/feedback-x.md"),
            &Note::new("feedback-x")
                .type_("feedback")
                .desc("Prefer small commits")
                .updated("2026-02-02")
                .text(),
            None,
        );
        c.commit_all(&root, "c");
        Some((c, root))
    }

    #[test]
    fn test_unapproved_repo_gets_one_hint_and_no_writes() {
        let Some((c, root)) = set_up() else { return };
        let old = c.config.join("projects").join(encode(&root)).join("memory");
        write(
            &old.join("project-old.md"),
            &Note::new("project-old").text(),
            None,
        );
        let before = snapshot(&root);
        let exclude = fs::read(root.join(".git/info/exclude")).unwrap_or_default();
        let status = c.git(&root, &["status", "--porcelain"]).stdout;
        let (ctx, msgs) = run(&c, &root);
        assert_eq!(ctx, personal_only(&c.anth));
        assert_eq!(msgs, [HINT]);
        assert_eq!(snapshot(&root), before);
        assert_eq!(
            fs::read(root.join(".git/info/exclude")).unwrap_or_default(),
            exclude
        );
        assert_eq!(c.git(&root, &["status", "--porcelain"]).stdout, status);
        assert!(!root.join(".claude/settings.local.json").exists());
        assert!(!root.join(".gitattributes").exists());
        assert!(!c.anth.join("approved-repos.json").exists());
        assert!(!c.anth.join("personal-memory").exists());
    }

    #[test]
    fn test_hint_for_agent_memory_only() {
        let Some((c, _root)) = set_up() else { return };
        let other = c.make_repo("other", None);
        write(
            &other.join(".claude/agent-memory/anthology-x/a.md"),
            &Note::new("a").text(),
            None,
        );
        assert_eq!(
            run(&c, &other),
            (personal_only(&c.anth), vec![HINT.to_string()])
        );
    }

    #[test]
    fn test_no_hint_without_committed_memory() {
        let Some((c, _root)) = set_up() else { return };
        let other = c.make_repo("other", None);
        assert_eq!(run(&c, &other), (personal_only(&c.anth), vec![]));
    }

    #[test]
    fn test_unapproved_notes_are_never_injected() {
        let Some((c, root)) = set_up() else { return };
        write(
            &c.anth.join("personal-memory/user-me.md"),
            &note("user-me", "user", "me"),
            None,
        );
        let (ctx, msgs) = run(&c, &root);
        assert!(!ctx.contains("feedback-x"));
        assert!(ctx.contains("user-me"));
        assert_eq!(msgs, [HINT]);
    }

    #[test]
    fn test_after_approve_the_repair_happens() {
        let Some((c, root)) = set_up() else { return };
        approve(&c, &root);
        let (ctx, msgs) = run(&c, &root);
        let settings = load(&root.join(".claude/settings.local.json"));
        assert_eq!(
            settings
                .as_object()
                .unwrap()
                .get("autoMemoryDirectory")
                .and_then(Value::as_str),
            Some(&*root.join(".claude/memory").to_string_lossy())
        );
        let index = read(&root.join(".claude/memory/MEMORY.md"));
        assert!(index.contains("[feedback-x](feedback-x.md) — Prefer small commits"));
        assert!(ctx.contains(&index));
        assert!(ctx.starts_with("## Personal memory"));
        let head = ctx
            .lines()
            .find(|l| l.starts_with("## Repo memory"))
            .unwrap();
        assert!(head.contains(&*root.join(".claude/memory").to_string_lossy()));
        assert!(!msgs.contains(&HINT.to_string()));
        assert!(!msgs.is_empty());
        assert!(!index.contains(&*root.to_string_lossy()));
    }
}

/// RepairTest (needs git).
mod repair_test {
    use super::*;
    use anthology_core::git::RealGit;

    fn approved_repo() -> Option<(MemCase, PathBuf)> {
        if !needs_git() {
            return None;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        approve(&c, &root);
        Some((c, root))
    }

    fn keys(v: &Value) -> Vec<String> {
        v.as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.to_string())
            .collect()
    }

    #[test]
    fn test_ensure_memory_dir_preserves_other_keys_and_order() {
        let Some((_c, root)) = approved_repo() else {
            return;
        };
        let sl = root.join(".claude/settings.local.json");
        write(
            &sl,
            "{\"zeta\": 1, \"permissions\": {\"allow\": [\"Bash(ls)\"]}, \"alpha\": [1]}",
            None,
        );
        let (data, changed) = sync::ensure_memory_dir(&root, &sl).unwrap();
        assert!(changed);
        let data = Value::Object(data);
        assert_eq!(
            keys(&data),
            ["zeta", "permissions", "alpha", "autoMemoryDirectory"]
        );
        let on_disk = load(&sl);
        assert_eq!(keys(&on_disk), keys(&data));
        assert_eq!(
            json::dumps(
                on_disk.as_object().unwrap().get("permissions").unwrap(),
                &json::PY_DEFAULT
            ),
            "{\"allow\": [\"Bash(ls)\"]}"
        );
        assert!(read(&sl).ends_with("}\n"));
        assert!(root.join(".claude/memory").is_dir());
        let mtime = fs::metadata(&sl).unwrap().modified().unwrap();
        let (data2, changed2) = sync::ensure_memory_dir(&root, &sl).unwrap();
        assert!(!changed2);
        assert_eq!(Value::Object(data2), data);
        assert_eq!(fs::metadata(&sl).unwrap().modified().unwrap(), mtime);
    }

    #[test]
    fn test_ensure_memory_dir_missing_file_creates_it() {
        let Some((_c, root)) = approved_repo() else {
            return;
        };
        let sl = root.join(".claude/settings.local.json");
        let (data, changed) = sync::ensure_memory_dir(&root, &sl).unwrap();
        assert!(changed);
        let want = format!(
            "{{\"autoMemoryDirectory\": \"{}\"}}",
            root.join(".claude/memory").to_string_lossy()
        );
        assert_eq!(
            json::dumps(&Value::Object(data.clone()), &json::PY_DEFAULT),
            want
        );
        assert_eq!(load(&sl), Value::Object(data));
    }

    #[test]
    fn test_ensure_memory_dir_corrects_a_stale_path() {
        let Some((_c, root)) = approved_repo() else {
            return;
        };
        let sl = root.join(".claude/settings.local.json");
        write(
            &sl,
            "{\"autoMemoryDirectory\": \"/some/old/clone/.claude/memory\"}",
            None,
        );
        let (data, changed) = sync::ensure_memory_dir(&root, &sl).unwrap();
        assert!(changed);
        assert_eq!(
            data.get("autoMemoryDirectory").and_then(Value::as_str),
            Some(&*root.join(".claude/memory").to_string_lossy())
        );
    }

    #[test]
    fn test_invalid_settings_are_left_alone() {
        let Some((c, root)) = approved_repo() else {
            return;
        };
        let sl = root.join(".claude/settings.local.json");
        for text in ["{ not json", "[1, 2]", ""] {
            write(&sl, text, None);
            let (data, changed) = sync::ensure_memory_dir(&root, &sl).unwrap();
            assert!(!changed);
            assert_eq!(read(&sl), text);
            assert!(data.is_empty());
        }
        write(&sl, "{ not json", None);
        let (_ctx, msgs) = run(&c, &root);
        assert_eq!(read(&sl), "{ not json");
        assert!(
            msgs.iter().any(|m| m.contains("settings.local.json")),
            "{msgs:?}"
        );
    }

    #[test]
    fn test_ensure_ignored() {
        let Some((c, root)) = approved_repo() else {
            return;
        };
        let d = SyncDeps::default();
        let exclude = root.join(".git/info/exclude");
        assert!(sync::ensure_ignored(&root, &c.env, &d).unwrap());
        assert_eq!(
            read(&exclude)
                .lines()
                .filter(|l| *l == "/.claude/settings.local.json")
                .count(),
            1
        );
        write(&root.join(".claude/settings.local.json"), "{}", None);
        assert_eq!(
            anthology_testkit::mem::try_git(
                &c.env,
                &root,
                &["check-ignore", "-q", ".claude/settings.local.json"]
            )
            .code,
            0
        );
        assert!(!sync::ensure_ignored(&root, &c.env, &d).unwrap());
    }

    #[test]
    fn test_ensure_ignored_noop_when_gitignore_already_covers_it() {
        let Some((c, root)) = approved_repo() else {
            return;
        };
        write(
            &root.join(".gitignore"),
            ".claude/settings.local.json\n",
            None,
        );
        let exclude = root.join(".git/info/exclude");
        let before = fs::read(&exclude).unwrap_or_default();
        assert!(!sync::ensure_ignored(&root, &c.env, &SyncDeps::default()).unwrap());
        assert_eq!(fs::read(&exclude).unwrap_or_default(), before);
    }

    #[test]
    fn test_ensure_ignored_handles_missing_trailing_newline() {
        let Some((c, root)) = approved_repo() else {
            return;
        };
        let exclude = root.join(".git/info/exclude");
        fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        fs::write(&exclude, "# keep\nfoo").unwrap();
        assert!(sync::ensure_ignored(&root, &c.env, &SyncDeps::default()).unwrap());
        assert_eq!(
            read(&exclude).lines().collect::<Vec<_>>(),
            ["# keep", "foo", "/.claude/settings.local.json"]
        );
    }

    #[test]
    fn test_linked_worktree_writes_the_right_exclude_file() {
        let Some((c, root)) = approved_repo() else {
            return;
        };
        write(&root.join("f"), "x", None);
        c.commit_all(&root, "c");
        let wt = c.tmp.join("wt");
        c.git(
            &root,
            &["worktree", "add", "-q", &wt.to_string_lossy(), "-b", "wt"],
        );
        let d = SyncDeps::default();
        assert!(sync::ensure_ignored(&wt, &c.env, &d).unwrap());
        let shared = c
            .git(
                &wt,
                &[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "info/exclude",
                ],
            )
            .stdout_text();
        let shared = PathBuf::from(shared.trim());
        assert_eq!(
            fs::canonicalize(&shared).unwrap(),
            fs::canonicalize(root.join(".git/info/exclude")).unwrap()
        );
        assert!(
            read(&shared)
                .lines()
                .any(|l| l == "/.claude/settings.local.json")
        );
        assert_eq!(
            anthology_testkit::mem::try_git(
                &c.env,
                &wt,
                &["check-ignore", "-q", ".claude/settings.local.json"]
            )
            .code,
            0
        );
        let wts = root.join(".git/worktrees");
        for e in fs::read_dir(&wts).unwrap().flatten() {
            assert!(!e.path().join("info/exclude").exists());
        }
        assert!(!sync::ensure_ignored(&wt, &c.env, &d).unwrap());
        let _ = RealGit;
    }

    #[test]
    fn test_run_in_a_linked_worktree_points_at_the_worktree() {
        let Some((c, root)) = approved_repo() else {
            return;
        };
        write(
            &root.join(".claude/memory/project-a.md"),
            &Note::new("project-a").desc("A").text(),
            None,
        );
        c.commit_all(&root, "c");
        let wt = c.tmp.join("wt");
        c.git(
            &root,
            &["worktree", "add", "-q", &wt.to_string_lossy(), "-b", "wt"],
        );
        let (ctx, _msgs) = run(&c, &wt);
        let settings = load(&wt.join(".claude/settings.local.json"));
        assert_eq!(
            settings
                .as_object()
                .unwrap()
                .get("autoMemoryDirectory")
                .and_then(Value::as_str),
            Some(&*wt.join(".claude/memory").to_string_lossy())
        );
        assert!(ctx.contains("project-a"));
        assert!(!root.join(".claude/settings.local.json").exists());
    }
}

/// ReCloneTest (needs git).
mod re_clone_test {
    use super::*;

    #[test]
    fn test_new_clone_of_an_approved_repo() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let bare = c.tmp.join("origin.git");
        fs::create_dir(&bare).unwrap();
        c.git(&bare, &["init", "-q", "--bare"]);
        let a = c.make_repo("clone-a", Some(&bare.to_string_lossy()));
        write(
            &a.join(".claude/memory/feedback-x.md"),
            &Note::new("feedback-x")
                .type_("feedback")
                .desc("Prefer small commits")
                .updated("2026-02-02")
                .text(),
            None,
        );
        c.commit_all(&a, "c");
        c.git(&a, &["push", "-q", "origin", "main"]);
        approve(&c, &a);
        run(&c, &a);
        let b = c.tmp.join("elsewhere/clone-b");
        fs::create_dir(b.parent().unwrap()).unwrap();
        c.git(
            &c.tmp,
            &["clone", "-q", &bare.to_string_lossy(), &b.to_string_lossy()],
        );
        let g = anthology_core::git::RealGit;
        assert_eq!(
            anthology_memory::repo::repo_key(&a, &c.env, &g),
            anthology_memory::repo::repo_key(&b, &c.env, &g)
        );
        assert!(anthology_memory::approve::is_approved(
            &anthology_memory::repo::repo_key(&b, &c.env, &g),
            &c.env
        ));
        let (ctx, _msgs) = run(&c, &b);
        let settings = load(&b.join(".claude/settings.local.json"));
        assert_eq!(
            settings
                .as_object()
                .unwrap()
                .get("autoMemoryDirectory")
                .and_then(Value::as_str),
            Some(&*b.join(".claude/memory").to_string_lossy())
        );
        assert!(ctx.contains("[feedback-x](feedback-x.md)"));
        assert_eq!(
            anthology_testkit::mem::try_git(
                &c.env,
                &b,
                &["check-ignore", "-q", ".claude/settings.local.json"]
            )
            .code,
            0
        );
        assert!(
            !c.git(&b, &["status", "--porcelain"])
                .stdout_text()
                .contains(".claude/settings.local.json")
        );
        let before = snapshot(&b);
        let exclude = fs::read(b.join(".git/info/exclude")).unwrap();
        assert_eq!(run(&c, &b), (personal_only(&c.anth), vec![]));
        assert_eq!(snapshot(&b), before);
        assert_eq!(fs::read(b.join(".git/info/exclude")).unwrap(), exclude);
    }

    #[test]
    fn test_empty_repo_memory_cover_has_a_placeholder() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        approve(&c, &root);
        let (ctx, _msgs) = run(&c, &root);
        assert!(ctx.starts_with("## "));
        assert!(!ctx.trim().lines().last().unwrap().trim().is_empty());
        assert_eq!(run(&c, &root), (personal_only(&c.anth), vec![]));
    }
}

/// RunFlowTest (needs git).
mod run_flow_test {
    use super::*;

    #[test]
    fn test_personal_comes_before_the_repo_cover() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        approve(&c, &root);
        write(
            &root.join(".claude/memory/project-a.md"),
            &Note::new("project-a").desc("Repo thing").text(),
            None,
        );
        write(
            &c.anth.join("personal-memory/user-me.md"),
            &note("user-me", "user", "Me"),
            None,
        );
        let (ctx, _) = run(&c, &root);
        let heads: Vec<&str> = ctx
            .lines()
            .filter(|l| l.starts_with("## ") && l.to_lowercase().contains("memory"))
            .collect();
        assert!(heads[0].contains(&*c.anth.join("personal-memory").to_string_lossy()));
        assert!(heads[1].contains(&*root.join(".claude/memory").to_string_lossy()));
        assert!(ctx.find("user-me").unwrap() < ctx.find("project-a").unwrap());
    }

    #[test]
    fn test_agent_memory_indexes_are_regenerated() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        approve(&c, &root);
        let agent = root.join(".claude/agent-memory/anthology-writer");
        write(
            &agent.join("feedback-q.md"),
            &note("feedback-q", "feedback", "Q"),
            None,
        );
        run(&c, &root);
        assert!(read(&agent.join("MEMORY.md")).contains("[feedback-q](feedback-q.md)"));
    }

    #[test]
    fn test_migration_runs_in_approved_repos_only_and_includes_main_worktree() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        write(&root.join("f"), "x", None);
        c.commit_all(&root, "c");
        approve(&c, &root);
        let wt = c.tmp.join("wt");
        c.git(
            &root,
            &["worktree", "add", "-q", &wt.to_string_lossy(), "-b", "wt"],
        );
        let old_main = c.config.join("projects").join(encode(&root)).join("memory");
        let old_wt = c.config.join("projects").join(encode(&wt)).join("memory");
        write(
            &old_main.join("project-m.md"),
            &note("project-m", "project", "from main"),
            None,
        );
        write(
            &old_main.join("user-u.md"),
            &note("user-u", "user", "personal one"),
            None,
        );
        write(
            &old_wt.join("feedback-w.md"),
            &note("feedback-w", "feedback", "from wt"),
            None,
        );
        let (ctx, msgs) = run(&c, &wt);
        let repo_mem = wt.join(".claude/memory");
        let personal = c.anth.join("personal-memory");
        assert!(md_names(&repo_mem).is_empty());
        assert_eq!(
            md_names(&personal),
            ["feedback-w.md", "project-m.md", "user-u.md"]
        );
        assert!(ctx.contains("personal one"));
        assert!(ctx.contains("from main"));
        assert!(c.anth.join("migrated.json").exists());
        let p = personal.to_string_lossy();
        assert!(
            msgs.iter().any(|m| m.contains("3 note(s)")
                && m.contains(&*p)
                && m.contains("/anthology:setup")),
            "{msgs:?}"
        );
        assert!(old_main.join("project-m.md").exists());
        run(&c, &wt);
        assert_eq!(
            md_names(&personal),
            ["feedback-w.md", "project-m.md", "user-u.md"]
        );
    }
}

/// TrackedSettingsTest (needs git): a committed settings.local.json never gets this clone's path.
mod tracked_settings_test {
    use super::*;

    #[test]
    fn test_tracked_settings_file_is_left_alone() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        write(&root.join(".claude/settings.local.json"), "{}\n", None);
        c.commit_all(&root, "c");
        approve(&c, &root);
        let exclude = root.join(".git/info/exclude");
        let exclude_before = fs::read(&exclude).unwrap_or_default();
        let before = snapshot(&root);
        let status = c.git(&root, &["status", "--porcelain"]).stdout;
        for _ in 0..3 {
            let (ctx, msgs) = run(&c, &root);
            assert_eq!(ctx, personal_only(&c.anth));
            assert!(
                msgs.iter()
                    .any(|m| m.contains("tracked") && m.contains("git rm --cached")),
                "{msgs:?}"
            );
        }
        assert_eq!(read(&root.join(".claude/settings.local.json")), "{}\n");
        assert_eq!(fs::read(&exclude).unwrap_or_default(), exclude_before);
        assert_eq!(c.git(&root, &["status", "--porcelain"]).stdout, status);
        assert_eq!(snapshot(&root), before);
    }

    #[test]
    fn test_exclude_line_is_never_appended_twice() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let d = SyncDeps::default();
        let root = c.make_repo("repo", None);
        write(
            &root.join(".gitignore"),
            "!.claude/settings.local.json\n",
            None,
        );
        let exclude = root.join(".git/info/exclude");
        fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        fs::write(&exclude, "/.claude/settings.local.json\n").unwrap();
        for _ in 0..3 {
            assert!(!sync::ensure_ignored(&root, &c.env, &d).unwrap());
        }
        assert_eq!(read(&exclude), "/.claude/settings.local.json\n");
        let other = c.make_repo("other", None);
        for _ in 0..3 {
            sync::ensure_ignored(&other, &c.env, &d).unwrap();
        }
        let lines = read(&other.join(".git/info/exclude"));
        assert_eq!(
            lines
                .lines()
                .filter(|l| *l == "/.claude/settings.local.json")
                .count(),
            1
        );
    }
}

/// SymlinkSafetyTest (needs git): a repo's own symlinks must not redirect anthology's writes.
mod symlink_safety_test {
    use super::*;

    fn repo() -> Option<(MemCase, PathBuf)> {
        if !needs_git() {
            return None;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        Some((c, root))
    }

    #[test]
    fn test_symlinked_gitattributes_is_not_appended_through() {
        let Some((c, root)) = repo() else { return };
        let victim = c.tmp.join("victim.rc");
        write(&victim, "export X=1\n", None);
        symlink(&victim, root.join(".gitattributes")).unwrap();
        let g = anthology_core::git::RealGit;
        let e = anthology_memory::approve::approve(&root, &c.env, &g).unwrap_err();
        assert_eq!(e.type_name(), "UnsafeTarget");
        assert_eq!(read(&victim), "export X=1\n");
        // recorded first: approving again finds the key and fails on the attributes only
        let approved = read(&c.anth.join("approved-repos.json"));
        assert!(approved.contains(&format!("path:{}", root.to_string_lossy())));
        let e = anthology_memory::approve::ensure_union_attrs(&root).unwrap_err();
        assert_eq!(e.type_name(), "UnsafeTarget");
        assert_eq!(read(&victim), "export X=1\n");
    }

    #[test]
    fn test_fifos_never_hang_a_session() {
        let Some((c, root)) = repo() else { return };
        mkfifo(&root.join(".gitattributes"));
        fs::create_dir(root.join(".claude")).unwrap();
        mkfifo(&root.join(".claude/settings.local.json"));
        let env = c.env.clone();
        let r2 = root.clone();
        let out = run_with_timeout(30, move || {
            let refused =
                anthology_memory::approve::approve(&r2, &env, &anthology_core::git::RealGit)
                    .is_err_and(|e| e.type_name() == "UnsafeTarget");
            let (_ctx, msgs) = sync::run(&r2, &env, &SyncDeps::default());
            (
                refused,
                msgs.iter().any(|m| m.contains("settings.local.json")),
            )
        });
        if out.is_none() {
            unblock(&root.join(".gitattributes"));
            unblock(&root.join(".claude/settings.local.json"));
        }
        assert_eq!(out, Some((true, true)));
    }

    #[test]
    fn test_symlinked_settings_local_is_not_written_through() {
        let Some((c, root)) = repo() else { return };
        approve(&c, &root);
        let victim = c.tmp.join("victim.json");
        write(&victim, "{}\n", None);
        fs::create_dir(root.join(".claude")).unwrap();
        symlink(&victim, root.join(".claude/settings.local.json")).unwrap();
        run(&c, &root);
        assert_eq!(read(&victim), "{}\n");
        assert!(
            fs::symlink_metadata(root.join(".claude/settings.local.json"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn test_symlinked_info_exclude_is_refused() {
        let Some((c, root)) = repo() else { return };
        let victim = c.tmp.join("victim.txt");
        write(&victim, "keep\n", None);
        let exclude = root.join(".git/info/exclude");
        fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        let _ = fs::remove_file(&exclude);
        symlink(&victim, &exclude).unwrap();
        let e = sync::ensure_ignored(&root, &c.env, &SyncDeps::default()).unwrap_err();
        assert_eq!(e.type_name(), "UnsafeTarget");
        assert_eq!(read(&victim), "keep\n");
    }

    fn setup_outside() -> Option<(MemCase, PathBuf, PathBuf)> {
        let (c, root) = repo()?;
        approve(&c, &root);
        let outside = c.tmp.join("outside");
        write(&outside.join("MEMORY.md"), "precious\n", None);
        write(
            &outside.join("project-x.md"),
            &Note::new("project-x").text(),
            None,
        );
        fs::create_dir(root.join(".claude")).unwrap();
        let old = c.config.join("projects").join(encode(&root)).join("memory");
        write(
            &old.join("project-old.md"),
            &Note::new("project-old").text(),
            None,
        );
        Some((c, root, outside))
    }

    #[test]
    fn test_symlinked_repo_memory_dir_skips_all_repo_memory_work() {
        let Some((c, root, outside)) = setup_outside() else {
            return;
        };
        symlink(&outside, root.join(".claude/memory")).unwrap();
        let before = snapshot(&outside);
        let (ctx, msgs) = run(&c, &root);
        assert_eq!(snapshot(&outside), before);
        assert_eq!(ctx, personal_only(&c.anth));
        assert!(
            msgs.iter()
                .any(|m| m.contains("symlink") && m.contains(".claude/memory")),
            "{msgs:?}"
        );
        assert!(!root.join(".claude/settings.local.json").exists());
        assert!(!c.anth.join("migrated.json").exists());
    }

    #[test]
    fn test_ensure_memory_dir_refuses_symlinked_or_non_folder_memory() {
        let Some((c, root, outside)) = setup_outside() else {
            return;
        };
        let settings = root.join(".claude/settings.local.json");
        symlink(&outside, root.join(".claude/memory")).unwrap();
        let (m, changed) = sync::ensure_memory_dir(&root, &settings).unwrap();
        assert!(m.is_empty() && !changed);
        assert!(!settings.exists());
        fs::remove_file(root.join(".claude/memory")).unwrap();
        write(&root.join(".claude/memory"), "a file, not a folder", None);
        let (m, changed) = sync::ensure_memory_dir(&root, &settings).unwrap();
        assert!(m.is_empty() && !changed);
        let (_ctx, msgs) = run(&c, &root);
        assert!(
            msgs.iter()
                .any(|m| m.contains("not a folder") || m.contains("symlink")),
            "{msgs:?}"
        );
        assert!(!settings.exists());
    }

    #[test]
    fn test_migrate_does_not_write_through_symlinked_destinations() {
        let c = MemCase::new();
        let old = c.tmp.join("old");
        write(
            &old.join("project-a.md"),
            &Note::new("project-a").text(),
            None,
        );
        write(
            &old.join("user-u.md"),
            &Note::new("user-u").type_("user").text(),
            None,
        );
        let outside = c.tmp.join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, c.tmp.join("repo-mem")).unwrap();
        symlink(&outside, c.tmp.join("personal-link")).unwrap();
        let r = sync::migrate(
            &old,
            Some(&c.tmp.join("repo-mem")),
            &c.tmp.join("personal-link"),
            None,
            None,
            &[],
            None,
            &SyncDeps::default(),
        );
        assert!(names(&outside).is_empty());
        assert!(r.copied.is_empty());
    }
}

/// ContextBudgetTest.
mod context_budget_test {
    use super::*;

    fn seed(c: &MemCase) {
        for i in 0..300 {
            write(
                &c.anth.join(format!("personal-memory/user-{i:03}.md")),
                &Note::new(&format!("user-{i:03}"))
                    .type_("user")
                    .desc(&format!("Personal note number {i}"))
                    .updated(&format!("2026-{:02}-{:02}", i / 28 % 12 + 1, i % 28 + 1))
                    .text(),
                None,
            );
        }
    }

    /// `re.split(r"(?m)^(?=## (?:Personal|Repo) memory \()", ctx)`, keeping the `## ` parts.
    fn sections(ctx: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for line in ctx.split_inclusive('\n') {
            if line.starts_with("## Personal memory (")
                || line.starts_with("## Repo memory (")
                || out.is_empty()
            {
                out.push(String::new());
            }
            out.last_mut().unwrap().push_str(line);
        }
        out.into_iter().filter(|p| p.starts_with("## ")).collect()
    }

    #[test]
    fn test_personal_section_is_capped_but_on_disk_index_is_not() {
        let c = MemCase::new();
        seed(&c);
        let folder = c.tmp.join("plain");
        fs::create_dir(&folder).unwrap();
        let (ctx, _msgs) = run(&c, &folder);
        let secs = sections(&ctx);
        assert_eq!(secs.len(), 1);
        let (heading, body) = secs[0].split_once("\n\n").unwrap();
        assert!(heading.starts_with("## Personal memory"));
        assert!(body.len() <= 2500);
        assert!(body.lines().count() <= 40);
        let disk = c.anth.join("personal-memory/MEMORY.md");
        assert!(body.contains("more — use `anthology-mem search` (full index: "));
        assert!(body.contains(&*disk.to_string_lossy()));
        assert!(read(&disk).lines().count() > 100);
        assert!(fs::read(&disk).unwrap().len() <= 20000);
    }

    #[test]
    fn test_repo_cover_still_present_after_a_repair() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        seed(&c);
        let root = c.make_repo("repo", None);
        write(
            &root.join(".claude/memory/project-a.md"),
            &Note::new("project-a").desc("Repo thing").text(),
            None,
        );
        approve(&c, &root);
        let (ctx, _msgs) = run(&c, &root);
        let secs = sections(&ctx);
        assert_eq!(secs.len(), 2);
        assert!(secs[0].starts_with("## Personal memory"));
        assert!(secs[1].starts_with("## Repo memory"));
        assert!(secs[1].contains("[project-a](project-a.md)"));
        assert!(secs[0].split_once("\n\n").unwrap().1.len() <= 2500);
    }
}

/// LegacyNotesStayOutOfTheRepoTest: B3 reproduced, without git (fake_repo).
mod legacy_notes_stay_out_of_the_repo_test {
    use super::*;

    #[test]
    fn test_session_start_copies_legacy_notes_to_personal_memory_only() {
        let c = MemCase::new();
        let g = fake_repo(&c, "repo", None);
        let root = g.root.clone();
        approve_fake(&c, &g);
        let old = c.config.join("projects").join(encode(&root)).join("memory");
        write(
            &old.join("reference-build-host.md"),
            &Note::new("reference-build-host")
                .type_("reference")
                .desc("The build host")
                .body(&["Builds run on build-", "box.lan as deploy.\n"].concat())
                .text(),
            None,
        );
        write(
            &old.join("project-plan.md"),
            &note("project-plan", "project", "The plan"),
            None,
        );
        let (ctx, msgs) = run_with(&c, &root, &g);
        let repo_mem = root.join(".claude/memory");
        assert!(names(&repo_mem).is_empty());
        let personal = c.anth.join("personal-memory");
        for n in ["reference-build-host.md", "project-plan.md"] {
            assert_eq!(
                fs::read(personal.join(n)).unwrap(),
                fs::read(old.join(n)).unwrap()
            );
        }
        let done = load(&c.anth.join("migrated.json"));
        assert_eq!(
            done.as_object()
                .unwrap()
                .get("done")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            2
        );
        let p = personal.to_string_lossy();
        assert!(
            msgs.iter().any(|m| m.contains("2 note(s)")
                && m.contains(&*p)
                && m.contains("/anthology:setup")),
            "{msgs:?}"
        );
        let bb = ["build-", "box"].concat();
        assert!(
            !ctx.lines()
                .filter(|l| l.contains("Repo memory"))
                .any(|l| l.contains(&bb))
        );
        let (_ctx, msgs) = run_with(&c, &root, &g);
        assert!(!msgs.iter().any(|m| m.contains("note(s)")), "{msgs:?}");
        assert!(names(&repo_mem).is_empty());
    }
}

/// UserNotesLeaveTheRepoTest: C1, without git (fake_repo).
mod user_notes_leave_the_repo_test {
    use super::*;
    use anthology_memory::sync::SweepResult;
    use anthology_pycompat::pyerr::PyExc;

    struct T {
        c: MemCase,
        g: FakeGit,
        root: PathBuf,
        repo_mem: PathBuf,
        agent: PathBuf,
        personal: PathBuf,
    }

    fn set_up() -> T {
        let c = MemCase::new();
        let g = fake_repo(&c, "repo", None);
        let root = g.root.clone();
        approve_fake(&c, &g);
        let repo_mem = root.join(".claude/memory");
        let agent = root.join(".claude/agent-memory/anthology-scout");
        let personal = c.anth.join("personal-memory");
        write(
            &repo_mem.join("user-role.md"),
            &note("user-role", "user", "Senior engineer"),
            None,
        );
        write(
            &repo_mem.join("project-a.md"),
            &note("project-a", "project", "Repo thing"),
            None,
        );
        write(
            &agent.join("user-tz.md"),
            &note("user-tz", "user", "Works in CET"),
            None,
        );
        write(
            &agent.join("feedback-b.md"),
            &note("feedback-b", "feedback", "Agent thing"),
            None,
        );
        T {
            c,
            g,
            root,
            repo_mem,
            agent,
            personal,
        }
    }

    impl T {
        fn run(&self) -> (String, Vec<String>) {
            run_with(&self.c, &self.root, &self.g)
        }
    }

    #[test]
    fn test_user_notes_move_to_personal_memory_at_session_start() {
        let t = set_up();
        let role = fs::read(t.repo_mem.join("user-role.md")).unwrap();
        let tz = fs::read(t.agent.join("user-tz.md")).unwrap();
        let (ctx, msgs) = t.run();
        assert!(!t.repo_mem.join("user-role.md").exists());
        assert!(!t.agent.join("user-tz.md").exists());
        assert_eq!(fs::read(t.personal.join("user-role.md")).unwrap(), role);
        assert_eq!(fs::read(t.personal.join("user-tz.md")).unwrap(), tz);
        assert!(t.repo_mem.join("project-a.md").exists());
        assert!(t.agent.join("feedback-b.md").exists());
        let moved: Vec<&String> = msgs
            .iter()
            .filter(|m| m.contains("moved 2 user note(s)"))
            .collect();
        assert_eq!(moved.len(), 1, "{msgs:?}");
        assert!(moved[0].contains(&*t.personal.to_string_lossy()));
        assert!(moved[0].contains("commit their removal"));
        for index in [t.repo_mem.join("MEMORY.md"), t.agent.join("MEMORY.md")] {
            let text = read(&index);
            assert!(!text.contains("## user"));
            assert!(!text.contains("user-"));
        }
        assert!(read(&t.personal.join("MEMORY.md")).contains("[user-role](user-role.md)"));
        let (personal_part, repo_part) = ctx.split_once("## Repo memory").unwrap();
        assert!(personal_part.contains("user-role"));
        assert!(repo_part.contains("project-a"));
        assert!(!repo_part.contains("user-"));
    }

    #[test]
    fn test_a_symlinked_personal_folder_is_a_visible_message_not_a_silent_skip() {
        let t = set_up();
        let target = t.c.tmp.join("dotfiles-memory");
        write(
            &target.join("feedback-mine.md"),
            &note("feedback-mine", "feedback", "Mine"),
            None,
        );
        fs::create_dir_all(t.personal.parent().unwrap()).unwrap();
        symlink(&target, &t.personal).unwrap();
        let old =
            t.c.config
                .join("projects")
                .join(encode(&t.root))
                .join("memory");
        write(
            &old.join("project-old.md"),
            &note("project-old", "project", "Old"),
            None,
        );
        let before = snapshot(&target);
        let (ctx, msgs) = t.run();
        assert_eq!(snapshot(&target), before);
        assert!(t.repo_mem.join("user-role.md").exists());
        assert!(!t.c.anth.join("migrated.json").exists());
        let warned: Vec<&String> = msgs
            .iter()
            .filter(|m| m.contains("personal-memory") && m.contains("symlink"))
            .collect();
        assert_eq!(warned.len(), 1, "{msgs:?}");
        assert!(warned[0].contains("not moved"));
        assert!(!ctx.contains("feedback-mine"));
        assert!(ctx.contains("Personal memory is unavailable"));
        assert!(!ctx.contains("Save notes about the user"));
        // `anthology-mem search` skips it too
        assert!(anthology_memory::repo::note_sources_opt(None, &t.c.anth).is_empty());
        let scopes: Vec<String> = anthology_memory::repo::note_sources(&t.root, &t.c.anth)
            .into_iter()
            .map(|n| n.scope)
            .collect();
        assert!(!scopes.contains(&"personal".to_string()));
    }

    #[test]
    fn test_a_symlinked_personal_folder_is_reported_outside_an_approved_repo_too() {
        let t = set_up();
        let target = t.c.tmp.join("dotfiles-memory");
        fs::create_dir(&target).unwrap();
        fs::create_dir_all(t.personal.parent().unwrap()).unwrap();
        symlink(&target, &t.personal).unwrap();
        let (_ctx, msgs) = run_with(&t.c, &t.c.tmp, &t.g);
        assert_eq!(
            msgs.iter()
                .filter(|m| m.contains("personal-memory") && m.contains("symlink"))
                .count(),
            1,
            "{msgs:?}"
        );
    }

    #[test]
    fn test_a_conflicting_personal_note_is_kept_side_by_side() {
        let t = set_up();
        write(&t.personal.join("user-role.md"), "my own\n", None);
        let role = fs::read(t.repo_mem.join("user-role.md")).unwrap();
        t.run();
        assert_eq!(read(&t.personal.join("user-role.md")), "my own\n");
        let copies: Vec<String> = names(&t.personal)
            .into_iter()
            .filter(|n| n.starts_with("user-role.migrated-") && n.ends_with(".md"))
            .collect();
        assert_eq!(copies.len(), 1);
        assert_eq!(fs::read(t.personal.join(&copies[0])).unwrap(), role);
        assert!(!t.repo_mem.join("user-role.md").exists());
    }

    #[test]
    fn test_a_symlinked_user_note_is_untouched() {
        let t = set_up();
        let outside = t.c.tmp.join("outside.md");
        write(&outside, &note("user-out", "user", "Outside"), None);
        symlink(&outside, t.repo_mem.join("user-link.md")).unwrap();
        t.run();
        assert!(
            fs::symlink_metadata(t.repo_mem.join("user-link.md"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(outside.is_file());
        assert!(!t.personal.join("user-link.md").exists());
        assert!(!read(&t.repo_mem.join("MEMORY.md")).contains("user-link"));
    }

    fn stuck(_: &Path, _: &Path, _: Option<Date>, _: &SyncDeps<'_>) -> Result<SweepResult, PyExc> {
        Ok(SweepResult::default())
    }

    #[test]
    fn test_repo_indexes_never_list_user_notes_even_if_the_move_fails() {
        let t = set_up();
        write(
            &t.repo_mem.join("MEMORY.md"),
            "## user\n- [user-role](user-role.md)\n",
            None,
        );
        write(
            &t.agent.join("MEMORY.md"),
            "## user\n- [user-tz](user-tz.md)\n",
            None,
        );
        fs::remove_file(t.agent.join("feedback-b.md")).unwrap();
        let deps = SyncDeps {
            git: &t.g,
            sweep_user_notes: stuck,
            ..SyncDeps::default()
        };
        let (ctx, _) = sync::run(&t.root, &t.c.env, &deps);
        assert!(t.repo_mem.join("user-role.md").exists());
        for index in [t.repo_mem.join("MEMORY.md"), t.agent.join("MEMORY.md")] {
            let text = read(&index);
            assert!(!text.contains("user-"));
            assert!(text.starts_with(anthology_memory::render::GENERATED));
        }
        assert!(ctx.contains("## Repo memory"));
        assert!(!ctx.contains("user-role"));
    }

    #[test]
    fn test_an_index_left_with_no_notes_drops_the_moved_ones() {
        let t = set_up();
        fs::remove_file(t.agent.join("feedback-b.md")).unwrap();
        anthology_memory::render::regenerate_index(
            &t.agent,
            anthology_memory::render::DEFAULT_BUDGET,
            &[],
        )
        .unwrap();
        assert!(read(&t.agent.join("MEMORY.md")).contains("user-tz"));
        t.run();
        assert_eq!(
            read(&t.agent.join("MEMORY.md")),
            format!("{}\n", anthology_memory::render::GENERATED)
        );
    }

    #[test]
    fn test_render_index_exclude_types_keyword() {
        let t = set_up();
        let b = anthology_memory::render::DEFAULT_BUDGET;
        assert!(
            anthology_memory::render::render_index(&t.repo_mem, b, "", &[])
                .unwrap()
                .unwrap()
                .contains("[user-role]")
        );
        let text = anthology_memory::render::render_index(&t.repo_mem, b, "", &["user"])
            .unwrap()
            .unwrap();
        assert!(!text.contains("user-role"));
        assert!(text.contains("[project-a]"));
        assert!(anthology_memory::render::regenerate_index(&t.repo_mem, b, &["user"]).unwrap());
        assert!(!read(&t.repo_mem.join("MEMORY.md")).contains("user-role"));
    }

    fn denied(_: &Path, _: &Path, _: Option<Date>, _: &SyncDeps<'_>) -> Result<SweepResult, PyExc> {
        // PermissionError(13, "denied"): str() is "[Errno 13] denied".
        Err(PyExc::Other("PermissionError", "[Errno 13] denied".into()))
    }

    #[test]
    fn test_a_failing_repo_step_keeps_the_personal_section() {
        let t = set_up();
        write(
            &t.personal.join("user-me.md"),
            &note("user-me", "user", "Me"),
            None,
        );
        let deps = SyncDeps {
            git: &t.g,
            sweep_user_notes: denied,
            ..SyncDeps::default()
        };
        let (ctx, msgs) = sync::run(&t.root, &t.c.env, &deps);
        assert!(ctx.starts_with(&format!(
            "## Personal memory ({})",
            t.personal.to_string_lossy()
        )));
        assert!(ctx.contains("[user-me](user-me.md)"));
        assert!(
            msgs.iter()
                .any(|m| m.contains("repo memory step failed") && m.contains("denied")),
            "{msgs:?}"
        );
    }

    #[test]
    fn test_odd_user_types_are_left_out_of_repo_indexes() {
        let t = set_up();
        write(
            &t.repo_mem.join("user-caps.md"),
            &note("user-caps", "USER", "Caps"),
            None,
        );
        let text = anthology_memory::render::render_index(
            &t.repo_mem,
            anthology_memory::render::DEFAULT_BUDGET,
            "",
            &["user"],
        )
        .unwrap()
        .unwrap();
        assert!(!text.contains("user-caps"));
        assert!(!text.contains("user-role"));
    }

    #[test]
    fn test_the_personal_section_is_always_there_with_its_path() {
        let t = set_up();
        let g = fake_repo(&t.c, "plain-repo", Some("example.invalid/acme/other"));
        let (ctx, _) = run_with(&t.c, &g.root, &g);
        assert_eq!(ctx, personal_only(&t.c.anth));
        assert!(!t.personal.exists());
        fs::create_dir_all(&t.personal).unwrap();
        let (ctx, _) = run_with(&t.c, &g.root, &g);
        assert_eq!(ctx, personal_only(&t.c.anth));
        assert!(ctx.contains(&format!(
            "{}/user-<slug>.md, never in the repo",
            t.personal.to_string_lossy()
        )));
    }
}

/// SpecialMemoryMdTest: the `save_note` case (the other four are MS1's).
mod special_memory_md_test {
    use super::*;

    #[test]
    fn test_save_note_returns_promptly_with_a_fifo_memory_md() {
        let c = MemCase::new();
        let root = c.tmp.join("work");
        let agent_dir = root.join(".claude/agent-memory/anthology-scout");
        fs::create_dir_all(&agent_dir).unwrap();
        let fifo = agent_dir.join("MEMORY.md");
        mkfifo(&fifo);
        let r = root.clone();
        let out = run_with_timeout(5, move || {
            anthology_memory::save::save_note(
                &r,
                "scout",
                "project",
                "x",
                "n",
                "d",
                "b",
                false,
                None,
                &Env::default(),
            )
        });
        if out.is_none() {
            unblock(&fifo);
        }
        let out = out
            .expect("save_note blocked on a FIFO MEMORY.md")
            .expect("saved");
        assert_eq!(out.index_warning, None);
        let m = fs::symlink_metadata(&fifo).unwrap();
        assert!(m.file_type().is_file());
    }
}

/// One sample per G6 pattern, assembled so no secret-looking literal is committed.
/// `_label_samples`: `(label, secret text, value parts)` for every `SECRET_PATTERNS` label.
fn label_samples() -> Vec<(&'static str, String, Vec<String>)> {
    let s = |parts: &[&str]| parts.concat();
    let row = |label, text: String, vals: &[&str]| {
        (label, text, vals.iter().map(|v| v.to_string()).collect())
    };
    vec![
        row(
            "aws-access-key",
            s(&["AKIA", "IOSFODNN7EXAMPLE"]),
            &["IOSFODNN7EXAMPLE"],
        ),
        row(
            "github-token",
            s(&["ghp", "_", &"a1b2".repeat(10)]),
            &["a1b2a1b2"],
        ),
        row(
            "github-token",
            s(&["github", "_pat_", "abcdefghij0123456789abc"]),
            &["abcdefghij0123456789abc"],
        ),
        row(
            "api-key",
            s(&["sk", "-", "a1b2c3d4e5f6g7h8i9j0"]),
            &["a1b2c3d4e5f6g7h8i9j0"],
        ),
        row(
            "slack-token",
            s(&["xox", "b-", "1234567890-abcdef"]),
            &["1234567890-abcdef"],
        ),
        row(
            "private-key",
            s(&["-----BEGIN OPENSSH ", "PRIVATE KEY-----"]),
            &[],
        ),
        row(
            "credential-assignment",
            s(&["pass", "word: hunter2-long-value"]),
            &["hunter2-long-value"],
        ),
        row(
            "jwt",
            s(&["eyJ", "hbGciOiJI", ".abcdefgh12.", "zyxwvuts98"]),
            &["abcdefgh12", "zyxwvuts98"],
        ),
        row(
            "cloud-secret-key",
            s(&["aws_secret_access_key = ", "wJalrXUtnFEMI-K7MDENG"]),
            &["wJalrXUtnFEMI-K7MDENG"],
        ),
        row(
            "url-credentials",
            s(&["postgres://app:", "hunter2-long-value", "@db.example.com/x"]),
            &["hunter2-long-value"],
        ),
        row(
            "anthropic-key",
            s(&["sk", "-ant-", "abcdefgh1234"]),
            &["abcdefgh1234"],
        ),
        row(
            "google-api-key",
            s(&["AIza", &"Ab1".repeat(11), "Ab"]),
            &["Ab1Ab1Ab1"],
        ),
        row(
            "stripe-live-key",
            s(&["sk", "_live_", "abcdefgh1234"]),
            &["abcdefgh1234"],
        ),
        row(
            "bearer-token",
            s(&["Bearer ", "abcdefghijklmnopqrstu"]),
            &["abcdefghijklmnopqrstu"],
        ),
        row(
            "basic-auth",
            s(&["Authorization: Basic ", "dXNlcjpwYXNzd29yZA=="]),
            &["dXNlcjpwYXNzd29yZA=="],
        ),
        row(
            "gitlab-token",
            s(&["glpat", "-", "abcdefghij0123456789"]),
            &["abcdefghij0123456789"],
        ),
        row(
            "huggingface-token",
            s(&["hf", "_", &"a1B2".repeat(8)]),
            &["a1B2a1B2"],
        ),
        row(
            "npm-token",
            s(&["npm", "_", &"a1B2c3D4".repeat(5)]),
            &["a1B2c3D4a1B2"],
        ),
    ]
}

fn secret_samples() -> Vec<(&'static str, String)> {
    vec![
        ("aws", ["AKIA", "IOSFODNN7EXAMPLE"].concat()),
        ("github token", ["ghp", "_abc123"].concat()),
        ("github pat", ["github", "_pat_", "abcdef"].concat()),
        ("openai style", ["sk", "-", "a1b2c3d4e5f6g7h8i9"].concat()),
        ("slack", ["xox", "b-123"].concat()),
        (
            "private key",
            ["-----BEGIN OPENSSH ", "PRIVATE KEY-----"].concat(),
        ),
        ("password", ["pass", "word: hunter2"].concat()),
        ("api key", ["API", "_KEY = abc"].concat()),
    ]
}

/// SaveNoteTest: `save_note`, the helper behind `anthology mem save` (no git needed).
mod save_note_test {
    use super::*;
    use anthology_memory::privacy::{Finding, PrivacyDeps};
    use anthology_memory::save::{
        SAVE_TYPES, SaveDeps, SaveError, Saved, save_note, save_note_with,
    };
    use anthology_pycompat::pyerr::PyExc;
    use std::cell::RefCell;

    const TODAY: Date = Date {
        y: 2026,
        m: 5,
        d: 6,
    };

    /// The keyword arguments of the Python helper `save(**kw)`, with its defaults.
    #[derive(Clone)]
    struct Args {
        agent: String,
        kind: String,
        slug: String,
        name: String,
        description: String,
        body: String,
        replace: bool,
        today: Option<Date>,
        env: Env,
    }

    fn args() -> Args {
        Args {
            agent: "scout".into(),
            kind: "project".into(),
            slug: "eval-probe".into(),
            name: "Eval probe".into(),
            description: "How the probe works".into(),
            body: "Body text.".into(),
            replace: false,
            today: Some(TODAY),
            env: Env::default(),
        }
    }

    impl Args {
        fn agent(mut self, v: &str) -> Args {
            self.agent = v.into();
            self
        }
        fn kind(mut self, v: &str) -> Args {
            self.kind = v.into();
            self
        }
        fn slug(mut self, v: &str) -> Args {
            self.slug = v.into();
            self
        }
        fn name(mut self, v: &str) -> Args {
            self.name = v.into();
            self
        }
        fn desc(mut self, v: &str) -> Args {
            self.description = v.into();
            self
        }
        fn body(mut self, v: &str) -> Args {
            self.body = v.into();
            self
        }
        fn replace(mut self, v: bool) -> Args {
            self.replace = v;
            self
        }
        fn today(mut self, v: Option<Date>) -> Args {
            self.today = v;
            self
        }
        fn env(mut self, v: Env) -> Args {
            self.env = v;
            self
        }
        fn field(self, f: &str, v: &str) -> Args {
            match f {
                "name" => self.name(v),
                "description" => self.desc(v),
                "body" => self.body(v),
                _ => panic!("{f}"),
            }
        }
    }

    struct T {
        c: MemCase,
        root: PathBuf,
        outside: PathBuf,
    }

    fn set_up() -> T {
        let c = MemCase::new();
        let root = c.tmp.join("work");
        fs::create_dir(&root).unwrap();
        let outside = c.tmp.join("outside");
        fs::create_dir(&outside).unwrap();
        T { c, root, outside }
    }

    impl T {
        fn save_in(&self, root: &Path, a: Args, deps: &SaveDeps) -> Result<Saved, SaveError> {
            save_note_with(
                root,
                &a.agent,
                &a.kind,
                &a.slug,
                &a.name,
                &a.description,
                &a.body,
                a.replace,
                a.today,
                &a.env,
                deps,
            )
        }
        fn save(&self, a: Args) -> Saved {
            self.save_in(&self.root, a, &SaveDeps::default())
                .expect("saved")
        }
        fn agent_dir(&self, agent: &str) -> PathBuf {
            self.root
                .join(".claude/agent-memory")
                .join(format!("anthology-{agent}"))
        }
        /// `assertRefused`: a `SaveError` (containing `fragment`) and nothing written anywhere.
        fn refused_in(&self, root: &Path, fragment: Option<&str>, a: Args) -> String {
            let before = snapshot(&self.c.tmp);
            let e = self
                .save_in(root, a, &SaveDeps::default())
                .expect_err("refused");
            if let Some(f) = fragment {
                assert!(e.0.contains(f), "{f:?} not in {:?}", e.0);
            }
            assert_eq!(snapshot(&self.c.tmp), before);
            e.0
        }
        fn refused(&self, fragment: Option<&str>, a: Args) -> String {
            self.refused_in(&self.root, fragment, a)
        }
    }

    /// The Python test's `subTest` rows (`tests/data/test_memory_sync.jsonl`).
    fn py_rows(test: &str) -> Vec<anthology_testkit::Row> {
        anthology_testkit::rows(
            "test_memory_sync",
            &format!("test_memory_sync.SaveNoteTest.{test}"),
        )
    }

    /// `memory_index.parse_note` (`notes::parse_note`).
    fn parse(p: &Path) -> anthology_memory::notes::NoteInfo {
        anthology_memory::notes::parse_note(p).expect("parse")
    }

    #[test]
    fn test_save_never_lists_user_notes_in_the_agent_index() {
        let t = set_up();
        let d = t.agent_dir("scout");
        write(
            &d.join("user-role.md"),
            &note("user-role", "user", "Senior engineer"),
            None,
        );
        write(
            &d.join("user-tz.md"),
            &note("user-tz", "User # me", "Works in CET"),
            None,
        );
        t.save(args());
        let index = read(&d.join("MEMORY.md"));
        assert!(index.contains("[Eval probe](project-eval-probe.md)"));
        assert!(!index.to_lowercase().contains("## user"));
        assert!(!index.contains("user-"));
    }

    #[test]
    fn test_privacy_check_is_the_single_wiring_point() {
        let t = set_up();
        let home = ["/", "Users", "/alice/notes"].concat();
        let msg = t.refused(
            Some("personal or private data"),
            args().body(&format!("Line one.\nsee {home} here")),
        );
        assert!(msg.contains("home path (line 2)"));
        assert!(!msg.contains("alice"));
        let mail = ["ping alice", "@", "corp-mail.io"].concat();
        let msg = t.refused(Some("email address"), args().desc(&mail));
        assert!(!msg.contains("corp-mail"));
        t.refused(
            Some("personal or private data"),
            args().name("Box at 192.168.1.20"),
        );
        t.save(
            args()
                .slug("clean")
                .body("Nothing private; see https://example.com/docs."),
        );
        assert!(t.agent_dir("scout").join("project-clean.md").is_file());
    }

    thread_local! {
        static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    fn spy(text: &str, _: Option<&str>, _: Option<&str>, _: &PrivacyDeps) -> Vec<Finding> {
        SEEN.with(|s| s.borrow_mut().push(text.to_string()));
        Vec::new()
    }

    #[test]
    fn test_save_calls_privacy_findings_for_each_field() {
        let t = set_up();
        let deps = SaveDeps {
            privacy_findings: spy,
            ..SaveDeps::default()
        };
        t.save_in(&t.root, args().slug("spy").body("Body here."), &deps)
            .unwrap();
        SEEN.with(|s| {
            assert_eq!(
                *s.borrow(),
                ["spy", "Eval probe", "How the probe works", "Body here.\n"]
            );
        });
    }

    /// The mode a file created with 0o644 gets under this process's umask (Python sets 022).
    fn mode_under_umask(dir: &Path) -> u32 {
        use std::os::unix::fs::OpenOptionsExt;
        let probe = dir.join(".umask-probe");
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o777)
            .open(&probe)
            .unwrap();
        let umask = !fs::metadata(&probe).unwrap().permissions().mode() & 0o777;
        fs::remove_file(&probe).unwrap();
        0o644 & !umask
    }

    #[test]
    fn test_creates_the_note_with_the_exact_shape() {
        let t = set_up();
        let want_mode = mode_under_umask(&t.c.tmp);
        let out = t.save(args());
        let path = t.agent_dir("scout").join("project-eval-probe.md");
        assert_eq!(out.path(), path);
        assert_eq!(out.root, t.root);
        assert_eq!(
            out.relative,
            ".claude/agent-memory/anthology-scout/project-eval-probe.md"
        );
        assert_eq!(out.action, "created");
        assert_eq!(out.index_warning, None);
        assert_eq!(
            read(&path),
            "---\nname: 'Eval probe'\ndescription: 'How the probe works'\nmetadata:\n  type: \
             project\n  modified: 2026-05-06\n---\n\nBody text.\n"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            want_mode
        );
    }

    #[test]
    fn test_modified_defaults_to_today() {
        let t = set_up();
        t.save(args().today(None));
        let text = read(&t.agent_dir("scout").join("project-eval-probe.md"));
        let today = anthology_core::clock::today_local().iso();
        assert!(text.contains(&format!("  modified: {today}\n")));
    }

    #[test]
    fn test_every_type_and_agent_is_accepted() {
        let t = set_up();
        for k in SAVE_TYPES {
            t.save(args().kind(k).slug("t"));
        }
        for agent in anthology_core::roster::ROSTER {
            if agent != "scout" {
                t.save(args().agent(agent));
            }
            assert!(
                md_names(&t.agent_dir(agent))
                    .iter()
                    .any(|n| n.contains('-')),
                "{agent}"
            );
        }
        let ts = md_names(&t.agent_dir("scout"));
        assert_eq!(ts.iter().filter(|n| n.ends_with("-t.md")).count(), 3);
        assert_eq!(SAVE_TYPES, ["project", "feedback", "reference"]);
    }

    #[test]
    fn test_roster_matches_the_agent_files() {
        let agents = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/anthology/agents");
        let mut stems: Vec<String> = md_names(&agents)
            .iter()
            .map(|n| n.trim_end_matches(".md").to_string())
            .collect();
        stems.sort();
        let mut roster: Vec<String> = anthology_core::roster::ROSTER
            .iter()
            .map(|s| s.to_string())
            .collect();
        roster.sort();
        assert_eq!(roster, stems);
    }

    #[test]
    fn test_save_uses_the_shared_secret_list() {
        // One anthology-side list (privacy); every guard G6 pattern is in it.
        let shared: Vec<(&str, bool)> = anthology_memory::privacy::SECRET_PATTERNS
            .iter()
            .map(|(_, p, ic)| (*p, *ic))
            .collect();
        for p in anthology_guard::secrets::GUARD_SECRET_PATTERNS {
            assert!(shared.contains(p), "{p:?}");
        }
    }

    #[test]
    fn test_tricky_values_round_trip_through_the_real_indexer() {
        let t = set_up();
        let rows = py_rows("test_tricky_values_round_trip_through_the_real_indexer");
        for (i, row) in rows.iter().enumerate() {
            let v = row.str("value");
            let out = t.save(
                args()
                    .slug(&format!("tricky-{i}"))
                    .name(v)
                    .desc(&format!("{v} d")),
            );
            let info = parse(&out.path());
            assert_eq!(info.kind.as_deref(), Some("project"), "{v}");
            assert_eq!(info.name, v.trim(), "{v}");
            assert_eq!(info.description, format!("{v} d").trim(), "{v}");
            assert_eq!(info.updated.as_deref(), Some("2026-05-06"), "{v}");
        }
    }

    #[test]
    fn test_indexer_build_search_and_memory_md() {
        // The parsed fields the indexer stores and the regenerated MEMORY.md. The SQLite build
        // and FTS search over these fields are MI1's (`memory_index` ids).
        let t = set_up();
        for k in ["project", "feedback", "reference"] {
            t.save(
                args()
                    .kind(k)
                    .slug(&format!("x-{k}"))
                    .name(&format!("Note {k}"))
                    .desc(&format!("desc {k}"))
                    .body(&format!("The quokka lives here, {k}.")),
            );
        }
        for k in ["project", "feedback", "reference"] {
            let info = parse(&t.agent_dir("scout").join(format!("{k}-x-{k}.md")));
            assert_eq!(info.name, format!("Note {k}"));
            assert_eq!(info.kind.as_deref(), Some(k));
            assert!(info.body.contains("quokka"));
        }
        let text = read(&t.agent_dir("scout").join("MEMORY.md"));
        let heads: Vec<&str> = text.lines().filter(|l| l.starts_with("## ")).collect();
        assert_eq!(heads, ["## feedback", "## project", "## reference"]);
        assert!(
            text.lines()
                .any(|l| l == "- [Note project](project-x-project.md) — desc project")
        );
    }

    #[test]
    fn test_save_regenerates_memory_md_each_time() {
        let t = set_up();
        t.save(args().slug("one").name("First"));
        t.save(args().slug("two").name("Second"));
        let index = read(&t.agent_dir("scout").join("MEMORY.md"));
        assert!(index.contains("[First](project-one.md)"));
        assert!(index.contains("[Second](project-two.md)"));
    }

    #[test]
    fn test_body_multiline_crlf_and_escapes() {
        let t = set_up();
        let d = t.agent_dir("scout");
        t.save(
            args()
                .slug("b1")
                .body("Line one.\nLine two\twith tab.\n\n- item; see [[link]]\n"),
        );
        assert!(
            read(&d.join("project-b1.md"))
                .ends_with("---\n\nLine one.\nLine two\twith tab.\n\n- item; see [[link]]\n")
        );
        t.save(
            args()
                .slug("b2")
                .body("\n\n  lead kept\r\nsecond\rthird   \n\n\n"),
        );
        let text = read(&d.join("project-b2.md"));
        assert!(text.ends_with("---\n\n  lead kept\nsecond\nthird\n"));
        assert!(!text.contains('\r'));
        t.save(
            args()
                .slug("b3")
                .body(r#"printf "a\n" and \t stay literal"#),
        );
        assert!(read(&d.join("project-b3.md")).ends_with("printf \"a\\n\" and \\t stay literal\n"));
        assert!(
            parse(&d.join("project-b1.md"))
                .body
                .contains("Line one.\nLine two")
        );
    }

    #[test]
    fn test_name_and_description_are_stored_stripped() {
        let t = set_up();
        let out = t.save(args().name("  Padded name  ").desc("padded  "));
        let info = parse(&out.path());
        assert_eq!(
            (info.name.as_str(), info.description.as_str()),
            ("Padded name", "padded")
        );
    }

    #[test]
    fn test_boundary_lengths_are_accepted() {
        let t = set_up();
        t.save(
            args()
                .slug(&"a".repeat(64))
                .name(&"n".repeat(120))
                .desc(&"d".repeat(300))
                .body(&"x".repeat(16384)),
        );
    }

    #[test]
    fn test_unknown_agent_and_type() {
        let t = set_up();
        let msg = t.refused(None, args().agent("nobody"));
        assert!(msg.contains("scout"));
        for a in ["", "Scout", "../scout", "anthology-scout", "scout/x"] {
            t.refused(None, args().agent(a));
        }
        let msg = t.refused(None, args().kind("user"));
        for k in SAVE_TYPES {
            assert!(msg.contains(k));
        }
        for k in ["other", "", "Project", "project/../x"] {
            t.refused(None, args().kind(k));
        }
    }

    #[test]
    fn test_bad_slugs() {
        let t = set_up();
        for row in py_rows("test_bad_slugs") {
            t.refused(None, args().slug(row.str("slug")));
        }
        assert!(!t.root.join(".claude").exists());
    }

    #[test]
    fn test_bad_name_and_description() {
        let t = set_up();
        for row in py_rows("test_bad_name_and_description") {
            t.refused(None, args().field(row.str("field"), row.str("value")));
        }
    }

    #[test]
    fn test_name_and_description_refuse_format_and_separator_characters() {
        let t = set_up();
        for row in py_rows("test_name_and_description_refuse_format_and_separator_characters") {
            let hex = row.str("char").trim_start_matches("0x");
            let ch = char::from_u32(u32::from_str_radix(hex, 16).unwrap()).unwrap();
            t.refused(None, args().field(row.str("field"), &format!("a{ch}b")));
        }
        for field in ["name", "description"] {
            t.save(
                args()
                    .slug(&format!("ok-{field}"))
                    .field(field, "plain ünïcode ✓ text"),
            );
        }
    }

    #[test]
    fn test_body_allows_format_characters_but_not_controls() {
        let t = set_up();
        t.save(
            args()
                .slug("zwj")
                .body("family \u{1F468}\u{200d}\u{1F469} ok\u{200b}"),
        );
        assert!(read(&t.agent_dir("scout").join("project-zwj.md")).contains('\u{200d}'));
        // The rows hold `repr(value)`.
        let rows = py_rows("test_body_allows_format_characters_but_not_controls");
        for (row, (v, r)) in rows
            .iter()
            .zip([("a\u{85}b", "'a\\x85b'"), ("a\u{9f}b", "'a\\x9fb'")])
        {
            assert_eq!(row.str("value"), r);
            t.refused(None, args().body(v));
        }
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_bad_body() {
        let t = set_up();
        // The rows hold `value[:12]`; the full values are Python's.
        let (x, e) = ("x".repeat(16385), "é".repeat(8193));
        let values = [
            "",
            "  \n\n \t ",
            "nul\x00",
            "bell\x07",
            "esc\x1b[0m",
            "del\x7f",
            x.as_str(),
            e.as_str(),
        ];
        let rows = py_rows("test_bad_body");
        assert_eq!(rows.len(), values.len());
        for (row, v) in rows.iter().zip(values) {
            assert_eq!(
                row.str("value"),
                anthology_pycompat::str::char_slice(v, 0, 12)
            );
            t.refused(None, args().body(v));
        }
    }

    fn file(o: Saved) -> String {
        o.path().file_name().unwrap().to_string_lossy().into_owned()
    }

    #[test]
    fn test_a_slug_that_repeats_the_type_is_not_doubled() {
        let t = set_up();
        let out = t.save(args().slug("project-eval-probe"));
        assert_eq!(
            out.path(),
            t.agent_dir("scout").join("project-eval-probe.md")
        );
        assert_eq!(
            out.relative,
            ".claude/agent-memory/anthology-scout/project-eval-probe.md"
        );
        assert_eq!(parse(&out.path()).kind.as_deref(), Some("project"));
        assert!(
            !t.agent_dir("scout")
                .join("project-project-eval-probe.md")
                .exists()
        );
        t.refused(Some("already exists"), args());
        assert_eq!(
            file(t.save(args().kind("feedback").slug("project-x"))),
            "feedback-project-x.md"
        );
        assert_eq!(
            file(t.save(args().kind("project").slug("project-project"))),
            "project-project.md"
        );
        assert_eq!(
            file(t.save(args().kind("reference").slug("reference-reference-x"))),
            "reference-reference-x.md"
        );
        for (k, s) in [
            ("feedback", "feedback"),
            ("project", "projectile"),
            ("project", "project2"),
        ] {
            assert_eq!(file(t.save(args().kind(k).slug(s))), format!("{k}-{s}.md"));
        }
    }

    #[test]
    fn test_the_slug_is_checked_for_secrets() {
        let t = set_up();
        let value = "abcdef0123456789abcd";
        let env = Env::default().with("X_API_TOKEN", value);
        let msg = t.refused(None, args().env(env).slug(value));
        assert!(msg.contains("$X_API_TOKEN"));
        assert!(!msg.contains(value));
        let key_like = ["sk", "-", "a1b2c3d4e5f6g7h8i9"].concat();
        let msg = t.refused(None, args().slug(&key_like));
        assert!(msg.contains("secret"));
        assert!(!msg.contains(&key_like));
        // the check runs on what the caller passed, before a type prefix is dropped
        let prefixed = format!("project-{value}");
        let env = Env::default().with("X_API_TOKEN", &prefixed);
        t.refused(None, args().env(env).slug(&prefixed));
    }

    #[test]
    fn test_each_secret_pattern_is_refused_everywhere() {
        let t = set_up();
        let samples = secret_samples();
        assert_eq!(
            samples.len(),
            anthology_guard::secrets::GUARD_SECRET_PATTERNS.len()
        );
        let rows = py_rows("test_each_secret_pattern_is_refused_everywhere");
        assert_eq!(rows.len(), samples.len() * 3);
        for row in rows {
            let (label, field) = (row.str("label"), row.str("field"));
            let secret = &samples
                .iter()
                .find(|(l, _)| *l == label)
                .expect("a sample")
                .1;
            let msg = t.refused(None, args().field(field, &format!("see {secret} here")));
            assert!(
                msg.contains("looks like it contains a secret"),
                "{label} {field}: {msg}"
            );
            assert!(!msg.contains(secret.as_str()));
        }
        assert!(!t.root.join(".claude").exists());
    }

    #[test]
    fn test_secret_refusal_names_the_kind_and_line_never_the_value() {
        let t = set_up();
        let secret = ["hunter", "2-long-value"].concat();
        let url = format!("postgres://app:{secret}@db.example.com/x");
        let msg = t.refused(
            None,
            args().body(&format!("first\nsecond\nconnect to {url}\nlast")),
        );
        assert!(
            msg.contains("Found: url-credentials on line 3 of the note body"),
            "{msg}"
        );
        // the instruction comes before the list
        assert!(
            msg.find("remove or replace that value") < msg.find("Found:"),
            "{msg}"
        );
        assert!(!msg.contains(&secret));
        assert!(!msg.contains('\n'));
        // several lines of one kind, and two kinds
        let aws = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
        let msg = t.refused(None, args().body(&format!("{aws}\nok\n{aws}\n{url}")));
        assert!(
            msg.contains("aws-access-key on lines 1, 3 of the note body"),
            "{msg}"
        );
        assert!(
            msg.contains("url-credentials on line 4 of the note body"),
            "{msg}"
        );
        assert!(!msg.contains(&aws));
        assert!(!msg.contains(&secret));
        // a single-line field is named, not numbered
        let msg = t.refused(None, args().name(&format!("see {aws}")));
        assert!(msg.contains("Found: aws-access-key in the name"), "{msg}");
        assert!(!msg.contains(&aws));
    }

    #[test]
    fn test_secret_refusal_never_contains_the_value_of_any_kind() {
        let t = set_up();
        let samples = label_samples();
        let labels: std::collections::BTreeSet<&str> = anthology_memory::privacy::SECRET_PATTERNS
            .iter()
            .map(|(l, _, _)| *l)
            .collect();
        let sampled: std::collections::BTreeSet<&str> =
            samples.iter().map(|(l, _, _)| *l).collect();
        assert_eq!(labels, sampled);
        for (label, secret, parts) in &samples {
            for field in ["name", "description", "body"] {
                let msg = t.refused(None, args().field(field, &format!("see {secret} here")));
                assert!(msg.contains(label), "{label} {field}: {msg}");
                assert!(!msg.contains('\n'));
                assert!(!msg.contains(secret.as_str()), "{label} {field}: {msg}");
                for part in parts {
                    assert!(!msg.contains(part), "{label} {field}: {msg}");
                }
            }
        }
    }

    #[test]
    fn test_secret_refusal_lists_at_most_five_findings() {
        let t = set_up();
        let aws = ["AKIA", "IOSFODNN7EXAMPLE"].concat();
        let msg = t.refused(None, args().body(&[aws.as_str(); 8].join("\n")));
        assert!(
            msg.contains("aws-access-key on lines 1, 2, 3, 4, 5 of the note body and 3 more"),
            "{msg}"
        );
        assert!(
            msg.find("remove or replace that value") < msg.find("Found:"),
            "{msg}"
        );
        assert!(!msg.contains(&aws));
        // exactly five: nothing is hidden
        let msg = t.refused(None, args().body(&[aws.as_str(); 5].join("\n")));
        assert!(
            msg.contains("lines 1, 2, 3, 4, 5 of the note body"),
            "{msg}"
        );
        assert!(!msg.contains("more"), "{msg}");
    }

    #[test]
    fn test_env_secret_values_are_refused_without_echoing_them() {
        let t = set_up();
        let value = ["vA1ue", "-of-the-thing"].concat();
        let env = Env::default()
            .with("MY_API_TOKEN", &value)
            .with("HOME", "/home/x");
        for row in py_rows("test_env_secret_values_are_refused_without_echoing_them") {
            if let Some(field) = row.params.get("field").and_then(Value::as_str) {
                let a = args()
                    .env(env.clone())
                    .field(field, &format!("contains {value}!"));
                let msg = t.refused(None, a);
                assert!(msg.contains("$MY_API_TOKEN"));
                assert!(!msg.contains(&value));
            } else {
                let var = row.str("var");
                t.refused(
                    None,
                    args().env(Env::default().with(var, &value)).body(&value),
                );
            }
        }
    }

    #[test]
    fn test_env_values_that_are_short_or_not_secret_named_do_not_trigger() {
        let t = set_up();
        let e = |k: &str, v: &str| Env::default().with(k, v);
        t.save(
            args()
                .slug("e1")
                .env(e("MY_API_TOKEN", "short"))
                .body("short appears: short"),
        );
        t.save(
            args()
                .slug("e2")
                .env(e("EDITOR", "vim-editor-long"))
                .body("I use vim-editor-long"),
        );
        let x7 = "x".repeat(7);
        t.save(args().slug("e3").env(e("MY_API_TOKEN", &x7)).body(&x7));
        let x8 = "x".repeat(8);
        t.refused(None, args().env(e("MY_API_TOKEN", &x8)).body(&x8));
    }

    #[test]
    fn test_env_defaults_to_the_process_environment() {
        // Python's `env=None` reads `os.environ`; the CLI passes the process environment.
        let t = set_up();
        let value = ["proc", "-env-value-123"].concat();
        let env = Env::from_process().with("SOME_TOKEN", &value);
        let r = save_note(
            &t.root, "scout", "project", "x", "n", "d", &value, false, None, &env,
        );
        assert!(r.is_err());
    }

    #[test]
    fn test_existing_note_needs_replace() {
        let t = set_up();
        t.save(args().body("first"));
        let path = t.agent_dir("scout").join("project-eval-probe.md");
        let before = fs::read(&path).unwrap();
        let msg = t.refused(None, args().body("second"));
        assert!(msg.contains("--replace"));
        assert!(msg.contains(".claude/agent-memory/anthology-scout/project-eval-probe.md"));
        assert_eq!(fs::read(&path).unwrap(), before);
        let out = t.save(args().body("second").replace(true));
        assert_eq!(out.action, "replaced");
        assert!(read(&path).ends_with("\n\nsecond\n"));
        assert!(
            !names(&t.agent_dir("scout"))
                .iter()
                .any(|n| n.starts_with(".tmp-"))
        );
    }

    #[test]
    fn test_replace_of_a_missing_note_creates_it() {
        let t = set_up();
        let out = t.save(args().replace(true));
        assert_eq!(out.action, "created");
        assert!(out.path().is_file());
    }

    #[test]
    fn test_root_must_be_a_directory() {
        let t = set_up();
        let e = Env::default();
        let missing = t.c.tmp.join("missing");
        assert!(
            save_note(
                &missing, "scout", "project", "x", "n", "d", "b", false, None, &e
            )
            .is_err()
        );
        let f = write(&t.c.tmp.join("afile"), "x", None);
        assert!(save_note(&f, "scout", "project", "x", "n", "d", "b", false, None, &e).is_err());
    }

    #[test]
    fn test_a_symlinked_root_is_resolved_and_paths_below_are_checked() {
        let t = set_up();
        let link = t.c.tmp.join("rootlink");
        symlink(&t.root, &link).unwrap();
        let e = Env::default();
        let out = save_note(
            &link,
            "scout",
            "project",
            "x",
            "n",
            "d",
            "b",
            false,
            Some(TODAY),
            &e,
        )
        .unwrap();
        assert!(t.agent_dir("scout").join("project-x.md").is_file());
        assert_eq!(out.path(), t.agent_dir("scout").join("project-x.md"));
    }

    #[test]
    fn test_a_symlinked_or_non_directory_component_is_refused() {
        let t = set_up();
        for row in py_rows("test_a_symlinked_or_non_directory_component_is_refused") {
            let label = row.str("component");
            let replace = row.param("replace").as_bool().expect("replace");
            let rel = match label {
                "claude" => ".claude",
                "memory" => ".claude/agent-memory",
                "agent" => ".claude/agent-memory/anthology-scout",
                other => panic!("{other}"),
            };
            let root = t.c.tmp.join(format!("r-{label}-{replace}"));
            fs::create_dir(&root).unwrap();
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let outside = t.outside.join(format!("o-{label}-{replace}"));
            fs::create_dir(&outside).unwrap();
            symlink(&outside, &path).unwrap();
            let msg = t.refused_in(&root, None, args().replace(replace));
            assert!(
                msg.contains("symlink or not a folder"),
                "{label} {replace}: {msg}"
            );
            assert!(names(&outside).is_empty());
        }
    }

    #[test]
    fn test_a_dangling_symlink_component_is_refused() {
        let t = set_up();
        fs::create_dir(t.root.join(".claude")).unwrap();
        symlink(t.outside.join("nope"), t.root.join(".claude/agent-memory")).unwrap();
        t.refused(Some("symlink or not a folder"), args());
        assert!(!t.outside.join("nope").exists());
    }

    #[test]
    fn test_a_file_where_a_folder_belongs_is_refused() {
        let t = set_up();
        write(&t.root.join(".claude"), "not a dir", None);
        t.refused(Some("symlink or not a folder"), args());
        assert_eq!(read(&t.root.join(".claude")), "not a dir");
    }

    #[test]
    fn test_a_symlinked_target_is_refused_with_and_without_replace() {
        let t = set_up();
        let victim = write(&t.outside.join("victim.txt"), "precious", None);
        fs::create_dir_all(t.agent_dir("scout")).unwrap();
        let target = t.agent_dir("scout").join("project-eval-probe.md");
        symlink(&victim, &target).unwrap();
        let rows = py_rows("test_a_symlinked_target_is_refused_with_and_without_replace");
        assert_eq!(rows.len(), 4);
        for row in &rows[..2] {
            t.refused(
                None,
                args().replace(row.param("replace").as_bool().unwrap()),
            );
            assert_eq!(read(&victim), "precious");
        }
        // a dangling link at the target is refused too, and nothing is created through it
        fs::remove_file(&target).unwrap();
        symlink(t.outside.join("new.txt"), &target).unwrap();
        for row in &rows[2..] {
            t.refused(
                None,
                args().replace(row.param("dangling").as_bool().unwrap()),
            );
            assert!(!t.outside.join("new.txt").exists());
        }
    }

    #[test]
    fn test_a_directory_at_the_target_is_refused() {
        let t = set_up();
        fs::create_dir_all(t.agent_dir("scout").join("project-eval-probe.md")).unwrap();
        t.refused(None, args().replace(true));
        t.refused(None, args().replace(false));
    }

    #[test]
    fn test_other_agents_dirs_are_untouched() {
        let t = set_up();
        t.save(args().agent("reviewer").body("theirs"));
        let before = snapshot(&t.agent_dir("reviewer"));
        t.save(args().agent("scout"));
        assert_eq!(snapshot(&t.agent_dir("reviewer")), before);
    }

    fn disk_says_no(
        _: &Path,
        _: anthology_memory::render::Budget,
        _: &[&str],
    ) -> Result<bool, PyExc> {
        Err(PyExc::Other("OSError", "disk says no".into()))
    }

    fn a_link(_: &Path, _: anthology_memory::render::Budget, _: &[&str]) -> Result<bool, PyExc> {
        Err(PyExc::Other("UnsafeTarget", "MEMORY.md is a link".into()))
    }

    #[test]
    fn test_a_failing_index_is_a_warning_not_a_failure() {
        let t = set_up();
        let deps = SaveDeps {
            regenerate_index: disk_says_no,
            ..SaveDeps::default()
        };
        let out = t.save_in(&t.root, args(), &deps).unwrap();
        assert!(out.path().is_file());
        assert!(out.index_warning.unwrap().contains("disk says no"));
        let deps = SaveDeps {
            regenerate_index: a_link,
            ..SaveDeps::default()
        };
        let out = t.save_in(&t.root, args().slug("two"), &deps).unwrap();
        assert!(out.index_warning.unwrap().contains("MEMORY.md is a link"));
    }
}

/// Rust-only cases (no Python id): the never-block rulings and the error branches of `run`.
mod ms2_rust_only {
    use super::*;
    use anthology_memory::sync::SweepResult;
    use anthology_pycompat::pyerr::PyExc;

    #[test]
    fn a_fifo_ledger_never_blocks_the_session() {
        let c = MemCase::new();
        let g = fake_repo(&c, "repo", None);
        approve_fake(&c, &g);
        let old = c
            .config
            .join("projects")
            .join(encode(&g.root))
            .join("memory");
        write(
            &old.join("project-a.md"),
            &Note::new("project-a").text(),
            None,
        );
        let ledger = c.anth.join("migrated.json");
        mkfifo(&ledger);
        let (env, root, key) = (c.env.clone(), g.root.clone(), g.key.clone());
        let out = run_with_timeout(10, move || {
            let fake = FakeGit {
                root: root.clone(),
                key,
            };
            sync::run(&root, &env, &deps(&fake))
        });
        if out.is_none() {
            unblock(&ledger);
        }
        let (ctx, _msgs) = out.expect("run blocked on a FIFO migrated.json");
        assert!(ctx.contains("project-a"));
    }

    #[test]
    fn a_fifo_hand_made_index_never_blocks_the_session() {
        let c = MemCase::new();
        let personal = c.anth.join("personal-memory");
        fs::create_dir_all(&personal).unwrap();
        let fifo = personal.join("MEMORY.md");
        mkfifo(&fifo);
        let (env, cwd) = (c.env.clone(), c.tmp.clone());
        let out = run_with_timeout(10, move || sync::run(&cwd, &env, &SyncDeps::default()));
        if out.is_none() {
            unblock(&fifo);
        }
        let out = out.expect("run blocked on a FIFO MEMORY.md");
        assert_eq!(out, (personal_only(&c.anth), vec![]));
    }

    fn boom(_: &Path, _: anthology_memory::render::Budget, _: &[&str]) -> Result<bool, PyExc> {
        Err(PyExc::Other("RuntimeError", "boom".into()))
    }

    #[test]
    fn an_error_outside_the_repo_step_is_one_message_and_no_context() {
        let c = MemCase::new();
        fs::create_dir_all(c.anth.join("personal-memory")).unwrap();
        let deps = SyncDeps {
            regenerate_index: boom,
            ..SyncDeps::default()
        };
        assert_eq!(
            sync::run(&c.tmp, &c.env, &deps),
            (String::new(), vec!["anthology memory: boom".to_string()])
        );
    }

    fn kaboom(_: &Path, _: &Path, _: Option<Date>, _: &SyncDeps<'_>) -> Result<SweepResult, PyExc> {
        panic!("kaboom")
    }

    #[test]
    fn a_panic_in_the_repo_step_keeps_the_personal_section() {
        let c = MemCase::new();
        let g = fake_repo(&c, "repo", None);
        approve_fake(&c, &g);
        write(
            &c.anth.join("personal-memory/user-me.md"),
            &note("user-me", "user", "Me"),
            None,
        );
        let deps = SyncDeps {
            git: &g,
            sweep_user_notes: kaboom,
            ..SyncDeps::default()
        };
        let (ctx, msgs) = sync::run(&g.root, &c.env, &deps);
        assert!(ctx.starts_with("## Personal memory ("));
        assert!(ctx.contains("[user-me](user-me.md)"));
        assert_eq!(msgs, ["anthology memory: repo memory step failed: kaboom"]);
    }
}

/// D-8 (Rust stricter, controller ruling): a symlinked hand-made MEMORY.md is never read into
/// the session context (Python's `read_text` follows it).
mod ms2_d8 {
    use super::*;

    #[test]
    fn a_symlinked_hand_made_index_is_not_read_into_the_context() {
        let c = MemCase::new();
        let personal = c.anth.join("personal-memory");
        fs::create_dir_all(&personal).unwrap();
        let outside = write(
            &c.tmp.join("outside.txt"),
            "private text outside memory\n",
            None,
        );
        symlink(&outside, personal.join("MEMORY.md")).unwrap();
        let (ctx, msgs) = run(&c, &c.tmp);
        assert_eq!((ctx, msgs), (personal_only(&c.anth), vec![]));
        // a regular hand-made index is still shown
        fs::remove_file(personal.join("MEMORY.md")).unwrap();
        write(&personal.join("MEMORY.md"), "hand-made\n", None);
        let (ctx, _) = run(&c, &c.tmp);
        assert!(ctx.ends_with("\n\nhand-made\n"), "{ctx}");
    }
}
