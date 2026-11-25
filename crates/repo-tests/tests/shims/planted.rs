//! Tools planted by the opened repository. Hooks run in that repository's folder, so a PATH
//! entry that is relative (`.`, `bin`) or empty (`::`, a leading or trailing `:`) would let it
//! supply the curl, tar or sha256 tool the installer runs. lib.sh's anth_safe_path drops such
//! entries (and falls back to a fixed system list when nothing is left): a planted tool never
//! runs, and the install either uses the real tools or fails closed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anthology_testkit::bs::{self, Mode, Sandbox};

use super::common::*;

/// The tools a planted copy stands in for.
const PLANTED: &[&str] = &[
    "curl",
    "wget",
    "node",
    "tar",
    "sha256sum",
    "shasum",
    "openssl",
    "date",
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

#[track_caller]
fn assert_nothing_planted_ran(sb: &Sandbox, ctx: &str) {
    let ran = std::fs::read_to_string(marker(sb)).unwrap_or_default();
    assert!(ran.is_empty(), "{ctx}: a planted tool ran:\n{ran}");
}

#[test]
fn the_guard_installs_from_the_seed_with_the_real_tools_only() {
    each_shell("planted tools, guard", |sh| {
        for entry in Entry::ALL {
            let sb = sandbox();
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
                assert_current(&sb, &fake(V, ALLOW), &c);
            }
        }
    });
}

#[test]
fn session_start_runs_no_planted_tool() {
    each_shell("planted tools, session start", |sh| {
        for entry in Entry::ALL {
            let sb = sandbox();
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
            let sb = sandbox();
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
                assert_current(&sb, &fake(V, ALLOW), &c);
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
