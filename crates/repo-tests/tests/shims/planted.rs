//! Tools planted by the opened repository. Hooks run in that repository's folder, so a PATH
//! entry that is relative (`.`, `bin`) or empty (`::`, a leading or trailing `:`) would let it
//! supply the curl, tar or sha256 tool the installer runs. lib.sh's anth_safe_path drops such
//! entries (and falls back to a fixed system list when nothing is left): a planted tool never
//! runs, and the install either uses the real tools or fails closed. The anthology binary itself
//! still gets the caller's PATH, unchanged.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anthology_testkit::bs::{self, Mode, Sandbox};

use super::common::*;

/// The tools a planted copy stands in for: every helper the shims and hooks run. The fake
/// binaries here use builtins only: they run on the caller's PATH on purpose.
const PLANTED: &[&str] = &[
    "cat",
    "sed",
    "head",
    "grep",
    "timeout",
    "osascript",
    "notify-send",
    "canberra-gtk-play",
    "paplay",
    "curl",
    "wget",
    "node",
    "tar",
    "sha256sum",
    "shasum",
    "openssl",
    "date",
    "sh",
    "setsid",
    "mktemp",
    "mkdir",
    "mv",
    "cp",
    "rm",
    "chmod",
    "tr",
    "awk",
    "find",
    "wc",
    "cut",
    "uname",
    "readlink",
    "dirname",
];

/// What an install from the seed runs, and session start's clock.
const SEED_TOOLS: &[&str] = &["tar", "sha256sum", "shasum", "openssl", "date"];

/// Where the unsafe entry sits in PATH.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    /// `.` first.
    Dot,
    /// A leading `:`.
    Leading,
    /// `::` after a folder without the tools.
    Middle,
    /// The relative folder `bin` first.
    RelDir,
    /// A trailing `:`, after real folders that lack the tools the case needs.
    Trailing,
    /// Nothing absolute at all (`.:bin:`): the fixed list.
    OnlyRelative,
}

impl Entry {
    const ALL: [Entry; 6] = [
        Entry::Dot,
        Entry::Leading,
        Entry::Middle,
        Entry::RelDir,
        Entry::Trailing,
        Entry::OnlyRelative,
    ];
}

fn marker(sb: &Sandbox) -> PathBuf {
    sb.root.join("planted-ran")
}

/// Fake tools in `dir` that record their call in the marker file and claim success.
fn plant(sb: &Sandbox, dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$0 $*\" >>'{}'\nexit 0\n",
        marker(sb).display()
    );
    for name in PLANTED {
        let p = dir.join(name);
        std::fs::write(&p, &script).unwrap();
        chmod(&p, 0o755);
    }
}

/// Plants the tools in the working folder (runs start in the sandbox root) and in its `bin`,
/// and returns a PATH with `entry`, then the real tools. For `Trailing` the real folder lacks
/// `masked`, so only the working folder has them.
fn planted_path(sb: &Sandbox, entry: Entry, masked: &[&str]) -> OsString {
    let base = Sandbox::base_path().to_string_lossy().into_owned();
    let trailing = if entry == Entry::Trailing {
        sb.path_with(masked, &[]).to_string_lossy().into_owned()
    } else {
        String::new()
    };
    planted_path_on(sb, entry, &base, &trailing)
}

/// [`planted_path`] with the real folders given: `base` after the unsafe entry, and `trailing`
/// before a trailing `:`.
fn planted_path_on(sb: &Sandbox, entry: Entry, base: &str, trailing: &str) -> OsString {
    plant(sb, &sb.root);
    plant(sb, &sb.root.join("bin"));
    let path = match entry {
        Entry::Dot => format!(".:{base}"),
        Entry::Leading => format!(":{base}"),
        Entry::Middle => format!("{}::{base}", sb.tmpdir.display()),
        Entry::RelDir => format!("bin:{base}"),
        Entry::Trailing => format!("{trailing}:"),
        Entry::OnlyRelative => ".:bin:".to_string(),
    };
    path.into()
}

/// A fake binary that needs no command from PATH: it runs on the caller's PATH, which in
/// these cases holds the planted tools (or, with `OnlyRelative`, no real ones at all).
fn builtin_fake() -> Vec<u8> {
    fake_hook(
        V,
        "printf 'anthology-guard:allow:v1\\n' >&2; exit 0",
        "printf '{\"systemMessage\":\"dl=%s t0=%s\"}\\n' \"${ANTHOLOGY_BOOTSTRAP_DOWNLOADED:-}\" \"${ANTHOLOGY_HOOK_T0:+set}\"; exit 0",
    )
}

/// A sandbox pinned to a release of [`builtin_fake`].
fn builtin_sandbox() -> Sandbox {
    Sandbox::new(release_of(V, &builtin_fake()))
}

