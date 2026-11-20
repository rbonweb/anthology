//! `anthology-dev rtk-trim` (scripts/rtk_trim.py). Owner: DV4.
//!
//! Trims the `rtk/` subtree to the Apache-2.0, Claude-Code-only edition: an ordered list of
//! operations ([`operations`]) over four idempotent primitives, every one anchored on exact text
//! so an upstream refactor of a trimmed region stops the trim instead of half-applying it.
//!
//! * `rm(paths...)`: delete a file or directory if present.
//! * `sub(path, old, new, done)`: replace the single occurrence of `old`.
//! * `cut(path, old, gone)`: delete the single occurrence of `old`.
//! * `drop(path, signature)`: delete the item whose first line is exactly `signature`.
//!
//! The Python stays the oracle until CO2; this port does the same operations in the same order
//! with the same messages, stdout, stderr and exit code (the tests and the acceptance runs
//! compare them). Differences, all deliberate: the default `--root` is `rtk` relative to the
//! working directory, usage errors exit 2 with their own wording, an I/O failure prints
//! `rtk-trim: <path>: <io error>` and exits 1, and `--lock-guard` has the `../crates/` allowlist.
//!
//! Usage:
//!
//! ```text
//! anthology-dev rtk-trim [--root DIR]                 apply the trim to DIR (default: rtk)
//! anthology-dev rtk-trim --lock-guard BEFORE AFTER [--root DIR]
//! anthology-dev rtk-trim --trimmed-conflicts          `git status --porcelain` on stdin
//! anthology-dev rtk-trim --removed-paths
//! ```

pub mod decl;
mod lockfile;
mod operations;
pub mod pystr;

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

use crate::Output;

pub use lockfile::{
    GuardReport, Pair, allowed_packages, lock_guard, lock_guard_allowlist, lock_packages,
    lock_packages_text, report_lines,
};
pub use operations::operations;

/// An anchor that no longer matches, or a file the trim cannot read or write: the trim stops
/// (exit 1). The text is what follows `rtk-trim: `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimError(pub String);

impl fmt::Display for TrimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TrimError {}

/// `<path>: <io error>`: what an unreadable file or a failing remove prints (Python would die
/// with a traceback, also with status 1).
fn io_error(path: &str, error: &std::io::Error) -> TrimError {
    TrimError(format!("{path}: {error}"))
}

/// One operation of the trim, as the generated table spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op<'a> {
    /// `t.rm(*paths)`.
    Rm(&'a [&'a str]),
    /// `t.sub(path, old, new, done=None)`.
    Sub {
        path: &'a str,
        old: &'a str,
        new: &'a str,
        done: Option<&'a str>,
    },
    /// `t.cut(path, old, gone)`.
    Cut {
        path: &'a str,
        old: &'a str,
        gone: &'a str,
    },
    /// `t.drop(path, sig, gone=None)`.
    Drop {
        path: &'a str,
        sig: &'a str,
        gone: Option<&'a str>,
    },
}

/// The primitives, applied under `root` (the `rtk/` directory).
#[derive(Debug)]
pub struct Trim {
    pub root: PathBuf,
    /// What changed, in order: `rm <path>`, `edit <path>`, `drop <path> <signature>`.
    pub changed: Vec<String>,
}

impl Trim {
    pub fn new(root: impl Into<PathBuf>) -> Trim {
        Trim {
            root: root.into(),
            changed: Vec::new(),
        }
    }

    fn fail<T>(&self, path: &str, reason: &str, anchor: &str) -> Result<T, TrimError> {
        Err(TrimError(format!(
            "{path}: {reason}: {}",
            pystr::repr(pystr::first_line(anchor))
        )))
    }

    fn read(&self, path: &str, anchor: &str) -> Result<String, TrimError> {
        let full = self.root.join(path);
        if !full.is_file() {
            return self.fail(path, "file missing", anchor);
        }
        // UTF-8, no newline translation.
        fs::read_to_string(&full).map_err(|e| io_error(&full.display().to_string(), &e))
    }

    fn write(&self, path: &str, text: &str) -> Result<(), TrimError> {
        let full = self.root.join(path);
        fs::write(&full, text).map_err(|e| io_error(&full.display().to_string(), &e))
    }

