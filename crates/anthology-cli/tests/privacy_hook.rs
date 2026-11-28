//! `anthology privacy hook`: the PreToolUse check of every `git commit`, run as the built binary
//! with a cleared environment, a sandbox home and config folder, and real temp repos. Exit 10
//! with a message on stderr blocks; every other outcome is exit 0 with no output.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");

/// The user name the tests give the binary (`USER`): a handle that is none of the generic ones.
const HANDLE: &str = "zedder";

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// A sandbox: a home, a config folder and a base folder that git never looks above.
struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    config: PathBuf,
}

impl Fx {
    fn new() -> Fx {
        let tmp = tempfile::tempdir().expect("temp dir");
        // The folder the OS reports, so a path from `rev-parse` equals the one the test holds.
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let home = base.join("zedhome");
        let config = base.join("config");
        for dir in [&home, &config] {
            std::fs::create_dir_all(dir).unwrap();
        }
        Fx {
            _tmp: tmp,
            base,
            home,
            config,
        }
    }

    /// A Git repo called `name`, with one clean tracked file `notes.md` committed.
    fn repo(&self, name: &str) -> PathBuf {
        let repo = self.base.join(name);
        std::fs::create_dir_all(&repo).unwrap();
        self.git(&repo, &["init", "-q"]);
        self.git(&repo, &["config", "user.name", "Test"]);
        self.git(&repo, &["config", "user.email", "tester@mailbox.dev"]);
        self.git(&repo, &["config", "commit.gpgsign", "false"]);
        self.write(&repo.join("notes.md"), "one\ntwo\n");
        self.git(&repo, &["add", "notes.md"]);
        self.git(&repo, &["commit", "-q", "-m", "first"]);
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

    fn write(&self, path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// The text of a line that holds the sandbox home.
    fn leak(&self) -> String {
        format!("see {}/proj/x", self.home.display())
    }

    /// Writes `notes.md` with a leaked home path on its second line and stages it.
    fn stage_leak(&self, repo: &Path) {
        self.write(
            &repo.join("notes.md"),
            &format!("one\n{}\nthree\n", self.leak()),
        );
        self.git(repo, &["add", "notes.md"]);
    }

    /// Changes the tracked `notes.md` to hold a leak, and stages nothing.
    fn dirty_leak(&self, repo: &Path) {
        self.write(
            &repo.join("notes.md"),
            &format!("one\n{}\nthree\n", self.leak()),
        );
    }

    /// The binary, with the hook input for `command` run from `cwd`.
    fn hook(&self, command: &str, cwd: &Path) -> Run {
        self.hook_env(command, cwd, &[])
    }

    fn hook_env(&self, command: &str, cwd: &Path, extra: &[(&str, &str)]) -> Run {
        self.run(&["privacy", "hook"], cwd, &input(command, cwd), extra)
    }

    fn run(&self, args: &[&str], cwd: &Path, input: &str, extra: &[(&str, &str)]) -> Run {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("USER", HANDLE)
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
        let _ = stdin.write_all(input.as_bytes());
        drop(stdin);
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
}

const RUN_LIMIT: Duration = Duration::from_secs(30);

fn drain(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
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

/// The PreToolUse input of a Bash call of `command`, made in `cwd`.
fn input(command: &str, cwd: &Path) -> String {
    format!(
        r#"{{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{{"command":{}}},"cwd":{}}}"#,
        js(command),
        js(cwd.to_str().unwrap())
    )
}

/// A shell word for a path: single-quoted.
fn q(path: &Path) -> String {
    format!("'{}'", path.display())
}

#[track_caller]
fn assert_allowed(run: &Run, what: &str) {
    assert_eq!(
        (run.code, run.stdout.as_str(), run.stderr.as_str()),
        (0, "", ""),
        "{what}"
    );
}

#[track_caller]
fn assert_blocked(run: &Run, fx: &Fx, what: &str) {
    assert_eq!(run.code, 10, "{what}: {}", run.stderr);
    assert!(run.stdout.is_empty(), "{what}: stdout {:?}", run.stdout);
    assert!(
        run.stderr
            .starts_with("Blocked by anthology (privacy): this commit adds "),
        "{what}: {}",
        run.stderr
    );
    // the message names a kind and a place, never the value
    let home = fx.home.display().to_string();
    assert!(!run.stderr.contains(&home), "{what}: {}", run.stderr);
}

#[test]
fn clean_commit_exit_0_silent() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.write(&repo.join("notes.md"), "one\ntwo\nthree\n");
    fx.git(&repo, &["add", "notes.md"]);
    assert_allowed(&fx.hook("git commit -m \"add a line\"", &repo), "clean");
}

#[test]
fn blocks_staged_home_path_exit_10() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.stage_leak(&repo);
    let run = fx.hook("git commit -m \"note\"", &repo);
    assert_blocked(&run, &fx, "staged");
    assert!(
        run.stderr.contains("home path (notes.md:2)"),
        "{}",
        run.stderr
    );
}

#[test]
fn blocks_handle_forms_from_the_user_variable() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.write(
        &repo.join("notes.md"),
        &format!("one\ncat /home/{HANDLE}/.ssh/config\n"),
    );
    fx.git(&repo, &["add", "notes.md"]);
    let run = fx.hook("git commit -m note", &repo);
    assert_blocked(&run, &fx, "handle form");
    assert!(!run.stderr.contains(HANDLE), "{}", run.stderr);
}

#[test]
fn blocks_message_home_path() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.write(&repo.join("notes.md"), "one\ntwo\nthree\n");
    fx.git(&repo, &["add", "notes.md"]);
    let cmd = format!(
        "git commit -m \"tidy\" -m \"ran in {}/proj\"",
        fx.home.display()
    );
    let run = fx.hook(&cmd, &repo);
    assert_blocked(&run, &fx, "message");
    assert!(
        run.stderr.contains("home path (commit message:3)"),
        "{}",
        run.stderr
    );
}

