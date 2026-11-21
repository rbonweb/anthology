//! `pycompat::pyerr`: Python exception types and their `str()` text (plan §3.2, GC-16).
//!
//! F3 declared this module so `anthology-core` could return [`PyExc`] (plan §3.3); P1 owns it.
//! The errno text in `[Errno N] <text>` comes from the committed per-OS tables in
//! [`crate::errno_tables`] (glibc Linux, macOS; GC-16, I-10), never libc `strerror` or
//! `io::Error`'s `Display`, and a filename is printed with Python's `repr`.

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
    /// `ENOTCAPABLE`: CPython 3.11.1+ maps it to `PermissionError` where the platform has it.
    pub const ENOTCAPABLE: Option<i32> = Some(107);
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
    /// Linux has no `ENOTCAPABLE`.
    pub const ENOTCAPABLE: Option<i32> = None;
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

    /// Python's `str(exc)`: `[Errno N] <text>` or `[Errno N] <text>: <repr(filename)>` for an
    /// `OSError` (the text is `os.strerror(N)` on this OS), `repr(key)` for a `KeyError`, and
    /// the message for the rest.
    pub fn py_str(&self) -> String {
        match self {
            PyExc::OSError { errno: e, filename } => {
                let text = crate::errno_tables::strerror(*e);
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
        e if Some(e) == errno::ENOTCAPABLE => "PermissionError",
        _ => "OSError",
    }
}

fn repr_str(s: &str) -> String {
    crate::str::repr(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enotcapable_is_a_permission_error_on_macos_only() {
        let name = os_error_class(107);
        if cfg!(target_os = "macos") {
            assert_eq!(name, "PermissionError");
        } else {
            assert_eq!(name, "OSError");
        }
    }

    #[test]
    fn os_errors_take_cpythons_subclass_names_and_forms() {
        let e = PyExc::OSError {
            errno: ENOENT,
            filename: Some("/tmp/it's".into()),
        };
        assert_eq!(e.type_name(), "FileNotFoundError");
        assert_eq!(
            e.py_str(),
            "[Errno 2] No such file or directory: \"/tmp/it's\""
        );
        let e = PyExc::OSError {
            errno: EEXIST,
            filename: None,
        };
        assert_eq!(e.type_name(), "FileExistsError");
        assert_eq!(e.py_str(), "[Errno 17] File exists");
        let e = PyExc::OSError {
            errno: EACCES,
            filename: Some("caf\u{e9}\u{200b}\n".into()),
        };
        assert_eq!(
            e.py_str(),
            "[Errno 13] Permission denied: 'caf\u{e9}\\u200b\\n'"
        );
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
