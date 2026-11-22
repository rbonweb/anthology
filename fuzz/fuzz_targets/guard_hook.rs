//! Fuzz target `guard_hook` (GD2, design §13.5): the hook decision on any stdin never panics (a panic
//! would be caught and turned into the GC-14 panic line, which this target treats as a bug), writes
//! nothing but one line or the shim token, and exits 0 or 2.
#![no_main]

use anthology_guard::{ALLOW_TOKEN, PANIC_LINE, decide};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let shim = data.first().is_some_and(|b| b & 0x80 != 0);
    let r = decide(data, "/home/tester", shim);
    assert_ne!(r.stderr, PANIC_LINE, "the check panicked");
    match r.code {
        0 if shim => assert_eq!(r.stderr, format!("{ALLOW_TOKEN}\n")),
        0 => assert!(r.stderr.is_empty()),
        2 => {
            assert!(r.stderr.starts_with("Blocked by anthology-guardrails ("));
            assert!(r.stderr.ends_with('\n') && r.stderr.matches('\n').count() == 1);
        }
        other => panic!("exit {other}"),
    }
});
