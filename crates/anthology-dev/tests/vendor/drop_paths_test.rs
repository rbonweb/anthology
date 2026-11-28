//! The `drop-paths` transform: it leaves named upstream files and folders out of a vendored tree.

use std::fs;
use std::path::PathBuf;

use anthology_dev::vendor;
use anthology_pycompat::json::{self, Value};

use crate::fixture::{Fixture, MIT_TEXT, entry, jv, symlink, validate, write};

const NEEDS_PATHS: &str = "drop-paths needs a non-empty list of paths inside the source";

fn paths(list: &[&str]) -> Vec<String> {
    list.iter().map(|p| (*p).to_string()).collect()
}

fn stat(stats: &json::Map, k: &str) -> i64 {
    stats.get(k).and_then(Value::as_i64).unwrap()
}

/// A tree of `files` under `<tmp>/tree`.
fn tree_of(f: &Fixture, files: &[&str]) -> PathBuf {
    let tree = f.tmp.join("tree");
    for rel in files {
        write(&tree, rel, "x\n", false);
    }
    tree
}

#[test]
fn test_drops_a_file_and_a_folder() {
    let f = Fixture::new();
    let tree = tree_of(
        &f,
        &[
            "evals/evals.json",
            "references/a.md",
            "references/b.md",
            "SKILL.md",
        ],
    );
    let stats = vendor::drop_paths(&tree, &paths(&["evals", "references/a.md"])).unwrap();
    assert!(!tree.join("evals").exists());
    assert!(!tree.join("references/a.md").exists());
    assert!(tree.join("references/b.md").is_file());
    assert!(tree.join("SKILL.md").is_file());
    assert_eq!(stats.len(), 1);
    assert_eq!(stat(&stats, "dropped"), 2);
}

#[test]
fn test_a_missing_path_fails_with_its_name() {
    let f = Fixture::new();
    let tree = tree_of(&f, &["SKILL.md"]);
    let err = vendor::drop_paths(&tree, &paths(&["nope"])).unwrap_err();
    assert!(
        err.0.contains("drop-paths: 'nope' is not in the source"),
        "{}",
        err.0
    );
}

#[test]
fn test_a_path_below_a_file_is_not_in_the_source() {
    let f = Fixture::new();
    let tree = tree_of(&f, &["SKILL.md"]);
    let err = vendor::drop_paths(&tree, &paths(&["SKILL.md/x"])).unwrap_err();
    assert!(
        err.0
            .contains("drop-paths: 'SKILL.md/x' is not in the source"),
        "{}",
        err.0
    );
}

#[test]
fn test_does_not_follow_a_linked_parent() {
    let f = Fixture::new();
    let tree = tree_of(&f, &["SKILL.md"]);
    let outside = f.tmp.join("outside");
    write(&outside, "x", "keep\n", false);
    symlink(&outside, &tree.join("link"));
    let err = vendor::drop_paths(&tree, &paths(&["link/x"])).unwrap_err();
    assert!(err.0.contains("goes through a link"), "{}", err.0);
    assert!(outside.join("x").is_file());
}

#[test]
fn test_removes_a_linked_leaf_without_touching_its_target() {
    let f = Fixture::new();
    let tree = tree_of(&f, &["SKILL.md"]);
    let outside = f.tmp.join("outside");
    write(&outside, "x", "keep\n", false);
    symlink(&outside, &tree.join("link"));
    let stats = vendor::drop_paths(&tree, &paths(&["link"])).unwrap();
    assert!(fs::symlink_metadata(tree.join("link")).is_err());
    assert!(outside.join("x").is_file());
    assert_eq!(stat(&stats, "dropped"), 1);
}

#[test]
fn test_validate_rejects_bad_paths() {
    for bad in [r"[]", r#"["../x"]"#, r#"["/abs"]"#, r#"[""]"#, r#""evals""#] {
        let t = jv(&format!(r#"[{{"name": "drop-paths", "paths": {bad}}}]"#));
        let errors = validate(&[entry("x", &[("transform", t)])]);
        assert!(
            errors.iter().any(|e| e.contains(NEEDS_PATHS)),
            "{bad}: {errors:?}"
        );
    }
    let missing = jv(r#"[{"name": "drop-paths"}]"#);
    let errors = validate(&[entry("x", &[("transform", missing)])]);
    assert!(errors.iter().any(|e| e.contains(NEEDS_PATHS)), "{errors:?}");
    let good = jv(r#"[{"name": "drop-paths", "paths": ["evals", "references/a.md"]}]"#);
    assert_eq!(
        validate(&[entry("x", &[("transform", good)])]),
        Vec::<String>::new()
    );
}

#[test]
fn test_pipeline_applies_it_from_sources_json() {
    let f = Fixture::new();
    let e = f.add_upstream(
        "dropped",
        &[
            ("LICENSE", MIT_TEXT),
            ("SKILL.md", "# T\n"),
            ("evals/evals.json", "{}\n"),
        ],
        &[(
            "transform",
            jv(r#"[{"name": "drop-paths", "paths": ["evals"]}]"#),
        )],
    );
    f.build_all(&[e]);
    assert_eq!(f.check(), Vec::<String>::new());
    assert!(!f.target("dropped", "evals").exists());
    assert!(f.target("dropped", "SKILL.md").is_file());
}
