//! `write_text` (ST1), with `settings_path` and `read_text`: `settings_patch.py`'s file
//! helpers.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::fsx;
use anthology_core::paths::home_raw;
use anthology_pycompat::json;
use anthology_pycompat::pyerr::PyExc;

use crate::scanner::py;

/// `str(pathlib.PurePosixPath(p))` for a non-empty `p`: `//` and `/./` collapse, a trailing `/`
/// goes, and exactly two leading slashes stay two.
fn pathlib_parts(p: &[u8]) -> (Vec<u8>, Vec<&[u8]>) {
    let root: &[u8] = if p.starts_with(b"//") && !p.starts_with(b"///") {
        b"//"
    } else if p.starts_with(b"/") {
        b"/"
    } else {
        b""
    };
    let parts = p
        .split(|&c| c == b'/')
        .filter(|c| !c.is_empty() && *c != b".")
        .collect();
    (root.to_vec(), parts)
}

/// `Path(base).joinpath(*more)` as pathlib prints it.
fn pathlib_join(base: &OsStr, more: &[&str]) -> PathBuf {
    let (mut out, mut parts) = pathlib_parts(base.as_bytes());
    parts.extend(more.iter().map(|m| m.as_bytes()));
    out.extend_from_slice(&parts.join(&b'/'));
    PathBuf::from(OsStr::from_bytes(&out))
}

/// `settings_path(env)`: `${CLAUDE_CONFIG_DIR:-<HOME>/.claude}/settings.json`, where HOME is
/// `env["HOME"] or expanduser("~")`.
pub fn settings_path(env: &Env) -> PathBuf {
    match env.get_os("CLAUDE_CONFIG_DIR") {
        Some(base) if !base.is_empty() => pathlib_join(base, &["settings.json"]),
        _ => pathlib_join(home_raw(env).as_os_str(), &[".claude", "settings.json"]),
    }
}

fn os_error(e: &io::Error, path: &Path) -> PyExc {
    let errno = match e.raw_os_error() {
        Some(n) => n,
        None => match PyExc::from(io::Error::from(e.kind())) {
            PyExc::OSError { errno, .. } => errno,
            _ => 5,
        },
    };
    PyExc::OSError {
        errno,
        filename: Some(path.to_string_lossy().into_owned()),
    }
}

/// `read_text(path)`: the file's text with newlines untouched; `None` when it does not exist
/// (Python's `""`). Invalid UTF-8 (a UTF-16 file, Latin-1) is CPython's `UnicodeDecodeError`.
pub fn read_text(path: &Path) -> Result<Option<String>, PyExc> {
    let bytes = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(os_error(&e, path)),
    };
    match String::from_utf8(bytes) {
        Ok(s) => Ok(Some(s)),
        // json::loads_utf8_sig reports a strict UTF-8 failure as CPython's exact
        // `UnicodeDecodeError` text, positions counted from the first byte (BOM included).
        Err(e) => match json::loads_utf8_sig(e.as_bytes()) {
            Err(j) if j.type_name() == "UnicodeDecodeError" => {
                Err(PyExc::UnicodeDecodeError(j.msg))
            }
            _ => unreachable!("invalid UTF-8 fails the strict decode"),
        },
    }
}

/// `pathlib.Path.is_symlink()`: `ENOENT`, `ENOTDIR`, `EBADF` and `ELOOP` read as `False`; any
/// other `lstat` error raises.
fn is_symlink(path: &Path) -> Result<bool, PyExc> {
    match fs::symlink_metadata(path) {
        Ok(m) => Ok(m.file_type().is_symlink()),
        Err(e) if matches!(e.raw_os_error(), Some(2 | 9) | Some(ENOTDIR) | Some(ELOOP)) => {
            Ok(false)
        }
        Err(e) => Err(os_error(&e, path)),
    }
}

#[cfg(target_os = "macos")]
const ELOOP: i32 = 62;
#[cfg(not(target_os = "macos"))]
const ELOOP: i32 = 40;
const ENOTDIR: i32 = 20;

/// `pathlib.Path.mkdir(parents=True, exist_ok=True)`.
fn mkdir_parents(p: &Path) -> Result<(), PyExc> {
    let p = if p.as_os_str().is_empty() {
        Path::new(".")
    } else {
        p
    };
    let mkdir = |p: &Path| fs::DirBuilder::new().mode(0o777).create(p);
    match mkdir(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let parent = p
                .parent()
                .filter(|q| !q.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            if p.parent().is_none() || parent == p {
                return Err(os_error(&e, p));
            }
            mkdir_parents(parent)?;
            match mkdir(p) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() != io::ErrorKind::NotFound && p.is_dir() => Ok(()),
                Err(e) => Err(os_error(&e, p)),
            }
        }
        Err(_) if p.is_dir() => Ok(()),
        Err(e) => Err(os_error(&e, p)),
    }
}

/// `write_text(path, text)`: replace the file atomically (a `.settings.*.tmp` file beside it,
/// then `os.replace`), keeping its mode (0644 for a new file). A symlinked settings.json is
/// written through to its target.
pub fn write_text(path: &Path, text: &str) -> Result<(), PyExc> {
    let target = if is_symlink(path)? {
        PathBuf::from(py::realpath_os(path.as_os_str()))
    } else {
        path.to_path_buf()
    };
    let parent = target.parent().unwrap_or(Path::new(""));
    mkdir_parents(parent)?;
    let mode = match fs::metadata(&target) {
        Ok(m) => m.permissions().mode() & 0o7777,
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0o644,
        Err(e) => return Err(os_error(&e, &target)),
    };
    fsx::atomic_write(&target, text.as_bytes(), mode, ".settings.", ".tmp")
}
