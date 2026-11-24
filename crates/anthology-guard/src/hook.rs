//! `hook()`: the PreToolUse hook entry (GD2): guard.py `main`, failing closed (design §9.5, F-10).
//!
//! - **Envelope** (GC-6): stdin is decoded as lossy UTF-8 and parsed with `pycompat::json`. Not JSON,
//!   not an object, or a `tool_name` that is not a string → allow (exit 0), as guard.py does.
//!   An integer past Python's 4300-digit limit is not malformed JSON but an input too large to
//!   read: guard.py's `ValueError` allowed it; Rust blocks it as too complex (G0; owner
//!   direction, fail closed, D-8).
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
use anthology_shell::{Limits, Opaque};

use crate::check::{TOO_COMPLEX, TOO_COMPLEX_EMBEDDED, TOO_COMPLEX_ID, Verdict, check_explained};

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
    block_line_for(verdict, false)
}

/// [`block_line`] for a host that may have no `/plugin` command (`embedded`, [`crate::host::embedded`]):
/// the `(G0)` line then has no `/plugin disable` parenthetical. Every other line is the same.
pub fn block_line_for(verdict: Verdict, embedded: bool) -> Option<String> {
    match verdict {
        Verdict::Allow => None,
        Verdict::Block(rule) => Some(format!(
            "Blocked by anthology-guardrails ({}): {}\n",
            rule.id(),
            rule.message()
        )),
        Verdict::Incomplete => Some(LIMIT_LINE.to_string()),
        Verdict::TooComplex => {
            let message = if embedded {
                TOO_COMPLEX_EMBEDDED
            } else {
                TOO_COMPLEX
            };
            Some(format!(
                "Blocked by anthology-guardrails ({TOO_COMPLEX_ID}): {message}\n"
            ))
        }
    }
}

/// The hook for one raw input, without the process-wide parts (the panic hook, the watchdog): the
/// envelope, the check under `catch_unwind`, the output. The difftest and the fuzz target call this.
pub fn decide(raw: &[u8], home: &str, shim: bool) -> HookResult {
    decide_with(raw, home, shim, &Limits::default())
}

/// [`decide`] with explicit limits (a test forces a backtrack limit, GC-19).
pub fn decide_with(raw: &[u8], home: &str, shim: bool, limits: &Limits) -> HookResult {
    decide_for(raw, home, shim, limits, false)
}

/// [`decide_with`] for a host: `embedded` ([`crate::host::embedded`]) drops the `/plugin disable`
/// parenthetical from the `(G0)` line. The verdict, the exit code and the rule are the same either way.
pub fn decide_for(
    raw: &[u8],
    home: &str,
    shim: bool,
    limits: &Limits,
    embedded: bool,
) -> HookResult {
    let verdict = catch_unwind(AssertUnwindSafe(|| {
        let data = match json::loads_bytes_lossy(raw) {
            Ok(data) => data,
            // `ValueError`: a number past the integer-digit limit
            Err(e) if e.type_name() == "ValueError" => {
                return (Verdict::TooComplex, String::new());
            }
            Err(_) => return (Verdict::Allow, String::new()),
        };
        let Some(map) = data.as_object() else {
            return (Verdict::Allow, String::new());
        };
        let Some(tool_name @ Value::Str(_)) = map.get("tool_name") else {
            return (Verdict::Allow, String::new());
        };
        #[cfg(feature = "panic-on-marker")]
        if String::from_utf8_lossy(raw).contains(PANIC_MARKER) {
            panic!("panic-on-marker");
        }
        let null = Value::Null;
        let tool_input = map.get("tool_input").unwrap_or(&null);
        let (verdict, cause) = check_explained(tool_name, tool_input, home, limits);
        let command = tool_input
            .as_object()
            .and_then(|o| o.get("command"))
            .and_then(Value::as_str)
            .filter(|_| verdict == Verdict::TooComplex);
        (verdict, hints_for(command, cause))
    }));
    match verdict {
        Ok((v, hints)) => match block_line_for(v, embedded) {
            None => HookResult::allow(shim),
            Some(line) if !hints.is_empty() => {
                HookResult::block(format!("{}{hints}\n", line.trim_end()))
            }
            Some(line) => HookResult::block(line),
        },
        Err(_) => HookResult::block(PANIC_LINE.to_string()),
    }
}

