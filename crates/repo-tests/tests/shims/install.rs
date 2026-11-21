//! Acceptance 1 (install) and 9 (`--quiet` lines and exit codes, GC-5).

use std::sync::Arc;

use anthology_testkit::bs::{self, Member, Release, Sandbox};

use super::common::*;

#[test]
fn a_seeded_archive_installs_with_bin_sha256_before_sha256() {
    each_shell("seeded install", |sh| {
        let sb = sandbox();
        sb.seed();
        // Record every mv destination (fault-injection seam on a scratch copy, T2).
        let log = sb.root.join("mv.log");
        patch_lib(
            &sb,
            &format!(
                "mv() {{ for _x in \"$@\"; do :; done; printf '%s\\n' \"$_x\" >>'{}'; command mv \"$@\"; }}",
                log.display()
            ),
        );
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
        assert_status(&run, "status=installed", &format!("{sh:?}"));
        assert_current(&sb, &fake(V, ALLOW), &format!("{sh:?}"));
        let order: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .filter(|l| l.starts_with(&sb.vdir().display().to_string()))
            .map(|l| l.rsplit('/').next().unwrap().to_string())
            .collect();
        assert_eq!(
            order,
            ["anthology", ".bin-sha256", ".sha256"],
            "{sh:?}: placement order"
        );
        // The seed stays.
        assert!(sb.bindir().join(sb.release.asset()).is_file());
        // Again: present, and .last-used is touched.
        let again = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
        assert_status(&again, "status=present", &format!("{sh:?} again"));
        assert!(sb.vdir().join(".last-used").is_file(), "{sh:?}: .last-used");
    });
}

/// Probe failures: each is a strike (`.broken-…` holding the staged binary), never an install.
fn probe_fails(version_line: Option<&str>, binary: Option<Vec<u8>>, ctx: &str) {
    each_shell(ctx, |sh| {
        let bin = binary
            .clone()
            .unwrap_or_else(|| bs::fake_binary(version_line.unwrap(), ALLOW, HOOK_DL));
        let sb = Sandbox::new(release_of(V, &bin));
        sb.seed();
        let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
        let c = format!("{ctx} {sh:?}");
        assert_status(&run, &format!("status=failed:{R_PROBE}"), &c);
        assert_not_installed(&sb, &c);
        let b = one_broken(&sb, &c);
        assert_eq!(
            std::fs::read(b.join("anthology")).unwrap(),
            bin,
            "{c}: the staged binary"
        );
        // The guard shim names it.
        let sb2 = Sandbox::new(release_of(V, &bin));
        sb2.seed();
        assert_block(&guard(&sb2, sh, BASH), &probe_failed(V), &c);
        assert_eq!(sb2.broken().len(), 1, "{c}: a strike");
    });
}

#[test]
fn the_version_probe_gates_the_install_wrong_version() {
    probe_fails(
        Some("anthology 9.9.9 (x86_64-unknown-linux-musl)"),
        None,
        "wrong version",
    );
}

#[test]
fn the_version_probe_gates_the_install_wrong_target() {
    let other = if bs::host_target().triple() == "aarch64-apple-darwin" {
        "x86_64-apple-darwin"
    } else {
        "aarch64-apple-darwin"
    };
    probe_fails(
        Some(&format!("anthology {V} ({other})")),
        None,
        "wrong target",
    );
}

#[test]
fn the_version_probe_gates_the_install_failing_exit_and_extra_output() {
    let line = version_line(V);
    for (name, script) in [
        (
            "exit 1",
            format!("#!/bin/sh\nprintf '%s\\n' '{line}'\nexit 1\n"),
        ),
        (
            "two lines",
            format!("#!/bin/sh\nprintf '%s\\nmore\\n' '{line}'\n"),
        ),
        ("no newline", format!("#!/bin/sh\nprintf '%s' '{line}'\n")),
        (
            "on stderr",
            format!("#!/bin/sh\nprintf '%s\\n' '{line}' >&2\n"),
        ),
        ("empty script", "#!/bin/sh\n".to_string()),
    ] {
        probe_fails(None, Some(script.into_bytes()), name);
    }
}

