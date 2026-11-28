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
use std::io::{self, PipeWriter, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anthology_core::Env;
use anthology_core::sys;
use anthology_pycompat::json::{self, DumpOpts, Value};
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
    let guard = Arc::new(Guard::default());
    watch_signals(&guard);
    let runner = RealRunner {
        path: env
            .get_os("PATH")
            .map(OsStr::to_os_string)
            .unwrap_or_default(),
        guard: Arc::clone(&guard),
    };
    run_with(args, &env, &runner, &guard)
}

/// The command with its environment, its way of starting programs and its cleanup list given.
fn run_with(args: &[OsString], env: &Env, runner: &dyn Runner, guard: &Guard) -> Output {
    let out = match execute(args, env, runner, guard) {
        Ok(out) => out,
        Err(message) => fail(&message),
    };
    // A signal caught on the way decides the exit code, not this result.
    guard.defer_to_signal();
    out
}

/// Catches `SIGINT`, `SIGTERM` and `SIGHUP`: a thread stops every child, removes every folder the
/// run made (the half-made quarantine, the scratch folder) and exits with `128 + signal`, as a
/// shell reports a death by that signal. If the handlers cannot be installed the run goes on
/// without: the watchdogs of the children still stop them and remove the folders.
fn watch_signals(guard: &Arc<Guard>) {
    let Ok(mut pipe) = sys::catch_termination() else {
        return;
    };
    let guard = Arc::clone(guard);
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        if pipe.read_exact(&mut byte).is_err() {
            return;
        }
        let sig = i32::from(byte[0]);
        guard.shutdown();
        let name = match sig {
            1 => "SIGHUP",
            2 => "SIGINT",
            15 => "SIGTERM",
            _ => "a signal",
        };
        let _ = writeln!(
            io::stderr(),
            "anthology check-repo: interrupted by {name}; the programs it started were stopped and its quarantine and scratch folders removed"
        );
        std::process::exit(128 + sig);
    });
}

/// What an interrupt, or the death of this process, has to clean up: the process groups of the
/// children that run now (each with a watchdog) and the folders this run made.
#[derive(Default)]
struct Guard {
    state: Mutex<GuardState>,
}

#[derive(Default)]
struct GuardState {
    /// Set by `shutdown`: no child starts after it.
    down: bool,
    next: u64,
    paths: Vec<PathBuf>,
    kids: Vec<Kid>,
}

struct Kid {
    id: u64,
    group: u32,
    watchdog: Watchdog,
}

/// A child that is registered with its guard until this value is dropped.
struct Registered<'a> {
    guard: &'a Guard,
    id: u64,
}

impl Registered<'_> {
    /// Takes the child out of the guard, then reaps it. The order is the point: a reaped child
    /// frees its pid, which is also its group's id, and the system may give that id to a stranger
    /// at once. Until the reap the child is a zombie that keeps the id, so the group may still be
    /// signaled; once it is reaped nothing may signal it, and nothing does: the watchdog stood
    /// down and `shutdown` no longer lists the child.
    fn reap(self, child: &mut Child) -> io::Result<ExitStatus> {
        self.guard.finish(self.id);
        child.wait()
    }
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        self.guard.finish(self.id);
    }
}

impl Guard {
    fn lock(&self) -> MutexGuard<'_, GuardState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `path` is a folder this run made; an interrupt removes it. Only a folder that did not exist
    /// before the run belongs here: the list is what `rm -rf` runs on.
    fn add_path(&self, path: &Path) {
        // Absolute: the watchdog runs in `/`, so a relative path would name another folder there.
        if let Ok(path) = std::path::absolute(path) {
            self.lock().paths.push(path);
        }
    }

    /// `path` is done with: it was removed, or it is the result and stays.
    fn forget_path(&self, path: &Path) {
        if let Ok(path) = std::path::absolute(path) {
            self.lock().paths.retain(|p| *p != path);
        }
    }

