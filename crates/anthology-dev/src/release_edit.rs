//! `anthology-dev release-edit` (scripts/release_edit.py): text and JSON helpers for release.sh.
//! Owner: DV2.
//!
//! Subcommands (a failure prints `release: <reason>` to stderr and exits 1):
//! `plugin-version`, `check-version`, `check-dependents`, `changelog-check`, `changelog-move`,
//! `set-version`, `set-source-version`. Edits are made in place and are minimal: the rest of each
//! file stays byte-identical.
//!
//! Changelog fragments (Rust only, no Python counterpart): `changelog-check` and `changelog-move`
//! also read `changelog.d/<plugin>/*.md` beside the CHANGELOG. With no such folder both behave
//! exactly as the Python did. `changelog-move` deletes the fragments it compiled.
//!
//! The library functions mirror the Python ones, so `tests/release_edit.rs` drives them as
//! `tests/test_release.py` does.

use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::clock;
use anthology_pycompat::argparse::{self, Arg, Subcommands, Value as ArgValue};
use anthology_pycompat::json::{self, Value};
use anthology_pycompat::pyerr::PyExc;

use crate::Output;
use crate::json_edit::{
    EditError, Token, Uncaught, member_value, py_repr, replace_string_value, str_repr, tokens,
    utf8_decode_error,
};
use crate::memory_diff::pystr::splitlines;
use crate::semver::{self, RangeError};
use anthology_pycompat::str as pystr;

/// `ReleaseError`: a precondition failed or a file could not be edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseError(pub String);

impl fmt::Display for ReleaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReleaseError {}

impl From<EditError> for ReleaseError {
    fn from(e: EditError) -> ReleaseError {
        ReleaseError(e.0)
    }
}

/// What can end a command: a `ReleaseError`, an `OSError` (both print `release: …`, exit 1), or
/// an exception Python does not catch (ruling R10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Release(ReleaseError),
    /// `str(OSError)`.
    Os(String),
    Uncaught(Uncaught),
}

impl From<ReleaseError> for Failure {
    fn from(e: ReleaseError) -> Failure {
        Failure::Release(e)
    }
}

// ---- version checks ------------------------------------------------------------------

/// `check_version`: `bump` if `new` > `current`; `same` if equal and its tag does not exist yet.
pub fn check_version(
    current: &str,
    new: &str,
    tag_exists: bool,
) -> Result<&'static str, ReleaseError> {
    let Some(new_v) = semver::parse(new) else {
        return Err(ReleaseError(format!(
            "{} is not a valid semver version (expected X.Y.Z)",
            str_repr(new)
        )));
    };
    let Some(current_v) = semver::parse(current) else {
        return Err(ReleaseError(format!(
            "the current plugin version {} is not valid semver",
            str_repr(current)
        )));
    };
    match semver::cmp_parsed(&new_v, &current_v) {
        Ordering::Greater => Ok("bump"),
        Ordering::Equal if tag_exists => Err(ReleaseError(format!(
            "version {new} is already released (its tag exists)"
        ))),
        Ordering::Equal => Ok("same"),
        Ordering::Less => Err(ReleaseError(format!(
            "{new} is lower than the current version {current}"
        ))),
    }
}

// ---- dependency ranges ---------------------------------------------------------------

/// `pathlib`'s `str(Path(p))`: empty and `.` components dropped, `.` for nothing, a leading `//`
/// (exactly two slashes) kept.
fn pathlib_str(p: &str) -> String {
    let lead = if p.starts_with("//") && !p.starts_with("///") {
        "//"
    } else if p.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = p
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if parts.is_empty() && lead.is_empty() {
        ".".into()
    } else {
        format!("{lead}{}", parts.join("/"))
    }
}

/// `str(OSError)` of `e` raised for `filename` (the text is P1's placeholder, ruling R3).
fn os_error_str(e: std::io::Error, filename: &str) -> String {
    match PyExc::from(e) {
        PyExc::OSError { errno, .. } => PyExc::OSError {
            errno,
            filename: Some(filename.to_string()),
        },
        other => other,
    }
    .py_str()
}

