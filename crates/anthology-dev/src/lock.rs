//! `anthology-dev lock`: the two plugins' `anthology.lock` (GC-17, design §8.2, §8.5, §10).
//! Owner: RL1.
//!
//! - `lock write --tag TAG --checksums FILE [--repo OWNER/NAME] [--plugins-dir DIR]` pins a
//!   release: the version is `TAG` without `anthology-bin--v`, the four hashes come from the
//!   release's `checksums.txt` (`sha256sum anthology-*.tar.gz | LC_ALL=C sort -k2`). It writes
//!   `DIR/anthology/anthology.lock` and `DIR/anthology-guardrails/anthology.lock`, byte for byte
//!   the design §8.2 layout (`anthology_core::lock::render_anthology`).
//! - `lock check [--plugins-dir DIR]`: both locks exist, each passes
//!   `anthology_core::lock::parse` with the `anthology-bin--v` prefix (every GC-17 field and the
//!   layout), and the two are byte-identical.
//! - `lock check-fresh [--plugins-dir DIR]`: `lock check`, then the design §8.5 comparison of
//!   the shipped set between the lock's tag commit and `HEAD` ([`fresh`]).
//!
//! `DIR` defaults to `plugins` and, like every path here, is relative to the working directory
//! (`scripts/dev.sh` runs at the repository root). CO1 passes `--plugins-dir port/staging/plugins`.
//! Exit codes: 0 done, 1 failed, 2 usage.

pub mod fresh;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use anthology_core::lock::{self as core_lock, ANTHOLOGY_TAG_PREFIX, Lock};
use anthology_core::platform::Target;

use crate::Output;

/// The two plugins that ship `anthology.lock` (design §8.1).
pub const PLUGINS: [&str; 2] = ["anthology", "anthology-guardrails"];
/// The lock's file name inside a plugin.
pub const LOCK_FILE: &str = "anthology.lock";
/// The repository the release assets come from, unless `--repo` names another.
pub const DEFAULT_REPO: &str = "rbonweb/anthology";
/// GC-14's reason for any invalid anthology lock.
pub const INVALID: &str = "anthology.lock is not valid";

const USAGE: &str = "usage: anthology-dev lock write --tag TAG --checksums FILE [--repo OWNER/NAME] [--plugins-dir DIR]
       anthology-dev lock check [--plugins-dir DIR]
       anthology-dev lock check-fresh [--plugins-dir DIR]";

/// Runs `anthology-dev lock …`; `args` are the arguments after `lock`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let rest = args.get(1..).unwrap_or(&[]);
    match args.first().and_then(|a| a.to_str()) {
        Some("write") => match WriteArgs::parse(rest) {
            Ok(a) => finish("write", write(&a)),
            Err(e) => usage("write", &e),
        },
        Some("check") => match parse_plugins_dir(rest) {
            Ok(dir) => finish("check", check(&dir).map(|_| check_message(&dir))),
            Err(e) => usage("check", &e),
        },
        Some("check-fresh") => match parse_plugins_dir(rest) {
            Ok(dir) => finish("check-fresh", check_fresh(&dir)),
            Err(e) => usage("check-fresh", &e),
        },
        Some(other) => usage_line(&format!("unknown lock subcommand: {other}")),
        None => usage_line("lock needs one of: write, check, check-fresh"),
    }
}

fn finish(sub: &str, result: Result<String, String>) -> Output {
    match result {
        Ok(message) => Output {
            stdout: format!("lock {sub}: {message}\n").into_bytes(),
            ..Output::default()
        },
        Err(message) => Output {
            stderr: format!("anthology-dev: lock {sub}: {message}\n").into_bytes(),
            code: 1,
            ..Output::default()
        },
    }
}

fn usage(sub: &str, message: &str) -> Output {
    usage_line(&format!("lock {sub}: {message}"))
}

fn usage_line(message: &str) -> Output {
    Output {
        stderr: format!("anthology-dev: {message}\n{USAGE}\n").into_bytes(),
        code: 2,
        ..Output::default()
    }
}

// ------------------------------------------------------------------------------------------
// Arguments

