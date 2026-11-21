//! `smoke` part `runner`: the smoke framework (EV1a): environments, subprocesses with a timeout,
//! the seams (`Deps`), `Ctx` with its budget accounting, scratch dirs, git helpers, plugin
//! writers and `run_smoke`.
//!
//! The pure helpers of smoke.py live in `helpers`; they are re-exported here so the one module
//! reads like the Python file.

use std::cell::Cell;
use std::fs;
use std::io::{Read, Write};
use std::ops::Deref;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_pycompat::json::Value;

use super::compat::{split_ws, strip_ws};
pub use super::helpers::*;

// ---- environment and processes ------------------------------------------------------------

/// The environment for a `claude` run: built from scratch, never a copy of the process
/// environment. Creates `<tmp>/home` and `<tmp>/cfg` (a failure shows when the run starts).
pub fn build_env(env: &Env, tmp: &Path, extra: Option<&EnvMap>) -> EnvMap {
    let home = tmp.join("home");
    let cfg = tmp.join("cfg");
    let _ = fs::create_dir_all(&home);
    let _ = fs::create_dir_all(&cfg);
    // `tempfile.gettempdir()`: TMPDIR, TEMP, TMP, then /tmp; never the process environment.
    let tmpdir = ["TMPDIR", "TEMP", "TMP"]
        .iter()
        .find_map(|k| env.get(k).filter(|t| !t.is_empty()))
        .unwrap_or("/tmp")
        .to_string();
    let mut out = EnvMap::new();
    let mut put = |k: &str, v: String| {
        out.insert(k.to_string(), v);
    };
    put("HOME", home.to_string_lossy().into_owned());
    put("CLAUDE_CONFIG_DIR", cfg.to_string_lossy().into_owned());
    put("PATH", env.get("PATH").unwrap_or("").to_string());
    put("TMPDIR", tmpdir);
    put("LANG", "C.UTF-8".to_string());
    put("TERM", "dumb".to_string());
    put("DISABLE_AUTOUPDATER", "1".to_string());
    // Tools and hooks never see the API key.
    put("CLAUDE_CODE_SUBPROCESS_ENV_SCRUB", "1".to_string());
    // A broken guard cannot fetch a package.
    put("NPM_CONFIG_OFFLINE", "true".to_string());
    for k in AUTH_VARS {
        if let Some(v) = env.get(k).filter(|v| !v.is_empty()) {
            out.insert(k.to_string(), v.to_string());
        }
    }
    for (k, v) in extra.into_iter().flatten() {
        out.insert(k.clone(), v.clone());
    }
    out
}

/// `type(exc).__name__` for the spawn errors Python would raise.
fn spawn_error_class(e: &std::io::Error) -> &'static str {
    match e.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    }
}

fn read_all<R: Read + Send + 'static>(
    mut pipe: R,
    is_err: bool,
    tx: mpsc::Sender<(bool, Vec<u8>)>,
) {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        let _ = tx.send((is_err, buf));
    });
}

