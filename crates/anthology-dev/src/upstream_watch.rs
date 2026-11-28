//! `anthology-dev upstream-watch` (scripts/upstream_watch.py). Owner: DV3.
//!
//! Compares every pin with upstream and keeps one GitHub issue per change. It watches every
//! entry of `vendor/sources.json` (git sources pinned to a tag or a branch, url + checksum
//! sources, subtree sources) and the Claude Code version pinned in `ci/claude-code/package.json`.
//! Each change opens one issue (label `upstream-watch`, hidden marker
//! `<!-- upstream-watch:<key> -->` first in the body) or updates the open one. Only issues the
//! Actions bot authored are managed, and per key the newest decides: a closed issue whose body
//! still matches means the release was dismissed (nothing is written); a closed issue with an
//! older target gets a new issue. Issues are never closed, and nothing is merged or vendored.
//!
//! `--dry-run` checks everything and prints the issues it would write; it makes no write call.
//! A real run needs `GITHUB_TOKEN` and `GITHUB_REPOSITORY` (`GITHUB_API_URL` optional). With
//! `GITHUB_OUTPUT` set, a real run appends `claude_code_changed=true|false`. Exit codes (GC-5):
//! 0 done, 1 an entry or an issue write failed, 2 bad usage or missing credentials.
//!
//! Everything from upstream (tags, shas, file paths) is untrusted: it is validated against
//! strict patterns, file paths lose control characters and backticks and are shown inside one
//! fenced block, and commit messages or author names are never used. Issue text travels only as
//! JSON over the API, never through a shell.
//!
//! **F-17 (GC-15).** The commands an issue names are `scripts/dev.sh vendor sync|fetch|build
//! …` and `scripts/dev.sh startup-budget …`, where the Python wrote `python3 scripts/vendor.py
//! …` and `python3 scripts/startup_budget.py …`.
//!
//! The GC-19 seams are [`Deps`]: the network ([`Net`]: `git ls-remote` and HTTP GET), the
//! issue client ([`GitHub`]), `startup_budget_sections` and the budget table. [`RealNet`] runs
//! git through its own `run` seam (`subprocess.run`), and [`GitHubClient`] takes its API base.
//!
//! A failure the Python does not catch (a `KeyError` on an issue without `number`, an `OSError`
//! writing `--json` or `GITHUB_OUTPUT`) exits 1 with `anthology-dev upstream-watch: <Type>:
//! <message>` (DV2 ruling R10). The one look-ahead, `_without_budget`, uses fancy-regex (GC-16);
//! its backtrack limit is the same kind of exit 1, naming the pattern.

pub mod http;

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_pycompat::json::{self, INDENT2, Int, Map, PY_DEFAULT, Value};
use anthology_pycompat::pyerr::PyExc;
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::Output;
use crate::json_edit::{Uncaught, py_repr, universal_newlines};
use crate::memory_diff::pystr::splitlines;
use crate::semver;
use crate::startup_budget::{self, Root};
use anthology_pycompat::argparse::{self, Arg};
use anthology_pycompat::str as pystr;
use http::{Http, Method};

pub const LABEL: &str = "upstream-watch";
pub const CLAUDE_CODE_KEY: &str = "claude-code";
pub const CLAUDE_CODE_PACKAGE: &str = "@anthropic-ai/claude-code";
pub const CLAUDE_CODE_LATEST_URL: &str =
    "https://registry.npmjs.org/@anthropic-ai/claude-code/latest";
pub const CHANGELOG_URL: &str = "https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md";
pub const USER_AGENT: &str = "anthology-upstream-watch";
pub const GITHUB_API: &str = "https://api.github.com";
/// Seconds, for each socket operation.
pub const HTTP_TIMEOUT: u64 = 30;
pub const MAX_RESPONSE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_FILES_SHOWN: usize = 50;
pub const MAX_PATH_LEN: usize = 200;
pub const MAX_VERSION_LEN: usize = 64;
/// The only author whose issues the watcher manages.
pub const BOT_LOGIN: &str = "github-actions[bot]";
/// The compare API returns at most this many files.
pub const COMPARE_FILE_LIMIT: usize = 300;
/// `git ls-remote`'s timeout.
pub const LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(120);

/// F-17: the issue body's maintainer commands (`python3 scripts/vendor.py` in the Python).
pub const VENDOR_COMMAND: &str = "scripts/dev.sh vendor";
/// F-17: `python3 scripts/startup_budget.py` in the Python.
pub const STARTUP_BUDGET_COMMAND: &str = "scripts/dev.sh startup-budget";

/// `re.fullmatch(pattern)`: the pattern anchored at both ends (`\A…\z`). None of these has a
/// lookaround, so the `regex` crate takes them; every class is ASCII, as in Python.
fn full(pattern: &str) -> Regex {
    Regex::new(&format!(r"\A(?:{pattern})\z")).expect("a fixed pattern")
}

static TAG_PIN_RE: LazyLock<Regex> =
    LazyLock::new(|| full(r"(v?)([0-9]+)\.([0-9]+)(?:\.([0-9]+))?"));
static SAFE_TAG_RE: LazyLock<Regex> = LazyLock::new(|| full(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,63}"));
static SAFE_BRANCH_RE: LazyLock<Regex> =
    LazyLock::new(|| full(r"[A-Za-z0-9][A-Za-z0-9._+/-]{0,127}"));
static SAFE_NAME_RE: LazyLock<Regex> = LazyLock::new(|| full(r"[A-Za-z0-9._-]+"));
static SAFE_PATH_RE: LazyLock<Regex> = LazyLock::new(|| full(r"[A-Za-z0-9._/-]*"));
static SHA1_RE: LazyLock<Regex> = LazyLock::new(|| full(r"[0-9a-f]{40}"));
static SHA256_RE: LazyLock<Regex> = LazyLock::new(|| full(r"[0-9a-f]{64}"));
static REPO_RE: LazyLock<Regex> =
    LazyLock::new(|| full(r"https://[A-Za-z0-9.-]+(?::[0-9]+)?/[A-Za-z0-9._~/-]+"));
static GITHUB_REPO_RE: LazyLock<Regex> =
    LazyLock::new(|| full(r"https://github\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+?)(?:\.git)?/?"));
static HTTPS_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| full(r"https://[A-Za-z0-9.-]+(?::[0-9]+)?(?:/[A-Za-z0-9._~%+@=:-]*)*"));
/// `MARKER_RE.match`: anchored at the start only.
static MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A<!-- upstream-watch:([A-Za-z0-9._-]+) -->").expect("MARKER_RE")
});
static MANIFEST_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| full(r"([0-9a-fA-F]{64})[ \t]+\*?(.+)"));
static PROSE_DROP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^A-Za-z0-9 ._/:+(),-]").expect("PROSE_DROP_RE"));
/// `_without_budget`'s pattern: a look-ahead, so fancy-regex (`\Z` is written `\z`).
static BUDGET_SECTION_RE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r"(?ms)^### Startup budget\n.*?(?=^### |\z)")
        .expect("BUDGET_SECTION_RE is a fixed pattern")
});

// ---- errors ------------------------------------------------------------------------------

/// A failure attached to one watched key or one issue call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// `WatchError(message)`.
    Watch(String),
    /// Any other exception, by its class name (`type(exc).__name__`).
    Other(String),
}

impl Error {
    /// `type(exc).__name__`.
    pub fn type_name(&self) -> &str {
        match self {
            Error::Watch(_) => "WatchError",
            Error::Other(t) => t,
        }
    }

    /// What `check_all` records: `str(exc)` for a `WatchError`, else the class name.
    pub fn shown(&self) -> &str {
        match self {
            Error::Watch(m) => m,
            Error::Other(t) => t,
        }
    }
}

fn watch(msg: impl Into<String>) -> Error {
    Error::Watch(msg.into())
}

fn exc_name(e: &std::io::Error) -> String {
    PyExc::from(std::io::Error::new(e.kind(), ""))
        .type_name()
        .to_string()
}

// ---- untrusted text ----------------------------------------------------------------------

/// `str(value)` for a JSON value: the text of a string, `repr` of anything else.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

/// Python's truth value of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(Int::Small(n)) => *n != 0,
        Value::Int(Int::Big(_)) => true,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

/// `text[:n]` in code points.
fn take_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// `category[0] == "C" or category in ("Zl", "Zp")`.
fn is_dropped_category(c: char) -> bool {
    let cat = pystr::unicode_category(c);
    cat.starts_with('C') || cat == "Zl" || cat == "Zp"
}

/// `sanitize_path`: control and format characters (`C*`), line and paragraph separators and
/// backticks removed, `strip()`ped, capped at [`MAX_PATH_LEN`] code points. Safe in a fence.
pub fn sanitize_path(text: &str) -> String {
    let kept: String = text
        .chars()
        .filter(|&c| c != '`' && !is_dropped_category(c))
        .collect();
    take_chars(pystr::strip_ws(&kept), MAX_PATH_LEN)
}

/// `sanitize_inline`: the same as [`sanitize_path`], for an inline code span.
pub fn sanitize_inline(text: &str) -> String {
    sanitize_path(text)
}

