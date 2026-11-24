//! Acceptance 4 (read-only tools with an invalid lock or an unsupported platform), 5 (self-heal:
//! rate-limited, NO_FETCH, the buffered payload, the same classification) and the "absent"
//! and lock and uname cases of acceptance 2.

use anthology_testkit::bs::{self, Sandbox};

use super::common::*;

/// Every GC-17 field broken, one at a time, plus layout faults.
fn bad_locks(good: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    let mut sub = |name: &str, from: &str, to: &str| {
        assert!(good.contains(from), "{from}");
        v.push((name.to_string(), good.replacen(from, to, 1)));
    };
    sub(
        "version not semver",
        "\"version\": \"0.1.0\"",
        "\"version\": \"0.1\"",
    );
    sub(
        "version with v",
        "\"version\": \"0.1.0\"",
        "\"version\": \"v0.1.0\"",
    );
    sub(
        "version bad prerelease",
        "\"version\": \"0.1.0\"",
        "\"version\": \"0.1.0-rc_1\"",
    );
    sub(
        "version empty prerelease",
        "\"version\": \"0.1.0\"",
        "\"version\": \"0.1.0-\"",
    );
    sub(
        "version four parts",
        "\"version\": \"0.1.0\"",
        "\"version\": \"0.1.0.1\"",
    );
    sub(
        "tag mismatch",
        "anthology-bin--v0.1.0",
        "anthology-bin--v0.1.1",
    );
    sub("tag prefix", "anthology-bin--v0.1.0", "anthology--v0.1.0");
    sub(
        "repo no slash",
        "\"repo\": \"anthology-test/",
        "\"repo\": \"anthology-test-",
    );
    sub(
        "repo dot part",
        "\"repo\": \"anthology-test/",
        "\"repo\": \"../",
    );
    sub(
        "repo dot name",
        "\"repo\": \"anthology-test/",
        "\"repo\": \"x/./",
    );
    sub(
        "repo bad char",
        "\"repo\": \"anthology-test/",
        "\"repo\": \"a b/",
    );
    sub(
        "repo quote",
        "\"repo\": \"anthology-test/",
        "\"repo\": \"a\\\"b/",
    );
    // The other targets' fixed hashes start with 0 (Release::lock_text).
    let host = bs::host_target();
    let others: Vec<&str> = anthology_core::platform::Target::ALL
        .into_iter()
        .filter(|t| *t != host)
        .map(|t| t.triple())
        .collect();
    let (a, b) = (others[0], others[1]);
    sub(
        "hash upper",
        &format!("\"{a}\": \"0"),
        &format!("\"{a}\": \"A"),
    );
    sub(
        "hash short",
        &format!("\"{b}\": \"0"),
        &format!("\"{b}\": \""),
    );
    sub(
        "hash long",
        &format!("\"{b}\": \"0"),
        &format!("\"{b}\": \"00"),
    );
    sub(
        "target renamed",
        "x86_64-unknown-linux-musl",
        "x86_64-unknown-linux-gnu",
    );
    sub(
        "targets swapped",
        "aarch64-apple-darwin\": \"",
        "aarch64-apple-darwinx\": \"",
    );
    sub("extra space", "\"tag\": ", "\"tag\":  ");
    sub("tab indent", "  \"repo\"", "\t\"repo\"");
    sub("crlf", "{\n", "{\r\n");
    sub("trailing comma", "  }\n}", "  },\n}");
    v.push(("trailing line".into(), format!("{good}\n")));
    v.push(("garbage after".into(), format!("{good}x")));
    v.push(("nul byte".into(), good.replacen("0.1.0", "0.1\u{0}.0", 1)));
    v.push(("compact json".into(), good.replace('\n', "")));
    v.push(("empty".into(), String::new()));
    v
}

