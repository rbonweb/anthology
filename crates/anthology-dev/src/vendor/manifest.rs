//! `vendor/sources.json`: load, save and validate (vendor.py `load_sources` … `find_entry`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use anthology_pycompat::json::{self, Value};
use regex::Regex;

use super::py;
use super::transforms::TRANSFORMS;
use super::{Entry, Result, VendorError};

pub const CACHE_DIRNAME: &str = ".vendor-cache";
pub const SOURCES: &str = "vendor/sources.json";
pub const LICENSES: &str = "vendor/licenses";
pub const SNAPSHOTS: &str = "vendor/snapshots";
pub const CHECKSUMS_NAME: &str = "CHECKSUMS.sha256";
pub const MAX_DOWNLOAD: usize = 10 * 1024 * 1024;
/// `sorted(ALLOWED_LICENSES)`.
pub const ALLOWED_LICENSES: [&str; 2] = ["Apache-2.0", "MIT"];
pub const SUBTREE: &str = "subtree";
pub const VENDORED_PLUGIN: &str = "plugins/anthology-superpowers";
pub const KNOWLEDGE_DIR: &str = "plugins/anthology/knowledge";
pub const IGNORED_EXTRAS: [&str; 1] = [".DS_Store"];
pub const PORTMAP_DIR: &str = "vendor/patches/";
/// Matched in lower case.
pub const NOTICE_NAMES: [&str; 3] = ["notice", "notice.txt", "notice.md"];
pub const FIXED_LICENSE_NAME: &str = "LICENSE";

/// A pattern compiled once. Python's `$` (without MULTILINE) also matches before one final
/// `\n`, so every `$` below is `\n?\z`.
fn cached(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap_or_else(|e| unreachable!("{pattern}: {e}")))
}

/// `SHA1.match`.
pub fn is_sha1(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, r"\A[0-9a-f]{40}\n?\z").is_match(s)
}

/// `SHA256.match`.
pub fn is_sha256(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, r"\A[0-9a-f]{64}\n?\z").is_match(s)
}

/// `NAME.match`.
pub fn is_name(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, r"\A[a-z0-9][a-z0-9-]*\n?\z").is_match(s)
}

/// `SOURCE_URL.match`.
pub fn is_source_url(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, r"\Ahttps://[A-Za-z0-9.-]+(?:/[A-Za-z0-9._~-]+)+/\n?\z").is_match(s)
}

/// The `repo` pattern of `validate_sources`.
pub fn is_repo_url(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    cached(&RE, r"\Ahttps://[A-Za-z0-9.-]+/[A-Za-z0-9._/-]+\n?\z").is_match(s)
}

/// `CHECKSUM_LINE.match(line)`: `(digest, path)`.
pub fn checksum_line(line: &str) -> Option<(&str, &str)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let caps = cached(&RE, r"\A([0-9a-f]{64})  (.+)\n?\z").captures(line)?;
    Some((caps.get(1)?.as_str(), caps.get(2)?.as_str()))
}

// ------------------------------------------------------------------------------ sources.json

/// `load_sources(root)`: the `sources` list of `vendor/sources.json`.
///
/// A non-list `sources` is iterated as Python iterates it (a dict's keys, a str's characters);
/// a non-iterable one, a missing file, bad JSON or a missing key is an error where Python raises.
pub fn load_sources(root: &Path) -> Result<Vec<Value>> {
    let path = root.join(SOURCES);
    let text = py::read_text(&path).map_err(VendorError)?;
    let mut data = json::loads(&text)
        .map_err(|e| VendorError(format!("{SOURCES}: {}: {}", e.type_name(), e.py_str(&text))))?;
    let Some(map) = data.as_object_mut() else {
        return Err(VendorError(format!(
            "{SOURCES}: the top level is not an object (TypeError)"
        )));
    };
    let Some(sources) = map.get_mut("sources") else {
        return Err(VendorError(format!("{SOURCES}: KeyError: 'sources'")));
    };
    match sources {
        Value::Array(_) => Ok(sources.take().into_array().unwrap_or_default()),
        Value::Object(m) => Ok(m.iter().map(|(k, _)| Value::Str(k.to_string())).collect()),
        Value::Str(s) => Ok(s.chars().map(|c| Value::Str(c.to_string())).collect()),
        other => Err(VendorError(format!(
            "{SOURCES}: TypeError: {} object is not iterable",
            py::repr_str(type_name(other))
        ))),
    }
}

