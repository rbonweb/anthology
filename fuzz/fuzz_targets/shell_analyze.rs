//! Fuzz target `shell_analyze` (SH2, design §13.5): `anthology_shell::analyze` never panics on any command
//! and home, and every entry's group is listed in the analysis.
#![no_main]

use anthology_shell::{Limits, analyze_with};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (home, command) = match text.split_once('\u{1}') {
        Some((home, command)) => (home, command),
        None => ("/home/tester", text.as_ref()),
    };
    let limit = if data.first().is_some_and(|b| b & 0x80 != 0) {
        0
    } else {
        1_000_000
    };
    let analysis = analyze_with(
        command,
        home,
        &Limits {
            backtrack_limit: limit,
        },
    );
    for entry in &analysis.entries {
        assert!(analysis.groups.iter().any(|g| g.id == entry.group));
    }
});
