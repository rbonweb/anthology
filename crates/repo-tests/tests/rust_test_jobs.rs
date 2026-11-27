//! The Rust test jobs in ci.yml. `ci/jobs/rust-scope.sh` decides what a run needs (its own tests
//! are in `ci_scope.rs`): the Linux and macOS shards come from its matrices, `rust-test-linux`
//! and `rust-test-macos` stay the job names to read (the required checks) and gate on their
//! shards, passing when the shards were rightly skipped. Here: the gate, the shard scripts, the
//! workflow's wiring and the cache.
//!
//! The scripts run here with stubs: a `docker` (Linux) or `cargo` and `rustup` (macOS) on PATH
//! that record their arguments.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use regex::Regex;
use repo_tests::repo;
use repo_tests::yaml::{self, Yaml};

const GATE: &str = "ci/jobs/rust-gate.sh";
const LINUX: &str = "ci/jobs/rust-test-linux.sh";
const MACOS: &str = "ci/jobs/rust-test-macos.sh";
const CACHE: &str = "ci/jobs/rust-cache.sh";

fn ci() -> Yaml {
    yaml::load_file(".github/workflows/ci.yml")
}

fn job(name: &str) -> Yaml {
    let job = ci()["jobs"][name].clone();
    assert!(!job.is_badvalue(), "ci.yml has no job {name}");
    job
}

fn text<'a>(y: &'a Yaml, key: &str) -> &'a str {
    y[key].as_str().unwrap_or("")
}

/// The `run` lines of every step of `job`, in order.
fn runs(job: &Yaml) -> Vec<String> {
    yaml::items(&job["steps"])
        .iter()
        .map(|s| text(s, "run").trim_end().to_string())
        .filter(|r| !r.is_empty())
        .collect()
}

fn step_running<'a>(job: &'a Yaml, run: &str) -> &'a Yaml {
    yaml::items(&job["steps"])
        .iter()
        .find(|s| text(s, "run").trim_end() == run)
        .unwrap_or_else(|| panic!("no step runs {run}"))
}

fn step_using<'a>(job: &'a Yaml, action: &str) -> &'a Yaml {
    yaml::items(&job["steps"])
        .iter()
        .find(|s| text(s, "uses").starts_with(action))
        .unwrap_or_else(|| panic!("no step uses {action}"))
}

fn executable(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
}

/// PATH with `dir` first.
fn path_with(dir: &Path) -> std::ffi::OsString {
    let mut path = vec![dir.to_path_buf()];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    std::env::join_paths(path).unwrap()
}

// ---- ci/jobs/rust-gate.sh ------------------------------------------------------------------

fn gate(scope: &str, run: &str, shards: &str) -> Output {
    Command::new("sh")
        .arg(repo::path(GATE))
        .args(["rust-test-linux", scope, run, shards])
        .output()
        .expect("sh ran")
}

#[test]
fn the_gate_passes_when_the_shards_pass_or_were_rightly_skipped() {
    for (run, shards) in [("true", "success"), ("false", "skipped")] {
        let out = gate("success", run, shards);
        assert!(out.status.success(), "{run} {shards}: {out:?}");
    }
}

#[test]
fn the_gate_fails_on_any_other_outcome() {
    for (scope, run, shards) in [
        ("success", "true", "failure"),
        ("success", "true", "cancelled"),
        ("success", "true", "skipped"),
        ("success", "false", "success"),
        ("success", "false", "failure"),
        ("success", "", "skipped"),
        ("failure", "", "skipped"),
        ("cancelled", "true", "success"),
    ] {
        let out = gate(scope, run, shards);
        assert!(!out.status.success(), "{scope} {run} {shards}: {out:?}");
        assert!(!out.stderr.is_empty(), "{scope} {run} {shards}: says why");
    }
}

// ---- ci/jobs/rust-test-linux.sh ------------------------------------------------------------

