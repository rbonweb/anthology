//! `vendor diff` (vendor.py `_unified`, `diff_text`, `diff_entry`): pristine + transforms (no
//! patches) against the shipped tree, as a patch `git apply -p1` takes.

use std::path::Path;

use super::build::build_tree;
use super::compare::{Snap, is_dir, show, snapshot};
use super::fetch::fetcher;
use super::manifest::{find_entry, load_sources, require_valid};
use super::py;
use super::{Fetch, Result, VendorError};

/// `_unified(rel, old, new)`: one file's unified diff (`None` is a missing side).
pub fn unified(rel: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> Result<String> {
    let binary = || {
        VendorError(format!(
            "{rel}: binary changes cannot be expressed as a patch"
        ))
    };
    let a = match old {
        None => String::new(),
        Some(b) => String::from_utf8(b.to_vec()).map_err(|_| binary())?,
    };
    let b = match new {
        None => String::new(),
        Some(b) => String::from_utf8(b.to_vec()).map_err(|_| binary())?,
    };
    let (a, b) = (py::splitlines_keepends(&a), py::splitlines_keepends(&b));
    let fromfile = if old.is_none() {
        "/dev/null".to_string()
    } else {
        format!("a/{rel}")
    };
    let tofile = if new.is_none() {
        "/dev/null".to_string()
    } else {
        format!("b/{rel}")
    };
    let mut out = String::new();
    for line in py::unified_diff_lineterm(&a, &b, &fromfile, &tofile, 3, "\n") {
        out.push_str(&line);
        if !line.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
    Ok(out)
}

/// A snapshot entry as `(exec, data)`; a link cannot be part of a patch.
fn file<'s>(rel: &str, s: Option<&'s Snap>) -> Result<Option<(bool, &'s [u8])>> {
    match s {
        None => Ok(None),
        Some(Snap::File { exec, data }) => Ok(Some((*exec, data.as_slice()))),
        Some(Snap::Link(_)) => Err(VendorError(format!(
            "{rel}: links cannot be expressed as a patch"
        ))),
    }
}

/// `diff_text(expected, actual)`: every file that differs, as one patch.
pub fn diff_text(expected: &Path, actual: &Path) -> Result<String> {
    let want = snapshot(expected)?;
    let have = snapshot(actual)?;
    let mut keys: Vec<&Vec<u8>> = want.keys().chain(have.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut chunks = String::new();
    for key in keys {
        let rel = show(key);
        let (w, h) = (want.get(key), have.get(key));
        let (w, h) = (file(&rel, w)?, file(&rel, h)?);
        if let (Some(w), Some(h)) = (w, h)
            && w.0 != h.0
        {
            return Err(VendorError(format!(
                "{rel}: mode changes cannot be expressed as a patch"
            )));
        }
        if let (None, Some(h)) = (w, h)
            && h.0
        {
            return Err(VendorError(format!(
                "{rel}: new executable files cannot be expressed as a patch"
            )));
        }
        if let (Some(w), Some(h)) = (w, h)
            && w.1 == h.1
        {
            continue;
        }
        chunks.push_str(&unified(&rel, w.map(|x| x.1), h.map(|x| x.1))?);
    }
    Ok(chunks)
}

/// `diff_entry(root, name, fetch)`: pristine + transforms (no patches) vs the shipped tree.
pub fn diff_entry(root: &Path, name: &str, fetch: Option<&Fetch>) -> Result<String> {
    let entries = load_sources(root)?;
    require_valid(&entries)?;
    let entry = find_entry(&entries, name)?;
    let pristine = fetcher(root, fetch)(entry)?;
    let tmp = tempfile::Builder::new()
        .prefix("vendor-diff-")
        .tempdir()
        .map_err(|e| VendorError(format!("cannot create a temporary directory: {e}")))?;
    let built = tmp.path().join("tree");
    build_tree(root, entry, &pristine, &built, false, false)?;
    let target = py::str_of(entry.get("target"));
    let shipped = root.join(&target);
    if !is_dir(&shipped) {
        return Err(VendorError(format!(
            "{target} does not exist; run build first"
        )));
    }
    diff_text(&built, &shipped)
}
