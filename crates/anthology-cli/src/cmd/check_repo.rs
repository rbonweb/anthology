//! `anthology check-repo [--json] [--opened] [--no-tools] [--all] [--rev REV] [--into DIR] TARGET`:
//! a scan of a folder or a git URL for malicious code (check-repo Task 8).
//!
//! The crate `anthology-scan` does the work; this file is the part that touches the machine. It
//! reads the arguments, fetches a URL into a quarantine folder, hands the tree to the scan and the
//! optional tools, and prints the report. Nothing of the target runs: the only programs started
//! are `git` for a URL and the three scanners, each by [`RealRunner`].
//!
//! - TARGET is an existing folder, or a URL that starts with `https://`, `ssh://` or `git@`.
//! - A folder is scanned in place and never written to. A URL goes into `--into` (a new folder) or
//!   `<anthology home>/check-repo/<host>-<owner>-<repo>-<epoch>`, mode 0700.
//! - stdout is the text report, or with `--json` one JSON object. A failure prints one line
//!   `anthology check-repo: <message>` on stderr and exits 3, as a usage error does.
//! - Exit codes are the report's: 2 any CRITICAL; else 3 error or partial; else 1 any HIGH; else 0.
//! - Every string that came from the repo goes through the report's escaping. The messages here
//!   hold only the user's own arguments, the messages of `fetch` and the OS, and they are escaped
//!   too.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anthology_core::Env;
use anthology_core::sys;
use anthology_scan::escape::escape;
use anthology_scan::fetch::{self, FetchPlan};
use anthology_scan::report::{self, Meta, Source};
use anthology_scan::tools::{self, Runner, ToolRun};
use anthology_scan::tree::{DiskTree, WalkOpts};

use super::Output;

/// One deadline for the whole fetch of a URL.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// The most output a child may hand back; the rest is read and dropped so the child never blocks.
const OUTPUT_CAP: usize = 32 << 20;
/// How long the readers of a finished child's pipes get before its process group is killed (a
/// helper it left behind may hold them open), and after the kill.
const PIPE_GRACE: Duration = Duration::from_millis(500);
const PIPE_GRACE_AFTER_KILL: Duration = Duration::from_secs(5);
/// Longest message printed on stderr, in characters.
const MESSAGE_CHARS: usize = 600;

const URL_START: [&str; 3] = ["https://", "ssh://", "git@"];

/// Runs `anthology check-repo …`; `args` are the arguments after `check-repo`.
pub fn run(args: &[OsString], _stdin: &mut dyn Read) -> Output {
    let env = Env::from_process();
    let runner = RealRunner {
        path: env.get_os("PATH").map(OsStr::to_os_string).unwrap_or_default(),
    };
    run_with(args, &env, &runner)
}

/// The command with its environment and its way of starting programs given.
fn run_with(args: &[OsString], env: &Env, runner: &dyn Runner) -> Output {
    match execute(args, env, runner) {
        Ok(out) => out,
        Err(message) => fail(&message),
    }
}

/// One line on stderr and exit 3.
fn fail(message: &str) -> Output {
    Output {
        stderr: format!("anthology check-repo: {}\n", one_line(message)).into_bytes(),
        code: 3,
        ..Output::default()
    }
}

