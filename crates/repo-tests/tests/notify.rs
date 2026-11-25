//! The attention alert: `plugins/anthology/hooks/notify.sh` and its two `Notification` entries in
//! `plugins/anthology/hooks/hooks.json`.
//!
//! The script runs for real under `sh`, with fake `osascript`, `notify-send`, `canberra-gtk-play`,
//! `paplay` and `uname` first on PATH. Each fake writes its arguments, one per line, to a log file,
//! so the tests see exactly what the desktop tool would have received.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anthology_pycompat::json::{self, Value};
use repo_tests::repo;

const SCRIPT: &str = "plugins/anthology/hooks/notify.sh";
const APPROVAL: &str = "Claude needs your approval";
const IDLE: &str = "Claude is waiting for you";
const OTHER: &str = "Claude needs your attention";
const MAC_SCRIPT: [&str; 3] = [
    "on run argv",
    "display notification (item 1 of argv) with title \"Claude Code\" sound name \"Glass\"",
    "end run",
];

/// A sandbox: a folder of fake tools, a log folder and a working folder.
struct Sandbox {
    _tmp: tempfile::TempDir,
    bin: PathBuf,
    log: PathBuf,
    work: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let (bin, log, work) = (
            tmp.path().join("bin"),
            tmp.path().join("log"),
            tmp.path().join("work"),
        );
        for d in [&bin, &log, &work] {
            std::fs::create_dir(d).unwrap();
        }
        let sb = Sandbox {
            _tmp: tmp,
            bin,
            log,
            work,
        };
        sb.fake("uname", "echo \"$FAKE_UNAME\"");
        for name in ["osascript", "notify-send", "canberra-gtk-play", "paplay"] {
            sb.fake(
                name,
                &format!("printf '%s\\n' \"$@\" > \"$FAKE_LOG/{name}\""),
            );
        }
        sb
    }

    fn fake(&self, name: &str, body: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Runs `sh notify.sh <args>` on `uname`, with the fakes first on PATH and `junk` on stdin.
    fn run(&self, args: &[&str], uname: &str, env: &[(&str, &str)], junk: &[u8]) -> Output {
        let path = format!("{}:/usr/bin:/bin", self.bin.display());
        self.run_on(&path, args, uname, env, junk)
    }

    fn run_on(
        &self,
        path: &str,
        args: &[&str],
        uname: &str,
        env: &[(&str, &str)],
        junk: &[u8],
    ) -> Output {
        use std::io::Write;
        let mut cmd = Command::new("/bin/sh");
        cmd.arg(repo::path(SCRIPT))
            .args(args)
            .current_dir(&self.work)
            .env_remove("ANTHOLOGY_NOTIFY")
            .env("PATH", path)
            .env("FAKE_LOG", &self.log)
            .env("FAKE_UNAME", uname)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        // Ignore a broken pipe: the script is allowed not to read its stdin at all.
        let _ = child.stdin.take().unwrap().write_all(junk);
        child.wait_with_output().unwrap()
    }

    /// The argument lines a fake logged, or `None` when it was never run.
    fn logged(&self, name: &str) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(self.log.join(name)).ok()?;
        Some(text.lines().map(str::to_string).collect())
    }

    fn nothing_logged(&self) -> bool {
        std::fs::read_dir(&self.log).unwrap().next().is_none()
    }
}

fn mac_args(message: &str) -> Vec<String> {
    let [a, b, c] = MAC_SCRIPT;
    ["-e", a, "-e", b, "-e", c, message]
        .map(str::to_string)
        .to_vec()
}

fn ok_and_quiet(out: &Output) {
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        (&out.stdout[..], &out.stderr[..]),
        (&b""[..], &b""[..]),
        "no output"
    );
}

#[test]
fn approval_on_macos_sends_the_fixed_message_to_osascript_as_an_argument() {
    let sb = Sandbox::new();
    let out = sb.run(&["approval"], "Darwin", &[], b"");
    ok_and_quiet(&out);
    assert_eq!(sb.logged("osascript"), Some(mac_args(APPROVAL)));
}

#[test]
fn idle_on_macos_sends_the_waiting_message() {
    let sb = Sandbox::new();
    let out = sb.run(&["idle"], "Darwin", &[], b"");
    ok_and_quiet(&out);
    assert_eq!(sb.logged("osascript"), Some(mac_args(IDLE)));
}

#[test]
fn an_unknown_or_missing_argument_gets_the_generic_message() {
    let hostile = "$(touch PWNED); \"quoted\" `touch PWNED`";
    for arg in [
        &["bogus"][..],
        &[""],
        &[],
        &["APPROVAL"],
        &["approval extra"],
        &[hostile],
    ] {
        let sb = Sandbox::new();
        let out = sb.run(arg, "Darwin", &[], b"");
        ok_and_quiet(&out);
        assert_eq!(sb.logged("osascript"), Some(mac_args(OTHER)), "{arg:?}");
        assert!(!sb.work.join("PWNED").exists(), "{arg:?}: nothing ran");
    }
}

