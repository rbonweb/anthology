//! `anthology atl-install` (GC-5, GC-9). Owner: RT2.
//!
//! `anthology atl-install --data-dir PATH [--lock PATH] [--dry-run] [--timeout S]`, as
//! `atl_install.py main()`: exit 0 with `installed atl <ver> at <path>` or
//! `already installed: atl <ver> at <path>` (or the four dry-run lines); exit 1 with
//! `atl_install: <warning>`; exit 2 with `atl_install: <msg>` for a bad lock or platform, and
//! for usage errors (argparse's usage text is D-4).
//!
//! The default `--lock` is `$CLAUDE_PLUGIN_ROOT/atl.lock`: the binary no longer sits next to
//! the plugin's `scripts/` (lead ruling R9).

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;

use anthology_core::Env;
use anthology_core::platform;
use anthology_runtime::atl_install::{self, AtlDeps, AtlLimits};
use anthology_runtime::fetch::Fetch;

use super::Output;

/// What the command reads from the outside world (the CLI's seams).
pub struct Context<'a> {
    pub env: Env,
    pub fetch: &'a dyn Fetch,
    pub uname: fn() -> (String, String),
}

/// Runs `anthology atl-install …`; `args` are the arguments after `atl-install`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let env = Env::from_process();
    let fetch = atl_install::default_fetch(&env, &AtlLimits::default());
    run_with(
        args,
        &Context {
            env,
            fetch: &fetch,
            uname: platform::uname,
        },
    )
}

struct Args {
    data_dir: PathBuf,
    lock: Option<PathBuf>,
    dry_run: bool,
    timeout: f64,
}

fn usage_error(msg: &str) -> Output {
    Output {
        stderr: format!(
            "usage: anthology atl-install [-h] --data-dir DATA_DIR [--lock LOCK] [--dry-run] \
             [--timeout TIMEOUT]\nanthology atl-install: error: {msg}\n"
        )
        .into_bytes(),
        code: 2,
        ..Output::default()
    }
}

const HELP: &str =
    "usage: anthology atl-install [-h] --data-dir DATA_DIR [--lock LOCK] [--dry-run] \
[--timeout TIMEOUT]

Install the atl binary pinned by atl.lock.

options:
  -h, --help         show this help message and exit
  --data-dir DATA_DIR
                     plugin data dir; atl goes to <dir>/atl/<version>/
  --lock LOCK        path to atl.lock (default: $CLAUDE_PLUGIN_ROOT/atl.lock)
  --dry-run          print the plan and fetch nothing
  --timeout TIMEOUT  network timeout in seconds
";

fn parse(args: &[OsString]) -> Result<Args, Output> {
    use lexopt::prelude::*;
    let mut p = lexopt::Parser::from_args(args.iter().cloned());
    let (mut data_dir, mut lock, mut dry_run, mut timeout) = (None, None, false, None);
    loop {
        let arg = match p.next() {
            Ok(Some(arg)) => arg,
            Ok(None) => break,
            Err(e) => return Err(usage_error(&e.to_string())),
        };
        let value = |p: &mut lexopt::Parser, name: &str| {
            p.value()
                .map_err(|_| usage_error(&format!("argument {name}: expected one argument")))
        };
        match arg {
            Long("data-dir") => data_dir = Some(PathBuf::from(value(&mut p, "--data-dir")?)),
            Long("lock") => lock = Some(PathBuf::from(value(&mut p, "--lock")?)),
            Long("dry-run") => dry_run = true,
            Long("timeout") => {
                let raw = value(&mut p, "--timeout")?;
                let text = raw.to_string_lossy().into_owned();
                let parsed = py_float(&text).ok_or_else(|| {
                    usage_error(&format!(
                        "argument --timeout: invalid float value: '{text}'"
                    ))
                })?;
                timeout = Some(parsed);
            }
            Short('h') | Long("help") => {
                return Err(Output {
                    stdout: HELP.as_bytes().to_vec(),
                    ..Output::default()
                });
            }
            other => {
                return Err(usage_error(&format!(
                    "unrecognized arguments: {}",
                    other.unexpected()
                )));
            }
        }
    }
    let Some(data_dir) = data_dir else {
        return Err(usage_error(
            "the following arguments are required: --data-dir",
        ));
    };
    Ok(Args {
        data_dir,
        lock,
        dry_run,
        timeout: timeout.unwrap_or(atl_install::DEFAULT_TIMEOUT),
    })
}

