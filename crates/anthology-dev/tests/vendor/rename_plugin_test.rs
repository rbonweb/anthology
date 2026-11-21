//! test_vendor.RenamePluginTest.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_dev::vendor;
use anthology_pycompat::json::{self, Value};

use crate::fixture::{Fixture, jv, s, write};

const OLD: &str = "superpowers";
const NEW: &str = "anthology-superpowers";

fn make_tree(f: &Fixture) -> PathBuf {
    let tree = f.tmp.join("tree");
    let manifest = jv(
        r#"{"name": "superpowers", "description": "d", "version": "7.0.2",
        "homepage": "https://github.com/obra/superpowers", "license": "MIT",
        "author": {"name": "Jesse"}, "keywords": ["a", "b"]}"#,
    );
    write(
        &tree,
        ".claude-plugin/plugin.json",
        json::dumps(&manifest, &json::INDENT2) + "\n",
        false,
    );
    write(
        &tree,
        "skills/a/SKILL.md",
        [
            "Use superpowers:brainstorming first.",
            "Already anthology-superpowers:writing-plans stays.",
            "See https://github.com/obra/superpowers and github.com/obra/superpowers:main.",
            "Superpowers: prose is untouched, as is using-superpowers: here.",
            "Drop the `superpowers:` prefix (`superpowers:test-driven-development`).",
            "\"superpowers:code-reviewer\", superpowers:skill-name, superpowers:",
            "path/superpowers:x and Xsuperpowers:y and _superpowers:z stay.",
            "",
        ]
        .join("\n"),
        false,
    );
    write(
        &tree,
        "hooks/session-start",
        "echo 'superpowers:using-superpowers'\n",
        true,
    );
    write(
        &tree,
        "assets/blob.bin",
        b"\x00superpowers:brainstorming\x00",
        false,
    );
    tree
}

fn rename(tree: &Path) -> vendor::Result<json::Map> {
    vendor::rename_plugin(tree, OLD, NEW, &s("0.1.0"))
}

fn is_exec(p: &Path) -> bool {
    fs::metadata(p).unwrap().permissions().mode() & 0o111 != 0
}

#[test]
fn test_rewrites_only_skill_reference_tokens() {
    let f = Fixture::new();
    let tree = make_tree(&f);
    let stats = rename(&tree).unwrap();
    let text = fs::read_to_string(tree.join("skills/a/SKILL.md")).unwrap();
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        vec![
            "Use anthology-superpowers:brainstorming first.",
            "Already anthology-superpowers:writing-plans stays.",
            "See https://github.com/obra/superpowers and github.com/obra/superpowers:main.",
            "Superpowers: prose is untouched, as is using-superpowers: here.",
            "Drop the `superpowers:` prefix (`anthology-superpowers:test-driven-development`).",
            "\"anthology-superpowers:code-reviewer\", anthology-superpowers:skill-name, superpowers:",
            "path/superpowers:x and Xsuperpowers:y and _superpowers:z stay.",
        ]
    );
    assert_eq!(
        fs::read_to_string(tree.join("hooks/session-start")).unwrap(),
        "echo 'anthology-superpowers:using-superpowers'\n"
    );
    assert_eq!(
        fs::read(tree.join("assets/blob.bin")).unwrap(),
        b"\x00superpowers:brainstorming\x00"
    );
    assert_eq!(stats.get("references").and_then(Value::as_i64), Some(5));
    assert_eq!(stats.get("files").and_then(Value::as_i64), Some(2));
}

#[test]
fn test_manifest_name_and_version() {
    let f = Fixture::new();
    let tree = make_tree(&f);
    rename(&tree).unwrap();
    let raw = fs::read_to_string(tree.join(".claude-plugin/plugin.json")).unwrap();
    let manifest = json::loads(&raw).unwrap();
    let m = manifest.as_object().unwrap();
    assert_eq!(
        m.get("name").and_then(Value::as_str),
        Some("anthology-superpowers")
    );
    assert_eq!(m.get("version").and_then(Value::as_str), Some("0.1.0"));
    assert_eq!(m.get("license").and_then(Value::as_str), Some("MIT"));
    assert_eq!(
        m.get("homepage").and_then(Value::as_str),
        Some("https://github.com/obra/superpowers")
    );
    let keys: Vec<&str> = m.iter().map(|(k, _)| k).collect();
    assert_eq!(
        keys,
        [
            "name",
            "description",
            "version",
            "homepage",
            "license",
            "author",
            "keywords"
        ]
    );
    assert!(raw.ends_with("}\n"));
}

#[test]
fn test_preserves_file_modes() {
    let f = Fixture::new();
    let tree = make_tree(&f);
    rename(&tree).unwrap();
    assert!(is_exec(&tree.join("hooks/session-start")));
    assert!(!is_exec(&tree.join("skills/a/SKILL.md")));
}

#[test]
fn test_refuses_a_manifest_with_another_name() {
    let f = Fixture::new();
    let tree = make_tree(&f);
    rename(&tree).unwrap();
    assert!(rename(&tree).is_err());
}

#[test]
fn test_drops_the_upstream_dev_marketplace() {
    let f = Fixture::new();
    let tree = make_tree(&f);
    write(
        &tree,
        ".claude-plugin/marketplace.json",
        "{\"plugins\": [{\"version\": \"7.0.2\"}]}\n",
        false,
    );
    rename(&tree).unwrap();
    assert!(!tree.join(".claude-plugin/marketplace.json").exists());
    assert!(tree.join(".claude-plugin/plugin.json").is_file());
}

#[test]
fn test_missing_manifest_is_an_error() {
    let f = Fixture::new();
    let tree = f.tmp.join("empty");
    fs::create_dir(&tree).unwrap();
    assert!(rename(&tree).is_err());
}

// ---- parity details beyond the Python tests

#[test]
fn bytes_pattern_keeps_non_utf8_files_and_uses_ascii_word_characters() {
    // python3: re.compile(rb"(?<![\w./-])superpowers:(?=[a-z0-9-])").subn(b"anthology-superpowers:", raw)
    let f = Fixture::new();
    let tree = make_tree(&f);
    write(
        &tree,
        "latin.txt",
        b"\xe9superpowers:a \xffsuperpowers:b superpowers:\xe9 \xc3\xa9superpowers:c",
        false,
    );
    rename(&tree).unwrap();
    assert_eq!(
        fs::read(tree.join("latin.txt")).unwrap(),
        b"\xe9anthology-superpowers:a \xffanthology-superpowers:b superpowers:\xe9 \xc3\xa9anthology-superpowers:c"
    );
}

#[test]
fn replacement_is_a_python_template() {
    // re.sub treats `new` as a template: `\g<0>` is the match, `\\` a backslash.
    let f = Fixture::new();
    let tree = make_tree(&f);
    vendor::rename_plugin(&tree, OLD, r"x\g<0>\\", &s("0.1.0")).unwrap();
    let text = fs::read_to_string(tree.join("hooks/session-start")).unwrap();
    assert_eq!(text, "echo 'xsuperpowers:\\:using-superpowers'\n");
}
