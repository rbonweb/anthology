//! The frozen goldens (CO2, design §4.5, §13.2): the oracle's answers, kept after Python is gone.
//!
//! `difftest freeze` runs a surface against the oracle as usual and, when the run is clean (no
//! unexplained diff, no oracle error), writes every case with Python's side of it to
//! `crates/<owner>/tests/golden/<surface>.jsonl.gz` ([`path`]). `difftest golden check` then judges
//! Rust against those answers with no oracle at all: the recorded cases, the recorded Python
//! result and the recorded divergence marks, against the same `port/difftest/divergences` files.
//! `difftest golden bless` rewrites the expected result of each case that fails with a diff to
//! Rust's current output (and drops its marks), so every deliberate change is a visible diff.
//!
//! The file is gzip ([`write`]: best compression, no name, mtime 0, so the same golden is the same
//! bytes on every machine; `zcat` reads it) of JSONL, ASCII only (`json.dumps` defaults):
//!
//! - line 1, the header: `{"difftest_golden": 1, "surface": S, "seed": N, "gen": N, "cases": N}`;
//! - then one line per case, in run order:
//!   `{"id": ID, "case": DATA, "expect": RESULT, "marks": [CLASS, ...]}`.
//!
//! `RESULT` is the oracle response shape ([`crate::observe`]): `exit`, `stdout` or `stdout_hex`,
//! `stderr` or `stderr_hex`, and `files` (the tree's JSON form) when there is a tree. A stream
//! or tree whose stored form would pass [`DIGEST_OVER`] bytes is kept as `<key>_sha256` and
//! `<key>_len` instead (the F2 rule for big outputs), and Rust's side is digested the same way
//! before comparing.
//!
//! A case whose data would pass [`CASE_DIGEST_OVER`] bytes (the shell sweeps, hostile repeats) is kept
//! as `case_sha256` and `case_len` instead of `case`: [`resolve`] gets it back from the surface's
//! own `seeds()` and `generate()` (the header's seed and gen) and checks the digest, so such a
//! case fails loudly if the surface stops making it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anthology_pycompat::json::{self, Int, Map, PY_DEFAULT, Value};
use sha2::{Digest, Sha256};

use crate::case::{Rng, Surface};
use crate::observe::{Observed, from_oracle};

/// The format version in the header.
pub const VERSION: i64 = 1;

/// A stored stream or tree over this many bytes is kept as a digest.
pub const DIGEST_OVER: usize = 16 * 1024;

/// A case's data over this many bytes is kept as a digest. Lower than [`DIGEST_OVER`]: a case
/// comes back from the surface, Python's answer does not. At 8 KiB the shell code-point sweeps
/// are digested, which takes `shell.segments` from about 6 MB to 0.5 MB compressed (CO2-prep).
pub const CASE_DIGEST_OVER: usize = 8 * 1024;

/// The crate a surface's golden lives in (its Rust side's crate).
pub fn owner(surface: &str) -> &'static str {
    let prefix = surface.split('.').next().unwrap_or(surface);
    match prefix {
        "pycompat" => "anthology-pycompat",
        "core" => "anthology-core",
        "shell" => "anthology-shell",
        "guard" => "anthology-guard",
        "settings" => "anthology-settings",
        "privacy" | "notes" | "memory" | "mem" => "anthology-memory",
        "rtk_install" | "doctor" | "hook" | "setup" | "runtime" => "anthology-runtime",
        "dev" => "anthology-dev",
        _ => "difftest",
    }
}

/// Where `surface`'s golden is: `<base>/crates/<owner>/tests/golden/<surface>.jsonl`, where
/// `base` is `--golden DIR` when given (the same layout elsewhere, so a freeze in the read-only
/// difftest container can write under `/out` and be copied over the repository with one
/// `cp -R`), else the repository root.
pub fn path(root: &Path, dir: Option<&Path>, surface: &str) -> PathBuf {
    dir.unwrap_or(root)
        .join("crates")
        .join(owner(surface))
        .join("tests/golden")
        .join(format!("{surface}.jsonl.gz"))
}