#[track_caller]
fn assert_nothing_planted_ran(sb: &Sandbox, ctx: &str) {
    let ran = std::fs::read_to_string(marker(sb)).unwrap_or_default();
    assert!(ran.is_empty(), "{ctx}: a planted tool ran:\n{ran}");
}

#[test]
fn the_guard_installs_from_the_seed_with_the_real_tools_only() {
    each_shell("planted tools, guard", |sh| {
        for entry in Entry::ALL {
            let sb = builtin_sandbox();
            sb.seed();
            let path = planted_path(&sb, entry, SEED_TOOLS);
            let c = format!("{sh:?} {entry:?}");
            let run = guard_path(&sb, sh, BASH, path);
            assert_nothing_planted_ran(&sb, &c);
            if entry == Entry::Trailing {
                assert_block(&run, &not_installed(&sb, R_SHA_TOOL), &c);
                assert_not_installed(&sb, &c);
            } else {
                assert_allowed(&run, &c);
                assert_current(&sb, &builtin_fake(), &c);
            }
        }
    });
}

#[test]
fn session_start_runs_no_planted_tool() {
    each_shell("planted tools, session start", |sh| {
        for entry in Entry::ALL {
            let sb = builtin_sandbox();
            sb.seed();
            let mut spec = sb.spec(sh, SESSION, &[], b"{}");
            spec.path = Some(planted_path(&sb, entry, SEED_TOOLS));
            let c = format!("{sh:?} {entry:?}");
            let run = sb.run_spec(spec);
            assert_nothing_planted_ran(&sb, &c);
            let want = if entry == Entry::Trailing {
                sys_anthology(R_SHA_TOOL)
            } else {
                "{\"systemMessage\":\"dl=1 t0=set\"}".to_string()
            };
            assert_eq!(
                (run.code, run.out()),
                (Some(0), format!("{want}\n")),
                "{c}: {}",
                run.err()
            );
        }
    });
}

#[test]
fn a_download_never_runs_a_planted_downloader() {
    each_shell("planted tools, download", |sh| {
        for entry in Entry::ALL {
            let sb = builtin_sandbox();
            let net = bs::net_enabled();
            if net {
                bs::server().register(
                    &sb.release.url_path(),
                    Mode::Normal(sb.release.archive.clone()),
                );
            }
            let mut spec = sb.spec(
                sh,
                A_BOOT,
                &["wrapper", "--plugin", "anthology", "--quiet"],
                b"",
            );
            spec.path = Some(planted_path(&sb, entry, DOWNLOADERS));
            let c = format!("{sh:?} {entry:?}");
            let run = sb.run_spec(spec);
            assert_nothing_planted_ran(&sb, &c);
            if entry == Entry::Trailing {
                let reason = no_downloader_reason(&sb.release.url());
                assert_status(&run, &format!("status=failed:{reason}"), &c);
                assert_not_installed(&sb, &c);
            } else if net {
                assert_status(&run, "status=installed", &c);
                assert_current(&sb, &builtin_fake(), &c);
            } else {
                // No network here: the real downloader fails, and nothing is installed.
                assert!(
                    run.out().starts_with("status=failed:"),
                    "{c}: {} {}",
                    run.out(),
                    run.err()
                );
                assert_eq!(run.code, Some(1), "{c}");
                assert_not_installed(&sb, &c);
            }
        }
    });
}

/// The guardrails' background download: the fetcher (`setsid /bin/sh bootstrap.sh fetch`), its
/// folder and its downloader all come from the safe PATH.
#[test]
fn the_background_download_runs_no_planted_tool() {
    each_shell("planted tools, background download", |sh| {
        for entry in Entry::ALL {
            let sb = builtin_sandbox();
            let net = bs::net_enabled();
            if net {
                bs::server().register(
                    &sb.release.url_path(),
                    Mode::Normal(sb.release.archive.clone()),
                );
            }
            let path = planted_path(&sb, entry, DOWNLOADERS);
            let c = format!("{sh:?} {entry:?}");
            let run = boot_path(
                &sb,
                sh,
                &["guard", "--plugin", "guardrails", "--quiet"],
                path,
            );
            stop_fetcher(&sb);
            assert_nothing_planted_ran(&sb, &c);
            if entry == Entry::Trailing {
                let reason = no_downloader_reason(&sb.release.url());
                assert_status(&run, &format!("status=failed:{reason}"), &c);
                assert_not_installed(&sb, &c);
            } else if net {
                assert_status(&run, "status=installed", &c);
                assert_current(&sb, &builtin_fake(), &c);
            } else {
                assert!(
                    run.out().starts_with("status=failed:"),
                    "{c}: {} {}",
                    run.out(),
                    run.err()
                );
                assert_not_installed(&sb, &c);
            }
        }
    });
}

