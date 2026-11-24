//! `scripts/rust-test.sh test`: which cargo scope the container runs. A stub `docker` on PATH
//! records its argv, so the script runs without a container.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use repo_tests::repo;
use tempfile::TempDir;

const SCRIPT: &str = "scripts/rust-test.sh";
const SCOPE: &str = "RUST_TEST_SCOPE=";

/// What the stub `docker` was called with.
struct Call {
    args: Vec<String>,
}

impl Call {
    /// The scope the script hands to the container shell: `--workspace`, or empty.
    fn scope(&self) -> &str {
        let entry = self.args.iter().find_map(|a| a.strip_prefix(SCOPE));
        entry.unwrap_or_else(|| panic!("no {SCOPE} argument in {:?}", self.args))
    }

    /// The container shell script (the argument after `sh -c`).
    fn inner(&self) -> &str {
        let at = self.args.iter().position(|a| a == "-c").unwrap();
        &self.args[at + 1]
    }

    /// The mode and cargo arguments after the script name `rust-test`.
    fn passed(&self) -> Vec<&str> {
        let at = self.args.iter().position(|a| a == "rust-test").unwrap();
        self.args[at + 1..].iter().map(String::as_str).collect()
    }
}

fn run(args: &[&str]) -> Call {
    let dir: TempDir = tempfile::tempdir().unwrap();
    let stub = dir.path().join("docker");
    fs::write(
        &stub,
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > \"$DOCKER_ARGV\"\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let argv = dir.path().join("argv");

    let mut path = vec![dir.path().to_path_buf()];
    path.extend(env::split_paths(&env::var_os("PATH").unwrap()));
    let out = Command::new(repo::path(SCRIPT))
        .args(args)
        .env("PATH", env::join_paths(path).unwrap())
        .env("DOCKER_ARGV", &argv)
        .output()
        .expect("script ran");
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let raw = fs::read_to_string(&argv).expect("docker stub was called");
    let args = raw
        .split_terminator('\0')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Call { args }
}

#[test]
fn a_named_package_drops_workspace() {
    let call = run(&["test", "-p", "foo"]);
    assert_eq!(call.scope(), "");
    assert_eq!(call.passed(), ["test", "-p", "foo"]);
}

#[test]
fn no_package_keeps_workspace() {
    let call = run(&["test"]);
    assert_eq!(call.scope(), "--workspace");
    assert_eq!(call.passed(), ["test"]);
}

#[test]
fn package_equals_drops_workspace() {
    let call = run(&["test", "--package=foo"]);
    assert_eq!(call.scope(), "");
    assert_eq!(call.passed(), ["test", "--package=foo"]);
}

#[test]
fn every_package_spelling_drops_workspace() {
    for spelling in [
        &["-p", "foo"][..],
        &["--package", "foo"],
        &["-pfoo"],
        &["--package=foo"],
        &["--locked", "-p", "foo", "--test", "bar"],
    ] {
        let mut args = vec!["test"];
        args.extend_from_slice(spelling);
        assert_eq!(run(&args).scope(), "", "{spelling:?}");
    }
}

#[test]
fn a_flag_after_the_separator_is_not_a_package() {
    let call = run(&["test", "--", "-p"]);
    assert_eq!(call.scope(), "--workspace");
}

#[test]
fn a_filter_or_other_flag_keeps_workspace() {
    for args in [&["test", "--profile", "dev"][..], &["test", "plain_name"]] {
        assert_eq!(run(args).scope(), "--workspace", "{args:?}");
    }
}

#[test]
fn the_plain_and_all_features_runs_both_take_the_scope() {
    let call = run(&["test", "-p", "foo"]);
    let inner = call.inner();
    // The quotes and `$@` are escaped in the script text: the container shell expands them later.
    let plain = r#"cargo test $RUST_TEST_SCOPE --locked \"\$@\""#;
    let all = r#"cargo test $RUST_TEST_SCOPE --locked --all-features \"\$@\""#;
    assert!(inner.contains(&format!("{plain} && {all}")), "{inner}");
}

#[test]
fn lint_and_all_ignore_the_package_scope() {
    // `all` takes no cargo arguments and `lint` always checks the whole workspace.
    for mode in ["lint", "all"] {
        let call = run(&[mode]);
        assert_eq!(call.scope(), "--workspace", "{mode}");
        assert!(call.inner().contains("cargo clippy --workspace"), "{mode}");
    }
}