/// `sanitize_prose(text, limit)`: only letters, digits, spaces and `. _ / : + ( ) , -` kept,
/// on one line, at most `limit` code points.
pub fn sanitize_prose(text: &str, limit: usize) -> String {
    take_chars(&PROSE_DROP_RE.replace_all(&sanitize_path(text), ""), limit)
}

fn expect(re: &Regex, value: Option<&Value>, what: &str) -> Result<String, Error> {
    match value {
        Some(Value::Str(s)) if re.is_match(s) => Ok(s.clone()),
        _ => Err(watch(format!("{what} is not in the expected format"))),
    }
}

// ---- data --------------------------------------------------------------------------------

/// One detected upstream change. `old`/`new` are the short labels of the title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub key: String,
    pub kind: String,
    pub old: String,
    pub new: String,
    pub new_target: String,
    pub command: String,
    pub details: Vec<(String, String)>,
    pub files: Vec<String>,
    pub files_total: usize,
    pub files_heading: String,
    pub notes: Vec<String>,
}

impl Change {
    /// A change with no details, files or notes.
    pub fn new(
        key: &str,
        kind: &str,
        old: &str,
        new: &str,
        new_target: &str,
        command: &str,
    ) -> Change {
        Change {
            key: key.into(),
            kind: kind.into(),
            old: old.into(),
            new: new.into(),
            new_target: new_target.into(),
            command: command.into(),
            details: Vec::new(),
            files: Vec::new(),
            files_total: 0,
            files_heading: "Changed files".into(),
            notes: Vec::new(),
        }
    }

    /// This change with `details`.
    pub fn with_details(mut self, details: &[(&str, &str)]) -> Change {
        self.details = details
            .iter()
            .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
            .collect();
        self
    }

    /// `Change.to_json()`.
    pub fn to_json(&self) -> Value {
        let strs = |v: &[String]| Value::Array(v.iter().map(|s| Value::Str(s.clone())).collect());
        let mut m = Map::new();
        for (k, v) in [
            ("key", &self.key),
            ("kind", &self.kind),
            ("old", &self.old),
            ("new", &self.new),
            ("new_target", &self.new_target),
        ] {
            m.insert(k.into(), Value::Str(v.clone()));
        }
        m.insert("title".into(), Value::Str(render_title(self)));
        m.insert("command".into(), Value::Str(self.command.clone()));
        m.insert(
            "details".into(),
            Value::Array(
                self.details
                    .iter()
                    .map(|(a, b)| strs(&[a.clone(), b.clone()]))
                    .collect(),
            ),
        );
        m.insert("files".into(), strs(&self.files));
        let total = self.files_total.max(self.files.len());
        m.insert("files_total".into(), Value::Int(Int::Small(total as i64)));
        m.insert("notes".into(), strs(&self.notes));
        Value::Object(m)
    }
}

/// `(heading, markdown)` pairs added to an issue body.
pub type Sections = Vec<(String, String)>;

// ---- rendering ---------------------------------------------------------------------------

/// `render_title(change)`.
pub fn render_title(change: &Change) -> String {
    format!(
        "Upstream: {} {} \u{2192} {}",
        change.key, change.old, change.new
    )
}

/// `render_body(change, extra_sections)`: the issue body. `extra_sections` come after the
/// summary and before the update command; their markdown must hold no untrusted text.
pub fn render_body(change: &Change, extra_sections: &[(String, String)]) -> String {
    let mut lines: Vec<String> = vec![
        format!("<!-- upstream-watch:{} -->", change.key),
        format!("## Upstream change: `{}`", sanitize_inline(&change.key)),
        String::new(),
    ];
    for (label, value) in &change.details {
        lines.push(format!(
            "- **{}:** `{}`",
            sanitize_inline(label),
            sanitize_inline(value)
        ));
    }
    let mut summary: Vec<String> = change.notes.iter().map(|n| format!("- {n}")).collect();
    let files: Vec<String> = change
        .files
        .iter()
        .map(|f| sanitize_path(f))
        .filter(|f| !f.is_empty())
        .collect();
    if !files.is_empty() {
        let total = change.files_total.max(files.len());
        summary.push(format!(
            "- {} ({total}):",
            pystr::split_ws(&change.files_heading).join(" ")
        ));
        summary.push(String::new());
        summary.push("```text".into());
        summary.extend(files.iter().take(MAX_FILES_SHOWN).cloned());
        summary.push("```".into());
        if total > MAX_FILES_SHOWN {
            summary.push(format!("...and {} more.", total - MAX_FILES_SHOWN));
        }
    }
    if !summary.is_empty() {
        lines.extend([String::new(), "### Summary".into(), String::new()]);
        lines.extend(summary);
    }
    for (heading, markdown) in extra_sections {
        lines.push(String::new());
        lines.push(format!("### {}", pystr::split_ws(heading).join(" ")));
        lines.push(markdown.trim_matches('\n').to_string());
    }
    lines.extend([
        String::new(),
        "### Update".into(),
        String::new(),
        "Run this on a branch. CI and the evals are the gate: do not merge without them.".into(),
        String::new(),
        "```sh".into(),
        change.command.clone(),
        "```".into(),
        String::new(),
    ]);
    lines.join("\n")
}

// ---- startup budget ----------------------------------------------------------------------

/// One startup-budget row: the agent, its startup bytes now (`None`: unreadable), its budget.
pub type BudgetRow = (String, Option<u64>, Option<u64>);

/// `render_startup_budget(key, rows)`: the markdown of the "Startup budget" section.
pub fn render_startup_budget(key: &str, rows: &[BudgetRow]) -> String {
    let agents: Vec<String> = rows.iter().map(|(a, _, _)| sanitize_prose(a, 80)).collect();
    let mut lines = vec![
        format!(
            "The vendored knowledge from `{}` is read at startup by the agents below, so this \
             change can grow their startup cost. Sizes are bytes: the agent body, its Startup \
             files and its preloaded skills.",
            sanitize_prose(key, 80)
        ),
        String::new(),
        "| Agent | Now | Budget | Used |".into(),
        "|---|---|---|---|".into(),
    ];
    for (agent, (_, now, budget)) in agents.iter().zip(rows) {
        let shown = budget.map_or("none".to_string(), |b| b.to_string());
        let Some(now) = now else {
            lines.push(format!("| `{agent}` | unreadable | {shown} | |"));
            continue;
        };
        let used = match budget {
            Some(b) if *b != 0 => {
                let over = if now > b { " OVER" } else { "" };
                format!("{}%{over}", u128::from(*now) * 100 / u128::from(*b))
            }
            _ => "no budget".to_string(),
        };
        lines.push(format!("| `{agent}` | {now} | {shown} | {used} |"));
    }
    lines.extend([
        String::new(),
        "Size after the sync: not computed here, because the new upstream files are not \
         fetched offline. After running the update command below, run:"
            .into(),
        String::new(),
        "```sh".into(),
        format!("{STARTUP_BUDGET_COMMAND} {}", agents.join(" ")),
        "```".into(),
        String::new(),
        "It prints each agent's size against its budget and exits 1 when one is over (the \
         `startup_budget` test in CI checks the same). Raising a budget needs a reason in the PR \
         (CONTRIBUTING.md, \"Startup budgets\")."
            .into(),
    ]);
    lines.join("\n")
}

/// The OSError subclasses (`except OSError`).
fn is_os_error(ty: &str) -> bool {
    matches!(
        ty,
        "OSError"
            | "BlockingIOError"
            | "ChildProcessError"
            | "ConnectionError"
            | "BrokenPipeError"
            | "ConnectionAbortedError"
            | "ConnectionRefusedError"
            | "ConnectionResetError"
            | "FileExistsError"
            | "FileNotFoundError"
            | "InterruptedError"
            | "IsADirectoryError"
            | "NotADirectoryError"
            | "PermissionError"
            | "ProcessLookupError"
            | "TimeoutError"
    )
}

fn read_text(path: &Path) -> Result<String, Uncaught> {
    let bytes = std::fs::read(path).map_err(|e| {
        let mut exc = PyExc::from(e);
        if let PyExc::OSError { filename, .. } = &mut exc {
            *filename = Some(path.to_string_lossy().into_owned());
        }
        Uncaught::from_exc(&exc)
    })?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(universal_newlines(&s)),
        Err(_) => Err(Uncaught::new("UnicodeDecodeError", "")),
    }
}

fn read_json(path: &Path) -> Result<Value, Uncaught> {
    let text = read_text(path)?;
    json::loads(&text).map_err(|e| Uncaught::new(e.type_name(), e.py_str(&text)))
}

