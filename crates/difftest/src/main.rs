//! `difftest`: the port-time differential harness (F4). F1 registers the surfaces only.
#![forbid(unsafe_code)]

mod case;
mod normalize;
mod observe;
mod oracle;
mod surfaces;

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--list") {
        let mut out = std::io::stdout().lock();
        for name in surfaces::ALL {
            if writeln!(out, "{name}").is_err() {
                return ExitCode::from(1);
            }
        }
        return ExitCode::SUCCESS;
    }
    eprintln!("difftest: not ported yet (F4); `difftest --list` lists the surfaces");
    ExitCode::from(70)
}
