//! Ports of tests/test_memory_diff.py (DV2): one `mod` per Python class, one test per method.
//!
//! The parse and escape classes call the library. `git_integration_tests` builds throwaway
//! repositories with git and drives `memory_diff::run_in`, as the Python tests run the script
//! there.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anthology_core::Env;
use anthology_dev::memory_diff::{self, escape_name, parse_name_status, render};

const HOSTILE: &str = "evil\n|`::warning::x.md";

fn entry(letter: &str, paths: &[&str]) -> (String, Vec<String>) {
    (
        letter.to_string(),
        paths.iter().map(|p| p.to_string()).collect(),
    )
}

mod parse_tests {
    use super::*;

    #[test]
    fn test_splits_on_nul_not_newline() {
        let raw =
            b"A\0.claude/memory/a\nb.md\0R100\0.claude/memory/old.md\0.claude/memory/new.md\0";
        assert_eq!(
            parse_name_status(raw),
            vec![
                entry("A", &[".claude/memory/a\nb.md"]),
                entry("R", &[".claude/memory/old.md", ".claude/memory/new.md"]),
            ]
        );
    }

    #[test]
    fn test_empty() {
        assert!(parse_name_status(b"").is_empty());
    }

    #[test]
    fn test_all_statuses() {
        let raw = b"M\0m\0D\0d\0T\0t\0C75\0src\0dst\0";
        let letters: Vec<String> = parse_name_status(raw).into_iter().map(|e| e.0).collect();
        assert_eq!(letters, ["M", "D", "T", "C"]);
    }
}

mod escape_tests {
    use super::*;

    #[test]
    fn test_control_backtick_and_pipe() {
        assert_eq!(escape_name("a\nb\rc\td\x00e\x1b"), "a?b?c?d?e?");
        assert_eq!(escape_name("a`b|c"), "a'b\\|c");
    }

    #[test]
    fn test_unicode_line_separators() {
        assert_eq!(escape_name("a\u{2028}b\u{85}c"), "a?b?c");
    }

    #[test]
    fn test_capped_at_200_characters() {
        assert_eq!(escape_name(&"x".repeat(500)).chars().count(), 200);
    }

    #[test]
    fn test_render_empty() {
        assert!(render(&[]).contains("No memory changes."));
    }
}

/// `GitIntegrationTests`' fixture: a repository in a temporary folder, a summary path beside it.
struct Repo {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    summary: PathBuf,
}

/// The process environment without `GIT_*` and `GITHUB_STEP_SUMMARY` (the Python `env()`).
fn base_env() -> Env {
    let mut env = Env::from_process();
    let drop: Vec<String> = env
        .iter()
        .filter_map(|(k, _)| k.to_str().map(str::to_string))
        .filter(|k| k.starts_with("GIT_") || k == "GITHUB_STEP_SUMMARY")
        .collect();
    for k in drop {
        env = env.without(&k);
    }
    env
}

impl Repo {
    fn new() -> Repo {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).expect("mkdir");
        let summary = tmp.path().join("summary.md");
        let r = Repo {
            _tmp: tmp,
            repo,
            summary,
        };
        r.git(&["init", "-q", "-b", "main"]);
        r.git(&["config", "user.name", "Test"]);
        r.git(&["config", "user.email", "test@users.noreply.github.com"]);
        r.git(&["config", "commit.gpgsign", "false"]);
        r
    }

    fn git(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .env_clear()
            .envs(base_env().iter())
            .stdin(Stdio::null())
            .output()
            .expect("git runs");
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout)
            .expect("utf-8")
            .trim()
            .to_string()
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.repo.join(rel);
        fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        fs::write(p, text).expect("write");
    }

    fn commit(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// (exit, stdout, stderr, summary text), as `run_diff`.
    fn run_diff(&self, base: &str, head: &str, summary: bool) -> (i32, String, String, String) {
        let _ = fs::remove_file(&self.summary);
        let mut env = base_env();
        if summary {
            env = env.with_os("GITHUB_STEP_SUMMARY", &self.summary);
        }
        let args = [OsString::from(base), OsString::from(head)];
        let out = memory_diff::run_in(&args, &mut std::io::empty(), &env, &self.repo);
        let text = fs::read_to_string(&self.summary).unwrap_or_default();
        (
            out.code,
            String::from_utf8(out.stdout).expect("utf-8"),
            String::from_utf8(out.stderr).expect("utf-8"),
            text,
        )
    }
}