/// `startup_budget_sections(root, changes)`: `{change key: [("Startup budget", markdown)]}`
/// for the changes whose source's `target` holds a file an agent reads at startup. Sizes come
/// from the checkout through [`startup_budget`]; `budget` is its `BUDGET` table.
pub fn startup_budget_sections(
    root: &Root,
    changes: &[Change],
    budget: &[(&str, u64)],
) -> Result<HashMap<String, Sections>, Uncaught> {
    let data = read_json(&root.fs.join("vendor/sources.json"))?;
    let Some(data) = data.as_object() else {
        return Err(Uncaught::new(
            "AttributeError",
            "object has no attribute 'get'",
        ));
    };
    let entries: Vec<&Value> = match data.get("sources") {
        None => Vec::new(),
        Some(Value::Array(items)) => items.iter().collect(),
        // Iterating a dict gives its keys and a string its characters: never a dict.
        Some(Value::Object(_) | Value::Str(_)) => Vec::new(),
        Some(_) => return Err(Uncaught::new("TypeError", "object is not iterable")),
    };
    let mut targets: HashMap<String, String> = HashMap::new();
    for e in entries {
        let Some(m) = e.as_object() else { continue };
        let Some(Value::Str(target)) = m.get("target") else {
            continue;
        };
        match m.get("name") {
            Some(Value::Str(name)) => {
                targets.insert(name.clone(), target.clone());
            }
            Some(Value::Array(_) | Value::Object(_)) => {
                return Err(Uncaught::new("TypeError", "unhashable type"));
            }
            _ => {}
        }
    }
    let mut found: HashMap<String, Sections> = HashMap::new();
    for change in changes {
        let agents = match targets.get(&change.key) {
            Some(t) if !t.is_empty() => startup_budget::agents_reading(root, t)?,
            _ => Vec::new(),
        };
        if agents.is_empty() {
            continue;
        }
        let mut rows = Vec::new();
        for agent in agents {
            let now = match startup_budget::startup_bytes(root, &agent) {
                Ok(n) => Some(n),
                Err(e) if is_os_error(&e.ty) => None,
                Err(e) => return Err(e),
            };
            let b = budget.iter().find(|(k, _)| *k == agent).map(|&(_, v)| v);
            rows.push((agent, now, b));
        }
        found.insert(
            change.key.clone(),
            vec![(
                "Startup budget".into(),
                render_startup_budget(&change.key, &rows),
            )],
        );
    }
    Ok(found)
}

/// The type of [`startup_budget_sections`] (the GC-19 seam).
pub type SectionsFn =
    fn(&Root, &[Change], &[(&str, u64)]) -> Result<HashMap<String, Sections>, Uncaught>;

// ---- parsing helpers ---------------------------------------------------------------------

/// Refs as `{refname: sha}` in insertion order (a repeated name keeps its first position and
/// takes the last value, as a dict).
pub type Refs = Vec<(String, String)>;

fn refs_get<'a>(refs: &'a Refs, name: &str) -> Option<&'a str> {
    refs.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn refs_insert(refs: &mut Refs, name: &str, sha: &str) {
    match refs.iter_mut().find(|(k, _)| k == name) {
        Some(slot) => slot.1 = sha.to_string(),
        None => refs.push((name.to_string(), sha.to_string())),
    }
}

/// `parse_ls_remote(text)`: `git ls-remote` output as refs; malformed lines are skipped.
pub fn parse_ls_remote(text: &str) -> Refs {
    let mut refs = Refs::new();
    for line in splitlines(text, false) {
        if let Some((sha, name)) = line.split_once('\t')
            && SHA1_RE.is_match(sha)
            && !name.is_empty()
        {
            refs_insert(&mut refs, name, sha);
        }
    }
    refs
}

/// A version `(major, minor, patch)` of [`TAG_PIN_RE`], each a decimal string without leading
/// zeros (Python's ints are unbounded).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Version([String; 3]);

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        for (a, b) in self.0.iter().zip(&other.0) {
            let o = a.len().cmp(&b.len()).then_with(|| a.cmp(b));
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `(v prefix, version)` of a tag that fully matches [`TAG_PIN_RE`].
fn tag_version(tag: &str) -> Option<(String, Version)> {
    let c = TAG_PIN_RE.captures(tag)?;
    let num = |i: usize| {
        let s = c.get(i).map_or("0", |m| m.as_str()).trim_start_matches('0');
        if s.is_empty() {
            "0".to_string()
        } else {
            s.to_string()
        }
    };
    Some((c[1].to_string(), Version([num(2), num(3), num(4)])))
}

/// `latest_stable_tag(refs, pinned_ref)`: the highest stable version tag with the pin's `v`
/// prefix convention, and its commit (the peeled one for an annotated tag). `pinned_ref` must
/// match the tag pattern.
pub fn latest_stable_tag(refs: &Refs, pinned_ref: &str) -> Option<(String, String)> {
    let (prefix, _) = tag_version(pinned_ref)?;
    let mut best: Option<(Version, &str)> = None;
    for (name, _) in refs {
        let Some(tag) = name.strip_prefix("refs/tags/") else {
            continue;
        };
        if name.ends_with("^{}") {
            continue;
        }
        let Some((p, version)) = tag_version(tag) else {
            continue;
        };
        if p != prefix || !SAFE_TAG_RE.is_match(tag) {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| version > *b) {
            best = Some((version, tag));
        }
    }
    let (_, tag) = best?;
    let commit = match refs_get(refs, &format!("refs/tags/{tag}^{{}}")) {
        Some(peeled) if !peeled.is_empty() => peeled,
        _ => refs_get(refs, &format!("refs/tags/{tag}"))?,
    };
    Some((tag.to_string(), commit.to_string()))
}

/// The shape of a `vendor/sources.json` entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Git,
    Url,
    Subtree,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Git => "git",
            Kind::Url => "url",
            Kind::Subtree => "subtree",
        }
    }
}

/// `classify(entry)`: git, url or subtree; any other shape is an error.
pub fn classify(entry: &Value) -> Result<Kind, Error> {
    let Some(m) = entry.as_object() else {
        return Err(watch("unrecognized source shape: not an object"));
    };
    let upstream = matches!(m.get("upstream"), Some(Value::Object(_)));
    if upstream || m.get("managed_by").and_then(Value::as_str) == Some("subtree") {
        if !upstream {
            return Err(watch(
                "unrecognized source shape: subtree entry without `upstream`",
            ));
        }
        return Ok(Kind::Subtree);
    }
    if m.contains_key("url") && m.contains_key("checksum") {
        return Ok(Kind::Url);
    }
    if m.contains_key("repo") {
        return Ok(Kind::Git);
    }
    Err(watch("unrecognized source shape: no repo, url or upstream"))
}

/// `parse_manifest(data)`: a sha256sum manifest as `{file: lowercase sha256}`.
pub fn parse_manifest(data: &[u8]) -> BTreeMap<String, String> {
    let text = String::from_utf8_lossy(data);
    let mut out = BTreeMap::new();
    for line in splitlines(&text, false) {
        if let Some(c) = MANIFEST_LINE_RE.captures(pystr::strip_ws(line)) {
            out.insert(
                pystr::strip_ws(&c[2]).to_string(),
                c[1].to_ascii_lowercase(),
            );
        }
    }
    out
}

fn under(path: &str, filename: Option<&Value>) -> bool {
    let Some(Value::Str(filename)) = filename else {
        return false;
    };
    let prefix = path.trim_matches('/');
    prefix.is_empty() || filename == prefix || filename.starts_with(&format!("{prefix}/"))
}

// ---- I/O ---------------------------------------------------------------------------------

/// The network the checker reads (the GC-19 seam; tests inject fakes).
pub trait Net {
    /// `git ls-remote` of `repo`'s tags and branches.
    fn ls_remote(&self, repo: &str) -> Result<Refs, Error>;
    /// The body at `url`; `headers` are added to the request.
    fn http_get(&self, url: &str, headers: &[(String, String)]) -> Result<Vec<u8>, Error>;
}

/// A finished subprocess: `returncode` (`-signal` when a signal ended it) and its output.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunOut {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// `subprocess.run(argv, capture_output=True, timeout=..., env=env)` (no shell): the result, or
/// the class of the exception it raised (`FileNotFoundError`, `TimeoutExpired`, …).
pub type RunFn = fn(&[String], &[(String, String)], Duration) -> Result<RunOut, String>;

/// The real [`RunFn`]. stdin is `/dev/null`; both pipes are read by their own threads; past
/// the timeout the child is killed and reaped. As `communicate(timeout)`, the timeout also
/// covers reading both pipes to their end (a grandchild holding a pipe open cannot hang it).
pub fn run_subprocess(
    argv: &[String],
    env: &[(String, String)],
    timeout: Duration,
) -> Result<RunOut, String> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .env_clear()
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| exc_name(&e))?;
    let pipe = |p: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut b = Vec::new();
            if let Some(mut p) = p {
                let _ = p.read_to_end(&mut b);
            }
            let _ = tx.send(b);
        });
        rx
    };
    let out = pipe(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let err = pipe(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("TimeoutExpired".into());
            }
            Err(e) => return Err(exc_name(&e)),
        }
    };
    let left = || timeout.saturating_sub(start.elapsed());
    let (Ok(stdout), Ok(stderr)) = (out.recv_timeout(left()), err.recv_timeout(left())) else {
        return Err("TimeoutExpired".into());
    };
    Ok(RunOut {
        code: status
            .code()
            .unwrap_or_else(|| status.signal().map_or(-1, |s| -s)),
        stdout,
        stderr,
    })
}