/// The hints for a `(G0)` line: what the text of `command` shows (a double-quoted script with an
/// expansion, a lone backtick) and what anthology-shell marked (`cause`, [`check_explained`]). Each hint
/// comes at most once, however many ways it was found. Hints only: they change no verdict.
fn hints_for(command: Option<&str>, cause: Option<Opaque>) -> String {
    let mut hints: Vec<&str> = Vec::new();
    let mut add = |hint: &'static str| {
        if !hints.contains(&hint) {
            hints.push(hint);
        }
    };
    if command.is_some_and(quoted_script_expansion) {
        add(QUOTED_SCRIPT_HINT);
    }
    if command.is_some_and(lone_backtick_in_double_quotes) {
        add(LONE_BACKTICK_HINT);
    }
    match cause {
        Some(Opaque::DynamicCommandWord) => add(PROGRAM_HINT),
        Some(Opaque::Eval | Opaque::ShellScriptFromExpansion) => add(QUOTED_SCRIPT_HINT),
        Some(Opaque::Source) => add(SOURCE_HINT),
        Some(Opaque::StdinShellUnreadable) => add(STDIN_SHELL_HINT),
        _ => {}
    }
    hints.concat()
}

/// Added to the `(G0)` line when the program (or a command) is only known at run time: it comes from a
/// variable, a glob, or another command's output (`xargs`, a substitution).
pub const PROGRAM_HINT: &str = ". The program comes from a variable, a glob or another command's \
     output: write its path literally";

/// Added to the `(G0)` line when a shell reads a script that the guard cannot see (a heredoc, a pipe).
pub const STDIN_SHELL_HINT: &str = ". A shell that reads its script from a heredoc or pipe cannot be \
     checked: run the commands directly, one per call";

/// Added to the `(G0)` line when a command runs a file the guard cannot read (`source`, `. file`, or a
/// shell given a script file).
pub const SOURCE_HINT: &str =
    ". A sourced file or script cannot be read: run its commands directly";

/// Carve-out (c): added to the `(G0)` line when the command holds a double-quoted script with an expansion
/// in it, which the outer shell fills in before the script runs, so the guard cannot read it.
pub const QUOTED_SCRIPT_HINT: &str = ". A `$` inside a double-quoted script (`sh -c \"...\"`, `eval \
     \"...\"`, `trap \"...\"`) is filled in before the guard can read the script: put the script in single \
     quotes and pass values as arguments (`sh -c '... \"$1\" ...' _ \"$x\"`)";

/// Carve-out (c): `command` holds, outside quotes, `-c "`, `eval "` or `trap "` whose double-quoted text holds
/// an unescaped `$` followed by a name, a digit, `{`, `(`, `@` or `*`. A hint only: it changes no verdict.
fn quoted_script_expansion(command: &str) -> bool {
    const OPENERS: [&[u8]; 3] = [b"-c \"", b"eval \"", b"trap \""];
    let b = command.as_bytes();
    // the index after the double-quoted text starting at `from`, and whether it holds an expansion
    let quoted = |from: usize| {
        let mut j = from;
        let mut expands = false;
        while j < b.len() && b[j] != b'"' {
            if b[j] == b'\\' {
                j += 2;
                continue;
            }
            expands |= b[j] == b'$'
                && b.get(j + 1).is_some_and(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'{' | b'(' | b'@' | b'*')
                });
            j += 1;
        }
        (j + 1, expands)
    };
    let mut k = 0;
    while k < b.len() {
        match b[k] {
            b'\\' => k += 2,
            b'\'' => {
                k = (k + 1..b.len())
                    .find(|&j| b[j] == b'\'')
                    .map_or(b.len(), |j| j + 1);
            }
            b'"' => k = quoted(k + 1).0,
            _ => match OPENERS.iter().find(|o| b[k..].starts_with(o)) {
                Some(opener) => {
                    let (end, expands) = quoted(k + opener.len());
                    if expands {
                        return true;
                    }
                    k = end;
                }
                None => k += 1,
            },
        }
    }
    false
}