/// One frozen case.
#[derive(Clone, Debug, PartialEq)]
pub struct Frozen {
    pub id: String,
    /// The case (`Null` for a digested one until [`resolve`]).
    pub data: Value,
    /// A digested case's `case_sha256`, as read (module docs).
    pub case_sha256: Option<String>,
    /// The stored form of the expected result ([`form`]).
    pub expect: Value,
    pub marks: Vec<String>,
    /// Rust's stored result when it differed from `expect` at the freeze (a case a divergence
    /// entry waived): `check` pins Rust to it, so a waived case cannot drift unseen.
    pub rust: Option<Value>,
}

/// The fewest generated cases a repository golden may hold (`freeze` without `--sample`).
pub const MIN_GEN: usize = 1000;

/// A golden file's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub surface: String,
    pub seed: u64,
    pub generated: usize,
    /// Frozen with `--sample` (fewer than [`MIN_GEN`] generated cases): a self-test or scratch
    /// golden, which `check` refuses as a repository golden.
    pub sample: bool,
}

/// Checks a read golden against its surface: at least one case, exactly `generated` `gen:`
/// cases, and at least every seed besides; `repository` (no `--golden DIR`) also refuses a
/// sample golden.
pub fn check_shape(
    surface: &dyn Surface,
    header: &Header,
    cases: &[Frozen],
    repository: bool,
) -> Result<(), String> {
    let name = surface.name();
    if cases.is_empty() {
        return Err(format!("{name}: the golden holds no case"));
    }
    let generated = cases.iter().filter(|c| c.id.starts_with("gen:")).count();
    if generated != header.generated {
        return Err(format!(
            "{name}: the golden holds {generated} generated case(s), its header says {}",
            header.generated
        ));
    }
    let seeds = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| surface.seeds().len()))
        .map_err(|_| format!("{name}: the surface panicked making its seeds"))?;
    if cases.len() < seeds + header.generated {
        return Err(format!(
            "{name}: the golden holds {} case(s), fewer than the surface's {seeds} seeds + {} \
             generated (a thin golden; freeze it again)",
            cases.len(),
            header.generated
        ));
    }
    if repository && (header.sample || header.generated < MIN_GEN) {
        return Err(format!(
            "{name}: the repository golden is a sample ({} generated, under {MIN_GEN})",
            header.generated
        ));
    }
    Ok(())
}

