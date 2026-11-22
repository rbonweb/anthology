//! The `core.state` difftest surface (F3's oracle `port/difftest/oracle/core_state.py`).
//!
//! Case: `{"op": "state_write", "prior": HEX | null, "kv": [[KEY, VALUE], ...]}`: the bytes of
//! `state.json` before the call, and `state.write(**kv)`. Both sides print
//! `"<mode> <sorted folder entries>\n"` and then the file's text, or exit 1 with
//! `"<type>: <message>"`. Seeds are the committed golden corpus; generated cases are random
//! priors (absent, objects in every `json.dumps` layout, odd and invalid texts, lone surrogate
//! escapes) merged with random key/value sets. The oracle marks D-6 when the prior holds a lone
//! surrogate (`port/difftest/divergences/core.state.toml`).

use std::os::unix::fs::MetadataExt;

use anthology_core::Env;
use anthology_core::state;
use anthology_pycompat::json::{self, DumpOpts, Int, Map, Value};

use crate::case::{Case, Rng, Surface, golden_seeds};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "core.state";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(CoreState))
}

struct CoreState;

impl Surface for CoreState {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        golden_seeds("crates/anthology-core/tests/golden/state.jsonl")
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("core_state", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.as_object()
        .and_then(|o| o.get(k))
        .unwrap_or_else(|| panic!("core.state: no {k:?}"))
}

fn run(case: &Value) -> Observed {
    assert_eq!(field(case, "op").as_str(), Some("state_write"));
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
        return Observed::text(1, "", &format!("{}: {msg}", e.type_name()));
    }
    let path = state::path(&env);
    let mode = std::fs::metadata(&path).expect("stat").mode() & 0o7777;
    let mut names: Vec<String> = std::fs::read_dir(&home)
        .expect("ls")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let body = std::fs::read(&path).expect("state");
    let mut stdout = format!("{mode:04o} {}\n", names.join(",")).into_bytes();
    stdout.extend_from_slice(&body);
    Observed {
        exit: 0,
        stdout,
        stderr: Vec::new(),
        files: None,
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

const KEYS: [&str; 7] = [
    "data_dir",
    "rtk_path",
    "setup_done",
    "settings_ops_version",
    "settings_ops",
    "settings_created",
    "agent_memory_warned",
];

const ODD_PRIORS: &[&[u8]] = &[
    b"",
    b"{",
    b"not json",
    b"\xef\xbb\xbf{\"a\": 1}",
    b"{\"a\": \"\xff\"}",
    b"{\"a\": \"\xc3\"}",
    b"[1, 2]",
    b"\"s\"",
    b"null",
    b"42",
    b"true",
    b"{}",
    b"  {}  \n",
    b"{\r\n  \"a\" : 1 \r\n}\r\n",
    b"{\"a\": 1, \"a\": 2, \"b\": {\"c\": 1, \"c\": [3]}}",
    b"{\"f\": 1e16, \"g\": -0.0, \"h\": 5e-324, \"i\": NaN, \"j\": Infinity, \"l\": 1E400}",
    b"{\"big\": 123456789012345678901234567890, \"neg\": -9223372036854775809}",
];

fn rand_key(rng: &mut Rng) -> String {
    if rng.chance(0.55) {
        (*rng.pick(&KEYS)).to_string()
    } else {
        rng.string(6)
    }
}

fn big_int(rng: &mut Rng) -> Value {
    let len = 19 + rng.index(30);
    let mut s = String::from(if rng.chance(0.3) { "-" } else { "" });
    s.push(char::from(b'1' + rng.below(9) as u8));
    for _ in 1..len {
        s.push(char::from(b'0' + rng.below(10) as u8));
    }
    json::loads(&s).expect("an int")
}

fn rand_value(rng: &mut Rng, depth: usize) -> Value {
    let k = rng.f64();
    if depth < 3 && k < 0.15 {
        let n = rng.index(4);
        return Value::Array((0..n).map(|_| rand_value(rng, depth + 1)).collect());
    }
    if depth < 3 && k < 0.3 {
        let mut m = Map::new();
        for _ in 0..rng.index(4) {
            let key = rand_key(rng);
            let v = rand_value(rng, depth + 1);
            m.insert(key, v);
        }
        return Value::Object(m);
    }
    if k < 0.4 {
        return rng
            .pick(&[Value::Null, Value::Bool(true), Value::Bool(false)])
            .clone();
    }
    if k < 0.5 {
        return Value::Int(Int::Small(rng.range(-100_000, 100_000)));
    }
    if k < 0.55 {
        return big_int(rng);
    }
    if k < 0.65 {
        let f = match rng.below(4) {
            0 => rng.float_bits(),
            1 => rng.f64(),
            2 => *rng.pick(&[0.5, -0.0, 1e16, 1e-7, 123.456, 2.5e-308, 1e300, 5e-324]),
            _ => *rng.pick(&[f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0]),
        };
        return Value::Float(f);
    }
    Value::Str(rng.string(12))
}

fn dump_opts(rng: &mut Rng) -> DumpOpts {
    let indent = if rng.chance(0.5) { Some(2) } else { None };
    DumpOpts {
        indent,
        sort_keys: rng.chance(0.3),
        ensure_ascii: rng.chance(0.5),
        item_sep: if indent.is_some() { "," } else { ", " },
        key_sep: ": ",
    }
}

fn rand_prior(rng: &mut Rng) -> Option<Vec<u8>> {
    let k = rng.f64();
    if k < 0.15 {
        return None;
    }
    if k < 0.25 {
        return Some(rng.pick(ODD_PRIORS).to_vec());
    }
    let mut obj = Map::new();
    for _ in 0..rng.index(6) {
        let key = rand_key(rng);
        let v = rand_value(rng, 0);
        obj.insert(key, v);
    }
    let opts = dump_opts(rng);
    let mut text = json::dumps(&Value::Object(obj), &opts);
    if k < 0.37 {
        // A lone surrogate escape in a key or a value (D-6).
        let c = 0xd800 + rng.below(0x800);
        let lone = format!(
            "\"\\u{c:04x}{}\"",
            rng.ascii_string(3).replace(['"', '\\'], "")
        );
        let extra = if rng.chance(0.5) {
            format!("{lone}: 1")
        } else {
            format!("\"rtk_path\": {lone}")
        };
        text = if text == "{}" {
            format!("{{{extra}}}")
        } else {
            format!("{{{extra}, {}", &text[1..])
        };
    } else if k < 0.42 {
        // A broken text: cut short.
        let cut = rng.index(text.len().max(1));
        let cut = (0..=cut)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        text.truncate(cut);
    }
    Some(text.into_bytes())
}

/// One random case.
pub fn gen_case(rng: &mut Rng) -> Value {
    let prior = rand_prior(rng);
    let mut kv: Vec<(String, Value)> = Vec::new();
    for _ in 0..rng.index(4) {
        let key = rand_key(rng);
        let v = rand_value(rng, 0);
        // `**kv` has unique names: a repeated key keeps its first place and its last value.
        match kv.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = v,
            None => kv.push((key, v)),
        }
    }
    let mut m = Map::new();
    m.insert("op".into(), Value::Str("state_write".into()));
    m.insert(
        "prior".into(),
        prior.map_or(Value::Null, |p| Value::Str(hex(&p))),
    );
    m.insert(
        "kv".into(),
        Value::Array(
            kv.into_iter()
                .map(|(k, v)| Value::Array(vec![Value::Str(k), v]))
                .collect(),
        ),
    );
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_cases_run_and_cover_priors() {
        let mut rng = Rng::new(5);
        let cases = CoreState.generate(&mut rng, 300);
        let (mut absent, mut lone) = (0, 0);
        for c in &cases {
            match field(&c.data, "prior").as_str() {
                None => absent += 1,
                Some(h) if String::from_utf8_lossy(&unhex(h)).contains("\\ud") => lone += 1,
                Some(_) => {}
            }
            let o = run(&c.data);
            assert!(o.exit == 0 || o.exit == 1, "{o:?}");
        }
        assert!(absent > 10 && lone > 10, "{absent} {lone}");
    }

    #[test]
    fn a_golden_seed_runs() {
        let seeds = CoreState.seeds();
        assert!(seeds.len() >= 500);
        let o = run(&seeds[0].data);
        assert_eq!(o.exit, 0);
        assert!(o.stdout.starts_with(b"0600 state.json\n"));
    }
}