/// `lock write` options.
#[derive(Debug)]
pub struct WriteArgs {
    pub tag: String,
    pub checksums: PathBuf,
    pub repo: String,
    pub plugins_dir: PathBuf,
}

fn os_string(value: OsString, what: &str) -> Result<String, String> {
    value
        .into_string()
        .map_err(|v| format!("{what} is not UTF-8: {}", v.to_string_lossy()))
}

impl WriteArgs {
    pub fn parse(args: &[OsString]) -> Result<WriteArgs, String> {
        use lexopt::prelude::*;
        let (mut tag, mut checksums, mut repo, mut dir) = (None, None, None, None);
        let mut parser = lexopt::Parser::from_args(args.iter().cloned());
        while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
            match arg {
                Long("tag") => {
                    tag = Some(os_string(
                        parser.value().map_err(|e| e.to_string())?,
                        "--tag",
                    )?)
                }
                Long("checksums") => {
                    checksums = Some(PathBuf::from(parser.value().map_err(|e| e.to_string())?))
                }
                Long("repo") => {
                    repo = Some(os_string(
                        parser.value().map_err(|e| e.to_string())?,
                        "--repo",
                    )?)
                }
                Long("plugins-dir") => {
                    dir = Some(PathBuf::from(parser.value().map_err(|e| e.to_string())?))
                }
                Value(v) => return Err(format!("unexpected argument: {}", v.to_string_lossy())),
                other => return Err(other.unexpected().to_string()),
            }
        }
        Ok(WriteArgs {
            tag: tag.ok_or("--tag is required")?,
            checksums: checksums.ok_or("--checksums is required")?,
            repo: repo.unwrap_or_else(|| DEFAULT_REPO.to_string()),
            plugins_dir: dir.unwrap_or_else(|| PathBuf::from("plugins")),
        })
    }
}

/// The only option of `check` and `check-fresh`.
fn parse_plugins_dir(args: &[OsString]) -> Result<PathBuf, String> {
    use lexopt::prelude::*;
    let mut dir = None;
    let mut parser = lexopt::Parser::from_args(args.iter().cloned());
    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("plugins-dir") => {
                dir = Some(PathBuf::from(parser.value().map_err(|e| e.to_string())?))
            }
            Value(v) => return Err(format!("unexpected argument: {}", v.to_string_lossy())),
            other => return Err(other.unexpected().to_string()),
        }
    }
    Ok(dir.unwrap_or_else(|| PathBuf::from("plugins")))
}

// ------------------------------------------------------------------------------------------
// write

/// The four digests of a release's `checksums.txt`: exactly one line
/// `<64 lowercase hex>  anthology-<target>.tar.gz` per target, in any order, and nothing else
/// (the final newline is optional).
pub fn parse_checksums(text: &str) -> Result<BTreeMap<Target, String>, String> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut out = BTreeMap::new();
    for (n, line) in body.split('\n').enumerate() {
        let bad = || {
            format!(
                "checksums line {}: expected `<sha256>  anthology-<target>.tar.gz`, got {line:?}",
                n + 1
            )
        };
        let (digest, name) = line.split_once("  ").ok_or_else(bad)?;
        if !is_hex64(digest) {
            return Err(bad());
        }
        let target = name
            .strip_prefix("anthology-")
            .and_then(|n| n.strip_suffix(".tar.gz"))
            .and_then(Target::from_triple)
            .ok_or_else(bad)?;
        if out.insert(target, digest.to_string()).is_some() {
            return Err(format!(
                "checksums line {}: a second line for {}",
                n + 1,
                target.triple()
            ));
        }
    }
    let missing: Vec<&str> = Target::ALL
        .iter()
        .filter(|t| !out.contains_key(t))
        .map(|t| t.triple())
        .collect();
    if !missing.is_empty() {
        return Err(format!("checksums has no line for {}", missing.join(", ")));
    }
    Ok(out)
}

