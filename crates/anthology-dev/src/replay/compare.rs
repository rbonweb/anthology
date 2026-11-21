//! `replay` part `compare`: Python against Rust on every replayed call and text (design §13.7).
//!
//! - **runners:** runners 1-3 must agree byte for byte on exit code and stderr.
//! - **check:** `guard_rules.check` against `anthology_guard::check` on every call, both fed the
//!   envelope exactly as `guard.py` decodes it (lossy UTF-8, then `loads`).
//! - **analyze:** `guard_rules._analyze` against `anthology_shell::analyze` on every Bash
//!   `command` (`Entry` minus `opaque`; groups numbered by first appearance, as SH's oracle).
//! - **privacy:** `memory_privacy.privacy_findings` against `anthology_memory::privacy` on every
//!   text, with the same real `HOME` and host name on both sides.
//!
//! The Python side is `port/difftest/oracle/replay.py` through F4's `_driver.py`.
//!
//! **Classification** ([`Outcome`]). Only GC-16 classes explain a difference, and only in the
//! allowed direction ("Rust may block or report more, never less"):
//! - D-1: Python raised (it fails open) and Rust blocks or reports.
//! - D-6: Python's result held lone surrogates and equals Rust's once they are U+FFFD.
//! - D-7: Rust hit a resource limit (the GC-14 limit block, an incomplete analysis, an
//!   `unscannable` privacy finding) where Python finished.
//! - A **looser** difference (Rust allows what Python blocked, or reports fewer privacy
//!   findings) and a difference between runners 2 and 3 (the shim must be transparent) can
//!   never be explained.
//! - Any other difference is unexplained until the owner records it, after looking at the local
//!   details, in `<replay>/replay-corpus-explained.tsv` (`<id>\t<kind>\t<D-n>\t<note>`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anthology_memory::privacy::{PrivacyDeps, UNSCANNABLE, privacy_findings};
use anthology_pycompat::json::{self, Int, Map, PY_DEFAULT, Value};

use super::runners::{RunOut, Runner};

// SWAP(GD2): delete `guard_standin` and import `anthology_guard::{check, Rule, Verdict}`.
use guard_standin::{Verdict, check};
// SWAP(GD2): delete `shell_standin` and import `anthology_shell::{Analysis, analyze}` (GD2's
// merge brings SH2).
use shell_standin::{Analysis, analyze};

/// The GC-14 limit block line (`anthology guard hook`, D-7).
pub const LIMIT_LINE: &[u8] = b"Blocked by anthology-guardrails (internal): the guard could not \
finish analysing this input (time or pattern limit); it was blocked to be safe. Rephrase the \
command; if this keeps happening, disable anthology-guardrails with /plugin\n";

/// Whether the guard and shell are the SWAP stand-ins (the run says so).
pub const STANDINS: bool = true; // SWAP(GD2): false once the stand-ins are gone.

/// The plan §3.5 guard surface, until GD merges. `check` allows everything, so a replay run
/// before the swap shows every Python block as an unexplained (looser) difference: a stand-in
/// can never make a run look clean.
#[allow(dead_code, reason = "SWAP(GD2) stand-in for the plan §3.5 signatures")]
mod guard_standin {
    use anthology_pycompat::json::Value;

    #[derive(Copy, Clone, Eq, PartialEq, Debug)]
    pub enum Rule {
        G1,
        G2,
        G3,
        G4,
        G5,
        G6,
    }

    impl Rule {
        pub fn id(self) -> &'static str {
            match self {
                Rule::G1 => "G1",
                Rule::G2 => "G2",
                Rule::G3 => "G3",
                Rule::G4 => "G4",
                Rule::G5 => "G5",
                Rule::G6 => "G6",
            }
        }
    }

    pub enum Verdict {
        Allow,
        Block(Rule),
        Incomplete,
    }

    pub fn check(_tool_name: &Value, _tool_input: &Value, _home: &str) -> Verdict {
        Verdict::Allow
    }
}

