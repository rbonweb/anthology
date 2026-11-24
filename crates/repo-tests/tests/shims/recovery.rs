//! Acceptance 3: every row of the design §9.2 matrix, under every shell here. Outcomes only
//! (exit 0 or 2, the stderr message, the files), never a shell's intermediate code (m2-9).

use std::sync::Arc;

use anthology_testkit::bs::{self, Run, Sandbox, Shell};

use super::common::*;

/// What the set-aside message must say.
enum Why {
    /// Exactly this reason.
    Exact(&'static str),
    /// `exit N` or `exit N: <line>` for whatever the shell made of it.
    AnyExit,
}

fn assert_set_aside(sb: &Sandbox, run: &Run, why: &Why, ctx: &str) {
    match why {
        Why::Exact(w) => assert_block(run, &set_aside(sb, w), ctx),
        Why::AnyExit => {
            assert_blocked(run, ctx);
            let err = run.err();
            let head = "Blocked by anthology-guardrails (setup): the guard binary did not work (";
            let tail = format!(
                ") and was set aside; it is reinstalled on a later call (at most once a minute){P}{}\n",
                fix_msg(sb)
            );
            assert!(
                err.starts_with(head) && err.ends_with(&tail),
                "{ctx}: {err:?}"
            );
            let reason = &err[head.len()..err.len() - tail.len()];
            let re = regex::Regex::new(r"^exit [0-9]+(: [ -~]{1,120})?$").unwrap();
            assert!(re.is_match(reason), "{ctx}: reason {reason:?}");
            assert_ne!(reason, "exit 0: anthology-guard:allow:v1", "{ctx}");
        }
    }
}

/// One matrix row: `bytes` installed (mode `mode`, with a valid marker), a Bash call. It must
/// quarantine and block, never allow; then a later call heals from the seed and allows.
fn row(name: &str, bytes: &[u8], mode: u32, why: Why) {
    each_shell(name, |sh| row_in(sh, name, bytes, mode, &why));
}

fn row_in(sh: Shell, name: &str, bytes: &[u8], mode: u32, why: &Why) {
    let ctx = format!("{name} under {sh:?}");
    let mut sb = sandbox();
    sb.seed();
    let ran = sb.root.join("ran");
    sb.set_env("BS_RAN", ran.clone());
    sb.install(bytes, mode);
    let run = guard(&sb, sh, BASH);
    assert_set_aside(&sb, &run, why, &ctx);
    assert!(!sb.vdir().exists(), "{ctx}: <version> moved away");
    let b = one_broken(&sb, &ctx);
    let name_re = regex::Regex::new(r"^\.broken-0\.1\.0-[0-9]+-[0-9]+$").unwrap();
    assert!(
        name_re.is_match(&sb.broken()[0]),
        "{ctx}: {:?}",
        sb.broken()
    );
    assert!(
        b.join("anthology").symlink_metadata().is_ok(),
        "{ctx}: the binary is kept aside"
    );
    if matches!(why, Why::Exact("it is empty" | "it is not executable")) {
        assert!(!ran.exists(), "{ctx}: it was run");
    }
    // Heal: the next call reinstalls (no stamp yet in this minute) and the binary allows.
    let healed = guard(&sb, sh, BASH);
    assert_allowed(&healed, &format!("{ctx}: heal"));
    assert_current(&sb, &fake(V, ALLOW), &ctx);
}

/// A `#!/bin/sh` script that records it ran (BS_RAN), then runs `body`.
fn script(body: &str) -> Vec<u8> {
    format!("#!/bin/sh\n: >\"${{BS_RAN:-/dev/null}}\"\ncat >/dev/null\n{body}\n").into_bytes()
}

#[test]
fn matrix_zero_byte_binary_is_never_run() {
    row("0-byte", b"", 0o755, Why::Exact("it is empty"));
}

#[test]
fn matrix_nul_filled_binaries() {
    row("NUL 4 KiB", &[0u8; 4096], 0o755, Why::AnyExit);
    row(
        "NUL 6 MiB",
        &vec![0u8; 6 * 1024 * 1024],
        0o755,
        Why::AnyExit,
    );
    let mixed: Vec<u8> = (0..4096)
        .map(|i| if i % 7 == 6 { b'\n' } else { 0 })
        .collect();
    row("NULs and newlines", &mixed, 0o755, Why::AnyExit);
}

#[test]
fn matrix_whitespace_only_binary() {
    row("whitespace", b"   \n\t  \n\n   ", 0o755, Why::AnyExit);
}

#[test]
fn matrix_empty_scripts() {
    row(
        "#!/bin/sh only",
        b"#!/bin/sh\n",
        0o755,
        Why::Exact("exit 0"),
    );
    row(":", b":\n", 0o755, Why::AnyExit);
    row("exit 0", b"exit 0\n", 0o755, Why::AnyExit);
}

#[test]
fn matrix_random_bytes_and_other_arch() {
    row("random", &bs::random_bytes(4096, 42), 0o755, Why::AnyExit);
    row("other-arch ELF", &bs::other_arch_elf(), 0o755, Why::AnyExit);
}

#[test]
fn matrix_foreign_stderr_and_wrong_tokens() {
    row(
        "exit 2 foreign",
        &script("echo 'something else went wrong' >&2; exit 2"),
        0o755,
        Why::Exact("exit 2: something else went wrong"),
    );
    row(
        "exit 0 empty",
        &script("exit 0"),
        0o755,
        Why::Exact("exit 0"),
    );
    row(
        "exit 0 other",
        &script("echo hello >&2; exit 0"),
        0o755,
        Why::Exact("exit 0: hello"),
    );
    row(
        "token without newline",
        &script("printf 'anthology-guard:allow:v1' >&2; exit 0"),
        0o755,
        Why::Exact("exit 0: anthology-guard:allow:v1"),
    );
    row(
        "token twice the newline",
        &script("printf 'anthology-guard:allow:v1\\n\\n' >&2; exit 0"),
        0o755,
        Why::Exact("exit 0: anthology-guard:allow:v1"),
    );
    row(
        "token plus a line",
        &script("printf 'anthology-guard:allow:v1\\nmore\\n' >&2; exit 0"),
        0o755,
        Why::Exact("exit 0: anthology-guard:allow:v1"),
    );
    // Command substitution drops NULs; a NUL anywhere must still break the token (review C1).
    row(
        "token, NUL, newline",
        &script("printf 'anthology-guard:allow:v1\\000\\n' >&2; exit 0"),
        0o755,
        Why::Exact("exit 0: anthology-guard:allow:v1?"),
    );
    row(
        "NUL, token, newline",
        &script("printf '\\000anthology-guard:allow:v1\\n' >&2; exit 0"),
        0o755,
        Why::Exact("exit 0: ?anthology-guard:allow:v1"),
    );
    row(
        "a NUL inside the token",
        &script("printf 'anthology-guard:\\000allow:v1\\n' >&2; exit 0"),
        0o755,
        Why::Exact("exit 0: anthology-guard:?allow:v1"),
    );
    row(
        "token, newline, NUL",
        &script("printf 'anthology-guard:allow:v1\\n\\000' >&2; exit 0"),
        0o755,
        Why::Exact("exit 0: anthology-guard:allow:v1"),
    );
    row(
        "token on stdout",
        &script("printf 'anthology-guard:allow:v1\\n'; exit 0"),
        0o755,
        Why::Exact("exit 0"),
    );
    row(
        "token with exit 1",
        &script("printf 'anthology-guard:allow:v1\\n' >&2; exit 1"),
        0o755,
        Why::Exact("exit 1: anthology-guard:allow:v1"),
    );
    row(
        "a block with exit 0",
        &script(&format!("printf '%s\\n' '{G1}' >&2; exit 0")),
        0o755,
        Why::AnyExit,
    );
    row(
        "a block with exit 1",
        &script(&format!("printf '%s\\n' '{G1}' >&2; exit 1")),
        0o755,
        Why::AnyExit,
    );
}

#[test]
fn matrix_exit_codes_and_signals() {
    row(
        "panic 101",
        &script("echo \"thread 'main' panicked at src/main.rs:1:1:\" >&2; exit 101"),
        0o755,
        Why::Exact("exit 101: thread 'main' panicked at src/main.rs:1:1:"),
    );
    row(
        "exit 126",
        &script("exit 126"),
        0o755,
        Why::Exact("exit 126"),
    );
    row(
        "SIGKILL",
        &script("kill -9 $$"),
        0o755,
        Why::Exact("exit 137"),
    );
    row(
        "SIGSEGV",
        &script("kill -SEGV $$"),
        0o755,
        Why::Exact("exit 139"),
    );
}

#[test]
fn matrix_stderr_is_cleaned_and_cut() {
    let long = "y".repeat(300);
    row(
        "non-printable",
        &script("printf 'bad\\001\\302\\251 x\\tz\\nsecond line\\n' >&2; exit 3"),
        0o755,
        Why::Exact("exit 3: bad??? x?z"),
    );
    let want: &'static str = Box::leak(format!("exit 4: {}", &long[..120]).into_boxed_str());
    row(
        "long",
        &script(&format!("echo {long} >&2; exit 4")),
        0o755,
        Why::Exact(want),
    );
}

#[test]
fn matrix_not_executable_or_not_a_file_is_never_run() {
    row(
        "mode 0644",
        &script("printf 'anthology-guard:allow:v1\\n' >&2"),
        0o644,
        Why::Exact("it is not executable"),
    );
    each_shell("symlinked binary", |sh| {
        let mut sb = sandbox();
        sb.seed();
        let ran = sb.root.join("ran");
        sb.set_env("BS_RAN", ran.clone());
        let target = sb.root.join("real-anthology");
        std::fs::write(&target, fake(V, ALLOW)).unwrap();
        chmod(&target, 0o755);
        sb.install(b"x", 0o755);
        std::fs::remove_file(sb.bin()).unwrap();
        std::os::unix::fs::symlink(&target, sb.bin()).unwrap();
        assert_block(
            &guard(&sb, sh, BASH),
            &set_aside(&sb, "it is not executable"),
            &format!("{sh:?}"),
        );
        one_broken(&sb, "symlink");
        // A directory where the binary should be.
        let sb = sandbox();
        sb.install(b"x", 0o755);
        std::fs::remove_file(sb.bin()).unwrap();
        std::fs::create_dir(sb.bin()).unwrap();
        assert_block(
            &guard(&sb, sh, BASH),
            &set_aside(&sb, "it is not executable"),
            &format!("{sh:?} dir"),
        );
    });
}

#[test]
fn a_real_block_passes_through_without_quarantine() {
    each_shell("pass-through", |sh| {
        for body in [
            BLOCK.to_string(),
            format!("cat >/dev/null; printf '%s\\n\\n\\n' '{G1}' >&2; exit 2"),
            format!("cat >/dev/null; printf '%s' '{G1}' >&2; exit 2"),
        ] {
            let sb = sandbox();
            sb.install(&fake(V, &body), 0o755);
            let run = guard(&sb, sh, BASH);
            assert_eq!(run.err(), format!("{G1}\n"), "{sh:?}: exactly one newline");
            assert_eq!(run.code, Some(2));
            assert!(sb.broken().is_empty(), "{sh:?}: no quarantine");
            assert!(sb.bin().is_file());
        }
    });
}

#[test]
fn the_allow_token_allows_on_the_hot_path_with_stdin_inherited() {
    each_shell("hot path", |sh| {
        let mut sb = sandbox();
        let log = sb.root.join("stdin.log");
        sb.set_env("BS_LOG", log.clone());
        sb.install(&fake(V, ALLOW_LOG), 0o755);
        let run = guard(&sb, sh, BASH);
        assert_allowed(&run, &format!("{sh:?}"));
        assert_eq!(
            std::fs::read(&log).unwrap(),
            BASH,
            "{sh:?}: the payload reached it"
        );
        assert!(
            std::fs::read_dir(&sb.tmpdir).unwrap().next().is_none(),
            "{sh:?}: no temp file on the hot path"
        );
        // Read-only tools also go to the binary when it is installed.
        assert_allowed(&guard(&sb, sh, READ), &format!("{sh:?} read"));
    });
}

#[test]
fn a_hanging_binary_never_allows() {
    each_shell("hang", |sh| {
        let sb = sandbox();
        sb.install(&fake(V, "exec sleep 600"), 0o755);
        let mut spec = sb.spec(sh, GUARD, &[], BASH);
        spec.timeout = std::time::Duration::from_secs(3);
        let run = sb.run_spec(spec);
        // Claude Code's 60 s timeout and onFailure block take over (design §9.2, m-R6).
        assert!(run.timed_out, "{sh:?}: {run:?}");
        assert_ne!(run.code, Some(0), "{sh:?}");
    });
}

#[test]
fn parallel_calls_on_one_broken_binary_say_set_aside_never_not_writable() {
    each_shell("parallel quarantine", |sh| {
        let mut sb = sandbox();
        sb.set_env("ANTHOLOGY_GUARD_NO_FETCH", "1");
        sb.install(&script("sleep 1; echo broken >&2; exit 3"), 0o755);
        let sb = Arc::new(sb);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let sb = sb.clone();
                std::thread::spawn(move || guard(&sb, sh, BASH))
            })
            .collect();
        let mut set = 0;
        for h in handles {
            let run = h.join().unwrap();
            let err = run.err();
            assert!(!err.contains("could not be set aside"), "{sh:?}: {err}");
            if err == format!("{}\n", set_aside(&sb, "exit 3: broken")) {
                set += 1;
            } else {
                // A call that started after the quarantine finds nothing installed.
                assert_block(
                    &run,
                    &not_installed(&sb, "not attempted"),
                    &format!("{sh:?}"),
                );
            }
            assert_eq!(run.code, Some(2));
        }
        assert!(set >= 1, "{sh:?}");
        let b = one_broken(&sb, &format!("{sh:?}"));
        assert!(b.join("anthology").is_file(), "{sh:?}: not nested");
    });
}

