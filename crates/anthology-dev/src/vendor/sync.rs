//! `vendor sync` (vendor.py `sync`): re-pin a source to a ref, rebuild its target and refresh
//! its license. Patch rejects and port map problems are reported, not fatal. Never commits.

use std::path::Path;

use anthology_pycompat::json::{self, Value};

use super::build::{build_tree, install_tree};
use super::compare::is_file;
use super::fetch::{fetcher, git_resolve_ref};
use super::licenses::fetch_licenses;
use super::manifest::{find_entry_index, is_sha1, load_sources, require_valid, save_sources};
use super::py;
use super::{Entry, Fetch, Result, VendorError};

/// `resolve(entry, ref) -> commit` (vendor.py's `git_resolve_ref` by default).
pub type Resolve<'a> = dyn Fn(&Entry, &str) -> Result<String> + 'a;

/// What `sync` returns.
#[derive(Clone, Debug)]
pub struct SyncReport {
    pub name: String,
    /// The old `ref` and `commit`, as they were in sources.json.
    pub old_ref: Value,
    pub old_commit: Value,
    pub new_ref: String,
    pub new_commit: String,
    pub transforms: Vec<(String, json::Map)>,
    pub rejects: Vec<String>,
    pub portmap: Vec<String>,
}

/// `sync(root, name, ref, fetch, resolve)`.
pub fn sync(
    root: &Path,
    name: &str,
    git_ref: &str,
    fetch: Option<&Fetch>,
    resolve: Option<&Resolve>,
) -> Result<SyncReport> {
    let mut entries = load_sources(root)?;
    require_valid(&entries)?;
    let index = find_entry_index(&entries, name)?;
    let entry = entries[index]
        .as_object()
        .cloned()
        .ok_or_else(|| VendorError(format!("{name}: not an object")))?;
    if py::truthy(entry.get("url")) {
        return Err(VendorError(format!(
            "{name} is a url source and has no ref to sync: update its checksum in \
             vendor/sources.json, then run `vendor.py fetch {name}`"
        )));
    }
    let commit = if is_sha1(git_ref) {
        git_ref.to_string()
    } else {
        match resolve {
            Some(r) => r(&entry, git_ref)?,
            None => git_resolve_ref(&entry, git_ref)?,
        }
    };
    let key_error = |k: &str| VendorError(format!("KeyError: {}", py::repr_str(k)));
    let old_ref = entry.get("ref").cloned().ok_or_else(|| key_error("ref"))?;
    let old_commit = entry
        .get("commit")
        .cloned()
        .ok_or_else(|| key_error("commit"))?;
    let mut entry = entry;
    entry.insert("ref".to_string(), Value::Str(git_ref.to_string()));
    entry.insert("commit".to_string(), Value::Str(commit.clone()));
    entries[index] = Value::Object(entry.clone());
    require_valid(&entries)?;
    let fetch = fetcher(root, fetch);
    let pristine = fetch(&entry)?;
    let manifest = pristine.join(".claude-plugin").join("plugin.json");
    if entry.contains_key("upstream_version") && is_file(&manifest) {
        let text = py::read_text(&manifest).map_err(VendorError)?;
        let data = json::loads(&text)
            .map_err(|e| VendorError(format!("{}: plugin.json: {}", name, e.py_str(&text))))?;
        let Some(data) = data.as_object() else {
            return Err(VendorError(format!(
                "{name}: plugin.json is not an object (AttributeError)"
            )));
        };
        if let Some(version) = data.get("version") {
            entry.insert("upstream_version".to_string(), version.clone());
            entries[index] = Value::Object(entry.clone());
        }
    }
    let stats = {
        let tmp = tempfile::Builder::new()
            .prefix("vendor-sync-")
            .tempdir()
            .map_err(|e| VendorError(format!("cannot create a temporary directory: {e}")))?;
        let built = tmp.path().join("tree");
        let stats = build_tree(root, &entry, &pristine, &built, true, true)?;
        install_tree(&built, &root.join(py::str_of(entry.get("target"))))?;
        stats
    };
    save_sources(root, &entries)?;
    fetch_licenses(root, Some(&[name.to_string()]), Some(&*fetch))?;
    Ok(SyncReport {
        name: name.to_string(),
        old_ref,
        old_commit,
        new_ref: git_ref.to_string(),
        new_commit: commit,
        transforms: stats.transforms,
        rejects: stats.rejects,
        portmap: stats.portmap.unwrap_or_default(),
    })
}
