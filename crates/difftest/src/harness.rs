//! Running one surface: both sides per case, the verdict, and the report (F4).
//!
//! For each case (the seeds, then `gen` generated ones): the oracle's notes are cleared, Python
//! runs (`run_python`), the notes are taken, Rust runs (`run_rust`, a panic caught), and the
//! case is judged against the surface's divergence file (`divergences` module docs):
//!
//! - an oracle error or a panic on either side is always unexplained;
//! - every divergence mark needs an entry of its class matching the case;
//! - a difference needs an entry with a `case` matching it. Such a waiver is reported apart
//!   from the marks it might share a class with (`waived N diff(s) by <file>:<line>`), and a
//!   glob entry that waives a generated case (`gen:<n>` ids depend on `--seed` and `--gen`)
//!   gets a warning.
//!
//! The cases must be well formed: `generate(n)` returns exactly `n` cases and ids are unique,
//! else the surface fails before anything runs.
//!
//! `--inject-diff` flips one bit of the first byte of Rust's stdout (or appends a byte to an
//! empty stdout) for the surface's first case: the self-test that a diff fails and a matching
//! entry passes.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use anthology_pycompat::json::{Int, Map, Value};

use crate::case::{Case, Rng, Surface};
use crate::divergences::{Class, Divergences};
use crate::observe::Observed;
use crate::oracle::{Notes, Oracle};

/// How a surface runs.
#[derive(Clone, Debug)]
pub struct Options {
    pub count: usize,
    pub seed: u64,
    /// Inject a diff into this surface's first case.
    pub inject: bool,
}

/// A case that did not pass.
#[derive(Clone, Debug)]
pub struct Failure {
    pub id: String,
    pub case: String,
    pub reason: String,
    pub python: String,
    pub rust: String,
    pub diff: Vec<String>,
}

/// One surface's result.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub name: String,
    pub cases: usize,
    pub seeds: usize,
    pub generated: usize,
    pub same: usize,
    /// Passing cases with an oracle- or hook-marked divergence, by class.
    pub explained: BTreeMap<Class, usize>,
    /// Diffs waived by an entry with a `case`, by entry (`<file>:<line> (<class>, case <c>)`).
    pub waived: BTreeMap<String, usize>,
    /// Glob entries that waived a generated case's diff.
    pub glob_gen_waivers: Vec<String>,
    /// How often the oracle process had to be started again (each death failed its case).
    pub restarts: usize,
    pub failures: Vec<Failure>,
    pub oracle_errors: usize,
    /// Unused divergence entries, as `<file>:<line> (<class>, case <c>)`.
    pub unused: Vec<String>,
    /// The case the injected diff went into.
    pub injected: Option<String>,
}

/// The judgement of one case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Same,
    /// Passed with marked divergences (`marks`) and/or a waived diff (`waiver`: the entry's
    /// label, and whether a glob waived a generated case).
    Explained {
        marks: Vec<Class>,
        waiver: Option<(String, bool)>,
    },
    Unexplained(String),
}

/// `<file>:<line> (<class>, case <c>)`.
pub fn entry_label(label: &str, e: &crate::divergences::Entry) -> String {
    format!(
        "{}:{} ({}, case {})",
        label,
        e.line,
        e.class,
        e.case.as_deref().unwrap_or("any")
    )
}

/// Judges one case (module docs).
pub fn judge(id: &str, equal: bool, notes: &Notes, div: &mut Divergences) -> Verdict {
    if let Some(e) = notes.errors.first() {
        return Verdict::Unexplained(e.clone());
    }
    let mut classes = Vec::new();
    for mark in &notes.marks {
        let Some(class) = Class::parse(mark) else {
            return Verdict::Unexplained(format!(
                "the oracle marked {mark:?}, which is not a GC-16 class"
            ));
        };
        if div.explain_mark(class, id).is_none() {
            return Verdict::Unexplained(format!(
                "the oracle marked {class}, but {} declares no {class} entry matching {id}",
                div.label
            ));
        }
        if !classes.contains(&class) {
            classes.push(class);
        }
    }
    let mut waiver = None;
    if !equal {
        let label = div.label.clone();
        match div.explain_diff(id) {
            Some(e) => {
                let glob = e.case.as_deref().is_some_and(|c| c.contains(['*', '?']));
                let gen_by_glob = glob && id.starts_with("gen:");
                waiver = Some((entry_label(&label, e), gen_by_glob));
            }
            None => {
                return Verdict::Unexplained(format!(
                    "unexplained diff (no entry with a case matching {id} in {label})"
                ));
            }
        }
    }
    if classes.is_empty() && waiver.is_none() {
        Verdict::Same
    } else {
        Verdict::Explained {
            marks: classes,
            waiver,
        }
    }
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "(no message)".into())
}

