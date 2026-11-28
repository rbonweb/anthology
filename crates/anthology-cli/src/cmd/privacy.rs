//! `anthology privacy hook`: the check of every `git commit` for the owner's private details.
//!
//! Stdin is the Claude Code PreToolUse JSON of a Bash call. Exit 10 with the message on stderr
//! blocks the commit; every other outcome is exit 0 with no output. The check fails open: a
//! command it cannot read, a folder that is no repo, a git that fails or runs past its time, a
//! panic and an unknown input all allow. What it looks for, and how it matches, is
//! `anthology_memory::commit_scan`; this file finds the commits in the command and gathers what
//! each one adds (the staged diff, the unstaged diff of `-a` and of named paths, the message).

use std::ffi::{OsStr, OsString};
use std::io::{IsTerminal, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_core::git::{Git, TimedGit};
use anthology_memory::commit_scan::{
    CommitFinding, ScanContext, banned_names, block_message, scan_diff, scan_message,
};
use anthology_pycompat::json::{self, Value};
use anthology_shell::Entry;

use super::Output;

/// The exit code of `privacy hook` that blocks the commit; the hook script turns it into exit 2.
pub const HOOK_BLOCK: i32 = 10;

/// How long each git call may take.
const GIT_STEP: Duration = Duration::from_secs(4);

/// How long the whole check may take before it gives up and allows: well inside the hook's own
/// 60 s, which blocks.
const BUDGET: Duration = Duration::from_secs(40);

/// How many commit calls of one command are looked at.
const MAX_CALLS: usize = 4;

/// The most of a diff that is scanned.
const DIFF_CAP: usize = 16 << 20;

/// The most of a message file or the allow file that is read.
const FILE_CAP: u64 = 1 << 20;

/// User names that are no one's own: a handle is never one of these.
const GENERIC_HANDLES: [&str; 11] = [
    "root",
    "user",
    "admin",
    "runner",
    "node",
    "ubuntu",
    "ec2-user",
    "vscode",
    "codespace",
    "app",
    "shared",
];

/// Runs `anthology privacy …`; `args` are the arguments after `privacy`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    match args.first().and_then(|a| a.to_str()) {
        // Extra arguments are ignored: the hook allows whatever it is given.
        Some("hook") => {
            // A caught panic must not print the default hook's report on the hook's stderr.
            std::panic::set_hook(Box::new(|_| {}));
            guarded(|| {
                let raw = if std::io::stdin().is_terminal() {
                    String::new()
                } else {
                    super::hook::read_input(stdin)
                };
                hook_with(&Env::from_process(), &raw)
            })
        }
        Some(other) => Output::usage(&format!("unknown command: privacy {other}")),
        None => Output::usage("privacy needs one of: hook"),
    }
}

/// `f()`, or an allow (exit 0, no output) when it panics.
fn guarded(f: impl FnOnce() -> Output) -> Output {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_default()
}

/// The hook on its input text: exit 0 with no output, or `HOOK_BLOCK` with the message.
fn hook_with(env: &Env, raw: &str) -> Output {
    match verdict(env, raw) {
        Some(message) => Output {
            stderr: format!("{message}\n").into_bytes(),
            code: HOOK_BLOCK,
            ..Output::default()
        },
        None => Output::default(),
    }
}

/// The block message for this tool call, or None to allow it.
fn verdict(env: &Env, raw: &str) -> Option<String> {
    if env.get_os("ANTHOLOGY_COMMIT_PRIVACY") == Some(OsStr::new("0")) {
        return None;
    }
    let input = json::loads(raw).ok()?;
    let top = input.as_object()?;
    if top.get("tool_name")?.as_str()? != "Bash" {
        return None;
    }
    let command = top
        .get("tool_input")?
        .as_object()?
        .get("command")?
        .as_str()?;
    // The cheap test first: most Bash calls are no commit.
    if !command.contains("git") || !command.contains("commit") {
        return None;
    }
    let cwd = top
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|c| c.is_absolute())
        .or_else(|| std::env::current_dir().ok())?;
    let home = anthology_core::paths::home_raw(env);
    let calls = commit_calls(command, &home.to_string_lossy());
    let deadline = Instant::now() + BUDGET;
    let mut findings: Vec<CommitFinding> = Vec::new();
    for call in calls.iter().filter(|c| !c.dry_run).take(MAX_CALLS) {
        for f in scan_call(env, &cwd, call, deadline) {
            if !findings.contains(&f) {
                findings.push(f);
            }
        }
    }
    (!findings.is_empty()).then(|| block_message(&findings))
}

