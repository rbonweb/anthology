//! Rust tests for what setup adds around Claude Code's own auto memory (no Python original):
//!
//! - `plan_auto_memory_note` and `try_move_auto_memory`: where an old auto-memory note goes and
//!   under what name (`<type>-<slug>.md`), behind the privacy scan.
//! - `run` while a repo's committed `.claude/settings.json` says `autoMemoryEnabled: false`
//!   (setup writes it for an approved repo): Anthology's memory step keeps working and injects the
//!   repo memory index itself, because Claude Code no longer loads it.

mod ms2_support;

use std::fs;
use std::path::Path;

use anthology_memory::sync::{self, Destination, SyncDeps};
use anthology_testkit::mem::{MemCase, Note, needs_git, write};

use ms2_support::*;

const HINT: &str = "this repo has memory notes; repo notes load only after /anthology:setup, \
                    but notes in .claude/agent-memory still reach their agents, so review them";

fn note(name: &str, type_: &str) -> Vec<u8> {
    Note::new(name)
        .type_(type_)
        .desc("A note")
        .text()
        .into_bytes()
}

fn plan(file: &str, data: &[u8]) -> sync::AutoMemoryPlan {
    sync::plan_auto_memory_note(file, data, true, None, &SyncDeps::default())
}

mod plan_test {
    use super::*;

    #[test]
    fn each_type_has_its_destination() {
        for (type_, destination) in [
            ("project", Destination::Repo),
            ("feedback", Destination::Repo),
            ("reference", Destination::Repo),
            ("user", Destination::Personal),
            ("Feedback # a comment", Destination::Repo),
            ("other", Destination::Nowhere),
            ("", Destination::Nowhere),
        ] {
            let p = plan("note.md", &note("note", type_));
            assert_eq!(p.destination, destination, "{type_:?}");
            assert_eq!(p.target.is_some(), destination != Destination::Nowhere);
        }
    }

    #[test]
    fn a_file_without_frontmatter_has_no_type_and_goes_nowhere() {
        let p = plan("loose.md", b"Just text.\n");
        assert_eq!(p.destination, Destination::Nowhere);
        assert!(p.reason.unwrap().contains("no type"));
    }

    #[test]
    fn names_follow_type_dash_slug() {
        for (file, type_, target) in [
            ("feedback_testing.md", "feedback", "feedback-testing.md"),
            ("feedback-testing.md", "feedback", "feedback-testing.md"),
            ("Feedback Testing.md", "feedback", "feedback-testing.md"),
            ("topics.md", "project", "project-topics.md"),
            ("user_role.md", "user", "user-role.md"),
            ("role.md", "user", "user-role.md"),
            ("a__b..c.md", "reference", "reference-a-b-c.md"),
        ] {
            let p = plan(file, &note("n", type_));
            assert_eq!(p.target.as_deref(), Some(target), "{file}");
        }
    }

    #[test]
    fn a_name_that_is_only_the_type_falls_back_to_the_notes_name_then_to_note() {
        let p = plan("project.md", &note("Release plan", "project"));
        assert_eq!(p.target.as_deref(), Some("project-release-plan.md"));
        let p = plan("project.md", b"---\ntype: project\n---\nBody\n");
        assert_eq!(p.target.as_deref(), Some("project-note.md"));
        let p = plan(
            "\u{e9}\u{e9}.md",
            b"---\nname: \xc3\xa9\ntype: project\n---\nBody\n",
        );
        assert_eq!(p.target.as_deref(), Some("project-note.md"));
    }

    #[test]
    fn a_long_name_is_cut_to_the_slug_limit() {
        let long = format!("{}.md", "word-".repeat(40));
        let p = plan(&long, &note("n", "project"));
        let target = p.target.unwrap();
        let slug = target
            .strip_prefix("project-")
            .and_then(|t| t.strip_suffix(".md"))
            .unwrap();
        assert!(
            slug.chars().count() <= anthology_memory::save::MAX_SLUG,
            "{slug}"
        );
        assert!(!slug.ends_with('-'));
        assert!(slug.starts_with("word-word"));
    }

    #[test]
    fn the_privacy_scan_refuses_a_repo_bound_note_but_not_a_user_note() {
        let body = format!("Checkout in /{}\n", "Users/alice/src");
        let dirty = Note::new("dirty")
            .type_("feedback")
            .body(&body)
            .text()
            .into_bytes();
        let mut deps = SyncDeps::default();
        deps.privacy.hostname = || None;
        let p = sync::plan_auto_memory_note("dirty.md", &dirty, true, Some("/Users/alice"), &deps);
        assert_eq!(p.destination, Destination::Nowhere);
        assert!(p.target.is_none());
        assert!(p.reason.unwrap().contains("home path"));
        assert_eq!(p.findings[0].kind, "home-path");
        let user = Note::new("me")
            .type_("user")
            .body(&body)
            .text()
            .into_bytes();
        let p = sync::plan_auto_memory_note("me.md", &user, true, Some("/Users/alice"), &deps);
        assert_eq!(p.destination, Destination::Personal);
    }

    #[test]
    fn an_unusable_repo_folder_sends_a_repo_note_nowhere() {
        let p = sync::plan_auto_memory_note(
            "a.md",
            &note("a", "project"),
            false,
            None,
            &SyncDeps::default(),
        );
        assert_eq!(p.destination, Destination::Nowhere);
        assert!(p.reason.unwrap().contains("symlink"));
    }
}

