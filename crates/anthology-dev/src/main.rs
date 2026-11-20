//! `anthology-dev`: writes out what `anthology_dev::dispatch` returns.
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::Write;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let out = anthology_dev::dispatch(&args, &mut std::io::stdin().lock());
    let _ = std::io::stdout().lock().write_all(&out.stdout);
    let _ = std::io::stderr().lock().write_all(&out.stderr);
    std::process::exit(out.code);
}
