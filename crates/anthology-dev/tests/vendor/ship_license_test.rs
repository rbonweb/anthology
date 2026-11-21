//! test_vendor.ShipLicenseTest: build_tree ships each source's license (and upstream NOTICE
//! files) inside the target.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_dev::vendor;
use anthology_pycompat::json::Value;

use crate::fixture::{
    FIXED_MIT, Fixture, MIT_TEXT, have_git, jv, names, s, set, symlink, validate, write,
};

const NOTICE: &str = "Thing\nCopyright 2026 Someone\n";
const SKILL_PATCH: &str = "--- a/SKILL.md\n+++ b/SKILL.md\n@@ -1 +1,2 @@\n s\n+patched\n";

/// `sub(name, files, **kw)`: a source taking `skills/s` of a repo whose root holds LICENSE.
fn sub(f: &Fixture, name: &str, files: &[(&str, &str)], kw: &[(&str, Value)]) -> Value {
    let mut all: Vec<(&str, &str)> = vec![("LICENSE", MIT_TEXT), ("skills/s/SKILL.md", "s\n")];
    for (k, v) in files {
        all.retain(|(a, _)| a != k);
        all.push((k, v));
    }
    let mut kw: Vec<(&str, Value)> = kw.to_vec();
    if !kw.iter().any(|(k, _)| *k == "path") {
        kw.push(("path", s("skills/s")));
    }
    f.add_upstream(name, &all, &kw)
}

fn sub_s(f: &Fixture, files: &[(&str, &str)], kw: &[(&str, Value)]) -> Value {
    sub(f, "s", files, kw)
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn err_of<T: std::fmt::Debug>(r: vendor::Result<T>) -> String {
    r.unwrap_err().0
}

/// `link(name, rel, to)`: replace `<pristine>/rel` with a symlink to `to`.
fn link(f: &Fixture, name: &str, rel: &str, to: impl AsRef<Path>) {
    let path = f.upstream(name).join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    if fs::symlink_metadata(&path).is_ok() {
        fs::remove_file(&path).unwrap();
    }
    symlink(to, &path);
}

fn outside_file(f: &Fixture) -> PathBuf {
    let secret = f.tmp.join("secret.txt");
    fs::write(&secret, "PRIVATE\n").unwrap();
    secret
}

/// `patched(patch_text, files, **kw)`: an entry with one patch (vendor/patches/s.patch).
fn patched(f: &Fixture, patch: &str, files: &[(&str, &str)], kw: &[(&str, Value)]) -> Value {
    let mut kw = kw.to_vec();
    kw.push(("transform", jv(r#"[{"patch": "vendor/patches/s.patch"}]"#)));
    let e = sub_s(f, files, &kw);
    write(&f.root, "vendor/patches/s.patch", patch, false);
    e
}

#[test]
fn test_license_outside_the_folder_is_copied_unchanged() {
    let f = Fixture::new();
    let raw: &[u8] = b"MIT License\r\n\r\nCopyright \xc2\xa9 x\r\n";
    let e = sub_s(&f, &[], &[]);
    write(&f.upstream("s"), "LICENSE", raw, false);
    f.build_all(&[e]);
    assert_eq!(fs::read(f.target("s", "LICENSE")).unwrap(), raw);
    assert_eq!(read(&f.target("s", "SKILL.md")), "s\n");
    assert_eq!(mode(&f.target("s", "LICENSE")), 0o644);
    assert_eq!(f.check(), Vec::<String>::new());
}

#[test]
fn test_license_file_keeps_its_basename() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("legal/COPYING.txt", MIT_TEXT)],
        &[("license_file", s("legal/COPYING.txt"))],
    );
    f.build_all(&[e]);
    assert_eq!(read(&f.target("s", "COPYING.txt")), MIT_TEXT);
    assert!(!f.target("s", "LICENSE").exists());
}

#[test]
fn test_license_inside_the_folder_is_not_duplicated() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("skills/s/LICENSE.txt", MIT_TEXT)],
        &[("license_file", s("skills/s/LICENSE.txt"))],
    );
    f.build_all(&[e]);
    assert_eq!(names(&f.target("s", "")), ["LICENSE.txt", "SKILL.md"]);
    assert_eq!(f.check(), Vec::<String>::new());
}