mod move_test {
    use super::*;

    #[test]
    fn nothing_is_deleted_or_changed_in_the_old_folder() {
        let c = MemCase::new();
        let old = c.tmp.join("old");
        write(
            &old.join("project_a.md"),
            &String::from_utf8(note("a", "project")).unwrap(),
            None,
        );
        write(
            &old.join("user_b.md"),
            &String::from_utf8(note("b", "user")).unwrap(),
            None,
        );
        let before = anthology_testkit::mem::snapshot(&old);
        let repo_mem = c.tmp.join("repo/.claude/memory");
        let personal = c.anth.join("personal-memory");
        let res = sync::try_move_auto_memory(
            &old,
            Some(&repo_mem),
            &personal,
            Some(&c.anth.join("migrated.json")),
            None,
            None,
            &SyncDeps::default(),
        )
        .unwrap();
        assert_eq!(res.repo, ["project-a.md"]);
        assert_eq!(res.personal, ["user-b.md"]);
        assert!(res.blocked.is_empty());
        assert_eq!(anthology_testkit::mem::snapshot(&old), before);
    }

    #[test]
    fn without_a_repo_folder_repo_notes_are_blocked_not_sent_to_personal_memory() {
        let c = MemCase::new();
        let old = c.tmp.join("old");
        write(
            &old.join("project_a.md"),
            &String::from_utf8(note("a", "project")).unwrap(),
            None,
        );
        let personal = c.anth.join("personal-memory");
        let res = sync::try_move_auto_memory(
            &old,
            None,
            &personal,
            None,
            None,
            None,
            &SyncDeps::default(),
        )
        .unwrap();
        assert!(res.repo.is_empty() && res.personal.is_empty());
        assert_eq!(res.blocked.len(), 1);
        assert!(!personal.exists());
    }
}

/// The approved repo of a case, with `settings.json` as setup leaves it.
fn approved_repo_with_switch(c: &MemCase) -> std::path::PathBuf {
    let root = c.make_repo("repo", None);
    write(
        &root.join(".claude/settings.json"),
        "{\"autoMemoryEnabled\": false}",
        None,
    );
    write(
        &root.join(".claude/memory/project-a.md"),
        &Note::new("a").desc("Alpha note").text(),
        None,
    );
    approve(c, &root);
    root
}

mod run_with_the_switch_test {
    use super::*;

    #[test]
    fn the_memory_step_keeps_working_and_injects_the_repo_index_every_session() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = approved_repo_with_switch(&c);
        write(
            &c.anth.join("personal-memory/user-me.md"),
            &Note::new("user-me")
                .type_("user")
                .desc("Likes terse")
                .text(),
            None,
        );
        // the first session sets autoMemoryDirectory; the next ones have nothing to change,
        // and still get the repo index
        for session in 1..=3 {
            let (ctx, _) = run(&c, &root);
            assert!(ctx.contains("## Personal memory"), "{session}: {ctx}");
            assert!(ctx.contains("[user-me](user-me.md)"), "{session}: {ctx}");
            assert!(ctx.contains("## Repo memory"), "{session}: {ctx}");
            assert!(
                ctx.contains("[a](project-a.md) — Alpha note"),
                "{session}: {ctx}"
            );
        }
    }

    #[test]
    fn without_the_switch_the_repo_index_is_left_to_claude_code_after_the_first_session() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        write(
            &root.join(".claude/memory/project-a.md"),
            &Note::new("a").desc("Alpha note").text(),
            None,
        );
        approve(&c, &root);
        let (first, _) = run(&c, &root);
        assert!(first.contains("## Repo memory"));
        let (second, _) = run(&c, &root);
        assert!(!second.contains("## Repo memory"), "{second}");
    }

    #[test]
    fn an_opt_out_in_local_or_user_settings_or_the_environment_still_wins() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = approved_repo_with_switch(&c);
        write(
            &root.join(".claude/settings.local.json"),
            "{\"autoMemoryEnabled\": false}",
            None,
        );
        assert_eq!(run(&c, &root), (String::new(), vec![]));
        fs::remove_file(root.join(".claude/settings.local.json")).unwrap();
        write(
            &c.config.join("settings.json"),
            "{\"autoMemoryEnabled\": false}",
            None,
        );
        assert_eq!(run(&c, &root), (String::new(), vec![]));
        fs::remove_file(c.config.join("settings.json")).unwrap();
        let env = c.env.clone().with("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
        assert_eq!(
            sync::run(&root, &env, &SyncDeps::default()),
            (String::new(), vec![])
        );
    }

    #[test]
    fn a_clone_that_has_not_approved_the_repo_still_gets_the_hint() {
        if !needs_git() {
            return;
        }
        let c = MemCase::new();
        let root = c.make_repo("repo", None);
        write(
            &root.join(".claude/settings.json"),
            "{\"autoMemoryEnabled\": false}",
            None,
        );
        write(
            &root.join(".claude/memory/project-a.md"),
            &Note::new("a").text(),
            None,
        );
        let (ctx, msgs) = run(&c, &root);
        assert!(msgs.iter().any(|m| m == HINT), "{msgs:?}");
        assert!(ctx.contains("## Personal memory"));
        assert!(!ctx.contains("## Repo memory"));
        assert!(!Path::new(&root.join(".claude/settings.local.json")).exists());
    }
}
