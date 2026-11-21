//! test_vendor.LicenseTest.

use std::fs;

use anthology_pycompat::json::Value;

use crate::fixture::{FIXED_MIT, Fixture, MIT_TEXT, entry, get_str, names, s, write};

fn setup() -> (Fixture, Value, Value) {
    let f = Fixture::new();
    let a = f.add_upstream("a", &[("LICENSE", MIT_TEXT), ("x.md", "x\n")], &[]);
    let b = f.add_upstream(
        "b",
        &[("README.md", "License: MIT\n")],
        &[("license_file", Value::Null), ("license_note", s("README"))],
    );
    (f, a, b)
}

#[test]
fn test_fetch_licenses_copies_unchanged_one_per_repo() {
    let (f, a, _) = setup();
    let a2 = entry(
        "a2",
        &[
            ("repo", s(&get_str(&a, "repo"))),
            ("target", s("plugins/anthology/knowledge/a2")),
        ],
    );
    f.set_upstream("a2", f.upstream("a"));
    f.build_all(&[a, a2]);
    assert_eq!(names(&f.root.join("vendor/licenses")), ["acme-a.txt"]);
    assert_eq!(
        fs::read_to_string(f.root.join("vendor/licenses/acme-a.txt")).unwrap(),
        MIT_TEXT
    );
}

#[test]
fn test_fetch_licenses_is_byte_exact() {
    let (f, a, _) = setup();
    let raw: &[u8] = b"MIT License\r\n\r\nCopyright \xc2\xa9 x\r\n";
    write(&f.upstream("a"), "LICENSE", raw, false);
    f.set_sources(&[a]);
    f.fetch_licenses().unwrap();
    assert_eq!(
        fs::read(f.root.join("vendor/licenses/acme-a.txt")).unwrap(),
        raw
    );
}

#[test]
fn test_check_flags_license_drift() {
    let (f, a, _) = setup();
    f.build_all(&[a]);
    fs::write(f.root.join("vendor/licenses/acme-a.txt"), "tampered\n").unwrap();
    assert!(f.check().iter().any(|p| p.contains("acme-a.txt")));
}

#[test]
fn test_check_flags_missing_license() {
    let (f, a, _) = setup();
    f.build_all(&[a]);
    fs::remove_file(f.root.join("vendor/licenses/acme-a.txt")).unwrap();
    assert!(f.check().iter().any(|p| p.contains("acme-a.txt")));
}

#[test]
fn test_check_flags_empty_license() {
    let (f, a, _) = setup();
    write(&f.upstream("a"), "LICENSE", "", false);
    f.set_sources(&[a]);
    assert!(f.fetch_licenses().is_err());
    f.build("a").unwrap();
    write(&f.root, "vendor/licenses/acme-a.txt", "", false);
    assert!(f.check().iter().any(|p| p.contains("acme-a.txt")));
}

#[test]
fn test_missing_upstream_license_file_is_an_error() {
    let (f, a, _) = setup();
    fs::remove_file(f.upstream("a").join("LICENSE")).unwrap();
    f.set_sources(&[a]);
    assert!(f.fetch_licenses().is_err());
}

#[test]
fn test_fixed_license_is_not_overwritten_or_compared_to_upstream() {
    let (f, _, b) = setup();
    f.set_sources(&[b]);
    assert!(f.build("b").is_err()); // build needs the fixed text to ship it
    write(&f.root, "vendor/licenses/acme-b.txt", FIXED_MIT, false);
    f.build("b").unwrap();
    f.fetch_licenses().unwrap(); // must not overwrite it
    let path = f.root.join("vendor/licenses/acme-b.txt");
    assert_eq!(fs::read_to_string(&path).unwrap(), FIXED_MIT);
    assert_eq!(f.check(), Vec::<String>::new());
    fs::remove_file(&path).unwrap();
    assert!(f.check().iter().any(|p| p.contains("acme-b.txt"))); // missing fixed file
    write(&f.root, "vendor/licenses/acme-b.txt", FIXED_MIT, false);
    write(
        &f.root,
        "vendor/licenses/acme-b.txt",
        "MIT License\n\nCopyright (c) Vercel\n",
        false,
    );
    assert!(f.check().iter().any(|p| p.contains("README"))); // note line is required
}
