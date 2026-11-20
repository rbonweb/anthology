//! The Python oracle driver (F4, design §13.2).
//!
//! One long-lived `<python> -I port/difftest/oracle/_driver.py` process per run, started at the
//! first call, speaking JSONL on its stdin and stdout:
//!
//! - request: `{"module": "<file stem>", "case": {...}}`;
//! - response: the module's `run(case)` dict, or `{"error": "<traceback>"}`.
//!
//! [`Oracle::call`] returns the raw response. It also notes, for the harness, every
//! `"divergence"` mark and every error it sees ([`Oracle::take_notes`]); a `"divergence"` that
//! is not a string is an error. A process that dies or answers garbage gives an error response
//! for that case (no retry) and is restarted at the next call. The process gets a fixed
//! environment: `PATH` and `LANG=C.UTF-8` only. A surface can
//! also note a divergence it applied itself ([`Oracle::mark`]: the D-2/D-4 normalization hooks).
#![allow(
    dead_code,
    reason = "published for the surface lanes; not every item has a caller yet"
)]

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use anthology_pycompat::json::{self, Map, PY_DEFAULT, Value};

use crate::observe::{divergence_mark, oracle_error};

/// What the oracle saw during one case.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    /// `"divergence"` marks (and [`Oracle::mark`] calls), in order.
    pub marks: Vec<String>,
    /// Error responses, in order.
    pub errors: Vec<String>,
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// The oracle process (module docs).
pub struct Oracle {
    program: OsString,
    args: Vec<OsString>,
    cwd: PathBuf,
    proc: Option<Proc>,
    notes: Notes,
    calls: usize,
    starts: usize,
}

impl Oracle {
    /// `<python> -I <root>/port/difftest/oracle/_driver.py`, run in `root`.
    pub fn python(python: &str, root: &std::path::Path) -> Oracle {
        let driver = root.join("port/difftest/oracle/_driver.py");
        Oracle::spawn(
            python.into(),
            vec!["-I".into(), driver.into_os_string()],
            root.to_path_buf(),
        )
    }

    /// Any program speaking the protocol (tests use fakes). Nothing starts until the first call.
    pub fn spawn(program: OsString, args: Vec<OsString>, cwd: PathBuf) -> Oracle {
        Oracle {
            program,
            args,
            cwd,
            proc: None,
            notes: Notes::default(),
            calls: 0,
            starts: 0,
        }
    }

    /// Sends `{"module": module, "case": case}` and returns the response (module docs).
    pub fn call(&mut self, module: &str, case: &Value) -> Value {
        self.calls += 1;
        // No retry: a healthy driver never dies, so a dead one fails the case it was asked
        // (and the next call starts a fresh one).
        let resp = match self.exchange(module, case) {
            Ok(resp) => resp,
            Err(why) => {
                self.stop();
                error_value(&format!("oracle process: {why}"))
            }
        };
        if let Some(e) = oracle_error(&resp) {
            self.notes.errors.push(e);
        }
        match resp.as_object().and_then(|o| o.get("divergence")) {
            None => {}
            Some(_) => match divergence_mark(&resp) {
                Some(d) => self.notes.marks.push(d.to_string()),
                None => self
                    .notes
                    .errors
                    .push("oracle: \"divergence\" is not a string".into()),
            },
        }
        resp
    }

    /// Notes a divergence class the surface applied itself (a normalization hook).
    pub fn mark(&mut self, class: &str) {
        self.notes.marks.push(class.to_string());
    }

    /// The notes since the last call of this, cleared.
    pub fn take_notes(&mut self) -> Notes {
        std::mem::take(&mut self.notes)
    }

    /// How many calls were made.
    pub fn calls(&self) -> usize {
        self.calls
    }

    /// How many times the process was started (more than 1 means it died at least once).
    pub fn starts(&self) -> usize {
        self.starts
    }

    fn exchange(&mut self, module: &str, case: &Value) -> Result<Value, String> {
        if self.proc.is_none() {
            self.starts += 1;
            self.proc = Some(self.start()?);
        }
        let p = self.proc.as_mut().expect("started");
        let mut req = Map::new();
        req.insert("module".into(), Value::Str(module.into()));
        req.insert("case".into(), case.clone());
        let mut line = json::dumps(&Value::Object(req), &PY_DEFAULT);
        line.push('\n');
        p.stdin
            .write_all(line.as_bytes())
            .and_then(|()| p.stdin.flush())
            .map_err(|e| format!("cannot send a request: {e}"))?;
        let mut answer = String::new();
        match p.stdout.read_line(&mut answer) {
            Ok(0) => Err("exited without an answer".into()),
            Ok(_) => json::loads(answer.trim_end_matches('\n')).map_err(|e| {
                format!(
                    "not a JSON answer ({e}): {}",
                    crate::case::cut(&answer, 200)
                )
            }),
            Err(e) => Err(format!("cannot read the answer: {e}")),
        }
    }

