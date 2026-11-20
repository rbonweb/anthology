//! `difftest`: the port-time differential harness (F4, design §13.2).
//!
//! ```text
//! difftest --list
//! difftest (--surface A,B | --all) [--gen N] [--seed S] [--python P] [--report PATH]
//!          [--divergences DIR] [--root DIR] [--show N]
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
//!
//! Exit 0 only with zero unexplained diffs; 1 with any; 2 for a usage error or a bad divergence
//! file. The hidden `--inject-diff SURFACE` flips a byte of Rust's output for that surface's
//! first case (the harness self-test, `ci/jobs/difftest-linux.sh`).
#![forbid(unsafe_code)]

mod case;
mod divergences;
mod harness;
mod normalize;
mod observe;
mod oracle;
mod surfaces;

use std::path::PathBuf;
use std::process::ExitCode;

use anthology_pycompat::json::{INDENT2, Int, Map, Value, dumps};

use crate::divergences::Divergences;
use crate::harness::{Options, Report, run_surface};
use crate::oracle::Oracle;

const USAGE: &str = "usage: difftest --list
       difftest (--surface A,B | --all) [--gen N] [--seed S] [--python P] [--report PATH]
                [--divergences DIR] [--root DIR] [--show N]";

#[derive(Debug)]
struct Args {
    list: bool,
    all: bool,
    surfaces: Vec<String>,
    count: usize,
    seed: u64,
    python: String,
    report: Option<PathBuf>,
    divergences: Option<PathBuf>,
    root: Option<PathBuf>,
    show: usize,
    inject: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    use lexopt::prelude::*;
    let mut a = Args {
        list: false,
        all: false,
        surfaces: Vec::new(),
        count: 100,
        seed: 20261121,
        python: "python3".into(),
        report: None,
        divergences: None,
        root: None,
        show: 10,
        inject: None,
    };
    let mut p = lexopt::Parser::from_env();
    let text = |v: std::ffi::OsString| {
        v.into_string()
            .map_err(|_| "a value is not UTF-8".to_string())
    };
    while let Some(arg) = p.next().map_err(|e| e.to_string())? {
        match arg {
            Long("list") => a.list = true,
            Long("all") => a.all = true,
            Long("surface") => {
                let v = text(p.value().map_err(|e| e.to_string())?)?;
                a.surfaces
                    .extend(v.split(',').filter(|s| !s.is_empty()).map(str::to_string));
            }
            Long("gen") => {
                a.count = p
                    .value()
                    .map_err(|e| e.to_string())?
                    .parse()
                    .map_err(|e| format!("--gen: {e}"))?
            }
            Long("seed") => {
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
            Long("inject-diff") => a.inject = Some(text(p.value().map_err(|e| e.to_string())?)?),
            Long("help") | Short('h') => return Err(String::new()),
            other => return Err(other.unexpected().to_string()),
        }
    }
    let modes = usize::from(a.list) + usize::from(a.all) + usize::from(!a.surfaces.is_empty());
    if modes != 1 {
        return Err("give exactly one of --list, --surface or --all".into());
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse_args() {
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
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("difftest: {e}");
            ExitCode::from(2)
        }
    }
}

/// Runs the chosen surfaces; `Ok(clean)`, or `Err` for a usage or configuration error.
fn run(args: &Args) -> Result<bool, String> {
    if let Some(root) = &args.root {
        case::set_repo_root(root.clone());
    }
    let root = case::repo_root().to_path_buf();
    let div_dir = args
        .divergences
        .clone()
        .unwrap_or_else(|| root.join("port/difftest/divergences"));
    let mut chosen = Vec::new();
    let mut skipped = Vec::new();
    let mut registry = surfaces::registry();
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
    for (name, surface) in registry.drain(..) {
        let wanted = args.all || args.surfaces.iter().any(|s| s == name);
        match (wanted, surface) {
            (false, _) => {}
            (true, Some(s)) => chosen.push(s),
            (true, None) if args.all => skipped.push(name),
            (true, None) => return Err(format!("{name} is not ported yet")),
        }
    }
    // Every divergence file is read before anything runs: a bad one is a usage error.
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
    let mut oracle = Oracle::python(&args.python, &root);
    let mut reports: Vec<Report> = Vec::new();
    for (s, div) in chosen.iter().zip(divs.iter_mut()) {
        let opts = Options {
            count: args.count,
            seed: args.seed,
            inject: args.inject.as_deref() == Some(s.name()),
        };
        let r = run_surface(s.as_ref(), &mut oracle, div, &opts);
        print_report(&r, args.show);
        reports.push(r);
    }
    for name in &skipped {
        println!("skip {name} (not ported yet)");
    }
    let unexplained: usize = reports.iter().map(|r| r.failures.len()).sum();
    println!(
        "difftest: {unexplained} unexplained diff(s) in {} surface(s), {} skipped",
        reports.len(),
        skipped.len()
    );
    if let Some(path) = &args.report {
        write_report(path, args, &reports, &skipped)?;
    }
    Ok(unexplained == 0)
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
    println!(
        "{}: {} cases, {} same, {explained} explained{classes}, {} unexplained, {} oracle errors",
        r.name,
        r.cases,
        r.same,
        r.failures.len(),
        r.oracle_errors
    );
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
    path: &std::path::Path,
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