/// Added to the `(G0)` line when a double-quoted word holds a backtick that is not closed (a grep
/// pattern in double quotes that ends in a backtick): the backtick starts a command substitution, so
/// the guard cannot read the word.
pub const LONE_BACKTICK_HINT: &str =
    ". A backtick inside double quotes runs a command; put the pattern in single quotes";

/// `command` holds a double-quoted text with an odd number of backticks that a backslash does not
/// escape, so one of them is unmatched. A hint only: it changes no verdict, and a command that is
/// too complex for another reason may get it when the text is odd (an accepted false hint).
fn lone_backtick_in_double_quotes(command: &str) -> bool {
    let b = command.as_bytes();
    let mut k = 0;
    while k < b.len() {
        match b[k] {
            b'\\' => k += 2,
            b'\'' => {
                k = (k + 1..b.len())
                    .find(|&j| b[j] == b'\'')
                    .map_or(b.len(), |j| j + 1);
            }
            b'"' => {
                let mut ticks = 0;
                k += 1;
                while k < b.len() && b[k] != b'"' {
                    match b[k] {
                        b'\\' => k += 1,
                        b'`' => ticks += 1,
                        _ => {}
                    }
                    k += 1;
                }
                if ticks % 2 == 1 {
                    return true;
                }
                k += 1;
            }
            _ => k += 1,
        }
    }
    false
}

/// Shared between the hook and its watchdog: set once the hook's result is final.
struct Done {
    finished: Mutex<bool>,
    wake: Condvar,
}