/// Python's type name of a JSON value.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `save_sources(root, entries)`: `json.dumps({"sources": entries}, indent=2,
/// ensure_ascii=False) + "\n"`.
pub fn save_sources(root: &Path, entries: &[Value]) -> Result<()> {
    let mut top = json::Map::new();
    top.insert("sources".to_string(), Value::Array(entries.to_vec()));
    let opts = json::DumpOpts {
        ensure_ascii: false,
        ..json::INDENT2
    };
    let text = json::dumps(&Value::Object(top), &opts) + "\n";
    let path = root.join(SOURCES);
    fs::write(&path, text).map_err(|e| VendorError(py::os_error_text(&e, &path)))
}

/// `is_subtree(entry)`: managed by `git subtree`, not by this tool.
pub fn is_subtree(entry: &Value) -> bool {
    entry.as_object().is_some_and(is_subtree_entry)
}

/// `is_subtree` of an entry known to be a dict.
pub fn is_subtree_entry(entry: &Entry) -> bool {
    entry.get("managed_by").and_then(Value::as_str) == Some(SUBTREE)
}

/// `vendored(entries)`: the entries this tool builds. A non-object entry is an error, as
/// `e.get` raises AttributeError in Python.
pub fn vendored(entries: &[Value]) -> Result<Vec<&Entry>> {
    let mut out = Vec::new();
    for e in entries {
        let Some(m) = e.as_object() else {
            return Err(not_a_dict(e));
        };
        if !is_subtree(e) {
            out.push(m);
        }
    }
    Ok(out)
}

/// The AttributeError Python raises for `e.get` on a non-dict entry.
pub fn not_a_dict(e: &Value) -> VendorError {
    VendorError(format!(
        "AttributeError: '{}' object has no attribute 'get'",
        type_name(e)
    ))
}

/// `source_slug(entry)`: `vendor/licenses/<slug>.txt` — owner-repo for git sources, else the
/// entry name. Where Python raises (a non-str repo or name, a repo of only slashes) the slug is
/// built from what is there; `validate_sources` reports those entries anyway.
pub fn source_slug(entry: &Entry) -> String {
    let repo = entry.get("repo");
    let slug = if py::truthy(repo) {
        let repo = py::str_of(repo);
        let mut parts: Vec<String> = repo
            .trim_end_matches('/')
            .split('/')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        if let Some(last) = parts.last_mut()
            && last.ends_with(".git")
        {
            last.truncate(last.len() - 4);
        }
        let from = parts.len().saturating_sub(2);
        parts[from..].join("-")
    } else {
        py::str_of(entry.get("name"))
    };
    let replaced: String = slug
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    py::lower(&replaced)
}

/// `_safe_relpath(value, allow_empty)`.
pub fn safe_relpath(value: Option<&Value>, allow_empty: bool) -> bool {
    match value {
        Some(Value::Str(s)) => safe_relpath_str(s, allow_empty),
        _ => false,
    }
}

/// `_safe_relpath` of a str: no absolute path, no backslash, no `""`, `.` or `..` component.
pub fn safe_relpath_str(value: &str, allow_empty: bool) -> bool {
    if value.is_empty() {
        return allow_empty;
    }
    if value.starts_with('/') || value.contains('\\') {
        return false;
    }
    value
        .split('/')
        .all(|part| !matches!(part, "" | "." | ".."))
}

