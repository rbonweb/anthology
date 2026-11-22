//! Shared pieces of the shim suite: the GC-14 strings (verbatim), payloads, fake binaries and
//! assertions.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anthology_testkit::bs::{self, Release, Run, Sandbox, Shell};

pub const V: &str = "0.1.0";
pub const GUARD: &str = "plugins/anthology-guardrails/hooks/guard.sh";
pub const G_BOOT: &str = "plugins/anthology-guardrails/hooks/bootstrap.sh";
pub const A_BOOT: &str = "plugins/anthology/hooks/bootstrap.sh";
pub const SESSION: &str = "plugins/anthology/hooks/session-start.sh";
pub const WRAP: &str = "plugins/anthology/bin/anthology";
pub const WRAP_MEM: &str = "plugins/anthology/bin/anthology-mem";
pub const G_LIB: &str = "plugins/anthology-guardrails/hooks/lib.sh";
pub const A_LIB: &str = "plugins/anthology/hooks/lib.sh";

pub const BASH: &[u8] = br#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls -la"}}"#;
pub const READ: &[u8] = br#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"/tmp/x"}}"#;
pub const MCP: &[u8] =
    br#"{"hook_event_name":"PreToolUse","tool_name":"mcp__browser__navigate","tool_input":{}}"#;
pub const NESTED: &[u8] = br#"{"tool_name":"Read","tool_input":{"tool_name":"Bash"}}"#;

pub const G1: &str = "Blocked by anthology-guardrails (G1): run it inside Docker (OrbStack); for browser work use BrowserOS Neo";

// --- GC-14 (verbatim) -----------------------------------------------------------------------

pub const P: &str = "; if this keeps happening, disable anthology-guardrails with /plugin";

pub fn not_installed(reason: &str) -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary is not installed ({reason}), so shell commands, file edits and MCP tools are blocked; run /anthology:setup (or start a new session while online) to install it{P}"
    )
}

pub fn rate_limited() -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary is not installed yet and the last install attempt was less than a minute ago; retry in a minute, or run /anthology:setup{P}"
    )
}

pub fn set_aside(why: &str) -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary did not work ({why}) and was set aside; it is reinstalled on a later call (at most once a minute), or run /anthology:setup{P}"
    )
}

pub fn not_set_aside(why: &str, bindir: &Path) -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary did not work ({why}) and could not be set aside because {} is not writable; fix that folder's permissions, then start a new session{P}",
        bindir.display()
    )
}

pub fn cannot_write(bindir: &Path) -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary cannot be installed because {} is not writable; fix that folder's permissions, then start a new session{P}",
        bindir.display()
    )
}

pub fn three_strikes(version: &str) -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary {version} failed to run 3 times in 24 hours, so tool calls no longer reinstall it; a new session tries once more at most once an hour, or disable anthology-guardrails with /plugin until a fixed version ships"
    )
}

pub fn probe_failed(version: &str) -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary {version} does not run on this machine (wrong architecture, or a home folder mounted noexec); disable anthology-guardrails with /plugin, then fix the cause and start a new session"
    )
}

pub fn lock_invalid() -> String {
    format!(
        "Blocked by anthology-guardrails (setup): anthology.lock in the anthology-guardrails plugin is not valid; reinstall anthology-guardrails{P}"
    )
}

pub fn no_home() -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary cannot be found because {R_HOME}; set it to an absolute path, then start a new session{P}"
    )
}

pub fn unsupported(os: &str, arch: &str) -> String {
    format!(
        "Blocked by anthology-guardrails (setup): there is no guard binary for this platform ({os}/{arch}); Anthology supports macOS and Linux on x86_64 and arm64. Disable anthology-guardrails (/plugin) to use Claude Code here"
    )
}

pub fn no_downloader(url: &str, bindir: &Path) -> String {
    format!(
        "Blocked by anthology-guardrails (setup): the guard binary is not installed and no downloader (curl, wget or node) was found; install it by hand: download {url} into {}/ and start a new session; or disable anthology-guardrails with /plugin",
        bindir.display()
    )
}

