//! Finds the sibling anthology-guardrails plugin, shared by session start, setup and doctor
//! (RT1a; port of `guardrails_loc.py`).
//!
//! [`inspect`] returns the folder the PreToolUse hook really runs, and a [`Problem`]:
//!
//! - `installed_plugins.json` is the source of truth. An `anthology-guardrails@*` entry counts
//!   when its install path has the guard (for a `directory` marketplace: when the live folder
//!   has it, whatever the cache snapshot holds). Its scope must be `user`, or `project`/`local`
//!   with a `projectPath` containing `cwd`. The newest `lastUpdated` wins.
//! - The folder returned (B2):
//!   - `directory` marketplace: Claude Code loads it in place, so the folder is
//!     `installLocation` joined with the plugin's relative `source` in
//!     `<installLocation>/.claude-plugin/marketplace.json`. The source must stay inside
//!     `installLocation` after realpath. Anything else is `LiveMissing` with no folder: a
//!     refused source, a contained folder without the guard, an unlisted plugin, a missing,
//!     invalid or oversized marketplace.json. **The cache copy never stands in.**
//!   - another source type: the cache, so `installPath`;
//!   - unknown marketplace: when `plugin_root` is not under `<config>/plugins/cache/`, the
//!     contained sibling `<plugin root>/../anthology-guardrails`; else `installPath`.
//! - No `anthology@*` entry either (a `--plugin-dir` dev session): the contained sibling.
//! - `NotInstalled`: anthology is installed without an applicable guardrails entry.
//! - `Disabled` (it overrides `LiveMissing`), when any of these holds:
//!   - an `enabledPlugins["anthology-guardrails@…"]` entry whose value is anything but `true`,
//!     in the user, project or local settings;
//!   - a settings file that exists but cannot be read;
//!   - a working-folder settings file over 2 MiB, or one that does not parse after decoding it
//!     as Claude Code does.
//! - `NotFound`: nothing found at all.
//!
//! F-6: "has the guard" is `hooks/guard.sh` (Python: `scripts/guard.py`).
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_pycompat::json::{self, Int, Value};

use crate::compat::{
    self, Opened, decode_text, is_file, loads_bounded, pathlib_child, pathlib_parent, pathlib_str,
    py_strip, realpath,
};

/// Claude Code's own limit for a settings file (m15).
pub const REPO_READ_LIMIT: u64 = compat::REPO_READ_LIMIT;
/// The exact key prefix of the guardrails plugin (`enabledPlugins`, `installed_plugins.json`).
pub const GUARDRAILS_PREFIX: &str = "anthology-guardrails@";
/// The key prefix of the anthology plugin itself.
pub const ANTHOLOGY_PREFIX: &str = "anthology@";
/// The guard the hook runs, relative to the plugin folder (F-6).
pub const GUARD_REL: &str = "hooks/guard.sh";

/// Why the live guardrails folder is not usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    Disabled,
    NotInstalled,
    NotFound,
    LiveMissing,
}

impl Problem {
    /// Python's string: `"disabled"`, `"not_installed"`, `"not_found"`, `"live_missing"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Problem::Disabled => "disabled",
            Problem::NotInstalled => "not_installed",
            Problem::NotFound => "not_found",
            Problem::LiveMissing => "live_missing",
        }
    }
}

/// `(folder or None, problem or None)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardLoc {
    pub root: Option<PathBuf>,
    pub problem: Option<Problem>,
}

fn path_of(b: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(b))
}

fn bytes(p: &Path) -> &[u8] {
    p.as_os_str().as_bytes()
}

fn has_guard(dir: &Path) -> bool {
    is_file(&dir.join(GUARD_REL))
}

/// What `_load` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ok,
    Absent,
    Refused,
    Invalid,
}

/// `_load(path, limit, closed)`: the parsed file and its state. `limit` is for files a cloned
/// repo controls (size and nesting bounded); with `closed`, a file that does not parse is
/// refused (so the caller fails closed) and an empty one is `{}`, as in Claude Code.
fn load(path: &Path, limit: Option<u64>, closed: bool) -> (Option<Value>, State) {
    let raw = match compat::open_read(path, limit) {
        Opened::Absent => return (None, State::Absent),
        Opened::Refused => return (None, State::Refused),
        Opened::Bytes(raw) => raw,
    };
    let text = decode_text(&raw);
    if closed && py_strip(&text).is_empty() {
        return (Some(Value::Object(json::Map::new())), State::Ok);
    }
    let parsed = if limit.is_some() {
        loads_bounded(&text)
    } else {
        json::loads(&text).ok()
    };
    match parsed {
        Some(v) => (Some(v), State::Ok),
        None if closed => (None, State::Refused),
        None => (None, State::Invalid),
    }
}

