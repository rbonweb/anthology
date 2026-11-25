//! `anthology guard hook [--shim]`: the PreToolUse guard (GC-5, GC-6). Owner: GD2.
//!
//! `HOME` is `os.environ.get("HOME") or os.path.expanduser("~")`, as guard.py reads it
//! (`paths::home_raw`). The hook itself is `anthology_guard::hook`: plain mode keeps guard.py's
//! contract (exit 0 silent, or exit 2 with one block line); `--shim` adds the GC-17 allow token.

use std::ffi::OsString;
use std::io::Read;

use anthology_core::{Env, paths};
use anthology_guard::host;
use anthology_guard::path_vars::HostPathVars;

use super::Output;

/// Runs `anthology guard …`; `args` are the arguments after `guard`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let words: Vec<Option<&str>> = args.iter().map(|a| a.to_str()).collect();
    let shim = match words.as_slice() {
        [Some("hook")] => false,
        [Some("hook"), Some("--shim")] => true,
        [Some("hook"), ..] => return Output::usage("usage: anthology guard hook [--shim]"),
        [] => return Output::usage("guard needs one of: hook"),
        [first, ..] => {
            return Output::usage(&format!("unknown command: guard {}", first.unwrap_or("?")));
        }
    };
    // Inside an SDK host there is no /plugin command, so the (G0) line leaves it out.
    let embedded = host::embedded_from_env();
    // HOME (and the passwd fallback) and the host path variables the user listed are read under the
    // hook's watchdog; the variables against this process's working folder.
    let result = anthology_guard::hook_resolving(
        stdin,
        || {
            let env = Env::from_process();
            let home = paths::home_raw(&env).to_string_lossy().into_owned();
            let cwd = std::env::current_dir().ok();
            let vars = HostPathVars::new(&|k| env.get(k).map(str::to_owned), cwd.as_deref());
            (home, vars)
        },
        shim,
        embedded,
    );
    Output {
        stdout: Vec::new(),
        stderr: result.stderr.into_bytes(),
        code: result.code,
    }
}