/// `git_env()`: a minimal environment for git, so no `GITHUB_TOKEN`/`GH_TOKEN` reaches a
/// third-party host: `LANG=C`, `GIT_TERMINAL_PROMPT=0`, and `PATH` and `HOME` when non-empty.
pub fn git_env(env: &Env) -> Vec<(String, String)> {
    let mut out = vec![
        ("LANG".to_string(), "C".to_string()),
        ("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()),
    ];
    for name in ["PATH", "HOME"] {
        if let Some(v) = env.get(name).filter(|v| !v.is_empty()) {
            out.push((name.to_string(), v.to_string()));
        }
    }
    out
}

/// `RealNet`: `git ls-remote` (no shell) and HTTP GET with a timeout.
pub struct RealNet {
    pub git_env: Vec<(String, String)>,
    pub http: Http,
    pub run: RunFn,
}

impl RealNet {
    /// The real network, for the environment `env`.
    pub fn new(env: &Env) -> RealNet {
        RealNet {
            git_env: git_env(env),
            http: Http::new(env),
            run: run_subprocess,
        }
    }
}

impl Net for RealNet {
    fn ls_remote(&self, repo: &str) -> Result<Refs, Error> {
        let argv: Vec<String> = [
            "git",
            "ls-remote",
            "--",
            repo,
            "refs/tags/*",
            "refs/heads/*",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
        let out = (self.run)(&argv, &self.git_env, LS_REMOTE_TIMEOUT).map_err(Error::Other)?;
        if out.code != 0 {
            return Err(watch(format!("git ls-remote failed (exit {})", out.code)));
        }
        let text =
            String::from_utf8(out.stdout).map_err(|_| Error::Other("UnicodeDecodeError".into()))?;
        Ok(parse_ls_remote(&text))
    }

    fn http_get(&self, url: &str, headers: &[(String, String)]) -> Result<Vec<u8>, Error> {
        let mut all = vec![("User-Agent".to_string(), USER_AGENT.to_string())];
        for (k, v) in headers {
            match all.iter_mut().find(|(a, _)| a.eq_ignore_ascii_case(k)) {
                Some(slot) => slot.1 = v.clone(),
                None => all.push((k.clone(), v.clone())),
            }
        }
        let data = self
            .http
            .open(Method::Get, url, all, None, MAX_RESPONSE_BYTES + 1)?;
        if data.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(watch("response too large"));
        }
        Ok(data)
    }
}

/// The issue calls the watcher makes (the GC-19 seam; tests inject fakes).
pub trait GitHub {
    /// Every labeled issue, open and closed (pull requests included; callers skip them).
    fn list_issues(&self) -> Result<Vec<Value>, Error>;
    /// The created issue, as the API returns it.
    fn create_issue(&self, title: &str, body: &str) -> Result<Value, Error>;
    /// Updates issue `number` (whatever the listing held there).
    fn update_issue(&self, number: &Value, title: &str, body: &str) -> Result<Value, Error>;
}

/// `GitHubClient`: the GitHub REST calls, JSON in and out.
pub struct GitHubClient {
    pub repo: String,
    pub token: String,
    pub api_base: String,
    pub http: Http,
}

impl GitHubClient {
    /// A client for `repo` (`owner/name`) at `api_base` (`GITHUB_API_URL`).
    pub fn new(repo: &str, token: &str, api_base: &str, env: &Env) -> GitHubClient {
        GitHubClient {
            repo: repo.into(),
            token: token.into(),
            api_base: api_base.trim_end_matches('/').into(),
            http: Http::new(env),
        }
    }

    fn request(&self, method: Method, path: &str, payload: Option<&Value>) -> Result<Value, Error> {
        let data = payload.map(|p| json::dumps(p, &PY_DEFAULT).into_bytes());
        let headers: Vec<(String, String)> = [
            ("Authorization", format!("Bearer {}", self.token)),
            ("Accept", "application/vnd.github+json".to_string()),
            ("X-GitHub-Api-Version", "2022-11-28".to_string()),
            ("User-Agent", USER_AGENT.to_string()),
            ("Content-Type", "application/json".to_string()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let url = format!("{}{path}", self.api_base);
        let bytes = self
            .http
            .open(method, &url, headers, data, MAX_RESPONSE_BYTES)?;
        let text =
            String::from_utf8(bytes).map_err(|_| Error::Other("UnicodeDecodeError".into()))?;
        json::loads(&text).map_err(|e| Error::Other(e.type_name().into()))
    }
}

fn str_map(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

impl GitHub for GitHubClient {
    fn list_issues(&self) -> Result<Vec<Value>, Error> {
        let mut issues = Vec::new();
        for page in 1..=50 {
            let batch = self.request(
                Method::Get,
                &format!(
                    "/repos/{}/issues?labels={LABEL}&state=all&per_page=100&page={page}",
                    self.repo
                ),
                None,
            )?;
            // `issues.extend(batch)`: a dict gives its keys, a string its characters.
            let items: Vec<Value> = match batch {
                Value::Array(_) => batch.into_array().unwrap_or_default(),
                Value::Object(_) => batch
                    .into_object()
                    .map(|m| m.into_iter().map(|(k, _)| Value::Str(k)).collect())
                    .unwrap_or_default(),
                Value::Str(ref s) => s.chars().map(|c| Value::Str(c.to_string())).collect(),
                _ => return Err(Error::Other("TypeError".into())),
            };
            let n = items.len();
            issues.extend(items);
            if n < 100 {
                break;
            }
        }
        Ok(issues)
    }

    fn create_issue(&self, title: &str, body: &str) -> Result<Value, Error> {
        let payload = str_map(vec![
            ("title", Value::Str(title.into())),
            ("body", Value::Str(body.into())),
            ("labels", Value::Array(vec![Value::Str(LABEL.into())])),
        ]);
        self.request(
            Method::Post,
            &format!("/repos/{}/issues", self.repo),
            Some(&payload),
        )
    }

    fn update_issue(&self, number: &Value, title: &str, body: &str) -> Result<Value, Error> {
        let number = py_int(number)?;
        let payload = str_map(vec![
            ("title", Value::Str(title.into())),
            ("body", Value::Str(body.into())),
        ]);
        self.request(
            Method::Patch,
            &format!("/repos/{}/issues/{number}", self.repo),
            Some(&payload),
        )
    }
}

/// `int(value)` as decimal text: ints, bools, finite floats (truncated) and the strings
/// `int()` accepts (whitespace, a sign, Unicode decimal digits, single `_` between digits).
pub fn py_int(v: &Value) -> Result<String, Error> {
    let value_error = || Error::Other("ValueError".into());
    match v {
        Value::Int(Int::Small(n)) => Ok(n.to_string()),
        Value::Int(Int::Big(s)) => Ok(s.clone()),
        Value::Bool(b) => Ok(u8::from(*b).to_string()),
        Value::Float(f) if f.is_nan() => Err(value_error()),
        Value::Float(f) if f.is_infinite() => Err(Error::Other("OverflowError".into())),
        Value::Float(f) => {
            let t = f.trunc();
            Ok(if t == 0.0 {
                "0".into()
            } else {
                format!("{t:.0}")
            })
        }
        Value::Str(s) => match pystr::py_int(s).ok_or_else(value_error)? {
            Int::Small(n) => Ok(n.to_string()),
            Int::Big(t) => Ok(t),
        },
        _ => Err(Error::Other("TypeError".into())),
    }
}

// ---- comparison --------------------------------------------------------------------------

/// `Checker`: compares pins with upstream; all I/O goes through `net`.
pub struct Checker<'a> {
    root: PathBuf,
    net: &'a dyn Net,
    token: Option<String>,
    refs_cache: HashMap<String, Result<Refs, Error>>,
    compare_cache: HashMap<(String, String, String), Value>,
}

impl<'a> Checker<'a> {
    /// A checker of the repository at `root` (a path to open). `token` authorizes the compare
    /// API when it is non-empty.
    pub fn new(root: &Path, net: &'a dyn Net, token: Option<&str>) -> Checker<'a> {
        Checker {
            root: root.to_path_buf(),
            net,
            token: token.map(str::to_string),
            refs_cache: HashMap::new(),
            compare_cache: HashMap::new(),
        }
    }

    /// The refs of `repo`, listed once per run (a failure too, so an unreachable repo is not
    /// retried for every entry).
    fn refs(&mut self, repo: &str) -> Result<Refs, Error> {
        if !self.refs_cache.contains_key(repo) {
            let result = self.net.ls_remote(repo).map_err(|e| match e {
                Error::Watch(m) => Error::Watch(m),
                Error::Other(t) => watch(format!("cannot list refs ({t})")),
            });
            self.refs_cache.insert(repo.to_string(), result);
        }
        self.refs_cache[repo].clone()
    }

    fn get_json(&self, url: &str, headers: &[(String, String)]) -> Result<Value, Error> {
        let data = self.net.http_get(url, headers).map_err(|e| match e {
            Error::Watch(m) => Error::Watch(m),
            Error::Other(t) => watch(format!("request failed ({t})")),
        })?;
        let text =
            String::from_utf8(data).map_err(|_| watch("request failed (UnicodeDecodeError)"))?;
        json::loads(&text).map_err(|e| watch(format!("request failed ({})", e.type_name())))
    }

    /// The GitHub compare API for `old...new`, cached per `(repo, old, new)`.
    fn compare(&mut self, repo: &str, old: &str, new: &str) -> Result<Value, Error> {
        let key = (repo.to_string(), old.to_string(), new.to_string());
        if let Some(v) = self.compare_cache.get(&key) {
            return Ok(v.clone());
        }
        let c = GITHUB_REPO_RE
            .captures(repo)
            .ok_or_else(|| watch("not a GitHub repository"))?;
        let url = format!(
            "{GITHUB_API}/repos/{}/{}/compare/{old}...{new}",
            &c[1], &c[2]
        );
        let mut headers = vec![
            (
                "Accept".to_string(),
                "application/vnd.github+json".to_string(),
            ),
            ("X-GitHub-Api-Version".to_string(), "2022-11-28".to_string()),
        ];
        if let Some(t) = self.token.as_deref().filter(|t| !t.is_empty()) {
            headers.push(("Authorization".to_string(), format!("Bearer {t}")));
        }
        let data = self.get_json(&url, &headers)?;
        if !matches!(
            data.as_object().and_then(|m| m.get("files")),
            Some(Value::Array(_))
        ) {
            return Err(watch("unexpected compare response"));
        }
        self.compare_cache.insert(key, data.clone());
        Ok(data)
    }

    /// `(file names under path, truncated?)` of a compare response.
    fn files_under(data: &Value, path: &str) -> (Vec<String>, bool) {
        let files = data
            .as_object()
            .and_then(|m| m.get("files"))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let truncated = files.len() >= COMPARE_FILE_LIMIT;
        let mut names = Vec::new();
        for f in files.iter().filter_map(Value::as_object) {
            if under(path, f.get("filename")) || under(path, f.get("previous_filename")) {
                let name = sanitize_path(&f.get("filename").map_or(String::new(), py_str));
                if !name.is_empty() {
                    names.push(name);
                }
            }
        }
        (names, truncated)
    }

    /// `check_entry(entry)`: the change of one `vendor/sources.json` entry, if any.
    pub fn check_entry(&mut self, entry: &Value) -> Result<Option<Change>, Error> {
        let kind = classify(entry)?;
        let m = entry.as_object().expect("classify checked the object");
        let name = expect(&SAFE_NAME_RE, m.get("name"), "source name")?;
        match kind {
            Kind::Url => self.check_url(&name, m),
            Kind::Subtree => {
                let spec = m
                    .get("upstream")
                    .and_then(Value::as_object)
                    .expect("classify checked upstream");
                self.check_git(&name, kind, spec, "")
            }
            Kind::Git => {
                let path = match m.get("path") {
                    None => String::new(),
                    Some(Value::Str(p)) if SAFE_PATH_RE.is_match(p) => p.clone(),
                    Some(_) => return Err(watch("path is not in the expected format")),
                };
                self.check_git(&name, kind, m, &path)
            }
        }
    }

    fn check_git(
        &mut self,
        name: &str,
        kind: Kind,
        spec: &Map,
        path: &str,
    ) -> Result<Option<Change>, Error> {
        let repo = expect(&REPO_RE, spec.get("repo"), "repo")?;
        let pin = spec.get("ref");
        let pinned = expect(&SHA1_RE, spec.get("commit"), "pinned commit")?;
        let refs = self.refs(&repo)?;
        if let Some(Value::Str(r)) = pin
            && TAG_PIN_RE.is_match(r)
        {
            return self.check_tag(name, kind, &repo, r, &pinned, path, &refs);
        }
        let branch = expect(&SAFE_BRANCH_RE, pin, "ref")?;
        self.check_branch(name, kind, &repo, &branch, &pinned, path, &refs)
    }

    /// `_command(name, kind, new_ref)` (F-17).
    fn command(name: &str, kind: Kind, new_ref: &str) -> String {
        if kind == Kind::Subtree {
            return format!("scripts/sync-atl.sh {new_ref}");
        }
        format!("{VENDOR_COMMAND} sync {name} --ref {new_ref}")
    }

    fn repo_details(repo: &str, path: &str, old: &str, new: &str) -> Vec<(String, String)> {
        let mut details = vec![("Source".to_string(), repo.to_string())];
        if !path.is_empty() {
            details.push(("Path".into(), path.into()));
        }
        if let Some(c) = GITHUB_REPO_RE.captures(repo) {
            details.push((
                "Compare".into(),
                format!(
                    "https://github.com/{}/{}/compare/{old}...{new}",
                    &c[1], &c[2]
                ),
            ));
        }
        details
    }

    #[allow(clippy::too_many_arguments, reason = "the Python method's parameters")]
    fn check_tag(
        &mut self,
        name: &str,
        kind: Kind,
        repo: &str,
        pin: &str,
        pinned: &str,
        path: &str,
        refs: &Refs,
    ) -> Result<Option<Change>, Error> {
        let (tag, new_commit) = latest_stable_tag(refs, pin)
            .ok_or_else(|| watch("no stable version tags found upstream"))?;
        let (_, newest) = tag_version(&tag).expect("a tag that matched");
        let (_, current) = tag_version(pin).expect("a pin that matched");
        if newest <= current {
            return Ok(None);
        }
        if !SHA1_RE.is_match(&new_commit) {
            return Err(watch("upstream tag commit is not in the expected format"));
        }
        let mut change = Change::new(
            name,
            kind.as_str(),
            pin,
            &tag,
            &tag,
            &Self::command(name, kind, &tag),
        );
        change.details = vec![
            ("Pinned".into(), format!("{pin} ({pinned})")),
            ("Latest stable".into(), format!("{tag} ({new_commit})")),
        ];
        change
            .details
            .extend(Self::repo_details(repo, path, pinned, &new_commit));
        if GITHUB_REPO_RE.is_match(repo) {
            match self.compare(repo, pinned, &new_commit) {
                Ok(data) => {
                    summarize(&mut change, &data, path, true);
                }
                Err(_) => change
                    .notes
                    .push("Diff summary unavailable (GitHub compare API request failed).".into()),
            }
        }
        Ok(Some(change))
    }

    #[allow(clippy::too_many_arguments, reason = "the Python method's parameters")]
    fn check_branch(
        &mut self,
        name: &str,
        kind: Kind,
        repo: &str,
        branch: &str,
        pinned: &str,
        path: &str,
        refs: &Refs,
    ) -> Result<Option<Change>, Error> {
        let head = refs_get(refs, &format!("refs/heads/{branch}"))
            .ok_or_else(|| watch("branch not found upstream"))?
            .to_string();
        if !SHA1_RE.is_match(&head) {
            return Err(watch("upstream head is not in the expected format"));
        }
        if head == pinned {
            return Ok(None);
        }
        let mut change = Change::new(
            name,
            kind.as_str(),
            &pinned[..12],
            &head[..12],
            &head,
            &Self::command(name, kind, branch),
        );
        change.details = vec![
            ("Branch".into(), branch.into()),
            ("Pinned commit".into(), pinned.into()),
            ("Upstream head".into(), head.clone()),
        ];
        change
            .details
            .extend(Self::repo_details(repo, path, pinned, &head));
        if !GITHUB_REPO_RE.is_match(repo) {
            change
                .notes
                .push("Not a GitHub repository: only the commit range is shown.".into());
            return Ok(Some(change));
        }
        let data = self.compare(repo, pinned, &head)?;
        if !summarize(&mut change, &data, path, false) {
            return Ok(None);
        }
        Ok(Some(change))
    }

    fn check_url(&mut self, name: &str, entry: &Map) -> Result<Option<Change>, Error> {
        let url = expect(&HTTPS_URL_RE, entry.get("url"), "url")?;
        let checksum = expect(&SHA256_RE, entry.get("checksum"), "checksum")?;
        let manifest_url = if url.ends_with(".sha256") {
            url.clone()
        } else {
            format!("{}/CHECKSUMS.sha256", url.trim_end_matches('/'))
        };
        let data = self
            .net
            .http_get(&manifest_url, &[])
            .map_err(|e| watch(format!("cannot fetch the manifest ({})", e.type_name())))?;
        let new_sum: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if new_sum == checksum {
            return Ok(None);
        }
        let mut change = Change::new(
            name,
            "url",
            &checksum[..12],
            &new_sum[..12],
            &new_sum,
            &format!(
                "set \"checksum\" of {name} in vendor/sources.json to {new_sum}, then run: \
                 {VENDOR_COMMAND} fetch {name} && {VENDOR_COMMAND} build {name}"
            ),
        );
        change.details = vec![
            ("Manifest".into(), manifest_url),
            ("Pinned checksum".into(), checksum),
            ("New checksum".into(), new_sum),
        ];
        let snapshot = self
            .root
            .join("vendor/snapshots")
            .join(name)
            .join("CHECKSUMS.sha256");
        let new_files = parse_manifest(&data);
        if !snapshot.is_file() {
            change.notes.push(format!(
                "No snapshot manifest at vendor/snapshots/{name}/CHECKSUMS.sha256: \
                 file-level diff unavailable."
            ));
        } else if new_files.is_empty() {
            change.notes.push(
                "The new manifest is not in sha256sum format: file-level diff unavailable.".into(),
            );
        } else {
            let old_bytes = std::fs::read(&snapshot).map_err(|e| Error::Other(exc_name(&e)))?;
            let old_files = parse_manifest(&old_bytes);
            let mut lines: Vec<String> = Vec::new();
            lines.extend(
                new_files
                    .keys()
                    .filter(|f| !old_files.contains_key(*f))
                    .map(|f| format!("+ {f}")),
            );
            lines.extend(
                old_files
                    .keys()
                    .filter(|f| !new_files.contains_key(*f))
                    .map(|f| format!("- {f}")),
            );
            lines.extend(
                old_files
                    .iter()
                    .filter(|(f, h)| new_files.get(*f).is_some_and(|n| n != *h))
                    .map(|(f, _)| format!("~ {f}")),
            );
            change.files = lines
                .iter()
                .map(|l| sanitize_path(l))
                .filter(|l| !l.is_empty())
                .collect();
            change.files_total = change.files.len();
            change.files_heading = "Files added (+), removed (-) or changed (~)".into();
        }
        Ok(Some(change))
    }

    /// `check_claude_code()`: the Claude Code release newer than the pin, if any.
    pub fn check_claude_code(&mut self) -> Result<Option<Change>, Error> {
        let pinned = read_json(&self.root.join("ci/claude-code/package.json"))
            .ok()
            .and_then(|pkg| {
                pkg.as_object()?
                    .get("dependencies")?
                    .as_object()?
                    .get(CLAUDE_CODE_PACKAGE)
                    .cloned()
            })
            .ok_or_else(|| watch("cannot read the pinned Claude Code version"))?;
        let latest = self.get_json(CLAUDE_CODE_LATEST_URL, &[])?;
        let latest = latest.as_object().and_then(|m| m.get("version"));
        if let Some(Value::Str(v)) = latest
            && v.chars().count() > MAX_VERSION_LEN
        {
            return Err(watch("Claude Code version from the registry is too long"));
        }
        let (Some(p), Some(l)) = (
            semver::parse_value(&pinned),
            latest.and_then(semver::parse_value),
        ) else {
            return Err(watch("Claude Code version is not valid semver"));
        };
        if semver::cmp_parsed(&l, &p) != Ordering::Greater {
            return Ok(None);
        }
        let pinned = pinned.as_str().unwrap_or_default();
        let latest = latest.and_then(Value::as_str).unwrap_or_default();
        Ok(Some(
            Change::new(
                CLAUDE_CODE_KEY,
                "claude-code",
                pinned,
                latest,
                latest,
                &format!(
                    "set \"{CLAUDE_CODE_PACKAGE}\" in ci/claude-code/package.json to {latest}, run \
                     npm install --package-lock-only in ci/claude-code, and let CI \
                     (plugin-validate) and evals run on the PR"
                ),
            )
            .with_details(&[
                ("Pinned", pinned),
                ("Latest release", latest),
                ("Changelog", CHANGELOG_URL),
            ]),
        ))
    }
}

/// `_summarize`: fills the change's summary from a compare response; false when nothing
/// changed under `path` (a branch diff then is no change).
fn summarize(change: &mut Change, data: &Value, path: &str, tag_diff: bool) -> bool {
    let (names, truncated) = Checker::files_under(data, path);
    match data.as_object().and_then(|m| m.get("total_commits")) {
        Some(Value::Int(Int::Small(n))) if *n >= 0 => change.notes.push(format!(
            "{n} commit(s) between the pinned and the new commit."
        )),
        Some(Value::Int(Int::Big(s))) if !s.starts_with('-') => change.notes.push(format!(
            "{s} commit(s) between the pinned and the new commit."
        )),
        _ => {}
    }
    if truncated {
        change
            .notes
            .push("The compare API truncated the file list: diff too large to summarize.".into());
        return true;
    }
    let scope = if path.is_empty() {
        "in the repository".to_string()
    } else {
        format!("under `{}`", sanitize_path(path))
    };
    change.files_total = names.len();
    change.files = names;
    change.files_heading = format!("Changed files {scope}");
    if change.files.is_empty() {
        if !tag_diff {
            return false;
        }
        change.notes.push(format!(
            "No files {scope} changed between the pinned and the new tag."
        ));
    }
    true
}

/// One `check_all` error: `(key, message)`.
pub type KeyedError = (String, String);

/// `check_all(root, net, token)`: every source, then Claude Code. A failure is recorded under
/// its key and the run goes on.
pub fn check_all(
    root: &Path,
    net: &dyn Net,
    token: Option<&str>,
) -> (Vec<Change>, Vec<KeyedError>) {
    let mut checker = Checker::new(root, net, token);
    let mut changes = Vec::new();
    let mut errors = Vec::new();
    let mut attempt = |key: String, result: Result<Option<Change>, Error>| match result {
        Ok(Some(c)) => changes.push(c),
        Ok(None) => {}
        Err(e) => errors.push((key, e.shown().to_string())),
    };
    let entries = read_json(&root.join("vendor/sources.json"))
        .ok()
        .and_then(|data| data.into_object())
        .and_then(|mut m| m.get_mut("sources").map(Value::take))
        .and_then(Value::into_array);
    let entries = match entries {
        Some(e) => e,
        None => {
            attempt(
                "sources.json".into(),
                Err(watch("cannot read vendor/sources.json")),
            );
            Vec::new()
        }
    };
    for entry in &entries {
        let key = match entry.as_object().and_then(|m| m.get("name")) {
            Some(Value::Str(n)) => n.clone(),
            _ => "?".to_string(),
        };
        let result = checker.check_entry(entry);
        attempt(sanitize_inline(&key), result);
    }
    let result = checker.check_claude_code();
    attempt(CLAUDE_CODE_KEY.into(), result);
    (changes, errors)
}

// ---- issue sync --------------------------------------------------------------------------

/// `_normal(text)`: `(text or "")` with CRLF as LF, `strip()`ped. A truthy non-string has no
/// `replace` (`AttributeError`).
fn normal(text: Option<&Value>) -> Result<String, Error> {
    match text {
        Some(Value::Str(s)) => Ok(pystr::strip_ws(&s.replace("\r\n", "\n")).to_string()),
        Some(v) if truthy(v) => Err(Error::Other("AttributeError".into())),
        _ => Ok(String::new()),
    }
}

/// `_without_budget(text)`: the normalized body without its "Startup budget" section (up to
/// the next `### ` heading). A backtrack-limit error names the pattern.
pub fn without_budget(text: &str) -> Result<String, Uncaught> {
    without_budget_with(&BUDGET_SECTION_RE, text)
}

/// [`without_budget`] with the compiled pattern as a parameter (the test seam for its
/// backtrack limit).
fn without_budget_with(re: &fancy_regex::Regex, text: &str) -> Result<String, Uncaught> {
    re.try_replacen(text, 0, "")
        .map(|c| c.into_owned())
        .map_err(|e| Uncaught::new("RegexError", format!("_without_budget: {e}")))
}

fn has_label(issue: &Map) -> bool {
    let Some(Value::Array(labels)) = issue.get("labels") else {
        return false;
    };
    labels.iter().any(|l| {
        let name = match l {
            Value::Object(m) => m.get("name"),
            other => Some(other),
        };
        matches!(name, Some(Value::Str(s)) if s == LABEL)
    })
}

/// `_is_ours(issue)`: bot-authored, labeled, not a pull request.
pub fn is_ours(issue: &Value) -> bool {
    let Some(m) = issue.as_object() else {
        return false;
    };
    if m.contains_key("pull_request") || !has_label(m) {
        return false;
    }
    matches!(
        m.get("user").and_then(Value::as_object).and_then(|u| u.get("login")),
        Some(Value::Str(l)) if l == BOT_LOGIN
    )
}

/// A sort key of `sorted(..., key=lambda i: i.get("number", 0))`.
enum NumberKey<'a> {
    Num(&'a Value),
    Str(&'a str),
    Other,
}

fn num_cmp(a: &Value, b: &Value) -> Ordering {
    fn int_text(v: &Value) -> Option<std::borrow::Cow<'_, str>> {
        match v {
            Value::Int(Int::Small(n)) => Some(n.to_string().into()),
            Value::Int(Int::Big(s)) => Some(s.as_str().into()),
            Value::Bool(b) => Some(u8::from(*b).to_string().into()),
            _ => None,
        }
    }
    fn as_f64(v: &Value) -> f64 {
        match v {
            Value::Int(Int::Small(n)) => *n as f64,
            Value::Int(Int::Big(s)) => s.parse().unwrap_or(0.0),
            Value::Bool(b) => f64::from(u8::from(*b)),
            Value::Float(f) => *f,
            _ => 0.0,
        }
    }
    if let (Some(x), Some(y)) = (int_text(a), int_text(b)) {
        let (xn, yn) = (x.starts_with('-'), y.starts_with('-'));
        return match (xn, yn) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (neg, _) => {
                let (x, y) = (x.trim_start_matches('-'), y.trim_start_matches('-'));
                let o = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
                if neg { o.reverse() } else { o }
            }
        };
    }
    as_f64(a).partial_cmp(&as_f64(b)).unwrap_or(Ordering::Equal)
}

/// `index_issues(issues)`: the newest (highest `number`) managed issue per marker key, open or
/// closed. Pull requests, unlabeled issues, issues another author wrote and bodies that do not
/// start with the marker are ignored. Ruling DV3-R4: `number`s order as Python's `<` does for
/// numbers and for strings; any other mix of two or more keys is the `TypeError` Python raises
/// for most of them (a list key, which Python orders, is one too).
pub fn index_issues(issues: &[Value]) -> Result<HashMap<String, Value>, Error> {
    static ZERO: Value = Value::Int(Int::Small(0));
    let ours: Vec<&Value> = issues.iter().filter(|i| is_ours(i)).collect();
    let keyed: Vec<(NumberKey<'_>, &Value)> = ours
        .iter()
        .map(|i| {
            let n = i.as_object().and_then(|m| m.get("number")).unwrap_or(&ZERO);
            let k = match n {
                Value::Int(_) | Value::Bool(_) | Value::Float(_) => NumberKey::Num(n),
                Value::Str(s) => NumberKey::Str(s),
                _ => NumberKey::Other,
            };
            (k, *i)
        })
        .collect();
    let all_num = keyed.iter().all(|(k, _)| matches!(k, NumberKey::Num(_)));
    let all_str = keyed.iter().all(|(k, _)| matches!(k, NumberKey::Str(_)));
    if keyed.len() > 1 && !all_num && !all_str {
        return Err(Error::Other("TypeError".into()));
    }
    let mut keyed = keyed;
    keyed.sort_by(|(a, _), (b, _)| match (a, b) {
        (NumberKey::Num(x), NumberKey::Num(y)) => num_cmp(x, y),
        (NumberKey::Str(x), NumberKey::Str(y)) => x.cmp(y),
        _ => Ordering::Equal,
    });
    let mut index = HashMap::new();
    for (_, issue) in keyed {
        let body = normal(issue.as_object().and_then(|m| m.get("body")))?;
        if let Some(c) = MARKER_RE.captures(&body) {
            index.insert(c[1].to_string(), issue.clone());
        }
    }
    Ok(index)
}

/// One `sync_issues` action record (`key`, `title`, `body`, `action`, then `number` and
/// `error` when set), in Python's dict order.
pub type Action = Map;

fn get_str<'a>(m: &'a Map, k: &str) -> &'a str {
    m.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// `sync_issues(changes, gh, dry_run, extra_sections)`: creates or updates one issue per
/// change and returns one action record per change. Per key the newest managed issue decides:
/// open and identical is `unchanged`, open otherwise `update`; closed with the same body
/// (ignoring the Startup budget section, which follows the checkout) is `dismissed`, closed
/// otherwise `create`. Without a client the action is `create-or-update`. A dry run only reads.
pub fn sync_issues(
    changes: &[Change],
    gh: Option<&dyn GitHub>,
    dry_run: bool,
    extra_sections: &HashMap<String, Sections>,
) -> Result<Vec<Action>, Uncaught> {
    let s = |t: &str| Value::Str(t.to_string());
    let mut index = None;
    if let Some(gh) = gh {
        match gh.list_issues().and_then(|issues| index_issues(&issues)) {
            Ok(i) => index = Some(i),
            Err(e) => {
                return Ok(changes
                    .iter()
                    .map(|c| {
                        let mut r = Map::new();
                        r.insert("key".into(), s(&c.key));
                        r.insert("action".into(), s("error"));
                        r.insert(
                            "error".into(),
                            s(&format!("cannot list issues ({})", e.type_name())),
                        );
                        r
                    })
                    .collect());
            }
        }
    }
    let number_of = |issue: &Value| -> Result<Value, Uncaught> {
        issue
            .as_object()
            .and_then(|m| m.get("number"))
            .cloned()
            .ok_or_else(|| Uncaught::new("KeyError", "'number'"))
    };
    let mut actions = Vec::new();
    for change in changes {
        let title = render_title(change);
        let body = render_body(
            change,
            extra_sections
                .get(&change.key)
                .map_or(&[][..], Vec::as_slice),
        );
        let mut record = Map::new();
        record.insert("key".into(), s(&change.key));
        record.insert("title".into(), s(&title));
        record.insert("body".into(), s(&body));
        match &index {
            None => record.insert("action".into(), s("create-or-update")),
            Some(index) => match index.get(&change.key) {
                None => record.insert("action".into(), s("create")),
                Some(existing) => {
                    let em = existing.as_object().expect("an issue the index kept");
                    if em.get("state").and_then(Value::as_str) == Some("closed") {
                        let old = normal(em.get("body")).unwrap_or_default();
                        let new = normal(Some(&s(&body))).unwrap_or_default();
                        if without_budget(&old)? == without_budget(&new)? {
                            record.insert("action".into(), s("dismissed"));
                            record.insert("number".into(), number_of(existing)?);
                        } else {
                            record.insert("action".into(), s("create"));
                        }
                    } else if normal(em.get("body")).unwrap_or_default()
                        == normal(Some(&s(&body))).unwrap_or_default()
                        && em.get("title").and_then(Value::as_str) == Some(title.as_str())
                    {
                        record.insert("action".into(), s("unchanged"));
                        record.insert("number".into(), number_of(existing)?);
                    } else {
                        record.insert("action".into(), s("update"));
                        record.insert("number".into(), number_of(existing)?);
                    }
                }
            },
        }
        let action = get_str(&record, "action").to_string();
        if !dry_run
            && (action == "create" || action == "update")
            && let Some(gh) = gh
        {
            let written = if action == "create" {
                gh.create_issue(&title, &body)
                    .and_then(|v| match v.as_object() {
                        Some(m) => Ok(m.get("number").cloned().unwrap_or(Value::Null)),
                        None => Err(Error::Other("AttributeError".into())),
                    })
            } else {
                let number = record.get("number").cloned().unwrap_or(Value::Null);
                gh.update_issue(&number, &title, &body).map(|_| Value::Null)
            };
            match written {
                Ok(number) if action == "create" => record.insert("number".into(), number),
                Ok(_) => {}
                Err(e) => {
                    record.insert("action".into(), s("error"));
                    record.insert(
                        "error".into(),
                        s(&format!("issue write failed ({})", e.type_name())),
                    );
                }
            }
        }
        actions.push(record);
    }
    Ok(actions)
}

// ---- CLI ---------------------------------------------------------------------------------

/// `_print_report(out, changes, actions, errors, dry_run)`.
fn print_report(
    out: &mut String,
    changes: &[Change],
    actions: &[Action],
    errors: &[KeyedError],
    dry_run: bool,
) {
    if changes.is_empty() && errors.is_empty() {
        out.push_str("No upstream changes.\n");
    }
    // `{a["key"]: a for a in actions}`: the last record of a key wins.
    let by_key = |key: &str| actions.iter().rev().find(|a| get_str(a, "key") == key);
    for change in changes {
        let action = by_key(&change.key)
            .and_then(|a| a.get("action").and_then(Value::as_str))
            .unwrap_or("?");
        let verb = if dry_run {
            format!("would {action}")
        } else {
            action.to_string()
        };
        out.push_str(&format!("[{verb}] {}\n", render_title(change)));
        if dry_run {
            let rule = "-".repeat(72);
            let body = by_key(&change.key)
                .map(|a| get_str(a, "body"))
                .filter(|b| !b.is_empty())
                .map_or_else(|| render_body(change, &[]), str::to_string);
            out.push_str(&format!("{rule}\n{body}\n{rule}\n"));
        }
    }
    for (key, error) in errors {
        out.push_str(&format!("ERROR {key}: {error}\n"));
    }
}

/// The GC-19 seams of [`main_with`]. `Default` is production: the real network and client,
/// [`startup_budget_sections`] and the real `BUDGET`.
pub struct Deps<'a> {
    /// The network (`net`); `None` is [`RealNet`].
    pub net: Option<&'a dyn Net>,
    /// The issue client (`gh`); `None` builds a [`GitHubClient`] when the token and the
    /// repository are set.
    pub gh: Option<&'a dyn GitHub>,
    /// `startup_budget.BUDGET`.
    pub budget: &'a [(&'a str, u64)],
    /// `startup_budget_sections`.
    pub startup_budget_sections: SectionsFn,
}