// --- the commit calls of a command -------------------------------------------------------------

/// One `git commit` the command runs.
#[derive(Debug, Default, PartialEq, Eq)]
struct CommitCall {
    /// The folder it runs in, from the `cd` the command made and the `-C` options: relative to the
    /// hook input's `cwd`, or absolute. Empty: the `cwd` itself.
    dir: PathBuf,
    /// `-a` or `--all`: the unstaged changes of tracked files are part of the commit.
    all: bool,
    /// The paths named on the command line: their unstaged changes are part of the commit.
    paths: Vec<String>,
    /// Each `-m`/`--message` value.
    messages: Vec<String>,
    /// The `-F`/`--file` value.
    message_file: Option<String>,
    /// `--dry-run`: no commit is made.
    dry_run: bool,
}

/// The `git commit` calls in `command`, in the order they run. A command the analysis could not
/// read to the end gives none.
fn commit_calls(command: &str, home: &str) -> Vec<CommitCall> {
    let analysis = anthology_shell::analyze(command, home);
    if analysis.incomplete {
        return Vec::new();
    }
    analysis
        .entries
        .iter()
        .filter(|e| e.prog == "git")
        .filter_map(|e| parse_commit(e, home))
        .map(|mut call| {
            // The analysis gives a message from a substitution as an empty word
            // (`-m "$(cat <<'EOF' … EOF)"`), and a heredoc or here-string on stdin
            // (`-F - <<'EOF'`) is no word at all: the text is in the command itself.
            let hidden = call.messages.iter().any(String::is_empty)
                || call.message_file.as_deref() == Some("-");
            if hidden {
                call.messages.retain(|m| !m.is_empty());
                call.messages.extend(heredoc_bodies(command));
                call.messages.extend(here_strings(command));
            }
            call
        })
        .collect()
}

/// The body of each heredoc in `command` that has its delimiter line: the text between the line
/// that holds `<<DELIM` and the line that is `DELIM` (with the leading tabs of `<<-` taken off).
/// A here-string (`<<<`) and a heredoc that is never closed (`$((1<<2))`) are no body.
fn heredoc_bodies(command: &str) -> Vec<String> {
    let lines: Vec<&str> = command.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let starts = heredoc_starts(lines[i]);
        i += 1;
        for (delimiter, strip) in starts {
            let first = i;
            let mut body = Vec::new();
            let mut closed = false;
            while let Some(line) = lines.get(i) {
                i += 1;
                let line = if strip {
                    line.trim_start_matches('\t')
                } else {
                    line
                };
                if line == delimiter {
                    closed = true;
                    break;
                }
                body.push(line);
            }
            if closed {
                out.push(body.join("\n"));
            } else {
                // not a heredoc: read the lines again for others
                i = first;
                break;
            }
        }
    }
    out
}

