//! Test helpers of lane RL (plan §0.2): a fake `anthology` binary (N-I4).
//!
//! [`fake_anthology`] writes a `#!/bin/sh` script that stands in for the real binary wherever a
//! test needs one before the guard is ported: `anthology-dev seed`, the guard shim, the
//! bootstrap's `--version` probe.
//!
//! - `--version` prints `anthology <version> (<target>)` and exits 0;
//! - `guard hook [--shim]` reads stdin; when it contains [`FAKE_BLOCK_MARKER`] it prints
//!   [`FAKE_BLOCK_LINE`] to stderr and exits 2; otherwise it exits 0 and, with `--shim` only,
//!   prints GC-17's allow token `anthology-guard:allow:v1` and one newline to stderr;
//! - anything else exits 64.
//!
//! [`fake_anthology_with`] takes a custom allow token, so a test can check that a shim
//! quarantines a binary whose token is wrong.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// A payload containing this is blocked by the fake guard.
pub const FAKE_BLOCK_MARKER: &str = "rl-fake-block-marker";
/// The fake guard's block line (GC-14's `Blocked by anthology-guardrails (` prefix).
pub const FAKE_BLOCK_LINE: &str = "Blocked by anthology-guardrails (G1): fake block";
/// GC-17's allow token, without its newline.
pub const ALLOW_TOKEN: &str = "anthology-guard:allow:v1";

/// Writes the fake binary as `dir/anthology` (mode 0755) and returns its path.
pub fn fake_anthology(dir: &Path, version: &str, target: &str) -> PathBuf {
    fake_anthology_with(dir, version, target, ALLOW_TOKEN)
}

/// [`fake_anthology`] with `token` printed on allow instead of the real token.
pub fn fake_anthology_with(dir: &Path, version: &str, target: &str, token: &str) -> PathBuf {
    for (what, s) in [("version", version), ("target", target), ("token", token)] {
        assert!(
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-:".contains(c)),
            "fake_anthology: unsafe {what} {s:?}"
        );
    }
    let script = format!(
        r#"#!/bin/sh
# A fake anthology binary (anthology-testkit rl.rs).
if [ "$#" -eq 1 ] && [ "$1" = --version ]; then
    echo 'anthology {version} ({target})'
    exit 0
fi
if [ "$#" -ge 2 ] && [ "$1" = guard ] && [ "$2" = hook ]; then
    shim=
    if [ "$#" -eq 3 ] && [ "$3" = --shim ]; then
        shim=1
    elif [ "$#" -ne 2 ]; then
        exit 64
    fi
    payload=$(cat)
    case $payload in
        *{marker}*)
            echo '{block}' >&2
            exit 2
            ;;
    esac
    if [ -n "$shim" ]; then
        echo '{token}' >&2
    fi
    exit 0
fi
exit 64
"#,
        marker = FAKE_BLOCK_MARKER,
        block = FAKE_BLOCK_LINE,
    );
    std::fs::create_dir_all(dir).expect("fake_anthology: create dir");
    let path = dir.join("anthology");
    std::fs::write(&path, script).expect("fake_anthology: write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("fake_anthology: chmod");
    settle_exec(&path, &[]);
    path
}

/// Runs the just-written executable `path` once (`--version`, stdio closed, `envs` set),
/// retrying while exec fails with ETXTBSY (Linux): another test thread that forked while the
/// file was open for writing holds a copy of that descriptor until it execs. After one
/// successful exec no writer is left, so later runs of `path` cannot hit ETXTBSY.
pub fn settle_exec(path: &Path, envs: &[(&str, &str)]) {
    const ETXTBSY: i32 = 26;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let spawned = std::process::Command::new(path)
            .arg("--version")
            .envs(envs.iter().copied())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(mut child) => {
                let _ = child.wait();
                return;
            }
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => panic!("settle_exec: {}: {e}", path.display()),
        }
    }
}
