//! test_vendor.PipelineTest.

use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use anthology_dev::vendor::{self, Entry};
use anthology_pycompat::json::{self, Value};

use crate::fixture::{Fixture, MIT_TEXT, is_exec, jv, s, symlink, tree_files, with, write};

struct Case {
    f: Fixture,
    plain: Value,
    entries: Vec<Value>,
}

fn setup() -> Case {
    let f = Fixture::new();
    let plain = f.add_upstream(
        "plain",
        &[
            ("LICENSE", MIT_TEXT),
            ("skills/a/SKILL.md", "---\nname: a\n---\nbody\n"),
            ("scripts/run.sh", "#!/bin/sh\necho hi\n"),
            ("README.md", "readme\n"),
        ],
        &[],
    );
    let sub = f.add_upstream(
        "sub",
        &[
            ("LICENSE.txt", "Apache License\n"),
            ("skills/pick/SKILL.md", "pick\n"),
            ("skills/pick/LICENSE.txt", "Apache License\n"),
            ("skills/other/SKILL.md", "other\n"),
        ],
        &[
            ("path", s("skills/pick")),
            ("license", s("Apache-2.0")),
            ("license_file", s("skills/pick/LICENSE.txt")),
        ],
    );
    let manifest = json::dumps(
        &jv(r#"{"name": "old", "version": "9.9.9"}"#),
        &json::INDENT2,
    ) + "\n";
    let renamed = f.add_upstream(
        "ren",
        &[
            (".claude-plugin/plugin.json", &manifest),
            ("skills/x/SKILL.md", "Use old:thing\n"),
            ("LICENSE", MIT_TEXT),
        ],
        &[
            (
                "transform",
                jv(r#"[{"name": "rename-plugin", "old": "old", "new": "new"}]"#),
            ),
            ("plugin_version", s("0.1.0")),
            ("upstream_version", s("9.9.9")),
        ],
    );
    let entries = vec![plain.clone(), sub, renamed];
    Case { f, plain, entries }
}

fn sorted_files(dir: &std::path::Path) -> Vec<String> {
    tree_files(dir).into_keys().collect()
}

#[test]
fn test_build_copies_whole_repo_without_git_dir() {
    let c = setup();
    c.f.build_all(&c.entries);
    let target = c.f.target("plain", "");
    assert_eq!(
        sorted_files(&target),
        [
            "LICENSE",
            "README.md",
            "scripts/run.sh",
            "skills/a/SKILL.md"
        ]
    );
    assert!(!target.join(".git").exists());
}

#[test]
fn test_build_preserves_exec_bit() {
    let c = setup();
    c.f.build_all(&c.entries);
    assert!(is_exec(&c.f.target("plain", "scripts/run.sh")));
    assert!(!is_exec(&c.f.target("plain", "README.md")));
}

#[test]
fn test_build_uses_only_the_pinned_subpath() {
    let c = setup();
    c.f.build_all(&c.entries);
    assert_eq!(
        sorted_files(&c.f.target("sub", "")),
        ["LICENSE.txt", "SKILL.md"]
    );
}

#[test]
fn test_build_applies_rename() {
    let c = setup();
    c.f.build_all(&c.entries);
    let target = c.f.target("ren", "");
    assert_eq!(
        fs::read_to_string(target.join("skills/x/SKILL.md")).unwrap(),
        "Use new:thing\n"
    );
    let manifest = jv(&fs::read_to_string(target.join(".claude-plugin/plugin.json")).unwrap());
    let m = manifest.as_object().unwrap();
    assert_eq!(m.get("name").and_then(Value::as_str), Some("new"));
    assert_eq!(m.get("version").and_then(Value::as_str), Some("0.1.0"));
}

#[test]
fn test_build_replaces_existing_target() {
    let c = setup();
    c.f.build_all(&c.entries);
    write(
        &c.f.root,
        "plugins/anthology/knowledge/plain/stale.txt",
        "stale",
        false,
    );
    c.f.build("plain").unwrap();
    assert!(!c.f.target("plain", "stale.txt").exists());
}

#[test]
fn test_build_out_dir_leaves_the_repo_alone() {
    let c = setup();
    c.f.set_sources(&c.entries);
    let out = c.f.tmp.join("out");
    vendor::build(
        &c.f.root,
        "plain",
        Some(&out),
        Some(&|e: &Entry| c.f.fetch(e)),
        false,
    )
    .unwrap();
    assert!(
        out.join("plugins/anthology/knowledge/plain/README.md")
            .is_file()
    );
    assert!(!c.f.root.join("plugins").exists());
}

#[test]
fn test_unknown_name() {
    let c = setup();
    c.f.set_sources(&c.entries);
    assert!(c.f.build("nope").is_err());
}

#[test]
fn test_check_passes_when_clean() {
    let c = setup();
    c.f.build_all(&c.entries);
    assert_eq!(c.f.check(), Vec::<String>::new());
}

#[test]
fn test_check_flags_modified_file() {
    let c = setup();
    c.f.build_all(&c.entries);
    write(
        &c.f.root,
        "plugins/anthology/knowledge/plain/README.md",
        "edited\n",
        false,
    );
    let problems = c.f.check();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("README.md") && p.contains("differs")),
        "{problems:?}"
    );
}