    /// Deletes each path: a real directory recursively, anything else that exists (a symlink,
    /// even a broken one) by unlinking it.
    pub fn rm(&mut self, paths: &[&str]) -> Result<(), TrimError> {
        for path in paths {
            let full = self.root.join(path);
            let removed = if full.is_dir() && !full.is_symlink() {
                fs::remove_dir_all(&full)
            } else if fs::symlink_metadata(&full).is_ok() {
                fs::remove_file(&full)
            } else {
                continue;
            };
            removed.map_err(|e| io_error(&full.display().to_string(), &e))?;
            self.changed.push(format!("rm {path}"));
        }
        Ok(())
    }

    /// Replaces the single occurrence of `old` by `new`. Already applied when `old` is absent
    /// and `done` (default: `new`) is present; otherwise the anchor is missing and the trim stops.
    pub fn sub(
        &mut self,
        path: &str,
        old: &str,
        new: &str,
        done: Option<&str>,
    ) -> Result<(), TrimError> {
        if done.is_none() && old.contains(new) {
            return self.fail(
                path,
                "bad anchor design, `new` occurs inside `old` (add trailing context)",
                old,
            );
        }
        let text = self.read(path, old)?;
        let marker = done.unwrap_or(new);
        let count = text.matches(old).count();
        let applied = new.contains(old) && text.contains(new);
        if count == 1 && !applied {
            self.write(path, &text.replace(old, new))?;
            self.changed.push(format!("edit {path}"));
            return Ok(());
        }
        if count > 1 && !applied {
            return self.fail(path, &format!("anchor occurs {count} times"), old);
        }
        if !marker.is_empty() && text.contains(marker) {
            return Ok(());
        }
        self.fail(path, "anchor missing", old)
    }

    /// Deletes the single occurrence of `old`. Already applied when the `gone` token is absent;
    /// `old` absent but `gone` present stops the trim.
    pub fn cut(&mut self, path: &str, old: &str, gone: &str) -> Result<(), TrimError> {
        let text = self.read(path, old)?;
        let count = text.matches(old).count();
        if count == 1 {
            self.write(path, &text.replace(old, ""))?;
            self.changed.push(format!("edit {path}"));
            return Ok(());
        }
        if count > 1 {
            return self.fail(path, &format!("anchor occurs {count} times"), old);
        }
        if text.contains(gone) {
            return self.fail(
                path,
                &format!("anchor missing but {} still present", pystr::repr(gone)),
                old,
            );
        }
        Ok(())
    }