/// The arguments `scripts/rust-test.sh` hands the container after its own name, `rust-test`,
/// when ci/jobs/rust-test-linux.sh runs with `args` and the environment `env` (a stub `docker`
/// records them).
fn linux(args: &[&str], env: &[(&str, &str)]) -> Result<Vec<String>, Output> {
    let dir = tempfile::tempdir().unwrap();
    let argv = dir.path().join("argv");
    executable(
        dir.path(),
        "docker",
        "for a in \"$@\"; do printf '%s\\0' \"$a\"; done > \"$DOCKER_ARGV\"",
    );
    let out = Command::new("sh")
        .arg(repo::path(LINUX))
        .args(args)
        .env_remove("PACKAGES")
        .env_remove("SURFACES")
        .envs(env.iter().copied())
        .env("PATH", path_with(dir.path()))
        .env("DOCKER_ARGV", &argv)
        .output()
        .expect("sh ran");
    if !out.status.success() {
        return Err(out);
    }
    let raw = fs::read_to_string(&argv).expect("docker stub was called");
    let all: Vec<String> = raw.split_terminator('\0').map(str::to_owned).collect();
    let at = all.iter().position(|a| a == "rust-test").unwrap();
    Ok(all[at + 1..].to_vec())
}

#[test]
fn the_rest_shards_test_the_named_crates_without_the_replay() {
    let env = [("PACKAGES", "anthology-guard anthology-cli")];
    for (features, extra) in [("plain", &[][..]), ("all", &["--all-features"][..])] {
        let got = linux(&[features, "rest"], &env).unwrap();
        let mut want = vec![
            "run",
            "cargo",
            "test",
            "--locked",
            "-p",
            "anthology-guard",
            "-p",
            "anthology-cli",
        ];
        want.extend_from_slice(extra);
        want.extend_from_slice(&["--", "--skip", "golden_replay::"]);
        assert_eq!(got, want, "{features}");
    }
}

#[test]
fn a_rest_shard_without_crates_tests_the_whole_workspace() {
    let got = linux(&["plain", "rest"], &[]).unwrap();
    assert_eq!(
        got,
        [
            "run",
            "cargo",
            "test",
            "--locked",
            "--workspace",
            "--",
            "--skip",
            "golden_replay::"
        ]
    );
}

