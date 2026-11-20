//! Acceptance 6: the `tool_name` key scan (design §9.3) against a Rust reference, on generated
//! payloads, under every shell here (T8).

use std::path::PathBuf;

use anthology_pycompat::json;
use anthology_testkit::bs::{self, Sandbox};
use proptest::prelude::*;

use super::common::*;

const READONLY: &[&str] = &[
    "Read",
    "Grep",
    "Glob",
    "LS",
    "NotebookRead",
    "WebFetch",
    "WebSearch",
    "TodoWrite",
    "TaskCreate",
    "TaskUpdate",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "Task",
    "Agent",
    "Skill",
    "ToolSearch",
    "AskUserQuestion",
    "EnterPlanMode",
    "ExitPlanMode",
    "BashOutput",
    "KillShell",
];

/// The reference: lines joined (CR, LF and NUL become spaces), every `"tool_name"` followed by
/// optional spaces or tabs and `:` counted; allowed only for exactly one, whose value (after
/// spaces or tabs) is a plain quoted string of letters on the allowlist.
fn reference(payload: &[u8]) -> bool {
    let s: Vec<u8> = payload
        .iter()
        .map(|&b| {
            if matches!(b, b'\n' | b'\r' | 0) {
                b' '
            } else {
                b
            }
        })
        .collect();
    let key = b"\"tool_name\"";
    let mut i = 0;
    let mut count = 0;
    let mut value: Option<String> = None;
    while i + key.len() <= s.len() {
        if &s[i..i + key.len()] != key {
            i += 1;
            continue;
        }
        let mut j = i + key.len();
        while j < s.len() && matches!(s[j], b' ' | b'\t') {
            j += 1;
        }
        if j < s.len() && s[j] == b':' {
            count += 1;
            j += 1;
            if count == 1 {
                let mut k = j;
                while k < s.len() && matches!(s[k], b' ' | b'\t') {
                    k += 1;
                }
                if k < s.len() && s[k] == b'"' {
                    let start = k + 1;
                    let mut e = start;
                    while e < s.len() && s[e].is_ascii_alphabetic() {
                        e += 1;
                    }
                    if e > start && e < s.len() && s[e] == b'"' {
                        value = Some(String::from_utf8_lossy(&s[start..e]).into_owned());
                    }
                }
            }
            i = j;
        } else {
            i += key.len();
        }
    }
    count == 1 && value.is_some_and(|v| READONLY.contains(&v.as_str()))
}

/// A sandbox with a harness that sources lib.sh and prints anth_readonly's verdict.
fn harness() -> Sandbox {
    let sb = sandbox();
    let script: PathBuf = sb.root.join("scan.sh");
    std::fs::write(
        &script,
        format!(
            ". '{}'\nif anth_readonly \"$1\"; then echo allow; else echo block; fi\n",
            sb.root.join(G_LIB).display()
        ),
    )
    .unwrap();
    sb
}

/// The shell's verdict under every shell here (they must agree).
fn shell_verdict(payload: &[u8]) -> bool {
    let sb = harness();
    let file = sb.root.join("payload");
    std::fs::write(&file, payload).unwrap();
    let mut verdicts = Vec::new();
    for sh in bs::shells("tool_name scan") {
        let run = sb.run(sh, "scan.sh", &[file.to_str().unwrap()], b"");
        let v = match run.out().as_str() {
            "allow\n" => true,
            "block\n" => false,
            other => panic!("{sh:?}: {other:?} {}", run.err()),
        };
        verdicts.push((sh, v));
    }
    let first = verdicts[0].1;
    assert!(
        verdicts.iter().all(|(_, v)| *v == first),
        "shells disagree: {verdicts:?}"
    );
    first
}

fn ws() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![" ", "\t", "\n", "\r", "\r\n", ""]),
        0..3,
    )
    .prop_map(|v| v.concat())
}

