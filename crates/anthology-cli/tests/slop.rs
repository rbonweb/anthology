//! `anthology slop hook|check|rules` (plan docs/superpowers/plans/2026-11-28-anti-slop-gate.md,
//! Task 7), run as the built binary with a cleared environment, a sandbox home and config folder,
//! and temp repos.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anthology_slop::{Kind, MAX_DOC, report};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");

/// A tell the gate blocks in Markdown (`text.ai-vocab`).
const TELL: &str = "We delve into caching.\n";
/// A second tell, of another rule (`text.chat-residue`).
const TELL2: &str = "Let me know if you have any questions.\n";
const CLEAN: &str = "The cache keeps the last ten answers.\n";

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// A sandbox: a home, a config folder and a repo (a folder holding a `.git` folder).
struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    config: PathBuf,
    repo: PathBuf,
}

impl Fx {
    fn new() -> Fx {
        let tmp = tempfile::tempdir().expect("temp dir");
        let base = tmp.path().to_path_buf();
        let home = base.join("user");
        let config = base.join("config");
        let repo = base.join("repo");
        for dir in [&home, &config, &repo.join(".git")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Fx {
            _tmp: tmp,
            base,
            home,
            config,
            repo,
        }
    }

    /// A Git repo of its own (a real `git init`), for `check --since`.
    fn git_repo(&self) -> PathBuf {
        let repo = self.base.join("gitrepo");
        std::fs::create_dir_all(&repo).unwrap();
        self.git(&repo, &["init", "-q"]);
        self.git(&repo, &["config", "user.name", "Test"]);
        self.git(
            &repo,
            &["config", "user.email", "test@users.noreply.github.com"],
        );
        self.git(&repo, &["config", "commit.gpgsign", "false"]);
        repo
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin:/usr/local/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .output()
            .expect("run git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn write(&self, path: &Path, text: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn run(&self, args: &[&str], cwd: &Path, input: &str, extra: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("CLAUDE_CONFIG_DIR", &self.config)
            .env("PATH", "/usr/bin:/bin:/usr/local/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            // git never looks above the sandbox for a repo
            .env("GIT_CEILING_DIRECTORIES", &self.base)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("run anthology");
        let stdout = drain(child.stdout.take().unwrap());
        let stderr = drain(child.stderr.take().unwrap());
        let mut stdin = child.stdin.take().unwrap();
        // The binary may exit before it reads everything; a broken pipe is fine.
        let _ = stdin.write_all(input.as_bytes());
        drop(stdin);
        // A run that hangs (a read of a FIFO or a device) fails the test instead of the suite.
        let deadline = Instant::now() + RUN_LIMIT;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("anthology {args:?} did not finish in {RUN_LIMIT:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        Run {
            code: status.code().expect("exit code"),
            stdout: String::from_utf8_lossy(&stdout.join().unwrap()).into_owned(),
            stderr: String::from_utf8_lossy(&stderr.join().unwrap()).into_owned(),
        }
    }

    fn hook(&self, payload: &str) -> Run {
        self.hook_env(payload, &[])
    }

    fn hook_env(&self, payload: &str, extra: &[(&str, &str)]) -> Run {
        self.run(&["slop", "hook"], &self.base, payload, extra)
    }

    fn check(&self, args: &[&str], cwd: &Path) -> Run {
        let all = [&["slop", "check"][..], args].concat();
        self.run(&all, cwd, "", &[])
    }
}

/// The longest a run may take.
const RUN_LIMIT: Duration = Duration::from_secs(30);

/// Reads a pipe to its end on its own thread.
fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

/// A FIFO at `path` (no process ever opens its other end).
fn mkfifo(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let status = Command::new("mkfifo").arg(path).status().expect("mkfifo");
    assert!(status.success());
}

/// A JSON string literal.
fn js(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn p(path: &Path) -> String {
    js(path.to_str().unwrap())
}

/// A PreToolUse payload; `input` is the JSON text of `tool_input`.
fn payload(tool: &str, input: &str, cwd: &Path) -> String {
    format!(
        r#"{{"session_id":"s","hook_event_name":"PreToolUse","tool_name":{},"tool_input":{input},"cwd":{}}}"#,
        js(tool),
        p(cwd)
    )
}

fn write_input(path: &str, content: &str) -> String {
    format!(r#"{{"file_path":{},"content":{}}}"#, js(path), js(content))
}

fn edit_input(path: &str, old: &str, new: &str) -> String {
    format!(
        r#"{{"file_path":{},"old_string":{},"new_string":{}}}"#,
        js(path),
        js(old),
        js(new)
    )
}

#[track_caller]
fn assert_allowed(run: &Run, ctx: &str) {
    assert_eq!(run.code, 0, "{ctx}: stderr {:?}", run.stderr);
    assert_eq!(run.stdout, "", "{ctx}");
    assert_eq!(run.stderr, "", "{ctx}");
}

#[track_caller]
fn assert_blocked(run: &Run, tool: &str, shown: &str, ctx: &str) {
    assert_eq!(run.code, 10, "{ctx}: stderr {:?}", run.stderr);
    assert_eq!(run.stdout, "", "{ctx}");
    let head = format!("anthology slop gate: the {tool} of {shown} was not applied.");
    assert!(run.stderr.starts_with(&head), "{ctx}: {:?}", run.stderr);
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

// ---- hook ----

#[test]
fn hook_blocks_a_write_with_a_tell() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    let run = fx.hook(&payload("Write", &write_input(s(&file), TELL), &fx.repo));
    assert_blocked(&run, "Write", "docs/a.md", "write");
    let want = report::hook_message(
        "Write",
        "docs/a.md",
        Kind::Markdown,
        &anthology_slop::check_all(Kind::Markdown, TELL),
    );
    assert!(want.contains("text.ai-vocab"), "{want}");
    assert_eq!(run.stderr, want);
    assert!(!file.exists(), "the hook never writes the file");
}

#[test]
fn hook_blocks_an_edit_and_a_multiedit_with_a_tell() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    fx.write(&file, b"First line.\n\nSecond line.\n");
    let edit = edit_input(s(&file), "Second line.", "We delve into it.");
    assert_blocked(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "Edit",
        "docs/a.md",
        "edit",
    );
    let multi = format!(
        r#"{{"file_path":{},"edits":[{{"old_string":"First line.","new_string":"Opening line."}},{{"old_string":"Second line.","new_string":"We delve into it.","replace_all":false}}]}}"#,
        p(&file)
    );
    let run = fx.hook(&payload("MultiEdit", &multi, &fx.repo));
    assert_blocked(&run, "MultiEdit", "docs/a.md", "multiedit");
    assert!(
        run.stderr.contains("  line 3: text.ai-vocab"),
        "{}",
        run.stderr
    );
    // replace_all reaches every copy
    fx.write(&file, b"x one\n\nx two\n");
    let all = format!(
        r#"{{"file_path":{},"old_string":"x","new_string":"We delve","replace_all":true}}"#,
        p(&file)
    );
    assert_blocked(
        &fx.hook(&payload("Edit", &all, &fx.repo)),
        "Edit",
        "docs/a.md",
        "replace_all",
    );
    // without replace_all, an old_string found twice does not apply: allowed
    let twice = edit_input(s(&file), "x", "We delve");
    assert_allowed(&fx.hook(&payload("Edit", &twice, &fx.repo)), "ambiguous");
}

#[test]
fn hook_allows_an_edit_that_keeps_a_line_with_a_tell() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    let old = "Sync is seamless across devices.\nIt runs nightly.";
    fx.write(&file, old.as_bytes());
    let new = "Sync is seamless across devices.\nIt runs hourly.";
    let edit = edit_input(s(&file), old, new);
    assert_allowed(&fx.hook(&payload("Edit", &edit, &fx.repo)), "kept line");
    // a tell on the changed line is still refused
    let edit = edit_input(
        s(&file),
        old,
        "Sync is seamless across devices.\nIt delves deeper.",
    );
    assert_blocked(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "Edit",
        "docs/a.md",
        "changed line",
    );
}

#[test]
fn hook_allows_a_one_word_fix_on_a_line_with_an_old_tell() {
    // review-2 I2: one paragraph per line, so the line holds text somebody else wrote
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    let line = "Our sync is seamless across devices, and it runs nighly at 2 am.";
    let fixed = line.replace("nighly", "nightly");
    fx.write(&file, format!("Intro.\n\n{line}\n").as_bytes());
    let edit = edit_input(s(&file), line, &fixed);
    assert_allowed(&fx.hook(&payload("Edit", &edit, &fx.repo)), "whole line");
    let edit = edit_input(s(&file), "runs nighly at", "runs nightly at");
    assert_allowed(&fx.hook(&payload("Edit", &edit, &fx.repo)), "phrase");
    let write = write_input(s(&file), &format!("Intro.\n\n{fixed}\n"));
    assert_allowed(&fx.hook(&payload("Write", &write, &fx.repo)), "write");
    let item = write_input(s(&file), &format!("Intro.\n\n- {line}\n"));
    assert_allowed(&fx.hook(&payload("Write", &item, &fx.repo)), "list item");
    let multi = format!(
        r#"{{"file_path":{},"edits":[{{"old_string":"nighly","new_string":"nightly"}}]}}"#,
        p(&file)
    );
    assert_allowed(
        &fx.hook(&payload("MultiEdit", &multi, &fx.repo)),
        "multiedit",
    );
    // a tell that the edit itself adds is refused, on the same line
    let edit = edit_input(
        s(&file),
        line,
        "Our sync is seamless across devices, and it runs nightly at 2 am. It delves deeper.",
    );
    assert_blocked(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "Edit",
        "docs/a.md",
        "added tell",
    );
    fx.write(&file, b"Intro.\n\nOur sync is quick across devices.\n");
    let edit = edit_input(s(&file), "is quick", "is seamless");
    assert_blocked(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "Edit",
        "docs/a.md",
        "swapped in tell",
    );
}

#[test]
fn hook_message_shows_the_waiver_syntax_of_the_file_kind() {
    let fx = Fx::new();
    let content = "export const A = () => <p>We delve into it.</p>;\n";
    let jsx = fx.repo.join("src/a.tsx");
    let run = fx.hook(&payload("Write", &write_input(s(&jsx), content), &fx.repo));
    assert_blocked(&run, "Write", "src/a.tsx", "jsx");
    assert!(
        run.stderr
            .contains("Add {/* slop-ok: <rule> <reason> */} only"),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("<!--"), "{}", run.stderr);
    let md = fx.repo.join("docs/a.md");
    let run = fx.hook(&payload("Write", &write_input(s(&md), TELL), &fx.repo));
    assert!(
        run.stderr
            .contains("Add <!-- slop-ok: <rule> <reason> --> only"),
        "{}",
        run.stderr
    );
}

#[test]
fn hook_allows_clean_text() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    let run = fx.hook(&payload("Write", &write_input(s(&file), CLEAN), &fx.repo));
    assert_allowed(&run, "clean write");
}

#[test]
fn hook_allows_an_edit_inside_a_code_fence() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    fx.write(&file, b"Intro.\n\n```text\nold line\n```\n");
    let edit = edit_input(s(&file), "old line", "I hope this helps.");
    assert_allowed(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "inside a fence",
    );
    // the same Edit outside a fence is refused
    fx.write(&file, b"Intro.\n\nold line\n");
    assert_blocked(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "Edit",
        "docs/a.md",
        "outside a fence",
    );
}

#[test]
fn hook_checks_only_the_new_text_of_an_edit() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    let before = format!("{TELL}\nSecond line.\n");
    fx.write(&file, before.as_bytes());
    let edit = edit_input(s(&file), "Second line.", "Second line, rewritten.");
    assert_allowed(&fx.hook(&payload("Edit", &edit, &fx.repo)), "edit");
    // a Write that keeps the old tell line and changes another is allowed too
    let after = format!("{TELL}\nSecond line, rewritten.\n");
    let write = write_input(s(&file), &after);
    assert_allowed(&fx.hook(&payload("Write", &write, &fx.repo)), "write");
    // and a Write that adds a tell line is refused
    let after = format!("{TELL}\n{TELL2}");
    let run = fx.hook(&payload("Write", &write_input(s(&file), &after), &fx.repo));
    assert_blocked(&run, "Write", "docs/a.md", "added tell");
    assert!(
        run.stderr.contains("line 3: text.chat-residue"),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("text.ai-vocab"), "{}", run.stderr);
}