/// The word after each `<<<` in `command`: what is between its quotes, or up to the next space
/// or operator. The text is only read, never run, so its escapes are left as they are.
fn here_strings(command: &str) -> Vec<String> {
    command
        .split("<<<")
        .skip(1)
        .map(|after| {
            let after = after.trim_start_matches([' ', '\t']);
            match after.chars().next() {
                Some(q @ ('\'' | '"')) => {
                    let inner = &after[1..];
                    let mut end = inner.len();
                    let mut escaped = false;
                    for (at, c) in inner.char_indices() {
                        if escaped {
                            escaped = false;
                        } else if q == '"' && c == '\\' {
                            escaped = true;
                        } else if c == q {
                            end = at;
                            break;
                        }
                    }
                    inner[..end].to_string()
                }
                _ => after
                    .split(|c: char| c.is_whitespace() || ";&|<>()".contains(c))
                    .next()
                    .unwrap_or("")
                    .to_string(),
            }
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// The `<<DELIM` operators of one line: each delimiter, with its quotes taken off, and whether
/// the operator is `<<-`.
fn heredoc_starts(line: &str) -> Vec<(String, bool)> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut k = 0;
    while k + 1 < b.len() {
        if b[k] != b'<' || b[k + 1] != b'<' {
            k += 1;
            continue;
        }
        if b.get(k + 2) == Some(&b'<') {
            // a here-string
            k += 3;
            continue;
        }
        let mut j = k + 2;
        let strip = b.get(j) == Some(&b'-');
        if strip {
            j += 1;
        }
        while matches!(b.get(j), Some(b' ' | b'\t')) {
            j += 1;
        }
        let word_start = j;
        let word = match b.get(j) {
            Some(&q @ (b'\'' | b'"')) => {
                j += 1;
                let inner = j;
                while b.get(j).is_some_and(|&c| c != q) {
                    j += 1;
                }
                let word = &line[inner..j];
                j += 1;
                word.to_string()
            }
            _ => {
                while b
                    .get(j)
                    .is_some_and(|&c| !c.is_ascii_whitespace() && !b";&|<>()'\"".contains(&c))
                {
                    j += 1;
                }
                // `<<\EOF` is `<<'EOF'`
                line[word_start..j].trim_start_matches('\\').to_string()
            }
        };
        if !word.is_empty() {
            out.push((word, strip));
        }
        k = j.max(k + 2);
    }
    out
}

/// git's own options (before the subcommand) that take their value as the next word. `-C` is
/// handled apart, as it adds up, and `--git-dir` and `--work-tree` as a name that is not an
/// `=` form.
const GIT_VALUE_OPTIONS: [&str; 4] = ["-c", "--namespace", "--config-env", "--super-prefix"];

/// `git commit` options that take their value as the next word (the `=` form is one word).
const COMMIT_VALUE_OPTIONS: [&str; 9] = [
    "author",
    "date",
    "cleanup",
    "reuse-message",
    "reedit-message",
    "fixup",
    "squash",
    "template",
    "trailer",
];

/// The commit an entry makes, or None when it is another git command (or its folder cannot be
/// told).
fn parse_commit(entry: &Entry, home: &str) -> Option<CommitCall> {
    let args = &entry.args;
    let mut globals: Vec<&str> = Vec::new();
    let mut i = 0;
    while let Some(arg) = args.get(i).map(String::as_str) {
        if arg == "-C" {
            globals.push(args.get(i + 1)?);
            i += 2;
        } else if arg == "--git-dir" || arg == "--work-tree" || GIT_VALUE_OPTIONS.contains(&arg) {
            i += 2;
        } else if arg.starts_with('-') {
            // `-c k=v` is above; `--git-dir=…`, `--work-tree=…`, `--no-pager`, `-P` and the other
            // options take no next word.
            i += 1;
        } else {
            break;
        }
    }
    if args.get(i)?.as_str() != "commit" {
        return None;
    }
    let rest = &args[i + 1..];
    // A `-C <commit>` of the commit itself: the analysis takes it for a folder too.
    let stray = rest
        .windows(2)
        .rfind(|w| w[0] == "-C")
        .map(|w| w[1].as_str());
    let mut call = CommitCall {
        dir: commit_dir(entry, &globals, stray, home)?,
        ..CommitCall::default()
    };
    let mut j = 0;
    let mut paths_only = false;
    while let Some(arg) = rest.get(j).map(String::as_str) {
        j += 1;
        if paths_only {
            call.paths.push(arg.to_string());
        } else if arg == "--" {
            paths_only = true;
        } else if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            match name {
                "all" => call.all = true,
                "dry-run" => call.dry_run = true,
                "message" | "file" => {
                    let value = value.or_else(|| {
                        j += 1;
                        rest.get(j - 1).cloned()
                    });
                    if let Some(value) = value {
                        if name == "message" {
                            call.messages.push(value);
                        } else {
                            call.message_file = Some(value);
                        }
                    }
                }
                other if COMMIT_VALUE_OPTIONS.contains(&other) && value.is_none() => j += 1,
                _ => {}
            }
        } else if arg.len() > 1 && arg.starts_with('-') {
            let letters = &arg[1..];
            for (at, c) in letters.char_indices() {
                match c {
                    'a' => call.all = true,
                    'm' | 'F' | 'c' | 'C' | 't' => {
                        // The rest of the word is the value, else the next word is.
                        let attached = &letters[at + c.len_utf8()..];
                        let value = if attached.is_empty() {
                            j += 1;
                            rest.get(j - 1).cloned()
                        } else {
                            Some(attached.to_string())
                        };
                        match (c, value) {
                            ('m', Some(value)) => call.messages.push(value),
                            ('F', Some(value)) => call.message_file = Some(value),
                            _ => {}
                        }
                        break;
                    }
                    // `-S[keyid]` and `-u[mode]` take an attached value only.
                    'S' | 'u' => break,
                    _ => {}
                }
            }
        } else {
            call.paths.push(arg.to_string());
        }
    }
    Some(call)
}

/// The folder a commit runs in, relative to the hook input's `cwd` or absolute: the folder the
/// shell had moved to with `cd`, then each `-C` of git added in turn (an absolute one replaces).
/// The analysis writes the folders it tracks in lowercase, so the case is put right against the
/// disk when the folder is used ([`exact_case`]).
///
/// The analysis already folds a `-C` word into `entry.cwd`: the last one among all the words,
/// resolved against the `cd` folder, with none added to the one before it. One `-C` before the
/// subcommand is so the folder exactly. Otherwise (several, or a `-C <commit>` after `commit`)
/// the folder of the `cd` is taken back out of it, and the options are applied here; when that
/// cannot be done the call is left unread.
fn commit_dir(entry: &Entry, globals: &[&str], stray: Option<&str>, home: &str) -> Option<PathBuf> {
    let analysed = entry.cwd.as_deref().unwrap_or("");
    if stray.is_none() && globals.len() <= 1 {
        return Some(PathBuf::from(analysed));
    }
    let last = stray.or_else(|| globals.last().copied())?;
    let mut dir = if stray.is_none() && (last.starts_with('/') || last.starts_with('~')) {
        // an absolute last `-C` replaces whatever came before it
        PathBuf::new()
    } else {
        // `analysed` is `last` below the folder of the `cd`, both in lowercase
        let tail = PathBuf::from(last.to_lowercase());
        let plain = tail
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        let path = Path::new(analysed);
        if !plain || !path.ends_with(&tail) {
            return None;
        }
        let mut base = path.to_path_buf();
        for _ in tail.components() {
            base.pop();
        }
        base
    };
    for global in globals {
        dir = dir.join(expand_home(global, home));
    }
    Some(dir)
}

/// `path` with each folder named as it is on the disk. The analysis lowercases the folders it
/// tracks, which names another folder on a case-sensitive disk; a name that is not there as given
/// is looked up in its parent without regard to case. None when a folder is not found.
fn exact_case(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Normal(name) => {
                let next = out.join(name);
                if next.symlink_metadata().is_ok() {
                    out = next;
                    continue;
                }
                let want = name.to_str()?.to_lowercase();
                let found = std::fs::read_dir(&out)
                    .ok()?
                    .filter_map(Result::ok)
                    .map(|e| e.file_name())
                    .find(|n| n.to_str().is_some_and(|n| n.to_lowercase() == want))?;
                out.push(found);
            }
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// `~` and `~/x` as the home folder says.
fn expand_home(path: &str, home: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => PathBuf::from(home),
        Some(rest) if rest.starts_with('/') => Path::new(home).join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

// --- what one commit adds -------------------------------------------------------------------

/// The findings of one commit call. Every step that cannot be completed adds nothing.
fn scan_call(env: &Env, cwd: &Path, call: &CommitCall, deadline: Instant) -> Vec<CommitFinding> {
    let Some(dir) = exact_case(&cwd.join(&call.dir)) else {
        return Vec::new();
    };
    let git = TimedGit { timeout: GIT_STEP };
    let git_env = env.clone().with("GIT_OPTIONAL_LOCKS", "0");
    let out = |args: &[&str]| -> Option<Vec<u8>> {
        if Instant::now() >= deadline {
            return None;
        }
        let done = git.run(&git_env, &dir, args)?;
        (done.code == 0).then_some(done.stdout)
    };
    let Some(top) = out(&["rev-parse", "--show-toplevel"]) else {
        return Vec::new();
    };
    let top = PathBuf::from(OsStr::from_bytes(first_line(&top)));
    if !top.is_absolute() {
        return Vec::new();
    }
    let repo = repo_name(&out, &top);
    let ctx = context(env, &top, &repo, &out);

    let mut findings = Vec::new();
    let diff = |extra: &[&str], paths: &[String]| -> Option<String> {
        let mut args = vec![
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U0",
            "--inter-hunk-context=0",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--submodule=short",
            "-M",
        ];
        args.extend_from_slice(extra);
        if !paths.is_empty() {
            args.push("--");
            args.extend(paths.iter().map(String::as_str));
        }
        let bytes = out(&args)?;
        Some(String::from_utf8_lossy(&bytes[..bytes.len().min(DIFF_CAP)]).into_owned())
    };
    if let Some(staged) = diff(&["--cached"], &[]) {
        findings.extend(scan_diff(&staged, &ctx));
    }
    if (call.all || !call.paths.is_empty())
        && let Some(unstaged) = diff(&[], &call.paths)
    {
        findings.extend(scan_diff(&unstaged, &ctx));
    }
    if !call.messages.is_empty() {
        findings.extend(scan_message(&call.messages.join("\n\n"), &ctx));
    }
    if let Some(file) = call.message_file.as_deref().filter(|f| *f != "-")
        && let Some(text) = read_small(&dir.join(file))
    {
        findings.extend(scan_message(&text, &ctx));
    }
    findings
}

/// The name of the repo: the folder that holds the shared `.git` (so a worktree has its main
/// repo's name), else the name of the top folder.
fn repo_name(out: &dyn Fn(&[&str]) -> Option<Vec<u8>>, top: &Path) -> String {
    if let Some(common) = out(&["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
        let common = PathBuf::from(OsStr::from_bytes(first_line(&common)));
        if common.is_absolute()
            && common.file_name() == Some(OsStr::new(".git"))
            && let Some(name) = common.parent().and_then(Path::file_name)
        {
            return name.to_string_lossy().into_owned();
        }
    }
    top.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// What the scan looks for: the owner's home, handles, git email, host name, banned names and the
/// repo's allow file.
fn context(
    env: &Env,
    top: &Path,
    repo: &str,
    out: &dyn Fn(&[&str]) -> Option<Vec<u8>>,
) -> ScanContext {
    let home = env
        .get("HOME")
        .filter(|h| !h.is_empty())
        .map(str::to_string);
    let mut handles: Vec<String> = Vec::new();
    let named = [
        home.as_deref()
            .and_then(|h| Path::new(h).file_name())
            .and_then(OsStr::to_str),
        env.get("USER"),
        env.get("LOGNAME"),
    ];
    for name in named.into_iter().flatten() {
        let lower = name.to_ascii_lowercase();
        if name.chars().count() >= 3
            && !GENERIC_HANDLES.contains(&lower.as_str())
            && !handles.iter().any(|h| h.eq_ignore_ascii_case(name))
        {
            handles.push(name.to_string());
        }
    }
    let emails = out(&["config", "user.email"])
        .map(|b| String::from_utf8_lossy(first_line(&b)).trim().to_string())
        .filter(|e| !e.is_empty())
        .into_iter()
        .collect();
    let allow = read_small(&top.join(".claude").join("privacy-allow.txt"))
        .map(|text| text.lines().map(str::to_string).collect())
        .unwrap_or_default();
    ScanContext {
        home,
        handles,
        emails,
        hostname: anthology_core::sys::hostname().map(|h| h.to_string_lossy().into_owned()),
        banned: banned_names(&anthology_core::paths::anthology_home(env), repo),
        allow,
    }
}

/// The text of a regular file of at most `FILE_CAP` bytes; None for anything else.
fn read_small(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    // Not a FIFO or a device: reading one could block the hook until its timeout.
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    file.take(FILE_CAP + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > FILE_CAP {
        return None;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn first_line(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(bytes.len());
    &bytes[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calls(command: &str) -> Vec<CommitCall> {
        commit_calls(command, "/home/tester")
    }

    fn one(command: &str) -> CommitCall {
        let mut all = calls(command);
        assert_eq!(all.len(), 1, "{command}: {all:?}");
        all.remove(0)
    }

    #[test]
    fn options_before_the_subcommand_are_skipped_and_dash_c_adds_up() {
        let call = one(
            "git -c user.name=x --no-pager -P -C /a -C b --git-dir=/g --work-tree /w commit -m hi",
        );
        assert_eq!(call.dir, Path::new("/a/b"));
        assert_eq!(call.messages, ["hi"]);
        assert!(!call.all);
    }

    #[test]
    fn short_clusters_set_all_and_take_their_values() {
        let call = one("git commit -avm 'one' -m'two' -F msg.txt -S -uno -c HEAD");
        assert!(call.all);
        assert_eq!(call.messages, ["one", "two"]);
        assert_eq!(call.message_file.as_deref(), Some("msg.txt"));
        assert!(call.paths.is_empty(), "{call:?}");
        // `-u` and `-S` take the rest of the word, so the `a` in it is no `-a`
        assert!(!one("git commit -uall -m x").all);
        assert!(!one("git commit -S1a2b -m x").all);
    }

    #[test]
    fn long_options_and_paths() {
        let call = one(
            "git commit --all --message=a --message b --file=m.txt --author 'N <n@x.io>' --date x --cleanup strip -- p1 p2",
        );
        assert!(call.all);
        assert_eq!(call.messages, ["a", "b"]);
        assert_eq!(call.message_file.as_deref(), Some("m.txt"));
        assert_eq!(call.paths, ["p1", "p2"]);
        let call = one("git commit a.txt -m x b.txt");
        assert_eq!(call.paths, ["a.txt", "b.txt"]);
        assert!(one("git commit --dry-run").dry_run);
    }

    #[test]
    fn the_folder_comes_from_cd_and_dash_c() {
        let call = one("cd /repo && git -C sub commit -a");
        assert_eq!(call.dir, Path::new("/repo/sub"));
        assert_eq!(one("git commit").dir, Path::new(""));
        assert_eq!(one("cd /r && git commit").dir, Path::new("/r"));
        // several `-C` add up, below the folder of the `cd`; an absolute one starts again
        assert_eq!(
            one("cd /r && git -C a -C b commit").dir,
            Path::new("/r/a/b")
        );
        assert_eq!(one("git -C a -C b commit").dir, Path::new("a/b"));
        assert_eq!(
            one("cd /r && git -C /abs -C rel commit").dir,
            Path::new("/abs/rel")
        );
        assert_eq!(one("git -C ~/x -C /y commit").dir, Path::new("/y"));
        assert_eq!(
            one("git -C ~/x -C y commit").dir,
            Path::new("/home/tester/x/y")
        );
        // `-C <commit>` after `commit` is no folder, though the analysis reads it as one
        assert_eq!(one("cd /r && git commit -C HEAD -m x").dir, Path::new("/r"));
        assert_eq!(one("git commit -C HEAD").dir, Path::new(""));
        assert_eq!(
            one("cd /r && git -C s commit -C HEAD").dir,
            Path::new("/r/s")
        );
    }

    #[test]
    fn heredoc_bodies_are_found_by_their_delimiters() {
        let cmd = "git commit -m \"$(cat <<'EOF'\ntidy notes\n\nran in /x/y\nEOF\n)\"";
        assert_eq!(heredoc_bodies(cmd), ["tidy notes\n\nran in /x/y"]);
        // quoted and bare delimiters, `<<-` with tabs, two on one line, text after the end
        let cmd = "a <<EOF; b <<-\"END\"\n one\nEOF\n\ttwo\n\tEND\ngit commit";
        assert_eq!(heredoc_bodies(cmd), [" one", "two"]);
        // a heredoc that never ends, an arithmetic shift and a here-string are no bodies
        assert_eq!(heredoc_bodies("cat <<EOF\nno end"), Vec::<String>::new());
        assert_eq!(heredoc_bodies("echo $((1<<2))\nx\n"), Vec::<String>::new());
        assert_eq!(heredoc_bodies("cat <<< hi\nEOF\n"), Vec::<String>::new());
    }

    #[test]
    fn here_strings_are_read_with_their_quotes() {
        let cmd = r#"cat <<< 'a b' && x <<<"c \" d" y <<< bare; z <<<"#;
        assert_eq!(here_strings(cmd), ["a b", r#"c \" d"#, "bare"]);
        assert_eq!(here_strings("no operator"), Vec::<String>::new());
    }

    #[test]
    fn a_message_a_substitution_hides_is_read_from_the_heredoc() {
        let cmd = "git commit -m \"$(cat <<'EOF'\nran in /x/y\nEOF\n)\"";
        assert_eq!(one(cmd).messages, ["ran in /x/y"]);
        // `-F -` reads the heredoc or the here-string
        let cmd = "git commit -F - <<'EOF'\nran in /x/y\nEOF\n";
        assert_eq!(one(cmd).messages, ["ran in /x/y"]);
        assert_eq!(
            one("git commit -F - <<< 'ran in /x/y'").messages,
            ["ran in /x/y"]
        );
        // a plain message reads no heredoc
        let cmd = "cat <<EOF >f\nx\nEOF\ngit commit -m plain";
        assert_eq!(one(cmd).messages, ["plain"]);
    }

    #[test]
    fn folders_are_named_as_the_disk_has_them() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("Work").join("Proj");
        std::fs::create_dir_all(&real).unwrap();
        // the analysis writes `cd` folders in lowercase
        let lowered = tmp.path().join("work").join("proj");
        let got = exact_case(&lowered).unwrap();
        // a case-insensitive disk keeps the name it was given
        let same = |a: &Path, b: &Path| {
            a.to_string_lossy()
                .eq_ignore_ascii_case(&b.to_string_lossy())
        };
        assert!(same(&got, &real), "{got:?}");
        assert!(got.is_dir());
        assert_eq!(exact_case(&real).unwrap(), real);
        let back = exact_case(&real.join("..").join("proj")).unwrap();
        assert!(same(&back, &real), "{back:?}");
        assert_eq!(exact_case(&tmp.path().join("missing").join("x")), None);
    }

    #[test]
    fn other_git_commands_and_other_programs_are_no_commits() {
        for command in [
            "git status",
            "git log -m x",
            "git",
            "echo git commit",
            "git -C",
            "git commit-tree HEAD^{tree}",
        ] {
            assert_eq!(calls(command), Vec::new(), "{command}");
        }
        // two commits in one command
        assert_eq!(calls("git commit -m a && git -C x commit -m b").len(), 2);
    }

    #[test]
    fn the_switch_allows_and_a_panic_allows() {
        let env = Env::default().with("ANTHOLOGY_COMMIT_PRIVACY", "0");
        let raw = r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m x"},"cwd":"/"}"#;
        let out = hook_with(&env, raw);
        assert_eq!((out.code, out.stderr.len()), (0, 0));
        let out = guarded(|| panic!("boom"));
        assert_eq!((out.code, out.stdout.len(), out.stderr.len()), (0, 0, 0));
    }
}
