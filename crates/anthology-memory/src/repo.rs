//! Repo identity and note folders (MS1): a port of `memory_sync`'s `repo_root`, `_main_root`,
//! `normalize_remote`, `repo_key`, `legacy_dirs`, `note_sources` and `_unsafe_dir` (plan §3.6).
//!
//! Git runs through the [`Git`] seam (GC-19; `memory_sync._git`): `RealGit` is
//! `git -c core.fsmonitor=false …` with the 10 s timeout and the `GIT_DIR` family removed.
//! `_git_out` is [`git_out`]: exit 0 and non-empty stripped stdout, else `None`.
//!
//! Python reads `os.environ` in `expanduser` and `os.getcwd()`; here `expanduser` reads `env`
//! (callers pass the process environment, as Python's callers do).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anthology_core::Env;
use anthology_core::git::Git;
use anthology_pycompat::path::{normpath_bytes, realpath};
use anthology_pycompat::re::{self, Regex};
use anthology_pycompat::str::{lower, split_ws, strip_ws};

use crate::notes::py_name_key;

/// `memory_sync.config_dir(env)` and `anthology_home(env)` (GC-1).
pub use anthology_core::paths::{anthology_home, config_dir};

/// `memory_sync._DEFAULT_PORTS`.
const DEFAULT_PORTS: [&str; 3] = ["22", "443", "80"];

/// `_git_out(root, *args)`: git's stdout (UTF-8 with replacement, stripped) when it exits 0
/// and prints something; `None` otherwise or when git cannot run.
pub fn git_out(git: &dyn Git, env: &Env, cwd: &Path, args: &[&str]) -> Option<String> {
    let out = git.run(env, cwd, args)?;
    if out.code != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let text = strip_ws(&text);
    (!text.is_empty()).then(|| text.to_string())
}

/// `memory_sync.repo_root(cwd)`: the work tree root (realpath) containing `cwd`, or `None`
/// when `cwd` is not in a git repo.
pub fn repo_root(cwd: &Path, env: &Env, git: &dyn Git) -> Option<PathBuf> {
    let top = git_out(git, env, cwd, &["rev-parse", "--show-toplevel"])?;
    Some(realpath(Path::new(&top)))
}

