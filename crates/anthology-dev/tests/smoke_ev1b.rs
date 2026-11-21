//! Rust-only tests of the EV1b smoke work (not in the ledger): streamed progress and summary,
//! the anthology-binary gate, `binary-missing` against the real staged shims, `live-seed` and
//! `--prepare-evals` with a fake binary through the real `seed`, the per-run seeding, and the
//! approval stand-in. No model, no network.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anthology_core::Env;
use anthology_core::git::RealGit;
use anthology_dev::smoke::runner::*;
use anthology_dev::smoke::{approve, cases, evals_ws, run_to, run_with};
use anthology_pycompat::json;
use anthology_testkit::ev::{FakeGuard, fake_guard_binary};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn args(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

/// The process environment without credentials or `CLAUDE_BIN`.
fn quiet_env() -> Env {
    let mut env = Env::from_process();
    for k in AUTH_VARS {
        env = env.without(k);
    }
    env.without("CLAUDE_BIN")
}

/// Production seams (the real stage copies the checkout's plugins with the staging overlay),
/// the root pinned to this checkout.
fn real_deps() -> Deps {
    let root = repo_root();
    Deps {
        root: Box::new(move || Ok(root.clone())),
        default_anthology: Box::new(|| None),
        ..Deps::default()
    }
}

fn ctx_with_bin(bin: &Path) -> Ctx {
    Ctx::new("claude", "1.00", 10.0, quiet_env(), real_deps())
        .with_anthology(Some(bin.to_string_lossy().into_owned()), true)
}

/// A progress writer that copies the `--out` file each time a line arrives.
struct Watch {
    out: PathBuf,
    seen: Rc<RefCell<Vec<(String, String)>>>,
}

impl Write for Watch {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let line = String::from_utf8_lossy(buf).into_owned();
        let summary = fs::read_to_string(&self.out).unwrap_or_default();
        self.seen.borrow_mut().push((line, summary));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn progress_lines_and_the_summary_are_written_as_each_smoke_finishes() {
    // A root without plugins: python-missing SKIPs at once, twice.
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let deps = Deps {
        root: Box::new(move || Ok(root.clone())),
        default_anthology: Box::new(|| None),
        ..Deps::default()
    };
    let out = tmp.path().join("res/smoke.json");
    let seen = Rc::new(RefCell::new(Vec::new()));
    let mut watch = Watch {
        out: out.clone(),
        seen: Rc::clone(&seen),
    };
    let argv = [
        "--out",
        out.to_str().expect("utf-8"),
        "python-missing",
        "python-missing",
    ];
    let res = run_to(&args(&argv), quiet_env(), deps, &mut watch);
    assert_eq!(res.code, 0);
    assert!(
        res.stdout.is_empty(),
        "every line went to the progress writer"
    );
    let seen = seen.borrow();
    let lines: Vec<&str> = seen.iter().map(|(l, _)| l.as_str()).collect();
    assert!(lines[0].starts_with("SKIP python-missing: "), "{lines:?}");
    assert!(
        lines[2].starts_with("smoke spend: $0.00 of $10.00"),
        "{lines:?}"
    );
    // The first line is written before the second smoke runs; the summary of the first smoke
    // is already on disk when the second line arrives.
    assert_eq!(seen[0].1, "");
    assert!(seen[1].1.contains("\"python-missing\""), "{:?}", seen[1].1);
    let doc = json::loads(&fs::read_to_string(&out).expect("summary")).expect("json");
    assert!(json::dumps(&doc, &json::PY_DEFAULT).contains("SKIP"));
}

#[test]
fn run_with_still_collects_the_lines_for_callers_that_buffer() {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let deps = Deps {
        root: Box::new(move || Ok(root.clone())),
        default_anthology: Box::new(|| None),
        ..Deps::default()
    };
    let res = run_with(&args(&["python-missing"]), quiet_env(), deps);
    let text = String::from_utf8(res.stdout).expect("utf-8");
    assert!(text.starts_with("SKIP python-missing: "), "{text}");
    assert!(text.ends_with("of $10.00\n"), "{text}");
}

#[test]
fn an_anthology_smoke_skips_without_the_binary_and_fails_when_it_was_named() {
    let res = run_with(&args(&["live-seed"]), quiet_env(), real_deps());
    let text = String::from_utf8(res.stdout).expect("utf-8");
    assert_eq!(res.code, 0, "{text}");
    assert!(
        text.starts_with("SKIP live-seed: anthology binary not found: none beside anthology-dev"),
        "{text}"
    );
    let res = run_with(
        &args(&["--anthology-bin", "/nonexistent/anthology", "live-seed"]),
        quiet_env(),
        real_deps(),
    );
    let text = String::from_utf8(res.stdout).expect("utf-8");
    assert_eq!(res.code, 1, "{text}");
    assert!(
        text.starts_with("FAIL live-seed: anthology binary not found: /nonexistent/anthology"),
        "{text}"
    );
    // A smoke that needs no claude runs although no claude binary exists.
    let res = run_with(
        &args(&["--claude", "/nonexistent/claude", "binary-missing"]),
        quiet_env(),
        real_deps(),
    );
    let text = String::from_utf8(res.stdout).expect("utf-8");
    assert!(!text.contains("claude binary not found"), "{text}");
}

#[test]
fn binary_missing_passes_against_the_staged_shims() {
    let ctx = Ctx::new("claude", "1.00", 10.0, quiet_env(), real_deps());
    let (status, reason) = cases::binary_missing::run(&ctx).expect("an outcome");
    assert_eq!(status, Status::Pass, "{reason}");
}

#[test]
fn live_seed_runs_the_staged_guard_on_the_seeded_binary() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bin = fake_guard_binary(tmp.path(), "0.1.0", FakeGuard::Real);
    let ctx = ctx_with_bin(&bin);
    let (status, reason) = cases::live_seed::run(&ctx).expect("an outcome");
    assert_eq!(status, Status::Pass, "{reason}");
    // The staged copy's locks now pin the fake binary.
    let lock = fs::read_to_string(
        ctx.staged_plugins()
            .expect("staged")
            .join("anthology-guardrails/anthology.lock"),
    )
    .expect("lock");
    assert!(lock.contains("anthology-bin--v0.1.0"), "{lock}");
}

#[test]
fn live_seed_fails_when_the_seeded_binary_does_not_answer() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bin = fake_guard_binary(tmp.path(), "0.1.0", FakeGuard::Stub);
    let ctx = ctx_with_bin(&bin);
    let (status, reason) = cases::live_seed::run(&ctx).expect("an outcome");
    assert_eq!(status, Status::Fail);
    assert!(reason.contains("was not allowed cleanly"), "{reason}");
    assert!(reason.contains("quarantined"), "{reason}");
}

