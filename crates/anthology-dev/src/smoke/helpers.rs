//! The pure helpers of smoke.py (unit-tested in `tests/smoke.rs`), plus the small JSON and
//! Python-formatting utilities they share.
//!
//! JSON goes through `pycompat::json` only (GC-16). Wherever Python raised on a malformed shape
//! (an `AttributeError` on a stream event that is not a dict), these helpers skip the entry
//! instead: the smoke then fails on its own check, not on a crash.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anthology_core::Env;
use anthology_pycompat::json::{self, Int, Map, Value};
use regex::Regex;

use super::compat::{shlex_split, split_ws, splitlines, strip_ws};

pub const PLUGINS: [&str; 3] = ["anthology", "anthology-guardrails", "anthology-superpowers"];
pub const AUTH_VARS: [&str; 3] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
];
pub const CLAUDE_TIMEOUT: u64 = 300;
pub const NO_AUTH: &str = "no API credentials (set ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN)";
pub const EXPECTED_AGENTS: usize = 11;
pub const MARKER: &str = "smoke-unpushed-marker";
pub const RTK_RAW_GIT: &str = "create/copy files and use";
pub const GUARD_G1: &str = "anthology-guardrails (G1)";
pub const PROMPT_WORKTREE: &str = "Use the Agent tool to run the anthology:implementer agent with the prompt \
'Run git log --oneline -3, change nothing, and report the output verbatim.' \
Then quote its report.";

/// An environment for a child process, as Python's `Dict[str, str]`.
pub type EnvMap = BTreeMap<String, String>;

/// PASS, FAIL or SKIP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Fail,
    Skip,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Skip => "SKIP",
        }
    }
}

/// `(status, one-line reason)`.
pub type Outcome = (Status, String);

/// Why a smoke stopped without an outcome.
#[derive(Debug, PartialEq, Eq)]
pub enum Abort {
    /// Python's `BudgetExhausted`.
    Budget,
    /// Any other exception, as `ClassName: message`.
    Error(String),
}

impl From<String> for Abort {
    fn from(message: String) -> Abort {
        Abort::Error(message)
    }
}

pub type Res<T> = Result<T, Abort>;

/// What a finished subprocess left: Python's `Proc`.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl Proc {
    pub fn new(code: i32, out: &str, err: &str) -> Proc {
        Proc {
            code,
            out: out.to_string(),
            err: err.to_string(),
        }
    }

    /// `out + "\n" + err`.
    pub fn text(&self) -> String {
        format!("{}\n{}", self.out, self.err)
    }
}

/// `NO_SCRUB`: three smokes need the subprocess scrub off (see the module doc of `smoke`).
pub fn no_scrub() -> EnvMap {
    EnvMap::from([(
        "CLAUDE_CODE_SUBPROCESS_ENV_SCRUB".to_string(),
        "0".to_string(),
    )])
}

// ---- Python formatting --------------------------------------------------------------------

