//! The only `unsafe` code in the workspace (GC-18): thin, checked wrappers over libc.
//!
//! - [`passwd_home`]: `getpwuid_r(getuid())`'s `pw_dir`, as `pwd.getpwuid(os.getuid()).pw_dir`.
//! - [`user_home`]: `getpwnam_r(name)`'s `pw_dir`, as `pwd.getpwnam(name).pw_dir`.
//! - [`local_date`]: `tzset` then `localtime_r`, as `datetime.date.fromtimestamp`.
//! - [`hostname`]: `gethostname`, as `socket.gethostname`.
//! - [`uname`]: `uname`'s `sysname` and `machine`, as `platform.system()` and `machine()`.
//! - [`close`]: `close(2)` with its error reported, as Python's `os.close` and file `close()`.
//! - [`new_session`] and [`kill_group`]: a child in its own session, and `SIGKILL` for its whole
//!   process group (`anthology check-repo` runs git and the scanners this way).
//! - [`catch_termination`]: `SIGINT`, `SIGTERM` and `SIGHUP` reported on a pipe instead of ending
//!   the process, so `check-repo` can stop its children and remove its folders first.
//!
//! `O_NOFOLLOW` and the other open flags need no `unsafe`: `fsx` passes the `libc` constants to
//! `OpenOptionsExt::custom_flags`.

use std::ffi::{CStr, OsString};
use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::IntoRawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};

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
        // SAFETY: on success `result` points at `pwd`, initialized by the lookup, whose
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

/// Makes the child of `cmd` the leader of a new session (`setsid(2)`): it gets its own process
/// group, which `kill_group` can end as a whole, and no controlling terminal, so nothing it starts
/// (an `ssh` password prompt, for one) can read the terminal of the caller.
pub fn new_session(cmd: &mut Command) {
    // SAFETY: the closure runs in the forked child before `exec` and calls only `setsid`, which
    // POSIX lists as async-signal-safe; it allocates nothing and touches no lock.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

/// Sends `SIGKILL` to every process of the group `pgid` (the pid of a child made with
/// `new_session`). Nothing happens for 0, which `kill(2)` would read as the caller's own group,
/// or for a number that is no pid; a group that is already gone is not an error.
pub fn kill_group(pgid: u32) {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return;
    };
    if pgid == 0 {
        return;
    }
    // SAFETY: `kill` takes two integers; a negative pid names the process group `pgid`, and
    // nothing in memory is read or written.
    unsafe { libc::kill(-pgid, libc::SIGKILL) };
}

/// Whether the child `pid` has ended, without reaping it (`waitid(2)` with `WNOWAIT`): an ended
/// child stays a zombie, so its pid, which is also the id of the group it leads, cannot be given to
/// another process. The caller can signal the group until it reaps the child (`Child::wait`), and
/// must not after. `Child::try_wait` reaps at once, which is why it is not used for this.
///
/// An error is a pid that is not a child of this process.
pub fn exited(pid: u32) -> io::Result<bool> {
    let id = libc::id_t::from(pid);
    loop {
        // SAFETY: an all-zero `siginfo_t` is a valid value; `waitid` fills it in, and with
        // `WNOHANG` leaves it alone when no child is ready, which is why its `si_pid` starts at 0.
        let mut info: libc::siginfo_t = unsafe { MaybeUninit::zeroed().assume_init() };
        // SAFETY: `info` is a writable `siginfo_t`; the other arguments are plain integers.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                id,
                &raw mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            // SAFETY: `info` was zeroed and `waitid` returned 0, so the field is readable.
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EINTR) {
            return Err(e);
        }
    }
}

/// The write end of the pipe `on_signal` reports on, or -1 before `catch_termination` ran.
static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

/// The handler of `catch_termination`: writes the signal number, as one byte, to the pipe. It
/// calls only `write` (async-signal-safe) and leaves `errno` as it found it.
extern "C" fn on_signal(sig: libc::c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    let byte = sig as u8;
    // SAFETY: `errno_slot` points at this thread's `errno`; `byte` is one readable byte and `fd`
    // is the write end `catch_termination` leaked, which stays open for the life of the process.
    unsafe {
        let slot = errno_slot();
        let saved = *slot;
        libc::write(fd, (&raw const byte).cast::<libc::c_void>(), 1);
        *slot = saved;
    }
}

