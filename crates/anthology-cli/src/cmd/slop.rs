//! `anthology slop hook|check|rules`: the slop gate (plan
//! docs/superpowers/plans/2026-11-28-anti-slop-gate.md, Task 7).
//!
//! - `hook`: stdin is the Claude Code PreToolUse JSON. Exit 0 allows (no output); exit
//!   `HOOK_BLOCK` blocks, with the message on stderr. Every failure, and a panic, allows: the
//!   gate fails open, and only `HOOK_BLOCK` refuses an edit.
//! - `check [--root DIR] [--since REF] [PATH...]`: one line per unwaived finding, then one per
//!   waiver in the new text. Exit 0 none unwaived, 1 some, 2 a usage or git error.
//! - `rules`: one tab-separated line per rule.

use std::collections::{BTreeMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::{IsTerminal, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use anthology_core::Env;
use anthology_core::git::{Git, RealGit};
use anthology_pycompat::json::{self, Map, Value};
use anthology_slop::edit::{self, ToolEdit};
use anthology_slop::{MAX_DOC, Span, paths, report};

use super::Output;

/// The exit code of `slop hook` that blocks the edit; the hook script turns it into exit 2.
pub const HOOK_BLOCK: i32 = 10;

/// Runs `anthology slop …`; `args` are the arguments after `slop`.
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
        Some("check") => match std::env::current_dir() {
            Ok(cwd) => check_with(&Env::from_process(), &RealGit, &cwd, &args[1..]),
            Err(_) => check_error("cannot read the current folder"),
        },
        Some("rules") => Output {
            stdout: report::rules_table().into_bytes(),
            ..Output::default()
        },
        Some(other) => Output::usage(&format!("unknown command: slop {other}")),
        None => Output::usage("slop needs one of: hook, check, rules"),
    }
}

/// `f()`, or an allow (exit 0, no output) when it panics.
fn guarded(f: impl FnOnce() -> Output) -> Output {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_default()
}

/// The hook on its input text: exit 0 with no output, or `HOOK_BLOCK` with the message.
fn hook_with(env: &Env, raw: &str) -> Output {
    match refusal(env, raw) {
        Some(message) => Output {
            stderr: message.into_bytes(),
            code: HOOK_BLOCK,
            ..Output::default()
        },
        None => Output::default(),
    }
}

/// The refusal message for this tool call, or None to allow it. Every step that cannot be
/// completed allows.
fn refusal(env: &Env, raw: &str) -> Option<String> {
    if env.get_os("ANTHOLOGY_SLOP_GATE") == Some(OsStr::new("0")) {
        return None;
    }
    let input = json::loads(raw).ok()?;
    let top = input.as_object()?;
    let tool = top.get("tool_name")?.as_str()?;
    if !matches!(tool, "Write" | "Edit" | "MultiEdit") {
        return None;
    }
    let tool_input = top.get("tool_input")?.as_object()?;
    let given = Path::new(tool_input.get("file_path")?.as_str()?);
    let kind = anthology_slop::kind_of(given)?;
    let cwd = top.get("cwd").and_then(Value::as_str).map(Path::new);
    // `..` is resolved first, so `.claude/../docs/a.md` is read as the docs file it is.
    let path = absolute(given, cwd)?;
    let root = paths::repo_root(&path);
    let config = normalise(&anthology_core::paths::config_dir(env));
    if paths::exempt(&path, root.as_deref(), Some(&config)) {
        return None;
    }
    let edit = tool_edit(tool, tool_input)?;
    let before = read_doc(&path)?;
    let (after, new) = edit::apply(before.as_deref(), &edit)?;
    if after.len() > MAX_DOC {
        return None;
    }
    let findings: Vec<_> = anthology_slop::check(kind, &after, &new)
        .into_iter()
        .filter(|f| f.waiver.is_none())
        .collect();
    if findings.is_empty() {
        return None;
    }
    let shown = paths::shown(&path, root.as_deref());
    Some(report::hook_message(edit.tool(), &shown, kind, &findings))
}

