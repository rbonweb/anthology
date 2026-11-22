//! `GuardSelfTestTest` rows that run the real guard (`tests/test_session_start.py`): the staged
//! `hooks/guard.sh` with this build's `anthology` installed as the guardrails' binary, and the
//! `anthology guard hook` process fed the compiled-in self-test sample.
#![forbid(unsafe_code)]

use std::io::Write;
use std::process::{Command, Stdio};

use anthology_core::Env;
use anthology_guard::data;
use anthology_pycompat::json;
use anthology_runtime::session_start::{self, SessionDeps};
use anthology_testkit::bs::{self, Release, Sandbox};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");

#[test]
fn test_real_guard_blocks_the_sample_quietly() {
    let binary = std::fs::read(BIN).expect("the anthology binary");
    let sb = Sandbox::new(Release::new(
        env!("CARGO_PKG_VERSION"),
        bs::release_archive(&binary),
    ));
    sb.install(&binary, 0o755);
    let env = Env::default()
        .with_os("HOME", &sb.home)
        .with_os("CLAUDE_CONFIG_DIR", &sb.config)
        .with_os("TMPDIR", &sb.tmpdir)
        .with("PATH", "/usr/bin:/bin");
    let deps = SessionDeps::default();
    let warning = session_start::guard_selftest(&env, &sb.plugin("anthology"), &sb.root, &deps)
        .expect("the self-test runs");
    assert_eq!(warning, None);
}

#[test]
fn test_sample_is_piped_as_json_and_blocked_with_exit_2() {
    let sample = json::dumps(&data::self_test_sample(), &json::PY_DEFAULT);
    let mut child = Command::new(BIN)
        .args(["guard", "hook"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run anthology");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(sample.as_bytes())
        .expect("write the sample");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(2));
}