/// The address of the calling thread's `errno`. The C library names this call differently per
/// system, so each family has its own definition below and an unlisted target does not build
/// (a handler that cannot keep `errno` intact must not be built by guess).
///
/// # Safety
/// Only meaningful on the thread that calls it.
#[cfg(any(
    all(target_os = "linux", not(target_os = "android")),
    target_os = "emscripten",
    target_os = "hurd",
    target_os = "l4re",
    target_os = "fuchsia",
    target_os = "redox",
    target_os = "dragonfly"
))]
unsafe fn errno_slot() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}

/// See the first `errno_slot`: Android, Cygwin, NetBSD and OpenBSD call it `__errno`.
///
/// # Safety
/// Only meaningful on the thread that calls it.
#[cfg(any(
    target_os = "android",
    target_os = "cygwin",
    target_os = "netbsd",
    target_os = "openbsd"
))]
unsafe fn errno_slot() -> *mut libc::c_int {
    unsafe { libc::__errno() }
}

/// See the first `errno_slot`: the Apple systems and FreeBSD call it `__error`.
///
/// # Safety
/// Only meaningful on the thread that calls it.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
    target_os = "freebsd"
))]
unsafe fn errno_slot() -> *mut libc::c_int {
    unsafe { libc::__error() }
}

/// See the first `errno_slot`: Solaris and illumos call it `___errno`.
///
/// # Safety
/// Only meaningful on the thread that calls it.
#[cfg(any(target_os = "solaris", target_os = "illumos"))]
unsafe fn errno_slot() -> *mut libc::c_int {
    unsafe { libc::___errno() }
}