/// `memory_sync._main_root(root)`: the main work tree's root, so linked worktrees map to the
/// same place as the clone.
pub fn main_root(root: &Path, env: &Env, git: &dyn Git) -> PathBuf {
    let common = git_out(
        git,
        env,
        root,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(String::into_bytes)
    .or_else(|| {
        git_out(git, env, root, &["rev-parse", "--git-common-dir"])
            .map(|rel| join_bytes(root.as_os_str().as_bytes(), rel.as_bytes()))
    });
    if let Some(common) = common {
        let norm = normpath_bytes(&common);
        if basename(&norm) == b".git" {
            return realpath(Path::new(OsStr::from_bytes(dirname(&norm))));
        }
    }
    match git_out(git, env, root, &["rev-parse", "--show-toplevel"]) {
        Some(top) => realpath(Path::new(&top)),
        None => realpath(root),
    }
}

/// `memory_sync.normalize_remote(url, base)`: a stable key for a git remote URL:
/// `host[:port]/owner/repo` lowercased, credentials and default ports dropped, `.git` and
/// trailing `/` removed; a local path remote becomes `path:<realpath>` (resolved against
/// `base`, or the current folder when `None`; `~` expanded with `env`'s `HOME`).
pub fn normalize_remote(url: &str, base: Option<&Path>, env: &Env) -> String {
    let u = strip_ws(url);
    let mut path: Option<&str> = None;
    let mut key = String::new();
    if lower(u).starts_with("file://") {
        path = Some(after_chars(u, 7));
    } else if let Some((_, rest)) = u.split_once("://") {
        let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
        let (mut host, mut port) = (authority, "");
        if authority.matches(':').count() == 1 {
            (host, port) = authority.split_once(':').expect("one colon");
        }
        if DEFAULT_PORTS.contains(&port) {
            port = "";
        }
        key = host.to_string();
        if !port.is_empty() {
            key.push(':');
            key.push_str(port);
        }
        key.push('/');
        key.push_str(tail);
    } else if ["/", "./", "../", "~"].iter().any(|p| u.starts_with(p)) {
        path = Some(u);
    } else {
        match scp_match(u) {
            Some((host, rest)) => key = format!("{host}/{rest}"),
            None => path = Some(u),
        }
    }
    if let Some(path) = path {
        let base: Vec<u8> = match base {
            Some(b) => b.as_os_str().as_bytes().to_vec(),
            None => std::env::current_dir()
                .map(|d| d.into_os_string().into_vec())
                .unwrap_or_default(),
        };
        let joined = join_bytes(&base, &expanduser(path, env));
        let full = realpath(Path::new(OsStr::from_bytes(&joined)));
        let mut full = full.into_os_string().into_vec();
        if full.ends_with(b".git") {
            full.truncate(full.len() - 4);
        }
        return format!("path:{}", String::from_utf8_lossy(&full));
    }
    let key = collapse_slashes(&key);
    let mut key = key.trim_end_matches('/');
    if let Some(k) = key.strip_suffix(".git") {
        key = k;
    }
    lower(key.trim_end_matches('/'))
}

/// `memory_sync.repo_key(root)`: the repo's normalised remote URL (`origin`, else the first
/// remote by name), else `path:<realpath of the main work tree>`.
pub fn repo_key(root: &Path, env: &Env, git: &dyn Git) -> String {
    let names = git_out(git, env, root, &["remote"])
        .map(|out| {
            let mut names: Vec<String> = split_ws(&out).into_iter().map(String::from).collect();
            names.sort();
            names
        })
        .unwrap_or_default();
    if let Some(first) = names.first() {
        let name = if names.iter().any(|n| n == "origin") {
            "origin"
        } else {
            first.as_str()
        };
        let config_key = format!("remote.{name}.url");
        let url = git_out(git, env, root, &["remote", "get-url", name])
            .or_else(|| git_out(git, env, root, &["config", "--get", &config_key]));
        if let Some(url) = url {
            return normalize_remote(&url, Some(root), env);
        }
    }
    format!("path:{}", main_root(root, env, git).to_string_lossy())
}

/// `memory_sync.legacy_dirs(root, config)`: the old path-keyed auto-memory folders
/// (`<config>/projects/<encoded path>/memory`) of the repo's work tree and its main work tree,
/// deduplicated, in that order (they may not exist).
pub fn legacy_dirs(root: &Path, config: &Path, env: &Env, git: &dyn Git) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for base in [root.to_path_buf(), main_root(root, env, git)] {
        let base = realpath(&base);
        if seen.contains(&base) {
            continue;
        }
        out.push(
            config
                .join("projects")
                .join(encode_project_path(&base))
                .join("memory"),
        );
        seen.push(base);
    }
    out
}

/// `memory_sync._encode_project_path(path)`: every code point of the realpath that is not an
/// ASCII letter or digit becomes `-` (a byte that is not UTF-8 counts as one, as Python's
/// surrogate escape makes it one code point).
pub fn encode_project_path(path: &Path) -> String {
    let real = realpath(path);
    let mut out = String::new();
    for chunk in real.as_os_str().as_bytes().utf8_chunks() {
        for c in chunk.valid().chars() {
            out.push(if c.is_ascii_alphanumeric() { c } else { '-' });
        }
        out.extend(std::iter::repeat_n('-', chunk.invalid().len()));
    }
    out
}

/// One note folder for the indexer: `scope` is `repo`, `agent` or `personal`; `agent` is the
/// agent folder's name (`directory.name`, the indexer's id prefix) for `agent`, else `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteSource {
    pub scope: String,
    pub dir: PathBuf,
    pub agent: Option<String>,
}

/// `memory_sync.note_sources(root, home)`: the existing note folders: repo, each agent (by
/// name), personal. Symlinked folders are never listed (ADR-020).
pub fn note_sources(root: &Path, home: &Path) -> Vec<NoteSource> {
    note_sources_opt(Some(root), home)
}

