//! Port of `tests/test_rtk_install.py` `CliTest`: the cases that run the installer as a
//! program (`subprocess.run([sys.executable, rtk_install.py, ...])`). They run the built
//! `anthology rtk-install` with a clean environment, so no network is touched: each one stops
//! at the lock.
#![forbid(unsafe_code)]

use std::process::{Command, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");

fn run_cli(dir: &std::path::Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN)
        .arg("rtk-install")
        .args(args)
        .env_clear()
        .env("HOME", dir.join("home"))
        .env("CLAUDE_CONFIG_DIR", dir.join("config"))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .expect("run anthology");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

mod cli_test {
    use super::*;

    #[test]
    fn test_bad_lock_exits_non_zero() {
        let tmp = tempfile::tempdir().expect("tmp");
        let lock = tmp.path().join("rtk.lock");
        std::fs::write(&lock, "{}").expect("lock");
        let data = tmp.path().join("data");
        let (code, _, stderr) = run_cli(
            tmp.path(),
            &[
                "--data-dir",
                data.to_str().expect("utf-8"),
                "--lock",
                lock.to_str().expect("utf-8"),
                "--dry-run",
            ],
        );
        assert_ne!(code, 0);
        assert_eq!(code, 2);
        assert_eq!(
            stderr,
            "rtk_install: rtk.lock must be an object with exactly version, tag, repo and sha256\n"
        );
    }

    #[test]
    fn test_missing_lock_exits_non_zero() {
        let tmp = tempfile::tempdir().expect("tmp");
        let lock = tmp.path().join("nope.lock");
        let data = tmp.path().join("data");
        let (code, stdout, stderr) = run_cli(
            tmp.path(),
            &[
                "--data-dir",
                data.to_str().expect("utf-8"),
                "--lock",
                lock.to_str().expect("utf-8"),
            ],
        );
        assert_ne!(code, 0);
        assert_eq!(code, 2);
        assert_eq!(stdout, "");
        let want = format!("rtk_install: cannot read {}: [Errno 2] ", lock.display());
        assert!(stderr.starts_with(&want), "{stderr}");
        assert!(!data.exists());
    }
}
