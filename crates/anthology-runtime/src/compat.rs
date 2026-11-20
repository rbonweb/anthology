//! Cheap compatibility checks shared by doctor and session start (RT1a; port of `compat.py`).
//!
//! Each helper reads a few small files and never runs a subprocess. Nothing here returns a value
//! from the user's settings or MCP configuration: only names, scopes and paths of folders
//! Anthology owns.
//!
//! Also home to the Python-semantics helpers that `guardrails_loc` shares: Claude Code's settings
//! decoding ([`decode_text`]), the bounded non-blocking read, `os.path.realpath`
//! ([`realpath`]), `pathlib`'s string form ([`pathlib_str`]) and Python's `str.isspace`.
//!
//! Forced change F-9: [`WRAPPERS`] adds `anthology` to Python's `anthology-mem`, `rtk` and
//! `rtk-hook`.
#![forbid(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anthology_core::{Env, paths, roster::ROSTER};
use anthology_pycompat::json::{self, Value};

/// The PATH wrappers under `<plugin root>/bin/` (F-9 adds `anthology`).
pub const WRAPPERS: [&str; 4] = ["anthology", "anthology-mem", "rtk", "rtk-hook"];
/// The environment variable whose source [`subagent_model_source`] reports.
pub const SUBAGENT_MODEL: &str = "CLAUDE_CODE_SUBAGENT_MODEL";
/// Files a cloned repo controls are read up to Claude Code's own settings limit (m15).
pub const REPO_READ_LIMIT: u64 = 2 * 1024 * 1024;
/// A repo-controlled JSON text nested deeper than this is treated as Python's `RecursionError`
/// (so as "does not parse"). CPython raises at about 994 levels (3.9) or 9,998 (3.12), less on
/// a deeper stack, so 512 refuses whenever any Python would; legitimate settings never nest
/// this deep. The user's own files have no cap: parsing them is the D-1 direction.
pub const REPO_JSON_DEPTH: usize = 512;

/// `O_NONBLOCK` (a FIFO must not block the open; runtime has no libc dependency).
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x0004;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("anthology-runtime supports Linux and macOS only");

const ENOTDIR: i32 = 20;
const EBADF: i32 = 9;
#[cfg(target_os = "linux")]
const ELOOP: i32 = 40;
#[cfg(target_os = "macos")]
const ELOOP: i32 = 62;

// ---------------------------------------------------------------------------------------------
// Python string semantics