#[test]
fn an_invalid_lock_allows_read_only_tools_and_blocks_the_rest() {
    each_shell("invalid lock", |sh| {
        let sb = sandbox();
        let good = sb.release.lock_text();
        // Both final-newline forms are valid.
        for ok in [good.clone(), good.trim_end_matches('\n').to_string()] {
            sb.write_lock(&ok);
            let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
            assert!(
                !run.out().contains(R_LOCK),
                "{sh:?}: valid lock refused: {}",
                run.out()
            );
        }
        for (name, text) in bad_locks(&good) {
            let sb = sandbox();
            sb.write_lock(&text);
            sb.install(&fake(V, ALLOW), 0o755);
            let c = format!("{sh:?} {name}");
            let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
            assert_status(&run, &format!("status=failed:{R_LOCK}"), &c);
            assert_allowed(&guard(&sb, sh, READ), &c);
            assert_block(&guard(&sb, sh, BASH), &lock_invalid(), &c);
            assert_block(&guard(&sb, sh, MCP), &lock_invalid(), &c);
            assert_block(&guard(&sb, sh, NESTED), &lock_invalid(), &c);
        }
        // A missing lock is invalid too.
        let sb = sandbox();
        std::fs::remove_file(sb.plugin("anthology-guardrails").join("anthology.lock")).unwrap();
        assert_block(
            &guard(&sb, sh, BASH),
            &lock_invalid(),
            &format!("{sh:?} missing"),
        );
        assert_allowed(&guard(&sb, sh, READ), &format!("{sh:?} missing"));
    });
}

fn uname_stub(os: &str, arch: &str) -> String {
    format!(
        "#!/bin/sh\ncase $1 in -s) printf '%s\\n' '{os}' ;; -m) printf '%s\\n' '{arch}' ;; *) echo '{os}' ;; esac\n"
    )
}

#[test]
fn an_unsupported_platform_allows_read_only_tools_and_names_the_platform() {
    each_shell("unsupported platform", |sh| {
        for (os, arch, want_os, want_arch) in [
            ("Plan9", "mips", "Plan9", "mips"),
            ("Linux", "riscv64", "Linux", "riscv64"),
            ("Darwin", "ppc", "Darwin", "ppc"),
            ("MINGW64_NT-10.0", "x86_64", "MINGW64_NT-10.0", "x86_64"),
            ("Lin ux\"\\", "x86_64", "Lin?ux??", "x86_64"),
            ("", "", "", ""),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
                "é",
                "ABCDEFGHIJKLMNOPQRSTUVWXYZ012345",
                "??",
            ),
        ] {
            let sb = sandbox();
            sb.install(&fake(V, ALLOW), 0o755);
            let path = sb.path_with(&["uname"], &[("uname", &uname_stub(os, arch))]);
            let c = format!("{sh:?} {os}/{arch}");
            let run = boot_path(
                &sb,
                sh,
                &["session", "--plugin", "guardrails", "--quiet"],
                path.clone(),
            );
            assert_status(
                &run,
                &format!("status=failed:unsupported platform {want_os}/{want_arch}"),
                &c,
            );
            assert_allowed(&guard_path(&sb, sh, READ, path.clone()), &c);
            assert_block(
                &guard_path(&sb, sh, BASH, path),
                &unsupported(want_os, want_arch),
                &c,
            );
        }
        // Every supported spelling maps (case-insensitive, atl's table).
        for (os, arch) in [
            ("darwin", "ARM64"),
            ("LINUX", "amd64"),
            ("Linux", "aarch64"),
        ] {
            let sb = sandbox();
            let path = sb.path_with(&["uname"], &[("uname", &uname_stub(os, arch))]);
            let run = boot_path(
                &sb,
                sh,
                &["guard", "--plugin", "guardrails", "--quiet"],
                path,
            );
            assert!(
                !run.out().contains("unsupported"),
                "{sh:?} {os}/{arch}: {}",
                run.out()
            );
        }
    });
}

#[test]
fn missing_binary_allows_read_only_tools_without_installing() {
    each_shell("missing, read-only", |sh| {
        let sb = sandbox();
        sb.seed();
        for payload in [
            READ,
            br#"{"tool_name":"Glob"}"#.as_slice(),
            b"{\"tool_name\"\n:\r\n\t\"TodoWrite\" }",
            br#"{"tool_name":"KillShell","x":1}"#,
        ] {
            assert_allowed(&guard(&sb, sh, payload), &format!("{sh:?}"));
        }
        assert!(
            !sb.vdir().exists(),
            "{sh:?}: read-only calls do not install"
        );
        assert!(!sb.bindir().join(".attempt").exists());
        assert!(
            std::fs::read_dir(&sb.tmpdir).unwrap().next().is_none(),
            "{sh:?}: the buffer is removed"
        );
    });
}