/// `run(argv, cwd, env, timeout, stdin)`: the process runs with exactly `env`, stdin from
/// `/dev/null` (or `stdin`), and both pipes read to the end inside the timeout. A timeout is
/// `Proc(-1, "", "timed out after Ns")`, a failure to start `Proc(-1, "", "could not run X: …")`.
pub fn run_process(
    argv: &[String],
    cwd: &Path,
    env: &EnvMap,
    timeout: u64,
    stdin: Option<&str>,
) -> Proc {
    let Some(program) = argv.first() else {
        return Proc::new(-1, "", "could not run : IndexError");
    };
    let mut cmd = Command::new(program);
    // Its own process group, so a timeout can end the grandchildren too (hooks, tool processes).
    cmd.args(&argv[1..])
        .process_group(0)
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Proc::new(
                -1,
                "",
                &format!("could not run {program}: {}", spawn_error_class(&e)),
            );
        }
    };
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let bytes = text.as_bytes().to_vec();
        thread::spawn(move || {
            let _ = pipe.write_all(&bytes);
        });
    }
    let (tx, rx) = mpsc::channel();
    if let Some(p) = child.stdout.take() {
        read_all(p, false, tx.clone());
    }
    if let Some(p) = child.stderr.take() {
        read_all(p, true, tx.clone());
    }
    drop(tx);
    let deadline = Instant::now() + Duration::from_secs(timeout);
    let (mut out, mut err, mut streams) = (Vec::new(), Vec::new(), 0);
    let mut status = None;
    loop {
        if status.is_none() {
            status = child.try_wait().ok().flatten();
        }
        while let Ok((is_err, bytes)) = rx.try_recv() {
            *(if is_err { &mut err } else { &mut out }) = bytes;
            streams += 1;
        }
        if let (Some(status), 2) = (status, streams) {
            let code = status
                .code()
                .unwrap_or_else(|| -status.signal().unwrap_or(0));
            return Proc {
                code,
                out: String::from_utf8_lossy(&out).into_owned(),
                err: String::from_utf8_lossy(&err).into_owned(),
            };
        }
        if Instant::now() >= deadline {
            kill_group(child.id());
            let _ = child.kill();
            let _ = child.wait();
            return Proc::new(-1, "", &format!("timed out after {timeout}s"));
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok((is_err, bytes)) => {
                *(if is_err { &mut err } else { &mut out }) = bytes;
                streams += 1;
            }
            // Both pipes are closed: wait for the exit without spinning.
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(10)),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// SIGKILL to the process group `pgid` (the child is its leader, see `process_group(0)`), through
/// the shell's builtin `kill`: the crate forbids `unsafe`, so no direct `killpg`.
fn kill_group(pgid: u32) {
    let _ = Command::new("sh")
        .args([
            "-c",
            "kill -s KILL -- \"-$1\" 2>/dev/null",
            "sh",
            &pgid.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// ---- the seams (GC-19 `dev::smoke`) -------------------------------------------------------

/// What `claude_run` is asked besides its arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeOpts {
    pub cwd: Option<PathBuf>,
    pub extra_env: Option<EnvMap>,
    pub timeout: u64,
}

impl Default for ClaudeOpts {
    fn default() -> ClaudeOpts {
        ClaudeOpts {
            cwd: None,
            extra_env: None,
            timeout: CLAUDE_TIMEOUT,
        }
    }
}

impl ClaudeOpts {
    /// Run in `cwd`, with `timeout` seconds.
    pub fn at(cwd: &Path, timeout: u64) -> ClaudeOpts {
        ClaudeOpts {
            cwd: Some(cwd.to_path_buf()),
            extra_env: None,
            timeout,
        }
    }
}

/// What `prompt_run` is asked besides the prompt and flags.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PromptOpts {
    pub cwd: Option<PathBuf>,
    pub extra_env: Option<EnvMap>,
    /// The variadic `--allowedTools` list, put last.
    pub variadic: Vec<String>,
}

pub type RunFn = Box<dyn Fn(&[String], &Path, &EnvMap, u64, Option<&str>) -> Proc>;
pub type GitFn = Box<dyn Fn(&Ctx, &Path, &Path, &[&str]) -> Result<String, String>>;
pub type MakeRepoFn = Box<dyn Fn(&Ctx, &Path) -> Result<PathBuf, String>>;
pub type ApprovedRepoFn = Box<dyn Fn(&Ctx, &Path) -> Result<PathBuf, String>>;
pub type RootFn = Box<dyn Fn() -> Result<PathBuf, String>>;
pub type CwdFn = Box<dyn Fn() -> PathBuf>;
pub type DefaultClaudeFn = Box<dyn Fn(&Env, Option<&Path>) -> String>;
pub type ClaudeRunFn = Box<dyn Fn(&Ctx, &Path, &[String], &ClaudeOpts) -> Proc>;
pub type PromptRunFn = Box<dyn Fn(&Ctx, &Path, &str, &[String], &PromptOpts) -> Res<Proc>>;

/// Everything the smokes reach out through, one field per `mock.patch` site of
/// tests/test_smoke.py. [`Deps::default`] is production; a test replaces the field it needs.
pub struct Deps {
    /// `smoke.run` (a subprocess): `(argv, cwd, env, timeout seconds, stdin)`.
    pub run: RunFn,
    /// `smoke.git(tmp, cwd, *args)`; the error is `RuntimeError: git <sub> failed: …`.
    pub git: GitFn,
    /// `smoke.make_repo(tmp)`: a git repo `<tmp>/repo` with one commit.
    pub make_repo: MakeRepoFn,
    /// `smoke.ROOT`: the repository root.
    pub root: RootFn,
    /// `os.getcwd()`, for `os.path.abspath` of `--claude`.
    pub cwd: CwdFn,
    /// `smoke.default_claude()`, given the environment and the root when known.
    pub default_claude: DefaultClaudeFn,
    /// `smoke._approved_repo(ctx, tmp)`.
    pub approved_repo: ApprovedRepoFn,
    /// `ctx.prompt_run(...)`: the budget accounting is the default's.
    pub prompt_run: PromptRunFn,
    /// `ctx.claude_run(...)`.
    pub claude_run: ClaudeRunFn,
}

impl Default for Deps {
    fn default() -> Deps {
        Deps {
            run: Box::new(run_process),
            git: Box::new(real_git),
            make_repo: Box::new(real_make_repo),
            root: Box::new(real_root),
            cwd: Box::new(|| std::env::current_dir().unwrap_or_default()),
            default_claude: Box::new(real_default_claude),
            approved_repo: Box::new(real_approved_repo),
            prompt_run: Box::new(real_prompt_run),
            claude_run: Box::new(real_claude_run),
        }
    }
}

/// The nearest ancestor of the process's working directory that holds
/// `plugins/anthology/.claude-plugin/plugin.json` (R2).
fn real_root() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir()
        .and_then(fs::canonicalize)
        .map_err(|e| format!("OSError: cannot find the working directory: {e}"))?;
    cwd.ancestors()
        .find(|dir| {
            dir.join("plugins/anthology/.claude-plugin/plugin.json")
                .is_file()
        })
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            "RuntimeError: run `anthology-dev smoke` inside the anthology checkout: no parent of the working directory holds plugins/anthology/.claude-plugin/plugin.json".to_string()
        })
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn real_default_claude(env: &Env, root: Option<&Path>) -> String {
    if let Some(bin) = env.get("CLAUDE_BIN").filter(|b| !b.is_empty()) {
        return bin.to_string();
    }
    root.map(|r| r.join("ci/claude-code/node_modules/.bin/claude"))
        .filter(|local| is_executable(local))
        .map_or_else(
            || "claude".to_string(),
            |p| p.to_string_lossy().into_owned(),
        )
}