#[test]
fn the_replay_shards_run_only_the_named_surfaces_with_the_same_features() {
    let env = [(
        "SURFACES",
        "guard.check,hook.session-start,dev.version-bump,mem",
    )];
    for features in ["plain", "all"] {
        let got = linux(&[features, "replay"], &env).unwrap();
        assert_eq!(got[..3], ["run", "sh", "-c"], "{features}");
        let script = &got[3];
        assert_eq!(
            got[4..],
            [
                "rust-test-linux",
                features,
                "golden_replay::guard_check",
                "golden_replay::hook_session_start",
                "golden_replay::dev_version_bump",
                "golden_replay::mem",
            ],
            "{features}"
        );
        // The whole workspace is selected, so --all-features unifies every member's features
        // exactly as in a full run; a name that matches no test fails instead of passing.
        assert!(
            script.contains(r#"cargo test --workspace --locked $f --test golden -- --list"#),
            "{script}"
        );
        assert!(
            script.contains(
                r#"exec cargo test --workspace --locked $f --test golden -- --exact "$@""#
            ),
            "{script}"
        );
        assert!(script.contains("is listed"), "{script}");
    }
}

#[test]
fn a_replay_shard_needs_surfaces_and_the_other_shards_are_refused() {
    for (args, env) in [
        (&["plain", "replay"][..], &[][..]),
        (&["plain", "x"], &[]),
        (&["some", "rest"], &[]),
        (&[], &[]),
        (&["plain"], &[]),
        (&["plain", "rest", "extra"], &[]),
    ] {
        let out = linux(args, env).unwrap_err();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}

/// The names the shards pass are the ones golden.rs defines: `.` and `-` of a surface written
/// `_`, one test per committed golden, and the shards' skip filter is its module's name.
#[test]
fn the_replay_tests_the_shards_name_exist_for_every_golden() {
    let golden = repo::read("crates/difftest/tests/golden.rs");
    assert!(
        golden.contains("mod golden_replay {"),
        "the module the shards filter on"
    );
    let pair = Regex::new(r#"(?m)^\s+(\w+) => "([^"]+)",(?: //.*)?$"#).unwrap();
    let mut tested = BTreeSet::new();
    for c in pair.captures_iter(&golden) {
        assert_eq!(c[1], c[2].replace(['.', '-'], "_"), "{}", &c[2]);
        tested.insert(c[2].to_string());
    }
    let mut committed = BTreeSet::new();
    for krate in repo::list_dir("crates") {
        let folder = format!("crates/{krate}/tests/golden");
        if !repo::path(&folder).is_dir() {
            continue;
        }
        for file in repo::list_dir(&folder) {
            if let Some(surface) = file.strip_suffix(".jsonl.gz") {
                committed.insert(surface.to_string());
            }
        }
    }
    assert_eq!(
        tested, committed,
        "replay tests (left) against committed goldens (right)"
    );
    assert!(!tested.is_empty());
}

// ---- ci/jobs/rust-test-macos.sh ------------------------------------------------------------

/// The `cargo` calls ci/jobs/rust-test-macos.sh makes for `shard` with `PACKAGES` set to
/// `packages`, outside CI (stub `cargo` and `rustup`), one line each.
fn macos(shard: &str, packages: Option<&str>) -> Result<Vec<String>, Output> {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls");
    executable(dir.path(), "cargo", "echo \"cargo $*\" >> \"$CALLS\"");
    executable(dir.path(), "rustup", "echo \"rustup $*\" >> \"$CALLS\"");
    let mut cmd = Command::new("sh");
    cmd.arg(repo::path(MACOS))
        .arg(shard)
        .env("PATH", path_with(dir.path()))
        .env("CALLS", &log)
        .env_remove("CI")
        .env_remove("PACKAGES");
    if let Some(p) = packages {
        cmd.env("PACKAGES", p);
    }
    let out = cmd.output().expect("sh ran");
    if !out.status.success() {
        return Err(out);
    }
    Ok(fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect())
}

#[test]
fn the_macos_shards_together_run_both_feature_sets_and_the_ledger() {
    let plain = macos("plain", None).unwrap();
    assert_eq!(
        plain,
        [
            "rustup toolchain install",
            "cargo --version",
            "cargo test --workspace --locked",
        ]
    );
    let all = macos("all", None).unwrap();
    assert_eq!(
        all,
        [
            "rustup toolchain install",
            "cargo --version",
            "cargo test --workspace --locked --all-features",
            "cargo test --workspace --locked -- --list",
            "cargo test --workspace --locked --all-features -- --list",
            "cargo test --workspace --locked -- --list --ignored",
            "cargo test --workspace --locked --all-features -- --list --ignored",
            "cargo run --locked --quiet -p anthology-dev -- ledger check --report --listings \
             target/ledger-macos --platform macos",
        ]
    );
    // The shim network cases run in CI only, after the plain tests.
    let script = repo::read(MACOS);
    assert_eq!(script.matches("--test shims").count(), 1);
    assert!(macos("other", None).is_err());
}

#[test]
fn a_macos_shard_with_crates_tests_only_those_and_skips_the_ledger() {
    let packages = Some("anthology-guard repo-tests");
    assert_eq!(
        macos("plain", packages).unwrap(),
        [
            "rustup toolchain install",
            "cargo --version",
            "cargo test -p anthology-guard -p repo-tests --locked",
        ]
    );
    assert_eq!(
        macos("all", packages).unwrap(),
        [
            "rustup toolchain install",
            "cargo --version",
            "cargo test -p anthology-guard -p repo-tests --locked --all-features",
        ]
    );
}

// ---- ci/jobs/atl-test-macos.sh -------------------------------------------------------------

#[test]
fn atl_tests_run_on_macos_with_atls_own_toolchain_and_lock() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls");
    for tool in ["cargo", "rustup"] {
        executable(
            dir.path(),
            tool,
            &format!("echo \"{tool} $* in $(basename \"$PWD\")\" >> \"$CALLS\""),
        );
    }
    let out = Command::new("sh")
        .arg(repo::path("ci/jobs/atl-test-macos.sh"))
        .env("PATH", path_with(dir.path()))
        .env("CALLS", &log)
        .output()
        .expect("sh ran");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // In atl/, where rust-toolchain.toml and Cargo.lock are atl's own.
    let calls: Vec<String> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        calls,
        [
            "rustup toolchain install in atl",
            "cargo --version in atl",
            "cargo test --locked in atl",
        ]
    );
}

// ---- ci/jobs/rust-cache.sh -----------------------------------------------------------------

/// What a stub `docker` recorded for `ci/jobs/rust-cache.sh args`, one call per line.
fn cache_calls(args: &[&str]) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls");
    executable(dir.path(), "docker", "echo \"docker $*\" >> \"$CALLS\"");
    let out = Command::new("sh")
        .arg(repo::path(CACHE))
        .args(args)
        .env("PATH", path_with(dir.path()))
        .env("CALLS", &log)
        .output()
        .expect("sh ran");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The named volumes `scripts/rust-test.sh run` mounts: its stub `docker` prints its argv.
fn runner_volumes() -> BTreeSet<String> {
    let dir = tempfile::tempdir().unwrap();
    let argv = dir.path().join("argv");
    executable(
        dir.path(),
        "docker",
        "for a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$DOCKER_ARGV\"",
    );
    let out = Command::new(repo::path("scripts/rust-test.sh"))
        .args(["run", "true"])
        .env("PATH", path_with(dir.path()))
        .env("DOCKER_ARGV", &argv)
        .output()
        .expect("rust-test.sh ran");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fs::read_to_string(&argv)
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("anthology-") && l.contains(':'))
        .map(|l| l.split(':').next().unwrap().to_string())
        .collect()
}

