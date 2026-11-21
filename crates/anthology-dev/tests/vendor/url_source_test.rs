//! test_vendor.UrlSourceTest: url sources build, check and license from a committed,
//! checksum-pinned snapshot with neither git nor network.

use std::fs;
use std::path::PathBuf;

use anthology_dev::vendor;
use anthology_pycompat::json::Value;

use crate::fixture::{
    Fixture, assert_err_matches, checksums_bytes, have_git, jv, s, set, sha, symlink, tree_files,
    url_entry, url_files, write, write_snapshot,
};

struct Case {
    f: Fixture,
    e: Value,
    snap: PathBuf,
}

fn setup() -> Case {
    let f = Fixture::new();
    let e = url_entry("ui", &[]);
    f.set_sources(std::slice::from_ref(&e));
    let snap = write_snapshot(&f.root, "ui", &url_files());
    Case { f, e, snap }
}

impl Case {
    fn fetch_offline(&self) -> vendor::Result<PathBuf> {
        vendor::snapshot_fetcher(&self.f.root)(self.e.as_object().unwrap())
    }

    fn knowledge(&self) -> PathBuf {
        self.f.root.join("plugins/anthology/knowledge/ui")
    }
}

#[test]
fn test_offline_fetch_returns_the_verified_snapshot() {
    let c = setup();
    assert_eq!(c.fetch_offline().unwrap(), c.snap);
}

#[test]
fn test_the_default_fetcher_routes_url_entries_to_the_snapshot() {
    let c = setup();
    assert_eq!(
        vendor::fetcher(&c.f.root, None)(c.e.as_object().unwrap()).unwrap(),
        c.snap
    );
}

#[test]
fn test_a_tampered_file_is_rejected() {
    let c = setup();
    write(&c.snap, "ref/a.md", b"changed\n", false);
    assert_err_matches(c.fetch_offline(), "ref/a.md");
}

#[test]
fn test_an_extra_file_is_rejected() {
    let c = setup();
    write(&c.snap, "ref/extra.md", b"x\n", false);
    assert_err_matches(c.fetch_offline(), "extra.md");
}

#[test]
fn test_a_missing_file_is_rejected() {
    let c = setup();
    fs::remove_file(c.snap.join("ref/a.md")).unwrap();
    assert_err_matches(c.fetch_offline(), "ref/a.md");
}

#[test]
fn test_a_wrong_pin_is_rejected() {
    let mut c = setup();
    set(&mut c.e, "checksum", s(&"0".repeat(64)));
    assert_err_matches(c.fetch_offline(), "checksum");
}

#[test]
fn test_a_rewritten_checksums_file_is_rejected_by_the_pin() {
    let c = setup();
    let mut files = url_files();
    files.insert("ref/a.md".into(), b"changed\n".to_vec());
    write_snapshot(&c.f.root, "ui", &files);
    assert_err_matches(c.fetch_offline(), "checksum");
}

#[test]
fn test_a_missing_snapshot_is_rejected() {
    let c = setup();
    fs::remove_dir_all(&c.snap).unwrap();
    assert_err_matches(c.fetch_offline(), "fetch ui");
}

#[test]
fn test_a_symlink_in_the_snapshot_is_rejected() {
    let c = setup();
    fs::remove_file(c.snap.join("ref/a.md")).unwrap();
    symlink("../SKILL.md", &c.snap.join("ref/a.md"));
    assert!(c.fetch_offline().is_err());
}

#[test]
fn test_unsafe_paths_in_checksums_are_rejected() {
    for bad in [
        "../x",
        "/etc/passwd",
        "a/../../b",
        "a//b",
        "./a",
        "a\\b",
        "CHECKSUMS.sha256",
    ] {
        let line = format!("{}  {bad}\n", "1".repeat(64));
        assert!(vendor::parse_checksums(line.as_bytes()).is_err(), "{bad}");
    }
}

