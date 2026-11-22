//! `anthology perm classify` (post-release plan P4, design §2): the read-only check behind the
//! permission mod.
//!
//! stdin is one JSON object, `{"command","cwd","roots":[abs],"read_rules":[rule text]}`, and
//! nothing else. The answer is exit 0 with exactly [`TOKEN`] on stdout when the command is proven
//! read-only and every path it reads passes the path check; anything else is exit 1 with one
//! `anthology perm: no decision: <reason>` line on stderr and nothing on stdout, so the caller keeps
//! the engine's verdict. There is no deny: this command only ever skips a prompt or stays out.
//!
//! The steps, each of which can only refuse:
//!
//! 1. `anthology_shell::classify_readonly` must prove the line ([`ReadOnly::Proven`]).
//! 2. Every `PATH` entry must be absolute: a relative one would let a file in the working
//!    directory stand in for `ls` or `git`.
//! 3. The roots and the working directory must resolve to directories; the `Read()` rules must all
//!    be readable and none may deny every read.
//! 4. `paths`: the `cd` targets and every path the proof lists (`readonly_paths`).
//! 5. `git`: for a line that runs git, the repository and its configuration.
//!
//! What this cannot see: the command runs in the user's shell, which may have its own aliases,
//! functions or environment (the shell snapshot); the check sees this process's environment.
//! Files can change between the check and the run.

mod git;
mod paths;
mod rules;

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_pycompat::json::{self, Value};
use anthology_shell::{ReadOnly, classify_readonly};

use super::Output;
use paths::{Checker, Place};
use rules::{Bases, Matcher};

/// The one answer that allows.
pub const TOKEN: &str = "anthology-perm:read-only:v1\n";

/// The largest request read from stdin.
const MAX_INPUT: usize = 1 << 20;

/// Runs `anthology perm …`; `args` are the arguments after `perm`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let words: Vec<Option<&str>> = args.iter().map(|a| a.to_str()).collect();
    match words.as_slice() {
        [Some("classify")] => {}
        [Some("classify"), ..] => return Output::usage("usage: anthology perm classify"),
        [] => return Output::usage("perm needs one of: classify"),
        [first, ..] => {
            return Output::usage(&format!("unknown command: perm {}", first.unwrap_or("?")));
        }
    }
    let env = Env::from_process();
    match read_request(stdin).and_then(|request| classify(&request, &env)) {
        Ok(()) => Output {
            stdout: TOKEN.as_bytes().to_vec(),
            ..Output::default()
        },
        Err(why) => Output {
            stderr: format!("anthology perm: no decision: {}\n", shown(&why)).into_bytes(),
            code: 1,
            ..Output::default()
        },
    }
}

/// `text` for a one-line reason: control characters become `?`, and it is cut at 300 characters.
fn shown(text: &str) -> String {
    let mut out: String = text
        .chars()
        .take(300)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if text.chars().count() > 300 {
        out.push_str("...");
    }
    out
}

struct Request {
    command: String,
    cwd: PathBuf,
    roots: Vec<PathBuf>,
    read_rules: Vec<String>,
}

fn read_request(stdin: &mut dyn Read) -> Result<Request, String> {
    let mut buf = Vec::new();
    stdin
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|_| "input: cannot read stdin".to_string())?;
    if buf.len() > MAX_INPUT {
        return Err("input: over 1 MiB".into());
    }
    let text = std::str::from_utf8(&buf).map_err(|_| "input: not UTF-8".to_string())?;
    let value = json::loads(text).map_err(|_| "input: not JSON".to_string())?;
    let map = value
        .into_object()
        .ok_or_else(|| "input: not a JSON object".to_string())?;
    let mut keys: Vec<&str> = map.iter().map(|(k, _)| k).collect();
    keys.sort_unstable();
    if keys != ["command", "cwd", "read_rules", "roots"] {
        return Err("input: the keys must be command, cwd, roots and read_rules".into());
    }
    let string = |key: &str| -> Result<String, String> {
        map.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("input: {key} is not a string"))
    };
    let strings = |key: &str| -> Result<Vec<String>, String> {
        let items = map
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("input: {key} is not a list"))?;
        items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect::<Option<Vec<String>>>()
            .ok_or_else(|| format!("input: {key} holds a value that is not a string"))
    };
    Ok(Request {
        command: string("command")?,
        cwd: PathBuf::from(string("cwd")?),
        roots: strings("roots")?.into_iter().map(PathBuf::from).collect(),
        read_rules: strings("read_rules")?,
    })
}

fn classify(request: &Request, env: &Env) -> Result<(), String> {
    let home = env.get("HOME").unwrap_or_default();
    let (paths, cd, runs_git_config) = match classify_readonly(&request.command, home) {
        ReadOnly::Proven {
            paths,
            cd,
            runs_git_config,
        } => (paths, cd, runs_git_config),
        ReadOnly::NotProven(why) => return Err(format!("not proven: {why}")),
    };
    if let Some(path) = env.get_os("PATH")
        && std::env::split_paths(path).any(|dir| !dir.is_absolute())
    {
        return Err("PATH holds a relative entry".into());
    }

    if !request.cwd.is_absolute() {
        return Err("cwd is not an absolute path".into());
    }
    let cwd_real = std::fs::canonicalize(&request.cwd)
        .ok()
        .filter(|p| p.is_dir())
        .ok_or("cwd is not a directory")?;
    if request.roots.iter().any(|root| !root.is_absolute()) {
        return Err("a root is not an absolute path".into());
    }
    let roots: Vec<PathBuf> = request
        .roots
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok().filter(|p| p.is_dir()))
        .collect();
    if roots.is_empty() {
        return Err("no root that is a directory".into());
    }
    let lex_cwd = paths::join_clean(Path::new("/"), &request.cwd.to_string_lossy());
    let home_path = (!home.is_empty()).then(|| PathBuf::from(home));
    let rules = Matcher::new(
        &request.read_rules,
        &Bases {
            cwd: &lex_cwd,
            roots: &request.roots,
            home: home_path.as_deref(),
        },
    )?;
    let mut checker = Checker::new(roots, rules);
    let start = Place {
        lex: lex_cwd,
        real: cwd_real,
    };
    let at = checker.follow_cd(start, &cd)?;
    for path in &paths {
        checker.check(&at, path)?;
    }
    if runs_git_config {
        git::check(&checker, &at.real, env)?;
    }
    Ok(())
}