/// The usual way a message is written: a heredoc inside the `-m` word.
#[test]
fn a_heredoc_message_is_scanned() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.write(&repo.join("notes.md"), "one\ntwo\nthree\n");
    fx.git(&repo, &["add", "notes.md"]);
    let body = format!("tidy notes\n\nran in {}/proj\n", fx.home.display());
    let cmd = format!("git commit -m \"$(cat <<'EOF'\n{body}EOF\n)\"");
    let run = fx.hook(&cmd, &repo);
    assert_blocked(&run, &fx, "heredoc");
    assert!(
        run.stderr.contains("home path (commit message:"),
        "{}",
        run.stderr
    );
    let clean = "git commit -m \"$(cat <<'EOF'\ntidy notes\nEOF\n)\"";
    assert_allowed(&fx.hook(clean, &repo), "clean heredoc");
}

#[test]
fn all_flag_scans_unstaged() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.dirty_leak(&repo);
    // nothing is staged: a plain commit takes nothing of it
    assert_allowed(&fx.hook("git commit -m note", &repo), "unstaged, no -a");
    let run = fx.hook("git commit -am note", &repo);
    assert_blocked(&run, &fx, "-am");
    assert!(
        run.stderr.contains("home path (notes.md:2)"),
        "{}",
        run.stderr
    );
    for cmd in [
        "git commit -a -m note",
        "git commit --all --message note",
        "git commit -qa -m note",
        "git commit notes.md -m note",
        "git commit -m note -- notes.md",
    ] {
        assert_blocked(&fx.hook(cmd, &repo), &fx, cmd);
    }
    // another path: the leak in notes.md is not part of that commit
    fx.write(&repo.join("other.txt"), "x\n");
    fx.git(&repo, &["add", "other.txt"]);
    assert_allowed(
        &fx.hook("git commit -m note -- other.txt", &repo),
        "paths limit the unstaged diff",
    );
}

