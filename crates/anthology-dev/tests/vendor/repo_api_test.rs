//! The vendor API that test_licenses, test_repo_layout and test_taste_browseros import, run on
//! the real repo (those tests belong to MT1's ledgers; this checks that the Rust API gives
//! what they rely on).

use std::collections::BTreeSet;
use std::fs;

use anthology_dev::vendor;
use anthology_pycompat::json::Value;

use crate::fixture::{jv, repo_root};

fn text<'a>(e: &'a vendor::Entry, k: &str) -> &'a str {
    e.get(k).and_then(Value::as_str).unwrap()
}

/// test_licenses `knowledge_sources()`.
#[test]
fn knowledge_sources_are_the_28_vendored_knowledge_targets() {
    let entries = vendor::load_sources(&repo_root()).unwrap();
    let knowledge: Vec<_> = vendor::vendored(&entries)
        .unwrap()
        .into_iter()
        .filter(|e| text(e, "target").starts_with("plugins/anthology/knowledge/"))
        .collect();
    assert_eq!(knowledge.len(), 28);
}

/// test_repo_layout `test_every_source_has_a_target_and_a_license`.
#[test]
fn license_slugs_are_the_vendor_licenses_files() {
    let root = repo_root();
    let entries = vendor::load_sources(&root).unwrap();
    let slugs: BTreeSet<String> = entries
        .iter()
        .filter(|e| !vendor::is_subtree(e))
        .map(|e| vendor::source_slug(e.as_object().unwrap()))
        .collect();
    let files: BTreeSet<String> = fs::read_dir(root.join("vendor/licenses"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            name.strip_suffix(".txt").map(str::to_string)
        })
        .collect();
    assert_eq!(slugs, files);
}

/// test_taste_browseros `test_the_shipped_scripts_are_exactly_keep_port_and_added` and
/// `test_the_sources_entry_points_at_the_map`.
#[test]
fn the_taste_port_map_rule_finds_the_shipped_scripts() {
    let root = repo_root();
    let map = jv(&fs::read_to_string(
        root.join("vendor/patches/design-taste-frontend.portmap.json"),
    )
    .unwrap());
    let map = map.as_object().unwrap();
    let mut wanted: BTreeSet<String> = map
        .get("scripts")
        .unwrap()
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, s)| {
            matches!(
                s.as_object().unwrap().get("action").and_then(Value::as_str),
                Some("keep" | "port")
            )
        })
        .map(|(rel, _)| rel.to_string())
        .collect();
    wanted.extend(
        map.get("added")
            .unwrap()
            .as_object()
            .unwrap()
            .iter()
            .map(|(rel, _)| rel.to_string()),
    );
    let folder = root.join("plugins/anthology/knowledge/design-taste-frontend");
    let rule = map.get("script_rule").unwrap().as_object().unwrap();
    assert_eq!(vendor::portmap_scripts(&folder, rule).unwrap(), wanted);
    let entries = vendor::load_sources(&root).unwrap();
    let entry = vendor::find_entry(&entries, "design-taste-frontend").unwrap();
    assert_eq!(
        text(entry, "portmap"),
        "vendor/patches/design-taste-frontend.portmap.json"
    );
    vendor::load_portmap(&root, entry).unwrap();
}
