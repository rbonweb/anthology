//! `anthology setup preview|apply|remove` (GC-10). Owner: RT4.
//!
//! As `anthology_setup.py main()`: one JSON object on stdout (`indent=2`), exit 2 on an
//! unsupported platform, 1 when `ok` is false, else 0 (GC-5); usage errors are argparse's
//! (exit 2, under the prog `anthology setup`). The environment is the process's, as Python's
//! `os.environ`. An argument that is not UTF-8 is read lossily.

use std::ffi::OsString;
use std::io::Read;

use anthology_core::Env;
use anthology_runtime::setup::{self, SetupDeps};

use super::Output;

/// Runs `anthology setup …`; `args` are the arguments after `setup`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let _ = stdin;
    run_with(args, &Env::from_process(), &SetupDeps::default())
}

/// `main(argv)` with the given environment and seams.
pub fn run_with(args: &[OsString], env: &Env, deps: &SetupDeps<'_>) -> Output {
    let argv: Vec<String> = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let (code, stdout, stderr) = setup::main(&argv, env, deps);
    Output {
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
        code,
    }
}

#[cfg(test)]
mod tests {
    //! `CliTest`'s `main()` rows (`tests/test_setup.py`).
    use super::*;
    use anthology_pycompat::json::{self, Value};
    use anthology_runtime::bootstrap::GuardBinaryStatus;
    use anthology_testkit::release::sha256_hex;
    use anthology_testkit::rt_c::{SETTINGS, SetupCase};
    use std::path::Path;

    fn present(_: &Path, _: u64, _: &Env) -> GuardBinaryStatus {
        GuardBinaryStatus::Present
    }

    fn windows() -> (String, String) {
        ("Windows".into(), "AMD64".into())
    }

    fn sha(text: &str) -> String {
        sha256_hex(text.as_bytes())
    }

    /// No download: the atl wiring is absent and the guard binary is "present".
    fn deps() -> SetupDeps<'static> {
        SetupDeps {
            atl: None,
            ensure_guard_binary: &present,
            ..SetupDeps::default()
        }
    }

    /// `run_main(*argv)`: the exit code and the parsed stdout.
    fn run_main(c: &SetupCase, d: &SetupDeps<'_>, argv: &[&str]) -> (i32, Value) {
        let args: Vec<OsString> = argv.iter().map(OsString::from).collect();
        let out = run_with(&args, &c.env, d);
        let text = String::from_utf8(out.stdout).unwrap();
        (out.code, json::loads(&text).unwrap())
    }

    fn get<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
        path.iter()
            .fold(v, |v, k| v.as_object().and_then(|m| m.get(k)).unwrap())
    }

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn test_preview_json() {
        let c = SetupCase::new();
        c.write_settings(SETTINGS);
        let (plugin, tmp) = (s(&c.plugin), s(&c.mem.tmp));
        let (code, out) = run_main(
            &c,
            &deps(),
            &["preview", "--plugin-root", &plugin, "--cwd", &tmp, "--json"],
        );
        assert_eq!(code, 0);
        let text = std::fs::read_to_string(&c.settings).unwrap();
        assert_eq!(
            get(&out, &["settings", "sha256"]).as_str(),
            Some(sha(&text).as_str())
        );
    }

    #[test]
    fn test_apply_and_remove_flags() {
        let c = SetupCase::new();
        let text = c.write_settings(SETTINGS);
        let (plugin, tmp, digest) = (s(&c.plugin), s(&c.mem.tmp), sha(&text));
        let (code, out) = run_main(
            &c,
            &deps(),
            &[
                "apply",
                "--plugin-root",
                &plugin,
                "--expect-sha256",
                &digest,
                "--cwd",
                &tmp,
                "--skip-approve",
            ],
        );
        assert_eq!(code, 0, "{out:?}");
        let (code, out) = run_main(
            &c,
            &deps(),
            &[
                "remove",
                "--plugin-root",
                &plugin,
                "--cwd",
                &tmp,
                "--unapprove",
                "--unapprove-all",
            ],
        );
        assert_eq!(code, 0, "{out:?}");
        assert_eq!(std::fs::read_to_string(&c.settings).unwrap(), text);
    }

    #[test]
    fn test_stale_digest_exits_1() {
        let c = SetupCase::new();
        c.write_settings(SETTINGS);
        let (plugin, tmp, zeros) = (s(&c.plugin), s(&c.mem.tmp), "0".repeat(64));
        let (code, out) = run_main(
            &c,
            &deps(),
            &[
                "apply",
                "--plugin-root",
                &plugin,
                "--expect-sha256",
                &zeros,
                "--cwd",
                &tmp,
            ],
        );
        assert_eq!(code, 1);
        assert_eq!(get(&out, &["ok"]), &Value::Bool(false));
    }

    #[test]
    fn test_windows_exits_2_with_the_refusal() {
        let c = SetupCase::new();
        let d = SetupDeps {
            uname: windows,
            ..deps()
        };
        let (plugin, zeros) = (s(&c.plugin), "0".repeat(64));
        for argv in [
            vec!["preview"],
            vec!["apply", "--expect-sha256", &zeros],
            vec!["remove"],
        ] {
            let mut full = argv.clone();
            full.extend(["--plugin-root", plugin.as_str()]);
            let (code, out) = run_main(&c, &d, &full);
            assert_eq!(code, 2, "{argv:?}");
            assert!(
                json::dumps(&out, &json::PY_DEFAULT).contains("macOS and Linux"),
                "{argv:?}"
            );
        }
    }
}
