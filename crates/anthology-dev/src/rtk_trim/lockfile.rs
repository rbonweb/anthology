//! `--lock-guard`: the lockfile step may only prune packages (scripts/rtk_trim.py `lock_guard`),
//! plus the Rust-only allowlist for path dependencies on `../crates/` workspace crates.
//!
//! The allowlist reads `<root>/Cargo.toml` by line scanning, without a TOML crate. Its limits:
//! a dependency entry is recognised on one line (`name = { path = "../crates/x", .. }`, or the
//! dotted `name.path = ".."`) in any table whose last name segment contains `dependencies`
//! (`[dependencies]`, `[dev-dependencies]`, `[target.'cfg(..)'.dependencies]`,
//! `[workspace.dependencies]`), or as `path = ".."` under `[<..dependencies>.NAME]`. Comments
//! after a value and quoted keys are handled; multi-line values, escapes inside strings and
//! `package = ".."` renames are not (the allowed name comes from the path crate's own manifest
//! anyway). A line the scanner does not understand allows nothing: the guard only gets stricter.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use super::{TrimError, io_error};

/// A `(name, version)` pair of a lockfile.
pub type Pair = (String, String);

/// The path prefix a dependency must have to allow its package.
pub const ALLOWED_PREFIX: &str = "../crates/";

/// Python's text mode: `\r\n` and a lone `\r` are `\n`.
fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn read_text(path: &Path) -> Result<String, TrimError> {
    fs::read_to_string(path).map_err(|e| io_error(&path.display().to_string(), &e))
}

/// `re.match(r'^(name|version) = "([^"]*)"$', line)`: the key and the value.
fn name_or_version(line: &str) -> Option<(&'static str, &str)> {
    for key in ["name", "version"] {
        let Some(rest) = line.strip_prefix(key).and_then(|r| r.strip_prefix(" = \"")) else {
            continue;
        };
        if let Some(value) = rest.strip_suffix('"')
            && !value.contains('"')
        {
            return Some((key, value));
        }
    }
    None
}

/// The `(name, version)` pairs of the `[[package]]` entries of a lockfile text.
pub fn lock_packages_text(text: &str) -> BTreeSet<Pair> {
    let mut pairs = BTreeSet::new();
    let mut name: Option<String> = None;
    let mut in_package = false;
    for line in universal_newlines(text).split('\n') {
        if line.starts_with('[') {
            in_package = line == "[[package]]";
            name = None;
            continue;
        }
        if !in_package {
            continue;
        }
        match name_or_version(line) {
            Some(("name", value)) => name = Some(value.to_string()),
            Some((_, value)) => {
                if let Some(n) = name.take() {
                    pairs.insert((n, value.to_string()));
                }
            }
            None => {}
        }
    }
    pairs
}

/// The `(name, version)` pairs of the `[[package]]` entries of the Cargo.lock at `path`.
pub fn lock_packages(path: &Path) -> Result<BTreeSet<Pair>, TrimError> {
    Ok(lock_packages_text(&read_text(path)?))
}

/// What the guard found: `added` are the allowlisted additions, `pruned` the removals.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct GuardReport {
    pub added: Vec<Pair>,
    pub pruned: Vec<Pair>,
}