/// The plan §3.4 shell surface, until SH merges (with GD). `analyze` finds nothing and is
/// complete, so every real command is an unexplained difference before the swap.
#[allow(dead_code, reason = "SWAP(GD2) stand-in for the plan §3.4 signatures")]
mod shell_standin {
    pub struct Redirect {
        pub op: String,
        pub target: String,
    }
    #[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
    pub struct GroupId(pub u32);
    pub struct Group {
        pub id: GroupId,
        pub texts: Vec<String>,
    }
    pub enum Opaque {}
    pub struct Entry {
        pub raw: Vec<String>,
        pub redirs: Vec<Redirect>,
        pub prog_path: String,
        pub prog: String,
        pub args: Vec<String>,
        pub cwd: Option<String>,
        pub exempt: bool,
        pub group: GroupId,
        pub code: Vec<String>,
        pub opaque: Vec<Opaque>,
    }
    pub struct Analysis {
        pub entries: Vec<Entry>,
        pub groups: Vec<Group>,
        pub incomplete: bool,
    }

    pub fn analyze(_command: &str, _home: &str) -> Analysis {
        Analysis {
            entries: Vec::new(),
            groups: Vec::new(),
            incomplete: false,
        }
    }
}

/// What one comparison found (module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Both sides agree.
    Same,
    /// A difference a GC-16 class explains.
    Explained(&'static str),
    /// A difference only the owner's explained file can explain.
    Diff,
    /// A difference nothing may explain (looser, or a shim that is not transparent).
    Never,
}

impl Outcome {
    fn rank(&self) -> u8 {
        match self {
            Outcome::Same => 0,
            Outcome::Explained(_) => 1,
            Outcome::Diff => 2,
            Outcome::Never => 3,
        }
    }

    /// The worse of two outcomes.
    pub fn worst(self, other: Outcome) -> Outcome {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

/// The comparison kinds, as named in the report and the explained file.
pub const KINDS: [&str; 4] = ["runners", "check", "analyze", "privacy"];

/// The GC-16 classes.
pub const CLASSES: [&str; 7] = ["D-1", "D-2", "D-3", "D-4", "D-5", "D-6", "D-7"];

// --- runners -------------------------------------------------------------------------------

/// Runners 1-3 (whichever ran) against each other. `py_raised`: Python's check raised on this
/// call (from the check comparison), so `guard.py` allowed by failing open.
pub fn classify_runners(outs: &[(Runner, RunOut)], py_raised: bool) -> Outcome {
    let get = |r: Runner| outs.iter().find(|(x, _)| *x == r).map(|(_, o)| o);
    let mut out = Outcome::Same;
    if let (Some(hook), Some(shim)) = (get(Runner::Hook), get(Runner::Shim))
        && hook != shim
    {
        out = Outcome::Never;
    }
    let Some(py) = get(Runner::Python) else {
        return out;
    };
    for rust in [Runner::Hook, Runner::Shim].into_iter().filter_map(get) {
        let o = if rust == py {
            Outcome::Same
        } else if rust.code == 2 && rust.stderr == LIMIT_LINE && (py.allowed() || py.blocked()) {
            Outcome::Explained("D-7")
        } else if py.blocked() && !rust.blocked() {
            Outcome::Never
        } else if py.allowed() && py.stderr.is_empty() && py_raised && rust.blocked() {
            Outcome::Explained("D-1")
        } else {
            Outcome::Diff
        };
        out = out.worst(o);
    }
    out
}

// --- check ----------------------------------------------------------------------------------

/// Rust's verdict on an envelope: `allow`, `block:<rule>`, `incomplete`, `malformed` (not an
/// object, or `tool_name` not a string: the hook allows), or `panic`.
pub fn rust_check(envelope: &[u8], home: &str) -> String {
    let Ok(v) = json::loads_bytes_lossy(envelope) else {
        return "malformed".into();
    };
    let Some(m) = v.as_object() else {
        return "malformed".into();
    };
    let Some(name @ Value::Str(_)) = m.get("tool_name") else {
        return "malformed".into();
    };
    let input = m.get("tool_input").cloned().unwrap_or(Value::Null);
    match catch_unwind(AssertUnwindSafe(|| check(name, &input, home))) {
        Ok(Verdict::Allow) => "allow".into(),
        Ok(Verdict::Block(rule)) => format!("block:{}", rule.id()),
        Ok(Verdict::Incomplete) => "incomplete".into(),
        Err(_) => "panic".into(),
    }
}

fn allows(verdict: &str) -> bool {
    verdict == "allow" || verdict == "malformed"
}

/// Python's check result (`{"verdict"}` or `{"raised"}`) against Rust's verdict.
pub fn classify_check(py: &PySide, rust: &str) -> Outcome {
    match py {
        PySide::Error(_) => Outcome::Diff,
        PySide::Raised(_) if allows(rust) => Outcome::Same,
        PySide::Raised(_) if rust == "panic" => Outcome::Diff,
        PySide::Raised(_) => Outcome::Explained("D-1"),
        PySide::Value(v) => {
            let py = v.as_str().unwrap_or("");
            if py == rust {
                Outcome::Same
            } else if rust == "incomplete" {
                Outcome::Explained("D-7")
            } else if py.starts_with("block:") && allows(rust) {
                Outcome::Never
            } else {
                Outcome::Diff
            }
        }
    }
}

// --- analyze --------------------------------------------------------------------------------

/// The Bash command of an envelope (Rust's decoding), if it has one.
pub fn bash_command(envelope: &[u8]) -> Option<String> {
    let v = json::loads_bytes_lossy(envelope).ok()?;
    let m = v.as_object()?;
    if m.get("tool_name")?.as_str()? != "Bash" {
        return None;
    }
    Some(
        m.get("tool_input")?
            .as_object()?
            .get("command")?
            .as_str()?
            .to_string(),
    )
}

/// `analyze(command, home)` in SH's oracle shape: `{"entries","groups","incomplete"}`.
pub fn rust_analyze(command: &str, home: &str) -> Value {
    match catch_unwind(AssertUnwindSafe(|| analyze(command, home))) {
        Ok(a) => analysis_value(&a),
        Err(_) => Value::Str("panic".into()),
    }
}

fn strs(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::Str(s.clone())).collect())
}

