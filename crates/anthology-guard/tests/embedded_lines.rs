//! The GC-14 lines (the limit line and the panic line) in an embedded host (`host::embedded`): a host has
//! no `/plugin` command, so the lines leave it out, as the `(G0)` line does. Text only: the verdict
//! (a block) and the exit code (2) are the same, and so is every line outside a host.

use std::io::{Error, ErrorKind, Read};

use anthology_guard::{
    HookResult, LIMIT_LINE, LIMIT_LINE_EMBEDDED, PANIC_LINE, PANIC_LINE_EMBEDDED, Verdict,
    block_line_for, decide_for, hook_resolving,
};
use anthology_shell::Limits;

const HOME: &str = "/home/tester";

fn payload(command: &str) -> Vec<u8> {
    format!("{{\"tool_name\": \"Bash\", \"tool_input\": {{\"command\": \"{command}\"}}}}")
        .into_bytes()
}

#[test]
fn the_embedded_lines_are_the_lines_without_the_plugin_text() {
    assert_eq!(
        LIMIT_LINE_EMBEDDED,
        "Blocked by anthology-guardrails (internal): the guard could not finish analyzing this \
         input (time or pattern limit); it was blocked to be safe. Rephrase the command\n"
    );
    assert_eq!(
        PANIC_LINE_EMBEDDED,
        "Blocked by anthology-guardrails (internal): the guard failed on this input; it was \
         blocked to be safe. Report it if this keeps happening\n"
    );
    for line in [LIMIT_LINE_EMBEDDED, PANIC_LINE_EMBEDDED] {
        assert!(!line.contains("/plugin"), "{line}");
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.ends_with('\n'));
    }
    // the lines outside a host keep their text
    assert!(LIMIT_LINE.contains("/plugin"));
    assert!(PANIC_LINE.contains("/plugin"));
    assert_eq!(
        block_line_for(Verdict::Incomplete, true).as_deref(),
        Some(LIMIT_LINE_EMBEDDED)
    );
    assert_eq!(
        block_line_for(Verdict::Incomplete, false).as_deref(),
        Some(LIMIT_LINE)
    );
}

#[test]
fn a_limit_in_an_embedded_host_gives_the_embedded_limit_line() {
    let raw = payload("cat $HOME/x");
    let low = Limits { backtrack_limit: 0 };
    for shim in [false, true] {
        assert_eq!(
            decide_for(&raw, HOME, shim, &low, true),
            HookResult {
                code: 2,
                stderr: LIMIT_LINE_EMBEDDED.into()
            }
        );
        assert_eq!(
            decide_for(&raw, HOME, shim, &low, false),
            HookResult {
                code: 2,
                stderr: LIMIT_LINE.into()
            }
        );
    }
}

#[cfg(feature = "panic-on-marker")]
#[test]
fn a_panic_in_an_embedded_host_gives_the_embedded_panic_line() {
    let raw = payload(&format!("ls {}", anthology_guard::PANIC_MARKER));
    let limits = Limits::default();
    for shim in [false, true] {
        assert_eq!(
            decide_for(&raw, HOME, shim, &limits, true),
            HookResult {
                code: 2,
                stderr: PANIC_LINE_EMBEDDED.into()
            }
        );
        assert_eq!(
            decide_for(&raw, HOME, shim, &limits, false),
            HookResult {
                code: 2,
                stderr: PANIC_LINE.into()
            }
        );
    }
}

/// A stdin that cannot be read.
struct Broken;

impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(Error::from(ErrorKind::Other))
    }
}

#[test]
fn an_unreadable_stdin_in_an_embedded_host_gives_the_embedded_panic_line() {
    let setup = || (HOME.to_string(), None);
    for shim in [false, true] {
        assert_eq!(
            hook_resolving(&mut Broken, setup, shim, true),
            HookResult {
                code: 2,
                stderr: PANIC_LINE_EMBEDDED.into()
            }
        );
        assert_eq!(
            hook_resolving(&mut Broken, setup, shim, false),
            HookResult {
                code: 2,
                stderr: PANIC_LINE.into()
            }
        );
    }
}
