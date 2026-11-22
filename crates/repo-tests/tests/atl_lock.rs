//! Port of `tests/test_rtk_lock.py`: `plugins/anthology/atl.lock` pins the released atl binaries.

use std::collections::BTreeSet;

use anthology_core::lock::{self, ATL_TAG_PREFIX};
use anthology_pycompat::json::{self, Value};
use regex::Regex;
use repo_tests::repo;

const LOCK: &str = "plugins/anthology/atl.lock";

/// NEEDS_RELEASE_HASHES: true while `atl.lock` still holds placeholder digests, because the
/// `atl--v1.2.1-anthology.6` release does not exist yet. While it is true a digest of 64 zeros
/// is accepted; once it is false only a real digest is. The release step replaces the four
/// digests in `atl.lock` and flips this to false in the same commit.
const NEEDS_RELEASE_HASHES: bool = true;
/// The digest `atl.lock` holds until the release exists.
const PLACEHOLDER_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// A lock digest: 64 lowercase hex digits, and the placeholder only while `needs_release_hashes`.
fn digest_ok(digest: &str, needs_release_hashes: bool) -> bool {
    let hex = Regex::new(r"^[0-9a-f]{64}$").unwrap();
    hex.is_match(digest) && (needs_release_hashes || digest != PLACEHOLDER_DIGEST)
}
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

fn lock_json() -> Value {
    json::loads(&repo::read(LOCK)).expect("atl.lock is JSON")
}

fn field(lock: &Value, key: &str) -> Value {
    lock.as_object()
        .and_then(|o| o.get(key))
        .unwrap_or_else(|| panic!("atl.lock has no {key}"))
        .clone()
}

fn string_field(lock: &Value, key: &str) -> String {
    field(lock, key).into_string().expect("a string")
}

mod atl_lock_test {
    use super::*;

    #[test]
    fn test_exact_keys() {
        let lock = lock_json();
        let keys: BTreeSet<&str> = lock
            .as_object()
            .expect("an object")
            .iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(keys, BTreeSet::from(["version", "tag", "repo", "sha256"]));
    }

    #[test]
    fn test_version_matches_cargo_toml() {
        let cargo = repo::read("atl/Cargo.toml");
        let re = Regex::new(r#"(?m)^version = "([^"]+)""#).unwrap();
        let version = &re.captures(&cargo).expect("a version line")[1];
        assert_eq!(string_field(&lock_json(), "version"), version);
    }

    #[test]
    fn test_tag_and_repo() {
        let lock = lock_json();
        assert_eq!(
            string_field(&lock, "tag"),
            format!("atl--v{}", string_field(&lock, "version"))
        );
        assert_eq!(string_field(&lock, "repo"), "rbonweb/anthology");
    }

    #[test]
    fn test_sha256_for_exactly_the_four_targets() {
        let sha = field(&lock_json(), "sha256")
            .into_object()
            .expect("an object");
        let keys: BTreeSet<&str> = sha.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, BTreeSet::from(TARGETS));
        for (target, digest) in sha.iter() {
            assert!(
                digest
                    .as_str()
                    .is_some_and(|d| digest_ok(d, NEEDS_RELEASE_HASHES)),
                "{target}"
            );
        }
    }

    #[test]
    fn test_placeholder_digest_is_accepted_only_behind_the_marker() {
        assert!(digest_ok(PLACEHOLDER_DIGEST, true));
        assert!(!digest_ok(PLACEHOLDER_DIGEST, false));
        let real = "13770cb0aa2545f899f6923df3b7f5fd360ae17ee6709ea4f47c381403e032d9";
        assert!(digest_ok(real, false));
        assert!(!digest_ok(&real.to_uppercase(), true));
    }

    // `atl_install.load_lock` is the Python loader; `anthology_core::lock::parse` is its Rust
    // equivalent (same checks, same messages).
    #[test]
    fn test_load_lock_accepts_it() {
        let loaded = lock::parse(&repo::read(LOCK), ATL_TAG_PREFIX)
            .unwrap_or_else(|e| panic!("atl.lock rejected: {e}"));
        assert_eq!(loaded.tag, string_field(&lock_json(), "tag"));
    }
}
