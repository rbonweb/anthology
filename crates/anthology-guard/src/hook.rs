//! `hook()`: the PreToolUse hook entry (GD2): guard.py `main`, failing closed (design §9.5, F-10).
//!
//! - **Envelope** (GC-6): stdin is decoded as lossy UTF-8 and parsed with `pycompat::json`. Not JSON,
//!   not an object, or a `tool_name` that is not a string → allow (exit 0), as guard.py does.
//! - **Verdict**: a block writes `Blocked by anthology-guardrails ({rule}): {message}\n` and exits 2.
//!   [`Verdict::Incomplete`] writes the GC-14 limit line, [`Verdict::TooComplex`] the `(G0)` line.
//! - **Panics** (m-5): [`hook`] installs a silent panic hook, so nothing but our line reaches stderr, and
//!   runs the check under `catch_unwind`: a panic gives the GC-14 panic line and exit 2.
//! - **Watchdog** (m-R6): [`hook`] starts a thread before it reads stdin. After 8 s (a short time with
//!   the `test-watchdog` feature) it writes the GC-14 limit line and exits the process with 2, unless
//!   the hook has finished first.
//! - **Shim mode** (N2-I1, GC-17): an allow also writes exactly [`ALLOW_TOKEN`] and `\n` to stderr;
//!   everything else is the plain mode's output, byte for byte.
//!
//! Stdout is always empty.

use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anthology_pycompat::json::{self, Value};
use anthology_shell::Limits;

use crate::check::{TOO_COMPLEX, TOO_COMPLEX_ID, Verdict, check_with};

/// The shim's allow token (GC-17), written with one `\n` on an allow in `--shim` mode only.
pub const ALLOW_TOKEN: &str = "anthology-guard:allow:v1";

/// GC-14, guard binary, panic (with its newline).
pub const PANIC_LINE: &str = "Blocked by anthology-guardrails (internal): the guard failed on this input; \
     it was blocked to be safe. Report it, and disable anthology-guardrails with /plugin until a fixed \
     version ships if this keeps happening\n";

/// GC-14, guard binary, limit or watchdog (with its newline).
pub const LIMIT_LINE: &str = "Blocked by anthology-guardrails (internal): the guard could not finish \
     analysing this input (time or pattern limit); it was blocked to be safe. Rephrase the command; if \
     this keeps happening, disable anthology-guardrails with /plugin\n";

/// The hook's watchdog (GC-13: 8 s; the `test-watchdog` feature makes it short so a test can see it).
pub const WATCHDOG: Duration = if cfg!(feature = "test-watchdog") {
    Duration::from_secs(2)
} else {
    Duration::from_secs(8)
};

/// With the `panic-on-marker` feature, a hook input holding this text panics inside the check (m-5).
#[cfg(feature = "panic-on-marker")]
pub const PANIC_MARKER: &str = "anthology-guard-test-panic-marker";

/// What the hook decided: the exit code and the exact stderr (stdout is always empty).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HookResult {
    pub code: i32,
    pub stderr: String,
}

impl HookResult {
    fn allow(shim: bool) -> HookResult {
        HookResult {
            code: 0,
            stderr: if shim {
                format!("{ALLOW_TOKEN}\n")
            } else {
                String::new()
            },
        }
    }

    fn block(stderr: String) -> HookResult {
        HookResult { code: 2, stderr }
    }
}

/// The block line for a verdict, or `None` for an allow.
pub fn block_line(verdict: Verdict) -> Option<String> {
    match verdict {
        Verdict::Allow => None,
        Verdict::Block(rule) => Some(format!(
            "Blocked by anthology-guardrails ({}): {}\n",
            rule.id(),
            rule.message()
        )),
        Verdict::Incomplete => Some(LIMIT_LINE.to_string()),
        Verdict::TooComplex => Some(format!(
            "Blocked by anthology-guardrails ({TOO_COMPLEX_ID}): {TOO_COMPLEX}\n"
        )),
    }
}

/// The hook for one raw input, without the process-wide parts (the panic hook, the watchdog): the
/// envelope, the check under `catch_unwind`, the output. The difftest and the fuzz target call this.
pub fn decide(raw: &[u8], home: &str, shim: bool) -> HookResult {
    decide_with(raw, home, shim, &Limits::default())
}

/// [`decide`] with explicit limits (a test forces a backtrack limit, GC-19).
pub fn decide_with(raw: &[u8], home: &str, shim: bool, limits: &Limits) -> HookResult {
    let verdict = catch_unwind(AssertUnwindSafe(|| {
        let Ok(data) = json::loads_bytes_lossy(raw) else {
            return Verdict::Allow;
        };
        let Some(map) = data.as_object() else {
            return Verdict::Allow;
        };
        let Some(tool_name @ Value::Str(_)) = map.get("tool_name") else {
            return Verdict::Allow;
        };
        #[cfg(feature = "panic-on-marker")]
        if String::from_utf8_lossy(raw).contains(PANIC_MARKER) {
            panic!("panic-on-marker");
        }
        let null = Value::Null;
        let tool_input = map.get("tool_input").unwrap_or(&null);
        check_with(tool_name, tool_input, home, limits)
    }));
    match verdict {
        Ok(v) => match block_line(v) {
            None => HookResult::allow(shim),
            Some(line) => HookResult::block(line),
        },
        Err(_) => HookResult::block(PANIC_LINE.to_string()),
    }
}

