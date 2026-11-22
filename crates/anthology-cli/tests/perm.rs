//! `anthology perm classify` (post-release plan P4, design §2): the read-only path check behind the
//! permission mod. The binary reads `{"command","cwd","roots","read_rules"}` on stdin and prints the
//! allow token and the resolved paths the command reads, or exits 1 with one reason line and no
//! decision.
//!
//! Every refusal class has a test here, and each git configuration trap first shows, with the real
//! `git` on `PATH`, that the trap runs a marker program on the very subcommand the classifier proves.
//! Only then does the test check that `classify` gives no decision for it. A trap whose test did not
//! show the marker would prove nothing, so each demonstration asserts the marker.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_pycompat::json::{self, Map, PY_DEFAULT, Value};
use anthology_testkit::run::{Output, run_program_timeout};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");
const TOKEN: &str = "anthology-perm:read-only:v1\n";

/// Apple's own git-core gitconfig files, as `git config --show-origin` names them.
const APPLE_GITCONFIGS: &[&str] = &[
    "/Library/Developer/CommandLineTools/usr/share/git-core/gitconfig",
    "/Applications/Xcode.app/Contents/Developer/usr/share/git-core/gitconfig",
];

/// A scratch tree: `home` (the global git config lives there), `proj` (the one root), `marks` (where
/// marker programs record that they ran) and room beside them for things outside the root.
struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    root: PathBuf,
    marks: PathBuf,
}

impl Fx {
    fn new() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        // macOS: /var is a symlink to /private/var; the fixture works on the real path.
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let fx = Fx {
            home: base.join("home"),
            root: base.join("proj"),
            marks: base.join("marks"),
            base,
            _tmp: tmp,
        };
        for dir in [&fx.home, &fx.root, &fx.marks] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            fx.home.join(".gitconfig"),
            "[user]\n\tname = t\n\temail = t@example.invalid\n[init]\n\tdefaultBranch = main\n",
        )
        .unwrap();
        fx
    }

    /// The environment of the binary: `PATH` and a home of the fixture's own. Any `GIT_*`
    /// variable refuses, so the machine's system git config is read (see [`Fx::platform_keys`]).
    fn env(&self) -> Env {
        Env::default()
            .with("PATH", &std::env::var("PATH").unwrap_or_default())
            .with("HOME", self.home.to_str().unwrap())
    }

    /// The environment of the git the fixtures run: without the system config, so the machine's
    /// own `/etc/gitconfig` cannot change a fixture or a trap's demonstration.
    fn git_env(&self) -> Env {
        self.env().with("GIT_CONFIG_NOSYSTEM", "1")
    }

    /// The config keys git reads on this machine outside the fixture (the system file; Apple's
    /// git carries `credential.helper` in its own). With any, every git line gets no decision, so
    /// the git tests can then only check that it is refused; Linux CI has none.
    fn platform_keys(&self) -> Vec<String> {
        let out = Command::new("git")
            .args(["config", "--list", "--show-scope", "--show-origin"])
            .current_dir(&self.base)
            .env_clear()
            .envs(self.env().iter())
            .output()
            .unwrap();
        // The fixture's own global file holds only allowlisted keys; the rest is the machine's,
        // bar what Apple's own git-core gitconfig may hold (`credential.helper=osxkeychain`).
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| !line.starts_with("global\t"))
            .filter(|line| {
                let apple = APPLE_GITCONFIGS
                    .iter()
                    .any(|file| line.contains(&format!("\tfile:{file}\t")));
                !(apple
                    && (line.ends_with("\tcredential.helper=osxkeychain")
                        || line.contains("\tinit.defaultbranch=")))
            })
            .map(str::to_string)
            .collect()
    }

    /// A git line expected to be allowed: allowed, or on a machine with its own git config keys,
    /// refused for one of them.
    #[track_caller]
    fn assert_git_allow(&self, out: &Output, ctx: &str) -> Vec<String> {
        let platform = self.platform_keys();
        if platform.is_empty() {
            return assert_allow(out, ctx);
        }
        eprintln!("{ctx}: this machine's git config has {platform:?}; the allow is not checked");
        assert_refused(out, "git config key not on the allowlist", ctx);
        Vec::new()
    }

    /// A git line refused for `key`, or, on a machine with its own git config keys, for one of
    /// those (they are listed first).
    #[track_caller]
    fn assert_key_refused(&self, out: &Output, key: &str, ctx: &str) {
        let why = if self.platform_keys().is_empty() {
            format!("git config key not on the allowlist: {key}")
        } else {
            "git config key not on the allowlist: ".to_string()
        };
        assert_refused(out, &why, ctx);
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn path(&self, rel: &str) -> String {
        self.root.join(rel).to_str().unwrap().to_string()
    }

    fn command(&self, program: &str, dir: &Path, args: &[&str], extra: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(dir)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in self.git_env().iter() {
            cmd.env(k, v);
        }
        for (k, v) in extra {
            cmd.env(k, v);
        }
        cmd
    }

    /// Runs git in `dir` and asserts it succeeded.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self.command("git", dir, args, &[]).output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Runs git in `dir` (the trap's demonstration): whether it succeeds does not matter, only
    /// whether the marker ran.
    fn git_try(&self, dir: &Path, args: &[&str], extra: &[(&str, &str)]) {
        let _ = self.command("git", dir, args, extra).output().unwrap();
    }

    fn git_stdin(&self, dir: &Path, args: &[&str], input: &str, extra: &[(&str, &str)]) {
        use std::io::Write;
        let mut cmd = self.command("git", dir, args, extra);
        cmd.stdin(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let _ = child.wait_with_output().unwrap();
    }

    /// A marker program: it records `name` in `marks`, then behaves like `cat`.
    fn marker(&self, name: &str) -> String {
        let script = self.base.join(format!("{name}.sh"));
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n: > '{}'\nexec cat \"$@\" 2>/dev/null\n",
                self.marks.join(name).display()
            ),
        )
        .unwrap();
        chmod_x(&script);
        script.to_str().unwrap().to_string()
    }

    fn ran(&self, name: &str) -> bool {
        self.marks.join(name).exists()
    }

    /// A repository at `root/rel` with one commit of `a.txt`.
    fn repo(&self, rel: &str) -> PathBuf {
        let dir = self.root.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        self.git(&dir, &["init", "-q"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        self.git(&dir, &["add", "a.txt"]);
        self.git(&dir, &["commit", "-qm", "one"]);
        dir
    }

    /// `command` run in `cwd`: the line is sent as `cd CWD && COMMAND`, since only a leading
    /// absolute `cd` tells the check where the shell is.
    fn classify(&self, cwd: &Path, command: &str, rules: &[&str]) -> Output {
        self.classify_env(cwd, command, rules, &[])
    }

    fn classify_env(
        &self,
        cwd: &Path,
        command: &str,
        rules: &[&str],
        extra: &[(&str, &str)],
    ) -> Output {
        let line = format!("cd {} && {command}", cwd.display());
        self.classify_raw(cwd, &line, rules, extra)
    }

    /// `line` sent as it is, with the fixture's root as the one root.
    fn classify_raw(
        &self,
        cwd: &Path,
        line: &str,
        rules: &[&str],
        extra: &[(&str, &str)],
    ) -> Output {
        let mut env = self.env();
        for (k, v) in extra {
            env = env.with(k, v);
        }
        let input = request(line, cwd, &[&self.root], rules);
        run(&input, &env)
    }
}

fn chmod_x(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn strings(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|s| Value::Str(s.to_string())).collect())
}

fn request(command: &str, cwd: &Path, roots: &[&Path], rules: &[&str]) -> Vec<u8> {
    let mut m = Map::new();
    m.insert("command".into(), Value::Str(command.into()));
    m.insert("cwd".into(), Value::Str(cwd.to_str().unwrap().into()));
    let roots: Vec<&str> = roots.iter().map(|r| r.to_str().unwrap()).collect();
    m.insert("roots".into(), strings(&roots));
    m.insert("read_rules".into(), strings(rules));
    json::dumps(&Value::Object(m), &PY_DEFAULT).into_bytes()
}

fn run(stdin: &[u8], env: &Env) -> Output {
    run_args(&["perm", "classify"], stdin, env)
}

fn run_args(args: &[&str], stdin: &[u8], env: &Env) -> Output {
    run_program_timeout(
        Path::new(BIN),
        args,
        stdin,
        env,
        Path::new("/"),
        Some(Duration::from_secs(60)),
    )
}