#[test]
fn test_license_inside_the_folder_must_be_there() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("skills/s/LICENSE.txt", MIT_TEXT)],
        &[("license_file", s("skills/s/LICENSE.txt"))],
    );
    fs::remove_file(f.upstream("s").join("skills/s/LICENSE.txt")).unwrap();
    f.set_sources(&[e]);
    assert!(f.build("s").is_err());
}

#[test]
fn test_root_path_source_keeps_its_tree_byte_identical() {
    let f = Fixture::new();
    let e = f.add_upstream("r", &[("LICENSE", MIT_TEXT), ("a.md", "a\n")], &[]);
    f.build_all(&[e]);
    assert_eq!(names(&f.target("r", "")), ["LICENSE", "a.md"]);
}

#[test]
fn test_null_license_ships_the_fixed_text_as_license() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("README.md", "License: MIT\n")],
        &[("license_file", Value::Null), ("license_note", s("README"))],
    );
    f.set_sources(&[e]);
    write(&f.root, "vendor/licenses/acme-s.txt", FIXED_MIT, false);
    f.build("s").unwrap();
    assert_eq!(read(&f.target("s", "LICENSE")), FIXED_MIT);
    assert_eq!(f.check(), Vec::<String>::new());
}

#[test]
fn test_missing_fixed_text_is_an_error() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[],
        &[("license_file", Value::Null), ("license_note", s("README"))],
    );
    f.set_sources(&[e]);
    assert!(f.build("s").is_err());
}