/// The entry names of `dir`; any error (a directory that cannot be listed, a bad entry) gives
/// `None`, as pathlib's `scandir` listing swallows it.
fn list_names(dir: &Path) -> Option<Vec<OsString>> {
    fs::read_dir(dir)
        .ok()?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<Result<Vec<_>, _>>()
        .ok()
}

/// `sorted(plugins_dir.glob("*/.claude-plugin/plugin.json"))` as Python 3.12 evaluates it: each
/// part is matched by name against a directory listing (case-sensitively, hidden names
/// included); a listing that fails is skipped; `*` and `.claude-plugin` must be directories
/// (symlinks followed) and `plugin.json` may be anything, a dangling symlink included. Sorted by
/// plugin directory name (pathlib compares path components).
///
/// `select_from` starts with `Path.is_dir()`, which ignores only ENOENT, ENOTDIR, EBADF and
/// ELOOP; any other error from stat-ing `plugins_dir` is returned.
fn glob_manifests(plugins_dir: &Path) -> Result<Vec<(OsString, PathBuf)>, std::io::Error> {
    #[cfg(target_os = "macos")]
    const ELOOP: i32 = 62;
    #[cfg(not(target_os = "macos"))]
    const ELOOP: i32 = 40;
    match fs::metadata(plugins_dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Ok(Vec::new()),
        Err(e) if matches!(e.raw_os_error(), Some(2 | 20 | 9 | ELOOP)) => {
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    }
    let mut names = list_names(plugins_dir).unwrap_or_default();
    names.sort();
    let mut found = Vec::new();
    for name in names {
        let dir = plugins_dir.join(&name);
        if !dir.is_dir() {
            continue;
        }
        let Some(inner) = list_names(&dir) else {
            continue;
        };
        let cp = dir.join(".claude-plugin");
        if !inner.iter().any(|n| n == ".claude-plugin") || !cp.is_dir() {
            continue;
        }
        let Some(files) = list_names(&cp) else {
            continue;
        };
        if files.iter().any(|n| n == "plugin.json") {
            found.push((name, cp.join("plugin.json")));
        }
    }
    Ok(found)
}

/// `check_dependents`: the problems (one message each) for plugins under `root/plugins` whose
/// dependency range on `plugin` excludes `version`.
pub fn check_dependents(root: &Path, plugin: &str, version: &str) -> Result<Vec<String>, Failure> {
    check_dependents_in(Path::new(""), root, plugin, version)
}

/// [`check_dependents`] with a relative `root` taken from `cwd`.
fn check_dependents_in(
    cwd: &Path,
    root: &Path,
    plugin: &str,
    version: &str,
) -> Result<Vec<String>, Failure> {
    let mut problems = Vec::new();
    // `str(Path(root) / rel)`, for an OSError's filename.
    let shown_root = pathlib_str(&root.to_string_lossy());
    let shown = |rel: &str| match shown_root.as_str() {
        "." => rel.to_string(),
        r if r.ends_with('/') => format!("{r}{rel}"),
        r => format!("{r}/{rel}"),
    };
    let plugins_dir = cwd.join(root).join("plugins");
    let manifests = glob_manifests(&plugins_dir)
        .map_err(|e| Failure::Os(os_error_str(e, &shown("plugins"))))?;
    for (name, manifest) in manifests {
        let dependent = name.to_string_lossy().into_owned();
        let rel = format!("plugins/{dependent}/.claude-plugin/plugin.json");
        let bytes = fs::read(&manifest).map_err(|e| Failure::Os(os_error_str(e, &shown(&rel))))?;
        let data = match String::from_utf8(bytes) {
            Ok(text) => json::loads(&text).ok(),
            Err(_) => None,
        };
        let Some(data) = data else {
            problems.push(format!(
                "{rel} is not valid JSON; can't check its dependencies"
            ));
            continue;
        };
        let deps = data
            .as_object()
            .and_then(|m| m.get("dependencies"))
            .and_then(Value::as_array);
        for dep in deps.into_iter().flatten() {
            // A bare name carries no range; anything but an object names no plugin.
            let Some(dep) = dep.as_object() else {
                continue;
            };
            if dep.get("name").and_then(Value::as_str) != Some(plugin) {
                continue;
            }
            let Some(rng) = dep.get("version") else {
                continue;
            };
            if rng.is_null() {
                continue;
            }
            match semver::range_accepts(rng, version) {
                Ok(true) => {}
                Ok(false) => problems.push(format!(
                    "{rel} depends on {plugin} {}, which does not accept {version}; widen \
                     that range first (it is an {dependent} change and needs its own version \
                     bump)",
                    rng.as_str().unwrap_or_default()
                )),
                Err(RangeError::Unsupported(_)) => problems.push(format!(
                    "{rel} depends on {plugin} with range {}: can't check range (supported: \
                     ~X, ~X.Y, ~X.Y.Z, ^X, ^X.Y, ^X.Y.Z, X.Y.Z, >=X.Y.Z)",
                    py_repr(rng)
                )),
                Err(RangeError::BadVersion(m)) => return Err(ReleaseError(m).into()),
                Err(RangeError::Uncaught { ty, msg }) => {
                    return Err(Failure::Uncaught(Uncaught::new(ty, msg)));
                }
            }
        }
    }
    Ok(problems)
}

// ---- CHANGELOG -----------------------------------------------------------------------

/// The `### [Unreleased]` heading.
pub const UNRELEASED: &str = "### [Unreleased]";

fn split_lines(text: &str) -> Vec<String> {
    splitlines(text, true)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `(start, end)` of `## <plugin>` up to the next `## ` heading.
fn plugin_span(lines: &[String], plugin: &str) -> Result<(usize, usize), ReleaseError> {
    let wanted = format!("## {plugin}");
    let mut start = None;
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("## ") && !line.starts_with("### ") {
            if let Some(s) = start {
                return Ok((s, i));
            }
            if pystr::strip_ws(line) == wanted {
                start = Some(i);
            }
        }
    }
    match start {
        Some(s) => Ok((s, lines.len())),
        None => Err(ReleaseError(format!(
            "CHANGELOG.md has no `## {plugin}` section"
        ))),
    }
}

/// `(heading index, first body index, end index exclusive)` of the Unreleased section.
fn unreleased_span(lines: &[String], plugin: &str) -> Result<(usize, usize, usize), ReleaseError> {
    let (p_start, p_end) = plugin_span(lines, plugin)?;
    for i in p_start + 1..p_end {
        if pystr::strip_ws(&lines[i]) == UNRELEASED {
            let end = (i + 1..p_end)
                .find(|&j| lines[j].starts_with("### "))
                .unwrap_or(p_end);
            return Ok((i, i + 1, end));
        }
    }
    Err(ReleaseError(format!(
        "CHANGELOG.md `## {plugin}` has no `{UNRELEASED}` heading"
    )))
}

/// The body lines without surrounding blank lines.
fn content(body: &[String]) -> &[String] {
    let blank = |l: &String| pystr::strip_ws(l).is_empty();
    let mut lo = 0;
    let mut hi = body.len();
    while lo < hi && blank(&body[lo]) {
        lo += 1;
    }
    while hi > lo && blank(&body[hi - 1]) {
        hi -= 1;
    }
    &body[lo..hi]
}

fn has_entries(body: &[String]) -> bool {
    // Subheadings alone (`#### Added` with nothing under it) are an empty skeleton.
    body.iter()
        .any(|l| !pystr::strip_ws(l).is_empty() && !l.starts_with('#'))
}

/// `changelog_check`: the plugin's `### [Unreleased]` has content.
pub fn changelog_check(text: &str, plugin: &str) -> Result<(), ReleaseError> {
    changelog_check_with(text, plugin, &[])
}

/// [`changelog_check`], where the texts of the plugin's changelog fragments count as content too.
pub fn changelog_check_with(
    text: &str,
    plugin: &str,
    fragments: &[String],
) -> Result<(), ReleaseError> {
    let lines = split_lines(text);
    let (_h, first, end) = unreleased_span(&lines, plugin)?;
    if !has_entries(&lines[first..end]) && !fragments.iter().any(|f| has_entries(&split_lines(f))) {
        return Err(ReleaseError(format!(
            "the `{UNRELEASED}` section of {plugin} in CHANGELOG.md is empty; write the \
             release notes there first"
        )));
    }
    Ok(())
}

/// `changelog_move`: moves the plugin's Unreleased content into a new `### [version] - date`
/// section.
pub fn changelog_move(
    text: &str,
    plugin: &str,
    version: &str,
    date: &str,
) -> Result<String, ReleaseError> {
    changelog_move_with(text, plugin, version, date, &[])
}

/// [`changelog_move`] with the plugin's changelog fragments (in the order given: filename order)
/// added after the Unreleased content. A fragment that opens with a heading starts a new block
/// (a blank line before it); any other fragment continues the lines above it. Fragments without
/// entries are skipped.
pub fn changelog_move_with(
    text: &str,
    plugin: &str,
    version: &str,
    date: &str,
    fragments: &[String],
) -> Result<String, ReleaseError> {
    changelog_check_with(text, plugin, fragments)?;
    let mut lines = split_lines(text);
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push('\n');
    }
    let (p_start, p_end) = plugin_span(&lines, plugin)?;
    let wanted = format!("### [{version}]");
    if lines[p_start..p_end].iter().any(|l| l.starts_with(&wanted)) {
        return Err(ReleaseError(format!(
            "CHANGELOG.md already has a {version} section for {plugin}"
        )));
    }
    let (heading, first, end) = unreleased_span(&lines, plugin)?;
    let mut body: Vec<String> = content(&lines[first..end]).to_vec();
    for fragment in fragments {
        let frag_lines = split_lines(fragment);
        if !has_entries(&frag_lines) {
            continue;
        }
        let piece = content(&frag_lines).to_vec();
        if !body.is_empty() && piece[0].starts_with('#') {
            body.push("\n".to_string());
        }
        body.extend(piece);
        if let Some(last) = body.last_mut()
            && !last.ends_with('\n')
        {
            last.push('\n');
        }
    }
    let mut out = String::new();
    for l in &lines[..heading] {
        out.push_str(l);
    }
    out.push_str(&lines[heading]);
    out.push('\n');
    out.push_str(&format!("### [{version}] - {date}\n"));
    out.push('\n');
    for l in &body {
        out.push_str(l);
    }
    if end < lines.len() {
        out.push('\n');
    }
    for l in &lines[end..] {
        out.push_str(l);
    }
    Ok(out)
}