/// The edit a Write, Edit or MultiEdit input describes; None when a field is missing or has the
/// wrong type. `replace_all` may be missing or null (false).
fn tool_edit(tool: &str, input: &Map) -> Option<ToolEdit> {
    fn text(m: &Map, key: &str) -> Option<String> {
        m.get(key)?.as_str().map(str::to_string)
    }
    fn flag(m: &Map) -> Option<bool> {
        match m.get("replace_all") {
            None => Some(false),
            Some(v) if v.is_null() => Some(false),
            Some(v) => v.as_bool(),
        }
    }
    match tool {
        "Write" => Some(ToolEdit::Write {
            content: text(input, "content")?,
        }),
        "Edit" => Some(ToolEdit::Edit {
            old: text(input, "old_string")?,
            new: text(input, "new_string")?,
            replace_all: flag(input)?,
        }),
        "MultiEdit" => {
            let edits = input
                .get("edits")?
                .as_array()?
                .iter()
                .map(|e| {
                    let e = e.as_object()?;
                    Some((text(e, "old_string")?, text(e, "new_string")?, flag(e)?))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(ToolEdit::MultiEdit { edits })
        }
        _ => None,
    }
}

/// `path` made absolute against `cwd` and normalised; None when it is relative and there is no
/// absolute `cwd`.
fn absolute(path: &Path, cwd: Option<&Path>) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(normalise(path));
    }
    let cwd = cwd.filter(|c| c.is_absolute())?;
    Some(normalise(&cwd.join(path)))
}

/// `path` with `.` dropped and `..` taking off the folder before it, without reading the disk.
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(".."),
            },
            other => out.push(other),
        }
    }
    out
}

/// The text of the file at `path`: `Some(None)` when it is missing, and None when it cannot be
/// read, is not a regular file, is not UTF-8 or is over `MAX_DOC` bytes.
fn read_doc(path: &Path) -> Option<Option<String>> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(None),
        Err(_) => return None,
    };
    // Not a FIFO or a device: reading one could block the hook until its timeout.
    if !meta.is_file() || meta.len() > MAX_DOC as u64 {
        return None;
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_DOC as u64 + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() > MAX_DOC {
        return None;
    }
    String::from_utf8(buf).ok().map(Some)
}

/// What `check` was asked for.
struct CheckArgs {
    root: Option<PathBuf>,
    since: Option<String>,
    paths: Vec<PathBuf>,
}

fn parse_check(args: &[OsString]) -> Result<CheckArgs, String> {
    use lexopt::prelude::*;
    let mut p = lexopt::Parser::from_args(args.iter().cloned());
    let mut out = CheckArgs {
        root: None,
        since: None,
        paths: Vec::new(),
    };
    while let Some(arg) = p.next().map_err(|e| e.to_string())? {
        match arg {
            Long("root") => out.root = Some(PathBuf::from(p.value().map_err(|e| e.to_string())?)),
            Long("since") => {
                let value = p.value().map_err(|e| e.to_string())?;
                let text = value
                    .into_string()
                    .map_err(|_| "--since needs a git ref".to_string())?;
                // A ref is passed to git as an argument, so it must not read as an option.
                if text.is_empty() || text.starts_with('-') {
                    return Err(format!("--since needs a git ref, not '{text}'"));
                }
                out.since = Some(text);
            }
            Value(path) => out.paths.push(PathBuf::from(path)),
            other => return Err(other.unexpected().to_string()),
        }
    }
    match (&out.since, out.paths.is_empty()) {
        (None, true) => Err("check needs a PATH or --since REF".to_string()),
        (Some(_), false) => Err("--since takes no PATH".to_string()),
        _ => Ok(out),
    }
}

/// `check` in `cwd` (relative `--root` and PATHs are read against it; PATHs against `--root`
/// when it is given), with `git` for `--since`.
fn check_with(env: &Env, git: &dyn Git, cwd: &Path, args: &[OsString]) -> Output {
    let args = match parse_check(args) {
        Ok(args) => args,
        Err(message) => return check_error(&message),
    };
    let base = normalise(&cwd.join(args.root.as_deref().unwrap_or(Path::new("."))));
    let config = normalise(&anthology_core::paths::config_dir(env));
    let files = match &args.since {
        Some(since) => match since_files(env, git, &base, since, &config) {
            Ok(files) => files,
            Err(message) => return check_error(&message),
        },
        None => match path_files(&base, &args.paths, &config) {
            Ok(files) => files,
            Err(message) => return check_error(&message),
        },
    };
    let mut stdout = String::new();
    let mut unwaived = false;
    for file in &files {
        let Some(kind) = anthology_slop::kind_of(&file.path) else {
            continue;
        };
        let Some(Some(doc)) = read_doc(&file.path) else {
            continue;
        };
        let new = match &file.lines {
            None => vec![Span {
                start: 0,
                end: doc.len(),
            }],
            Some(ranges) => edit::line_spans(&doc, ranges),
        };
        for f in anthology_slop::check(kind, &doc, &new) {
            if f.waiver.is_none() {
                unwaived = true;
                push_line(&mut stdout, &report::check_line(&file.shown, &f));
            }
        }
        let mut waivers = anthology_slop::waivers_in(kind, &doc, &new);
        waivers.sort_by_key(|w| w.line);
        for w in &waivers {
            push_line(&mut stdout, &report::waiver_line(&file.shown, w));
        }
    }
    Output {
        stdout: stdout.into_bytes(),
        code: i32::from(unwaived),
        ..Output::default()
    }
}

