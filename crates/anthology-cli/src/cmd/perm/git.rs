//! The git half of the path check: a line with `git` in it is allowed only when nothing git can
//! reach from the working directory can run a program, or read outside the roots.
//!
//! Git itself is asked where the repository is and what its configuration holds, from the same
//! working directory and with the same environment, so the repository is found the way git finds
//! it (`.git` files, bare repositories, worktrees, `safe.directory`) and the configuration is read
//! the way git reads it (system, global, repository and worktree files, `include` and `includeIf`).
//! Only `rev-parse` and `config --list` run, and neither runs a configured program. The git that
//! runs is the one `PATH` finds, resolved, and it must lie outside every root, the working
//! directory and every `cd` target (the caller checks that). Then:
//!
//! - Every `GIT_*` environment variable refuses (`GIT_DIR`, `GIT_CONFIG_*`, `GIT_EXTERNAL_DIFF`,
//!   `GIT_PAGER`, ...), whatever its value.
//! - Every configuration key must be on the allowlist in [`safe_key`]; any other key refuses, so
//!   a partial clone (`remote.*.promisor`, `extensions.*`), `core.fsmonitor`, `core.pager`,
//!   `core.hooksPath`, `diff.external`, `diff.*.textconv`, `filter.*`, `alias.*`,
//!   `credential.helper`, `core.gitProxy`, `protocol.*` and every key not yet known refuse. One
//!   exception: `credential.helper=osxkeychain` from Apple's own system file, told apart by the
//!   file's path ([`APPLE_GITCONFIGS`]). A `.gitattributes` file only names drivers; with no driver
//!   in the configuration it runs nothing.
//! - The object store must resolve under a root, with no symlink in it or its `pack` folder.
//! - The git dir, the common dir and the work tree must lie under a root and match no `Read()`
//!   rule (an anchored rule that could match below one of them counts) or secret pattern. Each is
//!   one of the paths the answer lists.
//! - No hook may be installed (any file in the hooks folder but `*.sample`): writing the index,
//!   as `git status` does, runs `post-index-change`.
//! - No `objects/info/alternates` (objects read from elsewhere), no submodule (`.gitmodules`, a
//!   `modules` folder, or a gitlink in the index: git runs `status` inside a populated submodule
//!   with that repository's configuration).
//! - The index is read: every tracked path is held against the `Read()` rules and the secret
//!   patterns, since `git diff` and `blame` print tracked content. A split or sparse index
//!   refuses. History is not searched: a secret that was committed and deleted is not seen, which
//!   is why every form that prints history refuses before this (`words`).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_shell::is_secret_path;

use super::paths::Checker;
use super::shown;

/// How long one git call may take.
const GIT_TIMEOUT: Duration = Duration::from_secs(2);
/// The most output one git call may print.
const MAX_OUTPUT: usize = 4 << 20;
/// The largest index that is read, and its most entries.
const MAX_INDEX_BYTES: u64 = 64 << 20;
const MAX_INDEX_ENTRIES: u32 = 100_000;

/// The system configuration files of Apple's own git (Command Line Tools and Xcode). Each sets
/// `credential.helper=osxkeychain`, which is allowed from these files and from no other.
pub const APPLE_GITCONFIGS: &[&str] = &[
    "/Library/Developer/CommandLineTools/usr/share/git-core/gitconfig",
    "/Applications/Xcode.app/Contents/Developer/usr/share/git-core/gitconfig",
];

