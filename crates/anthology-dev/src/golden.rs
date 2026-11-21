//! `anthology-dev golden check|bless`: the frozen difftest goldens (CO2, design §4.5, §13.2).
//!
//! ```text
//! anthology-dev golden check|bless (--surface A,B | --all) [--golden DIR] [--divergences DIR]
//!                                  [--root DIR] [--show N] [--report PATH]
//! ```
//!
//! `difftest freeze` writes `crates/<owner>/tests/golden/<surface>.jsonl` from the Python oracle
//! while it exists. `check` judges Rust against those answers with no Python (the same
//! divergence files); `bless` records Rust's result for every case that fails only by a
//! differing result, so each deliberate change is a visible diff, then checks again.
//!
//! The work is the `difftest` binary's (`difftest golden check|bless`), because the surfaces'
//! Rust sides live there and `difftest` depends on this crate. This command checks the
//! arguments and runs `cargo run --locked --quiet -p difftest -- golden …` in the repository
//! (the xtask pattern; `scripts/dev.sh golden check` runs it in the Rust image), or the binary
//! `ANTHOLOGY_DIFFTEST` names. The exit status is difftest's: 0 clean, 1 a failing case, 2 a
//! usage or file error.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;

use crate::Output;

const USAGE: &str = "usage: anthology-dev golden check|bless (--surface A,B | --all) [--golden DIR]
                                    [--divergences DIR] [--root DIR] [--show N] [--report PATH]";

/// The options passed through to `difftest golden`, and whether each takes a value.
const OPTIONS: &[(&str, bool)] = &[
    ("--surface", true),
    ("--all", false),
    ("--golden", true),
    ("--divergences", true),
    ("--root", true),
    ("--show", true),
    ("--report", true),
];

/// Runs `anthology-dev golden …`; `args` are the arguments after `golden`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let program = match std::env::var_os("ANTHOLOGY_DIFFTEST") {
        Some(bin) => vec![bin],
        None => [
            "cargo", "run", "--locked", "--quiet", "-p", "difftest", "--",
        ]
        .map(OsString::from)
        .to_vec(),
    };
    run_with(&program, args)
}

/// The checked arguments: the sub (`check`/`bless`), everything to pass on, and `--root`.
fn parse(args: &[OsString]) -> Result<(Vec<OsString>, Option<PathBuf>), String> {
    let sub = match args.first().and_then(|a| a.to_str()) {
        Some(s @ ("check" | "bless")) => s,
        Some(other) => return Err(format!("unknown command: golden {other}")),
        None => return Err("golden needs one of: check, bless".into()),
    };
    let mut out = vec![OsString::from("golden"), OsString::from(sub)];
    let mut root = None;
    let mut chosen = 0;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        let text = arg.to_string_lossy();
        let (name, inline) = match text.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(OsString::from(v))),
            None => (text.to_string(), None),
        };
        let Some(&(_, takes)) = OPTIONS.iter().find(|(o, _)| *o == name) else {
            return Err(format!("golden: unknown argument {text:?}"));
        };
        if name == "--surface" || name == "--all" {
            chosen += 1;
        }
        out.push(OsString::from(&name));
        if takes {
            let value = match inline {
                Some(v) => v,
                None => rest
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("golden: {name} needs a value"))?,
            };
            if name == "--root" {
                // Absolute, because cargo then runs in it (a relative one would resolve twice).
                let abs = std::path::absolute(&value)
                    .map_err(|e| format!("golden: --root {}: {e}", value.to_string_lossy()))?;
                out.push(abs.clone().into_os_string());
                root = Some(abs);
                continue;
            }
            out.push(value);
        } else if inline.is_some() {
            return Err(format!("golden: {name} takes no value"));
        }
    }
    if chosen != 1 {
        return Err("golden: give exactly one of --surface or --all".into());
    }
    Ok((out, root))
}

/// Runs `program` + the checked arguments (module docs), output passed through.
pub fn run_with(program: &[OsString], args: &[OsString]) -> Output {
    let (pass, root) = match parse(args) {
        Ok(p) => p,
        Err(e) => {
            return Output {
                stderr: format!("anthology-dev: {e}\n{USAGE}\n").into_bytes(),
                code: 2,
                ..Output::default()
            };
        }
    };
    let Some((first, rest)) = program.split_first() else {
        return Output::usage("golden: no difftest program");
    };
    let mut cmd = Command::new(first);
    cmd.args(rest).args(&pass);
    if let Some(root) = &root {
        cmd.current_dir(root);
    }
    match cmd.status() {
        Ok(status) => Output {
            code: status.code().unwrap_or(1),
            ..Output::default()
        },
        Err(e) => Output {
            stderr: format!(
                "anthology-dev: golden: cannot run {}: {e}\n",
                first.to_string_lossy()
            )
            .into_bytes(),
            code: 1,
            ..Output::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_arguments_are_checked_before_anything_runs() {
        let (pass, root) = parse(&os(&[
            "bless",
            "--surface",
            "core.lock",
            "--golden=/g",
            "--root",
            "/r",
        ]))
        .expect("valid");
        assert_eq!(
            pass,
            os(&[
                "golden",
                "bless",
                "--surface",
                "core.lock",
                "--golden",
                "/g",
                "--root",
                "/r"
            ])
        );
        assert_eq!(root, Some(PathBuf::from("/r")));
        for (args, want) in [
            (&[][..], "golden needs one of: check, bless"),
            (&["freeze"][..], "unknown command: golden freeze"),
            (&["check"][..], "exactly one of --surface or --all"),
            (&["check", "--all", "--surface", "x"][..], "exactly one of"),
            (
                &["check", "--all", "--gen", "5"][..],
                "unknown argument \"--gen\"",
            ),
            (&["check", "--surface"][..], "--surface needs a value"),
            (&["check", "--all=1"][..], "--all takes no value"),
        ] {
            let e = parse(&os(args)).expect_err(&format!("{args:?}"));
            assert!(e.contains(want), "{args:?}: {e}");
        }
    }

    #[test]
    fn difftest_runs_with_the_arguments_and_its_status_is_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("args");
        let script = format!("printf '%s\\n' \"$@\" >'{}'; exit 1", log.display());
        let program = os(&["sh", "-c", &script, "difftest"]);
        let out = run_with(&program, &os(&["check", "--all", "--show", "3"]));
        assert_eq!(out.code, 1, "{}", String::from_utf8_lossy(&out.stderr));
        let args = std::fs::read_to_string(&log).expect("log");
        assert_eq!(args, "golden\ncheck\n--all\n--show\n3\n");
        let out = run_with(&os(&["/nonexistent/difftest"]), &os(&["check", "--all"]));
        assert_eq!(out.code, 1);
        assert!(String::from_utf8_lossy(&out.stderr).contains("cannot run /nonexistent/difftest"));
        let out = run_with(&program, &os(&["check", "--bogus"]));
        assert_eq!(out.code, 2);
    }
}