/// `^[0-9a-f]{64}$`.
pub fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The lock for `tag`, `repo` and `sha256`, rendered and checked by `anthology_core::lock::parse`
/// (every GC-17 field and the layout) before anything is written.
pub fn render(tag: &str, repo: &str, sha256: BTreeMap<Target, String>) -> Result<String, String> {
    let version = tag
        .strip_prefix(ANTHOLOGY_TAG_PREFIX)
        .ok_or_else(|| format!("the tag must start with {ANTHOLOGY_TAG_PREFIX}, not {tag:?}"))?;
    let lock = Lock {
        version: version.to_string(),
        tag: tag.to_string(),
        repo: repo.to_string(),
        sha256,
    };
    let text = core_lock::render_anthology(&lock);
    core_lock::parse(&text, ANTHOLOGY_TAG_PREFIX)?;
    Ok(text)
}

/// Writes `text` to `path` through a temporary file in the same folder and a rename.
pub fn write_atomic(path: &Path, text: &[u8]) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp-{}", std::process::id()));
    let result = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("cannot write {}: {e}", path.display()));
    }
    Ok(())
}

/// The lock paths under `dir`; every plugin folder must exist.
fn lock_paths(dir: &Path) -> Result<Vec<PathBuf>, String> {
    PLUGINS
        .iter()
        .map(|p| {
            let plugin = dir.join(p);
            if plugin.is_dir() {
                Ok(plugin.join(LOCK_FILE))
            } else {
                Err(format!("no plugin folder {}", plugin.display()))
            }
        })
        .collect()
}

fn write(a: &WriteArgs) -> Result<String, String> {
    let text = std::fs::read_to_string(&a.checksums)
        .map_err(|e| format!("cannot read {}: {e}", a.checksums.display()))?;
    let digests = parse_checksums(&text)?;
    let lock = render(&a.tag, &a.repo, digests)?;
    let paths = lock_paths(&a.plugins_dir)?;
    for (i, path) in paths.iter().enumerate() {
        write_atomic(path, lock.as_bytes()).map_err(|e| {
            if i == 0 {
                e
            } else {
                format!("{e}; {} was already written, so the locks now differ: fix the cause and run lock write again", paths[0].display())
            }
        })?;
    }
    let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    Ok(format!("pinned {} in {}", a.tag, names.join(" and ")))
}

// ------------------------------------------------------------------------------------------
// check

/// Both locks under `dir` are valid anthology locks and byte-identical; returns the lock.
pub fn check(dir: &Path) -> Result<Lock, String> {
    let mut texts = Vec::new();
    for path in lock_paths(dir)? {
        let text =
            std::fs::read(&path).map_err(|e| format!("{}: {INVALID} ({e})", path.display()))?;
        let text = String::from_utf8(text)
            .map_err(|_| format!("{}: {INVALID} (not UTF-8)", path.display()))?;
        core_lock::parse(&text, ANTHOLOGY_TAG_PREFIX)
            .map_err(|e| format!("{}: {INVALID} ({e})", path.display()))?;
        texts.push((path, text));
    }
    let (first, rest) = texts.split_first().ok_or("no lock")?;
    for (path, text) in rest {
        if text != &first.1 {
            return Err(format!(
                "{} and {} differ; both plugins must pin the same binary (run lock write)",
                first.0.display(),
                path.display()
            ));
        }
    }
    core_lock::parse(&first.1, ANTHOLOGY_TAG_PREFIX)
}

fn check_message(dir: &Path) -> String {
    format!(
        "{} and {} are valid and identical",
        dir.join(PLUGINS[0]).join(LOCK_FILE).display(),
        dir.join(PLUGINS[1]).join(LOCK_FILE).display()
    )
}

fn check_fresh(dir: &Path) -> Result<String, String> {
    let lock = check(dir)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let root = fresh::toplevel(Path::new("."))?;
    match fresh::compare(&root, &lock.tag, &cargo)? {
        fresh::Verdict::Fresh { commit } => Ok(format!(
            "the shipped set is unchanged since {} ({commit})",
            lock.tag
        )),
        fresh::Verdict::Stale { commit, changed } => Err(format!(
            "the shipped set changed since {} ({commit}):\n  {}\n\
             release a new binary (tag anthology-bin--v<next>), then run lock write",
            lock.tag,
            changed.join("\n  ")
        )),
    }
}

#[cfg(test)]
mod tests;
