//! test_vendor.RenameSkillTest.

use std::fs;
use std::path::PathBuf;

use anthology_dev::vendor;

use crate::fixture::{Fixture, write};

fn make(f: &Fixture, text: &str) -> PathBuf {
    let tree = f.tmp.join("tree");
    write(&tree, "SKILL.md", text, false);
    tree
}

fn read(tree: &std::path::Path) -> String {
    fs::read_to_string(tree.join("SKILL.md")).unwrap()
}

#[test]
fn test_renames_the_frontmatter_name_and_the_slash_references() {
    let f = Fixture::new();
    let tree = make(
        &f,
        concat!(
            "---\nname: code-review\ndescription: Use /code-review here.\n---\n\n",
            "# /code-review\n\n```\n/code-review <PR URL>\n```\n\n",
            "Also /code-review-checklist, a/code-review and code-review stay.\n"
        ),
    );
    vendor::rename_skill(&tree, "code-review", "code-review-checklist").unwrap();
    assert_eq!(
        read(&tree),
        concat!(
            "---\nname: code-review-checklist\ndescription: Use /code-review here.\n---\n\n",
            "# code-review-checklist\n\n```\ncode-review-checklist <PR URL>\n```\n\n",
            "Also /code-review-checklist, a/code-review and code-review stay.\n"
        )
    );
}

#[test]
fn test_rename_is_idempotent_on_the_body() {
    let f = Fixture::new();
    let tree = make(&f, "---\nname: code-review\n---\n\n# /code-review\n");
    vendor::rename_skill(&tree, "code-review", "code-review-checklist").unwrap();
    let once = fs::read(tree.join("SKILL.md")).unwrap();
    let back = String::from_utf8(once.clone())
        .unwrap()
        .replace("name: code-review-checklist", "name: code-review");
    fs::write(tree.join("SKILL.md"), back).unwrap();
    vendor::rename_skill(&tree, "code-review", "code-review-checklist").unwrap();
    assert_eq!(fs::read(tree.join("SKILL.md")).unwrap(), once);
}

#[test]
fn test_a_different_name_is_an_error() {
    let f = Fixture::new();
    let tree = make(&f, "---\nname: other\n---\nbody\n");
    assert!(vendor::rename_skill(&tree, "code-review", "x").is_err());
}

#[test]
fn test_frontmatter_without_a_name_is_an_error() {
    let f = Fixture::new();
    let tree = make(&f, "---\ndescription: x\n---\n\n/a\n");
    assert!(vendor::rename_skill(&tree, "a", "b").is_err());
}

#[test]
fn test_missing_skill_file_or_frontmatter_is_an_error() {
    let f = Fixture::new();
    assert!(vendor::rename_skill(&f.tmp.join("nowhere"), "a", "b").is_err());
    let tree = make(&f, "no frontmatter\n");
    assert!(vendor::rename_skill(&tree, "a", "b").is_err());
}

#[test]
fn test_is_not_confused_by_a_name_line_in_the_body() {
    let f = Fixture::new();
    let tree = make(&f, "---\nname: a\n---\n\nname: a\n");
    vendor::rename_skill(&tree, "a", "b").unwrap();
    assert_eq!(read(&tree), "---\nname: b\n---\n\nname: a\n");
}

// ---- parity details beyond the Python tests

#[test]
fn read_text_newlines_quotes_and_unicode_word_characters() {
    // Path.read_text translates \r\n and \r; the name may be quoted; \w is Unicode.
    let f = Fixture::new();
    let tree = make(&f, "---\r\nname: 'a'  \r\n---\r\n/a é/a x/a_ /a-b /a.\r");
    let stats = vendor::rename_skill(&tree, "a", "b").unwrap();
    assert_eq!(read(&tree), "---\nname: b\n---\nb é/a x/a_ /a-b b.\n");
    assert_eq!(stats.get("references").and_then(|v| v.as_i64()), Some(2));
    let err = vendor::rename_skill(&tree, "c", "d").unwrap_err();
    assert_eq!(err.0, "rename-skill: the skill is named 'b', expected 'c'");
}