pub const FALLBACK: &str = "Blocked by anthology-guardrails (setup): the guard binary is not installed (the guard shim could not finish), so shell commands, file edits and MCP tools are blocked; run /anthology:setup (or start a new session while online) to install it; if this keeps happening, disable anthology-guardrails with /plugin";

pub fn sys_anthology(reason: &str) -> String {
    format!(
        "{{\"systemMessage\":\"anthology: could not install the anthology binary ({reason}); the routing guide, memory and rtk are off until it installs. Start a new session while online, or run /anthology:setup.\"}}"
    )
}

pub fn sys_guardrails(reason: &str) -> String {
    format!(
        "{{\"systemMessage\":\"anthology-guardrails: could not install the guard binary ({reason}); shell commands, file edits and MCP tools are blocked until it installs. Start a new session while online, or run /anthology:setup; if this keeps happening, disable anthology-guardrails with /plugin.\"}}"
    )
}

pub fn no_downloader_reason(url: &str) -> String {
    format!(
        "no downloader (curl, wget or node); download {url} into the anthology/bin folder of your Claude config and start a new session"
    )
}

pub fn incomplete(why: &str) -> String {
    format!(
        "{{\"systemMessage\":\"anthology: session start did not complete ({why}); the routing guide may be missing\"}}"
    )
}

pub fn wrapper_failed(name: &str, reason: &str) -> String {
    format!(
        "{name}: the anthology binary is not installed ({reason}); start a new session while online, or run /anthology:setup"
    )
}

pub const R_PROBE: &str = "the binary does not run on this machine";
pub const R_DOWNLOAD: &str = "download failed";
pub const R_SHA: &str = "sha256 does not match anthology.lock";
pub const R_MEMBER: &str = "the archive has no anthology binary";
pub const R_LOCK: &str = "anthology.lock is not valid";
pub const R_WRITE: &str = "cannot write the install folder";
pub const R_SHA_TOOL: &str = "no sha256 tool";
pub const R_HOME: &str = "HOME or CLAUDE_CONFIG_DIR is not an absolute path";

pub fn r_seed(target: &str) -> String {
    format!("seed file anthology-{target}.tar.gz does not match anthology.lock")
}

// --- fake binaries --------------------------------------------------------------------------

pub const ALLOW: &str = "cat >/dev/null; printf 'anthology-guard:allow:v1\\n' >&2; exit 0";
/// Allows and records its stdin in $BS_LOG.
pub const ALLOW_LOG: &str = "cat >\"$BS_LOG\"; printf 'anthology-guard:allow:v1\\n' >&2; exit 0";
pub const BLOCK: &str = "cat >/dev/null; printf '%s\\n' 'Blocked by anthology-guardrails (G1): run it inside Docker (OrbStack); for browser work use BrowserOS Neo' >&2; exit 2";
pub const HOOK_DL: &str = "cat >/dev/null; printf '{\"systemMessage\":\"dl=%s t0=%s\"}\\n' \"${ANTHOLOGY_BOOTSTRAP_DOWNLOADED:-}\" \"${ANTHOLOGY_HOOK_T0:+set}\"; exit 0";

pub fn version_line(version: &str) -> String {
    format!("anthology {version} ({})", bs::host_target().triple())
}

/// A fake binary for `version` with the given guard body.
pub fn fake(version: &str, guard: &str) -> Vec<u8> {
    bs::fake_binary(&version_line(version), guard, HOOK_DL)
}

pub fn fake_hook(version: &str, guard: &str, hook: &str) -> Vec<u8> {
    bs::fake_binary(&version_line(version), guard, hook)
}

/// A release whose archive holds `binary`.
pub fn release_of(version: &str, binary: &[u8]) -> Release {
    Release::new(version, bs::release_archive(binary))
}