fn a_lines(text: &str) -> Vec<&str> {
    text.lines().filter(|l| l.starts_with("| A")).collect()
}

mod git_integration_tests {
    use super::*;

    #[test]
    fn test_add_modify_delete_rename_under_both_roots() {
        let r = Repo::new();
        let long_text = "a long enough body so rename detection sees identical content\n".repeat(5);
        r.write("README.md", "hi\n");
        r.write(".claude/memory/mod.md", "one\n");
        r.write(".claude/memory/del.md", "gone\n");
        r.write(".claude/agent-memory/anthology-scout/old.md", &long_text);
        let base = r.commit("base");
        r.write(".claude/memory/new.md", "note\n");
        r.write(".claude/memory/mod.md", "two\n");
        fs::remove_file(r.repo.join(".claude/memory/del.md")).expect("rm");
        r.git(&[
            "mv",
            ".claude/agent-memory/anthology-scout/old.md",
            ".claude/agent-memory/anthology-scout/renamed.md",
        ]);
        r.write(
            ".claude/agent-memory/anthology-implementer/added.md",
            "note\n",
        );
        let head = r.commit("head");
        let (code, out, _, text) = r.run_diff(&base, &head, true);
        assert_eq!(code, 0);
        assert_eq!(out.trim(), "memory-diff: 5 changed memory file(s)");
        assert!(text.contains("### Memory changes in this PR"));
        assert!(text.contains("| A | `.claude/memory/new.md` |"));
        assert!(text.contains("| M | `.claude/memory/mod.md` |"));
        assert!(text.contains("| D | `.claude/memory/del.md` |"));
        assert!(text.contains("| A | `.claude/agent-memory/anthology-implementer/added.md` |"));
        assert!(text.contains(
            "| R | `.claude/agent-memory/anthology-scout/old.md` → \
             `.claude/agent-memory/anthology-scout/renamed.md` |"
        ));
    }

    #[test]
    fn test_files_outside_the_roots_and_nested_roots_are_ignored() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let base = r.commit("base");
        r.write("README.md", "changed\n");
        r.write("docs/note.md", "note\n");
        r.write("sub/.claude/memory/x.md", "note\n");
        r.write("sub/.claude/agent-memory/a/y.md", "note\n");
        r.write(".claude/memory-other/z.md", "note\n");
        let head = r.commit("head");
        let (code, out, _, text) = r.run_diff(&base, &head, true);
        assert_eq!(code, 0);
        assert_eq!(out.trim(), "memory-diff: 0 changed memory file(s)");
        assert!(text.contains("No memory changes."));
        assert!(!text.contains("x.md"));
    }

    #[test]
    fn test_merge_base_form_ignores_changes_only_on_base() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let fork = r.commit("fork point");
        r.git(&["checkout", "-q", "-b", "topic"]);
        r.write(".claude/memory/topic.md", "note\n");
        let topic = r.commit("topic");
        r.git(&["checkout", "-q", "main"]);
        r.write(".claude/memory/only-on-base.md", "note\n");
        let base = r.commit("base moves on");
        assert_ne!(fork, base);
        let (code, _, _, text) = r.run_diff(&base, &topic, true);
        assert_eq!(code, 0);
        assert!(text.contains("`.claude/memory/topic.md`"));
        assert!(!text.contains("only-on-base.md"));
    }

    #[test]
    fn test_hostile_file_name_is_escaped_and_never_on_stdout() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let base = r.commit("base");
        r.write(&format!(".claude/memory/{HOSTILE}"), "note\n");
        let head = r.commit("head");
        let (code, out, err, text) = r.run_diff(&base, &head, true);
        assert_eq!(code, 0);
        assert_eq!(out.trim(), "memory-diff: 1 changed memory file(s)");
        let both = format!("{out}{err}");
        assert!(!both.contains("::warning::"));
        assert!(!both.contains("evil"));
        assert!(text.contains("| A | `.claude/memory/evil?\\|'::warning::x.md` |"));
        assert_eq!(a_lines(&text).len(), 1);
        assert!(!text.lines().any(|l| l.starts_with("::")));
    }

    #[test]
    fn test_without_a_summary_file_the_section_goes_to_stdout_escaped() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let base = r.commit("base");
        r.write(&format!(".claude/memory/{HOSTILE}"), "note\n");
        let head = r.commit("head");
        let (code, out, _, _) = r.run_diff(&base, &head, false);
        assert_eq!(code, 0);
        assert!(!out.lines().any(|l| l.starts_with("::")));
        assert!(out.contains("evil?\\|'::warning::x.md"));
        assert!(
            out.trim_end()
                .ends_with("memory-diff: 1 changed memory file(s)")
        );
    }

    #[test]
    fn test_long_names_are_capped() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let base = r.commit("base");
        r.write(&format!(".claude/memory/{}.md", "n".repeat(240)), "note\n");
        let head = r.commit("head");
        let (_, _, _, text) = r.run_diff(&base, &head, true);
        let line = a_lines(&text)[0];
        let shown = line.split('`').nth(1).expect("a code span");
        assert!(shown.chars().count() <= 200, "{shown}");
    }

    #[test]
    fn test_no_changes() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let base = r.commit("base");
        let (code, out, _, text) = r.run_diff(&base, &base, true);
        assert_eq!(code, 0);
        assert!(text.contains("No memory changes."));
        assert_eq!(out.trim(), "memory-diff: 0 changed memory file(s)");
    }

    #[test]
    fn test_bad_ref_exits_1() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let base = r.commit("base");
        let (code, _, err, _) = r.run_diff(&base, "no-such-ref", true);
        assert_eq!(code, 1);
        assert!(err.contains("memory-diff:"), "{err}");
    }

    #[test]
    fn test_usage_error_exits_2() {
        let bin = env!("CARGO_BIN_EXE_anthology-dev");
        for args in [&["only-one"][..], &["--output=x", "HEAD"][..]] {
            let out = Command::new(bin)
                .arg("memory-diff")
                .args(args)
                .stdin(Stdio::null())
                .output()
                .expect("runs");
            assert_eq!(out.status.code(), Some(2), "{args:?}");
            assert_eq!(out.stderr, b"usage: memory_diff.py <base> <head>\n");
            assert!(out.stdout.is_empty());
        }
    }
}