#[test]
fn the_version_probe_gates_the_install_other_arch_and_garbage() {
    probe_fails(None, Some(bs::other_arch_elf()), "other-arch ELF");
    probe_fails(None, Some(vec![0u8; 4096]), "NUL-filled");
    probe_fails(None, Some(b"   \n\t\n".to_vec()), "whitespace-only");
    probe_fails(None, Some(bs::random_bytes(4096, 7)), "random bytes");
}

#[test]
fn a_hanging_probe_is_killed_after_five_seconds() {
    let sh = bs::shells("hanging probe")[0];
    let bin = b"#!/bin/sh\nexec sleep 60\n".to_vec();
    let sb = Sandbox::new(release_of(V, &bin));
    sb.seed();
    let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
    assert_status(&run, &format!("status=failed:{R_PROBE}"), "hang");
    assert!(
        run.elapsed.as_secs_f64() < 9.0,
        "the probe watchdog: {:?}",
        run.elapsed
    );
    assert_eq!(sb.broken().len(), 1);
}

#[test]
fn concurrent_installs_stay_consistent() {
    each_shell("concurrent installs", |sh| {
        let sb = Arc::new(sandbox());
        sb.seed();
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let sb = sb.clone();
                std::thread::spawn(move || {
                    boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"])
                })
            })
            .collect();
        for h in handles {
            let run = h.join().unwrap();
            let out = run.out();
            assert!(
                out == "status=installed\n" || out == "status=present\n",
                "{sh:?}: {out:?} {}",
                run.err()
            );
            assert_eq!(run.code, Some(0));
        }
        assert_current(&sb, &fake(V, ALLOW), &format!("{sh:?}"));
        assert!(sb.broken().is_empty(), "{sh:?}: {:?}", sb.broken());
    });
}

#[test]
fn parallel_guard_calls_never_give_a_wrong_verdict_or_a_corrupt_install() {
    each_shell("parallel guard calls", |sh| {
        let sb = Arc::new(sandbox());
        sb.seed();
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let sb = sb.clone();
                std::thread::spawn(move || guard(&sb, sh, BASH))
            })
            .collect();
        let mut allowed = 0;
        for h in handles {
            let run = h.join().unwrap();
            if run.code == Some(0) {
                assert_allowed(&run, &format!("{sh:?}"));
                allowed += 1;
            } else {
                // Only a race loser may see the minute's stamp.
                assert_block(&run, &rate_limited(), &format!("{sh:?}"));
            }
        }
        assert!(allowed >= 1, "{sh:?}: someone installed");
        assert_current(&sb, &fake(V, ALLOW), &format!("{sh:?}"));
    });
}

#[test]
fn session_mode_never_touches_attempt() {
    each_shell("session and .attempt", |sh| {
        // No stamp: none is created.
        let sb = sandbox();
        sb.seed();
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
        assert_status(&run, "status=installed", &format!("{sh:?}"));
        assert!(
            !sb.bindir().join(".attempt").exists(),
            "{sh:?}: no .attempt"
        );
        // A fresh stamp neither stops session mode nor changes.
        for secs in [5, 3600] {
            let sb = sandbox();
            sb.seed();
            let stamp = sb.bindir().join(".attempt");
            std::fs::write(&stamp, b"").unwrap();
            age(&stamp, secs);
            let before = mtime(&stamp);
            let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
            assert_status(&run, "status=installed", &format!("{sh:?} {secs}"));
            assert_eq!(mtime(&stamp), before, "{sh:?}: .attempt untouched");
            assert!(
                !sb.bin_entries().iter().any(|n| n.starts_with(".attempt.")),
                "{sh:?}: not retired"
            );
        }
    });
}

