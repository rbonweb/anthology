//! `anthology mem search|related|orphans|stale|rebuild|save` (GC-5, GC-12): the memory index CLI
//! behind `bin/anthology-mem`, prog name `anthology-mem`. Owner: MI1. The command lives in
//! `anthology_memory::index::main`, so the difftest drives the same code.

use std::ffi::OsString;
use std::io::Read;

use anthology_core::Env;
use anthology_memory::index::{self, MemDeps};

use super::Output;

/// Runs `anthology mem …`; `args` are the arguments after `mem`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let out = index::main(args, &Env::from_process(), &MemDeps::default());
    Output {
        stdout: out.stdout,
        stderr: out.stderr,
        code: out.code,
    }
}
