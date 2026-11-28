//! `anthology check-repo` (check-repo Task 8): the command as a user runs it, against fixtures
//! built in temp folders. Nothing of a fixture is ever executed; the only programs that run are
//! `git` and the small stand-in scripts the tests write (an `ssh` that talks to a local repo and
//! the three optional tools).
//!
//! URL mode is tested over `ssh://`: a stand-in `ssh` first on `PATH` runs `git upload-pack`
//! against a local repository, so the whole fetch (init, fetch, checkout, `ls-files` modes)
//! runs without a network. An `https://` server cannot serve a fetch here: the fetch clears the
//! environment, so git has no way to trust the test CA, and the failure that gives is a test of
//! its own.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anthology_pycompat::json::{self, Value};
use anthology_testkit::http::TestServer;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_anthology");

/// A link target no machine has: the scan must report the link and never follow it.
const LINK_TARGET: &str = "/nonexistent-anthology-check-repo-target";

/// The first line of the opened-folder warning (the report tests pin the whole text).
const OPENED_START: &str = "This folder is already open in Claude Code.";

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

/// A home, a config folder and a temp folder for one test; the binary runs with these and
/// nothing else of the environment except what a test adds to `PATH`.
struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        let dir = tempfile::tempdir().expect("temp dir");
        for sub in ["home", "tmp"] {
            fs::create_dir(dir.path().join(sub)).expect("make folder");
        }
        Sandbox { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn home(&self) -> PathBuf {
        self.path().join("home")
    }

    fn config(&self) -> PathBuf {
        self.path().join("config")
    }

    /// `TMPDIR` of the binary: the scratch folder of the tools is made here.
    fn tmp(&self) -> PathBuf {
        self.path().join("tmp")
    }

    /// Runs the binary with `front` before `/usr/bin:/bin` on `PATH`. A token is set in the
    /// environment so a test can see that it never reaches a child.
    fn run(&self, args: &[&str], front: &[&Path]) -> Out {
        self.run_in(args, front, &self.tmp())
    }

    /// The command of `run`: `TMPDIR` is `tmp`, `PATH` is `front` before `/usr/bin:/bin`.
    fn command(&self, args: &[&str], front: &[&Path], tmp: &Path) -> Command {
        let mut path = String::new();
        for p in front {
            path.push_str(&p.to_string_lossy());
            path.push(':');
        }
        path.push_str("/usr/bin:/bin");
        let mut cmd = Command::new(BIN);
        cmd.arg("check-repo")
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("CLAUDE_CONFIG_DIR", self.config())
            .env("TMPDIR", tmp)
            .env("PATH", path)
            .env("GITHUB_TOKEN", "must-not-reach-a-child")
            .env("GH_TOKEN", "must-not-reach-a-child")
            .current_dir(self.path())
            .stdin(Stdio::null());
        cmd
    }

    /// Starts the binary and leaves it running, for a test that signals it.
    fn spawn(&self, args: &[&str], front: &[&Path]) -> std::process::Child {
        self.command(args, front, &self.tmp())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start anthology")
    }

    /// `run` with `TMPDIR` set to `tmp`.
    fn run_in(&self, args: &[&str], front: &[&Path], tmp: &Path) -> Out {
        let out = self
            .command(args, front, tmp)
            .output()
            .expect("run anthology");
        Out {
            code: out.status.code().expect("exit code"),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

fn write(root: &Path, rel: &str, body: &str) {
    let at = root.join(rel);
    fs::create_dir_all(at.parent().expect("parent")).expect("make folder");
    fs::write(at, body).expect("write file");
}

fn script(root: &Path, name: &str, body: &str) -> PathBuf {
    write(root, name, body);
    let at = root.join(name);
    fs::set_permissions(&at, fs::Permissions::from_mode(0o755)).expect("chmod");
    at
}

/// A small repo with one HIGH finding (a link to an absolute path) and nothing else.
fn folder_fixture(root: &Path) {
    write(root, "README.md", "# Example\n\nA small sample project.\n");
    write(
        root,
        "package.json",
        "{\"name\": \"example\", \"version\": \"1.0.0\"}\n",
    );
    write(root, "src/main.js", "console.log('hello');\n");
    script(root, "bin/run.sh", "#!/bin/sh\necho hello\n");
    fs::create_dir_all(root.join("empty")).expect("make folder");
    symlink(LINK_TARGET, root.join("link")).expect("symlink");
}

/// Every name, kind, mode, size, mtime, file content and link target below `root`.
fn tree_state(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).expect("list") {
            let path = entry.expect("entry").path();
            let meta = fs::symlink_metadata(&path).expect("metadata");
            let rel = path
                .strip_prefix(root)
                .expect("below root")
                .display()
                .to_string();
            let kind = if meta.file_type().is_symlink() {
                format!(
                    "link -> {}",
                    fs::read_link(&path).expect("target").display()
                )
            } else if meta.is_dir() {
                "dir".to_string()
            } else {
                format!(
                    "file {}",
                    String::from_utf8_lossy(&fs::read(&path).expect("read"))
                )
            };
            out.insert(
                rel,
                format!(
                    "{kind} mode={:o} size={} mtime={}.{}",
                    meta.mode(),
                    meta.len(),
                    meta.mtime(),
                    meta.mtime_nsec()
                ),
            );
            if meta.is_dir() {
                walk(&path, root, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    // The root's own mtime changes when anything is made or removed in it.
    let meta = fs::symlink_metadata(root).expect("metadata");
    out.insert(
        ".".to_string(),
        format!(
            "mode={:o} mtime={}.{}",
            meta.mode(),
            meta.mtime(),
            meta.mtime_nsec()
        ),
    );
    walk(root, root, &mut out);
    out
}

/// Stand-ins for the three tools: each logs its name, working folder and environment to a file in
/// `HOME` (the one place a cleared environment still points to) and prints an empty result.
fn fake_tools(bin: &Path) {
    for name in ["osv-scanner", "gitleaks", "zizmor"] {
        let result = if name == "osv-scanner" {
            "{\"results\": []}"
        } else {
            "[]"
        };
        script(
            bin,
            name,
            &format!(
                "#!/bin/sh\necho \"{name} cwd=$PWD args=$* env=$(env | sort | tr '\\n' ' ')\" >> \"$HOME/tool.log\"\necho '{result}'\n"
            ),
        );
    }
}

/// A `ssh` that skips its options and the host, then runs the remote command here, with
/// `git-upload-pack` spelled as a git subcommand so it needs no libexec folder on `PATH`.
fn fake_ssh(bin: &Path) {
    script(
        bin,
        "ssh",
        "#!/bin/sh\n\
         while [ $# -gt 0 ]; do\n\
           case \"$1\" in\n\
             -o|-p|-i|-l|-F) shift 2 ;;\n\
             -*) shift ;;\n\
             *) break ;;\n\
           esac\n\
         done\n\
         shift\n\
         case \"$1\" in git-upload-pack*) set -- \"git upload-pack${1#git-upload-pack}\" ;; esac\n\
         exec sh -c \"$1\"\n",
    );
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Tester",
            "-c",
            "user.email=tester@example.invalid",
        ])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git (the tests need git on PATH)");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A committed repo of `folder_fixture`, and its HEAD hash.
fn source_repo(root: &Path) -> String {
    folder_fixture(root);
    git(root, &["init", "-q"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "fixture"]);
    git(root, &["rev-parse", "HEAD"])
}

fn ssh_url(repo: &Path) -> String {
    format!("ssh://example.invalid{}", repo.display())
}

fn one_line_error(out: &Out) {
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert_eq!(out.stdout, "");
    assert!(
        out.stderr.starts_with("anthology check-repo: "),
        "{}",
        out.stderr
    );
    assert!(out.stderr.ends_with('\n'), "{:?}", out.stderr);
    assert_eq!(out.stderr.lines().count(), 1, "{:?}", out.stderr);
}

/// A finished run says nothing on stderr but notes (an old git adds one).
fn only_notes(out: &Out) {
    for line in out.stderr.lines() {
        assert!(
            line.starts_with("anthology check-repo: note: "),
            "stderr of a report run: {:?}",
            out.stderr
        );
    }
}

fn parsed(out: &Out) -> json::Map {
    only_notes(out);
    json::loads(&out.stdout)
        .expect("one JSON object")
        .into_object()
        .expect("an object")
}

fn string<'a>(m: &'a json::Map, key: &str) -> &'a str {
    m.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no string {key}"))
}

#[test]
fn a_folder_scan_writes_nothing_into_it() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    folder_fixture(target.path());
    let tools = sb.path().join("tools");
    fake_tools(&tools);
    let before = tree_state(target.path());
    let out = sb.run(&[target.path().to_str().expect("utf-8")], &[&tools]);
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    assert_eq!(
        tree_state(target.path()),
        before,
        "the scan changed the folder"
    );
    // The tools ran, in a scratch folder outside the target that is gone afterwards.
    let log = fs::read_to_string(sb.home().join("tool.log")).expect("the tools ran");
    assert_eq!(log.lines().count(), 3, "{log}");
    let canonical = fs::canonicalize(target.path()).expect("canonical");
    for line in log.lines() {
        let cwd = line
            .split("cwd=")
            .nth(1)
            .and_then(|r| r.split(' ').next())
            .expect("cwd");
        assert!(
            !Path::new(cwd).starts_with(&canonical),
            "tool ran inside the target: {line}"
        );
        assert!(line.contains("HOME="), "{line}");
        assert!(
            !line.contains("must-not-reach-a-child"),
            "a token reached a tool: {line}"
        );
    }
    let left: Vec<_> = fs::read_dir(sb.tmp()).expect("tmp").collect();
    assert!(left.is_empty(), "scratch left behind: {left:?}");
}

#[test]
fn no_tools_runs_none() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    folder_fixture(target.path());
    let tools = sb.path().join("tools");
    fake_tools(&tools);
    let out = sb.run(
        &["--no-tools", target.path().to_str().expect("utf-8")],
        &[&tools],
    );
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    assert!(
        !sb.home().join("tool.log").exists(),
        "a tool ran with --no-tools"
    );
}

