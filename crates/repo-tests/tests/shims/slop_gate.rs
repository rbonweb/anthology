//! `hooks/slop-gate.sh`, the PreToolUse hook on Write, Edit and MultiEdit: it hands the hook input
//! to `anthology slop hook` and turns the binary's exit 10 into a block (exit 2, the binary's
//! stderr). Every other outcome allows, with no output: the gate fails open, so a missing, old or
//! crashing binary never stops an edit. The script never installs the binary. With
//! `ANTHOLOGY_SLOP_GATE=0` it exits 0 without running the binary.
//!
//! The binary here is a fake: a `#!/bin/sh` script that answers `--version` like the pinned
//! release and runs the case's body for `slop ...`.

use anthology_testkit::bs::Sandbox;

use super::common::*;

const SLOP_GATE: &str = "plugins/anthology/hooks/slop-gate.sh";

/// What Claude Code sends for a Write; the fake binary never parses it.
const WRITE: &[u8] = br#"{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"file_path":"/r/docs/a.md","content":"x"}}"#;

/// A fake binary for [`V`] whose `slop ...` runs `body`; everything else exits 0.
fn fake_slop(body: &str) -> Vec<u8> {
    format!(
        "#!/bin/sh\ncase ${{1:-}} in\n--version) printf '%s\\n' '{}'; exit 0 ;;\nslop) {body} ;;\nesac\nexit 0\n",
        version_line(V)
    )
    .into_bytes()
}

/// A sandbox pinned to a release of [`fake_slop`], with that binary installed.
fn installed(body: &str) -> Sandbox {
    let bytes = fake_slop(body);
    let sb = Sandbox::new(release_of(V, &bytes));
    sb.install(&bytes, 0o755);
    sb
}

/// The body of a binary that refuses the edit with `msg` (a shell `printf` format) on stderr.
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
    each_shell("slop gate, no binary", |sh| {
        let sb = sandbox();
        let run = sb.run(sh, SLOP_GATE, &[], WRITE);
        assert_allowed(&run, &format!("{sh:?}"));
        assert_not_installed(&sb, &format!("{sh:?}"));
    });
}

/// The gate only runs an installed binary: with the release archive in place as the offline
/// seed, session start would install it, and this hook must not.
#[test]
fn it_never_installs_the_binary() {
    each_shell("slop gate, seed", |sh| {
        let sb = sandbox();
        sb.seed();
        let c = format!("{sh:?}");
        assert_allowed(&sb.run(sh, SLOP_GATE, &[], WRITE), &c);
        assert_not_installed(&sb, &c);
        assert!(!sb.bin().exists(), "{c}: the binary was installed");
    });
}

#[test]
fn binary_exit_10_blocks_with_its_stderr() {
    each_shell("slop gate, block", |sh| {
        let sb = installed(&blocks_with("anthology slop gate: x\\n"));
        let run = sb.run(sh, SLOP_GATE, &[], WRITE);
        assert_refused(&run, "anthology slop gate: x\n", &format!("{sh:?}"));
    });
}

/// The real message has several lines; trailing blank lines are cut to one newline.
#[test]
fn the_message_keeps_its_lines_and_ends_with_one_newline() {
    each_shell("slop gate, multi-line message", |sh| {
        let sb = installed(&blocks_with("first line\\n  line 3: a\\n\\nlast\\n\\n\\n"));
        let run = sb.run(sh, SLOP_GATE, &[], WRITE);
        assert_refused(
            &run,
            "first line\n  line 3: a\n\nlast\n",
            &format!("{sh:?}"),
        );
    });
}

#[test]
fn binary_exit_2_allows() {
    each_shell("slop gate, old binary", |sh| {
        // An old binary answers `slop` with a usage error and exit 2.
        let sb =
            installed("cat >/dev/null; printf 'anthology: unknown command slop\\n' >&2; exit 2");
        assert_allowed(&sb.run(sh, SLOP_GATE, &[], WRITE), &format!("{sh:?}"));
    });
}

#[test]
fn binary_crash_allows() {
    each_shell("slop gate, crash", |sh| {
        let sb = installed("cat >/dev/null; printf 'thread panicked\\n' >&2; exit 101");
        assert_allowed(&sb.run(sh, SLOP_GATE, &[], WRITE), &format!("{sh:?}"));
    });
}

/// Only 10 blocks: its neighbors and a signal's code (128 + 15) allow.
#[test]
fn only_exit_10_blocks() {
    each_shell("slop gate, other codes", |sh| {
        for code in [1, 3, 9, 11, 70, 126, 127, 143, 255] {
            let sb = installed(&format!(
                "cat >/dev/null; printf 'anthology slop gate: x\\n' >&2; exit {code}"
            ));
            assert_allowed(
                &sb.run(sh, SLOP_GATE, &[], WRITE),
                &format!("{sh:?} exit {code}"),
            );
        }
    });
}

