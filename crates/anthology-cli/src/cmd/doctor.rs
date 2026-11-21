//! `anthology doctor [--json]` (GC-11). Owner: RT1b.
//!
//! As `doctor.py main()`: one `"%-18s %-4s %s"` line per check (then the unreadable note and
//! the duplicate lines), or with `--json` the report as `json.dumps(…, indent=2)`. Exit 0, or 2
//! when `platform_supported` is not ok (GC-5). Arguments go through `pycompat::argparse`
//! (usage text at exit 2 is D-4). The plugin folder is `$CLAUDE_PLUGIN_ROOT` (the binary does
//! not live in the plugin; see `anthology_runtime::doctor`).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_pycompat::argparse::{self, Arg, Spec};
use anthology_runtime::doctor::{self, DoctorDeps};

use super::Output;

/// Runs `anthology doctor …`; `args` are the arguments after `doctor`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            return Output {
                stderr: format!("anthology doctor: cannot read the working folder: {e}\n")
                    .into_bytes(),
                code: 1,
                ..Output::default()
            };
        }
    };
    run_with(args, &Env::from_process(), &cwd, &DoctorDeps::default())
}

fn spec() -> Spec {
    Spec::new("anthology doctor")
        .description("Anthology environment checks")
        .arg(
            Arg::option(&["--json"])
                .store_true()
                .help("print the result as JSON"),
        )
}

/// [`run`] with the environment, working folder and seams given.
pub fn run_with(args: &[OsString], env: &Env, cwd: &Path, deps: &DoctorDeps) -> Output {
    let parsed = match argparse::parse(&spec(), args) {
        Ok(p) => p,
        Err(u) => {
            return Output {
                stdout: u.stdout.into_bytes(),
                stderr: u.stderr.into_bytes(),
                code: u.code,
            };
        }
    };
    let report = doctor::report(env, &PathBuf::new(), cwd, deps);
    let (stdout, code) = doctor::render(&report, parsed.flag("json"));
    Output {
        stdout: stdout.into_bytes(),
        code,
        ..Output::default()
    }
}