/// `text` escaped and on one line: hidden characters written out, line breaks and tabs as `\n` and
/// `\t`, cut at `MESSAGE_CHARS`.
fn one_line(text: &str) -> String {
    escape(text.as_bytes(), MESSAGE_CHARS)
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// A path or argument as it may be printed.
fn shown(s: &OsStr) -> String {
    one_line(&String::from_utf8_lossy(s.as_bytes()))
}

#[derive(Default)]
struct Opts {
    json: bool,
    opened: bool,
    no_tools: bool,
    all: bool,
    notice: bool,
    rev: Option<String>,
    into: Option<PathBuf>,
    target: Option<OsString>,
}

fn parse(args: &[OsString]) -> Result<Opts, String> {
    use lexopt::prelude::*;
    let mut p = lexopt::Parser::from_args(args.iter().cloned());
    let mut o = Opts::default();
    let bad = |e: lexopt::Error| e.to_string();
    while let Some(arg) = p.next().map_err(bad)? {
        match arg {
            Long("json") => o.json = true,
            Long("opened") => o.opened = true,
            Long("no-tools") => o.no_tools = true,
            Long("all") => o.all = true,
            Long("notice") => o.notice = true,
            Long("rev") => {
                let value = p.value().map_err(bad)?;
                let text = value
                    .into_string()
                    .map_err(|_| "--rev needs a commit hash, branch or tag name".to_string())?;
                o.rev = Some(fetch::validate_rev(&text)?);
            }
            Long("into") => o.into = Some(PathBuf::from(p.value().map_err(bad)?)),
            Value(target) => {
                if o.target.replace(target).is_some() {
                    return Err("only one TARGET is allowed".to_string());
                }
            }
            other => return Err(other.unexpected().to_string()),
        }
    }
    Ok(o)
}

fn is_url(target: &OsStr) -> bool {
    target
        .to_str()
        .is_some_and(|t| URL_START.iter().any(|start| t.starts_with(start)))
}

/// What the scan runs on, and what the report says about where it came from.
struct Prepared {
    tree: DiskTree,
    source: Source,
    /// The `target:` line: the URL as given, or the folder's canonical path.
    target: String,
    /// A quarantine folder this run made; removed if the run then fails.
    made: Option<PathBuf>,
}

fn execute(args: &[OsString], env: &Env, runner: &dyn Runner) -> Result<Output, String> {
    let o = parse(args)?;
    if o.notice {
        return Ok(Output {
            stdout: report::NOTICE.as_bytes().to_vec(),
            ..Output::default()
        });
    }
    let target = o
        .target
        .clone()
        .ok_or("needs a TARGET: a folder, or a git URL (https://, ssh:// or git@host:path)")?;
    let prepared = if is_url(&target) {
        open_url(&o, &target, env, runner)?
    } else {
        open_folder(&o, &target)?
    };
    let made = prepared.made.clone();
    let result = scan_and_report(&o, prepared, env, runner);
    if result.is_err()
        && let Some(dest) = made
    {
        let _ = fs::remove_dir_all(dest);
    }
    result
}

/// A folder the user named, scanned in place.
fn open_folder(o: &Opts, target: &OsStr) -> Result<Prepared, String> {
    let path = Path::new(target);
    match fs::metadata(path) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "{} is a file: give a folder, or a git URL (https://, ssh:// or git@host:path)",
                shown(target)
            ));
        }
        Err(e) => {
            return Err(format!(
                "{} is not a folder you can read ({e}) and not a git URL (https://, ssh:// or git@host:path)",
                shown(target)
            ));
        }
    }
    if o.rev.is_some() || o.into.is_some() {
        return Err("--rev and --into apply to a git URL, not to a folder".to_string());
    }
    let tree = DiskTree::open(path, &walk_opts(o), None)
        .map_err(|e| format!("cannot read {}: {e}", shown(target)))?;
    let canonical = tree.root().to_string_lossy().into_owned();
    Ok(Prepared {
        tree,
        source: Source::Path,
        target: canonical,
        made: None,
    })
}

/// A URL, fetched into the quarantine folder.
fn open_url(o: &Opts, target: &OsStr, env: &Env, runner: &dyn Runner) -> Result<Prepared, String> {
    let url = target
        .to_str()
        .ok_or("the URL is not valid UTF-8")
        .map(str::to_string)?;
    fetch::validate_url(&url)?;
    let dest = match &o.into {
        Some(into) => into.clone(),
        None => default_dest(env, &url)?,
    };
    let plan = FetchPlan {
        url: url.clone(),
        rev: o.rev.clone(),
        dest,
    };
    let modes = fetch::fetch(&plan, runner, FETCH_TIMEOUT)?;
    // The fetch made the folder and removes it itself on failure; from here this run does.
    let dest = std::path::absolute(&plan.dest).unwrap_or_else(|_| plan.dest.clone());
    let tree = match DiskTree::open(&dest, &walk_opts(o), Some(&modes)) {
        Ok(tree) => tree,
        Err(e) => {
            let _ = fs::remove_dir_all(&dest);
            return Err(format!("cannot read the fetched folder: {e}"));
        }
    };
    Ok(Prepared {
        tree,
        source: Source::Url {
            url: url.clone(),
            rev: o.rev.clone().unwrap_or_else(|| "HEAD".to_string()),
            dest: dest.clone(),
        },
        target: url,
        made: Some(dest),
    })
}