/// Runs `surface` (module docs).
pub fn run_surface(
    surface: &dyn Surface,
    oracle: &mut Oracle,
    div: &mut Divergences,
    opts: &Options,
) -> Report {
    let mut report = Report {
        name: surface.name().to_string(),
        ..Report::default()
    };
    let made = catch_unwind(AssertUnwindSafe(|| {
        let seeds = surface.seeds();
        let mut rng = Rng::for_surface(opts.seed, surface.name());
        let generated = surface.generate(&mut rng, opts.count);
        (seeds, generated)
    }));
    let fail_cases = |report: &mut Report, reason: String| {
        report.failures.push(Failure {
            id: "(cases)".into(),
            case: String::new(),
            reason,
            python: String::new(),
            rust: String::new(),
            diff: Vec::new(),
        });
    };
    let (seeds, generated) = match made {
        Ok(c) => c,
        Err(p) => {
            let why = format!("the surface panicked making its cases: {}", panic_text(&*p));
            fail_cases(&mut report, why);
            return report;
        }
    };
    report.seeds = seeds.len();
    report.generated = generated.len();
    if generated.len() != opts.count {
        let why = format!(
            "generate(rng, {}) returned {} cases",
            opts.count,
            generated.len()
        );
        fail_cases(&mut report, why);
        return report;
    }
    let cases: Vec<Case> = seeds.into_iter().chain(generated).collect();
    let mut ids = std::collections::HashSet::new();
    if let Some(dup) = cases.iter().find(|c| !ids.insert(c.id.as_str())) {
        let why = format!("the case id {:?} is used twice", dup.id);
        fail_cases(&mut report, why);
        return report;
    }
    let starts_before = oracle.starts();
    for (i, case) in cases.iter().enumerate() {
        report.cases += 1;
        oracle.take_notes();
        let python = catch_unwind(AssertUnwindSafe(|| surface.run_python(case, &mut *oracle)));
        let notes = oracle.take_notes();
        if !notes.errors.is_empty() {
            report.oracle_errors += 1;
        }
        let (python, py_panic) = match python {
            Ok(o) => (o, None),
            Err(p) => (Observed::default(), Some(panic_text(&*p))),
        };
        let rust = catch_unwind(AssertUnwindSafe(|| surface.run_rust(case)));
        let (mut rust, panicked) = match rust {
            Ok(o) => (o, None),
            Err(p) => (Observed::default(), Some(panic_text(&*p))),
        };
        if opts.inject && i == 0 {
            match rust.stdout.first_mut() {
                Some(b) => *b ^= 1,
                None => rust.stdout.push(b'!'),
            }
            report.injected = Some(case.id.clone());
        }
        let verdict = match (py_panic, panicked) {
            (Some(msg), _) => Verdict::Unexplained(format!("the python side panicked: {msg}")),
            (None, Some(msg)) => Verdict::Unexplained(format!("rust panicked: {msg}")),
            (None, None) => judge(&case.id, python == rust, &notes, div),
        };
        match verdict {
            Verdict::Same => report.same += 1,
            Verdict::Explained { marks, waiver } => {
                for c in marks {
                    *report.explained.entry(c).or_default() += 1;
                }
                if let Some((label, gen_by_glob)) = waiver {
                    if gen_by_glob && !report.glob_gen_waivers.contains(&label) {
                        report.glob_gen_waivers.push(label.clone());
                    }
                    *report.waived.entry(label).or_default() += 1;
                }
            }
            Verdict::Unexplained(reason) => report.failures.push(Failure {
                id: case.id.clone(),
                case: case.compact(400),
                reason,
                python: python.render(300),
                rust: rust.render(300),
                diff: python.diff(&rust),
            }),
        }
    }
    let started = oracle.starts() - starts_before;
    report.restarts = if starts_before == 0 {
        started.saturating_sub(1)
    } else {
        started
    };
    report.unused = div
        .unused()
        .iter()
        .map(|e| entry_label(&div.label, e))
        .collect();
    report
}