#[test]
fn url_mode_with_a_local_bare_repo_over_file_is_refused() {
    let sb = Sandbox::new();
    let work = sb.path().join("work");
    source_repo(&work);
    let bare = sb.path().join("bare.git");
    git(
        sb.path(),
        &[
            "clone",
            "-q",
            "--bare",
            work.to_str().expect("utf-8"),
            bare.to_str().expect("utf-8"),
        ],
    );
    let into = sb.path().join("quarantine");
    let url = format!("file://{}", bare.display());
    let out = sb.run(&["--into", into.to_str().expect("utf-8"), &url], &[]);
    one_line_error(&out);
    assert!(!into.exists(), "a refused URL made the quarantine folder");
    assert!(out.stderr.contains("not a git URL"), "{}", out.stderr);
}

#[test]
fn into_with_a_parent_step_is_refused_and_touches_nothing() {
    let sb = Sandbox::new();
    let repo = sb.path().join("remote");
    source_repo(&repo);
    let bin = sb.path().join("bin");
    fake_ssh(&bin);
    // `missing` is not there, so `missing/../keep` does not resolve and looks like a new folder;
    // it names `keep`, which is, and an interrupt would have removed it.
    let keep = sb.path().join("keep");
    std::fs::create_dir(&keep).expect("make keep");
    std::fs::write(keep.join("mine.txt"), "mine").expect("write");
    let into = sb.path().join("missing").join("..").join("keep");
    let out = sb.run(
        &["--into", into.to_str().expect("utf-8"), &ssh_url(&repo)],
        &[&bin],
    );
    one_line_error(&out);
    assert!(out.stderr.contains("`..`"), "{}", out.stderr);
    assert!(!sb.path().join("missing").exists());
    assert_eq!(
        std::fs::read_to_string(keep.join("mine.txt")).expect("read"),
        "mine"
    );
}