/// Checks the repository git finds from `cwd` (resolved). `git` is the resolved git program, which
/// lies outside every root; `env` is the environment the command runs with, as far as this process
/// can see it.
pub fn check(ck: &mut Checker, git: &Path, cwd: &Path, env: &Env) -> Result<(), String> {
    for (key, _) in env.iter() {
        let key = key.to_string_lossy();
        if key.starts_with("GIT_") {
            return Err(format!("git environment variable {}", shown(&key)));
        }
    }
    // Both git calls only read, so they run side by side: one spawn less on the wait.
    let (out, listing) = std::thread::scope(|scope| {
        let listing = scope.spawn(|| {
            run_git(
                git,
                cwd,
                env,
                &["config", "--list", "--includes", "--show-origin", "-z"],
            )
        });
        let out = run_git(
            git,
            cwd,
            env,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--is-bare-repository",
                "--is-inside-work-tree",
                "--absolute-git-dir",
                "--git-common-dir",
                "--git-path",
                "index",
                "--git-path",
                "hooks",
                "--git-path",
                "objects/info/alternates",
                "--git-path",
                "objects",
                "--show-cdup",
            ],
        );
        let listing = listing
            .join()
            .unwrap_or_else(|_| Err("the git config call failed".into()));
        (out, listing)
    });
    let out = out.map_err(|why| format!("git rev-parse: {why}"))?;
    let text = String::from_utf8(out).map_err(|_| "git rev-parse: not UTF-8".to_string())?;
    let lines: Vec<&str> = text.lines().collect();
    let repo = Repo::from_lines(&lines, cwd)?;

    let listing = listing.map_err(|why| format!("git config: {why}"))?;
    config(&listing)?;

    let mut dirs = vec![&repo.git_dir, &repo.common_dir];
    dirs.extend(repo.top.as_ref());
    for dir in dirs {
        let real = std::fs::canonicalize(dir)
            .map_err(|_| format!("git: cannot resolve {}", shown(&dir.to_string_lossy())))?;
        let name = shown(&dir.to_string_lossy());
        if !ck.inside(&real) {
            return Err(format!("outside every root: git repository {name}"));
        }
        // Git reads the whole repository, and `show`, `diff`, `log -p` and `blame` print it: an
        // anchored rule that could match anything below refuses.
        for path in [dir.as_path(), real.as_path()] {
            if ck.rules.denied(path) || ck.rules.anchored_reaches(path) {
                return Err(format!("Read rule matches: git repository {name}"));
            }
            if is_secret_path(&path.to_string_lossy()) {
                return Err(format!("path of a secret: git repository {name}"));
            }
        }
        ck.record(&real);
    }
    hooks(&repo.hooks)?;
    if std::fs::symlink_metadata(&repo.alternates).is_ok() {
        return Err("git alternates: objects are read from another repository".into());
    }
    objects(ck, &repo.objects)?;
    for modules in [
        repo.git_dir.join("modules"),
        repo.common_dir.join("modules"),
    ] {
        if std::fs::symlink_metadata(&modules).is_ok() {
            return Err("submodule: the git dir has a modules folder".into());
        }
    }
    if let Some(top) = &repo.top
        && std::fs::symlink_metadata(top.join(".gitmodules")).is_ok()
    {
        return Err("submodule: .gitmodules".into());
    }
    index(ck, &repo)
}

/// Where git found the repository.
struct Repo {
    git_dir: PathBuf,
    common_dir: PathBuf,
    index: PathBuf,
    hooks: PathBuf,
    alternates: PathBuf,
    objects: PathBuf,
    /// The work tree, or `None` for a bare repository.
    top: Option<PathBuf>,
}

/// Holds every entry of `config --list --includes --show-origin -z` (`origin NUL key LF value NUL`,
/// or `origin NUL key NUL` for a key with no value) against the allowlist.
fn config(listing: &[u8]) -> Result<(), String> {
    let mut fields = listing.split(|b| *b == 0);
    while let Some(origin) = fields.next() {
        if origin.is_empty() {
            // The listing ends with a NUL, which leaves one empty field.
            if fields.next().is_none() {
                return Ok(());
            }
            return Err("git config: an entry with no origin".into());
        }
        let entry = fields.next().ok_or("git config: an origin with no entry")?;
        let (key, value) = match entry.iter().position(|b| *b == b'\n') {
            Some(at) => (&entry[..at], Some(&entry[at + 1..])),
            None => (entry, None),
        };
        let origin = String::from_utf8_lossy(origin);
        let key = String::from_utf8_lossy(key);
        if !safe_key(&key) && !apple_keychain(&origin, &key, value) {
            return Err(format!(
                "git config key not on the allowlist: {}",
                shown(&key)
            ));
        }
    }
    Ok(())
}