    /// Starts `cmd` (a session leader, see `sys::new_session`) and puts it under a watchdog.
    fn spawn(&self, cmd: &mut Command) -> io::Result<(Child, Registered<'_>)> {
        let mut st = self.lock();
        if st.down {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "the run was interrupted",
            ));
        }
        // The watchdog first, and the child tells it its group before it runs its program: no
        // moment in which the child runs and a kill of this process leaves it unwatched.
        let watchdog = Watchdog::start(&st.paths).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("cannot start the watchdog of a program: {e}"),
            )
        })?;
        watchdog.announce_to(cmd);
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                watchdog.finish();
                return Err(e);
            }
        };
        let group = child.id();
        st.next += 1;
        let id = st.next;
        st.kids.push(Kid {
            id,
            group,
            watchdog,
        });
        Ok((child, Registered { guard: self, id }))
    }

    /// The child `id` ended: its watchdog stands down.
    fn finish(&self, id: u64) {
        let kid = {
            let mut st = self.lock();
            st.kids
                .iter()
                .position(|k| k.id == id)
                .map(|at| st.kids.remove(at))
        };
        if let Some(kid) = kid {
            kid.watchdog.finish();
        }
    }

    /// Called by the main thread wherever it would go on after a child ended, and before it ends
    /// the run. If a termination signal was caught, the signal thread owns what happens next: it
    /// is stopping the children (so a child that just ended may be its doing, and the failure the
    /// main thread would report is not a real one), removing the folders and choosing the exit
    /// code. This waits for that thread to finish its cleanup (taking the lock does that) and then
    /// never returns: the signal thread ends the process with `128 + signal`.
    ///
    /// `down` is set under the lock before the first child is killed, so a child that died because
    /// of the signal is always seen as such here.
    fn defer_to_signal(&self) {
        if self.lock().down {
            loop {
                std::thread::park();
            }
        }
    }

    /// Stops every child, removes every registered folder and refuses new children. The lock is
    /// held throughout, so a child being started waits and then finds the run down.
    fn shutdown(&self) {
        let mut st = self.lock();
        st.down = true;
        for kid in st.kids.drain(..) {
            sys::kill_group(kid.group);
            kid.watchdog.finish();
        }
        for path in st.paths.drain(..) {
            let _ = fs::remove_dir_all(path);
        }
    }

    #[cfg(test)]
    fn running(&self) -> usize {
        self.lock().kids.len()
    }

    #[cfg(test)]
    fn registered(&self) -> Vec<PathBuf> {
        self.lock().paths.clone()
    }
}

/// What the watchdog of a child runs. The first line it reads is the group of the child, written
/// by the child itself before it runs its program (`sys::announce_group`), or `done` when no child
/// came; then a line from the run (it ended normally: stand down), or the end of the pipe without
/// one (the run died: stop the group, remove the folders). The end of the pipe before any group
/// means the run died before the child existed: only the folders are removed.
const WATCHDOG: &str = concat!(
    "if read -r group; then\n",
    "  case $group in\n",
    "    done) exit 0 ;;\n",
    "    ''|*[!0-9]*) group= ;;\n",
    "  esac\n",
    "  read -r line && exit 0\n",
    "else\n",
    "  group=\n",
    "fi\n",
    "[ -n \"$group\" ] && kill -s KILL -- \"-$group\" 2>/dev/null\n",
    "[ \"$#\" -gt 0 ] && rm -rf -- \"$@\"\n",
    "exit 0\n",
);

/// A shell in a session of its own that outlives a run killed by `SIGKILL`, which no handler can
/// catch, and cleans up: macOS has nothing like `PR_SET_PDEATHSIG`, so the pipe does it. It holds
/// the read end of a pipe; this process holds the only write end (no child inherits it past its
/// `exec`: pipes are close-on-exec), so it sees the end of the pipe when this process is gone,
/// whatever the cause.
///
/// It is started before the child it watches, and the child tells it its own group: a run killed
/// at any moment after the child exists has a watchdog that knows the group. Told by the run after
/// the start, the group would not be known for as long as the run takes to get from one to the
/// other, which on a loaded host is long enough for a kill to fall in.
struct Watchdog {
    child: Child,
    pipe: PipeWriter,
}

impl Watchdog {
    /// Watches the absolute folders `paths`, and the group of the one child that is started with
    /// `announce_to`.
    fn start(paths: &[PathBuf]) -> io::Result<Watchdog> {
        let (reader, pipe) = io::pipe()?;
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(WATCHDOG)
            .arg("anthology-watchdog")
            .args(paths)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .current_dir("/")
            .stdin(Stdio::from(reader))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        sys::new_session(&mut cmd);
        let child = cmd.spawn()?;
        Ok(Watchdog { child, pipe })
    }

