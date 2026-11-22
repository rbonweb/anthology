//! Fuzz target `shell_readonly` (P3): `anthology_shell::classify_readonly` never panics on any command and
//! home, and what it proves holds no control character, no `..` path component and only `cd` targets that
//! `literal_cd_target` accepts.
#![no_main]

use anthology_shell::{ReadOnly, classify_readonly, literal_cd_target};

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let (home, command) = match text.split_once('\u{1}') {
        Some((home, command)) => (home, command),
        None => ("/home/tester", text.as_ref()),
    };
    if let ReadOnly::Proven { paths, cd } = classify_readonly(command, home) {
        assert!(!command.contains(['\n', '\r', '\0']));
        for path in paths.iter().chain(&cd) {
            assert!(!path.split('/').any(|component| component == ".."));
        }
        for target in &cd {
            assert_eq!(literal_cd_target(target), Some(target.as_str()));
        }
    }
});