fn int(n: i64) -> Value {
    Value::Int(Int::Small(n))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn digest(m: &mut Map, key: &str, bytes: &[u8]) {
    m.insert(
        format!("{key}_sha256"),
        Value::Str(hex(&Sha256::digest(bytes))),
    );
    m.insert(format!("{key}_len"), int(bytes.len() as i64));
}

fn stream(m: &mut Map, key: &str, bytes: &[u8]) {
    let (k, v) = match std::str::from_utf8(bytes) {
        Ok(t) => (key.to_string(), t.to_string()),
        Err(_) => (format!("{key}_hex"), hex(bytes)),
    };
    if v.len() > DIGEST_OVER {
        digest(m, key, bytes);
    } else if !v.is_empty() {
        m.insert(k, Value::Str(v));
    }
}

/// The stored form of a result (module docs). Equal results have equal forms.
pub fn form(o: &Observed) -> Value {
    let mut m = Map::new();
    m.insert("exit".into(), int(i64::from(o.exit)));
    stream(&mut m, "stdout", &o.stdout);
    stream(&mut m, "stderr", &o.stderr);
    if let Some(t) = &o.files {
        let v = t.to_json();
        let text = json::dumps(&v, &PY_DEFAULT);
        if text.len() > DIGEST_OVER {
            digest(&mut m, "files", text.as_bytes());
        } else {
            m.insert("files".into(), v);
        }
    }
    Value::Object(m)
}

/// Whether a stored form holds a digest (so [`expected`] lost that part).
pub fn has_digest(v: &Value) -> bool {
    v.as_object()
        .is_some_and(|o| o.iter().any(|(k, _)| k.ends_with("_sha256")))
}

/// The result a stored form describes, for reports; a digested part reads as empty.
pub fn expected(v: &Value) -> Observed {
    let mut v = v.clone();
    if let Some(o) = v.as_object_mut() {
        let keys: Vec<String> = o
            .iter()
            .map(|(k, _)| k.to_string())
            .filter(|k| k.ends_with("_sha256") || k.ends_with("_len"))
            .collect();
        for k in keys {
            o.remove(&k);
        }
    }
    from_oracle(&v)
}

/// The text of a golden file.
pub fn render(header: &Header, cases: &[Frozen]) -> String {
    let mut out = String::new();
    let mut h = Map::new();
    h.insert("difftest_golden".into(), int(VERSION));
    h.insert("surface".into(), Value::Str(header.surface.clone()));
    let seed = i64::try_from(header.seed)
        .map(Int::Small)
        .unwrap_or_else(|_| Int::Big(header.seed.to_string()));
    h.insert("seed".into(), Value::Int(seed));
    h.insert("gen".into(), int(header.generated as i64));
    h.insert("cases".into(), int(cases.len() as i64));
    if header.sample {
        h.insert("sample".into(), Value::Bool(true));
    }
    out.push_str(&json::dumps(&Value::Object(h), &PY_DEFAULT));
    out.push('\n');
    for c in cases {
        let mut m = Map::new();
        m.insert("id".into(), Value::Str(c.id.clone()));
        let data = json::dumps(&c.data, &PY_DEFAULT);
        if data.len() > CASE_DIGEST_OVER {
            digest(&mut m, "case", data.as_bytes());
        } else {
            m.insert("case".into(), c.data.clone());
        }
        m.insert("expect".into(), c.expect.clone());
        let marks = c.marks.iter().map(|s| Value::Str(s.clone())).collect();
        m.insert("marks".into(), Value::Array(marks));
        if let Some(r) = &c.rust {
            m.insert("rust".into(), r.clone());
        }
        out.push_str(&json::dumps(&Value::Object(m), &PY_DEFAULT));
        out.push('\n');
    }
    out
}

/// The gzip of `text` (module docs: deterministic; flate2 and miniz_oxide are pinned).
pub fn compress(text: &str) -> Vec<u8> {
    use std::io::Write;
    let mut gz = flate2::GzBuilder::new()
        .mtime(0)
        .write(Vec::new(), flate2::Compression::best());
    gz.write_all(text.as_bytes()).expect("gzip into memory");
    gz.finish().expect("gzip into memory")
}

/// Writes a golden file (its folder is created).
pub fn write(path: &Path, header: &Header, cases: &[Frozen]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, compress(&render(header, cases)))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Parses a golden file's text; `label` names it in errors, `surface` is the one expected.
pub fn parse(text: &str, label: &str, surface: &str) -> Result<(Header, Vec<Frozen>), String> {
    let mut lines = text.lines().enumerate();
    let err = |n: usize, m: &str| format!("{label}:{n}: {m}");
    let (_, first) = lines.next().ok_or_else(|| err(1, "empty golden file"))?;
    let h = json::loads(first).map_err(|e| err(1, &e.to_string()))?;
    let h = h
        .as_object()
        .ok_or_else(|| err(1, "the header is not an object"))?;
    if h.get("difftest_golden").and_then(Value::as_i64) != Some(VERSION) {
        return Err(err(1, &format!("not a version {VERSION} difftest golden")));
    }
    let name = h.get("surface").and_then(Value::as_str).unwrap_or("");
    if name != surface {
        return Err(err(1, &format!("the golden of {name:?}, not {surface:?}")));
    }
    let seed = match h.get("seed") {
        Some(Value::Int(Int::Small(n))) => u64::try_from(*n).ok(),
        Some(Value::Int(Int::Big(s))) => s.parse().ok(),
        _ => None,
    }
    .ok_or_else(|| err(1, "no seed"))?;
    let generated = h
        .get("gen")
        .and_then(Value::as_i64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| err(1, "no gen"))?;
    let count = h.get("cases").and_then(Value::as_i64);
    let mut cases = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for (i, line) in lines {
        let n = i + 1;
        let mut v = json::loads(line).map_err(|e| err(n, &e.to_string()))?;
        let o = v
            .as_object_mut()
            .ok_or_else(|| err(n, "a case line is not an object"))?;
        let id = o
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| err(n, "no \"id\""))?
            .to_string();
        if !ids.insert(id.clone()) {
            return Err(err(n, &format!("the case id {id:?} is used twice")));
        }
        let (data, case_sha256) = match (o.remove("case"), o.get("case_sha256")) {
            (Some(data), None) => (data, None),
            (None, Some(Value::Str(sha))) => (Value::Null, Some(sha.clone())),
            _ => return Err(err(n, "no \"case\" (or \"case_sha256\")")),
        };
        let expect = o.remove("expect").ok_or_else(|| err(n, "no \"expect\""))?;
        if expect.as_object().and_then(|e| e.get("exit")).is_none() {
            return Err(err(n, "\"expect\" has no exit"));
        }
        let marks = o
            .get("marks")
            .and_then(Value::as_array)
            .ok_or_else(|| err(n, "no \"marks\" list"))?
            .iter()
            .map(|m| m.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| err(n, "a mark is not a string"))?;
        let rust = o.remove("rust");
        if rust
            .as_ref()
            .is_some_and(|r| r.as_object().and_then(|e| e.get("exit")).is_none())
        {
            return Err(err(n, "\"rust\" has no exit"));
        }
        cases.push(Frozen {
            id,
            data,
            case_sha256,
            expect,
            marks,
            rust,
        });
    }
    if count != Some(cases.len() as i64) {
        return Err(err(
            1,
            &format!(
                "the header says {count:?} cases, the file has {}",
                cases.len()
            ),
        ));
    }
    let header = Header {
        surface: surface.to_string(),
        seed,
        generated,
        sample: h.get("sample").and_then(Value::as_bool) == Some(true),
    };
    Ok((header, cases))
}

