//! Interim driver for the `shell.*` difftest surfaces until F4's harness lands (SH lead ruling R8); the
//! lead deletes it once `surfaces/shell_*.rs` implement F4's `Surface`.
//!
//!   shell_interim gen <surface> <seed> <n> [--no-seeds]   cases JSONL ({"id","data"}) on stdout: the seeds
//!                                                         and sweeps (unless --no-seeds), then n generated
//!   shell_interim run                                     cases on stdin → {"id","out"} JSONL on stdout, the
//!                                                         same form as `shell.py --batch`
//!
//! Run from the repository root (the seeds file is read relative to it). `port/difftest/shell_interim.sh`
//! runs both sides and compares them.
#![forbid(unsafe_code)]

#[path = "../src/surfaces/shell_segments.rs"]
mod shell_segments;

use std::io::{BufRead, Write};
use std::process::ExitCode;

use anthology_pycompat::json::{self, COMPACT_SORTED, Value};
use shell_segments::{field, generate, obj, run_rust, s, seed_cases, sweep_cases};

fn cases(surface: &str, seed: u64, n: usize, with_seeds: bool) -> Result<Vec<Value>, String> {
    let seeds_text = std::fs::read_to_string(shell_segments::SEEDS)
        .map_err(|e| format!("{}: {e}", shell_segments::SEEDS))?;
    let seeds = match surface {
        "shell.segments" => seed_cases(&seeds_text, &["segments", "shell_parse", "script_starts"]),
        _ => return Err(format!("unknown surface {surface}")),
    };
    let mut out = Vec::new();
    if with_seeds {
        for (k, data) in seeds.iter().enumerate() {
            out.push(obj(vec![
                ("id", s(&format!("seed-{k}"))),
                ("data", data.clone()),
            ]));
        }
        for (k, data) in sweep_cases().into_iter().enumerate() {
            out.push(obj(vec![("id", s(&format!("sweep-{k}"))), ("data", data)]));
        }
    }
    for (k, data) in generate(seed, n, &seeds).into_iter().enumerate() {
        out.push(obj(vec![
            ("id", s(&format!("gen-{seed}-{k}"))),
            ("data", data),
        ]));
    }
    Ok(out)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    match args.first().map(String::as_str) {
        Some("gen") if args.len() >= 4 => {
            let (Ok(seed), Ok(n)) = (args[2].parse::<u64>(), args[3].parse::<usize>()) else {
                eprintln!("shell_interim: seed and n must be numbers");
                return ExitCode::from(2);
            };
            let with_seeds = !args.iter().any(|a| a == "--no-seeds");
            match cases(&args[1], seed, n, with_seeds) {
                Ok(cases) => {
                    for case in cases {
                        if writeln!(out, "{}", json::dumps(&case, &COMPACT_SORTED)).is_err() {
                            return ExitCode::from(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("shell_interim: {e}");
                    return ExitCode::from(2);
                }
            }
        }
        Some("run") => {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else {
                    return ExitCode::from(1);
                };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(case) = json::loads(&line) else {
                    eprintln!("shell_interim: bad case line");
                    return ExitCode::from(2);
                };
                let id = field(&case, "id").cloned().unwrap_or(Value::Null);
                let data = field(&case, "data").cloned().unwrap_or(Value::Null);
                let Some(result) = run_rust(&data) else {
                    eprintln!("shell_interim: no Rust side for this case");
                    return ExitCode::from(2);
                };
                let line = obj(vec![("id", id), ("out", result)]);
                if writeln!(out, "{}", json::dumps(&line, &COMPACT_SORTED)).is_err() {
                    return ExitCode::from(1);
                }
            }
        }
        _ => {
            eprintln!(
                "usage: shell_interim gen <surface> <seed> <n> [--no-seeds] | shell_interim run"
            );
            return ExitCode::from(2);
        }
    }
    if out.flush().is_err() {
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