/// Apple's git sets `credential.helper=osxkeychain` in its own system file. It is told apart by
/// the path of the file it comes from, never by scope: a repository, global or included file
/// setting the same key and value refuses like any other helper.
fn apple_keychain(origin: &str, key: &str, value: Option<&[u8]>) -> bool {
    key == "credential.helper"
        && value == Some(b"osxkeychain".as_slice())
        && origin
            .strip_prefix("file:")
            .is_some_and(|path| APPLE_GITCONFIGS.contains(&path))
}

/// The object store must resolve under a root, and no entry of it or of its `pack` folder may be
/// a symlink: git reads objects through one as readily as through `alternates`.
fn objects(ck: &Checker, objects: &Path) -> Result<(), String> {
    let real = std::fs::canonicalize(objects)
        .map_err(|_| "git objects: the object store cannot be resolved".to_string())?;
    if !ck.inside(&real) {
        return Err(format!(
            "git objects outside every root: {}",
            shown(&objects.to_string_lossy())
        ));
    }
    let unlisted = || "git objects: the object store cannot be listed".to_string();
    for dir in [real.clone(), real.join("pack")] {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && dir != real => continue,
            Err(_) => return Err(unlisted()),
        };
        for entry in entries {
            let entry = entry.map_err(|_| unlisted())?;
            if entry.file_type().map_err(|_| unlisted())?.is_symlink() {
                return Err(format!(
                    "git objects: a symlink in the object store: {}",
                    shown(&entry.file_name().to_string_lossy())
                ));
            }
        }
    }
    Ok(())
}

impl Repo {
    fn from_lines(lines: &[&str], cwd: &Path) -> Result<Repo, String> {
        let absolute = |line: &str| -> Result<PathBuf, String> {
            let path = PathBuf::from(line);
            if path.is_absolute() {
                Ok(path)
            } else {
                Err("git rev-parse: a path that is not absolute".into())
            }
        };
        let fixed = |i: usize| absolute(lines.get(i).copied().unwrap_or(""));
        // Eight answers, then `--show-cdup`, which a bare repository leaves out or empty.
        let top = match (lines.first().copied(), lines.get(1).copied(), lines.get(8)) {
            (Some("true"), Some("false"), None | Some(&"")) if lines.len() <= 9 => None,
            (Some("false"), Some("true"), Some(cdup)) if lines.len() == 9 => Some(
                std::fs::canonicalize(cwd.join(cdup))
                    .map_err(|_| "git rev-parse: the work tree cannot be resolved".to_string())?,
            ),
            _ => return Err("git rev-parse: neither a work tree nor a bare repository".into()),
        };
        Ok(Repo {
            git_dir: fixed(2)?,
            common_dir: fixed(3)?,
            index: fixed(4)?,
            hooks: fixed(5)?,
            alternates: fixed(6)?,
            objects: fixed(7)?,
            top,
        })
    }
}

