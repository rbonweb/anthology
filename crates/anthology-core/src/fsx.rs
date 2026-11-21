//! File helpers with `memory_sync.py`'s exact semantics (plan §3.3, GC-19).
//!
//! | Rust | Python |
//! |---|---|
//! | [`atomic_write`] | `_atomic_write`: `mkdir -p` the parent, `mkstemp` beside the target, write, `chmod`, `os.replace`; on any failure unlink the temp file and re-raise |
//! | [`read_regular`] | `_read_regular`: absent is empty; a symlink, FIFO, device or folder is refused; `O_NOFOLLOW \| O_NONBLOCK`, then `fstat` must show a regular file |
//! | [`create_excl`] | `save_note`'s `os.open(O_WRONLY \| O_CREAT \| O_EXCL \| O_NOFOLLOW, 0o644)`; a failed write unlinks the file |
//! | [`append_line`] | `_append_line`: refuses non-regular files, `O_RDWR \| O_APPEND \| O_CREAT \| O_NOFOLLOW`, adds a `\n` first when the file does not end in one |
//! | [`is_real_dir`] | `not os.path.islink(p) and os.path.isdir(p)` |
//!
//! A refusal is `PyExc::Other("UnsafeTarget", <memory_sync's message>)`; an OS failure is
//! `PyExc::OSError` with the errno (and the filename where Python's exception has one).
//! Symlinks are refused as Python refuses them: by the `lstat` check, else by the kernel
//! (`ELOOP` from `O_NOFOLLOW`, `EEXIST` from `O_EXCL`); a symlink target of `atomic_write` is
//! replaced, never written through.
//!
//! `mkstemp` is `tempfile.mkstemp`'s: `<prefix><8 of [a-z0-9_]><suffix>` in the target's
//! folder, `O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW`, mode 0600, retried on `EEXIST`.
#![forbid(unsafe_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anthology_pycompat::pyerr::PyExc;

/// Test-only fault injection (GC-19): `fail_next(op, errno)` makes the next `op` on this
/// thread fail with `OSError(errno)`. Faults are per thread, so parallel tests do not see each
/// other's, and each fires once, in the order queued.
///
/// The ops, in the order the helpers (and `state::write`) reach them:
/// - `lstat`: the type check before `read_regular` and `append_line`;
/// - `mkdir`: each `mkdir` while creating the parent folders;
/// - `open`: opening the target, or creating the `mkstemp` file;
/// - `fstat`: the regular-file check after opening;
/// - `read`: reading the file (`read_regular`; `append_line`'s last-byte `pread`);
/// - `write`: writing the data;
/// - `chmod`: `atomic_write`'s `chmod` of the temp file (an `fchmod` while it is open);
/// - `close`: closing a file (the real close still runs; its error is replaced);
/// - `rename`: the `os.replace` of the temp file onto the target;
/// - `unlink`: the cleanup after a failure (Python ignores its error, and so does Rust).
#[cfg(any(test, feature = "test-faults"))]
pub mod fault {
    use std::cell::RefCell;

    /// Every op name [`fail_next`] accepts.
    pub const OPS: [&str; 10] = [
        "lstat", "mkdir", "open", "fstat", "read", "write", "chmod", "close", "rename", "unlink",
    ];

    thread_local! {
        static PENDING: RefCell<Vec<(&'static str, i32)>> = const { RefCell::new(Vec::new()) };
    }

    /// Makes the next `op` on this thread fail with `OSError(errno)`. Panics on an op name
    /// that is not in [`OPS`].
    pub fn fail_next(op: &str, errno: i32) {
        let op = OPS
            .iter()
            .find(|o| **o == op)
            .unwrap_or_else(|| panic!("fsx::fault: unknown op {op:?}; one of {OPS:?}"));
        PENDING.with(|p| p.borrow_mut().push((op, errno)));
    }

    pub(crate) fn take(op: &str) -> Option<i32> {
        PENDING.with(|p| {
            let mut p = p.borrow_mut();
            let i = p.iter().position(|(o, _)| *o == op)?;
            Some(p.remove(i).1)
        })
    }
}

/// An injected failure for `op`, if a test queued one.
fn fault(op: &str) -> io::Result<()> {
    #[cfg(any(test, feature = "test-faults"))]
    if let Some(e) = fault::take(op) {
        return Err(io::Error::from_raw_os_error(e));
    }
    let _ = op;
    Ok(())
}

/// Closes `file` and reports the close error, as Python's `close()` raises it (a delayed
/// `ENOSPC` or `EIO`, on NFS for example). An injected `close` fault still closes the file.
fn close(file: File) -> io::Result<()> {
    let injected = fault("close");
    let closed = crate::sys::close(file);
    injected.and(closed)
}

/// `str(path)`.
pub(crate) fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `OSError` with a filename, as `os.*(path)` raises it.
fn os_err(e: io::Error, p: &Path) -> PyExc {
    match PyExc::from(e) {
        PyExc::OSError { errno, .. } => PyExc::OSError {
            errno,
            filename: Some(path_str(p)),
        },
        other => other,
    }
}

/// `exc.strerror`: the errno text alone (the provisional `pyerr` text until P1's tables).
fn strerror(e: io::Error) -> String {
    let s = PyExc::from(e).py_str();
    match s.split_once("] ") {
        Some((_, text)) if s.starts_with("[Errno ") => text.to_string(),
        _ => s,
    }
}

fn unsafe_target(msg: String) -> PyExc {
    PyExc::Other("UnsafeTarget", msg)
}

/// `"."` for the empty parent of a bare file name, as `Path("f").parent` is `Path(".")`.
fn parent_of(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// `Path(p).mkdir(parents=True, exist_ok=True)`, step for step (CPython 3.12 `pathlib`), so
/// the error names the folder that failed.
pub fn mkdir_parents(p: &Path) -> Result<(), PyExc> {
    let p = if p.as_os_str().is_empty() {
        Path::new(".")
    } else {
        p
    };
    match mkdir_once(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = parent_of(p);
            if p.parent().is_none() || parent == p {
                return Err(os_err(e, p));
            }
            mkdir_parents(parent)?;
            match mkdir_once(p) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() != io::ErrorKind::NotFound && p.is_dir() => Ok(()),
                Err(e) => Err(os_err(e, p)),
            }
        }
        // EEXIST is not relied on: the OS may report EACCES or EROFS first.
        Err(_) if p.is_dir() => Ok(()),
        Err(e) => Err(os_err(e, p)),
    }
}

