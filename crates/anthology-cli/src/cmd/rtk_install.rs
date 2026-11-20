//! `anthology rtk-install` (GC-5, GC-9). Owner: RT2.
//!
//! `anthology rtk-install --data-dir PATH [--lock PATH] [--dry-run] [--timeout S]`, as
//! `rtk_install.py main()`: exit 0 with `installed rtk <ver> at <path>` or
//! `already installed: rtk <ver> at <path>` (or the four dry-run lines); exit 1 with
//! `rtk_install: <warning>`; exit 2 with `rtk_install: <msg>` for a bad lock or platform, and
//! for usage errors (argparse's usage text is D-4).
//!
//! The default `--lock` is `$CLAUDE_PLUGIN_ROOT/rtk.lock`: the binary no longer sits next to
//! the plugin's `scripts/` (lead ruling R9).

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;

use anthology_core::Env;
use anthology_core::platform;
use anthology_runtime::fetch::Fetch;
use anthology_runtime::rtk_install::{self, RtkDeps, RtkLimits};

use super::Output;

/// What the command reads from the outside world (the CLI's seams).
pub struct Context<'a> {
    pub env: Env,
    pub fetch: &'a dyn Fetch,
    pub uname: fn() -> (String, String),
}

/// Runs `anthology rtk-install …`; `args` are the arguments after `rtk-install`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    let env = Env::from_process();
    let fetch = rtk_install::default_fetch(&env, &RtkLimits::default());
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
            "usage: anthology rtk-install [-h] --data-dir DATA_DIR [--lock LOCK] [--dry-run] \
             [--timeout TIMEOUT]\nanthology rtk-install: error: {msg}\n"
        )
        .into_bytes(),
        code: 2,
        ..Output::default()
    }
}

const HELP: &str =
    "usage: anthology rtk-install [-h] --data-dir DATA_DIR [--lock LOCK] [--dry-run] \
[--timeout TIMEOUT]

Install the rtk binary pinned by rtk.lock.

options:
  -h, --help         show this help message and exit
  --data-dir DATA_DIR
                     plugin data dir; rtk goes to <dir>/rtk/<version>/
  --lock LOCK        path to rtk.lock (default: $CLAUDE_PLUGIN_ROOT/rtk.lock)
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
                let parsed = text.trim().replace('_', "").parse::<f64>().map_err(|_| {
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
        timeout: timeout.unwrap_or(rtk_install::DEFAULT_TIMEOUT),
    })
}

fn fail(code: i32, msg: &str) -> Output {
    Output {
        stderr: format!("rtk_install: {msg}\n").into_bytes(),
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
        (None, Some(root)) if !root.is_empty() => PathBuf::from(root).join("rtk.lock"),
        (None, _) => {
            return fail(2, "--lock is required when CLAUDE_PLUGIN_ROOT is not set");
        }
    };
    let lock = match rtk_install::load_lock(&lock_path) {
        Ok(l) => l,
        Err(e) => return fail(2, &e.py_str()),
    };
    let (system, machine) = (ctx.uname)();
    let tgt = match rtk_install::target(&system, &machine) {
        Ok(t) => t,
        Err(e) => return fail(2, &e.py_str()),
    };
    let dest = args.data_dir.join("rtk").join(&lock.version).join("rtk");
    if args.dry_run {
        let text = format!(
            "target:      {}\nurl:         {}\nsha256:      {}\ndestination: {}\n",
            tgt.triple(),
            rtk_install::release_url(&lock, tgt),
            lock.sha256.get(&tgt).map_or("", String::as_str),
            dest.to_string_lossy()
        );
        return Output {
            stdout: text.into_bytes(),
            ..Output::default()
        };
    }
    let mut deps = RtkDeps::new(ctx.fetch);
    deps.uname = ctx.uname;
    let result = rtk_install::reconcile(&ctx.env, &args.data_dir, &lock, args.timeout, &deps);
    if let Some(w) = result.warning {
        return fail(1, &w);
    }
    let path = result
        .rtk_path
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let head = if result.installed {
        "installed"
    } else {
        "already installed:"
    };
    Output {
        stdout: format!("{head} rtk {} at {path}\n", lock.version).into_bytes(),
        ..Output::default()
    }
}