/// `git -C dir commit -am "msg"` and `cd dir && git commit -F msg.txt` scan the right repo, its
/// unstaged tracked changes and the message.
#[test]
fn finds_commit_calls() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    let elsewhere = fx.base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    fx.dirty_leak(&repo);

    let cmd = format!("git -C {} commit -am \"msg\"", q(&repo));
    assert_blocked(&fx.hook(&cmd, &elsewhere), &fx, "git -C dir commit -am");
    // `-C` is cumulative: the base folder is no repo, the base folder and then `proj` is
    let cmd = format!(
        "git -c user.name=x --no-pager -C {} commit -a -m 'msg'",
        q(&fx.base)
    );
    assert_allowed(&fx.hook(&cmd, &elsewhere), "base is not the repo");
    let cmd = format!(
        "git -c user.name=x --no-pager -C {} -C proj commit -a -m 'msg'",
        q(&fx.base)
    );
    assert_blocked(&fx.hook(&cmd, &elsewhere), &fx, "cumulative -C");

    // the message file, relative to the repo the command moved into
    fx.git(&repo, &["checkout", "-q", "--", "notes.md"]);
    fx.write(
        &repo.join("msg.txt"),
        &format!("fix\n\nin {}/proj\n", fx.home.display()),
    );
    let cmd = format!("cd {} && git commit -F msg.txt", q(&repo));
    let run = fx.hook(&cmd, &elsewhere);
    assert_blocked(&run, &fx, "cd dir && git commit -F");
    assert!(run.stderr.contains("commit message:3"), "{}", run.stderr);
    // a message file that does not exist is nothing to scan
    let cmd = format!("cd {} && git commit -F missing.txt", q(&repo));
    assert_allowed(&fx.hook(&cmd, &elsewhere), "missing -F file");

    // forms of the message option
    fx.write(&repo.join("notes.md"), "one\ntwo\nthree\n");
    fx.git(&repo, &["add", "notes.md"]);
    let leak = format!("{}/p", fx.home.display());
    for cmd in [
        format!("git commit -m'{leak}'"),
        format!("git commit --message='{leak}'"),
        format!("git commit --message '{leak}'"),
        format!("git commit -vm '{leak}'"),
        format!("git commit -F- -m '{leak}'"),
        format!("true; git commit -m '{leak}'"),
        format!("git commit -F - <<< '{leak}'"),
        format!("git commit -F - <<'EOF'\n{leak}\nEOF\n"),
    ] {
        assert_blocked(&fx.hook(&cmd, &repo), &fx, &cmd);
    }
    // no commit call, or one that makes no commit
    for cmd in [
        format!("echo git commit -m '{leak}'"),
        format!("git log -m '{leak}'"),
        format!("git commit --dry-run -m '{leak}'"),
        format!("git status && git diff -m '{leak}'"),
        // a heredoc that no commit reads
        format!("cat <<EOF > ../x.txt\n{leak}\nEOF\ngit commit -m plain"),
    ] {
        assert_allowed(&fx.hook(&cmd, &repo), &cmd);
    }
}

#[test]
fn not_a_repo_exit_0() {
    let fx = Fx::new();
    let plain = fx.base.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    let cmd = format!("git commit -am \"{}/p\"", fx.home.display());
    assert_allowed(&fx.hook(&cmd, &plain), "not a repo");
    // a folder that does not exist
    let gone = fx.base.join("gone");
    let run = fx.run(&["privacy", "hook"], &plain, &input(&cmd, &gone), &[]);
    assert_allowed(&run, "missing cwd");
}

#[test]
fn other_input_exit_0() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.stage_leak(&repo);
    let ok = |input: &str, what: &str| {
        assert_allowed(&fx.run(&["privacy", "hook"], &repo, input, &[]), what);
    };
    ok("", "empty input");
    ok("not json", "not json");
    ok(
        r#"{"tool_name":"Write","tool_input":{"command":"git commit -m x"}}"#,
        "not Bash",
    );
    ok(r#"{"tool_name":"Bash","tool_input":{}}"#, "no command");
    ok(
        r#"{"tool_name":"Bash","tool_input":{"command":7}}"#,
        "command not a string",
    );
    // an unsupported shape of the command: too complex to read, so allowed
    let cmd = format!(
        "eval \"$(printf 'git commit -m x')\" # {}",
        fx.home.display()
    );
    assert_allowed(&fx.hook(&cmd, &repo), "unreadable command");
}

