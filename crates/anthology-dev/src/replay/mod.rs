//! `anthology-dev replay`: replay of local transcripts (design §13.7, plan RP1). Owner: RP1.
//!
//! ```text
//! anthology-dev replay collect [--projects DIR] [--dir DIR]
//! anthology-dev replay run [--runners 1,2,3,4] [--surfaces analyze,check,privacy] [--bin FILE]
//!                          [--python PROG] [--repo DIR] [--dir DIR] [--jobs N]
//!                          [--rust-commit SHA] [--date YYYY-MM-DD]
//! anthology-dev replay report [--dir DIR] [--date YYYY-MM-DD] [--repo DIR] [--out FILE]
//! ```
//!
//! - `collect` builds the local corpus ([`collect`]) in `--dir` (default
//!   `~/.claude/anthology/replay/`) from `--projects` (default `<config>/projects`).
//! - `run` replays every distinct call through the runners ([`runners`]) and compares Python
//!   with Rust ([`compare`]). It writes, in the replay folder only: `run-<date>.json` (the
//!   counts and ids a report is made from), `replay-corpus-details-<date>.jsonl` (what each side
//!   said for every difference, for the owner's scrubbing) and `runner4-<date>.tsv` (runner 4's
//!   allowed and blocked counts per `tool_name`, m2-7). `--bin` is the `anthology` binary under
//!   test (runners 2 and 3); `--repo` the checkout whose Python is the authority (default: the
//!   checkout around the working folder); `--rust-commit` the commit the binary was built from
//!   (default: the checkout's `HEAD`).
//! - `report` writes `port/replay-reports/<date>.md` (or `--out`) from a run file, after the
//!   corpus-string test passes on the corpus ([`report`]).
//!
//! Exit codes: 0 done (and, for `run`, no unexplained difference); 1 an unexplained difference,
//! or a report that fails the corpus-string test; 2 usage or setup error.

pub mod collect;
pub mod compare;
pub mod envelope;
pub mod hmac;
pub mod report;
pub mod runners;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use anthology_core::git::{Git, RealGit};
use anthology_core::{Env, clock, paths};
use anthology_pycompat::json::{self, Map, Value};

use crate::Output;
use compare::{Oracle, Outcome, PySide};
use envelope::Call;
use report::{DiffId, Summary};
use runners::{RunOut, Runner, Setup};

const USAGE: &str = "usage: anthology-dev replay collect [--projects DIR] [--dir DIR]
       anthology-dev replay run [--runners 1,2,3,4] [--surfaces analyze,check,privacy] [--bin FILE]
                                [--python PROG] [--repo DIR] [--dir DIR] [--jobs N]
                                [--rust-commit SHA] [--date YYYY-MM-DD]
       anthology-dev replay report [--dir DIR] [--date YYYY-MM-DD] [--repo DIR] [--out FILE]
";

/// Runs `anthology-dev replay …`; `args` are the arguments after `replay`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, &Env::from_process(), &cwd)
}

/// `replay …` with `env` as the environment, in `cwd`.
pub fn run_in(args: &[OsString], env: &Env, cwd: &Path) -> Output {
    let Some(sub) = args.first().and_then(|a| a.to_str()) else {
        return usage("replay needs one of: collect, run, report");
    };
    let opts = match parse_opts(&args[1..]) {
        Ok(o) => o,
        Err(e) => return usage(&e),
    };
    let result = match sub {
        "collect" => cmd_collect(&opts, env, cwd),
        "run" => cmd_run(&opts, env, cwd),
        "report" => cmd_report(&opts, env, cwd),
        "-h" | "--help" => Ok(Output {
            stdout: USAGE.as_bytes().to_vec(),
            ..Output::default()
        }),
        other => return usage(&format!("unknown command: replay {other}")),
    };
    result.unwrap_or_else(|e| Output {
        stderr: format!("anthology-dev replay {sub}: {e}\n").into_bytes(),
        code: 2,
        ..Output::default()
    })
}