/// Port of `tests/test_rtk_install.py` `CliTest` (the in-process `main()` cases; the
/// subprocess cases are `crates/anthology-cli/tests/rtk_install.rs`).
#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use anthology_core::state;
    use anthology_pycompat::json::{self, Value};
    use anthology_pycompat::pyerr::PyExc;
    use anthology_testkit::rt_b::{
        FakeAnswer, RTK_HOST, RtkFakeFetch, rtk_lock, rtk_lock_value, rtk_tarball, rtk_url_for,
    };

    use super::*;

    struct Fake(RtkFakeFetch);

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
        let v = rtk_lock_value(V, blob, REPO);
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
        let blob = rtk_tarball();
        let lock_path = b.tmp.join("rtk.lock");
        write_lock(&lock_path, &blob);
        let lock = rtk_lock(V, &blob, REPO);
        let data = b.tmp.join("data");
        let fetch = Fake(RtkFakeFetch::default());
        let ctx = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: linux,
        };
        let args = os(&["--data-dir", s(&data), "--lock", s(&lock_path), "--dry-run"]);
        let out = run_with(&args, &ctx);
        assert_eq!(out.code, 0, "{}", text(&out.stderr));
        let stdout = text(&out.stdout);
        let dest = data.join("rtk").join(V).join("rtk");
        assert!(stdout.contains(RTK_HOST));
        assert!(stdout.contains(&rtk_url_for(&lock, RTK_HOST)));
        assert!(stdout.contains(s(&dest)));
        assert_eq!(
            stdout,
            format!(
                "target:      {RTK_HOST}\nurl:         {}\nsha256:      {}\ndestination: {}\n",
                rtk_url_for(&lock, RTK_HOST),
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
        let lock_path = b.tmp.join("rtk.lock");
        write_lock(&lock_path, &rtk_tarball());
        let fetch = Fake(RtkFakeFetch::failing("OSError", "offline"));
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
        assert!(err.starts_with("rtk_install: rtk is not installed or up to date ("));
    }

    #[test]
    fn test_install_success_exits_zero_and_records_state() {
        let b = base();
        let blob = rtk_tarball();
        let lock_path = b.tmp.join("rtk.lock");
        write_lock(&lock_path, &blob);
        let lock = rtk_lock(V, &blob, REPO);
        let fetch = Fake(RtkFakeFetch::serving(&rtk_url_for(&lock, RTK_HOST), &blob));
        let ctx = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: linux,
        };
        let data = b.tmp.join("data");
        let args = os(&["--data-dir", s(&data), "--lock", s(&lock_path)]);
        let out = run_with(&args, &ctx);
        assert_eq!(out.code, 0, "{}", text(&out.stderr));
        let want = data.join("rtk").join(V).join("rtk");
        assert_eq!(
            state::read(&b.env).get("rtk_path").and_then(Value::as_str),
            want.to_str()
        );
        assert_eq!(
            text(&out.stdout),
            format!("installed rtk {V} at {}\n", s(&want))
        );
        let again = run_with(&args, &ctx);
        assert_eq!(
            text(&again.stdout),
            format!("already installed: rtk {V} at {}\n", s(&want))
        );
        assert_eq!(fetch.0.calls().len(), 1);
    }

    #[test]
    fn the_default_lock_is_the_plugin_root_s() {
        let b = base();
        let root = b.tmp.join("plugin");
        std::fs::create_dir_all(&root).expect("root");
        write_lock(&root.join("rtk.lock"), &rtk_tarball());
        let fetch = Fake(RtkFakeFetch::default());
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
            "rtk_install: --lock is required when CLAUDE_PLUGIN_ROOT is not set\n"
        );
    }

    #[test]
    fn an_unsupported_platform_exits_2() {
        let b = base();
        let lock_path = b.tmp.join("rtk.lock");
        write_lock(&lock_path, &rtk_tarball());
        let fetch = Fake(RtkFakeFetch::default());
        let ctx = Context {
            env: b.env.clone(),
            fetch: &fetch,
            uname: windows,
        };
        let out = run_with(&os(&["--data-dir", "/d", "--lock", s(&lock_path)]), &ctx);
        assert_eq!(out.code, 2);
        assert_eq!(
            text(&out.stderr),
            "rtk_install: Windows isn't supported: rtk is installed on macOS and Linux only\n"
        );
    }

    #[test]
    fn usage_errors_exit_2() {
        let b = base();
        let fetch = Fake(RtkFakeFetch::default());
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
            &["--data-dir", "d", "--bogus"][..],
        ] {
            let out = run_with(&os(args), &ctx);
            assert_eq!(out.code, 2, "{args:?}");
            assert!(text(&out.stderr).contains("error: "), "{args:?}");
        }
        let help = run_with(&os(&["--help"]), &ctx);
        assert_eq!(help.code, 0);
        assert!(text(&help.stdout).starts_with("usage: anthology rtk-install"));
    }
}
