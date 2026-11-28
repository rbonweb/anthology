//! The walk of a tree on disk (Task 2): never follows a symlink, never reads outside the root.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The limits of a walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkOpts {
    pub max_files: usize,
    pub max_bytes: u64,
    pub text_cap: usize,
    pub all: bool,
}

impl Default for WalkOpts {
    fn default() -> WalkOpts {
        WalkOpts {
            max_files: 100_000,
            max_bytes: 512 << 20,
            text_cap: 2 << 20,
            all: false,
        }
    }
}

/// What an entry of the tree is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File { exec: bool },
    Symlink { target: String },
    Submodule,
    Other,
}

/// One entry of the tree: a root-relative `/` path, its kind and its size in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
}

/// The git modes that change what an entry is.
const MODE_SYMLINK: u32 = 0o120_000;
const MODE_GITLINK: u32 = 0o160_000;

/// The longest symlink target read out of a file that git checked out in a link's place.
const LINK_TARGET_MAX: usize = 4096;

/// A tree on disk, opened once and read through `read`.
///
/// The walk uses `symlink_metadata` only and never enters a link. `read` opens only a path that is
/// an entry of the tree and a regular file, so nothing outside the root is ever read.
#[derive(Debug)]
pub struct DiskTree {
    root: PathBuf,
    opts: WalkOpts,
    entries: Vec<Entry>,
    index: HashMap<String, usize>,
    /// The real paths below a name that is not UTF-8, by the lossy path their entries carry.
    odd: HashMap<String, PathBuf>,
    skipped: Vec<String>,
    errors: Vec<String>,
    walk_partial: bool,
    read_total: AtomicU64,
    budget_spent: AtomicBool,
}