/// One file to check: its path, how it is shown, and its new lines (1-based inclusive ranges;
/// None: the whole file).
struct CheckFile {
    path: PathBuf,
    shown: String,
    lines: Option<Lines>,
}

/// Line ranges of a file, `(first, last)`, 1-based and inclusive.
type Lines = Vec<(usize, usize)>;

/// `line` and a newline, with control characters written as escapes: an excerpt or a waiver
/// reason is text from the file, and must not reach a terminal as an escape sequence.
fn push_line(out: &mut String, line: &str) {
    out.push_str(&escape_controls(line));
    out.push('\n');
}

fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Exit 2 with one line on stderr.
fn check_error(message: &str) -> Output {
    Output {
        stderr: format!("anthology slop: {}\n", escape_controls(message)).into_bytes(),
        code: 2,
        ..Output::default()
    }
}

/// The files the PATHs name, in order, each once: a file as is, a folder walked in name order
/// without following links and without its exempt folders. Exempt files are left out.
fn path_files(base: &Path, args: &[PathBuf], config: &Path) -> Result<Vec<CheckFile>, String> {
    let mut found = Vec::new();
    for arg in args {
        let path = normalise(&base.join(arg));
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => walk(&path, config, &mut found),
            Ok(_) => found.push(path),
            Err(_) => {
                return Err(format!("no such file or folder: {}", arg.to_string_lossy()));
            }
        }
    }
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for path in found {
        if anthology_slop::kind_of(&path).is_none() || !seen.insert(path.clone()) {
            continue;
        }
        let root = paths::repo_root(&path);
        if paths::exempt(&path, root.as_deref(), Some(config)) {
            continue;
        }
        let shown = paths::shown(&path, Some(root.as_deref().unwrap_or(base)));
        files.push(CheckFile {
            path,
            shown,
            lines: None,
        });
    }
    Ok(files)
}

fn walk(dir: &Path, config: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if !paths::exempt(&path, paths::repo_root(&path).as_deref(), Some(config)) {
                walk(&path, config, out);
            }
        } else if kind.is_file() {
            out.push(path);
        }
    }
}

/// The files changed between `git merge-base REF HEAD` and the working tree, with their added
/// lines, and the untracked files whole; in path order, exempt files left out.
fn since_files(
    env: &Env,
    git: &dyn Git,
    base: &Path,
    since: &str,
    config: &Path,
) -> Result<Vec<CheckFile>, String> {
    let top = git_out(env, git, base, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(OsStr::from_bytes(first_line(&top)));
    if !top.is_absolute() {
        return Err("git rev-parse --show-toplevel gave no folder".to_string());
    }
    let merge_base = git_out(env, git, &top, &["merge-base", since, "HEAD"])?;
    let merge_base = String::from_utf8_lossy(first_line(&merge_base)).into_owned();
    let diff = git_out(
        env,
        git,
        &top,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--unified=0",
            // A config default above 0 would turn the lines between two hunks into hunk lines.
            "--inter-hunk-context=0",
            // Fixed prefixes, whatever diff.noprefix or diff.mnemonicPrefix say.
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "-M",
            &merge_base,
            "--",
        ],
    )?;
    let untracked = git_out(
        env,
        git,
        &top,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut changed: BTreeMap<Vec<u8>, Option<Lines>> = BTreeMap::new();
    for (rel, ranges) in added_ranges(&diff) {
        changed.insert(rel, Some(ranges));
    }
    for rel in untracked.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        changed.insert(rel.to_vec(), None);
    }
    let mut files = Vec::new();
    for (rel, lines) in changed {
        let path = normalise(&top.join(OsStr::from_bytes(&rel)));
        if anthology_slop::kind_of(&path).is_none()
            || paths::exempt(&path, Some(&top), Some(config))
        {
            continue;
        }
        let shown = paths::shown(&path, Some(&top));
        files.push(CheckFile { path, shown, lines });
    }
    Ok(files)
}

/// Git's stdout, or the error line for a git that did not run or failed.
fn git_out(env: &Env, git: &dyn Git, cwd: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let what = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
    match git.run(env, cwd, args) {
        Some(out) if out.code == 0 => Ok(out.stdout),
        Some(out) => {
            let why = String::from_utf8_lossy(first_line(&out.stderr)).into_owned();
            Err(format!("git {what} failed: {why}"))
        }
        None => Err(format!("git {what} did not run")),
    }
}