fn analysis_value(a: &Analysis) -> Value {
    let mut order = Vec::new();
    let mut ordinal = |id| match order.iter().position(|x| *x == id) {
        Some(i) => i,
        None => {
            order.push(id);
            order.len() - 1
        }
    };
    let mut entries = Vec::new();
    for e in &a.entries {
        let mut m = Map::new();
        m.insert("raw".into(), strs(&e.raw));
        let redirs = e
            .redirs
            .iter()
            .map(|r| Value::Array(vec![Value::Str(r.op.clone()), Value::Str(r.target.clone())]))
            .collect();
        m.insert("redirs".into(), Value::Array(redirs));
        m.insert("prog_path".into(), Value::Str(e.prog_path.clone()));
        m.insert("prog".into(), Value::Str(e.prog.clone()));
        m.insert("args".into(), strs(&e.args));
        m.insert("cwd".into(), e.cwd.clone().map_or(Value::Null, Value::Str));
        m.insert("exempt".into(), Value::Bool(e.exempt));
        let n = ordinal(e.group);
        m.insert("group".into(), Value::Int(Int::Small(n as i64)));
        m.insert("code".into(), strs(&e.code));
        entries.push(Value::Object(m));
    }
    let groups = order
        .iter()
        .map(|id| match a.groups.iter().find(|g| g.id == *id) {
            Some(g) => strs(&g.texts),
            None => Value::Array(Vec::new()),
        })
        .collect();
    let mut m = Map::new();
    m.insert("entries".into(), Value::Array(entries));
    m.insert("groups".into(), Value::Array(groups));
    m.insert("incomplete".into(), Value::Bool(a.incomplete));
    Value::Object(m)
}