// ---- minimal JSON edits --------------------------------------------------------------

/// `plugin_version`: the top-level `"version"` string of a plugin.json.
pub fn plugin_version(text: &str) -> Result<String, ReleaseError> {
    let data = json::loads(text)
        .ok()
        .filter(|v| v.as_object().is_some())
        .ok_or_else(|| ReleaseError("plugin.json is not a valid JSON object".into()))?;
    match data.as_object().and_then(|m| m.get("version")) {
        Some(Value::Str(s)) => Ok(s.clone()),
        _ => Err(ReleaseError(
            "plugin.json has no top-level string \"version\"".into(),
        )),
    }
}

/// `set_plugin_version`: replaces only the top-level `"version"` string.
pub fn set_plugin_version(text: &str, version: &str) -> Result<String, ReleaseError> {
    plugin_version(text)?; // validates the shape
    let toks = tokens(text)?;
    if toks.first().is_none_or(|t| t.kind != '{') {
        return Err(ReleaseError("plugin.json is not a JSON object".into()));
    }
    let Some(idx) = member_value(text, &toks, 0, "version") else {
        return Err(ReleaseError(
            "plugin.json has no top-level \"version\"".into(),
        ));
    };
    Ok(replace_string_value(
        text,
        &toks,
        idx,
        version,
        "plugin.json \"version\"",
    )?)
}