/// `True` or `False`.
pub fn py_bool(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// `repr(str)`.
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
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
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr(value)` of a loaded JSON value (diagnostics only; nesting past 16 levels prints `...`).
pub fn py_repr(v: &Value) -> String {
    fn go(v: &Value, depth: usize) -> String {
        if depth > 16 {
            return "...".to_string();
        }
        match v {
            Value::Null => "None".to_string(),
            Value::Bool(b) => py_bool(*b).to_string(),
            Value::Int(Int::Small(n)) => n.to_string(),
            Value::Int(Int::Big(s)) => s.clone(),
            Value::Float(f) if f.is_nan() => "nan".to_string(),
            Value::Float(f) if f.is_infinite() => if *f < 0.0 { "-inf" } else { "inf" }.to_string(),
            Value::Float(_) => json::dumps(v, &json::PY_DEFAULT),
            Value::Str(s) => py_repr_str(s),
            Value::Array(items) => {
                let parts: Vec<String> = items.iter().map(|i| go(i, depth + 1)).collect();
                format!("[{}]", parts.join(", "))
            }
            Value::Object(map) => {
                let parts: Vec<String> = map
                    .iter()
                    .map(|(k, i)| format!("{}: {}", py_repr_str(k), go(i, depth + 1)))
                    .collect();
                format!("{{{}}}", parts.join(", "))
            }
        }
    }
    go(v, 0)
}

/// `str(value)`: a string as is, anything else as its `repr`.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

/// Python truthiness of a loaded JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(Int::Small(n)) => *n != 0,
        Value::Int(Int::Big(_)) => true,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `float(text)`: surrounding whitespace, `_` between digits, `inf` and `nan` are accepted.
pub fn py_float(text: &str) -> Option<f64> {
    let s = strip_ws(text);
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '_' {
            let digit = |j: Option<usize>| {
                j.and_then(|j| chars.get(j))
                    .is_some_and(char::is_ascii_digit)
            };
            if !(digit(i.checked_sub(1)) && digit(Some(i + 1))) {
                return None;
            }
        }
    }
    let cleaned: String = s.chars().filter(|c| *c != '_').collect();
    if cleaned.is_empty() || cleaned.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

/// The `positive` argparse type of smoke.py: a number greater than 0 and finite.
pub fn positive(text: &str) -> Result<f64, String> {
    let Some(value) = py_float(text) else {
        return Err(format!("not a number: {}", py_repr_str(text)));
    };
    if value > 0.0 && value.is_finite() {
        Ok(value)
    } else {
        Err(format!("must be greater than 0: {}", py_repr_str(text)))
    }
}

// ---- JSON building and reading ------------------------------------------------------------

pub fn jstr(s: &str) -> Value {
    Value::Str(s.to_string())
}

pub fn jarr(items: Vec<Value>) -> Value {
    Value::Array(items)
}

pub fn jobj(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (k, v) in pairs {
        map.insert(k.to_string(), v);
    }
    Value::Object(map)
}

pub fn jint(n: i64) -> Value {
    Value::Int(Int::Small(n))
}

/// `json.dumps(v)`.
pub fn dumps(v: &Value) -> String {
    json::dumps(v, &json::PY_DEFAULT)
}

/// `v.get(key)` of an object (`None` for a non-object).
pub fn field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object().and_then(|o| o.get(key))
}

fn field_str<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    field(v, key).and_then(Value::as_str)
}

/// `(ev.get("message") or {}).get("content")` when it is a list.
fn message_content(ev: &Value) -> &[Value] {
    field(ev, "message")
        .and_then(|m| field(m, "content"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// A JSON number as a Python `isinstance(x, (int, float))` sees it (a bool is an int).
fn json_number(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Int(Int::Small(n)) => Some(*n as f64),
        Value::Int(Int::Big(s)) => s.parse().ok(),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// The number of a result's `costUsd`-style field (`None` when it is not a number).
pub fn number_field(v: &Value, key: &str) -> Option<f64> {
    field(v, key).and_then(json_number)
}

// ---- output scrubbing ---------------------------------------------------------------------

/// `has_auth()`.
pub fn has_auth(env: &Env) -> bool {
    ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        .iter()
        .any(|k| env.get(k).is_some_and(|v| !v.is_empty()))
}

/// `redact(text)`: an auth value longer than 5 characters becomes `***`.
pub fn redact(env: &Env, text: &str) -> String {
    let mut text = text.to_string();
    for k in AUTH_VARS {
        if let Some(v) = env.get(k)
            && v.chars().count() > 5
        {
            text = text.replace(v, "***");
        }
    }
    text
}

/// `snippet(text, n)`: whitespace collapsed, auth values hidden, cut to `n` characters.
pub fn snippet(env: &Env, text: &str, n: usize) -> String {
    let redacted = redact(env, text);
    split_ws(&redacted).join(" ").chars().take(n).collect()
}

// ---- regexes (plain `regex` where there is no lookaround; fancy-regex where there is) -----

static NOT_FOUND_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)executable not found|command not found|\bexit(ed)?( code| status)? 127\b")
        .expect("NOT_FOUND_RE is a fixed pattern")
});
static BLOCKED_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)blocking because onFailure is .?block").expect("BLOCKED_RE is a fixed pattern")
});
static RTK_CLEAN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"clean . nothing to commit|Clean working tree").expect("RTK_CLEAN_RE is fixed")
});
static MEM_NOTE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"metadata:\s*\n\s+type:\s*project").expect("MEM_NOTE_RE is a fixed pattern")
});
static META_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\w+):\s*(.*)$").expect("META_LINE_RE is a fixed pattern"));
static STARTUP_ITEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^- `\$\{CLAUDE_PLUGIN_ROOT\}/knowledge/(.+?)/SKILL\.md`")
        .expect("STARTUP_ITEM_RE is a fixed pattern")
});
static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^# (.+)$").expect("HEADING_RE is a fixed pattern"));
/// The `## Startup` section: a look-ahead, so fancy-regex (`\Z` is written `\z`).
static STARTUP_SECTION_RE: LazyLock<fancy_regex::Regex> = LazyLock::new(|| {
    fancy_regex::Regex::new(r"(?ms)^## Startup\n(.*?)(?=^## |\z)")
        .expect("STARTUP_SECTION_RE is a fixed pattern")
});

/// `NOT_FOUND_RE.search(text)`.
pub fn not_found(text: &str) -> bool {
    NOT_FOUND_RE.is_match(text)
}

/// `BLOCKED_RE.search(text)`.
pub fn blocked(text: &str) -> bool {
    BLOCKED_RE.is_match(text)
}

fn limit_error(what: &str, err: &fancy_regex::Error) -> String {
    format!("RegexError: {what}: {err}")
}

/// `has_flag(help_text, flag)`: the flag as a whole word (not a prefix of `--scope-x`).
pub fn has_flag(help_text: &str, flag: &str) -> Result<bool, String> {
    let pattern = format!(r"(?<![\w-]){}(?![\w-])", fancy_regex::escape(flag));
    let re = fancy_regex::Regex::new(&pattern).map_err(|e| limit_error("has_flag", &e))?;
    re.is_match(help_text)
        .map_err(|e| limit_error("has_flag", &e))
}

/// The first `❯ problem` line of `claude plugin validate` output (else its start).
pub fn first_problem(env: &Env, text: &str) -> String {
    let line = splitlines(text)
        .into_iter()
        .find(|l| l.contains('\u{276f}'))
        .unwrap_or(text);
    snippet(env, &line.replace('\u{276f}', ""), 110)
}

// ---- context-cap hook ---------------------------------------------------------------------

/// Exactly `n` characters ending in ` END-<n>`.
pub fn cap_padding(n: usize) -> String {
    let tail = format!("END-{n}");
    let body = "lorem ipsum ".repeat(n / 12 + 1);
    // Python slices `[: n - len(tail) - 1]`; a negative end counts from the back.
    let end = n as i64 - tail.len() as i64 - 1;
    let end = if end < 0 {
        (body.len() as i64 + end).max(0) as usize
    } else {
        (end as usize).min(body.len())
    };
    format!("{} {tail}", &body[..end])
}

/// The SessionStart hook script of context-cap: POSIX `sh` (R4), no `python3`. It reads `CAP_N`
/// and prints the JSON object with the same padding as [`cap_padding`].
pub fn cap_hook_script() -> String {
    "#!/bin/sh\n\
# prints exactly CAP_N characters of additionalContext, ending in END-<CAP_N>\n\
n=${CAP_N:?CAP_N is not set}\n\
tail=\"END-$n\"\n\
keep=$((n - ${#tail} - 1))\n\
pad=$(awk -v k=\"$keep\" 'BEGIN { s = \"\"; while (length(s) < k) s = s \"lorem ipsum \"; printf \"%s\", substr(s, 1, k) }')\n\
printf '{\"hookSpecificOutput\": {\"hookEventName\": \"SessionStart\", \"additionalContext\": \"%s %s\"}}\\n' \"$pad\" \"$tail\"\n"
        .to_string()
}

// ---- hooks.json readers -------------------------------------------------------------------

/// The `hooks.SessionStart` list of a hooks.json, `None` on any read or shape problem.
fn session_start_groups(hooks_json: &Path) -> Option<Value> {
    let text = fs::read_to_string(hooks_json).ok()?;
    let mut data = json::loads(&text).ok()?;
    let mut hooks = data.as_object_mut()?.remove("hooks")?;
    hooks.as_object_mut()?.remove("SessionStart")
}

/// argv templates of the SessionStart hook commands that run a `.sh` file.
pub fn sh_session_start_commands(hooks_json: &Path) -> Result<Vec<Vec<String>>, String> {
    let Some(groups) = session_start_groups(hooks_json) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for group in groups.as_array().map_or(&[][..], Vec::as_slice) {
        let hooks = field(group, "hooks")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        for hook in hooks {
            let Some(cmd) = field_str(hook, "command") else {
                continue;
            };
            let argv: Vec<String> = match field(hook, "args").and_then(Value::as_array) {
                Some(args) => std::iter::once(cmd.to_string())
                    .chain(args.iter().map(py_str))
                    .collect(),
                None => shlex_split(cmd).map_err(|e| format!("ValueError: {e}"))?,
            };
            if argv.iter().any(|a| a.ends_with(".sh")) {
                found.push(argv);
            }
        }
    }
    Ok(found)
}

/// Whether the hooks.json has any SessionStart hook.
pub fn has_session_start(hooks_json: &Path) -> bool {
    session_start_groups(hooks_json).is_some_and(|v| truthy(&v))
}

// ---- stream-json and transcripts ----------------------------------------------------------

/// Every line of `text` that is a JSON object.
pub fn stream_events(text: &str) -> Vec<Value> {
    splitlines(text)
        .into_iter()
        .filter_map(|line| json::loads(line).ok())
        .filter(|ev| ev.as_object().is_some())
        .collect()
}

/// The text of every tool_result in a stream-json run (what the tool returned, not what was asked).
pub fn tool_result_texts(events: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for ev in events {
        if field_str(ev, "type") != Some("user") {
            continue;
        }
        for block in message_content(ev) {
            if field_str(block, "type") != Some("tool_result") {
                continue;
            }
            out.push(match field(block, "content") {
                Some(Value::Str(s)) => s.clone(),
                Some(Value::Array(parts)) => parts
                    .iter()
                    .filter(|b| b.as_object().is_some())
                    .map(|b| field_str(b, "text").unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => String::new(),
            });
        }
    }
    out
}

/// `(total_cost_usd, result text)` from the `result` event of a `claude -p` json or stream-json run.
pub fn result_cost_and_text(out: &str) -> (Option<f64>, Option<String>) {
    let events: Vec<Value> = match json::loads(strip_ws(out)) {
        Ok(data) if data.as_object().is_some() => vec![data],
        Ok(data) => data
            .into_array()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.as_object().is_some())
            .collect(),
        Err(_) => stream_events(out),
    };
    let Some(result) = events
        .iter()
        .rev()
        .find(|e| field_str(e, "type") == Some("result"))
    else {
        return (None, None);
    };
    (
        number_field(result, "total_cost_usd"),
        field_str(result, "result").map(str::to_string),
    )
}

/// Names of the tools the model asked to use in a stream-json run.
pub fn tool_uses(events: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for ev in events {
        if field_str(ev, "type") != Some("assistant") {
            continue;
        }
        for block in message_content(ev) {
            if field_str(block, "type") == Some("tool_use")
                && let Some(name) = field_str(block, "name")
            {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// Output, stderr and exit-code fields of the hook events only (not ids or other metadata).
pub fn hook_failure_text(events: &[Value]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for ev in events {
        let is_hook = field_str(ev, "type") == Some("system")
            && field_str(ev, "subtype").is_some_and(|s| s.starts_with("hook"));
        if !is_hook {
            continue;
        }
        for k in ["output", "stdout", "stderr", "message", "error"] {
            if let Some(s) = field_str(ev, k) {
                parts.push(s.to_string());
            }
        }
        for k in ["exit_code", "exitCode"] {
            match field(ev, k) {
                Some(Value::Int(Int::Small(n))) => parts.push(format!("exit code {n}")),
                Some(Value::Int(Int::Big(s))) => parts.push(format!("exit code {s}")),
                Some(Value::Bool(b)) => parts.push(format!("exit code {}", py_bool(*b))),
                _ => {}
            }
        }
    }
    parts.join("\n")
}

/// Every `*.jsonl` directly in a `subagents` folder anywhere under `<cfg>/projects`, sorted.
pub fn subagent_files(cfg: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `**` does not follow symlinks; `*.jsonl` matches a name, so a dot file counts.
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if entry.file_name() == "subagents"
                    && let Ok(inner) = fs::read_dir(&path)
                {
                    for file in inner.flatten() {
                        let is_file = file.path().is_file();
                        if is_file && file.file_name().to_string_lossy().ends_with(".jsonl") {
                            out.push(file.path());
                        }
                    }
                }
                walk(&path, out);
            }
        }
    }
    let projects = cfg.join("projects");
    let mut files = Vec::new();
    if projects.is_dir() {
        walk(&projects, &mut files);
    }
    files.sort();
    files
}

/// The `cwd` of every entry of one transcript JSONL.
pub fn transcript_cwds(text: &str) -> Vec<String> {
    splitlines(text)
        .into_iter()
        .filter_map(|line| json::loads(line).ok())
        .filter_map(|ev| field_str(&ev, "cwd").map(str::to_string))
        .collect()
}

/// `(tool name, input as JSON)` of every tool_use in one transcript JSONL, in order.
pub fn transcript_calls(text: &str) -> Vec<(String, String)> {
    let mut calls = Vec::new();
    for line in splitlines(text) {
        let Ok(ev) = json::loads(line) else {
            continue;
        };
        for block in message_content(&ev) {
            if field_str(block, "type") != Some("tool_use") {
                continue;
            }
            if let Some(name) = field_str(block, "name") {
                let input = field(block, "input")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Map::new()));
                calls.push((name.to_string(), dumps(&input)));
            }
        }
    }
    calls
}