fn first_line(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(bytes.len());
    &bytes[..end]
}

/// The added line ranges of each file in a `git diff --unified=0` output: `(path below the
/// top, [(first, last)])`, 1-based and inclusive. A deleted file (`+++ /dev/null`) has none.
/// Hunk bodies are counted out from their headers, so an added line that starts with `++` is
/// never read as a file header.
fn added_ranges(diff: &[u8]) -> Vec<(Vec<u8>, Lines)> {
    let mut out: Vec<(Vec<u8>, Lines)> = Vec::new();
    let mut current = false;
    let (mut old_left, mut new_left) = (0usize, 0usize);
    for line in diff.split(|&b| b == b'\n') {
        if old_left > 0 || new_left > 0 {
            match line.first() {
                Some(b'-') => old_left = old_left.saturating_sub(1),
                Some(b'+') => new_left = new_left.saturating_sub(1),
                Some(b' ') => {
                    old_left = old_left.saturating_sub(1);
                    new_left = new_left.saturating_sub(1);
                }
                // `\ No newline at end of file`
                Some(b'\\') => {}
                // Not a body line: the hunk was shorter than its header said.
                _ => {
                    old_left = 0;
                    new_left = 0;
                }
            }
            if old_left > 0
                || new_left > 0
                || matches!(line.first(), Some(b'-' | b'+' | b' ' | b'\\'))
            {
                continue;
            }
        }
        if line.starts_with(b"diff --git ") {
            current = false;
        } else if let Some(name) = line.strip_prefix(b"+++ ") {
            current = false;
            if let Some(rel) = new_name(name) {
                out.push((rel, Vec::new()));
                current = true;
            }
        } else if line.starts_with(b"@@ ") {
            let Some((old, new)) = hunk_counts(line) else {
                continue;
            };
            old_left = old.1;
            new_left = new.1;
            if current
                && new.1 > 0
                && let Some((_, ranges)) = out.last_mut()
            {
                ranges.push((new.0, new.0 + new.1 - 1));
            }
        }
    }
    out
}

/// The path in a `+++ ` line without its `b/` prefix: C-quoted names are unquoted, and the tab
/// git adds after a name with a space is dropped. None for `/dev/null` or another shape.
fn new_name(name: &[u8]) -> Option<Vec<u8>> {
    let name = if name.first() == Some(&b'"') {
        unquote(name)?
    } else {
        name.strip_suffix(b"\t").unwrap_or(name).to_vec()
    };
    let rel = name.strip_prefix(b"b/")?;
    (!rel.is_empty()).then(|| rel.to_vec())
}

/// A C-quoted git path (`"b/na\303\257ve.md"`) as its bytes; a trailing tab after the closing
/// quote is allowed.
fn unquote(quoted: &[u8]) -> Option<Vec<u8>> {
    let body = quoted.strip_suffix(b"\t").unwrap_or(quoted);
    let body = body.strip_prefix(b"\"")?.strip_suffix(b"\"")?;
    let mut out = Vec::with_capacity(body.len());
    let mut i = 0;
    while i < body.len() {
        let b = body[i];
        i += 1;
        if b != b'\\' {
            out.push(b);
            continue;
        }
        let esc = *body.get(i)?;
        i += 1;
        out.push(match esc {
            b'a' => 7,
            b'b' => 8,
            b't' => b'\t',
            b'n' => b'\n',
            b'v' => 11,
            b'f' => 12,
            b'r' => b'\r',
            b'0'..=b'3' => {
                let digits = body.get(i..i + 2)?;
                if !digits.iter().all(|d| (b'0'..=b'7').contains(d)) {
                    return None;
                }
                i += 2;
                (esc - b'0') * 64 + (digits[0] - b'0') * 8 + (digits[1] - b'0')
            }
            other => other,
        });
    }
    Some(out)
}

/// `(start, count)` of the old and new sides of `@@ -a[,b] +c[,d] @@`; a missing count is 1.
fn hunk_counts(line: &[u8]) -> Option<((usize, usize), (usize, usize))> {
    let text = std::str::from_utf8(line).ok()?;
    let mut parts = text.split(' ');
    parts.next()?;
    let old = side(parts.next()?.strip_prefix('-')?)?;
    let new = side(parts.next()?.strip_prefix('+')?)?;
    Some((old, new))
}