fn join_pairs(pairs: &[Pair]) -> String {
    pairs
        .iter()
        .map(|(n, v)| format!("{n} {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Fails unless the packages of `after` are a subset of those of `before` (the lockfile step may
/// only prune); returns the removed packages, sorted. Python's `lock_guard`.
pub fn lock_guard(before: &Path, after: &Path) -> Result<Vec<Pair>, TrimError> {
    let before_text = read_text(before)?;
    let report = guard(&before_text, &read_text(after)?, BTreeSet::new)?;
    Ok(report.pruned)
}

/// [`lock_guard`] with the allowlist of `root` (see the module note): an added pair passes when
/// it is a `../crates/` path package of `<root>/Cargo.toml` or in its dependency closure in the
/// `after` lockfile.
pub fn lock_guard_allowlist(
    before: &Path,
    after: &Path,
    root: &Path,
) -> Result<GuardReport, TrimError> {
    // Python reads BEFORE first.
    let before_text = read_text(before)?;
    let after_text = read_text(after)?;
    guard(&before_text, &after_text, || {
        allowed_packages(root, &after_text)
    })
}

fn guard(
    before: &str,
    after: &str,
    allowed: impl FnOnce() -> BTreeSet<Pair>,
) -> Result<GuardReport, TrimError> {
    let old = lock_packages_text(before);
    let new = lock_packages_text(after);
    let added: Vec<Pair> = new.difference(&old).cloned().collect();
    let allowed = if added.is_empty() {
        BTreeSet::new()
    } else {
        allowed()
    };
    let (ok, bad): (Vec<Pair>, Vec<Pair>) = added.into_iter().partition(|p| allowed.contains(p));
    if !bad.is_empty() {
        return Err(TrimError(format!(
            "Cargo.lock: the lockfile step added or changed packages: {}",
            join_pairs(&bad)
        )));
    }
    Ok(GuardReport {
        added: ok,
        pruned: old.difference(&new).cloned().collect(),
    })
}

/// The stdout lines of a successful guard: the allowlisted additions (only when there are some),
/// then the pruned line (only when something was pruned).
pub fn report_lines(report: &GuardReport) -> Vec<String> {
    let mut lines = Vec::new();
    if !report.added.is_empty() {
        lines.push(format!(
            "rtk-trim: Cargo.lock: {} packages added by the {ALLOWED_PREFIX} allowlist: {}",
            report.added.len(),
            join_pairs(&report.added)
        ));
    }
    if !report.pruned.is_empty() {
        lines.push(format!(
            "rtk-trim: Cargo.lock: {} packages pruned, none added or changed: {}",
            report.pruned.len(),
            join_pairs(&report.pruned)
        ));
    }
    lines
}

// ------------------------------------------------------------------------------------------
// The allowlist: path dependencies of <root>/Cargo.toml

/// The text of a quoted TOML string at the start of `s` (after trimming), and what follows it.
fn quoted(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let quote = s.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let body = &s[1..];
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '\\' && quote == '"' {
            chars.next();
        } else if c == quote {
            return Some((&body[..i], &body[i + 1..]));
        }
    }
    None
}

/// `line` without a trailing `# comment` (a `#` inside a string does not count).
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut chars = line.char_indices();
    while let Some((i, c)) = chars.next() {
        match quote {
            Some(q) if c == '\\' && q == '"' => {
                chars.next();
            }
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' => return &line[..i],
            None => {}
        }
    }
    line
}

/// A table name split at the dots outside quotes, quotes removed.
fn key_segments(key: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in key.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '.' => {
                segments.push(current.trim().to_string());
                current.clear();
            }
            None => current.push(c),
        }
    }
    segments.push(current.trim().to_string());
    segments
}

/// The `path` of an inline table body (`{ version = "1", path = "../crates/x" }`).
fn inline_table_path(value: &str) -> Option<String> {
    let body = value.trim().strip_prefix('{')?;
    let body = &body[..body.rfind('}')?];
    // Split at the commas outside strings and brackets.
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    let mut quote: Option<char> = None;
    for (i, c) in body.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '[' || c == '{' => depth += 1,
            None if c == ']' || c == '}' => depth = depth.saturating_sub(1),
            None if c == ',' && depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            None => {}
        }
    }
    parts.push(&body[start..]);
    parts.into_iter().find_map(|part| {
        let (key, value) = part.split_once('=')?;
        if key_segments(key) != ["path"] {
            return None;
        }
        quoted(value).map(|(path, _)| path.to_string())
    })
}

/// The `(dependency key, path)` pairs of a manifest text, as far as the module note says.
fn path_dependencies(manifest: &str) -> Vec<(String, String)> {
    #[derive(PartialEq)]
    enum Table {
        Other,
        /// `[dependencies]`: one dependency per line.
        List,
        /// `[dependencies.NAME]`: the keys of one dependency.
        Entry(String),
    }
    let mut found = Vec::new();
    let mut table = Table::Other;
    for raw in universal_newlines(manifest).split('\n') {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let single = !header.starts_with('[');
            let inner = header.trim_end_matches(']');
            let segments = key_segments(inner);
            table = if !single {
                Table::Other
            } else if segments.last().is_some_and(|s| s.contains("dependencies")) {
                Table::List
            } else if segments.len() >= 2 && segments[segments.len() - 2].contains("dependencies") {
                Table::Entry(segments[segments.len() - 1].clone())
            } else {
                Table::Other
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match &table {
            Table::Other => {}
            Table::Entry(name) => {
                if key_segments(key) == ["path"]
                    && let Some((path, _)) = quoted(value)
                {
                    found.push((name.clone(), path.to_string()));
                }
            }
            Table::List => {
                let key = key_segments(key);
                if key.len() == 2 && key[1] == "path" {
                    if let Some((path, _)) = quoted(value) {
                        found.push((key[0].clone(), path.to_string()));
                    }
                } else if key.len() == 1
                    && let Some(path) = inline_table_path(value)
                {
                    found.push((key[0].clone(), path));
                }
            }
        }
    }
    found
}

/// The `name` of the `[package]` table of a manifest text.
fn package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    for raw in universal_newlines(manifest).split('\n') {
        let line = strip_comment(raw).trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            return quoted(value).map(|(name, _)| name.to_string());
        }
    }
    None
}

