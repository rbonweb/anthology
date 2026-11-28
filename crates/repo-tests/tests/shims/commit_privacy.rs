//! `hooks/commit-privacy.sh`, the PreToolUse hook on Bash that checks every `git commit` for the
//! owner's private details: for a command that mentions `git` and `commit` it hands the hook input
//! to `anthology privacy hook`, and turns the binary's exit 10 into a block (exit 2, the binary's
//! stderr). Every other outcome allows, with no output: the check fails open, so a missing, old or
//! crashing binary never stops a command. The script never installs the binary, and a command
//! that is no commit never starts it. With `ANTHOLOGY_COMMIT_PRIVACY=0` it exits 0 first.
//!
//! The binary here is a fake: a `#!/bin/sh` script that answers `--version` like the pinned
//! release and runs the case's body for `privacy ...`.

use anthology_testkit::bs::Sandbox;

use super::common::*;

const HOOK: &str = "plugins/anthology/hooks/commit-privacy.sh";

/// What Claude Code sends for a commit; the fake binary never parses it.
const COMMIT: &[u8] = br#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"git commit -m \"fix\""}}"#;

/// A fake binary for [`V`] whose `privacy ...` runs `body`; everything else exits 0.
fn fake_privacy(body: &str) -> Vec<u8> {
    format!(
        "#!/bin/sh\ncase ${{1:-}} in\n--version) printf '%s\\n' '{}'; exit 0 ;;\nprivacy) {body} ;;\nesac\nexit 0\n",
        version_line(V)
    )
    .into_bytes()
}

/// A sandbox pinned to a release of [`fake_privacy`], with that binary installed.
fn installed(body: &str) -> Sandbox {
    let bytes = fake_privacy(body);
    let sb = Sandbox::new(release_of(V, &bytes));
    sb.install(&bytes, 0o755);
    sb
}

/// The body of a binary that refuses the commit with `msg` (a shell `printf` format) on stderr.
fn blocks_with(msg: &str) -> String {
    format!("cat >/dev/null; printf '{msg}' >&2; exit 10")
}

/// Exit 2, empty stdout, and stderr exactly `want`.
#[track_caller]
fn assert_refused(run: &anthology_testkit::bs::Run, want: &str, ctx: &str) {
    assert!(!run.timed_out, "{ctx}: timed out: {run:?}");
    assert_eq!(run.err(), want, "{ctx}: stderr");
    assert_eq!(run.code, Some(2), "{ctx}: exit code");
    assert!(run.stdout.is_empty(), "{ctx}: stdout {:?}", run.out());
}

#[test]
fn missing_binary_allows() {
    each_shell("commit privacy, no binary", |sh| {
        let sb = sandbox();
        let run = sb.run(sh, HOOK, &[], COMMIT);
        assert_allowed(&run, &format!("{sh:?}"));
        assert_not_installed(&sb, &format!("{sh:?}"));
    });
}

/// The hook only runs an installed binary: with the release archive in place as the offline
/// seed, session start would install it, and this hook must not.
#[test]
fn it_never_installs_the_binary() {
    each_shell("commit privacy, seed", |sh| {
        let sb = sandbox();
        sb.seed();
        let c = format!("{sh:?}");
        assert_allowed(&sb.run(sh, HOOK, &[], COMMIT), &c);
        assert_not_installed(&sb, &c);
        assert!(!sb.bin().exists(), "{c}: the binary was installed");
    });
}

#[test]
fn binary_exit_10_blocks_with_its_stderr() {
    each_shell("commit privacy, block", |sh| {
        let sb = installed(&blocks_with("Blocked by anthology (privacy): x\\n"));
        let run = sb.run(sh, HOOK, &[], COMMIT);
        assert_refused(
            &run,
            "Blocked by anthology (privacy): x\n",
            &format!("{sh:?}"),
        );
    });
}

/// The real message is one line, but blank lines after it are cut to one newline.
#[test]
fn the_message_ends_with_one_newline() {
    each_shell("commit privacy, trailing newlines", |sh| {
        let sb = installed(&blocks_with("first\\nsecond\\n\\n\\n"));
        let run = sb.run(sh, HOOK, &[], COMMIT);
        assert_refused(&run, "first\nsecond\n", &format!("{sh:?}"));
    });
}