fn walk_opts(o: &Opts) -> WalkOpts {
    WalkOpts {
        all: o.all,
        ..WalkOpts::default()
    }
}

/// `<anthology home>/check-repo/<host>-<owner>-<repo>-<epoch>`, with the folder above it made
/// (mode 0700) when it is not there.
fn default_dest(env: &Env, url: &str) -> Result<PathBuf, String> {
    let parent = anthology_core::paths::anthology_home(env).join("check-repo");
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&parent)
        .map_err(|e| format!("cannot create {}: {e}", shown(parent.as_os_str())))?;
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    Ok(parent.join(fetch::dest_name(url, epoch)))
}

/// The scan, the tools and the report of a prepared tree.
fn scan_and_report(
    o: &Opts,
    prepared: Prepared,
    env: &Env,
    runner: &dyn Runner,
) -> Result<Output, String> {
    let Prepared {
        tree,
        source,
        target,
        ..
    } = prepared;
    let mut scan = anthology_scan::scan(&tree);
    let mut tool_runs: Vec<ToolRun> = Vec::new();
    if !o.no_tools {
        let scratch = Scratch::new(env, tree.root())?;
        let (runs, found) = tools::run_tools(tree.root(), scratch.path(), runner);
        tool_runs = runs;
        scan.findings.extend(found);
    }
    let meta = Meta {
        target,
        source,
        opened: o.opened,
        tools: tool_runs,
    };
    let mut text = if o.json {
        let nonce =
            report::new_nonce().map_err(|e| format!("cannot read random bytes for the report: {e}"))?;
        report::render_json(&scan, &meta, &nonce)
    } else {
        report::render_text(&scan, &meta)
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let code = report::exit_code(&scan);
    let stderr = if code == 3 {
        b"anthology check-repo: the scan is partial or hit an error (exit 3); see the report\n"
            .to_vec()
    } else {
        Vec::new()
    };
    Ok(Output {
        stdout: text.into_bytes(),
        stderr,
        code,
    })
}

/// A new folder for the tools' config files and reports, outside the target, removed when the
/// value is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(env: &Env, target: &Path) -> Result<Scratch, String> {
        let base = env
            .get_os("TMPDIR")
            .filter(|t| !t.is_empty())
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        if fs::canonicalize(&base).is_ok_and(|b| b.starts_with(target)) {
            return Err(format!(
                "the temp folder {} is inside the target: run with --no-tools or set TMPDIR elsewhere",
                shown(base.as_os_str())
            ));
        }
        for _ in 0..8 {
            let nonce = report::new_nonce()
                .map_err(|e| format!("cannot read random bytes for a scratch folder: {e}"))?;
            let path = base.join(format!("anthology-check-repo-{nonce}"));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Scratch(path)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => {
                    return Err(format!(
                        "cannot create a scratch folder in {}: {e}",
                        shown(base.as_os_str())
                    ));
                }
            }
        }
        Err("cannot find a free name for a scratch folder".to_string())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Starts the programs `fetch` and `run_tools` ask for. Each runs with exactly the environment it
/// is given, stdin closed, and as the leader of a session of its own: no controlling terminal, so a
/// prompt (an `ssh` password) fails instead of waiting for the user, and one process group, killed
/// as a whole at the deadline.
struct RealRunner {
    /// `PATH` as the process got it.
    path: OsString,
}