fn usage(message: &str) -> Output {
    Output {
        stderr: format!("anthology-dev: {message}\n{USAGE}").into_bytes(),
        code: 2,
        ..Output::default()
    }
}

/// `--name value` options.
#[derive(Default)]
struct Opts(BTreeMap<String, OsString>);

const OPTIONS: [&str; 11] = [
    "--projects",
    "--dir",
    "--runners",
    "--surfaces",
    "--bin",
    "--python",
    "--repo",
    "--jobs",
    "--rust-commit",
    "--date",
    "--out",
];

/// Options without a value.
const FLAGS: [&str; 0] = [];

fn parse_opts(args: &[OsString]) -> Result<Opts, String> {
    let mut opts = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let a = a.to_str().ok_or("arguments must be UTF-8 option names")?;
        if FLAGS.contains(&a) {
            if opts.0.insert(a.to_string(), OsString::new()).is_some() {
                return Err(format!("{a} given twice"));
            }
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n, Some(OsString::from(v))),
            None => (a, None),
        };
        if !OPTIONS.contains(&name) {
            return Err(format!("unknown option {a}"));
        }
        let value = match inline {
            Some(v) => v,
            None => it
                .next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))?,
        };
        if opts.0.insert(name.to_string(), value).is_some() {
            return Err(format!("{name} given twice"));
        }
    }
    Ok(opts)
}

impl Opts {
    fn path(&self, name: &str) -> Option<PathBuf> {
        self.0.get(name).map(PathBuf::from)
    }

    fn str(&self, name: &str) -> Result<Option<&str>, String> {
        self.0
            .get(name)
            .map(|v| v.to_str().ok_or_else(|| format!("{name} must be UTF-8")))
            .transpose()
    }

    fn only(&self, allowed: &[&str]) -> Result<(), String> {
        match self.0.keys().find(|k| !allowed.contains(&k.as_str())) {
            Some(k) => Err(format!("{k} does not apply here")),
            None => Ok(()),
        }
    }

    fn dir(&self, env: &Env, cwd: &Path) -> PathBuf {
        let dir = self
            .path("--dir")
            .unwrap_or_else(|| collect::default_dir(env));
        if dir.is_absolute() {
            dir
        } else {
            cwd.join(dir)
        }
    }

    fn date(&self) -> Result<String, String> {
        match self.str("--date")? {
            Some(d) if is_date(d) => Ok(d.to_string()),
            Some(d) => Err(format!("--date {d:?} is not YYYY-MM-DD")),
            None => Ok(clock::today_local().iso()),
        }
    }
}

fn is_date(d: &str) -> bool {
    d.len() == 10
        && d.bytes().enumerate().all(|(i, c)| match i {
            4 | 7 => c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// The checkout around `cwd` (the folder with the Python guard and the oracle driver).
fn find_repo(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|d| {
            d.join(runners::GUARD_PY).is_file()
                && d.join("port/difftest/oracle/_driver.py").is_file()
        })
        .map(Path::to_path_buf)
}

fn repo_of(opts: &Opts, cwd: &Path) -> Result<PathBuf, String> {
    match opts.path("--repo") {
        Some(r) => {
            let r = if r.is_absolute() { r } else { cwd.join(r) };
            find_repo(&r)
                .filter(|found| found == &r)
                .ok_or_else(|| format!("--repo {} is not an anthology checkout", r.display()))
        }
        None => find_repo(cwd)
            .ok_or_else(|| "not inside an anthology checkout; pass --repo".to_string()),
    }
}

fn home_of(env: &Env) -> String {
    paths::home_raw(env).to_string_lossy().into_owned()
}

// --- collect --------------------------------------------------------------------------------

fn cmd_collect(opts: &Opts, env: &Env, cwd: &Path) -> Result<Output, String> {
    opts.only(&["--projects", "--dir"])?;
    let projects = opts
        .path("--projects")
        .unwrap_or_else(|| collect::default_projects(env));
    let dir = opts.dir(env, cwd);
    if !projects.is_dir() {
        return Err(format!("no transcripts folder at {}", projects.display()));
    }
    let c = collect::collect(&projects, &dir, cwd)?;
    let stdout = format!(
        "replay collect: {} transcripts, {} lines ({} skipped), {} tool calls ({} distinct), \
         {} texts ({} distinct)\nreplay folder: {}\n",
        c.files,
        c.lines,
        c.skipped,
        c.calls_total,
        c.calls_distinct,
        c.texts_total,
        c.texts_distinct,
        dir.display()
    );
    Ok(Output {
        stdout: stdout.into_bytes(),
        ..Output::default()
    })
}

// --- run ------------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Surfaces {
    analyze: bool,
    check: bool,
    privacy: bool,
}

fn parse_surfaces(s: &str) -> Result<Surfaces, String> {
    let mut out = Surfaces::default();
    for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part {
            "analyze" => out.analyze = true,
            "check" => out.check = true,
            "privacy" => out.privacy = true,
            other => {
                return Err(format!(
                    "unknown surface {other:?} (analyze, check, privacy)"
                ));
            }
        }
    }
    Ok(out)
}

