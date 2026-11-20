//! `pycompat::pyerr`: Python exception types and their `str()` text (plan §3.2, GC-16).
//!
//! F3 declares this module so `anthology-core` can return [`PyExc`] (plan §3.3) before P1
//! lands; P1 owns it from then on. **Provisional:** the errno text that [`PyExc::py_str`] puts
//! in `[Errno N] <text>` comes from one private function that returns a `strerror`-free
//! placeholder (`errno N`) until P1 commits the per-OS tables in `errno_tables` (GC-16, I-10).
//! The surrounding forms are already CPython's. Until then, compare errno numbers, not texts.

/// A Python exception: its class and what `str(exc)` needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PyExc {
    /// `OSError(errno, strerror[, filename])`. The class name follows the errno, as CPython
    /// picks the subclass (`ENOENT` is `FileNotFoundError`, and so on).
    OSError {
        errno: i32,
        filename: Option<String>,
    },
    ValueError(String),
    /// The key's text; `str(KeyError(k))` is `repr(k)`.
    KeyError(String),
    JSONDecodeError(String),
    UnicodeDecodeError(String),
    TimeoutExpired(String),
    /// Any other class: `(class name, str(exc))`.
    Other(&'static str, String),
}

/// The errno values CPython maps to an `OSError` subclass (`Objects/exceptions.c`).
#[cfg(target_os = "macos")]
mod errno {
    pub const EAGAIN: i32 = 35;
    pub const EINPROGRESS: i32 = 36;
    pub const EALREADY: i32 = 37;
    pub const ECONNABORTED: i32 = 53;
    pub const ECONNRESET: i32 = 54;
    pub const ESHUTDOWN: i32 = 58;
    pub const ETIMEDOUT: i32 = 60;
    pub const ECONNREFUSED: i32 = 61;
}
#[cfg(not(target_os = "macos"))]
mod errno {
    pub const EAGAIN: i32 = 11;
    pub const ECONNABORTED: i32 = 103;
    pub const ECONNRESET: i32 = 104;
    pub const ESHUTDOWN: i32 = 108;
    pub const ETIMEDOUT: i32 = 110;
    pub const ECONNREFUSED: i32 = 111;
    pub const EALREADY: i32 = 114;
    pub const EINPROGRESS: i32 = 115;
}
// The same on Linux and macOS.
const EPERM: i32 = 1;
const ENOENT: i32 = 2;
const ESRCH: i32 = 3;
const EINTR: i32 = 4;
const EIO: i32 = 5;
const ECHILD: i32 = 10;
const EACCES: i32 = 13;
const EEXIST: i32 = 17;
const ENOTDIR: i32 = 20;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const EPIPE: i32 = 32;

impl PyExc {
    /// The Python class name, as `type(exc).__name__`.
    pub fn type_name(&self) -> &'static str {
        match self {
            PyExc::OSError { errno: e, .. } => os_error_class(*e),
            PyExc::ValueError(_) => "ValueError",
            PyExc::KeyError(_) => "KeyError",
            PyExc::JSONDecodeError(_) => "JSONDecodeError",
            PyExc::UnicodeDecodeError(_) => "UnicodeDecodeError",
            PyExc::TimeoutExpired(_) => "TimeoutExpired",
            PyExc::Other(name, _) => name,
        }
    }

    /// Python's `str(exc)`: `[Errno N] <text>` or `[Errno N] <text>: '<filename>'` for an
    /// `OSError`, `repr(key)` for a `KeyError`, and the message for the rest. The errno text is
    /// provisional (module docs).
    pub fn py_str(&self) -> String {
        match self {
            PyExc::OSError { errno: e, filename } => {
                let text = errno_text(*e);
                match filename {
                    Some(f) => format!("[Errno {e}] {text}: {}", repr_str(f)),
                    None => format!("[Errno {e}] {text}"),
                }
            }
            PyExc::KeyError(k) => repr_str(k),
            PyExc::ValueError(m)
            | PyExc::JSONDecodeError(m)
            | PyExc::UnicodeDecodeError(m)
            | PyExc::TimeoutExpired(m)
            | PyExc::Other(_, m) => m.clone(),
        }
    }
}