    /// Makes `cmd`, which must be a session leader (`sys::new_session`), tell this watchdog its
    /// group before it runs its program.
    fn announce_to(&self, cmd: &mut Command) {
        sys::announce_group(cmd, self.pipe.as_raw_fd());
    }

    /// The run is over in an orderly way: the watchdog exits and touches nothing.
    fn finish(self) {
        let Watchdog {
            mut child,
            mut pipe,
        } = self;
        let _ = pipe.write_all(b"done\n");
        drop(pipe);
        let _ = child.wait();
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
    /// What the report adds as notes (an old git).
    notes: Vec<String>,
}

fn execute(
    args: &[OsString],
    env: &Env,
    runner: &dyn Runner,
    guard: &Guard,
) -> Result<Output, String> {
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
        open_url(&o, &target, env, runner, guard)?
    } else {
        open_folder(&o, &target)?
    };
    let made = prepared.made.clone();
    let result = scan_and_report(&o, prepared, env, runner, guard);
    if let Some(dest) = made {
        // The quarantine is the result of a finished run, and of nothing else.
        guard.defer_to_signal();
        if result.is_err() {
            let _ = fs::remove_dir_all(&dest);
        }
        guard.forget_path(&dest);
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
        notes: Vec::new(),
    })
}

/// A URL, fetched into the quarantine folder.
fn open_url(
    o: &Opts,
    target: &OsStr,
    env: &Env,
    runner: &dyn Runner,
    guard: &Guard,
) -> Result<Prepared, String> {
    let url = target
        .to_str()
        .ok_or("the URL is not valid UTF-8")
        .map(str::to_string)?;
    fetch::validate_url(&url)?;
    let dest = match &o.into {
        Some(into) => {
            // `new/../old` does not resolve while `new` is missing, so it would look like a folder
            // that does not exist, and an interrupt would then remove `old`. A path with no `..`
            // names exactly the folder it says.
            if into.components().any(|c| c == Component::ParentDir) {
                return Err(format!(
                    "--into {} has a `..` in it: give the folder without it",
                    shown(into.as_os_str())
                ));
            }
            into.clone()
        }
        None => default_dest(env, &url)?,
    };
    let dest = std::path::absolute(&dest)
        .map_err(|e| format!("cannot resolve {}: {e}", shown(dest.as_os_str())))?;
    // An interrupt removes the quarantine and the template folder next to it. Only a folder that
    // does not exist yet is ours to remove: the fetch refuses one that does and never touches it.
    let template = fetch::template_path(&dest);
    if dest.symlink_metadata().is_err() {
        guard.add_path(&dest);
        if let Some(t) = &template {
            guard.add_path(t);
        }
    }
    let plan = FetchPlan {
        url: url.clone(),
        rev: o.rev.clone(),
        dest: dest.clone(),
    };
    let done = fetch::fetch_with_notes(&plan, runner, FETCH_TIMEOUT);
    // The fetch removes the template folder itself, and the quarantine too when it fails.
    if let Some(t) = &template {
        guard.forget_path(t);
    }
    let done = match done {
        Ok(done) => done,
        Err(e) => {
            guard.forget_path(&dest);
            return Err(e);
        }
    };
    // From here this run removes the folder if it fails.
    let tree = match DiskTree::open(&dest, &walk_opts(o), Some(&done.modes)) {
        Ok(tree) => tree,
        Err(e) => {
            let _ = fs::remove_dir_all(&dest);
            guard.forget_path(&dest);
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
        notes: done.notes,
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
    guard: &Guard,
) -> Result<Output, String> {
    let Prepared {
        tree,
        source,
        target,
        notes,
        ..
    } = prepared;
    let mut scan = anthology_scan::scan(&tree);
    let mut tool_runs: Vec<ToolRun> = Vec::new();
    if !o.no_tools {
        let scratch = Scratch::new(env, tree.root(), guard)?;
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
        let nonce = report::new_nonce()
            .map_err(|e| format!("cannot read random bytes for the report: {e}"))?;
        with_notes(report::render_json(&scan, &meta, &nonce), &notes)
    } else {
        report::render_text(&scan, &meta)
    };
    if !text.ends_with('\n') {
        text.push('\n');
    }
    let code = report::exit_code(&scan);
    // A note is never an error: the scan ran, and its exit code is the report's.
    let mut stderr = String::new();
    for note in &notes {
        stderr.push_str(&format!("anthology check-repo: note: {}\n", one_line(note)));
    }
    if code == 3 {
        stderr.push_str(
            "anthology check-repo: the scan is partial or hit an error (exit 3); see the report\n",
        );
    }
    Ok(Output {
        stdout: text.into_bytes(),
        stderr: stderr.into_bytes(),
        code,
    })
}

/// `report` (the JSON) with `notes` added to its `notes` array, before the standing last note.
/// A document that does not parse is returned as it is.
fn with_notes(report: String, notes: &[String]) -> String {
    if notes.is_empty() {
        return report;
    }
    let Ok(mut doc) = json::loads(&report) else {
        return report;
    };
    let Some(list) = doc
        .as_object_mut()
        .and_then(|m| m.get_mut("notes"))
        .and_then(Value::as_array_mut)
    else {
        return report;
    };
    let at = list.len().saturating_sub(1);
    for (i, note) in notes.iter().enumerate() {
        list.insert(at + i, Value::Str(one_line(note)));
    }
    json::dumps(
        &doc,
        &DumpOpts {
            indent: None,
            sort_keys: false,
            ensure_ascii: true,
            item_sep: ",",
            key_sep: ":",
        },
    )
}

/// A new folder for the tools' config files and reports, outside the target, removed when the
/// value is dropped. While it exists an interrupt removes it too.
struct Scratch<'a> {
    path: PathBuf,
    guard: &'a Guard,
}

impl<'a> Scratch<'a> {
    fn new(env: &Env, target: &Path, guard: &'a Guard) -> Result<Scratch<'a>, String> {
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
                Ok(()) => {
                    guard.add_path(&path);
                    return Ok(Scratch { path, guard });
                }
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
        &self.path
    }
}

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
        self.guard.forget_path(&self.path);
    }
}