/// Whether `key` (as `git config --list` prints it: section and name in lower case) is one that
/// cannot run a program, fetch, or make git read a file it would not read anyway.
pub fn safe_key(key: &str) -> bool {
    let Some((section, rest)) = key.split_once('.') else {
        return false;
    };
    let (sub, name) = match rest.rsplit_once('.') {
        Some((sub, name)) => (Some(sub), name),
        None => (None, rest),
    };
    match (section, sub, name) {
        (
            "core",
            None,
            "repositoryformatversion"
            | "bare"
            | "filemode"
            | "ignorecase"
            | "precomposeunicode"
            | "logallrefupdates"
            | "symlinks"
            | "autocrlf"
            | "eol"
            | "safecrlf"
            | "quotepath",
        ) => true,
        ("user" | "author" | "committer", None, "name" | "email") => true,
        ("init", None, "defaultbranch") => true,
        ("remote", Some(_), "url" | "pushurl" | "fetch") => true,
        // The keys git knows under a branch (remote, merge, rebase, pushremote, description,
        // mergeoptions) name no program; tools add their own (vscode-merge-base).
        ("branch", Some(_), _) => true,
        // `config --list` lists what an include brings in, and each of those keys is checked.
        ("include", None, "path") | ("includeif", Some(_), "path") => true,
        ("safe", None, "directory") => true,
        ("color", _, _) | ("advice", None, _) => true,
        ("pull", None, "rebase" | "ff") => true,
        ("push", None, "default" | "autosetupremote") => true,
        ("fetch", None, "prune") => true,
        ("diff", None, "renames" | "algorithm") => true,
        _ => false,
    }
}

fn hooks(dir: &Path) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("git hook: the hooks folder cannot be listed".into()),
    };
    for entry in entries {
        let entry = entry.map_err(|_| "git hook: the hooks folder cannot be listed".to_string())?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".sample") {
            return Err(format!("git hook: {}", shown(&name)));
        }
    }
    Ok(())
}

/// Reads the index and holds every tracked path against the rules and the secret patterns.
fn index(ck: &Checker, repo: &Repo) -> Result<(), String> {
    let meta = match std::fs::metadata(&repo.index) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("index: cannot be read".into()),
    };
    if meta.len() > MAX_INDEX_BYTES {
        return Err("index: too large to check".into());
    }
    let Some(top) = &repo.top else {
        return Err("index: a bare repository with an index".into());
    };
    let data = std::fs::read(&repo.index).map_err(|_| "index: cannot be read".to_string())?;
    for_each_entry(&data, |mode, name| {
        let name = String::from_utf8_lossy(name);
        match mode & 0o170000 {
            0o160000 => return Err(format!("submodule: gitlink {}", shown(&name))),
            0o040000 => return Err("index: a sparse directory entry".into()),
            _ => {}
        }
        if is_secret_path(&name) {
            return Err(format!("path of a secret: tracked {}", shown(&name)));
        }
        // `top` is resolved; the rules are anchored both as given and resolved.
        if ck.rules.denied(&top.join(name.as_ref())) {
            return Err(format!("Read rule matches: tracked {}", shown(&name)));
        }
        Ok(())
    })
}

