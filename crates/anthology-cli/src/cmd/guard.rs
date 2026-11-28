//! `anthology guard hook [--shim]`: the PreToolUse guard (GC-5, GC-6). Owner: GD2. Also
//! `anthology guard mask`, which masks secrets in the text on stdin.
//!
//! `HOME` is `os.environ.get("HOME") or os.path.expanduser("~")`, as guard.py reads it
//! (`paths::home_raw`). The hook itself is `anthology_guard::hook`: plain mode keeps guard.py's
//! contract (exit 0 silent, or exit 2 with one block line); `--shim` adds the GC-17 allow token.

use std::ffi::OsString;
use std::io::Read;

use anthology_core::{Env, paths};
use anthology_guard::host;
use anthology_guard::mask;
use anthology_guard::path_vars::HostPathVars;

use super::Output;

/// The most `anthology guard mask` reads: more than this is exit 3 and nothing on stdout.
const MAX_MASK_INPUT: u64 = 4 * 1024 * 1024;

/// `anthology guard mask`: stdin with each secret replaced by a marker (`anthology_guard::mask`).
/// The text it cannot take (over 4 MiB, not UTF-8) is exit 3 with nothing on stdout, so the
/// caller keeps the text as it was.
fn run_mask(stdin: &mut dyn Read) -> Output {
    let refused = |why: &str| Output {
        stderr: format!("anthology: guard mask: {why}\n").into_bytes(),
        code: 3,
        ..Output::default()
    };
    let mut bytes = Vec::new();
    if Read::take(stdin, MAX_MASK_INPUT + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return refused("cannot read the input");
    }
    if bytes.len() as u64 > MAX_MASK_INPUT {
        return refused("the input is over 4 MiB");
    }
    match String::from_utf8(bytes) {
        Ok(text) => Output {
            stdout: mask::mask_secrets(&text).text.into_bytes(),
            ..Output::default()
        },
        Err(_) => refused("the input is not UTF-8"),
    }
}

/// Runs `anthology guard …`; `args` are the arguments after `guard`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let words: Vec<Option<&str>> = args.iter().map(|a| a.to_str()).collect();
    let shim = match words.as_slice() {
        [Some("mask")] => return run_mask(stdin),
        [Some("hook")] => false,
        [Some("hook"), Some("--shim")] => true,
        [Some("hook" | "mask"), ..] => {
            return Output::usage("usage: anthology guard hook [--shim] | anthology guard mask");
        }
        [] => return Output::usage("guard needs one of: hook, mask"),
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