/// `_safe_target(value, subtree)`: build/sync replace the target with rmtree, so a vendored
/// target is exactly the vendored plugin or one folder of the knowledge directory.
pub fn safe_target(value: Option<&Value>, subtree: bool) -> bool {
    if !safe_relpath(value, false) {
        return false;
    }
    if subtree {
        return true;
    }
    let Some(value) = value.and_then(Value::as_str) else {
        return false;
    };
    if value == VENDORED_PLUGIN {
        return true;
    }
    match value
        .strip_prefix(KNOWLEDGE_DIR)
        .and_then(|r| r.strip_prefix('/'))
    {
        Some(folder) => is_name(folder),
        None => false,
    }
}

/// `_safe_portmap(value)`.
pub fn safe_portmap(value: Option<&Value>) -> bool {
    safe_relpath(value, false)
        && value
            .and_then(Value::as_str)
            .is_some_and(|v| v.starts_with(PORTMAP_DIR) && v.ends_with(".json"))
}

fn check_transforms(label: &str, entry: &Entry, errors: &mut Vec<String>) {
    let Some(Value::Array(transforms)) = entry.get("transform") else {
        errors.push(format!("{label}: transform must be a list"));
        return;
    };
    for t in transforms {
        let wrapped;
        let t: &json::Map = match t {
            Value::Str(s) => {
                let mut m = json::Map::new();
                m.insert("name".to_string(), Value::Str(s.clone()));
                wrapped = m;
                &wrapped
            }
            Value::Object(m) => m,
            other => {
                errors.push(format!("{label}: bad transform {}", py::repr(other)));
                continue;
            }
        };
        let name = t.get("name");
        if t.contains_key("patch") {
            if !safe_relpath(t.get("patch"), false) {
                errors.push(format!(
                    "{label}: bad patch path {}",
                    py::repr_opt(t.get("patch"))
                ));
            }
        } else if !name
            .and_then(Value::as_str)
            .is_some_and(|n| TRANSFORMS.contains(&n))
        {
            // An unhashable name (a list or dict) raises TypeError in Python (D-1).
            errors.push(format!("{label}: unknown transform {}", py::repr_opt(name)));
        } else if name.and_then(Value::as_str) == Some("rename-plugin") {
            let ok = |k: &str| matches!(t.get(k), Some(Value::Str(s)) if !s.is_empty());
            if !(ok("old") && ok("new")) {
                errors.push(format!("{label}: rename-plugin needs old and new"));
            }
            if !py::truthy(entry.get("plugin_version")) {
                errors.push(format!("{label}: rename-plugin needs plugin_version"));
            }
        } else if name.and_then(Value::as_str) == Some("rename-skill") {
            let ok = |k: &str| matches!(t.get(k), Some(Value::Str(s)) if is_name(s));
            if !(ok("old") && ok("new")) {
                errors.push(format!(
                    "{label}: rename-skill needs old and new skill names"
                ));
            }
        } else if name.and_then(Value::as_str) == Some("drop-paths") {
            let ok = matches!(
                t.get("paths"),
                Some(Value::Array(items)) if !items.is_empty()
                    && items.iter().all(|v| safe_relpath(Some(v), false))
            );
            if !ok {
                errors.push(format!(
                    "{label}: drop-paths needs a non-empty list of paths inside the source"
                ));
            }
        }
    }
}