/// `_read_json(path, limit)`: the parsed file, or `None` when missing, refused or malformed.
fn read_json(path: &Path, limit: Option<u64>) -> Option<Value> {
    load(path, limit, false).0
}

/// `_inside(cwd, project)`: realpath(cwd) is realpath(project) or below it.
fn inside(cwd: &Path, project: &Path) -> bool {
    under(cwd, project, false)
}

/// `_under(path, parent, strict)`: realpath(path) is inside realpath(parent); `strict` also
/// refuses the parent itself. A NUL or an unreadable working folder is `false`.
fn under(path: &Path, parent: &Path, strict: bool) -> bool {
    let (Ok(path), Ok(parent)) = (realpath(path), realpath(parent)) else {
        return false;
    };
    let (path, parent) = (bytes(&path), bytes(&parent));
    if path == parent {
        return !strict;
    }
    let mut prefix = parent.to_vec();
    while prefix.last() == Some(&b'/') {
        prefix.pop();
    }
    prefix.push(b'/');
    path.starts_with(&prefix)
}

/// `Disabled` when any settings file disables the guardrails or fails closed (module docs).
pub fn disabled(env: &Env, cwd: &Path) -> bool {
    let cwd = effective_cwd(cwd);
    let repo = cwd.join(".claude");
    let files = [
        (compat::config_dir(env).join("settings.json"), None),
        (repo.join("settings.json"), Some(REPO_READ_LIMIT)),
        (repo.join("settings.local.json"), Some(REPO_READ_LIMIT)),
    ];
    for (path, limit) in files {
        let (data, state) = load(&path, limit, limit.is_some());
        if state == State::Refused {
            // fail closed: a file we cannot read or parse may be the one that disables it
            return true;
        }
        let enabled = data
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|o| o.get("enabledPlugins"))
            .and_then(Value::as_object);
        if let Some(enabled) = enabled {
            for (name, value) in enabled.iter() {
                if name.starts_with(GUARDRAILS_PREFIX) && !matches!(value, Value::Bool(true)) {
                    return true;
                }
            }
        }
    }
    false
}

/// The marketplace of a registry key (`name.split("@", 1)[1]`, or `""`).
fn marketplace_of(name: &str) -> &str {
    name.split_once('@').map_or("", |(_, m)| m)
}

/// `known_marketplaces.json[marketplace]`, when it is an object.
fn known_item(config: &Path, marketplace: &str) -> Option<Value> {
    let mut known = read_json(&config.join("plugins").join("known_marketplaces.json"), None)?;
    known.as_object_mut()?.remove(marketplace)
}

/// `_directory_root`: the live guardrails folder of a `directory` marketplace (contained in its
/// install location, whether or not it has the guard), or `None` when it is not one or any
/// piece is missing or invalid.
fn directory_root(config: &Path, marketplace: &str, plugin: &str) -> Option<PathBuf> {
    let item = known_item(config, marketplace)?;
    let item = item.as_object()?;
    let source = item.get("source")?.as_object()?;
    if source.get("source")?.as_str()? != "directory" {
        return None;
    }
    let location = item.get("installLocation")?.as_str()?;
    if location.is_empty() || !location.starts_with('/') {
        return None;
    }
    let manifest = read_json(
        &Path::new(location)
            .join(".claude-plugin")
            .join("marketplace.json"),
        Some(REPO_READ_LIMIT),
    )?;
    let listed = manifest.as_object()?.get("plugins")?.as_array()?;
    for spec in listed {
        let Some(spec) = spec.as_object() else {
            continue;
        };
        if spec.get("name").and_then(Value::as_str) != Some(plugin) {
            continue;
        }
        let rel = spec.get("source").and_then(Value::as_str)?;
        if rel.is_empty() || rel.starts_with('/') || rel.contains('\0') {
            return None;
        }
        let joined = if location.ends_with('/') {
            format!("{location}{rel}")
        } else {
            format!("{location}/{rel}")
        };
        let root = realpath(Path::new(&joined)).ok()?;
        let root = path_of(pathlib_str(bytes(&root)));
        return under(&root, Path::new(location), false).then_some(root);
    }
    None
}

/// `_marketplace_kind`: `Some(true)` for a `directory` source, `Some(false)` for a readable
/// source of another type, `None` when unreadable.
fn marketplace_is_directory(config: &Path, marketplace: &str) -> Option<bool> {
    let item = known_item(config, marketplace)?;
    let kind = item.as_object()?.get("source")?.as_object()?.get("source")?.as_str()?;
    Some(kind == "directory")
}