#[test]
fn hook_ignores_other_tools_and_kinds() {
    let fx = Fx::new();
    let bash = format!(r#"{{"command":{}}}"#, js("echo 'We delve' > docs/a.md"));
    assert_allowed(&fx.hook(&payload("Bash", &bash, &fx.repo)), "Bash");
    let rs = fx.repo.join("src/a.rs");
    assert_allowed(
        &fx.hook(&payload("Write", &write_input(s(&rs), TELL), &fx.repo)),
        ".rs",
    );
    let none = fx.repo.join("docs/README");
    assert_allowed(
        &fx.hook(&payload("Write", &write_input(s(&none), TELL), &fx.repo)),
        "no extension",
    );
}

#[test]
fn hook_fails_open_on_bad_input() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    let path = s(&file);
    let cases: Vec<(&str, String)> = vec![
        ("not JSON", "We delve into caching".to_string()),
        ("empty", String::new()),
        ("an array", "[]".to_string()),
        (
            "truncated",
            payload("Write", &write_input(path, TELL), &fx.repo)[..60].to_string(),
        ),
        (
            "no tool_input",
            format!(r#"{{"tool_name":"Write","cwd":{}}}"#, p(&fx.repo)),
        ),
        (
            "no file_path",
            payload("Write", &format!(r#"{{"content":{}}}"#, js(TELL)), &fx.repo),
        ),
        (
            "file_path not a string",
            payload(
                "Write",
                &format!(r#"{{"file_path":7,"content":{}}}"#, js(TELL)),
                &fx.repo,
            ),
        ),
        (
            "no content",
            payload(
                "Write",
                &format!(r#"{{"file_path":{}}}"#, js(path)),
                &fx.repo,
            ),
        ),
        (
            "tool_name not a string",
            payload("Write", &write_input(path, TELL), &fx.repo)
                .replace(r#""tool_name":"Write""#, r#""tool_name":["Write"]"#),
        ),
        (
            "a relative path and no cwd",
            format!(
                r#"{{"tool_name":"Write","tool_input":{}}}"#,
                write_input("docs/a.md", TELL)
            ),
        ),
        (
            "an edit with no new_string",
            payload(
                "Edit",
                &format!(r#"{{"file_path":{},"old_string":"x"}}"#, js(path)),
                &fx.repo,
            ),
        ),
        (
            "a MultiEdit edit that is not an object",
            payload(
                "MultiEdit",
                &format!(r#"{{"file_path":{},"edits":[7]}}"#, js(path)),
                &fx.repo,
            ),
        ),
    ];
    for (ctx, input) in &cases {
        assert_allowed(&fx.hook(input), ctx);
    }

    // a file that is not UTF-8
    let bad = fx.repo.join("docs/bad.md");
    fx.write(&bad, b"Intro \xff.\n\nSecond line.\n");
    let edit = edit_input(s(&bad), "Second line.", "We delve into it.");
    assert_allowed(&fx.hook(&payload("Edit", &edit, &fx.repo)), "not UTF-8");
    let write = write_input(s(&bad), TELL);
    assert_allowed(
        &fx.hook(&payload("Write", &write, &fx.repo)),
        "Write over non-UTF-8",
    );

    // a file over 1 MiB, before and after
    let big = fx.repo.join("docs/big.md");
    let mut text = "a\n".repeat(MAX_DOC / 2);
    text.push_str("Second line.\n");
    fx.write(&big, text.as_bytes());
    let edit = edit_input(s(&big), "Second line.", "We delve into it.");
    assert_allowed(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "before over 1 MiB",
    );
    let huge = format!("{TELL}{}", "b\n".repeat(MAX_DOC / 2));
    let new = fx.repo.join("docs/new.md");
    let write = write_input(s(&new), &huge);
    assert_allowed(
        &fx.hook(&payload("Write", &write, &fx.repo)),
        "after over 1 MiB",
    );

    // an old_string that is not in the file
    fx.write(&file, b"First line.\n");
    let edit = edit_input(path, "No such line.", "We delve into it.");
    assert_allowed(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "old_string not found",
    );
    // an Edit of a missing file with a non-empty old_string
    let missing = fx.repo.join("docs/missing.md");
    let edit = edit_input(s(&missing), "x", "We delve into it.");
    assert_allowed(&fx.hook(&payload("Edit", &edit, &fx.repo)), "missing file");

    // a folder where the file should be
    let dir = fx.repo.join("docs/folder.md");
    std::fs::create_dir_all(&dir).unwrap();
    let write = write_input(s(&dir), TELL);
    assert_allowed(&fx.hook(&payload("Write", &write, &fx.repo)), "a folder");
}

#[test]
fn hook_allows_a_fifo_or_a_device_promptly_and_unchecked() {
    let fx = Fx::new();
    let fifo = fx.repo.join("docs/fifo.md");
    mkfifo(&fifo);
    let zero = fx.repo.join("docs/zero.md");
    std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
    for (ctx, file) in [("FIFO", &fifo), ("symlink to /dev/zero", &zero)] {
        for input in [
            edit_input(s(file), "x", TELL),
            write_input(s(file), TELL),
            edit_input(s(file), "", TELL),
        ] {
            let tool = if input.contains("old_string") {
                "Edit"
            } else {
                "Write"
            };
            let start = Instant::now();
            let run = fx.hook(&payload(tool, &input, &fx.repo));
            assert_allowed(&run, ctx);
            assert!(start.elapsed() < Duration::from_secs(10), "{ctx}");
        }
    }
}

#[test]
fn hook_creates_from_an_empty_old_string_on_a_missing_file() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/new.md");
    let edit = edit_input(s(&file), "", TELL);
    assert_blocked(
        &fx.hook(&payload("Edit", &edit, &fx.repo)),
        "Edit",
        "docs/new.md",
        "new file",
    );
}

#[test]
fn hook_honours_the_switch() {
    let fx = Fx::new();
    let file = fx.repo.join("docs/a.md");
    let input = payload("Write", &write_input(s(&file), TELL), &fx.repo);
    assert_allowed(
        &fx.hook_env(&input, &[("ANTHOLOGY_SLOP_GATE", "0")]),
        "switch 0",
    );
    for on in ["1", "", "off"] {
        let run = fx.hook_env(&input, &[("ANTHOLOGY_SLOP_GATE", on)]);
        assert_blocked(&run, "Write", "docs/a.md", on);
    }
}

#[test]
fn hook_skips_exempt_paths() {
    let fx = Fx::new();
    for rel in [
        ".claude/memory/x.md",
        "node_modules/p/README.md",
        "vendor/lib/a.md",
        "docs/fixtures/a.md",
    ] {
        let file = fx.repo.join(rel);
        let run = fx.hook(&payload("Write", &write_input(s(&file), TELL), &fx.repo));
        assert_allowed(&run, rel);
    }
    // the Claude config folder, from CLAUDE_CONFIG_DIR. It is a repo of its own, as many are,
    // so no folder name below its root makes the file exempt: only the config-folder rule does.
    std::fs::create_dir_all(fx.config.join(".git")).unwrap();
    let file = fx.config.join("agents/x.md");
    let run = fx.hook(&payload("Write", &write_input(s(&file), TELL), &fx.repo));
    assert_allowed(&run, "CLAUDE_CONFIG_DIR");
    // the same file, with the config folder elsewhere, is gated
    let run = fx.hook_env(
        &payload("Write", &write_input(s(&file), TELL), &fx.repo),
        &[("CLAUDE_CONFIG_DIR", s(&fx.base.join("other")))],
    );
    assert_blocked(&run, "Write", "agents/x.md", "another config folder");
    // else from HOME
    std::fs::create_dir_all(fx.home.join(".claude/.git")).unwrap();
    let file = fx.home.join(".claude/agents/x.md");
    let input = payload("Write", &write_input(s(&file), TELL), &fx.repo);
    let run = fx.run(
        &["slop", "hook"],
        &fx.base,
        &input,
        &[("CLAUDE_CONFIG_DIR", "")],
    );
    assert_allowed(&run, "HOME/.claude");
}

#[test]
fn hook_reads_the_path_it_will_write_not_the_one_it_was_given() {
    let fx = Fx::new();
    // `..` and `.` are resolved before the exempt check: this is docs/a.md, not .claude/
    let dotted = format!("{}/.claude/../docs/./a.md", s(&fx.repo));
    let run = fx.hook(&payload("Write", &write_input(&dotted, TELL), &fx.repo));
    assert_blocked(&run, "Write", "docs/a.md", "dotted path");
    // a relative path is joined to cwd
    let run = fx.hook(&payload("Write", &write_input("docs/a.md", TELL), &fx.repo));
    assert_blocked(&run, "Write", "docs/a.md", "relative path");
    // an Edit reads the file at the resolved path
    fx.write(
        &fx.repo.join("docs/b.md"),
        b"Intro.\n\n```text\nold line\n```\n",
    );
    let edit = edit_input("docs/x/../b.md", "old line", "I hope this helps.");
    assert_allowed(&fx.hook(&payload("Edit", &edit, &fx.repo)), "relative edit");
}

#[test]
fn hook_gates_a_worktree_under_dot_claude() {
    let fx = Fx::new();
    let wt = fx.repo.join(".claude/worktrees/w");
    fx.write(&wt.join(".git"), b"gitdir: ../../../.git/worktrees/w\n");
    let file = wt.join("docs/a.md");
    let run = fx.hook(&payload("Write", &write_input(s(&file), TELL), &wt));
    assert_blocked(&run, "Write", "docs/a.md", "worktree");
    // the worktree's own .claude folder is still exempt
    let own = wt.join(".claude/memory/x.md");
    let run = fx.hook(&payload("Write", &write_input(s(&own), TELL), &wt));
    assert_allowed(&run, "worktree's .claude");
}

#[test]
fn hook_ignores_extra_arguments_and_a_closed_stdin() {
    let fx = Fx::new();
    let run = fx.run(&["slop", "hook", "--extra"], &fx.base, "", &[]);
    assert_allowed(&run, "extra argument");
}

// ---- check ----

#[test]
fn check_reports_findings_and_exits_1() {
    let fx = Fx::new();
    fx.write(&fx.repo.join("docs/a.md"), TELL.as_bytes());
    fx.write(&fx.repo.join("docs/b.md"), CLEAN.as_bytes());
    fx.write(&fx.repo.join("docs/c.rs"), TELL.as_bytes());
    fx.write(
        &fx.repo.join("docs/node_modules/p/README.md"),
        TELL.as_bytes(),
    );
    fx.write(&fx.repo.join("README.md"), TELL2.as_bytes());
    let line = |shown: &str, text: &str| {
        let f = anthology_slop::check_all(Kind::Markdown, text);
        assert_eq!(f.len(), 1, "{f:?}");
        format!("{}\n", report::check_line(shown, &f[0]))
    };
    let run = fx.check(&["docs"], &fx.repo);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(run.stdout, line("docs/a.md", TELL));
    assert_eq!(run.stderr, "");
    // a file and a folder, from another folder with --root; files in path order
    let run = fx.check(&["--root", s(&fx.repo), "README.md", "docs"], &fx.base);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(
        run.stdout,
        format!("{}{}", line("README.md", TELL2), line("docs/a.md", TELL))
    );
    // an absolute path, from a folder above the repo: shown relative to that folder (the cwd)
    let run = fx.check(&[s(&fx.repo.join("docs/a.md"))], &fx.base);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(run.stdout, line("repo/docs/a.md", TELL));
}

#[test]
fn check_skips_nested_clones_and_shows_paths_from_the_root() {
    // review-2 M8: `check .` walked an ignored clone and showed its files from the clone's root
    let fx = Fx::new();
    fx.write(&fx.repo.join("docs/a.md"), TELL.as_bytes());
    fx.write(
        &fx.repo.join(".vendor-cache/lib/README.md"),
        TELL.as_bytes(),
    );
    std::fs::create_dir_all(fx.repo.join(".vendor-cache/lib/.git")).unwrap();
    let line = |shown: &str| {
        let f = anthology_slop::check_all(Kind::Markdown, TELL);
        format!("{}\n", report::check_line(shown, &f[0]))
    };
    let run = fx.check(&["."], &fx.repo);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(run.stdout, line("docs/a.md"));
    // a folder that holds a `.git` file (a submodule, a worktree) is not a clone: it is walked
    fx.write(
        &fx.repo.join("docs/theme/.git"),
        b"gitdir: ../../.git/modules/theme\n",
    );
    fx.write(&fx.repo.join("docs/theme/b.md"), TELL.as_bytes());
    let run = fx.check(&["docs"], &fx.repo);
    assert_eq!(
        run.stdout,
        format!("{}{}", line("docs/a.md"), line("docs/theme/b.md"))
    );
    // named on the command line, the clone is checked, and shown from the cwd, not its own root
    let run = fx.check(&[".vendor-cache/lib"], &fx.repo);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(run.stdout, line(".vendor-cache/lib/README.md"));
    let run = fx.check(
        &["--root", s(&fx.repo), ".vendor-cache/lib/README.md"],
        &fx.base,
    );
    assert_eq!(run.stdout, line(".vendor-cache/lib/README.md"));
}

#[test]
fn check_clean_exits_0() {
    let fx = Fx::new();
    fx.write(&fx.repo.join("docs/a.md"), CLEAN.as_bytes());
    fx.write(&fx.repo.join("docs/bad.md"), b"We delve \xff.\n");
    fx.write(&fx.repo.join(".claude/memory/x.md"), TELL.as_bytes());
    let run = fx.check(&["."], &fx.repo);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert_eq!(run.stderr, "");
}

#[test]
fn check_skips_fifos_and_devices_promptly() {
    let fx = Fx::new();
    fx.write(&fx.repo.join("docs/a.md"), TELL.as_bytes());
    mkfifo(&fx.repo.join("docs/fifo.md"));
    std::os::unix::fs::symlink("/dev/zero", fx.repo.join("docs/zero.md")).unwrap();
    let start = Instant::now();
    let run = fx.check(&["docs"], &fx.repo);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(run.stdout.lines().count(), 1, "{}", run.stdout);
    assert!(run.stdout.starts_with("docs/a.md:1: "), "{}", run.stdout);
    // named on the command line, they are still skipped
    let run = fx.check(&["docs/fifo.md", "docs/zero.md"], &fx.repo);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "");
    // and as untracked files of a --since check
    let repo = fx.git_repo();
    fx.write(&repo.join("base.txt"), b"x\n");
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "base"]);
    mkfifo(&repo.join("docs/fifo.md"));
    std::os::unix::fs::symlink("/dev/zero", repo.join("docs/zero.md")).unwrap();
    let run = fx.check(&["--root", s(&repo), "--since", "HEAD"], &fx.base);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert!(start.elapsed() < Duration::from_secs(20));
}

#[test]
fn check_since_reports_only_added_lines() {
    let fx = Fx::new();
    let repo = fx.git_repo();
    let file = repo.join("docs/a.md");
    fx.write(&file, TELL.as_bytes());
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "base"]);
    fx.write(&file, format!("{TELL}{TELL2}").as_bytes());
    let run = fx.check(&["--root", s(&repo), "--since", "HEAD"], &fx.base);
    assert_eq!(run.code, 1, "{}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{}", run.stdout);
    assert!(
        lines[0].starts_with("docs/a.md:2: text.chat-residue: \""),
        "{}",
        run.stdout
    );
    assert_eq!(run.stderr, "");

    // committed lines since the merge base count too; the root defaults to the current folder
    fx.git(&repo, &["tag", "base"]);
    fx.git(&repo, &["commit", "-q", "-am", "second"]);
    fx.write(
        &file,
        format!("{TELL}{TELL2}Middle line.\n{TELL}").as_bytes(),
    );
    let run = fx.check(&["--since", "base"], &repo.join("docs"));
    assert_eq!(run.code, 1, "{}", run.stderr);
    let got: Vec<&str> = run
        .stdout
        .lines()
        .map(|l| &l[..l.find(": \"").unwrap()])
        .collect();
    assert_eq!(
        got,
        [
            "docs/a.md:2: text.chat-residue",
            "docs/a.md:4: text.ai-vocab"
        ]
    );

    // nothing added: clean
    fx.git(&repo, &["commit", "-q", "-am", "third"]);
    let run = fx.check(&["--root", s(&repo), "--since", "HEAD"], &fx.base);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, "");

    // a deleted file and a removed line add nothing
    fx.write(&file, TELL.as_bytes());
    fx.write(&repo.join("docs/gone.md"), TELL.as_bytes());
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "fourth"]);
    std::fs::remove_file(repo.join("docs/gone.md")).unwrap();
    let run = fx.check(&["--root", s(&repo), "--since", "HEAD"], &fx.base);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert_eq!(run.stdout, "");
}