/// Shared between the hook and its watchdog: set once the hook's result is final.
struct Done {
    finished: Mutex<bool>,
    wake: Condvar,
}

/// Starts the watchdog: after `limit`, unless `done` is set, it writes the limit line and exits 2.
/// It holds the lock while it exits, so a hook finishing at the same moment cannot write a second line.
fn start_watchdog(done: Arc<Done>, limit: Duration) {
    let spawned = std::thread::Builder::new()
        .name("anthology-guard-watchdog".into())
        .spawn(move || {
            let deadline = Instant::now() + limit;
            let Ok(mut finished) = done.finished.lock() else {
                return;
            };
            while !*finished {
                let now = Instant::now();
                if now >= deadline {
                    let mut err = std::io::stderr().lock();
                    let _ = err.write_all(LIMIT_LINE.as_bytes());
                    let _ = err.flush();
                    std::process::exit(2);
                }
                finished = match done.wake.wait_timeout(finished, deadline - now) {
                    Ok((guard, _)) => guard,
                    Err(_) => return,
                };
            }
        });
    // A thread that cannot start leaves no watchdog; the check itself is bounded (anthology-shell's work
    // budget and the backtrack limit), so the hook still finishes.
    drop(spawned);
}

/// The PreToolUse hook (plan §3.5): the watchdog first, then stdin, then the check.
pub fn hook(stdin: &mut dyn Read, home: &str, shim: bool) -> HookResult {
    hook_with(stdin, home, shim, &Limits::default(), WATCHDOG)
}

/// [`hook`] with explicit limits and watchdog time (tests).
pub fn hook_with(
    stdin: &mut dyn Read,
    home: &str,
    shim: bool,
    limits: &Limits,
    watchdog: Duration,
) -> HookResult {
    let done = Arc::new(Done {
        finished: Mutex::new(false),
        wake: Condvar::new(),
    });
    start_watchdog(Arc::clone(&done), watchdog);
    std::panic::set_hook(Box::new(|_| {}));
    let mut raw = Vec::new();
    let result = match stdin.read_to_end(&mut raw) {
        Ok(_) => decide_with(&raw, home, shim, limits),
        // Python's read raising meant "allow"; Rust cannot see the call, so it blocks (fail closed).
        Err(_) => HookResult::block(PANIC_LINE.to_string()),
    };
    if let Ok(mut finished) = done.finished.lock() {
        *finished = true;
        done.wake.notify_all();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/tester";

    fn payload(tool: &str, input: &str) -> Vec<u8> {
        format!("{{\"tool_name\": \"{tool}\", \"tool_input\": {input}}}").into_bytes()
    }

    #[test]
    fn allow_and_block_in_both_modes() {
        let allow = payload("Bash", r#"{"command": "git status"}"#);
        assert_eq!(decide(&allow, HOME, false), HookResult { code: 0, stderr: String::new() });
        assert_eq!(
            decide(&allow, HOME, true),
            HookResult { code: 0, stderr: "anthology-guard:allow:v1\n".into() }
        );
        let block = payload("Bash", r#"{"command": "npx playwright install"}"#);
        let plain = decide(&block, HOME, false);
        assert_eq!(plain.code, 2);
        assert_eq!(
            plain.stderr,
            "Blocked by anthology-guardrails (G1): run it inside Docker (OrbStack); for browser work \
             use BrowserOS Neo\n"
        );
        assert_eq!(decide(&block, HOME, true), plain);
    }

    #[test]
    fn malformed_envelopes_allow() {
        for raw in [&b"not json"[..], b"", b"[]", b"{\"tool_name\": 5}", b"\xff\xfe"] {
            assert_eq!(decide(raw, HOME, false).code, 0);
            assert_eq!(decide(raw, HOME, true).stderr, "anthology-guard:allow:v1\n");
        }
    }

    #[test]
    fn a_forced_regex_limit_gives_the_limit_line() {
        let raw = payload("Bash", r#"{"command": "cat $HOME/x"}"#);
        let low = Limits { backtrack_limit: 0 };
        let r = decide_with(&raw, HOME, true, &low);
        assert_eq!(r, HookResult { code: 2, stderr: LIMIT_LINE.into() });
    }

    #[test]
    fn the_lines_are_gc_14() {
        assert!(LIMIT_LINE.ends_with("; if this keeps happening, disable anthology-guardrails with /plugin\n"));
        assert_eq!(LIMIT_LINE.matches('\n').count(), 1);
        assert_eq!(PANIC_LINE.matches('\n').count(), 1);
        assert!(PANIC_LINE.contains("(internal): the guard failed on this input; it was blocked"));
        let too = block_line(Verdict::TooComplex).unwrap();
        assert!(too.starts_with("Blocked by anthology-guardrails (G0): this command is too complex"));
    }
}
