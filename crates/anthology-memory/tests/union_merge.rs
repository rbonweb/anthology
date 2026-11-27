//! The generated `MEMORY.md` indexes merge without a conflict once the repo carries Anthology's
//! `merge=union` rules. Each test builds a throw-away git repo, lets two "agents" on two branches
//! save one note each (the real `save_note`, so the real index file), and merges the branches
//! with the real git.
//!
//! Without the rule both branches add a line at the same place and git stops with a conflict,
//! including the add/add case where the index did not exist on the base. With the rule git keeps
//! both lines.

use std::fs;
use std::path::{Path, PathBuf};

use anthology_core::git::RealGit;
use anthology_memory::approve::{UNION_LINES, approve, ensure_union_attrs};
use anthology_memory::save::save_note;
use anthology_testkit::mem::{MemCase, needs_git, try_git};
use anthology_testkit::run::Output;

const AGENT_INDEX: &str = ".claude/agent-memory/anthology-implementer/MEMORY.md";
const REPO_INDEX: &str = ".claude/memory/MEMORY.md";

/// How the rule gets into the repo.
#[derive(Clone, Copy, PartialEq)]
enum Rule {
    /// Not at all: the situation before the fix.
    None,
    /// Added and committed on the base commit, as a user who commits setup's change has it.
    Committed,
    /// In the merging checkout's working tree only, never committed.
    WorkingTreeOnly,
}

fn save(case: &MemCase, root: &Path, slug: &str) {
    save_note(
        root,
        "implementer",
        "project",
        slug,
        &format!("Note {slug}"),
        &format!("About {slug}"),
        "Body.",
        false,
        None,
        &case.env,
    )
    .unwrap_or_else(|e| panic!("save {slug}: {e:?}"));
}

/// Two branches off one base each save one agent note; then `one` merges `two`. `base_note`: the
/// base already holds a note, so the index exists and both sides modify it (otherwise both add
/// it). Returns the repo and git's answer to the merge.
fn merge_two_agents(case: &MemCase, rule: Rule, base_note: bool) -> (PathBuf, Output) {
    let root = case.make_repo("repo", None);
    case.git(&root, &["checkout", "-q", "-b", "base"]);
    fs::write(root.join("README.md"), "app\n").unwrap();
    if rule == Rule::Committed {
        assert!(ensure_union_attrs(&root).expect("attrs"));
    }
    if base_note {
        save(case, &root, "base-note");
    }
    case.commit_all(&root, "base");
    case.git(&root, &["checkout", "-q", "-b", "one"]);
    save(case, &root, "note-one");
    case.commit_all(&root, "one");
    case.git(&root, &["checkout", "-q", "base"]);
    case.git(&root, &["checkout", "-q", "-b", "two"]);
    save(case, &root, "note-two");
    case.commit_all(&root, "two");
    case.git(&root, &["checkout", "-q", "one"]);
    if rule == Rule::WorkingTreeOnly {
        assert!(ensure_union_attrs(&root).expect("attrs"));
    }
    let merged = try_git(&case.env, &root, &["merge", "--no-edit", "two"]);
    (root, merged)
}

fn assert_conflict(merged: &Output) {
    let text = format!("{}{}", merged.stdout_text(), merged.stderr_text());
    assert_ne!(merged.code, 0, "{text}");
    assert!(
        text.contains("CONFLICT") && text.contains("MEMORY.md"),
        "{text}"
    );
}

fn assert_both_lines_kept(root: &Path, merged: &Output) {
    let text = format!("{}{}", merged.stdout_text(), merged.stderr_text());
    assert_eq!(merged.code, 0, "{text}");
    let index = fs::read_to_string(root.join(AGENT_INDEX)).unwrap();
    assert!(
        !index.contains("<<<<<<<") && !index.contains(">>>>>>>"),
        "{index}"
    );
    for slug in ["note-one", "note-two"] {
        assert_eq!(
            index.matches(&format!("(project-{slug}.md)")).count(),
            1,
            "{slug} once in\n{index}"
        );
    }
}

#[test]
fn without_the_rule_two_agents_adding_the_index_conflict_add_add() {
    if !needs_git() {
        return;
    }
    let case = MemCase::new();
    let (_, merged) = merge_two_agents(&case, Rule::None, false);
    assert_conflict(&merged);
}

