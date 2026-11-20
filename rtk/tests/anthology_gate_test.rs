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
    read_ask: &'a [&'a str],
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
            ("asked.txt", "asked\n"),
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
        let mut ask = bash(rules.ask);
        ask.extend(rules.read_ask.iter().map(|r| format!("Read({r})")));
        let settings = serde_json::json!({
            "permissions": {
                "deny": deny,
                "ask": ask,
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
            read_ask: &["./asked.txt"],
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
            // The hook may stop reading (an oversize payload) and exit first.
            let _ = pipe.write_all(input.as_bytes());
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
        // Round 7: `rtk kubectl` is not in the plain shape, so no rewrite.
        Said::Ask("kubectl apply -f x.yaml".into())
    );
    assert_eq!(
        sb.hook("rtk kubectl apply -f x.yaml"),
        Said::Ask("rtk kubectl apply -f x.yaml".into())
    );
    assert_eq!(
        sb.hook("aws s3 rm s3://b --recursive"),
        Said::Ask("aws s3 rm s3://b --recursive".into())
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
fn read_deny_rules_leave_other_paths_alone_and_ask_about_uncertain_ones() {
    let sb = Sandbox::rtk_allowed_with_rules();
    // Another file still rewrites, as it did before.
    assert_eq!(
        sb.hook("cat ok.txt"),
        Said::Rewrite("rtk read ok.txt".into())
    );
    // A glob might name the denied file: the hook asks (round 6: at top level too).
    assert_eq!(sb.hook("cat *.txt"), Said::Ask("cat *.txt".into()));
    assert_eq!(
        sb.hook("cat < sec?et.txt"),
        Said::Ask("cat < sec?et.txt".into())
    );
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

/// Every reproducer of the review of the first version of the gate.
#[test]
fn quoted_words_single_strings_and_exec_payloads_are_denied() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        // C1: quoted words
        "rtk git 'push' --force origin main",
        "rtk git \"push\"",
        "rtk bypass git p'u'sh",
        "rtk run -c 'git pu\\sh'",
        "git 'push'",
        // C2: a single string run through sh -c
        "rtk run 'git push --force'",
        "rtk test 'git push'",
        "rtk err 'git push'",
        "rtk summary 'git push'",
        "rtk run 'echo ok; curl x'",
        // C3: exec payloads, including a plain find the hook would rewrite
        "find . -exec git push \\;",
        "find . -execdir git push \\;",
        "find . -ok git push \\;",
        "find . -exec curl x \\;",
        "rtk bypass xargs -n1 git push",
        "rtk run env -i git push",
        "rtk run env -u X git push",
        // I2: Read() deny through any program inside rtk
        "rtk sed -n p secret.txt",
        "rtk awk 1 secret.txt",
        "rtk bypass base64 secret.txt",
        "rtk bypass cp secret.txt out.txt",
        "rtk bypass jq -R . secret.txt",
        // Minor: a read-write redirect reads
        "rtk bypass sort <>secret.txt",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    for cmd in [
        "rtk kubectl 'apply' -f x.yaml",
        "rtk run kubectl \"apply\" -f x.yaml",
    ] {
        assert!(matches!(sb.hook(cmd), Said::Ask(_)), "{cmd}");
    }
}

#[test]
fn what_the_gate_cannot_read_is_asked_or_left_unrewritten() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "rtk run eval 'git push'",
        "rtk run -c 'eval \"git push\"'",
        "rtk run -c 'g=git; $g push'",
        "echo 'curl x' | rtk bypass bash",
        "echo 'curl x' | rtk bypass sh -s",
        "rtk bypass bash <<< 'curl x'",
        "rtk bypass python3 -c 'import os; os.system(\"curl x\")'",
        "rtk bypass busybox sh -c 'curl x'",
    ] {
        assert!(
            matches!(sb.hook(cmd), Said::Ask(_)),
            "{cmd}: {:?}",
            sb.hook(cmd)
        );
    }
    // A top-level find whose payload the gate cannot read is not rewritten, so
    // `Bash(rtk *)` never sees `rtk find . -exec sh {} \\;`.
    assert_eq!(sb.hook("find . -exec sh {} \\;"), Said::Nothing);
    assert!(matches!(
        sb.hook("rtk find . -exec sh {} \\;"),
        Said::Ask(_)
    ));
}

#[test]
fn read_ask_rules_are_not_rewritten_into_an_allow() {
    let sb = Sandbox::rtk_allowed_with_rules();
    assert_eq!(sb.hook("cat asked.txt"), Said::Nothing);
    assert_eq!(sb.hook("head asked.txt"), Said::Nothing);
    assert!(matches!(sb.hook("rtk read asked.txt"), Said::Ask(_)));
}

#[test]
fn an_unreadable_payload_asks_instead_of_failing_open() {
    let sb = Sandbox::rtk_allowed_with_rules();
    let ask = |stdout: &str| {
        let v: serde_json::Value = serde_json::from_str(stdout).expect("JSON");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "ask");
        assert!(v["hookSpecificOutput"].get("updatedInput").is_none());
    };
    // Over the 1 MiB limit.
    let mut big = String::from("rtk bypass git push #");
    big.push_str(&"A".repeat(1_100_000));
    let (code, stdout, _) = sb.spawn(&["hook", "claude"], Some(Sandbox::payload(&big, None)), &[]);
    assert_eq!(code, 0);
    ask(&stdout);
    // Not JSON.
    let (code, stdout, _) = sb.spawn(&["hook", "claude"], Some("{not json".to_string()), &[]);
    assert_eq!(code, 0);
    ask(&stdout);
    // Nothing at all stays silent.
    let (code, stdout, _) = sb.spawn(&["hook", "claude"], Some(String::new()), &[]);
    assert_eq!((code, stdout.trim()), (0, ""));
}

