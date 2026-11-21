//! `anthology-dev memory-diff` (scripts/memory_diff.py). Owner: DV2.
//!
//! Lists the memory files a pull request changes, as a Markdown section for the job summary.
//! Exit codes (GC-5): 0 listed, 1 git error, 2 usage.

pub mod pystr;
pub mod unicode;

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_pycompat::pyerr::PyExc;

use crate::Output;
use crate::json_edit::Uncaught;
use unicode::Class;

pub const ROOTS: [&str; 2] = [".claude/memory", ".claude/agent-memory"];
pub const MAX_NAME: usize = 200;
pub const TITLE: &str = "Memory changes in this PR";
pub const EXPLANATION: &str = "Memory notes load into everyone's sessions, so they are reviewed \
                               like code. This lists them; it does not judge them.";
/// `subprocess.run(..., timeout=120)`.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const USAGE: &str = "usage: memory_diff.py <base> <head>\n";

/// One `git diff --name-status` entry: its status letter and its paths (two for R and C).
pub type Entry = (String, Vec<String>);

/// `parse_name_status`: `git diff --name-status -z` output → entries. Splits on NUL only.
pub fn parse_name_status(raw: &[u8]) -> Vec<Entry> {
    let mut fields: Vec<&[u8]> = raw.split(|&b| b == 0).collect();
    if fields.last().is_some_and(|f| f.is_empty()) {
        fields.pop();
    }
    let mut entries = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let status = String::from_utf8_lossy(fields[i]);
        let letter = match status.chars().next() {
            Some(c) => c.to_string(),
            None => "?".to_string(),
        };
        let count = if letter == "R" || letter == "C" { 2 } else { 1 };
        let end = (i + 1 + count).min(fields.len());
        let paths = fields[(i + 1).min(end)..end]
            .iter()
            .map(|f| String::from_utf8_lossy(f).into_owned())
            .collect();
        entries.push((letter, paths));
        i += 1 + count;
    }
    entries
}

/// `escape_name`: an untrusted file name made safe inside a Markdown table code span.
pub fn escape_name(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars().take(MAX_NAME) {
        match pystr::class(ch) {
            Some(Class::C | Class::Zl | Class::Zp) => out.push('?'),
            _ if ch == '`' => out.push('\''),
            _ if ch == '|' => out.push_str("\\|"),
            _ => out.push(ch),
        }
    }
    out
}

/// `render`: the Markdown section.
pub fn render(entries: &[Entry]) -> String {
    let mut lines = vec![
        format!("### {TITLE}"),
        String::new(),
        EXPLANATION.to_string(),
        String::new(),
    ];
    if entries.is_empty() {
        lines.push("No memory changes.".to_string());
    } else {
        lines.push("| status | file |".to_string());
        lines.push("| --- | --- |".to_string());
        for (letter, paths) in entries {
            let shown: Vec<String> = paths
                .iter()
                .map(|p| format!("`{}`", escape_name(p)))
                .collect();
            lines.push(format!(
                "| {} | {} |",
                escape_name(letter),
                shown.join(" → ")
            ));
        }
    }
    lines.join("\n") + "\n"
}

/// The argv of `changed_memory_files`.
pub fn git_argv(base: &OsStr, head: &OsStr) -> Vec<OsString> {
    let mut range = base.to_os_string();
    range.push("...");
    range.push(head);
    let mut argv: Vec<OsString> = ["diff", "--name-status", "-z", "--find-renames"]
        .iter()
        .map(OsString::from)
        .collect();
    argv.push(range);
    argv.push("--".into());
    argv.extend(
        ROOTS
            .iter()
            .map(|r| OsString::from(format!(":(top,literal){r}"))),
    );
    argv
}

/// `changed_memory_files`: the raw `git diff` output, or the `RuntimeError` text.
pub fn changed_memory_files(
    base: &OsStr,
    head: &OsStr,
    env: &Env,
    cwd: &Path,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let mut child = match Command::new("git")
        .args(git_argv(base, head))
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("git did not run: {}", PyExc::from(e).type_name())),
    };
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let mut err_pipe = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        b
    });
    let err_reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_reader.join();
                let _ = err_reader.join();
                return Err("git did not run: TimeoutExpired".to_string());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(format!("git did not run: {}", PyExc::from(e).type_name())),
        }
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    // subprocess's returncode: the exit code, or minus the signal number.
    let code = status
        .code()
        .unwrap_or_else(|| -status.signal().unwrap_or(0));
    if code != 0 {
        let text = String::from_utf8_lossy(&stderr);
        let detail = pystr::splitlines(pystr::strip(&text), false);
        return Err(match detail.first() {
            Some(first) => escape_name(first),
            None => format!("git exited {code}"),
        });
    }
    Ok(stdout)
}

/// Runs `anthology-dev memory-diff …`; `args` are the arguments after `memory-diff`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, stdin, &Env::from_process(), &cwd)
}

/// `memory_diff.main(args)` in `cwd`, with `env` as the environment.
pub fn run_in(args: &[OsString], stdin: &mut dyn Read, env: &Env, cwd: &Path) -> Output {
    run_with_timeout(args, stdin, env, cwd, GIT_TIMEOUT)
}

/// [`run_in`] with the git timeout as a parameter (the test seam for `TimeoutExpired`).
pub fn run_with_timeout(
    args: &[OsString],
    stdin: &mut dyn Read,
    env: &Env,
    cwd: &Path,
    timeout: Duration,
) -> Output {
    let _ = stdin;
    if args.len() != 2 || args.iter().any(|a| a.as_bytes().starts_with(b"-")) {
        return Output {
            stderr: USAGE.as_bytes().to_vec(),
            code: 2,
            ..Output::default()
        };
    }
    let raw = match changed_memory_files(&args[0], &args[1], env, cwd, timeout) {
        Ok(raw) => raw,
        Err(msg) => {
            return Output {
                stderr: format!("memory-diff: git failed: {msg}\n").into_bytes(),
                code: 1,
                ..Output::default()
            };
        }
    };
    let entries = parse_name_status(&raw);
    let section = render(&entries);
    let mut stdout = Vec::new();
    match env.get_os("GITHUB_STEP_SUMMARY").filter(|s| !s.is_empty()) {
        Some(summary) => {
            let path = cwd.join(summary);
            let appended = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .and_then(|mut f| f.write_all(format!("{section}\n").as_bytes()));
            if let Err(e) = appended {
                let mut exc = PyExc::from(e);
                if let PyExc::OSError { filename, .. } = &mut exc {
                    *filename = Some(summary.to_string_lossy().into_owned());
                }
                return Uncaught::from_exc(&exc).into_output("memory-diff", stdout);
            }
        }
        None => stdout.extend_from_slice(section.as_bytes()),
    }
    stdout.extend_from_slice(
        format!("memory-diff: {} changed memory file(s)\n", entries.len()).as_bytes(),
    );
    Output {
        stdout,
        stderr: Vec::new(),
        code: 0,
    }
}
