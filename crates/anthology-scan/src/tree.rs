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

/// The git modes that change what an entry is. The modes passed to `open` are octal values, as
/// `fetch` reads them from `ls-files`; the digits `120000` read as decimal are not a symlink.
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
    /// Folders queued so far: they count toward `max_files` like entries do.
    dirs: usize,
    walk_partial: bool,
    read_total: AtomicU64,
    budget_spent: AtomicBool,
}

impl DiskTree {
    /// Walks `root`, shallow folders first and each folder in name order. `modes` are the git modes
    /// of a fetched repo (`ls-files` path and mode); they override the kinds the disk shows.
    ///
    /// Without `modes` (a folder the user named) every `.git` gives `<dir>/.git/config`,
    /// `.git/config.worktree` and the non-sample `.git/hooks/*` files as entries and nothing else.
    /// A `.git` file that says `gitdir: <folder>`, and a `.git`, `config` or `hooks` that is a link,
    /// are followed when the place they point at is inside the root; the entries keep the `.git/...`
    /// names git would use. With `modes` the `.git` folder is the fetch's own and is left out, and
    /// `node_modules` and `.claude/worktrees` are walked like any folder: a committed one is repo
    /// content.
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
            dirs: 0,
            walk_partial: false,
            read_total: AtomicU64::new(0),
            budget_spent: AtomicBool::new(false),
        };
        let fetched = modes.is_some();
        let modes: HashMap<&str, u32> = modes
            .into_iter()
            .flatten()
            .map(|(path, mode)| (path.as_str(), *mode))
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
        let real = self.real_of(path).unwrap_or_else(|| self.root.join(path));
        self.read_budgeted(&real, max)
    }

    /// The path on disk behind the entry `path`: the same path under the root, except for an entry
    /// below a name that is not UTF-8 or a `.git` entry that a link or a `gitdir:` file moved.
    pub fn real_of(&self, path: &str) -> Option<PathBuf> {
        self.index.get(path)?;
        Some(
            self.odd
                .get(path)
                .cloned()
                .unwrap_or_else(|| self.root.join(path)),
        )
    }

    /// `target` taken from the folder `base` (an absolute `target` is taken as it is), with every
    /// link resolved; `None` if the result is missing or is not inside the root. This is how a
    /// file that another file includes is found without ever leaving the repo.
    pub fn resolve_inside(&self, base: &Path, target: &str) -> Option<PathBuf> {
        self.inside(&base.join(target))
    }

    /// Up to `max` bytes of a regular file that `resolve_inside` or `real_of` returned. Counted in
    /// the read budget like `read`; a path outside the root is an error.
    pub fn read_real(&self, real: &Path, max: usize) -> io::Result<Vec<u8>> {
        if !real.starts_with(&self.root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "outside the scan root",
            ));
        }
        self.read_budgeted(real, max)
    }

    fn read_budgeted(&self, real: &Path, max: usize) -> io::Result<Vec<u8>> {
        if self.budget_spent.load(Ordering::Relaxed) {
            return Err(budget_error());
        }
        let bytes = read_regular(real, max)?;
        let before = self
            .read_total
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        if before.saturating_add(bytes.len() as u64) > self.opts.max_bytes {
            self.budget_spent.store(true, Ordering::Relaxed);
            return Err(budget_error());
        }
        Ok(bytes)
    }

    /// `p` with every link resolved, if that is inside the root.
    fn inside(&self, p: &Path) -> Option<PathBuf> {
        let canonical = fs::canonicalize(p).ok()?;
        canonical.starts_with(&self.root).then_some(canonical)
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
                let git = name.to_str() == Some(".git");
                let more = if mode == Some(MODE_GITLINK) {
                    self.push(submodule(path), None)
                } else if git && !fetched {
                    self.git_item(&real, &path, &meta, child_odd)
                } else if meta.is_dir() {
                    self.dir(&real, path, git, child_odd, fetched, &mut queue)
                } else if git && meta.is_file() {
                    // A `.git` file only names a folder: it adds nothing.
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
        // Names past what the file cap leaves room for are never collected: one huge folder must
        // not be read in full before the cap is looked at.
        let room = self.opts.max_files.saturating_sub(self.count());
        let mut names = Vec::new();
        for item in listing {
            match item {
                Ok(item) => names.push(item.file_name()),
                Err(e) => self.fail("list", rel, &e),
            }
            if names.len() > room {
                self.walk_partial = true;
                names.truncate(room);
                break;
            }
        }
        Some(names)
    }

    /// Entries and folders so far: what the file cap counts.
    fn count(&self) -> usize {
        self.entries.len() + self.dirs
    }

    /// Handles a folder: queues it or skips it. Returns false when a cap ends the walk. `git` is
    /// true for a folder named `.git` of a fetched tree, which is left out.
    fn dir(
        &mut self,
        real: &Path,
        path: String,
        git: bool,
        odd: bool,
        fetched: bool,
        queue: &mut VecDeque<(PathBuf, String, bool)>,
    ) -> bool {
        if git {
            if path != ".git" {
                self.skipped.push(format!("{path}/"));
            }
            return true;
        }
        let worktrees = path == ".claude/worktrees" || path.ends_with("/.claude/worktrees");
        let node_modules = path == "node_modules" || path.ends_with("/node_modules");
        // A committed `node_modules` of a fetched repo is repo content, so only a folder the user
        // named leaves them out.
        if !fetched && !self.opts.all && (node_modules || worktrees) {
            self.skipped.push(format!("{path}/"));
            return true;
        }
        if self.count() >= self.opts.max_files {
            self.walk_partial = true;
            return false;
        }
        self.dirs += 1;
        queue.push_back((real.to_path_buf(), path, odd));
        true
    }

    /// A `.git` in a folder the user named (`path` is `.git` or `<dir>/.git`): a folder, a link or
    /// a file. Returns false when a cap ends the walk.
    fn git_item(&mut self, real: &Path, path: &str, meta: &fs::Metadata, odd: bool) -> bool {
        if meta.is_dir() {
            return self.git_entries(real, path);
        }
        if meta.file_type().is_symlink() {
            // The link is an entry (the orchestrator rates one that leaves the repo), and a link
            // that stays inside the repo also gives the entries of the folder it names.
            if let Some(entry) = self.leaf(real, path, meta, None)
                && !self.push(entry, odd.then(|| real.to_path_buf()))
            {
                return false;
            }
            return match self.inside(real) {
                Some(dir) if dir.is_dir() => self.git_entries(&dir, path),
                _ => true,
            };
        }
        if meta.is_file() {
            // `gitdir: <folder>`: the real git folder, if it is inside the repo.
            let Ok(bytes) = read_regular(real, LINK_TARGET_MAX) else {
                return true;
            };
            let text = String::from_utf8_lossy(&bytes);
            let target = text
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("gitdir:"))
                .map(str::trim);
            let base = real.parent().unwrap_or(real);
            return match target.and_then(|t| self.resolve_inside(base, t)) {
                Some(dir) if dir.is_dir() => self.git_entries(&dir, path),
                _ => true,
            };
        }
        match self.leaf(real, path, meta, None) {
            Some(entry) => self.push(entry, None),
            None => true,
        }
    }

    /// `config`, `config.worktree` and the non-sample files of `hooks` of the git folder `git`, as
    /// `<prefix>/config` and so on: what git runs in a folder. Returns false when a cap ends the walk.
    fn git_entries(&mut self, git: &Path, prefix: &str) -> bool {
        for name in ["config", "config.worktree"] {
            if !self.git_leaf(&git.join(name), &format!("{prefix}/{name}")) {
                return false;
            }
        }
        let hooks = git.join("hooks");
        let Ok(meta) = fs::symlink_metadata(&hooks) else {
            return true;
        };
        let hooks_path = format!("{prefix}/hooks");
        let hooks_dir = if meta.is_dir() {
            hooks
        } else {
            match self.inside(&hooks) {
                Some(dir) if meta.file_type().is_symlink() && dir.is_dir() => dir,
                _ => return self.git_leaf(&hooks, &hooks_path),
            }
        };
        let Some(mut names) = self.list(&hooks_dir, &hooks_path) else {
            return true;
        };
        names.sort();
        for name in names {
            let lossy = name.to_string_lossy().into_owned();
            if lossy.ends_with(".sample") {
                continue;
            }
            if !self.git_leaf(&hooks_dir.join(&name), &format!("{hooks_path}/{lossy}")) {
                return false;
            }
        }
        true
    }

    /// One file of a git folder. A link that stays inside the repo gives the file it names (git
    /// reads it); one that leaves stays a link entry. A missing file is nothing.
    fn git_leaf(&mut self, real: &Path, path: &str) -> bool {
        let meta = match fs::symlink_metadata(real) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return true,
            Err(e) => {
                self.fail("read", path, &e);
                return true;
            }
        };
        let mut real = real.to_path_buf();
        let mut meta = meta;
        if meta.file_type().is_symlink()
            && let Some(target) = self.inside(&real)
            && let Ok(m) = fs::symlink_metadata(&target)
            && m.is_file()
        {
            real = target;
            meta = m;
        }
        match self.leaf(&real, path, &meta, None) {
            Some(entry) => self.push(entry, Some(real)),
            None => true,
        }
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
    fn push(&mut self, mut entry: Entry, odd_real: Option<PathBuf>) -> bool {
        if self.count() >= self.opts.max_files {
            self.walk_partial = true;
            return false;
        }
        // Two names that are not UTF-8 can read the same once lossy: the second gets a suffix, so
        // both are listed and scanned.
        if self.index.contains_key(&entry.path) {
            let base = entry.path.clone();
            let free = (2..)
                .map(|n| format!("{base}~{n}"))
                .find(|p| !self.index.contains_key(p));
            entry.path = free.unwrap_or(base);
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
    let file = open_nofollow(path)?;
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

// The open flags, by hand because this crate has no libc. A platform not listed gets 0 and relies
// on the `symlink_metadata` and `fstat` checks around the open alone.
#[cfg(target_os = "macos")]
mod flags {
    pub const O_NOFOLLOW: i32 = 0x100;
    pub const O_NONBLOCK: i32 = 0x4;
}
#[cfg(all(target_os = "linux", any(target_arch = "x86", target_arch = "x86_64")))]
mod flags {
    pub const O_NOFOLLOW: i32 = 0o400_000;
    pub const O_NONBLOCK: i32 = 0o4000;
}
#[cfg(all(
    target_os = "linux",
    any(
        target_arch = "aarch64",
        target_arch = "arm",
        target_arch = "riscv64",
        target_arch = "loongarch64"
    )
))]
mod flags {
    pub const O_NOFOLLOW: i32 = 0o100_000;
    pub const O_NONBLOCK: i32 = 0o4000;
}
#[cfg(not(any(
    target_os = "macos",
    all(
        target_os = "linux",
        any(
            target_arch = "x86",
            target_arch = "x86_64",
            target_arch = "aarch64",
            target_arch = "arm",
            target_arch = "riscv64",
            target_arch = "loongarch64"
        )
    )
)))]
mod flags {
    pub const O_NOFOLLOW: i32 = 0;
    pub const O_NONBLOCK: i32 = 0;
}

