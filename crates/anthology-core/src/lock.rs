//! `atl.lock` and `anthology.lock` (plan §3.3, GC-9, GC-17, design §8.2).
//!
//! [`parse`] with [`ATL_TAG_PREFIX`] is `atl_install.load_lock`'s JSON step plus
//! `validate_lock`, message for message: `%r` is Python's `repr()` of the JSON value (str
//! quoting and escapes, `None`/`True`, float `repr`, list and dict forms), and `\d` in
//! `VERSION_RE` is Unicode `Nd`, as in a Python str pattern. The JSON step's message is
//! `"<path> is not valid JSON"` in Python; `parse` has no path and names the file
//! (`atl.lock is not valid JSON`), which is what `load_lock("atl.lock")` prints. A caller that
//! has the path runs `pycompat::json::loads` itself and then [`validate`] (and, for
//! `anthology.lock`, [`check_anthology_layout`]).
//!
//! With [`ANTHOLOGY_TAG_PREFIX`] the same checks and messages apply with `anthology.lock` in
//! place of `atl.lock` and GC-17's rules: the version is
//! `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$` (ASCII digits) and the tag is
//! `anthology-bin--v<version>`. `parse` also requires the exact design §8.2 layout
//! ([`render_anthology`], with or without its final newline), because `bootstrap.sh` reads the
//! file line by line with `sed`; any other layout is
//! `anthology.lock: not in the layout anthology-dev lock write produces`. A caller that runs
//! `loads` and [`validate`] itself **must** then call [`check_anthology_layout`], or it skips
//! that rule.
//!
//! The `anthology.lock …` messages are details, not GC-14 strings. RT1b, RT2 and RL1 show
//! GC-14's reason (`anthology.lock is not valid`) for every error from this module on an
//! anthology lock, and keep the detail for logs or `--verbose` output only.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use anthology_pycompat::json::{self, Int, Map, Value};
use anthology_pycompat::str as pystr;

use crate::platform::Target;

/// atl's release tag prefix (`atl--v<version>`).
pub const ATL_TAG_PREFIX: &str = "atl--v";
/// The anthology binary's release tag prefix (`anthology-bin--v<version>`, GC-17).
pub const ANTHOLOGY_TAG_PREFIX: &str = "anthology-bin--v";

/// A valid lock: every target has a 64-hex sha256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lock {
    pub version: String,
    pub tag: String,
    pub repo: String,
    pub sha256: BTreeMap<Target, String>,
}

#[derive(Clone, Copy)]
enum Kind {
    Atl,
    Anthology,
}

impl Kind {
    fn of(tag_prefix: &str) -> Result<Kind, String> {
        match tag_prefix {
            ATL_TAG_PREFIX => Ok(Kind::Atl),
            ANTHOLOGY_TAG_PREFIX => Ok(Kind::Anthology),
            other => Err(format!(
                "unknown lock tag prefix {}; use {ATL_TAG_PREFIX} or {ANTHOLOGY_TAG_PREFIX}",
                py_repr_str(other)
            )),
        }
    }

    fn file(self) -> &'static str {
        match self {
            Kind::Atl => "atl.lock",
            Kind::Anthology => "anthology.lock",
        }
    }

    fn version_ok(self, v: &str) -> bool {
        match self {
            Kind::Atl => atl_version_ok(v),
            Kind::Anthology => anthology_version_ok(v),
        }
    }
}

/// Parses and validates a lock file's text (module docs).
pub fn parse(text: &str, tag_prefix: &str) -> Result<Lock, String> {
    let kind = Kind::of(tag_prefix)?;
    let value = json::loads(text).map_err(|_| format!("{} is not valid JSON", kind.file()))?;
    let lock = validate(&value, tag_prefix)?;
    if let Kind::Anthology = kind {
        check_anthology_layout(text, &lock)?;
    }
    Ok(lock)
}

