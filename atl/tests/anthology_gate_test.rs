//! Anthology fork (C1): `atl hook claude` never auto-allows past a deny or ask
//! rule, however many `atl` prefixes or shell wrappers the command hides behind.
//!
//! The install allows `Bash(atl *)`, and Claude Code judges the command the hook
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
const DENY_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"ATL: a deny rule in the user's, project's or managed Claude Code settings matches the command or a command it runs."}}"#;

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

    /// The rules of the brief: the `Bash(atl *)` allow the plugin installs, next
    /// to deny, ask and `Read()` rules.
    fn atl_allowed_with_rules() -> Self {
        Self::new(Rules {
            deny: &["git push:*", "curl:*"],
            ask: &["kubectl apply:*", "aws:*"],
            allow: &["atl *"],
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
        let mut command = common::atl_command();
        command
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("CLAUDE_CONFIG_DIR", &self.claude_home)
            .env("ATL_DB_PATH", self.project.join("atl.db"))
            .env("ATL_TEE_DIR", self._root.path().join("tee"))
            .env("ATL_RECALL_DB", self._root.path().join("recall.db"))
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn atl");
        let mut pipe = child.stdin.take().expect("stdin");
        if let Some(input) = stdin {
            // The hook may stop reading (an oversize payload) and exit first.
            let _ = pipe.write_all(input.as_bytes());
        }
        drop(pipe);
        let out = child.wait_with_output().expect("wait atl");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(
            !stderr.contains("panicked"),
            "atl panicked on {args:?}: {stderr}"
        );
        (
            out.status.code().expect("exit code"),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr,
        )
    }

    /// Feed `cmd` to `atl hook claude` and return stdout verbatim.
    fn hook_raw(&self, cmd: &str, env: &[(&str, &str)]) -> String {
        let (code, stdout, stderr) =
            self.spawn(&["hook", "claude"], Some(Self::payload(cmd, None)), env);
        // The hook protocol requires exit 0 whatever it decides; without this a
        // crash is indistinguishable from a deliberate defer.
        assert_eq!(
            code, 0,
            "atl hook claude exited {code} for {cmd:?}: {stderr}"
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
fn deny_rules_hold_past_every_atl_prefix_and_wrapper() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "curl x",
        "atl curl x",
        "atl git push --force origin main",
        "atl bypass git push --force",
        "atl proxy git push",
        "atl run git push",
        "atl run -c 'git push'",
        "atl run --shell bash -c 'git push'",
        "atl err git push",
        "atl test git push",
        "atl summary git push",
        "atl -v git push",
        "atl --skip-env git push",
        "FOO=1 atl git push",
        "timeout 5 atl git push",
        "git status && atl git push",
        "/some/dir/atl git push",
        "bash -c 'git push'",
        "atl run atl run git push",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

#[test]
fn ask_rules_become_hook_asks_that_the_atl_allow_cannot_beat() {
    let sb = Sandbox::atl_allowed_with_rules();
    assert_eq!(
        sb.hook("kubectl apply -f x.yaml"),
        // Round 7: `atl kubectl` is not in the plain shape, so no rewrite.
        Said::Ask("kubectl apply -f x.yaml".into())
    );
    assert_eq!(
        sb.hook("atl kubectl apply -f x.yaml"),
        Said::Ask("atl kubectl apply -f x.yaml".into())
    );
    assert_eq!(
        sb.hook("aws s3 rm s3://b --recursive"),
        Said::Ask("aws s3 rm s3://b --recursive".into())
    );
    // No rewrite exists for an already-prefixed command: the hook asks about it
    // as written.
    assert_eq!(
        sb.hook("atl run aws s3 ls"),
        Said::Ask("atl run aws s3 ls".into())
    );
}

#[test]
fn read_deny_rules_block_the_commands_atl_turns_into_reads() {
    let sb = Sandbox::atl_allowed_with_rules();
    let mut denied = vec![
        "cat secret.txt",
        "head -n 5 secret.txt",
        "tail secret.txt",
        "cat ./x/../secret.txt",
        "atl read secret.txt",
        "atl run cat secret.txt",
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
    let sb = Sandbox::atl_allowed_with_rules();
    // Another file still rewrites, as it did before.
    assert_eq!(
        sb.hook("cat ok.txt"),
        Said::Rewrite("atl read ok.txt".into())
    );
    // A glob might name the denied file: the hook asks (round 6: at top level too).
    assert_eq!(sb.hook("cat *.txt"), Said::Ask("cat *.txt".into()));
    assert_eq!(
        sb.hook("cat < sec?et.txt"),
        Said::Ask("cat < sec?et.txt".into())
    );
    // Inside an atl expansion the same uncertainty asks.
    assert_eq!(
        sb.hook("atl read *.txt"),
        Said::Ask("atl read *.txt".into())
    );
}

#[test]
fn an_opaque_inner_command_asks() {
    let sb = Sandbox::atl_allowed_with_rules();
    assert_eq!(
        sb.hook("atl run -c 'git $(echo push)'"),
        Said::Ask("atl run -c 'git $(echo push)'".into())
    );
}

/// No rule anywhere: the hook says what it said before this change, byte for byte.
#[test]
fn the_bare_sandbox_is_unchanged() {
    let sb = Sandbox::bare();
    assert_eq!(
        sb.hook_raw("curl x", &[]).trim(),
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecisionReason":"ATL auto-rewrite","updatedInput":{"command":"atl curl x"}}}"#
    );
    assert_eq!(sb.hook("atl git status"), Said::Nothing);
    assert_eq!(sb.hook_raw("atl git status", &[]), "");
    assert_eq!(
        sb.hook_raw("cat secret.txt", &[]).trim(),
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecisionReason":"ATL auto-rewrite","updatedInput":{"command":"atl read secret.txt"}}}"#
    );
    assert_eq!(
        sb.hook("atl git push"),
        Said::Nothing,
        "the owner's accepted Bash(atl *) risk: no rule, no decision"
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
        r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecisionReason":"ATL auto-rewrite","updatedInput":{"command":"atl git status"},"permissionDecision":"allow"}}"#
    );
}

#[test]
fn the_payload_cwd_is_where_a_relative_path_and_a_relative_rule_anchor() {
    let sb = Sandbox::atl_allowed_with_rules();
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
        Said::Rewrite("atl read ok.txt".into())
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
    assert_eq!(sb.hook("atl git push"), Said::Nothing);
    let stdout = sb.hook_raw("atl git push", &[("CLAUDE_PROJECT_DIR", &dir)]);
    assert_eq!(Sandbox::classify("atl git push", &stdout), Said::Deny);
}

#[test]
fn hook_check_for_claude_reports_the_same_denials() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in ["atl git push", "bash -c 'git push'", "cat secret.txt"] {
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
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        // C1: quoted words
        "atl git 'push' --force origin main",
        "atl git \"push\"",
        "atl bypass git p'u'sh",
        "atl run -c 'git pu\\sh'",
        "git 'push'",
        // C2: a single string run through sh -c
        "atl run 'git push --force'",
        "atl test 'git push'",
        "atl err 'git push'",
        "atl summary 'git push'",
        "atl run 'echo ok; curl x'",
        // C3: exec payloads, including a plain find the hook would rewrite
        "find . -exec git push \\;",
        "find . -execdir git push \\;",
        "find . -ok git push \\;",
        "find . -exec curl x \\;",
        "atl bypass xargs -n1 git push",
        "atl run env -i git push",
        "atl run env -u X git push",
        // I2: Read() deny through any program inside atl
        "atl sed -n p secret.txt",
        "atl awk 1 secret.txt",
        "atl bypass base64 secret.txt",
        "atl bypass cp secret.txt out.txt",
        "atl bypass jq -R . secret.txt",
        // Minor: a read-write redirect reads
        "atl bypass sort <>secret.txt",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    for cmd in [
        "atl kubectl 'apply' -f x.yaml",
        "atl run kubectl \"apply\" -f x.yaml",
    ] {
        assert!(matches!(sb.hook(cmd), Said::Ask(_)), "{cmd}");
    }
}

#[test]
fn what_the_gate_cannot_read_is_asked_or_left_unrewritten() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "atl run -c 'g=git; $g push'",
        "atl bypass python3 -c 'import os; os.system(\"curl x\")'",
        "atl bypass busybox sh -c 'curl x'",
    ] {
        // a deny is stricter than an ask: the shell analysis (SH-FW) reads busybox's `sh -c` script
        assert!(
            matches!(sb.hook(cmd), Said::Ask(_) | Said::Deny),
            "{cmd}: {:?}",
            sb.hook(cmd)
        );
    }
    // A top-level find whose payload the gate cannot read is not rewritten, so
    // `Bash(atl *)` never sees `atl find . -exec sh {} \\;`.
    assert_eq!(sb.hook("find . -exec sh {} \\;"), Said::Nothing);
    assert!(matches!(
        sb.hook("atl find . -exec sh {} \\;"),
        Said::Ask(_)
    ));
    // RG1: anthology_shell::analyze reads what runs: these are denied now
    // (they were asked about before).
    for cmd in [
        "atl run eval 'git push'",
        "atl run -c 'eval \"git push\"'",
        "echo 'curl x' | atl bypass bash",
        "echo 'curl x' | atl bypass sh -s",
        "atl bypass bash <<< 'curl x'",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

#[test]
fn read_ask_rules_are_not_rewritten_into_an_allow() {
    let sb = Sandbox::atl_allowed_with_rules();
    assert_eq!(sb.hook("cat asked.txt"), Said::Nothing);
    assert_eq!(sb.hook("head asked.txt"), Said::Nothing);
    assert!(matches!(sb.hook("atl read asked.txt"), Said::Ask(_)));
}

#[test]
fn an_unreadable_payload_asks_instead_of_failing_open() {
    let sb = Sandbox::atl_allowed_with_rules();
    let ask = |stdout: &str| {
        let v: serde_json::Value = serde_json::from_str(stdout).expect("JSON");
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "ask");
        assert!(v["hookSpecificOutput"].get("updatedInput").is_none());
    };
    // Over the 1 MiB limit.
    let mut big = String::from("atl bypass git push #");
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
        allow: &["atl *"],
        read_deny: &["**/*key*.json"],
        ..Rules::default()
    });
    let started = std::time::Instant::now();
    let cmd = format!("cat {}; atl git push", "key".repeat(100_000));
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
        // a deny is stricter than an ask: the shell analysis (SH-FW) now reads the ruled command itself
        // (`$'\x67it'`, `script ... git push`, xargs fed `push`), so the rule denies it
        assert!(
            matches!(sb.hook(cmd), Said::Ask(_) | Said::Deny),
            "{cmd}: {:?}",
            sb.hook(cmd)
        );
    }
}

/// N1: a word that expands may become a ruled command.
#[test]
fn a_word_that_may_expand_into_a_ruled_command_is_not_allowed() {
    let sb = Sandbox::atl_allowed_with_rules();
    asked(
        &sb,
        &[
            "x=push; atl git $x",
            "atl git $X",
            "atl git ${X}",
            "atl git \"$x\"",
            "x=push atl git $x",
            "atl run git $x",
            "atl bypass git $x",
            "atl run -c 'x=push; git $x'",
            "atl run -c 'git ${x:-push}'",
            "atl run -c 'git pu${x}sh'",
            "atl git {push,}",
            "atl bypass git {push,}",
            "atl git $'\\x70ush'",
            "atl kubectl $'apply' -f x",
            "atl kubectl $a -f x.yaml",
            "atl git pu*h",
            "atl git pus?",
            "atl git pus[h]",
        ],
    );
    // A top level the hook would rewrite is left to the host unrewritten.
    for cmd in ["git $x", "git pu*h", "git {push,}"] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    // RG1: anthology_shell::analyze reads what runs: these are denied now
    // (they were asked about before).
    for cmd in ["atl git $'push'", "atl git $\"push\""] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

/// N2: grammar words and lowercase assignments in front of a ruled command.
#[test]
fn shell_grammar_in_front_of_a_ruled_command_does_not_hide_it() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "if true; then atl git push; fi",
        "while true; do atl git push; break; done",
        "until false; do atl git push; break; done",
        "for i in 1; do atl git push; done",
        "if atl git push; then :; fi",
        "function f { atl git push; }; f",
        "coproc atl git push",
        "x=1 atl git push",
        "a=1 b=2 atl git push",
        "A=1 b=2 atl git push",
        "atl run -c 'if true; then git push; fi'",
        "atl run -c 'while git push; do break; done'",
        "atl run -c 'until git push; do :; done'",
        "atl run -c 'for i in 1; do git push; done'",
        "atl run -c 'select i in 1; do git push; done'",
        "atl run -c 'function f { git push; }; f'",
        "atl run -c 'coproc git push'",
        "atl run -c 'x=1 git push'",
        "atl run -c 'a=1 b=2 git push'",
        "atl run -c 'A=1 b=2 git push'",
        "atl run -c 'time -p git push'",
        "atl run -c '! git push'",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    asked(
        &sb,
        &[
            "atl run -c 'case x in x) echo;; esac'",
            "atl run -c '[[ -f x ]] && echo y'",
        ],
    );
}

/// N3 and N4: interpreters, other shells and programs that run their arguments.
#[test]
fn interpreters_and_launchers_inside_atl_ask() {
    let sb = Sandbox::atl_allowed_with_rules();
    asked(
        &sb,
        &[
            "atl bypass python3 -X dev -c 'import os; os.system(\"git push\")'",
            "atl bypass perl -I lib -e 'system(\"git push\")'",
            "atl bypass ruby -I lib -e 'x'",
            "atl bypass node -r fs -e 'x'",
            "atl bypass python3 <<< 'import os'",
            "echo x | atl bypass python3 -",
            "atl bypass python3 script.py",
            "atl bypass tcsh",
            "atl bypass csh -f script.csh",
            "atl bypass pwsh -c 'git push'",
            "atl bypass expect -c 'spawn git push'",
            "atl bypass tclsh <<< 'exec git push'",
            "atl bypass Rscript -e 'system(\"git push\")'",
            "atl bypass xcrun git push",
            "atl bypass script -q /dev/null git push",
            "atl bypass sandbox-exec -p x git push",
            "atl bypass flock /tmp/l git push",
            "atl bypass chrt 0 git push",
            "atl bypass taskset 1 git push",
            "atl bypass su -c 'git push'",
            "atl bypass ssh localhost git push",
            "atl grep \"git push\" docs/",
        ],
    );
    assert_eq!(sb.hook("atl bypass tcsh -c 'git push'"), Said::Deny);
    assert_eq!(sb.hook("atl bypass parallel git push ::: a"), Said::Deny);
    // At top level a command atl rewrites into text that holds a ruled head asks
    // (the rewrite is gated too).
    for cmd in ["ssh localhost git push", "git log --grep='git push'"] {
        assert!(matches!(sb.hook(cmd), Said::Ask(_)), "{cmd}");
    }
    // RG1: anthology_shell::analyze reads what runs: these are denied now
    // (they were asked about before).
    for cmd in [
        "atl bypass caffeinate git push",
        "atl bypass arch -arm64 git push",
        "atl bypass watch git push",
        "atl bypass setsid git push",
        "atl bypass unbuffer git push",
        "atl bypass ionice -c3 git push",
        "atl bypass gtimeout 5 git push",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

/// N5: HOME and CDPATH assignments hidden behind quotes or computed names.
#[test]
fn a_hidden_home_or_cdpath_change_makes_tilde_unknown() {
    let sb = Sandbox::new(Rules {
        read_deny: &["~/.ssh/**"],
        allow: &["atl *"],
        ..Rules::default()
    });
    asked(
        &sb,
        &[
            "export HO\"ME\"=/; atl read ~/.ssh/id_rsa",
            "export H\\OME=/; atl read ~/.ssh/id_rsa",
            "v=HO; export ${v}ME=/; atl read ~/.ssh/id_rsa",
            "declare ${v}ME=/; atl read ~/.ssh/id_rsa",
        ],
    );
    assert_eq!(sb.hook("export FOO=1; atl read ~/.ssh/id_rsa"), Said::Deny);
}

/// N6: with the work budget spent, a rule still means ask.
#[test]
fn a_command_too_long_to_scan_asks_when_a_rule_exists() {
    let sb = Sandbox::atl_allowed_with_rules();
    let many = "true;".repeat(5000);
    asked(
        &sb,
        &[
            &format!("{many}\"a\"tl read secret.txt"),
            &format!("{many}a\\tl read secret.txt"),
        ],
    );
}

/// N7 and Minor 1: operands that expand, and `key=value` operands.
#[test]
fn an_operand_that_expands_or_hides_a_path_is_not_allowed() {
    let sb = Sandbox::atl_allowed_with_rules();
    asked(
        &sb,
        &[
            "atl bypass base64 secret.tx?",
            "atl bypass base64 secre*",
            "atl bypass base64 {secret,x}.txt",
            "f=secret.txt; atl bypass base64 $f",
            "atl bypass sed -n p \"$PWD/secret.txt\"",
            "atl bypass base64 ~root/.ssh/id_rsa",
        ],
    );
    assert_eq!(sb.hook("atl bypass dd if=secret.txt"), Said::Deny);
    // RG1: anthology_shell::analyze reads what runs: these are denied now
    // (they were asked about before).
    let cmd = "atl bypass base64 $'secret.txt'";
    assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
}

/// The backstops must not tax ordinary commands the rules cannot reach.
#[test]
fn ordinary_commands_keep_their_rewrite_beside_an_unrelated_rule() {
    let sb = Sandbox::new(Rules {
        deny: &["git push:*"],
        allow: &["atl *"],
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
                assert!(rewritten.starts_with("atl "), "{cmd}: {rewritten}");
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

/// Every line of `cmds` runs `atl bypass`, which asks even with no rule of any
/// kind: `Bash(atl *)` must not approve what it runs (see "`atl bypass` is judged
/// as the command it runs" below).
fn assert_bypass_asks(sb: &Sandbox, cmds: &[&str]) {
    for cmd in cmds {
        assert!(matches!(sb.hook(cmd), Said::Ask(_)), "{cmd}");
    }
}

/// With no rule of any kind every backstop is silent; `atl bypass` alone asks.
#[test]
fn every_backstop_is_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "x=push; atl git $x",
        "if true; then atl git push; fi",
        "export HO\"ME\"=/; atl read ~/.ssh/id_rsa",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    assert_bypass_asks(
        &sb,
        &[
            "atl bypass caffeinate git push",
            "atl bypass python3 script.py",
            "atl bypass base64 secre*",
        ],
    );
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
        allow: &["atl *"],
        ..Rules::default()
    })
}

/// C-A: options between a rule's words.
#[test]
fn options_between_a_rules_words_do_not_hide_it() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "git -C . push origin main --force",
        "git -C sub push",
        "git --git-dir=.git push",
        "git --work-tree=. push",
        "git -c x.y=z push",
        "git --no-pager push",
        "GIT_DIR=.git git -C . push",
        "cd sub && git -C .. push",
        "atl git -C . push",
        "atl -v git -C . push",
        "atl git -c x.y=z push",
        "atl git --no-pager push",
        "atl git --git-dir=.git push",
        "atl bypass git -C . push",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    asked(
        &sb,
        &[
            "atl kubectl --context c apply -f x.yaml",
            "atl kubectl -n prod apply -f x.yaml",
        ],
    );
    assert_eq!(
        sb.hook("kubectl -n prod apply -f x.yaml"),
        Said::Nothing,
        "the host judges the original"
    );
    let only = push_only_sandbox();
    asked(&only, &["atl git -C . $x"]);
    assert!(matches!(
        only.hook("git commit -m 'push fix'"),
        Said::Allow(_) | Said::Rewrite(_)
    ));
}

/// C-B: a crafted run of grammar words must not stall the hook.
#[test]
fn a_crafted_run_of_grammar_words_does_not_stall_the_hook() {
    let sb = Sandbox::atl_allowed_with_rules();
    let started = std::time::Instant::now();
    let inside = format!(
        "atl run -c '{}git push {}'",
        "time ".repeat(20_000),
        "x ".repeat(20_000)
    );
    assert!(matches!(sb.hook(&inside), Said::Ask(_) | Said::Deny));
    let top = format!(
        "{}atl git status {}",
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
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "atl /usr/bin/git push",
        "atl bypass /usr/bin/git push",
        "atl =git push",
        "atl /usr/bin/curl x",
        // RG1: the shared parser peels the launcher (it was asked about before).
        "atl bypass caffeinate =git push",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    if cfg!(any(target_os = "macos", windows)) {
        for cmd in [
            "atl GIT push",
            "atl Git push",
            "atl bypass GIT push",
            "atl CURL x",
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
            "atl bypass ssh h \"g\"\"it push\"",
            "x=it; atl bypass caffeinate g$x push",
            "atl bypass caffeinate {git,} push",
            "atl bypass caffeinate $'\\x67it' push",
            "atl bypass caffeinate git pu*h",
            "atl bypass caffeinate bash -c 'x=push; git $x'",
            "echo 'git push' | atl bypass caffeinate python3",
            "atl bypass ssh h 'x=push; git $x'",
            "atl git -c 'alias.p=!git push' p",
        ],
    );
    // RG1: anthology_shell::analyze reads what runs: these are denied now
    // (they were asked about before).
    for cmd in [
        "atl bypass caffeinate g''it push",
        "atl bypass caffeinate gi\\t push",
        "atl bypass caffeinate \"g\"it push",
        "atl bypass exec -a x g''it push",
        "atl bypass command -p g''it push",
        "atl bypass caffeinate sh -c 'g\"\"it push'",
        "echo 'git push' | atl bypass caffeinate bash",
        "atl bypass caffeinate bash <<< 'g''it push'",
        "atl bypass caffeinate eval 'g\"\"it push'",
        "atl bypass env -i caffeinate g''it push",
        "atl bypass caffeinate env g''it push",
        "atl bypass caffeinate find . -maxdepth 0 -exec g''it push \\;",
        "atl bypass npm exec -c 'g\"\"it push'",
        "atl bypass watch -n1 'g\"\"it push'",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

/// I-C: arguments from stdin or the file system complete a rule's head.
#[test]
fn arguments_from_stdin_or_the_filesystem_are_asked_about() {
    let sb = push_only_sandbox();
    asked(
        &sb,
        &[
            "echo push | atl bypass xargs git",
            "printf push | atl bypass xargs -n1 git",
            "atl bypass xargs -a args.txt git",
            "atl bypass caffeinate xargs git <<< push",
            "touch push; atl bypass find . -name push -execdir git {} \\;",
            "atl bypass parallel git ::: push",
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
                assert!(rewritten.starts_with("atl "), "{cmd}: {rewritten}");
            }
            other => panic!("{cmd} lost its rewrite: {other:?}"),
        }
    }
    for cmd in [
        "atl git status",
        "atl grep python src/",
        "atl git log --oneline",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

/// No rule anywhere: every round-3 spelling says nothing, as before.
#[test]
fn the_round_three_backstops_are_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "atl git -C . push",
        "atl /usr/bin/git push",
        "atl run -c 'time time git push'",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    assert_bypass_asks(
        &sb,
        &[
            "atl bypass caffeinate g''it push",
            "echo push | atl bypass xargs git",
            "atl bypass ssh h 'x=push; git $x'",
        ],
    );
}

// --- review round 4 ---

/// N-1: a command atl rewrites into `atl <launcher> ...` is gated as that text.
#[test]
fn the_text_the_hook_rewrites_a_command_into_is_gated_too() {
    let sb = Sandbox::atl_allowed_with_rules();
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
        allow: &["atl *"],
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
                assert!(rewritten.starts_with("atl "), "{cmd}: {rewritten}");
            }
            // Some of these are not rewritten at all by atl; that is no regression.
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
    let sb = Sandbox::atl_allowed_with_rules();
    asked(&sb, &["atl bypass caffeinate sh -c 'f=secret; cat $f.txt'"]);
    let only = push_only_sandbox();
    asked(
        &only,
        &[
            "atl bypass caffeinate sh -c 'echo Z2l0IHB1c2g= | base64 -d | sh'",
            "atl bypass caffeinate bash -c 'source /dev/stdin <<< \"x\"'",
        ],
    );
    // RG1: anthology_shell::analyze reads what runs: these are denied now
    // (they were asked about before).
    for cmd in [
        "atl bypass caffeinate sh -c 'cat secret.txt'",
        "atl bypass caffeinate sh -c 'head secret.txt'",
        "atl bypass caffeinate sh -c 'cat < secret.txt'",
        "atl bypass caffeinate sh -c \"cat secret.txt; echo\"",
        "atl find . -maxdepth 0 -exec caffeinate sh -c 'cat secret.txt' \\;",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    // A program that is no read, in a script atl runs, is still asked about.
    asked(
        &sb,
        &[
            "atl bypass caffeinate sh -c 'base64 -i secret.txt'",
            "atl bypass caffeinate sh -c 'cp secret.txt /tmp/x'",
        ],
    );
}

/// N-3 and N-4: atl drops a `--`, and git can be told to run something else.
#[test]
fn a_double_dash_and_git_config_do_not_hide_a_denied_command() {
    let only = push_only_sandbox();
    for cmd in [
        "atl git -- push",
        "atl git -C . -- push",
        "atl git -- push --force origin main",
    ] {
        assert_eq!(only.hook(cmd), Said::Deny, "{cmd}");
    }
    asked(
        &only,
        &[
            "atl git -c alias.p=push p",
            "PUSHVAR=push atl git --config-env=alias.p=PUSHVAR p",
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.p GIT_CONFIG_VALUE_0=push atl git p",
            "atl git -c 'alias.p=!echo Z2l0IHB1c2g= | base64 -d | sh' p",
            "atl git -c diff.external='sh -c \"eval \\$X\"' diff",
            "atl git -c core.pager='x' log",
        ],
    );
    let sb = Sandbox::atl_allowed_with_rules();
    asked(&sb, &["atl kubectl -- apply -f x.yaml", "atl aws -- s3 ls"]);
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
    for cmd in ["atl ls .", "atl tree .", "atl git add .", "atl cargo fmt"] {
        assert_eq!(only.hook(cmd), Said::Nothing, "{cmd}");
    }
    // Round 7: off the plain list, whatever the operands.
    asked(
        &only,
        &[
            "atl npx prettier --check .",
            "atl ruff check .",
            "atl docker build -t app .",
            "atl docker exec app python manage.py migrate",
            "atl uv run python script.py",
            "atl bypass rsync -a . /tmp/x",
        ],
    );
}

/// No rule anywhere: the round-4 spellings say nothing, as before.
#[test]
fn the_round_four_backstops_are_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "atl git -- push",
        "atl git -c alias.p=push p",
        "atl pnpm exec cat secret.txt",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    assert_bypass_asks(&sb, &["atl bypass caffeinate sh -c 'cat secret.txt'"]);
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
        "atl bypass ed -s <<< '!git push'",
        "echo '!git push' | atl bypass ed -s",
        "atl bypass ed -s <<'EOF'\n!git push\nEOF",
        "atl run -c 'ed -s' <<< '!git push'",
        "atl bypass vim -es <<< '!git push'",
        "atl bypass awk -f /dev/stdin <<< 'BEGIN{system(\"git push\")}'",
        "atl bypass make -f - <<< 'all: ; git push'",
        "atl bypass sqlite3 :memory: <<< '.shell git push'",
        "atl bypass ed -s <<< 'r secret.txt\n,p'",
        // B-2: git options and subcommands that run a string.
        "atl git grep -O'git push' x",
        "atl git grep -O'g\\it pu\\sh' x",
        "atl git grep --open-files-in-pager='sh -c x' x",
        "atl git grep --open='sh -c x' x",
        "atl git rebase -x 'echo x | sh' HEAD~1",
        "atl git rebase --exec='echo x | sh' HEAD~1",
        "atl git bisect run sh -c 'echo x | sh'",
        "atl git submodule foreach 'echo x | sh'",
        "atl git difftool -x 'echo x | sh' HEAD",
        "atl git ls-remote --upload-pack='echo x | sh' .",
        "atl git -c core.pager='sh -c x' log",
        // B-3: env -S escapes.
        "atl bypass env -S 'git\\_push'",
        "atl bypass env -S 'cat\\_secret.txt'",
        // B-4: dashed git builtins.
        "atl bypass /usr/lib/git-core/git-push",
        "atl bypass /Library/Developer/CommandLineTools/usr/libexec/git-core/git-push",
        "atl run /usr/lib/git-core/git-push",
        "atl bypass git-push",
        // B-5: hash -p and friends.
        "atl run --shell bash -c 'hash -p /usr/bin/git g; g push'",
        "atl run -c 'hash -p /usr/bin/git g; g push'",
    ];
    let sb = Sandbox::atl_allowed_with_rules();
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
        only.hook("atl bypass /usr/lib/git-core/git-push"),
        Said::Deny
    );
}

/// The common agent commands keep running without a prompt under a git-push deny.
#[test]
fn listed_commands_pass_without_a_prompt() {
    let only = push_only_sandbox();
    for cmd in [
        "atl git status",
        "atl git log --oneline -5",
        "atl git diff HEAD~1",
        "atl git add src/main.rs",
        "atl git commit -m \"fix: x\"",
        "atl ls -la",
        "atl cargo test",
        "atl cargo clippy --all-targets",
        "atl git commit -m 'feat!: breaking'",
        "atl git log HEAD~5..HEAD",
        "atl git status -uno",
        "atl grep -rn foo src/",
        "atl read ok.txt",
        "atl find . -name '*.rs'",
        "atl wc -l ok.txt",
    ] {
        assert_eq!(only.hook(cmd), Said::Nothing, "{cmd}");
    }
    // Round 7: everything else asks, a compound with a plain part included.
    asked(
        &only,
        &[
            "git status && atl cargo build",
            // Round 8: after cargo's `--`, an `@` word, `git add -f`, a `:` in git.
            "atl cargo clippy --all-targets -- -D warnings",
            "atl cargo clippy -- -D @secret.txt",
            "atl git add -f .",
            "atl git show --pretty HEAD:secret.txt",
            "atl git diff -U HEAD:secret.txt --unified HEAD:ok.txt",
            "atl git add src/*.rs",
            "atl git -C sub status",
            "atl cargo test 2>&1 | tail -5",
            "atl npm run build",
            "atl npx prettier --check .",
            "atl pytest -x",
            "atl go test ./...",
            "atl run -c 'git status && git diff'",
            "atl test cargo test",
            "atl lint ruff check .",
            "atl bypass cat ok.txt",
            "atl docker compose up",
            "atl kubectl get pods",
            "atl gh pr view 12",
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
            "atl bypass sh -c 'ls'",
            "atl bypass python3 x.py",
            "atl bypass sed -n p ok.txt",
            "atl bypass ./x.sh",
            "atl lint sh -c 'x'",
            "atl git config alias.p push",
            "atl git p",
            "atl git GREP -O'x' .",
            "atl git clone 'ext::sh -c x' d",
            "atl git grep foo $x",
            "atl cargo --config 'target.x.runner=\"sh\"' run",
            "atl npx -c 'x'",
            "atl npm explore x -- ls",
            "atl go test -exec 'sh -c x' ./...",
            "atl docker run alpine sh",
            "atl docker build -f /dev/stdin .",
            "atl find . -exec sh -c x \\;",
            "atl grep --pre=sh foo .",
            "atl curl https://x",
            "atl run --shell zsh -c 'ls'",
            "atl run -c 'ls; sh'",
            "atl 'ls; git status'",
            "atl ls ${x@P}",
            "atl ls *(e:'x':)",
            "nice atl git status",
            "X=1 atl git status",
            "x=1; atl git status",
            "f() { ls; }; atl git status",
            "atl git status && echo $(date)",
        ],
    );
}

/// A redirect between a command's words does not hide the words after it.
#[test]
fn a_redirect_between_words_does_not_hide_a_denied_command() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "atl git >/dev/null push",
        "atl git 2>&1 push",
        "atl bypass git >x push",
        "atl run -c 'git >/dev/null push'",
        "atl read >x secret.txt",
        "atl bypass grep -fsecret.txt .",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

/// A comment, a here-document or a line continuation cannot put the lexer out of
/// step with the shell to hide an atl command.
#[test]
fn a_comment_or_here_document_cannot_hide_an_atl_command() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "atl ls #'\natl bypass ed -s <<< 'r secret.txt' #'",
        "echo #'\natl bypass ed -s <<< 'r secret.txt' #'",
        "atl git commit -F - <<'EOF'\ndon't\nEOF\natl bypass ed -s <<< 'r secret.txt'",
        "atl git grep --op\\\nen-files-in-pager='sh -c x' x",
    ] {
        assert!(stopped(&sb, cmd), "{cmd}: {:?}", sb.hook(cmd));
    }
}