#[test]
fn the_staged_copy_has_the_shims_and_no_python_scripts() {
    let ctx = Ctx::new("claude", "1.00", 10.0, quiet_env(), real_deps());
    let plugins = ctx.staged_plugins().expect("staged");
    assert!(
        plugins
            .join("anthology-guardrails/hooks/guard.sh")
            .is_file()
    );
    assert!(plugins.join("anthology/bin/anthology").is_file());
    assert!(!plugins.join("anthology/scripts").exists());
    assert!(!plugins.join("anthology-guardrails/scripts").exists());
    assert!(plugins.join("anthology-superpowers").is_dir());
    let hooks = fs::read_to_string(plugins.join("anthology-guardrails/hooks/hooks.json"))
        .expect("hooks.json");
    assert!(
        hooks.contains("guard.sh") && !hooks.contains("python"),
        "{hooks}"
    );
    // Made once per run.
    assert_eq!(ctx.staged_plugins().expect("staged"), plugins);
}

#[test]
fn each_config_dir_of_a_run_is_seeded_once() {
    let calls: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
    let log = Rc::clone(&calls);
    let tmp = tempfile::tempdir().expect("tmp");
    let plugins = tmp.path().join("plugins");
    let staged_at = plugins.clone();
    let deps = Deps {
        stage: Box::new(move |_| {
            Ok(Staged {
                plugins: staged_at.clone(),
                keep: None,
            })
        }),
        seed: Box::new(move |bin, p, config| {
            assert_eq!(bin, Path::new("/x/anthology"));
            assert!(p.ends_with("plugins"));
            log.borrow_mut().push(config.to_path_buf());
            Ok(())
        }),
        ..Deps::default()
    };
    let ctx = Ctx::new("claude", "1.00", 10.0, quiet_env(), deps);
    let run_tmp = tmp.path().join("run");
    let env = EnvMap::from([(
        "CLAUDE_CONFIG_DIR".to_string(),
        run_tmp.join("home/.claude").to_string_lossy().into_owned(),
    )]);
    // No binary, or staged plugins never used: nothing is seeded.
    ctx.seed_run(&run_tmp, &env).expect("no-op");
    let ctx = ctx.with_anthology(Some("/x/anthology".to_string()), true);
    ctx.seed_run(&run_tmp, &env).expect("no-op");
    assert!(calls.borrow().is_empty());
    ctx.plugin_flags().expect("flags");
    ctx.seed_run(&run_tmp, &env).expect("seeds");
    ctx.seed_run(&run_tmp, &env).expect("seeds once");
    assert_eq!(
        *calls.borrow(),
        [run_tmp.join("cfg"), run_tmp.join("home/.claude")]
    );
    let flags = ctx.plugin_flags().expect("flags");
    assert_eq!(flags[1], plugins.join("anthology").to_string_lossy());
}

