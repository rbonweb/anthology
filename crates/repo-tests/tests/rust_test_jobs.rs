//! The Rust test jobs in ci.yml: the full suite runs in parallel shards, and a pull request that
//! changes nothing but release metadata runs only the repository checks (repo-tests, which hold
//! the `anthology_lock` and `atl_lock` tests). `rust-test-linux` and `rust-test-macos` stay the
//! job names to read: each gates on its shards.
//!
//! The scripts run here with stubs: a throwaway git repository for `ci/jobs/rust-scope.sh`, and
//! a `docker` (Linux) or `cargo` and `rustup` (macOS) on PATH that record their arguments.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use repo_tests::repo;
use repo_tests::yaml::{self, Yaml};
use tempfile::TempDir;

const SCOPE: &str = "ci/jobs/rust-scope.sh";
const GATE: &str = "ci/jobs/rust-gate.sh";
const LINUX: &str = "ci/jobs/rust-test-linux.sh";
const MACOS: &str = "ci/jobs/rust-test-macos.sh";
/// The difftest golden replay: about 9 of the 16 minutes of one Linux run, so it is a shard.
const REPLAY: &str = "the_committed_goldens_replay_clean";

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

fn executable(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
}

// ---- ci/jobs/rust-scope.sh -----------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .output()
        .expect("git ran");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn put(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// Replaces `from` (which must be there) with `to` in `rel`.
fn edit(dir: &Path, rel: &str, from: &str, to: &str) {
    let p = dir.join(rel);
    let old = fs::read_to_string(&p).unwrap();
    assert!(old.contains(from), "{rel} has no {from:?}");
    fs::write(p, old.replacen(from, to, 1)).unwrap();
}

const CARGO_TOML: &str = "[workspace]\nmembers = [\"crates/*\"]\n\n\
[workspace.package]\nversion = \"0.2.3\"\nedition = \"2024\"\n\n\
[workspace.dependencies]\nregex = \"=1.13.2\"\n\n\
[dependencies.extra]\nversion = \"=1.0.0\"\n";

const CARGO_LOCK: &str = "version = 4\n\n\
[[package]]\nname = \"anthology-cli\"\nversion = \"0.2.3\"\ndependencies = [\n \"regex\",\n]\n\n\
[[package]]\nname = \"regex\"\nversion = \"1.13.2\"\n\
source = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"aaaa\"\n";

const PLUGIN_JSON: &str = "plugins/anthology/.claude-plugin/plugin.json";

/// A repository at a base commit holding every kind of release metadata and some code.
fn fixture() -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    put(d, "Cargo.toml", CARGO_TOML);
    put(d, "Cargo.lock", CARGO_LOCK);
    put(d, "fuzz/Cargo.lock", CARGO_LOCK);
    put(
        d,
        PLUGIN_JSON,
        "{\n  \"name\": \"anthology\",\n  \"version\": \"0.5.0\",\n  \"description\": \"d\"\n}\n",
    );
    put(
        d,
        "plugins/anthology/anthology.lock",
        "{\n  \"version\": \"0.2.2\",\n  \"sha256\": \"aa\"\n}\n",
    );
    put(d, "CHANGELOG.md", "# Changelog\n");
    put(d, "changelog.d/README.md", "How to write a fragment.\n");
    put(d, ".claude/memory/reference-x.md", "note\n");
    put(
        d,
        ".claude/agent-memory/anthology-implementer/a.md",
        "note\n",
    );
    put(d, "crates/x/src/lib.rs", "pub fn x() {}\n");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    let base = git(d, &["rev-parse", "HEAD"]);
    (dir, base)
}

fn run_scope(dir: &Path, base: &str, head: &str) -> Output {
    Command::new("sh")
        .arg(repo::path(SCOPE))
        .args([base, head])
        .current_dir(dir)
        .output()
        .expect("sh ran")
}

/// rust-scope.sh's answer for a commit that `change` makes on top of the fixture's base.
fn scope_after(change: impl FnOnce(&Path)) -> String {
    let (dir, base) = fixture();
    let d = dir.path();
    change(d);
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "--allow-empty", "-m", "head"]);
    let head = git(d, &["rev-parse", "HEAD"]);
    let out = run_scope(d, &base, &head);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Every release-metadata change at once: the versions in Cargo.toml and both locks, a plugin