#[test]
fn check_since_handles_names_git_quotes() {
    let fx = Fx::new();
    let repo = fx.git_repo();
    fx.write(&repo.join("base.txt"), b"x\n");
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "base"]);
    for name in [
        "docs/with space.md",
        "docs/na\u{ef}ve.md",
        "docs/q\"uote.md",
    ] {
        fx.write(&repo.join(name), b"First line.\n");
    }
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "names"]);
    fx.git(&repo, &["tag", "names"]);
    for name in [
        "docs/with space.md",
        "docs/na\u{ef}ve.md",
        "docs/q\"uote.md",
    ] {
        fx.write(&repo.join(name), format!("First line.\n{TELL}").as_bytes());
    }
    let run = fx.check(&["--root", s(&repo), "--since", "names"], &fx.base);
    assert_eq!(run.code, 1, "{}", run.stderr);
    let got: Vec<&str> = run
        .stdout
        .lines()
        .map(|l| &l[..l.find(": \"").unwrap()])
        .collect();
    assert_eq!(
        got,
        [
            "docs/na\u{ef}ve.md:2: text.ai-vocab",
            "docs/q\"uote.md:2: text.ai-vocab",
            "docs/with space.md:2: text.ai-vocab",
        ]
    );
}

#[test]
fn check_since_ignores_the_users_diff_settings() {
    let fx = Fx::new();
    let repo = fx.git_repo();
    let file = repo.join("docs/a.md");
    // the tell on line 2 is old; lines 1 and 3 change around it
    fx.write(&file, format!("One.\n{TELL}Three.\n").as_bytes());
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "base"]);
    for (key, value) in [
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
        ("diff.interHunkContext", "10"),
        ("diff.context", "5"),
        ("core.quotePath", "false"),
        ("color.ui", "always"),
        ("color.diff", "always"),
    ] {
        fx.git(&repo, &["config", key, value]);
    }
    fx.write(
        &file,
        format!("One, changed.\n{TELL}Three, changed.\n").as_bytes(),
    );
    let run = fx.check(&["--root", s(&repo), "--since", "HEAD"], &fx.base);
    assert_eq!(run.code, 0, "{} {}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "");
    // a new tell is still found
    fx.write(&file, format!("One, changed.\n{TELL}{TELL2}").as_bytes());
    let run = fx.check(&["--root", s(&repo), "--since", "HEAD"], &fx.base);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(
        run.stdout.starts_with("docs/a.md:3: text.chat-residue: \""),
        "{}",
        run.stdout
    );
    assert_eq!(run.stdout.lines().count(), 1, "{}", run.stdout);
}