#[test]
fn binary_exit_2_allows() {
    each_shell("commit privacy, old binary", |sh| {
        // An old binary answers `privacy` with a usage error and exit 2.
        let sb =
            installed("cat >/dev/null; printf 'anthology: unknown command privacy\\n' >&2; exit 2");
        assert_allowed(&sb.run(sh, HOOK, &[], COMMIT), &format!("{sh:?}"));
    });
}

#[test]
fn binary_crash_allows() {
    each_shell("commit privacy, crash", |sh| {
        let sb = installed("cat >/dev/null; printf 'thread panicked\\n' >&2; exit 101");
        assert_allowed(&sb.run(sh, HOOK, &[], COMMIT), &format!("{sh:?}"));
    });
}

/// Only 10 blocks: its neighbours and a signal's code (128 + 15) allow.
#[test]
fn only_exit_10_blocks() {
    each_shell("commit privacy, other codes", |sh| {
        for code in [1, 3, 9, 11, 70, 126, 127, 143, 255] {
            let sb = installed(&format!(
                "cat >/dev/null; printf 'Blocked by anthology (privacy): x\\n' >&2; exit {code}"
            ));
            assert_allowed(
                &sb.run(sh, HOOK, &[], COMMIT),
                &format!("{sh:?} exit {code}"),
            );
        }
    });
}

/// `ANTHOLOGY_COMMIT_PRIVACY=0` switches the check off in the script itself: exit 0, no output,
/// and the binary is never started, though the one installed would refuse.
#[test]
fn the_switch_set_to_0_allows_without_running_the_binary() {
    each_shell("commit privacy, switch off", |sh| {
        let mut sb = installed(&format!(
            ": >\"$BS_RAN\"; {}",
            blocks_with("Blocked by anthology (privacy): x\\n")
        ));
        let ran = sb.root.join("ran");
        sb.set_env("BS_RAN", ran.clone());
        sb.set_env("ANTHOLOGY_COMMIT_PRIVACY", "0");
        let c = format!("{sh:?}");
        assert_allowed(&sb.run(sh, HOOK, &[], COMMIT), &c);
        assert!(!ran.exists(), "{c}: the binary was run");
    });
}

/// Only exactly `0` switches it off.
#[test]
fn any_other_switch_value_still_blocks() {
    each_shell("commit privacy, switch not 0", |sh| {
        for value in ["", "1", "00", "off", "false", "0 ", " 0"] {
            let mut sb = installed(&blocks_with("Blocked by anthology (privacy): x\\n"));
            sb.set_env("ANTHOLOGY_COMMIT_PRIVACY", value);
            assert_refused(
                &sb.run(sh, HOOK, &[], COMMIT),
                "Blocked by anthology (privacy): x\n",
                &format!("{sh:?} value {value:?}"),
            );
        }
    });
}

/// A command that is no commit never starts the binary: the input must hold both `git` and
/// `commit`, in that order. The fake binary would refuse, and leaves a marker when it starts.
#[test]
fn a_command_that_is_no_commit_does_not_start_the_binary() {
    each_shell("commit privacy, other commands", |sh| {
        let other: [&[u8]; 5] = [
            BASH,
            br#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#,
            br#"{"tool_name":"Bash","tool_input":{"command":"echo commit"}}"#,
            br#"{"tool_name":"Bash","tool_input":{"command":"commit git"}}"#,
            br#"{"tool_name":"Read","tool_input":{"file_path":"/r/a"}}"#,
        ];
        for input in other {
            let mut sb = installed(&format!(
                ": >\"$BS_RAN\"; {}",
                blocks_with("Blocked by anthology (privacy): x\\n")
            ));
            let ran = sb.root.join("ran");
            sb.set_env("BS_RAN", ran.clone());
            let c = format!("{sh:?} {}", String::from_utf8_lossy(input));
            assert_allowed(&sb.run(sh, HOOK, &[], input), &c);
            assert!(!ran.exists(), "{c}: the binary was run");
        }
    });
}