/// A rewrite into an atl command off the allowlist is left to the host: no prompt
/// from the hook, no `Bash(atl *)` approval.
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
        "atl git grep -O'git push' x",
        "atl git >/dev/null push",
        "atl ls # x",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    assert_bypass_asks(
        &sb,
        &[
            "atl bypass ed -s <<< '!git push'",
            "atl bypass env -S 'git\\_push'",
            "atl bypass /usr/lib/git-core/git-push",
        ],
    );
}

// --- review round 6 ---

/// R5-1..R5-8 and their variants: denied or asked about.
#[test]
fn the_round_five_bypass_classes_are_stopped() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "atl bypass prettier --plugin='data:text/javascript,import(\"child_process\").execSync(\"git push\")' x.js",
        "atl test prettier --plugin=./p.mjs x.js",
        "atl rspec /dev/fd/3 3<<<'system(\"git push\")'",
        "atl rspec -r ./x.rb",
        "atl docker compose -f/dev/stdin up",
        "atl docker compose -f/dev/fd/3 up 3<<<'services: {}'",
        "atl docker build -f/dev/stdin .",
        "atl kubectl --kubeconfig=/dev/fd/3 get pods 3<<<'x'",
        "atl oc --kubeconfig=/proc/self/fd/3 get pods",
        "atl dotnet build -p:PreBuildEvent=sh",
        "atl dotnet build /p:RunCommand=sh",
        "atl dotnet test -- x",
        "atl go build -ldflags='-linkmode=external -extld=sh' .",
        "atl git ls-remote --exec='echo x | sh' .",
        "atl cat < secr?t.txt",
        "atl cat < $PWD/secret.txt",
        "atl cat /dev/fd/3 3< secr?t.txt",
        "atl cat < config/.en?",
        "atl pip install git+https://example.com/x",
        "atl uv pip install git+https://example.com/x",
        "atl npx -y -p github:a/b cat",
        "atl ls 'git   push'",
    ] {
        assert!(stopped(&sb, cmd), "{cmd}: {:?}", sb.hook(cmd));
    }
}