#[test]
fn check_since_includes_untracked_files() {
    let fx = Fx::new();
    let repo = fx.git_repo();
    fx.write(&repo.join(".gitignore"), b"ignored.md\n");
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "base"]);
    fx.write(
        &repo.join("docs/new.md"),
        format!("Intro.\n{TELL}").as_bytes(),
    );
    fx.write(&repo.join("ignored.md"), TELL.as_bytes());
    fx.write(&repo.join("node_modules/p/README.md"), TELL.as_bytes());
    let run = fx.check(&["--root", s(&repo), "--since", "HEAD"], &fx.base);
    assert_eq!(run.code, 1, "{}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 1, "{}", run.stdout);
    assert!(
        lines[0].starts_with("docs/new.md:2: text.ai-vocab: \""),
        "{}",
        run.stdout
    );
}

#[test]
fn check_since_gates_a_worktree_under_dot_claude() {
    let fx = Fx::new();
    let repo = fx.git_repo();
    fx.write(&repo.join("base.txt"), b"x\n");
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "base"]);
    let wt = repo.join(".claude/worktrees/w");
    fx.git(&repo, &["worktree", "add", "-q", s(&wt), "HEAD"]);
    fx.write(&wt.join("docs/a.md"), TELL.as_bytes());
    let run = fx.check(&["--root", s(&wt), "--since", "HEAD"], &fx.base);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(
        run.stdout.starts_with("docs/a.md:1: text.ai-vocab: \""),
        "{}",
        run.stdout
    );
}