/// bin/anthology finds its plugin root with readlink and dirname before lib.sh loads: those
/// come from the safe PATH too, through a symlink to the wrapper as well.
#[test]
fn the_wrappers_run_no_planted_tool() {
    each_shell("planted tools, wrappers", |sh| {
        for entry in Entry::ALL {
            let sb = builtin_sandbox();
            sb.install(&builtin_fake(), 0o755);
            let link_dir = sb.root.join("links");
            std::fs::create_dir_all(&link_dir).unwrap();
            for (script, want) in [
                (WRAP, "args:perm classify|atl=1\n"),
                (WRAP_MEM, "args:mem perm classify|atl=1\n"),
            ] {
                let link = link_dir.join(Path::new(script).file_name().unwrap());
                std::os::unix::fs::symlink(sb.root.join(script), &link).unwrap();
                for via_link in [false, true] {
                    let c = format!("{sh:?} {entry:?} {script} link={via_link}");
                    let mut spec = sb.spec(sh, script, &["perm", "classify"], b"");
                    if via_link {
                        let i = spec.argv.len() - 3;
                        spec.argv[i] = link.clone().into();
                    }
                    spec.path = Some(planted_path(&sb, entry, &[]));
                    let run = sb.run_spec(spec);
                    assert_nothing_planted_ran(&sb, &c);
                    assert_eq!(
                        (run.code, run.out()),
                        (Some(0), want.to_string()),
                        "{c}: {}",
                        run.err()
                    );
                }
            }
        }
    });
}

/// The binary is not handed the safe PATH: the wrappers, session start and the guard run it
/// with the caller's PATH, relative and empty entries included (`anthology perm classify`
/// judges that PATH itself).
#[test]
fn the_binary_gets_the_callers_path() {
    let show = "printf '%s\\n' \"$PATH\"; exit 0";
    let guard_log = "cat >/dev/null; printf '%s\\n' \"$PATH\" >\"$BS_LOG\"; printf 'anthology-guard:allow:v1\\n' >&2; exit 0";
    each_shell("caller's PATH", |sh| {
        let mut sb = sandbox();
        let log = sb.root.join("guard-path");
        sb.set_env("BS_LOG", &log);
        sb.install(&fake_hook(V, guard_log, show), 0o755);
        let base = Sandbox::base_path().to_string_lossy().into_owned();
        let path = format!(".:bin::{base}:");
        // `hook` reaches the fake's hook body, which prints PATH (anthology-mem would add
        // `mem` first; the two wrappers differ in nothing else, statics.rs).
        let mut spec = sb.spec(sh, WRAP, &["hook"], b"");
        spec.path = Some(path.clone().into());
        let run = sb.run_spec(spec);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), format!("{path}\n")),
            "{sh:?} wrapper: {}",
            run.err()
        );
        let mut spec = sb.spec(sh, SESSION, &[], b"{}");
        spec.path = Some(path.clone().into());
        let run = sb.run_spec(spec);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), format!("{path}\n")),
            "{sh:?} session start: {}",
            run.err()
        );
        let run = guard_path(&sb, sh, BASH, path.clone().into());
        assert_allowed(&run, &format!("{sh:?} guard"));
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!("{path}\n"),
            "{sh:?} guard"
        );
    });
}

/// A long PATH costs time linear in its length (the quadratic split took seconds on 64 KB).
/// About 100 KB, relative entries throughout, on the guard's hot path and in session start.
#[test]
fn a_long_path_is_made_safe_quickly() {
    let base = Sandbox::base_path().to_string_lossy().into_owned();
    let mut path = base.clone();
    while path.len() < 100_000 {
        path.push_str(":/nonexistent/anthology-x:.:rel/bin:");
    }
    path.push_str(&base);
    each_shell("long PATH", |sh| {
        let sb = sandbox();
        sb.install(&fake(V, ALLOW), 0o755);
        let run = guard_path(&sb, sh, BASH, path.clone().into());
        assert_allowed(&run, &format!("{sh:?} guard"));
        assert!(
            run.elapsed < Duration::from_secs(5),
            "{sh:?} guard: {:?}",
            run.elapsed
        );
        let mut spec = sb.spec(sh, SESSION, &[], b"{}");
        spec.path = Some(path.clone().into());
        let run = sb.run_spec(spec);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), "{\"systemMessage\":\"dl=0 t0=set\"}\n".to_string()),
            "{sh:?} session start: {}",
            run.err()
        );
        assert!(
            run.elapsed < Duration::from_secs(5),
            "{sh:?} session start: {:?}",
            run.elapsed
        );
    });
}

/// A PATH every entry of which is absolute and not empty.
#[track_caller]
fn assert_absolute_path(text: &str, ctx: &str) {
    let p = text.trim_end_matches('\n');
    assert!(
        !p.is_empty() && p.split(':').all(|d| d.starts_with('/')),
        "{ctx}: the binary got {p:?}"
    );
}