impl Default for Deps<'_> {
    fn default() -> Self {
        Deps {
            net: None,
            gh: None,
            budget: &startup_budget::BUDGET,
            startup_budget_sections,
        }
    }
}

/// `main`'s `ArgumentParser` (`prog` is the script's name, as `sys.argv[0]` gives it).
/// `COLUMNS` sets the help width, as `shutil.get_terminal_size` reads it (ruling DV3-R5).
fn spec(env: &Env) -> argparse::Spec {
    argparse::Spec::new("upstream_watch.py")
        .description("Compare every upstream pin and keep one issue per change.")
        .width(argparse::help_width(env.get("COLUMNS"), None))
        .arg(
            Arg::option(&["--dry-run"])
                .store_true()
                .help("print the issues that would be created or updated; write nothing"),
        )
        .arg(
            Arg::option(&["--json"])
                .metavar("OUT")
                .help("write the changes and errors as JSON"),
        )
        .arg(Arg::option(&["--root"]).help("repository root (default: this checkout)"))
}

/// Runs `anthology-dev upstream-watch …`; `args` are the arguments after `upstream-watch`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, stdin, &Env::from_process(), &cwd)
}

/// `upstream_watch.main(args)` in `cwd`, with `env` as the environment.
pub fn run_in(args: &[OsString], stdin: &mut dyn Read, env: &Env, cwd: &Path) -> Output {
    let _ = stdin;
    main_with(args, env, cwd, &Deps::default())
}