/// The review's case: a 300k-character word against a two-star Read rule took over
/// a minute, past Claude Code's hook timeout.
#[test]
fn a_huge_read_operand_does_not_stall_the_hook() {
    let sb = Sandbox::new(Rules {
        deny: &["git push:*"],
        allow: &["rtk *"],
        read_deny: &["**/*key*.json"],
        ..Rules::default()
    });
    let started = std::time::Instant::now();
    let cmd = format!("cat {}; rtk git push", "key".repeat(100_000));
    assert_eq!(sb.hook(&cmd), Said::Deny);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(120),
        "{:?}",
        started.elapsed()
    );
}

// --- review round 2 ---

fn asked(sb: &Sandbox, cmds: &[&str]) {
    for cmd in cmds {
        assert!(
            matches!(sb.hook(cmd), Said::Ask(_)),
            "{cmd}: {:?}",
            sb.hook(cmd)
        );
    }
}

/// N1: a word that expands may become a ruled command.
#[test]
fn a_word_that_may_expand_into_a_ruled_command_is_not_allowed() {
    let sb = Sandbox::rtk_allowed_with_rules();
    asked(
        &sb,
        &[
            "x=push; rtk git $x",
            "rtk git $X",
            "rtk git ${X}",
            "rtk git \"$x\"",
            "x=push rtk git $x",
            "rtk run git $x",
            "rtk bypass git $x",
            "rtk run -c 'x=push; git $x'",
            "rtk run -c 'git ${x:-push}'",
            "rtk run -c 'git pu${x}sh'",
            "rtk git {push,}",
            "rtk bypass git {push,}",
            "rtk git $'push'",
            "rtk git $'\\x70ush'",
            "rtk git $\"push\"",
            "rtk kubectl $'apply' -f x",
            "rtk kubectl $a -f x.yaml",
            "rtk git pu*h",
            "rtk git pus?",
            "rtk git pus[h]",
        ],
    );
    // A top level the hook would rewrite is left to the host unrewritten.
    for cmd in ["git $x", "git pu*h", "git {push,}"] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

/// N2: grammar words and lowercase assignments in front of a ruled command.
#[test]
fn shell_grammar_in_front_of_a_ruled_command_does_not_hide_it() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "if true; then rtk git push; fi",
        "while true; do rtk git push; break; done",
        "until false; do rtk git push; break; done",
        "for i in 1; do rtk git push; done",
        "if rtk git push; then :; fi",
        "function f { rtk git push; }; f",
        "coproc rtk git push",
        "x=1 rtk git push",
        "a=1 b=2 rtk git push",
        "A=1 b=2 rtk git push",
        "rtk run -c 'if true; then git push; fi'",
        "rtk run -c 'while git push; do break; done'",
        "rtk run -c 'until git push; do :; done'",
        "rtk run -c 'for i in 1; do git push; done'",
        "rtk run -c 'select i in 1; do git push; done'",
        "rtk run -c 'function f { git push; }; f'",
        "rtk run -c 'coproc git push'",
        "rtk run -c 'x=1 git push'",
        "rtk run -c 'a=1 b=2 git push'",
        "rtk run -c 'A=1 b=2 git push'",
        "rtk run -c 'time -p git push'",
        "rtk run -c '! git push'",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    asked(
        &sb,
        &[
            "rtk run -c 'case x in x) echo;; esac'",
            "rtk run -c '[[ -f x ]] && echo y'",
        ],
    );
}

/// N3 and N4: interpreters, other shells and programs that run their arguments.
#[test]
fn interpreters_and_launchers_inside_rtk_ask() {
    let sb = Sandbox::rtk_allowed_with_rules();
    asked(
        &sb,
        &[
            "rtk bypass python3 -X dev -c 'import os; os.system(\"git push\")'",
            "rtk bypass perl -I lib -e 'system(\"git push\")'",
            "rtk bypass ruby -I lib -e 'x'",
            "rtk bypass node -r fs -e 'x'",
            "rtk bypass python3 <<< 'import os'",
            "echo x | rtk bypass python3 -",
            "rtk bypass python3 script.py",
            "rtk bypass tcsh",
            "rtk bypass csh -f script.csh",
            "rtk bypass pwsh -c 'git push'",
            "rtk bypass expect -c 'spawn git push'",
            "rtk bypass tclsh <<< 'exec git push'",
            "rtk bypass Rscript -e 'system(\"git push\")'",
            "rtk bypass caffeinate git push",
            "rtk bypass arch -arm64 git push",
            "rtk bypass xcrun git push",
            "rtk bypass script -q /dev/null git push",
            "rtk bypass sandbox-exec -p x git push",
            "rtk bypass watch git push",
            "rtk bypass flock /tmp/l git push",
            "rtk bypass setsid git push",
            "rtk bypass unbuffer git push",
            "rtk bypass ionice -c3 git push",
            "rtk bypass chrt 0 git push",
            "rtk bypass taskset 1 git push",
            "rtk bypass gtimeout 5 git push",
            "rtk bypass su -c 'git push'",
            "rtk bypass ssh localhost git push",
            "rtk grep \"git push\" docs/",
        ],
    );
    assert_eq!(sb.hook("rtk bypass tcsh -c 'git push'"), Said::Deny);
    assert_eq!(sb.hook("rtk bypass parallel git push ::: a"), Said::Deny);
    // At top level a command rtk rewrites into text that holds a ruled head asks
    // (the rewrite is gated too).
    for cmd in ["ssh localhost git push", "git log --grep='git push'"] {
        assert!(matches!(sb.hook(cmd), Said::Ask(_)), "{cmd}");
    }
}