#[test]
fn only_the_argument_picks_the_message_stdin_is_ignored() {
    let sb = Sandbox::new();
    let junk = br#"{"message":"$(touch PWNED)","notification_type":"idle_prompt"}"#;
    let out = sb.run(&["approval"], "Darwin", &[], junk);
    ok_and_quiet(&out);
    assert_eq!(sb.logged("osascript"), Some(mac_args(APPROVAL)));
    assert!(!sb.work.join("PWNED").exists());
}

#[test]
fn anthology_notify_0_turns_the_alert_off() {
    let sb = Sandbox::new();
    for uname in ["Darwin", "Linux"] {
        let out = sb.run(&["approval"], uname, &[("ANTHOLOGY_NOTIFY", "0")], b"");
        ok_and_quiet(&out);
        assert!(sb.nothing_logged(), "{uname}: nothing was run");
    }
}

#[test]
fn other_values_of_anthology_notify_keep_the_alert_on() {
    for value in ["1", "", "off"] {
        let sb = Sandbox::new();
        let out = sb.run(&["idle"], "Darwin", &[("ANTHOLOGY_NOTIFY", value)], b"");
        ok_and_quiet(&out);
        assert_eq!(sb.logged("osascript"), Some(mac_args(IDLE)), "{value:?}");
    }
}

#[test]
fn linux_uses_notify_send_and_a_sound_player() {
    let sb = Sandbox::new();
    let out = sb.run(&["approval"], "Linux", &[], b"");
    ok_and_quiet(&out);
    assert_eq!(sb.logged("osascript"), None);
    assert_eq!(
        sb.logged("notify-send"),
        Some(vec!["Claude Code".into(), APPROVAL.into()])
    );
    assert_eq!(
        sb.logged("canberra-gtk-play"),
        Some(vec!["-i".into(), "message".into()])
    );
}

#[test]
fn linux_without_notify_send_still_exits_0() {
    let sb = Sandbox::new();
    std::fs::remove_file(sb.bin.join("notify-send")).unwrap();
    let path = format!("{}:/usr/bin:/bin", sb.bin.display());
    let out = sb.run_on(&path, &["idle"], "Linux", &[], b"");
    // A real notify-send on the host would be found after the fakes: only the exit code is stable.
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        sb.logged("canberra-gtk-play"),
        Some(vec!["-i".into(), "message".into()])
    );
}

#[test]
fn with_no_notifier_at_all_it_exits_0_quietly() {
    let sb = Sandbox::new();
    for name in ["osascript", "notify-send", "canberra-gtk-play", "paplay"] {
        std::fs::remove_file(sb.bin.join(name)).unwrap();
    }
    // PATH is the fake `uname` alone: no notifier, no `timeout`, no other tool.
    let only = sb.bin.display().to_string();
    for uname in ["Darwin", "Linux", "FreeBSD"] {
        let out = sb.run_on(&only, &["approval"], uname, &[], b"");
        ok_and_quiet(&out);
    }
}

#[test]
fn a_failing_notifier_never_fails_the_hook() {
    let sb = Sandbox::new();
    sb.fake("osascript", "echo boom >&2; exit 9");
    let out = sb.run(&["approval"], "Darwin", &[], b"");
    ok_and_quiet(&out);
}

#[test]
fn the_script_is_posix_sh_and_parses_nothing() {
    let text = repo::read(SCRIPT);
    assert!(text.starts_with("#!/bin/sh\n"));
    let mode = std::fs::metadata(repo::path(SCRIPT))
        .unwrap()
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "executable");
    let code: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for banned in ["[[", "local ", "function ", "<<<", "pipefail"] {
        assert!(!code.contains(banned), "{banned}");
    }
    // Whole words only: "cat" is inside "notification".
    for word in code.split(|c: char| !c.is_alphanumeric() && c != '_') {
        assert!(
            !["jq", "eval", "cat", "read", "source"].contains(&word),
            "{word}: the script ignores stdin and parses nothing"
        );
    }
}

fn notification_entries() -> Value {
    let hooks = json::loads(&repo::read("plugins/anthology/hooks/hooks.json")).unwrap();
    let events = hooks.as_object().unwrap().get("hooks").unwrap();
    events
        .as_object()
        .unwrap()
        .get("Notification")
        .expect("a Notification event")
        .clone()
}

#[test]
fn hooks_json_runs_notify_sh_for_approval_and_idle_prompts() {
    let want = json::loads(
        r#"[
          {"matcher": "permission_prompt", "hooks": [{"type": "command", "command": "/bin/sh",
              "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/notify.sh", "approval"], "timeout": 5}]},
          {"matcher": "idle_prompt", "hooks": [{"type": "command", "command": "/bin/sh",
              "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/notify.sh", "idle"], "timeout": 5}]}
        ]"#,
    )
    .unwrap();
    assert_eq!(notification_entries(), want);
}

#[test]
fn the_notify_hook_script_ships() {
    assert!(Path::new(&repo::path(SCRIPT)).is_file());
}