/// `shutil.which(cmd) or (Path(cmd).is_file() and executable)`.
pub fn binary_exists(env: &Env, claude: &str) -> bool {
    if claude.contains('/') {
        return is_executable(Path::new(claude));
    }
    env.get("PATH")
        .unwrap_or("")
        .split(':')
        .any(|dir| is_executable(&Path::new(dir).join(claude)))
}

fn git_env(ctx: &Ctx, tmp: &Path) -> EnvMap {
    let mut env = build_env(&ctx.env, tmp, None);
    for (k, v) in [
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_AUTHOR_NAME", "Smoke"),
        ("GIT_COMMITTER_NAME", "Smoke"),
        ("GIT_AUTHOR_EMAIL", "smoke@example.invalid"),
        ("GIT_COMMITTER_EMAIL", "smoke@example.invalid"),
    ] {
        env.insert(k.to_string(), v.to_string());
    }
    env
}

fn real_git(ctx: &Ctx, tmp: &Path, cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut argv: Vec<String> = [
        "git",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "init.defaultBranch=main",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.extend(args.iter().map(|s| s.to_string()));
    let p = ctx.run(&argv, cwd, &git_env(ctx, tmp), 60, None);
    if p.code != 0 {
        return Err(format!(
            "RuntimeError: git {} failed: {}",
            args.first().copied().unwrap_or(""),
            ctx.snippet(&p.err, 160)
        ));
    }
    Ok(strip_ws(&p.out).to_string())
}

fn real_make_repo(ctx: &Ctx, tmp: &Path) -> Result<PathBuf, String> {
    let repo = tmp.join("repo");
    fs::create_dir(&repo).map_err(|e| format!("OSError: {}: {e}", repo.display()))?;
    ctx.git(tmp, &repo, &["init", "-q", "-b", "main"])?;
    ctx.set_identity(tmp, &repo)?;
    write_text(&repo.join("README.md"), "smoke\n")?;
    ctx.git(tmp, &repo, &["add", "-A"])?;
    ctx.git(tmp, &repo, &["commit", "-q", "-m", "init"])?;
    Ok(repo)
}

/// The repo of the auto-memory smokes: `make_repo`, a fake `origin`, and the memory sync's
/// approval. Still the Python `memory_sync.approve` until CO2; EV1b swaps it for the Rust binary.
fn real_approved_repo(ctx: &Ctx, tmp: &Path) -> Result<PathBuf, String> {
    let repo = ctx.make_repo(tmp)?;
    ctx.git(
        tmp,
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/smoke/repo.git",
        ],
    )?;
    let code = "import sys; sys.path.insert(0, sys.argv[1]); import memory_sync; \
                from pathlib import Path; memory_sync.approve(Path(sys.argv[2]))";
    let root = ctx.root()?;
    let argv = [
        "python3".to_string(),
        "-c".to_string(),
        code.to_string(),
        root.join("plugins/anthology/scripts")
            .to_string_lossy()
            .into_owned(),
        repo.to_string_lossy().into_owned(),
    ];
    let p = ctx.run(&argv, &repo, &build_env(&ctx.env, tmp, None), 60, None);
    if p.code != 0 {
        return Err(format!(
            "RuntimeError: memory_sync.approve failed: {}",
            ctx.snippet(&p.err, 160)
        ));
    }
    Ok(repo)
}