/// N5: HOME and CDPATH assignments hidden behind quotes or computed names.
#[test]
fn a_hidden_home_or_cdpath_change_makes_tilde_unknown() {
    let sb = Sandbox::new(Rules {
        read_deny: &["~/.ssh/**"],
        allow: &["rtk *"],
        ..Rules::default()
    });
    asked(
        &sb,
        &[
            "export HO\"ME\"=/; rtk read ~/.ssh/id_rsa",
            "export H\\OME=/; rtk read ~/.ssh/id_rsa",
            "v=HO; export ${v}ME=/; rtk read ~/.ssh/id_rsa",
            "declare ${v}ME=/; rtk read ~/.ssh/id_rsa",
        ],
    );
    assert_eq!(sb.hook("export FOO=1; rtk read ~/.ssh/id_rsa"), Said::Deny);
}

/// N6: with the work budget spent, a rule still means ask.
#[test]
fn a_command_too_long_to_scan_asks_when_a_rule_exists() {
    let sb = Sandbox::rtk_allowed_with_rules();
    let many = "true;".repeat(5000);
    asked(
        &sb,
        &[
            &format!("{many}\"r\"tk read secret.txt"),
            &format!("{many}r\\tk read secret.txt"),
        ],
    );
}

/// N7 and Minor 1: operands that expand, and `key=value` operands.
#[test]
fn an_operand_that_expands_or_hides_a_path_is_not_allowed() {
    let sb = Sandbox::rtk_allowed_with_rules();
    asked(
        &sb,
        &[
            "rtk bypass base64 secret.tx?",
            "rtk bypass base64 secre*",
            "rtk bypass base64 {secret,x}.txt",
            "rtk bypass base64 $'secret.txt'",
            "f=secret.txt; rtk bypass base64 $f",
            "rtk bypass sed -n p \"$PWD/secret.txt\"",
            "rtk bypass base64 ~root/.ssh/id_rsa",
        ],
    );
    assert_eq!(sb.hook("rtk bypass dd if=secret.txt"), Said::Deny);
}

/// The backstops must not tax ordinary commands the rules cannot reach.
#[test]
fn ordinary_commands_keep_their_rewrite_beside_an_unrelated_rule() {
    let sb = Sandbox::new(Rules {
        deny: &["git push:*"],
        allow: &["rtk *"],
        ..Rules::default()
    });
    for cmd in [
        "grep 'a.*b' f",
        "cat README.md",
        "git status",
        "git log --oneline -5",
        "cargo test",
    ] {
        match sb.hook(cmd) {
            Said::Allow(rewritten) | Said::Rewrite(rewritten) => {
                assert!(rewritten.starts_with("rtk "), "{cmd}: {rewritten}");
            }
            other => panic!("{cmd} lost its rewrite: {other:?}"),
        }
    }
    // Round 7: a rewrite out of the plain shape is not made; the host judges the
    // original, with no prompt from the hook.
    for cmd in ["ls src/*.rs", "git log -- *.rs"] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

/// With no rule of any kind every backstop is silent.
#[test]
fn every_backstop_is_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "x=push; rtk git $x",
        "rtk bypass caffeinate git push",
        "if true; then rtk git push; fi",
        "rtk bypass python3 script.py",
        "rtk bypass base64 secre*",
        "export HO\"ME\"=/; rtk read ~/.ssh/id_rsa",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    assert!(
        matches!(
            sb.hook("git log --grep='git push'"),
            Said::Rewrite(_) | Said::Allow(_)
        ),
        "an embedded word is no reason to hold back a rewrite"
    );
}

// --- review round 3 ---

fn push_only_sandbox() -> Sandbox {
    Sandbox::new(Rules {
        deny: &["git push:*"],
        allow: &["rtk *"],
        ..Rules::default()
    })
}