#[test]
fn three_strikes_are_counted_by_the_name_epoch() {
    each_shell("strike epochs", |sh| {
        let n = now();
        // Two recent strikes (one with an ancient mtime: only the name counts) + this one.
        let sb = sandbox();
        strike(&sb, V, n - 10, "11");
        let old_mtime = strike(&sb, V, n - 20, "12");
        age(&old_mtime, 5 * 86_400);
        sb.install(&script("exit 5"), 0o755);
        assert_block(
            &guard(&sb, sh, BASH),
            &three_strikes(V),
            &format!("{sh:?} third"),
        );
        // Missing and capped: no bootstrap at all.
        assert_block(
            &guard(&sb, sh, BASH),
            &three_strikes(V),
            &format!("{sh:?} capped"),
        );
        assert!(!sb.bindir().join(".attempt").exists(), "{sh:?}: no attempt");
        // Old, foreign and malformed names do not count.
        let sb = sandbox();
        sb.seed();
        strike(&sb, V, n - 90_000, "1");
        strike(&sb, V, n - 86_401, "2");
        strike(&sb, "0.1.0-rc.1", n - 5, "3");
        strike(&sb, "0.2.0", n - 5, "4");
        std::fs::create_dir_all(sb.bindir().join(format!(".broken-{V}-notanepoch-5"))).unwrap();
        std::fs::create_dir_all(sb.bindir().join(format!(".broken-{V}-{}", n - 5))).unwrap();
        std::fs::write(
            sb.bindir().join(format!(".broken-{V}-{}-6", n - 5)),
            b"file",
        )
        .unwrap();
        // A future epoch, leading zeros (an octal error would end the shell: review C2), and a
        // prerelease's quarantines, which the version's glob also matches.
        strike(&sb, V, n + 200_000, "8");
        for odd in [
            format!(".broken-{V}-08-1"),
            format!(".broken-{V}-0089-1"),
            format!(".broken-{V}-0{}-1", n - 5),
            format!(".broken-{V}-{}-007", n - 5),
            format!(".broken-{V}-{}-7-0", n - 5),
            format!(".broken-{V}-08-{}-9", n - 5),
            format!(".broken-{V}-5-{}-10", n - 5),
            format!(".broken-{V}-{}--1", n - 5),
        ] {
            std::fs::create_dir_all(sb.bindir().join(odd)).unwrap();
        }
        strike(&sb, V, n - 30, "7");
        // Missing, with all of them present: a GC-14 block, never a shell error.
        let mut spec = sb.spec(sh, GUARD, &[], BASH);
        spec.env
            .push(("ANTHOLOGY_GUARD_NO_FETCH".into(), "1".into()));
        assert_block(
            &sb.run_spec(spec),
            &not_installed(&sb, "not attempted"),
            &format!("{sh:?} odd names, missing"),
        );
        sb.install(&script("exit 5"), 0o755);
        assert_block(
            &guard(&sb, sh, BASH),
            &set_aside(&sb, "exit 5"),
            &format!("{sh:?} one plus one"),
        );
        // The third quarantine uses the three-strikes text.
        sb.install(&script("exit 5"), 0o755);
        assert_block(
            &guard(&sb, sh, BASH),
            &three_strikes(V),
            &format!("{sh:?} third"),
        );
    });
}