/// Allowed: exit 0, no stderr, the token line, then the paths read, one per line, each absolute
/// and resolved. Returns the paths.
#[track_caller]
fn assert_allow(out: &Output, ctx: &str) -> Vec<String> {
    let text = out.stdout_text();
    assert_eq!(
        (out.code, out.stderr_text().as_str()),
        (0, ""),
        "{ctx}: {text}"
    );
    let rest = text
        .strip_prefix(TOKEN)
        .unwrap_or_else(|| panic!("{ctx}: {text:?}"));
    let paths: Vec<String> = rest.lines().map(str::to_string).collect();
    assert!(rest.is_empty() || rest.ends_with('\n'), "{ctx}: {text:?}");
    for path in &paths {
        assert!(path.starts_with('/'), "{ctx}: {path}");
        assert_eq!(
            std::fs::canonicalize(path).unwrap(),
            PathBuf::from(path),
            "{ctx}: resolved"
        );
    }
    paths
}

/// No decision: exit 1, nothing on stdout, one reason line on stderr that holds `why`.
#[track_caller]
fn assert_refused(out: &Output, why: &str, ctx: &str) {
    let err = out.stderr_text();
    assert_eq!(out.code, 1, "{ctx}: {err}");
    assert!(
        out.stdout.is_empty(),
        "{ctx}: stdout {:?}",
        out.stdout_text()
    );
    assert_eq!(err.lines().count(), 1, "{ctx}: one line: {err:?}");
    assert!(
        err.starts_with("anthology perm: no decision: "),
        "{ctx}: {err}"
    );
    assert!(err.contains(why), "{ctx}: want {why:?} in {err:?}");
}

// --- the protocol -------------------------------------------------------------------------------------------

#[test]
fn a_proof_prints_the_token_then_every_resolved_path_read() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    fx.write("b.txt", "b\n");
    fx.write("src/x.rs", "x\n");
    let root = fx.path("");
    let root = root.trim_end_matches('/').to_string();
    assert_eq!(
        assert_allow(&fx.classify(&fx.root, "cat ./a.txt ./b.txt", &[]), "cat"),
        [root.clone(), fx.path("a.txt"), fx.path("b.txt")]
    );
    assert_eq!(
        assert_allow(&fx.classify(&fx.root, "ls -la", &[]), "ls"),
        vec![root.clone()],
        "the cd target and the implicit `.` are one path"
    );
    assert_eq!(
        assert_allow(&fx.classify(&fx.root, "rg x ./src", &[]), "rg"),
        [root.clone(), fx.path("src")],
        "a recursive reader lists its start"
    );
    assert_eq!(
        assert_allow(
            &fx.classify(&fx.root, "cat ./a.txt | head -n 3", &[]),
            "pipe"
        ),
        [root.clone(), fx.path("a.txt")]
    );
    symlink(fx.root.join("b.txt"), fx.root.join("link")).unwrap();
    assert_eq!(
        assert_allow(&fx.classify(&fx.root, "cat ./link", &[]), "link"),
        [root.clone(), fx.path("b.txt")],
        "a link is listed as its target"
    );
    let absolute = format!("cat {}", fx.path("a.txt"));
    assert_eq!(
        assert_allow(&fx.classify_raw(&fx.root, &absolute, &[], &[]), "absolute"),
        [fx.path("a.txt")],
        "an absolute path needs no cd"
    );
}

#[test]
fn usage_errors_exit_2_without_reading_stdin() {
    let fx = Fx::new();
    for args in [&["perm"][..], &["perm", "nope"], &["perm", "classify", "x"]] {
        let out = run_args(args, b"", &fx.env());
        assert_eq!(out.code, 2, "{args:?}: {}", out.stderr_text());
        assert!(out.stdout.is_empty(), "{args:?}");
    }
    let usage = run_args(&["--help"], b"", &fx.env()).stdout_text();
    assert!(usage.contains("perm classify"), "{usage}");
}