impl From<std::io::Error> for PyExc {
    /// `OSError { errno, filename: None }`. An error without an OS code gets the errno its
    /// kind stands for (`EIO` when none fits).
    fn from(e: std::io::Error) -> PyExc {
        use std::io::ErrorKind as K;
        let code = e.raw_os_error().unwrap_or(match e.kind() {
            K::NotFound => ENOENT,
            K::PermissionDenied => EACCES,
            K::AlreadyExists => EEXIST,
            K::Interrupted => EINTR,
            K::BrokenPipe => EPIPE,
            K::TimedOut => errno::ETIMEDOUT,
            K::WouldBlock => errno::EAGAIN,
            K::InvalidInput | K::InvalidData => EINVAL,
            K::NotADirectory => ENOTDIR,
            K::IsADirectory => EISDIR,
            _ => EIO,
        });
        PyExc::OSError {
            errno: code,
            filename: None,
        }
    }
}

fn os_error_class(e: i32) -> &'static str {
    match e {
        EPERM | EACCES => "PermissionError",
        ENOENT => "FileNotFoundError",
        ESRCH => "ProcessLookupError",
        EINTR => "InterruptedError",
        ECHILD => "ChildProcessError",
        EEXIST => "FileExistsError",
        ENOTDIR => "NotADirectoryError",
        EISDIR => "IsADirectoryError",
        EPIPE | errno::ESHUTDOWN => "BrokenPipeError",
        errno::EAGAIN | errno::EALREADY | errno::EINPROGRESS => "BlockingIOError",
        errno::ECONNABORTED => "ConnectionAbortedError",
        errno::ECONNRESET => "ConnectionResetError",
        errno::ECONNREFUSED => "ConnectionRefusedError",
        errno::ETIMEDOUT => "TimeoutError",
        _ => "OSError",
    }
}

/// The text after `[Errno N] `.
// P1 replaces with errno_tables: the committed per-OS strerror tables (GC-16, I-10). Never libc
// `strerror` or `io::Error`'s `Display`.
fn errno_text(e: i32) -> String {
    format!("errno {e}")
}

/// `repr(s)` for the ASCII range, which is what filenames and keys hold in practice. P1's
/// `pycompat::str` owns the full `isprintable` tables; until then a non-ASCII character is
/// printed as is.
fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_errors_take_cpythons_subclass_names_and_forms() {
        let e = PyExc::OSError {
            errno: ENOENT,
            filename: Some("/tmp/it's".into()),
        };
        assert_eq!(e.type_name(), "FileNotFoundError");
        assert_eq!(e.py_str(), "[Errno 2] errno 2: \"/tmp/it's\"");
        let e = PyExc::OSError {
            errno: EEXIST,
            filename: None,
        };
        assert_eq!(e.type_name(), "FileExistsError");
        assert_eq!(e.py_str(), "[Errno 17] errno 17");
        assert_eq!(
            PyExc::OSError {
                errno: 9999,
                filename: None
            }
            .type_name(),
            "OSError"
        );
        assert_eq!(
            PyExc::OSError {
                errno: errno::ETIMEDOUT,
                filename: None
            }
            .type_name(),
            "TimeoutError"
        );
    }

    #[test]
    fn the_other_classes_print_their_message() {
        assert_eq!(PyExc::ValueError("bad".into()).py_str(), "bad");
        assert_eq!(PyExc::KeyError("k\n".into()).py_str(), "'k\\n'");
        assert_eq!(
            PyExc::Other("UnsafeTarget", "no".into()).type_name(),
            "UnsafeTarget"
        );
        assert_eq!(PyExc::Other("UnsafeTarget", "no".into()).py_str(), "no");
    }

    #[test]
    fn io_errors_keep_their_os_code_or_get_one() {
        let e: PyExc = std::io::Error::from_raw_os_error(EISDIR).into();
        assert_eq!(
            e,
            PyExc::OSError {
                errno: EISDIR,
                filename: None
            }
        );
        let e: PyExc = std::io::Error::from(std::io::ErrorKind::NotFound).into();
        assert_eq!(e.type_name(), "FileNotFoundError");
        let e: PyExc = std::io::Error::other("x").into();
        assert_eq!(
            e,
            PyExc::OSError {
                errno: EIO,
                filename: None
            }
        );
    }
}