#[test]
fn session_mode_honours_the_cap_except_once_an_hour() {
    each_shell("session cap", |sh| {
        let sb = sandbox();
        sb.seed();
        let n = now();
        for i in 0..3 {
            strike(&sb, V, n - 100 - i, "1");
        }
        // Guard mode is capped.
        let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
        assert_status(
            &run,
            "status=failed:three-strikes",
            &format!("{sh:?} guard"),
        );
        assert!(!sb.bindir().join(".attempt").exists());
        // Session mode: one fresh attempt, recorded in .session-retry.
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
        assert_status(&run, "status=installed", &format!("{sh:?} first"));
        let stamp = sb.bindir().join(".session-retry");
        assert!(stamp.is_file(), "{sh:?}: .session-retry");
        assert!(
            !sb.bindir().join(".attempt").exists(),
            "{sh:?}: no .attempt"
        );
        // Within the hour: capped (the install is gone again).
        std::fs::remove_dir_all(sb.vdir()).unwrap();
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
        assert_status(
            &run,
            "status=failed:three-strikes",
            &format!("{sh:?} second"),
        );
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails"]);
        assert_eq!(run.out(), format!("{}\n", sys_guardrails("three-strikes")));
        assert_eq!(run.code, Some(0));
        // A stale stamp is retired and the hour's attempt runs again.
        age(&stamp, 3700);
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
        assert_status(&run, "status=installed", &format!("{sh:?} after the hour"));
        assert!(
            stamp.is_file()
                && mtime(&stamp)
                    > std::time::SystemTime::now() - std::time::Duration::from_secs(60)
        );
        assert!(
            !sb.bin_entries()
                .iter()
                .any(|n| n.starts_with(".session-retry."))
        );
        assert!(!sb.bindir().join(".attempt").exists());
    });
}

#[test]
fn probe_failures_count_as_strikes_until_the_cap() {
    each_shell("probe strikes", |sh| {
        let bin = bs::other_arch_elf();
        let sb = Sandbox::new(release_of(V, &bin));
        sb.seed();
        for i in 0..3 {
            let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
            assert_status(
                &run,
                &format!("status=failed:{R_PROBE}"),
                &format!("{sh:?} {i}"),
            );
            assert_eq!(sb.broken().len(), i + 1, "{sh:?}: strike {i}");
        }
        assert_block(&guard(&sb, sh, BASH), &three_strikes(V), &format!("{sh:?}"));
        let run = boot(&sb, sh, &["wrapper", "--plugin", "anthology", "--quiet"]);
        assert_status(
            &run,
            "status=failed:three-strikes",
            &format!("{sh:?} wrapper"),
        );
    });
}

#[test]
fn a_missing_bin_folder_is_created() {
    each_shell("missing bin", |sh| {
        let sb = sandbox();
        assert!(!sb.ah().exists());
        let path = sb.path_with(DOWNLOADERS, &[]);
        let run = boot_path(
            &sb,
            sh,
            &["guard", "--plugin", "guardrails", "--quiet"],
            path,
        );
        assert_status(
            &run,
            &format!("status=failed:{}", no_downloader_reason(&sb.release.url())),
            &format!("{sh:?}"),
        );
        assert!(sb.bindir().is_dir(), "{sh:?}: <ah>/bin created");
    });
}

#[test]
fn an_unwritable_bin_folder_is_cannot_write_even_with_a_stale_attempt() {
    if !bs::capability("nonroot", !bs::is_root(), "unwritable bin") {
        return;
    }
    each_shell("unwritable bin", |sh| {
        for stamp in [None, Some(3600u64), Some(5)] {
            let sb = sandbox();
            std::fs::create_dir_all(sb.bindir()).unwrap();
            if let Some(secs) = stamp {
                let p = sb.bindir().join(".attempt");
                std::fs::write(&p, b"").unwrap();
                age(&p, secs);
            }
            chmod(&sb.bindir(), 0o555);
            let c = format!("{sh:?} stamp {stamp:?}");
            let guard_run = guard(&sb, sh, BASH);
            let boot_run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
            let session_run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
            chmod(&sb.bindir(), 0o755);
            if stamp == Some(5) {
                // A fresh stamp is the minute's rate limit, as designed (step 5.2 first).
                assert_status(&boot_run, "status=failed:rate-limited", &c);
                assert_block(&guard_run, &rate_limited(), &c);
            } else {
                assert_status(&boot_run, &format!("status=failed:{R_WRITE}"), &c);
                assert_block(&guard_run, &cannot_write(&sb.bindir()), &c);
            }
            assert_status(&session_run, &format!("status=failed:{R_WRITE}"), &c);
        }
    });
}