/// `float(text)` for the shapes argparse's `type=float` takes: surrounding whitespace, and `_`
/// only between two digits (`1_0`, not `_1`, `1_`, `1__0` or `1_.5`).
fn py_float(text: &str) -> Option<f64> {
    let t = text.trim();
    let b = t.as_bytes();
    let underscores_ok = b.iter().enumerate().all(|(i, &c)| {
        c != b'_'
            || (i > 0 && i + 1 < b.len() && b[i - 1].is_ascii_digit() && b[i + 1].is_ascii_digit())
    });
    if !underscores_ok {
        return None;
    }
    t.replace('_', "").parse::<f64>().ok()
}

fn fail(code: i32, msg: &str) -> Output {
    Output {
        stderr: format!("atl_install: {msg}\n").into_bytes(),
        code,
        ..Output::default()
    }
}

/// [`run`] with explicit seams (tests).
pub fn run_with(args: &[OsString], ctx: &Context<'_>) -> Output {
    let args = match parse(args) {
        Ok(a) => a,
        Err(out) => return out,
    };
    let lock_path = match (&args.lock, ctx.env.get_os("CLAUDE_PLUGIN_ROOT")) {
        (Some(p), _) => p.clone(),
        (None, Some(root)) if !root.is_empty() => PathBuf::from(root).join("atl.lock"),
        (None, _) => {
            return fail(2, "--lock is required when CLAUDE_PLUGIN_ROOT is not set");
        }
    };
    let lock = match atl_install::load_lock(&lock_path) {
        Ok(l) => l,
        Err(e) => return fail(2, &e.py_str()),
    };
    let (system, machine) = (ctx.uname)();
    let tgt = match atl_install::target(&system, &machine) {
        Ok(t) => t,
        Err(e) => return fail(2, &e.py_str()),
    };
    let dest = args.data_dir.join("atl").join(&lock.version).join("atl");
    if args.dry_run {
        let text = format!(
            "target:      {}\nurl:         {}\nsha256:      {}\ndestination: {}\n",
            tgt.triple(),
            atl_install::release_url(&lock, tgt),
            lock.sha256.get(&tgt).map_or("", String::as_str),
            dest.to_string_lossy()
        );
        return Output {
            stdout: text.into_bytes(),
            ..Output::default()
        };
    }
    let mut deps = AtlDeps::new(ctx.fetch);
    deps.uname = ctx.uname;
    let result = atl_install::reconcile(&ctx.env, &args.data_dir, &lock, args.timeout, &deps);
    if let Some(w) = result.warning {
        return fail(1, &w);
    }
    let path = result
        .atl_path
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let head = if result.installed {
        "installed"
    } else {
        "already installed:"
    };
    Output {
        stdout: format!("{head} atl {} at {path}\n", lock.version).into_bytes(),
        ..Output::default()
    }
}

