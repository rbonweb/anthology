//! The shell shims (BS1; design §8, §9, §13.4): `hooks/{lib,bootstrap,guard,session-start}.sh`
//! and `bin/{anthology,anthology-mem}` under `port/staging/plugins/`.
//!
//! Every behavior case runs under each shell present here (dash, BusyBox sh, `bash --posix`,
//! macOS `/bin/sh`) in its own sandbox, and asserts outcomes only: the exit code (0 or 2 for the
//! guard), the stderr or stdout line, and the files left behind. `ANTHOLOGY_SHIMS_REQUIRE` makes a
//! missing shell or tool a failure (ci/jobs/shims-distros.sh sets it per image); the real
//! downloaders run against the testkit TLS server only with `ANTHOLOGY_SHIMS_NET=1`.
//!
//! Acceptance (plan BS1) → module: 1 install, 9 GC-5 → `install`; 2 real downloaders →
//! `downloads` (network) and `offline` (absent tools, lock, uname, seed); 3 recovery matrix →
//! `recovery`; 4, 5 → `offline`; 6 → `scan`; 7, 8 → `entry` and `downloads`; static rules
//! (R2, R3, R14, the NODE_GET constant) → `statics`. A download slower than one call, its
//! resume, the failure reasons and the installer the guard lets through → `slow`. Tools the
//! opened repository plants on a relative or empty PATH entry → `planted`.

// An inline `shims` module, so its files live in tests/shims/ (no `#[path]`).
mod shims {
    mod commit_privacy;
    mod common;
    mod downloads;
    mod entry;
    mod install;
    mod offline;
    mod planted;
    mod recovery;
    mod scan;
    mod slop_gate;
    mod slow;
    mod statics;
}