/// `_contained_sibling`: `<plugin root>/../anthology-guardrails` when it has the guard and does
/// not escape the folder next to the plugin (a symlink out of it, or to the folder itself, is
/// refused).
fn contained_sibling(plugin_root: &Path) -> Option<PathBuf> {
    let parent = pathlib_parent(bytes(plugin_root));
    let sibling = path_of(pathlib_child(&parent, b"anthology-guardrails"));
    (has_guard(&sibling) && under(&sibling, &path_of(parent), true)).then_some(sibling)
}

/// `_live_root`: the folder the hook executes for the chosen entry, or `LiveMissing`.
fn live_root(plugin_root: &Path, env: &Env, name: &str, install_path: &str) -> GuardLoc {
    let config = compat::config_dir(env);
    let marketplace = marketplace_of(name);
    let install = path_of(pathlib_str(install_path.as_bytes()));
    match marketplace_is_directory(&config, marketplace) {
        Some(false) => GuardLoc {
            root: Some(install),
            problem: None,
        },
        Some(true) => {
            // never the cache copy: a source that is refused, unlisted or has no manifest is a
            // problem too
            match directory_root(&config, marketplace, "anthology-guardrails") {
                Some(live) if has_guard(&live) => GuardLoc {
                    root: Some(live),
                    problem: None,
                },
                _ => GuardLoc {
                    root: None,
                    problem: Some(Problem::LiveMissing),
                },
            }
        }
        None => {
            if !under(plugin_root, &config.join("plugins").join("cache"), false)
                && let Some(sibling) = contained_sibling(plugin_root)
            {
                return GuardLoc {
                    root: Some(sibling),
                    problem: None,
                };
            }
            GuardLoc {
                root: Some(install),
                problem: None,
            }
        }
    }
}

/// `_live_has_guard`: whether `name` is in a `directory` marketplace whose live folder has the
/// guard (that marketplace runs the live folder, so its cache snapshot does not decide, m13).
fn live_has_guard(env: &Env, name: &str) -> bool {
    directory_root(&compat::config_dir(env), marketplace_of(name), "anthology-guardrails")
        .is_some_and(|live| has_guard(&live))
}

/// `cwd or os.getcwd()`.
fn effective_cwd(cwd: &Path) -> PathBuf {
    if cwd.as_os_str().is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        cwd.to_path_buf()
    }
}

