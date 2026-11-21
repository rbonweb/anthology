//! `difftest`: the port-time differential harness (F4, design §13.2), and its frozen goldens
//! (CO2, design §4.5).
//!
//! ```text
//! difftest --list
//! difftest (--surface A,B | --all) [--gen N] [--seed S] [--python P] [--report PATH]
//!          [--divergences DIR] [--root DIR] [--show N]
//! difftest freeze (--surface A,B | --all) [--gen N] [--seed S] [--python P] [--golden DIR] …
//! difftest golden check|bless (--surface A,B | --all) [--golden DIR] [--divergences DIR] …
//! ```
//!
//! - `--list` prints every surface name (plan §1.4).
//! - `--surface A,B` runs those surfaces; naming one that is not ported yet is an error.
//!   `--all` runs every ported surface and prints the others as `skip`.
//! - Each surface runs its seeds and `--gen N` generated cases (default 100) from `--seed S`
//!   (default 20261121) against the oracle `<P> -I port/difftest/oracle/_driver.py` (`--python`,
//!   default `python3`), judged against `<DIR>/<surface>.toml` (`--divergences`, default
//!   `port/difftest/divergences`).
//! - The output gives a count line per surface, and for each failing case (the first `--show N`,
//!   default 10, per surface) its id, the case and both sides. `--report PATH` writes everything
//!   as JSON.
//! - `--root DIR` is the repository (default: found from the working folder).
//! - `freeze` is that run, and writes each surface that ran clean (0 unexplained, 0 oracle
//!   errors) to its golden ([`golden::path`]: `crates/<owner>/tests/golden/<surface>.jsonl`
//!   under the repository, or under `--golden DIR`). A surface that did not run clean is not
//!   written, and the exit is 1.
//! - `golden check` judges Rust against the goldens with no oracle (`--gen`, `--seed` and
//!   `--python` do not apply; `--python` is accepted and ignored, for `scripts/difftest.sh`).
//!   With `--all`, a ported surface without a golden is a `skip` while the oracle exists, and a
//!   failure once `port/difftest/oracle/_driver.py` is gone (after the cutover every ported
//!   surface needs one). `golden bless` does the same, then records Rust's result for every case
//!   that failed only by a differing result, and checks again.
//!
//! Exit 0 only with zero unexplained diffs; 1 with any; 2 for a usage error or a bad divergence
//! or golden file. The hidden `--inject-diff SURFACE` flips a byte of Rust's output for that
//! surface's first case (the harness self-test, `ci/jobs/difftest-linux.sh`).
#![forbid(unsafe_code)]

mod case;
mod divergences;
mod golden;
mod harness;
mod normalize;
mod observe;
mod oracle;
mod surfaces;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anthology_pycompat::json::{INDENT2, Int, Map, Value, dumps};

use crate::case::Surface;
use crate::divergences::Divergences;
use crate::harness::{Options, Report, run_frozen, run_surface};
use crate::oracle::Oracle;

const USAGE: &str = "usage: difftest --list
       difftest (--surface A,B | --all) [--gen N] [--seed S] [--python P] [--report PATH]
                [--divergences DIR] [--root DIR] [--show N]
       difftest freeze (--surface A,B | --all) [--gen N] [--seed S] [--python P] [--golden DIR]
                [--report PATH] [--divergences DIR] [--root DIR] [--show N]
       difftest golden check|bless (--surface A,B | --all) [--golden DIR] [--report PATH]
                [--divergences DIR] [--root DIR] [--show N]";

/// What a run does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The live difftest against the oracle.
    Live,
    /// The live difftest, writing the goldens of the clean surfaces.
    Freeze,
    /// Rust against the goldens.
    Check,
    /// Rust against the goldens, recording Rust's result for differing cases.
    Bless,
}