/// Opens a file for reading without following a link in its last component and without waiting
/// on a pipe: a file swapped for a link or a FIFO after the `symlink_metadata` check is refused
/// or opens at once, and the `fstat` that follows tells what it is.
#[cfg(unix)]
fn open_nofollow(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags::O_NOFOLLOW | flags::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_nofollow(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
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
        let t = open(tmp.path());
        assert_eq!(paths(&t), [".git/config", ".git/hooks/pre-commit", "a.txt"]);
        assert_eq!(
            t.read(".git/hooks/pre-commit", 100).unwrap(),
            b"#!/bin/sh\n"
        );
        assert!(t.skipped().is_empty());
    }

    #[test]
    fn nested_git_dirs_give_their_config_and_hooks() {
        // I2: a nested repo's config runs commands when git is used in it.
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "sub/.git/config", b"[core]\n\tfsmonitor = x\n");
        write(tmp.path(), "sub/.git/hooks/post-checkout", b"#!/bin/sh\n");
        write(tmp.path(), "sub/.git/objects/ab/cd", b"x");
        write(tmp.path(), "sub/a.txt", b"x");
        let t = open(tmp.path());
        assert_eq!(
            paths(&t),
            [
                "sub/.git/config",
                "sub/.git/hooks/post-checkout",
                "sub/a.txt"
            ]
        );
        assert_eq!(
            t.read("sub/.git/config", 100).unwrap(),
            b"[core]\n\tfsmonitor = x\n"
        );
        assert!(t.skipped().is_empty());
    }

    #[test]
    fn a_nested_git_dir_of_a_fetched_tree_is_left_out() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "sub/.git/config", b"[core]\n");
        write(tmp.path(), "sub/a.txt", b"x");
        let t = DiskTree::open(tmp.path(), &WalkOpts::default(), Some(&[])).unwrap();
        assert_eq!(paths(&t), ["sub/a.txt"]);
        assert_eq!(t.skipped(), ["sub/.git/"]);
    }

    #[test]
    fn config_worktree_is_an_entry() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".git/config.worktree", b"[core]\n");
        let t = open(tmp.path());
        assert_eq!(paths(&t), [".git/config.worktree"]);
    }

    #[test]
    fn a_gitdir_file_that_names_a_folder_in_the_repo_is_followed() {
        // I2: `.git` as a file, `gitdir: .g`, puts the real git folder at `.g`.
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), ".git", b"gitdir: .g\n");
        write(tmp.path(), ".g/config", b"[core]\n\tfsmonitor = x\n");
        write(tmp.path(), ".g/hooks/pre-commit", b"#!/bin/sh\n");
        let t = open(tmp.path());
        let p = paths(&t);
        assert!(p.contains(&".git/config"), "{p:?}");
        assert!(p.contains(&".git/hooks/pre-commit"), "{p:?}");
        assert_eq!(
            t.read(".git/config", 100).unwrap(),
            b"[core]\n\tfsmonitor = x\n"
        );
        assert_eq!(
            t.read(".git/hooks/pre-commit", 100).unwrap(),
            b"#!/bin/sh\n"
        );
    }

    #[test]
    fn a_gitdir_file_that_names_a_folder_outside_the_repo_is_not_read() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "outside/config", b"SENTINEL");
        write(tmp.path(), "repo/a.txt", b"x");
        let absolute = format!("{}/outside", tmp.path().display());
        for target in ["../outside", absolute.as_str()] {
            write(
                tmp.path(),
                "repo/.git",
                format!("gitdir: {target}\n").as_bytes(),
            );
            let t = open(&tmp.path().join("repo"));
            assert_eq!(paths(&t), ["a.txt"], "{target}");
        }
    }

    #[test]
    fn a_git_dir_link_that_stays_in_the_repo_is_followed_and_one_that_leaves_is_not() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "repo/real/config", b"[core]\n\tpager = x\n");
        write(tmp.path(), "repo/real/hooks/pre-push", b"#!/bin/sh\n");
        write(tmp.path(), "outside/config", b"SENTINEL");
        symlink("real", tmp.path().join("repo/.git")).unwrap();
        let t = open(&tmp.path().join("repo"));
        assert!(matches!(kind(&t, ".git"), EntryKind::Symlink { .. }));
        assert_eq!(
            t.read(".git/config", 100).unwrap(),
            b"[core]\n\tpager = x\n"
        );
        assert!(paths(&t).contains(&".git/hooks/pre-push"));

        fs::remove_file(tmp.path().join("repo/.git")).unwrap();
        symlink("../outside", tmp.path().join("repo/.git")).unwrap();
        let t = open(&tmp.path().join("repo"));
        assert!(!paths(&t).contains(&".git/config"));
    }

    #[test]
    fn config_and_hooks_links_that_stay_in_the_repo_are_followed() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "repo/ci/cfg", b"[core]\n\tfsmonitor = x\n");
        write(tmp.path(), "repo/ci/hooks/pre-commit", b"#!/bin/sh\n");
        write(tmp.path(), "outside/config", b"SENTINEL");
        fs::create_dir_all(tmp.path().join("repo/.git")).unwrap();
        symlink("../ci/cfg", tmp.path().join("repo/.git/config")).unwrap();
        symlink("../ci/hooks", tmp.path().join("repo/.git/hooks")).unwrap();
        let t = open(&tmp.path().join("repo"));
        assert_eq!(
            kind(&t, ".git/config"),
            &EntryKind::File { exec: false },
            "{:?}",
            paths(&t)
        );
        assert_eq!(
            t.read(".git/config", 100).unwrap(),
            b"[core]\n\tfsmonitor = x\n"
        );
        assert_eq!(
            t.read(".git/hooks/pre-commit", 100).unwrap(),
            b"#!/bin/sh\n"
        );
    }

    #[test]
    fn resolve_inside_and_read_real_stay_in_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "outside/x.cfg", b"SENTINEL");
        write(tmp.path(), "repo/.git/config", b"[core]\n");
        write(tmp.path(), "repo/.git/inc.cfg", b"[core]\n\tpager = x\n");
        write(tmp.path(), "repo/ci/y.cfg", b"y");
        symlink("../../outside/x.cfg", tmp.path().join("repo/.git/link.cfg")).unwrap();
        let t = open(&tmp.path().join("repo"));
        let base = t
            .real_of(".git/config")
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let inc = t.resolve_inside(&base, "inc.cfg").unwrap();
        assert_eq!(t.read_real(&inc, 100).unwrap(), b"[core]\n\tpager = x\n");
        assert!(t.resolve_inside(&base, "../ci/y.cfg").is_some());
        assert!(t.resolve_inside(&base, "../../outside/x.cfg").is_none());
        assert!(t.resolve_inside(&base, "link.cfg").is_none(), "a link out");
        assert!(t.resolve_inside(&base, "/etc/hostname").is_none());
        assert!(t.resolve_inside(&base, "missing.cfg").is_none());
        let out = tmp.path().join("outside/x.cfg");
        assert!(t.read_real(&out, 100).is_err());
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
    fn a_committed_node_modules_is_scanned_in_url_mode() {
        // I3: a planted node_modules/lodash that a postinstall runs must be read.
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "node_modules/lodash/index.js", b"x");
        write(tmp.path(), ".claude/worktrees/w1/a.js", b"x");
        let t = DiskTree::open(tmp.path(), &WalkOpts::default(), Some(&[])).unwrap();
        assert_eq!(
            paths(&t),
            ["node_modules/lodash/index.js", ".claude/worktrees/w1/a.js"]
        );
        assert!(t.skipped().is_empty());
        assert!(!t.partial());
    }

    #[test]
    fn two_non_utf8_names_with_one_lossy_name_are_both_listed_and_readable() {
        // I3: both names read as `a\u{fffd}b`; neither may be dropped.
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (
            std::ffi::OsStr::from_bytes(b"a\xffb"),
            std::ffi::OsStr::from_bytes(b"a\xfeb"),
        );
        let made = fs::write(tmp.path().join(a), b"first")
            .and_then(|()| fs::write(tmp.path().join(b), b"second"));
        if made.is_err() {
            eprintln!("skipped: this file system refuses names that are not UTF-8");
            return;
        }
        let t = open(tmp.path());
        let p = paths(&t);
        assert_eq!(p.len(), 2, "{p:?}");
        let mut got: Vec<Vec<u8>> = p.iter().map(|x| t.read(x, 100).unwrap()).collect();
        got.sort();
        assert_eq!(got, [b"first".to_vec(), b"second".to_vec()]);
        assert!(t.skipped().is_empty(), "{:?}", t.skipped());
    }

    #[test]
    fn folders_count_toward_the_file_cap() {
        // Minor 8: empty folders cost the walk too.
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..6 {
            fs::create_dir(tmp.path().join(format!("d{i}"))).unwrap();
        }
        let o = WalkOpts {
            max_files: 3,
            ..WalkOpts::default()
        };
        let t = DiskTree::open(tmp.path(), &o, None).unwrap();
        assert!(t.partial());
    }

    #[test]
    fn one_huge_folder_is_cut_at_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..60 {
            write(tmp.path(), &format!("f{i:02}.txt"), b"x");
        }
        let o = WalkOpts {
            max_files: 10,
            ..WalkOpts::default()
        };
        let t = DiskTree::open(tmp.path(), &o, None).unwrap();
        assert!(t.entries().len() <= 10);
        assert!(t.partial());
    }

    #[test]
    fn open_nofollow_refuses_a_link_and_does_not_block_on_a_pipe() {
        // Minor 1: the open itself must refuse a link and never wait on a pipe.
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "real.txt", b"x");
        symlink("real.txt", tmp.path().join("link.txt")).unwrap();
        assert!(open_nofollow(&tmp.path().join("real.txt")).is_ok());
        assert!(open_nofollow(&tmp.path().join("link.txt")).is_err());
        let fifo = tmp.path().join("pipe");
        if std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            // Returns at once; the fstat that follows rejects it as not a regular file.
            let file = open_nofollow(&fifo).unwrap();
            assert!(!file.metadata().unwrap().is_file());
        }
    }

    #[test]
    fn modes_are_octal_numbers_never_decimal_digits() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "link", b"../../etc/passwd");
        fs::create_dir(tmp.path().join("vendor")).unwrap();
        write(tmp.path(), "vendor/a.txt", b"x");
        let modes = vec![
            ("link".to_string(), 120_000),
            ("vendor".to_string(), 160_000),
        ];
        let t = DiskTree::open(tmp.path(), &WalkOpts::default(), Some(&modes)).unwrap();
        assert!(matches!(kind(&t, "link"), EntryKind::File { .. }));
        assert!(matches!(kind(&t, "vendor/a.txt"), EntryKind::File { .. }));
        assert!(!paths(&t).contains(&"vendor"));
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
        let made = fs::create_dir(tmp.path().join(odd))
            .and_then(|()| fs::write(tmp.path().join(odd).join("a.txt"), b"inside"));
        if let Err(e) = made {
            // APFS refuses names that are not valid UTF-8 (EILSEQ: 92 on macOS, 84 on Linux). Any
            // other error is a real failure; Linux accepts the name, so it asserts there.
            let refused = matches!(e.raw_os_error(), Some(84 | 92))
                || matches!(
                    e.kind(),
                    io::ErrorKind::InvalidInput | io::ErrorKind::InvalidFilename
                );
            assert!(refused, "the fixture could not be made: {e}");
            eprintln!("skipped: this file system refuses names that are not UTF-8: {e}");
            return;
        }
        let t = open(tmp.path());
        let p = paths(&t);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].starts_with("odd\u{fffd}name/"), "{p:?}");
        assert_eq!(t.read(p[0], 100).unwrap(), b"inside");
    }
}
