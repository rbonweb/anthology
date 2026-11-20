//! `anthology`: the one binary behind the hooks and the `bin/` wrappers (design §6).
//!
//! This crate is the only one that reads `std::env` (GC-2). Each command lives in `cmd/<name>.rs`
//! and returns an `Output`; this file only writes it out.
#![forbid(unsafe_code)]

mod cmd;

use std::ffi::OsString;
use std::io::Write;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let out = cmd::dispatch(&args, &mut std::io::stdin().lock());
    // A closed stdout or stderr must not turn into a panic: the exit code is the contract.
    let _ = std::io::stdout().lock().write_all(&out.stdout);
    let _ = std::io::stderr().lock().write_all(&out.stderr);
    std::process::exit(out.code);
}