/// Python's analysis (`{"analysis": <canonical JSON>, "d6"}` or `{"raised"}`) against Rust's.
pub fn classify_analyze(py: &PySide, d6: bool, rust: &Value) -> Outcome {
    let rust_incomplete = rust
        .as_object()
        .and_then(|m| m.get("incomplete"))
        .and_then(Value::as_bool)
        == Some(true);
    match py {
        PySide::Error(_) => Outcome::Diff,
        _ if rust.as_str() == Some("panic") => Outcome::Diff,
        PySide::Raised(_) => Outcome::Explained("D-1"),
        PySide::Value(v) => {
            let Some(text) = v.as_str() else {
                return Outcome::Diff;
            };
            let Ok(py) = json::loads(text) else {
                return Outcome::Diff;
            };
            if &py == rust {
                if d6 {
                    Outcome::Explained("D-6")
                } else {
                    Outcome::Same
                }
            } else if rust_incomplete {
                Outcome::Explained("D-7")
            } else {
                Outcome::Diff
            }
        }
    }
}

// --- privacy --------------------------------------------------------------------------------

/// Rust's privacy findings as `(line, kind)`, sorted.
pub fn rust_privacy(text: &str, home: &str, hostname: &str) -> Option<Vec<(usize, String)>> {
    let deps = PrivacyDeps {
        hostname: || None,
        home: Some(home.to_string()),
        limits: Default::default(),
    };
    catch_unwind(AssertUnwindSafe(|| {
        let mut out: Vec<(usize, String)> =
            privacy_findings(text, Some(home), Some(hostname), &deps)
                .into_iter()
                .map(|f| (f.line, f.kind))
                .collect();
        out.sort();
        out
    }))
    .ok()
}

/// Python's findings (`{"findings": [[line, kind], …]}` or `{"raised"}`) against Rust's
/// (`None` when Rust panicked).
pub fn classify_privacy(py: &PySide, rust: &Option<Vec<(usize, String)>>) -> Outcome {
    let Some(rust) = rust else {
        return Outcome::Diff;
    };
    let py = match py {
        PySide::Error(_) => return Outcome::Diff,
        PySide::Raised(_) if rust.is_empty() => return Outcome::Same,
        PySide::Raised(_) => return Outcome::Explained("D-1"),
        PySide::Value(v) => match findings_of(v) {
            Some(f) => f,
            None => return Outcome::Diff,
        },
    };
    if &py == rust {
        return Outcome::Same;
    }
    if !py.iter().all(|f| rust.contains(f)) {
        return Outcome::Never;
    }
    if rust
        .iter()
        .filter(|f| !py.contains(f))
        .all(|(_, k)| k == UNSCANNABLE)
    {
        Outcome::Explained("D-7")
    } else {
        Outcome::Diff
    }
}

fn findings_of(v: &Value) -> Option<Vec<(usize, String)>> {
    let mut out = Vec::new();
    for f in v.as_array()? {
        let pair = f.as_array()?;
        let line = usize::try_from(pair.first()?.as_i64()?).ok()?;
        out.push((line, pair.get(1)?.as_str()?.to_string()));
    }
    out.sort();
    Some(out)
}

// --- the Python side ------------------------------------------------------------------------

/// One Python answer.
#[derive(Clone, Debug, PartialEq)]
pub enum PySide {
    /// The op's value (`verdict`, `analysis` or `findings`).
    Value(Value),
    /// Python raised this exception type.
    Raised(String),
    /// The oracle failed (a traceback, a dead process): always a difference.
    Error(String),
}

/// `port/difftest/oracle/replay.py` through `_driver.py`: one long-lived process, one JSON line
/// each way per request. A process that dies fails that request and is restarted at the next.
pub struct Oracle {
    python: std::path::PathBuf,
    repo: std::path::PathBuf,
    proc: Option<(Child, ChildStdin, BufReader<ChildStdout>)>,
}

impl Oracle {
    pub fn new(python: &Path, repo: &Path) -> Oracle {
        Oracle {
            python: python.to_path_buf(),
            repo: repo.to_path_buf(),
            proc: None,
        }
    }

