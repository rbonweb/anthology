//! `anthology memory export|import` (memory_portable; GC-5, GC-12). Owner: MX1. The command
//! lives in `anthology_memory::portable::main`, so the difftest drives the same code.

use std::ffi::OsString;
use std::io::Read;

use anthology_core::Env;
use anthology_core::git::RealGit;
use anthology_memory::portable;
use anthology_pycompat::pyerr::PyExc;

use super::Output;

/// Runs `anthology memory …`; `args` are the arguments after `memory`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    // `os.getcwd()`: a working folder that is gone is an `OSError` (exit 1).
    let cwd = match std::env::current_dir() {
        Ok(c) => c,
        Err(e) => {
            return Output {
                stderr: format!("memory_portable: {}\n", PyExc::from(e).py_str()).into_bytes(),
                code: 1,
                ..Output::default()
            };
        }
    };
    let out = portable::main(args, &cwd, &Env::from_process(), &RealGit);
    Output {
        stdout: out.stdout.into_bytes(),
        stderr: out.stderr.into_bytes(),
        code: out.code,
    }
}