/// [`note_sources`] with Python's `root=None`: personal memory only.
pub fn note_sources_opt(root: Option<&Path>, home: &Path) -> Vec<NoteSource> {
    let mut out = Vec::new();
    if let Some(root) = root {
        let repo = root.join(".claude").join("memory");
        if real_dir(&repo) {
            out.push(source("repo", repo, None));
        }
        let agents = root.join(".claude").join("agent-memory");
        if real_dir(&agents) {
            let mut dirs: Vec<(Vec<u32>, OsString)> = std::fs::read_dir(&agents)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| (py_name_key(&e.file_name()), e.file_name()))
                        .collect()
                })
                .unwrap_or_default();
            dirs.sort();
            for (_, name) in dirs {
                let d = agents.join(&name);
                if real_dir(&d) {
                    let agent = name.to_string_lossy().into_owned();
                    out.push(source("agent", d, Some(agent)));
                }
            }
        }
    }
    let personal = home.join("personal-memory");
    if real_dir(&personal) {
        out.push(source("personal", personal, None));
    }
    out
}

fn source(scope: &str, dir: PathBuf, agent: Option<String>) -> NoteSource {
    NoteSource {
        scope: scope.to_string(),
        dir,
        agent,
    }
}

/// `p.is_dir() and not p.is_symlink()`.
fn real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

/// A folder anthology must not write through (`memory_sync.UnsafeTarget`, as
/// [`unsafe_dir`] reports it): the path and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsafeTarget {
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for UnsafeTarget {
    /// `refusing to use <path>: <reason>`, as `memory_sync`'s refusals read.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "refusing to use {}: {}",
            self.path.to_string_lossy(),
            self.reason
        )
    }
}

impl std::error::Error for UnsafeTarget {}

/// The reason of an [`UnsafeTarget`] for a symlink.
pub const REASON_SYMLINK: &str = "it is a symlink";
/// The reason for something that exists and is not a folder.
pub const REASON_NOT_A_FOLDER: &str = "it is not a folder";
/// The reason when `lstat` fails other than "absent" (Python's `Path.is_symlink` raises
/// there; Rust refuses, D-1).
pub const REASON_UNEXAMINABLE: &str = "it cannot be examined";

/// `memory_sync._unsafe_dir(path)`: `Some` when `path` is a symlink or exists as something
/// other than a folder; `None` for a real folder or nothing at all.
pub fn unsafe_dir(path: &Path) -> Option<UnsafeTarget> {
    let reason = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => REASON_SYMLINK,
        Ok(m) if m.is_dir() => return None,
        Ok(_) => REASON_NOT_A_FOLDER,
        // `Path.is_symlink` (3.12) is False for these (`pathlib._ignore_error`), and so is
        // `lexists`.
        Err(e)
            if e.raw_os_error()
                .is_some_and(|n| IGNORED_ERRNOS.contains(&n)) =>
        {
            return None;
        }
        Err(_) => REASON_UNEXAMINABLE,
    };
    Some(UnsafeTarget {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    })
}

/// `pathlib._IGNORED_ERRNOS`: `ENOENT`, `ENOTDIR`, `EBADF`, `ELOOP`.
#[cfg(target_os = "macos")]
const IGNORED_ERRNOS: [i32; 4] = [2, 20, 9, 62];
#[cfg(not(target_os = "macos"))]
const IGNORED_ERRNOS: [i32; 4] = [2, 20, 9, 40];

// ------------------------------------------------------------------------------------------
// Small helpers
// ------------------------------------------------------------------------------------------

/// `_SCP.match(u)` (`^(?:[^@/\s]+@)?([^:/\s]+):(.*)$`): the host and the rest.
fn scp_match(u: &str) -> Option<(String, String)> {
    static RX: OnceLock<Regex> = OnceLock::new();
    let rx = RX.get_or_init(|| {
        re::compile(r"^(?:[^@/\s]+@)?([^:/\s]+):(.*)$", false).expect("_SCP compiles")
    });
    // `$` makes it fancy-regex; a backtrack limit is not reachable for this linear pattern,
    // and an error reads as no match (the URL is then treated as a path, as Python's
    // fallthrough does).
    let groups = rx.captures_at(u, 0).ok().flatten()?;
    let (h0, h1) = groups.get(1).copied().flatten()?;
    let (r0, r1) = groups.get(2).copied().flatten()?;
    Some((u[h0..h1].to_string(), u[r0..r1].to_string()))
}

/// `s[n:]` in code points.
fn after_chars(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or("", |(i, _)| &s[i..])
}

/// `re.sub(r"/+", "/", key)`.
fn collapse_slashes(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut prev = false;
    for c in key.chars() {
        if c == '/' && prev {
            continue;
        }
        prev = c == '/';
        out.push(c);
    }
    out
}