/// A sandbox pinned to a release of an allowing fake binary.
pub fn sandbox() -> Sandbox {
    Sandbox::new(release_of(V, &fake(V, ALLOW)))
}

pub fn sandbox_with(guard: &str) -> Sandbox {
    Sandbox::new(release_of(V, &fake(V, guard)))
}

/// The programs that make "no downloader" (T3).
pub const DOWNLOADERS: &[&str] = &["curl", "wget", "node"];

// --- running --------------------------------------------------------------------------------

pub fn guard(sb: &Sandbox, sh: Shell, payload: &[u8]) -> Run {
    sb.run(sh, GUARD, &[], payload)
}

pub fn guard_path(sb: &Sandbox, sh: Shell, payload: &[u8], path: std::ffi::OsString) -> Run {
    let mut spec = sb.spec(sh, GUARD, &[], payload);
    spec.path = Some(path);
    sb.run_spec(spec)
}

pub fn boot(sb: &Sandbox, sh: Shell, args: &[&str]) -> Run {
    sb.run(sh, G_BOOT, args, b"")
}

pub fn boot_path(sb: &Sandbox, sh: Shell, args: &[&str], path: std::ffi::OsString) -> Run {
    let mut spec = sb.spec(sh, G_BOOT, args, b"");
    spec.path = Some(path);
    sb.run_spec(spec)
}

/// Every available shell, a fresh sandbox each (from `make`).
pub fn each_shell(case: &str, mut f: impl FnMut(Shell)) {
    for sh in bs::shells(case) {
        f(sh);
    }
}

// --- assertions -----------------------------------------------------------------------------

fn names_the_escape(msg: &str) -> bool {
    let l = msg.to_lowercase();
    l.contains("/plugin") && l.contains("disable anthology-guardrails")
}

/// Exit 2, empty stdout, stderr exactly `msg` plus one newline, and the message names the
/// `/plugin` escape (R12).
#[track_caller]
pub fn assert_block(run: &Run, msg: &str, ctx: &str) {
    assert!(!run.timed_out, "{ctx}: timed out: {run:?}");
    assert_eq!(
        run.err(),
        format!("{msg}\n"),
        "{ctx}: stderr (exit {:?})",
        run.code
    );
    assert_eq!(run.code, Some(2), "{ctx}: exit code; stderr {}", run.err());
    assert!(run.stdout.is_empty(), "{ctx}: stdout {:?}", run.out());
    assert!(
        names_the_escape(msg),
        "{ctx}: the block does not name /plugin: {msg}"
    );
}

/// A block of any kind (exit 2, a `Blocked by anthology-guardrails (` line naming /plugin).
#[track_caller]
pub fn assert_blocked(run: &Run, ctx: &str) {
    assert!(!run.timed_out, "{ctx}: timed out: {run:?}");
    assert_eq!(run.code, Some(2), "{ctx}: exit code; stderr {}", run.err());
    let err = run.err();
    assert!(
        err.starts_with("Blocked by anthology-guardrails (") && err.ends_with('\n'),
        "{ctx}: stderr {err:?}"
    );
    assert!(
        names_the_escape(&err) || err.starts_with("Blocked by anthology-guardrails (G"),
        "{ctx}: the block does not name /plugin: {err}"
    );
    assert!(run.stdout.is_empty(), "{ctx}: stdout {:?}", run.out());
}

#[track_caller]
pub fn assert_allowed(run: &Run, ctx: &str) {
    assert!(!run.timed_out, "{ctx}: timed out: {run:?}");
    assert_eq!(run.code, Some(0), "{ctx}: exit code; stderr {}", run.err());
    assert!(run.stderr.is_empty(), "{ctx}: stderr {:?}", run.err());
    assert!(run.stdout.is_empty(), "{ctx}: stdout {:?}", run.out());
}

