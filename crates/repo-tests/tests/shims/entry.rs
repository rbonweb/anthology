//! Acceptance 7 (the bin/ wrappers) and 8 (session-start.sh: one JSON line, the downloaded
//! flag, GC-7's incomplete message).

use anthology_testkit::bs;

use super::common::*;

#[test]
fn the_wrappers_exec_the_installed_binary_and_follow_symlinks() {
    each_shell("wrappers", |sh| {
        let sb = sandbox();
        sb.install(&fake(V, ALLOW), 0o755);
        let run = sb.run(sh, WRAP, &["setup", "apply", "a b"], b"");
        assert_eq!(
            (run.code, run.out()),
            (Some(0), "args:setup apply a b|rtk=1\n".into()),
            "{sh:?}: {}",
            run.err()
        );
        let run = sb.run(sh, WRAP_MEM, &["search", "x"], b"");
        assert_eq!(
            (run.code, run.out()),
            (Some(0), "args:mem search x|rtk=1\n".into()),
            "{sh:?}"
        );
        // Through an absolute symlink, then a relative symlink to that symlink.
        let links = sb.root.join("links");
        let deeper = links.join("deeper");
        std::fs::create_dir_all(&deeper).unwrap();
        std::os::unix::fs::symlink(sb.root.join(WRAP_MEM), links.join("anthology-mem")).unwrap();
        std::os::unix::fs::symlink("../anthology-mem", deeper.join("mem")).unwrap();
        for via in ["links/anthology-mem", "links/deeper/mem"] {
            let run = sb.run(sh, via, &["stale"], b"");
            assert_eq!(
                (run.code, run.out()),
                (Some(0), "args:mem stale|rtk=1\n".into()),
                "{sh:?} {via}: {}",
                run.err()
            );
        }
        // CDPATH cannot confuse the root lookup.
        let mut spec = sb.spec(sh, "links/deeper/mem", &["x"], b"");
        spec.env.push(("CDPATH".into(), sb.root.clone().into()));
        let run = sb.run_spec(spec);
        assert_eq!(run.out(), "args:mem x|rtk=1\n", "{sh:?} CDPATH");
    });
}

#[test]
fn the_wrappers_install_when_missing_and_exit_127_when_they_cannot() {
    each_shell("wrapper bootstrap", |sh| {
        let sb = sandbox();
        sb.seed();
        let run = sb.run(sh, WRAP, &["doctor"], b"");
        assert_eq!(
            (run.code, run.out()),
            (Some(0), "args:doctor|rtk=1\n".into()),
            "{sh:?}: {}",
            run.err()
        );
        assert_current(&sb, &fake(V, ALLOW), &format!("{sh:?}"));
        // Bootstrap failures: 127 and the GC-14 line.
        let sb = sandbox();
        let path = sb.path_with(DOWNLOADERS, &[]);
        let reason = no_downloader_reason(&sb.release.url());
        for (rel, name) in [(WRAP, "anthology"), (WRAP_MEM, "anthology-mem")] {
            std::fs::remove_file(sb.bindir().join(".attempt")).ok();
            let mut spec = sb.spec(sh, rel, &["x"], b"");
            spec.path = Some(path.clone());
            let run = sb.run_spec(spec);
            assert_eq!(run.code, Some(127), "{sh:?} {name}");
            assert!(run.stdout.is_empty());
            assert_eq!(
                run.err(),
                format!("{}\n", wrapper_failed(name, &reason)),
                "{sh:?} {name}"
            );
        }
        // An invalid lock, and a missing lib.sh.
        let sb = sandbox();
        sb.write_lock("{}\n");
        let run = sb.run(sh, WRAP, &[], b"");
        assert_eq!(
            (run.code, run.err()),
            (
                Some(127),
                format!("{}\n", wrapper_failed("anthology", R_LOCK))
            )
        );
        std::fs::remove_file(sb.root.join(A_LIB)).unwrap();
        let run = sb.run(sh, WRAP_MEM, &[], b"");
        assert_eq!(
            (run.code, run.err()),
            (
                Some(127),
                format!(
                    "{}\n",
                    wrapper_failed("anthology-mem", "hooks/lib.sh did not load")
                )
            )
        );
    });
}

#[test]
fn session_start_sets_the_downloaded_flag_and_prints_one_line() {
    each_shell("session start", |sh| {
        let sb = sandbox();
        sb.seed();
        let input = br#"{"hook_event_name":"SessionStart","cwd":"/tmp"}"#;
        let run = sb.run(sh, SESSION, &[], input);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), "{\"systemMessage\":\"dl=1 t0=set\"}\n".into()),
            "{sh:?}: {}",
            run.err()
        );
        let run = sb.run(sh, SESSION, &[], input);
        assert_eq!(
            (run.code, run.out()),
            (Some(0), "{\"systemMessage\":\"dl=0 t0=set\"}\n".into()),
            "{sh:?}"
        );
        assert!(
            !sb.bindir().join(".attempt").exists(),
            "{sh:?}: session mode"
        );
    });
}

#[test]
fn session_start_passes_the_hook_input_and_reports_a_failed_binary() {
    each_shell("session start failures", |sh| {
        let mut sb = bs::Sandbox::new(release_of(
            V,
            &fake_hook(
                V,
                ALLOW,
                "cat >\"$BS_LOG\"; printf '{\"x\":1}\\n\\n\\n'; exit 0",
            ),
        ));
        let log = sb.root.join("in.log");
        sb.set_env("BS_LOG", log.clone());
        sb.seed();
        let input = b"{\"cwd\":\"/w\"}\n";
        let run = sb.run(sh, SESSION, &[], input);
        assert_eq!(run.out(), "{\"x\":1}\n", "{sh:?}: one line");
        assert_eq!(
            std::fs::read(&log).unwrap(),
            input,
            "{sh:?}: stdin reached it"
        );
        for (body, why) in [
            ("exit 3", "exit 3"),
            ("printf '{\"x\":1}\\n'; exit 4", "exit 4"),
            ("exit 0", "exit 0"),
            ("kill -9 $$", "exit 137"),
        ] {
            let sb = bs::Sandbox::new(release_of(V, &fake_hook(V, ALLOW, body)));
            sb.seed();
            let run = sb.run(sh, SESSION, &[], b"{}");
            assert_eq!(
                (run.code, run.out()),
                (Some(0), format!("{}\n", incomplete(why))),
                "{sh:?} {body}"
            );
        }
        // The bootstrap fails: the anthology systemMessage, no session start.
        let sb = sandbox();
        let mut spec = sb.spec(sh, SESSION, &[], b"{}");
        spec.path = Some(sb.path_with(DOWNLOADERS, &[]));
        let run = sb.run_spec(spec);
        assert_eq!(
            (run.code, run.out()),
            (
                Some(0),
                format!(
                    "{}\n",
                    sys_anthology(&no_downloader_reason(&sb.release.url()))
                )
            ),
            "{sh:?}"
        );
        // lib.sh missing.
        let sb = sandbox();
        std::fs::remove_file(sb.root.join(A_LIB)).unwrap();
        let run = sb.run(sh, SESSION, &[], b"{}");
        assert_eq!(
            (run.code, run.out()),
            (
                Some(0),
                format!("{}\n", incomplete("hooks/lib.sh did not load"))
            )
        );
    });
}
