//! A slow link (field report): a release download that takes longer than one hook call's budget
//! must still install, and a missing binary must never leave the session without a way out.
//!
//! - The guardrails' guard and session modes download in the background and resume a stopped
//!   download from its part, so a 30 s download installs where every 20 s attempt used to start
//!   from zero and fail.
//! - The block says why (still downloading, timed out, no network, sha256 mismatch) and names
//!   the installer command, and the guard lets exactly that command through while the binary is
//!   missing; every widened form stays blocked.
//!
//! The downloader is a `curl` stub on PATH that serves the release from disk, slowly.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anthology_testkit::bs::{self, Run, Sandbox, Shell};

use super::common::*;

/// A curl that serves `$BS_ARCHIVE` in `$BS_CHUNK`-byte pieces, `$BS_DELAY` seconds apart. It
/// honours `-o` and `-C -` (it continues the file it finds) and logs where each run started.
const SLOW_CURL: &str = r#"#!/bin/sh
out= resume=
while [ "$#" -gt 0 ]; do
    case $1 in
        -o) out=$2; shift ;;
        -C) resume=$2; shift ;;
        --proto | --proto-redir | --connect-timeout | --max-time | --max-filesize | -A) shift ;;
    esac
    shift
done
size=$(wc -c <"$BS_ARCHIVE" | tr -d ' ')
from=0
if [ "$resume" = - ] && [ -f "$out" ]; then
    from=$(wc -c <"$out" | tr -d ' ')
else
    : >"$out"
fi
i=$((from / BS_CHUNK))
printf 'from=%s\n' "$((i * BS_CHUNK))" >>"$BS_DL_LOG"
while [ "$((i * BS_CHUNK))" -lt "$size" ]; do
    sleep "$BS_DELAY"
    dd if="$BS_ARCHIVE" of="$out" bs="$BS_CHUNK" skip="$i" seek="$i" count=1 conv=notrunc 2>/dev/null
    i=$((i + 1))
done
exit 0
"#;

/// A curl that cannot connect (curl's exit 7).
const OFFLINE_CURL: &str = "#!/bin/sh\nexit 7\n";

/// A curl that delivers other bytes than the release.
const WRONG_CURL: &str = r#"#!/bin/sh
out=
while [ "$#" -gt 0 ]; do
    case $1 in
        -o) out=$2; shift ;;
    esac
    shift
done
printf 'not the release archive' >"$out"
exit 0
"#;

/// A sandbox whose release `curl` serves in `pieces` pieces, `delay` seconds apart, and the PATH
/// with that curl.
fn served(pieces: usize, delay: u64, curl: &str) -> (Sandbox, OsString) {
    let mut sb = sandbox();
    let archive = sb.root.join("release.tar.gz");
    std::fs::write(&archive, &sb.release.archive).unwrap();
    let chunk = sb.release.archive.len().div_ceil(pieces);
    sb.set_env("BS_ARCHIVE", archive);
    sb.set_env("BS_CHUNK", chunk.to_string());
    sb.set_env("BS_DELAY", delay.to_string());
    sb.set_env("BS_DL_LOG", sb.root.join("dl.log"));
    let path = sb.path_with(&[], &[("curl", curl)]);
    (sb, path)
}

/// Where each curl run started, from the stub's log.
fn starts(sb: &Sandbox) -> Vec<u64> {
    std::fs::read_to_string(sb.root.join("dl.log"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.strip_prefix("from=").unwrap().parse().unwrap())
        .collect()
}

/// The `<n>` of a `still downloading (<n> bytes so far)` block or reason.
fn bytes_so_far(text: &str) -> u64 {
    let re = regex::Regex::new(r"downloading in the background \(([0-9]+) bytes so far\)|still downloading \(([0-9]+) bytes so far\)").unwrap();
    let c = re
        .captures(text)
        .unwrap_or_else(|| panic!("no byte count in {text:?}"));
    c.get(1).or(c.get(2)).unwrap().as_str().parse().unwrap()
}

/// The background fetcher runs: its lock is there and its pid is a live, non-zombie process.
fn fetcher_alive(sb: &Sandbox) -> bool {
    let Ok(pid) = std::fs::read_to_string(fdir(sb).join("run/pid")) else {
        return false;
    };
    let pid = pid.trim().to_string();
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        return !stat
            .rsplit_once(") ")
            .is_some_and(|(_, s)| s.starts_with('Z'));
    }
    std::process::Command::new("/bin/sh")
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .is_ok_and(|s| s.success())
}