#[test]
fn test_check_flags_extra_and_missing_files() {
    let c = setup();
    c.f.build_all(&c.entries);
    write(
        &c.f.root,
        "plugins/anthology/knowledge/plain/extra.md",
        "x",
        false,
    );
    fs::remove_file(c.f.target("plain", "README.md")).unwrap();
    let problems = c.f.check().join("\n");
    assert!(problems.contains("extra.md"));
    assert!(problems.contains("README.md"));
}

#[test]
fn test_check_flags_lost_exec_bit() {
    let c = setup();
    c.f.build_all(&c.entries);
    let run = c.f.target("plain", "scripts/run.sh");
    fs::set_permissions(&run, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        c.f.check()
            .iter()
            .any(|p| p.contains("run.sh") && p.contains("mode"))
    );
}

#[test]
fn test_check_flags_missing_target() {
    let c = setup();
    c.f.build_all(&c.entries);
    fs::remove_dir_all(c.f.target("plain", "")).unwrap();
    assert!(c.f.check().iter().any(|p| p.contains("plain")));
}

#[test]
fn test_directory_symlinks_fail_loudly_in_pristine_trees() {
    let c = setup();
    symlink("skills", &c.f.upstream("plain").join("linkdir"));
    c.f.set_sources(std::slice::from_ref(&c.plain));
    let err = c.f.build("plain").unwrap_err();
    assert!(err.0.contains("linkdir"), "{}", err.0);
    assert!(err.0.contains("symlinks to directories"), "{}", err.0);
    assert!(!c.f.target("plain", "").exists());
    assert!(c.f.check().iter().any(|p| p.contains("linkdir")));
}

#[test]
fn test_check_flags_a_directory_symlink_in_the_committed_tree() {
    let c = setup();
    c.f.build_all(&c.entries);
    symlink("skills", &c.f.target("plain", "linkdir"));
    let problems = c.f.check();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("linkdir") && p.contains("symlinks to directories")),
        "{problems:?}"
    );
}

#[test]
fn test_file_symlinks_are_copied_and_compared() {
    let c = setup();
    symlink("README.md", &c.f.upstream("plain").join("link.md"));
    c.f.build_all(&c.entries);
    let link = c.f.target("plain", "link.md");
    assert_eq!(fs::read_link(&link).unwrap().to_str(), Some("README.md"));
    assert_eq!(c.f.check(), Vec::<String>::new());
    fs::remove_file(&link).unwrap();
    symlink("LICENSE", &link);
    assert!(c.f.check().iter().any(|p| p.contains("link.md")));
}

#[test]
fn test_check_ignores_ds_store_but_not_other_extras() {
    let c = setup();
    c.f.build_all(&c.entries);
    write(
        &c.f.root,
        "plugins/anthology/knowledge/plain/.DS_Store",
        "x",
        false,
    );
    assert_eq!(c.f.check(), Vec::<String>::new());
}

#[test]
fn test_check_flags_upstream_change_at_same_pin() {
    let c = setup();
    c.f.build_all(&c.entries);
    write(
        &c.f.upstream("plain"),
        "README.md",
        "upstream moved\n",
        false,
    );
    assert!(!c.f.check().is_empty());
}

fn all_paths(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in fs::read_dir(&dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() && !p.is_symlink() {
                stack.push(p.clone());
            }
            out.push(p.display().to_string());
        }
    }
    out.sort();
    out
}

#[test]
fn test_check_does_not_touch_the_repo() {
    let c = setup();
    c.f.build_all(&c.entries);
    let before = all_paths(&c.f.root);
    c.f.check();
    assert_eq!(before, all_paths(&c.f.root));
}

#[test]
fn test_check_validates_sources_first() {
    let c = setup();
    let bad = with(&c.plain, &[("license", s("GPL-3.0-only"))]);
    c.f.set_sources(&[bad]);
    assert!(c.f.check().iter().any(|p| p.contains("license")));
}

#[test]
fn test_check_skips_subtree_entries() {
    let c = setup();
    let calls = RefCell::new(Vec::<String>::new());
    let fetch = |e: &Entry| {
        calls.borrow_mut().push(
            e.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
        c.f.fetch(e)
    };
    let subtree =
        jv(r#"{"name": "atl", "managed_by": "subtree", "license": "Apache-2.0", "target": "atl"}"#);
    c.f.build_all(&c.entries);
    let mut all = c.entries.clone();
    all.push(subtree);
    c.f.set_sources(&all);
    assert_eq!(
        vendor::check(&c.f.root, Some(&fetch)).unwrap(),
        Vec::<String>::new()
    );
    assert!(!calls.borrow().iter().any(|n| n == "atl"));
    vendor::build(&c.f.root, "plain", None, Some(&fetch), false).unwrap(); // others still build
    assert!(vendor::build(&c.f.root, "atl", None, Some(&fetch), false).is_err());
}