/// Other spellings that mention both words reach the binary: `git -C dir commit`, a chain.
#[test]
fn every_command_that_mentions_git_and_commit_reaches_the_binary() {
    each_shell("commit privacy, spellings", |sh| {
        let sb = installed(&blocks_with("Blocked by anthology (privacy): x\\n"));
        for command in [
            r#"git -C d commit -am x"#,
            r#"cd d && git commit -F m.txt"#,
            r#"GIT_AUTHOR_NAME=x git commit"#,
            r#"/usr/bin/git commit"#,
        ] {
            let input = format!(r#"{{"tool_name":"Bash","tool_input":{{"command":"{command}"}}}}"#);
            assert_refused(
                &sb.run(sh, HOOK, &[], input.as_bytes()),
                "Blocked by anthology (privacy): x\n",
                &format!("{sh:?} {command}"),
            );
        }
    });
}

#[test]
fn stdin_reaches_the_binary_whole() {
    each_shell("commit privacy, stdin", |sh| {
        let sb = installed(
            "in=$(cat); case $in in *'commit -m seen'*) printf 'seen\\n' >&2; exit 10 ;; esac; exit 0",
        );
        let c = format!("{sh:?}");
        let seen = br#"{"tool_name":"Bash","tool_input":{"command":"git commit -m seen"}}"#;
        let other = br#"{"tool_name":"Bash","tool_input":{"command":"git commit -m other"}}"#;
        assert_refused(&sb.run(sh, HOOK, &[], seen), "seen\n", &c);
        assert_allowed(&sb.run(sh, HOOK, &[], other), &c);
        // a message of many lines (the shell reads the input into a variable)
        let many = br#"{"tool_name":"Bash","tool_input":{"command":"git commit -m \"a\n\nb\n\nc\" -m seen"}}"#;
        assert_allowed(&sb.run(sh, HOOK, &[], many), &c);
    });
}

#[test]
fn nothing_goes_to_stdout() {
    each_shell("commit privacy, stdout", |sh| {
        // The binary's stdout is dropped on an allow and on a block alike.
        let allow = installed("cat >/dev/null; printf 'noise\\n'; printf 'err\\n' >&2; exit 0");
        assert_allowed(&allow.run(sh, HOOK, &[], COMMIT), &format!("{sh:?} allow"));
        let block = installed("cat >/dev/null; printf 'noise\\n'; printf 'why\\n' >&2; exit 10");
        assert_refused(
            &block.run(sh, HOOK, &[], COMMIT),
            "why\n",
            &format!("{sh:?} block"),
        );
    });
}

/// A hook that cannot load its library, or find a usable lock, must not stop a commit even when
/// a binary that would refuse is installed.
#[test]
fn a_library_or_lock_that_is_unusable_allows() {
    each_shell("commit privacy, broken library", |sh| {
        let refusing = blocks_with("Blocked by anthology (privacy): x\\n");
        let c = format!("{sh:?}");

        let sb = installed(&refusing);
        std::fs::remove_file(sb.root.join(A_LIB)).unwrap();
        assert_allowed(&sb.run(sh, HOOK, &[], COMMIT), &format!("{c} no lib.sh"));

        let sb = installed(&refusing);
        patch_lib(&sb, "if then fi (");
        assert_allowed(
            &sb.run(sh, HOOK, &[], COMMIT),
            &format!("{c} lib.sh syntax error"),
        );

        let sb = installed(&refusing);
        sb.write_lock("not a lock\n");
        assert_allowed(&sb.run(sh, HOOK, &[], COMMIT), &format!("{c} bad lock"));

        let sb = installed(&refusing);
        std::fs::remove_file(sb.plugin("anthology").join("anthology.lock")).unwrap();
        assert_allowed(&sb.run(sh, HOOK, &[], COMMIT), &format!("{c} no lock"));
    });
}

/// A binary that is not an executable regular file is not run (anth_installed).
#[test]
fn a_binary_that_is_not_executable_allows() {
    each_shell("commit privacy, mode 0644", |sh| {
        let bytes = fake_privacy(&blocks_with("Blocked by anthology (privacy): x\\n"));
        let sb = Sandbox::new(release_of(V, &bytes));
        sb.install(&bytes, 0o644);
        assert_allowed(&sb.run(sh, HOOK, &[], COMMIT), &format!("{sh:?}"));
    });
}

#[test]
fn the_script_is_executable_posix_sh() {
    use std::os::unix::fs::PermissionsExt;
    let path = anthology_testkit::staged(HOOK);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("#!/bin/sh\n"), "#!/bin/sh");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "mode");
}