/// version, an anthology.lock pin, the changelog, its fragments and memory notes.
fn release_metadata(d: &Path) {
    edit(
        d,
        "Cargo.toml",
        "version = \"0.2.3\"",
        "version = \"0.2.4\"",
    );
    for lock in ["Cargo.lock", "fuzz/Cargo.lock"] {
        edit(d, lock, "version = \"0.2.3\"", "version = \"0.2.4\"");
    }
    edit(
        d,
        PLUGIN_JSON,
        "\"version\": \"0.5.0\"",
        "\"version\": \"0.5.1\"",
    );
    put(
        d,
        "plugins/anthology/anthology.lock",
        "{\n  \"version\": \"0.2.3\",\n  \"sha256\": \"bb\"\n}\n",
    );
    put(d, "CHANGELOG.md", "# Changelog\n\n## 0.2.4\n");
    put(d, "changelog.d/anthology/10-pin.md", "- Pin.\n");
    fs::remove_file(d.join("changelog.d/README.md")).unwrap();
    put(d, ".claude/memory/reference-x.md", "note, changed\n");
    put(
        d,
        ".claude/agent-memory/anthology-implementer/b.md",
        "new note\n",
    );
}

/// A named change to the fixture.
type Case = (&'static str, fn(&Path));

#[test]
fn release_metadata_alone_skips_the_full_suite() {
    assert_eq!(scope_after(release_metadata), "full=false\n");
}

#[test]
fn each_kind_of_release_metadata_alone_skips_the_full_suite() {
    let cases: [Case; 6] = [
        ("Cargo.toml version", |d| {
            edit(
                d,
                "Cargo.toml",
                "version = \"0.2.3\"",
                "version = \"0.2.4\"",
            )
        }),
        ("fuzz lock version", |d| {
            edit(
                d,
                "fuzz/Cargo.lock",
                "version = \"0.2.3\"",
                "version = \"0.2.4\"",
            )
        }),
        ("plugin version", |d| {
            edit(
                d,
                PLUGIN_JSON,
                "\"version\": \"0.5.0\"",
                "\"version\": \"0.5.1\"",
            )
        }),
        ("anthology.lock", |d| {
            edit(d, "plugins/anthology/anthology.lock", "aa", "cc")
        }),
        ("changelog fragment", |d| {
            put(d, "changelog.d/anthology-guardrails/x.md", "- x\n")
        }),
        ("memory note", |d| put(d, ".claude/memory/new.md", "n\n")),
    ];
    for (what, change) in cases {
        assert_eq!(scope_after(change), "full=false\n", "{what}");
    }
}

#[test]
fn anything_beyond_release_metadata_runs_the_full_suite() {
    let cases: [Case; 19] = [
        ("code", |d| put(d, "crates/x/src/lib.rs", "pub fn y() {}\n")),
        ("code beside metadata", |d| {
            release_metadata(d);
            put(d, "crates/x/src/lib.rs", "pub fn y() {}\n");
        }),
        ("a workspace dependency", |d| {
            edit(d, "Cargo.toml", "=1.13.2", "=1.13.3")
        }),
        ("a version line outside a package table", |d| {
            edit(
                d,
                "Cargo.toml",
                "version = \"=1.0.0\"",
                "version = \"=1.0.1\"",
            )
        }),
        ("a new Cargo.toml line", |d| {
            edit(
                d,
                "Cargo.toml",
                "edition = \"2024\"\n",
                "edition = \"2024\"\nlicense = \"x\"\n",
            )
        }),
        ("a registry dependency in the lock", |d| {
            edit(
                d,
                "Cargo.lock",
                "version = \"1.13.2\"",
                "version = \"1.13.3\"",
            );
            edit(
                d,
                "Cargo.lock",
                "checksum = \"aaaa\"",
                "checksum = \"bbbb\"",
            );
        }),
        ("a lock checksum", |d| {
            edit(
                d,
                "fuzz/Cargo.lock",
                "checksum = \"aaaa\"",
                "checksum = \"bbbb\"",
            )
        }),
        ("a plugin.json key other than version", |d| {
            edit(
                d,
                PLUGIN_JSON,
                "\"description\": \"d\"",
                "\"description\": \"e\"",
            )
        }),
        ("Cargo.toml deleted", |d| {
            fs::remove_file(d.join("Cargo.toml")).unwrap()
        }),
        ("an anthology.lock below a plugin's top folder", |d| {
            put(d, "plugins/anthology/knowledge/x/anthology.lock", "{}\n")
        }),
        ("a changelog that is not the root one", |d| {
            put(d, "atl/CHANGELOG.md", "# x\n")
        }),
        ("another .claude file", |d| {
            put(d, ".claude/settings.json", "{}\n")
        }),
        ("no change at all", |_| {}),
        // A version line that goes or is added is not a version bump, only a changed one is.
        ("a removed Cargo.toml version line", |d| {
            edit(d, "Cargo.toml", "version = \"0.2.3\"\n", "")
        }),
        ("a removed lock version line", |d| {
            edit(d, "Cargo.lock", "version = \"0.2.3\"\n", "")
        }),
        ("a removed plugin.json version line", |d| {
            edit(d, PLUGIN_JSON, "  \"version\": \"0.5.0\",\n", "")
        }),
        ("a duplicate Cargo.toml version line", |d| {
            edit(
                d,
                "Cargo.toml",
                "version = \"0.2.3\"\n",
                "version = \"0.2.3\"\nversion = \"0.2.3\"\n",
            )
        }),
        ("a duplicate lock version line", |d| {
            edit(
                d,
                "fuzz/Cargo.lock",
                "version = \"0.2.3\"\n",
                "version = \"0.2.3\"\nversion = \"0.2.3\"\n",
            )
        }),
        ("a duplicate plugin.json version line", |d| {
            edit(
                d,
                PLUGIN_JSON,
                "  \"version\": \"0.5.0\",\n",
                "  \"version\": \"0.5.0\",\n  \"version\": \"0.5.1\",\n",
            )
        }),
    ];
    for (what, change) in cases {
        assert_eq!(scope_after(change), "full=true\n", "{what}");
    }
}

#[test]
fn without_a_usable_base_the_full_suite_runs() {
    let (dir, base) = fixture();
    let d = dir.path();
    for (b, h) in [
        ("", base.as_str()),
        (base.as_str(), ""),
        ("0000000000000000000000000000000000000000", base.as_str()),
        ("not-a-commit", base.as_str()),
    ] {
        let out = run_scope(d, b, h);
        assert!(out.status.success(), "{b:?} {h:?}: {out:?}");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "full=true\n",
            "{b:?} {h:?}"
        );
    }
}