/// The design §8.2 layout rule for `anthology.lock`: `text` must be [`render_anthology`] of the
/// lock it holds, with or without the final newline. [`parse`] applies it; a caller that runs
/// `loads` and [`validate`] itself must call it next.
pub fn check_anthology_layout(text: &str, lock: &Lock) -> Result<(), String> {
    let layout = render_anthology(lock);
    if text == layout || Some(text) == layout.strip_suffix('\n') {
        Ok(())
    } else {
        Err("anthology.lock: not in the layout anthology-dev lock write produces".into())
    }
}

/// `validate_lock(value)` for an already decoded JSON value (no layout check).
pub fn validate(value: &Value, tag_prefix: &str) -> Result<Lock, String> {
    let kind = Kind::of(tag_prefix)?;
    let name = kind.file();
    let keys = ["version", "tag", "repo", "sha256"];
    let obj = value
        .as_object()
        .filter(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(k)))
        .ok_or_else(|| {
            format!("{name} must be an object with exactly version, tag, repo and sha256")
        })?;
    // Every key is present (checked above); NULL only satisfies the type.
    static NULL: Value = Value::Null;
    let field = |k: &str| obj.get(k).unwrap_or(&NULL);

    let version = match field("version") {
        Value::Str(v) if kind.version_ok(v) => v.clone(),
        other => return Err(format!("{name}: invalid version {}", py_repr(other))),
    };
    let tag = format!("{tag_prefix}{version}");
    if field("tag").as_str() != Some(tag.as_str()) {
        return Err(format!("{name}: tag must be {tag}"));
    }
    let repo = match field("repo") {
        Value::Str(r) if repo_ok(r) => r.clone(),
        other => return Err(format!("{name}: invalid repo {}", py_repr(other))),
    };
    let digests = field("sha256")
        .as_object()
        .filter(|d| {
            d.len() == Target::ALL.len() && Target::ALL.iter().all(|t| d.contains_key(t.triple()))
        })
        .ok_or_else(|| {
            let all: Vec<_> = Target::ALL.iter().map(|t| t.triple()).collect();
            format!(
                "{name}: sha256 must have exactly the targets {}",
                all.join(", ")
            )
        })?;
    let mut sha256 = BTreeMap::new();
    for (triple, digest) in digests.iter() {
        match (Target::from_triple(triple), digest.as_str()) {
            (Some(target), Some(d)) if hex64(d) => {
                sha256.insert(target, d.to_string());
            }
            _ => {
                return Err(format!(
                    "{name}: sha256 for {triple} must be 64 lowercase hex characters"
                ));
            }
        }
    }
    Ok(Lock {
        version,
        tag,
        repo,
        sha256,
    })
}

/// `anthology.lock` in the design §8.2 layout: `json.dumps(lock, indent=2) + "\n"` with the
/// keys `version`, `tag`, `repo`, `sha256` and the targets in `TARGETS` order.
pub fn render_anthology(lock: &Lock) -> String {
    let mut digests = Map::new();
    for (t, d) in &lock.sha256 {
        digests.insert(t.triple().to_string(), Value::Str(d.clone()));
    }
    let mut m = Map::new();
    m.insert("version".into(), Value::Str(lock.version.clone()));
    m.insert("tag".into(), Value::Str(lock.tag.clone()));
    m.insert("repo".into(), Value::Str(lock.repo.clone()));
    m.insert("sha256".into(), Value::Object(digests));
    let mut text = json::dumps(&Value::Object(m), &json::INDENT2);
    text.push('\n');
    text
}