/// A caller with no PATH in its environment (`env -u PATH`): the shell fills in its own default,
/// which it does not export (macOS sh's ends in `.`). That is no PATH of the caller's, so the
/// binary gets the safe one: never a relative entry, from the wrapper, session start or guard.
#[test]
fn without_a_path_the_binary_gets_no_relative_entry() {
    let show = "printf '%s\\n' \"$PATH\"; exit 0";
    let guard_log = "while IFS= read -r _l; do :; done; printf '%s\\n' \"$PATH\" >\"$BS_LOG\"; printf 'anthology-guard:allow:v1\\n' >&2; exit 0";
    each_shell("no PATH", |sh| {
        let sb = sandbox();
        let log = sb.root.join("guard-path");
        sb.install(&fake_hook(V, guard_log, show), 0o755);
        let env: Vec<(OsString, OsString)> = vec![
            ("HOME".into(), sb.home.clone().into()),
            ("CLAUDE_CONFIG_DIR".into(), sb.config.clone().into()),
            ("TMPDIR".into(), sb.tmpdir.clone().into()),
            ("LANG".into(), "C".into()),
            ("BS_LOG".into(), log.clone().into()),
        ];
        let run = |script: &str, args: &[&str], stdin: &[u8]| {
            let mut argv = sh.argv().expect("the shell is present");
            argv.push(sb.root.join(script).into());
            argv.extend(args.iter().map(OsString::from));
            bs::run_argv(&argv, &env, stdin, Duration::from_secs(60), &sb.root)
        };
        let r = run(WRAP, &["hook"], b"");
        assert_eq!(r.code, Some(0), "{sh:?} wrapper: {}", r.err());
        assert_absolute_path(&r.out(), &format!("{sh:?} wrapper"));
        let r = run(SESSION, &[], b"{}");
        assert_eq!(r.code, Some(0), "{sh:?} session start: {}", r.err());
        assert_absolute_path(&r.out(), &format!("{sh:?} session start"));
        let r = run(GUARD, &[], BASH);
        assert_allowed(&r, &format!("{sh:?} guard"));
        assert_absolute_path(
            &std::fs::read_to_string(&log).unwrap(),
            &format!("{sh:?} guard"),
        );
    });
}

/// The anthology plugin's other hooks (atl-hook, no-attribution.sh) run on the safe PATH from
/// their first line.
#[test]
fn the_other_hooks_run_no_planted_tool() {
    let ask = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"x"}}"#;
    let atl_input =
        br#"{"session_id":"s1","tool_name":"Bash","tool_input":{"command":"atl gain"}}"#;
    let attribution = br#"{"session_id":"s1","tool_name":"Bash","tool_input":{"command":"git commit -m \"fix\n\nCo-Authored-By: Claude <noreply@anthropic.com>\""}}"#;
    each_shell("planted tools, other hooks", |sh| {
        for entry in Entry::ALL {
            let c = format!("{sh:?} {entry:?}");
            let sb = builtin_sandbox();
            // atl-hook: atl is installed and asks on a line that names atl, which is kept.
            let atl = sb.root.join("fake-atl");
            std::fs::write(
                &atl,
                format!("#!/bin/sh\nwhile IFS= read -r _l; do :; done\nprintf '%s\\n' '{ask}'\n"),
            )
            .unwrap();
            chmod(&atl, 0o755);
            std::fs::create_dir_all(sb.ah()).unwrap();
            std::fs::write(
                sb.ah().join("state.json"),
                format!(
                    "{{\n  \"atl_path\": \"{}\",\n  \"z\": 1\n}}\n",
                    atl.display()
                ),
            )
            .unwrap();
            let mut spec = sb.spec(sh, "plugins/anthology/bin/atl-hook", &[], atl_input);
            spec.path = Some(planted_path(&sb, entry, &[]));
            let run = sb.run_spec(spec);
            assert_nothing_planted_ran(&sb, &format!("{c} atl-hook"));
            assert_eq!(
                (run.code, run.out()),
                (Some(0), format!("{ask}\n")),
                "{c} atl-hook: {}",
                run.err()
            );
            // no-attribution.sh: denies an assistant co-author trailer.
            let mut spec = sb.spec(
                sh,
                "plugins/anthology/hooks/no-attribution.sh",
                &[],
                attribution,
            );
            spec.path = Some(planted_path(&sb, entry, &[]));
            let run = sb.run_spec(spec);
            assert_nothing_planted_ran(&sb, &format!("{c} no-attribution"));
            assert_eq!(run.code, Some(0), "{c} no-attribution: {}", run.err());
            assert!(
                run.out().contains("\"permissionDecision\":\"deny\""),
                "{c} no-attribution: {}",
                run.out()
            );
        }
    });
}