/// One replayed call's results.
struct CallDone {
    id: String,
    bucket: usize,
    outs: Vec<(Runner, RunOut)>,
    /// Kind, outcome, Python's side, Rust's side, and whether Rust is the stricter side (D-8).
    kinds: Vec<(&'static str, Outcome, String, String, bool)>,
}

/// One text's privacy result.
struct TextDone {
    id: String,
    outcome: Outcome,
    python: String,
    rust: String,
    stricter: bool,
}

fn py_text(p: &PySide) -> String {
    match p {
        PySide::Value(v) => json::dumps(v, &json::PY_DEFAULT_UNICODE),
        PySide::Raised(t) => format!("raised {t}"),
        PySide::TimedOut => "timed out".into(),
        PySide::Error(e) => format!("oracle error: {e}"),
    }
}

struct Ctx<'a> {
    setup: &'a Setup,
    runners: &'a [Runner],
    surfaces: Surfaces,
    key: &'a hmac::Key,
    home: &'a str,
    hostname: &'a str,
}

fn replay_call(
    ctx: &Ctx<'_>,
    slot: &mut runners::Slot,
    oracle: &mut Oracle,
    call: &Call,
) -> CallDone {
    let envelope = call.envelope();
    let id = ctx.key.case_id(call.canonical().as_bytes());
    let outs: Vec<(Runner, RunOut)> = ctx
        .runners
        .iter()
        .map(|r| (*r, ctx.setup.run(slot, *r, &envelope)))
        .collect();
    let mut kinds = Vec::new();
    let mut py_raised = false;
    let hex_env = Value::Str(hmac::hex(&envelope));
    let home = Value::Str(ctx.home.to_string());
    // The in-process verdict also corroborates a D-7 limit block from the hook.
    let rust_verdict = compare::rust_check(&envelope, ctx.home);
    if ctx.surfaces.check {
        let req = compare::case(
            "check",
            &[("envelope_hex", hex_env.clone()), ("home", home.clone())],
        );
        let (py, _) = oracle.ask(req, "verdict");
        py_raised = matches!(py, PySide::Raised(_));
        kinds.push((
            "check",
            compare::classify_check(&py, &rust_verdict),
            py_text(&py),
            rust_verdict.clone(),
            compare::stricter_check(&py, &rust_verdict),
        ));
    }
    let compared: Vec<(Runner, RunOut)> = outs
        .iter()
        .filter(|(r, _)| *r != Runner::Missing)
        .cloned()
        .collect();
    if compared.len() >= 2 {
        let outcome = compare::classify_runners(&compared, py_raised, rust_verdict == "incomplete");
        let show = |o: &RunOut| {
            format!(
                "exit {} ({} ms) stdout {:?} stderr {}",
                o.code,
                o.elapsed.as_millis(),
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        };
        let py = compared.first().map(|(_, o)| show(o)).unwrap_or_default();
        let rust = compared[1..]
            .iter()
            .map(|(r, o)| format!("runner {}: {}", r.number(), show(o)))
            .collect::<Vec<_>>()
            .join(" | ");
        let stricter = compare::stricter_runners(&compared);
        kinds.insert(0, ("runners", outcome, py, rust, stricter));
    }
    if ctx.surfaces.analyze
        && let Some(command) = compare::bash_command(&envelope)
    {
        let req = compare::case("analyze", &[("envelope_hex", hex_env), ("home", home)]);
        let (py, d6) = oracle.ask(req, "analysis");
        let rust = compare::rust_analyze(&command, ctx.home);
        let outcome = match &py {
            PySide::Value(Value::Null) => Outcome::Diff,
            _ => compare::classify_analyze(&py, d6, &rust),
        };
        let rust_text = json::dumps(&rust, &json::COMPACT_SORTED);
        // Both verdict comparisons ran on this call and were equal.
        let same = |kind: &str| {
            kinds
                .iter()
                .any(|(k, o, ..)| *k == kind && *o == Outcome::Same)
        };
        let same_verdicts = same("runners") && same("check");
        let stricter = compare::stricter_analyze(&py, &rust, &rust_verdict, same_verdicts);
        kinds.push(("analyze", outcome, py_text(&py), rust_text, stricter));
    }
    CallDone {
        id,
        bucket: report::bucket(&call.tool_name),
        outs,
        kinds,
    }
}

fn replay_text(ctx: &Ctx<'_>, oracle: &mut Oracle, text: &str) -> TextDone {
    let mut msg = b"text\0".to_vec();
    msg.extend_from_slice(text.as_bytes());
    let id = ctx.key.case_id(&msg);
    let req = compare::case(
        "privacy",
        &[
            ("text", Value::Str(text.to_string())),
            ("home", Value::Str(ctx.home.to_string())),
            ("hostname", Value::Str(ctx.hostname.to_string())),
        ],
    );
    let (py, _) = oracle.ask(req, "findings");
    let rust = compare::rust_privacy(text, ctx.home, ctx.hostname);
    let rust_text = match &rust {
        Some(f) => format!("{f:?}"),
        None => "panic".into(),
    };
    TextDone {
        id,
        outcome: compare::classify_privacy(&py, &rust),
        stricter: compare::stricter_privacy(&py, &rust),
        python: py_text(&py),
        rust: rust_text,
    }
}

/// Progress lines on stderr every [`Progress::EVERY`] items (counts only). They go straight to
/// the terminal: a long local run must show it is alive, and `Output` is written at the end.
#[derive(Default)]
struct Progress(AtomicUsize);

impl Progress {
    const EVERY: usize = 1000;

    fn tick(&self, what: &str, total: usize) {
        let n = self.0.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(Self::EVERY) {
            let _ = writeln!(
                std::io::stderr(),
                "anthology-dev replay run: {n}/{total} {what}"
            );
        }
    }
}

/// Runs `n` items on `jobs` threads; `make(worker)` gives each thread its own `work(item)`. The
/// results come back in item order.
fn parallel<T, W, F>(jobs: usize, n: usize, make: F) -> Result<Vec<T>, String>
where
    T: Send,
    W: FnMut(usize) -> T,
    F: Fn(usize) -> Result<W, String> + Sync,
{
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<T>>> = Mutex::new((0..n).map(|_| None).collect());
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for w in 0..jobs.max(1) {
            let (next, results, errors, make) = (&next, &results, &errors, &make);
            scope.spawn(move || {
                let mut work = match make(w) {
                    Ok(f) => f,
                    Err(e) => {
                        errors.lock().unwrap_or_else(|p| p.into_inner()).push(e);
                        return;
                    }
                };
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let r = work(i);
                    results.lock().unwrap_or_else(|p| p.into_inner())[i] = Some(r);
                }
            });
        }
    });
    if let Some(e) = errors.into_inner().unwrap_or_default().into_iter().next() {
        return Err(e);
    }
    results
        .into_inner()
        .unwrap_or_default()
        .into_iter()
        .map(|r| r.ok_or_else(|| "a replay worker stopped early".to_string()))
        .collect()
}

