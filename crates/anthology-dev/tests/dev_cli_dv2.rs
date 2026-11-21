//! CLI behaviour of `version-bump` and `release-edit` that no Python test pins (exit codes
//! GC-5, argparse messages R5, uncaught exceptions R10, the git gate marker R7). Not ledger
//! targets: the Python suite has no such ids; the difftest compares these against the Python.

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anthology_core::Env;
use anthology_dev::{release_edit, version_bump};

struct Ran {
    code: i32,
    out: String,
    err: String,
}

fn args(a: &[&str]) -> Vec<OsString> {
    a.iter().map(OsString::from).collect()
}

fn release(cwd: &Path, a: &[&str]) -> Ran {
    let o = release_edit::run_in(&args(a), &mut std::io::empty(), &Env::default(), cwd);
    Ran {
        code: o.code,
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

fn bump(cwd: &Path, env: &Env, a: &[&str]) -> Ran {
    let o = version_bump::run_in(&args(a), &mut std::io::empty(), env, cwd);
    Ran {
        code: o.code,
        out: String::from_utf8_lossy(&o.stdout).into_owned(),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

mod release_edit_cli {
    use super::*;

    const PLUGIN: &str = "{\n  \"name\": \"p\",\n  \"version\": \"0.1.0\"\n}\n";

    #[test]
    fn relative_paths_resolve_against_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("plugin.json"), PLUGIN).unwrap();
        let r = release(tmp.path(), &["plugin-version", "plugin.json"]);
        assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "0.1.0\n", ""));
        let r = release(tmp.path(), &["set-version", "plugin.json", "0.2.0"]);
        assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""));
        assert_eq!(
            fs::read_to_string(tmp.path().join("plugin.json")).unwrap(),
            PLUGIN.replace("0.1.0", "0.2.0")
        );
    }

    #[test]
    fn a_non_ascii_version_is_escaped_like_json_dumps() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("plugin.json"), PLUGIN).unwrap();
        let r = release(
            tmp.path(),
            &["set-version", "plugin.json", "caf\u{e9}\u{1f600}"],
        );
        assert_eq!(r.code, 0, "{}", r.err);
        let text = fs::read_to_string(tmp.path().join("plugin.json")).unwrap();
        assert!(
            text.contains("\"version\": \"caf\\u00e9\\ud83d\\ude00\""),
            "{text}"
        );
    }

    #[test]
    fn check_version_prints_and_refuses() {
        let cwd = Path::new(".");
        let r = release(cwd, &["check-version", "0.1.0", "0.1.1", "1"]);
        assert_eq!((r.code, r.out.as_str()), (0, "bump\n"));
        let r = release(cwd, &["check-version", "0.1.0", "0.1.0", "0"]);
        assert_eq!((r.code, r.out.as_str()), (0, "same\n"));
        let r = release(cwd, &["check-version", "0.1.0", "0.1.0", "1"]);
        assert_eq!(r.code, 1);
        assert_eq!(
            r.err,
            "release: version 0.1.0 is already released (its tag exists)\n"
        );
        let r = release(cwd, &["check-version", "0.1.0", "1.2", "0"]);
        assert_eq!(
            r.err,
            "release: '1.2' is not a valid semver version (expected X.Y.Z)\n"
        );
    }

    #[test]
    fn usage_errors_exit_2_with_argparse_messages() {
        let cwd = Path::new(".");
        for (a, msg) in [
            (&[][..], "the following arguments are required: cmd"),
            (
                &["check-version", "a", "b", "2"][..],
                "argument tag_exists: invalid choice: '2' (choose from 0, 1)",
            ),
            (
                &["bogus"][..],
                "argument cmd: invalid choice: 'bogus' (choose from plugin-version, \
                 check-version, check-dependents, changelog-check, changelog-move, \
                 set-version, set-source-version)",
            ),
            (
                &["plugin-version"][..],
                "the following arguments are required: path",
            ),
            (
                &["plugin-version", "a", "b"][..],
                "unrecognized arguments: b",
            ),
            (
                &["changelog-move", "a", "b", "c", "--date"][..],
                "argument --date: expected one argument",
            ),
        ] {
            let r = release(cwd, a);
            assert_eq!(r.code, 2, "{a:?}");
            assert_eq!(r.out, "");
            assert!(
                r.err.contains(&format!(": error: {msg}\n")),
                "{a:?}: {}",
                r.err
            );
            assert!(
                r.err.starts_with("usage: anthology-dev release-edit"),
                "{}",
                r.err
            );
        }
    }

    #[test]
    fn a_leading_dashdash_is_dropped_before_the_command() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        fs::write(cwd.join("f"), "x").unwrap();
        let ok = |a: &[&str], out: &str| {
            let r = release(cwd, a);
            assert_eq!((r.code, r.out.as_str()), (0, out), "{a:?}: {}", r.err);
        };
        ok(&["--", "check-version", "1.0.0", "1.0.1", "1"], "bump\n");
        ok(
            &["--", "check-version", "--", "1.0.0", "1.0.1", "1"],
            "bump\n",
        );
        ok(
            &["--", "check-version", "1.0.0", "--", "1.0.1", "1"],
            "bump\n",
        );
        let r = release(
            cwd,
            &["--", "changelog-move", "f", "p", "1.0.0", "--date", "x"],
        );
        assert_eq!(
            (r.code, r.err.as_str()),
            (1, "release: date 'x' is not YYYY-MM-DD\n")
        );
        let tail = |a: &[&str]| {
            let r = release(cwd, a);
            assert_eq!(r.code, 2, "{a:?}");
            r.err.lines().last().unwrap().to_string()
        };
        assert_eq!(
            tail(&[
                "--",
                "changelog-move",
                "f",
                "p",
                "1.0.0",
                "--",
                "--date",
                "x"
            ]),
            "anthology-dev release-edit: error: unrecognized arguments: --date x"
        );
        assert_eq!(
            tail(&["--"]),
            "anthology-dev release-edit: error: the following arguments are required: cmd"
        );
        let choices = "plugin-version, check-version, check-dependents, changelog-check, \
                       changelog-move, set-version, set-source-version";
        for (a, bad) in [
            (&["--", "--", "x"][..], "--"),
            (&["--", "-x"][..], "-x"),
            (&["--", "bogus"][..], "bogus"),
            (&["--", "-h"][..], "-h"),
        ] {
            assert_eq!(
                tail(a),
                format!(
                    "anthology-dev release-edit: error: argument cmd: invalid choice: '{bad}' \
                     (choose from {choices})"
                )
            );
        }
        assert_eq!(
            tail(&["-x", "--", "check-version", "1", "2", "1"]),
            "anthology-dev release-edit: error: unrecognized arguments: -x"
        );
    }

    #[test]
    fn an_ambiguous_option_prefix_is_an_error() {
        let cwd = Path::new(".");
        let r = release(cwd, &["changelog-move", "a", "b", "c", "--=x"]);
        assert_eq!(r.code, 2);
        assert!(
            r.err.ends_with(
                "anthology-dev release-edit changelog-move: error: ambiguous option: --=x \
                 could match --help, --date\n"
            ),
            "{}",
            r.err
        );
        let r = bump(cwd, &Env::default(), &["a", "b", "--=x"]);
        assert_eq!(r.code, 2);
        assert!(
            r.err.ends_with(
                "anthology-dev version-bump: error: ambiguous option: --=x could match \
                 --help, --repo\n"
            ),
            "{}",
            r.err
        );
        // The error is raised when parsing reaches the argument: an earlier `-h`, or an option
        // still waiting for its value, wins.
        let r = release(cwd, &["changelog-move", "-h", "--=x"]);
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        let r = release(cwd, &["changelog-move", "--date", "--=x", "a", "b", "c"]);
        assert_eq!(r.code, 2);
        assert!(
            r.err
                .ends_with("error: argument --date: expected one argument\n"),
            "{}",
            r.err
        );
        let r = bump(cwd, &Env::default(), &["-h", "--=x"]);
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        let r = bump(cwd, &Env::default(), &["--repo", "--=x", "a", "b"]);
        assert_eq!(r.code, 2);
        assert!(
            r.err
                .ends_with("error: argument --repo: expected one argument\n"),
            "{}",
            r.err
        );
        // `-=x` matches every option string.
        for (a, prog, could) in [
            (
                &["-=x", "check-version", "1", "2", "1"][..],
                "",
                "-h, --help",
            ),
            (
                &["check-version", "1", "2", "1", "-=x"][..],
                " check-version",
                "-h, --help",
            ),
            (
                &["changelog-move", "a", "b", "c", "-=x"][..],
                " changelog-move",
                "-h, --help, --date",
            ),
            (&["-="][..], "", "-h, --help"),
        ] {
            let r = release(cwd, a);
            assert_eq!(r.code, 2, "{a:?}");
            let arg = a.iter().rev().find(|s| s.starts_with("-=")).unwrap();
            let want = format!(
                "anthology-dev release-edit{prog}: error: ambiguous option: {arg} could match {could}\n"
            );
            assert!(r.err.ends_with(&want), "{a:?}: {}", r.err);
        }
        let r = bump(cwd, &Env::default(), &["-=x"]);
        assert_eq!(r.code, 2);
        assert!(
            r.err
                .ends_with("error: ambiguous option: -=x could match -h, --help, --repo\n"),
            "{}",
            r.err
        );
    }

    #[test]
    fn an_unsearchable_root_fails_closed() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("g");
        fs::create_dir_all(root.join("plugins")).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o600)).unwrap();
        // Root ignores the mode: then the directory stays searchable and nothing fails.
        let searchable = fs::metadata(root.join("plugins")).is_ok();
        let r = release(tmp.path(), &["check-dependents", "g", "core", "1.0.0"]);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        if searchable {
            return;
        }
        assert_eq!(r.code, 1, "{}", r.err);
        assert!(r.err.starts_with("release: [Errno 13] "), "{}", r.err);
        assert!(r.err.ends_with(": 'g/plugins'\n"), "{}", r.err);
    }

    #[test]
    fn an_empty_path_is_enoent() {
        let tmp = tempfile::tempdir().unwrap();
        for a in [
            &["plugin-version", ""][..],
            &["set-version", "", "1.0.0"][..],
            &["changelog-check", "", "p"][..],
        ] {
            let r = release(tmp.path(), a);
            assert_eq!(r.code, 1, "{a:?}");
            assert!(r.err.starts_with("release: [Errno 2] "), "{}", r.err);
            assert!(r.err.ends_with(": ''\n"), "{}", r.err);
        }
    }

    #[test]
    fn a_dangling_plugin_json_symlink_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins/x/.claude-plugin");
        fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink("/nonexistent/target", dir.join("plugin.json")).unwrap();
        let r = release(tmp.path(), &["check-dependents", ".", "core", "1.0.0"]);
        assert_eq!(r.code, 1);
        assert!(r.err.starts_with("release: [Errno 2] "), "{}", r.err);
        assert!(
            r.err
                .ends_with(": 'plugins/x/.claude-plugin/plugin.json'\n"),
            "{}",
            r.err
        );
    }

    #[test]
    fn a_deeply_nested_range_is_printed_in_full() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins/a/.claude-plugin");
        fs::create_dir_all(&dir).unwrap();
        let deep = format!("{}{}", "[".repeat(1000), "]".repeat(1000));
        fs::write(
            dir.join("plugin.json"),
            format!("{{\"dependencies\": [{{\"name\": \"core\", \"version\": {deep}}}]}}"),
        )
        .unwrap();
        let r = release(tmp.path(), &["check-dependents", ".", "core", "1.0.0"]);
        assert_eq!(r.code, 1);
        assert!(
            r.err
                .contains(&format!("with range {deep}: can't check range")),
            "{}",
            &r.err[..r.err.len().min(200)]
        );
    }

    #[test]
    fn changelog_move_takes_a_date_and_refuses_a_bad_one() {
        let tmp = tempfile::tempdir().unwrap();
        let text = "## p\n\n### [Unreleased]\n\n- x\n";
        fs::write(tmp.path().join("C.md"), text).unwrap();
        let r = release(
            tmp.path(),
            &["changelog-move", "C.md", "p", "1.0.0", "--da=2026-01-02"],
        );
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        assert_eq!(
            fs::read_to_string(tmp.path().join("C.md")).unwrap(),
            "## p\n\n### [Unreleased]\n\n### [1.0.0] - 2026-01-02\n\n- x\n"
        );
        let r = release(
            tmp.path(),
            &["changelog-move", "C.md", "p", "1.0.1", "--date", "2026-1-2"],
        );
        assert_eq!(r.code, 1);
        assert_eq!(r.err, "release: date '2026-1-2' is not YYYY-MM-DD\n");
        // No --date: today, which is a valid date.
        fs::write(tmp.path().join("C.md"), text).unwrap();
        let r = release(tmp.path(), &["changelog-move", "C.md", "p", "1.0.0"]);
        assert_eq!((r.code, r.err.as_str()), (0, ""));
    }

    #[test]
    fn a_missing_file_is_an_oserror_line_and_a_bad_utf8_file_is_uncaught() {
        let tmp = tempfile::tempdir().unwrap();
        let r = release(tmp.path(), &["plugin-version", "nope.json"]);
        assert_eq!(r.code, 1);
        assert!(r.err.starts_with("release: [Errno 2] "), "{}", r.err);
        assert!(r.err.ends_with(": 'nope.json'\n"), "{}", r.err);
        fs::write(tmp.path().join("bad.json"), b"{\"v\": \"\xff\"}").unwrap();
        let r = release(tmp.path(), &["plugin-version", "bad.json"]);
        assert_eq!(r.code, 1);
        assert_eq!(
            r.err,
            "anthology-dev release-edit: UnicodeDecodeError: 'utf-8' codec can't decode byte \
             0xff in position 7: invalid start byte\n"
        );
    }

    #[test]
    fn check_dependents_prints_each_problem_and_exits_1() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins/app/.claude-plugin");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("plugin.json"),
            r#"{"dependencies": [{"name": "lib", "version": "~0.1"}]}"#,
        )
        .unwrap();
        let r = release(tmp.path(), &["check-dependents", ".", "lib", "0.2.0"]);
        assert_eq!(r.code, 1);
        assert!(
            r.err
                .starts_with("release: plugins/app/.claude-plugin/plugin.json depends on lib ~0.1")
        );
        let r = release(tmp.path(), &["check-dependents", ".", "lib", "0.1.2"]);
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        let r = release(
            tmp.path(),
            &["check-dependents", "no-such-dir", "lib", "not-semver"],
        );
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        // A bad version surfaces only once a dependency range is evaluated.
        let r = release(tmp.path(), &["check-dependents", ".", "lib", "not-semver"]);
        assert_eq!(r.code, 1);
        assert_eq!(
            r.err,
            "release: 'not-semver' is not a valid semver version\n"
        );
    }

    #[test]
    fn a_range_with_an_inner_newline_is_an_uncaught_attribute_error() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugins/app/.claude-plugin");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("plugin.json"),
            r#"{"dependencies": [{"name": "lib", "version": "1.2.3\n4"}]}"#,
        )
        .unwrap();
        let r = release(tmp.path(), &["check-dependents", ".", "lib", "1.2.3"]);
        assert_eq!(r.code, 1);
        assert_eq!(
            r.err,
            "anthology-dev release-edit: AttributeError: 'NoneType' object has no attribute 'group'\n"
        );
    }

    #[test]
    fn set_source_version_edits_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let text = "{\"sources\": [{\"target\": \"t\", \"plugin_version\": \"1.0.0\"}]}\n";
        fs::write(tmp.path().join("s.json"), text).unwrap();
        let r = release(tmp.path(), &["set-source-version", "s.json", "t", "1.1.0"]);
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        assert_eq!(
            fs::read_to_string(tmp.path().join("s.json")).unwrap(),
            text.replace("1.0.0", "1.1.0")
        );
        let r = release(tmp.path(), &["set-source-version", "s.json", "zz", "1.1.0"]);
        assert_eq!(
            r.err,
            "release: sources.json has no entry with target 'zz'\n"
        );
    }
}