/// Every message.model in the subagent transcripts under `<cfg>/projects`, and the files read.
pub fn transcript_models(cfg: &Path) -> Result<(Vec<String>, Vec<PathBuf>), String> {
    let files = subagent_files(cfg);
    let mut models = Vec::new();
    for f in &files {
        let bytes = fs::read(f).map_err(|e| format!("OSError: {}: {e}", f.display()))?;
        let text = String::from_utf8_lossy(&bytes);
        for line in splitlines(&text) {
            let Ok(ev) = json::loads(line) else {
                continue;
            };
            if let Some(m) = field(&ev, "message").and_then(|m| field_str(m, "model")) {
                models.push(m.to_string());
            }
        }
    }
    Ok((models, files))
}

/// A transcript file read the way `read_text(errors="replace")` does.
pub fn read_lossy(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| format!("OSError: {}: {e}", path.display()))
}

// ---- agents -------------------------------------------------------------------------------

/// One agent file: `{model, worktree, preloads, startup}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpec {
    pub model: String,
    pub worktree: bool,
    pub preloads: Vec<String>,
    pub startup: Vec<String>,
}

fn strip_quotes(s: &str) -> &str {
    s.trim_matches(['"', '\''])
}

/// One agent file, by plain line parsing (no PyYAML): model / isolation from the frontmatter,
/// preloads from its `skills:` list (the `plugin:` prefix dropped), startup from the `## Startup`
/// section (refs `x` or `ds/x` of its `knowledge/.../SKILL.md` paths).
pub fn parse_agent(text: &str) -> Result<AgentSpec, String> {
    let (head, body) = match text.strip_prefix("---\n") {
        Some(rest) => rest.split_once("\n---").unwrap_or((rest, "")),
        None => ("", text),
    };
    let mut meta: BTreeMap<String, String> = BTreeMap::new();
    let mut preloads = Vec::new();
    let mut listing = false;
    for line in splitlines(head) {
        if let Some(m) = META_LINE_RE.captures(line) {
            listing = &m[1] == "skills";
            meta.insert(m[1].to_string(), strip_quotes(strip_ws(&m[2])).to_string());
        } else if listing && strip_ws(line).starts_with("- ") {
            let item = strip_quotes(strip_ws(&strip_ws(line)[2..]));
            preloads.push(item.rsplit(':').next().unwrap_or(item).to_string());
        }
    }
    let section = STARTUP_SECTION_RE
        .captures(body)
        .map_err(|e| limit_error("parse_agent", &e))?;
    let section_text = section
        .as_ref()
        .and_then(|c| c.get(1))
        .map_or("", |m| m.as_str());
    let startup = STARTUP_ITEM_RE
        .captures_iter(section_text)
        .map(|c| {
            let r = &c[1];
            match r.strip_prefix("design-skills/skills/") {
                Some(rest) => format!("ds/{rest}"),
                None => r.to_string(),
            }
        })
        .collect();
    Ok(AgentSpec {
        model: meta
            .get("model")
            .cloned()
            .unwrap_or_else(|| "?".to_string()),
        worktree: meta.get("isolation").is_some_and(|v| v == "worktree"),
        preloads,
        startup,
    })
}

