//! Anthology fork (C1): `rtk hook claude` never auto-allows past a deny or ask
//! rule, however many `rtk` prefixes or shell wrappers the command hides behind.
//!
//! The install allows `Bash(rtk *)`, and Claude Code judges the command the hook
//! hands back, so each case here spawns the real binary against settings that carry
//! that allow rule next to the deny, ask and `Read()` rules under test, and pins
//! what the hook prints. The managed-settings sources are covered by unit tests
//! (`hooks::anthology_gate`): a release build has no environment override for the
//! managed directory, by design, so a binary test cannot point it anywhere.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use tempfile::TempDir;

mod common;

/// The exact JSON the hook prints for an explicit deny.
const DENY_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"RTK: a deny rule in the user's, project's or managed Claude Code settings matches the command or a command it runs."}}"#;

/// What the hook said about a command.
#[derive(Debug, PartialEq, Eq)]
enum Said {
    /// Nothing: the host judges the original command.
    Nothing,
    /// An explicit deny, with no `updatedInput`.
    Deny,
    /// `permissionDecision: "ask"` with this command.
    Ask(String),
    /// `permissionDecision: "allow"` with this command.
    Allow(String),
    /// A rewrite with no `permissionDecision`: the host decides on this command.
    Rewrite(String),
}

/// An isolated machine whose project settings carry exactly the given rules.
struct Sandbox {
    _root: TempDir,
    home: PathBuf,
    claude_home: PathBuf,
    project: PathBuf,
}

/// The rules a sandbox's project settings hold.
#[derive(Default)]
struct Rules<'a> {
    deny: &'a [&'a str],
    ask: &'a [&'a str],
    allow: &'a [&'a str],
    read_deny: &'a [&'a str],
}

impl Sandbox {
    fn new(rules: Rules) -> Self {
        let root = TempDir::new().expect("tempdir");
        let home = root.path().join("home");
        let claude_home = root.path().join("claude-home");
        let project = root.path().join("project");
        std::fs::create_dir_all(home.join(".config")).expect("mkdir home config");
        std::fs::create_dir_all(&claude_home).expect("mkdir claude home");
        std::fs::create_dir_all(project.join(".claude")).expect("mkdir project claude");
        std::fs::create_dir_all(project.join("sub")).expect("mkdir sub");
        std::fs::create_dir_all(project.join("config")).expect("mkdir config");
        for (name, body) in [
            ("secret.txt", "s3cret\n"),
            ("ok.txt", "fine\n"),
            ("config/.env", "KEY=1\n"),
            ("sub/note.txt", "note\n"),
        ] {
            std::fs::write(project.join(name), body).expect("write fixture");
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(project.join("secret.txt"), project.join("link"))
            .expect("symlink");
        let tee = root.path().join("tee");
        std::fs::create_dir_all(&tee).expect("mkdir tee");

        let bash = |rules: &[&str]| -> Vec<String> {
            rules.iter().map(|r| format!("Bash({r})")).collect()
        };
        let mut deny = bash(rules.deny);
        deny.extend(rules.read_deny.iter().map(|r| format!("Read({r})")));
        let settings = serde_json::json!({
            "permissions": {
                "deny": deny,
                "ask": bash(rules.ask),
                "allow": bash(rules.allow),
            }
        });
        std::fs::write(project.join(".claude/settings.json"), settings.to_string())
            .expect("write settings");

        Self {
            _root: root,
            home,
            claude_home,
            project,
        }
    }

    /// No rule of any kind anywhere.
    fn bare() -> Self {
        Self::new(Rules::default())
    }

    /// The rules of the brief: the `Bash(rtk *)` allow the plugin installs, next
    /// to deny, ask and `Read()` rules.
    fn rtk_allowed_with_rules() -> Self {
        Self::new(Rules {
            deny: &["git push:*", "curl:*"],
            ask: &["kubectl apply:*", "aws:*"],
            allow: &["rtk *"],
            read_deny: &["./secret.txt", "**/.env"],
        })
    }

    fn payload(cmd: &str, cwd: Option<&std::path::Path>) -> String {
        let mut payload = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd },
        });
        if let Some(cwd) = cwd {
            payload["cwd"] = serde_json::Value::String(cwd.to_string_lossy().into_owned());
        }
        payload.to_string()
    }

    /// Run the binary with the sandbox's isolation and `env` on top.
    fn spawn(
        &self,
        args: &[&str],
        stdin: Option<String>,
        env: &[(&str, &str)],
    ) -> (i32, String, String) {
        let mut command = common::rtk_command();
        command
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", &self.claude_home)
            .env("RTK_DB_PATH", self.project.join("rtk.db"))
            .env("RTK_TEE_DIR", self._root.path().join("tee"))
            .env("RTK_RECALL_DB", self._root.path().join("recall.db"))
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn rtk");
        let mut pipe = child.stdin.take().expect("stdin");
        if let Some(input) = stdin {
            pipe.write_all(input.as_bytes()).expect("write payload");
        }
        drop(pipe);
        let out = child.wait_with_output().expect("wait rtk");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !stderr.contains("panicked"),
            "rtk panicked on {args:?}: {stderr}"
        );
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr,
        )
    }

    /// Feed `cmd` to `rtk hook claude` and return stdout verbatim.
    fn hook_raw(&self, cmd: &str, env: &[(&str, &str)]) -> String {
        let (code, stdout, stderr) =
            self.spawn(&["hook", "claude"], Some(Self::payload(cmd, None)), env);
        // The hook protocol requires exit 0 whatever it decides; without this a
        // crash is indistinguishable from a deliberate defer.
        assert_eq!(
            code, 0,
            "rtk hook claude exited {code} for {cmd:?}: {stderr}"
        );
        stdout
    }

    fn hook(&self, cmd: &str) -> Said {
        Self::classify(cmd, &self.hook_raw(cmd, &[]))
    }

    fn classify(cmd: &str, stdout: &str) -> Said {
        if stdout.trim().is_empty() {
            return Said::Nothing;
        }
        let v: serde_json::Value = serde_json::from_str(stdout).expect("hook emitted valid JSON");
        let out = &v["hookSpecificOutput"];
        assert_eq!(out["hookEventName"], "PreToolUse", "{cmd}");
        let command = out["updatedInput"]["command"].as_str().map(str::to_owned);
        match out["permissionDecision"].as_str() {
            Some("deny") => {
                assert_eq!(stdout.trim(), DENY_JSON, "{cmd}");
                assert!(command.is_none(), "a deny carries no updatedInput: {cmd}");
                Said::Deny
            }
            Some("ask") => Said::Ask(command.expect("an ask carries the command")),
            Some("allow") => Said::Allow(command.expect("an allow carries the command")),
            Some(other) => panic!("unexpected permissionDecision {other:?} for {cmd:?}"),
            None => Said::Rewrite(command.expect("a rewrite carries the command")),
        }
    }
}