/// The folder the guardrails hook really runs and its problem (module docs).
pub fn inspect(plugin_root: &Path, env: &Env, cwd: &Path) -> GuardLoc {
    let cwd = effective_cwd(cwd);
    let data = read_json(
        &compat::config_dir(env)
            .join("plugins")
            .join("installed_plugins.json"),
        None,
    );
    let empty = json::Map::new();
    let plugins = data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|o| o.get("plugins"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut best: Option<(String, &str, &str)> = None;
    for (name, entries) in plugins.iter() {
        if !name.starts_with(GUARDRAILS_PREFIX) {
            continue;
        }
        let Some(entries) = entries.as_array() else {
            continue;
        };
        for entry in entries {
            let Some(entry) = entry.as_object() else {
                continue;
            };
            let Some(install) = entry.get("installPath").and_then(Value::as_str) else {
                continue;
            };
            let scope = entry.get("scope").and_then(Value::as_str);
            if matches!(scope, Some("project" | "local")) {
                let project = entry.get("projectPath").and_then(Value::as_str);
                if !project.is_some_and(|p| inside(&cwd, Path::new(p))) {
                    continue;
                }
            }
            if !has_guard(Path::new(install)) && !live_has_guard(env, name) {
                continue;
            }
            let stamp = py_str_or_empty(entry.get("lastUpdated"));
            if best.as_ref().is_none_or(|(b, _, _)| stamp > *b) {
                best = Some((stamp, install, name));
            }
        }
    }
    let mut loc = GuardLoc {
        root: None,
        problem: None,
    };
    if let Some((_, install, name)) = best {
        loc = live_root(plugin_root, env, name, install);
    } else if plugins.iter().any(|(name, _)| name.starts_with(ANTHOLOGY_PREFIX)) {
        loc.problem = Some(Problem::NotInstalled);
    } else {
        loc.root = contained_sibling(plugin_root);
        if loc.root.is_none() {
            loc.problem = Some(Problem::NotFound);
        }
    }
    if matches!(loc.problem, None | Some(Problem::LiveMissing)) && disabled(env, &cwd) {
        loc.problem = Some(Problem::Disabled);
    }
    loc
}

// ---------------------------------------------------------------------------------------------
// `str(value or "")` for a JSON value

/// `str(v or "")`: Python's truthiness, then `str()`.
fn py_str_or_empty(v: Option<&Value>) -> String {
    match v {
        None => String::new(),
        Some(v) if !truthy(v) => String::new(),
        Some(Value::Str(s)) => s.clone(),
        Some(v) => py_repr(v),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(Int::Small(i)) => *i != 0,
        Value::Int(Int::Big(_)) => true,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `repr()` of a JSON value (`str()` too, except for a top-level str). Iterative, so any depth
/// works (Python raises `RecursionError` on a very deep one: D-1).
fn py_repr(v: &Value) -> String {
    enum Step<'a> {
        Value(&'a Value),
        Text(&'static str),
        Key(&'a str),
    }
    let mut out = String::new();
    let mut stack = vec![Step::Value(v)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Text(t) => out.push_str(t),
            Step::Key(k) => {
                repr_str(k, &mut out);
                out.push_str(": ");
            }
            Step::Value(v) => match v {
                Value::Null => out.push_str("None"),
                Value::Bool(true) => out.push_str("True"),
                Value::Bool(false) => out.push_str("False"),
                Value::Int(Int::Small(i)) => out.push_str(&i.to_string()),
                Value::Int(Int::Big(s)) => out.push_str(s),
                Value::Float(f) => out.push_str(&float_repr(*f)),
                Value::Str(s) => repr_str(s, &mut out),
                Value::Array(a) => {
                    out.push('[');
                    stack.push(Step::Text("]"));
                    for (i, item) in a.iter().enumerate().rev() {
                        stack.push(Step::Value(item));
                        if i > 0 {
                            stack.push(Step::Text(", "));
                        }
                    }
                }
                Value::Object(o) => {
                    out.push('{');
                    stack.push(Step::Text("}"));
                    let items: Vec<_> = o.iter().collect();
                    for (i, (k, item)) in items.into_iter().enumerate().rev() {
                        stack.push(Step::Value(item));
                        stack.push(Step::Key(k));
                        if i > 0 {
                            stack.push(Step::Text(", "));
                        }
                    }
                }
            },
        }
    }
    out
}

/// `repr(float)`: the shortest round-trip form, as `json.dumps` prints it, except non-finite.
fn float_repr(f: f64) -> String {
    if f.is_nan() {
        "nan".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        json::dumps(&Value::Float(f), &json::PY_DEFAULT)
    }
}

/// `repr(str)`: single quotes unless the text has `'` and no `"`; `\\`, `\n`, `\r`, `\t`,
/// `\xhh` for the other C0 controls and DEL; non-printable characters escaped.
fn repr_str(s: &str, out: &mut String) {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !printable(c) => {
                let n = c as u32;
                if n < 0x100 {
                    out.push_str(&format!("\\x{n:02x}"));
                } else if n < 0x10000 {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
}

/// `str.isprintable()` for one character, approximated outside ASCII.
// TODO(P1): pycompat::str printable table (categories Cc, Cf, Cs, Co, Cn, Zl, Zp, Zs but space).
fn printable(c: char) -> bool {
    let n = c as u32;
    if n < 0x80 {
        return (0x20..0x7f).contains(&n);
    }
    !matches!(n, 0x80..=0xa0 | 0xad | 0x2028 | 0x2029 | 0xfeff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repr_of(text: &str) -> String {
        py_str_or_empty(Some(&json::loads(text).unwrap()))
    }

    #[test]
    fn last_updated_reads_like_python_str() {
        for (text, want) in [
            ("null", ""),
            ("false", ""),
            ("0", ""),
            ("0.0", ""),
            ("-0.0", ""),
            ("\"\"", ""),
            ("[]", ""),
            ("{}", ""),
            ("true", "True"),
            ("5", "5"),
            ("1.5", "1.5"),
            ("1e16", "1e+16"),
            ("Infinity", "inf"),
            ("-Infinity", "-inf"),
            ("NaN", "nan"),
            ("123456789012345678901234567890", "123456789012345678901234567890"),
            ("\"2026-01-01\"", "2026-01-01"),
            (r#"[1, "a", null, true, 2.5]"#, "[1, 'a', None, True, 2.5]"),
            (r#"{"a": [1, {}], "b": "it's"}"#, r#"{'a': [1, {}], 'b': "it's"}"#),
            (r#"["it's \"q\"", "\\", "\n\u0001\u007f"]"#, r#"['it\'s "q"', '\\', '\n\x01\x7f']"#),
            (r#"[" é "]"#, "['\\xa0é\\u2028']"),
        ] {
            assert_eq!(repr_of(text), want, "{text}");
        }
    }

    #[test]
    fn problems_have_their_python_names() {
        assert_eq!(Problem::LiveMissing.as_str(), "live_missing");
        assert_eq!(Problem::NotInstalled.as_str(), "not_installed");
        assert_eq!(Problem::NotFound.as_str(), "not_found");
        assert_eq!(Problem::Disabled.as_str(), "disabled");
    }
}
