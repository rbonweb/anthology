//! Acceptance 2 and 8 with the real downloaders (curl, GNU wget, BusyBox wget, node) against
//! the testkit TLS server as github.com (m-R5, T4). Network mode only
//! (`ANTHOLOGY_SHIMS_NET=1`, set by ci/jobs/shims-distros.sh); elsewhere each case skips.

use std::sync::Arc;
use std::time::Duration;

use anthology_testkit::bs::{self, Downloader, Mode, Sandbox, Shell};

use super::common::*;

/// The downloaders present here (after the REQUIRE check), when the network cases run.
fn downloaders(case: &str) -> Vec<Downloader> {
    if !bs::capability("net", bs::net_enabled(), case) {
        return Vec::new();
    }
    Downloader::ALL
        .into_iter()
        .filter(|d| bs::capability(d.cap(), d.present(), case))
        .collect()
}

fn first_shell() -> Shell {
    bs::shells("downloads")[0]
}

/// A sandbox whose release the server serves with `mode`, and the PATH that makes `d` the
/// bootstrap's downloader.
fn served(d: Downloader, mode: impl FnOnce(&Sandbox) -> Mode) -> (Sandbox, std::ffi::OsString) {
    let sb = sandbox();
    bs::server().register(&sb.release.url_path(), mode(&sb));
    let path = sb.path_with(&d.mask_before(), &[]);
    (sb, path)
}

fn normal(sb: &Sandbox) -> Mode {
    Mode::Normal(sb.release.archive.clone())
}

#[test]
fn each_downloader_installs_from_the_server() {
    for d in downloaders("downloader install") {
        for sh in bs::shells("downloader install") {
            let (sb, path) = served(d, normal);
            let run = boot_path(
                &sb,
                sh,
                &["guard", "--plugin", "guardrails", "--quiet"],
                path,
            );
            assert_status(&run, "status=installed", &format!("{d:?} {sh:?}"));
            assert_current(&sb, &fake(V, ALLOW), &format!("{d:?} {sh:?}"));
            assert_eq!(
                bs::server().hits(&sb.release.url_path()),
                1,
                "{d:?}: one download"
            );
        }
    }
}

#[test]
fn a_fast_download_is_not_held_until_the_budget() {
    for d in downloaders("fast download") {
        let (sb, path) = served(d, normal);
        let run = boot_path(
            &sb,
            first_shell(),
            &["session", "--plugin", "guardrails", "--quiet"],
            path,
        );
        assert_status(&run, "status=installed", &format!("{d:?}"));
        // m-R4: the watchdog must not hold stdout until its 20 s budget runs out. The bound is
        // half the budget, not the design's 5 s proxy: on a loaded CI container process start,
        // TLS and the install alone took 5.06 s, while a held stdout costs >= 20 s.
        assert!(
            run.elapsed < Duration::from_secs(10),
            "{d:?}: {:?} (m-R4)",
            run.elapsed
        );
    }
}

#[test]
fn a_slow_download_that_finishes_installs() {
    for d in downloaders("slow download") {
        let (sb, path) = served(d, |sb| Mode::Slow(sb.release.archive.clone(), 3000));
        let run = boot_path(
            &sb,
            first_shell(),
            &["guard", "--plugin", "guardrails", "--quiet"],
            path,
        );
        assert_status(&run, "status=installed", &format!("{d:?}"));
        assert!(run.elapsed >= Duration::from_secs(2), "{d:?}: it was slow");
    }
}