#[test]
fn url_mode_end_to_end() {
    let sb = Sandbox::new();
    let repo = sb.path().join("remote");
    let head = source_repo(&repo);
    let bin = sb.path().join("bin");
    fake_ssh(&bin);
    let url = ssh_url(&repo);
    let into = sb.path().join("quarantine");
    let into_arg = into.to_str().expect("utf-8");

    let out = sb.run(&["--no-tools", "--into", into_arg, &url], &[&bin]);
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    only_notes(&out);
    let first = out.stdout.lines().next().expect("a verdict line");
    assert_eq!(first, "Review before opening", "{}", out.stdout);
    // The header names the URL as given, the quarantine folder and how to work in it.
    assert!(
        out.stdout.contains(&format!("target: {url}\n")),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("rev: HEAD\n"), "{}", out.stdout);
    assert!(
        out.stdout
            .contains(&format!("quarantine: {}\n", into.display())),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains(&format!(
            "To work in it: git -C '{}' status",
            into.display()
        )),
        "{}",
        out.stdout
    );
    // The link came from git's mode 120000, not from the disk: it is a plain file there.
    assert!(
        out.stdout.contains("autorun.symlink-escape"),
        "{}",
        out.stdout
    );
    let on_disk = fs::symlink_metadata(into.join("link")).expect("link file");
    assert!(
        on_disk.file_type().is_file(),
        "core.symlinks=false should leave a file"
    );
    assert_eq!(
        fs::metadata(&into).expect("quarantine").mode() & 0o777,
        0o700
    );
    assert!(into.join("README.md").exists());

    // JSON of a second fetch: the commit as the revision, excerpts from the quarantine folder.
    let again = sb.path().join("quarantine2");
    let out = sb.run(
        &[
            "--json",
            "--no-tools",
            "--rev",
            &head,
            "--into",
            again.to_str().expect("utf-8"),
            &url,
        ],
        &[&bin],
    );
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    let m = parsed(&out);
    assert_eq!(string(&m, "source"), "url");
    assert_eq!(string(&m, "url"), url);
    assert_eq!(string(&m, "target"), url);
    assert_eq!(string(&m, "rev"), head);
    assert_eq!(string(&m, "quarantine"), again.to_str().expect("utf-8"));
    let triage = m.get("triage").and_then(Value::as_object).expect("triage");
    let nonce = triage.get("nonce").and_then(Value::as_str).expect("nonce");
    let items = triage
        .get("items")
        .and_then(Value::as_array)
        .expect("items");
    assert_eq!(items.len(), 1, "one HIGH finding, one excerpt");
    let excerpt = items[0]
        .as_object()
        .and_then(|i| i.get("excerpt"))
        .and_then(Value::as_str)
        .expect("excerpt");
    assert!(
        excerpt.starts_with(&format!("<<untrusted-{nonce}>>")),
        "{excerpt}"
    );
    assert!(
        excerpt.contains(LINK_TARGET),
        "the excerpt is the quarantine file: {excerpt}"
    );
}

