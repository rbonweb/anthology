//! Port of `tests/test_doctor.py` `CliTest`: the built `anthology doctor` as a program
//! (`subprocess.run([sys.executable, doctor.py, "--json"])`), in a sandboxed config. F-5:
//! `anthology_binary` replaces `python3`.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::process::{Command, Stdio};

use anthology_pycompat::json::{self, Value};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");

const INSTALLED: &str = r#"{
    "version": 2,
    "plugins": {
        "anthology@anthology": [{"scope": "user", "installPath": "/x/a", "version": "0.1.0"}],
        "superpowers@claude-plugins-official": [
            {"scope": "user", "installPath": "/x/s", "version": "7.0.2"}],
        "superpowers@other-market": [{"scope": "project", "installPath": "/x/s2"},
                                     {"scope": "user", "installPath": "/x/s3"}],
        "design-skills@design-skills": [{"scope": "local", "installPath": "/x/d"}],
        "anthology-superpowers@anthology": [{"scope": "user", "installPath": "/x/as"}],
        "superpowers-extras@m": [{"scope": "user", "installPath": "/x/e"}]
    }
}"#;

mod cli_test {
    use super::*;

    #[test]
    fn test_json_output() {
        let tmp = tempfile::tempdir().expect("tmp");
        let config = tmp.path().join("c");
        std::fs::create_dir_all(config.join("plugins")).expect("mkdir");
        std::fs::write(
            config.join("plugins").join("installed_plugins.json"),
            INSTALLED,
        )
        .expect("write");
        let out = Command::new(BIN)
            .args(["doctor", "--json"])
            .env_clear()
            .env("HOME", tmp.path())
            .env("CLAUDE_CONFIG_DIR", &config)
            .env("PATH", "/usr/bin:/bin")
            .current_dir(tmp.path())
            .stdin(Stdio::null())
            .output()
            .expect("run anthology");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{stderr}");
        let data = json::loads(&String::from_utf8_lossy(&out.stdout)).expect("json");
        let top = data.as_object().expect("object");
        let names: HashSet<&str> = top
            .get("checks")
            .and_then(Value::as_array)
            .expect("checks")
            .iter()
            .filter_map(|c| c.as_object()?.get("name")?.as_str())
            .collect();
        let want: HashSet<&str> = [
            "anthology_binary",
            "node",
            "docker",
            "browseros",
            "rtk",
            "platform_supported",
            "plugin_read_rule",
            "browseros_tools",
            "plugin_data",
            "path_wrappers",
            "agent_memory_path",
            "subagent_model",
        ]
        .into_iter()
        .collect();
        assert_eq!(names, want);
        assert_eq!(
            top.get("duplicates")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(4)
        );
    }

    /// argparse: `--json` abbreviates, an unknown option is a usage error (exit 2).
    #[test]
    fn arguments_follow_argparse() {
        let tmp = tempfile::tempdir().expect("tmp");
        let run = |args: &[&str]| {
            Command::new(BIN)
                .arg("doctor")
                .args(args)
                .env_clear()
                .env("HOME", tmp.path())
                .env("CLAUDE_CONFIG_DIR", tmp.path().join("c"))
                .env("PATH", "/usr/bin:/bin")
                .current_dir(tmp.path())
                .stdin(Stdio::null())
                .output()
                .expect("run anthology")
        };
        let out = run(&["--js"]);
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.starts_with(b"{\n  \"checks\": ["));
        let out = run(&["--nope"]);
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("unrecognized arguments: --nope"));
        let out = run(&[]);
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stdout.starts_with(b"anthology_binary   "));
    }
}
