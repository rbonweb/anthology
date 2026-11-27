//! Repo approval (MS1): a port of `memory_sync`'s `_load_approved`, `is_approved`, `approve`,
//! `unapprove` and `ensure_union_attrs` (plan §3.6).
//!
//! Approvals live in `<anthology home>/approved-repos.json`: `{"repos": {<key>: {"approved":
//! "YYYY-MM-DD"}}, "version": 1}`, written as `json.dumps(indent=2, sort_keys=True) + "\n"`
//! through `_atomic_write`. An unreadable or invalid file means nothing is approved; the next
//! approval rewrites it.
//!
//! Errors are Python's: an `OSError` as `PyExc::OSError`, and a refusal to write through a
//! symlinked or special `.gitattributes` as `PyExc::Other("UnsafeTarget", <message>)`
//! (`anthology_core::fsx`).

use std::path::{Path, PathBuf};

use anthology_core::git::Git;
use anthology_core::{Env, clock, fsx};
use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::str::split_ws;

use crate::repo::{anthology_home, repo_key};

/// `memory_sync.UNION_LINES`: the `merge=union` attributes of the generated indexes.
pub const UNION_LINES: [&str; 2] = [
    ".claude/memory/MEMORY.md merge=union",
    ".claude/agent-memory/*/MEMORY.md merge=union",
];

/// `_approved_path(env)`.
pub fn approved_path(env: &Env) -> PathBuf {
    anthology_home(env).join("approved-repos.json")
}

/// `memory_sync._load_approved(env)`: `{"version": 1, "repos": {...}}`, with `repos` empty
/// when the file is missing, unreadable, not UTF-8, not JSON, or not an object holding an
/// object under `repos`.
pub fn load_approved(env: &Env) -> Map {
    let repos = std::fs::read(approved_path(env))
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|text| json::loads(&text).ok())
        .and_then(Value::into_object)
        .and_then(|mut data| data.remove("repos"))
        .and_then(Value::into_object)
        .unwrap_or_default();
    let mut out = Map::new();
    out.insert("version".into(), Value::Int(json::Int::Small(1)));
    out.insert("repos".into(), Value::Object(repos));
    out
}

/// `memory_sync.is_approved(key, env)`.
pub fn is_approved(key: &str, env: &Env) -> bool {
    repos(&load_approved(env)).contains_key(key)
}

/// `memory_sync.approve(root, env)`: records the repo at `root` as approved (today's date,
/// once), then adds the union merge attributes. Returns its key.
///
/// The approval is saved first: when `.gitattributes` is a symlink or not a regular file, the
/// `UnsafeTarget` error comes after the approval is already recorded, and nothing is written
/// through the link.
pub fn approve(root: &Path, env: &Env, git: &dyn Git) -> Result<String, PyExc> {
    let key = repo_key(root, env, git);
    let mut data = load_approved(env);
    if !repos(&data).contains_key(&key) {
        let mut entry = Map::new();
        entry.insert("approved".into(), Value::Str(clock::today_local().iso()));
        repos_mut(&mut data).insert(key.clone(), Value::Object(entry));
        save_approved(&data, env)?;
    }
    ensure_union_attrs(root)?;
    Ok(key)
}

/// `memory_sync.unapprove(key, env)`: `false` when `key` was not approved.
pub fn unapprove(key: &str, env: &Env) -> Result<bool, PyExc> {
    let mut data = load_approved(env);
    if repos_mut(&mut data).remove(key).is_none() {
        return Ok(false);
    }
    save_approved(&data, env)?;
    Ok(true)
}

/// `memory_sync.ensure_union_attrs(root)`: adds each missing [`UNION_LINES`] line to
/// `<root>/.gitattributes` (lines compare with their whitespace collapsed); returns whether
/// it changed anything. Refuses a symlinked or special `.gitattributes` (`UnsafeTarget`).
pub fn ensure_union_attrs(root: &Path) -> Result<bool, PyExc> {
    let plan = plan_union_attrs(root)?;
    for line in &plan.missing {
        fsx::append_line(&plan.path, line)?;
    }
    Ok(!plan.missing.is_empty())
}

/// What [`ensure_union_attrs`] would do to `<root>/.gitattributes`; see [`plan_union_attrs`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnionAttrsPlan {
    /// `<root>/.gitattributes`.
    pub path: PathBuf,
    /// The file as it is now (lossy UTF-8); empty when it does not exist.
    pub existing: String,
    /// The [`UNION_LINES`] the file lacks, in order.
    pub missing: Vec<&'static str>,
}

impl UnionAttrsPlan {
    /// The file once [`ensure_union_attrs`] has run: `existing` kept as it is (a newline is added
    /// after its last line when that has none), then each missing line.
    pub fn updated(&self) -> String {
        let mut out = self.existing.clone();
        for line in &self.missing {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(line);
            out.push('\n');
        }
        out
    }
}

/// Reads `<root>/.gitattributes` and returns which [`UNION_LINES`] it lacks (lines compare with
/// their whitespace collapsed), without writing anything. Refuses a symlinked or special file
/// (`UnsafeTarget`), as [`ensure_union_attrs`] does.
pub fn plan_union_attrs(root: &Path) -> Result<UnionAttrsPlan, PyExc> {
    let path = root.join(".gitattributes");
    let existing = fsx::read_regular(&path, None)?;
    let existing = String::from_utf8_lossy(&existing).into_owned();
    let have: Vec<String> = splitlines(&existing)
        .into_iter()
        .map(|l| split_ws(l).join(" "))
        .collect();
    let missing = UNION_LINES
        .into_iter()
        .filter(|line| !have.iter().any(|h| h == line))
        .collect();
    Ok(UnionAttrsPlan {
        path,
        existing,
        missing,
    })
}

fn save_approved(data: &Map, env: &Env) -> Result<(), PyExc> {
    let mut text = json::dumps(&Value::Object(data.clone()), &json::INDENT2_SORTED);
    text.push('\n');
    fsx::atomic_write(
        &approved_path(env),
        text.as_bytes(),
        0o644,
        ".tmp-",
        ".part",
    )
}

fn repos(data: &Map) -> &Map {
    data.get("repos")
        .and_then(Value::as_object)
        .expect("load_approved always holds repos")
}

fn repos_mut(data: &mut Map) -> &mut Map {
    data.get_mut("repos")
        .and_then(Value::as_object_mut)
        .expect("load_approved always holds repos")
}

/// `str.splitlines()`: lines split at `\n`, `\r\n`, `\r`, `\v`, `\f`, `\x1c`–`\x1e`, U+0085,
/// U+2028 and U+2029, without the breaks; no trailing empty line.
pub(crate) fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut it = s.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        let brk = matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !brk {
            continue;
        }
        out.push(&s[start..i]);
        let mut next = i + c.len_utf8();
        if c == '\r' && it.peek().is_some_and(|&(_, n)| n == '\n') {
            it.next();
            next += 1;
        }
        start = next;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitlines_matches_python() {
        assert_eq!(splitlines(""), Vec::<&str>::new());
        assert_eq!(splitlines("a\nb"), ["a", "b"]);
        assert_eq!(splitlines("a\r\nb\n"), ["a", "b"]);
        assert_eq!(splitlines("a\r\rb\u{2028}c\x0bd"), ["a", "", "b", "c", "d"]);
        assert_eq!(splitlines("\n"), [""]);
    }
}