#[test]
fn the_default_quarantine_is_under_the_anthology_home() {
    let sb = Sandbox::new();
    let repo = sb.path().join("remote");
    source_repo(&repo);
    let bin = sb.path().join("bin");
    fake_ssh(&bin);
    let out = sb.run(&["--no-tools", &ssh_url(&repo)], &[&bin]);
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    let parent = sb.config().join("anthology").join("check-repo");
    assert_eq!(fs::metadata(&parent).expect("parent").mode() & 0o777, 0o700);
    let made: Vec<_> = fs::read_dir(&parent)
        .expect("list")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert_eq!(made.len(), 1, "{made:?}");
    let name = made[0].to_string_lossy().into_owned();
    assert!(name.starts_with("example.invalid-"), "{name}");
    let epoch = name.rsplit('-').next().expect("epoch");
    assert!(
        epoch.parse::<u64>().is_ok_and(|e| e > 1_700_000_000),
        "{name}"
    );
    assert!(
        out.stdout
            .contains(&format!("quarantine: {}\n", parent.join(&name).display())),
        "{}",
        out.stdout
    );
}

#[test]
fn a_failed_fetch_exits_3_and_leaves_no_quarantine() {
    let sb = Sandbox::new();
    // The server speaks TLS with a CA git does not trust; the fetch must fail, not prompt or hang.
    let server = TestServer::https();
    let into = sb.path().join("quarantine");
    let out = sb.run(
        &[
            "--into",
            into.to_str().expect("utf-8"),
            &server.url("/owner/repo.git"),
        ],
        &[],
    );
    one_line_error(&out);
    assert!(!into.exists(), "the half-made quarantine was not removed");
    let left: Vec<_> = fs::read_dir(sb.path())
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("template") || n.contains("quarantine"))
        .collect();
    assert!(left.is_empty(), "left behind: {left:?}");
}

#[test]
fn a_failure_after_the_fetch_removes_the_quarantine() {
    let sb = Sandbox::new();
    let repo = sb.path().join("remote");
    source_repo(&repo);
    let bin = sb.path().join("bin");
    fake_ssh(&bin);
    let into = sb.path().join("quarantine");
    // The scratch folder of the tools would be made inside the quarantine: the run refuses, and
    // the folder it fetched must not be left behind.
    let out = sb.run_in(
        &["--into", into.to_str().expect("utf-8"), &ssh_url(&repo)],
        &[&bin],
        &into,
    );
    one_line_error(&out);
    assert!(out.stderr.contains("inside the target"), "{}", out.stderr);
    assert!(!into.exists(), "the quarantine of a failed run was kept");
}

#[test]
fn an_existing_into_folder_is_refused_and_kept() {
    let sb = Sandbox::new();
    let into = sb.path().join("taken");
    write(&into, "keep.txt", "mine\n");
    let out = sb.run(
        &[
            "--into",
            into.to_str().expect("utf-8"),
            "https://example.invalid/owner/repo.git",
        ],
        &[],
    );
    one_line_error(&out);
    assert_eq!(
        fs::read_to_string(into.join("keep.txt")).expect("kept"),
        "mine\n"
    );
}