#[test]
fn the_cache_volumes_are_the_ones_the_test_runner_mounts() {
    let volumes = runner_volumes();
    assert_eq!(volumes.len(), 3, "{volumes:?}");
    let calls = cache_calls(&["volumes", "/cache"]);
    let created: BTreeSet<String> = calls
        .iter()
        .map(|c| {
            let name = c.split_whitespace().last().unwrap();
            name.to_string()
        })
        .collect();
    assert_eq!(created, volumes, "{calls:?}");
    for call in &calls {
        assert!(
            call.starts_with("docker volume create --driver local"),
            "{call}"
        );
        assert!(
            call.contains("--opt type=none") && call.contains("--opt o=bind"),
            "{call}"
        );
        assert!(call.contains("device=/cache/"), "{call}");
    }
}

#[test]
fn the_cache_folders_are_made_before_docker_binds_them() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let out = Command::new("sh")
        .arg(repo::path(CACHE))
        .args(["dirs"])
        .arg(&cache)
        .output()
        .unwrap();
    assert!(out.status.success());
    for sub in ["cargo", "rustup", "target"] {
        assert!(cache.join(sub).is_dir(), "{sub}");
    }
}

#[test]
fn trimming_keeps_what_the_dependencies_built_and_drops_the_rest() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipped: node is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let debug = dir.path().join("target/debug");
    let make = |rel: &str| {
        let p = debug.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "x").unwrap();
    };
    // third party: kept
    for keep in [
        "deps/libserde-1a.rlib",
        "deps/serde-1a.d",
        ".fingerprint/serde-1a/lib-serde",
        "build/serde-1a/out/x",
    ] {
        make(keep);
    }
    // the workspace's own crates, test programs and incremental state: dropped
    for drop in [
        "deps/libanthology_core-9f.rlib",
        "deps/libanthology_core-9f.rmeta",
        ".fingerprint/anthology-core-9f/lib-anthology_core",
        "build/anthology-core-9f/out/x",
        "deps/golden-77",
        "deps/difftest-77",
        "incremental/x/y",
        "examples/e",
    ] {
        make(drop);
    }
    let out = Command::new("sh")
        .arg(repo::path(CACHE))
        .arg("trim")
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for keep in [
        "deps/libserde-1a.rlib",
        "deps/serde-1a.d",
        ".fingerprint/serde-1a/lib-serde",
        "build/serde-1a/out/x",
    ] {
        assert!(debug.join(keep).exists(), "{keep} was kept");
    }
    for gone in [
        "deps/libanthology_core-9f.rlib",
        "deps/libanthology_core-9f.rmeta",
        ".fingerprint/anthology-core-9f",
        "build/anthology-core-9f",
        "deps/golden-77",
        "deps/difftest-77",
        "incremental",
        "examples",
    ] {
        assert!(!debug.join(gone).exists(), "{gone} was dropped");
    }
}