#[test]
fn two_quarantines_in_the_same_second_do_not_nest() {
    each_shell("no nesting", |sh| {
        let sb = sandbox();
        let harness = sb.root.join("q.sh");
        std::fs::write(
            &harness,
            format!(
                ". '{lib}'\nanth_resolve '{root}' || exit 9\n\
                 for i in 1 2 3; do mkdir -p \"$ANTH_VDIR\" && : >\"$ANTH_BIN\" && anth_quarantine && echo \"$ANTH_QUAR\"; done\n",
                lib = sb.root.join(G_LIB).display(),
                root = sb.plugin("anthology-guardrails").display()
            ),
        )
        .unwrap();
        let stub = sb.path_with(&["date"], &[("date", "#!/bin/sh\necho 1700000000\n")]);
        std::fs::create_dir_all(sb.bindir()).unwrap();
        // The 2nd and 3rd find a same-named directory there and never nest into it.
        let mut spec = sb.spec(sh, "q.sh", &[], b"");
        spec.path = Some(stub);
        let run = sb.run_spec(spec);
        assert_eq!(run.out(), "moved\nmoved\nmoved\n", "{sh:?}: {}", run.err());
        let b = sb.broken();
        assert_eq!(b.len(), 3, "{sh:?}: {b:?}");
        let pid = b
            .iter()
            .find(|n| n.matches('-').count() == 3)
            .unwrap()
            .rsplit('-')
            .next()
            .unwrap()
            .to_string();
        for n in &b {
            assert!(
                n.starts_with(&format!(".broken-{V}-1700000000-{pid}")),
                "{sh:?}: {n}"
            );
            let d = sb.bindir().join(n);
            let inner: Vec<_> = std::fs::read_dir(&d)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name())
                .collect();
            assert_eq!(
                inner,
                vec![std::ffi::OsString::from("anthology")],
                "{sh:?}: {n} not nested"
            );
        }
        assert!(b.contains(&format!(".broken-{V}-1700000000-{pid}-2")));
        assert!(b.contains(&format!(".broken-{V}-1700000000-{pid}-3")));
    });
}