mod version_bump_cli {
    use super::*;

    fn git(repo: &Path, a: &[&str]) {
        let st = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(a)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "git {a:?}");
    }

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        git(p, &["init", "-q", "-b", "main"]);
        git(p, &["config", "user.name", "T"]);
        git(p, &["config", "user.email", "t@users.noreply.github.com"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        tmp
    }

    fn commit(p: &Path, files: &[(&str, &str)], msg: &str) -> String {
        for (rel, text) in files {
            let f = p.join(rel);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, text).unwrap();
        }
        git(p, &["add", "-A"]);
        git(p, &["commit", "-q", "-m", msg]);
        let out = Command::new("git")
            .arg("-C")
            .arg(p)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn env() -> Env {
        let mut e = Env::from_process();
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("GIT_") {
                e = e.without(&k.to_string_lossy());
            }
        }
        e
    }

    #[test]
    fn usage_errors_exit_2() {
        let cwd = Path::new(".");
        let r = bump(cwd, &env(), &["only-base"]);
        assert_eq!(r.code, 2);
        assert!(
            r.err.ends_with(
                "anthology-dev version-bump: error: the following arguments are required: head\n"
            ),
            "{}",
            r.err
        );
        let r = bump(cwd, &env(), &["a", "b", "--nope"]);
        assert_eq!(r.code, 2);
        assert!(
            r.err.contains("error: unrecognized arguments: --nope"),
            "{}",
            r.err
        );
    }

    #[test]
    fn the_rust_gate_marker_counts_too() {
        let tmp = repo();
        let p = tmp.path();
        let base = commit(
            p,
            &[
                ("crates/anthology-dev/src/version_bump.rs", "// gate\n"),
                (
                    "plugins/p/.claude-plugin/plugin.json",
                    "{\"version\": \"0.1.0\"}\n",
                ),
            ],
            "base",
        );
        git(p, &["tag", "p--v0.1.0"]);
        let head = commit(p, &[("plugins/p/x.md", "x\n")], "edit");
        let r = bump(
            Path::new("."),
            &env(),
            &[&base, &head, "--repo", p.to_str().unwrap()],
        );
        assert_eq!(r.code, 1, "{} {}", r.out, r.err);
        assert_eq!(
            r.out,
            "FAIL plugins/p changed without a version bump (still 0.1.0)\n"
        );
    }

    #[test]
    fn repo_defaults_to_the_cwd_and_a_non_object_manifest_counts_as_unreadable() {
        let tmp = repo();
        let p = tmp.path();
        let base = commit(
            p,
            &[
                ("scripts/check_version_bump.py", "# gate\n"),
                ("plugins/p/.claude-plugin/plugin.json", "[1, 2]\n"),
            ],
            "base",
        );
        git(p, &["tag", "p--v0.1.0"]);
        let head = commit(
            p,
            &[(
                "plugins/p/.claude-plugin/plugin.json",
                "{\"version\": \"0.1.0\"}\n",
            )],
            "fix manifest",
        );
        let r = bump(p, &env(), &[&base, &head]);
        assert_eq!(r.code, 0, "{} {}", r.out, r.err);
        assert_eq!(
            r.out,
            "ok   plugins/p 0.1.0 (base version None was invalid)\n"
        );
    }

    #[test]
    fn a_missing_git_is_an_uncaught_file_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let e = env().with_os("PATH", &empty);
        let r = bump(tmp.path(), &e, &["abc", "def"]);
        assert_eq!(r.code, 1);
        assert!(
            r.err
                .starts_with("anthology-dev version-bump: FileNotFoundError: [Errno 2] "),
            "{}",
            r.err
        );
        assert!(r.err.ends_with(": 'git'\n"), "{}", r.err);
    }

    #[test]
    fn the_binary_runs_the_gate_through_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_anthology-dev"))
            .args(["version-bump", &"0".repeat(40), "HEAD"])
            .current_dir(tmp.path())
            .env_clear()
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "version gate: no base commit (new branch); nothing to compare\n"
        );
    }
}