#[test]
fn missing_binary_allows_the_agent_and_cron_read_tools_and_still_blocks_writes() {
    each_shell("missing, new read-only tools", |sh| {
        let mut sb = sandbox();
        sb.seed();
        // No fetch: a call that is not allowed must block, not install.
        sb.set_env("ANTHOLOGY_GUARD_NO_FETCH", "1");
        for tool in ["ListAgents", "CronList", "SubagentHandback"] {
            let payload = format!(
                r#"{{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"{tool}","tool_input":{{}}}}"#
            );
            assert_allowed(
                &guard(&sb, sh, payload.as_bytes()),
                &format!("{sh:?} {tool}"),
            );
        }
        for tool in ["Bash", "Write", "Edit", "CronCreate"] {
            let payload = format!(
                r#"{{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"{tool}","tool_input":{{}}}}"#
            );
            assert_block(
                &guard(&sb, sh, payload.as_bytes()),
                &not_installed(&sb, "not attempted"),
                &format!("{sh:?} {tool}"),
            );
        }
        assert!(!sb.vdir().exists() && !sb.bindir().join(".attempt").exists());
    });
}

#[test]
fn no_fetch_blocks_with_not_attempted() {
    each_shell("NO_FETCH", |sh| {
        let mut sb = sandbox();
        sb.seed();
        sb.set_env("ANTHOLOGY_GUARD_NO_FETCH", "1");
        for p in [BASH, MCP, NESTED, b"not json".as_slice(), b""] {
            assert_block(
                &guard(&sb, sh, p),
                &not_installed(&sb, "not attempted"),
                &format!("{sh:?}"),
            );
        }
        assert_allowed(&guard(&sb, sh, READ), &format!("{sh:?} read"));
        assert!(!sb.vdir().exists() && !sb.bindir().join(".attempt").exists());
    });
}

#[test]
fn self_heal_is_rate_limited() {
    each_shell("rate limit", |sh| {
        let sb = sandbox();
        sb.seed();
        std::fs::write(sb.bindir().join(".attempt"), b"").unwrap();
        assert_block(
            &guard(&sb, sh, BASH),
            &rate_limited(&sb),
            &format!("{sh:?}"),
        );
        assert!(!sb.vdir().exists());
        // A stale stamp is retired and the call heals.
        age(&sb.bindir().join(".attempt"), 120);
        assert_allowed(&guard(&sb, sh, BASH), &format!("{sh:?} heal"));
        assert!(!sb.bin_entries().iter().any(|n| n.starts_with(".attempt.")));
        // The wrapper is rate-limited too.
        std::fs::remove_dir_all(sb.vdir()).unwrap();
        let run = sb.run(sh, WRAP, &["x"], b"");
        assert_eq!(run.code, Some(127));
        assert_eq!(
            run.err(),
            format!("{}\n", wrapper_failed("anthology", "rate-limited"))
        );
    });
}

#[test]
fn self_heal_runs_the_binary_on_the_buffered_payload() {
    each_shell("heal and buffer", |sh| {
        let mut sb = sandbox_with(ALLOW_LOG);
        let log = sb.root.join("stdin.log");
        sb.set_env("BS_LOG", log.clone());
        sb.seed();
        let payload = [BASH, b"\n", &vec![b' '; 100_000], b"\n"].concat();
        assert_allowed(&guard(&sb, sh, &payload), &format!("{sh:?}"));
        assert_eq!(
            std::fs::read(&log).unwrap(),
            payload,
            "{sh:?}: the same bytes"
        );
        assert_current(&sb, &fake(V, ALLOW_LOG), &format!("{sh:?}"));
        assert!(
            std::fs::read_dir(&sb.tmpdir).unwrap().next().is_none(),
            "{sh:?}: the buffer is removed"
        );
    });
}

#[test]
fn self_heal_classifies_the_new_binary_like_the_hot_path() {
    each_shell("heal classification", |sh| {
        // A real block passes through.
        let sb = sandbox_with(BLOCK);
        sb.seed();
        let run = guard(&sb, sh, BASH);
        assert_eq!(
            (run.code, run.err()),
            (Some(2), format!("{G1}\n")),
            "{sh:?}"
        );
        assert!(sb.broken().is_empty());
        // Anything else is quarantined.
        let sb = sandbox_with("cat >/dev/null; echo nope >&2; exit 0");
        sb.seed();
        assert_block(
            &guard(&sb, sh, BASH),
            &set_aside(&sb, "exit 0: nope"),
            &format!("{sh:?}"),
        );
        assert_eq!(sb.broken().len(), 1);
    });
}