#[test]
fn a_symlinked_bin_folder_is_refused() {
    each_shell("symlinked bin", |sh| {
        let sb = sandbox();
        let real = sb.root.join("elsewhere");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir_all(sb.ah()).unwrap();
        std::os::unix::fs::symlink(&real, sb.bindir()).unwrap();
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
        assert_status(
            &run,
            &format!("status=failed:{R_WRITE}"),
            &format!("{sh:?}"),
        );
        assert!(
            std::fs::read_dir(&real).unwrap().next().is_none(),
            "nothing written there"
        );
    });
}

#[test]
fn a_changed_install_is_quarantined_and_reinstalled() {
    each_shell("not current", |sh| {
        // .sha256 names another archive; then the binary's own hash differs.
        for case in ["marker", "binary"] {
            let sb = sandbox();
            sb.seed();
            sb.install(&fake(V, ALLOW), 0o755);
            if case == "marker" {
                std::fs::write(sb.vdir().join(".sha256"), format!("{:064x}\n", 1)).unwrap();
            } else {
                std::fs::write(sb.bin(), fake(V, BLOCK)).unwrap();
            }
            let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
            assert_status(&run, "status=installed", &format!("{sh:?} {case}"));
            assert_current(&sb, &fake(V, ALLOW), &format!("{sh:?} {case}"));
            assert_eq!(sb.broken().len(), 1, "{sh:?} {case}: quarantined");
        }
        // A partial install (no .sha256) is installed over in place, without a strike.
        let sb = sandbox();
        sb.seed();
        std::fs::create_dir_all(sb.vdir()).unwrap();
        std::fs::write(sb.bin(), b"partial").unwrap();
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails", "--quiet"]);
        assert_status(&run, "status=installed", &format!("{sh:?} partial"));
        assert_current(&sb, &fake(V, ALLOW), &format!("{sh:?} partial"));
        assert!(sb.broken().is_empty(), "{sh:?} partial: no strike");
    });
}

#[test]
fn archive_member_failures_name_the_missing_binary() {
    let members: Vec<(&str, Vec<Member>)> = vec![
        (
            "no member",
            vec![Member::File {
                name: "LICENSE".into(),
                mode: 0o644,
                data: b"l".to_vec(),
            }],
        ),
        (
            "symlink",
            vec![Member::Symlink {
                name: "anthology".into(),
                target: "/bin/sh".into(),
            }],
        ),
        (
            "directory",
            vec![Member::Dir {
                name: "anthology".into(),
            }],
        ),
        (
            "empty",
            vec![Member::File {
                name: "anthology".into(),
                mode: 0o755,
                data: vec![],
            }],
        ),
        (
            "nested",
            vec![Member::File {
                name: "x/anthology".into(),
                mode: 0o755,
                data: fake(V, ALLOW),
            }],
        ),
    ];
    each_shell("archive members", |sh| {
        for (name, m) in &members {
            let sb = Sandbox::new(Release::new(V, bs::tar_gz(m)));
            sb.seed();
            let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
            assert_status(
                &run,
                &format!("status=failed:{R_MEMBER}"),
                &format!("{sh:?} {name}"),
            );
            assert_not_installed(&sb, name);
            assert!(sb.broken().is_empty(), "{name}: not a strike");
        }
        // Not gzip at all.
        let sb = Sandbox::new(Release::new(V, b"not an archive".to_vec()));
        sb.seed();
        let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
        assert_status(
            &run,
            &format!("status=failed:{R_MEMBER}"),
            &format!("{sh:?} garbage"),
        );
    });
}