#[test]
fn prepare_evals_builds_a_seeded_workspace_whose_session_start_installs_the_binary() {
    let tmp = tempfile::tempdir().expect("tmp");
    let bin = fake_guard_binary(&tmp.path().join("bin"), "0.1.0", FakeGuard::Real);
    let ctx = ctx_with_bin(&bin);
    let ws = tmp.path().join("ws");
    assert_eq!(evals_ws::prepare(&ctx, &ws).expect("prepared"), ws);
    assert!(ws.join(".claude-plugin/marketplace.json").is_file());
    assert!(ws.join("evals/guard-g1/case.yaml").is_file());
    assert!(!ws.join("evals/smoke").exists() && !ws.join("evals/results").exists());
    assert!(!ws.join(".seed-config").exists());
    let guardrails = ws.join("plugins/anthology-guardrails");
    assert!(
        fs::read_to_string(guardrails.join("anthology.lock"))
            .expect("lock")
            .contains("anthology-bin--v0.1.0")
    );
    let archive = format!("anthology-{}.tar.gz", anthology_testkit::ev::host_triple());
    for p in ["anthology", "anthology-guardrails"] {
        assert!(
            ws.join("plugins")
                .join(p)
                .join("eval-seed")
                .join(&archive)
                .is_file()
        );
    }
    // A second prepare into the same folder is refused.
    assert!(evals_ws::prepare(&ctx, &ws).is_err());

    // What `plugin eval` does: a fresh config dir, the SessionStart hook as hooks.json says
    // (now through eval-seed.sh), then the guard shim, offline.
    let config = tmp.path().join("eval-config");
    let home = tmp.path().join("eval-home");
    let project = tmp.path().join("project");
    for d in [&home, &project] {
        fs::create_dir_all(d).expect("mkdir");
    }
    let env = EnvMap::from([
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("HOME".to_string(), home.to_string_lossy().into_owned()),
        (
            "CLAUDE_CONFIG_DIR".to_string(),
            config.to_string_lossy().into_owned(),
        ),
        (
            "CLAUDE_PLUGIN_ROOT".to_string(),
            guardrails.to_string_lossy().into_owned(),
        ),
        (
            "TMPDIR".to_string(),
            tmp.path().to_string_lossy().into_owned(),
        ),
        ("LANG".to_string(), "C.UTF-8".to_string()),
    ]);
    let session =
        anthology_dev::smoke::staged::hook_command(&guardrails, "SessionStart", "bootstrap.sh")
            .expect("session hook");
    assert!(
        session[1].ends_with("eval-seed/eval-seed.sh"),
        "{session:?}"
    );
    let p = run_process(&session, &project, &env, 60, Some("{}"));
    assert_eq!(p.code, 0, "{p:?}");
    assert_eq!(p.out, "", "no failure message: {p:?}");
    let installed = config.join("anthology/bin/0.1.0/anthology");
    assert!(
        installed.is_file(),
        "the bootstrap installed the seed: {p:?}"
    );
    let guard = anthology_dev::smoke::staged::hook_command(&guardrails, "PreToolUse", "guard.sh")
        .expect("guard hook");
    let mut env = env;
    env.insert("ANTHOLOGY_GUARD_NO_FETCH".to_string(), "1".to_string());
    let ls = anthology_dev::smoke::staged::bash("ls", &project);
    let p = run_process(&guard, &project, &env, 60, Some(&ls));
    assert_eq!((p.code, p.err.as_str()), (0, ""), "{p:?}");
    let g1 = anthology_dev::smoke::staged::bash("npx playwright install chromium", &project);
    let p = run_process(&guard, &project, &env, 60, Some(&g1));
    assert_eq!(p.code, 2, "{p:?}");
    assert!(
        p.err.starts_with("Blocked by anthology-guardrails (G1): "),
        "{p:?}"
    );
}