/// No rule anywhere: the round-6 screens say nothing.
#[test]
fn the_round_six_screens_are_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "atl rspec /dev/fd/3 3<<<'x'",
        "atl ls 'git push'",
        "atl cat < secr?t.txt",
        "atl dotnet build -p:X=y",
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
        "< secret.txt atl cat",
        "{ atl cat; } < secret.txt",
        "(atl cat) < secret.txt",
        "atl ls; < secret.txt atl cat",
        "cd sub && atl cat < ../secret.txt",
        "cd sub; atl head -1 < ../secret.txt",
        "cd /dev && echo x | atl kubectl --kubeconfig=stdin get pods",
        "cd /dev && atl read stdin",
        "atl kubectl --kubeconfig=/dev/stdout get pods 1<<<'x'",
        "atl read /dev/stdout 1<<<x",
        "atl kubectl --kubeconfig=/DEV/stdin get pods",
        "atl dotnet build '--property:PreBuildEvent=git -C . push'",
        "echo x | atl kubectl --kubeconfig=/dev/{x,stdin} get pods",
        "atl read /proc/self/task/1/fd/0",
        "cd sub && atl cat //proc/self/cwd/../secret.txt",
        "atl read /./proc/self/cwd/secret.txt",
        "atl file -Lfsecret.txt",
        "atl docker build -qf- .",
        "atl read -",
    ];
    let sb = Sandbox::atl_allowed_with_rules();
    let only = push_only_sandbox();
    for cmd in payloads {
        assert!(stopped(&sb, cmd), "{cmd}: {:?}", sb.hook(cmd));
        assert!(stopped(&only, cmd), "{cmd}: {:?}", only.hook(cmd));
    }
    // APFS folds case: `/DEV/stdin` is `/dev/stdin` there (on Linux it is no device).
    #[cfg(target_os = "macos")]
    assert!(stopped(&sb, "atl read /DEV/stdin"));
    for cmd in [
        "< secret.txt cat",
        "{ cat; } < secret.txt",
        "atl read ./sub/../secret.txt",
        "atl git log secret~1",
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
        "atl npm run build",
        "atl ls $HOME",
        "< secret.txt atl cat",
        "atl read /dev/stdin",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    assert_bypass_asks(&sb, &["atl bypass git push"]);
    assert_eq!(
        sb.hook("ls src/*.rs"),
        Said::Rewrite("atl ls src/*.rs".into())
    );
}