#[derive(Debug)]
struct Args {
    mode: Mode,
    list: bool,
    all: bool,
    surfaces: Vec<String>,
    count: usize,
    seed: u64,
    /// Whether `--gen` or `--seed` was given (an error for `golden`).
    generation_given: bool,
    python: String,
    report: Option<PathBuf>,
    divergences: Option<PathBuf>,
    root: Option<PathBuf>,
    golden: Option<PathBuf>,
    show: usize,
    inject: Option<String>,
}

fn parse_args(args: Vec<std::ffi::OsString>) -> Result<Args, String> {
    use lexopt::prelude::*;
    let mut a = Args {
        mode: Mode::Live,
        list: false,
        all: false,
        surfaces: Vec::new(),
        count: 100,
        seed: 20261121,
        generation_given: false,
        python: "python3".into(),
        report: None,
        divergences: None,
        root: None,
        golden: None,
        show: 10,
        inject: None,
    };
    let mut p = lexopt::Parser::from_args(args);
    let text = |v: std::ffi::OsString| {
        v.into_string()
            .map_err(|_| "a value is not UTF-8".to_string())
    };
    let mut words: Vec<String> = Vec::new();
    while let Some(arg) = p.next().map_err(|e| e.to_string())? {
        match arg {
            Value(v) => words.push(text(v)?),
            Long("list") => a.list = true,
            Long("all") => a.all = true,
            Long("surface") => {
                let v = text(p.value().map_err(|e| e.to_string())?)?;
                a.surfaces
                    .extend(v.split(',').filter(|s| !s.is_empty()).map(str::to_string));
            }
            Long("gen") => {
                a.generation_given = true;
                a.count = p
                    .value()
                    .map_err(|e| e.to_string())?
                    .parse()
                    .map_err(|e| format!("--gen: {e}"))?
            }
            Long("seed") => {
                a.generation_given = true;
                a.seed = p
                    .value()
                    .map_err(|e| e.to_string())?
                    .parse()
                    .map_err(|e| format!("--seed: {e}"))?
            }
            Long("show") => {
                a.show = p
                    .value()
                    .map_err(|e| e.to_string())?
                    .parse()
                    .map_err(|e| format!("--show: {e}"))?
            }
            Long("python") => a.python = text(p.value().map_err(|e| e.to_string())?)?,
            Long("report") => a.report = Some(p.value().map_err(|e| e.to_string())?.into()),
            Long("divergences") => {
                a.divergences = Some(p.value().map_err(|e| e.to_string())?.into())
            }
            Long("root") => a.root = Some(p.value().map_err(|e| e.to_string())?.into()),
            Long("golden") => a.golden = Some(p.value().map_err(|e| e.to_string())?.into()),
            Long("inject-diff") => a.inject = Some(text(p.value().map_err(|e| e.to_string())?)?),
            Long("help") | Short('h') => return Err(String::new()),
            other => return Err(other.unexpected().to_string()),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    a.mode = match words[..] {
        [] => Mode::Live,
        ["freeze"] => Mode::Freeze,
        ["golden", "check"] => Mode::Check,
        ["golden", "bless"] => Mode::Bless,
        ["golden", ..] => return Err("golden needs check or bless".into()),
        _ => return Err(format!("unexpected argument {:?}", words.join(" "))),
    };
    let modes = usize::from(a.list) + usize::from(a.all) + usize::from(!a.surfaces.is_empty());
    if modes != 1 || (a.list && a.mode != Mode::Live) {
        return Err("give exactly one of --list, --surface or --all".into());
    }
    match a.mode {
        Mode::Freeze if a.inject.is_some() => {
            return Err("freeze does not take --inject-diff (it would freeze the diff)".into());
        }
        Mode::Check | Mode::Bless if a.generation_given => {
            return Err(
                "golden: the goldens hold their cases; --gen and --seed do not apply".into(),
            );
        }
        Mode::Bless if a.inject.is_some() => {
            return Err("golden bless does not take --inject-diff".into());
        }
        Mode::Live if a.golden.is_some() => {
            return Err("--golden goes with freeze or golden check|bless".into());
        }
        _ => {}
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args_os().skip(1).collect()) {
        Ok(a) => a,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("difftest: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.list {
        for name in surfaces::ALL {
            println!("{name}");
        }
        return ExitCode::SUCCESS;
    }
    let result = match args.mode {
        Mode::Live | Mode::Freeze => run(&args),
        Mode::Check | Mode::Bless => run_golden(&args),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("difftest: {e}");
            ExitCode::from(2)
        }
    }
}

/// The surfaces a run covers: the chosen ported ones, and the skipped ones with the reason.
type Chosen = (Vec<Box<dyn Surface>>, Vec<(&'static str, &'static str)>);

/// Picks the surfaces of `--surface`/`--all` (and checks `--inject-diff` names one of them).
fn choose(args: &Args) -> Result<Chosen, String> {
    let mut chosen = Vec::new();
    let mut skipped = Vec::new();
    for name in &args.surfaces {
        if !surfaces::ALL.contains(&name.as_str()) {
            return Err(format!("unknown surface {name:?} (difftest --list)"));
        }
    }
    if let Some(name) = &args.inject
        && !surfaces::ALL.contains(&name.as_str())
    {
        return Err(format!("--inject-diff: unknown surface {name:?}"));
    }
    for (name, surface) in surfaces::registry() {
        let wanted = args.all || args.surfaces.iter().any(|s| s == name);
        match (wanted, surface) {
            (false, _) => {}
            (true, Some(_)) if args.all && !surfaces::runs_here(name) => {
                skipped.push((name, "Linux only"));
            }
            (true, Some(s)) => chosen.push(s),
            (true, None) if args.all => skipped.push((name, "not ported yet")),
            (true, None) => return Err(format!("{name} is not ported yet")),
        }
    }
    if let Some(name) = &args.inject
        && !chosen.iter().any(|s| s.name() == name)
    {
        return Err(format!(
            "--inject-diff {name}: that surface is not in this run"
        ));
    }
    Ok((chosen, skipped))
}

/// The repository root and the divergence folder of a run (every file in it checked).
fn setup(args: &Args) -> Result<(PathBuf, PathBuf), String> {
    if let Some(root) = &args.root {
        case::set_repo_root(root.clone());
    }
    let root = case::repo_root().to_path_buf();
    let div_dir = args
        .divergences
        .clone()
        .unwrap_or_else(|| root.join("port/difftest/divergences"));
    check_divergence_folder(&div_dir)?;
    Ok((root, div_dir))
}

/// Runs the chosen surfaces against the oracle (and, for `freeze`, writes the goldens);
/// `Ok(clean)`, or `Err` for a usage or configuration error.
fn run(args: &Args) -> Result<bool, String> {
    let (root, div_dir) = setup(args)?;
    let (chosen, skipped) = choose(args)?;
    // Every divergence file is read before anything runs: a bad one, or one named after no
    // surface (a typo), is a usage error.
    let mut divs = Vec::new();
    for s in &chosen {
        divs.push(Divergences::load(&div_dir, s.name())?);
    }
    println!(
        "difftest: seed {}, --gen {}, python {}, divergences {}",
        args.seed,
        args.count,
        args.python,
        div_dir.display()
    );
    let freeze = args.mode == Mode::Freeze;
    let mut oracle = Oracle::python(&args.python, &root);
    let mut reports: Vec<Report> = Vec::new();
    let mut unfrozen = 0;
    for (s, div) in chosen.iter().zip(divs.iter_mut()) {
        let opts = Options {
            count: args.count,
            seed: args.seed,
            inject: args.inject.as_deref() == Some(s.name()),
            record: freeze,
        };
        let mut r = run_surface(s.as_ref(), &mut oracle, div, &opts);
        print_report(&r, args.show);
        if freeze {
            let path = golden::path(&root, args.golden.as_deref(), s.name());
            if r.failures.is_empty() && r.oracle_errors == 0 {
                let header = golden::Header {
                    surface: s.name().to_string(),
                    seed: args.seed,
                    generated: args.count,
                };
                golden::write(&path, &header, &r.frozen)?;
                println!(
                    "freeze: wrote {} ({} cases)",
                    path.display(),
                    r.frozen.len()
                );
            } else {
                unfrozen += 1;
                println!(
                    "freeze: {} NOT frozen ({} unexplained, {} oracle errors); {} is unchanged",
                    s.name(),
                    r.failures.len(),
                    r.oracle_errors,
                    path.display()
                );
            }
            r.frozen = Vec::new();
        }
        reports.push(r);
    }
    for (name, why) in &skipped {
        println!("skip {name} ({why})");
    }
    if let Some(name) = &args.inject
        && reports
            .iter()
            .any(|r| r.name == *name && r.injected.is_none())
    {
        return Err(format!(
            "--inject-diff {name}: the surface ran no case to inject into"
        ));
    }
    let unexplained: usize = reports.iter().map(|r| r.failures.len()).sum();
    println!(
        "difftest: {unexplained} unexplained diff(s) in {} surface(s), {} skipped",
        reports.len(),
        skipped.len()
    );
    if let Some(path) = &args.report {
        let names: Vec<&str> = skipped.iter().map(|(n, _)| *n).collect();
        write_report(path, args, &reports, &names)?;
    }
    Ok(unexplained == 0 && unfrozen == 0)
}

/// `golden check|bless`: Rust against the goldens, no oracle (module docs).
fn run_golden(args: &Args) -> Result<bool, String> {
    let (root, div_dir) = setup(args)?;
    let (chosen, mut skipped) = choose(args)?;
    let post_cutover = !root.join("port/difftest/oracle/_driver.py").is_file();
    let bless = args.mode == Mode::Bless;
    // Every golden and divergence file is read before anything runs.
    let mut work = Vec::new();
    let mut missing = Vec::new();
    for s in chosen {
        let path = golden::path(&root, args.golden.as_deref(), s.name());
        match golden::read(&path, s.name())? {
            Some((header, mut cases)) => {
                golden::resolve(s.as_ref(), &header, &mut cases)?;
                let div = Divergences::load(&div_dir, s.name())?;
                work.push((s, path, header, cases, div));
            }
            None if !args.all => {
                return Err(format!("{}: no golden at {}", s.name(), path.display()));
            }
            None if post_cutover => missing.push((s.name(), path)),
            None => skipped.push((s.name(), "no golden yet")),
        }
    }
    println!(
        "difftest golden {}: divergences {}{}",
        if bless { "bless" } else { "check" },
        div_dir.display(),
        if post_cutover {
            " (no Python oracle: every ported surface needs a golden)"
        } else {
            ""
        }
    );
    let mut reports: Vec<Report> = Vec::new();
    for (s, path, header, mut cases, mut div) in work {
        let inject = args.inject.as_deref() == Some(s.name());
        let mut r = run_frozen(s.as_ref(), &cases, &mut div, inject);
        if bless && !r.blessable.is_empty() {
            let n = r.blessable.len();
            for (i, stored) in std::mem::take(&mut r.blessable) {
                cases[i].expect = stored;
                cases[i].marks.clear();
            }
            golden::write(&path, &header, &cases)?;
            println!(
                "bless: {}: recorded Rust's result for {n} case(s) in {}",
                s.name(),
                path.display()
            );
            let mut div = Divergences::load(&div_dir, s.name())?;
            r = run_frozen(s.as_ref(), &cases, &mut div, false);
        }
        print_report(&r, args.show);
        reports.push(r);
    }
    for (name, why) in &skipped {
        println!("skip {name} ({why})");
    }
    for (name, path) in &missing {
        println!(
            "FAIL {name}: no golden at {} (freeze it before the oracle is deleted)",
            path.display()
        );
    }
    if let Some(name) = &args.inject
        && !reports
            .iter()
            .any(|r| r.name == *name && r.injected.is_some())
    {
        return Err(format!(
            "--inject-diff {name}: the surface ran no case to inject into"
        ));
    }
    let unexplained: usize = reports.iter().map(|r| r.failures.len()).sum();
    println!(
        "difftest: {unexplained} unexplained diff(s) in {} surface(s), {} skipped, {} without a \
         golden",
        reports.len(),
        skipped.len(),
        missing.len()
    );
    if let Some(path) = &args.report {
        let names: Vec<&str> = skipped.iter().map(|(n, _)| *n).collect();
        write_report(path, args, &reports, &names)?;
    }
    Ok(unexplained == 0 && missing.is_empty())
}

/// Every `*.toml` in `dir` is `<surface>.toml` for a registered surface, and parses.
fn check_divergence_folder(dir: &Path) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("--divergences {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".toml") else {
            continue;
        };
        if !surfaces::ALL.contains(&stem) {
            return Err(format!(
                "{}: no surface is named {stem:?} (difftest --list)",
                path.display()
            ));
        }
        Divergences::load(dir, stem)?;
    }
    Ok(())
}