    fn start(&self) -> Result<Proc, String> {
        // A fixed environment: this process's PATH and LANG=C.UTF-8, nothing of the developer's
        // or the runner's (HOME, TZ, CLAUDE_*). Modules set what their case needs.
        let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .current_dir(&self.cwd)
            .env_clear()
            .env("PATH", path)
            .env("LANG", "C.UTF-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", self.program.to_string_lossy()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        Ok(Proc {
            child,
            stdin,
            stdout,
        })
    }

    fn stop(&mut self) {
        if let Some(mut p) = self.proc.take() {
            drop(p.stdin);
            let _ = p.child.kill();
            let _ = p.child.wait();
        }
    }
}

impl Drop for Oracle {
    fn drop(&mut self) {
        if let Some(mut p) = self.proc.take() {
            // EOF on stdin ends the driver's loop.
            drop(p.stdin);
            let _ = p.child.wait();
        }
    }
}

/// `{"error": msg}`.
pub fn error_value(msg: &str) -> Value {
    let mut m = Map::new();
    m.insert("error".into(), Value::Str(msg.into()));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn sh(script: &str) -> Oracle {
        Oracle::spawn(
            "/bin/sh".into(),
            vec!["-c".into(), script.into()],
            Path::new("/").to_path_buf(),
        )
    }

    #[test]
    fn a_call_sends_one_line_and_reads_one_line() {
        // Echoes the request back inside a result, and marks D-6.
        let mut o = sh(r#"while IFS= read -r l; do
            printf '{"exit": 0, "stdout": "%s", "stderr": "", "divergence": "D-6"}\n' \
                "$(printf '%s' "$l" | sed 's/["\\]/./g')"; done"#);
        assert_eq!(o.calls(), 0);
        let case = json::loads(r#"{"x": [1, "a"]}"#).expect("json");
        let resp = o.call("json", &case);
        let stdout = resp
            .as_object()
            .and_then(|m| m.get("stdout"))
            .and_then(Value::as_str);
        assert_eq!(stdout, Some("{.module.: .json., .case.: {.x.: [1, .a.]}}"));
        o.mark("D-2");
        let notes = o.take_notes();
        assert_eq!(notes.marks, ["D-6", "D-2"]);
        assert!(notes.errors.is_empty());
        assert_eq!(o.take_notes(), Notes::default());
        o.call("json", &case);
        assert_eq!(o.calls(), 2);
    }

    #[test]
    fn errors_are_noted_and_a_dead_oracle_restarts() {
        // Answers one error, then exits: the next case fails (no retry hides the death), the
        // one after gets a fresh process.
        let mut o = sh(r#"read -r l; echo '{"error": "Traceback: boom"}'"#);
        let v = Value::Null;
        let first = o.call("m", &v);
        assert_eq!(
            oracle_error(&first).as_deref(),
            Some("oracle error: Traceback: boom")
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
        let second = o.call("m", &v);
        let e = oracle_error(&second).expect("an error");
        assert!(e.starts_with("oracle error: oracle process: "), "{e}");
        let third = o.call("m", &v);
        assert_eq!(
            oracle_error(&third).as_deref(),
            Some("oracle error: Traceback: boom")
        );
        assert_eq!(o.starts(), 2);
        assert_eq!(o.take_notes().errors.len(), 3);
        let mut odd = sh(r#"read -r l; echo '{"exit": 0, "divergence": 6}'"#);
        odd.call("m", &v);
        assert_eq!(
            odd.take_notes().errors,
            ["oracle: \"divergence\" is not a string"]
        );
        let mut dead = sh("exit 3");
        let resp = dead.call("m", &v);
        let e = oracle_error(&resp).expect("an error");
        // Either the request or the answer fails, depending on when the process exits.
        assert!(
            e.contains("exited without an answer") || e.contains("cannot send a request"),
            "{e}"
        );
        let mut garbage = sh("read -r l; echo nope");
        let e = oracle_error(&garbage.call("m", &v)).expect("an error");
        assert!(e.contains("not a JSON answer"), "{e}");
        let mut missing = Oracle::spawn("/nonexistent/python".into(), vec![], "/".into());
        let e = oracle_error(&missing.call("m", &v)).expect("an error");
        assert!(e.contains("cannot start /nonexistent/python"), "{e}");
    }
}