/// `(agent name, parse_agent())` of `plugins/anthology/agents/<name>.md`, in file-name order.
pub fn load_agents(agents_dir: &Path) -> Result<Vec<(String, AgentSpec)>, String> {
    let mut files: Vec<PathBuf> = match fs::read_dir(agents_dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md") && p.file_name().is_some())
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    let mut agents = Vec::new();
    for f in files {
        let text = fs::read_to_string(&f).map_err(|e| format!("OSError: {}: {e}", f.display()))?;
        let stem = f
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        agents.push((stem, parse_agent(&text)?));
    }
    Ok(agents)
}

/// Path regex of a Startup file; a ref is `x` or `ds/x` (folder names hold only letters, digits,
/// hyphens). The text is a regex source, compiled with fancy-regex like every pattern built from
/// run-time text (plan GC-16).
pub fn startup_regex(reference: &str) -> String {
    match reference.strip_prefix("ds/") {
        Some(rest) => format!(r"knowledge/design-skills/skills/{rest}/SKILL\.md"),
        None => format!(r"knowledge/{reference}/SKILL\.md"),
    }
}

/// `re.search(pattern, text)` with fancy-regex.
pub fn search_fancy(pattern: &str, text: &str) -> Result<bool, String> {
    let re = fancy_regex::Regex::new(pattern).map_err(|e| limit_error("startup_regex", &e))?;
    re.is_match(text)
        .map_err(|e| limit_error("startup_regex", &e))
}

/// The first `# ` heading of a superpowers skill, without the marker ('' when there is none).
pub fn skill_heading(root: &Path, skill: &str) -> Result<String, String> {
    let path = root
        .join("plugins")
        .join("anthology-superpowers")
        .join("skills")
        .join(skill)
        .join("SKILL.md");
    let text =
        fs::read_to_string(&path).map_err(|e| format!("OSError: {}: {e}", path.display()))?;
    let body = if text.starts_with("---\n") {
        text.split_once("\n---\n")
            .map(|(_, body)| body)
            .ok_or_else(|| "IndexError: list index out of range".to_string())?
    } else {
        text.as_str()
    };
    Ok(HEADING_RE
        .captures(body)
        .map_or_else(String::new, |m| strip_ws(&m[1]).to_string()))
}

/// What is wrong with an agent's Startup reading: every ref read, in order, before its `*.md` Glob.
pub fn startup_problems(
    calls: &[(String, String)],
    refs: &[String],
) -> Result<Vec<String>, String> {
    let Some(glob) = calls
        .iter()
        .position(|(n, t)| n == "Glob" && t.contains("*.md"))
    else {
        return Ok(vec!["no Glob of *.md".to_string()]);
    };
    let mut problems = Vec::new();
    let mut last: Option<usize> = None;
    for reference in refs {
        let pattern = startup_regex(reference);
        let mut at = None;
        for (i, (n, t)) in calls.iter().enumerate() {
            if n == "Read" && search_fancy(&pattern, t)? {
                at = Some(i);
                break;
            }
        }
        match at {
            None => problems.push(format!("{reference} never read")),
            Some(i) if i > glob => problems.push(format!("{reference} read after the Glob")),
            Some(i) if last.is_some_and(|l| i < l) => {
                problems.push(format!("{reference} read out of order"));
            }
            Some(i) => last = Some(i),
        }
    }
    Ok(problems)
}

// ---- files under a scratch dir --------------------------------------------------------------

/// Whether any file under `directory` contains `needle` (lower case) in its lower-cased text.
pub fn find_text(directory: &Path, needle: &str) -> bool {
    fn walk(dir: &Path, needle: &str) -> bool {
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                if walk(&path, needle) {
                    return true;
                }
            } else if path.is_file()
                && let Ok(bytes) = fs::read(&path)
                && String::from_utf8_lossy(&bytes)
                    .to_lowercase()
                    .contains(needle)
            {
                return true;
            }
        }
        false
    }
    directory.is_dir() && walk(directory, needle)
}

