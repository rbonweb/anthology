//! The `guard.hook` difftest surface: the hook decision (`anthology_guard::decide`, plain mode) against
//! `guard.py` run as a process by `port/difftest/oracle/guard.py` (plan GD2). Plain mode only: `--shim`
//! is Rust's alone (N2-I1).
//!
//! Case: `{"op": "hook", "stdin_hex": X, "home": H | null}`. Both sides give the exit code, stdout (always
//! empty) and stderr bytes. A null home means `HOME` is unset: Python falls back to `expanduser("~")`,
//! Rust to the passwd home (`paths::home`), the same entry.
//!
//! Seeds: the payloads of `test_guard_cli`, malformed envelopes, and every `guard.check` seed (`check:*`)
//! as a hook payload. Generated: `guard.check`'s generated cases as payloads, plus byte mutations
//! (invalid UTF-8, cut JSON) and odd envelopes.

use anthology_guard::decide;
use anthology_pycompat::json::{self, Map, PY_DEFAULT, Value};

use super::guard_check;
use super::shell_segments::{GenRng, field, field_str, obj, s};
use crate::case::{Case, Rng, Surface};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "guard.hook";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn crate::case::Surface>> {
    Some(Box::new(GuardHook))
}

const HOME: &str = "/home/tester";

struct GuardHook;

impl Surface for GuardHook {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = Vec::new();
        for (k, raw) in fixed_payloads().into_iter().enumerate() {
            out.push(Case::new(
                format!("fixed:{}", k + 1),
                hook_case(&raw, Some(HOME)),
            ));
        }
        let surface = guard_check::surface().expect("guard.check is ported");
        for c in surface.seeds() {
            // the fw:* cases are D-8 on guard.check; here they would be the same divergence again
            if c.id.starts_with("fw:") {
                continue;
            }
            out.push(Case::new(format!("check:{}", c.id), from_check(&c.data)));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        let seed = rng.next_u64();
        let mut g = GenRng::new(seed);
        guard_check::generate(seed ^ 0x5eed, n)
            .into_iter()
            .enumerate()
            .map(|(k, data)| {
                let mut c = from_check(&data);
                if g.chance(10) {
                    c = mutate_bytes(&mut g, &c);
                }
                Case::new(format!("gen:{}", k + 1), c)
            })
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("guard", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run_rust(&c.data)
    }
}

fn run_rust(data: &Value) -> Observed {
    if field_str(data, "op") != "hook" {
        return Observed::text(2, "", "guard.hook: unknown op");
    }
    let raw = unhex(field_str(data, "stdin_hex"));
    let home = match field(data, "home") {
        Some(Value::Str(h)) if !h.is_empty() => h.clone(),
        // `os.environ.get("HOME") or os.path.expanduser("~")` with HOME unset or empty
        _ => anthology_core::paths::home(&anthology_core::Env::default())
            .to_string_lossy()
            .into_owned(),
    };
    let r = decide(&raw, &home, false);
    Observed {
        exit: r.code,
        stdout: Vec::new(),
        stderr: r.stderr.into_bytes(),
        files: None,
    }
}

fn hex(raw: &[u8]) -> String {
    raw.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len() / 2)
        .filter_map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).ok())
        .collect()
}

fn hook_case(raw: &[u8], home: Option<&str>) -> Value {
    obj(vec![
        ("op", s("hook")),
        ("stdin_hex", s(&hex(raw))),
        ("home", home.map_or(Value::Null, s)),
    ])
}

/// A `guard.check` case as the hook payload Claude Code sends (`json.dumps` form).
fn from_check(data: &Value) -> Value {
    let mut m = Map::new();
    m.insert("hook_event_name".into(), s("PreToolUse"));
    m.insert(
        "tool_name".into(),
        field(data, "tool").cloned().unwrap_or(Value::Null),
    );
    m.insert(
        "tool_input".into(),
        field(data, "input").cloned().unwrap_or(Value::Null),
    );
    let raw = json::dumps(&Value::Object(m), &PY_DEFAULT);
    let home = field_str(data, "home");
    hook_case(
        raw.as_bytes(),
        if home.is_empty() { None } else { Some(home) },
    )
}

/// Cut, flip or splice bytes of a payload (invalid UTF-8, broken JSON).
fn mutate_bytes(g: &mut GenRng, case: &Value) -> Value {
    let mut raw = unhex(field_str(case, "stdin_hex"));
    match g.below(4) {
        0 => {
            let at = g.below(raw.len() + 1);
            raw.truncate(at);
        }
        1 => {
            let at = g.below(raw.len() + 1);
            raw.insert(at, *g.pick(&[0xff, 0xfe, 0xc3, 0x80, 0xed, 0x00]));
        }
        2 if !raw.is_empty() => {
            let at = g.below(raw.len());
            raw[at] = *g.pick(&[b'"', b'\\', b'{', b'}', b'[', b',', 0xff]);
        }
        _ => raw = [b"\xef\xbb\xbf".as_slice(), &raw].concat(),
    }
    let home = field(case, "home").and_then(Value::as_str);
    hook_case(&raw, home)
}

/// The payloads of `test_guard_cli` and odd envelopes.
fn fixed_payloads() -> Vec<Vec<u8>> {
    let mut out: Vec<Vec<u8>> = [
        "not json",
        "",
        "   ",
        "{",
        "[",
        "null",
        "[]",
        "[1, 2]",
        "42",
        "\"str\"",
        "true",
        "{}",
        "{\"tool_name\": 5}",
        "{\"tool_name\": null}",
        "{\"tool_name\": [\"Bash\"]}",
        "{\"tool_input\": {\"command\": \"npx playwright install\"}}",
        "{\"tool_name\": \"Bash\"}",
        "{\"tool_name\": \"Bash\", \"tool_input\": \"npx playwright install\"}",
        "{\"tool_name\": \"Bash\", \"tool_input\": null}",
        "{\"tool_name\": \"Bash\", \"tool_input\": []}",
        "{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": 5}}",
        "{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": null}}",
        "{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"npx playwright install\"}}",
        "{\"tool_name\": \"Bash\", \"tool_name\": 5, \"tool_input\": {\"command\": \"npx playwright install\"}}",
        "{\"tool_name\": 5, \"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"npx playwright install\"}}",
        "{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"npx playwright install\", \"x\": NaN}}",
        "{\"tool_name\": \"Read\", \"tool_input\": {\"file_path\": \"~/.config/chromium/x\"}}",
        "  {\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"google-chrome\"}}\n\n",
        "{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"\\ud800npx playwright install\"}}",
    ]
    .iter()
    .map(|t| t.as_bytes().to_vec())
    .collect();
    out.push("[".repeat(100_000).into_bytes());
    out.push(b"\xff\xfe\x00 not utf-8".to_vec());
    out.push(b"{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"ls \xff\"}}".to_vec());
    out.push(
        b"{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"echo \xff; npx playwright install\"}}"
            .to_vec(),
    );
    out.push(b"\xef\xbb\xbf{\"tool_name\": \"Bash\", \"tool_input\": {\"command\": \"npx playwright install\"}}".to_vec());
    out
}
