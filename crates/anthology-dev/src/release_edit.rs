//! `anthology-dev release-edit` (scripts/release_edit.py): text and JSON helpers for release.sh.
//! Owner: DV2.
//!
//! Subcommands (a failure prints `release: <reason>` to stderr and exits 1):
//! `plugin-version`, `check-version`, `check-dependents`, `changelog-check`, `changelog-move`,
//! `set-version`, `set-source-version`. Edits are made in place and are minimal: the rest of each
//! file stays byte-identical.
//!
//! The library functions mirror the Python ones, so `tests/release_edit.rs` drives them as
//! `tests/test_release.py` does.

pub mod argp;

use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::clock;
use anthology_pycompat::json::{self, Value};
use anthology_pycompat::pyerr::PyExc;

use crate::Output;
use crate::json_edit::{
    EditError, Token, Uncaught, member_value, py_repr, replace_string_value, str_repr, tokens,
    utf8_decode_error,
};
use crate::memory_diff::pystr;
use crate::semver::{self, RangeError};

use argp::{OptSpec, Parser, PosSpec, Stop};

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
fn glob_manifests(plugins_dir: &Path) -> Vec<(OsString, PathBuf)> {
    if !plugins_dir.is_dir() {
        return Vec::new();
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
    found
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
    for (name, manifest) in glob_manifests(&plugins_dir) {
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
    pystr::splitlines(text, true)
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
            if pystr::strip(line) == wanted {
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
        if pystr::strip(&lines[i]) == UNRELEASED {
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
    let blank = |l: &String| pystr::strip(l).is_empty();
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
        .any(|l| !pystr::strip(l).is_empty() && !l.starts_with('#'))
}

/// `changelog_check`: the plugin's `### [Unreleased]` has content.
pub fn changelog_check(text: &str, plugin: &str) -> Result<(), ReleaseError> {
    let lines = split_lines(text);
    let (_h, first, end) = unreleased_span(&lines, plugin)?;
    if !has_entries(&lines[first..end]) {
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
    changelog_check(text, plugin)?;
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
    let body = content(&lines[first..end]);
    let mut out = String::new();
    for l in &lines[..heading] {
        out.push_str(l);
    }
    out.push_str(&lines[heading]);
    out.push('\n');
    out.push_str(&format!("### [{version}] - {date}\n"));
    out.push('\n');
    for l in body {
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

const SUBCOMMANDS: [&str; 7] = [
    "plugin-version",
    "check-version",
    "check-dependents",
    "changelog-check",
    "changelog-move",
    "set-version",
    "set-source-version",
];

const PROG: &str = "anthology-dev release-edit";

const fn pos(name: &'static str) -> PosSpec {
    PosSpec { name, choices: &[] }
}

const DATE_OPT: [OptSpec; 1] = [OptSpec {
    long: "--date",
    dest: "date",
}];

/// The positionals, options and usage tail of subcommand `name`.
fn sub_spec(name: &str) -> (Vec<PosSpec>, &'static [OptSpec], &'static str) {
    match name {
        "plugin-version" => (vec![pos("path")], &[], "path"),
        "check-version" => (
            vec![
                pos("current"),
                pos("new"),
                PosSpec {
                    name: "tag_exists",
                    choices: &["0", "1"],
                },
            ],
            &[],
            "current new {0,1}",
        ),
        "check-dependents" => (
            vec![pos("root"), pos("plugin"), pos("version")],
            &[],
            "root plugin version",
        ),
        "changelog-check" => (vec![pos("path"), pos("plugin")], &[], "path plugin"),
        "changelog-move" => (
            vec![pos("path"), pos("plugin"), pos("version")],
            &DATE_OPT,
            "[--date DATE] path plugin version",
        ),
        "set-version" => (vec![pos("path"), pos("version")], &[], "path version"),
        _ => (
            vec![pos("path"), pos("target"), pos("version")],
            &[],
            "path target version",
        ),
    }
}

fn main_usage() -> String {
    format!(
        "usage: {PROG} [-h]\n                    {{{}}}\n                    ...",
        SUBCOMMANDS.join(",")
    )
}

/// Runs `anthology-dev release-edit …`; `args` are the arguments after `release-edit`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, stdin, &Env::from_process(), &cwd)
}

/// `release_edit.main(args)`, with relative paths resolved against `cwd`.
pub fn run_in(args: &[OsString], stdin: &mut dyn Read, env: &Env, cwd: &Path) -> Output {
    let _ = (stdin, env);
    let usage = main_usage();
    let main = Parser {
        prog: PROG,
        usage: &usage,
        opts: &[],
        pos: &[],
    };
    // The main parser: options before the command are strays; the first positional is the command.
    let (cmd_at, mut extras) = match argp::find_command(&main, args) {
        Ok(found) => found,
        Err(Stop::Help) => return argp::help(&main),
        Err(Stop::Error(m)) => return argp::usage_error(&main, &m),
    };
    let cmd = args[cmd_at].to_string_lossy().into_owned();
    if !SUBCOMMANDS.contains(&cmd.as_str()) {
        return argp::usage_error(&main, &argp::invalid_choice("cmd", &cmd, &SUBCOMMANDS));
    }
    let (positionals, opts, tail) = sub_spec(&cmd);
    let sub_prog = format!("{PROG} {cmd}");
    let sub_usage = format!("usage: {sub_prog} [-h] {tail}");
    let sub = Parser {
        prog: &sub_prog,
        usage: &sub_usage,
        opts,
        pos: &positionals,
    };
    let parsed = match argp::parse_known(&sub, &args[cmd_at + 1..]) {
        Ok(p) => p,
        Err(Stop::Help) => return argp::help(&sub),
        Err(Stop::Error(m)) => return argp::usage_error(&sub, &m),
    };
    extras.extend(parsed.extras.iter().cloned());
    if !extras.is_empty() {
        return argp::usage_error(
            &main,
            &format!("unrecognized arguments: {}", extras.join(" ")),
        );
    }
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

fn execute(cmd: &str, a: &argp::Parsed, cwd: &Path, out: &mut Output) -> Result<i32, Failure> {
    let arg = |i: usize| a.pos[i].as_os_str();
    let sarg = |i: usize| a.pos[i].to_string_lossy().into_owned();
    fn say(out: &mut Output, s: &str) {
        out.stdout.extend_from_slice(s.as_bytes());
        out.stdout.push(b'\n');
    }
    match cmd {
        "plugin-version" => {
            let version = plugin_version(&read_text(cwd, arg(0))?)?;
            say(out, &version);
        }
        "check-version" => {
            let verdict = check_version(&sarg(0), &sarg(1), sarg(2) == "1")?;
            say(out, verdict);
        }
        "check-dependents" => {
            let problems = check_dependents_in(cwd, Path::new(arg(0)), &sarg(1), &sarg(2))?;
            if !problems.is_empty() {
                for msg in problems {
                    out.stderr
                        .extend_from_slice(format!("release: {msg}\n").as_bytes());
                }
                return Ok(1);
            }
        }
        "changelog-check" => changelog_check(&read_text(cwd, arg(0))?, &sarg(1))?,
        "changelog-move" => {
            let given = a.opt("date").map(|d| d.to_string_lossy().into_owned());
            let date = match given.filter(|d| !d.is_empty()) {
                Some(d) => d,
                None => clock::today_local().iso(),
            };
            if !is_iso_date(&date) {
                return Err(
                    ReleaseError(format!("date {} is not YYYY-MM-DD", str_repr(&date))).into(),
                );
            }
            let moved = changelog_move(&read_text(cwd, arg(0))?, &sarg(1), &sarg(2), &date)?;
            write_text(cwd, arg(0), &moved)?;
        }
        "set-version" => {
            let edited = set_plugin_version(&read_text(cwd, arg(0))?, &sarg(1))?;
            write_text(cwd, arg(0), &edited)?;
        }
        _ => {
            let edited = set_source_plugin_version(&read_text(cwd, arg(0))?, &sarg(1), &sarg(2))?;
            write_text(cwd, arg(0), &edited)?;
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