/// Starts the watchdog: after `limit`, unless `done` is set, it writes the limit line and exits 2.
/// It holds the lock while it exits, so a hook finishing at the same moment cannot write a second line.
fn start_watchdog(done: Arc<Done>, limit: Duration) {
    // The deadline counts from the call, not from when the thread first runs.
    let deadline = Instant::now() + limit;
    let spawned = std::thread::Builder::new()
        .name("anthology-guard-watchdog".into())
        .spawn(move || {
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
    run_hook(stdin, || home.to_string(), shim, false, limits, watchdog)
}

/// [`hook`] for the CLI: `home` is resolved after the watchdog has started, so a slow passwd
/// lookup (HOME unset) is under the watchdog too. `embedded` is [`crate::host::embedded`].
pub fn hook_resolving_home(
    stdin: &mut dyn Read,
    home: impl FnOnce() -> String,
    shim: bool,
    embedded: bool,
) -> HookResult {
    run_hook(stdin, home, shim, embedded, &Limits::default(), WATCHDOG)
}

fn run_hook(
    stdin: &mut dyn Read,
    home: impl FnOnce() -> String,
    shim: bool,
    embedded: bool,
    limits: &Limits,
    watchdog: Duration,
) -> HookResult {
    let done = Arc::new(Done {
        finished: Mutex::new(false),
        wake: Condvar::new(),
    });
    start_watchdog(Arc::clone(&done), watchdog);
    std::panic::set_hook(Box::new(|_| {}));
    let home = home();
    let mut raw = Vec::new();
    let result = match stdin.read_to_end(&mut raw) {
        Ok(_) => decide_for(&raw, &home, shim, limits, embedded),
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
        assert_eq!(
            decide(&allow, HOME, false),
            HookResult {
                code: 0,
                stderr: String::new()
            }
        );
        assert_eq!(
            decide(&allow, HOME, true),
            HookResult {
                code: 0,
                stderr: "anthology-guard:allow:v1\n".into()
            }
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
        for raw in [
            &b"not json"[..],
            b"",
            b"[]",
            b"{\"tool_name\": 5}",
            b"\xff\xfe",
        ] {
            assert_eq!(decide(raw, HOME, false).code, 0);
            assert_eq!(decide(raw, HOME, true).stderr, "anthology-guard:allow:v1\n");
        }
    }

    #[test]
    fn an_integer_past_the_digit_limit_blocks_as_too_complex() {
        let too_complex = block_line(Verdict::TooComplex).unwrap();
        for digits in [4301, 100_000] {
            let raw = payload(
                "Bash",
                &format!(r#"{{"command": "ls", "n": {}}}"#, "9".repeat(digits)),
            );
            for shim in [false, true] {
                assert_eq!(
                    decide(&raw, HOME, shim),
                    HookResult {
                        code: 2,
                        stderr: too_complex.clone()
                    }
                );
            }
        }
        // at the limit, and as a float, it is an ordinary number
        let raw = payload(
            "Bash",
            &format!(r#"{{"command": "ls", "n": {}}}"#, "9".repeat(4300)),
        );
        assert_eq!(decide(&raw, HOME, false).code, 0);
        let raw = payload(
            "Bash",
            &format!(r#"{{"command": "ls", "n": {}.5}}"#, "9".repeat(5000)),
        );
        assert_eq!(decide(&raw, HOME, false).code, 0);
        assert!(too_complex.contains("/plugin disable anthology-guardrails@anthology"));
    }

    #[test]
    fn a_forced_regex_limit_gives_the_limit_line() {
        let raw = payload("Bash", r#"{"command": "cat $HOME/x"}"#);
        let low = Limits { backtrack_limit: 0 };
        let r = decide_with(&raw, HOME, true, &low);
        assert_eq!(
            r,
            HookResult {
                code: 2,
                stderr: LIMIT_LINE.into()
            }
        );
    }

    #[test]
    fn the_lines_are_gc_14_byte_for_byte() {
        let tail = "; if this keeps happening, disable anthology-guardrails with /plugin";
        assert_eq!(
            LIMIT_LINE,
            format!(
                "Blocked by anthology-guardrails (internal): the guard could not finish analysing \
                 this input (time or pattern limit); it was blocked to be safe. Rephrase the \
                 command{tail}\n"
            )
        );
        assert_eq!(
            PANIC_LINE,
            "Blocked by anthology-guardrails (internal): the guard failed on this input; it was \
             blocked to be safe. Report it, and disable anthology-guardrails with /plugin until a \
             fixed version ships if this keeps happening\n"
        );
        assert_eq!(ALLOW_TOKEN, "anthology-guard:allow:v1");
    }

    #[test]
    fn a_hint_comes_once_and_only_for_a_named_cause() {
        let quoted = r#"sh -c "echo $x""#;
        for cause in [Opaque::Eval, Opaque::ShellScriptFromExpansion] {
            // the text and the cause both call for it: one copy; the cause alone: one copy
            assert_eq!(hints_for(Some(quoted), Some(cause)), QUOTED_SCRIPT_HINT);
            assert_eq!(hints_for(Some("bash x"), Some(cause)), QUOTED_SCRIPT_HINT);
        }
        assert_eq!(hints_for(Some(quoted), None), QUOTED_SCRIPT_HINT);
        assert_eq!(
            hints_for(Some(quoted), Some(Opaque::DynamicCommandWord)),
            format!("{QUOTED_SCRIPT_HINT}{PROGRAM_HINT}")
        );
        assert_eq!(hints_for(None, Some(Opaque::Source)), SOURCE_HINT);
        assert_eq!(
            hints_for(None, Some(Opaque::StdinShellUnreadable)),
            STDIN_SHELL_HINT
        );
        // the other reasons name nothing, and no command and no cause names nothing
        for cause in [
            Opaque::DepthCap,
            Opaque::InterpreterCode,
            Opaque::UnknownWrapperOption,
            Opaque::Unbalanced,
            Opaque::RegexLimit,
        ] {
            assert_eq!(hints_for(Some("ls"), Some(cause)), "");
        }
        assert_eq!(hints_for(None, None), "");
    }

    #[test]
    fn the_lines_are_gc_14() {
        assert!(
            LIMIT_LINE.ends_with(
                "; if this keeps happening, disable anthology-guardrails with /plugin\n"
            )
        );
        assert_eq!(LIMIT_LINE.matches('\n').count(), 1);
        assert_eq!(PANIC_LINE.matches('\n').count(), 1);
        assert!(PANIC_LINE.contains("(internal): the guard failed on this input; it was blocked"));
        let too = block_line(Verdict::TooComplex).unwrap();
        assert!(
            too.starts_with("Blocked by anthology-guardrails (G0): this command is too complex")
        );
    }
}