/// Every path under `root` through an `agent-memory/anthology:<agent>` folder (the agent type's colon).
pub fn colon_memory_paths(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in &entries {
            let path = entry.path().to_string_lossy().into_owned();
            if path.contains("agent-memory/anthology:") {
                out.push(path);
            }
        }
        for entry in &entries {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&entry.path(), out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

// ---- verdict helpers ----------------------------------------------------------------------

/// What is wrong with an rtk-rewrite run: rtk's condensed status seen, no raw git status, G1 seen.
pub fn rtk_problems(results: &[String], hooks: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if !results.iter().any(|r| RTK_CLEAN_RE.is_match(r)) {
        problems.push(
            "no tool result shows rtk's condensed git status (clean - nothing to commit)"
                .to_string(),
        );
    }
    if results.iter().any(|r| r.contains(RTK_RAW_GIT)) {
        problems.push("a tool result shows the raw git status text".to_string());
    }
    let seen = format!("{hooks}\n{}", results.join("\n"));
    if !seen.contains(GUARD_G1) {
        problems.push(format!(
            "no hook event or tool result shows '{GUARD_G1}' (the guard did not fire on the playwright command)"
        ));
    }
    problems
}

/// What is wrong with an agent's memory save: `anthology-mem save` run by the SUBAGENT (not the main
/// session), no heredoc or write tool in either, a nested `type: project`, no colon path.
pub fn memory_save_problems(
    main_calls: &[(String, String)],
    sub_calls: &[(String, String)],
    note: Option<&str>,
    stray: &[String],
) -> Vec<String> {
    let mut problems = Vec::new();
    let calls: Vec<&(String, String)> = main_calls.iter().chain(sub_calls).collect();
    if !sub_calls
        .iter()
        .any(|(n, i)| n == "Bash" && i.contains("anthology-mem save"))
    {
        problems.push("anthology-mem save was not run by the subagent".to_string());
    }
    if calls.iter().any(|(n, i)| n == "Bash" && i.contains("<<")) {
        problems.push("a Bash call used a heredoc".to_string());
    }
    let mut used: Vec<&str> = calls
        .iter()
        .map(|(n, _)| n.as_str())
        .filter(|n| ["Write", "Edit", "NotebookEdit"].contains(n))
        .collect();
    used.sort_unstable();
    used.dedup();
    if !used.is_empty() {
        problems.push(format!("used {}", used.join(", ")));
    }
    match note {
        None => problems.push("the note file was not written".to_string()),
        Some(text) if !MEM_NOTE_RE.is_match(text) => {
            problems.push("the note lacks a nested `metadata: type: project`".to_string());
        }
        Some(_) => {}
    }
    if let Some(first) = stray.first() {
        problems.push(format!("a path has the agent type's colon: {first}"));
    }
    problems
}

/// What is wrong with a guard-timeout run: the Bash call was made and blocked, the hook is
/// reported as timed out, and the command's output never reached a tool result.
///
/// Verified on Claude Code 2.1.311: a `PreToolUse` command hook that outlives its `timeout` ends
/// as the tool result `PreToolUse:Bash hook error: [sleep 30]: timed out; blocking because
/// onFailure is "block"` (an error result); its `hook_response` event has an empty output,
/// `exit_code` 1 and `outcome` `cancelled`, so the words come from the tool result, not the hook
/// event.
pub fn guard_timeout_problems(bash_called: bool, hooks: &str, results: &[String]) -> Vec<String> {
    let mut problems = Vec::new();
    let all = format!("{hooks}\n{}", results.join("\n"));
    if !bash_called {
        problems.push("Bash was never called".to_string());
    }
    if !blocked(&all) {
        problems.push("'blocking because onFailure is \"block\"' not seen".to_string());
    }
    if !all.to_lowercase().contains("timed out") {
        problems.push("'timed out' not seen in the hook events or tool results".to_string());
    }
    if results.iter().any(|t| t.contains("RAN-OK")) {
        problems.push("the command ran (RAN-OK is in a tool result)".to_string());
    }
    problems
}