/// A `path` that stays under `../crates/`: the prefix, a non-empty rest, no `..` or `.` segment.
fn under_crates(path: &str) -> bool {
    path.strip_prefix(ALLOWED_PREFIX).is_some_and(|rest| {
        rest.split('/')
            .next()
            .is_some_and(|first| !first.is_empty())
            && rest.split('/').all(|s| s != ".." && s != ".")
    })
}

/// The names of the path packages `<root>/Cargo.toml` depends on under `../crates/`. A crate
/// whose manifest cannot be read allows nothing.
fn path_package_names(root: &Path) -> BTreeSet<String> {
    let Ok(manifest) = fs::read_to_string(root.join("Cargo.toml")) else {
        return BTreeSet::new();
    };
    let mut names = BTreeSet::new();
    for (key, path) in path_dependencies(&manifest) {
        if !under_crates(&path) {
            continue;
        }
        let Ok(crate_manifest) = fs::read_to_string(root.join(&path).join("Cargo.toml")) else {
            continue;
        };
        names.insert(package_name(&crate_manifest).unwrap_or(key));
    }
    names
}

/// One `[[package]]` of a lockfile with what the closure needs.
struct LockEntry {
    name: String,
    version: String,
    has_source: bool,
    dependencies: Vec<String>,
}

/// Every double-quoted string of `s`.
fn quoted_strings(s: &str) -> Vec<&str> {
    s.split('"').skip(1).step_by(2).collect()
}

fn lock_entries(text: &str) -> Vec<LockEntry> {
    let mut entries: Vec<LockEntry> = Vec::new();
    let mut in_package = false;
    let mut in_dependencies = false;
    for line in universal_newlines(text).split('\n') {
        if line.starts_with('[') {
            in_package = line == "[[package]]";
            in_dependencies = false;
            if in_package {
                entries.push(LockEntry {
                    name: String::new(),
                    version: String::new(),
                    has_source: false,
                    dependencies: Vec::new(),
                });
            }
            continue;
        }
        let Some(entry) = entries.last_mut().filter(|_| in_package) else {
            continue;
        };
        if in_dependencies {
            entry
                .dependencies
                .extend(quoted_strings(line).into_iter().map(str::to_string));
            in_dependencies = !line.contains(']');
        } else if let Some(rest) = line.strip_prefix("dependencies = [") {
            entry
                .dependencies
                .extend(quoted_strings(rest).into_iter().map(str::to_string));
            in_dependencies = !rest.contains(']');
        } else if line.starts_with("source = ") {
            entry.has_source = true;
        } else {
            match name_or_version(line) {
                Some(("name", value)) => entry.name = value.to_string(),
                Some((_, value)) => entry.version = value.to_string(),
                None => {}
            }
        }
    }
    entries
}

/// The path packages of `<root>/Cargo.toml` and their dependency closure in the lockfile `after`.
pub fn allowed_packages(root: &Path, after: &str) -> BTreeSet<Pair> {
    let names = path_package_names(root);
    if names.is_empty() {
        return BTreeSet::new();
    }
    let entries = lock_entries(after);
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, entry) in entries.iter().enumerate() {
        by_name.entry(entry.name.as_str()).or_default().push(i);
    }
    let mut seen: BTreeSet<usize> = BTreeSet::new();
    let mut queue: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| names.contains(&e.name) && !e.has_source)
        .map(|(i, _)| i)
        .collect();
    while let Some(i) = queue.pop() {
        if !seen.insert(i) {
            continue;
        }
        for dependency in &entries[i].dependencies {
            // "name", "name version" or "name version (source)".
            let mut parts = dependency.split_whitespace();
            let Some(name) = parts.next() else { continue };
            let version = parts.next();
            for &j in by_name.get(name).map_or(&[][..], Vec::as_slice) {
                if version.is_none_or(|v| entries[j].version == v) {
                    queue.push(j);
                }
            }
        }
    }
    seen.into_iter()
        .map(|i| (entries[i].name.clone(), entries[i].version.clone()))
        .collect()
}