#[test]
fn json_and_text_agree() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    folder_fixture(target.path());
    let arg = target.path().to_str().expect("utf-8");
    let text = sb.run(&["--no-tools", arg], &[]);
    let json_out = sb.run(&["--json", "--no-tools", arg], &[]);
    assert_eq!(text.code, 1, "{}{}", text.stdout, text.stderr);
    assert_eq!(json_out.code, text.code);
    let m = parsed(&json_out);
    assert_eq!(
        m.get("exit").and_then(Value::as_i64),
        Some(i64::from(text.code))
    );
    assert_eq!(
        string(&m, "verdict"),
        text.stdout.lines().next().expect("verdict")
    );
    let counts = m.get("counts").and_then(Value::as_object).expect("counts");
    let n = |k: &str| counts.get(k).and_then(Value::as_i64).expect("count");
    let line = format!(
        "CRITICAL {}  HIGH {}  MEDIUM {}  LOW {}  INFO {}",
        n("CRITICAL"),
        n("HIGH"),
        n("MEDIUM"),
        n("LOW"),
        n("INFO")
    );
    assert_eq!(text.stdout.lines().nth(1), Some(line.as_str()));
    // A folder scan names the canonical folder, in the text and in the JSON.
    let canonical = fs::canonicalize(target.path()).expect("canonical");
    let canonical = canonical.to_str().expect("utf-8");
    assert_eq!(string(&m, "target"), canonical);
    assert!(
        text.stdout.contains(&format!("target: {canonical}\n")),
        "{}",
        text.stdout
    );
    assert_eq!(string(&m, "source"), "path");
    assert!(m.get("url").is_some_and(Value::is_null));
    // Both come from the same report: the JSON `report` is the text, but for the timing line.
    let strip = |s: &str| {
        s.lines()
            .filter(|l| !l.starts_with("scanned:"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(strip(string(&m, "report")), strip(&text.stdout));
    let blocks = text
        .stdout
        .lines()
        .filter(|l| l.starts_with('[') && l.contains("] F"))
        .count();
    assert_eq!(
        m.get("findings").and_then(Value::as_array).map(Vec::len),
        Some(blocks)
    );
    // A new nonce for every run.
    let again = parsed(&sb.run(&["--json", "--no-tools", arg], &[]));
    let nonce = |m: &json::Map| {
        m.get("triage")
            .and_then(Value::as_object)
            .and_then(|t| t.get("nonce"))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let (a, b) = (nonce(&m).expect("nonce"), nonce(&again).expect("nonce"));
    assert_eq!(a.len(), 16);
    assert!(a.bytes().all(|c| c.is_ascii_hexdigit()), "{a}");
    assert_ne!(a, b);
}

#[test]
fn a_clean_folder_exits_0() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    write(target.path(), "README.md", "# Plain\n");
    write(target.path(), "a.txt", "text\n");
    let out = sb.run(&["--no-tools", target.path().to_str().expect("utf-8")], &[]);
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stdout.starts_with("No known-bad signals found\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("No findings is not proof of safety."),
        "{}",
        out.stdout
    );
}

#[test]
fn opened_puts_the_warning_first() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    folder_fixture(target.path());
    let out = sb.run(
        &[
            "--opened",
            "--no-tools",
            target.path().to_str().expect("utf-8"),
        ],
        &[],
    );
    assert!(out.stdout.starts_with(OPENED_START), "{}", out.stdout);
    let json_out = sb.run(
        &[
            "--json",
            "--opened",
            "--no-tools",
            target.path().to_str().expect("utf-8"),
        ],
        &[],
    );
    assert_eq!(
        parsed(&json_out).get("opened").and_then(Value::as_bool),
        Some(true)
    );
}

#[test]
fn all_includes_node_modules() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    write(target.path(), "README.md", "# Plain\n");
    write(
        target.path(),
        "node_modules/dep/index.js",
        "module.exports = 1;\n",
    );
    let arg = target.path().to_str().expect("utf-8");
    let by_default = parsed(&sb.run(&["--json", "--no-tools", arg], &[]));
    let skipped = by_default
        .get("skipped")
        .and_then(Value::as_array)
        .expect("skipped");
    // Each skipped path is repo text, so it sits between the nonce markers.
    let nonce = by_default
        .get("triage")
        .and_then(Value::as_object)
        .and_then(|t| t.get("nonce"))
        .and_then(Value::as_str)
        .expect("nonce");
    let (open, close) = (
        format!("<<untrusted-{nonce}>>"),
        format!("<</untrusted-{nonce}>>"),
    );
    assert_eq!(
        skipped
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s
                .strip_prefix(open.as_str())
                .and_then(|r| r.strip_suffix(close.as_str()))
                .expect("wrapped in the markers"))
            .collect::<Vec<_>>(),
        ["node_modules/"]
    );
    let files = by_default
        .get("files")
        .and_then(Value::as_i64)
        .expect("files");
    let all = parsed(&sb.run(&["--json", "--no-tools", "--all", arg], &[]));
    assert_eq!(
        all.get("skipped").and_then(Value::as_array).map(Vec::len),
        Some(0)
    );
    assert_eq!(all.get("files").and_then(Value::as_i64), Some(files + 1));
}

#[test]
fn repo_names_reach_the_output_escaped() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    symlink(LINK_TARGET, target.path().join("a\u{202e}b")).expect("symlink");
    let arg = target.path().to_str().expect("utf-8");
    for args in [vec!["--no-tools", arg], vec!["--json", "--no-tools", arg]] {
        let out = sb.run(&args, &[]);
        assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
        assert!(
            !out.stdout.contains('\u{202e}'),
            "a raw bidi character was printed"
        );
        assert!(out.stdout.contains("\\u{202E}"), "{}", out.stdout);
    }
}