/// Fills in each digested case from `surface`'s own seeds and generated cases (module docs).
/// Nothing runs when no case is digested.
pub fn resolve(surface: &dyn Surface, header: &Header, cases: &mut [Frozen]) -> Result<(), String> {
    if cases.iter().all(|c| c.case_sha256.is_none()) {
        return Ok(());
    }
    let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut rng = Rng::for_surface(header.seed, surface.name());
        let generated = surface.generate(&mut rng, header.generated);
        surface
            .seeds()
            .into_iter()
            .chain(generated)
            .collect::<Vec<_>>()
    }))
    .map_err(|_| format!("{}: the surface panicked making its cases", surface.name()))?;
    let made: HashMap<String, Value> = made.into_iter().map(|c| (c.id, c.data)).collect();
    for c in cases.iter_mut() {
        let Some(sha) = &c.case_sha256 else {
            continue;
        };
        let found = made
            .get(&c.id)
            .filter(|data| hex(&Sha256::digest(json::dumps(data, &PY_DEFAULT).as_bytes())) == *sha);
        match found {
            Some(data) => c.data = data.clone(),
            None => {
                return Err(format!(
                    "{} {}: the golden keeps this case as a digest, and the surface no longer \
                     makes a case with that id and digest (its seeds or generator changed)",
                    surface.name(),
                    c.id
                ));
            }
        }
    }
    Ok(())
}