fn mkdir_once(p: &Path) -> io::Result<()> {
    fault("mkdir")?;
    fs::DirBuilder::new().mode(0o777).create(p)
}

const TEMP_CHARS: &[u8; 37] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
/// How many names `mkstemp` tries before it gives up.
const TEMP_TRIES: u32 = 10_000;

/// Eight random characters from `tempfile._RandomNameSequence`'s alphabet.
fn random_name() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u32(std::process::id());
    let mut x = h.finish();
    (0..8)
        .map(|_| {
            let c = TEMP_CHARS[(x % 37) as usize];
            x /= 37;
            char::from(c)
        })
        .collect()
}

/// `tempfile.mkstemp(dir=dir, prefix=prefix, suffix=suffix)`: the open file and its path.
fn mkstemp(dir: &Path, prefix: &str, suffix: &str) -> Result<(File, PathBuf), PyExc> {
    for _ in 0..TEMP_TRIES {
        let tmp = dir.join(format!("{prefix}{}{suffix}", random_name()));
        let opened = fault("open").and_then(|()| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(0o600)
                .open(&tmp)
        });
        match opened {
            Ok(f) => return Ok((f, tmp)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(os_err(e, &tmp)),
        }
    }
    Err(PyExc::OSError {
        errno: libc::EEXIST,
        filename: None,
    })
}

/// Writes `data` to a `mkstemp` file beside `path`, sets its mode to `mode` when given (with
/// `fchmod` while it is open: Python's `os.chmod(tmp)` without the path race), closes it with
/// the close error checked, and renames it onto `path`. On any failure the temp file is
/// unlinked and the error returned.
/// The parent folder must exist (callers run [`mkdir_parents`] first, at the point Python does).
pub(crate) fn replace_via_temp(
    path: &Path,
    data: &[u8],
    prefix: &str,
    suffix: &str,
    mode: Option<u32>,
) -> Result<(), PyExc> {
    let (mut file, tmp) = mkstemp(parent_of(path), prefix, suffix)?;
    let written = fault("write")
        .and_then(|()| file.write_all(data))
        .map_err(PyExc::from)
        .and_then(|()| match mode {
            Some(m) => fault("chmod")
                .and_then(|()| file.set_permissions(fs::Permissions::from_mode(m)))
                .map_err(|e| os_err(e, &tmp)),
            None => Ok(()),
        });
    // The close runs whatever happened before; its error only counts after a success.
    let closed = close(file).map_err(PyExc::from);
    let result = written.and(closed).and_then(|()| {
        // Python's error also names the target (`'tmp' -> 'path'`); `PyExc` holds one name.
        fault("rename")
            .and_then(|()| fs::rename(&tmp, path))
            .map_err(|e| os_err(e, &tmp))
    });
    if result.is_err() {
        let _ = fault("unlink").and_then(|()| fs::remove_file(&tmp));
    }
    result
}

/// `_atomic_write(path, data)`, with `mode` for its `0o644` and `prefix`/`suffix` for its
/// `.tmp-`/`.part`.
pub fn atomic_write(
    path: &Path,
    data: &[u8],
    mode: u32,
    prefix: &str,
    suffix: &str,
) -> Result<(), PyExc> {
    mkdir_parents(parent_of(path))?;
    replace_via_temp(path, data, prefix, suffix, Some(mode))
}

