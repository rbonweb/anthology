//! Port of `tests/test_rtk_lock.py`: `plugins/anthology/rtk.lock` pins the released rtk binaries.

use std::collections::BTreeSet;

use anthology_core::lock::{self, RTK_TAG_PREFIX};
use anthology_pycompat::json::{self, Value};
use regex::Regex;
use repo_tests::repo;

const LOCK: &str = "plugins/anthology/rtk.lock";
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

fn lock_json() -> Value {
    json::loads(&repo::read(LOCK)).expect("rtk.lock is JSON")
}

fn field(lock: &Value, key: &str) -> Value {
    lock.as_object()
        .and_then(|o| o.get(key))
        .unwrap_or_else(|| panic!("rtk.lock has no {key}"))
        .clone()
}

fn string_field(lock: &Value, key: &str) -> String {
    field(lock, key).into_string().expect("a string")
}

mod rtk_lock_test {
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
        let cargo = repo::read("rtk/Cargo.toml");
        let re = Regex::new(r#"(?m)^version = "([^"]+)""#).unwrap();
        let version = &re.captures(&cargo).expect("a version line")[1];
        assert_eq!(string_field(&lock_json(), "version"), version);
    }

    #[test]
    fn test_tag_and_repo() {
        let lock = lock_json();
        assert_eq!(
            string_field(&lock, "tag"),
            format!("rtk--v{}", string_field(&lock, "version"))
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
        let hex = Regex::new(r"^[0-9a-f]{64}$").unwrap();
        for (target, digest) in sha.iter() {
            assert!(digest.as_str().is_some_and(|d| hex.is_match(d)), "{target}");
        }
    }

    // `rtk_install.load_lock` is the Python loader; `anthology_core::lock::parse` is its Rust
    // equivalent (same checks, same messages).
    #[test]
    fn test_load_lock_accepts_it() {
        let loaded = lock::parse(&repo::read(LOCK), RTK_TAG_PREFIX)
            .unwrap_or_else(|e| panic!("rtk.lock rejected: {e}"));
        assert_eq!(loaded.tag, string_field(&lock_json(), "tag"));
    }
}