fn side(range: &str) -> Option<(usize, usize)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_in_the_hook_allows() {
        let out = guarded(|| panic!("boom"));
        assert_eq!(out.code, 0);
        assert!(out.stdout.is_empty() && out.stderr.is_empty());
    }

    #[test]
    fn the_switch_allows_before_the_input_is_read() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.md");
        let raw = format!(
            r#"{{"tool_name":"Write","tool_input":{{"file_path":"{}","content":"We delve into it."}}}}"#,
            file.display()
        );
        let env = Env::default().with_os("HOME", tmp.path().join("user"));
        let out = hook_with(&env.clone().with("ANTHOLOGY_SLOP_GATE", "0"), &raw);
        assert_eq!((out.code, out.stderr.len()), (0, 0));
        let out = hook_with(&env, &raw);
        assert_eq!(
            out.code,
            HOOK_BLOCK,
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn normalise_resolves_dots_without_the_disk() {
        for (path, want) in [
            ("/r/.claude/../docs/./a.md", "/r/docs/a.md"),
            ("/../a.md", "/a.md"),
            ("/r/a/b/../../c.md", "/r/c.md"),
            ("a/../../b.md", "../b.md"),
            ("./a.md", "a.md"),
        ] {
            assert_eq!(normalise(Path::new(path)), Path::new(want), "{path}");
        }
        assert_eq!(absolute(Path::new("a.md"), None), None);
        assert_eq!(absolute(Path::new("a.md"), Some(Path::new("rel"))), None);
        assert_eq!(
            absolute(Path::new("../a.md"), Some(Path::new("/r/docs"))),
            Some(PathBuf::from("/r/a.md"))
        );
    }

    #[test]
    fn hunks_are_counted_out_so_body_lines_are_never_headers() {
        let diff = b"diff --git a/a.md b/a.md\n\
index 1..2 100644\n\
--- a/a.md\n\
+++ b/a.md\n\
@@ -1,0 +2,2 @@ heading\n\
+++ b/fake.md\n\
+@@ -1 +9,9 @@\n\
@@ -5 +7 @@\n\
-old\n\
+new\n\
@@ -9,2 +10,0 @@\n\
--- gone\n\
-gone too\n\
diff --git a/gone.md b/gone.md\n\
--- a/gone.md\n\
+++ /dev/null\n\
@@ -1 +0,0 @@\n\
-x\n\
diff --git a/c.md b/c.md\n\
--- a/c.md\n\
+++ b/c.md\n\
@@ -1 +1 @@\n\
-x\n\
\\ No newline at end of file\n\
+y\n";
        assert_eq!(
            added_ranges(diff),
            [
                (b"a.md".to_vec(), vec![(2, 3), (7, 7)]),
                (b"c.md".to_vec(), vec![(1, 1)]),
            ]
        );
    }

    #[test]
    fn quoted_and_spaced_names_are_read() {
        assert_eq!(
            new_name(b"b/with space.md\t"),
            Some(b"with space.md".to_vec())
        );
        assert_eq!(
            new_name(b"\"b/na\\303\\257ve.md\""),
            Some("na\u{ef}ve.md".as_bytes().to_vec())
        );
        assert_eq!(
            new_name(b"\"b/q\\\"t\\\\\\t.md\"\t"),
            Some(b"q\"t\\\t.md".to_vec())
        );
        assert_eq!(new_name(b"/dev/null"), None);
        assert_eq!(new_name(b"a.md"), None);
        assert_eq!(new_name(b"\"b/bad\\"), None);
        assert_eq!(new_name(b"\"b/bad\\39x\""), None);
    }

    #[test]
    fn hunk_counts_default_to_one() {
        assert_eq!(hunk_counts(b"@@ -3 +4 @@"), Some(((3, 1), (4, 1))));
        assert_eq!(hunk_counts(b"@@ -3,2 +4,0 @@ fn x"), Some(((3, 2), (4, 0))));
        assert_eq!(hunk_counts(b"@@ nonsense @@"), None);
    }

    #[test]
    fn check_arguments_are_checked() {
        let args = |list: &[&str]| -> Vec<OsString> { list.iter().map(OsString::from).collect() };
        assert!(parse_check(&args(&[])).is_err());
        assert!(parse_check(&args(&["--since", "HEAD", "a.md"])).is_err());
        assert!(parse_check(&args(&["--since=-x"])).is_err());
        assert!(parse_check(&args(&["--since", ""])).is_err());
        let ok = parse_check(&args(&["--root", "r", "--since=main"])).unwrap();
        assert_eq!(ok.root, Some(PathBuf::from("r")));
        assert_eq!(ok.since.as_deref(), Some("main"));
        let ok = parse_check(&args(&["a.md", "docs"])).unwrap();
        assert_eq!(ok.paths, [PathBuf::from("a.md"), PathBuf::from("docs")]);
    }
}