/// `_check_license_extra`: upstream files (NOTICE, a second license) shipped unchanged next to
/// the source folder, as `<target>/<basename>`.
fn check_license_extra(label: &str, entry: &Entry, errors: &mut Vec<String>) {
    if py::truthy(entry.get("url")) {
        errors.push(format!(
            "{label}: a url source has no license_extra (its snapshot is the folder)"
        ));
        return;
    }
    let extra = match entry.get("license_extra") {
        Some(Value::Array(items)) if items.iter().all(|v| safe_relpath(Some(v), false)) => items,
        _ => {
            errors.push(format!(
                "{label}: license_extra must be a list of upstream repo-relative paths"
            ));
            return;
        }
    };
    let mut taken = BTreeSet::new();
    taken.insert(match entry.get("license_file") {
        Some(Value::Str(s)) => py::basename(s).to_string(),
        _ => FIXED_LICENSE_NAME.to_string(),
    });
    for value in extra {
        let Some(s) = value.as_str() else { continue };
        let base = py::basename(s).to_string();
        if taken.contains(&base) {
            errors.push(format!(
                "{label}: license_extra {} clashes with another shipped license file named {base}",
                py::repr_str(s)
            ));
        }
        taken.insert(base);
    }
}

/// `_check_url_entry`: a downloaded skill folder.
fn check_url_entry(label: &str, entry: &Entry, errors: &mut Vec<String>) {
    let url_ok = match entry.get("url") {
        Some(Value::Str(url)) => {
            is_source_url(url)
                && url
                    .split('/')
                    .skip(3)
                    .all(|part| part != "." && part != "..")
        }
        _ => false,
    };
    if !url_ok {
        errors.push(format!(
            "{label}: url must be an https URL ending in a slash"
        ));
    }
    if py::truthy(entry.get("repo")) || !is_none(entry.get("commit")) {
        errors.push(format!("{label}: a url source has no repo or commit"));
    }
    if !entry.contains_key("checksum") {
        errors.push(format!("{label}: a url source needs a checksum"));
    }
    if entry.get("path").and_then(Value::as_str) != Some("") {
        errors.push(format!("{label}: a url source has an empty path"));
    }
    if !safe_relpath(entry.get("license_file"), false) {
        errors.push(format!(
            "{label}: a url source needs a license_file inside the snapshot"
        ));
    }
}

/// `e.get(k) is None` (a missing key or a JSON null).
fn is_none(v: Option<&Value>) -> bool {
    v.is_none_or(Value::is_null)
}