    /// Deletes the item whose first line is exactly `sig` (with its indentation), the
    /// `#[...]`/`///`/`//` lines directly above it, through the first line that is `<indent>}`
    /// (or `},` `});` `};`). Body-tolerant. Already applied when `<keyword> <name>` (or `gone`)
    /// no longer occurs; the name still present but not as that exact line stops the trim.
    pub fn drop_item(
        &mut self,
        path: &str,
        sig: &str,
        gone: Option<&str>,
    ) -> Result<(), TrimError> {
        let text = self.read(path, sig)?;
        let mut lines: Vec<&str> = text.split('\n').collect();
        let hits: Vec<usize> = (0..lines.len()).filter(|&i| lines[i] == sig).collect();
        if hits.is_empty() {
            let (present, token) = match gone {
                None => {
                    let Some((keyword, name)) = decl::first_declaration(sig) else {
                        return self.fail(path, "drop needs gone= for this signature", sig);
                    };
                    (
                        decl::declaration_present(&text, keyword, name),
                        format!("{keyword} {name}"),
                    )
                }
                Some(gone) => (text.contains(gone), gone.to_string()),
            };
            if present {
                return self.fail(
                    path,
                    &format!(
                        "signature changed? {} present but not as this exact line",
                        pystr::repr(&token)
                    ),
                    sig,
                );
            }
            return Ok(());
        }
        if hits.len() > 1 {
            return self.fail(path, &format!("signature occurs {} times", hits.len()), sig);
        }
        let i = hits[0];
        let indent = &sig[..sig.len() - pystr::lstrip(sig).len()];
        let deeper = format!("{indent} ");
        let mut start = i;
        while start > 0 {
            let prev = lines[start - 1];
            let stripped = pystr::strip(prev);
            if prev.starts_with(indent)
                && !prev.starts_with(&deeper)
                && (stripped.starts_with("#[")
                    || stripped.starts_with("///")
                    || stripped.starts_with("//"))
            {
                start -= 1;
            } else {
                break;
            }
        }
        let mut end;
        if pystr::rstrip(sig).ends_with([';', ',']) {
            end = i;
        } else {
            let closers = [
                format!("{indent}}}"),
                format!("{indent}}},"),
                format!("{indent}}});"),
                format!("{indent}}};"),
            ];
            let mut found = None;
            for (j, line) in lines.iter().enumerate().skip(i + 1) {
                if closers.iter().any(|c| c == line) {
                    found = Some(j);
                    break;
                }
                if !pystr::strip(line).is_empty() && !line.starts_with(indent) {
                    return self.fail(path, "item has no closing brace at its indentation", sig);
                }
            }
            let Some(j) = found else {
                return self.fail(path, "no end for item", sig);
            };
            end = j;
        }
        // Drop one adjacent blank line, so no double blank line remains.
        if start > 0
            && pystr::strip(lines[start - 1]).is_empty()
            && end + 1 < lines.len()
            && pystr::strip(lines[end + 1]).is_empty()
        {
            end += 1;
        }
        lines.drain(start..=end);
        self.write(path, &lines.join("\n"))?;
        self.changed
            .push(format!("drop {path} {}", pystr::strip(sig)));
        Ok(())
    }

    /// Applies one operation.
    pub fn apply(&mut self, op: &Op<'_>) -> Result<(), TrimError> {
        match *op {
            Op::Rm(paths) => self.rm(paths),
            Op::Sub {
                path,
                old,
                new,
                done,
            } => self.sub(path, old, new, done),
            Op::Cut { path, old, gone } => self.cut(path, old, gone),
            Op::Drop { path, sig, gone } => self.drop_item(path, sig, gone),
        }
    }

    /// Applies `ops` in order; the first failure stops the trim.
    pub fn apply_all(&mut self, ops: &[Op<'_>]) -> Result<(), TrimError> {
        ops.iter().try_for_each(|op| self.apply(op))
    }

    /// Applies the whole trim ([`operations`]).
    pub fn run_operations(&mut self) -> Result<(), TrimError> {
        self.apply_all(&operations())
    }
}

/// Every path (relative to `rtk/`) the trim removes, in order, duplicates kept.
pub fn removed_paths() -> Vec<&'static str> {
    operations()
        .into_iter()
        .filter_map(|op| match op {
            Op::Rm(paths) => Some(paths),
            _ => None,
        })
        .flat_map(|paths| paths.iter().copied())
        .collect()
}

/// The `DU` (deleted by us, modified by them) paths of a `git status --porcelain` text that lie
/// inside a path the trim removes: re-running the trim would delete them again, so a sync can
/// resolve them by deletion. Quoted (unusual) paths are left for a human.
pub fn trimmed_conflicts(status: &str, removed: &[&str], prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in pystr::splitlines(status) {
        let Some(path) = line.strip_prefix("DU ") else {
            continue;
        };
        if path.starts_with('"') {
            continue;
        }
        let Some(rel) = path.strip_prefix(prefix) else {
            continue;
        };
        let inside = removed.iter().any(|p| {
            rel == *p
                || rel
                    .strip_prefix(p)
                    .is_some_and(|rest| rest.starts_with('/'))
        });
        if inside {
            out.push(path.to_string());
        }
    }
    out
}

// ------------------------------------------------------------------------------------------
// The command line

const USAGE: &str = "usage: anthology-dev rtk-trim [--root DIR]
       anthology-dev rtk-trim --lock-guard BEFORE AFTER [--root DIR]
       anthology-dev rtk-trim --trimmed-conflicts
       anthology-dev rtk-trim --removed-paths
";

/// The default `--root`: the `rtk` directory of the working directory.
const DEFAULT_ROOT: &str = "rtk";

