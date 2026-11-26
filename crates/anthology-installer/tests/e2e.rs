//! End to end: the real `anthology-installer` binary, run against a temporary Claude config
//! folder with a fake `claude`, `git`, `uname`, plugin shim and `bootstrap.sh`. Nothing here
//! touches a real `~/.claude`: HOME and CLAUDE_CONFIG_DIR both point into the temporary folder,
//! and PATH holds only the fakes and the system's `/usr/bin:/bin`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anthology_pycompat::json::{self, Value};

struct Env {
    dir: tempfile::TempDir,
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fake")
        .join(name)
}

fn install_file(from: &str, to: &Path) {
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    fs::copy(fixture(from), to).unwrap();
    fs::set_permissions(to, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Env {
    fn new() -> Env {
        let env = Env {
            dir: tempfile::tempdir().unwrap(),
        };
        let bin = env.dir.path().join("bin");
        for program in ["claude", "git", "uname"] {
            install_file(program, &bin.join(program));
        }
        for plugin in ["anthology", "anthology-guardrails", "anthology-superpowers"] {
            let root = env.state().join("plugins").join(plugin);
            install_file("bootstrap.sh", &root.join("hooks/bootstrap.sh"));
            install_file("anthology", &root.join("bin/anthology"));
        }
        env
    }

    fn state(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    fn config(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    fn ah(&self) -> PathBuf {
        self.config().join("anthology")
    }

    fn run_with(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        let bin = self.dir.path().join("bin");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_anthology-installer"));
        cmd.args(args)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", self.dir.path().join("home"))
            .env("CLAUDE_CONFIG_DIR", self.config())
            .env("FAKE_STATE", self.state())
            .envs(extra.iter().copied())
            .stdin(Stdio::null());
        cmd.output().unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with(args, &[])
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.state().join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn called(&self, prefix: &str) -> bool {
        self.calls().iter().any(|c| c.starts_with(prefix))
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

#[test]
fn a_fresh_install_reports_one_line_per_step_and_changes_only_through_the_official_commands() {
    let env = Env::new();
    let out = env.run(&[]);
    let text = stdout(&out);
    assert_eq!(
        code(&out),
        0,
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[..9],
        [
            "ok preflight: Linux x86_64, claude 2.1.330 (Claude Code), git found",
            "ok marketplace: added rbonweb/anthology",
            "ok plugin-anthology: installed (1.0.0)",
            // Installing anthology installed both with it, so they are updated, not installed.
            "ok plugin-anthology-guardrails: up to date (1.0.0)",
            "ok plugin-anthology-superpowers: up to date (1.0.0)",
            "ok engine: Claude Code is new enough for every plugin",
            "ok binary: installed",
            lines[7],
            lines[8],
        ]
    );
    assert!(
        lines[7].starts_with("skip setup: no terminal"),
        "{}",
        lines[7]
    );
    assert!(lines[8].starts_with("ok state: "), "{}", lines[8]);
    // The only things that ran: claude, the bootstrap, nothing that edits settings.
    let calls = env.calls();
    assert!(calls.contains(&"claude plugin marketplace add rbonweb/anthology --json".to_string()));
    assert!(calls.contains(&"claude plugin install anthology@anthology --json".to_string()));
    assert!(
        calls
            .iter()
            .any(|c| c.starts_with("bootstrap.sh guard --plugin guardrails --quiet --budget 300")),
        "{calls:?}"
    );
    assert!(!env.called("shim"), "setup must not run without a terminal");
    assert!(
        calls
            .iter()
            .all(|c| !c.contains(" -y") && !c.contains("--accept-command")),
        "{calls:?}"
    );
    assert!(!env.config().join("settings.json").exists());
    // The record.
    let state = fs::read_to_string(env.ah().join("installer.json")).unwrap();
    assert!(state.contains("\"binary\": \"installed\""), "{state}");
    assert!(state.contains("\"addedByInstaller\": true"), "{state}");
    assert!(state.contains("\"setup\": \"not-run\""), "{state}");
    assert!(
        !env.ah().join("installer.lock").exists(),
        "the lock is released"
    );
}

#[test]
fn running_it_again_upgrades_and_keeps_what_the_first_run_recorded() {
    let env = Env::new();
    assert_eq!(code(&env.run(&[])), 0);
    let out = env.run(&[]);
    let text = stdout(&out);
    assert_eq!(code(&out), 0, "{text}");
    assert!(text.contains("ok marketplace: updated anthology"), "{text}");
    assert!(
        text.contains("ok plugin-anthology: up to date (1.0.0)"),
        "{text}"
    );
    assert!(text.contains("ok binary: present"), "{text}");
    let state = fs::read_to_string(env.ah().join("installer.json")).unwrap();
    assert!(
        state.contains("\"addedByInstaller\": true"),
        "added stays true: {state}"
    );
}

#[test]
fn json_mode_prints_one_document_and_nothing_else() {
    let env = Env::new();
    let out = env.run(&["--json"]);
    assert_eq!(code(&out), 0);
    let doc = json::loads(&stdout(&out)).expect("stdout is one JSON document");
    let doc = doc.as_object().unwrap();
    assert_eq!(doc.get("command").and_then(Value::as_str), Some("install"));
    assert_eq!(doc.get("exitCode").and_then(Value::as_i64), Some(0));
    let steps = doc.get("steps").and_then(Value::as_array).unwrap();
    let names: Vec<&str> = steps
        .iter()
        .filter_map(|s| s.as_object()?.get("step")?.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "preflight",
            "marketplace",
            "plugin-anthology",
            "plugin-anthology-guardrails",
            "plugin-anthology-superpowers",
            "engine",
            "binary",
            "setup",
            "state"
        ]
    );
}

#[test]
fn a_claude_that_is_too_old_finishes_the_install_and_exits_3() {
    let env = Env::new();
    let out = env.run_with(&[], &[("FAKE_ENGINE_OLD", "1")]);
    let text = stdout(&out);
    assert_eq!(code(&out), 3, "{text}");
    assert!(text.contains("fail engine: "), "{text}");
    assert!(text.contains("needs Claude Code >=2.1.330"), "{text}");
    assert!(
        text.contains("ok binary: installed"),
        "the install still finished: {text}"
    );
}

#[test]
fn a_failing_step_exits_1_and_nothing_after_it_runs() {
    let env = Env::new();
    let out = env.run_with(&[], &[("FAKE_FAIL", "marketplace-add")]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    assert!(
        text.contains("fail marketplace: could not add rbonweb/anthology: clone failed"),
        "{text}"
    );
    assert!(!env.called("claude plugin install"), "{:?}", env.calls());
    assert!(!env.called("bootstrap.sh"));
    assert!(!env.ah().join("installer.json").exists());
    assert!(
        !env.ah().join("installer.lock").exists(),
        "the lock is released on failure too"
    );
}

#[test]
fn a_failing_bootstrap_fails_the_binary_step() {
    let env = Env::new();
    let out = env.run_with(&[], &[("FAKE_BOOTSTRAP_FAIL", "1")]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    assert!(
        text.contains("fail binary: could not install it: offline"),
        "{text}"
    );
}

#[test]
fn another_sources_marketplace_is_left_alone() {
    let env = Env::new();
    fs::create_dir_all(env.state()).unwrap();
    fs::write(
        env.state().join("marketplaces"),
        r#"[{"name":"anthology","source":"directory","path":"/work/anthology","installLocation":"/work/anthology"}]"#,
    )
    .unwrap();
    let out = env.run(&[]);
    let text = stdout(&out);
    assert_eq!(code(&out), 0, "{text}");
    assert!(text.contains("skip marketplace: "), "{text}");
    assert!(!env.called("claude plugin marketplace add"));
    assert!(!env.called("claude plugin marketplace update"));
}

#[test]
fn native_windows_exits_2_and_points_to_wsl() {
    let env = Env::new();
    let out = env.run_with(&[], &[("FAKE_UNAME_S", "MINGW64_NT-10.0")]);
    let text = stdout(&out);
    assert_eq!(code(&out), 2, "{text}");
    assert!(text.contains("WSL"), "{text}");
    assert!(env.calls().is_empty(), "nothing ran");
}

#[test]
fn a_missing_claude_prints_the_official_link_and_installs_nothing() {
    let env = Env::new();
    fs::remove_file(env.dir.path().join("bin/claude")).unwrap();
    let out = env.run(&[]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    assert!(
        text.contains("https://code.claude.com/docs/en/setup"),
        "{text}"
    );
}

#[test]
fn inside_a_claude_session_without_the_binary_it_says_to_use_a_terminal() {
    let env = Env::new();
    let out = env.run_with(&[], &[("CLAUDECODE", "1")]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    assert!(text.contains("run it in a terminal"), "{text}");
    assert!(!env.called("claude plugin"), "{:?}", env.calls());
}

#[test]
fn a_second_run_at_the_same_time_is_refused() {
    let env = Env::new();
    fs::create_dir_all(env.ah()).unwrap();
    // This test process is alive, so its pid is a live lock holder.
    fs::write(
        env.ah().join("installer.lock"),
        format!("{}\n", std::process::id()),
    )
    .unwrap();
    let out = env.run(&[]);
    let text = stdout(&out);
    assert_eq!(code(&out), 1, "{text}");
    assert!(text.contains("already running"), "{text}");
    assert!(!env.called("claude plugin"));
    assert!(
        env.ah().join("installer.lock").exists(),
        "the other run's lock is not ours to delete"
    );
}

#[test]
fn bad_arguments_exit_64_with_the_usage_on_stderr() {
    let env = Env::new();
    let out = env.run(&["frobnicate"]);
    assert_eq!(code(&out), 64);
    assert!(String::from_utf8_lossy(&out.stderr).contains("usage: anthology-installer"));
    assert_eq!(stdout(&out), "");
}

#[test]
fn version_and_help_work_without_any_claude() {
    let env = Env::new();
    let out = env.run(&["--version"]);
    assert_eq!(code(&out), 0);
    assert!(
        stdout(&out).starts_with("anthology-installer "),
        "{}",
        stdout(&out)
    );
    let out = env.run(&["--help"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("usage: anthology-installer"));
}

#[test]
fn status_is_not_ok_before_the_install_and_ok_after() {
    let env = Env::new();
    let out = env.run(&["status"]);
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    assert!(
        stdout(&out).contains("fail marketplace: "),
        "{}",
        stdout(&out)
    );
    assert_eq!(code(&env.run(&[])), 0);
    let before = env.calls().len();
    let out = env.run(&["status"]);
    let text = stdout(&out);
    assert_eq!(code(&out), 0, "{text}");
    assert!(
        text.contains("ok plugin-anthology: 1.0.0, enabled"),
        "{text}"
    );
    assert!(text.contains("ok binary: installed"), "{text}");
    assert!(text.contains("ok state: "), "{text}");
    // Status changes nothing: it only lists.
    let after: Vec<String> = env.calls().split_off(before);
    assert!(
        after
            .iter()
            .all(|c| c.ends_with(" list --json") || c.ends_with("marketplace list --json")),
        "{after:?}"
    );
}

#[test]
fn uninstall_needs_a_confirmation_and_a_terminal_or_yes() {
    let env = Env::new();
    assert_eq!(code(&env.run(&[])), 0);
    let before = env.calls().len();
    let out = env.run(&["uninstall"]);
    assert_eq!(code(&out), 1, "{}", stdout(&out));
    assert!(stdout(&out).contains("--yes"), "{}", stdout(&out));
    assert_eq!(env.calls().len(), before, "nothing was run");
}

#[test]
fn uninstall_yes_removes_setup_plugins_marketplace_and_the_binary_folder() {
    let env = Env::new();
    assert_eq!(code(&env.run(&[])), 0);
    // Setup was applied at some point.
    fs::write(env.state().join("setup-applied"), "").unwrap();
    let out = env.run(&["uninstall", "--yes"]);
    let text = stdout(&out);
    assert_eq!(
        code(&out),
        0,
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = env.calls();
    let position = |needle: &str| {
        calls
            .iter()
            .position(|c| c.contains(needle))
            .unwrap_or_else(|| panic!("no call with {needle}: {calls:?}"))
    };
    let removes: Vec<&String> = calls
        .iter()
        .filter(|c| c.contains("setup remove"))
        .collect();
    assert_eq!(removes.len(), 2, "{removes:?}");
    assert!(
        removes[0].ends_with("--preview"),
        "the preview comes first: {removes:?}"
    );
    assert!(!removes[1].contains("--preview"), "{removes:?}");
    assert!(position("setup remove") < position("uninstall anthology@anthology --prune -y"));
    assert!(
        position("uninstall anthology@anthology --prune -y")
            < position("uninstall anthology-guardrails@anthology")
    );
    assert!(
        position("uninstall anthology-superpowers@anthology")
            < position("marketplace remove anthology")
    );
    assert!(!env.state().join("setup-applied").exists());
    assert!(!env.ah().join("bin").exists(), "the binary folder is gone");
    assert!(!env.ah().join("installer.json").exists());
    assert!(text.contains("ok marketplace: removed"), "{text}");
    assert!(text.contains("ok binary: deleted"), "{text}");
    // Settings were never written by the installer.
    assert!(!env.config().join("settings.json").exists());
}

#[test]
fn uninstall_json_with_yes_prints_one_document() {
    let env = Env::new();
    assert_eq!(code(&env.run(&[])), 0);
    let out = env.run(&["uninstall", "--yes", "--json"]);
    assert_eq!(code(&out), 0);
    let doc = json::loads(&stdout(&out)).unwrap();
    assert_eq!(
        doc.as_object()
            .unwrap()
            .get("command")
            .and_then(Value::as_str),
        Some("uninstall")
    );
}