impl DiskTree {
    /// Walks `root`, shallow folders first and each folder in name order. `modes` are the git modes
    /// of a fetched repo (`ls-files` path and mode); they override the kinds the disk shows. Without
    /// `modes` (a folder the user named) the `.git` folder gives `.git/config` and the non-sample
    /// `.git/hooks/*` files as entries and nothing else; with them, the folder is the fetch's own and
    /// is left out.
    pub fn open(
        root: &Path,
        opts: &WalkOpts,
        modes: Option<&[(String, u32)]>,
    ) -> io::Result<DiskTree> {
        let root = fs::canonicalize(root)?;
        if !fs::metadata(&root)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "the scan target is not a folder",
            ));
        }
        let mut tree = DiskTree {
            root,
            opts: *opts,
            entries: Vec::new(),
            index: HashMap::new(),
            odd: HashMap::new(),
            skipped: Vec::new(),
            errors: Vec::new(),
            walk_partial: false,
            read_total: AtomicU64::new(0),
            budget_spent: AtomicBool::new(false),
        };
        let fetched = modes.is_some();
        let modes: HashMap<&str, u32> = modes
            .into_iter()
            .flatten()
            .map(|(path, mode)| (path.as_str(), git_mode(*mode)))
            .collect();
        tree.walk(&modes, fetched);
        Ok(tree)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Up to `max` bytes of the file at the root-relative `path`. Only a regular file that is an
    /// entry of the tree is opened; a link, a path that is not an entry and a path that leaves the
    /// root are errors. The reads of one tree add up to `max_bytes`; past that every read fails
    /// with `QuotaExceeded` and `partial` turns true.
    pub fn read(&self, path: &str, max: usize) -> io::Result<Vec<u8>> {
        let Some(&at) = self.index.get(path) else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "not an entry of the tree",
            ));
        };
        if !matches!(self.entries[at].kind, EntryKind::File { .. }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        if self.budget_spent.load(Ordering::Relaxed) {
            return Err(budget_error());
        }
        let real = self
            .odd
            .get(path)
            .cloned()
            .unwrap_or_else(|| self.root.join(path));
        let bytes = read_regular(&real, max)?;
        let before = self
            .read_total
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        if before.saturating_add(bytes.len() as u64) > self.opts.max_bytes {
            self.budget_spent.store(true, Ordering::Relaxed);
            return Err(budget_error());
        }
        Ok(bytes)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The limits the tree was opened with.
    pub fn opts(&self) -> &WalkOpts {
        &self.opts
    }

    /// Folders the walk left out, each as a root-relative path with a trailing `/`.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// Problems met on the way (a folder that cannot be listed), for the report. Names in them are
    /// quoted and escaped by `{:?}`.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// The walk or the reads hit a cap or a failure, so the tree is not fully covered.
    pub fn partial(&self) -> bool {
        self.walk_partial || self.budget_spent.load(Ordering::Relaxed)
    }

    fn walk(&mut self, modes: &HashMap<&str, u32>, fetched: bool) {
        let mut queue = VecDeque::from([(self.root.clone(), String::new(), false)]);
        'walk: while let Some((dir, rel, odd)) = queue.pop_front() {
            let Some(mut names) = self.list(&dir, &rel) else {
                continue;
            };
            names.sort();
            for name in names {
                let real = dir.join(&name);
                let (path, child_odd) = match name.to_str() {
                    Some(s) => (join(&rel, s), odd),
                    None => (join(&rel, &name.to_string_lossy()), true),
                };
                let meta = match fs::symlink_metadata(&real) {
                    Ok(meta) => meta,
                    Err(e) => {
                        self.fail("read", &path, &e);
                        continue;
                    }
                };
                let mode = modes.get(path.as_str()).copied();
                let more = if mode == Some(MODE_GITLINK) {
                    self.push(submodule(path), None)
                } else if meta.is_dir() {
                    let git = name.to_str() == Some(".git");
                    self.dir(&real, path, git, child_odd, fetched, &mut queue)
                } else if path == ".git" && meta.is_file() {
                    // A `.git` file only names a folder elsewhere: it adds nothing.
                    true
                } else if let Some(entry) = self.leaf(&real, &path, &meta, mode) {
                    self.push(entry, child_odd.then_some(real))
                } else {
                    true
                };
                if !more {
                    break 'walk;
                }
            }
        }
        // A submodule git lists but the checkout left out still gets its entry.
        let mut gitlinks: Vec<&str> = modes
            .iter()
            .filter(|(_, mode)| **mode == MODE_GITLINK)
            .map(|(path, _)| *path)
            .collect();
        gitlinks.sort_unstable();
        for path in gitlinks {
            if !self.index.contains_key(path) && !self.push(submodule(path.to_string()), None) {
                break;
            }
        }
    }

    /// The names in `dir`, unsorted; `None` if it cannot be listed.
    fn list(&mut self, dir: &Path, rel: &str) -> Option<Vec<OsString>> {
        let listing = match fs::read_dir(dir) {
            Ok(listing) => listing,
            Err(e) => {
                self.fail("list", rel, &e);
                return None;
            }
        };
        let mut names = Vec::new();
        for item in listing {
            match item {
                Ok(item) => names.push(item.file_name()),
                Err(e) => self.fail("list", rel, &e),
            }
        }
        Some(names)
    }

    /// Handles a folder: queues it, skips it, or (for `.git`) lists its two virtual entries.
    /// Returns false when a cap ends the walk.
    fn dir(
        &mut self,
        real: &Path,
        path: String,
        git: bool,
        odd: bool,
        fetched: bool,
        queue: &mut VecDeque<(PathBuf, String, bool)>,
    ) -> bool {
        if path == ".git" {
            return fetched || self.git_entries(real);
        }
        let worktrees = path == ".claude/worktrees" || path.ends_with("/.claude/worktrees");
        let node_modules = path == "node_modules" || path.ends_with("/node_modules");
        if git || (!self.opts.all && (node_modules || worktrees)) {
            self.skipped.push(format!("{path}/"));
            return true;
        }
        queue.push_back((real.to_path_buf(), path, odd));
        true
    }

    /// `.git/config` and the non-sample files of `.git/hooks`: what git runs from a folder.
    fn git_entries(&mut self, git: &Path) -> bool {
        let config = git.join("config");
        if let Ok(meta) = fs::symlink_metadata(&config)
            && let Some(entry) = self.leaf(&config, ".git/config", &meta, None)
            && !self.push(entry, None)
        {
            return false;
        }
        let hooks = git.join("hooks");
        let Ok(meta) = fs::symlink_metadata(&hooks) else {
            return true;
        };
        if !meta.is_dir() {
            return match self.leaf(&hooks, ".git/hooks", &meta, None) {
                Some(entry) => self.push(entry, None),
                None => true,
            };
        }
        let Some(mut names) = self.list(&hooks, ".git/hooks") else {
            return true;
        };
        names.sort();
        for name in names {
            let lossy = name.to_string_lossy().into_owned();
            if lossy.ends_with(".sample") {
                continue;
            }
            let real = hooks.join(&name);
            let path = format!(".git/hooks/{lossy}");
            let meta = match fs::symlink_metadata(&real) {
                Ok(meta) => meta,
                Err(e) => {
                    self.fail("read", &path, &e);
                    continue;
                }
            };
            let odd = name.to_str().is_none();
            if let Some(entry) = self.leaf(&real, &path, &meta, None)
                && !self.push(entry, odd.then_some(real))
            {
                return false;
            }
        }
        true
    }

    /// The entry for something that is not a folder; `None` for a folder.
    fn leaf(
        &mut self,
        real: &Path,
        path: &str,
        meta: &fs::Metadata,
        mode: Option<u32>,
    ) -> Option<Entry> {
        let file_type = meta.file_type();
        let kind = if file_type.is_symlink() {
            let target = match fs::read_link(real) {
                Ok(target) => target.to_string_lossy().into_owned(),
                Err(e) => {
                    self.fail("read the link", path, &e);
                    String::new()
                }
            };
            EntryKind::Symlink { target }
        } else if file_type.is_file() {
            if mode == Some(MODE_SYMLINK) {
                // `core.symlinks=false` checks a link out as a file that holds its target.
                let target = match read_regular(real, LINK_TARGET_MAX) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                    Err(e) => {
                        self.fail("read the link", path, &e);
                        String::new()
                    }
                };
                return Some(Entry {
                    path: path.to_string(),
                    kind: EntryKind::Symlink { target },
                    size: 0,
                });
            }
            let exec = mode.map_or_else(|| is_exec(meta), |m| m & 0o111 != 0);
            return Some(Entry {
                path: path.to_string(),
                kind: EntryKind::File { exec },
                size: meta.len(),
            });
        } else if file_type.is_dir() {
            return None;
        } else {
            EntryKind::Other
        };
        Some(Entry {
            path: path.to_string(),
            kind,
            size: 0,
        })
    }

    /// Adds `entry`; false when the file cap is hit (the walk then stops and is partial).
    fn push(&mut self, entry: Entry, odd_real: Option<PathBuf>) -> bool {
        if self.entries.len() >= self.opts.max_files {
            self.walk_partial = true;
            return false;
        }
        if self.index.contains_key(&entry.path) {
            self.skipped.push(format!(
                "{:?} (another entry has the same name)",
                entry.path
            ));
            return true;
        }
        if let Some(real) = odd_real {
            self.odd.insert(entry.path.clone(), real);
        }
        self.index.insert(entry.path.clone(), self.entries.len());
        self.entries.push(entry);
        true
    }

    /// Records a failure; the tree is then not fully covered.
    fn fail(&mut self, what: &str, rel: &str, e: &io::Error) {
        let rel = if rel.is_empty() { "." } else { rel };
        self.errors.push(format!("cannot {what} {rel:?}: {e}"));
        self.walk_partial = true;
    }
}

