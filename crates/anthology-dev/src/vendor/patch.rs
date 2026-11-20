//! Patches (vendor.py `apply_patch`): `git apply -p1`, its exit status checked.

use std::path::Path;

use super::compare::{rel_posix, rglob};
use super::fetch::git;
use super::py;
use super::{Result, VendorError};

/// `apply_patch(tree, patch, reject)`: apply a patch (-p1) to `tree`. A failure is an error;
/// with `reject`, the rejected-hunk files (`*.rej`) are returned instead.
pub fn apply_patch(tree: &Path, patch: &Path, reject: bool) -> Result<Vec<String>> {
    let resolved = std::fs::canonicalize(patch).unwrap_or_else(|_| patch.to_path_buf());
    let resolved = resolved.to_string_lossy().into_owned();
    let mut args = vec!["apply", "-p1", "--whitespace=nowarn"];
    if reject {
        args.push("--reject");
    }
    args.push(&resolved);
    let result = git(&args, Some(tree))?;
    if result.code == 0 {
        return Ok(Vec::new());
    }
    let name = patch
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !reject {
        return Err(VendorError(format!(
            "patch {name} does not apply:\n{}",
            py::strip(&result.stderr)
        )));
    }
    let mut rejects: Vec<String> = rglob(tree)
        .iter()
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.as_encoded_bytes().ends_with(b".rej"))
        })
        .map(|p| rel_posix(p, tree))
        .collect();
    rejects.sort();
    if rejects.is_empty() {
        rejects.push(format!("{name}: {}", py::strip(&result.stderr)));
    }
    Ok(rejects)
}