fn real_claude_run(ctx: &Ctx, tmp: &Path, args: &[String], opts: &ClaudeOpts) -> Proc {
    let mut argv = vec![ctx.claude.clone()];
    argv.extend(args.iter().cloned());
    let env = build_env(&ctx.env, tmp, opts.extra_env.as_ref());
    let cwd = opts.cwd.as_deref().unwrap_or(tmp);
    ctx.run(&argv, cwd, &env, opts.timeout, None)
}

/// `claude -p PROMPT FLAGS --max-budget-usd B [--allowedTools ...]`; the variadic flag goes last.
///
/// Unless `flags` pick an output format, the run asks for `json` and `Proc.out` becomes the reply
/// text, so the reported cost can be added to the running total (an unreported cost counts as
/// the budget).
fn real_prompt_run(
    ctx: &Ctx,
    tmp: &Path,
    prompt: &str,
    flags: &[String],
    opts: &PromptOpts,
) -> Res<Proc> {
    if ctx.spent() >= ctx.total {
        return Err(Abort::Budget);
    }
    let own_format = flags.iter().any(|f| f == "--output-format");
    let mut args = vec![
        "-p".to_string(),
        prompt.to_string(),
        "--max-budget-usd".to_string(),
        ctx.budget.clone(),
    ];
    args.extend(flags.iter().cloned());
    if !own_format {
        args.extend(["--output-format".to_string(), "json".to_string()]);
    }
    if !opts.variadic.is_empty() {
        args.push("--allowedTools".to_string());
        args.extend(opts.variadic.iter().cloned());
    }
    let claude_opts = ClaudeOpts {
        cwd: opts.cwd.clone(),
        extra_env: opts.extra_env.clone(),
        timeout: CLAUDE_TIMEOUT,
    };
    let mut p = ctx.claude_run(tmp, &args, &claude_opts);
    let (cost, text) = result_cost_and_text(&p.out);
    let charge = match cost {
        Some(c) => c,
        None => py_float(&ctx.budget).ok_or_else(|| {
            Abort::Error(format!(
                "ValueError: could not convert string to float: {}",
                py_repr_str(&ctx.budget)
            ))
        })?,
    };
    ctx.add_spent(charge);
    if let (false, Some(text)) = (own_format, text) {
        p.out = text;
    }
    Ok(p)
}

