//! Fuzz target `shell_segments` (SH1, design §13.5): `anthology_shell::segments` never panics on any text at
//! any depth, every segment's group is listed, and the shell option parsers never panic on its words.
#![no_main]

use anthology_shell::{Limits, script_starts, segments_with, shell_parse};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let (depth, limit, text) = match data.split_first() {
        Some((&first, rest)) => (
            usize::from(first & 0x0f),
            if first & 0x80 != 0 { 0 } else { 1_000_000 },
            String::from_utf8_lossy(rest),
        ),
        None => (0, 1_000_000, String::from_utf8_lossy(data)),
    };
    let lexed = segments_with(&text, depth, &Limits { backtrack_limit: limit });
    for seg in &lexed.segments {
        assert!(lexed.groups.iter().any(|g| g.id == seg.group));
        script_starts(&seg.tokens);
        for prog in ["bash", "fish"] {
            for i in 0..=seg.tokens.len().min(4) {
                assert!(shell_parse(prog, &seg.tokens, i).start <= seg.tokens.len());
            }
        }
    }
});