fn name() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(READONLY.to_vec()).prop_map(str::to_string),
        prop::sample::select(vec![
            "Bash",
            "Write",
            "Edit",
            "mcp__x__y",
            "ListMcpResourcesTool",
            "Re\\u0061d",
            "Read\\\"",
            "read",
            "",
            "Read ",
            "ReadX",
            "NotebookEdit",
        ])
        .prop_map(str::to_string),
    ]
}

fn key() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "\"tool_name\"",
        "\"tool_name\"",
        "\"tool_name\"",
        "\"tool\\u005fname\"",
        "\"a\\\"tool_name\"",
        "\"tool_names\"",
        "\"Tool_name\"",
    ])
    .prop_map(str::to_string)
}

/// A value, sometimes a string holding key-like text, sometimes an object with more keys.
fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        name().prop_map(|n| format!("\"{n}\"")),
        Just("1".to_string()),
        Just("null".to_string()),
        Just("\"say \\\"tool_name\\\": \\\"Read\\\"\"".to_string()),
        (key(), ws(), name()).prop_map(|(k, w, n)| format!("{{{k}{w}:{w}\"{n}\"}}")),
        Just("[\"tool_name\", \"Read\"]".to_string()),
    ]
}

fn payload() -> impl Strategy<Value = Vec<u8>> {
    (
        prop::collection::vec((key(), ws(), ws(), value()), 0..4),
        ws(),
        ws(),
        name(),
        prop::bool::ANY,
    )
        .prop_map(|(extra, w1, w2, top, nul)| {
            let mut parts = vec![format!("\"tool_name\"{w1}:{w2}\"{top}\"")];
            for (i, (k, a, b, v)) in extra.into_iter().enumerate() {
                if i % 2 == 0 {
                    parts.push(format!("\"tool_input\"{a}:{b}{{{k}{a}:{b}{v}}}"));
                } else {
                    parts.push(format!("{k}{a}:{b}{v}"));
                }
            }
            let mut text = format!("{{{}}}", parts.join(", "));
            if nul {
                text.push('\0');
            }
            text.into_bytes()
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 160, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn the_tool_name_scan_matches_the_reference(p in payload()) {
        let want = reference(&p);
        prop_assert_eq!(shell_verdict(&p), want, "payload {:?}", String::from_utf8_lossy(&p));
        // Soundness: an allowed payload that is JSON has an allowlisted top-level tool_name.
        // The scan reads raw keys, so this holds for keys written without `\u` escapes, as
        // Claude Code's serializer writes them (design §9.3).
        let escaped = p.windows(2).any(|w| w == b"\\u");
        if want && !escaped && let Ok(v) = json::loads(&String::from_utf8_lossy(&p)) {
            if let Some(obj) = v.as_object() {
                let t = obj.get("tool_name").and_then(|t| t.as_str()).unwrap_or("");
                prop_assert!(READONLY.contains(&t), "allowed {:?}", String::from_utf8_lossy(&p));
            }
        }
    }
}

#[test]
fn the_tool_name_scan_fixed_cases() {
    for (p, want) in [
        (READ, true),
        (BASH, false),
        (MCP, false),
        (NESTED, false),
        (b"{\"tool_name\" \t\n: \r\n\"Grep\"}".as_slice(), true),
        (br#"{"tool_name":"Read","tool_name":"Read"}"#, false),
        (br#"{"tool_name":"Read"}"#, true),
        (br#"{"tool_name":"Read\"x"}"#, false),
        (br#"{"tool_name":"ReadX"}"#, false),
        (
            br#"{"tool_input":{"command":"echo \"tool_name\": \"Bash\""},"tool_name":"Read"}"#,
            true,
        ),
        (br#"{"a\"tool_name":"Read","tool_name":"Bash"}"#, false),
        (br#"{"tool_name":["Read"]}"#, false),
        (b"", false),
        (b"tool_name", false),
    ] {
        assert_eq!(reference(p), want, "{:?}", String::from_utf8_lossy(p));
        assert_eq!(shell_verdict(p), want, "{:?}", String::from_utf8_lossy(p));
    }
}
