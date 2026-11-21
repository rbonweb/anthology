//! test_vendor.CompatibilityTest. `test_vendor_py_parses_as_python_39` is dropped (py-compat):
//! there is no Python left to parse.

use std::fs;
use std::process::{Command, Stdio};

use crate::fixture::repo_root;

#[test]
fn test_cli_is_runnable_and_lists_commands() {
    let sandbox = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_anthology-dev"))
        .args(["vendor", "--help"])
        .current_dir(sandbox.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    for cmd in ["fetch", "build", "check", "sync", "diff"] {
        let re = regex::Regex::new(&format!(r"\b{cmd}\b")).unwrap();
        assert!(re.is_match(&text), "{cmd} not in {text}");
    }
}

#[test]
fn test_cache_dir_is_git_ignored() {
    let ignore = fs::read_to_string(repo_root().join(".gitignore")).unwrap();
    assert_eq!(anthology_dev::vendor::CACHE_DIRNAME, ".vendor-cache");
    assert!(ignore.lines().any(|l| l == ".vendor-cache/"));
}
