//! `vendor set-rtk-upstream --ref R --commit C`: re-pin the `rtk` subtree's `upstream` in
//! `vendor/sources.json`. It replaces the inline Python in `scripts/sync-rtk.sh`, which:
//! 1. printed the old pin as `<ref> <commit>`;
//! 2. set `upstream.ref` and `upstream.commit`;
//! 3. rewrote the whole file as `json.dumps(data, indent=2, ensure_ascii=False) + "\n"` (the
//!    same layout as `save_sources`).
//!
//! The old pin line on stdout is a contract with `sync-rtk.sh` (DV2). Where the inline Python
//! died with a traceback (no rtk entry, no `upstream` object), this is a VendorError.

use std::fs;
use std::path::Path;

use anthology_pycompat::json::{self, Value};

use super::manifest::SOURCES;
use super::py;
use super::{Result, VendorError};

/// Sets the rtk entry's `upstream.ref` and `upstream.commit`; returns the old pin,
/// `"<ref> <commit>"` (each as Python's `str()` of the JSON value).
pub fn set_rtk_upstream(root: &Path, git_ref: &str, commit: &str) -> Result<String> {
    let path = root.join(SOURCES);
    let text = py::read_text(&path).map_err(VendorError)?;
    let mut data =
        json::loads(&text).map_err(|e| VendorError(format!("{SOURCES}: {}", e.py_str(&text))))?;
    let no_entry = || VendorError(format!("{SOURCES} has no rtk entry"));
    let sources = data
        .as_object_mut()
        .and_then(|m| m.get_mut("sources"))
        .and_then(Value::as_array_mut)
        .ok_or_else(no_entry)?;
    // `next(e for e in data["sources"] if e.get("name") == "rtk")`: a non-object entry before
    // it fails, as `e.get` does.
    let mut found = None;
    for e in sources.iter_mut() {
        let Some(e) = e.as_object_mut() else {
            return Err(VendorError(format!(
                "{SOURCES}: an entry before rtk is not an object"
            )));
        };
        if e.get("name").and_then(Value::as_str) == Some("rtk") {
            found = Some(e);
            break;
        }
    }
    let entry = found.ok_or_else(no_entry)?;
    let bad_upstream = || {
        VendorError(format!(
            "the rtk entry in {SOURCES} has no upstream object with a ref and a commit"
        ))
    };
    let upstream = entry
        .get_mut("upstream")
        .and_then(Value::as_object_mut)
        .ok_or_else(bad_upstream)?;
    let (Some(old_ref), Some(old_commit)) = (upstream.get("ref"), upstream.get("commit")) else {
        return Err(bad_upstream());
    };
    let old = format!(
        "{} {}",
        py::str_of(Some(old_ref)),
        py::str_of(Some(old_commit))
    );
    upstream.insert("ref".to_string(), Value::Str(git_ref.to_string()));
    upstream.insert("commit".to_string(), Value::Str(commit.to_string()));
    let opts = json::DumpOpts {
        ensure_ascii: false,
        ..json::INDENT2
    };
    let out = json::dumps(&data, &opts) + "\n";
    fs::write(&path, out).map_err(|e| VendorError(py::os_error_text(&e, &path)))?;
    Ok(old)
}
