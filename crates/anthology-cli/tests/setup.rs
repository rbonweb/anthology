//! `CliTest.test_subprocess_preview_runs_as_a_script` (`tests/test_setup.py`): the built
//! `anthology setup preview` as a separate process (Python ran the script with
//! `sys.executable`).
#![forbid(unsafe_code)]

use anthology_pycompat::json;
use anthology_testkit::rt_c::SetupCase;
use anthology_testkit::run::run_program;

const BIN: &str = env!("CARGO_BIN_EXE_anthology");

mod cli_test {
    use super::*;

    #[test]
    fn test_subprocess_preview_runs_as_a_script() {
        let c = SetupCase::new();
        let (plugin, tmp) = (c.plugin.to_string_lossy(), c.mem.tmp.to_string_lossy());
        let out = run_program(
            std::path::Path::new(BIN),
            &[
                "setup",
                "preview",
                "--plugin-root",
                &plugin,
                "--cwd",
                &tmp,
                "--json",
            ],
            b"",
            &c.env,
            &c.mem.tmp,
        );
        assert_eq!(out.code, 0, "{}", out.stderr_text());
        let v = json::loads(&out.stdout_text()).unwrap();
        assert!(v.as_object().unwrap().get("settings").is_some());
    }
}