/// `validate_sources(entries)`: every problem, in vendor.py's order (empty when valid).
pub fn validate_sources(entries: &[Value]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut targets: Vec<(String, String)> = Vec::new();
    let mut license_files: BTreeMap<String, Option<Value>> = BTreeMap::new();
    for (i, e) in entries.iter().enumerate() {
        let Some(e) = e.as_object() else {
            errors.push(format!("entry {i}: not an object"));
            continue;
        };
        let label = if e.contains_key("name") {
            py::str_of(e.get("name"))
        } else {
            format!("entry {i}")
        };
        match e.get("name") {
            Some(Value::Str(name)) if is_name(name) => {
                if names.contains(name) {
                    errors.push(format!("{label}: duplicate name"));
                }
                names.insert(name.clone());
            }
            _ => errors.push(format!(
                "{label}: name must be lowercase letters, digits and dashes"
            )),
        }
        let license_ok = e
            .get("license")
            .and_then(Value::as_str)
            .is_some_and(|l| ALLOWED_LICENSES.contains(&l));
        if !license_ok {
            errors.push(format!(
                "{label}: license {} is not in {}; the owner must decide",
                py::repr_opt(e.get("license")),
                py::repr_str_list(&ALLOWED_LICENSES)
            ));
        }
        let subtree = is_subtree_entry(e);
        if !safe_target(e.get("target"), subtree) {
            errors.push(format!(
                "{label}: target must be {VENDORED_PLUGIN} or {KNOWLEDGE_DIR}/<name> (build \
                 deletes and recreates it)"
            ));
        } else if let Some(t) = e.get("target").and_then(Value::as_str) {
            targets.push((label.clone(), t.trim_end_matches('/').to_string()));
        }
        if subtree {
            continue;
        }
        for field in ["path", "ref", "transform"] {
            if !e.contains_key(field) {
                errors.push(format!("{label}: missing {field}"));
            }
        }
        if e.contains_key("path") && !safe_relpath(e.get("path"), true) {
            errors.push(format!("{label}: path must stay inside the upstream repo"));
        }
        let has_repo = py::truthy(e.get("repo"));
        if !(has_repo || py::truthy(e.get("url"))) {
            errors.push(format!("{label}: needs repo or url"));
        }
        // A truthy non-str repo raises TypeError in `re.match` (D-1): reported as invalid.
        if has_repo
            && !e
                .get("repo")
                .and_then(Value::as_str)
                .is_some_and(is_repo_url)
        {
            errors.push(format!("{label}: repo must be an https URL"));
        }
        let (commit, checksum) = (e.get("commit"), e.get("checksum"));
        if !is_none(commit) && !commit.and_then(Value::as_str).is_some_and(is_sha1) {
            errors.push(format!(
                "{label}: commit must be 40 lowercase hex characters"
            ));
        } else if !is_none(checksum) && !checksum.and_then(Value::as_str).is_some_and(is_sha256) {
            errors.push(format!(
                "{label}: checksum must be 64 lowercase hex characters"
            ));
        } else if is_none(commit) && is_none(checksum) {
            errors.push(format!("{label}: needs a 40-hex commit or a checksum"));
        }
        if has_repo {
            match e.get("license_file") {
                None => errors.push(format!(
                    "{label}: missing license_file (null needs a license_note)"
                )),
                Some(Value::Null) => {
                    if !py::truthy(e.get("license_note")) {
                        errors.push(format!("{label}: license_file null needs a license_note"));
                    }
                }
                Some(v) => {
                    if !safe_relpath(Some(v), false) {
                        errors.push(format!("{label}: bad license_file"));
                    }
                }
            }
            let slug = source_slug(e);
            let mine = e.get("license_file");
            match license_files.get(&slug) {
                Some(first) => {
                    if !py::py_eq(first.as_ref(), mine) {
                        errors.push(format!(
                            "{label}: entries from one repo must agree on license_file"
                        ));
                    }
                }
                None => {
                    license_files.insert(slug, mine.cloned());
                }
            }
        }
        if e.contains_key("license_extra") {
            check_license_extra(&label, e, &mut errors);
        }
        if e.contains_key("url") {
            check_url_entry(&label, e, &mut errors);
        }
        check_transforms(&label, e, &mut errors);
        if e.contains_key("portmap") && !safe_portmap(e.get("portmap")) {
            errors.push(format!(
                "{label}: portmap must be a .json file under {PORTMAP_DIR}"
            ));
        }
    }
    for (i, (a_label, a)) in targets.iter().enumerate() {
        for (b_label, b) in &targets[i + 1..] {
            if a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/")) {
                errors.push(format!("{a_label}: target overlaps {b_label}"));
            }
        }
    }
    errors
}

/// `_require_valid(entries)`.
pub fn require_valid(entries: &[Value]) -> Result<()> {
    let errors = validate_sources(entries);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(VendorError(format!(
            "vendor/sources.json is invalid:\n  {}",
            errors.join("\n  ")
        )))
    }
}

/// The index of `find_entry(entries, name)`.
pub fn find_entry_index(entries: &[Value], name: &str) -> Result<usize> {
    for (i, e) in entries.iter().enumerate() {
        let Some(m) = e.as_object() else {
            return Err(not_a_dict(e));
        };
        if m.get("name").and_then(Value::as_str) == Some(name) {
            if is_subtree(e) {
                return Err(VendorError(format!(
                    "{name} is managed_by subtree, not by vendor.py"
                )));
            }
            return Ok(i);
        }
    }
    Err(VendorError(format!("no such source: {name}")))
}

/// `find_entry(entries, name)`.
pub fn find_entry<'a>(entries: &'a [Value], name: &str) -> Result<&'a Entry> {
    let i = find_entry_index(entries, name)?;
    entries[i]
        .as_object()
        .ok_or_else(|| not_a_dict(&entries[i]))
}