#[test]
fn input_that_is_not_the_request_shape_gives_no_decision() {
    let fx = Fx::new();
    let root = fx.root.to_str().unwrap();
    let cases: &[(&str, String)] = &[
        ("not json", "{".into()),
        ("not an object", "[]".into()),
        (
            "missing command",
            format!(r#"{{"cwd":"{root}","roots":["{root}"],"read_rules":[]}}"#),
        ),
        (
            "missing read_rules",
            format!(r#"{{"command":"ls","cwd":"{root}","roots":["{root}"]}}"#),
        ),
        (
            "number",
            format!(r#"{{"command":1,"cwd":"{root}","roots":["{root}"],"read_rules":[]}}"#),
        ),
        (
            "root not a string",
            format!(r#"{{"command":"ls","cwd":"{root}","roots":[1],"read_rules":[]}}"#),
        ),
        (
            "unknown key",
            format!(
                r#"{{"command":"ls","cwd":"{root}","roots":["{root}"],"read_rules":[],"allow":true}}"#
            ),
        ),
    ];
    for (ctx, input) in cases {
        assert_refused(&run(input.as_bytes(), &fx.env()), "input", ctx);
    }
    let huge = format!(
        r#"{{"command":"ls","cwd":"{root}","roots":["{root}"],"read_rules":["{}"]}}"#,
        "x".repeat(2 << 20)
    );
    assert_refused(
        &run(huge.as_bytes(), &fx.env()),
        "input",
        "over the size cap",
    );
}

#[test]
fn a_command_the_classifier_does_not_prove_gives_no_decision() {
    let fx = Fx::new();
    for command in [
        "rm -rf ./x",
        "echo hi > ./f",
        "cat ./a.txt; rm ./a.txt",
        "git diff main -- ./a.txt",
    ] {
        assert_refused(&fx.classify(&fx.root, command, &[]), "not proven", command);
    }
}

#[test]
fn a_command_with_invisible_or_non_ascii_characters_gives_no_decision() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    for command in [
        "cat './a\u{200b}.txt'",
        "cat './caf\u{e9}.txt'",
        "cat './a.txt\u{202e}'",
        "cat './a\u{feff}.txt'",
        "cat ./a.txt\u{0}",
    ] {
        assert_refused(
            &fx.classify(&fx.root, command, &[]),
            "not printable ASCII",
            command,
        );
    }
}

#[test]
fn a_relative_path_entry_gives_no_decision() {
    // `PATH=.:…` would let a file called `ls` or `git` in the working directory run instead.
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    let path = std::env::var("PATH").unwrap_or_default();
    for bad in [
        format!(".:{path}"),
        format!("{path}::"),
        format!("bin:{path}"),
    ] {
        assert_refused(
            &fx.classify_env(&fx.root, "cat ./a.txt", &[], &[("PATH", &bad)]),
            "PATH",
            &bad,
        );
    }
}

// --- the working directory ----------------------------------------------------------------------------------

#[test]
fn without_a_leading_absolute_cd_only_absolute_paths_are_allowed() {
    // The request's cwd is not trusted to be the shell's (subagents, worktrees).
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    fx.write("sub/b.txt", "b\n");
    let repo = fx.repo("r");
    for (cwd, line, why) in [
        (&fx.root, "cat ./a.txt", "a relative path"),
        (&fx.root, "cat a.txt", "a relative path"),
        (&fx.root, "ls", "a relative path"),
        (&fx.root, "rg x", "a relative path"),
        (&fx.root, "cd ./sub && cat ./b.txt", "a relative cd target"),
        (
            &repo,
            "git status",
            "git, and the shell's working directory",
        ),
    ] {
        assert_refused(&fx.classify_raw(cwd, line, &[], &[]), why, line);
    }
    let line = format!("cd {} && cd ./sub && cat ./b.txt", fx.root.display());
    assert_allow(
        &fx.classify_raw(&fx.root, &line, &[], &[]),
        "anchored chain",
    );
}

// --- roots and containment ----------------------------------------------------------------------------------

#[test]
fn roots_and_the_cwd_must_be_real_absolute_directories() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    let cwd = fx.root.to_str().unwrap();
    // The binary runs in `/`, so this relative path names the root: only the absolute rule refuses.
    let relative = cwd.trim_start_matches('/');
    let command = format!("cat {cwd}/a.txt");
    for (ctx, roots, why) in [
        ("no root", vec![], "root"),
        (
            "relative root",
            vec![relative],
            "root is not an absolute path",
        ),
        ("missing root", vec!["/nonexistent-anthology-root"], "root"),
    ] {
        let input = format!(
            r#"{{"command":"{command}","cwd":"{cwd}","roots":{},"read_rules":[]}}"#,
            json::dumps(&strings(&roots), &PY_DEFAULT)
        );
        assert_refused(&run(input.as_bytes(), &fx.env()), why, ctx);
    }
    let input = request(&command, Path::new(relative), &[&fx.root], &[]);
    assert_refused(
        &run(&input, &fx.env()),
        "cwd is not an absolute path",
        "relative cwd",
    );
}

#[test]
fn a_path_outside_every_root_gives_no_decision() {
    let fx = Fx::new();
    std::fs::write(fx.base.join("outside.txt"), "o\n").unwrap();
    let outside = fx.base.join("outside.txt");
    let command = format!("cat {}", outside.display());
    assert_refused(&fx.classify(&fx.root, &command, &[]), "outside", "absolute");
    // A second root that holds it makes it fine.
    let input = request(&command, &fx.root, &[&fx.root, &fx.base], &[]);
    assert_allow(&run(&input, &fx.env()), "second root");
}

#[test]
fn a_symlink_out_of_the_root_is_followed_before_the_check() {
    let fx = Fx::new();
    std::fs::write(fx.base.join("outside.txt"), "o\n").unwrap();
    symlink(fx.base.join("outside.txt"), fx.root.join("link")).unwrap();
    assert_refused(
        &fx.classify(&fx.root, "cat ./link", &[]),
        "outside",
        "file link",
    );
    symlink(&fx.base, fx.root.join("up")).unwrap();
    assert_refused(
        &fx.classify(&fx.root, "cat ./up/outside.txt", &[]),
        "outside",
        "directory link",
    );
}

#[test]
fn a_glob_gives_no_decision() {
    // What the shell expands a glob to (dotglob, nullglob, files made meanwhile) is not known here.
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    assert_refused(&fx.classify(&fx.root, "cat ./*.txt", &[]), "a glob", "star");
    // The classifier may refuse some of these shapes first; either way there is no decision.
    for command in ["ls ./a?txt", "cat ./[a].txt", "ls ./**/a.txt"] {
        assert_refused(&fx.classify(&fx.root, command, &[]), "", command);
    }
}

#[test]
fn a_missing_path_or_a_dangling_symlink_gives_no_decision() {
    let fx = Fx::new();
    assert_refused(
        &fx.classify(&fx.root, "cat ./missing.txt", &[]),
        "does not exist",
        "missing",
    );
    symlink(fx.base.join("not-there"), fx.root.join("dangling")).unwrap();
    assert_refused(
        &fx.classify(&fx.root, "cat ./dangling", &[]),
        "dangling",
        "dangling",
    );
    assert_refused(
        &fx.classify(&fx.root, "cat ./dangling/x.txt", &[]),
        "does not exist",
        "dangling on the way",
    );
}

#[test]
fn a_special_file_gives_no_decision() {
    let fx = Fx::new();
    let fifo = fx.root.join("pipe");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());
    assert_refused(
        &fx.classify(&fx.root, "cat ./pipe", &[]),
        "special file",
        "fifo",
    );
    std::fs::create_dir_all(fx.root.join("d")).unwrap();
    std::fs::rename(&fifo, fx.root.join("d/pipe")).unwrap();
    assert_refused(
        &fx.classify(&fx.root, "rg x ./d", &[]),
        "special file",
        "fifo in a tree",
    );
}

#[test]
fn cd_targets_are_followed_and_checked() {
    let fx = Fx::new();
    fx.write("sub/x.txt", "x\n");
    assert_allow(
        &fx.classify(&fx.root, "cd ./sub && cat ./x.txt", &[]),
        "cd inside",
    );
    let command = format!("cd {} && ls", fx.base.display());
    assert_refused(
        &fx.classify(&fx.root, &command, &[]),
        "outside",
        "cd outside",
    );
    assert_refused(
        &fx.classify(&fx.root, "cd ./nope && ls", &[]),
        "cd target",
        "cd to a missing directory",
    );
    symlink(&fx.base, fx.root.join("out")).unwrap();
    assert_refused(
        &fx.classify(&fx.root, "cd ./out && ls", &[]),
        "outside",
        "cd via a link",
    );
    // The target is checked itself, not only through the paths read after it.
    assert_refused(
        &fx.classify(&fx.root, "cd ./out && pwd", &[]),
        "outside",
        "cd, no path",
    );
}

#[cfg(target_os = "macos")]
#[test]
fn containment_ignores_case_on_macos() {
    // The default APFS volume ignores case, so a root given in another case still holds the path.
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    let upper = PathBuf::from(fx.root.to_str().unwrap().to_uppercase());
    if !upper.is_dir() {
        eprintln!("a case-sensitive volume: nothing to check");
        return;
    }
    let command = format!("cat {}", fx.path("a.txt"));
    let input = request(&command, &fx.root, &[&upper], &[]);
    assert_allow(&run(&input, &fx.env()), "root in upper case");
}

// --- secrets ------------------------------------------------------------------------------------------------

#[test]
fn a_link_to_a_secret_is_judged_by_its_target() {
    let fx = Fx::new();
    fx.write(".env", "K=v\n");
    symlink(fx.root.join(".env"), fx.root.join("innocent")).unwrap();
    assert_refused(
        &fx.classify(&fx.root, "cat ./innocent", &[]),
        "secret",
        "link to .env",
    );
}

#[test]
fn a_recursive_read_that_reaches_a_secret_gives_no_decision() {
    let fx = Fx::new();
    fx.write("src/main.rs", "fn main() {}\n");
    fx.write("src/deep/er/id_ed25519", "k\n");
    for command in [
        "rg main ./src",
        "grep -r main ./src",
        "find ./src",
        "du ./src",
    ] {
        assert_refused(&fx.classify(&fx.root, command, &[]), "secret", command);
    }
    std::fs::remove_file(fx.root.join("src/deep/er/id_ed25519")).unwrap();
    assert_allow(
        &fx.classify(&fx.root, "rg main ./src", &[]),
        "no secret left",
    );
}

#[test]
fn a_recursive_read_of_a_tree_too_large_to_check_gives_no_decision() {
    let fx = Fx::new();
    let big = fx.root.join("big");
    std::fs::create_dir_all(&big).unwrap();
    for i in 0..20_001 {
        std::fs::write(big.join(i.to_string()), "").unwrap();
    }
    assert_refused(
        &fx.classify(&fx.root, "rg x ./big", &[]),
        "too many",
        "20001 files",
    );
}

// --- Read() rules -------------------------------------------------------------------------------------------

#[test]
fn a_read_rule_on_the_path_gives_no_decision() {
    let fx = Fx::new();
    fx.write("secret.txt", "s\n");
    fx.write("other.txt", "o\n");
    let rule = &["Read(./secret.txt)"];
    assert_refused(
        &fx.classify(&fx.root, "cat ./secret.txt", rule),
        "Read rule",
        "plain",
    );
    assert_allow(
        &fx.classify(&fx.root, "cat ./other.txt", rule),
        "other file",
    );
    assert_refused(
        &fx.classify(&fx.root, "cat < ./secret.txt", rule),
        "Read rule",
        "input redirect",
    );
    assert_refused(
        &fx.classify(&fx.root, "cat ./secret.txt", &["Read(./Secret.TXT)"]),
        "Read rule",
        "rule case",
    );
}

#[test]
fn a_read_rule_sees_through_links_and_cd() {
    let fx = Fx::new();
    fx.write("secret.txt", "s\n");
    fx.write("sub/x.txt", "x\n");
    symlink(fx.root.join("secret.txt"), fx.root.join("alias")).unwrap();
    assert_refused(
        &fx.classify(&fx.root, "cat ./alias", &["Read(./secret.txt)"]),
        "Read rule",
        "link",
    );
    assert_refused(
        &fx.classify(&fx.root, "cd ./sub && cat ./x.txt", &["Read(./sub/x.txt)"]),
        "Read rule",
        "after cd",
    );
    assert_refused(
        &fx.classify(&fx.root, "cd ./sub && ls", &["Read(./sub)"]),
        "Read rule",
        "the cd target",
    );
}

#[test]
fn every_rule_anchor_is_honoured() {
    let fx = Fx::new();
    let file = fx.write("conf/app.cfg", "c\n");
    let abs = format!("Read(/{})", file.display());
    let home_file = fx.home.join("notes.txt");
    std::fs::write(&home_file, "n\n").unwrap();
    for (rule, ctx) in [
        (abs.as_str(), "// absolute"),
        ("Read(/conf/app.cfg)", "/ the project root"),
        ("Read(conf/app.cfg)", "relative to the cwd"),
        ("Read(app.cfg)", "a name at any depth"),
        ("Read(*.cfg)", "a glob at any depth"),
        ("Read(./conf)", "a directory"),
        ("Read(./conf/**)", "a double star"),
        ("Read(./c?nf/*.cfg)", "glob characters"),
    ] {
        assert_refused(
            &fx.classify(&fx.root, "cat ./conf/app.cfg", &[rule]),
            "Read rule",
            ctx,
        );
    }
    let input = request(
        &format!("cat {}", home_file.display()),
        &fx.root,
        &[&fx.root, &fx.home],
        &["Read(~/notes.txt)"],
    );
    assert_refused(&run(&input, &fx.env()), "Read rule", "~ the home");
}

#[test]
fn a_recursive_read_that_could_reach_a_denied_path_gives_no_decision() {
    let fx = Fx::new();
    fx.write("src/a.rs", "x\n");
    fx.write("src/private/key.cfg", "k\n");
    for rule in [
        "Read(./src/private/**)",
        "Read(*.cfg)",
        "Read(./src/private)",
    ] {
        assert_refused(
            &fx.classify(&fx.root, "rg x ./src", &[rule]),
            "Read rule",
            rule,
        );
        assert_refused(&fx.classify(&fx.root, "ls -R", &[rule]), "Read rule", rule);
    }
    // An anchored rule below the tree refuses even when nothing it names exists yet.
    assert_refused(
        &fx.classify(&fx.root, "rg x ./src", &["Read(./src/later/**)"]),
        "could match below",
        "nothing there yet",
    );
    // The rule names nothing under the tree.
    assert_allow(
        &fx.classify(&fx.root, "rg x ./src", &["Read(./elsewhere/**)"]),
        "nothing under the tree",
    );
}

#[test]
fn a_bare_read_rule_or_one_this_check_does_not_know_gives_no_decision() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    for rule in ["Read", "Read()", "Read(**)", "Read(*)"] {
        assert_refused(
            &fx.classify(&fx.root, "cat ./a.txt", &[rule]),
            "every read",
            rule,
        );
    }
    for rule in ["Edit(./a.txt)", "Read(./a.txt", "read(./a.txt)", ""] {
        assert_refused(&fx.classify(&fx.root, "cat ./a.txt", &[rule]), "rule", rule);
    }
    assert_allow(
        &fx.classify(&fx.root, "cat ./a.txt", &["Read(./b.txt)"]),
        "an unrelated rule",
    );
}