/// Python's `type(v).__name__` for a JSON value.
fn py_type(v: &Value) -> &'static str {
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

/// The token indexes of the `{` entries directly inside the array opened at token `open`.
fn array_entries(toks: &[Token], open: usize) -> Vec<usize> {
    let mut depth = 0i64;
    let mut entries = Vec::new();
    for (k, t) in toks.iter().enumerate().skip(open) {
        match t.kind {
            '{' | '[' => {
                if t.kind == '{' && depth == 1 {
                    entries.push(k);
                }
                depth += 1;
            }
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    entries
}

/// `set_source_plugin_version`: replaces only the `"plugin_version"` of the sources.json entry
/// whose `"target"` is `target`.
pub fn set_source_plugin_version(
    text: &str,
    target: &str,
    version: &str,
) -> Result<String, Failure> {
    let data =
        json::loads(text).map_err(|_| ReleaseError("sources.json is not valid JSON".into()))?;
    let toks = tokens(text).map_err(ReleaseError::from)?;
    let no_array = || ReleaseError("sources.json has no \"sources\" array".into());
    let sources_val = member_value(text, &toks, 0, "sources").ok_or_else(no_array)?;
    if toks[sources_val].kind != '[' {
        return Err(no_array().into());
    }
    // Each `{` at relative depth 1 starts an entry.
    let entries = array_entries(&toks, sources_val);
    // `data["sources"]` is the LAST duplicate key; the token walk found the first.
    let Some(sources) = data.as_object().and_then(|m| m.get("sources")) else {
        return Err(no_array().into());
    };
    let len = match sources {
        Value::Array(a) => a.len(),
        Value::Object(m) => m.len(),
        Value::Str(s) => s.chars().count(),
        other => {
            return Err(Failure::Uncaught(Uncaught::new(
                "TypeError",
                format!("object of type '{}' has no len()", py_type(other)),
            )));
        }
    };
    if len != entries.len() {
        return Err(ReleaseError("could not map sources.json entries".into()).into());
    }
    let Value::Array(items) = sources else {
        // Iterating a dict or a str yields strings, which have no `.get`.
        if len > 0 {
            return Err(Failure::Uncaught(Uncaught::new(
                "AttributeError",
                "'str' object has no attribute 'get'",
            )));
        }
        return Err(no_entry(target));
    };
    for (entry, &open_idx) in items.iter().zip(&entries) {
        let Some(entry) = entry.as_object() else {
            return Err(Failure::Uncaught(Uncaught::new(
                "AttributeError",
                format!("'{}' object has no attribute 'get'", py_type(entry)),
            )));
        };
        if entry.get("target").and_then(Value::as_str) != Some(target) {
            continue;
        }
        let Some(idx) = member_value(text, &toks, open_idx, "plugin_version") else {
            return Err(ReleaseError(format!(
                "the sources.json entry for {target} has no plugin_version"
            ))
            .into());
        };
        return replace_string_value(text, &toks, idx, version, "plugin_version")
            .map_err(|e| ReleaseError::from(e).into());
    }
    Err(no_entry(target))
}

fn no_entry(target: &str) -> Failure {
    ReleaseError(format!(
        "sources.json has no entry with target {}",
        str_repr(target)
    ))
    .into()
}

// ---- command line --------------------------------------------------------------------

/// `main`'s `ArgumentParser` and its subparsers (`prog` is the script's name, as `sys.argv[0]`
/// gives it). `COLUMNS` sets the help width, as `shutil.get_terminal_size` reads it.
fn spec(env: &Env) -> argparse::Spec {
    let sub = |names: &[&str]| {
        names.iter().fold(argparse::Spec::command(), |spec, name| {
            spec.arg(Arg::positional(name))
        })
    };
    let subcommands = Subcommands::new()
        .dest("cmd")
        .required()
        .parser("plugin-version", None, sub(&["path"]))
        .parser(
            "check-version",
            None,
            sub(&["current", "new"]).arg(Arg::positional("tag_exists").choices(&["0", "1"])),
        )
        .parser(
            "check-dependents",
            None,
            sub(&["root", "plugin", "version"]),
        )
        .parser("changelog-check", None, sub(&["path", "plugin"]))
        .parser(
            "changelog-move",
            None,
            sub(&["path", "plugin", "version"])
                .arg(Arg::option(&["--date"]).default(ArgValue::str(""))),
        )
        .parser("set-version", None, sub(&["path", "version"]))
        .parser(
            "set-source-version",
            None,
            sub(&["path", "target", "version"]),
        );
    argparse::Spec::new("release_edit.py")
        .description(
            "Text and JSON helpers for scripts/release.sh (the shell orchestrates git and claude).",
        )
        .width(argparse::help_width(env.get("COLUMNS"), None))
        .subcommands(subcommands)
}

/// Runs `anthology-dev release-edit …`; `args` are the arguments after `release-edit`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, stdin, &Env::from_process(), &cwd)
}

/// `release_edit.main(args)`, with relative paths resolved against `cwd`.
pub fn run_in(args: &[OsString], stdin: &mut dyn Read, env: &Env, cwd: &Path) -> Output {
    let _ = stdin;
    let parsed = match argparse::parse(&spec(env), args) {
        Ok(p) => p,
        Err(e) => {
            return Output {
                stdout: e.stdout.into_bytes(),
                stderr: e.stderr.into_bytes(),
                code: e.code,
            };
        }
    };
    let cmd = parsed.str("cmd").unwrap_or_default().to_string();
    let mut out = Output::default();
    match execute(&cmd, &parsed, cwd, &mut out) {
        Ok(code) => {
            out.code = code;
            out
        }
        Err(Failure::Release(e)) => {
            out.stderr
                .extend_from_slice(format!("release: {e}\n").as_bytes());
            out.code = 1;
            out
        }
        Err(Failure::Os(m)) => {
            out.stderr
                .extend_from_slice(format!("release: {m}\n").as_bytes());
            out.code = 1;
            out
        }
        Err(Failure::Uncaught(u)) => u.into_output("release-edit", out.stdout),
    }
}

/// The file `open(path)` would open: an empty `path` is `ENOENT` (joined to `cwd` it would be
/// the directory itself).
fn open_path(cwd: &Path, path: &OsStr) -> Result<PathBuf, Failure> {
    if path.is_empty() {
        let enoent = std::io::Error::from_raw_os_error(2);
        return Err(Failure::Os(os_error_str(enoent, "")));
    }
    Ok(cwd.join(path))
}

fn read_text(cwd: &Path, path: &OsStr) -> Result<String, Failure> {
    let shown = path.to_string_lossy();
    let bytes =
        fs::read(open_path(cwd, path)?).map_err(|e| Failure::Os(os_error_str(e, &shown)))?;
    match String::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(e) => Err(Failure::Uncaught(Uncaught::new(
            "UnicodeDecodeError",
            utf8_decode_error(e.as_bytes()).unwrap_or_default(),
        ))),
    }
}