/// C-A: options between a rule's words.
#[test]
fn options_between_a_rules_words_do_not_hide_it() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "git -C . push origin main --force",
        "git -C sub push",
        "git --git-dir=.git push",
        "git --work-tree=. push",
        "git -c x.y=z push",
        "git --no-pager push",
        "GIT_DIR=.git git -C . push",
        "cd sub && git -C .. push",
        "rtk git -C . push",
        "rtk -v git -C . push",
        "rtk git -c x.y=z push",
        "rtk git --no-pager push",
        "rtk git --git-dir=.git push",
        "rtk bypass git -C . push",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    asked(
        &sb,
        &[
            "rtk kubectl --context c apply -f x.yaml",
            "rtk kubectl -n prod apply -f x.yaml",
        ],
    );
    assert_eq!(
        sb.hook("kubectl -n prod apply -f x.yaml"),
        Said::Nothing,
        "the host judges the original"
    );
    let only = push_only_sandbox();
    asked(&only, &["rtk git -C . $x"]);
    assert!(matches!(
        only.hook("git commit -m 'push fix'"),
        Said::Allow(_) | Said::Rewrite(_)
    ));
}

/// C-B: a crafted run of grammar words must not stall the hook.
#[test]
fn a_crafted_run_of_grammar_words_does_not_stall_the_hook() {
    let sb = Sandbox::rtk_allowed_with_rules();
    let started = std::time::Instant::now();
    let inside = format!(
        "rtk run -c '{}git push {}'",
        "time ".repeat(20_000),
        "x ".repeat(20_000)
    );
    assert!(matches!(sb.hook(&inside), Said::Ask(_) | Said::Deny));
    let top = format!(
        "{}rtk git status {}",
        "time ".repeat(20_000),
        "x ".repeat(20_000)
    );
    assert!(matches!(sb.hook(&top), Said::Ask(_) | Said::Deny));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
}

/// I-A: the command word is read by its basename.
#[test]
fn a_path_or_equals_sign_in_front_of_a_denied_program_does_not_hide_it() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "rtk /usr/bin/git push",
        "rtk bypass /usr/bin/git push",
        "rtk =git push",
        "rtk /usr/bin/curl x",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    asked(&sb, &["rtk bypass caffeinate =git push"]);
    if cfg!(any(target_os = "macos", windows)) {
        for cmd in [
            "rtk GIT push",
            "rtk Git push",
            "rtk bypass GIT push",
            "rtk CURL x",
        ] {
            assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
        }
    }
}

/// I-B: a denied command behind a launcher the gate does not model.
#[test]
fn a_denied_command_behind_an_unmodelled_launcher_is_asked_about() {
    let sb = push_only_sandbox();
    asked(
        &sb,
        &[
            "rtk bypass caffeinate g''it push",
            "rtk bypass caffeinate gi\\t push",
            "rtk bypass caffeinate \"g\"it push",
            "rtk bypass ssh h \"g\"\"it push\"",
            "rtk bypass exec -a x g''it push",
            "rtk bypass command -p g''it push",
            "x=it; rtk bypass caffeinate g$x push",
            "rtk bypass caffeinate {git,} push",
            "rtk bypass caffeinate $'\\x67it' push",
            "rtk bypass caffeinate git pu*h",
            "rtk bypass caffeinate sh -c 'g\"\"it push'",
            "rtk bypass caffeinate bash -c 'x=push; git $x'",
            "echo 'git push' | rtk bypass caffeinate bash",
            "rtk bypass caffeinate bash <<< 'g''it push'",
            "echo 'git push' | rtk bypass caffeinate python3",
            "rtk bypass caffeinate eval 'g\"\"it push'",
            "rtk bypass env -i caffeinate g''it push",
            "rtk bypass caffeinate env g''it push",
            "rtk bypass caffeinate find . -maxdepth 0 -exec g''it push \\;",
            "rtk bypass npm exec -c 'g\"\"it push'",
            "rtk bypass watch -n1 'g\"\"it push'",
            "rtk bypass ssh h 'x=push; git $x'",
            "rtk git -c 'alias.p=!git push' p",
        ],
    );
}

/// I-C: arguments from stdin or the file system complete a rule's head.
#[test]
fn arguments_from_stdin_or_the_filesystem_are_asked_about() {
    let sb = push_only_sandbox();
    asked(
        &sb,
        &[
            "echo push | rtk bypass xargs git",
            "printf push | rtk bypass xargs -n1 git",
            "rtk bypass xargs -a args.txt git",
            "rtk bypass caffeinate xargs git <<< push",
            "touch push; rtk bypass find . -name push -execdir git {} \\;",
            "rtk bypass parallel git ::: push",
        ],
    );
}

