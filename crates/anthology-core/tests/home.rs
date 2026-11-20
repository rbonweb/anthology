//! `paths::home` without `HOME` falls back to the password database (GC-1, GC-2), and that
//! equals what the system's own tools report for this uid: `getent passwd <uid>` (or
//! `/etc/passwd`) on Linux, `dscl` on macOS.

use std::path::PathBuf;
use std::process::Command;

use anthology_core::{Env, paths};

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// This uid's home directory as the system tools see it.
fn tools_home() -> Option<String> {
    let uid = run("id", &["-u"])?.trim().to_string();
    if cfg!(target_os = "macos") {
        let user = run("id", &["-un"])?.trim().to_string();
        let out = run(
            "dscl",
            &[".", "-read", &format!("/Users/{user}"), "NFSHomeDirectory"],
        )?;
        return out
            .split_once("NFSHomeDirectory:")
            .map(|(_, h)| h.trim().to_string());
    }
    let entry = run("getent", &["passwd", &uid]).or_else(|| {
        std::fs::read_to_string("/etc/passwd")
            .ok()?
            .lines()
            .find(|l| l.split(':').nth(2) == Some(uid.as_str()))
            .map(str::to_string)
    })?;
    entry.trim_end().split(':').nth(5).map(str::to_string)
}

/// `expanduser`'s treatment of the passwd home: trailing `/` stripped, empty is `/`.
fn expanded(h: &str) -> PathBuf {
    let h = h.trim_end_matches('/');
    PathBuf::from(if h.is_empty() { "/" } else { h })
}

#[test]
fn without_home_the_passwd_home_is_used() {
    let Some(want) = tools_home() else {
        eprintln!("skipped: neither getent, /etc/passwd nor dscl has this uid");
        return;
    };
    let env = Env::default().with("PATH", "/usr/bin:/bin");
    assert_eq!(paths::home(&env), expanded(&want));
    assert_eq!(paths::config_dir(&env), expanded(&want).join(".claude"));
}

#[test]
fn home_wins_over_passwd_even_when_empty() {
    assert_eq!(
        paths::home(&Env::default().with("HOME", "/somewhere/else/")),
        PathBuf::from("/somewhere/else")
    );
    assert_eq!(
        paths::home(&Env::default().with("HOME", "")),
        PathBuf::from("/")
    );
}