/// Reads `surface`'s golden at `path`; `Ok(None)` when there is none.
pub fn read(path: &Path, surface: &str) -> Result<Option<(Header, Vec<Frozen>)>, String> {
    use std::io::Read;
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut text = String::new();
    flate2::read::GzDecoder::new(bytes.as_slice())
        .read_to_string(&mut text)
        .map_err(|e| format!("{}: not a gzip of UTF-8 text: {e}", path.display()))?;
    parse(&text, &path.display().to_string(), surface).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_testkit::{Tree, TreeEntry};

    fn frozen(id: &str, o: &Observed, marks: &[&str]) -> Frozen {
        Frozen {
            id: id.into(),
            data: json::loads(r#"{"text": "café", "n": [1, 2.5, 18446744073709551616]}"#)
                .expect("json"),
            case_sha256: None,
            expect: form(o),
            marks: marks.iter().map(|s| s.to_string()).collect(),
            rust: None,
        }
    }

    /// A surface whose cases carry a big string: seed `s:1`, and `gen:<n>` from the generator.
    struct Big;

    fn big_case(id: &str, x: u64) -> crate::case::Case {
        let text = format!("{x}{}", "\u{1F600}".repeat(DIGEST_OVER));
        let mut m = Map::new();
        m.insert("text".into(), Value::Str(text));
        crate::case::Case::new(id, Value::Object(m))
    }

    impl Surface for Big {
        fn name(&self) -> &'static str {
            "big"
        }
        fn seeds(&self) -> Vec<crate::case::Case> {
            vec![big_case("s:1", 0)]
        }
        fn generate(&self, rng: &mut Rng, n: usize) -> Vec<crate::case::Case> {
            (1..=n)
                .map(|i| big_case(&format!("gen:{i}"), rng.next_u64()))
                .collect()
        }
        fn run_python(&self, _: &crate::case::Case, _: &mut crate::oracle::Oracle) -> Observed {
            Observed::default()
        }
        fn run_rust(&self, _: &crate::case::Case) -> Observed {
            Observed::default()
        }
    }

    #[test]
    fn a_big_case_is_kept_as_a_digest_and_resolved_from_the_surface() {
        let header = Header {
            surface: "big".into(),
            seed: 7,
            generated: 2,
            sample: true,
        };
        let mut rng = Rng::for_surface(7, "big");
        let made: Vec<_> = Big
            .seeds()
            .into_iter()
            .chain(Big.generate(&mut rng, 2))
            .collect();
        let cases: Vec<Frozen> = made
            .iter()
            .map(|c| Frozen {
                id: c.id.clone(),
                data: c.data.clone(),
                case_sha256: None,
                expect: form(&Observed::text(0, "ok", "")),
                marks: Vec::new(),
                rust: None,
            })
            .collect();
        let text = render(&header, &cases);
        assert!(text.len() < 2000, "{} bytes", text.len());
        let (h, mut back) = parse(&text, "x", "big").expect("parses");
        assert!(
            back.iter()
                .all(|c| c.data.is_null() && c.case_sha256.is_some())
        );
        resolve(&Big, &h, &mut back).expect("resolves");
        for (b, c) in back.iter().zip(&cases) {
            assert_eq!(b.data, c.data, "{}", b.id);
        }
        // Rendering again gives the same file.
        assert_eq!(render(&h, &back), text);
        // A generator that changed (another seed here) is a loud error, not a silent pass.
        let mut back = parse(&text, "x", "big").expect("parses").1;
        let other = Header { seed: 8, ..h };
        let e = resolve(&Big, &other, &mut back).expect_err("changed");
        assert!(
            e.contains("big gen:1: the golden keeps this case as a digest"),
            "{e}"
        );
    }

    #[test]
    fn a_golden_round_trips_ascii_only() {
        let mut tree = Tree::default();
        tree.0.insert(
            "a/b".into(),
            TreeEntry::File {
                bytes: vec![0xff, 0],
                mode: 0o644,
            },
        );
        let mut with_files = Observed::text(3, "out\u{2028}", "");
        with_files.files = Some(tree);
        let raw = Observed {
            exit: 0,
            stdout: vec![0xc3],
            stderr: b"err".to_vec(),
            files: None,
        };
        let cases = vec![
            frozen("golden:1", &with_files, &[]),
            Frozen {
                rust: Some(form(&Observed::text(1, "rust", ""))),
                ..frozen("gen:1", &raw, &["D-6", "D-2"])
            },
        ];
        let header = Header {
            surface: "core.lock".into(),
            seed: u64::MAX,
            generated: 1,
            sample: false,
        };
        let text = render(&header, &cases);
        assert!(text.is_ascii());
        assert_eq!(text.lines().count(), 3);
        let (h, back) = parse(&text, "x", "core.lock").expect("parses");
        assert_eq!(h, header);
        assert_eq!(back, cases);
        assert_eq!(expected(&back[0].expect), with_files);
        assert_eq!(expected(&back[1].expect), raw);
        // On disk it is gzip with no name and mtime 0: the same golden is the same bytes.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = path(dir.path(), None, "core.lock");
        write(&file, &header, &cases).expect("write");
        let bytes = std::fs::read(&file).expect("read");
        assert_eq!(bytes, compress(&text));
        assert_eq!(&bytes[..4], &[0x1f, 0x8b, 8, 0]);
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0]);
        assert_eq!(read(&file, "core.lock").expect("reads"), Some((h, back)));
        std::fs::write(&file, &text).expect("write");
        let e = read(&file, "core.lock").expect_err("plain text");
        assert!(e.contains("not a gzip of UTF-8 text"), "{e}");
    }

    #[test]
    fn big_outputs_are_digested_on_both_sides() {
        let big = "x".repeat(DIGEST_OVER + 1);
        let a = Observed::text(0, &big, "small");
        let f = form(&a);
        assert!(has_digest(&f));
        let o = f.as_object().expect("object");
        assert!(o.get("stdout").is_none());
        assert_eq!(
            o.get("stdout_len").and_then(Value::as_i64),
            Some(big.len() as i64)
        );
        assert_eq!(o.get("stderr").and_then(Value::as_str), Some("small"));
        assert_eq!(form(&a.clone()), f);
        let mut b = a.clone();
        b.stdout[5] = b'y';
        assert_ne!(form(&b), f);
        assert!(!has_digest(&form(&Observed::text(0, "x", ""))));
        // A digested part reads as empty in the report form.
        assert_eq!(expected(&f).stdout, b"");
    }

    #[test]
    fn bad_files_are_refused_with_the_line() {
        let ok = render(
            &Header {
                surface: "s".into(),
                seed: 1,
                generated: 0,
                sample: true,
            },
            &[],
        );
        assert!(parse(&ok, "g", "s").is_ok());
        assert!(
            parse(&ok, "g", "t")
                .expect_err("wrong surface")
                .contains("not \"t\"")
        );
        for (text, want) in [
            ("", "empty golden file"),
            ("[]", "header is not an object"),
            ("{\"surface\": \"s\"}", "not a version 1"),
            (
                "{\"difftest_golden\": 1, \"surface\": \"s\", \"seed\": 1, \"gen\": 0, \"cases\": 1}",
                "the file has 0",
            ),
            (
                "{\"difftest_golden\": 1, \"surface\": \"s\", \"seed\": 1, \"gen\": 0, \"cases\": 1}\n\
                 {\"id\": \"a\", \"case\": 1, \"expect\": {}, \"marks\": []}",
                "g:2: \"expect\" has no exit",
            ),
            (
                "{\"difftest_golden\": 1, \"surface\": \"s\", \"seed\": 1, \"gen\": 0, \"cases\": 2}\n\
                 {\"id\": \"a\", \"case\": 1, \"expect\": {\"exit\": 0}, \"marks\": []}\n\
                 {\"id\": \"a\", \"case\": 1, \"expect\": {\"exit\": 0}, \"marks\": []}",
                "g:3: the case id \"a\" is used twice",
            ),
        ] {
            let e = parse(text, "g", "s").expect_err(text);
            assert!(e.contains(want), "{text}: {e}");
        }
    }

    #[test]
    fn thin_and_sample_goldens_are_refused() {
        let header = |generated, sample| Header {
            surface: "big".into(),
            seed: 7,
            generated,
            sample,
        };
        let case = |id: &str| Frozen {
            rust: None,
            ..frozen(id, &Observed::default(), &[])
        };
        let full: Vec<Frozen> = ["s:1", "gen:1", "gen:2"].iter().map(|i| case(i)).collect();
        assert!(check_shape(&Big, &header(2, true), &full, false).is_ok());
        for (h, cases, repository, want) in [
            (header(0, true), &[][..], false, "holds no case"),
            (
                header(3, true),
                &full[..],
                false,
                "2 generated case(s), its header says 3",
            ),
            (
                header(1, true),
                &full[1..2],
                false,
                "holds 1 case(s), fewer than",
            ),
            (
                header(2, true),
                &full[1..],
                false,
                "holds 2 case(s), fewer than",
            ),
            (
                header(2, true),
                &full[..],
                true,
                "is a sample (2 generated, under 1000)",
            ),
            (header(2, false), &full[..], true, "is a sample"),
        ] {
            let e = check_shape(&Big, &h, cases, repository).expect_err(want);
            assert!(e.contains(want), "{want}: {e}");
        }
    }

    #[test]
    fn every_surface_has_an_owner_crate_with_a_manifest() {
        let root = crate::case::repo_root();
        for name in crate::surfaces::ALL {
            let owner = owner(name);
            assert_ne!(owner, "difftest", "{name} has no owner crate");
            assert!(
                root.join("crates").join(owner).join("Cargo.toml").is_file(),
                "{name}: {owner}"
            );
        }
        assert_eq!(
            path(Path::new("/r"), None, "guard.check"),
            Path::new("/r/crates/anthology-guard/tests/golden/guard.check.jsonl.gz")
        );
        assert_eq!(
            path(Path::new("/r"), Some(Path::new("/g")), "mem"),
            Path::new("/g/crates/anthology-memory/tests/golden/mem.jsonl.gz")
        );
    }
}