// ---- ci/jobs/rust-gate.sh ------------------------------------------------------------------

fn gate(scope: &str, full: &str, shards: &str) -> Output {
    Command::new("sh")
        .arg(repo::path(GATE))
        .args(["rust-test-linux", scope, full, shards])
        .output()
        .expect("sh ran")
}

#[test]
fn the_gate_passes_when_the_shards_pass_or_were_rightly_skipped() {
    for (full, shards) in [("true", "success"), ("false", "skipped")] {
        let out = gate("success", full, shards);
        assert!(out.status.success(), "{full} {shards}: {out:?}");
    }
}

#[test]
fn the_gate_fails_on_any_other_outcome() {
    for (scope, full, shards) in [
        ("success", "true", "failure"),
        ("success", "true", "cancelled"),
        ("success", "true", "skipped"),
        ("success", "false", "success"),
        ("success", "false", "failure"),
        ("success", "", "skipped"),
        ("failure", "", "skipped"),
        ("cancelled", "true", "success"),
    ] {
        let out = gate(scope, full, shards);
        assert!(!out.status.success(), "{scope} {full} {shards}: {out:?}");
        assert!(!out.stderr.is_empty(), "{scope} {full} {shards}: says why");
    }
}

// ---- ci/jobs/rust-test-linux.sh ------------------------------------------------------------