// --- git: what is allowed -----------------------------------------------------------------------------------

#[test]
fn git_in_a_repository_with_only_safe_keys_is_allowed() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    fx.git(
        &repo,
        &["remote", "add", "origin", "https://example.invalid/r.git"],
    );
    fx.git(&repo, &["config", "branch.main.remote", "origin"]);
    fx.git(
        &repo,
        &["config", "branch.main.vscode-merge-base", "origin/main"],
    );
    for command in [
        "git status",
        "git log --oneline",
        "git diff",
        "git --no-pager log -n 1",
        "git branch -a",
        "git remote -v",
    ] {
        fx.assert_git_allow(&fx.classify(&repo, command, &[]), command);
    }
    let paths = fx.assert_git_allow(&fx.classify(&repo, "git status", &[]), "paths");
    if !paths.is_empty() {
        let top = repo.to_str().unwrap().to_string();
        let git_dir = repo.join(".git").to_str().unwrap().to_string();
        assert_eq!(paths, [top, git_dir], "the work tree and the git dir");
    }
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    fx.assert_git_allow(
        &fx.classify(&repo.join("sub"), "git status", &[]),
        "a subdirectory",
    );
    fx.assert_git_allow(
        &fx.classify(&fx.root, "cd ./r && git status", &[]),
        "after a cd",
    );
}

#[test]
fn gitattributes_without_a_driver_in_the_config_runs_nothing_and_is_allowed() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    std::fs::write(
        repo.join(".gitattributes"),
        "*.txt filter=f diff=c merge=m\n",
    )
    .unwrap();
    fx.git(&repo, &["add", ".gitattributes"]);
    fx.git(&repo, &["commit", "-qm", "attrs"]);
    std::fs::write(repo.join("a.txt"), "a\nb\n").unwrap();
    fx.git_try(&repo, &["status"], &[]);
    fx.git_try(&repo, &["diff"], &[]);
    fx.git_try(&repo, &["log", "-p"], &[]);
    assert_eq!(std::fs::read_dir(&fx.marks).unwrap().count(), 0);
    fx.assert_git_allow(&fx.classify(&repo, "git diff", &[]), "attributes alone");
}

// --- git: the traps -----------------------------------------------------------------------------------------

/// Sets `key` to a marker in the repository's config, shows that `demo` runs it, and checks that
/// `command` gets no decision naming the key.
fn config_trap(fx: &Fx, repo: &Path, key: &str, demo: &[&str], command: &str) {
    let name = key.replace('.', "_");
    fx.git(repo, &["config", key, &fx.marker(&name)]);
    fx.git_try(repo, demo, &[]);
    assert!(fx.ran(&name), "{key}: the trap did not run on git {demo:?}");
    fx.assert_key_refused(
        &fx.classify(repo, command, &[]),
        &key.to_ascii_lowercase(),
        key,
    );
}

#[test]
fn trap_core_fsmonitor_runs_on_git_status() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    config_trap(&fx, &repo, "core.fsmonitor", &["status"], "git status");
}

#[test]
fn trap_diff_external_runs_on_git_diff() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    std::fs::write(repo.join("a.txt"), "a\nb\n").unwrap();
    config_trap(&fx, &repo, "diff.external", &["diff"], "git diff");
}

#[test]
fn trap_textconv_runs_on_git_diff() {
    // `git log -p` runs it too, and refuses on its own (it prints history).
    let fx = Fx::new();
    let repo = fx.repo("r");
    std::fs::write(repo.join(".gitattributes"), "*.txt diff=c\n").unwrap();
    fx.git(&repo, &["add", "."]);
    fx.git(&repo, &["commit", "-qm", "attributes"]);
    std::fs::write(repo.join("a.txt"), "a\nb\n").unwrap();
    config_trap(&fx, &repo, "diff.c.textconv", &["diff"], "git diff");
}

#[test]
fn trap_filter_clean_runs_on_git_diff() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    std::fs::write(repo.join(".gitattributes"), "*.txt filter=f\n").unwrap();
    std::fs::write(repo.join("a.txt"), "a\nb\n").unwrap();
    config_trap(&fx, &repo, "filter.f.clean", &["diff"], "git diff");
}

#[test]
fn trap_core_ssh_command_runs_on_a_fetch() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    config_trap(
        &fx,
        &repo,
        "core.sshCommand",
        &["ls-remote", "ssh://example.invalid/x"],
        "git log",
    );
}

#[test]
fn trap_alias_runs_a_program() {
    // An alias never replaces a builtin (`alias.status` is ignored), so this shows `!` aliases run
    // programs at all; the key is refused whatever it names.
    let fx = Fx::new();
    let repo = fx.repo("r");
    fx.git(
        &repo,
        &["config", "alias.zz", &format!("!{}", fx.marker("alias"))],
    );
    fx.git_try(&repo, &["zz"], &[]);
    assert!(fx.ran("alias"));
    fx.assert_key_refused(&fx.classify(&repo, "git status", &[]), "alias.zz", "alias");
}

#[test]
fn trap_credential_helper_runs_a_program() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    fx.git(&repo, &["config", "credential.helper", &fx.marker("cred")]);
    fx.git_stdin(
        &repo,
        &["credential", "fill"],
        "protocol=https\nhost=example.invalid\n\n",
        &[("GIT_TERMINAL_PROMPT", "0")],
    );
    assert!(fx.ran("cred"));
    fx.assert_key_refused(
        &fx.classify(&repo, "git log", &[]),
        "credential.helper",
        "credential.helper",
    );
}

#[test]
fn trap_core_git_proxy_runs_a_program() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    config_trap(
        &fx,
        &repo,
        "core.gitProxy",
        &["ls-remote", "git://example.invalid/x"],
        "git log",
    );
}

#[test]
fn trap_protocol_ext_allow_runs_a_program() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    fx.git(&repo, &["config", "protocol.ext.allow", "always"]);
    let url = format!("ext::sh -c touch% {}", fx.marks.join("ext").display());
    fx.git_try(&repo, &["ls-remote", &url], &[]);
    assert!(fx.ran("ext"));
    fx.assert_key_refused(
        &fx.classify(&repo, "git log", &[]),
        "protocol.ext.allow",
        "protocol",
    );
}

#[test]
fn trap_core_hooks_path_runs_a_hook_on_git_status() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    let hooks = fx.base.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::copy(fx.marker("hookspath"), hooks.join("post-index-change")).unwrap();
    chmod_x(&hooks.join("post-index-change"));
    fx.git(
        &repo,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    touch_later(&repo.join("a.txt"));
    fx.git_try(&repo, &["status"], &[]);
    assert!(fx.ran("hookspath"));
    fx.assert_key_refused(
        &fx.classify(&repo, "git status", &[]),
        "core.hookspath",
        "core.hooksPath",
    );
}