/// The round-2 negatives still keep their rewrite, and so do the common commands a
/// round-3 backstop could have caught by accident.
#[test]
fn the_round_three_backstops_leave_ordinary_commands_alone() {
    let sb = push_only_sandbox();
    for cmd in [
        "grep 'a.*b' f",
        "cat README.md",
        "git status",
        "git commit -m 'fix the push button'",
    ] {
        match sb.hook(cmd) {
            Said::Allow(rewritten) | Said::Rewrite(rewritten) => {
                assert!(rewritten.starts_with("rtk "), "{cmd}: {rewritten}");
            }
            other => panic!("{cmd} lost its rewrite: {other:?}"),
        }
    }
    for cmd in [
        "rtk git status",
        "rtk grep python src/",
        "rtk git log --oneline",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

/// No rule anywhere: every round-3 spelling says nothing, as before.
#[test]
fn the_round_three_backstops_are_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "rtk git -C . push",
        "rtk /usr/bin/git push",
        "rtk bypass caffeinate g''it push",
        "echo push | rtk bypass xargs git",
        "rtk bypass ssh h 'x=push; git $x'",
        "rtk run -c 'time time git push'",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

// --- review round 4 ---

/// N-1: a command rtk rewrites into `rtk <launcher> ...` is gated as that text.
#[test]
fn the_text_the_hook_rewrites_a_command_into_is_gated_too() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "pnpm exec cat secret.txt",
        "npx -c 'cat secret.txt'",
        "pnpm exec base64 secret.txt",
        "npx -c 'cat config/.e*'",
        "npm exec -c 'f=secret; cat $f.txt'",
        "bunx sh -c 'f=secret; cat $f.txt'",
        "pnpm exec cat secre?.txt",
        "find . -maxdepth 0 -exec caffeinate sh -c 'cat secret.txt' \\;",
    ] {
        assert!(
            matches!(sb.hook(cmd), Said::Ask(_) | Said::Deny | Said::Nothing),
            "{cmd}"
        );
        assert!(
            !matches!(sb.hook(cmd), Said::Allow(_) | Said::Rewrite(_)),
            "{cmd} was rewritten with no decision: {:?}",
            sb.hook(cmd)
        );
    }
    let only = push_only_sandbox();
    for cmd in [
        "pnpm exec sh -c 'x=push; git $x'",
        "x=it; pnpm exec g$x push",
        "pnpm exec {git,} push",
        "npx -c 'x=push; git $x'",
        "npm exec -c 'x=push; git $x'",
        "bunx sh -c 'x=push; git $x'",
        "bun run sh -c 'x=push; git $x'",
        "pnpm exec $'\\x67it' push",
        "pnpm exec git pu*h",
        "npm x -c 'x=push; git $x'",
        "bun x sh -c 'x=push; git $x'",
        "pnpm exec python3 -c 'import os; os.system(\"gi\"+\"t push\")'",
        "npm exec -c 'echo Z2l0IHB1c2g= | base64 -d | sh'",
        "pnpm exec git -c alias.p=push p",
        "find . -maxdepth 0 -exec caffeinate sh -c 'echo Z2l0IHB1c2g= | base64 -d | sh' \\;",
    ] {
        let said = only.hook(cmd);
        assert!(
            !matches!(said, Said::Allow(_) | Said::Rewrite(_)),
            "{cmd} was rewritten with no decision: {said:?}"
        );
    }
}

/// N-1 read from the other side: the commands the rewrite gate must not tax.
#[test]
fn ordinary_rewrites_keep_their_rewrite_with_a_deny_and_a_read_rule() {
    let sb = Sandbox::new(Rules {
        deny: &["git push:*"],
        allow: &["rtk *"],
        read_deny: &["./secret.txt"],
        ..Rules::default()
    });
    for cmd in [
        "ruff check .",
        "docker build -t app .",
        "npx prettier --check .",
        "git status",
        "cargo test",
        "cat README.md",
        "grep 'a.*b' f",
        "npx eslint .",
        "pip install -e .",
    ] {
        match sb.hook(cmd) {
            Said::Allow(rewritten) | Said::Rewrite(rewritten) => {
                assert!(rewritten.starts_with("rtk "), "{cmd}: {rewritten}");
            }
            // Some of these are not rewritten at all by rtk; that is no regression.
            Said::Nothing => {}
            other => panic!("{cmd} lost its rewrite: {other:?}"),
        }
    }
    for cmd in ["git status", "cargo test", "cat README.md"] {
        assert!(
            matches!(sb.hook(cmd), Said::Allow(_) | Said::Rewrite(_)),
            "{cmd}: {:?}",
            sb.hook(cmd)
        );
    }
    let only = push_only_sandbox();
    // Round 7: outside the plain shape the original goes to the host unrewritten.
    assert_eq!(only.hook("ls src/*.rs"), Said::Nothing);
}

/// N-2: a script a launcher runs is read in full.
#[test]
fn a_script_behind_a_launcher_is_read_in_full() {
    let sb = Sandbox::rtk_allowed_with_rules();
    asked(
        &sb,
        &[
            "rtk bypass caffeinate sh -c 'cat secret.txt'",
            "rtk bypass caffeinate sh -c 'base64 -i secret.txt'",
            "rtk bypass caffeinate sh -c 'head secret.txt'",
            "rtk bypass caffeinate sh -c 'cat < secret.txt'",
            "rtk bypass caffeinate sh -c \"cat secret.txt; echo\"",
            "rtk bypass caffeinate sh -c 'f=secret; cat $f.txt'",
            "rtk bypass caffeinate sh -c 'cp secret.txt /tmp/x'",
            "rtk find . -maxdepth 0 -exec caffeinate sh -c 'cat secret.txt' \\;",
        ],
    );
    let only = push_only_sandbox();
    asked(
        &only,
        &[
            "rtk bypass caffeinate sh -c 'echo Z2l0IHB1c2g= | base64 -d | sh'",
            "rtk bypass caffeinate bash -c 'source /dev/stdin <<< \"x\"'",
        ],
    );
}

