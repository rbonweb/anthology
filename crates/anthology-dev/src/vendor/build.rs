//! Building (vendor.py `build_tree`, `install_tree`, `build`): pristine → transforms →
//! patches → licenses (→ port map check), written to the target.

use std::fs;
use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Value};

use super::compare::{copy_tree, is_dir, is_file, is_symlink, os_err};
use super::fetch::fetcher;
use super::licenses::ship_licenses;
use super::manifest::{find_entry, load_sources, require_valid};
use super::patch::apply_patch;
use super::portmap::{load_portmap, portmap_problems};
use super::py;
use super::transforms::run_transform;
use super::{Entry, Fetch, Result, VendorError};

/// What `build_tree` returns: each transform's stats, the patch rejects (`reject` mode) and,
/// for an entry with a port map, its problems.
#[derive(Clone, Debug, Default)]
pub struct BuildStats {
    pub transforms: Vec<(String, json::Map)>,
    pub rejects: Vec<String>,
    pub portmap: Option<Vec<String>>,
}

impl BuildStats {
    /// `", ".join(f"{t}: {s}" for t, s in stats["transforms"]) or "no transforms"`.
    pub fn detail(&self) -> String {
        if self.transforms.is_empty() {
            return "no transforms".to_string();
        }
        self.transforms
            .iter()
            .map(|(t, s)| format!("{t}: {}", py::repr(&Value::Object(s.clone()))))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `_pristine_subtree(entry, pristine)`: the resolved source folder, which must be a directory
/// inside the checkout.
pub fn pristine_subtree(entry: &Entry, pristine: &Path) -> Result<PathBuf> {
    let path = py::str_of(entry.get("path"));
    let bad = || {
        VendorError(format!(
            "{}: path {} is not a directory upstream",
            py::str_of(entry.get("name")),
            py::repr_opt(entry.get("path"))
        ))
    };
    let src = fs::canonicalize(pristine.join(&path)).map_err(|_| bad())?;
    let root = fs::canonicalize(pristine).map_err(|_| bad())?;
    if !is_dir(&src) || !src.starts_with(&root) {
        return Err(bad());
    }
    Ok(src)
}

/// `build_tree(root, entry, pristine, dest, patches, reject)`.
pub fn build_tree(
    root: &Path,
    entry: &Entry,
    pristine: &Path,
    dest: &Path,
    patches: bool,
    reject: bool,
) -> Result<BuildStats> {
    let mut stats = BuildStats::default();
    copy_tree(&pristine_subtree(entry, pristine)?, dest)?;
    let empty = Vec::new();
    let transforms = entry
        .get("transform")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    for t in transforms {
        let spec = match t {
            Value::Str(s) => {
                let mut m = json::Map::new();
                m.insert("name".to_string(), Value::Str(s.clone()));
                m
            }
            other => other.as_object().cloned().unwrap_or_default(),
        };
        if spec.contains_key("patch") {
            if patches {
                let rel = py::str_of(spec.get("patch"));
                let patch = root.join(&rel);
                if !is_file(&patch) {
                    return Err(VendorError(format!(
                        "{}: patch {rel} not found",
                        py::str_of(entry.get("name"))
                    )));
                }
                stats.rejects.extend(apply_patch(dest, &patch, reject)?);
            }
        } else {
            let name = py::str_of(spec.get("name"));
            let result = run_transform(dest, entry, &spec)?;
            stats.transforms.push((name, result));
        }
    }
    ship_licenses(root, entry, pristine, dest, patches)?;
    if patches && py::truthy(entry.get("portmap")) {
        let portmap = load_portmap(root, entry)?;
        stats.portmap = Some(portmap_problems(
            &portmap,
            &pristine_subtree(entry, pristine)?,
            dest,
        )?);
    }
    Ok(stats)
}

/// `install_tree(src, dest)`: replace `dest` with a copy of `src`. Like `shutil.rmtree`, a
/// symlink or a file at `dest` is refused, and nothing is followed when deleting.
pub fn install_tree(src: &Path, dest: &Path) -> Result<()> {
    if is_symlink(dest) {
        return Err(VendorError(format!(
            "Cannot call rmtree on a symbolic link: {}",
            py::repr_str(&dest.to_string_lossy())
        )));
    }
    if dest.exists() {
        if !is_dir(dest) {
            return Err(VendorError(format!(
                "[Errno 20] Not a directory: {}",
                py::repr_str(&dest.to_string_lossy())
            )));
        }
        fs::remove_dir_all(dest).map_err(|e| os_err(&e, dest))?;
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| os_err(&e, parent))?;
    }
    copy_tree(src, dest)
}

/// `build(root, name, out, fetch, reject)`: build one source into `<out or root>/<target>`.
pub fn build(
    root: &Path,
    name: &str,
    out: Option<&Path>,
    fetch: Option<&Fetch>,
    reject: bool,
) -> Result<BuildStats> {
    let entries = load_sources(root)?;
    require_valid(&entries)?;
    let entry = find_entry(&entries, name)?;
    let pristine = fetcher(root, fetch)(entry)?;
    let tmp = tempfile::Builder::new()
        .prefix("vendor-build-")
        .tempdir()
        .map_err(|e| VendorError(format!("cannot create a temporary directory: {e}")))?;
    let built = tmp.path().join("tree");
    let stats = build_tree(root, entry, &pristine, &built, true, reject)?;
    if let Some(problems) = stats.portmap.as_ref().filter(|p| !p.is_empty()) {
        return Err(VendorError(format!(
            "{name}: the port map is not satisfied:\n  {}",
            problems.join("\n  ")
        )));
    }
    let target = py::str_of(entry.get("target"));
    install_tree(&built, &out.unwrap_or(root).join(target))?;
    Ok(stats)
}