/// Python's `str.isspace` for one character (the set is fixed by the Unicode data; Python
/// counts the bidi classes WS, B, S and category Zs).
// TODO(P1): use pycompat::str::is_space.
pub(crate) fn py_isspace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{b}'
            | '\u{c}'
            | '\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Python's `s.strip()` (no argument).
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// The text of a JSON file the way Claude Code reads settings: UTF-16LE when it starts with
/// that BOM, else UTF-8 with invalid bytes replaced; one leading U+FEFF dropped. UTF-16BE with
/// a BOM is decoded too (Python's harmless superset: Claude Code ignores such a file).
pub fn decode_text(raw: &[u8]) -> String {
    let text = if let Some(rest) = raw.strip_prefix(b"\xff\xfe") {
        utf16_replace(rest, u16::from_le_bytes)
    } else if let Some(rest) = raw.strip_prefix(b"\xfe\xff") {
        utf16_replace(rest, u16::from_be_bytes)
    } else {
        String::from_utf8_lossy(raw).into_owned()
    };
    match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// CPython's `bytes.decode("utf-16-le"/"utf-16-be", errors="replace")`: an unpaired surrogate is
/// one U+FFFD and decoding resumes at the next unit; a trailing odd byte, or a high surrogate
/// cut off by the end of the data (with its odd byte), is one U+FFFD.
fn utf16_replace(b: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let n = b.len() / 2;
    let at = |i: usize| unit([b[2 * i], b[2 * i + 1]]);
    let mut out = String::with_capacity(n);
    let mut i = 0;
    while i < n {
        let u = at(i);
        match u {
            0xd800..=0xdbff => {
                if i + 1 < n {
                    let v = at(i + 1);
                    if (0xdc00..=0xdfff).contains(&v) {
                        let c = 0x10000 + ((u32::from(u) - 0xd800) << 10) + (u32::from(v) - 0xdc00);
                        out.push(char::from_u32(c).unwrap_or('\u{fffd}'));
                        i += 2;
                        continue;
                    }
                    out.push('\u{fffd}');
                    i += 1;
                } else {
                    // "unexpected end of data": the cut-off unit and any odd byte are one error.
                    out.push('\u{fffd}');
                    return out;
                }
            }
            0xdc00..=0xdfff => {
                out.push('\u{fffd}');
                i += 1;
            }
            _ => {
                out.push(char::from_u32(u32::from(u)).unwrap_or('\u{fffd}'));
                i += 1;
            }
        }
    }
    if b.len() % 2 == 1 {
        out.push('\u{fffd}');
    }
    out
}

/// The deepest `[`/`{` nesting of a JSON text that parsed (strings skipped).
pub(crate) fn json_depth(text: &str) -> usize {
    let (mut depth, mut max, mut in_str, mut esc) = (0usize, 0usize, false, false);
    for b in text.bytes() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' | b'{' => {
                depth += 1;
                max = max.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

/// `json.loads(text)` for a file a cloned repo controls: `None` when it does not parse, or when
/// it nests past [`REPO_JSON_DEPTH`] (Python's `RecursionError`).
pub(crate) fn loads_bounded(text: &str) -> Option<Value> {
    let v = json::loads(text).ok()?;
    (json_depth(text) <= REPO_JSON_DEPTH).then_some(v)
}

// ---------------------------------------------------------------------------------------------
// Reads

/// How a non-blocking open of a settings-like file went.
pub(crate) enum Opened {
    /// `FileNotFoundError`, `NotADirectoryError` or `ValueError` (a NUL in the path).
    Absent,
    /// Any other `OSError`, a file that is not regular after `fstat`, a read error, or a file
    /// over the limit.
    Refused,
    /// The bytes (at most `limit` of them).
    Bytes(Vec<u8>),
}

/// `os.open(path, O_RDONLY | O_NONBLOCK)` (following symlinks), `fstat` must say regular, then
/// `read()` or `read(limit + 1)`; more than `limit` bytes is refused. A FIFO never blocks and a
/// device is never read.
pub(crate) fn open_read(path: &Path, limit: Option<u64>) -> Opened {
    if path.as_os_str().as_bytes().contains(&0) {
        return Opened::Absent;
    }
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) => {
            return match (e.kind(), e.raw_os_error()) {
                (io::ErrorKind::NotFound, _) | (_, Some(ENOTDIR)) => Opened::Absent,
                (io::ErrorKind::InvalidInput, None) => Opened::Absent,
                _ => Opened::Refused,
            };
        }
    };
    match read_regular_file(&file, limit) {
        Some(raw) => Opened::Bytes(raw),
        None => Opened::Refused,
    }
}

fn read_regular_file(file: &File, limit: Option<u64>) -> Option<Vec<u8>> {
    if !file.metadata().ok()?.file_type().is_file() {
        return None;
    }
    let mut raw = Vec::new();
    match limit {
        None => {
            let mut f = file;
            f.read_to_end(&mut raw).ok()?;
        }
        Some(n) => {
            file.take(n.saturating_add(1)).read_to_end(&mut raw).ok()?;
            if raw.len() as u64 > n {
                return None;
            }
        }
    }
    Some(raw)
}

/// The parsed JSON file, or `None` when it is missing or malformed (`compat.read_json(path)`):
/// `Path.read_bytes()` (following symlinks, any file type, as Python does), decoded as Claude
/// Code does ([`decode_text`]). The user's own files: any size, any depth.
pub fn read_json(path: &Path) -> Option<Value> {
    if path.as_os_str().as_bytes().contains(&0) {
        return None;
    }
    let raw = std::fs::read(path).ok()?;
    json::loads(&decode_text(&raw)).ok()
}

/// `compat.read_json(path, limit)`, for files a cloned repo controls ([`REPO_READ_LIMIT`]): the
/// file must also be a regular file (a symlink to one is fine; a device or FIFO is not, and is
/// never read or waited on) of at most `limit` bytes, nesting at most [`REPO_JSON_DEPTH`];
/// otherwise it is skipped like an absent file.
pub fn read_json_limited(path: &Path, limit: u64) -> Option<Value> {
    match open_read(path, Some(limit)) {
        Opened::Bytes(raw) => loads_bounded(&decode_text(&raw)),
        Opened::Absent | Opened::Refused => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Paths

/// `<config>`: `$CLAUDE_CONFIG_DIR` when non-empty, else `(HOME or expanduser("~"))/.claude`.
pub fn config_dir(env: &Env) -> PathBuf {
    match env.get_os("CLAUDE_CONFIG_DIR") {
        Some(c) if !c.is_empty() => PathBuf::from(c),
        _ => paths::home_raw(env).join(".claude"),
    }
}

fn has_nul(p: &[u8]) -> bool {
    p.contains(&0)
}

/// `os.path.join(a, b)` (posix, two parts).
fn py_join(a: &[u8], b: &[u8]) -> Vec<u8> {
    if b.first() == Some(&b'/') || a.is_empty() {
        return b.to_vec();
    }
    let mut out = a.to_vec();
    if out.last() != Some(&b'/') {
        out.push(b'/');
    }
    out.extend_from_slice(b);
    out
}

/// `os.path.split(p)`: head (trailing slashes stripped unless it is all slashes) and tail.
fn py_split(p: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let i = p.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    let (head, tail) = (&p[..i], &p[i..]);
    let mut head = head.to_vec();
    if head.iter().any(|&c| c != b'/') {
        while head.last() == Some(&b'/') {
            head.pop();
        }
    }
    (head, tail.to_vec())
}

/// `os.path.normpath(p)` (posix): one leading slash, or exactly two; `.` and empty parts
/// dropped; `..` resolved against a preceding name (at the root it is dropped); `""` is `"."`.
fn py_normpath(p: &[u8]) -> Vec<u8> {
    if p.is_empty() {
        return b".".to_vec();
    }
    let lead = p.iter().take_while(|&&c| c == b'/').count();
    let initial: &[u8] = match lead {
        0 => b"",
        2 => b"//",
        _ => b"/",
    };
    let mut parts: Vec<&[u8]> = Vec::new();
    for comp in p.split(|&c| c == b'/') {
        if comp.is_empty() || comp == b"." {
            continue;
        }
        if comp != b".."
            || (initial.is_empty() && parts.is_empty())
            || parts.last() == Some(&&b".."[..])
        {
            parts.push(comp);
        } else if !parts.is_empty() {
            parts.pop();
        }
    }
    let mut out = initial.to_vec();
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `os.path.abspath(p)` with the process's working folder.
fn py_abspath(p: &[u8]) -> io::Result<Vec<u8>> {
    if p.first() == Some(&b'/') {
        return Ok(py_normpath(p));
    }
    let cwd = std::env::current_dir()?;
    Ok(py_normpath(&py_join(cwd.as_os_str().as_bytes(), p)))
}

/// CPython 3.12 `os.path.realpath(path)` (`strict=False`): symlinks resolved component by
/// component, a missing or unreadable component kept as it is (any `lstat` error means "not a
/// link"), a symlink loop returned with the rest unresolved. `Err` only for a NUL in the path
/// (Python's `ValueError`) or an unreadable working folder.
// TODO(P1): use pycompat::path::realpath.
pub(crate) fn realpath(path: &Path) -> io::Result<PathBuf> {
    let p = path.as_os_str().as_bytes();
    if has_nul(p) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut seen = std::collections::HashMap::new();
    let (joined, _) = join_realpath(Vec::new(), p.to_vec(), &mut seen)?;
    Ok(PathBuf::from(OsString::from_vec(py_abspath(&joined)?)))
}

/// `posixpath._joinrealpath(path, rest, strict=False, seen)`.
fn join_realpath(
    mut path: Vec<u8>,
    rest: Vec<u8>,
    seen: &mut std::collections::HashMap<Vec<u8>, Option<Vec<u8>>>,
) -> io::Result<(Vec<u8>, bool)> {
    let mut rest: &[u8] = &rest;
    if rest.first() == Some(&b'/') {
        rest = &rest[1..];
        path = b"/".to_vec();
    }
    let rest_owned = rest.to_vec();
    let mut rest: &[u8] = &rest_owned;
    while !rest.is_empty() {
        let (name, tail) = match rest.iter().position(|&c| c == b'/') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, &b""[..]),
        };
        rest = tail;
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            if !path.is_empty() {
                let (head, last) = py_split(&path);
                path = head;
                if last == b".." {
                    path = py_join(&py_join(&path, b".."), b"..");
                }
            } else {
                path = b"..".to_vec();
            }
            continue;
        }
        let newpath = py_join(&path, name);
        let is_link = std::fs::symlink_metadata(OsStr::from_bytes(&newpath))
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(cached) = seen.get(&newpath) {
            match cached {
                Some(resolved) => {
                    path = resolved.clone();
                    continue;
                }
                None => return Ok((py_join(&newpath, rest), false)),
            }
        }
        seen.insert(newpath.clone(), None);
        let target = std::fs::read_link(OsStr::from_bytes(&newpath))?;
        let (resolved, ok) = join_realpath(path, target.into_os_string().into_vec(), seen)?;
        path = resolved;
        if !ok {
            return Ok((py_join(&path, rest), false));
        }
        seen.insert(newpath, Some(path.clone()));
    }
    Ok((path, true))
}

/// `str(pathlib.PurePosixPath(p))` (CPython 3.12): repeated `/` collapsed except exactly two
/// leading ones, `.` parts and a trailing `/` dropped, `..` kept, `""` is `"."`.
pub(crate) fn pathlib_str(p: &[u8]) -> Vec<u8> {
    let lead = p.iter().take_while(|&&c| c == b'/').count();
    let root: &[u8] = match lead {
        0 => b"",
        2 => b"//",
        _ => b"/",
    };
    let parts: Vec<&[u8]> = p
        .split(|&c| c == b'/')
        .filter(|c| !c.is_empty() && *c != b".")
        .collect();
    let mut out = root.to_vec();
    out.extend_from_slice(&parts.join(&b'/'));
    if out.is_empty() {
        out.push(b'.');
    }
    out
}

/// `str(pathlib.PurePosixPath(p).parent)`.
pub(crate) fn pathlib_parent(p: &[u8]) -> Vec<u8> {
    let s = pathlib_str(p);
    let lead = s.iter().take_while(|&&c| c == b'/').count();
    let body = &s[lead..];
    if body.is_empty() || body == b"." {
        return s; // the root, or "."
    }
    match body.iter().rposition(|&c| c == b'/') {
        Some(i) => {
            let mut out = s[..lead].to_vec();
            out.extend_from_slice(&body[..i]);
            out
        }
        None if lead > 0 => s[..lead].to_vec(),
        None => b".".to_vec(),
    }
}

/// `str(PurePosixPath(a) / b)` for a single name `b`.
pub(crate) fn pathlib_child(a: &[u8], b: &[u8]) -> Vec<u8> {
    let a = pathlib_str(a);
    if a == b"." {
        return b.to_vec();
    }
    let mut out = a;
    if out.last() != Some(&b'/') {
        out.push(b'/');
    }
    out.extend_from_slice(b);
    out
}

/// Whether `Path.exists()`/`is_dir()` style errors are the ones pathlib turns into `False`.
fn ignored_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::NotFound || matches!(e.raw_os_error(), Some(ENOTDIR | EBADF | ELOOP))
}