enum Kind {
    Absent,
    File,
    Other,
}

/// `_lstat_kind`: `absent` only for `ENOENT`; any other `lstat` error is `other`.
fn lstat_kind(p: &Path) -> Kind {
    match fault("lstat").and_then(|()| fs::symlink_metadata(p)) {
        Ok(m) if m.file_type().is_file() => Kind::File,
        Ok(_) => Kind::Other,
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => Kind::Absent,
        Err(_) => Kind::Other,
    }
}

/// The open behind `read_regular`: `O_RDONLY | O_NOFOLLOW | O_NONBLOCK`.
fn open_read_nofollow(p: &Path) -> io::Result<File> {
    fault("open")?;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(p)
}

fn is_regular(f: &File) -> Result<bool, PyExc> {
    fault("fstat")?;
    Ok(f.metadata()?.file_type().is_file())
}

/// `_read_regular(path)`: the bytes of a regular file, empty when it is absent. `max` caps the
/// read at that many bytes, as `fh.read(max)` (Python's helper has no cap: pass `None`).
pub fn read_regular(path: &Path, max: Option<u64>) -> Result<Vec<u8>, PyExc> {
    let refused = || {
        unsafe_target(format!(
            "refusing to use {}: it is not a regular file",
            path_str(path)
        ))
    };
    match lstat_kind(path) {
        Kind::Absent => return Ok(Vec::new()),
        Kind::Other => return Err(refused()),
        Kind::File => {}
    }
    let file = open_read_nofollow(path).map_err(|e| {
        unsafe_target(format!(
            "refusing to use {}: {}",
            path_str(path),
            strerror(e)
        ))
    })?;
    let body = || -> Result<Vec<u8>, PyExc> {
        if !is_regular(&file)? {
            return Err(refused());
        }
        let mut out = Vec::new();
        fault("read")?;
        match max {
            None => (&file).read_to_end(&mut out)?,
            Some(n) => (&file).take(n).read_to_end(&mut out)?,
        };
        Ok(out)
    };
    let result = body();
    // `finally: os.close(fd)`: a close error replaces the result.
    close(file)?;
    result
}

/// `os.open(path, O_WRONLY | O_CREAT | O_EXCL [| O_NOFOLLOW], mode)`, then write `data`. An
/// existing path (a symlink too) is `FileExistsError`; a failed write or close unlinks the
/// file.
pub fn create_excl(path: &Path, data: &[u8], mode: u32, nofollow: bool) -> Result<(), PyExc> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true).mode(mode);
    if nofollow {
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = fault("open")
        .and_then(|()| opts.open(path))
        .map_err(|e| os_err(e, path))?;
    let written = fault("write").and_then(|()| file.write_all(data));
    let closed = close(file);
    if let Err(e) = written.and(closed) {
        let _ = fault("unlink").and_then(|()| fs::remove_file(path));
        return Err(e.into());
    }
    Ok(())
}

/// `_append_line(path, line)`: appends `line` and `\n` to a regular file (created 0644 if
/// absent), first adding a `\n` when the file is non-empty and does not end in one.
pub fn append_line(path: &Path, line: &str) -> Result<(), PyExc> {
    let refused = || {
        unsafe_target(format!(
            "refusing to write {}: it is not a regular file",
            path_str(path)
        ))
    };
    if let Kind::Other = lstat_kind(path) {
        return Err(refused());
    }
    mkdir_parents(parent_of(path))?;
    let file = fault("open")
        .and_then(|()| {
            OpenOptions::new()
                .read(true)
                .append(true)
                .create(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(0o644)
                .open(path)
        })
        .map_err(|e| {
            unsafe_target(format!(
                "refusing to write {}: {}",
                path_str(path),
                strerror(e)
            ))
        })?;
    let body = || -> Result<(), PyExc> {
        fault("fstat")?;
        let info = file.metadata()?;
        if !info.file_type().is_file() {
            return Err(refused());
        }
        let size = info.len();
        let needs_newline = if size == 0 {
            false
        } else {
            let mut last = [0u8; 1];
            fault("read")?;
            let n = file.read_at(&mut last, size - 1)?;
            !(n == 1 && last[0] == b'\n')
        };
        let mut data = Vec::with_capacity(line.len() + 2);
        if needs_newline {
            data.push(b'\n');
        }
        data.extend_from_slice(line.as_bytes());
        data.push(b'\n');
        fault("write")?;
        // Python makes one `os.write` and ignores a short count; a regular file only writes
        // short when it then fails, which `write_all` reports.
        (&file).write_all(&data)?;
        Ok(())
    };
    let result = body();
    // `finally: os.close(fd)`: a close error replaces the result.
    close(file)?;
    result
}

/// `not os.path.islink(p) and os.path.isdir(p)`: a folder that is not a symlink.
pub fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_dir())
}

#[cfg(test)]
mod tests;