#[test]
fn trap_a_hook_in_the_default_folder_runs_on_git_status() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    let hook = repo.join(".git/hooks/post-index-change");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::copy(fx.marker("pic"), &hook).unwrap();
    chmod_x(&hook);
    touch_later(&repo.join("a.txt"));
    fx.git_try(&repo, &["status"], &[]);
    assert!(fx.ran("pic"));
    if fx.platform_keys().is_empty() {
        assert_refused(&fx.classify(&repo, "git status", &[]), "git hook", "hook");
    }
}

/// Sets the file's modification time a few seconds ahead, so git sees a changed stat and writes
/// the index (the write runs `post-index-change`).
fn touch_later(path: &Path) {
    let later = std::time::SystemTime::now() + Duration::from_secs(5);
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(later).unwrap();
}

#[test]
fn trap_a_partial_clone_fetches_with_upload_pack_on_git_log_p() {
    let fx = Fx::new();
    let src = fx.repo("src");
    std::fs::write(src.join("big.txt"), "big\n").unwrap();
    fx.git(&src, &["add", "big.txt"]);
    fx.git(&src, &["commit", "-qm", "big"]);
    fx.git(&src, &["config", "uploadpack.allowFilter", "true"]);
    let url = format!("file://{}", src.display());
    let clone = fx.root.join("pc");
    fx.git(
        &fx.root,
        &[
            "clone",
            "-q",
            "--filter=blob:none",
            "--no-checkout",
            &url,
            "pc",
        ],
    );
    let wrapper = fx.base.join("up.sh");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\n: > '{}'\nexec git-upload-pack \"$@\"\n",
            fx.marks.join("uploadpack").display()
        ),
    )
    .unwrap();
    chmod_x(&wrapper);
    fx.git(
        &clone,
        &[
            "config",
            "remote.origin.uploadpack",
            wrapper.to_str().unwrap(),
        ],
    );
    fx.git_try(&clone, &["log", "-p"], &[]);
    assert!(fx.ran("uploadpack"), "the lazy fetch ran upload-pack");
    assert_refused(&fx.classify(&clone, "git log -p", &[]), "", "log -p");
    // The configuration refuses every git line, not only the one that fetched.
    fx.assert_key_refused(
        &fx.classify(&clone, "git status", &[]),
        "remote.origin.",
        "partial clone",
    );
    // The partial clone alone is refused, without the upload-pack override. Depending on the git
    // version, the clone records `remote.origin.promisor` or `extensions.partialclone` first.
    fx.git(&clone, &["config", "--unset", "remote.origin.uploadpack"]);
    let out = fx.classify(&clone, "git status", &[]);
    assert_refused(&out, "git config key not on the allowlist: ", "promisor");
    let err = out.stderr_text();
    if fx.platform_keys().is_empty() {
        assert!(
            err.contains("promisor") || err.contains("partialclone"),
            "{err}"
        );
    }
}

#[test]
fn trap_an_include_brings_in_a_program_key() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    let inc = fx.base.join("inc.cfg");
    std::fs::write(
        &inc,
        format!("[core]\n\tfsmonitor = {}\n", fx.marker("incl")),
    )
    .unwrap();
    fx.git(&repo, &["config", "include.path", inc.to_str().unwrap()]);
    fx.git_try(&repo, &["status"], &[]);
    assert!(fx.ran("incl"));
    fx.assert_key_refused(
        &fx.classify(&repo, "git status", &[]),
        "core.fsmonitor",
        "include",
    );
}

#[test]
fn trap_an_include_if_brings_in_a_program_key() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    let inc = fx.base.join("incif.cfg");
    std::fs::write(
        &inc,
        format!("[core]\n\tfsmonitor = {}\n", fx.marker("inclif")),
    )
    .unwrap();
    let key = format!("includeIf.gitdir:{}/.path", repo.display());
    // In the global config, so the condition is what includes it.
    fx.git(&repo, &["config", "--global", &key, inc.to_str().unwrap()]);
    fx.git_try(&repo, &["status"], &[]);
    assert!(fx.ran("inclif"));
    fx.assert_key_refused(
        &fx.classify(&repo, "git status", &[]),
        "core.fsmonitor",
        "includeIf",
    );
}

#[test]
fn trap_a_program_key_in_the_global_config() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    fx.git(
        &repo,
        &["config", "--global", "core.fsmonitor", &fx.marker("global")],
    );
    fx.git_try(&repo, &["status"], &[]);
    assert!(fx.ran("global"));
    fx.assert_key_refused(
        &fx.classify(&repo, "git status", &[]),
        "core.fsmonitor",
        "global",
    );
}

#[test]
fn trap_the_worktree_config() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    fx.git(&repo, &["config", "extensions.worktreeConfig", "true"]);
    fx.git(
        &repo,
        &["config", "--worktree", "core.fsmonitor", &fx.marker("wt")],
    );
    fx.git_try(&repo, &["status"], &[]);
    assert!(fx.ran("wt"));
    fx.assert_key_refused(
        &fx.classify(&repo, "git status", &[]),
        "extensions.worktreeconfig",
        "worktree config",
    );
}

#[test]
fn trap_a_signed_commit_in_a_bare_repository_reached_by_cd() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    let tree = fx.git(&repo, &["write-tree"]);
    let commit = format!(
        "tree {}\nauthor t <t@e> 0 +0000\ncommitter t <t@e> 0 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n x\n -----END PGP SIGNATURE-----\n\nsigned\n",
        tree.trim()
    );
    let body = fx.base.join("commit.txt");
    std::fs::write(&body, commit).unwrap();
    let id = fx.git(
        &repo,
        &["hash-object", "-t", "commit", "-w", body.to_str().unwrap()],
    );
    fx.git(&repo, &["update-ref", "refs/heads/main", id.trim()]);
    fx.git(&fx.root, &["clone", "-q", "--bare", "./r", "sb.git"]);
    let bare = fx.root.join("sb.git");
    fx.git(&bare, &["config", "log.showSignature", "true"]);
    fx.git(&bare, &["config", "gpg.program", &fx.marker("gpg")]);
    fx.git_try(&bare, &["log"], &[]);
    assert!(fx.ran("gpg"));
    fx.assert_key_refused(
        &fx.classify(&fx.root, "cd ./sb.git && git log", &[]),
        "log.showsignature",
        "bare repository",
    );
}

#[test]
fn trap_core_pager_runs_under_a_terminal() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    fx.git(&repo, &["config", "core.pager", &fx.marker("pager")]);
    // The pager only runs when stdout is a terminal: `script` gives git one (util-linux on Linux,
    // BSD `script` on macOS).
    let args: Vec<&str> = if cfg!(target_os = "macos") {
        vec!["-q", "/dev/null", "git", "log"]
    } else {
        vec!["-qec", "git log", "/dev/null"]
    };
    let _ = fx.command("script", &repo, &args, &[]).output().unwrap();
    assert!(fx.ran("pager"));
    fx.assert_key_refused(&fx.classify(&repo, "git log", &[]), "core.pager", "pager");
}

#[test]
fn trap_a_populated_submodule_without_gitmodules_runs_its_own_config() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    let sub = fx.repo("r/sm");
    // `git add` of a nested repository records a gitlink, with no .gitmodules and no .git/modules.
    fx.git_try(&repo, &["add", "sm"], &[]);
    fx.git(&repo, &["commit", "-qm", "gitlink"]);
    assert!(!repo.join(".gitmodules").exists() && !repo.join(".git/modules").exists());
    fx.git(&sub, &["config", "core.fsmonitor", &fx.marker("subfs")]);
    touch_later(&sub.join("a.txt"));
    fx.git_try(&repo, &["status"], &[]);
    assert!(fx.ran("subfs"));
    if fx.platform_keys().is_empty() {
        assert_refused(
            &fx.classify(&repo, "git status", &[]),
            "submodule",
            "gitlink",
        );
    }
}

#[test]
fn a_gitmodules_file_or_a_modules_folder_gives_no_decision() {
    let fx = Fx::new();
    if !fx.platform_keys().is_empty() {
        return;
    }
    let repo = fx.repo("r");
    std::fs::write(repo.join(".gitmodules"), "").unwrap();
    assert_refused(
        &fx.classify(&repo, "git status", &[]),
        "submodule",
        ".gitmodules",
    );
    std::fs::remove_file(repo.join(".gitmodules")).unwrap();
    std::fs::create_dir_all(repo.join(".git/modules")).unwrap();
    assert_refused(
        &fx.classify(&repo, "git status", &[]),
        "submodule",
        ".git/modules",
    );
}