/// `prog` as an absolute path: itself when it names a folder, else the first executable
/// `<dir>/<prog>` on `path`. One resolution serves runner 1 and the oracle (GC-16: the
/// authority is the Python the run names).
fn resolve_program(prog: &Path, path: Option<&std::ffi::OsStr>) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let found = if prog.components().count() > 1 {
        Some(prog.to_path_buf())
    } else {
        path.into_iter()
            .flat_map(std::env::split_paths)
            .map(|d| d.join(prog))
            .find(|p| {
                std::fs::metadata(p)
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    };
    found
        .and_then(|p| std::fs::canonicalize(p).ok())
        .ok_or_else(|| format!("cannot find {} on PATH", prog.display()))
}

/// The Python version (`3.12.1`), or an error when it is not a working Python 3.9+.
fn python_version(python: &Path) -> Result<String, String> {
    let out = runners::exec(
        python,
        &[
            "-c".as_ref(),
            "import sys; print('%d.%d.%d' % sys.version_info[:3]); \
             sys.exit(0 if sys.version_info >= (3, 9) else 1)"
                .as_ref(),
        ],
        b"",
        &Env::default(),
        Path::new("/"),
    );
    if out.code == 0 {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!("{} is not a working Python 3.9+", python.display()))
    }
}

fn head_commit(env: &Env, repo: &Path) -> String {
    RealGit
        .run(env, repo, &["rev-parse", "HEAD"])
        .filter(|o| o.code == 0)
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// The runners' base environment: the real `HOME`, `PATH` and locale (design §13.7).
fn base_env(env: &Env, home: &str) -> Env {
    let mut base = Env::default().with("HOME", home);
    for k in ["PATH", "LANG", "LC_ALL", "LC_CTYPE", "USER", "LOGNAME"] {
        if let Some(v) = env.get_os(k) {
            base = base.with_os(k, v);
        }
    }
    base
}

fn cmd_run(opts: &Opts, env: &Env, cwd: &Path) -> Result<Output, String> {
    opts.only(&[
        "--runners",
        "--surfaces",
        "--bin",
        "--python",
        "--repo",
        "--dir",
        "--jobs",
        "--rust-commit",
        "--date",
    ])?;
    let runners = Runner::parse_list(opts.str("--runners")?.unwrap_or("1,2,3,4"))?;
    let surfaces = parse_surfaces(opts.str("--surfaces")?.unwrap_or("analyze,check,privacy"))?;
    let jobs = match opts.str("--jobs")? {
        Some(j) => j
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=256).contains(n))
            .ok_or_else(|| format!("--jobs {j:?} is not a number from 1 to 256"))?,
        None => std::thread::available_parallelism().map_or(4, |n| n.get().min(8)),
    };
    let date = opts.date()?;
    let repo = repo_of(opts, cwd)?;
    let dir = opts.dir(env, cwd);
    collect::refuse_in_checkout(&dir, cwd)?;
    let key = hmac::load(&dir)?;
    let manifest_sha256 = collect::manifest_sha256(&dir)?;
    let calls = collect::read_calls(&dir)?;
    let texts = if surfaces.privacy {
        collect::read_texts(&dir)?
    } else {
        Vec::new()
    };
    let explained = compare::load_explained(&dir)?;
    let needs_python =
        runners.contains(&Runner::Python) || surfaces.analyze || surfaces.check || surfaces.privacy;
    let python_arg = opts
        .path("--python")
        .unwrap_or_else(|| PathBuf::from("python3"));
    let (python, python_version) = if needs_python {
        let p = resolve_program(&python_arg, env.get_os("PATH"))?;
        let v = python_version(&p)?;
        (p, v)
    } else {
        (python_arg, String::new())
    };
    let path_var = env
        .get_os("PATH")
        .map_or_else(|| OsString::from("/usr/bin:/bin"), OsString::from);
    let home = home_of(env);
    let hostname = anthology_core::sys::hostname()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default();
    let setup = Setup::new(
        &repo,
        &python,
        opts.path("--bin").as_deref(),
        base_env(env, &home),
        &runners,
    )?;
    let ctx = Ctx {
        setup: &setup,
        runners: &runners,
        surfaces,
        key: &key,
        home: &home,
        hostname: &hostname,
    };
    let quarantined = AtomicUsize::new(0);
    let progress = Progress::default();
    let progress_texts = Progress::default();
    let done_calls = parallel(jobs, calls.len(), |w| {
        let mut slot = setup.slot(w)?;
        let mut oracle = Oracle::new(&python, &repo, path_var.clone());
        let (ctx, calls, quarantined, progress) = (&ctx, &calls, &quarantined, &progress);
        Ok(move |i: usize| {
            progress.tick("calls", calls.len());
            let before = slot.quarantined;
            let done = replay_call(ctx, &mut slot, &mut oracle, &calls[i]);
            quarantined.fetch_add((slot.quarantined - before) as usize, Ordering::Relaxed);
            done
        })
    })?;
    let done_texts = parallel(jobs, texts.len(), |_| {
        let mut oracle = Oracle::new(&python, &repo, path_var.clone());
        let (ctx, texts, progress) = (&ctx, &texts, &progress_texts);
        Ok(move |i: usize| {
            progress.tick("texts", texts.len());
            replay_text(ctx, &mut oracle, &texts[i].1)
        })
    })?;

    let (rust_commit, rust_commit_from) = match opts.str("--rust-commit")? {
        Some(c) => (c.to_string(), "--rust-commit"),
        None => (
            head_commit(env, &repo),
            "the checkout's HEAD; pass --rust-commit for a binary built elsewhere",
        ),
    };
    // Aggregate, in corpus order.
    let mut s = Summary {
        date: date.clone(),
        python_commit: head_commit(env, &repo),
        rust_commit: rust_commit.clone(),
        manifest_sha256,
        calls_distinct: calls.len() as u64,
        calls_total: calls.iter().map(|c| c.count).sum(),
        texts_distinct: texts.len() as u64,
        texts_total: texts.iter().map(|(n, _)| n).sum(),
        ..Summary::default()
    };
    let mut details = String::new();
    let mut runner4: BTreeMap<&str, [u64; 6]> = BTreeMap::new();
    let mut note = |id: &str,
                    kind: &'static str,
                    outcome: Outcome,
                    stricter: bool,
                    python: &str,
                    rust: &str,
                    tool: Option<&Call>,
                    s: &mut Summary| {
        let e = s.compared.entry(kind.to_string()).or_default();
        e.0 += 1;
        let entry = explained.get(&(id.to_string(), kind.to_string())).copied();
        let outcome = compare::apply_explained(outcome, entry, stricter);
        let class = match outcome {
            Outcome::Same => {
                e.1 += 1;
                return;
            }
            Outcome::Explained(c) => Some(c.to_string()),
            Outcome::Diff | Outcome::Never => None,
        };
        s.diffs.push(DiffId {
            id: id.to_string(),
            kind: kind.to_string(),
            class: class.clone(),
        });
        let mut m = Map::new();
        m.insert("id".into(), Value::Str(id.into()));
        m.insert("kind".into(), Value::Str(kind.into()));
        m.insert("class".into(), class.map_or(Value::Null, Value::Str));
        m.insert(
            "never_explainable".into(),
            Value::Bool(outcome == Outcome::Never),
        );
        if let Some(c) = tool {
            m.insert("tool_name".into(), Value::Str(c.tool_name.clone()));
            m.insert(
                "envelope".into(),
                Value::Str(String::from_utf8_lossy(&c.envelope()).into_owned()),
            );
        }
        m.insert("python".into(), Value::Str(python.into()));
        m.insert("rust".into(), Value::Str(rust.into()));
        details.push_str(&json::js_stringify(&Value::Object(m)));
        details.push('\n');
    };
    for (call, done) in calls.iter().zip(&done_calls) {
        s.bucket_distinct[done.bucket] += 1;
        s.bucket_total[done.bucket] += call.count;
        for (r, o) in &done.outs {
            let col = if o.allowed() {
                0
            } else if o.blocked() {
                1
            } else {
                2
            };
            s.runners.entry(r.number()).or_default()[done.bucket][col] += 1;
            if *r == Runner::Missing {
                let row = runner4.entry(call.tool_name.as_str()).or_insert([
                    done.bucket as u64,
                    0,
                    0,
                    0,
                    0,
                    0,
                ]);
                row[1] += 1;
                row[2] += call.count;
                row[3 + col] += 1;
            }
        }
        for (kind, outcome, py, rust, stricter) in &done.kinds {
            note(
                &done.id,
                kind,
                outcome.clone(),
                *stricter,
                py,
                rust,
                Some(call),
                &mut s,
            );
        }
    }
    for t in &done_texts {
        note(
            &t.id,
            "privacy",
            t.outcome.clone(),
            t.stricter,
            &t.python,
            &t.rust,
            None,
            &mut s,
        );
    }

    let run_file = dir.join(format!("run-{date}.json"));
    write_private(&run_file, s.to_json().as_bytes())?;
    write_private(
        &dir.join(format!("replay-corpus-details-{date}.jsonl")),
        details.as_bytes(),
    )?;
    if runners.contains(&Runner::Missing) {
        let mut tsv = String::from("tool_name\tbucket\tdistinct\ttotal\tallowed\tblocked\tother\n");
        for (name, row) in &runner4 {
            let _ = writeln!(
                tsv,
                "{name}\t{}\t{}\t{}\t{}\t{}\t{}",
                report::BUCKETS[row[0] as usize],
                row[1],
                row[2],
                row[3],
                row[4],
                row[5]
            );
        }
        write_private(&dir.join(format!("runner4-{date}.tsv")), tsv.as_bytes())?;
    }

    let mut stdout = format!(
        "replay run: {} distinct calls, {} distinct texts\n",
        s.calls_distinct, s.texts_distinct
    );
    for kind in compare::KINDS {
        if let Some((n, same)) = s.compared.get(kind) {
            let unexplained = s
                .diffs
                .iter()
                .filter(|d| d.kind == kind && d.class.is_none())
                .count();
            let _ = writeln!(
                stdout,
                "{kind}: {n} compared, {same} same, {} explained, {unexplained} unexplained",
                n - same - unexplained as u64
            );
        }
    }
    if needs_python {
        let _ = writeln!(stdout, "python: {} {python_version}", python.display());
    }
    let _ = writeln!(stdout, "rust commit: {rust_commit} ({rust_commit_from})");
    let _ = writeln!(stdout, "run file: {}", run_file.display());
    let mut stderr = String::new();
    if needs_python && !python_version.starts_with("3.12.") {
        let _ = writeln!(
            stderr,
            "anthology-dev replay run: Python {python_version} is not 3.12, the authority (GC-16); \
             pass --python"
        );
    }
    if runners.contains(&Runner::Hook) != runners.contains(&Runner::Shim) {
        stderr.push_str(
            "anthology-dev replay run: only one of runners 2 and 3 ran, so the shim's \
             transparency was not checked\n",
        );
    }
    let n = quarantined.into_inner();
    if n > 0 {
        let _ = writeln!(
            stderr,
            "anthology-dev replay run: runner 3 quarantined its seeded binary {n} times"
        );
    }
    let unexplained = s.unexplained();
    if unexplained > 0 {
        let _ = writeln!(
            stderr,
            "anthology-dev replay run: {unexplained} unexplained differences (details: {})",
            dir.join(format!("replay-corpus-details-{date}.jsonl"))
                .display()
        );
    }
    Ok(Output {
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
        code: i32::from(unexplained > 0),
    })
}