/// `Path.exists()` (CPython 3.12): `Err` for the errors pathlib re-raises.
fn py_exists(p: &Path) -> io::Result<bool> {
    match std::fs::metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if ignored_error(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `Path.is_dir()` (CPython 3.12): `Err` for the errors pathlib re-raises.
fn py_is_dir(p: &Path) -> io::Result<bool> {
    match std::fs::metadata(p) {
        Ok(m) => Ok(m.is_dir()),
        Err(e) if ignored_error(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

/// `Path.is_file()`, with every error read as `False` (where Python re-raises, it crashed: D-1).
pub(crate) fn is_file(p: &Path) -> bool {
    !has_nul(p.as_os_str().as_bytes()) && std::fs::metadata(p).is_ok_and(|m| m.is_file())
}

/// The nearest folder at or above `cwd` holding a `.git` (folder or file), or `None`. No git.
pub fn repo_top(cwd: &Path) -> Option<PathBuf> {
    repo_top_with(cwd, &py_exists)
}

/// [`repo_top`] with `Path.exists` injected (the `pathlib.Path.exists` patch seam, GC-19): an
/// `Err` from it ends the walk with `None`, as Python's `except OSError` does.
pub fn repo_top_with(cwd: &Path, exists: &dyn Fn(&Path) -> io::Result<bool>) -> Option<PathBuf> {
    let here = realpath(cwd).ok()?;
    let mut folder: Option<&Path> = Some(&here);
    while let Some(f) = folder {
        match exists(&f.join(".git")) {
            Ok(true) => return Some(f.to_path_buf()),
            Ok(false) => {}
            Err(_) => return None,
        }
        folder = f.parent();
    }
    None
}

// ---------------------------------------------------------------------------------------------
// Checks

/// Whether `path` is executable for this process (`os.access(path, X_OK)`), approximated as
/// "some execute bit is set": the wrappers are files of the user's own plugin install.
fn executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

/// Names of the PATH wrappers ([`WRAPPERS`]) that are missing or not executable under the
/// plugin root.
pub fn wrapper_problems(plugin_root: &Path) -> Vec<String> {
    WRAPPERS
        .iter()
        .filter(|name| {
            let path = plugin_root.join("bin").join(name);
            !(is_file(&path) && executable(&path))
        })
        .map(|name| (*name).to_string())
        .collect()
}

/// Folders under `<repo>/.claude/agent-memory/` named like a roster agent without the
/// `anthology-` prefix (`implementer`, `anthology:implementer`, `anthology_implementer`):
/// Claude Code may not be mapping `anthology:<agent>` to `anthology-<agent>/`. `bad` is only
/// filled when no `anthology-<agent>/` folder exists for any roster agent. Returns
/// `(bad, good)`; both are empty when there is no such folder or no repo.
pub fn agent_memory_problems(cwd: &Path) -> (Vec<String>, Vec<String>) {
    let Some(top) = repo_top(cwd) else {
        return (Vec::new(), Vec::new());
    };
    let Some(names) = dir_names(&top.join(".claude").join("agent-memory")) else {
        return (Vec::new(), Vec::new());
    };
    let (mut bad, mut good) = (Vec::new(), Vec::new());
    for name in &names {
        for agent in ROSTER {
            if *name == agent
                || *name == format!("anthology:{agent}")
                || *name == format!("anthology_{agent}")
            {
                bad.push(name.clone());
            } else if *name == format!("anthology-{agent}") {
                good.push(name.clone());
            }
        }
    }
    if !good.is_empty() {
        return (Vec::new(), good);
    }
    (bad, good)
}

/// `sorted(p.name for p in folder.iterdir() if p.is_dir())`, or `None` on an `OSError`. Names
/// that are not UTF-8 can never match a roster form, so they are left out.
fn dir_names(folder: &Path) -> Option<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(folder).ok()? {
        let entry = entry.ok()?;
        if py_is_dir(&entry.path()).ok()?
            && let Ok(name) = entry.file_name().into_string()
        {
            names.push(name);
        }
    }
    names.sort();
    Some(names)
}

/// The soft warning for a bare roster-name folder (shared by doctor and session start).
pub fn agent_memory_hint(name: &str) -> String {
    format!(
        "`.claude/agent-memory/{name}/` exists but no `anthology-{name}/` does: either Claude \
         Code changed the agent-memory path or this is an agent of your own named {name}; if \
         anthology agents have run here, report it"
    )
}

fn env_has_model(data: Option<&Value>) -> bool {
    data.and_then(Value::as_object)
        .and_then(|o| o.get("env"))
        .and_then(Value::as_object)
        .and_then(|e| e.get(SUBAGENT_MODEL))
        .and_then(Value::as_str)
        .is_some_and(|v| !py_strip(v).is_empty())
}

/// Where `CLAUDE_CODE_SUBAGENT_MODEL` is set (never its value), or `None`: "the process
/// environment", "user settings.json", "the project's .claude/settings.json" or "the project's
/// .claude/settings.local.json".
pub fn subagent_model_source(env: &Env, cwd: &Path) -> Option<&'static str> {
    if let Some(v) = env.get_os(SUBAGENT_MODEL)
        && !py_strip(&v.to_string_lossy()).is_empty()
    {
        return Some("the process environment");
    }
    if env_has_model(read_json(&config_dir(env).join("settings.json")).as_ref()) {
        return Some("user settings.json");
    }
    let top = repo_top(cwd).unwrap_or_else(|| cwd.to_path_buf());
    // the project's two files come with the clone, so they are bounded
    for (label, name) in [
        ("the project's .claude/settings.json", "settings.json"),
        (
            "the project's .claude/settings.local.json",
            "settings.local.json",
        ),
    ] {
        let data = read_json_limited(&top.join(".claude").join(name), REPO_READ_LIMIT);
        if env_has_model(data.as_ref()) {
            return Some(label);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathlib_forms_match_cpython() {
        for (p, want) in [
            ("", "."),
            (".", "."),
            ("a/", "a"),
            ("a//b/./c/", "a/b/c"),
            ("//a", "//a"),
            ("///a", "/a"),
            ("/", "/"),
            ("//", "//"),
            ("./a/../b", "a/../b"),
        ] {
            assert_eq!(pathlib_str(p.as_bytes()), want.as_bytes(), "{p:?}");
        }
        for (p, want) in [
            ("a", "."),
            ("/a", "/"),
            ("//a", "//"),
            ("/", "/"),
            ("a/b/", "a"),
            ("", "."),
        ] {
            assert_eq!(pathlib_parent(p.as_bytes()), want.as_bytes(), "{p:?}");
        }
        assert_eq!(pathlib_child(b".", b"x"), b"x");
        assert_eq!(pathlib_child(b"/", b"x"), b"/x");
        assert_eq!(pathlib_child(b"a/", b"x"), b"a/x");
    }

    #[test]
    fn normpath_matches_cpython() {
        for (p, want) in [
            ("", "."),
            ("/..", "/"),
            ("a/../..", ".."),
            ("../a/..", ".."),
            ("//x/../y", "//y"),
            ("///x", "/x"),
            ("a/./b//", "a/b"),
        ] {
            assert_eq!(py_normpath(p.as_bytes()), want.as_bytes(), "{p:?}");
        }
    }

    #[test]
    fn utf16_replacement_follows_cpython() {
        assert_eq!(utf16_replace(b"a\x00", u16::from_le_bytes), "a");
        assert_eq!(utf16_replace(b"a\x00b", u16::from_le_bytes), "a\u{fffd}");
        assert_eq!(
            utf16_replace(b"\x00\xd8a\x00", u16::from_le_bytes),
            "\u{fffd}a"
        );
        assert_eq!(utf16_replace(b"\x00\xdc", u16::from_le_bytes), "\u{fffd}");
        assert_eq!(
            utf16_replace(b"\x3d\xd8\x00\xde", u16::from_le_bytes),
            "\u{1f600}"
        );
    }

    #[test]
    fn json_depth_skips_strings() {
        assert_eq!(json_depth(r#"{"a": "[[[", "b": [[1]]}"#), 3);
        assert_eq!(json_depth(r#""\"[""#), 0);
    }
}