/// Makes `SIGINT`, `SIGTERM` and `SIGHUP` stop ending the process: each one is written as one
/// byte (its number) to the pipe this returns, which a thread of the caller reads to clean up and
/// exit. Children are not affected: a caught signal goes back to its default over `exec`.
///
/// One pipe serves the whole process, so a second call is an `AlreadyExists` error.
pub fn catch_termination() -> io::Result<std::io::PipeReader> {
    let (reader, writer) = std::io::pipe()?;
    let fd = std::os::fd::OwnedFd::from(writer).into_raw_fd();
    // The handler must never block on a full pipe: a write that cannot happen is dropped.
    // SAFETY: `fd` is the write end of the pipe made above and is owned here.
    let nonblocking = unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        flags != -1 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) != -1
    };
    if !nonblocking {
        let e = io::Error::last_os_error();
        // SAFETY: `fd` is owned here and not shared with anything yet.
        unsafe { libc::close(fd) };
        return Err(e);
    }
    if SIGNAL_PIPE
        .compare_exchange(-1, fd, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // SAFETY: as above.
        unsafe { libc::close(fd) };
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "the termination signals are already caught",
        ));
    }
    // SAFETY: `act` is fully set before the calls (zeroed, then a handler, a full mask and
    // `SA_RESTART`), and `on_signal` has the signature `sigaction` expects.
    unsafe {
        let mut act: libc::sigaction = MaybeUninit::zeroed().assume_init();
        act.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigfillset(&mut act.sa_mask);
        act.sa_flags = libc::SA_RESTART;
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            if libc::sigaction(sig, &act, std::ptr::null_mut()) != 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    Ok(reader)
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

    fn pid_of(child: &std::process::Child) -> libc::pid_t {
        libc::pid_t::try_from(child.id()).expect("pid fits")
    }

    #[test]
    fn a_new_session_makes_the_child_its_own_session_and_group_leader() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        new_session(&mut cmd);
        let mut child = cmd.spawn().expect("spawn sleep");
        let pid = pid_of(&child);
        // SAFETY: getsid and getpgid only read process state.
        let (sid, pgid, mine) = unsafe { (libc::getsid(pid), libc::getpgid(pid), libc::getsid(0)) };
        kill_group(child.id());
        child.wait().expect("wait");
        assert_eq!(sid, pid, "the child leads a new session");
        assert_eq!(pgid, pid, "the child leads its own process group");
        assert_ne!(sid, mine, "not the session of the test");
    }

    #[test]
    fn kill_group_kills_the_grandchildren_too() {
        use std::io::{BufRead, BufReader, Read};
        // The background `sleep` holds the write end of the pipe, so the pipe reaches its end only
        // when `sleep` is dead. (Checking the pid would not do: in a container nothing reaps the
        // orphan, and a zombie still answers signal 0.)
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 60 & echo started; wait"])
            .stdout(std::process::Stdio::piped());
        new_session(&mut cmd);
        let mut child = cmd.spawn().expect("spawn sh");
        let mut pipe = BufReader::new(child.stdout.take().expect("stdout"));
        let mut line = String::new();
        pipe.read_line(&mut line).expect("the first line");
        assert_eq!(line, "started\n");
        kill_group(child.id());
        child.wait().expect("wait");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut rest = Vec::new();
            let _ = tx.send(pipe.read_to_end(&mut rest).is_ok());
        });
        let ended = rx.recv_timeout(std::time::Duration::from_secs(10));
        assert_eq!(ended, Ok(true), "the grandchild survived kill_group");
    }

    #[test]
    fn catch_termination_turns_each_signal_into_a_byte_and_installs_once() {
        use std::io::Read;
        let mut pipe = catch_termination().expect("install the handlers");
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            // SAFETY: `raise` sends the signal to this thread; the handler is installed.
            assert_eq!(unsafe { libc::raise(sig) }, 0);
            let mut byte = [0u8; 1];
            pipe.read_exact(&mut byte)
                .expect("the handler wrote the signal number");
            assert_eq!(i32::from(byte[0]), sig);
        }
        // One pipe per process: a second install is refused, and the first keeps working.
        let again = catch_termination().expect_err("second install");
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        // SAFETY: as above.
        assert_eq!(unsafe { libc::raise(libc::SIGTERM) }, 0);
        let mut byte = [0u8; 1];
        pipe.read_exact(&mut byte).expect("still installed");
        assert_eq!(i32::from(byte[0]), libc::SIGTERM);
    }

    #[test]
    fn exited_sees_an_ended_child_without_reaping_it() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "exit 7"]);
        let mut child = cmd.spawn().expect("spawn sh");
        let start = std::time::Instant::now();
        while !exited(child.id()).expect("waitid") {
            assert!(start.elapsed().as_secs() < 30, "the child never ended");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Asking again changes nothing: the child is still there to be reaped, with its status.
        assert!(exited(child.id()).expect("waitid"));
        let status = child.try_wait().expect("try_wait").expect("not reaped yet");
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn exited_is_false_while_the_child_runs_and_an_error_for_a_stranger() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        let mut child = cmd.spawn().expect("spawn sleep");
        assert!(!exited(child.id()).expect("waitid"));
        child.kill().expect("kill");
        child.wait().expect("wait");
        // Reaped: the pid is no child of ours any more.
        assert!(exited(child.id()).is_err());
    }

    #[test]
    fn errno_slot_is_the_errno_of_the_calling_thread() {
        // SAFETY: the slot belongs to this thread and is read and written on it only.
        unsafe { *errno_slot() = libc::ENOENT };
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOENT)
        );
        // SAFETY: as above.
        unsafe { *errno_slot() = libc::EPERM };
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EPERM));
    }

    #[test]
    fn kill_group_ignores_pid_zero_and_overflowing_ids() {
        // pid 0 would be the caller's own group; a pid above i32::MAX is no pid at all.
        kill_group(0);
        kill_group(u32::MAX);
    }

    #[test]
    fn local_date_is_a_calendar_date() {
        let (y, m, d) = local_date(0).expect("localtime_r");
        assert!((1969..=1970).contains(&y));
        assert!((1..=12).contains(&m) && (1..=31).contains(&d));
    }
}