impl Runner for RealRunner {
    fn find(&self, name: &str) -> Option<PathBuf> {
        if name.is_empty() || name.contains('/') {
            return None;
        }
        // A relative entry (an empty one, `.`) would run a program from the working folder, which
        // may be the repo.
        std::env::split_paths(&self.path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| {
                fs::metadata(candidate)
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    }

    fn run(
        &self,
        prog: &Path,
        args: &[OsString],
        cwd: &Path,
        env: &[(OsString, OsString)],
        timeout: Duration,
    ) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
        let mut cmd = Command::new(prog);
        cmd.args(args)
            .current_dir(cwd)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        sys::new_session(&mut cmd);
        let mut child = cmd.spawn()?;
        // The child leads its own group, so its process id is the group's.
        let group = child.id();
        let (tx, rx) = mpsc::channel::<(usize, Vec<u8>)>();
        if let Some(out) = child.stdout.take() {
            spawn_reader(0, out, tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_reader(1, err, tx.clone());
        }
        drop(tx);

        let deadline = Instant::now() + timeout;
        let mut nap = Duration::from_millis(1);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                sys::kill_group(group);
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the program ran past {timeout:?} and was killed"),
                ));
            }
            std::thread::sleep(nap);
            nap = (nap * 2).min(Duration::from_millis(20));
        };

        // Both pipes close when the child and everything it started are gone. A helper that
        // outlived the child holds them open: kill the group, then take what was read.
        let mut bufs = [Vec::new(), Vec::new()];
        let mut got = 0;
        let mut wait = PIPE_GRACE;
        let mut killed = false;
        while got < 2 {
            match rx.recv_timeout(wait) {
                Ok((i, bytes)) => {
                    bufs[i] = bytes;
                    got += 1;
                }
                Err(mpsc::RecvTimeoutError::Timeout) if !killed => {
                    sys::kill_group(group);
                    killed = true;
                    wait = PIPE_GRACE_AFTER_KILL;
                }
                Err(_) => break,
            }
        }
        let [stdout, stderr] = bufs;
        let code = status
            .code()
            .or_else(|| status.signal().map(|s| 128 + s))
            .unwrap_or(-1);
        Ok((code, stdout, stderr))
    }
}