#[test]
fn approve_records_the_key_and_the_union_lines() {
    let tmp = tempfile::tempdir().expect("tmp");
    let repo = tmp.path().join("repo");
    fs::create_dir(&repo).expect("mkdir");
    let env = Env::default()
        .with(
            "PATH",
            Env::from_process().get("PATH").unwrap_or("/usr/bin:/bin"),
        )
        .with("HOME", &tmp.path().to_string_lossy())
        .with(
            "CLAUDE_CONFIG_DIR",
            &tmp.path().join("cfg").to_string_lossy(),
        )
        .with("GIT_CONFIG_NOSYSTEM", "1");
    let git = |a: &[&str]| {
        let out = std::process::Command::new("git")
            .args(a)
            .current_dir(&repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", tmp.path())
            .output()
            .expect("git");
        assert!(out.status.success(), "{out:?}");
    };
    git(&["init", "-q"]);
    git(&[
        "remote",
        "add",
        "origin",
        "https://Example.invalid/Smoke/Repo.git",
    ]);
    fs::write(repo.join(".gitattributes"), "*.png binary\n").expect("write");
    let key = approve::approve(&repo, &env, &RealGit).expect("approves");
    assert_eq!(key, "example.invalid/smoke/repo");
    let text = fs::read_to_string(tmp.path().join("cfg/anthology/approved-repos.json"))
        .expect("approved-repos.json");
    let doc = json::loads(&text).expect("json");
    let entry = doc
        .as_object()
        .and_then(|o| o.get("repos"))
        .and_then(|r| r.as_object())
        .and_then(|r| r.get(&key))
        .and_then(|e| e.as_object())
        .and_then(|e| e.get("approved"))
        .and_then(|d| d.as_str())
        .expect("approved date");
    assert_eq!(entry.len(), 10);
    assert!(
        text.starts_with("{\n  \"repos\": {") && text.ends_with("\"version\": 1\n}\n"),
        "{text}"
    );
    let attrs = fs::read_to_string(repo.join(".gitattributes")).expect("attrs");
    assert_eq!(
        attrs,
        "*.png binary\n.claude/memory/MEMORY.md merge=union\n.claude/agent-memory/*/MEMORY.md merge=union\n"
    );
    // Again: nothing changes.
    approve::approve(&repo, &env, &RealGit).expect("approves");
    assert_eq!(
        fs::read_to_string(repo.join(".gitattributes")).expect("attrs"),
        attrs
    );
}
