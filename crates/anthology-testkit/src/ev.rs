//! Test helpers of lane EV (plan §0.2).
//!
//! [`fake_guard_binary`] stands in for the real `anthology` binary in the EV1b smoke tests,
//! which run the staged shims before the guard (GD2) has landed: its `--version` passes the
//! seed and bootstrap probe, and `guard hook --shim` answers like the real guard for the two
//! commands the live smokes send (G1 for a Playwright install, the allow token otherwise).

use std::path::{Path, PathBuf};

use anthology_core::platform;

/// What the fake guard does on `guard hook`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeGuard {
    /// G1 for a payload naming `playwright install`, else the allow token (with `--shim`).
    Real,
    /// Exits 70 with the "not ported yet" stub text, as `anthology guard` did before GD2.
    Stub,
}

/// The host's target triple, as `anthology --version` reports it.
pub fn host_triple() -> &'static str {
    let (s, m) = platform::uname();
    platform::detect_with((&s, &m))
        .expect("the tests run on a supported platform")
        .triple()
}

/// Writes a fake `anthology` (a `#!/bin/sh` script, mode 0755) as `dir/anthology` and returns
/// its path. `--version` prints `anthology <version> (<host triple>)`.
pub fn fake_guard_binary(dir: &Path, version: &str, guard: FakeGuard) -> PathBuf {
    let guard_body = match guard {
        FakeGuard::Real => {
            r#"    payload=$(cat)
    case $payload in
        *'playwright install'*)
            echo 'Blocked by anthology-guardrails (G1): run it inside Docker (OrbStack); for browser work use BrowserOS Neo' >&2
            exit 2
            ;;
    esac
    [ "${3:-}" != --shim ] || echo 'anthology-guard:allow:v1' >&2
    exit 0"#
        }
        FakeGuard::Stub => {
            r#"    cat >/dev/null
    echo 'anthology: guard hook is not ported yet' >&2
    exit 70"#
        }
    };
    let script = format!(
        "#!/bin/sh\n# A fake anthology binary (anthology-testkit ev.rs).\n\
         if [ \"$#\" -eq 1 ] && [ \"$1\" = --version ]; then\n    \
         echo 'anthology {version} ({triple})'\n    exit 0\nfi\n\
         if [ \"${{1:-}}\" = guard ] && [ \"${{2:-}}\" = hook ]; then\n{guard_body}\nfi\nexit 64\n",
        triple = host_triple(),
    );
    std::fs::create_dir_all(dir).expect("fake_guard_binary: create dir");
    let path = dir.join("anthology");
    std::fs::write(&path, script).expect("fake_guard_binary: write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("fake_guard_binary: chmod");
    crate::rl::settle_exec(&path, &[]);
    path
}