/// Writes `data` to `path`, mode 0600, through a temporary file in the same folder whose name
/// `.gitignore` covers too (`replay-corpus*`, `runner4-*.tsv`).
fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (prefix, suffix) = if name.starts_with("runner4-") {
        ("runner4-tmp-", ".tsv")
    } else {
        ("replay-corpus-tmp-", ".tmp")
    };
    anthology_core::fsx::atomic_write(path, data, 0o600, prefix, suffix)
        .map_err(|e| format!("cannot write {}: {}", path.display(), e.py_str()))
}

// --- report ---------------------------------------------------------------------------------

/// The newest `run-<date>.json` in `dir`.
fn latest_run(dir: &Path) -> Option<String> {
    let mut dates: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let date = name.strip_prefix("run-")?.strip_suffix(".json")?;
            is_date(date).then(|| date.to_string())
        })
        .collect();
    dates.sort();
    dates.pop()
}

/// `HOME`, its path parts, the user and host names: strings the report must never hold even
/// when no transcript line names them.
fn identity_strings(env: &Env) -> Vec<String> {
    let home = home_of(env);
    let mut out: Vec<String> = home.split('/').map(str::to_string).collect();
    out.push(home);
    for k in ["USER", "LOGNAME"] {
        if let Some(v) = env.get(k) {
            out.push(v.to_string());
        }
    }
    if let Some(h) = anthology_core::sys::hostname() {
        let h = h.to_string_lossy().into_owned();
        out.extend(h.split('.').map(str::to_string));
        out.push(h);
    }
    out
}