#[test]
fn deny_rules_hold_past_every_rtk_prefix_and_wrapper() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "curl x",
        "rtk curl x",
        "rtk git push --force origin main",
        "rtk bypass git push --force",
        "rtk proxy git push",
        "rtk run git push",
        "rtk run -c 'git push'",
        "rtk run --shell bash -c 'git push'",
        "rtk err git push",
        "rtk test git push",
        "rtk summary git push",
        "rtk -v git push",
        "rtk --skip-env git push",
        "FOO=1 rtk git push",
        "timeout 5 rtk git push",
        "git status && rtk git push",
        "/some/dir/rtk git push",
        "bash -c 'git push'",
        "rtk run rtk run git push",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

#[test]
fn ask_rules_become_hook_asks_that_the_rtk_allow_cannot_beat() {
    let sb = Sandbox::rtk_allowed_with_rules();
    assert_eq!(
        sb.hook("kubectl apply -f x.yaml"),
        Said::Ask("rtk kubectl apply -f x.yaml".into())
    );
    assert_eq!(
        sb.hook("rtk kubectl apply -f x.yaml"),
        Said::Ask("rtk kubectl apply -f x.yaml".into())
    );
    assert_eq!(
        sb.hook("aws s3 rm s3://b --recursive"),
        Said::Ask("rtk aws s3 rm s3://b --recursive".into())
    );
    // No rewrite exists for an already-prefixed command: the hook asks about it
    // as written.
    assert_eq!(
        sb.hook("rtk run aws s3 ls"),
        Said::Ask("rtk run aws s3 ls".into())
    );
}