#[test]
fn without_the_rule_two_agents_adding_a_line_conflict_modify_modify() {
    if !needs_git() {
        return;
    }
    let case = MemCase::new();
    let (_, merged) = merge_two_agents(&case, Rule::None, true);
    assert_conflict(&merged);
}

#[test]
fn with_the_rule_committed_two_agents_adding_the_index_merge_add_add() {
    if !needs_git() {
        return;
    }
    let case = MemCase::new();
    let (root, merged) = merge_two_agents(&case, Rule::Committed, false);
    assert_both_lines_kept(&root, &merged);
}

#[test]
fn with_the_rule_committed_two_agents_adding_a_line_merge_modify_modify() {
    if !needs_git() {
        return;
    }
    let case = MemCase::new();
    let (root, merged) = merge_two_agents(&case, Rule::Committed, true);
    assert_both_lines_kept(&root, &merged);
    let index = fs::read_to_string(root.join(AGENT_INDEX)).unwrap();
    assert_eq!(
        index.matches("(project-base-note.md)").count(),
        1,
        "{index}"
    );
}

#[test]
fn a_rule_only_in_the_merging_working_tree_is_enough() {
    if !needs_git() {
        return;
    }
    let case = MemCase::new();
    let (root, merged) = merge_two_agents(&case, Rule::WorkingTreeOnly, true);
    assert_both_lines_kept(&root, &merged);
}

mod plan {
    //! `plan_union_attrs` says what `ensure_union_attrs` would add, and writes nothing.
    use super::*;
    use anthology_memory::approve::plan_union_attrs;

    fn dir(case: &MemCase) -> PathBuf {
        let root = case.tmp.join("plain");
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn a_missing_file_needs_both_lines_and_stays_missing() {
        let case = MemCase::new();
        let root = dir(&case);
        let plan = plan_union_attrs(&root).expect("plan");
        assert_eq!(plan.path, root.join(".gitattributes"));
        assert_eq!(plan.missing, UNION_LINES);
        assert_eq!(
            plan.updated(),
            format!("{}\n{}\n", UNION_LINES[0], UNION_LINES[1])
        );
        assert!(!root.join(".gitattributes").exists());
    }

    #[test]
    fn the_updated_text_is_what_ensure_writes() {
        let case = MemCase::new();
        let root = dir(&case);
        fs::write(root.join(".gitattributes"), "*.png binary\n# keep   me").unwrap();
        let plan = plan_union_attrs(&root).expect("plan");
        let want = plan.updated();
        assert!(ensure_union_attrs(&root).expect("attrs"));
        assert_eq!(
            fs::read_to_string(root.join(".gitattributes")).unwrap(),
            want
        );
        assert!(want.starts_with("*.png binary\n# keep   me\n"), "{want}");
    }

    #[test]
    fn nothing_is_missing_when_the_rules_are_there_in_any_spacing() {
        let case = MemCase::new();
        let root = dir(&case);
        fs::write(
            root.join(".gitattributes"),
            ".claude/memory/MEMORY.md   merge=union\n.claude/agent-memory/*/MEMORY.md\tmerge=union\n",
        )
        .unwrap();
        let plan = plan_union_attrs(&root).expect("plan");
        assert!(plan.missing.is_empty());
        assert_eq!(plan.updated(), plan.existing);
    }

    #[test]
    fn a_symlinked_file_is_refused() {
        let case = MemCase::new();
        let root = dir(&case);
        fs::write(root.join("elsewhere"), "x").unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join(".gitattributes")).unwrap();
        assert!(plan_union_attrs(&root).is_err());
    }
}

#[test]
fn approve_writes_rules_that_git_matches_to_both_index_paths() {
    if !needs_git() {
        return;
    }
    let case = MemCase::new();
    let root = case.make_repo("repo", None);
    approve(&root, &case.env, &RealGit).expect("approve");
    assert_eq!(UNION_LINES.len(), 2);
    for path in [REPO_INDEX, AGENT_INDEX] {
        let out = case.git(&root, &["check-attr", "merge", "--", path]);
        assert!(
            out.stdout_text().trim_end().ends_with("merge: union"),
            "{path}: {}",
            out.stdout_text()
        );
    }
}