#[test]
fn an_unknown_subcommand_is_a_usage_error() {
    let fx = Fx::new();
    let run = fx.run(&["privacy", "nope"], &fx.base, "", &[]);
    assert_eq!(run.code, 2);
    let run = fx.run(&["privacy"], &fx.base, "", &[]);
    assert_eq!(run.code, 2);
}

#[test]
fn the_allow_file_lets_a_listed_value_through() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.stage_leak(&repo);
    assert_blocked(&fx.hook("git commit -m note", &repo), &fx, "before");
    fx.write(
        &repo.join(".claude/privacy-allow.txt"),
        &format!("# the public sample\n\n{}/proj/x\n", fx.home.display()),
    );
    assert_allowed(&fx.hook("git commit -m note", &repo), "allowed");
}

#[test]
fn banned_names_block_in_the_diff_and_the_message() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.write(
        &fx.config
            .join("anthology/personal-memory/user-banned-names.md"),
        "---\nname: Banned\n---\n- Globex Corp\n",
    );
    fx.write(&repo.join("notes.md"), "one\ndone for globex corp\n");
    fx.git(&repo, &["add", "notes.md"]);
    let run = fx.hook("git commit -m note", &repo);
    assert_blocked(&run, &fx, "banned in diff");
    assert!(
        run.stderr.contains("banned name (notes.md:2)"),
        "{}",
        run.stderr
    );
    assert!(
        !run.stderr.to_lowercase().contains("globex"),
        "{}",
        run.stderr
    );
    fx.write(&repo.join("notes.md"), "one\ntwo\n");
    fx.git(&repo, &["add", "notes.md"]);
    let run = fx.hook("git commit -m 'for Globex Corp'", &repo);
    assert!(
        run.stderr.contains("banned name (commit message:1)"),
        "{}",
        run.stderr
    );
    assert_eq!(run.code, 10);
}

/// A commit in a worktree takes the main repo's name for the per-repo banned list.
#[test]
fn repo_name_from_a_worktree_is_the_main_repo() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    let tree = fx.base.join("wt1");
    fx.git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            tree.to_str().unwrap(),
            "-b",
            "side",
        ],
    );
    let names = fx.config.join("anthology/personal-memory");
    fx.write(&names.join("user-banned-names--wt1.md"), "- Umbrella\n");
    fx.write(&tree.join("notes.md"), "one\nfor umbrella\n");
    fx.git(&tree, &["add", "notes.md"]);
    // the list of the worktree's own folder name is not read
    assert_allowed(&fx.hook("git commit -m note", &tree), "folder name list");
    fx.write(&names.join("user-banned-names--proj.md"), "- Umbrella\n");
    let run = fx.hook("git commit -m note", &tree);
    assert_blocked(&run, &fx, "main repo name");
    assert!(
        run.stderr.contains("banned name (notes.md:2)"),
        "{}",
        run.stderr
    );
    // the list of another repo is not read either
    fx.write(&names.join("user-banned-names--proj.md"), "- Other\n");
    assert_allowed(&fx.hook("git commit -m note", &tree), "other repo list");
}

#[test]
fn a_leak_in_a_file_that_is_not_staged_does_not_block() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.write(&repo.join("scratch.txt"), &fx.leak());
    fx.write(&repo.join("notes.md"), "one\ntwo\nthree\n");
    fx.git(&repo, &["add", "notes.md"]);
    assert_allowed(&fx.hook("git commit -m note", &repo), "untracked file");
}

#[test]
fn the_git_email_is_a_value_that_blocks() {
    let fx = Fx::new();
    let repo = fx.repo("proj");
    fx.write(&repo.join("notes.md"), "one\nmail tester@mailbox.dev\n");
    fx.git(&repo, &["add", "notes.md"]);
    let run = fx.hook("git commit -m note", &repo);
    assert_blocked(&run, &fx, "git email");
    assert!(
        run.stderr.contains("user email (notes.md:2)"),
        "{}",
        run.stderr
    );
    assert!(!run.stderr.contains("tester@"), "{}", run.stderr);
}