/// Not a ledger target: R4's timeout, the spawn failure and the summary-write failure.
mod lead_extras {
    use super::*;

    #[test]
    fn git_timeout_and_missing_git_are_runtime_errors() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let base = r.commit("base");
        let args = [OsString::from(&base), OsString::from(&base)];
        // A git that hangs: a script named `git` first on PATH.
        let bin = r._tmp.path().join("slow-bin");
        fs::create_dir(&bin).expect("mkdir");
        let fake = bin.join("git");
        fs::write(&fake, "#!/bin/sh\nexec sleep 30\n").expect("write");
        let mode = <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755);
        fs::set_permissions(&fake, mode).expect("chmod");
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let started = std::time::Instant::now();
        let out = memory_diff::run_with_timeout(
            &args,
            &mut std::io::empty(),
            &base_env().with("PATH", &path),
            &r.repo,
            std::time::Duration::from_millis(200),
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
        assert_eq!(out.code, 1);
        assert_eq!(
            out.stderr,
            b"memory-diff: git failed: git did not run: TimeoutExpired\n"
        );
        let no_git = base_env().with("PATH", "/nonexistent");
        let out = memory_diff::run_in(&args, &mut std::io::empty(), &no_git, &r.repo);
        assert_eq!(out.code, 1);
        assert_eq!(
            out.stderr,
            b"memory-diff: git failed: git did not run: FileNotFoundError\n"
        );
    }

    #[test]
    fn an_unwritable_summary_is_an_uncaught_os_error() {
        let r = Repo::new();
        r.write("README.md", "hi\n");
        let base = r.commit("base");
        let args = [OsString::from(&base), OsString::from(&base)];
        let env = base_env().with_os("GITHUB_STEP_SUMMARY", Path::new("/"));
        let out = memory_diff::run_in(&args, &mut std::io::empty(), &env, &r.repo);
        assert_eq!(out.code, 1);
        assert!(out.stdout.is_empty());
        let err = String::from_utf8(out.stderr).expect("utf-8");
        assert!(
            err.starts_with("anthology-dev memory-diff: IsADirectoryError: [Errno 21] "),
            "{err}"
        );
    }

    #[test]
    fn invalid_utf8_is_replaced_and_short_entries_keep_what_is_there() {
        assert_eq!(
            parse_name_status(b"A\0a\xffb\0R\0only\0"),
            vec![entry("A", &["a\u{fffd}b"]), entry("R", &["only"])]
        );
        assert_eq!(parse_name_status(b"\0x\0"), vec![entry("?", &["x"])]);
    }
}