enum Mode {
    Trim,
    LockGuard(PathBuf, PathBuf),
    TrimmedConflicts,
    RemovedPaths,
}

struct Args {
    root: PathBuf,
    mode: Mode,
    help: bool,
}

fn parse(args: &[OsString]) -> Result<Args, String> {
    use lexopt::prelude::*;
    let mut parsed = Args {
        root: PathBuf::from(DEFAULT_ROOT),
        mode: Mode::Trim,
        help: false,
    };
    let mut mode_set = false;
    let mut set_mode = |mode: Mode, parsed: &mut Args, flag: &str| {
        if mode_set {
            return Err(format!("{flag}: not allowed with another mode"));
        }
        mode_set = true;
        parsed.mode = mode;
        Ok(())
    };
    let mut parser = lexopt::Parser::from_args(args.iter().cloned());
    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("root") => parsed.root = parser.value().map_err(|e| e.to_string())?.into(),
            Long("lock-guard") => {
                let before = parser.value().map_err(|e| e.to_string())?;
                let after = parser.value().map_err(|e| e.to_string())?;
                set_mode(
                    Mode::LockGuard(before.into(), after.into()),
                    &mut parsed,
                    "--lock-guard",
                )?;
            }
            Long("trimmed-conflicts") => {
                set_mode(Mode::TrimmedConflicts, &mut parsed, "--trimmed-conflicts")?;
            }
            Long("removed-paths") => {
                set_mode(Mode::RemovedPaths, &mut parsed, "--removed-paths")?;
            }
            Long("help") | Short('h') => parsed.help = true,
            Value(v) => return Err(format!("unexpected argument: {}", v.to_string_lossy())),
            other => return Err(other.unexpected().to_string()),
        }
    }
    Ok(parsed)
}

fn fail(error: &TrimError) -> Output {
    Output {
        stderr: format!("rtk-trim: {error}\n").into_bytes(),
        code: 1,
        ..Output::default()
    }
}

fn lines_out(lines: &[String]) -> Output {
    let mut stdout = lines.join("\n").into_bytes();
    if !lines.is_empty() {
        stdout.push(b'\n');
    }
    Output {
        stdout,
        ..Output::default()
    }
}

/// Python's `main` for the parsed arguments.
fn execute(args: &Args, stdin: &mut dyn Read) -> Result<Output, TrimError> {
    match &args.mode {
        Mode::RemovedPaths => {
            let paths = removed_paths();
            Ok(Output {
                stdout: format!("{}\n", paths.join("\n")).into_bytes(),
                ..Output::default()
            })
        }
        Mode::TrimmedConflicts => {
            let mut bytes = Vec::new();
            stdin
                .read_to_end(&mut bytes)
                .map_err(|e| io_error("stdin", &e))?;
            let status = String::from_utf8(bytes).map_err(|e| TrimError(format!("stdin: {e}")))?;
            let paths = trimmed_conflicts(&status, &removed_paths(), "rtk/");
            Ok(lines_out(&paths))
        }
        Mode::LockGuard(before, after) => {
            let report = lock_guard_allowlist(before, after, &args.root)?;
            Ok(lines_out(&report_lines(&report)))
        }
        Mode::Trim => {
            let mut trim = Trim::new(&args.root);
            if !trim.root.is_dir() {
                return Err(TrimError(format!(
                    "{}: not a directory: ''",
                    args.root.display()
                )));
            }
            trim.run_operations()?;
            if trim.changed.is_empty() {
                Ok(lines_out(&["no changes".to_string()]))
            } else {
                Ok(lines_out(&trim.changed))
            }
        }
    }
}

/// Runs `anthology-dev rtk-trim …`; `args` are the arguments after `rtk-trim`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            return Output {
                stderr: format!("anthology-dev rtk-trim: {message}\n{USAGE}").into_bytes(),
                code: 2,
                ..Output::default()
            };
        }
    };
    if parsed.help {
        return Output {
            stdout: USAGE.as_bytes().to_vec(),
            ..Output::default()
        };
    }
    match execute(&parsed, stdin) {
        Ok(output) => output,
        Err(error) => fail(&error),
    }
}
