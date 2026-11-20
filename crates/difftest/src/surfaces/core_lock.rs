//! The `core.lock` difftest surface (F3's oracle `port/difftest/oracle/core_lock.py`).
//!
//! Case: `{"op": "lock", "prefix": "rtk--v" | "anthology-bin--v", "text": TEXT}`:
//! `anthology_core::lock::parse(TEXT, PREFIX)`. For `rtk--v` the oracle is
//! `rtk_install.load_lock`; for `anthology-bin--v` it is the oracle's hand-written GC-17
//! reference (there is no Python). Both sides print the version, tag, repo and digests, or exit
//! 1 with `InstallError: <message>`. Seeds are the committed golden corpus; generated cases are
//! mutated locks: valid ones in several layouts, one field replaced by a random value, edited
//! text (a character deleted, inserted or replaced; cut short), repeated keys and lone surrogate
//! escapes (D-6 for `rtk--v`, `port/difftest/divergences/core.lock.toml`).

use anthology_core::lock::{self, ANTHOLOGY_TAG_PREFIX, Lock, RTK_TAG_PREFIX};
use anthology_core::platform::Target;
use anthology_pycompat::json::{self, DumpOpts, INDENT2, Int, Map, PY_DEFAULT, Value};

use crate::case::{Case, Rng, Surface, golden_seeds};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "core.lock";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(CoreLock))
}

struct CoreLock;

impl Surface for CoreLock {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        golden_seeds("crates/anthology-core/tests/golden/lock.jsonl")
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        (0..n)
            .map(|i| Case::new(format!("gen:{}", i + 1), gen_case(rng)))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("core_lock", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        run(&c.data)
    }
}

fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v.as_object()
        .and_then(|o| o.get(k))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("core.lock: no {k:?}"))
}

fn run(case: &Value) -> Observed {
    assert_eq!(text(case, "op"), "lock");
    match lock::parse(text(case, "text"), text(case, "prefix")) {
        Ok(l) => {
            let mut out = format!("{}\n{}\n{}\n", l.version, l.tag, l.repo);
            for t in Target::ALL {
                out.push_str(&format!("{} {}\n", t.triple(), l.sha256[&t]));
            }
            Observed::text(0, &out, "")
        }
        Err(msg) => Observed::text(1, "", &format!("InstallError: {msg}")),
    }
}

fn rand_hex(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|_| char::from(b"0123456789abcdef"[rng.index(16)]))
        .collect()
}

fn version(rng: &mut Rng) -> String {
    let mut v = format!("{}.{}.{}", rng.below(30), rng.below(30), rng.below(300));
    if rng.chance(0.5) {
        let tail: String = (0..1 + rng.index(8))
            .map(|_| char::from(*rng.pick(b"abcz0129.AZ-")))
            .collect();
        v.push('-');
        v.push_str(&tail);
    }
    v
}

fn valid_lock(rng: &mut Rng, prefix: &str) -> Lock {
    let version = version(rng);
    let owner = rng.pick(&["rbonweb", "a", "A_b.c-d", "x1"]).to_string();
    let name = rng
        .pick(&["anthology", "rtk", "x..y", "r.e-p_o"])
        .to_string();
    Lock {
        tag: format!("{prefix}{version}"),
        version,
        repo: format!("{owner}/{name}"),
        sha256: Target::ALL
            .into_iter()
            .map(|t| (t, rand_hex(rng, 64)))
            .collect(),
    }
}

fn lock_value(l: &Lock) -> Map {
    let mut digests = Map::new();
    for t in Target::ALL {
        digests.insert(t.triple().into(), Value::Str(l.sha256[&t].clone()));
    }
    let mut m = Map::new();
    m.insert("version".into(), Value::Str(l.version.clone()));
    m.insert("tag".into(), Value::Str(l.tag.clone()));
    m.insert("repo".into(), Value::Str(l.repo.clone()));
    m.insert("sha256".into(), Value::Object(digests));
    m
}

fn odd_value(rng: &mut Rng) -> Value {
    match rng.below(9) {
        0 => Value::Null,
        1 => Value::Bool(rng.chance(0.5)),
        2 => Value::Int(Int::Small(rng.range(-5, 100))),
        3 => Value::Float(rng.float_bits()),
        4 => Value::Float(*rng.pick(&[1.5, -0.0, 1e16, 1e-7, 0.1, 5e-324, 1e300])),
        5 => Value::Array(vec![Value::Int(Int::Small(1)), Value::Str(rng.string(3))]),
        6 => {
            let mut m = Map::new();
            m.insert(rng.string(3), Value::Str(rng.string(3)));
            Value::Object(m)
        }
        7 => json::loads("123456789012345678901234567890").expect("int"),
        _ => Value::Str(rng.string(10)),
    }
}