// ---- ci.yml --------------------------------------------------------------------------------

fn event_types() -> Vec<String> {
    yaml::strings(&ci()["on"]["pull_request"]["types"])
}

#[test]
fn a_label_change_and_a_manual_run_start_the_workflow() {
    // The full-ci label is read by rust-scope, so adding or removing it re-runs the checks.
    let types = event_types();
    for t in ["opened", "synchronize", "reopened", "labeled", "unlabeled"] {
        assert!(types.contains(&t.to_string()), "{t}");
    }
    assert!(yaml::has(&ci()["on"], "workflow_dispatch"));
    assert_eq!(
        yaml::strings(&ci()["on"]["push"]["branches"]),
        ["main", "build"]
    );
}

#[test]
fn the_scope_job_reads_the_pull_request_range_and_labels() {
    let job = job("rust-scope");
    for out in [
        "full",
        "atl",
        "atl_macos",
        "fuzz",
        "run_linux",
        "run_macos",
        "linux_matrix",
        "macos_matrix",
    ] {
        assert_eq!(
            text(&job["outputs"], out),
            format!("${{{{ steps.scope.outputs.{out} }}}}"),
            "{out}"
        );
    }
    let run = r#"ci/jobs/rust-scope.sh "$BASE" "$HEAD_SHA" >>"$GITHUB_OUTPUT""#;
    let step = step_running(&job, run);
    assert_eq!(text(step, "id"), "scope");
    assert_eq!(
        text(&step["env"], "BASE"),
        "${{ github.event.pull_request.base.sha }}"
    );
    assert_eq!(
        text(&step["env"], "HEAD_SHA"),
        "${{ github.event.pull_request.head.sha }}"
    );
    assert_eq!(
        text(&step["env"], "PR_LABELS"),
        "${{ toJSON(github.event.pull_request.labels.*.name) }}"
    );
    // cargo metadata must not install the pinned toolchain on the runner just to read manifests.
    assert_eq!(text(&step["env"], "RUSTUP_TOOLCHAIN"), "stable");
    let checkout = &yaml::items(&job["steps"])[0];
    assert_eq!(checkout["with"]["fetch-depth"].as_i64(), Some(0));
}

/// A shard job: gated on a scope output, with its matrix read from another.
fn check_shard_job(name: &str, runner: &str, run_output: &str, matrix_output: &str) -> Yaml {
    let job = job(name);
    assert_eq!(text(&job, "needs"), "rust-scope", "{name}");
    assert_eq!(
        text(&job, "if"),
        format!("needs.rust-scope.outputs.{run_output} == 'true'"),
        "{name}"
    );
    assert_eq!(text(&job, "runs-on"), runner, "{name}");
    assert_eq!(
        job["strategy"]["fail-fast"].as_bool(),
        Some(false),
        "{name}"
    );
    assert_eq!(
        text(&job["strategy"], "matrix"),
        format!("${{{{ fromJSON(needs.rust-scope.outputs.{matrix_output}) }}}}"),
        "{name}"
    );
    assert_eq!(
        text(&job, "name"),
        format!("{name} (${{{{ matrix.name }}}})"),
        "{name}: the shard name shows what it runs"
    );
    job
}