fn write_failed(e: std::io::Error, shown: &str) -> Uncaught {
    let mut exc = PyExc::from(e);
    if let PyExc::OSError { filename, .. } = &mut exc {
        *filename = Some(shown.to_string());
    }
    Uncaught::from_exc(&exc)
}

/// `main(argv, env, net, gh)` in `cwd` (module docs).
pub fn main_with(args: &[OsString], env: &Env, cwd: &Path, deps: &Deps<'_>) -> Output {
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
    let dry_run = parsed.flag("dry_run");
    let mut out = String::new();
    let token = env.get("GITHUB_TOKEN").filter(|t| !t.is_empty());
    let repo = env.get("GITHUB_REPOSITORY").filter(|r| !r.is_empty());
    let built;
    let gh: Option<&dyn GitHub> = match (deps.gh, token, repo) {
        (Some(gh), _, _) => Some(gh),
        (None, Some(token), Some(repo)) => {
            let base = env
                .get("GITHUB_API_URL")
                .filter(|b| !b.is_empty())
                .unwrap_or(GITHUB_API);
            built = GitHubClient::new(repo, token, base, env);
            Some(&built)
        }
        _ => None,
    };
    if gh.is_none() && !dry_run {
        return Output {
            stdout: b"GITHUB_TOKEN and GITHUB_REPOSITORY are required (or use --dry-run).\n"
                .to_vec(),
            code: 2,
            ..Output::default()
        };
    }
    let root = match parsed.os_str("root") {
        Some(r) => Root::new(Path::new(r), cwd),
        None => Root::new(&startup_budget::default_root(), cwd),
    };
    let real;
    let net: &dyn Net = match deps.net {
        Some(n) => n,
        None => {
            real = RealNet::new(env);
            &real
        }
    };
    let (changes, mut errors) = check_all(&root.fs, net, env.get("GITHUB_TOKEN"));
    let extra = match (deps.startup_budget_sections)(&root, &changes, deps.budget) {
        Ok(x) => x,
        Err(e) => {
            out.push_str(&format!("WARNING startup budget not computed ({})\n", e.ty));
            HashMap::new()
        }
    };
    let actions = match sync_issues(&changes, gh, dry_run, &extra) {
        Ok(a) => a,
        Err(e) => return e.into_output("upstream-watch", out.into_bytes()),
    };
    errors.extend(
        actions
            .iter()
            .filter(|a| get_str(a, "action") == "error")
            .map(|a| {
                (
                    get_str(a, "key").to_string(),
                    get_str(a, "error").to_string(),
                )
            }),
    );
    print_report(&mut out, &changes, &actions, &errors, dry_run);

    if let Some(target) = parsed.os_str("json").filter(|t| !t.is_empty()) {
        let mut doc = Map::new();
        doc.insert(
            "changes".into(),
            Value::Array(changes.iter().map(Change::to_json).collect()),
        );
        doc.insert(
            "errors".into(),
            Value::Array(
                errors
                    .iter()
                    .map(|(k, e)| {
                        str_map(vec![
                            ("key", Value::Str(k.clone())),
                            ("error", Value::Str(e.clone())),
                        ])
                    })
                    .collect(),
            ),
        );
        doc.insert(
            "issues".into(),
            Value::Array(
                actions
                    .iter()
                    .map(|a| {
                        let mut m = a.clone();
                        m.remove("body");
                        Value::Object(m)
                    })
                    .collect(),
            ),
        );
        let text = json::dumps(&Value::Object(doc), &INDENT2) + "\n";
        if let Err(e) = std::fs::write(cwd.join(target), text) {
            let shown = startup_budget::pathlib_str(&target.to_string_lossy());
            return write_failed(e, &shown).into_output("upstream-watch", out.into_bytes());
        }
    }
    if let Some(path) = env.get_os("GITHUB_OUTPUT").filter(|p| !p.is_empty())
        && !dry_run
    {
        let flagged = actions.iter().any(|a| {
            get_str(a, "key") == CLAUDE_CODE_KEY
                && matches!(get_str(a, "action"), "create" | "update")
        });
        let line = format!("claude_code_changed={flagged}\n");
        let written = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(cwd.join(path))
            .and_then(|mut f| f.write_all(line.as_bytes()));
        if let Err(e) = written {
            return write_failed(e, &path.to_string_lossy())
                .into_output("upstream-watch", out.into_bytes());
        }
    }
    Output {
        stdout: out.into_bytes(),
        stderr: Vec::new(),
        code: i32::from(!errors.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backtrack_limit_is_exit_1_naming_the_pattern() {
        // The real pattern stays far below fancy-regex's limit (a 1 MiB body passes below), so
        // a pattern that backtracks by construction drives the error path.
        let re = fancy_regex::RegexBuilder::new(r"(?=x)(x*)*y")
            .backtrack_limit(1)
            .build()
            .expect("pattern");
        let err = without_budget_with(&re, &"x".repeat(64)).expect_err("the limit is hit");
        assert_eq!(err.ty, "RegexError");
        assert!(err.msg.starts_with("_without_budget: "), "{}", err.msg);
        let out = err.into_output("upstream-watch", b"report\n".to_vec());
        assert_eq!(out.code, 1);
        assert_eq!(out.stdout, b"report\n");
        let stderr = String::from_utf8(out.stderr).expect("utf-8");
        assert!(stderr.starts_with("anthology-dev upstream-watch: RegexError: _without_budget: "));
    }

    #[test]
    fn without_budget_cuts_to_the_next_heading_or_the_end() {
        let body = "a\n### Startup budget\nx\ny\n### Update\nz";
        assert_eq!(without_budget(body).unwrap(), "a\n### Update\nz");
        assert_eq!(without_budget("a\n### Startup budget\nx").unwrap(), "a\n");
        assert_eq!(without_budget("### Startup budget\n").unwrap(), "");
        // `^` only after `\n`, never after `\r`.
        assert_eq!(
            without_budget("a\r### Startup budget\nx").unwrap(),
            "a\r### Startup budget\nx"
        );
        let long = format!("### Startup budget\n{}", "x".repeat(1 << 20));
        assert_eq!(without_budget(&long).unwrap(), "");
    }

    #[test]
    fn py_int_follows_int() {
        let v = |t: &str| json::loads(t).unwrap();
        assert_eq!(py_int(&v("7")).unwrap(), "7");
        assert_eq!(py_int(&v("7.9")).unwrap(), "7");
        assert_eq!(py_int(&v("-0.5")).unwrap(), "0");
        assert_eq!(py_int(&v("true")).unwrap(), "1");
        assert_eq!(py_int(&v("\" +1_2 \"")).unwrap(), "12");
        assert_eq!(py_int(&v("\"\\u0661\\u0662\"")).unwrap(), "12");
        assert_eq!(py_int(&v("\"-007\"")).unwrap(), "-7");
        for bad in ["\"x\"", "\"1__2\"", "\"_1\"", "\"\"", "\"0x1\""] {
            assert_eq!(
                py_int(&v(bad)),
                Err(Error::Other("ValueError".into())),
                "{bad}"
            );
        }
        assert_eq!(py_int(&v("null")), Err(Error::Other("TypeError".into())));
        assert_eq!(
            py_int(&v("1e400")),
            Err(Error::Other("OverflowError".into()))
        );
    }

    #[test]
    fn versions_compare_as_unbounded_ints() {
        let v = |t: &str| tag_version(t).unwrap().1;
        assert!(v("v7.10.0") > v("v7.9.9"));
        assert_eq!(v("v7.1"), v("v7.1.0"));
        assert_eq!(v("007.01"), v("7.1.0"));
        assert!(v("1.0.100000000000000000000000") > v("1.0.99999999999999999999999"));
    }
}