#[test]
fn a_never_ending_download_is_killed_at_the_budget() {
    let ds = downloaders("never-ending download");
    let handles: Vec<_> = ds
        .into_iter()
        .map(|d| {
            std::thread::spawn(move || {
                let (sb, path) = served(d, |_| Mode::Never);
                // The default budget (20 s): killed within 20 s + 2 s.
                let run = boot_path(
                    &sb,
                    first_shell(),
                    &["guard", "--plugin", "guardrails", "--quiet"],
                    path,
                );
                assert_status(
                    &run,
                    &format!("status=failed:{R_DOWNLOAD}"),
                    &format!("{d:?}"),
                );
                assert!(
                    run.elapsed >= Duration::from_secs(19)
                        && run.elapsed <= Duration::from_secs(22),
                    "{d:?}: {:?}",
                    run.elapsed
                );
                assert_not_installed(&sb, &format!("{d:?}"));
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn session_start_with_a_never_ending_download_prints_one_line_within_45_seconds() {
    let ds = downloaders("session-start budget");
    let handles: Vec<_> = ds
        .into_iter()
        .map(|d| {
            std::thread::spawn(move || {
                let (sb, path) = served(d, |_| Mode::Never);
                let mut spec = sb.spec(first_shell(), SESSION, &[], b"{}");
                spec.path = Some(path);
                spec.timeout = Duration::from_secs(60);
                let run = sb.run_spec(spec);
                assert_eq!(
                    (run.code, run.out()),
                    (Some(0), format!("{}\n", sys_anthology(R_DOWNLOAD))),
                    "{d:?}"
                );
                // The budget (20 s) stopped it, not an early failure.
                assert!(
                    run.elapsed >= Duration::from_secs(19) && run.elapsed < Duration::from_secs(45),
                    "{d:?}: {:?}",
                    run.elapsed
                );
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn download_failures_have_their_reasons() {
    for d in downloaders("download failures") {
        let sh = first_shell();
        for (name, mode, want) in [
            ("404", Mode::NotFound, R_DOWNLOAD),
            ("offline", Mode::Drop, R_DOWNLOAD),
            ("redirect to http", Mode::RedirectHttp, R_DOWNLOAD),
            ("other bytes", Mode::Normal(b"other bytes".to_vec()), R_SHA),
        ] {
            let (sb, path) = served(d, |_| mode);
            let run = boot_path(
                &sb,
                sh,
                &[
                    "guard",
                    "--plugin",
                    "guardrails",
                    "--budget",
                    "8",
                    "--quiet",
                ],
                path,
            );
            let c = format!("{d:?} {name}");
            assert_status(&run, &format!("status=failed:{want}"), &c);
            assert_not_installed(&sb, &c);
        }
        // Over 64 MiB: refused by the cap, or by the hash after the fact; never installed.
        let (sb, path) = served(d, |_| Mode::Oversize);
        let run = boot_path(
            &sb,
            sh,
            &[
                "guard",
                "--plugin",
                "guardrails",
                "--budget",
                "15",
                "--quiet",
            ],
            path,
        );
        assert!(
            run.out().starts_with("status=failed:"),
            "{d:?} oversize: {}",
            run.out()
        );
        assert_not_installed(&sb, &format!("{d:?} oversize"));
        // An https redirect is followed.
        let target = format!("/anthology-test/{}/moved", bs::unique("m"));
        let (sb, path) = served(d, |_| Mode::Redirect(target.clone()));
        bs::server().register(&target, Mode::Normal(sb.release.archive.clone()));
        let run = boot_path(
            &sb,
            sh,
            &["guard", "--plugin", "guardrails", "--quiet"],
            path,
        );
        assert_status(&run, "status=installed", &format!("{d:?} redirect"));
    }
}

#[test]
fn a_stale_seed_falls_through_to_the_network() {
    for d in downloaders("stale seed") {
        let sh = first_shell();
        let (sb, path) = served(d, normal);
        sb.seed_bytes(b"stale");
        let run = boot_path(
            &sb,
            sh,
            &["guard", "--plugin", "guardrails", "--quiet"],
            path,
        );
        assert_status(&run, "status=installed", &format!("{d:?}"));
        assert_eq!(
            std::fs::read(sb.bindir().join(sb.release.asset())).unwrap(),
            b"stale"
        );
        // Named only when the network fails too.
        let (sb, path) = served(d, |_| Mode::NotFound);
        sb.seed_bytes(b"stale");
        let run = boot_path(
            &sb,
            sh,
            &["guard", "--plugin", "guardrails", "--quiet"],
            path.clone(),
        );
        assert_status(
            &run,
            &format!("status=failed:{}", r_seed(sb.release.target.triple())),
            &format!("{d:?}"),
        );
        std::fs::remove_file(sb.bindir().join(".attempt")).unwrap();
        assert_block(
            &guard_path(&sb, sh, BASH, path),
            &not_installed(&r_seed(sb.release.target.triple())),
            &format!("{d:?}"),
        );
    }
}

#[test]
fn the_guard_heals_over_the_network_and_parallel_calls_share_downloads() {
    for d in downloaders("network heal") {
        for sh in bs::shells("network heal") {
            let (sb, path) = served(d, |sb| Mode::Slow(sb.release.archive.clone(), 1000));
            let sb = Arc::new(sb);
            let handles: Vec<_> = (0..5)
                .map(|_| {
                    let (sb, path) = (sb.clone(), path.clone());
                    std::thread::spawn(move || guard_path(&sb, sh, BASH, path))
                })
                .collect();
            let mut allowed = 0;
            for h in handles {
                let run = h.join().unwrap();
                if run.code == Some(0) {
                    assert_allowed(&run, &format!("{d:?} {sh:?}"));
                    allowed += 1;
                } else {
                    assert_block(&run, &rate_limited(), &format!("{d:?} {sh:?}"));
                }
            }
            assert!(allowed >= 1, "{d:?} {sh:?}");
            assert_current(&sb, &fake(V, ALLOW), &format!("{d:?} {sh:?}"));
            let hits = bs::server().hits(&sb.release.url_path());
            assert!(
                (1..5).contains(&hits),
                "{d:?} {sh:?}: {hits} downloads for 5 calls"
            );
        }
    }
}

#[test]
fn the_wrapper_and_session_start_download_too() {
    for d in downloaders("wrapper and session downloads") {
        let sh = first_shell();
        let (sb, path) = served(d, normal);
        let mut spec = sb.spec(sh, SESSION, &[], b"{}");
        spec.path = Some(path.clone());
        let run = sb.run_spec(spec);
        assert_eq!(
            run.out(),
            "{\"systemMessage\":\"dl=1 t0=set\"}\n",
            "{d:?}: {}",
            run.err()
        );
        let (sb, path) = served(d, normal);
        let mut spec = sb.spec(sh, WRAP_MEM, &["search", "q"], b"");
        spec.path = Some(path);
        let run = sb.run_spec(spec);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), "args:mem search q|atl=1\n".into()),
            "{d:?}"
        );
    }
}