#[test]
fn trap_a_git_file_pointing_out_of_the_root() {
    let fx = Fx::new();
    let work = fx.root.join("g");
    std::fs::create_dir_all(&work).unwrap();
    let elsewhere = fx.base.join("elsewhere.git");
    fx.git(
        &work,
        &[
            "init",
            "-q",
            "--separate-git-dir",
            elsewhere.to_str().unwrap(),
        ],
    );
    std::fs::write(work.join("a"), "a\n").unwrap();
    fx.git(&work, &["add", "a"]);
    fx.git(&work, &["commit", "-qm", "a"]);
    if fx.platform_keys().is_empty() {
        assert_refused(
            &fx.classify(&work, "git status", &[]),
            "outside",
            "git dir outside",
        );
    }
    fx.git(&work, &["config", "core.fsmonitor", &fx.marker("gitfile")]);
    fx.git_try(&work, &["status"], &[]);
    assert!(
        fx.ran("gitfile"),
        "the config of the git dir it points to applies"
    );
    let line = format!("cd {} && git status", work.display());
    let input = request(&line, &work, &[&fx.root, &fx.base], &[]);
    fx.assert_key_refused(
        &run(&input, &fx.env()),
        "core.fsmonitor",
        "git dir inside a second root",
    );
}

#[test]
fn trap_git_environment_variables() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    std::fs::write(repo.join("a.txt"), "a\nb\n").unwrap();
    let ext = fx.marker("envext");
    fx.git_try(&repo, &["diff"], &[("GIT_EXTERNAL_DIFF", &ext)]);
    assert!(fx.ran("envext"));
    let fsm = fx.marker("envcfg");
    let cfg = [
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "core.fsmonitor"),
        ("GIT_CONFIG_VALUE_0", fsm.as_str()),
    ];
    fx.git_try(&repo, &["status"], &cfg);
    assert!(fx.ran("envcfg"));
    for (k, v) in [
        ("GIT_EXTERNAL_DIFF", ext.as_str()),
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_PARAMETERS", "'core.fsmonitor'='x'"),
        ("GIT_DIR", "/tmp"),
        ("GIT_WORK_TREE", "/tmp"),
        ("GIT_CONFIG_GLOBAL", "/tmp/x"),
        ("GIT_PAGER", "x"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_TERMINAL_PROMPT", "0"),
    ] {
        assert_refused(
            &fx.classify_env(&repo, "git diff", &[], &[(k, v)]),
            &format!("git environment variable {k}"),
            k,
        );
    }
    // A GIT_ variable says nothing about a line without git.
    fx.write("x.txt", "x\n");
    assert_allow(
        &fx.classify_env(&fx.root, "cat ./x.txt", &[], &[("GIT_DIR", "/tmp")]),
        "no git",
    );
}

// --- git: the rest of the repository ------------------------------------------------------------------------

#[test]
fn git_outside_a_repository_or_without_git_gives_no_decision() {
    let fx = Fx::new();
    std::fs::create_dir_all(fx.root.join("plain")).unwrap();
    assert_refused(
        &fx.classify(&fx.root.join("plain"), "git status", &[]),
        "git rev-parse",
        "no repository",
    );
    let repo = fx.repo("r");
    assert_refused(
        &fx.classify_env(&repo, "git status", &[], &[("PATH", "/nonexistent")]),
        "no git on PATH",
        "no git",
    );
}

#[test]
fn a_tracked_secret_or_a_rule_below_the_repository_gives_no_decision_for_git() {
    let fx = Fx::new();
    if !fx.platform_keys().is_empty() {
        return;
    }
    let repo = fx.repo("r");
    std::fs::write(repo.join("notes.txt"), "n\n").unwrap();
    fx.git(&repo, &["add", "notes.txt"]);
    fx.git(&repo, &["commit", "-qm", "notes"]);
    for rule in [
        "Read(./notes.txt)",
        "Read(./later/**)",
        "Read(./.git/**)",
        "Read(./.git/config)",
    ] {
        assert_refused(&fx.classify(&repo, "git diff", &[rule]), "Read rule", rule);
    }
    // A name rule is held against the tracked paths, not git's own files.
    assert_refused(
        &fx.classify(&repo, "git diff", &["Read(notes.txt)"]),
        "Read rule matches: tracked",
        "name rule on a tracked file",
    );
    fx.assert_git_allow(
        &fx.classify(&repo, "git status", &["Read(config)"]),
        "name rule on git's files",
    );
    std::fs::write(repo.join(".env.example"), "K=\n").unwrap();
    fx.git(&repo, &["add", ".env.example"]);
    assert_refused(
        &fx.classify(&repo, "git status", &[]),
        "secret",
        "tracked secret",
    );
}

#[test]
fn alternates_or_an_unreadable_index_give_no_decision() {
    let fx = Fx::new();
    if !fx.platform_keys().is_empty() {
        return;
    }
    let repo = fx.repo("r");
    let other = fx.repo("o");
    let alternates = repo.join(".git/objects/info/alternates");
    std::fs::create_dir_all(alternates.parent().unwrap()).unwrap();
    std::fs::write(
        &alternates,
        format!("{}\n", other.join(".git/objects").display()),
    )
    .unwrap();
    assert_refused(
        &fx.classify(&repo, "git log", &[]),
        "alternates",
        "alternates",
    );
    std::fs::remove_file(&alternates).unwrap();
    assert_allow(&fx.classify(&repo, "git log", &[]), "back to normal");
    std::fs::write(repo.join(".git/index"), b"DIRC\0\0\0\x02\0\0\0\x05short").unwrap();
    assert_refused(
        &fx.classify(&repo, "git log", &[]),
        "index",
        "corrupt index",
    );
    std::fs::write(repo.join(".git/index"), b"nope").unwrap();
    assert_refused(&fx.classify(&repo, "git log", &[]), "index", "not an index");
}

#[test]
fn every_index_version_is_read() {
    let fx = Fx::new();
    if !fx.platform_keys().is_empty() {
        return;
    }
    let repo = fx.repo("r");
    std::fs::write(repo.join(".env"), "K=v\n").unwrap();
    fx.git(&repo, &["add", "-f", ".env"]);
    for version in ["2", "3", "4"] {
        fx.git(&repo, &["update-index", "--index-version", version]);
        assert_refused(
            &fx.classify(&repo, "git status", &[]),
            "secret",
            &format!("index v{version}"),
        );
    }
    fx.git(&repo, &["rm", "-q", "--cached", ".env"]);
    for version in ["2", "3", "4"] {
        fx.git(&repo, &["update-index", "--index-version", version]);
        assert_allow(
            &fx.classify(&repo, "git status", &[]),
            &format!("index v{version}"),
        );
    }
}

// --- latency ------------------------------------------------------------------------------------------------

/// Design §2 targets p95 < 50 ms on macOS for the spawn; this bound only catches a regression of
/// an order of magnitude on a loaded CI machine. The numbers are printed for the report.
#[test]
fn classify_latency() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    let repo = fx.repo("r");
    let git_allowed = fx.platform_keys().is_empty();
    for (cwd, command) in [(&fx.root, "cat ./a.txt"), (&repo, "git status")] {
        let line = format!("cd {} && {command}", cwd.display());
        let input = request(&line, cwd, &[&fx.root], &[]);
        let mut times: Vec<Duration> = (0..60)
            .map(|_| {
                let start = Instant::now();
                let out = run(&input, &fx.env());
                let elapsed = start.elapsed();
                if git_allowed || !command.starts_with("git") {
                    assert_allow(&out, command);
                }
                elapsed
            })
            .collect();
        times.sort();
        let (p50, p95) = (times[times.len() / 2], times[times.len() * 95 / 100]);
        eprintln!("classify latency {command:?}: p50 {p50:?} p95 {p95:?}");
        assert!(p95 < Duration::from_millis(500), "{command}: p95 {p95:?}");
    }
}

// --- review round 2 (#172) ----------------------------------------------------------------------------------

/// A program called `name` in `dir` that records `mark` and then runs the real one.
fn fake_program(fx: &Fx, dir: &Path, name: &str, mark: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n: > '{}'\nexit 1\n",
            fx.marks.join(mark).display()
        ),
    )
    .unwrap();
    chmod_x(&path);
    path
}

/// Points `main` of `repo` at a commit that carries a (fake) signature.
fn sign_head(fx: &Fx, repo: &Path) {
    let tree = fx.git(repo, &["write-tree"]);
    let commit = format!(
        "tree {}\nauthor t <t@e> 0 +0000\ncommitter t <t@e> 0 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n x\n -----END PGP SIGNATURE-----\n\nsigned\n",
        tree.trim()
    );
    let body = fx.base.join("signed-commit.txt");
    std::fs::write(&body, commit).unwrap();
    let id = fx.git(
        repo,
        &["hash-object", "-t", "commit", "-w", body.to_str().unwrap()],
    );
    fx.git(repo, &["update-ref", "refs/heads/main", id.trim()]);
}