#[test]
fn a_failed_quarantine_says_not_set_aside_and_does_not_loop() {
    if !bs::capability("nonroot", !bs::is_root(), "failed quarantine") {
        return;
    }
    each_shell("failed quarantine", |sh| {
        let sb = sandbox();
        sb.install(&script("exit 7"), 0o755);
        chmod(&sb.bindir(), 0o555);
        let first = guard(&sb, sh, BASH);
        let second = guard(&sb, sh, BASH);
        chmod(&sb.bindir(), 0o755);
        let msg = not_set_aside("exit 7", &sb.bindir());
        assert_block(&first, &msg, &format!("{sh:?}"));
        assert_block(&second, &msg, &format!("{sh:?} again"));
        assert!(sb.bin().is_file() && sb.broken().is_empty());
    });
}

#[test]
fn a_stale_attempt_in_an_unwritable_bin_is_cannot_write() {
    if !bs::capability("nonroot", !bs::is_root(), "stale attempt unwritable") {
        return;
    }
    each_shell("stale attempt", |sh| {
        let sb = sandbox();
        std::fs::create_dir_all(sb.bindir()).unwrap();
        let p = sb.bindir().join(".attempt");
        std::fs::write(&p, b"").unwrap();
        age(&p, 600);
        chmod(&sb.bindir(), 0o555);
        let run = guard(&sb, sh, BASH);
        chmod(&sb.bindir(), 0o755);
        assert_block(&run, &cannot_write(&sb.bindir()), &format!("{sh:?}"));
    });
}