#[test]
fn test_license_extra_files_are_copied_unchanged() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("NOTICE", NOTICE), ("LICENSE-MIT", "MIT only\n")],
        &[("license_extra", jv(r#"["NOTICE", "LICENSE-MIT"]"#))],
    );
    f.build_all(&[e]);
    assert_eq!(read(&f.target("s", "NOTICE")), NOTICE);
    assert_eq!(read(&f.target("s", "LICENSE-MIT")), "MIT only\n");
    assert_eq!(mode(&f.target("s", "NOTICE")), 0o644);
    assert_eq!(f.check(), Vec::<String>::new());
}

#[test]
fn test_missing_or_empty_license_extra_is_an_error() {
    let f = Fixture::new();
    let e = sub_s(&f, &[], &[("license_extra", jv(r#"["NOTICE"]"#))]);
    f.set_sources(&[e]);
    assert!(f.build("s").is_err());
    write(&f.upstream("s"), "NOTICE", "  \n", false);
    assert!(f.build("s").is_err());
}

#[test]
fn test_license_extra_validation() {
    let f = Fixture::new();
    let errors = |kw: &[(&str, Value)]| validate(&[sub_s(&f, &[], kw)]);
    assert_eq!(
        errors(&[("license_extra", jv(r#"["NOTICE", "docs/LICENSE-MIT"]"#))]),
        Vec::<String>::new()
    );
    assert_eq!(errors(&[("license_extra", jv("[]"))]), Vec::<String>::new());
    for bad in [
        r#""NOTICE""#,
        r#"["../NOTICE"]"#,
        r#"["/NOTICE"]"#,
        r#"[""]"#,
        "[3]",
        r#"["a//b"]"#,
    ] {
        assert!(
            errors(&[("license_extra", jv(bad))])
                .iter()
                .any(|m| m.contains("license_extra")),
            "{bad}"
        );
    }
    let clashes = |kw: &[(&str, Value)]| errors(kw).iter().any(|m| m.contains("clashes"));
    assert!(clashes(&[(
        "license_extra",
        jv(r#"["NOTICE", "docs/NOTICE"]"#)
    )]));
    assert!(clashes(&[("license_extra", jv(r#"["docs/LICENSE"]"#))]));
    assert!(clashes(&[
        ("license_file", Value::Null),
        ("license_note", s("README")),
        ("license_extra", jv(r#"["LICENSE"]"#)),
    ]));
}

#[test]
fn test_license_extra_is_not_allowed_on_url_sources() {
    let e = jv(&format!(
        r#"{{"name": "u", "url": "https://example.com/.well-known/skills/u/", "path": "",
        "ref": "main", "license": "MIT", "license_file": "LICENSE", "checksum": "{}",
        "target": "plugins/anthology/knowledge/u", "transform": [], "license_extra": ["NOTICE"]}}"#,
        "a".repeat(64)
    ));
    assert!(validate(&[e]).iter().any(|m| m.contains("license_extra")));
}

#[test]
fn test_identical_existing_file_is_fine() {
    let f = Fixture::new();
    let e = sub_s(&f, &[("skills/s/LICENSE", MIT_TEXT)], &[]);
    f.build_all(&[e]);
    assert_eq!(read(&f.target("s", "LICENSE")), MIT_TEXT);
    assert_eq!(f.check(), Vec::<String>::new());
}

#[test]
fn test_differing_existing_file_is_an_error() {
    let f = Fixture::new();
    let e = sub_s(&f, &[("skills/s/LICENSE", "other terms\n")], &[]);
    f.set_sources(&[e]);
    assert!(err_of(f.build("s")).contains("LICENSE"));
    assert!(f.check().iter().any(|m| m.contains("LICENSE")));
}

#[test]
fn test_differing_existing_extra_is_an_error() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("NOTICE", NOTICE), ("skills/s/NOTICE", "different\n")],
        &[("license_extra", jv(r#"["NOTICE"]"#))],
    );
    f.set_sources(&[e]);
    assert!(err_of(f.build("s")).contains("NOTICE"));
}

#[test]
fn test_unshipped_root_notice_is_an_error() {
    let f = Fixture::new();
    let e = sub_s(&f, &[("NOTICE", NOTICE)], &[]);
    f.set_sources(&[e]);
    let err = err_of(f.build("s"));
    assert!(err.contains("NOTICE"), "{err}");
    assert!(err.contains("license_extra"), "{err}");
    assert!(f.check().iter().any(|m| m.contains("license_extra")));
}

#[test]
fn test_unshipped_notice_on_the_way_down_is_an_error() {
    let f = Fixture::new();
    for name in ["NOTICE.txt", "NOTICE.md"] {
        let rel = format!("skills/{name}");
        let e = sub_s(&f, &[(rel.as_str(), NOTICE)], &[]);
        f.set_sources(&[e]);
        assert!(err_of(f.build("s")).contains(&rel));
        fs::remove_file(f.upstream("s").join("skills").join(name)).unwrap();
    }
}

#[test]
fn test_listed_notice_on_the_way_down_is_shipped() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("skills/NOTICE.md", NOTICE)],
        &[("license_extra", jv(r#"["skills/NOTICE.md"]"#))],
    );
    f.build_all(&[e]);
    assert_eq!(read(&f.target("s", "NOTICE.md")), NOTICE);
    assert_eq!(f.check(), Vec::<String>::new());
}

#[test]
fn test_notice_inside_the_folder_ships_with_the_folder() {
    let f = Fixture::new();
    let e = sub_s(&f, &[("skills/s/NOTICE", NOTICE)], &[]);
    f.build_all(&[e]);
    assert_eq!(read(&f.target("s", "NOTICE")), NOTICE);
}

#[test]
fn test_notice_beside_the_path_is_not_required() {
    let f = Fixture::new();
    let e = sub_s(&f, &[("skills/other/NOTICE", NOTICE)], &[]);
    f.build_all(&[e]);
    assert!(!f.target("s", "NOTICE").exists());
}

#[test]
fn test_notice_in_a_root_path_source_ships_with_the_tree() {
    let f = Fixture::new();
    let e = f.add_upstream("r", &[("LICENSE", MIT_TEXT), ("NOTICE", NOTICE)], &[]);
    f.build_all(&[e]);
    assert_eq!(read(&f.target("r", "NOTICE")), NOTICE);
}

#[test]
fn test_check_flags_a_missing_or_altered_license_copy() {
    let f = Fixture::new();
    let e = sub_s(&f, &[], &[]);
    f.build_all(&[e]);
    assert_eq!(f.check(), Vec::<String>::new());
    write(&f.target("s", ""), "LICENSE", "tampered\n", false);
    assert!(f.check().iter().any(|m| m.contains("LICENSE differs")));
    fs::remove_file(f.target("s", "LICENSE")).unwrap();
    assert!(
        f.check()
            .iter()
            .any(|m| m.contains("missing from the committed tree: LICENSE"))
    );
}

#[test]
fn test_check_flags_a_missing_extra() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("NOTICE", NOTICE)],
        &[("license_extra", jv(r#"["NOTICE"]"#))],
    );
    f.build_all(&[e]);
    fs::remove_file(f.target("s", "NOTICE")).unwrap();
    assert!(f.check().iter().any(|m| m.contains("NOTICE")));
}

#[test]
fn test_diff_does_not_show_license_copies() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("NOTICE", NOTICE)],
        &[("license_extra", jv(r#"["NOTICE"]"#))],
    );
    f.build_all(&[e]);
    assert_eq!(f.diff("s").unwrap(), "");
    write(&f.target("s", ""), "SKILL.md", "s\nlocal\n", false);
    let text = f.diff("s").unwrap();
    assert!(text.contains("SKILL.md"));
    assert!(!text.contains("LICENSE"));
    assert!(!text.contains("NOTICE"));
}

#[test]
fn test_licenses_are_shipped_whether_or_not_patches_apply() {
    let f = Fixture::new();
    let e = sub_s(&f, &[], &[]);
    f.build_all(std::slice::from_ref(&e));
    let built = f.tmp.join("nopatch");
    vendor::build_tree(
        &f.root,
        e.as_object().unwrap(),
        &f.upstream("s"),
        &built,
        false,
        false,
    )
    .unwrap();
    assert_eq!(read(&built.join("LICENSE")), MIT_TEXT);
}

// -- symlinks, NOTICE names, and the patch interplay ------------------------------------------

#[test]
fn test_symlinked_license_file_is_refused() {
    let f = Fixture::new();
    let e = sub_s(&f, &[], &[]);
    link(&f, "s", "LICENSE", outside_file(&f));
    f.set_sources(&[e]);
    assert!(err_of(f.build("s")).contains("LICENSE"));
    assert!(!f.target("s", "LICENSE").exists());
    assert!(f.fetch_licenses().is_err()); // vendor/licenses/ is fed by the same read
    assert!(!f.root.join("vendor/licenses/acme-s.txt").exists());
}

#[test]
fn test_symlinked_license_file_to_a_file_inside_the_checkout_is_refused_too() {
    let f = Fixture::new();
    let e = sub_s(&f, &[("real/LICENSE", MIT_TEXT)], &[]);
    link(&f, "s", "LICENSE", "real/LICENSE");
    f.set_sources(&[e]);
    assert!(f.build("s").is_err());
}

#[test]
fn test_symlinked_license_extra_is_refused() {
    let f = Fixture::new();
    let e = sub_s(&f, &[], &[("license_extra", jv(r#"["NOTICE"]"#))]);
    link(&f, "s", "NOTICE", outside_file(&f));
    f.set_sources(&[e]);
    assert!(err_of(f.build("s")).contains("NOTICE"));
    assert!(!f.target("s", "NOTICE").exists());
}

#[test]
fn test_license_extra_behind_a_symlinked_directory_is_refused() {
    let f = Fixture::new();
    let outside = f.tmp.join("elsewhere");
    write(&outside, "NOTICE", "PRIVATE\n", false);
    let e = sub_s(&f, &[], &[("license_extra", jv(r#"["docs/NOTICE"]"#))]);
    link(&f, "s", "docs", &outside);
    f.set_sources(&[e]);
    assert!(f.build("s").is_err());
}

#[test]
fn test_symlinked_upstream_notice_is_refused() {
    let f = Fixture::new();
    let e = sub_s(&f, &[], &[]);
    link(&f, "s", "NOTICE", outside_file(&f));
    f.set_sources(&[e]);
    assert!(err_of(f.build("s")).contains("symlink"));
    assert!(f.check().iter().any(|m| m.contains("NOTICE")));
}

#[test]
fn test_notice_names_match_in_any_case() {
    let f = Fixture::new();
    for name in ["notice", "NOTICE.TXT", "Notice.md"] {
        let e = sub_s(&f, &[(name, NOTICE)], &[]);
        f.set_sources(&[e]);
        let err = err_of(f.build("s"));
        assert!(
            err.contains(&format!("upstream {name} is not shipped")),
            "{err}"
        );
        fs::remove_file(f.upstream("s").join(name)).unwrap();
    }
}

#[test]
fn test_a_notice_directory_is_not_a_notice() {
    let f = Fixture::new();
    let e = sub_s(&f, &[("NOTICE/readme.md", "x\n")], &[]);
    f.build_all(&[e]);
}

#[test]
fn test_license_in_a_sibling_folder_with_the_same_prefix_is_copied() {
    let f = Fixture::new();
    let e = sub_s(
        &f,
        &[("skills/s2/LICENSE", MIT_TEXT)],
        &[("license_file", s("skills/s2/LICENSE"))],
    );
    f.build_all(&[e]);
    assert_eq!(read(&f.target("s", "LICENSE")), MIT_TEXT);
}

#[test]
fn test_diff_reproduces_a_patch_without_license_hunks() {
    if !have_git() {
        return;
    }
    let f = Fixture::new();
    let e = patched(
        &f,
        SKILL_PATCH,
        &[("NOTICE", NOTICE)],
        &[("license_extra", jv(r#"["NOTICE"]"#))],
    );
    f.build_all(&[e]);
    assert_eq!(read(&f.target("s", "SKILL.md")), "s\npatched\n");
    assert_eq!(f.check(), Vec::<String>::new());
    let text = f.diff("s").unwrap();
    assert_eq!(
        text,
        "--- a/SKILL.md\n+++ b/SKILL.md\n@@ -1 +1,2 @@\n s\n+patched\n"
    );
    assert!(!text.contains("LICENSE"));
    assert!(!text.contains("NOTICE"));
}

#[test]
fn test_a_patch_may_resolve_a_license_name_conflict() {
    if !have_git() {
        return;
    }
    let f = Fixture::new();
    let drop = "--- a/LICENSE\n+++ /dev/null\n@@ -1 +0,0 @@\n-stale\n";
    let mut e = patched(&f, drop, &[("skills/s/LICENSE", "stale\n")], &[]);
    f.build_all(std::slice::from_ref(&e));
    assert_eq!(read(&f.target("s", "LICENSE")), MIT_TEXT); // the root text ships
    assert_eq!(f.check(), Vec::<String>::new());
    f.diff("s").unwrap(); // must not raise
    write(&f.root, "vendor/patches/s.patch", "", false); // without the patch the conflict is an error
    set(&mut e, "transform", jv("[]"));
    f.set_sources(&[e]);
    assert!(f.build("s").is_err());
    assert!(f.check().iter().any(|m| m.contains("LICENSE")));
}

#[test]
fn test_a_patch_that_deletes_the_in_folder_license_is_an_error() {
    if !have_git() {
        return;
    }
    let f = Fixture::new();
    let drop = "--- a/LICENSE.txt\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-MIT License\n-\n-Copyright (c) Someone\n";
    let e = patched(
        &f,
        drop,
        &[("skills/s/LICENSE.txt", MIT_TEXT)],
        &[("license_file", s("skills/s/LICENSE.txt"))],
    );
    f.set_sources(&[e]);
    assert!(f.build("s").is_err());
    assert!(f.check().iter().any(|m| m.contains("LICENSE.txt")));
}

#[test]
fn test_a_patch_that_deletes_an_in_folder_notice_is_an_error() {
    if !have_git() {
        return;
    }
    let f = Fixture::new();
    let drop = "--- a/NOTICE\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-Thing\n-Copyright 2026 Someone\n";
    let e = patched(&f, drop, &[("skills/s/NOTICE", NOTICE)], &[]);
    f.set_sources(&[e]);
    assert!(err_of(f.build("s")).contains("a patch removed it"));
}