#[test]
fn check_lists_waivers() {
    let fx = Fx::new();
    let waived = "We delve into it. <!-- slop-ok: text.ai-vocab the product is named Delve -->\n\
                  <!-- slop-ignore-start quoted from the brief -->\nI hope this helps.\n<!-- slop-ignore-end -->\n";
    fx.write(&fx.repo.join("docs/a.md"), waived.as_bytes());
    let run = fx.check(&["docs"], &fx.repo);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        run.stdout,
        "docs/a.md:1: slop-ok text.ai-vocab: the product is named Delve\n\
         docs/a.md:2: slop-ignore region\n"
    );
    // an unwaived finding sets the exit code; the findings come before the waivers
    fx.write(
        &fx.repo.join("docs/a.md"),
        format!("{waived}{TELL}").as_bytes(),
    );
    let run = fx.check(&["docs"], &fx.repo);
    assert_eq!(run.code, 1, "{}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.len(), 3, "{}", run.stdout);
    assert!(
        lines[0].starts_with("docs/a.md:5: text.ai-vocab: \""),
        "{}",
        run.stdout
    );
    assert_eq!(
        lines[1],
        "docs/a.md:1: slop-ok text.ai-vocab: the product is named Delve"
    );
    assert_eq!(lines[2], "docs/a.md:2: slop-ignore region");
}