// ---- Ctx ----------------------------------------------------------------------------------

/// One smoke run: the claude binary, the per-run and total budgets, what has been spent, the
/// environment it was started with and the seams.
pub struct Ctx {
    pub claude: String,
    /// `--max-budget-usd` for every `claude -p`, as the user wrote it.
    pub budget: String,
    pub total: f64,
    pub env: Env,
    pub deps: Deps,
    spent: Cell<f64>,
}

impl Ctx {
    pub fn new(claude: &str, budget: &str, total: f64, env: Env, deps: Deps) -> Ctx {
        Ctx {
            claude: claude.to_string(),
            budget: budget.to_string(),
            total,
            env,
            deps,
            spent: Cell::new(0.0),
        }
    }

    pub fn spent(&self) -> f64 {
        self.spent.get()
    }

    pub fn add_spent(&self, amount: f64) {
        self.spent.set(self.spent.get() + amount);
    }

    pub fn run(
        &self,
        argv: &[String],
        cwd: &Path,
        env: &EnvMap,
        timeout: u64,
        stdin: Option<&str>,
    ) -> Proc {
        (self.deps.run)(argv, cwd, env, timeout, stdin)
    }

    pub fn git(&self, tmp: &Path, cwd: &Path, args: &[&str]) -> Result<String, String> {
        (self.deps.git)(self, tmp, cwd, args)
    }

    /// Claude's own `git commit` in a scratch repo needs an identity (the HOME has no .gitconfig).
    pub fn set_identity(&self, tmp: &Path, repo: &Path) -> Result<(), String> {
        self.git(tmp, repo, &["config", "user.name", "smoke"])?;
        self.git(
            tmp,
            repo,
            &["config", "user.email", "smoke@example.invalid"],
        )?;
        Ok(())
    }

    pub fn make_repo(&self, tmp: &Path) -> Result<PathBuf, String> {
        (self.deps.make_repo)(self, tmp)
    }

    pub fn approved_repo(&self, tmp: &Path) -> Result<PathBuf, String> {
        (self.deps.approved_repo)(self, tmp)
    }

    pub fn root(&self) -> Result<PathBuf, String> {
        (self.deps.root)()
    }

    pub fn claude_run(&self, tmp: &Path, args: &[String], opts: &ClaudeOpts) -> Proc {
        (self.deps.claude_run)(self, tmp, args, opts)
    }

    pub fn prompt_run(
        &self,
        tmp: &Path,
        prompt: &str,
        flags: &[String],
        opts: &PromptOpts,
    ) -> Res<Proc> {
        (self.deps.prompt_run)(self, tmp, prompt, flags, opts)
    }

    pub fn snippet(&self, text: &str, n: usize) -> String {
        snippet(&self.env, text, n)
    }

    /// `plugin_flags()`: `--plugin-dir <root>/plugins/<p>` for every plugin.
    pub fn plugin_flags(&self) -> Result<Vec<String>, String> {
        let root = self.root()?;
        Ok(PLUGINS
            .iter()
            .flat_map(|p| {
                [
                    "--plugin-dir".to_string(),
                    root.join("plugins").join(p).to_string_lossy().into_owned(),
                ]
            })
            .collect())
    }

    /// `READ_ALLOW`: agents read their knowledge. `//` marks an absolute path in permission
    /// rules (one `/` is project-relative).
    pub fn read_allow(&self) -> Result<Value, String> {
        Ok(read_allow(&self.root()?))
    }
}

/// `READ_ALLOW` for a repository root.
pub fn read_allow(root: &Path) -> Value {
    jobj(vec![(
        "permissions",
        jobj(vec![(
            "allow",
            jarr(vec![jstr(&format!("Read(/{}/plugins/**)", root.display()))]),
        )]),
    )])
}

// ---- scratch dirs and plugin files --------------------------------------------------------

