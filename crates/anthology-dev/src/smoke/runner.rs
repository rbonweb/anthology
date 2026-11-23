//! `smoke` part `runner`: the smoke framework (EV1a): environments, subprocesses with a timeout,
//! the seams (`Deps`), `Ctx` with its budget accounting, scratch dirs, git helpers, plugin
//! writers and `run_smoke`.
//!
//! The pure helpers of smoke.py live in `helpers`; they are re-exported here so the one module
//! reads like the Python file.

use std::cell::{Cell, OnceCell, RefCell};
use std::fs;
use std::io::{Read, Write};
use std::ops::Deref;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
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

/// How often, and how long apart, a spawn is retried when the program is busy (`ETXTBSY`).
const BUSY_RETRIES: u32 = 20;
const BUSY_BACKOFF: Duration = Duration::from_millis(25);

/// `cmd.spawn()`, retried while the program file is "Text file busy". On Linux, a thread that
/// writes an executable and runs it right away can lose a race with another thread's fork: the
/// forked child still holds the write fd until its own exec, and `execve` of a file open for
/// writing fails with `ETXTBSY`. The window is short; a file that stays busy is still an error.
fn spawn_retrying(cmd: &mut Command) -> std::io::Result<std::process::Child> {
    let mut retries = 0;
    loop {
        match cmd.spawn() {
            Err(e)
                if e.kind() == std::io::ErrorKind::ExecutableFileBusy && retries < BUSY_RETRIES =>
            {
                retries += 1;
                thread::sleep(BUSY_BACKOFF);
            }
            other => return other,
        }
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
/// `Proc(-1, "", "timed out after Ns")` (only the direct child is killed, as in Python; the child keeps our process group so Ctrl-C reaches `claude`), a failure to start `Proc(-1, "", "could not run X: …")`.
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
    cmd.args(&argv[1..])
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
    let mut child = match spawn_retrying(&mut cmd) {
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
pub type StageFn = Box<dyn Fn(&Ctx) -> Result<Staged, String>>;
pub type SeedFn = Box<dyn Fn(&Path, &Path, &Path) -> Result<(), String>>;
pub type DefaultAnthologyFn = Box<dyn Fn() -> Option<PathBuf>>;

/// The plugin trees the smokes load: `plugins/<p>` with `port/staging/plugins/<p>` copied over
/// it (the staged shims; after CO2 the overlay is empty), in a scratch folder kept as long as
/// the run.
pub struct Staged {
    /// The folder holding `anthology`, `anthology-guardrails` and `anthology-superpowers`.
    pub plugins: PathBuf,
    pub keep: Option<Scratch>,
}

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
    /// The staged plugin copy (EV1b): made once per run, on first use.
    pub stage: StageFn,
    /// `anthology-dev seed --bin B --plugins-dir P --config-dir C` (RL1), in process.
    pub seed: SeedFn,
    /// The `anthology` binary when `--anthology-bin` is not given: the one beside this
    /// `anthology-dev` (one `cargo build` makes both).
    pub default_anthology: DefaultAnthologyFn,
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
            stage: Box::new(real_stage),
            seed: Box::new(real_seed),
            default_anthology: Box::new(real_default_anthology),
        }
    }
}

/// `plugins/` with the `port/staging/plugins/` overlay, copied to a scratch folder. The Python
/// `scripts/` of anthology and anthology-guardrails are left out: CO2 deletes them, and the
/// staged shims never call them, so nothing can quietly fall back to Python.
fn real_stage(ctx: &Ctx) -> Result<Staged, String> {
    let root = ctx.root()?;
    let keep = scratch()?;
    let plugins = keep.join("plugins");
    for p in PLUGINS {
        let dest = plugins.join(p);
        copy_tree(&root.join("plugins").join(p), &dest)?;
        let overlay = root.join("port/staging/plugins").join(p);
        if overlay.is_dir() {
            copy_tree(&overlay, &dest)?;
        }
        if p != "anthology-superpowers" {
            let py = dest.join("scripts");
            if py.is_dir() {
                fs::remove_dir_all(&py).map_err(|e| format!("OSError: {}: {e}", py.display()))?;
            }
        }
    }
    Ok(Staged {
        plugins,
        keep: Some(keep),
    })
}

fn real_seed(bin: &Path, plugins: &Path, config: &Path) -> Result<(), String> {
    let args = crate::seed::SeedArgs {
        bin: bin.to_path_buf(),
        plugins_dir: plugins.to_path_buf(),
        config_dir: config.to_path_buf(),
        target: None,
        repo: crate::lock::DEFAULT_REPO.to_string(),
    };
    crate::seed::seed(&args)
        .map(|_| ())
        .map_err(|e| format!("RuntimeError: anthology-dev seed failed: {e}"))
}