fn path_with(dir: &Path) -> String {
    format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

#[test]
fn c1_a_symlink_below_a_recursive_start_gives_no_decision() {
    // GNU and Apple diff follow a symlink they find inside the two directories they compare.
    let fx = Fx::new();
    std::fs::write(fx.base.join("outside.txt"), "OUTSIDE-SECRET\n").unwrap();
    fx.write("a/x", "a\n");
    std::fs::create_dir_all(fx.root.join("b")).unwrap();
    symlink(fx.base.join("outside.txt"), fx.root.join("b/x")).unwrap();
    let shown = Command::new("diff")
        .args(["./a", "./b"])
        .current_dir(&fx.root)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("OUTSIDE-SECRET"),
        "diff followed the link"
    );
    assert_refused(
        &fx.classify(&fx.root, "diff ./a ./b", &[]),
        "symlink below",
        "diff",
    );
    for command in [
        "rg x ./b",
        "grep -r x ./b",
        "find ./b",
        "du ./b",
        "ls -R ./b",
    ] {
        assert_refused(
            &fx.classify(&fx.root, command, &[]),
            "symlink below",
            command,
        );
    }
}

#[test]
fn c2_a_path_entry_inside_a_root_gives_no_decision_and_nothing_there_runs() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    let bin = fx.root.join("bin");
    fake_program(&fx, &bin, "cat", "fake-cat");
    let _ = Command::new("sh")
        .args(["-c", "cat ./a.txt"])
        .current_dir(&fx.root)
        .env("PATH", path_with(&bin))
        .output()
        .unwrap();
    assert!(fx.ran("fake-cat"), "the repo's cat ran in place of cat");
    assert_refused(
        &fx.classify_env(&fx.root, "cat ./a.txt", &[], &[("PATH", &path_with(&bin))]),
        "PATH entry inside",
        "cat",
    );
    // A git in a root is never run by the check itself.
    let repo = fx.repo("r");
    fake_program(&fx, &bin, "git", "fake-git");
    assert_refused(
        &fx.classify_env(&repo, "git status", &[], &[("PATH", &path_with(&bin))]),
        "PATH entry inside",
        "git",
    );
    assert!(!fx.ran("fake-git"), "the check ran the repo's git");
    // A PATH entry outside the roots whose git is a link into a root.
    let tools = fx.base.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    symlink(bin.join("git"), tools.join("git")).unwrap();
    assert_refused(
        &fx.classify_env(&repo, "git status", &[], &[("PATH", &path_with(&tools))]),
        "git",
        "git linked into a root",
    );
    assert!(!fx.ran("fake-git"), "the check ran the linked git");
    // A PATH entry that is the cwd of the request, outside the roots.
    let elsewhere = fx.base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let line = format!("cat {}", fx.path("a.txt"));
    let input = request(&line, &elsewhere, &[&fx.root], &[]);
    let env = fx.env().with("PATH", &path_with(&elsewhere));
    assert_refused(&run(&input, &env), "PATH entry inside", "the cwd");
}

#[test]
fn i1_a_signature_atom_in_a_git_format_or_sort_gives_no_decision() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    sign_head(&fx, &repo);
    let tools = fx.base.join("tools");
    fake_program(&fx, &tools, "gpg", "gpg-ran");
    let path = path_with(&tools);
    fx.git_try(
        &repo,
        &["branch", "--format=%(signature)"],
        &[("PATH", &path)],
    );
    assert!(
        fx.ran("gpg-ran"),
        "git branch --format=%(signature) ran gpg"
    );
    for command in [
        "git branch '--format=%(signature)'",
        "git branch '--format=%(signature:grade)'",
        "git branch --sort=signature",
        "git log --format=%GS",
        "git log '--pretty=format:%G?'",
    ] {
        assert_refused(
            &fx.classify_env(&repo, command, &[], &[("PATH", &path)]),
            "",
            command,
        );
        let err = fx
            .classify_env(&repo, command, &[], &[("PATH", &path)])
            .stderr_text();
        assert!(
            err.contains("signature") || err.contains("not proven"),
            "{command}: {err}"
        );
    }
    assert_refused(
        &fx.classify_env(
            &repo,
            "git branch --sort=signature",
            &[],
            &[("PATH", &path)],
        ),
        "signature",
        "sort",
    );
}

/// A repository whose `secret.txt` was committed and then removed from the index and the work
/// tree: the index scan cannot see it, git can still print it.
fn removed_secret(fx: &Fx) -> PathBuf {
    let repo = fx.repo("r");
    std::fs::write(repo.join("secret.txt"), "TOPSECRET\n").unwrap();
    fx.git(&repo, &["add", "secret.txt"]);
    fx.git(&repo, &["commit", "-qm", "secret"]);
    fx.git(&repo, &["rm", "-q", "--cached", "secret.txt"]);
    std::fs::remove_file(repo.join("secret.txt")).unwrap();
    repo
}

#[test]
fn i2_git_forms_that_print_history_give_no_decision() {
    let fx = Fx::new();
    let repo = removed_secret(&fx);
    let rule = &["Read(secret.txt)"];
    for (demo, command) in [
        (&["status", "-v"][..], "git status -v"),
        (&["status", "--verbose"], "git status --verbose"),
        (&["diff", "--cached"], "git diff --cached"),
        (&["diff", "--staged"], "git diff --staged"),
        (&["log", "-p"], "git log -p"),
        (&["log", "-u"], "git log -u"),
        (&["log", "--patch"], "git log --patch"),
        (&["log", "-W", "-p"], "git log -W"),
        (&["log", "--word-diff", "-p"], "git log --word-diff"),
        (&["log", "--color-words", "-p"], "git log --color-words"),
        (&["log", "--binary", "-p"], "git log --binary"),
        (&["show"], "git show"),
    ] {
        // The demonstration of `-W`, `--word-diff`, `--color-words` and `--binary` adds `-p`, which
        // they shape (newer git implies it for some); the line checked is the one without it.
        let out = fx.command("git", &repo, demo, &[]).output().unwrap();
        let shown = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            shown.contains("TOPSECRET"),
            "git {demo:?} printed it: {shown}"
        );
        assert_refused(&fx.classify(&repo, command, rule), "", command);
    }
    // `--stat` prints the names of history, and `diff` with a revision prints it too.
    let stat = fx
        .command("git", &repo, &["log", "--stat"], &[])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&stat.stdout).contains("secret.txt"));
    assert_refused(&fx.classify(&repo, "git log --stat", rule), "", "--stat");
    std::fs::write(repo.join("main"), "m\n").unwrap();
    assert_refused(&fx.classify(&repo, "git diff main", rule), "", "a revision");
    // What stays: plain status, log and diff, and blame.
    for command in [
        "git status",
        "git log --oneline",
        "git diff",
        "git blame ./a.txt",
    ] {
        fx.assert_git_allow(&fx.classify(&repo, command, rule), command);
    }
}

#[test]
fn i2_the_history_refusals_name_their_reason() {
    let fx = Fx::new();
    if !fx.platform_keys().is_empty() {
        return;
    }
    let repo = removed_secret(&fx);
    for command in [
        "git status -v",
        "git status -sv",
        "git diff --staged",
        "git log -p",
        "git log -pu",
        "git log --stat=80",
        "git show",
        "git diff main",
    ] {
        assert_refused(&fx.classify(&repo, command, &[]), "git history", command);
    }
}

#[test]
fn r3_git_log_reads_only_its_allowlist() {
    let fx = Fx::new();
    let repo = removed_secret(&fx);
    for (demo, command) in [
        (&["log", "-U3"][..], "git log -U3"),
        (&["log", "-U0"], "git log -U0"),
        (&["log", "-U1"], "git log -U1"),
    ] {
        let out = fx.command("git", &repo, demo, &[]).output().unwrap();
        let shown = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            shown.contains("TOPSECRET"),
            "git {demo:?} printed it: {shown}"
        );
        assert_refused(&fx.classify(&repo, command, &[]), "", command);
    }
    // `--check` prints every added line that has a whitespace error.
    std::fs::write(repo.join("ws.txt"), "WS-SECRET \n").unwrap();
    fx.git(&repo, &["add", "ws.txt"]);
    fx.git(&repo, &["commit", "-qm", "ws"]);
    fx.git(&repo, &["rm", "-q", "--cached", "ws.txt"]);
    std::fs::remove_file(repo.join("ws.txt")).unwrap();
    let out = fx
        .command("git", &repo, &["log", "--check"], &[])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("WS-SECRET"));
    assert_refused(&fx.classify(&repo, "git log --check", &[]), "", "--check");
    assert_refused(
        &fx.classify(&repo, "git log --no-merges", &[]),
        "",
        "an option off the list",
    );
    fx.assert_git_allow(
        &fx.classify(&repo, "git log --oneline -n 5", &[]),
        "the allowlist",
    );
}

