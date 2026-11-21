//! Licenses and NOTICE files (vendor.py `_ship_file` … `check_licenses`).
//!
//! The safety checks of the F5 review stay: an upstream license or NOTICE that is a symlink, or
//! that resolves outside the pristine checkout, is never read (it could ship a local file into
//! a public tree); a file already in the folder is never overwritten; NOTICE names match in any
//! case.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_pycompat::json::Value;

use super::compare::{exists, is_file, is_symlink, os_err};
use super::fetch::fetcher;
use super::manifest::{
    FIXED_LICENSE_NAME, LICENSES, NOTICE_NAMES, load_sources, require_valid, source_slug, vendored,
};
use super::py;
use super::{Entry, Fetch, Result, VendorError};

fn name_of(entry: &Entry) -> String {
    py::str_of(entry.get("name"))
}

/// `_ship_file(dest, name, data, entry, strict)`: write a license file to `dest/name` (mode
/// 644). A file already there is fine with the same bytes and an error when it differs; with
/// `strict` off (`diff`, before the patches) a differing file is left alone.
pub fn ship_file(dest: &Path, name: &str, data: &[u8], entry: &Entry, strict: bool) -> Result<()> {
    let target = dest.join(name);
    if is_symlink(&target) || exists(&target) {
        let same = !is_symlink(&target)
            && is_file(&target)
            && fs::read(&target).map_err(|e| os_err(&e, &target))? == data;
        if !same {
            if !strict {
                return Ok(());
            }
            return Err(VendorError(format!(
                "{}: {name} already exists in the source folder and differs from the license \
                 text to ship",
                name_of(entry)
            )));
        }
        return Ok(());
    }
    fs::write(&target, data).map_err(|e| os_err(&e, &target))?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).map_err(|e| os_err(&e, &target))
}

/// `Path.resolve()` of an existing path.
fn resolved(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `_read_upstream(entry, pristine, rel, what)`: the bytes of a license-like file of the
/// pristine checkout. A symlink, or a path that resolves outside the checkout, is refused.
pub fn read_upstream(entry: &Entry, pristine: &Path, rel: &str, what: &str) -> Result<Vec<u8>> {
    let src = pristine.join(rel);
    let outside = || {
        let (root, real) = (resolved(pristine), resolved(&src));
        !(real.starts_with(&root) && real != root)
    };
    if is_symlink(&src) || (exists(&src) && outside()) {
        return Err(VendorError(format!(
            "{}: upstream {what} {rel} is a symlink or lies outside the checkout; it is not read",
            name_of(entry)
        )));
    }
    if !is_file(&src) {
        return Err(VendorError(format!(
            "{}: upstream {what} {rel} not found",
            name_of(entry)
        )));
    }
    let data = fs::read(&src).map_err(|e| os_err(&e, &src))?;
    if py::bytes_strip(&data).is_empty() {
        return Err(VendorError(format!(
            "{}: upstream {what} {rel} is empty",
            name_of(entry)
        )));
    }
    Ok(data)
}

/// `_upstream_license(entry, pristine)`.
pub fn upstream_license(entry: &Entry, pristine: &Path) -> Result<Vec<u8>> {
    let rel = py::str_of(entry.get("license_file"));
    read_upstream(entry, pristine, &rel, "license file")
}

/// The `license_extra` paths of an entry (validated: a list of str).
fn license_extra(entry: &Entry) -> Vec<String> {
    entry
        .get("license_extra")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|v| py::str_of(Some(v))).collect())
        .unwrap_or_default()
}

/// `_notice_problems(entry, pristine, dest)`: every NOTICE in the upstream repo root or on the
/// way down to the source folder must ship — inside the folder, or listed in license_extra
/// (Apache-2.0 section 4(d)).
pub fn notice_problems(entry: &Entry, pristine: &Path, dest: &Path) -> Vec<String> {
    let folder = py::str_of(entry.get("path"));
    let parts: Vec<&str> = if folder.is_empty() {
        Vec::new()
    } else {
        folder.split('/').collect()
    };
    let listed: BTreeSet<String> = license_extra(entry).into_iter().collect();
    let mut problems = Vec::new();
    for i in 0..=parts.len() {
        let directory = parts[..i].join("/");
        let here = if directory.is_empty() {
            pristine.to_path_buf()
        } else {
            pristine.join(&directory)
        };
        let Ok(read) = fs::read_dir(&here) else {
            continue;
        };
        let mut names: Vec<Vec<u8>> = read
            .flatten()
            .map(|e| e.file_name().as_encoded_bytes().to_vec())
            .filter(|n| {
                let text = String::from_utf8_lossy(n);
                NOTICE_NAMES.contains(&py::lower(&text).as_str())
            })
            .collect();
        names.sort();
        for raw in names {
            let name = String::from_utf8_lossy(&raw).into_owned();
            let rel = if directory.is_empty() {
                name.clone()
            } else {
                format!("{directory}/{name}")
            };
            let path = here.join(OsStr::from_bytes(&raw));
            if is_symlink(&path) {
                problems.push(format!(
                    "{}: upstream {rel} is a symlink; it is not supported",
                    name_of(entry)
                ));
            } else if !is_file(&path) {
                continue;
            } else if directory == folder {
                if !is_file(&dest.join(OsStr::from_bytes(&raw))) {
                    problems.push(format!(
                        "{}: upstream {rel} is not shipped (a patch removed it)",
                        name_of(entry)
                    ));
                }
            } else if !listed.contains(&rel) {
                problems.push(format!(
                    "{}: upstream {rel} is not shipped; add it to license_extra",
                    name_of(entry)
                ));
            }
        }
    }
    problems
}