impl Report {
    /// The JSON form for `--report`.
    pub fn to_json(&self) -> Value {
        let int = |n: usize| Value::Int(Int::Small(n as i64));
        let s = |t: &str| Value::Str(t.to_string());
        let mut m = Map::new();
        m.insert("surface".into(), s(&self.name));
        m.insert("cases".into(), int(self.cases));
        m.insert("seeds".into(), int(self.seeds));
        m.insert("generated".into(), int(self.generated));
        m.insert("same".into(), int(self.same));
        let mut ex = Map::new();
        for (c, n) in &self.explained {
            ex.insert(c.as_str().into(), int(*n));
        }
        m.insert("explained".into(), Value::Object(ex));
        let mut waived = Map::new();
        for (label, n) in &self.waived {
            waived.insert(label.clone(), int(*n));
        }
        m.insert("waived".into(), Value::Object(waived));
        m.insert(
            "glob_gen_waivers".into(),
            Value::Array(self.glob_gen_waivers.iter().map(|u| s(u)).collect()),
        );
        m.insert("restarts".into(), int(self.restarts));
        m.insert("unexplained".into(), int(self.failures.len()));
        m.insert("oracle_errors".into(), int(self.oracle_errors));
        m.insert(
            "unused".into(),
            Value::Array(self.unused.iter().map(|u| s(u)).collect()),
        );
        if let Some(id) = &self.injected {
            m.insert("injected".into(), s(id));
        }
        let failures = self
            .failures
            .iter()
            .map(|f| {
                let mut o = Map::new();
                o.insert("id".into(), s(&f.id));
                o.insert("reason".into(), s(&f.reason));
                o.insert("case".into(), s(&f.case));
                o.insert("python".into(), s(&f.python));
                o.insert("rust".into(), s(&f.rust));
                o.insert(
                    "diff".into(),
                    Value::Array(f.diff.iter().map(|d| s(d)).collect()),
                );
                Value::Object(o)
            })
            .collect();
        m.insert("failures".into(), Value::Array(failures));
        Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_pycompat::json::loads;

    /// A surface whose Python side comes from the case (`py`, `mark`) and whose Rust side
    /// prints `rust`; `panic` makes Rust panic.
    struct Fake;

    fn field<'a>(c: &'a Case, k: &str) -> Option<&'a str> {
        c.data
            .as_object()
            .and_then(|o| o.get(k))
            .and_then(Value::as_str)
    }

    impl Surface for Fake {
        fn name(&self) -> &'static str {
            "fake"
        }
        fn seeds(&self) -> Vec<Case> {
            vec![
                Case::new(
                    "s:same",
                    loads(r#"{"py": "a", "rust": "a"}"#).expect("json"),
                ),
                Case::new(
                    "s:marked",
                    loads(r#"{"py": "a", "rust": "a", "mark": "D-6"}"#).expect("json"),
                ),
                Case::new(
                    "s:diff",
                    loads(r#"{"py": "a", "rust": "b"}"#).expect("json"),
                ),
            ]
        }
        fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
            (0..n)
                .map(|i| {
                    let x = rng.below(3).to_string();
                    let mut m = Map::new();
                    m.insert("py".into(), Value::Str(x.clone()));
                    m.insert("rust".into(), Value::Str(x));
                    Case::new(format!("gen:{}", i + 1), Value::Object(m))
                })
                .collect()
        }
        fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
            if let Some(m) = field(c, "mark") {
                oracle.mark(m);
            }
            Observed::text(0, field(c, "py").unwrap_or(""), "")
        }
        fn run_rust(&self, c: &Case) -> Observed {
            if field(c, "panic").is_some() {
                panic!("boom");
            }
            Observed::text(0, field(c, "rust").unwrap_or(""), "")
        }
    }

    fn oracle() -> Oracle {
        Oracle::spawn("/nonexistent".into(), vec![], "/".into())
    }

    fn run(toml: &str, opts: &Options) -> Report {
        let mut div = Divergences::parse(toml, "fake.toml").expect("valid");
        run_surface(&Fake, &mut oracle(), &mut div, opts)
    }

    const OPTS: Options = Options {
        count: 20,
        seed: 1,
        inject: false,
    };

    const ENTRIES: &str = "[[divergence]]\nclass = \"D-6\"\nreason = \"F-19\"\n\
        [[divergence]]\nclass = \"D-2\"\ncase = \"s:diff\"\nreason = \"text\"\n";

    #[test]
    fn a_diff_and_an_undeclared_mark_fail_without_entries() {
        let r = run("", &OPTS);
        assert_eq!(r.cases, 23);
        assert_eq!(r.same, 21);
        let ids: Vec<&str> = r.failures.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["s:marked", "s:diff"]);
        assert!(
            r.failures[0]
                .reason
                .contains("marked D-6, but fake.toml declares no D-6")
        );
        assert!(r.failures[1].reason.starts_with("unexplained diff"));
        assert_eq!(r.failures[1].diff.len(), 1);
    }

    #[test]
    fn matching_entries_explain_them() {
        let r = run(ENTRIES, &OPTS);
        assert!(r.failures.is_empty(), "{:?}", r.failures);
        assert_eq!(r.explained.get(&Class::D6), Some(&1));
        // A waived diff is reported by entry, not as a marked divergence.
        assert_eq!(r.explained.get(&Class::D2), None);
        assert_eq!(r.waived.get("fake.toml:4 (D-2, case s:diff)"), Some(&1));
        assert!(r.glob_gen_waivers.is_empty());
        assert_eq!((r.seeds, r.generated), (3, 20));
        assert!(r.unused.is_empty());
    }

    #[test]
    fn a_wrong_class_fails_and_unused_entries_are_listed() {
        let toml = "[[divergence]]\nclass = \"D-1\"\nreason = \"wrong class\"\n\
            [[divergence]]\nclass = \"D-2\"\ncase = \"s:diff\"\nreason = \"text\"\n";
        let r = run(toml, &OPTS);
        assert_eq!(r.failures.len(), 1);
        assert!(r.failures[0].reason.contains("no D-6 entry"));
        assert_eq!(r.unused, ["fake.toml:1 (D-1, case any)"]);
    }

    #[test]
    fn an_entry_without_a_case_does_not_hide_a_diff() {
        let toml = "[[divergence]]\nclass = \"D-6\"\nreason = \"F-19\"\n";
        let r = run(toml, &OPTS);
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].id, "s:diff");
    }

    #[test]
    fn an_injected_diff_fails_and_an_entry_for_it_passes() {
        let opts = Options {
            inject: true,
            ..OPTS
        };
        let r = run(ENTRIES, &opts);
        assert_eq!(r.injected.as_deref(), Some("s:same"));
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].id, "s:same");
        let with = format!(
            "{ENTRIES}[[divergence]]\nclass = \"D-2\"\ncase = \"s:same\"\nreason = \"injected\"\n"
        );
        let r = run(&with, &opts);
        assert!(r.failures.is_empty(), "{:?}", r.failures);
    }

    #[test]
    fn a_rust_panic_and_an_oracle_error_are_never_explained() {
        let mut div = Divergences::parse(
            "[[divergence]]\nclass = \"D-1\"\ncase = \"*\"\nreason = \"all\"\n",
            "x",
        )
        .expect("valid");
        let case = Case::new("p", loads(r#"{"panic": "1"}"#).expect("json"));
        struct One(Case);
        impl Surface for One {
            fn name(&self) -> &'static str {
                "one"
            }
            fn seeds(&self) -> Vec<Case> {
                vec![self.0.clone()]
            }
            fn generate(&self, _: &mut Rng, _: usize) -> Vec<Case> {
                Vec::new()
            }
            fn run_python(&self, c: &Case, o: &mut Oracle) -> Observed {
                Fake.run_python(c, o)
            }
            fn run_rust(&self, c: &Case) -> Observed {
                Fake.run_rust(c)
            }
        }
        let opts = Options { count: 0, ..OPTS };
        let r = run_surface(&One(case), &mut oracle(), &mut div, &opts);
        assert_eq!(r.failures.len(), 1);
        assert_eq!(r.failures[0].reason, "rust panicked: boom");
        let notes = Notes {
            marks: vec![],
            errors: vec!["oracle error: x".into()],
        };
        assert_eq!(
            judge("p", true, &notes, &mut div),
            Verdict::Unexplained("oracle error: x".into())
        );
        let notes = Notes {
            marks: vec!["D-9".into()],
            errors: vec![],
        };
        assert!(matches!(
            judge("p", true, &notes, &mut div),
            Verdict::Unexplained(_)
        ));
    }

    #[test]
    fn a_glob_that_waives_generated_diffs_is_flagged() {
        struct Gen;
        impl Surface for Gen {
            fn name(&self) -> &'static str {
                "gen"
            }
            fn seeds(&self) -> Vec<Case> {
                Vec::new()
            }
            fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
                Fake.generate(rng, n)
            }
            fn run_python(&self, c: &Case, o: &mut Oracle) -> Observed {
                Fake.run_python(c, o)
            }
            fn run_rust(&self, _: &Case) -> Observed {
                Observed::text(0, "different", "")
            }
        }
        let mut div = Divergences::parse(
            "[[divergence]]\nclass = \"D-2\"\ncase = \"gen:*\"\nreason = \"all\"\n",
            "g.toml",
        )
        .expect("valid");
        let r = run_surface(&Gen, &mut oracle(), &mut div, &OPTS);
        assert!(r.failures.is_empty());
        assert_eq!(r.waived.get("g.toml:1 (D-2, case gen:*)"), Some(&20));
        assert_eq!(r.glob_gen_waivers, ["g.toml:1 (D-2, case gen:*)"]);
    }

    #[test]
    fn bad_case_lists_and_python_panics_fail() {
        struct Short(bool);
        impl Surface for Short {
            fn name(&self) -> &'static str {
                "short"
            }
            fn seeds(&self) -> Vec<Case> {
                let c = Case::new("x", Value::Null);
                if self.0 { vec![c.clone(), c] } else { vec![c] }
            }
            fn generate(&self, _: &mut Rng, n: usize) -> Vec<Case> {
                (0..n / 2)
                    .map(|i| Case::new(format!("gen:{i}"), Value::Null))
                    .collect()
            }
            fn run_python(&self, _: &Case, _: &mut Oracle) -> Observed {
                panic!("helper broke")
            }
            fn run_rust(&self, _: &Case) -> Observed {
                Observed::default()
            }
        }
        let mut div = Divergences::default();
        let r = run_surface(&Short(false), &mut oracle(), &mut div, &OPTS);
        assert_eq!(r.failures[0].reason, "generate(rng, 20) returned 10 cases");
        let r = run_surface(
            &Short(true),
            &mut oracle(),
            &mut div,
            &Options { count: 0, ..OPTS },
        );
        assert_eq!(r.failures[0].reason, "the case id \"x\" is used twice");
        let r = run_surface(
            &Short(false),
            &mut oracle(),
            &mut div,
            &Options { count: 0, ..OPTS },
        );
        assert_eq!(
            r.failures[0].reason,
            "the python side panicked: helper broke"
        );
    }

    #[test]
    fn the_report_has_a_json_form() {
        let r = run("", &OPTS);
        let v = r.to_json();
        let o = v.as_object().expect("object");
        assert_eq!(o.get("unexplained").and_then(Value::as_i64), Some(2));
        assert_eq!(o.get("cases").and_then(Value::as_i64), Some(23));
    }
}