#[test]
fn r3_every_program_is_resolved_and_one_linked_into_a_root_gives_no_decision() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    let evil = fake_program(&fx, &fx.root, "evil", "evil-cat");
    let tools = fx.base.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    symlink(&evil, tools.join("cat")).unwrap();
    let path = path_with(&tools);
    let _ = Command::new("sh")
        .args(["-c", "cat ./a.txt"])
        .current_dir(&fx.root)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        fx.ran("evil-cat"),
        "the linked cat ran the file in the root"
    );
    std::fs::remove_file(fx.marks.join("evil-cat")).unwrap();
    assert_refused(
        &fx.classify_env(&fx.root, "cat ./a.txt", &[], &[("PATH", &path)]),
        "resolves inside",
        "cat",
    );
    assert_refused(
        &fx.classify_env(&fx.root, "ls ./a.txt | cat", &[], &[("PATH", &path)]),
        "resolves inside",
        "cat in a pipe",
    );
    assert!(!fx.ran("evil-cat"), "the check ran it");
    // The same link to a file outside every root is fine.
    let tool = fake_program(&fx, &fx.base.join("real"), "cat", "outside-cat");
    let fine = fx.base.join("fine");
    std::fs::create_dir_all(&fine).unwrap();
    symlink(&tool, fine.join("cat")).unwrap();
    assert_allow(
        &fx.classify_env(&fx.root, "cat ./a.txt", &[], &[("PATH", &path_with(&fine))]),
        "a cat outside the roots",
    );
    assert!(!fx.ran("outside-cat"), "the check ran it");
}

#[test]
fn r3_an_assignment_or_a_quoted_program_gives_no_decision() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    let bin = fx.root.join("bin");
    fake_program(&fx, &bin, "cat", "assigned-cat");
    let line = format!("PATH={} cat ./a.txt", bin.display());
    let _ = Command::new("sh")
        .args(["-c", &line])
        .current_dir(&fx.root)
        .output()
        .unwrap();
    assert!(fx.ran("assigned-cat"), "the PATH in the line chose the cat");
    std::fs::remove_file(fx.marks.join("assigned-cat")).unwrap();
    for command in [line.as_str(), "FOO=1 cat ./a.txt"] {
        assert_refused(&fx.classify(&fx.root, command, &[]), "", command);
    }
    // `\cat` finds cat on PATH like `cat`: a cat linked into the root refuses either way.
    let evil = fake_program(&fx, &fx.root, "evil", "quoted-cat");
    let tools = fx.base.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    symlink(&evil, tools.join("cat")).unwrap();
    let path = path_with(&tools);
    for command in ["\\cat ./a.txt", "'cat' ./a.txt"] {
        let _ = Command::new("sh")
            .args(["-c", command])
            .current_dir(&fx.root)
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(fx.ran("quoted-cat"), "{command} ran the linked cat");
        std::fs::remove_file(fx.marks.join("quoted-cat")).unwrap();
        assert_refused(
            &fx.classify_env(&fx.root, command, &[], &[("PATH", &path)]),
            "",
            command,
        );
    }
    assert!(!fx.ran("assigned-cat") && !fx.ran("quoted-cat"));
}

#[test]
fn minor_a_hard_link_gives_no_decision() {
    let fx = Fx::new();
    std::fs::write(fx.base.join("outside.txt"), "OUTSIDE\n").unwrap();
    std::fs::hard_link(fx.base.join("outside.txt"), fx.root.join("hl")).unwrap();
    let shown = Command::new("cat")
        .arg("./hl")
        .current_dir(&fx.root)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&shown.stdout), "OUTSIDE\n");
    assert_refused(&fx.classify(&fx.root, "cat ./hl", &[]), "hard link", "cat");
    std::fs::create_dir_all(fx.root.join("d")).unwrap();
    std::fs::rename(fx.root.join("hl"), fx.root.join("d/hl")).unwrap();
    assert_refused(&fx.classify(&fx.root, "rg x ./d", &[]), "hard link", "walk");
}

#[test]
fn minor_git_objects_outside_the_roots_give_no_decision() {
    let fx = Fx::new();
    if !fx.platform_keys().is_empty() {
        return;
    }
    let repo = fx.repo("r");
    std::fs::write(repo.join("a.txt"), "OBJECT-SECRET\n").unwrap();
    fx.git(&repo, &["commit", "-qam", "two"]);
    std::fs::rename(repo.join(".git/objects"), fx.base.join("objects")).unwrap();
    symlink(fx.base.join("objects"), repo.join(".git/objects")).unwrap();
    let out = fx
        .command("git", &repo, &["log", "-1", "--format=%H"], &[])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git reads its objects through the link"
    );
    assert_refused(
        &fx.classify(&repo, "git log --oneline", &[]),
        "objects",
        "objects",
    );
    // The store in place, one loose-object folder of it linked out of the root.
    std::fs::remove_file(repo.join(".git/objects")).unwrap();
    std::fs::rename(fx.base.join("objects"), repo.join(".git/objects")).unwrap();
    assert_allow(&fx.classify(&repo, "git log --oneline", &[]), "store back");
    let head = fx.git(&repo, &["rev-parse", "HEAD"]);
    let fan = &head.trim()[..2];
    let moved = fx.base.join("fan");
    std::fs::rename(repo.join(".git/objects").join(fan), &moved).unwrap();
    symlink(&moved, repo.join(".git/objects").join(fan)).unwrap();
    let out = fx
        .command("git", &repo, &["log", "-1", "--format=%H"], &[])
        .output()
        .unwrap();
    assert!(out.status.success(), "git reads a loose object through it");
    assert_refused(
        &fx.classify(&repo, "git log --oneline", &[]),
        "symlink in the object store",
        "a linked fan-out folder",
    );
}

#[test]
fn minor_grep_options_and_ripgrep_config_give_no_decision() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    if cfg!(target_os = "macos") {
        // BSD grep (egrep and fgrep too) honours GREP_OPTIONS.
        let out = Command::new("grep")
            .args(["a", "./a.txt"])
            .current_dir(&fx.root)
            .env("GREP_OPTIONS", "-c")
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "1\n");
    }
    for command in ["grep a ./a.txt", "egrep a ./a.txt", "fgrep a ./a.txt"] {
        assert_refused(
            &fx.classify_env(&fx.root, command, &[], &[("GREP_OPTIONS", "-c")]),
            "GREP_OPTIONS",
            command,
        );
    }
    assert_refused(
        &fx.classify_env(
            &fx.root,
            "rg a ./a.txt",
            &[],
            &[("RIPGREP_CONFIG_PATH", "/tmp/rgrc")],
        ),
        "RIPGREP_CONFIG_PATH",
        "rg",
    );
    assert_allow(
        &fx.classify_env(&fx.root, "cat ./a.txt", &[], &[("GREP_OPTIONS", "-c")]),
        "no grep",
    );
}

#[test]
fn minor_a_rule_with_braces_or_a_bang_is_not_read() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    for rule in ["Read(./{a,b}.txt)", "Read(./a.txt})", "Read(!./b.txt)"] {
        assert_refused(
            &fx.classify(&fx.root, "cat ./a.txt", &[rule]),
            "does not read",
            rule,
        );
    }
}

#[test]
fn gap5_duplicate_keys_give_no_decision() {
    let fx = Fx::new();
    fx.write("a.txt", "a\n");
    let root = fx.root.to_str().unwrap();
    let line = format!("cat {root}/a.txt");
    for input in [
        format!(
            r#"{{"command":"rm -rf /","command":"{line}","cwd":"{root}","roots":["{root}"],"read_rules":[]}}"#
        ),
        format!(
            r#"{{"command":"{line}","cwd":"{root}","roots":["{root}"],"read_rules":["Read(./a.txt)"],"read_rules":[]}}"#
        ),
        format!(
            r#"{{"command":"{line}","cwd":"{root}","roots":["{root}"],"read_rules":[],"roots":["/"]}}"#
        ),
    ] {
        assert_refused(&run(input.as_bytes(), &fx.env()), "duplicate", &input);
    }
}

#[test]
fn gap4_osxkeychain_from_anywhere_but_apples_own_file_gives_no_decision() {
    let fx = Fx::new();
    let repo = fx.repo("r");
    fx.git(&repo, &["config", "credential.helper", "osxkeychain"]);
    fx.assert_key_refused(
        &fx.classify(&repo, "git status", &[]),
        "credential.helper",
        "local osxkeychain",
    );
}