fn cmd_report(opts: &Opts, env: &Env, cwd: &Path) -> Result<Output, String> {
    opts.only(&["--dir", "--date", "--repo", "--out"])?;
    let dir = opts.dir(env, cwd);
    collect::refuse_in_checkout(&dir, cwd)?;
    let date = match opts.str("--date")? {
        Some(_) => opts.date()?,
        None => latest_run(&dir).ok_or_else(|| {
            format!(
                "no run file in {}; run `anthology-dev replay run` first",
                dir.display()
            )
        })?,
    };
    let run_file = dir.join(format!("run-{date}.json"));
    let text = std::fs::read_to_string(&run_file)
        .map_err(|e| format!("cannot read {}: {e}", run_file.display()))?;
    let summary = Summary::from_json(&text)
        .ok_or_else(|| format!("{} is not a run file", run_file.display()))?;
    let out = match opts.path("--out") {
        Some(o) if o.is_absolute() => o,
        Some(o) => cwd.join(o),
        None => repo_of(opts, cwd)?.join(format!("port/replay-reports/{date}.md")),
    };
    let rendered = report::render(&summary);
    let strings = report::corpus_strings(&dir, identity_strings(env))?;
    let check = report::check_report(&rendered, &mut strings.into_iter());
    if !check.ok() {
        let mut msg = format!(
            "the report failed the corpus-string test ({} unknown words, {} corpus strings, {} \
             long fragments); nothing was written:",
            check.unknown.len(),
            check.hits.len(),
            check.long_fragments
        );
        for w in check.unknown.iter().chain(&check.hits).take(10) {
            let shown: String = w.chars().take(40).collect();
            let _ = write!(msg, "\n  {shown:?}");
        }
        return Ok(Output {
            stderr: format!("anthology-dev replay report: {msg}\n").into_bytes(),
            code: 1,
            ..Output::default()
        });
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut f =
        std::fs::File::create(&out).map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    f.write_all(rendered.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", out.display()))?;
    Ok(Output {
        stdout: format!(
            "replay report: {} ({} differences, {} unexplained)\n",
            out.display(),
            summary.diffs.len(),
            summary.unexplained()
        )
        .into_bytes(),
        ..Output::default()
    })
}