#[test]
fn test_malformed_checksums_are_rejected() {
    let line = format!("{}  a\n", "1".repeat(64)).into_bytes();
    let short = [b"1".repeat(64), b" a\n".to_vec()].concat();
    let blank = [line.clone(), b"\n".to_vec()].concat();
    let twice = [line.clone(), line.clone()].concat();
    let cases: [&[u8]; 6] = [b"", b"nothex  a\n", &short, &blank, &twice, b"\xff\xfe"];
    for bad in cases {
        assert!(vendor::parse_checksums(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn test_well_formed_checksums_parse() {
    let files = url_files();
    let want: std::collections::BTreeMap<String, String> = files
        .iter()
        .map(|(rel, data)| (rel.clone(), sha(data)))
        .collect();
    assert_eq!(
        vendor::parse_checksums(&checksums_bytes(&files)).unwrap(),
        want
    );
}

#[test]
fn test_build_check_and_license_work_without_git_or_network() {
    let c = setup();
    vendor::build(&c.f.root, "ui", None, None, false).unwrap();
    let mut want: Vec<String> = url_files().into_keys().collect();
    want.push("CHECKSUMS.sha256".into());
    want.sort();
    assert_eq!(
        tree_files(&c.knowledge()).into_keys().collect::<Vec<_>>(),
        want
    );
    vendor::fetch_licenses(&c.f.root, None, None).unwrap();
    assert_eq!(
        fs::read(c.f.root.join("vendor/licenses/ui.txt")).unwrap(),
        url_files()["LICENSE"]
    );
    assert_eq!(
        vendor::check(&c.f.root, None).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn test_check_reports_a_tampered_snapshot() {
    let c = setup();
    vendor::build(&c.f.root, "ui", None, None, false).unwrap();
    vendor::fetch_licenses(&c.f.root, None, None).unwrap();
    write(&c.snap, "SKILL.md", b"changed\n", false);
    assert!(
        vendor::check(&c.f.root, None)
            .unwrap()
            .iter()
            .any(|p| p.contains("SKILL.md"))
    );
}

#[test]
fn test_check_reports_a_drifted_license() {
    let c = setup();
    vendor::build(&c.f.root, "ui", None, None, false).unwrap();
    vendor::fetch_licenses(&c.f.root, None, None).unwrap();
    write(&c.f.root, "vendor/licenses/ui.txt", "other\n", false);
    assert!(
        vendor::check(&c.f.root, None)
            .unwrap()
            .iter()
            .any(|p| p.contains("ui.txt"))
    );
}

#[test]
fn test_diff_works_on_a_url_entry() {
    let c = setup();
    vendor::build(&c.f.root, "ui", None, None, false).unwrap();
    write(
        &c.knowledge(),
        "SKILL.md",
        b"---\nname: ui\n---\nnew\n",
        false,
    );
    assert!(
        vendor::diff_entry(&c.f.root, "ui", None)
            .unwrap()
            .contains("-body")
    );
}

#[test]
fn test_a_patch_on_a_url_entry_can_drop_the_shipped_checksums() {
    if !have_git() {
        return;
    }
    let mut c = setup();
    vendor::build(&c.f.root, "ui", None, None, false).unwrap();
    let target = c.knowledge();
    fs::remove_file(target.join("CHECKSUMS.sha256")).unwrap();
    let patch = vendor::diff_entry(&c.f.root, "ui", None).unwrap();
    write(&c.f.root, "vendor/patches/ui.patch", patch, false);
    set(
        &mut c.e,
        "transform",
        jv(r#"[{"patch": "vendor/patches/ui.patch"}]"#),
    );
    c.f.set_sources(std::slice::from_ref(&c.e));
    vendor::build(&c.f.root, "ui", None, None, false).unwrap();
    assert!(!target.join("CHECKSUMS.sha256").exists());
    vendor::fetch_licenses(&c.f.root, None, None).unwrap();
    assert_eq!(
        vendor::check(&c.f.root, None).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn test_sync_on_a_url_entry_is_an_error_with_a_hint() {
    let c = setup();
    assert_err_matches(
        vendor::sync(&c.f.root, "ui", "v2", None, None),
        r"checksum.*vendor\.py fetch ui",
    );
}