const EDIT_CHARS: &[char] = &[
    '{', '}', '[', ']', ':', ',', '"', '\\', ' ', '\n', '0', '9', 'a', 'f', 'g', 'A', '.', '-',
    '/', 'v', 'é', '\u{2028}', '😀', '\0', '\t',
];

fn edit(rng: &mut Rng, text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return text.to_string();
    }
    let i = rng.index(chars.len());
    let c = *rng.pick(EDIT_CHARS);
    let mut out: Vec<char> = chars[..i].to_vec();
    match rng.below(4) {
        0 => out.extend_from_slice(&chars[i + 1..]),
        1 => {
            out.push(c);
            out.extend_from_slice(&chars[i..]);
        }
        2 => {
            out.push(c);
            out.extend_from_slice(&chars[i + 1..]);
        }
        _ => {}
    }
    out.into_iter().collect()
}

/// One random case.
pub fn gen_case(rng: &mut Rng) -> Value {
    let prefix = if rng.chance(0.5) {
        RTK_TAG_PREFIX
    } else {
        ANTHOLOGY_TAG_PREFIX
    };
    let l = valid_lock(rng, prefix);
    let mut m = lock_value(&l);
    let layout = |rng: &mut Rng, m: &Map| -> String {
        let v = Value::Object(m.clone());
        match rng.below(4) {
            0 => json::dumps(&v, &PY_DEFAULT),
            1 => json::dumps(
                &v,
                &DumpOpts {
                    indent: Some(4),
                    ..INDENT2
                },
            ),
            _ => json::dumps(&v, &INDENT2) + "\n",
        }
    };
    let text = match rng.below(6) {
        // Valid, in the layout anthology-dev writes (or rtk's).
        0 => {
            if prefix == ANTHOLOGY_TAG_PREFIX && rng.chance(0.7) {
                lock::render_anthology(&l)
            } else {
                layout(rng, &m)
            }
        }
        // One field replaced by a random value.
        1 | 2 => {
            let field = *rng.pick(&["version", "tag", "repo", "sha256", "digest"]);
            let value = if rng.chance(0.5) {
                Value::Str(rng.string(10))
            } else {
                odd_value(rng)
            };
            if field == "digest" {
                let t = rng.pick(&Target::ALL).triple();
                if let Some(Value::Object(d)) = m.get_mut("sha256") {
                    d.insert(t.into(), value);
                }
            } else {
                if field == "version"
                    && let Some(s) = value.as_str()
                    && rng.chance(0.7)
                {
                    m.insert("tag".into(), Value::Str(format!("{prefix}{s}")));
                }
                m.insert(field.into(), value);
            }
            if prefix == ANTHOLOGY_TAG_PREFIX && rng.chance(0.5) {
                json::dumps(&Value::Object(m), &INDENT2) + "\n"
            } else {
                layout(rng, &m)
            }
        }
        // Edited text.
        3 | 4 => {
            let base = if prefix == ANTHOLOGY_TAG_PREFIX {
                lock::render_anthology(&l)
            } else {
                layout(rng, &m)
            };
            let mut t = edit(rng, &base);
            if rng.chance(0.3) {
                t = edit(rng, &t);
            }
            t
        }
        // Repeated keys and lone surrogate escapes.
        _ => {
            let base = json::dumps(&Value::Object(m), &INDENT2);
            if rng.chance(0.5) {
                let c = 0xd800 + rng.below(0x800);
                let field = *rng.pick(&["version", "repo", "tag"]);
                base.replacen(
                    &format!("\"{field}\": \""),
                    &format!("\"{field}\": \"\\u{c:04x}"),
                    1,
                )
            } else {
                let dup = format!("\"version\": \"{}\", ", version(rng));
                format!("{{{dup}{}", &base[1..])
            }
        }
    };
    let mut c = Map::new();
    c.insert("op".into(), Value::Str("lock".into()));
    c.insert("prefix".into(), Value::Str(prefix.into()));
    c.insert("text".into(), Value::Str(text));
    Value::Object(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_cases_mix_valid_and_invalid_locks_of_both_prefixes() {
        let mut rng = Rng::new(9);
        let cases = CoreLock.generate(&mut rng, 1000);
        let (mut ok, mut err, mut anth_ok) = (0, 0, 0);
        for c in &cases {
            let o = run(&c.data);
            if o.exit == 0 {
                ok += 1;
                if text(&c.data, "prefix") == ANTHOLOGY_TAG_PREFIX {
                    anth_ok += 1;
                }
            } else {
                err += 1;
            }
        }
        assert!(
            ok > 100 && err > 300 && anth_ok > 50,
            "{ok} {err} {anth_ok}"
        );
    }
}