fn write_text(cwd: &Path, path: &OsStr, text: &str) -> Result<(), Failure> {
    let shown = path.to_string_lossy();
    fs::write(open_path(cwd, path)?, text).map_err(|e| Failure::Os(os_error_str(e, &shown)))
}

/// The changelog fragments of `plugin`: the regular `*.md` files directly inside
/// `changelog.d/<plugin>/` beside the CHANGELOG (hidden files and other extensions are not
/// fragments), as (path, text) in filename order. No such folder means no fragments.
fn read_fragments(
    cwd: &Path,
    changelog: &OsStr,
    plugin: &str,
) -> Result<Vec<(PathBuf, String)>, Failure> {
    if plugin.is_empty() || plugin == "." || plugin == ".." || plugin.contains(['/', '\\']) {
        return Ok(Vec::new());
    }
    let changelog = open_path(cwd, changelog)?;
    let dir = changelog
        .parent()
        .unwrap_or(cwd)
        .join("changelog.d")
        .join(plugin);
    let shown = dir.to_string_lossy().into_owned();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Failure::Os(os_error_str(e, &shown))),
    };
    let mut names: Vec<OsString> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Failure::Os(os_error_str(e, &shown)))?;
        let name = entry.file_name();
        let lossy = name.to_string_lossy();
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if is_file && !lossy.starts_with('.') && lossy.ends_with(".md") {
            names.push(name);
        }
    }
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let path = dir.join(name);
            let text = read_text(cwd, path.as_os_str())?;
            Ok((path, text))
        })
        .collect()
}

