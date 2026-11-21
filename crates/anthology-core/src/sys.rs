//! The only `unsafe` code in the workspace (GC-18): thin, checked wrappers over libc.
//!
//! - [`passwd_home`]: `getpwuid_r(getuid())`'s `pw_dir`, as `pwd.getpwuid(os.getuid()).pw_dir`.
//! - [`user_home`]: `getpwnam_r(name)`'s `pw_dir`, as `pwd.getpwnam(name).pw_dir`.
//! - [`local_date`]: `tzset` then `localtime_r`, as `datetime.date.fromtimestamp`.
//! - [`hostname`]: `gethostname`, as `socket.gethostname`.
//! - [`uname`]: `uname`'s `sysname` and `machine`, as `platform.system()` and `machine()`.
//! - [`close`]: `close(2)` with its error reported, as Python's `os.close` and file `close()`.
//!
//! `O_NOFOLLOW` and the other open flags need no `unsafe`: `fsx` passes the `libc` constants to
//! `OpenOptionsExt::custom_flags`.

use std::ffi::{CStr, OsString};
use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::IntoRawFd;

unsafe extern "C" {
    /// POSIX `tzset(3)` (the `libc` crate does not declare it).
    fn tzset();
}

/// The largest buffer the passwd lookups try before they give up.
const PASSWD_BUF_MAX: usize = 1 << 20;

/// Runs a `getpw*_r` call, growing the buffer on `ERANGE`, and returns the entry's `pw_dir`.
/// `call` gets the `passwd` storage, the string buffer and its size, and the result out pointer.
fn pw_dir<F>(call: F) -> Option<OsString>
where
    F: Fn(*mut libc::passwd, *mut libc::c_char, usize, *mut *mut libc::passwd) -> libc::c_int,
{
    let mut size = 1024usize;
    loop {
        let mut buf = vec![0u8; size];
        let mut pwd = MaybeUninit::<libc::passwd>::uninit();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // `pwd` is writable storage for one `passwd`, `buf` is `size` writable bytes and
        // `result` is a valid out pointer; all three outlive the call.
        let rc = call(
            pwd.as_mut_ptr(),
            buf.as_mut_ptr().cast::<libc::c_char>(),
            size,
            &mut result,
        );
        if rc == libc::ERANGE && size < PASSWD_BUF_MAX {
            size *= 2;
            continue;
        }
        if rc != 0 || result.is_null() {
            return None;
        }
        // SAFETY: on success `result` points at `pwd`, initialised by the lookup, whose
        // `pw_dir` is null or a NUL-terminated string inside `buf`, which is still alive.
        let dir = unsafe { (*result).pw_dir };
        if dir.is_null() {
            return None;
        }
        // SAFETY: `dir` is non-null and NUL-terminated (above); the bytes are copied before
        // `buf` is dropped.
        let bytes = unsafe { CStr::from_ptr(dir) }.to_bytes().to_vec();
        return Some(OsString::from_vec(bytes));
    }
}

/// The current user's home directory from the password database, or `None` when the uid has
/// no entry (Python: `KeyError`) or the lookup fails.
pub fn passwd_home() -> Option<OsString> {
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    // SAFETY: the pointers come from `pw_dir`, which upholds getpwuid_r's contract.
    pw_dir(|pwd, buf, size, result| unsafe { libc::getpwuid_r(uid, pwd, buf, size, result) })
}

/// The home directory of the user called `name` from the password database
/// (`pwd.getpwnam(name).pw_dir`), or `None` when there is no such user, the lookup fails, or
/// `name` holds a NUL byte.
pub fn user_home(name: &[u8]) -> Option<OsString> {
    let name = std::ffi::CString::new(name).ok()?;
    // SAFETY: `name` is NUL-terminated and outlives the call; the other pointers come from
    // `pw_dir`, which upholds getpwnam_r's contract.
    pw_dir(|pwd, buf, size, result| unsafe {
        libc::getpwnam_r(name.as_ptr(), pwd, buf, size, result)
    })
}