fn be32(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Calls `f` with the mode and path of every entry of an index file (versions 2, 3 and 4, SHA-1).
/// Anything it cannot read, a split index (`link`) and a sparse index (`sdir`) are errors.
fn for_each_entry(
    data: &[u8],
    mut f: impl FnMut(u32, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let bad = || "index: cannot be read".to_string();
    if data.len() < 32 || &data[..4] != b"DIRC" {
        return Err(bad());
    }
    let version = be32(data, 4).ok_or_else(bad)?;
    let count = be32(data, 8).ok_or_else(bad)?;
    if !(2..=4).contains(&version) {
        return Err(bad());
    }
    if count > MAX_INDEX_ENTRIES {
        return Err("index: too many entries to check".into());
    }
    let end = data.len() - 20;
    let mut at = 12;
    let mut previous: Vec<u8> = Vec::new();
    for _ in 0..count {
        let mode = be32(data, at + 24).ok_or_else(bad)?;
        let flags = data.get(at + 60..at + 62).ok_or_else(bad)?;
        let flags = u16::from_be_bytes([flags[0], flags[1]]);
        let mut name_at = at + 62;
        if flags & 0x4000 != 0 {
            if version < 3 {
                return Err(bad());
            }
            name_at += 2;
        }
        let name: Vec<u8>;
        let next;
        if version == 4 {
            let (strip, used) = varint(data.get(name_at..end).ok_or_else(bad)?).ok_or_else(bad)?;
            let suffix_at = name_at + used;
            let nul = data[suffix_at..end]
                .iter()
                .position(|b| *b == 0)
                .ok_or_else(bad)?;
            let keep = previous.len().checked_sub(strip).ok_or_else(bad)?;
            let mut full = previous[..keep].to_vec();
            full.extend_from_slice(&data[suffix_at..suffix_at + nul]);
            name = full;
            next = suffix_at + nul + 1;
        } else {
            let nul = data
                .get(name_at..end)
                .ok_or_else(bad)?
                .iter()
                .position(|b| *b == 0)
                .ok_or_else(bad)?;
            let length = usize::from(flags & 0x0fff);
            if length != 0x0fff && length != nul {
                return Err(bad());
            }
            name = data[name_at..name_at + nul].to_vec();
            next = at + ((name_at - at + nul + 8) & !7);
        }
        if name.is_empty() || next > end {
            return Err(bad());
        }
        f(mode, &name)?;
        previous = name;
        at = next;
    }
    while at < end {
        let signature = data.get(at..at + 4).ok_or_else(bad)?;
        let size = be32(data, at + 4).ok_or_else(bad)? as usize;
        match signature {
            b"link" => return Err("index: a split index".into()),
            b"sdir" => return Err("index: a sparse index".into()),
            _ => {}
        }
        at = at
            .checked_add(8 + size)
            .filter(|n| *n <= end)
            .ok_or_else(bad)?;
    }
    Ok(())
}

/// Git's offset varint (index v4): the value and the bytes it used.
fn varint(data: &[u8]) -> Option<(usize, usize)> {
    let mut used = 0;
    let mut byte = *data.get(used)?;
    used += 1;
    let mut value = usize::from(byte & 0x7f);
    while byte & 0x80 != 0 {
        value = value.checked_add(1)?;
        byte = *data.get(used)?;
        used += 1;
        value = value
            .checked_mul(128)?
            .checked_add(usize::from(byte & 0x7f))?;
    }
    Some((value, used))
}

/// Runs `git args` in `cwd` with exactly `env`, no stdin and a time limit. Its stdout, or why
/// not: a failed exit, any stderr (a warning such as an ignored repository), a timeout.
fn run_git(git: &Path, cwd: &Path, env: &Env, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(git);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env.iter() {
        cmd.env(key, value);
    }
    let mut child = cmd.spawn().map_err(|_| "cannot run git".to_string())?;
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(pipe) = pipe {
                let _ = pipe.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < GIT_TIMEOUT => {
                std::thread::sleep(Duration::from_micros(500));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
        }
    };
    let out = stdout.join().map_err(|_| "no output".to_string())?;
    let err = stderr.join().map_err(|_| "no output".to_string())?;
    if !status.success() || !err.is_empty() {
        let line = String::from_utf8_lossy(&err);
        let line = line.lines().next().unwrap_or("failed");
        return Err(shown(line));
    }
    if out.len() > MAX_OUTPUT {
        return Err("too much output".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_splits_the_subsection_at_the_first_and_last_dot() {
        for key in [
            "core.bare",
            "remote.a.b.url",
            "branch.feature.x.merge",
            "branch.main.vscode-merge-base",
            "includeif.gitdir:/a.b/.path",
            "color.diff.meta",
            "user.email",
        ] {
            assert!(safe_key(key), "{key}");
        }
        for key in [
            "core.fsmonitor",
            "core.pager",
            "core.hookspath",
            "core.worktree",
            "core.sshcommand",
            "core.gitproxy",
            "core.attributesfile",
            "remote.origin.promisor",
            "remote.origin.uploadpack",
            "remote.origin.vcs",
            "remote.url",
            "extensions.partialclone",
            "diff.external",
            "diff.c.textconv",
            "filter.lfs.process",
            "alias.st",
            "credential.helper",
            "credential.https://x.helper",
            "protocol.allow",
            "log.showsignature",
            "gpg.program",
            "submodule.recurse",
            "include.x.path",
            "color",
            "nodot",
        ] {
            assert!(!safe_key(key), "{key}");
        }
    }

    #[test]
    fn osxkeychain_is_allowed_only_from_apples_own_file() {
        let apple = "file:/Library/Developer/CommandLineTools/usr/share/git-core/gitconfig";
        let xcode = "file:/Applications/Xcode.app/Contents/Developer/usr/share/git-core/gitconfig";
        let listing = |origin: &str, entry: &str| format!("{origin}\0{entry}\0").into_bytes();
        for origin in [apple, xcode] {
            assert!(config(&listing(origin, "credential.helper\nosxkeychain")).is_ok());
            assert!(config(&listing(origin, "credential.helper\nstore")).is_err());
            assert!(config(&listing(origin, "credential.helper")).is_err());
            assert!(config(&listing(origin, "credential.x.helper\nosxkeychain")).is_err());
        }
        for origin in [
            "file:.git/config",
            "file:/etc/gitconfig",
            "file:/Library/Developer/CommandLineTools/usr/share/git-core/gitconfig.d/x",
            "command line:",
            "/Library/Developer/CommandLineTools/usr/share/git-core/gitconfig",
        ] {
            assert!(
                config(&listing(origin, "credential.helper\nosxkeychain")).is_err(),
                "{origin}"
            );
        }
        assert!(config(b"").is_ok());
        assert!(config(&listing("file:x", "core.bare\nfalse")).is_ok());
        assert!(config(b"file:x\0core.bare\nfalse").is_ok(), "no final NUL");
        assert!(config(b"file:x\0core.bare\nfalse\0\0core.pager\0").is_err());
    }

    fn index_bytes(version: u32, names: &[(&str, u32)]) -> Vec<u8> {
        let mut out = b"DIRC".to_vec();
        out.extend(version.to_be_bytes());
        out.extend((names.len() as u32).to_be_bytes());
        let mut previous = String::new();
        for (name, mode) in names {
            let start = out.len();
            out.extend([0u8; 24]);
            out.extend(mode.to_be_bytes());
            out.extend([0u8; 12 + 20]);
            out.extend((name.len() as u16).to_be_bytes());
            if version == 4 {
                out.push(previous.len() as u8); // strip everything: a one-byte varint
                out.extend(name.as_bytes());
                out.push(0);
            } else {
                out.extend(name.as_bytes());
                let len = out.len() - start;
                out.extend(std::iter::repeat_n(0, 8 - len % 8));
            }
            previous = name.to_string();
        }
        out.extend([0u8; 20]);
        out
    }

    #[test]
    fn index_entries_of_each_version_are_read() {
        for version in [2, 3, 4] {
            let data = index_bytes(version, &[("a.txt", 0o100644), ("dir/b", 0o120000)]);
            let mut seen = Vec::new();
            for_each_entry(&data, |mode, name| {
                seen.push((mode, String::from_utf8_lossy(name).into_owned()));
                Ok(())
            })
            .unwrap();
            assert_eq!(
                seen,
                [
                    (0o100644, "a.txt".to_string()),
                    (0o120000, "dir/b".to_string())
                ],
                "v{version}"
            );
        }
    }

    #[test]
    fn a_damaged_or_unusual_index_is_an_error() {
        let good = index_bytes(2, &[("a.txt", 0o100644)]);
        let ok = |data: &[u8]| for_each_entry(data, |_, _| Ok(()));
        assert!(ok(&good).is_ok());
        assert!(ok(&good[..good.len() - 25]).is_err(), "cut short");
        let mut count = good.clone();
        count[11] = 2;
        assert!(ok(&count).is_err(), "more entries than bytes");
        let mut link = good[..good.len() - 20].to_vec();
        link.extend(b"link");
        link.extend(0u32.to_be_bytes());
        link.extend([0u8; 20]);
        assert_eq!(ok(&link), Err("index: a split index".into()));
        let mut v5 = good.clone();
        v5[7] = 5;
        assert!(ok(&v5).is_err());
    }
}