/// Reads `pipe` to its end on a thread of its own and sends the first `OUTPUT_CAP` bytes.
fn spawn_reader<R: Read + Send + 'static>(
    which: usize,
    mut pipe: R,
    tx: mpsc::Sender<(usize, Vec<u8>)>,
) {
    std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 16 << 10];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let room = OUTPUT_CAP.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = tx.send((which, kept));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner() -> RealRunner {
        RealRunner {
            path: OsString::from("/usr/bin:/bin"),
        }
    }

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn the_runner_hands_the_child_only_the_environment_it_is_given() {
        let env = [(os("ONLY"), os("this"))];
        let (code, out, _) = runner()
            .run(
                Path::new("/usr/bin/env"),
                &[],
                Path::new("/"),
                &env,
                Duration::from_secs(10),
            )
            .expect("run env");
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8_lossy(&out), "ONLY=this\n");
    }

    #[test]
    fn the_runner_closes_stdin() {
        let started = Instant::now();
        let (code, out, _) = runner()
            .run(
                Path::new("/bin/cat"),
                &[],
                Path::new("/"),
                &[],
                Duration::from_secs(30),
            )
            .expect("run cat");
        assert_eq!(code, 0);
        assert!(out.is_empty());
        assert!(started.elapsed() < Duration::from_secs(10), "cat waited for input");
    }

    #[test]
    fn the_runner_gives_a_child_no_controlling_terminal() {
        // `/dev/tty` cannot be opened by a process of a session with no terminal.
        let (code, _, _) = runner()
            .run(
                Path::new("/bin/sh"),
                &[os("-c"), os("exec 3</dev/tty")],
                Path::new("/"),
                &[],
                Duration::from_secs(10),
            )
            .expect("run sh");
        assert_ne!(code, 0, "the child opened /dev/tty");
    }

    #[test]
    fn the_runner_kills_the_whole_group_at_the_deadline() {
        // A background loop appends to `beat`; once the deadline has killed the group the file
        // stops growing. (A pid check would not do: in a container nothing reaps the orphan, and
        // a zombie still answers signal 0.)
        let dir = tempfile::tempdir().expect("temp dir");
        let started = Instant::now();
        let err = runner()
            .run(
                Path::new("/bin/sh"),
                &[
                    os("-c"),
                    os("(while :; do echo x >> beat; sleep 0.05; done) & wait"),
                ],
                dir.path(),
                &[],
                Duration::from_millis(500),
            )
            .expect_err("the deadline passes");
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
        let beat = dir.path().join("beat");
        std::thread::sleep(Duration::from_millis(300));
        let first = fs::metadata(&beat).expect("the loop ran before the deadline").len();
        assert!(first > 0);
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(
            fs::metadata(&beat).expect("beat").len(),
            first,
            "the grandchild kept running after the deadline"
        );
    }

    #[test]
    fn the_runner_returns_when_a_helper_keeps_the_pipes_open() {
        // The shell exits at once; its background `sleep` still holds stdout.
        let started = Instant::now();
        let (code, _, _) = runner()
            .run(
                Path::new("/bin/sh"),
                &[os("-c"), os("sleep 60 &")],
                Path::new("/"),
                &[],
                Duration::from_secs(30),
            )
            .expect("run sh");
        assert_eq!(code, 0);
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
    }

    #[test]
    fn the_runner_reports_the_exit_code_and_both_streams() {
        let (code, out, err) = runner()
            .run(
                Path::new("/bin/sh"),
                &[os("-c"), os("echo out; echo err >&2; exit 7")],
                Path::new("/"),
                &[],
                Duration::from_secs(10),
            )
            .expect("run sh");
        assert_eq!((code, out.as_slice(), err.as_slice()), (7, &b"out\n"[..], &b"err\n"[..]));
    }

    #[test]
    fn scratch_is_private_outside_the_target_and_removed_on_drop() {
        let base = tempfile::tempdir().expect("temp dir");
        let target = tempfile::tempdir().expect("temp dir");
        let env = Env::default().with_os("TMPDIR", base.path());
        let made = {
            let s = Scratch::new(&env, &fs::canonicalize(target.path()).expect("canonical"))
                .expect("a scratch folder");
            let mode = fs::metadata(s.path()).expect("scratch").permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
            assert!(s.path().starts_with(base.path()));
            s.path().to_path_buf()
        };
        assert!(!made.exists(), "the scratch folder outlived its guard");
    }

    #[test]
    fn scratch_refuses_a_temp_folder_inside_the_target() {
        let target = tempfile::tempdir().expect("temp dir");
        let inside = target.path().join("tmp");
        fs::create_dir(&inside).expect("make folder");
        let env = Env::default().with_os("TMPDIR", &inside);
        let err = Scratch::new(&env, &fs::canonicalize(target.path()).expect("canonical"))
            .err()
            .expect("refused");
        assert!(err.contains("inside the target"), "{err}");
        assert_eq!(fs::read_dir(&inside).expect("list").count(), 0, "a folder was made anyway");
    }

    #[test]
    fn find_takes_only_an_absolute_path_entry_with_an_executable_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let tool = dir.path().join("a-tool");
        fs::write(&tool, "#!/bin/sh\n").expect("write");
        // The same folder as a path relative to the working folder of the test.
        let cwd = std::env::current_dir().expect("cwd");
        let mut relative = PathBuf::new();
        for _ in cwd.components().skip(1) {
            relative.push("..");
        }
        relative.push(dir.path().strip_prefix("/").expect("absolute"));
        assert_eq!(fs::canonicalize(&relative).ok(), fs::canonicalize(dir.path()).ok());
        let r = RealRunner {
            path: std::env::join_paths([Path::new(""), Path::new("."), relative.as_path(), dir.path()])
                .expect("join"),
        };
        assert_eq!(r.find("a-tool"), None, "not executable yet");
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("chmod");
        // The relative entry comes first and holds the program too; only the absolute one counts.
        assert_eq!(r.find("a-tool"), Some(tool));
        assert_eq!(r.find("a/tool"), None);
        assert_eq!(r.find(""), None);
        assert_eq!(r.find("no-such-tool-anywhere"), None);
    }
}