#[test]
fn check_usage_errors_exit_2() {
    let fx = Fx::new();
    let cases: &[(&str, &[&str])] = &[
        ("no paths and no --since", &[]),
        ("an unknown flag", &["--bogus", "docs"]),
        ("--since with no value", &["--since"]),
        ("--root with no value", &["docs", "--root"]),
        (
            "a ref that looks like an option",
            &["--since", "--output=x"],
        ),
        ("--since and paths", &["--since", "HEAD", "docs"]),
        ("a path that does not exist", &["no-such-folder"]),
        ("--since outside a git repo", &["--since", "HEAD"]),
    ];
    for (ctx, args) in cases {
        let run = fx.check(args, &fx.repo);
        assert_eq!(run.code, 2, "{ctx}: {:?} {:?}", run.stdout, run.stderr);
        assert_eq!(run.stdout, "", "{ctx}");
        assert!(
            run.stderr.starts_with("anthology slop: "),
            "{ctx}: {:?}",
            run.stderr
        );
        assert!(
            run.stderr.ends_with('\n') && run.stderr.matches('\n').count() == 1,
            "{ctx}: {:?}",
            run.stderr
        );
    }
    // a ref git does not know
    let repo = fx.git_repo();
    fx.write(&repo.join("a.md"), CLEAN.as_bytes());
    fx.git(&repo, &["add", "-A"]);
    fx.git(&repo, &["commit", "-q", "-m", "base"]);
    let run = fx.check(&["--since", "no-such-ref"], &repo);
    assert_eq!(run.code, 2, "{:?}", run.stderr);
    assert!(
        run.stderr.starts_with("anthology slop: "),
        "{:?}",
        run.stderr
    );
    assert_eq!(run.stderr.matches('\n').count(), 1, "{:?}", run.stderr);
}

