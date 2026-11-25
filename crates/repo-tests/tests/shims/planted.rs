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

/// The tools a planted copy stands in for: every helper the shims run. Not `cat`: the fake
/// binaries use it, and they run on the caller's PATH on purpose.
const PLANTED: &[&str] = &[
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
    plant(sb, &sb.root);
    plant(sb, &sb.root.join("bin"));
    let base = Sandbox::base_path().to_string_lossy().into_owned();
    let path = match entry {
        Entry::Dot => format!(".:{base}"),
        Entry::Leading => format!(":{base}"),
        Entry::Middle => format!("{}::{base}", sb.tmpdir.display()),
        Entry::RelDir => format!("bin:{base}"),
        Entry::Trailing => format!("{}:", sb.path_with(masked, &[]).to_string_lossy()),
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

/// The guardrails' background download: the fetcher (`setsid sh bootstrap.sh fetch`), its
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