/// `ship_licenses(root, entry, pristine, dest, strict)`: put the source's license text, and any
/// upstream NOTICE, inside the built folder, unchanged. With `strict` off a license copy that
/// conflicts with a file of the folder is skipped: a patch may be what resolves it.
pub fn ship_licenses(
    root: &Path,
    entry: &Entry,
    pristine: &Path,
    dest: &Path,
    strict: bool,
) -> Result<()> {
    let folder = py::str_of(entry.get("path"));
    match entry.get("license_file") {
        None | Some(Value::Null) => {
            let fixed = root
                .join(LICENSES)
                .join(format!("{}.txt", source_slug(entry)));
            if !is_file(&fixed) {
                return Err(VendorError(format!(
                    "{}: fixed license text {LICENSES}/{}.txt not found",
                    name_of(entry),
                    source_slug(entry)
                )));
            }
            let data = fs::read(&fixed).map_err(|e| os_err(&e, &fixed))?;
            ship_file(dest, FIXED_LICENSE_NAME, &data, entry, strict)?;
        }
        Some(v) => {
            let license_file = py::str_of(Some(v));
            let prefix = if folder.is_empty() {
                String::new()
            } else {
                format!("{folder}/")
            };
            if let Some(inside) = license_file.strip_prefix(&prefix) {
                if !is_file(&dest.join(inside)) {
                    return Err(VendorError(format!(
                        "{}: license file {license_file} is not in the built folder",
                        name_of(entry)
                    )));
                }
            } else {
                let data = upstream_license(entry, pristine)?;
                ship_file(dest, py::basename(&license_file), &data, entry, strict)?;
            }
        }
    }
    for rel in license_extra(entry) {
        let data = read_upstream(entry, pristine, &rel, "license_extra file")?;
        ship_file(dest, py::basename(&rel), &data, entry, strict)?;
    }
    if py::truthy(entry.get("repo")) {
        let problems = notice_problems(entry, pristine, dest);
        if !problems.is_empty() {
            return Err(VendorError(problems.join("\n")));
        }
    }
    Ok(())
}

/// `fixed_license_problems(root, entry)`: a hand-written license text (license_file null).
pub fn fixed_license_problems(root: &Path, entry: &Entry) -> Result<Vec<String>> {
    let file = format!("{}.txt", source_slug(entry));
    let path = root.join(LICENSES).join(&file);
    let rel = format!("{LICENSES}/{file}");
    if !is_file(&path) {
        return Ok(vec![format!(
            "{rel}: missing (a fixed license text for {})",
            name_of(entry)
        )]);
    }
    let text = py::read_text(&path).map_err(VendorError)?;
    let first = if py::strip(&text).is_empty() {
        ""
    } else {
        py::splitlines(&text).first().copied().unwrap_or("")
    };
    let note = py::str_of(entry.get("license_note"));
    let mut problems = Vec::new();
    if !first.contains(&note) {
        problems.push(format!(
            "{rel}: first line must note the license is declared in the upstream {note}"
        ));
    }
    if !text.contains("License") {
        problems.push(format!("{rel}: does not contain a license text"));
    }
    Ok(problems)
}

/// `fetch_licenses(root, names, fetch)`: copy each upstream repo's license file, unchanged, to
/// `vendor/licenses/<slug>.txt`. Fixed licenses (license_file null) are never fetched.
pub fn fetch_licenses(
    root: &Path,
    names: Option<&[String]>,
    fetch: Option<&Fetch>,
) -> Result<Vec<PathBuf>> {
    let entries = load_sources(root)?;
    require_valid(&entries)?;
    let fetch = fetcher(root, fetch);
    let mut written: Vec<PathBuf> = Vec::new();
    for e in vendored(&entries)? {
        let wanted = match names {
            Some(names) if !names.is_empty() => names
                .iter()
                .any(|n| Some(n.as_str()) == e.get("name").and_then(Value::as_str)),
            _ => true,
        };
        if !wanted || !py::truthy(e.get("license_file")) {
            continue;
        }
        let dest = root.join(LICENSES).join(format!("{}.txt", source_slug(e)));
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir).map_err(|err| os_err(&err, dir))?;
        }
        let data = upstream_license(e, &fetch(e)?)?;
        fs::write(&dest, data).map_err(|err| os_err(&err, &dest))?;
        if !written.contains(&dest) {
            written.push(dest);
        }
    }
    Ok(written)
}

/// `check_licenses(root, entries, fetch)`: one problem per missing, empty or drifted license.
pub fn check_licenses(root: &Path, entries: &[Value], fetch: &Fetch) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    for e in vendored(entries)? {
        let slug = source_slug(e);
        if !seen.insert(slug.clone()) {
            continue;
        }
        let path = root.join(LICENSES).join(format!("{slug}.txt"));
        let rel = format!("{LICENSES}/{slug}.txt");
        if !py::truthy(e.get("license_file")) {
            problems.extend(fixed_license_problems(root, e)?);
        } else if !is_file(&path) {
            problems.push(format!("{rel}: missing"));
        } else {
            let have = fs::read(&path).map_err(|err| os_err(&err, &path))?;
            if py::bytes_strip(&have).is_empty() {
                problems.push(format!("{rel}: empty"));
            } else if have != upstream_license(e, &fetch(e)?)? {
                problems.push(format!(
                    "{rel}: differs from the upstream {}",
                    py::str_of(e.get("license_file"))
                ));
            }
        }
    }
    Ok(problems)
}
