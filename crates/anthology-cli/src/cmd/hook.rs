//! `anthology hook session-start`: the SessionStart hook (GC-7). Owner: RT3.
//!
//! As `session_start.py main()`: stdin is read only when it is not a terminal (and must be
//! UTF-8, else it counts as empty, as Python's strict decode), the hook output is one JSON line
//! on stdout, and the exit code is always 0 (GC-5). Extra arguments are ignored, as Python's
//! `main()` ignores `argv`.

use std::ffi::OsString;
use std::io::{IsTerminal, Read};

use anthology_core::Env;
use anthology_runtime::session_start::{self, SessionDeps};

use super::Output;

/// The most stdin bytes read; a larger hook input counts as empty (the hook input is a small
/// JSON object).
const STDIN_MAX: u64 = 8 << 20;

/// Runs `anthology hook …`; `args` are the arguments after `hook`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    match args.first().and_then(|a| a.to_str()) {
        Some("session-start") => {
            let raw = if std::io::stdin().is_terminal() {
                String::new()
            } else {
                read_input(stdin)
            };
            // A caught panic must not print the default hook's report on the hook's stderr.
            std::panic::set_hook(Box::new(|_| {}));
            session_start_with(&Env::from_process(), &raw, &SessionDeps::default())
        }
        Some(other) => Output::usage(&format!("unknown command: hook {other}")),
        None => Output::usage("hook needs one of: session-start"),
    }
}

/// The hook input as text: `""` when it cannot be read, is not UTF-8 or is too large.
pub fn read_input(stdin: &mut dyn Read) -> String {
    let mut buf = Vec::new();
    match stdin.take(STDIN_MAX + 1).read_to_end(&mut buf) {
        Ok(_) if buf.len() as u64 <= STDIN_MAX => String::from_utf8(buf).unwrap_or_default(),
        _ => String::new(),
    }
}

/// `main()` after reading stdin: always exit 0 with one JSON line.
pub fn session_start_with(env: &Env, raw: &str, deps: &SessionDeps<'_>) -> Output {
    main_with(env, raw, deps, &session_start::run)
}

/// The runner `main()` calls (Python's `run`), a parameter so a test can make it fail.
type Runner<'a> = &'a dyn Fn(&Env, &str, &SessionDeps<'_>) -> String;

fn main_with(env: &Env, raw: &str, deps: &SessionDeps<'_>, run: Runner<'_>) -> Output {
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(env, raw, deps)));
    let out = attempt.unwrap_or_else(|_| {
        session_start::system_message_json("anthology: session start did not complete (panic)")
    });
    Output {
        stdout: format!("{out}\n").into_bytes(),
        ..Output::default()
    }
}

#[cfg(test)]
mod tests {
    //! `FailureTest`'s `main()` rows (`tests/test_session_start.py`).
    use super::*;
    use anthology_pycompat::json::{self, Value};
    use anthology_runtime::bootstrap::GuardBinaryStatus;
    use anthology_testkit::session::SessionCase;
    use std::path::Path;

    fn no_ensure(_: &Path, _: u64, _: &Env) -> GuardBinaryStatus {
        GuardBinaryStatus::Present
    }

    fn raising_run(_: &Env, _: &str, _: &SessionDeps<'_>) -> String {
        panic!("boom")
    }

    #[test]
    fn test_main_exits_zero_even_when_run_raises() {
        let c = SessionCase::new();
        let deps = SessionDeps {
            rtk: None,
            ensure_guard_binary: &no_ensure,
            ..SessionDeps::default()
        };
        let raw = read_input(&mut &b"{}"[..]);
        let out = main_with(&c.env, &raw, &deps, &raising_run);
        assert_eq!(out.code, 0);
        assert!(out.stderr.is_empty());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(
            text.ends_with('\n') && text.matches('\n').count() == 1,
            "{text:?}"
        );
        let v = json::loads(&text).unwrap();
        let msg = v
            .as_object()
            .and_then(|o| o.get("systemMessage"))
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(msg, "anthology: session start did not complete (panic)");
    }

    #[test]
    fn test_main_prints_one_json_object_on_success() {
        let c = SessionCase::new();
        let deps = SessionDeps {
            rtk: None,
            ensure_guard_binary: &no_ensure,
            ..SessionDeps::default()
        };
        let input = format!(r#"{{"cwd": "{}"}}"#, c.mem.tmp.display());
        let raw = read_input(&mut input.as_bytes());
        let out = session_start_with(&c.env, &raw, &deps);
        assert_eq!(out.code, 0);
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(
            text.ends_with('\n') && text.matches('\n').count() == 1,
            "{text:?}"
        );
        let v = json::loads(&text).unwrap();
        assert!(v.as_object().unwrap().get("hookSpecificOutput").is_some());
    }

    #[test]
    fn stdin_that_is_not_utf8_or_too_large_reads_as_empty() {
        assert_eq!(read_input(&mut &b"{\"cwd\": \"\xff\"}"[..]), "");
        let big = vec![b' '; (STDIN_MAX + 1) as usize];
        assert_eq!(read_input(&mut &big[..]), "");
        assert_eq!(read_input(&mut &b"{}"[..]), "{}");
    }
}