fn real_default_anthology() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.parent()?.join("anthology");
    is_executable(&sibling).then_some(sibling)
}

/// Copies `src` into `dest` (created), keeping modes and symlinks and replacing what is there;
/// `__pycache__` folders are left out. testkit's `copy_tree`, returning errors.
pub fn copy_tree(src: &Path, dest: &Path) -> Result<(), String> {
    let os = |p: &Path, e: std::io::Error| format!("OSError: {}: {e}", p.display());
    let meta = fs::symlink_metadata(src).map_err(|e| os(src, e))?;
    let mut stack = vec![(src.to_path_buf(), dest.to_path_buf(), meta)];
    while let Some((from, to, meta)) = stack.pop() {
        let kind = meta.file_type();
        let existing = fs::symlink_metadata(&to).ok();
        let clear = |to: &Path| -> Result<(), String> {
            match &existing {
                Some(m) if m.is_dir() => fs::remove_dir_all(to).map_err(|e| os(to, e)),
                Some(_) => fs::remove_file(to).map_err(|e| os(to, e)),
                None => Ok(()),
            }
        };
        if kind.is_symlink() {
            clear(&to)?;
            let target = fs::read_link(&from).map_err(|e| os(&from, e))?;
            std::os::unix::fs::symlink(target, &to).map_err(|e| os(&to, e))?;
        } else if kind.is_dir() {
            if existing.as_ref().is_some_and(|m| !m.is_dir()) {
                clear(&to)?;
            }
            fs::create_dir_all(&to).map_err(|e| os(&to, e))?;
            fs::set_permissions(&to, meta.permissions()).map_err(|e| os(&to, e))?;
            for entry in fs::read_dir(&from).map_err(|e| os(&from, e))? {
                let entry = entry.map_err(|e| os(&from, e))?;
                if entry.file_name() == "__pycache__" {
                    continue;
                }
                let m = fs::symlink_metadata(entry.path()).map_err(|e| os(&entry.path(), e))?;
                stack.push((entry.path(), to.join(entry.file_name()), m));
            }
        } else if kind.is_file() {
            if existing
                .as_ref()
                .is_some_and(|m| m.is_dir() || m.is_symlink())
            {
                clear(&to)?;
            }
            fs::copy(&from, &to).map_err(|e| os(&to, e))?;
            fs::set_permissions(&to, fs::Permissions::from_mode(meta.permissions().mode()))
                .map_err(|e| os(&to, e))?;
        }
    }
    Ok(())
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
/// approval, recorded in the run's config dir (`<tmp>/cfg`) by MS1's `approve` in process
/// (EV1b; smoke.py ran `memory_sync.approve` under python3).
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
    let env = env_of(&without_auth(build_env(&ctx.env, tmp, None)));
    anthology_memory::approve::approve(&repo, &env, &anthology_core::git::RealGit).map_err(
        |e| {
            format!(
                "RuntimeError: approve failed: {}",
                ctx.snippet(&format!("{}: {}", e.type_name(), e.py_str()), 160)
            )
        },
    )?;
    Ok(repo)
}

/// `map` without the model credentials: for the subprocesses that call no model.
pub fn without_auth(mut map: EnvMap) -> EnvMap {
    for k in AUTH_VARS {
        map.remove(k);
    }
    map
}

/// An `Env` holding exactly `map`.
pub fn env_of(map: &EnvMap) -> Env {
    map.iter()
        .fold(Env::default(), |env, (k, v)| env.with(k, v))
}

fn real_claude_run(ctx: &Ctx, tmp: &Path, args: &[String], opts: &ClaudeOpts) -> Proc {
    let mut argv = vec![ctx.claude.clone()];
    argv.extend(args.iter().cloned());
    let env = build_env(&ctx.env, tmp, opts.extra_env.as_ref());
    if let Err(e) = ctx.seed_run(tmp, &env, args) {
        return Proc::new(-1, "", &e);
    }
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
    // Seeded here first, so a seed failure fails the smoke without being charged as a run.
    ctx.seed_run(
        tmp,
        &build_env(&ctx.env, tmp, opts.extra_env.as_ref()),
        &args,
    )?;
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
    /// The `anthology` binary the staged plugins run (`--anthology-bin`, else
    /// `deps.default_anthology`), and whether the caller named it.
    pub anthology: Option<String>,
    pub anthology_explicit: bool,
    spent: Cell<f64>,
    staged: OnceCell<Result<Staged, String>>,
    seeded: RefCell<Vec<PathBuf>>,
}

impl Ctx {
    pub fn new(claude: &str, budget: &str, total: f64, env: Env, deps: Deps) -> Ctx {
        Ctx {
            claude: claude.to_string(),
            budget: budget.to_string(),
            total,
            env,
            deps,
            anthology: None,
            anthology_explicit: false,
            spent: Cell::new(0.0),
            staged: OnceCell::new(),
            seeded: RefCell::new(Vec::new()),
        }
    }