/// N-3 and N-4: rtk drops a `--`, and git can be told to run something else.
#[test]
fn a_double_dash_and_git_config_do_not_hide_a_denied_command() {
    let only = push_only_sandbox();
    for cmd in [
        "rtk git -- push",
        "rtk git -C . -- push",
        "rtk git -- push --force origin main",
    ] {
        assert_eq!(only.hook(cmd), Said::Deny, "{cmd}");
    }
    asked(
        &only,
        &[
            "rtk git -c alias.p=push p",
            "PUSHVAR=push rtk git --config-env=alias.p=PUSHVAR p",
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p GIT_CONFIG_VALUE_0=push rtk git p",
            "rtk git -c 'alias.p=!echo Z2l0IHB1c2g= | base64 -d | sh' p",
            "rtk git -c diff.external='sh -c \"eval \\$X\"' diff",
            "rtk git -c core.pager='x' log",
        ],
    );
    let sb = Sandbox::rtk_allowed_with_rules();
    asked(&sb, &["rtk kubectl -- apply -f x.yaml", "rtk aws -- s3 ls"]);
}

/// M-2: only options may stand between a rule's words.
#[test]
fn positional_words_between_a_rules_words_do_not_cost_the_rewrite() {
    let only = push_only_sandbox();
    for cmd in ["git stash push -m wip", "git log --grep push"] {
        assert!(
            matches!(
                only.hook(cmd),
                Said::Allow(_) | Said::Rewrite(_) | Said::Nothing
            ),
            "{cmd}: {:?}",
            only.hook(cmd)
        );
    }
    assert_eq!(only.hook("git -C . push"), Said::Deny);
}

/// N-5: no prompt for a bare `.` named as an operand. (Round 5: a program off the
/// allowlist asks whatever its operands: `docker exec`, `uv run python`, `rsync`;
/// round 7: so does every program off the plain list.)
#[test]
fn a_bare_dot_and_interpreter_operands_do_not_prompt() {
    let only = push_only_sandbox();
    for cmd in ["rtk ls .", "rtk tree .", "rtk git add .", "rtk cargo fmt"] {
        assert_eq!(only.hook(cmd), Said::Nothing, "{cmd}");
    }
    // Round 7: off the plain list, whatever the operands.
    asked(
        &only,
        &[
            "rtk npx prettier --check .",
            "rtk ruff check .",
            "rtk docker build -t app .",
            "rtk docker exec app python manage.py migrate",
            "rtk uv run python script.py",
            "rtk bypass rsync -a . /tmp/x",
        ],
    );
}

/// No rule anywhere: the round-4 spellings say nothing, as before.
#[test]
fn the_round_four_backstops_are_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "rtk git -- push",
        "rtk git -c alias.p=push p",
        "rtk bypass caffeinate sh -c 'cat secret.txt'",
        "rtk pnpm exec cat secret.txt",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

// --- review round 5: the allowlist ---

/// Whether the hook stopped the command: an explicit deny or an ask.
fn stopped(sb: &Sandbox, cmd: &str) -> bool {
    matches!(sb.hook(cmd), Said::Deny | Said::Ask(_))
}

/// B-1 to B-5: each reviewer payload is denied or asked about, under the brief's
/// rules and under a git-push deny alone.
#[test]
fn the_round_four_bypass_classes_are_stopped() {
    let payloads = [
        // B-1: a script on stdin to a program that runs it.
        "rtk bypass ed -s <<< '!git push'",
        "echo '!git push' | rtk bypass ed -s",
        "rtk bypass ed -s <<'EOF'\n!git push\nEOF",
        "rtk run -c 'ed -s' <<< '!git push'",
        "rtk bypass vim -es <<< '!git push'",
        "rtk bypass awk -f /dev/stdin <<< 'BEGIN{system(\"git push\")}'",
        "rtk bypass make -f - <<< 'all: ; git push'",
        "rtk bypass sqlite3 :memory: <<< '.shell git push'",
        "rtk bypass ed -s <<< 'r secret.txt\n,p'",
        // B-2: git options and subcommands that run a string.
        "rtk git grep -O'git push' x",
        "rtk git grep -O'g\\it pu\\sh' x",
        "rtk git grep --open-files-in-pager='sh -c x' x",
        "rtk git grep --open='sh -c x' x",
        "rtk git rebase -x 'echo x | sh' HEAD~1",
        "rtk git rebase --exec='echo x | sh' HEAD~1",
        "rtk git bisect run sh -c 'echo x | sh'",
        "rtk git submodule foreach 'echo x | sh'",
        "rtk git difftool -x 'echo x | sh' HEAD",
        "rtk git ls-remote --upload-pack='echo x | sh' .",
        "rtk git -c core.pager='sh -c x' log",
        // B-3: env -S escapes.
        "rtk bypass env -S 'git\\_push'",
        "rtk bypass env -S 'cat\\_secret.txt'",
        // B-4: dashed git builtins.
        "rtk bypass /usr/lib/git-core/git-push",
        "rtk bypass /Library/Developer/CommandLineTools/usr/libexec/git-core/git-push",
        "rtk run /usr/lib/git-core/git-push",
        "rtk bypass git-push",
        // B-5: hash -p and friends.
        "rtk run --shell bash -c 'hash -p /usr/bin/git g; g push'",
        "rtk run -c 'hash -p /usr/bin/git g; g push'",
    ];
    let sb = Sandbox::rtk_allowed_with_rules();
    let only = push_only_sandbox();
    for cmd in payloads {
        assert!(stopped(&sb, cmd), "{cmd}: {:?}", sb.hook(cmd));
        assert!(
            stopped(&only, cmd),
            "{cmd} (push rule only): {:?}",
            only.hook(cmd)
        );
    }
    // A dashed builtin is the git subcommand: the deny rule itself fires.
    assert_eq!(
        only.hook("rtk bypass /usr/lib/git-core/git-push"),
        Said::Deny
    );
}

