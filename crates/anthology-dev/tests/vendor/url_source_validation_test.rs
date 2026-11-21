//! test_vendor.UrlSourceValidationTest.

use anthology_dev::vendor;
use anthology_pycompat::json::Value;

use crate::fixture::{SHA_A, del, s, url_entry, validate};

#[test]
fn test_valid_url_entry_has_no_errors() {
    assert_eq!(validate(&[url_entry("ui", &[])]), Vec::<String>::new());
}

#[test]
fn test_url_must_be_https_and_end_with_a_slash() {
    for bad in [
        "http://example.test/s/ui/",
        "file:///tmp/ui/",
        "ftp://example.test/ui/",
        "https://",
        "https:///ui/",
        "https://example.test/ui",
        "//example.test/ui/",
        "https://example.test/u i/",
        "https://user@example.test/ui/",
        "",
    ] {
        assert!(
            !validate(&[url_entry("ui", &[("url", s(bad))])]).is_empty(),
            "{bad}"
        );
    }
}

#[test]
fn test_a_url_entry_needs_a_checksum_and_no_repo_or_commit() {
    let mut e = url_entry("ui", &[]);
    del(&mut e, "checksum");
    assert!(!validate(&[e]).is_empty());
    for bad in [
        "abc".to_string(),
        "C".repeat(64),
        "c".repeat(63),
        String::new(),
    ] {
        assert!(
            !validate(&[url_entry("ui", &[("checksum", s(&bad))])]).is_empty(),
            "{bad}"
        );
    }
    assert!(!validate(&[url_entry("ui", &[("repo", s("https://github.com/a/b"))])]).is_empty());
    assert!(!validate(&[url_entry("ui", &[("commit", s(SHA_A))])]).is_empty());
}

#[test]
fn test_a_url_entry_has_an_empty_path() {
    assert!(!validate(&[url_entry("ui", &[("path", s("skills/ui"))])]).is_empty());
}

#[test]
fn test_a_url_entry_needs_a_license_file_inside_the_snapshot() {
    let mut e = url_entry("ui", &[]);
    del(&mut e, "license_file");
    assert!(!validate(&[e]).is_empty());
    assert!(!validate(&[url_entry("ui", &[("license_file", Value::Null)])]).is_empty());
    for bad in ["/etc/passwd", "../LICENSE", "a/../../x"] {
        assert!(
            !validate(&[url_entry("ui", &[("license_file", s(bad))])]).is_empty(),
            "{bad}"
        );
    }
}

#[test]
fn test_the_license_slug_of_a_url_entry_is_its_name() {
    assert_eq!(
        vendor::source_slug(url_entry("ui", &[]).as_object().unwrap()),
        "ui"
    );
}
