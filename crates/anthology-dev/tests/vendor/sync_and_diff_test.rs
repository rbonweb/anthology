//! test_vendor.SyncAndDiffTest.

use std::fs;

use anthology_dev::vendor::{self, Entry};
use anthology_pycompat::json::{self, Value};

use crate::fixture::{Fixture, MIT_TEXT, SHA_B, have_git, jv, tree_files, write};

fn setup() -> Fixture {
    let f = Fixture::new();
    let e = f.add_upstream(
        "p",
        &[("LICENSE", MIT_TEXT), ("skills/a/SKILL.md", "v1\n")],
        &[],
    );
    f.build_all(&[e]);
    f
}

fn resolve(_: &Entry, git_ref: &str) -> vendor::Result<String> {
    assert_eq!(git_ref, "v2");
    Ok(SHA_B.to_string())
}

fn pinned(f: &Fixture) -> (String, String) {
    let sources = vendor::load_sources(&f.root).unwrap();
    let e = sources[0].as_object().unwrap();
    let get = |k: &str| e.get(k).and_then(Value::as_str).unwrap().to_string();
    (get("ref"), get("commit"))
}

fn knowledge(f: &Fixture) -> std::path::PathBuf {
    f.root.join("plugins/anthology/knowledge/p")
}

/// Rewrite sources.json with the first entry's transform set to the p.patch patch.
fn use_patch(f: &Fixture) {
    let path = f.root.join("vendor/sources.json");
    let mut sources = jv(&fs::read_to_string(&path).unwrap());
    let list = sources.as_object_mut().unwrap().get_mut("sources").unwrap();
    let first = list.as_array_mut().unwrap()[0].as_object_mut().unwrap();
    first.insert(
        "transform".into(),
        jv(r#"[{"patch": "vendor/patches/p.patch"}]"#),
    );
    fs::write(&path, json::dumps(&sources, &json::INDENT2) + "\n").unwrap();
}

fn write_patch(f: &Fixture) {
    let patch = f.root.join("vendor/patches/p.patch");
    fs::create_dir(patch.parent().unwrap()).unwrap();
    fs::write(&patch, f.diff("p").unwrap()).unwrap();
}

#[test]
fn test_sync_repins_rebuilds_and_never_commits() {
    let f = setup();
    write(&f.upstream("p"), "skills/a/SKILL.md", "v2\n", false);
    let report = f.sync("p", "v2", &resolve).unwrap();
    assert_eq!(pinned(&f), ("v2".to_string(), SHA_B.to_string()));
    assert_eq!(
        fs::read_to_string(knowledge(&f).join("skills/a/SKILL.md")).unwrap(),
        "v2\n"
    );
    assert_eq!(report.rejects, Vec::<String>::new());
    assert_eq!(f.check(), Vec::<String>::new());
    assert!(!f.root.join(".git").exists());
}

#[test]
fn test_sync_keeps_other_fields_and_order() {
    let f = setup();
    let text = fs::read_to_string(f.root.join("vendor/sources.json")).unwrap();
    let top = jv(&text);
    let before = top
        .as_object()
        .unwrap()
        .get("sources")
        .unwrap()
        .as_array()
        .unwrap()[0]
        .as_object()
        .unwrap()
        .clone();
    f.sync("p", "v2", &resolve).unwrap();
    let sources = vendor::load_sources(&f.root).unwrap();
    let after = sources[0].as_object().unwrap();
    let keys = |m: &json::Map| m.iter().map(|(k, _)| k.to_string()).collect::<Vec<_>>();
    assert_eq!(keys(&before), keys(after));
    for (k, v) in before.iter() {
        if k != "ref" && k != "commit" {
            assert_eq!(Some(v), after.get(k), "{k}");
        }
    }
}

#[test]
fn test_sync_accepts_a_full_sha_without_resolving() {
    let f = setup();
    let boom =
        |_: &Entry, _: &str| -> vendor::Result<String> { panic!("must not resolve a 40-hex ref") };
    f.sync("p", SHA_B, &boom).unwrap();
    assert_eq!(pinned(&f), (SHA_B.to_string(), SHA_B.to_string()));
}

#[test]
fn test_sync_refreshes_the_license() {
    let f = setup();
    write(
        &f.upstream("p"),
        "LICENSE",
        "MIT License\n\nCopyright (c) New\n",
        false,
    );
    f.sync("p", "v2", &resolve).unwrap();
    assert!(
        fs::read_to_string(f.root.join("vendor/licenses/acme-p.txt"))
            .unwrap()
            .contains("New")
    );
}

#[test]
fn test_diff_reports_nothing_for_a_pristine_copy() {
    let f = setup();
    assert_eq!(f.diff("p").unwrap(), "");
}

#[test]
fn test_diff_shows_shipped_changes_as_a_patch() {
    let f = setup();
    write(
        &f.root,
        "plugins/anthology/knowledge/p/skills/a/SKILL.md",
        "v1\nlocal edit\n",
        false,
    );
    write(
        &f.root,
        "plugins/anthology/knowledge/p/new.md",
        "new\n",
        false,
    );
    let text = f.diff("p").unwrap();
    for want in [
        "--- a/skills/a/SKILL.md",
        "+++ b/skills/a/SKILL.md",
        "+local edit",
        "--- /dev/null",
        "+++ b/new.md",
    ] {
        assert!(text.contains(want), "{want} not in {text}");
    }
}

#[test]
fn test_diff_output_round_trips_through_a_patch() {
    if !have_git() {
        return;
    }
    let f = setup();
    write(&f.upstream("p"), "old.md", "old\n", false);
    f.build("p").unwrap();
    write(
        &f.root,
        "plugins/anthology/knowledge/p/skills/a/SKILL.md",
        "v1\nlocal edit\n",
        false,
    );
    write(
        &f.root,
        "plugins/anthology/knowledge/p/new.md",
        "new\n",
        false,
    );
    fs::remove_file(knowledge(&f).join("old.md")).unwrap();
    write_patch(&f);
    let shipped = tree_files(&knowledge(&f));
    use_patch(&f);
    f.build("p").unwrap();
    assert_eq!(shipped, tree_files(&knowledge(&f)));
    assert_eq!(f.check(), Vec::<String>::new());
}

#[test]
fn test_sync_reports_patch_rejects() {
    if !have_git() {
        return;
    }
    let f = setup();
    write(
        &f.root,
        "plugins/anthology/knowledge/p/skills/a/SKILL.md",
        "v1\nlocal edit\n",
        false,
    );
    write_patch(&f);
    use_patch(&f);
    write(
        &f.upstream("p"),
        "skills/a/SKILL.md",
        "completely different\n",
        false,
    );
    let report = f.sync("p", "v2", &resolve).unwrap();
    assert_eq!(report.rejects.len(), 1);
    assert!(report.rejects[0].contains("SKILL.md"));
}