// --- review round 8: zsh EXTENDED_GLOB operators ---

/// R8-1: `^`, `#` and a later `~` are glob operators under zsh's EXTENDED_GLOB
/// (`c^1./.env` matches `config/.env`): only a strict revision word may hold them.
/// A commit message is no path, and single-quoted text is literal.
#[test]
fn the_round_eight_glob_operators_are_stopped() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "atl git log -p c^1./.env",
        "atl git show HEAD -- c^1./.env",
        "atl git log -p ~/project/config/.e^1",
        "atl git log -p ~/notes~1",
        "atl git log secret.txt~1",
        "atl git log a#b",
    ] {
        assert!(stopped(&sb, cmd), "{cmd}: {:?}", sb.hook(cmd));
    }
    for cmd in [
        "atl git log HEAD~5..HEAD",
        "atl git log main^..HEAD",
        "atl git commit -m secret.txt",
        "atl git commit -m 'costs $5'",
        "atl ls '$(id)'",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

// --- RG1: the shared parser (anthology_shell::analyze) ---

/// The shared parser only adds denies and asks. Its first draft deferred some
/// lines outside atl, which stopped the hook before atl's own rewrite was judged
/// and lost these asks; the old/new corpus run found it.
#[test]
fn the_shared_parser_never_loosens_a_verdict() {
    let sb = Sandbox::atl_allowed_with_rules();
    for cmd in [
        "pnpm exec sh -c 'x=push; git $x'",
        "npx -c 'x=push; git $x'",
        "pnpm exec git pu*h",
        "pnpm exec cat asked.txt",
    ] {
        assert!(
            matches!(sb.hook(cmd), Said::Ask(_)),
            "{cmd}: {:?}",
            sb.hook(cmd)
        );
    }
    // What it sees run is denied, in and out of atl.
    for cmd in [
        "bash <<EOF\ngit push\nEOF",
        "eval 'git push'",
        "atl bypass caffeinate git push",
        "bash <<EOF\ncat secret.txt\nEOF",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    // And the plain shape still passes.
    for cmd in [
        "atl git status",
        "atl ls",
        "atl read ok.txt",
        "atl git log -n 5",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

// --- P3: `cd <literal dir> && ...` and `... | head -n N` / `| tail -n N` ---

/// The command the hook hands back, when it rewrites.
fn rewrite_of(said: Said) -> Option<String> {
    match said {
        Said::Allow(command) | Said::Rewrite(command) => Some(command),
        _ => None,
    }
}

/// A sandbox with the plugin's allow, a deny, and `Read()` rules, and a link to a directory that holds a
/// denied file.
fn cd_sandbox() -> Sandbox {
    let sb = Sandbox::new(Rules {
        deny: &["git push:*"],
        allow: &["atl *"],
        read_deny: &["./secret.txt", "**/.env"],
        ..Rules::default()
    });
    #[cfg(unix)]
    std::os::unix::fs::symlink(sb.project.join("config"), sb.project.join("cfglink"))
        .expect("symlink to a directory");
    sb
}

/// With rules, a rewrite is kept for a literal `cd` in front and a counted `head` or `tail` behind.
#[test]
fn a_literal_cd_and_a_counted_head_keep_their_rewrite_under_rules() {
    let sb = cd_sandbox();
    let abs = sb.project.display().to_string();
    for cmd in [
        "cd ./sub && git status".to_string(),
        "cd ./sub && git log --oneline -5".to_string(),
        format!("cd {abs}/sub && git status"),
        "git log --oneline | head -n 5".to_string(),
        "git log --oneline | tail -n 20".to_string(),
        "git status | head -n5".to_string(),
        "cd ./sub && git log --oneline | head -n 5".to_string(),
        format!("cd {abs}/sub && git log --oneline | tail -n 3"),
    ] {
        let rewritten = rewrite_of(sb.hook(&cmd)).unwrap_or_else(|| panic!("{cmd}: no rewrite"));
        assert!(rewritten.contains("atl git"), "{cmd}: {rewritten}");
    }
    // typed with `atl` already: the plain shape passes with no prompt and nothing to rewrite
    for cmd in [
        "cd ./sub && atl git status".to_string(),
        "atl git log --oneline | head -n 5".to_string(),
        "cd ./sub && atl ls -la | tail -n 3".to_string(),
        format!("cd {abs}/sub && atl ls"),
    ] {
        assert_eq!(sb.hook(&cmd), Said::Nothing, "{cmd}");
    }
}

/// Relative reads resolve against the new directory before the deny check, through a link too.
#[test]
fn a_read_after_a_cd_still_meets_the_deny_rules() {
    let sb = cd_sandbox();
    let abs = sb.project.display().to_string();
    for cmd in [
        "cd ./config && atl read .env",
        "cd ./config && atl read .env | head -n 5",
        "cd ./sub && atl read ../secret.txt",
        "cd ./sub && atl read ../config/.env",
        "cd ./cfglink && atl read .env",
        "cd ./cfglink && atl read ../secret.txt",
        "cd ./config && cat .env",
        "cd ./config && cat .env | head -n 5",
        "cd ./cfglink && cat .env",
        "cd ./sub && cat ../secret.txt",
        "cd config && cat .env",
        "cd config && atl read .env",
        &format!("cd {abs}/config && atl read .env"),
        &format!("cd {abs}/config && cat .env"),
        &format!("cd {abs}/cfglink && cat .env"),
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
    // a path that is not the denied one is left alone
    for cmd in [
        "cd ./sub && atl read note.txt",
        "cd ./config && atl read ../ok.txt",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

/// A `cd` the gate cannot follow, a join other than `&&` and a count that is not literal are not made
/// plain: the original goes to the host unrewritten, or the hook asks when `atl` is typed.
#[test]
fn a_cd_or_a_count_the_gate_cannot_follow_is_not_rewritten() {
    let sb = cd_sandbox();
    // not rewritten: the host judges what was typed
    for cmd in [
        "cd sub && git status",
        "cd .. && git status",
        "cd ./.. && git status",
        "cd - && git status",
        "cd ~ && git status",
        "cd ~root && git status",
        "cd $X && git status",
        "cd \"$X\" && git status",
        "cd $(pwd) && git status",
        "cd ./nope && git status",
        "cd ./ok.txt && git status",
        "cd ./sub; git status",
        "cd ./sub || git status",
        "cd ./sub & git status",
        "cd ./sub && git status && git log",
        "cd ./sub && cd ./config && git status",
        "git log | head -n 5 | cat",
        "git log | head -n 5 | head -n 2",
        "git log | head -n 5; ls",
        "git log | head -n 5 && ls",
        "git log | head",
        "git log | head -n -5",
        "git log | head -c 5",
        "git log | tail -f",
        "git log | cat",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
    // a count that expands: the rule scan asks about the expanding operand of `head` (a read command)
    // whether or not atl is typed, and the hook hands the original back
    for cmd in [
        "git log | head -n $N",
        "git log | head -n \"$N\"",
        "git log | head -n \"$(echo 5)\"",
    ] {
        assert_eq!(sb.hook(cmd), Said::Ask(cmd.to_string()), "{cmd}");
    }
    // typed with `atl`: the hook asks
    asked(
        &sb,
        &[
            "cd sub && atl git status",
            "cd .. && atl git status",
            "cd - && atl git status",
            "cd $X && atl git status",
            "cd ./nope && atl git status",
            "cd ./sub; atl git status",
            "cd ./sub || atl git status",
            "cd ./sub && atl git status && atl git log",
            "cd ./sub && cd ./config && atl git status",
            "atl git log | head -n $N",
            "atl git log | head -n \"$(echo 5)\"",
            "atl git log | head -n 5 | cat",
            "atl git log | head -n 5; atl ls",
            "atl git log | head -n 5 && atl ls",
            "atl git log |& head -n 5",
            "atl git log || head -n 5",
            "atl git log | tail -f",
            "atl git log | tail -F",
            "atl git log | head -n 5 > out",
            "atl git log | tee out",
            "cd ./sub && atl npm test",
            "cd ./sub && atl bypass git status",
            "cd ./sub && atl git log | head -n 5 | head -n 2",
        ],
    );
}

/// A ruled command anywhere on such a line is still denied.
#[test]
fn a_ruled_command_after_a_cd_or_before_a_count_is_still_denied() {
    let sb = cd_sandbox();
    for cmd in [
        "cd ./sub && git push",
        "cd ./sub && atl git push",
        "cd ./sub && atl bypass git push",
        "git push | head -n 5",
        "atl git push | head -n 5",
        "git log | head -n 5; git push",
        "cd ./sub && git log | head -n 5 && git push",
        "cd /tmp && git push",
        // a file after the count is a read of that file, and the count is no longer literal
        "git log | head -n 5 secret.txt",
        "atl git log | head -n 5 secret.txt",
        "cd ./sub && git log | head -n 5 ../secret.txt",
    ] {
        assert_eq!(sb.hook(cmd), Said::Deny, "{cmd}");
    }
}

/// No rule anywhere: the gate is inert and nothing about these shapes changes.
#[test]
fn the_cd_and_count_shapes_are_inert_in_a_bare_sandbox() {
    let sb = Sandbox::bare();
    for cmd in [
        "cd ./sub && git status",
        "cd sub && git status",
        "git log | head -n 5",
    ] {
        let rewritten = rewrite_of(sb.hook(cmd)).unwrap_or_else(|| panic!("{cmd}: no rewrite"));
        assert!(rewritten.contains("atl git"), "{cmd}: {rewritten}");
    }
    for cmd in [
        "cd $X && atl git status",
        "cd - && atl git status",
        "atl git log | head -n $N",
        "atl git log | tail -f",
    ] {
        assert_eq!(sb.hook(cmd), Said::Nothing, "{cmd}");
    }
}

// --- needless asks on read-only commands ---
//
// With any rule in place, a line that merely contains `atl` (a path such as
// `atl/src/main.rs`) is not in the plain shape, so the hook asks, and a hook ask
// beats the `Bash(atl *)` allow in every mode. When the original line is shown
// read-only by `anthology perm classify` the ask is needless. The tests stand a
// script in for the classifier: ATL reaches it through `ATL_PERM_CLASSIFY`.

/// A stand-in for `anthology perm classify`. It answers like the real one: the
/// token line and then the absolute paths read (here: each absolute word of the
/// command, the `cd` target included), or exit 1 and no decision. It
/// refuses a request naming `rm`, `git push`, `kubectl` or `curl`, unless
/// `FAKE_PROVE_ALL` is set; `FAKE_LOG` names a file each request is appended to.
const FAKE_CLASSIFIER: &str = r#"#!/bin/sh
[ "$1" = perm ] && [ "$2" = classify ] || exit 2
input=$(cat)
[ -z "${FAKE_LOG:-}" ] || printf '%s\n' "$input" >> "$FAKE_LOG"
if [ -z "${FAKE_PROVE_ALL:-}" ]; then
  case $input in
    *' rm '*|*'git push'*|*kubectl*|*curl*)
      echo 'anthology perm: no decision: not read-only' >&2
      exit 1 ;;
  esac
fi
command=$(printf '%s' "$input" | sed -n 's/.*"command":"\([^"]*\)".*/\1/p')
printf 'anthology-perm:read-only:v1\n'
set -f
for word in $command; do
  case $word in /*) printf '%s\n' "$word" ;; esac
done
"#;

/// The stand-in, written once for the whole test binary (a script written while
/// another test thread forks can fail to start with ETXTBSY, so it is finished
/// before any test spawns atl with it).
fn fake_classifier() -> &'static str {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let path = common::scratch_dir().join("fake-perm-classify.sh");
        std::fs::write(&path, FAKE_CLASSIFIER).expect("write the fake classifier");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod the fake classifier");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        path.to_string_lossy().into_owned()
    })
}

/// A sandbox whose project holds `atl/notes.txt`, a path that mentions atl.
fn atl_named_sandbox(rules: Rules) -> Sandbox {
    let sb = Sandbox::new(rules);
    std::fs::create_dir_all(sb.project.join("atl")).expect("mkdir atl");
    std::fs::write(sb.project.join("atl/notes.txt"), "notes\n").expect("write notes");
    sb
}

/// What the hook says about `cmd` in permission mode `mode` (`None`: the payload
/// carries none), with the classifier on or off.
fn hook_in(
    sb: &Sandbox,
    cmd: &str,
    mode: Option<&str>,
    classifier: bool,
    env: &[(&str, &str)],
) -> Said {
    hook_at(sb, &sb.project, cmd, mode, classifier, env)
}

/// [`hook_in`] with the payload's working directory spelled as `cwd`.
fn hook_at(
    sb: &Sandbox,
    cwd: &std::path::Path,
    cmd: &str,
    mode: Option<&str>,
    classifier: bool,
    env: &[(&str, &str)],
) -> Said {
    let mut payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": cmd },
        "cwd": cwd.to_string_lossy(),
    });
    if let Some(mode) = mode {
        payload["permission_mode"] = serde_json::Value::String(mode.to_string());
    }
    let mut vars: Vec<(&str, &str)> = env.to_vec();
    if classifier {
        vars.push(("ATL_PERM_CLASSIFY", fake_classifier()));
    }
    let (code, stdout, stderr) = sb.spawn(&["hook", "claude"], Some(payload.to_string()), &vars);
    assert_eq!(
        code, 0,
        "atl hook claude exited {code} for {cmd:?}: {stderr}"
    );
    Sandbox::classify(cmd, &stdout)
}

/// Read-only reads of a path that mentions atl get the plain rewrite, with no
/// decision of the hook's own: the user's `Bash(atl *)` rule (or its absence, or
/// `allowManagedPermissionRulesOnly`) decides, as for a line that does not mention
/// atl. An explicit allow would beat settings that have no such rule.
#[test]
fn a_provably_read_only_original_gets_the_plain_rewrite_instead_of_an_ask() {
    let sb = atl_named_sandbox(Rules {
        deny: &["git push:*"],
        allow: &["atl *"],
        ..Rules::default()
    });
    for (cmd, rewritten) in [
        ("grep -n foo atl/notes.txt", "atl grep -n foo atl/notes.txt"),
        ("ls -la atl | head -60", "atl ls -la atl | head -60"),
    ] {
        assert_eq!(
            hook_in(&sb, cmd, None, true, &[]),
            Said::Rewrite(rewritten.into()),
            "{cmd}"
        );
        // The same mode the host reports for an ordinary session.
        assert_eq!(
            hook_in(&sb, cmd, Some("default"), true, &[]),
            Said::Rewrite(rewritten.into()),
            "{cmd} in default mode"
        );
    }
    // Without a `Bash(atl *)` rule: still no decision of the hook's own.
    let no_allow = atl_named_sandbox(Rules {
        deny: &["git push:*"],
        ..Rules::default()
    });
    assert_eq!(
        hook_in(&no_allow, "grep -n foo atl/notes.txt", None, true, &[]),
        Said::Rewrite("atl grep -n foo atl/notes.txt".into())
    );
    // A rewrite that is not in the plain shape is not made: the host judges the
    // original (the permission mod may then allow it).
    assert_eq!(
        hook_in(&sb, "grep -n \"foo\\|bar\" atl/notes.txt", None, true, &[]),
        Said::Nothing
    );
}

/// No classifier, or one that does not prove the line: the ask stays, as before.
#[test]
fn what_is_not_proven_read_only_still_asks() {
    let sb = atl_named_sandbox(Rules {
        deny: &["git push:*"],
        allow: &["atl *"],
        ..Rules::default()
    });
    // `ATL_PERM_CLASSIFY` unset: nothing proves anything.
    for cmd in ["grep -n foo atl/notes.txt", "ls -la atl | head -60"] {
        assert!(
            matches!(hook_in(&sb, cmd, None, false, &[]), Said::Ask(_)),
            "{cmd}"
        );
    }
    // The classifier refuses a line that writes.
    assert!(matches!(
        hook_in(&sb, "rm -f atl/notes.txt", None, true, &[]),
        Said::Ask(_)
    ));
    // A classifier that cannot be run proves nothing.
    assert!(matches!(
        hook_in(
            &sb,
            "grep -n foo atl/notes.txt",
            None,
            false,
            &[("ATL_PERM_CLASSIFY", "/nonexistent/anthology")]
        ),
        Said::Ask(_)
    ));
    assert!(matches!(
        hook_in(
            &sb,
            "grep -n foo atl/notes.txt",
            None,
            false,
            &[("ATL_PERM_CLASSIFY", "relative/anthology")]
        ),
        Said::Ask(_)
    ));
}

/// A proof never loosens a rule: a deny stays a deny, and a user's ask rule
/// stays an ask, even when the classifier proves every line read-only.
#[test]
fn a_proof_never_beats_a_deny_or_an_ask_rule() {
    let log = TempDir::new().expect("tempdir");
    let log_path = log.path().join("requests.log");
    let log_arg = log_path.to_string_lossy().into_owned();
    let env = [("FAKE_PROVE_ALL", "1"), ("FAKE_LOG", log_arg.as_str())];
    let sb = atl_named_sandbox(Rules {
        deny: &["git push:*"],
        ask: &["grep *"],
        allow: &["atl *"],
        read_deny: &["**/.env"],
        read_ask: &["./asked.txt"],
    });
    // The deny comes first: the classifier is never even asked.
    for cmd in [
        "git push origin atl",
        "cat config/.env",
        "ls atl && git push",
    ] {
        assert_eq!(hook_in(&sb, cmd, None, true, &env), Said::Deny, "{cmd}");
    }
    assert!(
        !log_path.exists(),
        "a denied command is not sent to the classifier"
    );
    // An ask rule: still asked.
    for cmd in ["grep -n foo atl/notes.txt", "ls atl | grep notes"] {
        assert!(
            matches!(hook_in(&sb, cmd, None, true, &env), Said::Ask(_)),
            "{cmd}"
        );
    }
    // A `Read()` ask rule: no rewrite, as before (an atl read of it asks).
    assert_eq!(
        hook_in(&sb, "cat asked.txt", None, true, &env),
        Said::Nothing
    );
    assert!(matches!(
        hook_in(&sb, "atl read asked.txt", None, true, &env),
        Said::Ask(_)
    ));
}

/// A line that runs atl is never read-only, whatever the classifier says: the
/// ask on a typed `atl ...` stays.
#[test]
fn a_typed_atl_command_is_asked_about_even_if_a_classifier_would_pass_it() {
    let sb = atl_named_sandbox(Rules {
        ask: &["kubectl apply:*"],
        allow: &["atl *"],
        ..Rules::default()
    });
    let env = [("FAKE_PROVE_ALL", "1")];
    for cmd in [
        "atl kubectl apply -f x.yaml",
        "atl run -c 'kubectl apply -f x'",
    ] {
        assert!(
            matches!(hook_in(&sb, cmd, None, true, &env), Said::Ask(_)),
            "{cmd}"
        );
    }
}

/// Without a rule the hook says what it said before, classifier or not.
#[test]
fn the_classifier_is_inert_in_a_bare_sandbox() {
    let sb = atl_named_sandbox(Rules::default());
    for cmd in ["grep -n foo atl/notes.txt", "ls -la atl | head -60"] {
        assert_eq!(
            hook_in(&sb, cmd, None, true, &[]),
            hook_in(&sb, cmd, None, false, &[]),
            "{cmd}"
        );
    }
}

/// Auto mode: the host's classifier judges what the model wrote, so a hook that
/// changes the input makes it refuse ("a hook changed this call's input after
/// auto mode reviewed it"). Nothing is rewritten; a deny is still a deny.
#[test]
fn in_auto_mode_nothing_is_rewritten() {
    let rules = || Rules {
        deny: &["git push:*", "curl:*"],
        ask: &["kubectl apply:*"],
        allow: &["atl *", "grep *"],
        read_deny: &["./secret.txt"],
        ..Rules::default()
    };
    let sb = atl_named_sandbox(rules());
    for cmd in [
        "git status",
        "git log --oneline -5",
        "ls -la sub | head -60",
        "cat ok.txt",
        "grep -n foo sub/note.txt",
        "grep -n foo atl/notes.txt",
        "ls -la atl | head -60",
    ] {
        // Today's answer, for contrast: a rewrite.
        assert!(
            matches!(
                hook_in(&sb, cmd, Some("default"), true, &[]),
                Said::Rewrite(_) | Said::Allow(_) | Said::Ask(_)
            ),
            "{cmd} outside auto mode"
        );
        assert_eq!(
            hook_in(&sb, cmd, Some("auto"), true, &[]),
            Said::Nothing,
            "{cmd} in auto mode"
        );
    }
    // The gate's deny comes first, in every mode.
    for cmd in ["git push origin main", "curl x", "cat secret.txt"] {
        assert_eq!(
            hook_in(&sb, cmd, Some("auto"), true, &[]),
            Said::Deny,
            "{cmd}"
        );
    }
    // A line the proof does not cover keeps its ask in auto mode too.
    assert!(matches!(
        hook_in(&sb, "kubectl apply -f x.yaml", Some("auto"), true, &[]),
        Said::Ask(_)
    ));
    assert!(matches!(
        hook_in(&sb, "rm -f atl/notes.txt", Some("auto"), true, &[]),
        Said::Ask(_)
    ));
    // No rule anywhere: still no rewrite in auto mode.
    let bare = Sandbox::bare();
    assert_eq!(
        hook_in(&bare, "git status", Some("auto"), false, &[]),
        Said::Nothing
    );
    assert_eq!(
        hook_in(&bare, "git status", Some("default"), false, &[]),
        Said::Rewrite("atl git status".into())
    );
}

/// Every other mode rewrites as before; bypass included (the shim drops an ask
/// there, and a plain rewrite or an allow is fine).
#[test]
fn the_other_modes_keep_their_rewrite() {
    let bare = Sandbox::bare();
    for mode in [
        None,
        Some("default"),
        Some("acceptEdits"),
        Some("plan"),
        Some("dontAsk"),
        Some("bypassPermissions"),
        // Not a mode Claude Code names: treated as no mode, never as auto.
        Some("Auto"),
        Some("autopilot"),
    ] {
        assert_eq!(
            hook_in(&bare, "git status", mode, false, &[]),
            Said::Rewrite("atl git status".into()),
            "{mode:?}"
        );
    }
    let allowed = Sandbox::new(Rules {
        allow: &["git status"],
        ..Rules::default()
    });
    assert_eq!(
        hook_in(
            &allowed,
            "git status",
            Some("bypassPermissions"),
            false,
            &[]
        ),
        Said::Allow("atl git status".into())
    );
    assert_eq!(
        hook_in(&allowed, "git status", Some("auto"), false, &[]),
        Said::Nothing
    );
}

/// A project under a home that has a `.claude` folder, and no `.claude` of its own,
/// with the rules in `~/.claude/settings.json`. The settings walk from the project
/// reaches the home's `.claude` first, so it is no marker of where the project ends.
fn home_marker_sandbox() -> Sandbox {
    let root = TempDir::new().expect("tempdir");
    let home = root.path().join("home");
    let claude_home = home.join(".claude");
    let project = home.join("proj");
    std::fs::create_dir_all(home.join(".config")).expect("mkdir config");
    std::fs::create_dir_all(&claude_home).expect("mkdir claude home");
    std::fs::create_dir_all(&project).expect("mkdir project");
    std::fs::create_dir_all(root.path().join("tee")).expect("mkdir tee");
    std::fs::write(
        claude_home.join("settings.json"),
        serde_json::json!({"permissions": {"deny": ["Bash(git push:*)"], "allow": ["Bash(atl *)"]}})
            .to_string(),
    )
    .expect("write settings");
    std::fs::write(home.join("secret.txt"), "s3cret\n").expect("write secret");
    std::fs::write(project.join("notes.txt"), "notes\n").expect("write notes");
    Sandbox {
        _root: root,
        home,
        claude_home,
        project,
    }
}

/// The root of the proof is the project (`CLAUDE_PROJECT_DIR`, else the working
/// directory's git top level, else the directory itself), never the first folder
/// up the tree that has a `.claude`: that is the home when the repository has none,
/// and the whole home is not the project.
#[test]
fn the_proof_root_is_the_project_not_the_first_claude_folder_up_the_tree() {
    let sb = home_marker_sandbox();
    let secret = sb.home.join("secret.txt");
    let outside = format!("grep -n atl {}", secret.display());
    let inside = format!("grep -n atl {}", sb.project.join("notes.txt").display());
    // Outside the project, inside the home: asked, as without the proof.
    assert!(
        matches!(hook_in(&sb, &outside, None, true, &[]), Said::Ask(_)),
        "{outside}"
    );
    // Inside it: the plain rewrite.
    assert_eq!(
        hook_in(&sb, "grep -n atl notes.txt", None, true, &[]),
        Said::Rewrite("atl grep -n atl notes.txt".into())
    );
    assert!(
        matches!(hook_in(&sb, &inside, None, true, &[]), Said::Rewrite(_)),
        "{inside}"
    );
    // `CLAUDE_PROJECT_DIR` names the root, and only it.
    let dir = sb.project.to_string_lossy().into_owned();
    let env = [("CLAUDE_PROJECT_DIR", dir.as_str())];
    assert!(
        matches!(hook_in(&sb, &outside, None, true, &env), Said::Ask(_)),
        "{outside} with CLAUDE_PROJECT_DIR"
    );
    assert!(
        matches!(hook_in(&sb, &inside, None, true, &env), Said::Rewrite(_)),
        "{inside} with CLAUDE_PROJECT_DIR"
    );
}

/// In auto mode an ask the proof does not cover is an ask about the line as typed:
/// carrying the rewrite would make Claude Code's classifier refuse the changed input.
#[test]
fn in_auto_mode_an_ask_that_stays_is_about_the_original_with_no_rewrite() {
    let sb = atl_named_sandbox(Rules {
        deny: &["git push:*"],
        allow: &["atl *"],
        ..Rules::default()
    });
    // The stand-in refuses a line naming curl, so nothing is proven.
    let cmd = "grep -n curl atl/notes.txt";
    assert_eq!(
        hook_in(&sb, cmd, Some("default"), true, &[]),
        Said::Ask("atl grep -n curl atl/notes.txt".into()),
        "outside auto mode the ask carries the rewrite, as before"
    );
    assert_eq!(
        hook_in(&sb, cmd, Some("auto"), true, &[]),
        Said::Ask(cmd.into())
    );
}

// --- `atl bypass` is judged as the command it runs ---
//
// The install allows `Bash(atl *)`, and `atl bypass <cmd>` (or its old name `atl
// proxy`) runs any command unfiltered, so that allow would approve everything. The
// hook judges the inner command instead: the user's deny and ask rules apply to it,
// and it is asked about unless `anthology perm classify` shows the line read-only.

/// `inner` behind every spelling of `atl bypass` the hook must see through, alone
/// and next to other commands.
fn bypass_shapes(inner: &str) -> Vec<String> {
    let mut shapes: Vec<String> = [
        "atl bypass",
        "atl proxy",
        "atl -v bypass",
        "/usr/local/bin/atl bypass",
        "env atl bypass",
        "FOO=1 atl bypass",
        "rtk bypass", // legacy
    ]
    .iter()
    .map(|prefix| format!("{prefix} {inner}"))
    .collect();
    shapes.extend([
        format!("ls && atl bypass {inner}"),
        format!("ls; atl bypass {inner}"),
        format!("ls | atl bypass {inner}"),
        format!("ls || atl bypass {inner}"),
        format!("atl bypass {inner} | head -3"),
        format!("atl bypass {inner} && ls"),
        format!("bash -c 'atl bypass {inner}'"),
    ]);
    shapes
}

/// A typed `atl bypass rm -rf x` is not covered by `Bash(atl *)`: the hook asks, in
/// every shape, with no deny, ask or `Read()` rule anywhere, with or without a
/// classifier, and whatever the stand-in would say about the inner command.
#[test]
fn atl_bypass_of_a_command_that_writes_asks_whatever_the_allow_rule_says() {
    let allowed = Sandbox::new(Rules {
        allow: &["atl *"],
        ..Rules::default()
    });
    let unallowed = Sandbox::bare();
    for sb in [&allowed, &unallowed] {
        for cmd in bypass_shapes("rm -rf x") {
            for classifier in [false, true] {
                for mode in [None, Some("default"), Some("acceptEdits")] {
                    assert!(
                        matches!(hook_in(sb, &cmd, mode, classifier, &[]), Said::Ask(_)),
                        "{cmd} ({mode:?}, classifier {classifier})"
                    );
                }
            }
        }
    }
    // Even a classifier that proves everything read-only proves nothing about a
    // line that still runs atl in a shape the hook does not read through.
    let env = [("FAKE_PROVE_ALL", "1")];
    for cmd in [
        "env atl bypass rm -rf x",
        "timeout 5 atl bypass rm -rf x",
        "atl bypass atl bypass rm -rf x",
        "atl bypass 'rm -rf x'",
    ] {
        assert!(
            matches!(hook_in(&allowed, cmd, None, true, &env), Said::Ask(_)),
            "{cmd}"
        );
    }
}

/// The spellings a shell reads as `atl bypass rm -rf x` too: quotes, escapes,
/// grammar, groups, substitutions, launchers and the payloads of other commands.
#[test]
fn atl_bypass_is_found_behind_quotes_grammar_groups_and_launchers() {
    let sb = Sandbox::new(Rules {
        allow: &["atl *"],
        ..Rules::default()
    });
    for cmd in [
        "'atl' bypass rm -rf x",
        "atl 'bypass' rm -rf x",
        "atl \"proxy\" rm -rf x",
        "\\atl bypass rm -rf x",
        "a\\tl bypass rm -rf x",
        "(atl bypass rm -rf x)",
        "{ atl bypass rm -rf x; }",
        "echo $(atl bypass rm -rf x)",
        "echo `atl bypass rm -rf x`",
        "atl bypass rm -rf x &",
        "ls\natl bypass rm -rf x",
        "if true; then atl bypass rm -rf x; fi",
        "for i in 1 2; do atl bypass rm -rf $i; done",
        "time atl bypass rm -rf x",
        "! atl bypass rm -rf x",
        "nice atl bypass rm -rf x",
        "sudo atl bypass rm -rf x",
        "xargs atl bypass rm -rf",
        "find . -name x -exec atl bypass rm -rf {} \\;",
        "eval 'atl bypass rm -rf x'",
        "sh -c \"atl bypass rm -rf x\"",
        "atl run -c 'atl bypass rm -rf x'",
        "atl err atl bypass rm -rf x",
        "atl bypass atl bypass rm -rf x",
        "cd sub && atl bypass rm -rf x",
        "ls > /dev/null && atl bypass rm -rf x",
    ] {
        assert!(
            matches!(hook_in(&sb, cmd, None, true, &[]), Said::Ask(_)),
            "{cmd:?}"
        );
    }
}

/// A read-only inner command goes through with no ask, as a read-only line does
/// (no rewrite and no decision of the hook's own: the host decides).
#[test]
fn atl_bypass_of_a_provably_read_only_command_is_not_asked_about() {
    for rules in [
        Rules {
            allow: &["atl *"],
            ..Rules::default()
        },
        // With a rule in place every `atl` line has to be in the plain shape.
        Rules {
            deny: &["git push:*"],
            allow: &["atl *"],
            ..Rules::default()
        },
    ] {
        let sb = Sandbox::new(rules);
        for cmd in [
            "atl bypass git log -5",
            "atl proxy git log -5",
            "atl -v bypass git log -5",
            "ls && atl bypass git log -5",
            "ls; atl bypass git log -5",
            "ls | atl bypass git log -5",
            "atl bypass git log -5 | head -3",
            "atl bypass git status; atl bypass git log -5",
        ] {
            for mode in [None, Some("default"), Some("auto")] {
                assert_eq!(
                    hook_in(&sb, cmd, mode, true, &[]),
                    Said::Nothing,
                    "{cmd} ({mode:?})"
                );
            }
        }
        // One part that is not read-only keeps the ask.
        for cmd in [
            "atl bypass git log -5 && atl bypass rm -rf x",
            "ls | atl bypass tee x; atl bypass rm x",
            "atl bypass git log -5 && rm -rf x",
        ] {
            assert!(
                matches!(hook_in(&sb, cmd, None, true, &[]), Said::Ask(_)),
                "{cmd}"
            );
        }
        // No classifier (an install without the plugin) proves nothing: it asks.
        assert!(matches!(
            hook_in(&sb, "atl bypass git log -5", None, false, &[]),
            Said::Ask(_)
        ));
    }
}

/// The user's deny rule on the inner command denies, in every shape, and their ask
/// rule asks even when the classifier proves the line read-only.
#[test]
fn atl_bypass_meets_the_deny_and_ask_rules_of_the_inner_command() {
    let sb = Sandbox::new(Rules {
        deny: &["rm:*", "git push:*"],
        ask: &["kubectl apply:*"],
        allow: &["atl *"],
        ..Rules::default()
    });
    let env = [("FAKE_PROVE_ALL", "1")];
    for inner in ["rm -rf x", "git push origin main"] {
        for cmd in bypass_shapes(inner) {
            assert_eq!(hook_in(&sb, &cmd, None, true, &env), Said::Deny, "{cmd}");
            assert_eq!(
                hook_in(&sb, &cmd, Some("auto"), false, &env),
                Said::Deny,
                "{cmd} in auto mode"
            );
        }
    }
    for cmd in bypass_shapes("kubectl apply -f x.yaml") {
        assert!(
            matches!(hook_in(&sb, &cmd, None, true, &env), Said::Ask(_)),
            "{cmd}"
        );
    }
    // A read-only inner command is judged by the same rules: no rule, no ask.
    assert_eq!(
        hook_in(&sb, "atl bypass git log -5", None, true, &[]),
        Said::Nothing
    );
}

/// Auto mode keeps the earlier rule: nothing is rewritten, so the ask is about the
/// line as typed.
#[test]
fn atl_bypass_in_auto_mode_asks_about_the_line_as_typed() {
    for rules in [
        Rules {
            allow: &["atl *"],
            ..Rules::default()
        },
        Rules {
            deny: &["git push:*"],
            allow: &["atl *"],
            ..Rules::default()
        },
    ] {
        let sb = Sandbox::new(rules);
        for cmd in [
            "atl bypass rm -rf x",
            "atl proxy rm -rf x",
            "ls && atl bypass rm -rf x",
            "git status && atl bypass rm -rf x",
            "env atl bypass rm -rf x",
        ] {
            assert_eq!(
                hook_in(&sb, cmd, Some("auto"), true, &[]),
                Said::Ask(cmd.into()),
                "{cmd}"
            );
        }
    }
}

/// A line that only mentions the words, or runs another atl subcommand, is not
/// asked about on account of `bypass`.
#[test]
fn what_is_not_atl_bypass_is_not_asked_about_as_one() {
    let sb = Sandbox::new(Rules {
        allow: &["atl *"],
        ..Rules::default()
    });
    for cmd in [
        "atl git status",
        "atl llm-proxy status",
        "echo atl bypass rm -rf x",
        "git commit -m 'atl bypass rm'",
        "grep -rn 'atl bypass' docs",
        "ls bypass",
    ] {
        assert!(
            !matches!(hook_in(&sb, cmd, None, true, &[]), Said::Ask(_)),
            "{cmd}"
        );
    }
}

/// A project reached through a symlink (macOS: `/var` and `/tmp` are links into
/// `/private`) is still the project: the paths a proof lists and the root are
/// compared as the file system resolves them, not as they are spelled.
#[cfg(unix)]
#[test]
fn a_project_reached_through_a_symlink_is_still_the_project() {
    let sb = atl_named_sandbox(Rules {
        deny: &["git push:*"],
        allow: &["atl *"],
        ..Rules::default()
    });
    let link = sb._root.path().join("via-link");
    std::os::unix::fs::symlink(&sb.project, &link).expect("symlink the project");
    assert_eq!(
        hook_at(&sb, &link, "grep -n foo atl/notes.txt", None, true, &[]),
        Said::Rewrite("atl grep -n foo atl/notes.txt".into())
    );
    // The same with the root named by `CLAUDE_PROJECT_DIR` through the link.
    let dir = link.to_string_lossy().into_owned();
    assert_eq!(
        hook_at(
            &sb,
            &sb.project,
            "grep -n foo atl/notes.txt",
            None,
            true,
            &[("CLAUDE_PROJECT_DIR", dir.as_str())]
        ),
        Said::Rewrite("atl grep -n foo atl/notes.txt".into())
    );
}

/// A path the proof lists that is spelled inside the project but resolves outside
/// it (a link in the project to somewhere else) is not under the project: the ask
/// stays, whatever the classifier printed.
#[cfg(unix)]
#[test]
fn a_listed_path_that_resolves_outside_the_project_keeps_the_ask() {
    let sb = atl_named_sandbox(Rules {
        deny: &["git push:*"],
        allow: &["atl *"],
        ..Rules::default()
    });
    let outside = sb._root.path().join("outside");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    std::fs::write(outside.join("secret.txt"), "s3cret\n").expect("write secret");
    std::os::unix::fs::symlink(&outside, sb.project.join("escape")).expect("symlink out");
    // The stand-in prints each absolute word as written, as a classifier that
    // did not resolve links would.
    let cmd = format!("grep -n atl {}/escape/secret.txt", sb.project.display());
    assert!(
        matches!(hook_in(&sb, &cmd, None, true, &[]), Said::Ask(_)),
        "{cmd}"
    );
    // Control: the same file inside the project is fine.
    let inside = format!("grep -n atl {}/atl/notes.txt", sb.project.display());
    assert!(
        matches!(hook_in(&sb, &inside, None, true, &[]), Said::Rewrite(_)),
        "{inside}"
    );
}