#[test]
fn check_escapes_control_characters_in_its_output() {
    let fx = Fx::new();
    let text =
        "We delve \u{1b}[31minto it. <!-- slop-ok: text.hype-cta say \u{1b}[2J goodbye to it -->\n";
    fx.write(&fx.repo.join("docs/a.md"), text.as_bytes());
    let run = fx.check(&["docs"], &fx.repo);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(!run.stdout.contains('\u{1b}'), "{:?}", run.stdout);
    assert_eq!(run.stdout.lines().count(), 2, "{:?}", run.stdout);
}

// ---- rules ----

#[test]
fn rules_prints_every_rule() {
    let fx = Fx::new();
    let run = fx.run(&["slop", "rules"], &fx.base, "", &[]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stdout, report::rules_table());
    assert_eq!(run.stdout.lines().count(), 16);
    assert!(
        run.stdout
            .starts_with("text.ai-vocab\tblock\tmarkdown,html,jsx\t")
    );
    assert_eq!(run.stderr, "");
}

#[test]
fn an_unknown_subcommand_is_a_usage_error() {
    let fx = Fx::new();
    let run = fx.run(&["slop", "nope"], &fx.base, "", &[]);
    assert_eq!(run.code, 2);
    assert!(
        run.stderr.contains("unknown command: slop nope"),
        "{}",
        run.stderr
    );
    let run = fx.run(&["slop"], &fx.base, "", &[]);
    assert_eq!(run.code, 2);
}