/// The local calendar date `(year, month 1-12, day 1-31)` of the epoch second `t`, after
/// `tzset()` so the process's `TZ` applies. `None` if `localtime_r` fails.
pub fn local_date(t: i64) -> Option<(i32, u8, u8)> {
    // time_t is i64 on every supported target (64-bit Linux and macOS). libc marks musl's
    // `time_t` deprecated for its 32-bit time64 transition, which 64-bit targets don't have.
    #[allow(deprecated)]
    let t = t as libc::time_t;
    let mut tm = MaybeUninit::<libc::tm>::uninit();
    // SAFETY: tzset reads `TZ` with getenv and updates libc's own time zone state. That is
    // sound while no thread changes the environment at the same time, which safe Rust cannot
    // do: in edition 2024 `std::env::set_var` and `remove_var` are `unsafe`, with that very
    // contract, and the workspace forbids `unsafe` outside this module.
    unsafe { tzset() };
    // SAFETY: `t` is a valid time_t and `tm` is writable storage for one `tm`.
    let r = unsafe { libc::localtime_r(&t, tm.as_mut_ptr()) };
    if r.is_null() {
        return None;
    }
    // SAFETY: localtime_r returned non-null, so it filled `tm`.
    let tm = unsafe { tm.assume_init() };
    Some((
        tm.tm_year.checked_add(1900)?,
        u8::try_from(tm.tm_mon + 1).ok()?,
        u8::try_from(tm.tm_mday).ok()?,
    ))
}

/// The host name, or `None` if `gethostname` fails.
pub fn hostname() -> Option<OsString> {
    let mut buf = vec![0u8; 1025];
    // SAFETY: `buf` is `buf.len()` writable bytes; one byte is kept back for the NUL.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast::<libc::c_char>(), buf.len() - 1) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len() - 1);
    buf.truncate(end);
    Some(OsString::from_vec(buf))
}

/// Closes `file` and returns `close(2)`'s error, which dropping a `File` ignores (a delayed
/// `ENOSPC` or `EIO`, on NFS for example). `EINTR` counts as success: the descriptor is gone
/// either way, and Python ignores it too (PEP 475).
pub fn close(file: File) -> io::Result<()> {
    let fd = file.into_raw_fd();
    // SAFETY: `fd` came from `into_raw_fd`, so this function owns it and nothing else closes or
    // uses it; it is closed exactly once, here.
    if unsafe { libc::close(fd) } == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EINTR) {
        Ok(())
    } else {
        Err(e)
    }
}

/// `(sysname, machine)` from `uname(2)`, for example `("Darwin", "arm64")` or `("Linux",
/// "x86_64")`. `None` if the call fails.
pub fn uname() -> Option<(String, String)> {
    let mut u = MaybeUninit::<libc::utsname>::uninit();
    // SAFETY: `u` is writable storage for one `utsname`.
    if unsafe { libc::uname(u.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: uname returned 0, so it filled `u`.
    let u = unsafe { u.assume_init() };
    Some((field(&u.sysname), field(&u.machine)))
}

/// A NUL-terminated `utsname` field as text (lossy, as `platform` decodes it).
fn field(f: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = f
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c.to_ne_bytes()[0])
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_closes_and_reports_success() {
        let f = File::open("/dev/null").expect("open");
        close(f).expect("close");
    }

    #[test]
    fn uname_names_a_supported_kernel() {
        let (system, machine) = uname().expect("uname");
        assert!(["Linux", "Darwin"].contains(&system.as_str()), "{system}");
        assert!(!machine.is_empty());
    }

    #[test]
    fn user_home_looks_a_user_up_by_name() {
        let root = user_home(b"root").expect("root has a passwd entry");
        assert!(!root.is_empty());
        assert_eq!(user_home(b"no-such-user-ms1-f"), None);
        assert_eq!(user_home(b"ro\0ot"), None);
        assert_eq!(user_home(b""), None);
    }

    #[test]
    fn hostname_is_not_empty() {
        assert!(!hostname().expect("gethostname").is_empty());
    }

    #[test]
    fn local_date_is_a_calendar_date() {
        let (y, m, d) = local_date(0).expect("localtime_r");
        assert!((1969..=1970).contains(&y));
        assert!((1..=12).contains(&m) && (1..=31).contains(&d));
    }
}