fn submodule(path: String) -> Entry {
    Entry {
        path,
        kind: EntryKind::Submodule,
        size: 0,
    }
}

fn join(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_string()
    } else {
        format!("{rel}/{name}")
    }
}

/// The mode as the number it is. A caller that read the `ls-files` digits as decimal gets the same
/// answer for the modes that matter here.
fn git_mode(mode: u32) -> u32 {
    match mode {
        100_644 => 0o100_644,
        100_755 => 0o100_755,
        120_000 => MODE_SYMLINK,
        160_000 => MODE_GITLINK,
        other => other,
    }
}

fn budget_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::QuotaExceeded,
        "the read limit of the scan is spent",
    )
}

/// Up to `max` bytes of a regular file. The path is checked with `symlink_metadata` first and the
/// opened file must be the same file, so a link swapped in after the walk is refused.
fn read_regular(path: &Path, max: usize) -> io::Result<Vec<u8>> {
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let file = fs::File::open(path)?;
    let after = file.metadata()?;
    if !after.is_file() || !same_file(&before, &after) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the file changed while it was opened",
        ));
    }
    let size = usize::try_from(after.len()).unwrap_or(usize::MAX);
    let mut bytes = Vec::with_capacity(max.min(size));
    file.take(max as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_a: &fs::Metadata, _b: &fs::Metadata) -> bool {
    true
}