#[test]
fn no_downloader_gives_the_manual_command() {
    each_shell("no downloader", |sh| {
        let sb = sandbox();
        let path = sb.path_with(DOWNLOADERS, &[]);
        let url = sb.release.url();
        assert_block(
            &guard_path(&sb, sh, BASH, path.clone()),
            &no_downloader(&url, &sb.bindir()),
            &format!("{sh:?}"),
        );
        // The second call in the minute is rate-limited.
        assert_block(
            &guard_path(&sb, sh, BASH, path.clone()),
            &rate_limited(&sb),
            &format!("{sh:?}"),
        );
        let mut spec = sb.spec(
            sh,
            G_BOOT,
            &["session", "--plugin", "guardrails", "--quiet"],
            b"",
        );
        spec.path = Some(path);
        assert_status(
            &sb.run_spec(spec),
            &format!("status=failed:{}", no_downloader_reason(&url)),
            &format!("{sh:?}"),
        );
    });
}

#[test]
fn the_shim_messages_name_the_real_install_folder() {
    // <ah> is the real path in plain text, even with CLAUDE_CONFIG_DIR unset (HOME/.claude).
    let sh = bs::shells("real <ah>")[0];
    let sb = sandbox();
    let path = sb.path_with(DOWNLOADERS, &[]);
    let mut spec = sb.spec(sh, GUARD, &[], BASH);
    spec.path = Some(path);
    spec.env.push(("CLAUDE_CONFIG_DIR".into(), "".into()));
    let run = sb.run_spec(spec);
    let bindir = sb.home.join(".claude/anthology/bin");
    assert_block(
        &run,
        &no_downloader(&sb.release.url(), &bindir),
        "HOME/.claude",
    );
    assert!(bindir.is_dir());
    assert!(!Sandbox::base_path().is_empty());
}

/// Early whole-branch review M6: `<ah>` from a relative, empty or unset HOME or
/// CLAUDE_CONFIG_DIR is never used. The guard closes (read-only tools still pass) and nothing
/// runs or installs, even when the relative path names an installed, allowing binary under the
/// hook's working folder.
#[test]
fn a_relative_or_empty_config_folder_closes_the_guard_and_runs_nothing() {
    each_shell("relative config folder", |sh| {
        // An empty value stands for an unset one: the scripts read both as `${X:-}`.
        for (home, config) in [
            (None, "config"),
            (None, "./config"),
            (Some("home"), ""),
            (Some(""), ""),
        ] {
            let mut sb = sandbox();
            sb.install(&fake(V, ALLOW), 0o755);
            // `home/.claude/anthology` (relative to the working folder) is the install too.
            std::os::unix::fs::symlink(&sb.config, sb.home.join(".claude")).unwrap();
            let c = format!("{sh:?} HOME={home:?} CLAUDE_CONFIG_DIR={config:?}");
            if let Some(h) = home {
                sb.set_env("HOME", h);
            }
            sb.set_env("CLAUDE_CONFIG_DIR", config);
            let before = sb.bin_entries();
            assert_allowed(&guard(&sb, sh, READ), &c);
            assert_block(&guard(&sb, sh, BASH), &no_home(), &c);
            assert_block(&guard(&sb, sh, MCP), &no_home(), &c);
            let run = boot(&sb, sh, &["guard", "--plugin", "guardrails", "--quiet"]);
            assert_status(&run, &format!("status=failed:{R_HOME}"), &c);
            let run = sb.run(sh, WRAP, &["doctor"], b"");
            assert_eq!(
                (run.code, run.out(), run.err()),
                (
                    Some(127),
                    String::new(),
                    format!("{}\n", wrapper_failed("anthology", R_HOME))
                ),
                "{c}"
            );
            let run = sb.run(sh, SESSION, &[], b"{}");
            assert_eq!(
                (run.code, run.out()),
                (Some(0), format!("{}\n", sys_anthology(R_HOME))),
                "{c}"
            );
            assert_eq!(sb.bin_entries(), before, "{c}: <ah>/bin changed");
        }
    });
}