#[test]
fn notice_prints_guarddog_and_exits_0() {
    let sb = Sandbox::new();
    let out = sb.run(&["--notice"], &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stderr, "");
    assert!(out.stdout.contains("DataDog/guarddog"), "{}", out.stdout);
    assert!(
        out.stdout.contains("Apache License, Version 2.0"),
        "{}",
        out.stdout
    );
    assert_eq!(out.stdout, anthology_scan::report::NOTICE);
}

#[test]
fn usage_errors_exit_3_with_one_line() {
    let sb = Sandbox::new();
    let folder = tempfile::tempdir().expect("temp dir");
    let f = folder.path().to_str().expect("utf-8").to_string();
    let a_file = folder.path().join("a.txt");
    fs::write(&a_file, "x").expect("write");
    let missing = folder.path().join("missing");
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["--json"],
        vec![&f, &f],
        vec!["--bogus", &f],
        vec!["--rev", "main", &f],
        vec!["--into", "somewhere", &f],
        vec!["--rev"],
        vec![missing.to_str().expect("utf-8")],
        vec![a_file.to_str().expect("utf-8")],
        vec!["http://example.invalid/owner/repo.git"],
        vec!["--rev", "-bad", "https://example.invalid/owner/repo.git"],
        vec!["https://example.invalid/--upload-pack=x/repo.git", "extra"],
        vec!["-x"],
    ];
    for args in cases {
        let out = sb.run(&args, &[]);
        assert_eq!(out.code, 3, "{args:?}: {}", out.stderr);
        one_line_error(&out);
    }
}

#[test]
fn usage_errors_escape_what_they_echo() {
    let sb = Sandbox::new();
    let out = sb.run(&["no-such-\u{202e}folder\u{1b}[31m"], &[]);
    one_line_error(&out);
    assert!(
        !out.stderr.contains('\u{202e}') && !out.stderr.contains('\u{1b}'),
        "{:?}",
        out.stderr
    );
    assert!(out.stderr.contains("\\u{202E}"), "{}", out.stderr);
}

// ---- Interrupts: nothing is left running, and no folder is left behind ----

/// A program that says it started and then runs until it is killed, writing a line to `beat` in
/// `HOME` every 50 ms. (A pid check cannot show it died: in a container nothing reaps an orphan,
/// and a zombie still answers signal 0. A file that stops growing can.)
fn hang(bin: &Path, name: &str) {
    script(
        bin,
        name,
        "#!/bin/sh\n: > \"$HOME/started\"\nwhile :; do echo x >> \"$HOME/beat\"; sleep 0.05; done\n",
    );
}

fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    done()
}

/// The `beat` file has stopped growing, after it grew.
fn heartbeat_stopped(beat: &Path) -> bool {
    std::thread::sleep(Duration::from_millis(400));
    let first = fs::metadata(beat).map_or(0, |m| m.len());
    std::thread::sleep(Duration::from_millis(500));
    let second = fs::metadata(beat).map_or(0, |m| m.len());
    first > 0 && first == second
}

fn send(child: &std::process::Child, signal: &str) {
    let status = Command::new("kill")
        .args(["-s", signal, &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill -s {signal} failed");
}

fn finish(child: &mut std::process::Child, limit: Duration) -> std::process::ExitStatus {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    panic!("the process did not end within {limit:?}");
}

fn stderr_of(child: &mut std::process::Child) -> String {
    use std::io::Read;
    let mut text = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut text);
    }
    text
}

/// Starts a fetch whose `ssh` never answers, waits until git is running it, and returns the
/// pieces a test checks afterwards.
fn hung_fetch(sb: &Sandbox) -> (std::process::Child, PathBuf, PathBuf) {
    let bin = sb.path().join("bin");
    hang(&bin, "ssh");
    let into = sb.path().join("quarantine");
    let template = sb.path().join(".quarantine.template");
    let child = sb.spawn(
        &[
            "--no-tools",
            "--into",
            into.to_str().expect("utf-8"),
            "ssh://example.invalid/some/repo",
        ],
        &[&bin],
    );
    assert!(
        wait_until(Duration::from_secs(30), || sb
            .home()
            .join("started")
            .exists()),
        "git never started ssh"
    );
    assert!(
        into.is_dir(),
        "the quarantine folder should exist mid-fetch"
    );
    assert!(
        template.is_dir(),
        "the template folder should exist mid-fetch"
    );
    (child, into, template)
}