/// Port of `tests/test_rtk_install.py` `CliTest` (the in-process `main()` cases; the
/// subprocess cases are `crates/anthology-cli/tests/atl_install.rs`).
#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use anthology_core::state;
    use anthology_pycompat::json::{self, Value};
    use anthology_pycompat::pyerr::PyExc;
    use anthology_testkit::rt_b::{
        ATL_HOST, AtlFakeFetch, FakeAnswer, atl_lock, atl_lock_value, atl_tarball, atl_url_for,
    };

    use super::*;

    struct Fake(AtlFakeFetch);

    impl Fetch for Fake {
        fn fetch(&self, url: &str, dest: &mut dyn Write, timeout: f64) -> Result<(), PyExc> {
            match self.0.call(url, timeout) {
                FakeAnswer::Write(b) => {
                    dest.write_all(b)?;
                    Ok(())
                }
                FakeAnswer::Raise(t, m) => Err(PyExc::Other(t, m)),
            }
        }
    }

    fn linux() -> (String, String) {
        ("Linux".into(), "x86_64".into())
    }

    fn windows() -> (String, String) {
        ("Windows".into(), "AMD64".into())
    }

    struct Base {
        _dir: tempfile::TempDir,
        tmp: PathBuf,
        env: Env,
    }

    fn base() -> Base {
        let dir = tempfile::tempdir().expect("tmp");
        let tmp = std::fs::canonicalize(dir.path()).expect("realpath");
        let env = Env::default()
            .with_os("CLAUDE_CONFIG_DIR", tmp.join("config"))
            .with_os("HOME", tmp.join("home"));
        Base {
            _dir: dir,
            tmp,
            env,
        }
    }

    const V: &str = "1.2.1-anthology.1";
    const REPO: &str = "rbonweb/anthology";

    fn write_lock(path: &Path, blob: &[u8]) {
        let v = atl_lock_value(V, blob, REPO);
        std::fs::write(path, json::dumps(&v, &json::PY_DEFAULT)).expect("lock");
    }

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn text(b: &[u8]) -> String {
        String::from_utf8_lossy(b).into_owned()
    }

    fn s(p: &Path) -> &str {
        p.to_str().expect("utf-8 temp path")
    }

    #[test]
    fn test_dry_run_prints_the_plan_and_changes_nothing() {
        let b = base();
        let blob = atl_tarball();
        let lock_path = b.tmp.join("atl.lock");
        write_lock(&lock_path, &blob);
        let lock = atl_lock(V, &blob, REPO);
        let data = b.tmp.join("data");
        let fetch = Fake(AtlFakeFetch::default());
        let ctx = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: linux,
        };
        let args = os(&["--data-dir", s(&data), "--lock", s(&lock_path), "--dry-run"]);
        let out = run_with(&args, &ctx);
        assert_eq!(out.code, 0, "{}", text(&out.stderr));
        let stdout = text(&out.stdout);
        let dest = data.join("atl").join(V).join("atl");
        assert!(stdout.contains(ATL_HOST));
        assert!(stdout.contains(&atl_url_for(&lock, ATL_HOST)));
        assert!(stdout.contains(s(&dest)));
        assert_eq!(
            stdout,
            format!(
                "target:      {ATL_HOST}\nurl:         {}\nsha256:      {}\ndestination: {}\n",
                atl_url_for(&lock, ATL_HOST),
                anthology_testkit::release::sha256_hex(&blob),
                s(&dest)
            )
        );
        assert!(!data.exists());
        assert!(fetch.0.calls().is_empty());
    }

    #[test]
    fn test_install_failure_exits_non_zero_with_a_message() {
        let b = base();
        let lock_path = b.tmp.join("atl.lock");
        write_lock(&lock_path, &atl_tarball());
        let fetch = Fake(AtlFakeFetch::failing("OSError", "offline"));
        let ctx = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: linux,
        };
        let data = b.tmp.join("data");
        let out = run_with(
            &os(&["--data-dir", s(&data), "--lock", s(&lock_path)]),
            &ctx,
        );
        assert_eq!(out.code, 1);
        let err = text(&out.stderr);
        assert!(err.contains("offline"), "{err}");
        assert!(err.starts_with("atl_install: atl is not installed or up to date ("));
    }

    #[test]
    fn test_install_success_exits_zero_and_records_state() {
        let b = base();
        let blob = atl_tarball();
        let lock_path = b.tmp.join("atl.lock");
        write_lock(&lock_path, &blob);
        let lock = atl_lock(V, &blob, REPO);
        let fetch = Fake(AtlFakeFetch::serving(&atl_url_for(&lock, ATL_HOST), &blob));
        let ctx = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: linux,
        };
        let data = b.tmp.join("data");
        let args = os(&["--data-dir", s(&data), "--lock", s(&lock_path)]);
        let out = run_with(&args, &ctx);
        assert_eq!(out.code, 0, "{}", text(&out.stderr));
        let want = data.join("atl").join(V).join("atl");
        assert_eq!(
            state::read(&b.env).get("atl_path").and_then(Value::as_str),
            want.to_str()
        );
        assert_eq!(
            text(&out.stdout),
            format!("installed atl {V} at {}\n", s(&want))
        );
        let again = run_with(&args, &ctx);
        assert_eq!(
            text(&again.stdout),
            format!("already installed: atl {V} at {}\n", s(&want))
        );
        assert_eq!(fetch.0.calls().len(), 1);
    }

    #[test]
    fn the_default_lock_is_the_plugin_root_s() {
        let b = base();
        let root = b.tmp.join("plugin");
        std::fs::create_dir_all(&root).expect("root");
        write_lock(&root.join("atl.lock"), &atl_tarball());
        let fetch = Fake(AtlFakeFetch::default());
        let data = b.tmp.join("data");
        let args = os(&["--data-dir", s(&data), "--dry-run"]);
        let with_root = Context {
            env: b.env.clone().with_os("CLAUDE_PLUGIN_ROOT", &root),
            fetch: &fetch,
            uname: linux,
        };
        assert_eq!(run_with(&args, &with_root).code, 0);
        let without = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: linux,
        };
        let out = run_with(&args, &without);
        assert_eq!(out.code, 2);
        assert_eq!(
            text(&out.stderr),
            "atl_install: --lock is required when CLAUDE_PLUGIN_ROOT is not set\n"
        );
    }

    #[test]
    fn an_unsupported_platform_exits_2() {
        let b = base();
        let lock_path = b.tmp.join("atl.lock");
        write_lock(&lock_path, &atl_tarball());
        let fetch = Fake(AtlFakeFetch::default());
        let ctx = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: windows,
        };
        let out = run_with(&os(&["--data-dir", "/d", "--lock", s(&lock_path)]), &ctx);
        assert_eq!(out.code, 2);
        assert_eq!(
            text(&out.stderr),
            "atl_install: Windows isn't supported: atl is installed on macOS and Linux only\n"
        );
    }

    #[test]
    fn timeouts_parse_as_python_float() {
        assert_eq!(py_float(" 5 "), Some(5.0));
        assert_eq!(py_float("1_0"), Some(10.0));
        assert_eq!(py_float("2.5e1"), Some(25.0));
        assert!(py_float("inf").is_some_and(f64::is_infinite));
        for bad in ["_1", "1_", "1__0", "1_.5", "", "soon"] {
            assert_eq!(py_float(bad), None, "{bad}");
        }
    }

    #[test]
    fn usage_errors_exit_2() {
        let b = base();
        let fetch = Fake(AtlFakeFetch::default());
        let ctx = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: linux,
        };
        for args in [
            &[][..],
            &["--lock", "x"][..],
            &["--data-dir"][..],
            &["--data-dir", "d", "--timeout", "soon"][..],
            &["--data-dir", "d", "--timeout", "_1"][..],
            &["--data-dir", "d", "--timeout", "1__0"][..],
            &["--data-dir", "d", "--bogus"][..],
        ] {
            let out = run_with(&os(args), &ctx);
            assert_eq!(out.code, 2, "{args:?}");
            assert!(text(&out.stderr).contains("error: "), "{args:?}");
        }
        let help = run_with(&os(&["--help"]), &ctx);
        assert_eq!(help.code, 0);
        assert!(text(&help.stdout).starts_with("usage: anthology atl-install"));
    }
}