#[track_caller]
pub fn assert_status(run: &Run, want: &str, ctx: &str) {
    assert!(!run.timed_out, "{ctx}: timed out: {run:?}");
    assert_eq!(
        run.out(),
        format!("{want}\n"),
        "{ctx}: stderr {}",
        run.err()
    );
    let code = if want.starts_with("status=failed:") {
        1
    } else {
        0
    };
    assert_eq!(run.code, Some(code), "{ctx}: exit code");
}

/// `<version>` is installed and current: the binary is `bytes`, both markers right.
#[track_caller]
pub fn assert_current(sb: &Sandbox, bytes: &[u8], ctx: &str) {
    assert_eq!(
        std::fs::read(sb.bin()).ok().as_deref(),
        Some(bytes),
        "{ctx}: binary"
    );
    assert_eq!(
        std::fs::read_to_string(sb.vdir().join(".sha256")).ok(),
        Some(format!("{}\n", sb.release.sha256())),
        "{ctx}: .sha256"
    );
    assert_eq!(
        std::fs::read_to_string(sb.vdir().join(".bin-sha256")).ok(),
        Some(format!("{}\n", bs::sha256_hex(bytes))),
        "{ctx}: .bin-sha256"
    );
    assert!(
        !sb.bin_entries().iter().any(|n| n.starts_with(".tmp-")),
        "{ctx}: staging left behind: {:?}",
        sb.bin_entries()
    );
}

/// No marker: nothing is installed.
#[track_caller]
pub fn assert_not_installed(sb: &Sandbox, ctx: &str) {
    assert!(
        !sb.vdir().join(".sha256").exists(),
        "{ctx}: an install marker exists"
    );
    assert!(
        !sb.bin_entries().iter().any(|n| n.starts_with(".tmp-")),
        "{ctx}: staging left behind: {:?}",
        sb.bin_entries()
    );
}

// --- files ----------------------------------------------------------------------------------

/// Sets the mtime of `p` to `secs_ago` seconds in the past.
pub fn age(p: &Path, secs_ago: u64) {
    let t = SystemTime::now() - Duration::from_secs(secs_ago);
    let f = std::fs::File::open(p).unwrap();
    f.set_modified(t).unwrap();
}

pub fn mtime(p: &Path) -> SystemTime {
    std::fs::metadata(p).unwrap().modified().unwrap()
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A `.broken-<version>-<epoch>-<pid>` directory, as a past quarantine left it.
pub fn strike(sb: &Sandbox, version: &str, epoch: u64, pid: &str) -> PathBuf {
    let d = sb.bindir().join(format!(".broken-{version}-{epoch}-{pid}"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("anthology"), b"old").unwrap();
    d
}

pub fn chmod(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Appends `text` to both scratch lib.sh copies (fault injection on a scratch copy, T2).
pub fn patch_lib(sb: &Sandbox, text: &str) {
    for rel in [A_LIB, G_LIB] {
        let p = sb.root.join(rel);
        let mut s = std::fs::read_to_string(&p).unwrap();
        s.push('\n');
        s.push_str(text);
        s.push('\n');
        std::fs::write(&p, s).unwrap();
    }
}

/// The single `.broken-*` directory, or a panic.
#[track_caller]
pub fn one_broken(sb: &Sandbox, ctx: &str) -> PathBuf {
    let b = sb.broken();
    assert_eq!(b.len(), 1, "{ctx}: quarantines {b:?}");
    sb.bindir().join(&b[0])
}

/// Unit-tests for the helpers themselves.
#[test]
fn every_gc14_shim_message_names_the_plugin_escape() {
    let bindir = Path::new("/h/.claude/anthology/bin");
    let all = [
        not_installed("x"),
        rate_limited(),
        set_aside("exit 1"),
        not_set_aside("exit 1", bindir),
        cannot_write(bindir),
        three_strikes(V),
        probe_failed(V),
        lock_invalid(),
        unsupported("Plan9", "mips"),
        no_downloader("https://github.com/a/b/x.tar.gz", bindir),
        FALLBACK.to_string(),
        sys_guardrails("x"),
    ];
    for m in all {
        assert!(names_the_escape(&m), "{m}");
    }
}