#[test]
fn no_sha256_tool_is_named() {
    each_shell("no sha tool", |sh| {
        let sb = sandbox();
        sb.seed();
        let path = sb.path_with(&["sha256sum", "shasum", "openssl"], &[]);
        let run = boot_path(
            &sb,
            sh,
            &["guard", "--plugin", "guardrails", "--quiet"],
            path.clone(),
        );
        assert_status(
            &run,
            &format!("status=failed:{R_SHA_TOOL}"),
            &format!("{sh:?}"),
        );
        let sb = sandbox();
        sb.seed();
        assert_block(
            &guard_path(&sb, sh, BASH, path),
            &not_installed(R_SHA_TOOL),
            &format!("{sh:?}"),
        );
    });
}

#[test]
fn a_stale_seed_is_kept_and_named_only_when_the_network_also_fails() {
    each_shell("stale seed offline", |sh| {
        let sb = sandbox();
        sb.seed_bytes(b"stale bytes");
        let path = sb.path_with(DOWNLOADERS, &[]);
        let run = boot_path(
            &sb,
            sh,
            &["guard", "--plugin", "guardrails", "--quiet"],
            path,
        );
        assert_status(
            &run,
            &format!("status=failed:{}", r_seed(sb.release.target.triple())),
            &format!("{sh:?}"),
        );
        assert_eq!(
            std::fs::read(sb.bindir().join(sb.release.asset())).unwrap(),
            b"stale bytes",
            "kept"
        );
    });
}

#[test]
fn quiet_lines_and_exit_codes_follow_gc5() {
    each_shell("quiet and exit codes", |sh| {
        let sb = sandbox();
        sb.seed();
        for args in [
            vec![],
            vec!["session"],
            vec!["bogus", "--plugin", "guardrails"],
            vec!["guard", "--plugin", "other"],
            vec!["guard", "--plugin"],
            vec!["guard", "--plugin", "guardrails", "--budget", "0"],
            vec!["guard", "--plugin", "guardrails", "--budget", "x"],
            vec!["guard", "--plugin", "guardrails", "--extra"],
        ] {
            let run = boot(&sb, sh, &args);
            assert_eq!(run.code, Some(2), "{sh:?} {args:?}: usage");
            assert!(run.stdout.is_empty());
            assert!(
                run.err().starts_with("usage: bootstrap.sh"),
                "{sh:?} {args:?}"
            );
        }
        assert!(
            !sb.bindir().join("0.1.0").exists(),
            "usage errors install nothing"
        );
        // Non-quiet session: silent success.
        let run = boot(&sb, sh, &["session", "--plugin", "guardrails"]);
        assert_eq!((run.code, run.out()), (Some(0), String::new()), "{sh:?}");
        let run = sb.run(
            sh,
            A_BOOT,
            &["session", "--plugin", "anthology", "--budget", "7"],
            b"",
        );
        assert_eq!((run.code, run.out()), (Some(0), String::new()), "{sh:?}");
        let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
        assert_status(&run, "status=present", &format!("{sh:?}"));
        // Without --quiet, guard and wrapper modes still print the status line.
        let run = boot(&sb, sh, &["wrapper", "--plugin", "anthology"]);
        assert_status(&run, "status=present", &format!("{sh:?}"));
        // Failures: the plugin's systemMessage and exit 0, or the line and exit 1.
        let sb = sandbox();
        let path = sb.path_with(DOWNLOADERS, &[]);
        let r = no_downloader_reason(&sb.release.url());
        let mut spec = sb.spec(sh, G_BOOT, &["session", "--plugin", "guardrails"], b"");
        spec.path = Some(path.clone());
        let run = sb.run_spec(spec);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), format!("{}\n", sys_guardrails(&r)))
        );
        let mut spec = sb.spec(sh, A_BOOT, &["session", "--plugin", "anthology"], b"");
        spec.path = Some(path.clone());
        let run = sb.run_spec(spec);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), format!("{}\n", sys_anthology(&r)))
        );
        let run = boot_path(
            &sb,
            sh,
            &["session", "--plugin", "guardrails", "--quiet"],
            path,
        );
        assert_status(&run, &format!("status=failed:{r}"), &format!("{sh:?}"));
        assert!(run.stderr.is_empty(), "{sh:?}: quiet stderr {}", run.err());
    });
}