fn print_report(r: &Report, show: usize) {
    let explained: usize = r.explained.values().sum();
    let classes: Vec<String> = r
        .explained
        .iter()
        .map(|(c, n)| format!("{c} {n}"))
        .collect();
    let classes = if classes.is_empty() {
        String::new()
    } else {
        format!(" ({})", classes.join(", "))
    };
    let waived: usize = r.waived.values().sum();
    println!(
        "{}: {} cases ({} seeds + {} generated), {} same, {explained} explained{classes}, \
         {waived} waived, {} unexplained, {} oracle errors",
        r.name,
        r.cases,
        r.seeds,
        r.generated,
        r.same,
        r.failures.len(),
        r.oracle_errors
    );
    for (entry, n) in &r.waived {
        println!("{}: waived {n} diff(s) by {entry}", r.name);
    }
    for entry in &r.glob_gen_waivers {
        println!(
            "warning: {entry} waives generated cases by a glob (gen: ids change with --seed and \
             --gen)"
        );
    }
    if r.restarts > 0 {
        println!(
            "warning: {}: the oracle process died and was restarted {} time(s)",
            r.name, r.restarts
        );
    }
    if let Some(id) = &r.injected {
        println!("{}: injected a diff into case {id}", r.name);
    }
    for f in r.failures.iter().take(show) {
        println!("FAIL {} {}: {}", r.name, f.id, f.reason);
        if !f.case.is_empty() {
            println!("  case:   {}", f.case);
            println!("  python: {}", f.python);
            println!("  rust:   {}", f.rust);
            for d in f.diff.iter().take(8) {
                println!("  diff:   {d}");
            }
        }
    }
    if r.failures.len() > show {
        println!("  … {} more failing case(s)", r.failures.len() - show);
    }
    for u in &r.unused {
        println!("warning: unused divergence entry {u}");
    }
}

fn write_report(
    path: &Path,
    args: &Args,
    reports: &[Report],
    skipped: &[&str],
) -> Result<(), String> {
    let mut m = Map::new();
    let seed = i64::try_from(args.seed)
        .map(Int::Small)
        .unwrap_or_else(|_| Int::Big(args.seed.to_string()));
    m.insert("seed".into(), Value::Int(seed));
    m.insert("gen".into(), Value::Int(Int::Small(args.count as i64)));
    m.insert(
        "surfaces".into(),
        Value::Array(reports.iter().map(Report::to_json).collect()),
    );
    m.insert(
        "skipped".into(),
        Value::Array(skipped.iter().map(|s| Value::Str(s.to_string())).collect()),
    );
    let text = dumps(&Value::Object(m), &INDENT2) + "\n";
    std::fs::write(path, text).map_err(|e| format!("--report {}: {e}", path.display()))
}