/// `ANTHOLOGY_SLOP_GATE=0` switches the gate off in the script itself: exit 0, no output, and the
/// binary is never started, though the one installed would refuse.
#[test]
fn the_switch_set_to_0_allows_without_running_the_binary() {
    each_shell("slop gate, switch off", |sh| {
        let mut sb = installed(&format!(
            ": >\"$BS_RAN\"; {}",
            blocks_with("anthology slop gate: x\\n")
        ));
        let ran = sb.root.join("ran");
        sb.set_env("BS_RAN", ran.clone());
        sb.set_env("ANTHOLOGY_SLOP_GATE", "0");
        let c = format!("{sh:?}");
        assert_allowed(&sb.run(sh, SLOP_GATE, &[], WRITE), &c);
        assert!(!ran.exists(), "{c}: the binary was run");
    });
}

/// Only exactly `0` switches it off: an empty value, `1`, `00`, a word and `0` with a space
/// leave the gate on.
#[test]
fn any_other_switch_value_still_blocks() {
    each_shell("slop gate, switch not 0", |sh| {
        for value in ["", "1", "00", "off", "false", "0 ", " 0"] {
            let mut sb = installed(&blocks_with("anthology slop gate: x\\n"));
            sb.set_env("ANTHOLOGY_SLOP_GATE", value);
            assert_refused(
                &sb.run(sh, SLOP_GATE, &[], WRITE),
                "anthology slop gate: x\n",
                &format!("{sh:?} value {value:?}"),
            );
        }
    });
}

#[test]
fn stdin_reaches_the_binary() {
    each_shell("slop gate, stdin", |sh| {
        let sb = installed(
            "in=$(cat); if [ \"$in\" = MARK ]; then printf 'seen\\n' >&2; exit 10; fi; exit 0",
        );
        let c = format!("{sh:?}");
        assert_refused(&sb.run(sh, SLOP_GATE, &[], b"MARK"), "seen\n", &c);
        assert_allowed(&sb.run(sh, SLOP_GATE, &[], b"other"), &c);
    });
}

#[test]
fn nothing_goes_to_stdout() {
    each_shell("slop gate, stdout", |sh| {
        // The binary's stdout is dropped on an allow and on a block alike.
        let allow = installed("cat >/dev/null; printf 'noise\\n'; printf 'err\\n' >&2; exit 0");
        assert_allowed(
            &allow.run(sh, SLOP_GATE, &[], WRITE),
            &format!("{sh:?} allow"),
        );
        let block = installed("cat >/dev/null; printf 'noise\\n'; printf 'why\\n' >&2; exit 10");
        assert_refused(
            &block.run(sh, SLOP_GATE, &[], WRITE),
            "why\n",
            &format!("{sh:?} block"),
        );
    });
}

/// A hook that cannot load its library, or find a usable lock, must not stop an edit even when
/// a binary that would refuse is installed.
#[test]
fn a_library_or_lock_that_is_unusable_allows() {
    each_shell("slop gate, broken library", |sh| {
        let refusing = blocks_with("anthology slop gate: x\\n");
        let c = format!("{sh:?}");

        let sb = installed(&refusing);
        std::fs::remove_file(sb.root.join(A_LIB)).unwrap();
        assert_allowed(
            &sb.run(sh, SLOP_GATE, &[], WRITE),
            &format!("{c} no lib.sh"),
        );

        let sb = installed(&refusing);
        patch_lib(&sb, "if then fi (");
        assert_allowed(
            &sb.run(sh, SLOP_GATE, &[], WRITE),
            &format!("{c} lib.sh syntax error"),
        );

        let sb = installed(&refusing);
        sb.write_lock("not a lock\n");
        assert_allowed(&sb.run(sh, SLOP_GATE, &[], WRITE), &format!("{c} bad lock"));

        let sb = installed(&refusing);
        std::fs::remove_file(sb.plugin("anthology").join("anthology.lock")).unwrap();
        assert_allowed(&sb.run(sh, SLOP_GATE, &[], WRITE), &format!("{c} no lock"));
    });
}

/// A binary that is not an executable regular file is not run (anth_installed).
#[test]
fn a_binary_that_is_not_executable_allows() {
    each_shell("slop gate, mode 0644", |sh| {
        let bytes = fake_slop(&blocks_with("anthology slop gate: x\\n"));
        let sb = Sandbox::new(release_of(V, &bytes));
        sb.install(&bytes, 0o644);
        assert_allowed(&sb.run(sh, SLOP_GATE, &[], WRITE), &format!("{sh:?}"));
    });
}

#[test]
fn the_script_is_executable_posix_sh() {
    use std::os::unix::fs::PermissionsExt;
    let path = anthology_testkit::staged(SLOP_GATE);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("#!/bin/sh\n"), "#!/bin/sh");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o755, "mode");
}
