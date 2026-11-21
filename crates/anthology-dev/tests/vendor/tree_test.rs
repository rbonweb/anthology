//! Tree helpers beyond the Python tests (vendor.py semantics; no ledger rows).

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;

use anthology_dev::vendor;

use crate::fixture::Fixture;

/// Creates `dir/<name bytes>`; `false` where the file system refuses non-UTF-8 names (APFS).
fn create_raw(dir: &std::path::Path, name: &[u8]) -> bool {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(OsStr::from_bytes(name)), b"x").is_ok()
}

#[test]
fn non_utf8_names_stay_distinct_and_sort_by_raw_bytes() {
    // Python keys snapshots by surrogate-escaped names: `a\xff` and `a\xfe` differ, and
    // `compare_trees` reports the missing and the extra files (controller ruling (b): raw-byte
    // order).
    let f = Fixture::new();
    let (want, have) = (f.tmp.join("want"), f.tmp.join("have"));
    if !create_raw(&want, b"a\xff") || !create_raw(&have, b"a\xfe") {
        eprintln!("skipped: this file system refuses non-UTF-8 names");
        return;
    }
    assert!(create_raw(&have, b"a\xfd"));
    let problems = vendor::compare_trees(&want, &have, "t").unwrap();
    assert_eq!(
        problems,
        vec![
            "t: extra file not in the build: a\u{fffd}".to_string(),
            "t: extra file not in the build: a\u{fffd}".to_string(),
            "t: missing from the committed tree: a\u{fffd}".to_string(),
        ]
    );
    let keys: Vec<Vec<u8>> = vendor::snapshot(&have).unwrap().into_keys().collect();
    assert_eq!(keys, vec![b"a\xfd".to_vec(), b"a\xfe".to_vec()]);
}