#[test]
fn an_interrupt_mid_fetch_stops_git_and_removes_the_quarantine() {
    for (signal, code) in [("INT", 130), ("TERM", 143), ("HUP", 129)] {
        let sb = Sandbox::new();
        let (mut child, into, template) = hung_fetch(&sb);
        send(&child, signal);
        let status = finish(&mut child, Duration::from_secs(30));
        // The handler ran (a plain death by signal has no exit code) and said so.
        assert_eq!(status.code(), Some(code), "SIG{signal}: {status:?}");
        let err = stderr_of(&mut child);
        assert!(err.contains("interrupted"), "SIG{signal}: {err}");
        assert!(!into.exists(), "SIG{signal}: the quarantine was left");
        assert!(
            !template.exists(),
            "SIG{signal}: the template folder was left"
        );
        assert!(
            heartbeat_stopped(&sb.home().join("beat")),
            "SIG{signal}: ssh kept running"
        );
    }
}

/// The signal thread kills git's group, and the main thread sees git die. If the main thread
/// reported the failed fetch and exited 3 before the signal thread exited, the exit code would be
/// 3, not 128 plus the signal. Fifty interrupts, five at a time so the machine is busy, and no
/// sleep between the signal and the check: the exit code and the folders must be right at once.
#[test]
fn an_interrupt_mid_fetch_always_exits_with_the_signal_code() {
    let workers: Vec<_> = (0..5)
        .map(|_| {
            std::thread::spawn(|| {
                for round in 0..10 {
                    let sb = Sandbox::new();
                    let (mut child, into, template) = hung_fetch(&sb);
                    send(&child, "TERM");
                    let status = finish(&mut child, Duration::from_secs(30));
                    assert_eq!(status.code(), Some(143), "round {round}: {status:?}");
                    let err = stderr_of(&mut child);
                    assert!(err.contains("interrupted"), "round {round}: {err}");
                    assert!(!into.exists(), "round {round}: the quarantine was left");
                    assert!(!template.exists(), "round {round}: the template was left");
                }
            })
        })
        .collect();
    for w in workers {
        w.join().expect("a round failed");
    }
}

#[test]
fn a_killed_cli_leaves_no_child_and_no_quarantine() {
    kill_during_the_fetch();
}

/// Kills the CLI the moment the program git runs has started: the watchdog must do the cleaning.
fn kill_during_the_fetch() {
    // SIGKILL cannot be caught: the watchdog of each child does the cleaning.
    let sb = Sandbox::new();
    let (mut child, into, template) = hung_fetch(&sb);
    child.kill().expect("kill the CLI");
    let _ = child.wait();
    assert!(
        wait_until(Duration::from_secs(20), || !into.exists()
            && !template.exists()),
        "the watchdog left the quarantine"
    );
    assert!(
        heartbeat_stopped(&sb.home().join("beat")),
        "ssh kept running"
    );
}

#[test]
fn an_interrupt_during_the_tools_removes_the_scratch_folder() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    folder_fixture(target.path());
    let bin = sb.path().join("bin");
    hang(&bin, "osv-scanner");
    let mut child = sb.spawn(&[target.path().to_str().expect("utf-8")], &[&bin]);
    assert!(
        wait_until(Duration::from_secs(30), || sb
            .home()
            .join("started")
            .exists()),
        "the tool never started"
    );
    assert_eq!(
        fs::read_dir(sb.tmp()).expect("tmp").count(),
        1,
        "one scratch folder"
    );
    send(&child, "TERM");
    let status = finish(&mut child, Duration::from_secs(30));
    assert_eq!(status.code(), Some(143), "{status:?}");
    assert_eq!(
        fs::read_dir(sb.tmp()).expect("tmp").count(),
        0,
        "the scratch folder was left"
    );
    assert!(
        heartbeat_stopped(&sb.home().join("beat")),
        "the tool kept running"
    );
    // The folder that was scanned is untouched.
    assert!(target.path().join("README.md").exists());
}

#[test]
fn a_killed_cli_leaves_no_tool_and_no_scratch_folder() {
    kill_during_the_tools();
}

/// Kills the CLI the moment a tool has started: that is the earliest moment the test can see, and
/// so the one a watchdog that learned the group late would miss on a loaded host.
fn kill_during_the_tools() {
    let sb = Sandbox::new();
    let target = tempfile::tempdir().expect("temp dir");
    folder_fixture(target.path());
    let bin = sb.path().join("bin");
    hang(&bin, "osv-scanner");
    let mut child = sb.spawn(&[target.path().to_str().expect("utf-8")], &[&bin]);
    assert!(
        wait_until(Duration::from_secs(30), || sb
            .home()
            .join("started")
            .exists()),
        "the tool never started"
    );
    child.kill().expect("kill the CLI");
    let _ = child.wait();
    assert!(
        wait_until(Duration::from_secs(20), || {
            fs::read_dir(sb.tmp()).is_ok_and(|mut d| d.next().is_none())
        }),
        "the watchdog left the scratch folder"
    );
    assert!(
        heartbeat_stopped(&sb.home().join("beat")),
        "the tool kept running"
    );
}