/// `VERSION_RE.fullmatch(v)`: `\d+\.\d+\.\d+(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?`.
fn atl_version_ok(v: &str) -> bool {
    let Some(rest) = three_numbers(v, pystr::is_decimal) else {
        return false;
    };
    if rest.is_empty() {
        return true;
    }
    let Some(pre) = rest.strip_prefix('-') else {
        return false;
    };
    pre.split('.')
        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// GC-17: `^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$`.
fn anthology_version_ok(v: &str) -> bool {
    let Some(rest) = three_numbers(v, |c| c.is_ascii_digit()) else {
        return false;
    };
    rest.is_empty()
        || rest.strip_prefix('-').is_some_and(|p| {
            !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
        })
}

/// `N.N.N` at the start of `v` (each `N` one or more `digit`s); the rest of `v`.
fn three_numbers(v: &str, digit: impl Fn(char) -> bool) -> Option<&str> {
    let mut rest = v;
    for i in 0..3 {
        let n = rest
            .char_indices()
            .find(|&(_, c)| !digit(c))
            .map_or(rest.len(), |(at, _)| at);
        if n == 0 {
            return None;
        }
        rest = &rest[n..];
        if i < 2 {
            rest = rest.strip_prefix('.')?;
        }
    }
    Some(rest)
}

/// `REPO_RE.fullmatch(r)` (`[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+`) with no `.` or `..` part.
fn repo_ok(r: &str) -> bool {
    let Some((owner, name)) = r.split_once('/') else {
        return false;
    };
    [owner, name].iter().all(|p| {
        !p.is_empty()
            && *p != "."
            && *p != ".."
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    })
}

/// `[0-9a-f]{64}`.
fn hex64(d: &str) -> bool {
    d.len() == 64 && d.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Python's `repr()` of a str: CPython's quote choice and escapes; printable non-ASCII
/// characters stay as they are (`str.isprintable`).
pub(crate) fn py_repr_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    push_repr_str(&mut out, s);
    out
}

fn push_repr_str(out: &mut String, s: &str) {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for c in s.chars() {
        let n = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if n < 0x20 || n == 0x7f => out.push_str(&format!("\\x{n:02x}")),
            _ if n < 0x7f || pystr::is_printable(c) => out.push(c),
            _ if n <= 0xff => out.push_str(&format!("\\x{n:02x}")),
            _ if n <= 0xffff => out.push_str(&format!("\\u{n:04x}")),
            _ => out.push_str(&format!("\\U{n:08x}")),
        }
    }
    out.push(quote);
}

/// Python's `repr()` of a decoded JSON value (iterative, so any depth works).
pub(crate) fn py_repr(v: &Value) -> String {
    enum Item<'a> {
        Val(&'a Value),
        Key(&'a str),
        Lit(&'static str),
    }
    let mut out = String::new();
    let mut stack = vec![Item::Val(v)];
    while let Some(item) = stack.pop() {
        let v = match item {
            Item::Lit(s) => {
                out.push_str(s);
                continue;
            }
            Item::Key(k) => {
                push_repr_str(&mut out, k);
                continue;
            }
            Item::Val(v) => v,
        };
        match v {
            Value::Null => out.push_str("None"),
            Value::Bool(true) => out.push_str("True"),
            Value::Bool(false) => out.push_str("False"),
            Value::Int(Int::Small(n)) => out.push_str(&n.to_string()),
            Value::Int(Int::Big(digits)) => out.push_str(digits),
            Value::Float(f) => out.push_str(&float_repr(*f)),
            Value::Str(s) => push_repr_str(&mut out, s),
            Value::Array(items) => {
                out.push('[');
                stack.push(Item::Lit("]"));
                for (i, item) in items.iter().enumerate().rev() {
                    stack.push(Item::Val(item));
                    if i > 0 {
                        stack.push(Item::Lit(", "));
                    }
                }
            }
            Value::Object(m) => {
                out.push('{');
                stack.push(Item::Lit("}"));
                let entries: Vec<_> = m.iter().collect();
                for (i, (k, item)) in entries.into_iter().enumerate().rev() {
                    stack.push(Item::Val(item));
                    stack.push(Item::Lit(": "));
                    stack.push(Item::Key(k));
                    if i > 0 {
                        stack.push(Item::Lit(", "));
                    }
                }
            }
        }
    }
    out
}

/// `repr(float)`: `json.dumps`' float text, except the non-finite spellings.
fn float_repr(f: f64) -> String {
    match json::dumps(&Value::Float(f), &json::PY_DEFAULT).as_str() {
        "NaN" => "nan".into(),
        "Infinity" => "inf".into(),
        "-Infinity" => "-inf".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod tests;