/// Starts the programs `fetch` and `run_tools` ask for. Each runs with exactly the environment it
/// is given, stdin closed, and as the leader of a session of its own: no controlling terminal, so a
/// prompt (an `ssh` password) fails instead of waiting for the user, and one process group, killed
/// as a whole at the deadline.
struct RealRunner {
    /// `PATH` as the process got it.
    path: OsString,
    /// Every child it starts is registered here, with a watchdog.
    guard: Arc<Guard>,
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
        // Registered until the child is reaped, whichever way this call ends: an interrupt or the
        // death of this process stops the group while it runs.
        let (mut child, registered) = self.guard.spawn(&mut cmd)?;
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
        // The child is not reaped until the end: an ended child stays a zombie that keeps its pid,
        // and with it the id of its group, so every kill below hits the child's own group and
        // never a stranger that was given the id.
        loop {
            if sys::exited(group)? {
                break;
            }
            if Instant::now() >= deadline {
                sys::kill_group(group);
                let _ = child.kill();
                let _ = registered.reap(&mut child);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("the program ran past {timeout:?} and was killed"),
                ));
            }
            std::thread::sleep(nap);
            nap = (nap * 2).min(Duration::from_millis(20));
        }
        // The signal thread may have killed it: then it, not this thread, ends the run (the
        // fetch must not clean up or report a failure of its own).
        self.guard.defer_to_signal();

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
        // Only now, with nothing left to signal, is the child reaped.
        let status = registered.reap(&mut child)?;
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
            guard: Arc::new(Guard::default()),
        }
    }

    /// Waits up to `limit` for `child` to end.
    fn wait_for(
        child: &mut std::process::Child,
        limit: Duration,
    ) -> Option<std::process::ExitStatus> {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            if let Some(status) = child.try_wait().expect("try_wait") {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// A `sleep` that leads its own session, like every child of the runner, under a watchdog of
    /// `paths` that it announces itself to.
    fn watched_sleeper(paths: &[PathBuf]) -> (std::process::Child, Watchdog) {
        let watchdog = Watchdog::start(paths).expect("start the watchdog");
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("60");
        sys::new_session(&mut cmd);
        watchdog.announce_to(&mut cmd);
        (cmd.spawn().expect("spawn sleep"), watchdog)
    }

    fn gone(path: &Path) -> bool {
        for _ in 0..100 {
            if path.symlink_metadata().is_err() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn a_watchdog_kills_the_group_and_removes_the_paths_when_the_parent_goes_away() {
        let dir = tempfile::tempdir().expect("temp dir");
        let victim = dir.path().join("victim");
        fs::create_dir(&victim).expect("make folder");
        fs::write(victim.join("file"), "x").expect("write");
        let (mut child, watchdog) = watched_sleeper(std::slice::from_ref(&victim));
        // The parent says nothing: the child told the watchdog its group itself. Closing the pipe
        // without a word is what the death of the parent looks like.
        drop(watchdog);
        let status = wait_for(&mut child, Duration::from_secs(10)).expect("the group was killed");
        assert_eq!(status.signal(), Some(9));
        assert!(gone(&victim), "the watchdog left {}", victim.display());
    }

    #[test]
    fn a_watchdog_removes_the_paths_when_the_run_dies_before_the_child_exists() {
        let dir = tempfile::tempdir().expect("temp dir");
        let victim = dir.path().join("victim");
        fs::create_dir(&victim).expect("make folder");
        let watchdog = Watchdog::start(std::slice::from_ref(&victim)).expect("start the watchdog");
        // No child ever announced itself: the group is unknown, the folders still go.
        drop(watchdog);
        assert!(gone(&victim), "the watchdog left {}", victim.display());
    }

    #[test]
    fn a_watchdog_that_was_finished_before_any_child_came_touches_nothing() {
        let dir = tempfile::tempdir().expect("temp dir");
        let keep = dir.path().join("keep");
        fs::create_dir(&keep).expect("make folder");
        let watchdog = Watchdog::start(std::slice::from_ref(&keep)).expect("start the watchdog");
        // A child that failed to start: the run stands the watchdog down.
        watchdog.finish();
        std::thread::sleep(Duration::from_millis(300));
        assert!(keep.is_dir(), "the folder was removed");
    }

    #[test]
    fn a_finished_watchdog_spares_the_group_and_the_paths() {
        let dir = tempfile::tempdir().expect("temp dir");
        let keep = dir.path().join("keep");
        fs::create_dir(&keep).expect("make folder");
        let (mut child, watchdog) = watched_sleeper(std::slice::from_ref(&keep));
        watchdog.finish();
        std::thread::sleep(Duration::from_millis(500));
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "the group was killed"
        );
        assert!(keep.is_dir(), "the folder was removed");
        sys::kill_group(child.id());
        let _ = child.wait();
    }

    #[test]
    fn shutdown_kills_every_group_removes_every_path_and_refuses_new_children() {
        let dir = tempfile::tempdir().expect("temp dir");
        let victim = dir.path().join("quarantine");
        fs::create_dir(&victim).expect("make folder");
        let r = Arc::new(runner());
        r.guard.add_path(&victim);
        let busy = Arc::clone(&r);
        let run = std::thread::spawn(move || {
            busy.run(
                Path::new("/bin/sleep"),
                &[os("60")],
                Path::new("/"),
                &[],
                Duration::from_secs(60),
            )
        });
        for _ in 0..200 {
            if r.guard.running() == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(r.guard.running(), 1, "the child never registered");
        r.guard.shutdown();
        assert!(!victim.exists(), "the registered folder was not removed");
        assert_eq!(r.guard.running(), 0);
        // The child is dead, and the thread that ran it sees that within a few milliseconds. It
        // must not go on, report a failure or clean up: the signal thread ends the run. (Before
        // the fix it returned `137` here, and a fetch would have exited 3 ahead of the signal
        // thread's `exit(143)`.)
        std::thread::sleep(Duration::from_millis(600));
        assert!(
            !run.is_finished(),
            "the runner returned after the signal thread killed its child"
        );
        let err = r
            .run(
                Path::new("/bin/true"),
                &[],
                Path::new("/"),
                &[],
                Duration::from_secs(5),
            )
            .expect_err("no new child after shutdown");
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn the_main_thread_defers_to_the_signal_thread_only_once_a_signal_was_caught() {
        let guard = Arc::new(Guard::default());
        // No signal: it returns at once.
        guard.defer_to_signal();
        guard.shutdown();
        let waiting = Arc::clone(&guard);
        let main = std::thread::spawn(move || waiting.defer_to_signal());
        std::thread::sleep(Duration::from_millis(400));
        assert!(!main.is_finished(), "it went on after a shutdown");
    }

    #[test]
    fn an_ended_child_stays_listed_and_signalable_until_it_is_reaped() {
        let guard = Guard::default();
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "exit 3"]);
        sys::new_session(&mut cmd);
        let (mut child, registered) = guard.spawn(&mut cmd).expect("spawn");
        let end = Instant::now() + Duration::from_secs(10);
        while !sys::exited(child.id()).expect("waitid") {
            assert!(Instant::now() < end, "the child never ended");
            std::thread::sleep(Duration::from_millis(5));
        }
        // Ended, not reaped: the pid is still the child's, so the guard may still signal its
        // group (and `try_wait` still has the status to hand over).
        assert_eq!(guard.running(), 1);
        // The reap takes it out of the guard first, so nothing signals the group afterwards.
        let status = registered.reap(&mut child).expect("reap");
        assert_eq!(status.code(), Some(3));
        assert_eq!(guard.running(), 0);
    }

    #[test]
    fn the_runner_returns_the_exit_code_of_a_child_it_reaped_late() {
        let r = runner();
        let (code, out, _) = r
            .run(
                Path::new("/bin/sh"),
                &[os("-c"), os("echo hi; exit 5")],
                Path::new("/"),
                &[],
                Duration::from_secs(10),
            )
            .expect("run");
        assert_eq!(code, 5);
        assert_eq!(out, b"hi\n");
        assert_eq!(r.guard.running(), 0, "the child is still listed");
    }

    #[test]
    fn a_path_that_is_forgotten_is_not_removed_by_shutdown() {
        let dir = tempfile::tempdir().expect("temp dir");
        let keep = dir.path().join("kept-quarantine");
        fs::create_dir(&keep).expect("make folder");
        let guard = Guard::default();
        guard.add_path(&keep);
        guard.forget_path(&keep);
        guard.shutdown();
        assert!(keep.is_dir());
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
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "cat waited for input"
        );
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
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        let beat = dir.path().join("beat");
        std::thread::sleep(Duration::from_millis(300));
        let first = fs::metadata(&beat)
            .expect("the loop ran before the deadline")
            .len();
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
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
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
        assert_eq!(
            (code, out.as_slice(), err.as_slice()),
            (7, &b"out\n"[..], &b"err\n"[..])
        );
    }

    #[test]
    fn scratch_is_private_outside_the_target_and_removed_on_drop() {
        let base = tempfile::tempdir().expect("temp dir");
        let target = tempfile::tempdir().expect("temp dir");
        let env = Env::default().with_os("TMPDIR", base.path());
        let guard = Guard::default();
        let made = {
            let s = Scratch::new(
                &env,
                &fs::canonicalize(target.path()).expect("canonical"),
                &guard,
            )
            .expect("a scratch folder");
            let mode = fs::metadata(s.path())
                .expect("scratch")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
            assert!(s.path().starts_with(base.path()));
            // An interrupt removes it too, while it is in use.
            assert_eq!(guard.registered(), [s.path().to_path_buf()]);
            s.path().to_path_buf()
        };
        assert!(!made.exists(), "the scratch folder outlived its guard");
        assert!(
            guard.registered().is_empty(),
            "a dropped scratch stays registered"
        );
    }

    #[test]
    fn scratch_refuses_a_temp_folder_inside_the_target() {
        let target = tempfile::tempdir().expect("temp dir");
        let inside = target.path().join("tmp");
        fs::create_dir(&inside).expect("make folder");
        let env = Env::default().with_os("TMPDIR", &inside);
        let guard = Guard::default();
        let err = Scratch::new(
            &env,
            &fs::canonicalize(target.path()).expect("canonical"),
            &guard,
        )
        .err()
        .expect("refused");
        assert!(err.contains("inside the target"), "{err}");
        assert_eq!(
            fs::read_dir(&inside).expect("list").count(),
            0,
            "a folder was made anyway"
        );
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
        assert_eq!(
            fs::canonicalize(&relative).ok(),
            fs::canonicalize(dir.path()).ok()
        );
        let r = RealRunner {
            path: std::env::join_paths([
                Path::new(""),
                Path::new("."),
                relative.as_path(),
                dir.path(),
            ])
            .expect("join"),
            guard: Arc::new(Guard::default()),
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