#[cfg(unix)]
fn is_exec(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_exec(_meta: &fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn write(root: &Path, rel: &str, body: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn open(root: &Path) -> DiskTree {
        DiskTree::open(root, &WalkOpts::default(), None).unwrap()
    }

    fn paths(t: &DiskTree) -> Vec<&str> {
        t.entries().iter().map(|e| e.path.as_str()).collect()
    }

    fn kind<'a>(t: &'a DiskTree, path: &str) -> &'a EntryKind {
        &t.entries()
            .iter()
            .find(|e| e.path == path)
            .unwrap_or_else(|| panic!("no entry {path}: {:?}", paths(t)))
            .kind
    }

    #[test]
    fn walk_opts_default_to_the_plan_caps() {
        let o = WalkOpts::default();
        assert_eq!(o.max_files, 100_000);
        assert_eq!(o.max_bytes, 512 * 1024 * 1024);
        assert_eq!(o.text_cap, 2 * 1024 * 1024);
        assert!(!o.all);
    }

    #[test]
    fn symlinks_are_listed_and_never_followed() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "outside/secret", b"SENTINEL");
        write(tmp.path(), "repo/ok.txt", b"fine");
        symlink("../outside/secret", tmp.path().join("repo/link")).unwrap();
        symlink("../outside", tmp.path().join("repo/dir")).unwrap();
        let t = open(&tmp.path().join("repo"));
        assert_eq!(
            kind(&t, "link"),
            &EntryKind::Symlink {
                target: "../outside/secret".into()
            }
        );
        assert!(matches!(kind(&t, "dir"), EntryKind::Symlink { .. }));
        assert!(!paths(&t).iter().any(|p| p.starts_with("dir/")));
        assert!(t.read("link", 100).is_err());
        assert!(t.read("dir/secret", 100).is_err());
        assert_eq!(t.read("ok.txt", 100).unwrap(), b"fine");
    }

    #[test]
    fn read_refuses_paths_that_are_not_entries() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "outside/secret", b"SENTINEL");
        write(tmp.path(), "repo/ok.txt", b"fine");
        let t = open(&tmp.path().join("repo"));
        for p in ["../outside/secret", "/etc/hostname", "ok.txt/../ok.txt", ""] {
            assert!(t.read(p, 100).is_err(), "{p:?} was read");
        }
    }

    #[test]
    fn read_stops_at_max() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.txt", b"0123456789");
        let t = open(tmp.path());
        assert_eq!(t.read("a.txt", 4).unwrap(), b"0123");
    }

    #[test]
    fn a_root_that_is_a_symlink_is_canonicalized_once() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "real/a.txt", b"x");
        symlink(tmp.path().join("real"), tmp.path().join("lnk")).unwrap();
        let t = open(&tmp.path().join("lnk"));
        assert_eq!(t.root(), fs::canonicalize(tmp.path().join("real")).unwrap());
        assert_eq!(paths(&t), ["a.txt"]);
    }

    #[test]
    fn a_root_that_is_a_file_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.txt", b"x");
        assert!(DiskTree::open(&tmp.path().join("a.txt"), &WalkOpts::default(), None).is_err());
        assert!(DiskTree::open(&tmp.path().join("gone"), &WalkOpts::default(), None).is_err());
    }

    #[test]
    fn skips_node_modules_and_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.js", b"x");
        write(tmp.path(), "node_modules/dep/index.js", b"x");
        write(tmp.path(), "pkg/node_modules/dep/index.js", b"x");
        write(tmp.path(), ".claude/worktrees/w1/a.js", b"x");
        write(tmp.path(), ".claude/settings.json", b"{}");
        let t = open(tmp.path());
        assert_eq!(paths(&t), ["a.js", ".claude/settings.json"]);
        let mut skipped = t.skipped().to_vec();
        skipped.sort();
        assert_eq!(
            skipped,
            [".claude/worktrees/", "node_modules/", "pkg/node_modules/"]
        );
        assert!(!t.partial());
    }

    #[test]
    fn all_walks_node_modules_and_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "node_modules/dep/index.js", b"x");
        write(tmp.path(), ".claude/worktrees/w1/a.js", b"x");
        let o = WalkOpts {
            all: true,
            ..WalkOpts::default()
        };
        let t = DiskTree::open(tmp.path(), &o, None).unwrap();
        assert_eq!(
            paths(&t),
            ["node_modules/dep/index.js", ".claude/worktrees/w1/a.js"]
        );
        assert!(t.skipped().is_empty());
    }

    #[test]
    fn entries_come_shallow_first_and_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "b/deep/x.txt", b"x");
        write(tmp.path(), "b/y.txt", b"x");
        write(tmp.path(), "z.txt", b"x");
        write(tmp.path(), "a.txt", b"x");
        let t = open(tmp.path());
        assert_eq!(paths(&t), ["a.txt", "z.txt", "b/y.txt", "b/deep/x.txt"]);
    }

    #[test]
    fn caps_mark_partial_on_file_count() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..5 {
            write(tmp.path(), &format!("f{i}.txt"), b"x");
        }
        let o = WalkOpts {
            max_files: 3,
            ..WalkOpts::default()
        };
        let t = DiskTree::open(tmp.path(), &o, None).unwrap();
        assert_eq!(t.entries().len(), 3);
        assert!(t.partial());
    }

    #[test]
    fn a_tree_at_the_file_cap_is_not_partial() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..3 {
            write(tmp.path(), &format!("f{i}.txt"), b"x");
        }
        let o = WalkOpts {
            max_files: 3,
            ..WalkOpts::default()
        };
        let t = DiskTree::open(tmp.path(), &o, None).unwrap();
        assert_eq!(t.entries().len(), 3);
        assert!(!t.partial());
    }

    #[test]
    fn caps_mark_partial_on_bytes_read() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.txt", &[b'a'; 100]);
        write(tmp.path(), "b.txt", &[b'b'; 100]);
        let o = WalkOpts {
            max_bytes: 150,
            ..WalkOpts::default()
        };
        let t = DiskTree::open(tmp.path(), &o, None).unwrap();
        assert_eq!(t.read("a.txt", 1000).unwrap().len(), 100);
        assert!(!t.partial());
        assert!(t.read("b.txt", 1000).is_err());
        assert!(t.partial());
        assert!(t.read("a.txt", 1).is_err(), "the budget stays spent");
    }

    #[test]
    fn git_dir_hooks_and_config_are_virtual_entries() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "a.txt", b"x");
        write(tmp.path(), ".git/config", b"[core]\n");
        write(tmp.path(), ".git/HEAD", b"ref: refs/heads/main\n");
        write(tmp.path(), ".git/objects/ab/cdef", b"x");
        write(tmp.path(), ".git/hooks/pre-commit", b"#!/bin/sh\n");
        write(tmp.path(), ".git/hooks/pre-push.sample", b"#!/bin/sh\n");
        write(tmp.path(), "sub/.git/config", b"[core]\n");
        let t = open(tmp.path());
        assert_eq!(paths(&t), [".git/config", ".git/hooks/pre-commit", "a.txt"]);
        assert_eq!(
            t.read(".git/hooks/pre-commit", 100).unwrap(),
            b"#!/bin/sh\n"
        );
        assert_eq!(t.skipped(), ["sub/.git/"]);
    }

    #[test]
    fn a_git_file_adds_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".git", b"gitdir: ../elsewhere\n");
        write(tmp.path(), "a.txt", b"x");
        let t = open(tmp.path());
        assert_eq!(paths(&t), ["a.txt"]);
    }

    #[test]
    fn a_symlinked_git_dir_is_a_symlink_entry() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "outside/config", b"SENTINEL");
        write(tmp.path(), "repo/a.txt", b"x");
        symlink("../outside", tmp.path().join("repo/.git")).unwrap();
        let t = open(&tmp.path().join("repo"));
        assert_eq!(paths(&t), [".git", "a.txt"]);
        assert!(matches!(kind(&t, ".git"), EntryKind::Symlink { .. }));
    }

    #[test]
    fn a_symlinked_git_config_is_never_read() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "outside/config", b"SENTINEL");
        fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();
        symlink("../../outside/config", tmp.path().join("repo/.git/config")).unwrap();
        let t = open(&tmp.path().join("repo"));
        assert!(matches!(kind(&t, ".git/config"), EntryKind::Symlink { .. }));
        assert!(t.read(".git/config", 100).is_err());
    }

    #[test]
    fn a_fetched_tree_ignores_its_own_git_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".git/config", b"[core]\n");
        write(tmp.path(), ".git/hooks/pre-commit", b"x");
        write(tmp.path(), "a.txt", b"x");
        let t = DiskTree::open(tmp.path(), &WalkOpts::default(), Some(&[])).unwrap();
        assert_eq!(paths(&t), ["a.txt"]);
        assert!(t.skipped().is_empty());
    }

    #[test]
    fn git_modes_override_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        // `core.symlinks=false` checks a symlink out as a file holding its target.
        write(tmp.path(), "link", b"../../etc/passwd");
        write(tmp.path(), "run.sh", b"#!/bin/sh\n");
        write(tmp.path(), "plain.txt", b"x");
        fs::create_dir(tmp.path().join("vendor")).unwrap();
        let modes = vec![
            ("link".to_string(), 0o120000),
            ("run.sh".to_string(), 0o100755),
            ("plain.txt".to_string(), 0o100644),
            ("vendor".to_string(), 0o160000),
            ("gone".to_string(), 0o160000),
        ];
        let t = DiskTree::open(tmp.path(), &WalkOpts::default(), Some(&modes)).unwrap();
        assert_eq!(
            kind(&t, "link"),
            &EntryKind::Symlink {
                target: "../../etc/passwd".into()
            }
        );
        assert_eq!(kind(&t, "run.sh"), &EntryKind::File { exec: true });
        assert_eq!(kind(&t, "plain.txt"), &EntryKind::File { exec: false });
        assert_eq!(kind(&t, "vendor"), &EntryKind::Submodule);
        assert_eq!(kind(&t, "gone"), &EntryKind::Submodule);
    }

    #[test]
    fn the_exec_bit_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "run.sh", b"x");
        write(tmp.path(), "plain.txt", b"x");
        fs::set_permissions(tmp.path().join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(
            tmp.path().join("plain.txt"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let t = open(tmp.path());
        assert_eq!(kind(&t, "run.sh"), &EntryKind::File { exec: true });
        assert_eq!(kind(&t, "plain.txt"), &EntryKind::File { exec: false });
    }

    #[test]
    fn a_fifo_is_listed_and_never_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("pipe");
        // This crate has no libc: the `mkfifo` tool makes the pipe, and a host without it skips.
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if made.map(|s| s.success()).unwrap_or(false) {
            let t = open(tmp.path());
            assert_eq!(kind(&t, "pipe"), &EntryKind::Other);
            assert!(t.read("pipe", 10).is_err());
        }
    }

    #[test]
    fn non_utf8_names_are_listed_and_readable() {
        let tmp = tempfile::tempdir().unwrap();
        let odd = std::ffi::OsStr::from_bytes(b"odd\xffname");
        fs::create_dir(tmp.path().join(odd)).unwrap();
        fs::write(tmp.path().join(odd).join("a.txt"), b"inside").unwrap();
        let t = open(tmp.path());
        let p = paths(&t);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].starts_with("odd\u{fffd}name/"), "{p:?}");
        assert_eq!(t.read(p[0], 100).unwrap(), b"inside");
    }
}