/// A scratch directory, removed on drop; derefs to its resolved path.
pub struct Scratch {
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

/// `scratch()`: a fresh `anthology-smoke-*` temp dir.
pub fn scratch() -> Result<Scratch, String> {
    let dir = tempfile::Builder::new()
        .prefix("anthology-smoke-")
        .tempdir()
        .map_err(|e| format!("OSError: cannot make a scratch dir: {e}"))?;
    let path = fs::canonicalize(dir.path()).map_err(|e| format!("OSError: {e}"))?;
    Ok(Scratch { path, _dir: dir })
}

/// `Path.write_text(text, encoding="utf-8")`.
pub fn write_text(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("OSError: {}: {e}", path.display()))
}

/// `Path.mkdir(parents=True, exist_ok=True)`.
pub fn mkdirs(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("OSError: {}: {e}", path.display()))
}

/// A throwaway plugin: `.claude-plugin/plugin.json`, `hooks/hooks.json` for `hooks`, and `files`.
pub fn write_plugin(
    root: &Path,
    name: &str,
    hooks: Option<&Value>,
    files: &[(&str, &str)],
) -> Result<PathBuf, String> {
    mkdirs(&root.join(".claude-plugin"))?;
    let manifest = jobj(vec![
        ("name", jstr(name)),
        ("version", jstr("0.0.1")),
        ("description", jstr("anthology smoke probe")),
    ]);
    write_text(&root.join(".claude-plugin/plugin.json"), &dumps(&manifest))?;
    if let Some(hooks) = hooks {
        mkdirs(&root.join("hooks"))?;
        let doc = jobj(vec![("hooks", hooks.clone())]);
        write_text(&root.join("hooks/hooks.json"), &dumps(&doc))?;
    }
    for (rel, text) in files {
        let target = root.join(rel);
        if let Some(parent) = target.parent() {
            mkdirs(parent)?;
        }
        write_text(&target, text)?;
    }
    Ok(root.to_path_buf())
}

/// `[a, b, …]` as owned strings.
pub fn sv<const N: usize>(items: [&str; N]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

// ---- running one smoke --------------------------------------------------------------------

/// A smoke body.
pub type SmokeFn = fn(&Ctx) -> Res<Outcome>;

fn exhausted(ctx: &Ctx) -> String {
    format!(
        "smoke budget exhausted (${:.2} of ${:.2}); raise --total-budget-usd",
        ctx.spent(),
        ctx.total
    )
}

/// The reason of an exception: `ClassName: message`, the message cut as `snippet` does.
fn abort_reason(ctx: &Ctx, message: &str) -> String {
    match message.split_once(": ") {
        Some((class, text)) => format!("{class}: {}", ctx.snippet(text, 160)),
        None => ctx.snippet(message, 160),
    }
}

/// `run_smoke(name, ctx, explicit)`: PASS, FAIL or SKIP with a one-line reason (at most 400
/// characters, auth values hidden). A smoke never crashes the run.
pub fn run_smoke(name: &str, ctx: &Ctx, explicit: bool) -> Outcome {
    let (status, reason) = match super::SMOKES.iter().find(|d| d.name == name) {
        Some(def) => run_smoke_inner(name, def, ctx, explicit),
        None => (Status::Fail, format!("KeyError: {}", py_repr_str(name))),
    };
    let reason: String = split_ws(&redact(&ctx.env, &reason))
        .join(" ")
        .chars()
        .take(400)
        .collect();
    (status, reason)
}

fn run_smoke_inner(name: &str, def: &super::SmokeDef, ctx: &Ctx, explicit: bool) -> Outcome {
    if ctx.spent() >= ctx.total {
        return (Status::Fail, exhausted(ctx));
    }
    if name != "python-missing" && !binary_exists(&ctx.env, &ctx.claude) {
        // A binary the caller named (flag or $CLAUDE_BIN) must exist: a silent SKIP would hide a
        // broken install.
        return (
            if explicit { Status::Fail } else { Status::Skip },
            format!("claude binary not found: {}", ctx.claude),
        );
    }
    if def.needs_model && !has_auth(&ctx.env) {
        return (Status::Skip, NO_AUTH.to_string());
    }
    match (def.run)(ctx) {
        Ok(outcome) => outcome,
        Err(Abort::Budget) => (Status::Fail, exhausted(ctx)),
        Err(Abort::Error(message)) => (Status::Fail, abort_reason(ctx, &message)),
    }
}