#[test]
fn read_deny_rules_block_the_commands_rtk_turns_into_reads() {
    let sb = Sandbox::rtk_allowed_with_rules();
    let mut denied = vec![
        "cat secret.txt",
        "head -n 5 secret.txt",
        "tail secret.txt",
        "cat ./x/../secret.txt",
        "rtk read secret.txt",
        "rtk run cat secret.txt",
        "cat config/.env",
        "grep foo secret.txt",
        "cd sub && cat ../secret.txt",
    ];
    #[cfg(unix)]
    denied.push("cat link");
    for cmd in denied {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

#[test]
fn read_deny_rules_leave_other_paths_and_uncertain_ones_to_the_host() {
    let sb = Sandbox::rtk_allowed_with_rules();
    // Another file still rewrites, as it did before.
    assert_eq!(
        sb.hook("cat ok.txt"),
        Said::Rewrite("rtk read ok.txt".into())
    );
    // A glob might name the denied file: no rewrite, the host judges the
    // original natively.
    assert_eq!(sb.hook("cat *.txt"), Said::Nothing);
    // Inside an rtk expansion the same uncertainty asks.
    assert_eq!(
        sb.hook("rtk read *.txt"),
        Said::Ask("rtk read *.txt".into())
    );
}

#[test]
fn an_opaque_inner_command_asks() {
    let sb = Sandbox::rtk_allowed_with_rules();
    assert_eq!(
        sb.hook("rtk run -c 'git $(echo push)'"),
        Said::Ask("rtk run -c 'git $(echo push)'".into())
    );
}

/// No rule anywhere: the hook says what it said before this change, byte for byte.
#[test]
fn the_bare_sandbox_is_unchanged() {
    let sb = Sandbox::bare();
    assert_eq!(
        sb.hook_raw("curl x", &[]).trim(),
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecisionReason":"RTK auto-rewrite","updatedInput":{"command":"rtk curl x"}}}"#
    );
    assert_eq!(sb.hook("rtk git status"), Said::Nothing);
    assert_eq!(sb.hook_raw("rtk git status", &[]), "");
    assert_eq!(
        sb.hook_raw("cat secret.txt", &[]).trim(),
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecisionReason":"RTK auto-rewrite","updatedInput":{"command":"rtk read secret.txt"}}}"#
    );
    assert_eq!(
        sb.hook("rtk git push"),
        Said::Nothing,
        "the owner's accepted Bash(rtk *) risk: no rule, no decision"
    );
}

#[test]
fn an_allow_only_sandbox_still_allows_the_rewrite() {
    let sb = Sandbox::new(Rules {
        allow: &["git status"],
        ..Rules::default()
    });
    assert_eq!(
        sb.hook_raw("git status", &[]).trim(),
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecisionReason":"RTK auto-rewrite","updatedInput":{"command":"rtk git status"},"permissionDecision":"allow"}}"#
    );
}

#[test]
fn the_payload_cwd_is_where_a_relative_path_and_a_relative_rule_anchor() {
    let sb = Sandbox::rtk_allowed_with_rules();
    let at = |cwd: &std::path::Path, cmd: &str| {
        let (code, stdout, _) = sb.spawn(
            &["hook", "claude"],
            Some(Sandbox::payload(cmd, Some(cwd))),
            &[],
        );
        assert_eq!(code, 0);
        Sandbox::classify(cmd, &stdout)
    };
    let project = std::fs::canonicalize(&sb.project).expect("canonical project");
    assert_eq!(at(&project, "cat secret.txt"), Said::Deny);
    assert_eq!(
        at(&project, "cat ok.txt"),
        Said::Rewrite("rtk read ok.txt".into())
    );
}

#[test]
fn claude_project_dir_adds_its_settings() {
    let sb = Sandbox::bare();
    let other = sb._root.path().join("elsewhere");
    std::fs::create_dir_all(other.join(".claude")).expect("mkdir other project");
    std::fs::write(
        other.join(".claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(git push:*)"]}}"#,
    )
    .expect("write settings");
    let dir = other.to_string_lossy().into_owned();
    assert_eq!(sb.hook("rtk git push"), Said::Nothing);
    let stdout = sb.hook_raw("rtk git push", &[("CLAUDE_PROJECT_DIR", &dir)]);
    assert_eq!(Sandbox::classify("rtk git push", &stdout), Said::Deny);
}

#[test]
fn hook_check_for_claude_reports_the_same_denials() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in ["rtk git push", "bash -c 'git push'", "cat secret.txt"] {
        let (code, stdout, stderr) =
            sb.spawn(&["hook", "check", "--agent", "claude", cmd], None, &[]);
        assert_eq!((code, stdout.trim()), (1, ""), "{cmd}");
        assert!(
            stderr.contains("Denied by a permission rule"),
            "{cmd}: {stderr}"
        );
    }
}