    fn start(&self) -> Result<(Child, ChildStdin, BufReader<ChildStdout>), String> {
        let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        let mut child = Command::new(&self.python)
            .arg("-I")
            .arg(self.repo.join("port/difftest/oracle/_driver.py"))
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", path)
            .env("LANG", "C.UTF-8")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", self.python.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        Ok((child, stdin, stdout))
    }

    /// Sends `case` to the `replay` module; returns its answer object.
    pub fn call(&mut self, case: Map) -> Result<Map, String> {
        if self.proc.is_none() {
            self.proc = Some(self.start()?);
        }
        let (_, stdin, stdout) = self.proc.as_mut().expect("started");
        let mut req = Map::new();
        req.insert("module".into(), Value::Str("replay".into()));
        req.insert("case".into(), Value::Object(case));
        let mut line = json::dumps(&Value::Object(req), &PY_DEFAULT);
        line.push('\n');
        let mut answer = String::new();
        let got = stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .and_then(|()| stdout.read_line(&mut answer));
        let parsed = match got {
            Ok(0) => Err("the oracle exited without an answer".to_string()),
            Ok(_) => json::loads(answer.trim_end())
                .ok()
                .and_then(|v| v.into_object())
                .ok_or_else(|| "the oracle's answer is not a JSON object".to_string()),
            Err(e) => Err(format!("the oracle pipe failed: {e}")),
        };
        if parsed.is_err() {
            self.stop();
        }
        parsed
    }

    /// An answer as a [`PySide`]: `key` holds the value.
    pub fn ask(&mut self, case: Map, key: &str) -> (PySide, bool) {
        match self.call(case) {
            Err(e) => (PySide::Error(e), false),
            Ok(m) => {
                let d6 = m.get("d6").and_then(Value::as_bool) == Some(true);
                if let Some(e) = m.get("error").and_then(Value::as_str) {
                    (PySide::Error(e.to_string()), d6)
                } else if let Some(t) = m.get("raised").and_then(Value::as_str) {
                    (PySide::Raised(t.to_string()), d6)
                } else if let Some(v) = m.get(key) {
                    (PySide::Value(v.clone()), d6)
                } else {
                    (
                        PySide::Error(format!("the oracle's answer has no {key:?}")),
                        d6,
                    )
                }
            }
        }
    }

    fn stop(&mut self) {
        if let Some((mut child, stdin, _)) = self.proc.take() {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        if let Some((mut child, stdin, _)) = self.proc.take() {
            drop(stdin); // EOF ends the driver's loop
            let _ = child.wait();
        }
    }
}

/// A request map.
pub fn case(op: &str, fields: &[(&str, Value)]) -> Map {
    let mut m = Map::new();
    m.insert("op".into(), Value::Str(op.into()));
    for (k, v) in fields {
        m.insert((*k).into(), v.clone());
    }
    m
}

// --- the owner's explained file --------------------------------------------------------------

pub const EXPLAINED_FILE: &str = "replay-corpus-explained.tsv";

/// `(id, kind) → class` from `<replay>/replay-corpus-explained.tsv` (absent: empty). Each line
/// is `<id>\t<kind>\t<D-n>\t<note>`; `#` lines and blank lines are skipped; anything else is an
/// error, as is an empty note.
pub fn load_explained(dir: &Path) -> Result<HashMap<(String, String), &'static str>, String> {
    let path = dir.join(EXPLAINED_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(HashMap::new());
    };
    let mut out = HashMap::new();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || {
            format!(
                "{}:{}: expected <id>\\t<kind>\\t<D-n>\\t<note>",
                path.display(),
                n + 1
            )
        };
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        let [id, kind, class, note] = parts[..] else {
            return Err(bad());
        };
        let class = CLASSES.iter().find(|c| **c == class).ok_or_else(bad)?;
        let id_ok = id.len() == super::hmac::ID_HEX && id.bytes().all(|b| b.is_ascii_hexdigit());
        if !id_ok || !KINDS.contains(&kind) || note.trim().is_empty() {
            return Err(bad());
        }
        out.insert((id.to_string(), kind.to_string()), *class);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(code: i32, stderr: &[u8]) -> RunOut {
        RunOut {
            code,
            stderr: stderr.to_vec(),
        }
    }

