//! `core.state` and `core.lock` against their oracles (plan F3).
//!
//! `golden/state.jsonl` is written by `port/difftest/oracle/core_state.py --golden` and
//! `golden/lock.jsonl` by `port/difftest/oracle/core_lock.py --golden`, in the pinned CPython
//! 3.12 image (CI regenerates both and compares them byte for byte). The `rtk--v` lock lines
//! and every state line are CPython's output; the `anthology-bin--v` lock lines come from the
//! oracle's hand-written GC-17 reference (there is no Python for anthology.lock) and say so.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;

use anthology_core::Env;
use anthology_core::lock::{self, ANTHOLOGY_TAG_PREFIX, RTK_TAG_PREFIX};
use anthology_core::platform::Target;
use anthology_core::state;
use anthology_pycompat::json::{self, PY_DEFAULT, Value};

fn golden(name: &str) -> Vec<Value> {
    let path = format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).expect("the golden corpus");
    text.lines()
        .enumerate()
        .map(|(i, line)| json::loads(line).unwrap_or_else(|e| panic!("{name}:{}: {e:?}", i + 1)))
        .collect()
}

fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.as_object()
        .and_then(|o| o.get(k))
        .unwrap_or_else(|| panic!("no {k:?}"))
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    field(v, k)
        .as_str()
        .unwrap_or_else(|| panic!("{k:?} is not a string"))
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn short(s: &str) -> String {
    let cut: String = s.chars().take(300).collect();
    if cut.len() < s.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// Replays `lines` with `run`, which returns `(exit, stdout, stderr)`; returns the tags seen.
fn replay(
    name: &str,
    lines: &[Value],
    run: impl Fn(&Value) -> (i64, String, String),
) -> BTreeMap<String, usize> {
    let mut failures = Vec::new();
    let mut seen = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        let case = field(line, "case");
        let want = field(line, "result");
        let want = (
            field(want, "exit").as_i64().expect("exit"),
            text(want, "stdout").to_string(),
            text(want, "stderr").to_string(),
        );
        let got = run(case);
        if got != want {
            failures.push(format!(
                "{name}:{}: {}\n  want {:?}\n  got  {:?}",
                i + 1,
                short(&json::dumps(case, &PY_DEFAULT)),
                (want.0, short(&want.1), short(&want.2)),
                (got.0, short(&got.1), short(&got.2)),
            ));
        }
        let result = field(line, "result");
        *seen.entry(format!("exit {}", want.0)).or_default() += 1;
        if let Some(d) = result.as_object().and_then(|o| o.get("divergence")) {
            *seen
                .entry(format!("divergence {}", d.as_str().unwrap_or("?")))
                .or_default() += 1;
        }
        let stderr = &want.2;
        if let Some((_, rest)) = stderr.split_once(": ") {
            let what = rest.split(' ').take(2).collect::<Vec<_>>().join(" ");
            *seen.entry(format!("error {what}")).or_default() += 1;
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} {name} cases differ:\n{}",
        failures.len(),
        lines.len(),
        failures
            .iter()
            .take(12)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    seen
}

fn run_state(case: &Value) -> (i64, String, String) {
    assert_eq!(text(case, "op"), "state_write");
    let dir = tempfile::tempdir().expect("tempdir");
    let env = Env::default()
        .with("HOME", "/nonexistent")
        .with_os("CLAUDE_CONFIG_DIR", dir.path());
    let home = dir.path().join("anthology");
    if let Some(prior) = field(case, "prior").as_str() {
        std::fs::create_dir(&home).expect("mkdir");
        std::fs::write(home.join("state.json"), unhex(prior)).expect("prior");
    }
    let kv: Vec<(String, Value)> = field(case, "kv")
        .as_array()
        .expect("kv")
        .iter()
        .map(|pair| {
            let pair = pair.as_array().expect("pair");
            (pair[0].as_str().expect("key").to_string(), pair[1].clone())
        })
        .collect();
    if let Err(e) = state::write(&env, kv) {
        let msg = e
            .py_str()
            .replace(&*dir.path().to_string_lossy(), "<config>");
        return (1, String::new(), format!("{}: {msg}", e.type_name()));
    }
    let path = state::path(&env);
    assert_eq!(path, home.join("state.json"));
    let mode = std::fs::metadata(&path).expect("stat").mode() & 0o7777;
    let mut names: Vec<String> = std::fs::read_dir(&home)
        .expect("ls")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let body = std::fs::read_to_string(&path).expect("utf-8 state");
    (
        0,
        format!("{mode:04o} {}\n{body}", names.join(",")),
        String::new(),
    )
}

#[test]
fn state_write_matches_cpython_byte_for_byte() {
    let lines = golden("state.jsonl");
    assert!(lines.len() >= 500, "{} lines", lines.len());
    let seen = replay("state.jsonl", &lines, run_state);
    for tag in ["exit 0", "divergence D-6"] {
        assert!(seen.contains_key(tag), "no {tag}: {seen:?}");
    }
    // The corpus covers absent, invalid, non-object, BOM, non-ASCII and nested priors.
    let priors: Vec<Option<Vec<u8>>> = lines
        .iter()
        .map(|l| field(field(l, "case"), "prior").as_str().map(unhex))
        .collect();
    for want in [
        None,
        Some(b"\xef\xbb\xbf{\"a\": 1}".to_vec()),
        Some(b"{\"a\": \"\xff\"}".to_vec()),
        Some(b"[1, 2]".to_vec()),
        Some(b"{".to_vec()),
    ] {
        assert!(priors.contains(&want), "no prior {want:?}");
    }
}

fn lock_out(l: &lock::Lock) -> String {
    let mut out = format!("{}\n{}\n{}\n", l.version, l.tag, l.repo);
    for t in Target::ALL {
        out.push_str(&format!("{} {}\n", t.triple(), l.sha256[&t]));
    }
    out
}

fn run_lock(case: &Value) -> (i64, String, String) {
    assert_eq!(text(case, "op"), "lock");
    match lock::parse(text(case, "text"), text(case, "prefix")) {
        Ok(l) => (0, lock_out(&l), String::new()),
        Err(msg) => (1, String::new(), format!("InstallError: {msg}")),
    }
}

#[test]
fn lock_parse_matches_validate_lock_and_gc17() {
    let lines = golden("lock.jsonl");
    assert!(lines.len() >= 700, "{} lines", lines.len());
    for line in &lines {
        let prefix = text(field(line, "case"), "prefix");
        let label = line
            .as_object()
            .and_then(|o| o.get("oracle"))
            .and_then(Value::as_str);
        match prefix {
            RTK_TAG_PREFIX => assert_eq!(label, None),
            ANTHOLOGY_TAG_PREFIX => {
                assert_eq!(label, Some("hand-written from GC-17 (no Python)"));
            }
            other => panic!("prefix {other}"),
        }
    }
    let seen = replay("lock.jsonl", &lines, run_lock);
    for tag in [
        "exit 0",
        "divergence D-6",
        "error rtk.lock is",
        "error rtk.lock must",
        "error rtk.lock: invalid",
        "error rtk.lock: tag",
        "error rtk.lock: sha256",
        "error anthology.lock is",
        "error anthology.lock must",
        "error anthology.lock: invalid",
        "error anthology.lock: tag",
        "error anthology.lock: sha256",
        "error anthology.lock: not",
    ] {
        assert!(seen.contains_key(tag), "no {tag}: {seen:?}");
    }
}