    /// The same `Ctx` with the `anthology` binary set.
    pub fn with_anthology(mut self, bin: Option<String>, explicit: bool) -> Ctx {
        self.anthology = bin;
        self.anthology_explicit = explicit;
        self
    }

    /// The `anthology` binary, or the error a smoke that needs it fails with.
    pub fn anthology_bin(&self) -> Result<PathBuf, String> {
        self.anthology.as_ref().map(PathBuf::from).ok_or_else(|| {
            "RuntimeError: no anthology binary; pass --anthology-bin PATH".to_string()
        })
    }

    /// The staged plugin folder (made on first use, kept for the run).
    pub fn staged_plugins(&self) -> Result<PathBuf, String> {
        match self.staged.get_or_init(|| (self.deps.stage)(self)) {
            Ok(staged) => Ok(staged.plugins.clone()),
            Err(e) => Err(e.clone()),
        }
    }

    /// Seeds the `anthology` binary into the config dir `config`: the staged plugins' locks pin
    /// it, and `<config>/anthology/bin` holds it installed plus its offline seed (once per dir).
    pub fn seed_config(&self, config: &Path) -> Result<(), String> {
        if self.seeded.borrow().iter().any(|d| d == config) {
            return Ok(());
        }
        let plugins = self.staged_plugins()?;
        let bin = self.anthology_bin()?;
        (self.deps.seed)(&bin, &plugins, config)?;
        self.seeded.borrow_mut().push(config.to_path_buf());
        Ok(())
    }

    /// Before a `claude` run whose `args` load the staged plugins: the binary goes into every
    /// config dir its hooks may use. That is `CLAUDE_CONFIG_DIR` and `$HOME/.claude`, the hooks'
    /// own fallback when the subprocess scrub strips `CLAUDE_CONFIG_DIR`. Any other run (a
    /// throwaway probe plugin, `claude plugin validate`) is left alone.
    pub fn seed_run(&self, tmp: &Path, env: &EnvMap, args: &[String]) -> Result<(), String> {
        let Some(Ok(staged)) = self.staged.get() else {
            return Ok(());
        };
        let loads_staged = args
            .iter()
            .any(|a| Path::new(a).starts_with(&staged.plugins));
        if !loads_staged || self.anthology.is_none() {
            return Ok(());
        }
        let mut dirs = vec![tmp.join("cfg"), tmp.join("home").join(".claude")];
        if let Some(cfg) = env.get("CLAUDE_CONFIG_DIR") {
            dirs.push(PathBuf::from(cfg));
        }
        if let Some(home) = env.get("HOME") {
            dirs.push(Path::new(home).join(".claude"));
        }
        for dir in dirs {
            self.seed_config(&dir)?;
        }
        Ok(())
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

    /// `plugin_flags()`: `--plugin-dir <staged>/<p>` for every plugin. The plugins are the
    /// staged copy with the `anthology` binary seeded (EV1b), not the live `plugins/` tree.
    pub fn plugin_flags(&self) -> Result<Vec<String>, String> {
        let plugins = self.staged_plugins()?;
        Ok(PLUGINS
            .iter()
            .flat_map(|p| {
                [
                    "--plugin-dir".to_string(),
                    plugins.join(p).to_string_lossy().into_owned(),
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
        Some(def) => run_smoke_inner(def, ctx, explicit),
        None => (Status::Fail, format!("KeyError: {}", py_repr_str(name))),
    };
    let reason: String = split_ws(&redact(&ctx.env, &reason))
        .join(" ")
        .chars()
        .take(400)
        .collect();
    (status, reason)
}

fn run_smoke_inner(def: &super::SmokeDef, ctx: &Ctx, explicit: bool) -> Outcome {
    if ctx.spent() >= ctx.total {
        return (Status::Fail, exhausted(ctx));
    }
    if def.needs_claude && !binary_exists(&ctx.env, &ctx.claude) {
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
    if def.needs_anthology {
        let usable = ctx
            .anthology
            .as_deref()
            .is_some_and(|b| is_executable(Path::new(b)));
        if !usable {
            // Like the claude binary: a binary the caller named must work; the default one may
            // simply not be built.
            return (
                if ctx.anthology_explicit {
                    Status::Fail
                } else {
                    Status::Skip
                },
                format!(
                    "anthology binary not found: {} (pass --anthology-bin PATH)",
                    ctx.anthology
                        .as_deref()
                        .unwrap_or("none beside anthology-dev")
                ),
            );
        }
    }
    match (def.run)(ctx) {
        Ok(outcome) => outcome,
        Err(Abort::Budget) => (Status::Fail, exhausted(ctx)),
        Err(Abort::Error(message)) => (Status::Fail, abort_reason(ctx, &message)),
    }
}