    const G1: &[u8] = b"Blocked by anthology-guardrails (G1): run it inside Docker (OrbStack); for browser work use BrowserOS Neo\n";

    #[test]
    fn runner_differences_are_classified() {
        let allow = out(0, b"");
        let block = out(2, G1);
        let limit = out(2, LIMIT_LINE);
        let run = |outs: &[(Runner, &RunOut)], raised| {
            let v: Vec<(Runner, RunOut)> = outs.iter().map(|(r, o)| (*r, (*o).clone())).collect();
            classify_runners(&v, raised)
        };
        use Runner::{Hook, Python, Shim};
        assert_eq!(
            run(&[(Python, &block), (Hook, &block), (Shim, &block)], false),
            Outcome::Same
        );
        assert_eq!(
            run(&[(Python, &allow), (Hook, &limit), (Shim, &limit)], false),
            Outcome::Explained("D-7")
        );
        assert_eq!(
            run(&[(Python, &block), (Hook, &limit)], false),
            Outcome::Explained("D-7")
        );
        assert_eq!(
            run(&[(Python, &allow), (Hook, &block)], true),
            Outcome::Explained("D-1")
        );
        assert_eq!(
            run(&[(Python, &allow), (Hook, &block)], false),
            Outcome::Diff
        );
        assert_eq!(
            run(&[(Python, &block), (Hook, &allow)], true),
            Outcome::Never
        );
        assert_eq!(
            run(&[(Python, &block), (Hook, &out(70, b"x"))], false),
            Outcome::Never
        );
        // The shim must be transparent, whatever Python said.
        assert_eq!(
            run(&[(Python, &allow), (Hook, &limit), (Shim, &block)], false),
            Outcome::Never
        );
        assert_eq!(run(&[(Hook, &allow), (Shim, &allow)], false), Outcome::Same);
        assert_eq!(run(&[(Python, &allow)], false), Outcome::Same);
    }

    #[test]
    fn check_differences_are_classified() {
        let py = |s: &str| PySide::Value(Value::Str(s.into()));
        assert_eq!(classify_check(&py("block:G1"), "block:G1"), Outcome::Same);
        assert_eq!(
            classify_check(&py("allow"), "incomplete"),
            Outcome::Explained("D-7")
        );
        assert_eq!(
            classify_check(&py("block:G2"), "incomplete"),
            Outcome::Explained("D-7")
        );
        assert_eq!(classify_check(&py("block:G2"), "allow"), Outcome::Never);
        assert_eq!(classify_check(&py("block:G2"), "malformed"), Outcome::Never);
        assert_eq!(classify_check(&py("block:G2"), "block:G3"), Outcome::Diff);
        assert_eq!(classify_check(&py("allow"), "block:G3"), Outcome::Diff);
        assert_eq!(classify_check(&py("allow"), "panic"), Outcome::Diff);
        let raised = PySide::Raised("RecursionError".into());
        assert_eq!(classify_check(&raised, "allow"), Outcome::Same);
        assert_eq!(
            classify_check(&raised, "block:G1"),
            Outcome::Explained("D-1")
        );
        assert_eq!(classify_check(&raised, "panic"), Outcome::Diff);
        assert_eq!(
            classify_check(&PySide::Error("x".into()), "allow"),
            Outcome::Diff
        );
    }