/// The common agent commands keep running without a prompt under a git-push deny.
#[test]
fn listed_commands_pass_without_a_prompt() {
    let only = push_only_sandbox();
    for cmd in [
        "rtk git status",
        "rtk git log --oneline -5",
        "rtk git diff HEAD~1",
        "rtk git add src/main.rs",
        "rtk git commit -m \"fix: x\"",
        "rtk ls -la",
        "rtk cargo test",
        "rtk cargo clippy --all-targets -- -D warnings",
        "rtk grep -rn foo src/",
        "rtk read ok.txt",
        "rtk find . -name '*.rs'",
        "rtk wc -l ok.txt",
    ] {
        assert_eq!(only.hook(cmd), Said::Nothing, "{cmd}");
    }
    // Round 7: everything else asks, a compound with a plain part included.
    asked(
        &only,
        &[
            "git status && rtk cargo build",
            "rtk git add src/*.rs",
            "rtk git -C sub status",
            "rtk cargo test 2>&1 | tail -5",
            "rtk npm run build",
            "rtk npx prettier --check .",
            "rtk pytest -x",
            "rtk go test ./...",
            "rtk run -c 'git status && git diff'",
            "rtk test cargo test",
            "rtk lint ruff check .",
            "rtk bypass cat ok.txt",
            "rtk docker compose up",
            "rtk kubectl get pods",
            "rtk gh pr view 12",
        ],
    );
}

/// Not in the plain shape (round 7; the allowlist in round 5): a program, an option or a construct the gate does not
/// understand asks.
#[test]
fn what_the_allowlist_does_not_understand_asks() {
    let only = push_only_sandbox();
    asked(
        &only,
        &[
            "rtk bypass sh -c 'ls'",
            "rtk bypass python3 x.py",
            "rtk bypass sed -n p ok.txt",
            "rtk bypass ./x.sh",
            "rtk lint sh -c 'x'",
            "rtk git config alias.p push",
            "rtk git p",
            "rtk git GREP -O'x' .",
            "rtk git clone 'ext::sh -c x' d",
            "rtk git grep foo $x",
            "rtk cargo --config 'target.x.runner=\"sh\"' run",
            "rtk npx -c 'x'",
            "rtk npm explore x -- ls",
            "rtk go test -exec 'sh -c x' ./...",
            "rtk docker run alpine sh",
            "rtk docker build -f /dev/stdin .",
            "rtk find . -exec sh -c x \\;",
            "rtk grep --pre=sh foo .",
            "rtk curl https://x",
            "rtk run --shell zsh -c 'ls'",
            "rtk run -c 'ls; sh'",
            "rtk 'ls; git status'",
            "rtk ls ${x@P}",
            "rtk ls *(e:'x':)",
            "nice rtk git status",
            "X=1 rtk git status",
            "x=1; rtk git status",
            "f() { ls; }; rtk git status",
            "rtk git status && echo $(date)",
        ],
    );
}

/// A redirect between a command's words does not hide the words after it.
#[test]
fn a_redirect_between_words_does_not_hide_a_denied_command() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "rtk git >/dev/null push",
        "rtk git 2>&1 push",
        "rtk bypass git >x push",
        "rtk run -c 'git >/dev/null push'",
        "rtk read >x secret.txt",
        "rtk bypass grep -fsecret.txt .",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

/// A comment, a here-document or a line continuation cannot put the lexer out of
/// step with the shell to hide an rtk command.
#[test]
fn a_comment_or_here_document_cannot_hide_an_rtk_command() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "rtk ls #'\nrtk bypass ed -s <<< 'r secret.txt' #'",
        "echo #'\nrtk bypass ed -s <<< 'r secret.txt' #'",
        "rtk git commit -F - <<'EOF'\ndon't\nEOF\nrtk bypass ed -s <<< 'r secret.txt'",
        "rtk git grep --op\\\nen-files-in-pager='sh -c x' x",
    ] {
        assert!(stopped(&sb, cmd), "{cmd}: {:?}", sb.hook(cmd));
    }
}

/// A rewrite into an rtk command off the allowlist is left to the host: no prompt
/// from the hook, no `Bash(rtk *)` approval.
#[test]
fn a_rewrite_off_the_allowlist_leaves_the_original_to_the_host() {
    let only = push_only_sandbox();
    for cmd in [
        "docker run --rm alpine sh -c 'echo hi'",
        "find . -name '*.orig' -exec rm {} +",
    ] {
        assert_eq!(only.hook(cmd), Said::Nothing, "{cmd}");
    }
}