#[test]
fn faults_end_in_a_block() {
    each_shell("fault injection", |sh| {
        // mktemp fails: the stdin buffer cannot be made.
        let sb = sandbox();
        let path = sb.path_with(&["mktemp"], &[("mktemp", "#!/bin/sh\nexit 1\n")]);
        let run = guard_path(&sb, sh, READ, path.clone());
        assert_block(&run, FALLBACK, &format!("{sh:?} mktemp read"));
        assert_block(
            &guard_path(&sb, sh, BASH, path),
            FALLBACK,
            &format!("{sh:?} mktemp"),
        );
        // mktemp "succeeds" without a file.
        let path = sb.path_with(
            &["mktemp"],
            &[("mktemp", "#!/bin/sh\necho /nonexistent/x\n")],
        );
        assert_block(
            &guard_path(&sb, sh, BASH, path),
            FALLBACK,
            &format!("{sh:?} mktemp lie"),
        );
        // TMPDIR is not writable.
        if !bs::is_root() {
            let mut sb = sandbox();
            let ro = sb.root.join("ro");
            std::fs::create_dir(&ro).unwrap();
            chmod(&ro, 0o555);
            sb.set_env("TMPDIR", ro.clone());
            assert_block(&guard(&sb, sh, BASH), FALLBACK, &format!("{sh:?} TMPDIR"));
            chmod(&ro, 0o755);
        }
        // Odd bootstrap output.
        for (i, out) in [
            "",
            "garbage",
            "status=present",
            "status=installed",
            "status=present\\nstatus=present",
            "status=installed extra",
            "status=failed:",
            "status=",
            "STATUS=present",
        ]
        .iter()
        .enumerate()
        {
            let sb = sandbox();
            patch_lib(
                &sb,
                &format!("anth_bootstrap_main() {{ printf '{out}\\n'; return 0; }}"),
            );
            let run = guard(&sb, sh, BASH);
            assert_block(
                &run,
                FALLBACK,
                &format!("{sh:?} bootstrap output {i} {out:?}"),
            );
        }
        // "present" while the binary is empty: never run.
        let sb = sandbox();
        sb.install(b"", 0o755);
        std::fs::remove_file(sb.vdir().join(".sha256")).unwrap();
        patch_lib(&sb, "anth_bootstrap_main() { echo status=present; }");
        assert_block(
            &guard(&sb, sh, BASH),
            &set_aside(&sb, "it is empty"),
            &format!("{sh:?} empty"),
        );
        // lib.sh missing, unparsable, or failing as it loads.
        for (i, lib) in [
            None,
            Some("if then fi ((\n"),
            Some("return 1\n"),
            Some("anth_x() {\n"),
        ]
        .iter()
        .enumerate()
        {
            let sb = sandbox();
            sb.install(&fake(V, ALLOW), 0o755);
            let p = sb.root.join(G_LIB);
            match lib {
                None => std::fs::remove_file(&p).unwrap(),
                Some(t) => std::fs::write(&p, t).unwrap(),
            }
            assert_block(&guard(&sb, sh, BASH), FALLBACK, &format!("{sh:?} lib {i}"));
            assert_block(
                &guard(&sb, sh, READ),
                FALLBACK,
                &format!("{sh:?} lib {i} read"),
            );
        }
        // An environment that tries to pre-set the shim's variables.
        let mut sb = sandbox();
        for (k, v) in [
            ("ANTH_RUN", "1"),
            ("ANTH_STATE", "hot"),
            ("ANTH_RC", "0"),
            ("ANTH_MSG", "x"),
            ("anth_call", "1"),
            ("anth_input", "/nonexistent-bs-input"),
        ] {
            sb.set_env(k, v);
        }
        sb.set_env("ANTHOLOGY_GUARD_NO_FETCH", "1");
        assert_block(
            &guard(&sb, sh, BASH),
            &not_installed(&sb, "not attempted"),
            &format!("{sh:?} env"),
        );
    });
}