#[test]
fn the_linux_shards_come_from_the_scope_matrix() {
    let job = check_shard_job(
        "rust-test-linux-shard",
        "ubuntu-latest",
        "run_linux",
        "linux_matrix",
    );
    let run = r#"ci/jobs/rust-test-linux.sh "$FEATURES" "$PART""#;
    let step = step_running(&job, run);
    for (key, axis) in [
        ("FEATURES", "features"),
        ("PART", "part"),
        ("PACKAGES", "packages"),
        ("SURFACES", "surfaces"),
    ] {
        assert_eq!(
            text(&step["env"], key),
            format!("${{{{ matrix.{axis} }}}}"),
            "{key}"
        );
    }
}

#[test]
fn the_macos_shards_come_from_the_scope_matrix() {
    let job = check_shard_job(
        "rust-test-macos-shard",
        "macos-15",
        "run_macos",
        "macos_matrix",
    );
    let run = r#"ci/jobs/rust-test-macos.sh "$FEATURES""#;
    let step = step_running(&job, run);
    assert_eq!(text(&step["env"], "FEATURES"), "${{ matrix.features }}");
    assert_eq!(text(&step["env"], "PACKAGES"), "${{ matrix.packages }}");
}

fn check_gate(name: &str, run_output: &str) {
    let job = job(name);
    let needs = yaml::strings(&job["needs"]);
    assert_eq!(needs, ["rust-scope", &format!("{name}-shard")], "{name}");
    assert_eq!(text(&job, "if"), "${{ !cancelled() }}", "{name}");
    let gate_run = format!(r#"ci/jobs/rust-gate.sh {name} "$SCOPE" "$RUN" "$SHARDS""#);
    assert_eq!(runs(&job), [gate_run.as_str()], "{name}");
    let step = step_running(&job, &gate_run);
    assert_eq!(
        text(&step["env"], "SCOPE"),
        "${{ needs.rust-scope.result }}"
    );
    assert_eq!(
        text(&step["env"], "RUN"),
        format!("${{{{ needs.rust-scope.outputs.{run_output} }}}}")
    );
    assert_eq!(
        text(&step["env"], "SHARDS"),
        &format!("${{{{ needs.{name}-shard.result }}}}")
    );
}

#[test]
fn the_required_checks_keep_their_names_and_gate_on_the_shards() {
    check_gate("rust-test-linux", "run_linux");
    check_gate("rust-test-macos", "run_macos");
}

#[test]
fn atl_test_macos_runs_for_a_change_to_atl_itself() {
    let job = job("atl-test-macos");
    assert_eq!(text(&job, "needs"), "rust-scope");
    assert_eq!(
        text(&job, "if"),
        "needs.rust-scope.outputs.atl_macos == 'true'"
    );
    assert_eq!(text(&job, "runs-on"), "macos-15");
    assert_eq!(
        runs(&job),
        ["rustup toolchain install", "ci/jobs/atl-test-macos.sh"]
    );
    assert_eq!(
        text(&job["steps"][0], "uses").split('@').next(),
        Some("actions/checkout")
    );
    let cache = step_using(&job, "Swatinem/rust-cache@");
    assert_eq!(text(&cache["with"], "workspaces"), "atl");
}

#[test]
fn atl_test_and_fuzz_build_run_only_when_their_inputs_changed() {
    for (name, output) in [("atl-test", "atl"), ("fuzz-build", "fuzz")] {
        let job = job(name);
        assert_eq!(text(&job, "needs"), "rust-scope", "{name}");
        assert_eq!(
            text(&job, "if"),
            format!("needs.rust-scope.outputs.{output} == 'true'"),
            "{name}"
        );
    }
}

/// The folders `actions/cache` keeps for the Linux shards: the three the test runner mounts.
const CACHE_DIRS: [&str; 3] = [
    "${{ runner.temp }}/rust-cache/cargo",
    "${{ runner.temp }}/rust-cache/rustup",
    "${{ runner.temp }}/rust-cache/target",
];

#[test]
fn a_linux_shard_restores_binds_trims_and_saves_the_rust_cache() {
    let job = job("rust-test-linux-shard");
    let order: Vec<String> = yaml::items(&job["steps"])
        .iter()
        .map(|s| {
            let run = text(s, "run").trim_end();
            let uses = text(s, "uses");
            if !run.is_empty() {
                run.to_string()
            } else {
                uses.to_string()
            }
        })
        .collect();
    let at = |needle: &str| {
        order
            .iter()
            .position(|s| s.contains(needle))
            .unwrap_or_else(|| panic!("no step with {needle}: {order:?}"))
    };
    // dirs, restore, volumes, test, trim, save
    assert!(at("rust-cache.sh dirs") < at("actions/cache/restore@"));
    assert!(at("actions/cache/restore@") < at("rust-cache.sh volumes"));
    assert!(at("rust-cache.sh volumes") < at("rust-test-linux.sh"));
    assert!(at("rust-test-linux.sh") < at("rust-cache.sh trim"));
    assert!(at("rust-cache.sh trim") < at("actions/cache/save@"));
    for sub in ["dirs", "volumes", "trim"] {
        let step = step_running(
            &job,
            &format!(r#"ci/jobs/rust-cache.sh {sub} "$CACHE_DIR""#),
        );
        assert_eq!(
            text(&step["env"], "CACHE_DIR"),
            "${{ runner.temp }}/rust-cache",
            "{sub}"
        );
    }
    let restore = step_using(&job, "actions/cache/restore@");
    let save = step_using(&job, "actions/cache/save@");
    for step in [restore, save] {
        let paths: Vec<&str> = text(&step["with"], "path").lines().collect();
        assert_eq!(paths, CACHE_DIRS);
    }
    // One key per shard, for the toolchain, the image and the lock: the first full run saves it.
    let key = text(&restore["with"], "key");
    assert_eq!(key, text(&save["with"], "key"));
    for part in [
        "matrix.name",
        "hashFiles('Cargo.lock', 'rust-toolchain.toml', 'ci/images/rust.Dockerfile')",
    ] {
        assert!(key.contains(part), "{key}");
    }
    assert!(text(&restore["with"], "restore-keys").contains("matrix.name"));
    // Only a push (build, main) or a manual run saves: a pull request only reads, so it cannot
    // fill the repository's cache quota with its own copies.
    assert_eq!(
        text(save, "if"),
        "${{ success() && github.event_name != 'pull_request' }}"
    );
    // A cached folder is not trusted for anything but speed: the pinned image runs the tests.
    assert!(!text(restore, "if").contains("pull_request"));
}

#[test]
fn a_macos_shard_caches_the_dependency_build() {
    let job = job("rust-test-macos-shard");
    let cache = step_using(&job, "Swatinem/rust-cache@");
    assert_eq!(text(&cache["with"], "key"), "${{ matrix.features }}");
    assert_eq!(
        text(&cache["with"], "save-if"),
        "${{ github.event_name != 'pull_request' }}"
    );
    // The pinned toolchain is installed first, so the cache key is made for the toolchain that
    // builds, not for the runner's own.
    let steps = yaml::items(&job["steps"]);
    let at = |pred: &dyn Fn(&Yaml) -> bool| steps.iter().position(pred).unwrap();
    let install = at(&|s| text(s, "run").trim_end() == "rustup toolchain install");
    let cached = at(&|s| text(s, "uses").starts_with("Swatinem/rust-cache@"));
    let tests = at(&|s| text(s, "run").contains("rust-test-macos.sh"));
    assert!(install < cached && cached < tests);
}

#[test]
fn the_cache_actions_are_pinned_and_the_workflow_stays_read_only() {
    let uses_re = Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$").unwrap();
    for name in ["rust-test-linux-shard", "rust-test-macos-shard"] {
        for step in yaml::items(&job(name)["steps"]) {
            if yaml::has(step, "uses") {
                assert!(
                    uses_re.is_match(text(step, "uses")),
                    "{name}: {}",
                    text(step, "uses")
                );
            }
        }
    }
    assert_eq!(
        ci()["permissions"].as_hash().map(|h| h.len()),
        Some(1),
        "contents: read and nothing else"
    );
    assert_eq!(text(&ci()["permissions"], "contents"), "read");
}
