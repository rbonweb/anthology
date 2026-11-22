//! `anthology perm classify` (post-release plan P4, design §2): the read-only check behind the
//! permission mod.
//!
//! stdin is one JSON object, `{"command","cwd","roots":[abs],"read_rules":[rule text]}`, and
//! nothing else. On a proof the answer is exit 0 and, on stdout, the [`TOKEN`] line followed by one
//! line per path the command reads: the resolved absolute path of every `cd` target, every file and
//! every start directory of a recursive reader, and for git the work tree, the git dir and the
//! common dir. The caller checks each of those with its own Read permission. Anything else is
//! exit 1 with one `anthology perm: no decision: <reason>` line on stderr and nothing on stdout, so
//! the caller keeps the engine's verdict. There is no deny: this command only ever skips a prompt
//! or stays out. A panic exits 101, never 0.
//!
//! The steps, each of which can only refuse:
//!
//! 1. The request is one JSON object with exactly the four keys, none twice.
//! 2. The command is plain ASCII (no invisible Unicode), and `anthology_shell::classify_readonly`
//!    proves the line ([`ReadOnly::Proven`]).
//! 3. `words`: what the table allows but a word or the environment changes (`GREP_OPTIONS`,
//!    `RIPGREP_CONFIG_PATH`, git signature atoms, the git forms that print history).
//! 4. The working directory the command runs in is known only from a leading absolute `cd`: the
//!    `cwd` of the request is not trusted to be the shell's (subagents, worktrees), so without
//!    such a `cd` every relative path, every implicit `.` and every git line refuses.
//! 5. The roots and `cwd` resolve to directories; the `Read()` rules are all readable and none
//!    denies every read.
//! 6. `paths`: the `cd` targets and every path the proof lists (`readonly_paths`).
//! 7. `PATH` (read from this process's environment, which the hook shares with the shell that
//!    runs the command): every entry is absolute, and none lies, as written or resolved, under a
//!    root, the request's `cwd` or a `cd` target, since a file there could stand in for `cat` or
//!    `git`. Nothing found under those places is ever run by this check.
//! 8. `git`: for a line that runs git, the git `PATH` finds, resolved, must also lie outside those
//!    places; it is asked about the repository and its configuration.
//!
//! What this cannot see: the command runs in the user's shell, which may have its own aliases,
//! functions or environment (the shell snapshot); the check sees this process's environment.
//! Files can change between the check and the run.

mod git;
mod paths;
mod rules;
mod words;

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
        Ok(reads) => {
            let mut stdout = TOKEN.to_string();
            for path in reads {
                stdout.push_str(&path);
                stdout.push('\n');
            }
            Output {
                stdout: stdout.into_bytes(),
                ..Output::default()
            }
        }
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
    // The parser keeps the last of two equal keys; a reader that keeps the first would see
    // another request, so a repeated key refuses.
    let mut written = top_keys(text)?;
    written.sort_unstable();
    if written.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("input: a duplicate key".into());
    }
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

/// The keys of the outermost object of `text` (valid JSON), decoded, in the order written.
fn top_keys(text: &str) -> Result<Vec<String>, String> {
    let bytes = text.as_bytes();
    let mut keys = Vec::new();
    let mut depth = 0usize;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'"' => {
                let start = at;
                at += 1;
                while at < bytes.len() && bytes[at] != b'"' {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
                let token = text
                    .get(start..=at)
                    .ok_or_else(|| "input: not JSON".to_string())?;
                let mut next = at + 1;
                while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
                    next += 1;
                }
                if depth == 1 && bytes.get(next) == Some(&b':') {
                    let key = json::loads(token)
                        .ok()
                        .and_then(Value::into_string)
                        .ok_or_else(|| "input: not JSON".to_string())?;
                    keys.push(key);
                }
            }
            _ => {}
        }
        at += 1;
    }
    Ok(keys)
}

/// The resolved paths the command reads, in order, or why there is no decision.
fn classify(request: &Request, env: &Env) -> Result<Vec<String>, String> {
    // Invisible Unicode (zero-width, bidi controls) and NUL can make a line read one way and run
    // another: only printable ASCII and tabs are read at all.
    if !request
        .command
        .bytes()
        .all(|b| b == b'\t' || (b' '..=b'~').contains(&b))
    {
        return Err("the command holds a character that is not printable ASCII".into());
    }
    let home = env.get("HOME").unwrap_or_default();
    let (paths, cd, runs_git_config) = match classify_readonly(&request.command, home) {
        ReadOnly::Proven {
            paths,
            cd,
            runs_git_config,
        } => (paths, cd, runs_git_config),
        ReadOnly::NotProven(why) => return Err(format!("not proven: {why}")),
    };
    words::check(&request.command, home, env)?;
    let path_dirs: Vec<PathBuf> = env
        .get_os("PATH")
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    if path_dirs.iter().any(|dir| !dir.is_absolute()) {
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
    // The request's cwd is not trusted to be the shell's: only a leading absolute `cd` says where
    // the rest runs.
    let anchored = cd.first().is_some_and(|target| target.starts_with('/'));
    if !anchored {
        if let Some(target) = cd.first() {
            return Err(format!(
                "a relative cd target, and the shell's working directory is not known: {}",
                shown(target)
            ));
        }
        if let Some(path) = paths.iter().find(|p| !p.path.starts_with('/')) {
            return Err(format!(
                "a relative path, and the shell's working directory is not known: {}",
                shown(&path.path)
            ));
        }
        if runs_git_config {
            return Err("git, and the shell's working directory is not known".into());
        }
    }
    let mut checker = Checker::new(roots, rules);
    let start = Place {
        lex: lex_cwd,
        real: cwd_real,
    };
    // The places a file of the project could sit in: each root, the request's cwd and every cd
    // target, as written and resolved.
    let mut places: Vec<PathBuf> = checker.roots.clone();
    places.extend(request.roots.iter().cloned());
    places.extend([start.lex.clone(), start.real.clone()]);
    let mut lex = start.lex.clone();
    for target in &cd {
        lex = paths::join_clean(&lex, target);
        places.push(lex.clone());
    }
    let at = checker.follow_cd(start, &cd)?;
    // `follow_cd` records the resolved cd targets and nothing else yet.
    places.extend(checker.reads.iter().cloned());
    let in_place = |p: &Path| places.iter().any(|place| paths::under(p, place));
    for dir in &path_dirs {
        let real = std::fs::canonicalize(dir).ok();
        if in_place(dir) || real.as_deref().is_some_and(in_place) {
            return Err(format!(
                "a PATH entry inside a root, the cwd or a cd target: {}",
                shown(&dir.to_string_lossy())
            ));
        }
    }
    for path in &paths {
        checker.check(&at, path)?;
    }
    if runs_git_config {
        let git = git::find_git(env).ok_or("no git on PATH")?;
        let git = std::fs::canonicalize(&git).map_err(|_| "git cannot be resolved")?;
        if in_place(&git) {
            return Err(format!(
                "git resolves inside a root, the cwd or a cd target: {}",
                shown(&git.to_string_lossy())
            ));
        }
        git::check(&mut checker, &git, &at.real, env)?;
    }
    // One path per line: a path that cannot be written so refuses.
    checker
        .reads
        .iter()
        .map(|path| {
            path.to_str()
                .filter(|p| !p.chars().any(char::is_control))
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "a path that cannot be listed: {}",
                        shown(&path.to_string_lossy())
                    )
                })
        })
        .collect()
}