fn execute(cmd: &str, a: &argparse::Parsed, cwd: &Path, out: &mut Output) -> Result<i32, Failure> {
    let arg = |name: &str| a.os_str(name).unwrap_or_default();
    let sarg = |name: &str| arg(name).to_string_lossy().into_owned();
    fn say(out: &mut Output, s: &str) {
        out.stdout.extend_from_slice(s.as_bytes());
        out.stdout.push(b'\n');
    }
    match cmd {
        "plugin-version" => {
            let version = plugin_version(&read_text(cwd, arg("path"))?)?;
            say(out, &version);
        }
        "check-version" => {
            let verdict = check_version(&sarg("current"), &sarg("new"), sarg("tag_exists") == "1")?;
            say(out, verdict);
        }
        "check-dependents" => {
            let problems = check_dependents_in(
                cwd,
                Path::new(arg("root")),
                &sarg("plugin"),
                &sarg("version"),
            )?;
            if !problems.is_empty() {
                for msg in problems {
                    out.stderr
                        .extend_from_slice(format!("release: {msg}\n").as_bytes());
                }
                return Ok(1);
            }
        }
        "changelog-check" => {
            let text = read_text(cwd, arg("path"))?;
            let fragments = read_fragments(cwd, arg("path"), &sarg("plugin"))?;
            let texts: Vec<String> = fragments.into_iter().map(|(_, t)| t).collect();
            changelog_check_with(&text, &sarg("plugin"), &texts)?;
        }
        "changelog-move" => {
            let given = a.os_str("date").map(|d| d.to_string_lossy().into_owned());
            let date = match given.filter(|d| !d.is_empty()) {
                Some(d) => d,
                None => clock::today_local().iso(),
            };
            if !is_iso_date(&date) {
                return Err(
                    ReleaseError(format!("date {} is not YYYY-MM-DD", str_repr(&date))).into(),
                );
            }
            let text = read_text(cwd, arg("path"))?;
            let fragments = read_fragments(cwd, arg("path"), &sarg("plugin"))?;
            let texts: Vec<String> = fragments.iter().map(|(_, t)| t.clone()).collect();
            let moved =
                changelog_move_with(&text, &sarg("plugin"), &sarg("version"), &date, &texts)?;
            write_text(cwd, arg("path"), &moved)?;
            // The fragments now live in the CHANGELOG: consume them (the release commit records
            // the deletion). The emptied folder goes too, when nothing else is in it.
            for (path, _) in &fragments {
                let shown = path.to_string_lossy();
                fs::remove_file(path).map_err(|e| Failure::Os(os_error_str(e, &shown)))?;
            }
            if let Some(dir) = fragments.first().and_then(|(p, _)| p.parent()) {
                let _ = fs::remove_dir(dir);
            }
        }
        "set-version" => {
            let edited = set_plugin_version(&read_text(cwd, arg("path"))?, &sarg("version"))?;
            write_text(cwd, arg("path"), &edited)?;
        }
        _ => {
            let edited = set_source_plugin_version(
                &read_text(cwd, arg("path"))?,
                &sarg("target"),
                &sarg("version"),
            )?;
            write_text(cwd, arg("path"), &edited)?;
        }
    }
    Ok(0)
}

/// `re.fullmatch(r"\d{4}-\d{2}-\d{2}", date)`, with Python's Unicode `\d`.
fn is_iso_date(date: &str) -> bool {
    let chars: Vec<char> = date.chars().collect();
    chars.len() == 10
        && chars.iter().enumerate().all(|(i, &c)| match i {
            4 | 7 => c == '-',
            _ => pystr::is_decimal(c),
        })
}