    #[test]
    fn rust_check_reads_the_envelope_as_the_hook_does() {
        assert_eq!(rust_check(b"not json", "/h"), "malformed");
        assert_eq!(rust_check(b"[1]", "/h"), "malformed");
        assert_eq!(rust_check(br#"{"tool_name": 1}"#, "/h"), "malformed");
        let v = rust_check(
            br#"{"tool_name": "Bash", "tool_input": {"command": "ls"}}"#,
            "/h",
        );
        assert!(
            ["allow", "block:G1", "incomplete"].contains(&v.as_str()) || v.starts_with("block:"),
            "{v}"
        );
    }

    #[test]
    fn analyze_differences_are_classified() {
        let rust = rust_analyze("ls", "/h");
        let text = json::dumps(&rust, &json::COMPACT_SORTED);
        let py = PySide::Value(Value::Str(text));
        assert_eq!(classify_analyze(&py, false, &rust), Outcome::Same);
        assert_eq!(
            classify_analyze(&py, true, &rust),
            Outcome::Explained("D-6")
        );
        let other = PySide::Value(Value::Str(
            r#"{"entries":[{"x":1}],"groups":[],"incomplete":false}"#.into(),
        ));
        assert_eq!(classify_analyze(&other, false, &rust), Outcome::Diff);
        let mut incomplete = rust.clone();
        if let Some(m) = incomplete.as_object_mut() {
            m.insert("incomplete".into(), Value::Bool(true));
        }
        assert_eq!(
            classify_analyze(&other, false, &incomplete),
            Outcome::Explained("D-7")
        );
        assert_eq!(
            classify_analyze(&PySide::Raised("RecursionError".into()), false, &rust),
            Outcome::Explained("D-1")
        );
        assert_eq!(
            bash_command(br#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#).as_deref(),
            Some("ls")
        );
        assert_eq!(
            bash_command(br#"{"tool_name":"Read","tool_input":{"command":"ls"}}"#),
            None
        );
    }

    #[test]
    fn privacy_differences_are_classified() {
        let f = |v: &[(usize, &str)]| {
            v.iter()
                .map(|(l, k)| (*l, k.to_string()))
                .collect::<Vec<_>>()
        };
        let py = |v: &[(usize, &str)]| {
            PySide::Value(Value::Array(
                v.iter()
                    .map(|(l, k)| {
                        Value::Array(vec![
                            Value::Int(Int::Small(*l as i64)),
                            Value::Str((*k).into()),
                        ])
                    })
                    .collect(),
            ))
        };
        let email = [(1, "email address")];
        assert_eq!(
            classify_privacy(&py(&email), &Some(f(&email))),
            Outcome::Same
        );
        assert_eq!(
            classify_privacy(
                &py(&email),
                &Some(f(&[(1, "email address"), (2, UNSCANNABLE)]))
            ),
            Outcome::Explained("D-7")
        );
        assert_eq!(
            classify_privacy(
                &py(&email),
                &Some(f(&[(1, "email address"), (2, "home path")]))
            ),
            Outcome::Diff
        );
        assert_eq!(
            classify_privacy(&py(&email), &Some(Vec::new())),
            Outcome::Never
        );
        assert_eq!(classify_privacy(&py(&email), &None), Outcome::Diff);
        assert_eq!(
            classify_privacy(&PySide::Raised("X".into()), &Some(Vec::new())),
            Outcome::Same
        );
        assert_eq!(
            classify_privacy(&PySide::Raised("X".into()), &Some(f(&email))),
            Outcome::Explained("D-1")
        );
        let found =
            rust_privacy("mail me: someone@example.org\nok", "/home/u", "box").expect("no panic");
        assert!(found.iter().all(|(l, _)| *l == 1), "{found:?}");
    }

    #[test]
    fn the_explained_file_is_strict() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load_explained(dir.path()).expect("absent").is_empty());
        let path = dir.path().join(EXPLAINED_FILE);
        std::fs::write(
            &path,
            "# comment\n\n0123456789abcdef\tcheck\tD-3\tword boundary next to a mark\n",
        )
        .expect("write");
        let got = load_explained(dir.path()).expect("ok");
        assert_eq!(
            got.get(&("0123456789abcdef".into(), "check".into())),
            Some(&"D-3")
        );
        for bad in [
            "0123456789abcdef\tcheck\tD-8\tx\n",
            "0123456789abcdef\tcheck\tD-3\t \n",
            "0123456789abcdeg\tcheck\tD-3\tx\n",
            "0123456789abcdef\tguard\tD-3\tx\n",
            "0123456789abcdef\tcheck\tD-3\n",
        ] {
            std::fs::write(&path, bad).expect("write");
            assert!(load_explained(dir.path()).is_err(), "{bad:?}");
        }
    }
}