#[test]
fn a_killed_cli_always_cleans_up_however_early_it_is_killed() {
    // Several rounds at once, so the host is busy and the CLI is often descheduled right after it
    // started a child: the cleaning must not depend on how fast the CLI got round to arming it.
    let workers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..4 {
                    kill_during_the_tools();
                    kill_during_the_fetch();
                }
            })
        })
        .collect();
    for w in workers {
        w.join().expect("a round failed");
    }
}

// ---- Relative PATH entries, the quarantine's own config, an old git ----

#[test]
fn a_relative_path_entry_never_runs_a_program_from_the_working_folder() {
    let sb = Sandbox::new();
    // git runs in the parent folder of `--into`; a `.` entry would find this `ssh` there.
    let marker = sb.path().join("ran-from-the-working-folder");
    script(
        sb.path(),
        "ssh",
        &format!("#!/bin/sh\n: > '{}'\nexit 1\n", marker.display()),
    );
    let into = sb.path().join("quarantine");
    let out = sb.run(
        &[
            "--no-tools",
            "--into",
            into.to_str().expect("utf-8"),
            "ssh://example.invalid/some/repo",
        ],
        &[Path::new("."), Path::new("")],
    );
    one_line_error(&out);
    assert!(!marker.exists(), "a program from the working folder ran");
    assert!(!into.exists());
}

#[test]
fn the_quarantine_keeps_its_own_protections_in_git_config() {
    let sb = Sandbox::new();
    let repo = sb.path().join("remote");
    source_repo(&repo);
    let bin = sb.path().join("bin");
    fake_ssh(&bin);
    let into = sb.path().join("quarantine");
    let out = sb.run(
        &[
            "--no-tools",
            "--into",
            into.to_str().expect("utf-8"),
            &ssh_url(&repo),
        ],
        &[&bin],
    );
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    let config = fs::read_to_string(into.join(".git/config")).expect("the quarantine's config");
    for want in [
        "symlinks = false",
        "hooksPath = /dev/null",
        "fsmonitor = false",
    ] {
        assert!(config.contains(want), "{want} missing in {config}");
    }
}

/// A `git` that reports `version` and otherwise is the real one (found by its folder).
fn git_claiming(bin: &Path, version: &str) {
    let real = [
        "/usr/bin/git",
        "/bin/git",
        "/usr/local/bin/git",
        "/opt/homebrew/bin/git",
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file())
    .expect("a git next to /usr/bin");
    script(
        bin,
        "git",
        &format!(
            "#!/bin/sh\ncase \"$*\" in *--version*) echo 'git version {version}' ;; *) exec {real} \"$@\" ;; esac\n"
        ),
    );
}

#[test]
fn an_old_git_is_a_note_in_the_json_and_on_stderr_never_a_refusal() {
    let sb = Sandbox::new();
    let repo = sb.path().join("remote");
    source_repo(&repo);
    let bin = sb.path().join("bin");
    fake_ssh(&bin);
    git_claiming(&bin, "2.34.1");
    let url = ssh_url(&repo);
    let into = sb.path().join("quarantine");
    let out = sb.run(
        &[
            "--json",
            "--no-tools",
            "--into",
            into.to_str().expect("utf-8"),
            &url,
        ],
        &[&bin],
    );
    // The scan ran to its end: the one HIGH finding gives exit 1.
    assert_eq!(out.code, 1, "{}{}", out.stdout, out.stderr);
    assert!(
        out.stderr.starts_with("anthology check-repo: note: "),
        "{}",
        out.stderr
    );
    assert_eq!(out.stderr.lines().count(), 1, "{}", out.stderr);
    assert!(
        out.stderr.contains("2.34.1") && out.stderr.contains("2.39.1"),
        "{}",
        out.stderr
    );
    let stdout = Out {
        code: out.code,
        stdout: out.stdout.clone(),
        stderr: String::new(),
    };
    let m = parsed(&stdout);
    let notes: Vec<&str> = m
        .get("notes")
        .and_then(Value::as_array)
        .expect("notes")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(
        notes[0].contains("2.34.1") && notes[0].contains("CVE-2022-23521"),
        "{notes:?}"
    );
    // The standing note stays last.
    assert_eq!(notes[1], "No findings is not proof of safety.");
    // A current git adds nothing.
    let again = sb.path().join("quarantine2");
    git_claiming(&bin, "2.43.0");
    let out = sb.run(
        &[
            "--json",
            "--no-tools",
            "--into",
            again.to_str().expect("utf-8"),
            &url,
        ],
        &[&bin],
    );
    assert_eq!(out.stderr, "");
    let m = parsed(&out);
    assert_eq!(
        m.get("notes").and_then(Value::as_array).map(Vec::len),
        Some(1)
    );
}