/// `posixpath.join(a, b)`.
fn join_bytes(a: &[u8], b: &[u8]) -> Vec<u8> {
    if b.first() == Some(&b'/') {
        return b.to_vec();
    }
    let mut out = a.to_vec();
    if !out.is_empty() && !out.ends_with(b"/") {
        out.push(b'/');
    }
    out.extend_from_slice(b);
    out
}

/// `posixpath.basename(p)`.
fn basename(p: &[u8]) -> &[u8] {
    p.iter()
        .rposition(|&c| c == b'/')
        .map_or(p, |i| &p[i + 1..])
}

/// `posixpath.dirname(p)`.
fn dirname(p: &[u8]) -> &[u8] {
    let i = p.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1);
    let head = &p[..i];
    if !head.is_empty() && head.iter().any(|&c| c != b'/') {
        let end = head.iter().rposition(|&c| c != b'/').map_or(0, |j| j + 1);
        &head[..end]
    } else {
        head
    }
}

/// `os.path.expanduser(path)` (CPython 3.12 `posixpath`) with `env`'s `HOME`: `~` and `~/…`
/// use `HOME` (as given, even empty) or the passwd home; `~user` uses that user's passwd home
/// and is left as it is when the user is unknown.
fn expanduser(path: &str, env: &Env) -> Vec<u8> {
    let p = path.as_bytes();
    if !path.starts_with('~') {
        return p.to_vec();
    }
    let i = p
        .iter()
        .skip(1)
        .position(|&c| c == b'/')
        .map_or(p.len(), |j| j + 1);
    let found = if i != 1 {
        anthology_core::sys::user_home(&p[1..i])
    } else {
        match env.get_os("HOME") {
            Some(h) => Some(h.to_os_string()),
            None => anthology_core::sys::passwd_home(),
        }
    };
    let Some(home) = found else {
        return p.to_vec();
    };
    let home = home.as_bytes();
    let end = home.iter().rposition(|&c| c != b'/').map_or(0, |j| j + 1);
    let mut out = home[..end].to_vec();
    out.extend_from_slice(&p[i..]);
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirname_and_basename_follow_posixpath() {
        for (p, d, b) in [
            ("/a/b/.git", "/a/b", ".git"),
            ("/.git", "/", ".git"),
            ("//.git", "//", ".git"),
            ("a/.git", "a", ".git"),
            (".git", "", ".git"),
            ("/a//b", "/a", "b"),
        ] {
            assert_eq!(dirname(p.as_bytes()), d.as_bytes(), "dirname {p}");
            assert_eq!(basename(p.as_bytes()), b.as_bytes(), "basename {p}");
        }
    }

    #[test]
    fn expanduser_uses_home() {
        let env = Env::default().with("HOME", "/h/");
        assert_eq!(expanduser("~/x", &env), b"/h/x");
        assert_eq!(expanduser("~", &env), b"/h");
        assert_eq!(
            expanduser("~no-such-user-ms1-f/x", &env),
            b"~no-such-user-ms1-f/x"
        );
        assert_eq!(expanduser("x~", &env), b"x~");
        let env = Env::default().with("HOME", "/");
        assert_eq!(expanduser("~", &env), b"/");
        assert_eq!(expanduser("~/x", &env), b"/x");
    }

    #[test]
    fn expanduser_looks_a_named_user_up() {
        let env = Env::default().with("HOME", "/h");
        let root = anthology_core::sys::user_home(b"root").expect("root");
        let root = root.as_bytes();
        let end = root.iter().rposition(|&c| c != b'/').map_or(0, |j| j + 1);
        let mut want = root[..end].to_vec();
        want.extend_from_slice(b"/x.git");
        assert_eq!(expanduser("~root/x.git", &env), want);
    }

    #[test]
    fn encode_keeps_ascii_alnum_only() {
        assert_eq!(
            encode_project_path(Path::new("/no-such-dir-ms1/a b_é")),
            "-no-such-dir-ms1-a-b--"
        );
        let p = Path::new(OsStr::from_bytes(b"/no-such-dir-ms1/\xff\xfe"));
        assert_eq!(encode_project_path(p), "-no-such-dir-ms1---");
    }

    #[test]
    fn collapse() {
        assert_eq!(collapse_slashes("a//b///c/"), "a/b/c/");
    }
}