fn wait_for(secs: u64, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    done()
}

/// Whether a fetcher outlives the hook that started it here: it leaves the run's session with
/// setsid, or its process group with a shell's job control (bash, macOS /bin/sh).
fn detaches(sh: Shell) -> bool {
    bs::which("setsid").is_some() || matches!(sh, Shell::BashPosix | Shell::MacSh)
}

/// Retries the Bash call as the block tells the model to, until it is allowed (at most six
/// calls). Before each call it waits for a running download to end and lets the minute's rate
/// limit pass (the stamp is aged), so a download that ended with its hook resumes on the next.
fn retry_until_allowed(sb: &Sandbox, sh: Shell, path: &OsString, ctx: &str) -> Run {
    for i in 0..6 {
        wait_for(90, || !fetcher_alive(sb));
        let stamp = sb.bindir().join(".attempt");
        if stamp.exists() {
            age(&stamp, 120);
        }
        let run = guard_path(sb, sh, BASH, path.clone());
        if run.code == Some(0) {
            return run;
        }
        assert_blocked(&run, &format!("{ctx}: retry {i}"));
    }
    panic!("{ctx}: still blocked after six retries");
}

/// Runs `f` once per shell, each in its own thread (these cases wait on real time).
fn per_shell(case: &str, f: fn(Shell)) {
    let handles: Vec<_> = bs::shells(case)
        .into_iter()
        .map(|sh| std::thread::spawn(move || f(sh)))
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

/// The field report: about 30 s for the archive, against 20 s per call. The first call blocks
/// with the progress, and the download goes on (in the background, or resumed by the next
/// call) until a retry is allowed; no attempt starts again from zero.
#[test]
fn a_download_slower_than_one_call_installs_instead_of_failing_forever() {
    per_shell("slow link", |sh| {
        let ctx = format!("slow link under {sh:?}");
        let (sb, path) = served(10, 3, SLOW_CURL);
        let first = guard_path(&sb, sh, BASH, path.clone());
        let got = bytes_so_far(&first.err());
        assert_block(&first, &downloading(&sb, got), &ctx);
        assert!(
            got > 0 && got < sb.release.archive.len() as u64,
            "{ctx}: {got} bytes"
        );
        assert!(
            first.elapsed >= Duration::from_secs(19) && first.elapsed <= Duration::from_secs(26),
            "{ctx}: the call waited {:?}",
            first.elapsed
        );
        let run = retry_until_allowed(&sb, sh, &path, &ctx);
        assert_allowed(&run, &ctx);
        assert_current(&sb, &fake(V, ALLOW), &ctx);
        let s = starts(&sb);
        assert_eq!(s[0], 0, "{ctx}: {s:?}");
        assert!(
            s[1..].iter().all(|&b| b > 0),
            "{ctx}: a later attempt started from zero: {s:?}"
        );
        stop_fetcher(&sb);
    });
}

/// A download stopped mid-way (the session ended, the machine slept) keeps its part, and the
/// next attempt continues it instead of starting again. The dead fetcher's lock is retired.
#[test]
fn a_stopped_download_resumes_from_its_part() {
    per_shell("resume", |sh| {
        let ctx = format!("resume under {sh:?}");
        let (sb, path) = served(8, 1, SLOW_CURL);
        patch_lib(&sb, "ANTH_BUDGET=3");
        let first = guard_path(&sb, sh, BASH, path.clone());
        let got = bytes_so_far(&first.err());
        assert_block(&first, &downloading(&sb, got), &ctx);
        stop_fetcher(&sb);
        assert!(wait_for(10, || !fetcher_alive(&sb)), "{ctx}: still running");
        let part = std::fs::metadata(fdir(&sb).join("part")).unwrap().len();
        assert!(
            part > 0 && part < sb.release.archive.len() as u64,
            "{ctx}: part {part}"
        );
        age(&sb.bindir().join(".attempt"), 120);
        let second = guard_path(&sb, sh, BASH, path.clone());
        assert_blocked(&second, &ctx);
        let s = starts(&sb);
        assert_eq!(s.len(), 2, "{ctx}: {s:?}");
        assert!(
            s[1] > 0 && s[1] <= part,
            "{ctx}: resumed at {} of {part}",
            s[1]
        );
        let run = retry_until_allowed(&sb, sh, &path, &ctx);
        assert_allowed(&run, &ctx);
        assert_current(&sb, &fake(V, ALLOW), &ctx);
        assert!(
            starts(&sb)[1..].iter().all(|&b| b > 0),
            "{ctx}: {:?}",
            starts(&sb)
        );
        stop_fetcher(&sb);
    });
}

/// Each failure has its own block, the next call in the minute repeats it (not "rate
/// limited"), and the bootstrap's status line carries the same reason.
#[test]
fn a_failed_download_says_why() {
    per_shell("download reasons", |sh| {
        // No network: curl cannot connect.
        let (sb, path) = served(1, 0, OFFLINE_CURL);
        let c = format!("{sh:?} offline");
        assert_block(
            &guard_path(&sb, sh, BASH, path.clone()),
            &no_network(&sb),
            &c,
        );
        assert_block(
            &guard_path(&sb, sh, BASH, path.clone()),
            &no_network(&sb),
            &c,
        );
        age(&sb.bindir().join(".attempt"), 120);
        let run = boot_path(
            &sb,
            sh,
            &["guard", "--plugin", "guardrails", "--quiet"],
            path,
        );
        assert_status(&run, &format!("status=failed:{R_OFFLINE}"), &c);
        assert_not_installed(&sb, &c);

        // Other bytes than the pin: never kept, never installed.
        let (sb, path) = served(1, 0, WRONG_CURL);
        let c = format!("{sh:?} sha256");
        assert_block(
            &guard_path(&sb, sh, BASH, path.clone()),
            &sha_mismatch(&sb),
            &c,
        );
        assert_block(&guard_path(&sb, sh, BASH, path), &sha_mismatch(&sb), &c);
        assert!(
            !fdir(&sb).join("part").exists() && !fdir(&sb).join("done").exists(),
            "{c}: {:?}",
            std::fs::read_dir(fdir(&sb)).map(|r| r.count())
        );
        assert_not_installed(&sb, &c);

        // Too slow for the download's own budget: the part is kept for the next try.
        let (sb, path) = served(20, 1, SLOW_CURL);
        patch_lib(&sb, "ANTH_FETCH_BUDGET=3");
        let c = format!("{sh:?} timeout");
        assert_block(
            &guard_path(&sb, sh, BASH, path.clone()),
            &timed_out(&sb),
            &c,
        );
        let part = std::fs::metadata(fdir(&sb).join("part")).map(|m| m.len());
        assert!(part.as_ref().is_ok_and(|&n| n > 0), "{c}: {part:?}");
        assert_not_installed(&sb, &c);
        stop_fetcher(&sb);
    });
}

/// A symbolic link planted in the fetch folder, or as the folder itself, is never followed:
/// the guard blocks and names it, the fetcher run directly neither downloads nor writes, and
/// the link's target is untouched (a `part` link would get the archive appended through
/// `curl -C -`, a `fail.<pid>` link truncated, a `done` link to a folder receive the archive).
#[test]
fn a_planted_link_in_the_fetch_folder_is_never_followed() {
    each_shell("planted links", |sh| {
        for (name, to_dir) in [
            ("part", false),
            ("done", false),
            ("done", true),
            ("fail", false),
            ("fail.1", false),
            ("fail.99999", true),
            ("", true),
        ] {
            let c = format!(
                "{sh:?} link {name:?} to a {}",
                if to_dir { "folder" } else { "file" }
            );
            let (sb, path) = served(1, 0, SLOW_CURL);
            let target = sb.root.join("outside");
            if to_dir {
                std::fs::create_dir(&target).unwrap();
            } else {
                std::fs::write(&target, "keep\n").unwrap();
            }
            let link = if name.is_empty() {
                std::fs::create_dir_all(sb.bindir()).unwrap();
                fdir(&sb)
            } else {
                std::fs::create_dir_all(fdir(&sb)).unwrap();
                fdir(&sb).join(name)
            };
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let reason = format!(
                "a symbolic link in bin/.fetch-{} was refused",
                sb.release.version
            );
            assert_block(
                &guard_path(&sb, sh, BASH, path.clone()),
                &not_installed(&sb, &reason),
                &c,
            );
            let run = boot_path(&sb, sh, &["fetch", "--plugin", "guardrails"], path);
            assert_eq!((run.code, run.out()), (Some(0), String::new()), "{c}");
            stop_fetcher(&sb);
            assert!(starts(&sb).is_empty(), "{c}: downloaded {:?}", starts(&sb));
            if to_dir {
                let n = std::fs::read_dir(&target).unwrap().count();
                assert_eq!(n, 0, "{c}: written into the target folder");
            } else {
                assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep\n", "{c}");
            }
            assert!(
                std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "{c}: the link itself is left for the user"
            );
            assert_not_installed(&sb, &c);
        }
    });
}

/// Session start begins the download, so it is often done before the first tool call; where
/// the fetcher outlives the hook, that call installs it without a second download.
#[test]
fn session_start_begins_the_background_download() {
    per_shell("session download", |sh| {
        let ctx = format!("session download under {sh:?}");
        let (sb, path) = served(6, 1, SLOW_CURL);
        patch_lib(&sb, "ANTH_BUDGET=1");
        let run = boot_path(
            &sb,
            sh,
            &["session", "--plugin", "guardrails"],
            path.clone(),
        );
        let out = run.out();
        let got = bytes_so_far(&out);
        assert_eq!(
            (run.code, out.clone()),
            (
                Some(0),
                format!("{}\n", sys_guardrails(&sb, &r_downloading(got)))
            ),
            "{ctx}"
        );
        assert!(
            run.elapsed < Duration::from_secs(10),
            "{ctx}: {:?}",
            run.elapsed
        );
        if detaches(sh) {
            assert!(
                wait_for(30, || fdir(&sb).join("done").is_file()),
                "{ctx}: the download did not outlive the hook"
            );
            assert_allowed(&guard_path(&sb, sh, BASH, path), &ctx);
            assert_eq!(starts(&sb), vec![0], "{ctx}: one download");
            assert_current(&sb, &fake(V, ALLOW), &ctx);
        }
        stop_fetcher(&sb);
    });
}

// --- the way out ----------------------------------------------------------------------------

/// `s` as a JSON string, escaped as JSON.stringify does.
fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A PreToolUse payload of a Bash call running `cmd`.
fn bash(cmd: &str) -> Vec<u8> {
    format!(
        r#"{{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{{"command":{},"description":"Install the guard binary","timeout":360000}}}}"#,
        json_str(cmd)
    )
    .into_bytes()
}

/// The terminal sentence, for a plugin folder the guard cannot match a command against.
fn terminal_msg() -> &'static str {
    ". To install it now, run hooks/bootstrap.sh guard --plugin guardrails --quiet --budget 300 from the anthology-guardrails plugin folder in a terminal"
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

#[test]
fn the_installer_is_the_one_command_let_through_while_the_binary_is_missing() {
    each_shell("installer allowance", |sh| {
        let mut sb = sandbox();
        sb.seed();
        // Nothing installs on a blocked call, so every block below is the same and immediate.
        sb.set_env("ANTHOLOGY_GUARD_NO_FETCH", "1");
        let fix_of = super::common::fix;
        let fix = fix_of(&sb);
        let base = fix
            .strip_suffix(" --quiet --budget 300")
            .unwrap()
            .to_string();
        let hooks = sb
            .plugin("anthology-guardrails")
            .join("hooks")
            .display()
            .to_string();

        // Exactly these forms are let through, and letting one through installs nothing.
        for cmd in [
            fix.clone(),
            base.clone(),
            format!("{base} --quiet"),
            format!("{base} --budget 7"),
            format!("{base} --quiet --budget 9999"),
        ] {
            assert_allowed(&guard(&sb, sh, &bash(&cmd)), &format!("{sh:?} {cmd}"));
        }
        let spaced = format!(
            "{{\n  \"tool_name\" : \"Bash\",\n  \"tool_input\" : {{\n    \"command\" : {}\n  }}\n}}\n",
            json_str(&fix)
        );
        assert_allowed(
            &guard(&sb, sh, spaced.as_bytes()),
            &format!("{sh:?} spaced"),
        );
        assert!(!sb.vdir().exists(), "{sh:?}: nothing installed yet");

        // Everything else is blocked, whatever it adds, drops or swaps.
        let link = sb.root.join("link");
        std::os::unix::fs::symlink(sb.plugin("anthology-guardrails"), &link).unwrap();
        let blocked = not_installed(&sb, "not attempted");
        let mut commands = vec![
            format!("{fix}; touch /tmp/anthology-bs-x"),
            format!("{fix};id"),
            format!("{fix} && id"),
            format!("{fix} || id"),
            format!("{fix} | sh"),
            format!("{fix} & id"),
            format!("{fix} >/tmp/anthology-bs-x"),
            format!("{fix} 2>&1"),
            format!("{fix} <&0"),
            format!("{fix} $(id)"),
            format!("{fix} `id`"),
            format!("{fix} #"),
            format!("{fix}\nid"),
            format!("{fix}\rid"),
            format!("{fix} "),
            format!(" {fix}"),
            format!("{base}  --quiet"),
            format!("{base}\t--quiet"),
            format!("{base} --budget 300 --quiet"),
            format!("{base} --quiet --quiet"),
            format!("{base} --budget"),
            format!("{base} --budget 0"),
            format!("{base} --budget 0300"),
            format!("{base} --budget 10000"),
            format!("{base} --budget -1"),
            format!("{base} --budget 3e2"),
            format!("{base} --budget 30 0"),
            format!("{base} --extra"),
            format!("FOO=1 {fix}"),
            format!("exec {fix}"),
            format!("({fix})"),
            format!("{{ {fix}; }}"),
            format!("sh -c \"{fix}\""),
            fix.replacen("/bin/sh ", "sh ", 1),
            fix.replacen("/bin/sh ", "/bin/bash ", 1),
            fix.replacen("/bin/sh ", "/bin/sh -x ", 1),
            fix.replacen("/bin/sh ", "/bin//sh ", 1),
            fix.replacen("/bin/sh ", "env /bin/sh ", 1),
            fix.replace('\'', ""),
            fix.replace('\'', "\""),
            fix.replace(" guard ", " session "),
            fix.replace(" guard ", " fetch "),
            fix.replace(" guard ", " wrapper "),
            fix.replace("--plugin guardrails", "--plugin anthology"),
            fix.replace("bootstrap.sh", "guard.sh"),
            fix.replace(&hooks, &format!("{}/hooks", link.display())),
            fix.replace(&hooks, &format!("{hooks}/../hooks")),
            fix.replace(&hooks, &format!("{hooks}/")),
            fix.replace(&hooks, "/tmp/anthology-bs-evil/hooks"),
            fix.replace(&hooks, &hooks.replacen('/', "//", 1)),
            fix.to_uppercase(),
            String::new(),
        ];
        commands.dedup();
        for cmd in &commands {
            assert_block(
                &guard(&sb, sh, &bash(cmd)),
                &blocked,
                &format!("{sh:?} {cmd:?}"),
            );
        }
        // Payloads that hold the exact command but are not that one Bash call.
        let f = json_str(&fix);
        let escaped_slash = json_str(&fix).replace('/', "\\/");
        let unicode_space = json_str(&fix).replacen(' ', "\\u0020", 1);
        // "command" with its first letter as a JSON escape: a parser reads it as the command
        // key, a raw-text match does not.
        let k = format!("\"{}u0063ommand\"", '\\');
        for (name, payload) in [
            (
                "an escaped command key, the installer nested",
                format!(
                    r#"{{"tool_name":"Bash","tool_input":{{{k}:"id","x":{{"command":{f}}}}}}}"#
                ),
            ),
            (
                "an escaped command key after the installer",
                format!(r#"{{"tool_name":"Bash","tool_input":{{"command":{f},{k}:"id"}}}}"#),
            ),
            (
                "an escaped command key before the installer",
                format!(r#"{{"tool_name":"Bash","tool_input":{{{k}:"id","command":{f}}}}}"#),
            ),
            (
                "an escaped tool_input key",
                format!(
                    r#"{{"tool_name":"Bash","tool_{}u0069nput":{{"command":"id"}},"x":{{"command":{f}}}}}"#,
                    '\\'
                ),
            ),
            (
                "an escape in another value",
                format!(
                    r#"{{"tool_name":"Bash","tool_input":{{"command":{f},"description":"a \"quoted\" word"}}}}"#
                ),
            ),
            (
                "a second command key",
                format!(r#"{{"tool_name":"Bash","tool_input":{{"command":{f},"command":"id"}}}}"#),
            ),
            (
                "a second command key first",
                format!(r#"{{"tool_name":"Bash","tool_input":{{"command":"id","command":{f}}}}}"#),
            ),
            (
                "a nested command key",
                format!(
                    r#"{{"tool_name":"Bash","tool_input":{{"command":{f},"x":{{"command":"id"}}}}}}"#
                ),
            ),
            (
                "only a nested one is exact",
                format!(
                    r#"{{"tool_name":"Bash","tool_input":{{"command":"id","x":{{"command":{f}}}}}}}"#
                ),
            ),
            (
                "a top-level command key",
                format!(r#"{{"tool_name":"Bash","command":"id","tool_input":{{"command":{f}}}}}"#),
            ),
            (
                "an escaped slash",
                format!(r#"{{"tool_name":"Bash","tool_input":{{"command":{escaped_slash}}}}}"#),
            ),
            (
                "a unicode escape",
                format!(r#"{{"tool_name":"Bash","tool_input":{{"command":{unicode_space}}}}}"#),
            ),
            (
                "an array",
                format!(r#"{{"tool_name":"Bash","tool_input":{{"command":[{f}]}}}}"#),
            ),
            (
                "a tab between tokens",
                format!("{{\"tool_name\":\"Bash\",\t\"tool_input\":{{\"command\":{f}}}}}"),
            ),
            (
                "two tool names",
                format!(
                    r#"{{"tool_name":"Bash","tool_name":"Bash","tool_input":{{"command":{f}}}}}"#
                ),
            ),
            (
                "Write",
                format!(r#"{{"tool_name":"Write","tool_input":{{"command":{f}}}}}"#),
            ),
            (
                "an MCP tool",
                format!(r#"{{"tool_name":"mcp__shell__run","tool_input":{{"command":{f}}}}}"#),
            ),
            (
                "no tool name",
                format!(r#"{{"tool_input":{{"command":{f}}}}}"#),
            ),
        ] {
            assert_block(
                &guard(&sb, sh, payload.as_bytes()),
                &blocked,
                &format!("{sh:?} {name}"),
            );
        }
        // An environment cannot name another command.
        let mut env_sb = sandbox();
        env_sb.set_env("ANTHOLOGY_GUARD_NO_FETCH", "1");
        env_sb.set_env("ANTH_FIX", "ls -la");
        env_sb.set_env("ANTH_FIXMSG", "x");
        assert_block(
            &guard(&env_sb, sh, BASH),
            &not_installed(&env_sb, "not attempted"),
            &format!("{sh:?} env"),
        );

        // Only while the binary is missing: a working binary, a broken one and a closed guard
        // decide as before.
        let hot = sandbox();
        hot.install(&fake(V, BLOCK), 0o755);
        let run = guard(&hot, sh, &bash(&fix_of(&hot)));
        assert_eq!(
            (run.code, run.err()),
            (Some(2), format!("{G1}\n")),
            "{sh:?} hot"
        );
        let broken = sandbox();
        broken.install(b"", 0o755);
        assert_block(
            &guard(&broken, sh, &bash(&fix_of(&broken))),
            &set_aside(&broken, "it is empty"),
            &format!("{sh:?} broken"),
        );
        let closed = sandbox();
        closed.write_lock("{}\n");
        assert_block(
            &guard(&closed, sh, &bash(&fix_of(&closed))),
            &lock_invalid(),
            &format!("{sh:?} closed"),
        );

        // A hook started by a relative path, or from a folder with a quote in its name, cannot
        // be matched exactly: nothing is let through, and the block says to use a terminal.
        let mut spec = sb.spec(sh, GUARD, &[], b"");
        *spec.argv.last_mut().unwrap() = GUARD.into();
        spec.stdin = bash(
            "/bin/sh 'plugins/anthology-guardrails/hooks/bootstrap.sh' guard --plugin guardrails --quiet --budget 300",
        );
        let tail = format!(
            "Blocked by anthology-guardrails (setup): the guard binary is not installed (not attempted), so shell commands, file edits and MCP tools are blocked; start a new session while online to install it{P}{}",
            terminal_msg()
        );
        assert_block(&sb.run_spec(spec), &tail, &format!("{sh:?} relative"));
        let odd = sb.root.join("it's").join("anthology-guardrails");
        copy_dir(
            &sb.plugin("anthology-guardrails").join("hooks"),
            &odd.join("hooks"),
        );
        std::fs::copy(
            sb.plugin("anthology-guardrails").join("anthology.lock"),
            odd.join("anthology.lock"),
        )
        .unwrap();
        let odd_guard: PathBuf = odd.join("hooks/guard.sh");
        let mut spec = sb.spec(sh, GUARD, &[], b"");
        *spec.argv.last_mut().unwrap() = odd_guard.into();
        spec.stdin = bash(&format!(
            "/bin/sh '{}/bootstrap.sh' guard --plugin guardrails",
            odd.join("hooks").display()
        ));
        assert_block(&sb.run_spec(spec), &tail, &format!("{sh:?} quote"));

        // The named command is the real installer: run as written, it installs the binary.
        let mut spec = sb.spec(sh, GUARD, &[], b"");
        spec.argv = vec!["/bin/sh".into(), "-c".into(), fix.clone().into()];
        let run = sb.run_spec(spec);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), "status=installed\n".to_string()),
            "{sh:?}: {}",
            run.err()
        );
        assert_allowed(&guard(&sb, sh, BASH), &format!("{sh:?} after the install"));
    });
}

/// The setup skill names the same command, so a model that runs /anthology:setup while the
/// guard blocks recovers instead of retrying a blocked call.
#[test]
fn the_setup_skill_names_the_installer() {
    let skill = anthology_testkit::staged("plugins/anthology/skills/setup/SKILL.md");
    let text = std::fs::read_to_string(&skill).unwrap();
    assert!(
        text.contains(
            "/bin/sh '<guardrails plugin folder>/hooks/bootstrap.sh' guard --plugin guardrails --quiet --budget 300"
        ),
        "{}",
        skill.display()
    );
}