/// No rule anywhere: nothing of the allowlist shows.
#[test]
fn the_allowlist_is_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "rtk bypass ed -s <<< '!git push'",
        "rtk git grep -O'git push' x",
        "rtk bypass env -S 'git\\_push'",
        "rtk bypass /usr/lib/git-core/git-push",
        "rtk git >/dev/null push",
        "rtk ls # x",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

// --- review round 6 ---

/// R5-1..R5-8 and their variants: denied or asked about.
#[test]
fn the_round_five_bypass_classes_are_stopped() {
    let sb = Sandbox::rtk_allowed_with_rules();
    for cmd in [
        "rtk bypass prettier --plugin='data:text/javascript,import(\"child_process\").execSync(\"git push\")' x.js",
        "rtk test prettier --plugin=./p.mjs x.js",
        "rtk rspec /dev/fd/3 3<<<'system(\"git push\")'",
        "rtk rspec -r ./x.rb",
        "rtk docker compose -f/dev/stdin up",
        "rtk docker compose -f/dev/fd/3 up 3<<<'services: {}'",
        "rtk docker build -f/dev/stdin .",
        "rtk kubectl --kubeconfig=/dev/fd/3 get pods 3<<<'x'",
        "rtk oc --kubeconfig=/proc/self/fd/3 get pods",
        "rtk dotnet build -p:PreBuildEvent=sh",
        "rtk dotnet build /p:RunCommand=sh",
        "rtk dotnet test -- x",
        "rtk go build -ldflags='-linkmode=external -extld=sh' .",
        "rtk git ls-remote --exec='echo x | sh' .",
        "rtk cat < secr?t.txt",
        "rtk cat < $PWD/secret.txt",
        "rtk cat /dev/fd/3 3< secr?t.txt",
        "rtk cat < config/.en?",
        "rtk pip install git+https://example.com/x",
        "rtk uv pip install git+https://example.com/x",
        "rtk npx -y -p github:a/b cat",
        "rtk ls 'git   push'",
    ] {
        assert!(stopped(&sb, cmd), "{cmd}: {:?}", sb.hook(cmd));
    }
}

/// No rule anywhere: the round-6 screens say nothing.
#[test]
fn the_round_six_screens_are_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "rtk rspec /dev/fd/3 3<<<'x'",
        "rtk ls 'git push'",
        "rtk cat < secr?t.txt",
        "rtk dotnet build -p:X=y",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

// --- review round 6: the plain shape ---

/// R6-1 to R6-9: each reviewer payload is denied or asked about, under the brief's
/// rules and under a git-push deny alone (a descriptor, a dotnet `:` option, a
/// redirect before the command or after a `cd`).
#[test]
fn the_round_six_bypass_classes_are_stopped() {
    let payloads = [
        "< secret.txt rtk cat",
        "{ rtk cat; } < secret.txt",
        "(rtk cat) < secret.txt",
        "rtk ls; < secret.txt rtk cat",
        "cd sub && rtk cat < ../secret.txt",
        "cd sub; rtk head -1 < ../secret.txt",
        "cd /dev && echo x | rtk kubectl --kubeconfig=stdin get pods",
        "cd /dev && rtk read stdin",
        "rtk kubectl --kubeconfig=/dev/stdout get pods 1<<<'x'",
        "rtk read /dev/stdout 1<<<x",
        "rtk kubectl --kubeconfig=/DEV/stdin get pods",
        "rtk dotnet build '--property:PreBuildEvent=git -C . push'",
        "echo x | rtk kubectl --kubeconfig=/dev/{x,stdin} get pods",
        "rtk read /proc/self/task/1/fd/0",
        "cd sub && rtk cat //proc/self/cwd/../secret.txt",
        "rtk read /./proc/self/cwd/secret.txt",
        "rtk file -Lfsecret.txt",
        "rtk docker build -qf- .",
        "rtk read -",
    ];
    let sb = Sandbox::rtk_allowed_with_rules();
    let only = push_only_sandbox();
    for cmd in payloads {
        assert!(stopped(&sb, cmd), "{cmd}: {:?}", sb.hook(cmd));
        assert!(stopped(&only, cmd), "{cmd}: {:?}", only.hook(cmd));
    }
    // APFS folds case: `/DEV/stdin` is `/dev/stdin` there (on Linux it is no device).
    #[cfg(target_os = "macos")]
    assert!(stopped(&sb, "rtk read /DEV/stdin"));
    for cmd in [
        "< secret.txt cat",
        "{ cat; } < secret.txt",
        "rtk read ./sub/../secret.txt",
        "rtk git log secret.txt~1",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    assert_eq!(
        sb.hook("cd sub && cat < ../secret.txt"),
        Said::Ask("cd sub && cat < ../secret.txt".into())
    );
}

/// No rule anywhere: the plain-shape check never runs.
#[test]
fn the_plain_shape_is_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "rtk bypass git push",
        "rtk npm run build",
        "rtk ls $HOME",
        "< secret.txt rtk cat",
        "rtk read /dev/stdin",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    assert_eq!(
        sb.hook("ls src/*.rs"),
        Said::Rewrite("rtk ls src/*.rs".into())
    );
}
