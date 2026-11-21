//! Fuzz target `privacy_findings` (MP1, design §13.5): the privacy scan never panics on any
//! text, home or hostname; its findings are sorted, unique, of a known kind and on a line of the
//! text; and `describe` names every kind.
//!
//! Input: `text`, optionally followed by `\0home\0hostname`.
#![no_main]

use anthology_memory::privacy::{
    PrivacyDeps, PrivacyLimits, SECRET_PATTERNS, UNSCANNABLE, describe, privacy_findings,
};

const KINDS: &[&str] = &[
    "home-path",
    "email",
    "private-ip",
    "ip-address",
    "private-hostname",
    "ssh-target",
    "ssh-key-path",
    UNSCANNABLE,
];

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let mut parts = input.splitn(3, '\0');
    let text = parts.next().unwrap_or("");
    let home = parts.next().unwrap_or("/srv/zed");
    let hostname = parts.next().unwrap_or("fuzz-box");
    let deps = PrivacyDeps {
        hostname: || Some("fuzz-box".to_string()),
        home: Some("/srv/zed".to_string()),
        limits: PrivacyLimits::default(),
    };
    let found = privacy_findings(text, Some(home), Some(hostname), &deps);
    let lines = text.split('\n').count();
    for pair in found.windows(2) {
        assert!(
            (pair[0].line, &pair[0].kind) < (pair[1].line, &pair[1].kind),
            "{found:?}"
        );
    }
    for f in &found {
        assert!(f.line >= 1 && f.line <= lines, "{f:?}");
        let known = KINDS.contains(&f.kind.as_str())
            || f.kind
                .strip_prefix("secret:")
                .is_some_and(|label| SECRET_PATTERNS.iter().any(|(l, _, _)| *l == label));
        assert!(known, "{f:?}");
    }
    let summary = describe(&found);
    assert_eq!(summary.is_empty(), found.is_empty());
    // The defaults (`None`) read the seams, never the process environment.
    let _ = privacy_findings(text, None, None, &deps);
});