/// The arguments `scripts/rust-test.sh` hands the container after its own name, `rust-test`,
/// when ci/jobs/rust-test-linux.sh runs with `args` (a stub `docker` records them).
fn linux(args: &[&str]) -> Result<Vec<String>, Output> {
    let dir = tempfile::tempdir().unwrap();
    let argv = dir.path().join("argv");
    executable(
        dir.path(),
        "docker",
        "for a in \"$@\"; do printf '%s\\0' \"$a\"; done > \"$DOCKER_ARGV\"",
    );
    let mut path = vec![dir.path().to_path_buf()];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let out = Command::new("sh")
        .arg(repo::path(LINUX))
        .args(args)
        .env("PATH", std::env::join_paths(path).unwrap())
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
fn the_rest_shards_run_the_workspace_without_the_replay() {
    for (features, extra) in [("plain", &[][..]), ("all", &["--all-features"][..])] {
        let got = linux(&[features, "rest"]).unwrap();
        let mut want = vec!["run", "cargo", "test", "--workspace", "--locked"];
        want.extend_from_slice(extra);
        want.extend_from_slice(&["--", "--exact", "--skip", REPLAY]);
        assert_eq!(got, want, "{features}");
    }
}

#[test]
fn the_replay_shards_run_only_the_replay_with_the_same_features() {
    for (features, extra) in [("plain", &[][..]), ("all", &["--all-features"][..])] {
        let got = linux(&[features, "replay"]).unwrap();
        assert_eq!(got[..3], ["run", "sh", "-c"], "{features}");
        let script = &got[3];
        let mut tail = vec!["rust-test-linux", REPLAY];
        tail.extend_from_slice(extra);
        assert_eq!(got[4..], tail, "{features}");
        // The whole workspace is selected, so --all-features unifies every member's features
        // exactly as in the full run; a filter that matches no test fails instead of passing.
        assert!(
            script.contains(
                r#"cargo test --workspace --locked "$@" --test golden -- --list --exact "$replay""#
            ),
            "{script}"
        );
        assert!(
            script.contains(
                r#"exec cargo test --workspace --locked "$@" --test golden -- --exact "$replay""#
            ),
            "{script}"
        );
    }
}

#[test]
fn the_release_metadata_run_is_repo_tests_with_both_lock_tests() {
    let got = linux(&["release-metadata"]).unwrap();
    assert_eq!(
        got,
        ["run", "cargo", "test", "--locked", "-p", "repo-tests"]
    );
    for t in ["anthology_lock", "atl_lock"] {
        assert!(
            repo::path(&format!("crates/repo-tests/tests/{t}.rs")).is_file(),
            "{t}"
        );
    }
}

#[test]
fn an_unknown_linux_shard_is_refused() {
    for args in [&[][..], &["plain"], &["plain", "x"], &["some", "rest"]] {
        let out = linux(args).unwrap_err();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
}

/// The shards' filter names a test that exists, once, as a Linux-only test of difftest.
#[test]
fn the_replay_test_the_shards_split_on_exists() {
    let golden = repo::read("crates/difftest/tests/golden.rs");
    let at = golden
        .find(&format!("fn {REPLAY}()"))
        .expect("difftest golden.rs defines the replay test");
    assert!(golden[..at].trim_end().ends_with("#[test]"));
    assert_eq!(golden.matches(REPLAY).count(), 1);
    assert!(repo::read(LINUX).contains(&format!("replay={REPLAY}\n")));
}

// ---- ci/jobs/rust-test-macos.sh ------------------------------------------------------------

/// The `cargo` calls ci/jobs/rust-test-macos.sh makes for `shard`, outside CI (stub `cargo` and
/// `rustup`), one line each.
fn macos(shard: &str) -> Result<Vec<String>, Output> {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("calls");
    executable(dir.path(), "cargo", "echo \"cargo $*\" >> \"$CALLS\"");
    executable(dir.path(), "rustup", "echo \"rustup $*\" >> \"$CALLS\"");
    let mut path = vec![dir.path().to_path_buf()];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let out = Command::new("sh")
        .arg(repo::path(MACOS))
        .arg(shard)
        .env("PATH", std::env::join_paths(path).unwrap())
        .env("CALLS", &log)
        .env_remove("CI")
        .output()
        .expect("sh ran");
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
    let plain = macos("plain").unwrap();
    assert_eq!(
        plain,
        [
            "rustup toolchain install",
            "cargo --version",
            "cargo test --workspace --locked",
        ]
    );
    let all = macos("all").unwrap();
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
    assert!(macos("other").is_err());
}

// ---- ci.yml --------------------------------------------------------------------------------

#[test]
fn the_scope_job_reads_the_pull_request_range() {
    let job = job("rust-scope");
    assert_eq!(
        text(&job["outputs"], "full"),
        "${{ steps.scope.outputs.full }}"
    );
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
    let checkout = &yaml::items(&job["steps"])[0];
    assert_eq!(checkout["with"]["fetch-depth"].as_i64(), Some(0));
}

fn check_shards(name: &str, runner: &str, axes: &[(&str, &[&str])], run: &str) {
    let job = job(&format!("{name}-shard"));
    assert_eq!(text(&job, "needs"), "rust-scope", "{name}");
    assert_eq!(
        text(&job, "if"),
        "needs.rust-scope.outputs.full == 'true'",
        "{name}"
    );
    assert_eq!(text(&job, "runs-on"), runner, "{name}");
    assert_eq!(
        job["strategy"]["fail-fast"].as_bool(),
        Some(false),
        "{name}"
    );
    let matrix = &job["strategy"]["matrix"];
    assert_eq!(yaml::keys(matrix).len(), axes.len(), "{name}");
    for (axis, values) in axes {
        assert_eq!(yaml::strings(&matrix[*axis]), *values, "{name} {axis}");
    }
    assert_eq!(runs(&job), [run], "{name}");
}

fn check_gate(name: &str, extra: &[&str]) {
    let job = job(name);
    let needs = yaml::strings(&job["needs"]);
    assert_eq!(needs, ["rust-scope", &format!("{name}-shard")], "{name}");
    assert_eq!(text(&job, "if"), "${{ !cancelled() }}", "{name}");
    let gate_run = format!(r#"ci/jobs/rust-gate.sh {name} "$SCOPE" "$FULL" "$SHARDS""#);
    let mut want = vec![gate_run.as_str()];
    want.extend_from_slice(extra);
    assert_eq!(runs(&job), want, "{name}");
    let step = step_running(&job, &gate_run);
    assert_eq!(
        text(&step["env"], "SCOPE"),
        "${{ needs.rust-scope.result }}"
    );
    assert_eq!(
        text(&step["env"], "FULL"),
        "${{ needs.rust-scope.outputs.full }}"
    );
    assert_eq!(
        text(&step["env"], "SHARDS"),
        &format!("${{{{ needs.{name}-shard.result }}}}")
    );
}

#[test]
fn rust_test_linux_runs_four_shards_and_gates_on_them() {
    check_shards(
        "rust-test-linux",
        "ubuntu-latest",
        &[
            ("features", &["plain", "all"]),
            ("part", &["replay", "rest"]),
        ],
        r#"ci/jobs/rust-test-linux.sh "$FEATURES" "$PART""#,
    );
    check_gate(
        "rust-test-linux",
        &["ci/jobs/rust-test-linux.sh release-metadata"],
    );
    let job = job("rust-test-linux");
    let light = step_running(&job, "ci/jobs/rust-test-linux.sh release-metadata");
    assert_eq!(
        text(light, "if"),
        "needs.rust-scope.outputs.full == 'false'"
    );
}

#[test]
fn rust_test_macos_runs_two_shards_and_gates_on_them() {
    check_shards(
        "rust-test-macos",
        "macos-15",
        &[("features", &["plain", "all"])],
        r#"ci/jobs/rust-test-macos.sh "$FEATURES""#,
    );
    check_gate("rust-test-macos", &[]);
}

#[test]
fn the_shards_get_their_matrix_values_by_env() {
    for (name, axes) in [
        ("rust-test-linux-shard", &["features", "part"][..]),
        ("rust-test-macos-shard", &["features"]),
    ] {
        let job = job(name);
        let step = yaml::items(&job["steps"])
            .iter()
            .find(|s| !text(s, "run").is_empty())
            .unwrap();
        for axis in axes {
            assert_eq!(
                text(&step["env"], &axis.to_uppercase()),
                format!("${{{{ matrix.{axis} }}}}"),
                "{name}"
            );
        }
    }
}
