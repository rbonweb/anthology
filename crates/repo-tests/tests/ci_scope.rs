//! `ci/jobs/rust-scope.sh` on a pull request: from the changed files and the workspace's
//! dependency graph it decides which Rust tests run (the changed crates and every crate that
//! depends on them), which golden surfaces replay, whether atl, the fuzz targets and macOS run,
//! and how the Linux and macOS shards split, and which repository jobs (ledger, shims-distros,
//! cli-syntax, startup-budget, lint and the rest of `ALL_JOBS`) run. Anything it cannot place runs
//! everything.
//!
//! Each case builds a throwaway git repository holding a small workspace with the real crate
//! names (so the real `ci/scope-rules.tsv` and `ci/test-weights.tsv` apply), commits a change on
//! top of a base commit, and reads the script's `key=value` lines. A few tests then check the
//! rules, the weights and the graph helper against the real repository.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use anthology_pycompat::json::{self, Value};
use repo_tests::repo;
use tempfile::TempDir;

const SCOPE: &str = "ci/jobs/rust-scope.sh";
const GRAPH: &str = "ci/jobs/rust-graph.mjs";
const RULES: &str = "ci/scope-rules.tsv";
const WEIGHTS: &str = "ci/test-weights.tsv";

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

/// A crate of the fixture workspace: its name, its dependencies and its dev-dependencies.
fn manifest(name: &str, deps: &[&str], dev: &[&str]) -> String {
    let table = |header: &str, names: &[&str]| {
        let mut t = format!("\n[{header}]\n");
        for n in names {
            t.push_str(&format!("{n} = {{ path = \"../{n}\" }}\n"));
        }
        t
    };
    format!(
        "[package]\nname = \"{name}\"\nversion.workspace = true\nedition.workspace = true\n{}{}",
        table("dependencies", deps),
        table("dev-dependencies", dev)
    )
}

/// The fixture's crates, leaves first: (name, dependencies, dev-dependencies). It is the real
/// repository in miniature: the `shell` crate is atl's one dependency, and the fuzz targets use
/// pycompat, shell, guard and memory.
const CRATES: [(&str, &[&str], &[&str]); 8] = [
    ("anthology-pycompat", &[], &[]),
    ("anthology-shell", &[], &[]),
    (
        "anthology-guard",
        &["anthology-pycompat", "anthology-shell"],
        &[],
    ),
    ("anthology-memory", &["anthology-guard"], &[]),
    (
        "anthology-cli",
        &["anthology-guard", "anthology-memory"],
        &[],
    ),
    ("anthology-dev", &["anthology-pycompat"], &[]),
    (
        "difftest",
        &[
            "anthology-pycompat",
            "anthology-shell",
            "anthology-guard",
            "anthology-memory",
            "anthology-dev",
        ],
        &[],
    ),
    ("repo-tests", &["anthology-dev"], &["anthology-guard"]),
];

/// The golden files of the fixture, with the crate each lives in.
const GOLDENS: [(&str, &str); 5] = [
    ("anthology-pycompat", "pycompat.json"),
    ("anthology-shell", "shell.analyze"),
    ("anthology-guard", "guard.check"),
    ("anthology-memory", "mem"),
    ("anthology-dev", "dev.vendor"),
];

const PLUGIN_JSON: &str = "plugins/anthology/.claude-plugin/plugin.json";

/// A lock with a workspace package (its version moves on a release) and a registry one.
const LOCK: &str = "version = 4\n\n\
[[package]]\nname = \"anthology-cli\"\nversion = \"0.2.3\"\ndependencies = [\n \"regex\",\n]\n\n\
[[package]]\nname = \"regex\"\nversion = \"1.13.2\"\n\
source = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"aaaa\"\n";

/// A repository at a base commit: the workspace, atl and fuzz manifests, the goldens, and some
/// plugin, docs and release-metadata files.
fn fixture() -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    git(d, &["init", "-q", "-b", "main"]);
    put(
        d,
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n\
         exclude = [\"atl\", \"fuzz\", \"crates/loose\"]\n\n\
         [workspace.package]\nversion = \"0.2.3\"\nedition = \"2024\"\n",
    );
    put(
        d,
        "crates/loose/Cargo.toml",
        "[package]\nname = \"loose\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    put(d, "crates/loose/src/lib.rs", "");
    put(d, "Cargo.lock", LOCK);
    for (name, deps, dev) in CRATES {
        put(
            d,
            &format!("crates/{name}/Cargo.toml"),
            &manifest(name, deps, dev),
        );
        put(d, &format!("crates/{name}/src/lib.rs"), "");
    }
    for (krate, surface) in GOLDENS {
        put(
            d,
            &format!("crates/{krate}/tests/golden/{surface}.jsonl.gz"),
            "gz",
        );
    }
    // A seed corpus is not a golden.
    put(
        d,
        "crates/anthology-pycompat/tests/golden/json.jsonl",
        "{}\n",
    );
    put(
        d,
        "atl/Cargo.toml",
        "[package]\nname = \"atl\"\nversion = \"1.0.0\"\n\n[dependencies]\n\
         anthology-shell = { path = \"../crates/anthology-shell\" }\n",
    );
    put(d, "atl/src/main.rs", "fn main() {}\n");
    put(
        d,
        "fuzz/Cargo.toml",
        "[package]\nname = \"anthology-fuzz\"\nversion = \"0.0.0\"\n\n[dependencies]\n\
         anthology-pycompat = { path = \"../crates/anthology-pycompat\" }\n\
         anthology-shell = { path = \"../crates/anthology-shell\" }\n\
         anthology-guard = { path = \"../crates/anthology-guard\" }\n\
         anthology-memory = { path = \"../crates/anthology-memory\" }\n",
    );
    put(d, "fuzz/Cargo.lock", LOCK);
    put(d, "fuzz/fuzz_targets/a.rs", "");
    put(
        d,
        PLUGIN_JSON,
        "{\n  \"name\": \"anthology\",\n  \"version\": \"0.5.0\",\n  \"description\": \"d\"\n}\n",
    );
    put(d, "plugins/anthology/bin/anthology", "#!/bin/sh\n");
    put(d, "plugins/anthology/agents/scout.md", "scout\n");
    put(d, "plugins/anthology/hooks/mods/index.ts", "export {};\n");
    put(d, "docs/setup.md", "setup\n");
    put(d, "README.md", "readme\n");
    put(d, "CHANGELOG.md", "# Changelog\n");
    put(d, ".claude/memory/note.md", "note\n");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "base"]);
    let base = git(d, &["rev-parse", "HEAD"]);
    (dir, base)
}

/// What `ci/jobs/rust-scope.sh` printed.
struct Scope(BTreeMap<String, String>);

impl Scope {
    fn get(&self, key: &str) -> &str {
        self.0
            .get(key)
            .unwrap_or_else(|| panic!("no {key} line in {:?}", self.0))
    }

    fn flag(&self, key: &str) -> bool {
        match self.get(key) {
            "true" => true,
            "false" => false,
            other => panic!("{key}={other}"),
        }
    }

    /// A space or comma separated list.
    fn list(&self, key: &str) -> Vec<&str> {
        self.get(key)
            .split([' ', ','])
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// The shards of `linux_matrix` or `macos_matrix`.
    fn shards(&self, key: &str) -> Vec<Value> {
        let text = self.get(key);
        if text.is_empty() {
            return Vec::new();
        }
        let v = json::loads(text).unwrap_or_else(|e| panic!("{key}: {}", e.py_str(text)));
        v.as_object()
            .and_then(|o| o.get("include"))
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("{key} has no include list: {text}"))
            .clone()
    }
}

fn field<'a>(shard: &'a Value, key: &str) -> &'a str {
    shard
        .as_object()
        .and_then(|o| o.get(key))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("shard has no {key}"))
}

fn run_scope(dir: &Path, base: &str, head: &str, labels: &str) -> Output {
    Command::new("sh")
        .arg(repo::path(SCOPE))
        .args([base, head])
        .env("PR_LABELS", labels)
        .current_dir(dir)
        .output()
        .expect("sh ran")
}

fn parse(out: &Output) -> Scope {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    Scope(
        text.lines()
            .map(|l| {
                let (k, v) = l.split_once('=').unwrap_or_else(|| panic!("line {l:?}"));
                (k.to_string(), v.to_string())
            })
            .collect(),
    )
}

/// The script's answer for a commit that `change` makes on top of the fixture's base, with the
/// pull request carrying `labels` (a JSON list) and the commit message `message`.
fn scope_with(change: impl FnOnce(&Path), labels: &str, message: &str) -> Scope {
    let (dir, base) = fixture();
    let d = dir.path();
    change(d);
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "--allow-empty", "-m", message]);
    let head = git(d, &["rev-parse", "HEAD"]);
    parse(&run_scope(d, &base, &head, labels))
}

fn scope_after(change: impl FnOnce(&Path)) -> Scope {
    scope_with(change, "[]", "head")
}

/// Touches `rel` (it exists in the fixture, or is created).
fn touch(d: &Path, rel: &str) {
    let p = d.join(rel);
    let mut body = fs::read_to_string(&p).unwrap_or_default();
    body.push_str("// changed\n");
    put(d, rel, &body);
}

fn edit(dir: &Path, rel: &str, from: &str, to: &str) {
    let p = dir.join(rel);
    let old = fs::read_to_string(&p).unwrap();
    assert!(old.contains(from), "{rel} has no {from:?}");
    fs::write(p, old.replacen(from, to, 1)).unwrap();
}

const ALL_CRATES: [&str; 8] = [
    "anthology-cli",
    "anthology-dev",
    "anthology-guard",
    "anthology-memory",
    "anthology-pycompat",
    "anthology-shell",
    "difftest",
    "repo-tests",
];

// ---- which crates run -----------------------------------------------------------------------

#[test]
fn a_leaf_crate_change_runs_every_crate_that_depends_on_it() {
    // Everything but the shell crate, which does not use pycompat in the fixture.
    let s = scope_after(|d| touch(d, "crates/anthology-pycompat/src/lib.rs"));
    assert!(!s.flag("full"));
    assert_eq!(
        s.list("packages"),
        [
            "anthology-cli",
            "anthology-dev",
            "anthology-guard",
            "anthology-memory",
            "anthology-pycompat",
            "difftest",
            "repo-tests"
        ]
    );
    assert_eq!(
        s.list("surfaces"),
        ["dev.vendor", "guard.check", "mem", "pycompat.json"]
    );
}

#[test]
fn a_middle_crate_change_runs_it_and_its_dependents_only() {
    let s = scope_after(|d| touch(d, "crates/anthology-guard/src/lib.rs"));
    assert_eq!(
        s.list("packages"),
        [
            "anthology-cli",
            "anthology-guard",
            "anthology-memory",
            "difftest",
            "repo-tests"
        ]
    );
    // dev.vendor (anthology-dev) and the shell and pycompat surfaces do not depend on guard.
    assert_eq!(s.list("surfaces"), ["guard.check", "mem"]);
}

#[test]
fn a_crate_nothing_depends_on_runs_alone() {
    let s = scope_after(|d| touch(d, "crates/anthology-cli/tests/x.rs"));
    assert_eq!(s.list("packages"), ["anthology-cli"]);
    assert!(s.list("surfaces").is_empty());
    assert!(!s.flag("atl") && !s.flag("fuzz"));
}

#[test]
fn a_dev_dependency_counts_as_a_dependency() {
    // repo-tests dev-depends on guard, so a guard change reaches it (see the middle crate case);
    // anthology-dev is a normal dependency of repo-tests and difftest.
    let s = scope_after(|d| touch(d, "crates/anthology-dev/src/lib.rs"));
    assert_eq!(
        s.list("packages"),
        ["anthology-dev", "difftest", "repo-tests"]
    );
    assert_eq!(s.list("surfaces"), ["dev.vendor"]);
}

#[test]
fn a_golden_file_change_replays_its_own_surface() {
    let s = scope_after(|d| touch(d, "crates/anthology-memory/tests/golden/mem.jsonl.gz"));
    assert_eq!(
        s.list("packages"),
        ["anthology-cli", "anthology-memory", "difftest"]
    );
    assert_eq!(s.list("surfaces"), ["mem"]);
}

#[test]
fn a_difftest_change_replays_every_surface() {
    let s = scope_after(|d| touch(d, "crates/difftest/src/lib.rs"));
    assert_eq!(s.list("packages"), ["difftest"]);
    assert_eq!(
        s.list("surfaces"),
        [
            "dev.vendor",
            "guard.check",
            "mem",
            "pycompat.json",
            "shell.analyze"
        ]
    );
}

#[test]
fn several_changed_crates_union_their_dependents() {
    let s = scope_after(|d| {
        touch(d, "crates/anthology-cli/src/lib.rs");
        touch(d, "crates/anthology-dev/src/lib.rs");
    });
    assert_eq!(
        s.list("packages"),
        ["anthology-cli", "anthology-dev", "difftest", "repo-tests"]
    );
}

// ---- docs, plugins and release metadata -----------------------------------------------------

#[test]
fn a_docs_only_change_runs_the_repository_checks_and_nothing_else() {
    for file in ["docs/setup.md", "README.md"] {
        let s = scope_after(|d| touch(d, file));
        assert!(!s.flag("full"), "{file}");
        assert_eq!(s.list("packages"), ["repo-tests"], "{file}");
        assert!(s.list("surfaces").is_empty(), "{file}");
        assert!(
            !s.flag("atl") && !s.flag("fuzz") && !s.flag("run_macos"),
            "{file}"
        );
    }
}

#[test]
fn release_metadata_alone_runs_the_repository_checks() {
    let s = scope_after(|d| {
        edit(
            d,
            "Cargo.toml",
            "version = \"0.2.3\"",
            "version = \"0.2.4\"",
        );
        edit(
            d,
            PLUGIN_JSON,
            "\"version\": \"0.5.0\"",
            "\"version\": \"0.5.1\"",
        );
        put(d, "plugins/anthology/anthology.lock", "{}\n");
        put(d, "CHANGELOG.md", "# Changelog\n\n## 0.2.4\n");
        put(d, "changelog.d/anthology/10-pin.md", "- Pin.\n");
        put(d, ".claude/memory/note.md", "changed\n");
        put(d, ".claude/agent-memory/x/y.md", "new\n");
    });
    assert!(!s.flag("full"));
    assert_eq!(s.list("packages"), ["repo-tests"]);
    assert!(s.list("surfaces").is_empty());
    assert!(!s.flag("atl") && !s.flag("fuzz") && !s.flag("run_macos"));
}

#[test]
fn a_plugin_change_runs_the_repository_checks() {
    let s = scope_after(|d| touch(d, "plugins/anthology/hooks/mods/index.ts"));
    assert_eq!(s.list("packages"), ["repo-tests"]);
    assert!(s.list("surfaces").is_empty());
    assert!(!s.flag("run_macos"), "mods are not platform code");
}

#[test]
fn an_agent_prompt_change_replays_the_surfaces_that_print_its_budget() {
    let s = scope_after(|d| touch(d, "plugins/anthology/agents/scout.md"));
    assert_eq!(s.list("packages"), ["repo-tests"]);
    assert_eq!(
        s.list("surfaces"),
        ["dev.startup-budget", "dev.upstream-watch"]
    );
}

// ---- atl and fuzz ---------------------------------------------------------------------------

#[test]
fn an_atl_change_runs_atl_test_and_no_anthology_crate() {
    let s = scope_after(|d| touch(d, "atl/src/main.rs"));
    assert!(!s.flag("full"));
    assert!(s.flag("atl"));
    assert!(!s.flag("fuzz"));
    assert!(s.list("packages").is_empty());
    assert!(s.list("surfaces").is_empty());
    assert!(!s.flag("run_linux") && !s.flag("run_macos"));
    // An empty matrix is still valid JSON, which `fromJSON` in ci.yml needs.
    assert_eq!(s.get("linux_matrix"), "{\"include\":[]}");
    assert_eq!(s.get("macos_matrix"), "{\"include\":[]}");
}

#[test]
fn an_atl_change_beside_an_anthology_crate_runs_both() {
    let s = scope_after(|d| {
        touch(d, "atl/src/main.rs");
        touch(d, "crates/anthology-cli/src/lib.rs");
    });
    assert!(s.flag("atl"));
    assert_eq!(s.list("packages"), ["anthology-cli"]);
    assert!(s.flag("run_linux"));
}

#[test]
fn only_an_atl_change_runs_atl_on_macos() {
    // atl's own tests meet macOS-only path behaviour (/private/var symlinks) that Linux cannot
    // show, but a change to the shell crate atl builds is not an atl change.
    assert!(scope_after(|d| touch(d, "atl/src/main.rs")).flag("atl_macos"));
    for file in [
        "crates/anthology-shell/src/lib.rs",
        "crates/anthology-guard/src/lib.rs",
        "docs/setup.md",
        "scripts/atl-test.sh",
    ] {
        assert!(!scope_after(|d| touch(d, file)).flag("atl_macos"), "{file}");
    }
}

#[test]
fn a_change_to_what_atl_builds_runs_atl_test() {
    // atl depends on the shell crate (and only on it), and the guard does not matter to atl.
    assert!(scope_after(|d| touch(d, "crates/anthology-shell/src/lib.rs")).flag("atl"));
    assert!(!scope_after(|d| touch(d, "crates/anthology-guard/src/lib.rs")).flag("atl"));
    assert!(!scope_after(|d| touch(d, "docs/setup.md")).flag("atl"));
}

#[test]
fn a_change_to_what_the_fuzz_targets_build_runs_the_fuzz_job() {
    // fuzz uses pycompat, shell, guard and memory; cli and dev are not among them.
    for krate in [
        "anthology-pycompat",
        "anthology-shell",
        "anthology-guard",
        "anthology-memory",
    ] {
        let s = scope_after(|d| touch(d, &format!("crates/{krate}/src/lib.rs")));
        assert!(s.flag("fuzz"), "{krate}");
    }
    for krate in ["anthology-cli", "anthology-dev", "difftest"] {
        let s = scope_after(|d| touch(d, &format!("crates/{krate}/src/lib.rs")));
        assert!(!s.flag("fuzz"), "{krate}");
    }
    assert!(scope_after(|d| touch(d, "fuzz/fuzz_targets/a.rs")).flag("fuzz"));
    assert!(!scope_after(|d| touch(d, "docs/setup.md")).flag("fuzz"));
}

// ---- the repository jobs --------------------------------------------------------------------

/// Every job of ci.yml that rust-scope.sh switches on its `jobs` output. `gitleaks`, `deny` and
/// `shellcheck` are not here: they run on every pull request.
const ALL_JOBS: [&str; 11] = [
    "cli-syntax",
    "ledger",
    "lock-fresh",
    "memory-diff",
    "mods",
    "plugin-validate",
    "rust-lint",
    "shims-distros",
    "startup-budget",
    "vendor-check",
    "version-bump",
];

/// The `jobs` a commit that changes `file` (created when the fixture lacks it) switches on.
fn jobs_for(file: &str) -> Vec<String> {
    let s = scope_after(|d| {
        if file.ends_with("Cargo.toml") {
            // a manifest must stay valid TOML for cargo metadata
            edit(
                d,
                file,
                "edition.workspace = true\n",
                "edition.workspace = true\n# changed\n",
            );
        } else {
            touch(d, file);
        }
    });
    assert!(!s.flag("full"), "{file}");
    s.list("jobs").into_iter().map(str::to_string).collect()
}

/// Sorted job names.
fn jobs(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

#[test]
fn a_change_nothing_reads_switches_no_repository_job_on() {
    // Docs, scripts, evals, release notes and the divergence files are read by the repository
    // checks (repo-tests) only, and by the jobs that run on every pull request (shellcheck).
    for file in [
        "docs/setup.md",
        "README.md",
        "CHANGELOG.md",
        "changelog.d/anthology/10-pin.md",
        "evals/cases/x.md",
        "scripts/release.sh",
        "port/difftest/x.tsv",
        ".github/workflows/evals.yml",
        "ci/images/shellcheck.Dockerfile",
        "ci/images/gitleaks.Dockerfile",
        "deny.toml",
        ".gitleaks.toml",
    ] {
        assert_eq!(jobs_for(file), jobs(&[]), "{file}");
    }
}

#[test]
fn memory_notes_switch_on_only_the_memory_listing() {
    for file in [".claude/memory/note.md", ".claude/agent-memory/x/y.md"] {
        assert_eq!(jobs_for(file), jobs(&["memory-diff"]), "{file}");
    }
}

#[test]
fn a_version_only_cargo_edit_switches_on_only_the_lock_check() {
    let bump = |d: &Path| {
        edit(
            d,
            "Cargo.toml",
            "version = \"0.2.3\"",
            "version = \"0.2.4\"",
        )
    };
    let s = scope_after(bump);
    assert!(!s.flag("full"));
    assert_eq!(s.list("jobs"), ["lock-fresh"]);
    let s = scope_after(|d| {
        edit(
            d,
            "Cargo.lock",
            "version = \"0.2.3\"",
            "version = \"0.2.4\"",
        )
    });
    assert_eq!(s.list("jobs"), ["lock-fresh"]);
    // The fuzz lock is no part of the shipped set.
    let s = scope_after(|d| {
        edit(
            d,
            "fuzz/Cargo.lock",
            "version = \"0.2.3\"",
            "version = \"0.2.4\"",
        )
    });
    assert!(s.list("jobs").is_empty());
}

#[test]
fn a_plugin_change_switches_on_the_jobs_that_read_that_plugin() {
    let cases: [(&str, &[&str]); 9] = [
        (
            // the mods are the anthology plugin's, and the shim job validates it too
            "plugins/anthology/hooks/mods/index.ts",
            &[
                "cli-syntax",
                "mods",
                "plugin-validate",
                "shims-distros",
                "version-bump",
            ],
        ),
        (
            PLUGIN_JSON,
            &[
                "cli-syntax",
                "mods",
                "plugin-validate",
                "shims-distros",
                "version-bump",
            ],
        ),
        (
            "plugins/anthology-guardrails/hooks/guard.sh",
            &[
                "cli-syntax",
                "plugin-validate",
                "shims-distros",
                "version-bump",
            ],
        ),
        (
            "plugins/anthology-superpowers/skills/x/SKILL.md",
            &[
                "cli-syntax",
                "plugin-validate",
                "startup-budget",
                "vendor-check",
                "version-bump",
            ],
        ),
        (
            "plugins/anthology/agents/scout.md",
            &[
                "cli-syntax",
                "mods",
                "plugin-validate",
                "shims-distros",
                "startup-budget",
                "version-bump",
            ],
        ),
        (
            "plugins/anthology/templates/routing-guide.md",
            &[
                "cli-syntax",
                "mods",
                "plugin-validate",
                "shims-distros",
                "startup-budget",
                "version-bump",
            ],
        ),
        (
            "plugins/anthology/knowledge/caveman/SKILL.md",
            &[
                "cli-syntax",
                "mods",
                "plugin-validate",
                "shims-distros",
                "startup-budget",
                "vendor-check",
                "version-bump",
            ],
        ),
        (
            "plugins/anthology/anthology.lock",
            &[
                "cli-syntax",
                "lock-fresh",
                "mods",
                "plugin-validate",
                "shims-distros",
                "version-bump",
            ],
        ),
        // the marketplace manifest is validated as the `.` target
        (".claude-plugin/marketplace.json", &["plugin-validate"]),
    ];
    for (file, expected) in cases {
        assert_eq!(jobs_for(file), jobs(expected), "{file}");
    }
}

#[test]
fn a_crate_change_switches_on_the_jobs_that_build_that_crate() {
    // Every crate: the test listing (ledger), lint, and the shipped-set check.
    let always = ["ledger", "lock-fresh", "rust-lint"];
    let with = |more: &[&str]| -> Vec<String> {
        let mut v: Vec<&str> = always.to_vec();
        v.extend_from_slice(more);
        jobs(&v)
    };
    // The shim tests build pycompat (and core and the testkit, which the fixture lacks); the
    // smoke, vendor, version, memory and budget commands run anthology-dev, which uses pycompat.
    assert_eq!(
        jobs_for("crates/anthology-pycompat/src/lib.rs"),
        with(&[
            "cli-syntax",
            "memory-diff",
            "shims-distros",
            "startup-budget",
            "vendor-check",
            "version-bump",
        ])
    );
    // repo-tests links anthology-dev, anthology-memory and anthology-guard too (musl build).
    assert_eq!(
        jobs_for("crates/anthology-dev/src/lib.rs"),
        with(&[
            "cli-syntax",
            "memory-diff",
            "shims-distros",
            "startup-budget",
            "vendor-check",
            "version-bump",
        ])
    );
    // guard and memory (shell reaches them): the shim build, nothing else of the dev tool, since
    // the fixture's anthology-dev uses only pycompat.
    for krate in ["anthology-guard", "anthology-memory", "anthology-shell"] {
        assert_eq!(
            jobs_for(&format!("crates/{krate}/src/lib.rs")),
            with(&["shims-distros"]),
            "{krate}"
        );
    }
    // Nothing the shim tests link, and the fixture's anthology-dev does not use them.
    for krate in ["anthology-cli", "difftest"] {
        assert_eq!(
            jobs_for(&format!("crates/{krate}/src/lib.rs")),
            with(&[]),
            "{krate}"
        );
    }
}

#[test]
fn a_change_to_the_repository_checks_switches_on_their_jobs_by_file() {
    // repo-tests holds the shim tests (tests/shims*, and src for all of them) and the roster
    // budget (tests/roster.rs); its other tests are read by the ledger and lint alone.
    let base = ["ledger", "lock-fresh", "rust-lint"];
    let with = |more: &[&str]| -> Vec<String> {
        let mut v: Vec<&str> = base.to_vec();
        v.extend_from_slice(more);
        jobs(&v)
    };
    assert_eq!(jobs_for("crates/repo-tests/tests/docs.rs"), with(&[]));
    assert_eq!(
        jobs_for("crates/repo-tests/tests/shims/offline.rs"),
        with(&["shims-distros"])
    );
    assert_eq!(
        jobs_for("crates/repo-tests/tests/shims.rs"),
        with(&["shims-distros"])
    );
    assert_eq!(
        jobs_for("crates/repo-tests/tests/roster.rs"),
        with(&["startup-budget"])
    );
    assert_eq!(
        jobs_for("crates/repo-tests/src/repo.rs"),
        with(&["shims-distros", "startup-budget"])
    );
    assert_eq!(
        jobs_for("crates/repo-tests/Cargo.toml"),
        with(&["shims-distros", "startup-budget"])
    );
}

#[test]
fn a_shim_script_switches_on_the_shim_tests_and_the_plugin_jobs() {
    let expected = jobs(&[
        "cli-syntax",
        "mods",
        "plugin-validate",
        "shims-distros",
        "version-bump",
    ]);
    assert_eq!(jobs_for("plugins/anthology/bin/anthology"), expected);
    assert_eq!(jobs_for("plugins/anthology/hooks/lib.sh"), expected);
    assert_eq!(
        jobs_for("plugins/anthology-guardrails/hooks/guard.sh"),
        jobs(&[
            "cli-syntax",
            "plugin-validate",
            "shims-distros",
            "version-bump"
        ])
    );
}

#[test]
fn the_vendoring_and_dev_tool_files_switch_on_their_jobs() {
    assert_eq!(jobs_for("vendor/sources.json"), jobs(&["vendor-check"]));
    // atl is a vendored tree too (vendor/sources.json: managed by subtree).
    assert_eq!(jobs_for("atl/src/main.rs"), jobs(&["vendor-check"]));
    // scripts/dev.sh runs anthology-dev for these five.
    assert_eq!(
        jobs_for("scripts/dev.sh"),
        jobs(&[
            "lock-fresh",
            "memory-diff",
            "startup-budget",
            "vendor-check",
            "version-bump",
        ])
    );
    // Ledger tables.
    assert_eq!(jobs_for("port/ledger/shims.tsv"), jobs(&["ledger"]));
    assert_eq!(jobs_for("port/python-test-ids.txt"), jobs(&["ledger"]));
    assert_eq!(jobs_for("port/regex-resolved.tsv"), jobs(&["ledger"]));
    // fuzz targets are linted.
    assert_eq!(jobs_for("fuzz/fuzz_targets/a.rs"), jobs(&["rust-lint"]));
}

#[test]
fn the_pinned_claude_code_and_the_job_images_switch_on_the_jobs_that_use_them() {
    assert_eq!(
        jobs_for("ci/claude-code/package.json"),
        jobs(&["cli-syntax", "mods", "plugin-validate", "shims-distros"])
    );
    assert_eq!(jobs_for("ci/images/node.Dockerfile"), jobs(&["mods"]));
    for image in ["alpine", "debian-slim", "ubuntu"] {
        assert_eq!(
            jobs_for(&format!("ci/images/{image}.Dockerfile")),
            jobs(&["shims-distros"]),
            "{image}"
        );
    }
}

#[test]
fn a_job_script_switches_its_own_job_on() {
    for (file, job) in [
        ("ci/jobs/shims-distros.sh", "shims-distros"),
        ("ci/jobs/ledger.sh", "ledger"),
        ("ci/jobs/cli-syntax.sh", "cli-syntax"),
        ("ci/jobs/mods.sh", "mods"),
        ("ci/jobs/lock-fresh.sh", "lock-fresh"),
    ] {
        assert_eq!(jobs_for(file), jobs(&[job]), "{file}");
    }
}

#[test]
fn the_security_jobs_and_the_gates_are_not_scoped() {
    // gitleaks, deny and shellcheck (which checks the guard's shell scripts) run on every pull
    // request, so ci.yml names no scope output for them.
    for name in ["gitleaks", "deny", "shellcheck"] {
        let job = repo_tests::yaml::load_file(".github/workflows/ci.yml")["jobs"][name].clone();
        assert!(!job.is_badvalue(), "{name}");
        assert!(job["if"].is_badvalue(), "{name} has an if");
        assert!(job["needs"].is_badvalue(), "{name} has needs");
    }
}

#[test]
fn every_scoped_job_in_ci_yml_reads_the_jobs_output_by_its_own_name() {
    let ci = repo_tests::yaml::load_file(".github/workflows/ci.yml");
    for name in ALL_JOBS {
        let job = &ci["jobs"][name];
        assert!(!job.is_badvalue(), "ci.yml has no job {name}");
        assert_eq!(job["needs"].as_str(), Some("rust-scope"), "{name}");
        let cond = job["if"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} has no if"));
        assert!(
            cond.contains(&format!(
                "contains(needs.rust-scope.outputs.jobs, ',{name},')"
            )),
            "{name}: {cond}"
        );
    }
    // And no other job reads the output, so a name that is not in ALL_JOBS cannot hide there.
    let text = repo::read(".github/workflows/ci.yml");
    assert_eq!(
        text.matches("needs.rust-scope.outputs.jobs").count(),
        ALL_JOBS.len(),
        "a job reads the jobs output that is not in ALL_JOBS"
    );
}

// ---- macOS ----------------------------------------------------------------------------------

#[test]
fn only_platform_sensitive_paths_run_macos_on_a_pull_request() {
    for (file, macos) in [
        ("crates/anthology-guard/src/lib.rs", true),
        ("plugins/anthology/bin/anthology", true),
        ("crates/anthology-cli/src/lib.rs", false),
        ("crates/anthology-memory/src/lib.rs", false),
        ("docs/setup.md", false),
        ("plugins/anthology/hooks/mods/index.ts", false),
    ] {
        let s = scope_after(|d| touch(d, file));
        assert_eq!(s.flag("run_macos"), macos, "{file}");
        assert_eq!(s.shards("macos_matrix").is_empty(), !macos, "{file}");
    }
}

#[test]
fn the_macos_shards_run_the_affected_crates_with_both_feature_sets() {
    let s = scope_after(|d| touch(d, "crates/anthology-guard/src/lib.rs"));
    let shards = s.shards("macos_matrix");
    let features: Vec<&str> = shards.iter().map(|x| field(x, "features")).collect();
    assert_eq!(features, ["plain", "all"]);
    for shard in &shards {
        // repo-tests stays: some of its tests run shell and node under the macOS runner.
        assert_eq!(
            field(shard, "packages"),
            "anthology-cli anthology-guard anthology-memory difftest repo-tests"
        );
    }
}

#[test]
fn macos_always_runs_repo_tests_when_it_runs_at_all() {
    // Every path that fires a macos rule: repo-tests (its host_predicate, session-start and
    // shim tests run hooks under macOS bash 3.2) is among the crates of each shard.
    for file in [
        "crates/anthology-guard/src/lib.rs",
        "crates/anthology-pycompat/src/pyerr.rs",
        "plugins/anthology/bin/anthology",
        "plugins/anthology/hooks/lib.sh",
        "plugins/anthology-guardrails/hooks/guard.sh",
        "plugins/anthology-superpowers/hooks/session-start",
        "plugins/anthology/tests/mods/delegation.test.ts",
        "crates/repo-tests/tests/shims/offline.rs",
    ] {
        let s = scope_after(|d| touch(d, file));
        assert!(s.flag("run_macos"), "{file}");
        for shard in s.shards("macos_matrix") {
            assert!(
                field(&shard, "packages")
                    .split(' ')
                    .any(|p| p == "repo-tests"),
                "{file}: {}",
                field(&shard, "packages")
            );
        }
    }
}

// ---- the Linux shards -----------------------------------------------------------------------

/// The (features, part, packages, surfaces) of every Linux shard.
fn linux_shards(s: &Scope) -> Vec<(String, String, String, String)> {
    s.shards("linux_matrix")
        .iter()
        .map(|x| {
            (
                field(x, "features").to_string(),
                field(x, "part").to_string(),
                field(x, "packages").to_string(),
                field(x, "surfaces").to_string(),
            )
        })
        .collect()
}

#[test]
fn each_feature_set_gets_rest_shards_that_together_cover_the_affected_crates() {
    let s = scope_after(|d| touch(d, "crates/anthology-guard/src/lib.rs"));
    for features in ["plain", "all"] {
        let mut crates: Vec<String> = Vec::new();
        for (f, part, packages, _) in linux_shards(&s) {
            if f == features && part == "rest" {
                assert!(!packages.is_empty());
                crates.extend(packages.split(' ').map(str::to_string));
            }
        }
        crates.sort();
        assert_eq!(
            crates,
            [
                "anthology-cli",
                "anthology-guard",
                "anthology-memory",
                "difftest",
                "repo-tests"
            ],
            "{features}: every crate once"
        );
    }
}

#[test]
fn each_feature_set_replays_the_affected_surfaces_in_one_shard() {
    let s = scope_after(|d| touch(d, "crates/anthology-guard/src/lib.rs"));
    for features in ["plain", "all"] {
        let replays: Vec<_> = linux_shards(&s)
            .into_iter()
            .filter(|(f, part, ..)| f == features && part == "replay")
            .collect();
        assert_eq!(replays.len(), 1, "{features}");
        assert_eq!(replays[0].3, "guard.check,mem", "{features}");
        assert_eq!(replays[0].2, "", "a replay shard names no crates");
    }
}

#[test]
fn a_small_change_is_one_rest_shard_per_feature_set_and_no_replay_without_surfaces() {
    let s = scope_after(|d| touch(d, "crates/anthology-cli/src/lib.rs"));
    assert_eq!(
        linux_shards(&s),
        [
            (
                "plain".into(),
                "rest".into(),
                "anthology-cli".into(),
                String::new()
            ),
            (
                "all".into(),
                "rest".into(),
                "anthology-cli".into(),
                String::new()
            ),
        ]
    );
}

#[test]
fn a_prompt_change_is_one_rest_shard_and_one_replay_shard_per_feature_set() {
    // The repository checks, and the replay of dev.startup-budget and dev.upstream-watch.
    let s = scope_after(|d| touch(d, "plugins/anthology/agents/scout.md"));
    let parts: Vec<String> = linux_shards(&s)
        .into_iter()
        .map(|(f, p, ..)| format!("{f}-{p}"))
        .collect();
    assert_eq!(
        parts,
        ["plain-rest", "plain-replay", "all-rest", "all-replay"]
    );
}

#[test]
fn every_shard_has_a_name_that_says_what_it_runs() {
    let s = scope_after(|d| touch(d, "crates/anthology-pycompat/src/lib.rs"));
    let names: Vec<String> = s
        .shards("linux_matrix")
        .iter()
        .map(|x| field(x, "name").to_string())
        .collect();
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(names.len(), unique.len(), "{names:?}");
    assert!(
        names
            .iter()
            .all(|n| n.starts_with("plain-") || n.starts_with("all-")),
        "{names:?}"
    );
}

// ---- running everything ---------------------------------------------------------------------

/// Everything runs: every crate, every surface, atl, fuzz, and macOS with the whole workspace
/// (an empty `packages` in its shards).
fn assert_full(s: &Scope, what: &str) {
    assert!(s.flag("full"), "{what}");
    assert_eq!(s.list("packages"), ALL_CRATES, "{what}");
    let every_surface = [
        "dev.vendor",
        "guard.check",
        "mem",
        "pycompat.json",
        "shell.analyze",
    ];
    assert_eq!(s.list("surfaces"), every_surface, "{what}");
    assert_eq!(s.list("jobs"), ALL_JOBS, "{what}: every repository job");
    assert!(
        s.flag("atl") && s.flag("atl_macos") && s.flag("fuzz"),
        "{what}"
    );
    assert!(s.flag("run_linux") && s.flag("run_macos"), "{what}");
    for features in ["plain", "all"] {
        let mut crates: Vec<String> = Vec::new();
        let mut replayed: Vec<String> = Vec::new();
        for (f, part, packages, surfaces) in linux_shards(s) {
            if f != features {
                continue;
            }
            match part.as_str() {
                "rest" => crates.extend(packages.split(' ').map(str::to_string)),
                "replay" => replayed.extend(surfaces.split(',').map(str::to_string)),
                other => panic!("{what}: part {other}"),
            }
        }
        crates.sort();
        replayed.sort();
        assert_eq!(crates, ALL_CRATES, "{what}: {features}: every crate once");
        assert_eq!(
            replayed, every_surface,
            "{what}: {features}: every golden once"
        );
    }
    let macos = s.shards("macos_matrix");
    assert_eq!(macos.len(), 2, "{what}");
    assert!(
        macos.iter().all(|m| field(m, "packages").is_empty()),
        "{what}"
    );
}

#[test]
fn the_escape_hatches_run_everything() {
    assert_full(
        &scope_with(
            |d| touch(d, "docs/setup.md"),
            "[\"bug\",\"full-ci\"]",
            "head",
        ),
        "the full-ci label",
    );
    assert_full(
        &scope_with(|d| touch(d, "docs/setup.md"), "[]", "docs\n\n[full-ci]"),
        "a [full-ci] trailer",
    );
    assert_full(
        &scope_with(|d| touch(d, "docs/setup.md"), "[]", "docs [full-ci]"),
        "[full-ci] in the subject",
    );
}

#[test]
fn other_labels_and_look_alike_messages_do_not() {
    for (labels, message) in [
        ("[\"bug\"]", "docs"),
        ("[\"not-full-ci\"]", "docs"),
        ("", "docs"),
        ("[]", "full-ci"),
        ("[]", "full ci"),
    ] {
        let s = scope_with(|d| touch(d, "docs/setup.md"), labels, message);
        assert!(!s.flag("full"), "{labels} {message}");
    }
}

#[test]
fn only_the_pull_requests_own_commits_can_ask_for_everything() {
    // A [full-ci] in a commit that is already in the base (not in BASE..HEAD) does not count.
    let (dir, _) = fixture();
    let d = dir.path();
    touch(d, "docs/setup.md");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "earlier [full-ci]"]);
    let base = git(d, &["rev-parse", "HEAD"]);
    touch(d, "README.md");
    git(d, &["add", "-A"]);
    git(d, &["commit", "-q", "-m", "readme"]);
    let head = git(d, &["rev-parse", "HEAD"]);
    assert!(!parse(&run_scope(d, &base, &head, "[]")).flag("full"));
}

#[test]
fn workspace_wide_files_run_everything() {
    type Change = (&'static str, fn(&Path));
    let cases: [Change; 8] = [
        ("the CI workflow", |d| touch(d, ".github/workflows/ci.yml")),
        ("a Rust job script", |d| {
            touch(d, "ci/jobs/rust-test-linux.sh")
        }),
        ("the scope rules", |d| touch(d, "ci/scope-rules.tsv")),
        ("the Rust image", |d| touch(d, "ci/images/rust.Dockerfile")),
        ("the toolchain", |d| touch(d, "rust-toolchain.toml")),
        ("the test runner", |d| touch(d, "scripts/rust-test.sh")),
        ("a workspace dependency", |d| {
            edit(
                d,
                "Cargo.toml",
                "edition = \"2024\"\n",
                "edition = \"2024\"\nlicense = \"x\"\n",
            )
        }),
        ("a lock that is not only versions", |d| {
            put(
                d,
                "Cargo.lock",
                "version = 4\n\n[[package]]\nname = \"x\"\nversion = \"1\"\n",
            )
        }),
    ];
    for (what, change) in cases {
        assert_full(&scope_after(change), what);
    }
}

/// A named change to the fixture.
type Case = (&'static str, fn(&Path));

#[test]
fn a_version_and_nothing_else_in_a_cargo_file_is_release_metadata() {
    let cases: [Case; 4] = [
        ("Cargo.toml version", |d| {
            edit(
                d,
                "Cargo.toml",
                "version = \"0.2.3\"",
                "version = \"0.2.4\"",
            )
        }),
        ("lock package version", |d| {
            edit(
                d,
                "Cargo.lock",
                "version = \"0.2.3\"",
                "version = \"0.2.4\"",
            )
        }),
        ("fuzz lock package version", |d| {
            edit(
                d,
                "fuzz/Cargo.lock",
                "version = \"0.2.3\"",
                "version = \"0.2.4\"",
            )
        }),
        ("a changelog fragment", |d| {
            put(d, "changelog.d/anthology-guardrails/x.md", "- x\n")
        }),
    ];
    for (what, change) in cases {
        let s = scope_after(change);
        assert!(!s.flag("full"), "{what}");
        assert_eq!(s.list("packages"), ["repo-tests"], "{what}");
        assert!(!s.flag("fuzz") && !s.flag("atl"), "{what}");
    }
}

#[test]
fn more_than_a_version_in_a_root_cargo_file_runs_everything() {
    let cases: [Case; 7] = [
        ("a version bump with a new Cargo.toml line", |d| {
            edit(
                d,
                "Cargo.toml",
                "version = \"0.2.3\"\n",
                "version = \"0.2.4\"\nlicense = \"x\"\n",
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
                "Cargo.lock",
                "checksum = \"aaaa\"",
                "checksum = \"bbbb\"",
            )
        }),
        // A version line that goes or is added is not a version bump, only a changed one is.
        ("a removed Cargo.toml version line", |d| {
            edit(d, "Cargo.toml", "version = \"0.2.3\"\n", "")
        }),
        ("a removed lock version line", |d| {
            edit(d, "Cargo.lock", "version = \"0.2.3\"\n", "")
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
                "Cargo.lock",
                "version = \"0.2.3\"\n",
                "version = \"0.2.3\"\nversion = \"0.2.3\"\n",
            )
        }),
    ];
    for (what, change) in cases {
        // Some of these leave a manifest cargo cannot read (the crates inherit the version), so
        // only the decision is checked here, not the crate list.
        assert!(scope_after(change).flag("full"), "{what}");
    }
}

#[test]
fn more_than_a_version_in_the_fuzz_lock_runs_the_fuzz_job_not_everything() {
    let cases: [Case; 2] = [
        ("a lock checksum", |d| {
            edit(
                d,
                "fuzz/Cargo.lock",
                "checksum = \"aaaa\"",
                "checksum = \"bbbb\"",
            )
        }),
        ("a duplicate version line", |d| {
            edit(
                d,
                "fuzz/Cargo.lock",
                "version = \"0.2.3\"\n",
                "version = \"0.2.3\"\nversion = \"0.2.3\"\n",
            )
        }),
    ];
    for (what, change) in cases {
        let s = scope_after(change);
        assert!(!s.flag("full") && s.flag("fuzz"), "{what}");
        assert_eq!(s.list("packages"), ["repo-tests"], "{what}");
    }
}

#[test]
fn a_path_no_rule_places_runs_everything() {
    assert_full(
        &scope_after(|d| touch(d, "brand-new-folder/x")),
        "an unplaced path",
    );
}

#[test]
fn a_crate_folder_that_is_not_a_workspace_member_runs_everything() {
    // crates/loose is excluded from the workspace, so no member lives there.
    assert_full(
        &scope_after(|d| touch(d, "crates/loose/src/lib.rs")),
        "a crate folder the workspace excludes",
    );
}

#[test]
fn a_workspace_cargo_cannot_read_runs_the_whole_workspace_and_every_golden() {
    let s = scope_after(|d| {
        put(d, "crates/anthology-cli/Cargo.toml", "this is not toml [");
        touch(d, "crates/anthology-cli/src/lib.rs");
    });
    assert!(s.flag("full") && s.flag("atl") && s.flag("atl_macos") && s.flag("fuzz"));
    assert!(s.flag("run_linux") && s.flag("run_macos"));
    // No crate list: each `rest` shard runs the workspace, and the surfaces come from the goldens.
    assert!(s.list("packages").is_empty());
    let rest: Vec<_> = linux_shards(&s)
        .into_iter()
        .filter(|(_, part, ..)| part == "rest")
        .collect();
    assert_eq!(rest.len(), 2, "{rest:?}");
    assert!(rest.iter().all(|(_, _, packages, _)| packages.is_empty()));
    assert_eq!(
        s.list("surfaces"),
        [
            "dev.vendor",
            "guard.check",
            "mem",
            "pycompat.json",
            "shell.analyze"
        ]
    );
}

#[test]
fn without_a_usable_range_everything_runs() {
    let (dir, base) = fixture();
    let d = dir.path();
    for (b, h) in [
        ("", base.as_str()),
        (base.as_str(), ""),
        ("0000000000000000000000000000000000000000", base.as_str()),
        ("not-a-commit", base.as_str()),
        // no change at all
        (base.as_str(), base.as_str()),
    ] {
        assert_full(&parse(&run_scope(d, b, h, "[]")), &format!("{b:?} {h:?}"));
    }
}

// ---- the helper, the rules and the weights, against the real repository ----------------------

fn repo_metadata() -> TempDir {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--offline",
        ])
        .current_dir(repo::root())
        .output()
        .expect("cargo ran");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("metadata.json"), out.stdout).unwrap();
    dir
}

fn graph(dir: &TempDir, args: &[&str]) -> Vec<String> {
    let out = Command::new("node")
        .arg(repo::path(GRAPH))
        .arg(dir.path().join("metadata.json"))
        .args(args)
        .output()
        .expect("node ran");
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn node_is_missing() -> bool {
    Command::new("node").arg("--version").output().is_err()
}

#[test]
fn the_graph_helper_reads_the_real_workspace() {
    if node_is_missing() {
        eprintln!("skipped: node is not installed");
        return;
    }
    let meta = repo_metadata();
    let dirs = graph(&meta, &["dirs"]);
    assert!(
        dirs.contains(&"crates/anthology-guard\tanthology-guard".to_string()),
        "{dirs:?}"
    );
    // The build graph of the project map: shell <- guard <- memory, settings, runtime.
    let affected = graph(&meta, &["affected", "anthology-shell"]);
    for krate in [
        "anthology-shell",
        "anthology-guard",
        "anthology-memory",
        "anthology-settings",
        "anthology-runtime",
        "anthology-cli",
        "difftest",
    ] {
        assert!(
            affected.contains(&krate.to_string()),
            "{krate} in {affected:?}"
        );
    }
    assert!(
        !affected.contains(&"anthology-pycompat".to_string()),
        "{affected:?}"
    );
    assert_eq!(
        graph(&meta, &["affected", "anthology-cli"]),
        ["anthology-cli"]
    );
    // What atl builds: the shell crate alone.
    assert_eq!(
        graph(&meta, &["closure", "anthology-shell"]),
        ["anthology-shell"]
    );
    // What a build needs: guard's tests use the testkit, but a fuzz build or atl build does not.
    let guard = graph(&meta, &["closure", "anthology-guard"]);
    assert_eq!(
        guard,
        ["anthology-guard", "anthology-pycompat", "anthology-shell"]
    );
    // ...while the tests of a crate depend on a dev dependency: a testkit change reaches guard.
    assert!(
        graph(&meta, &["affected", "anthology-testkit"]).contains(&"anthology-guard".to_string())
    );
    // A name that is not a member is refused.
    let out = Command::new("node")
        .arg(repo::path(GRAPH))
        .arg(meta.path().join("metadata.json"))
        .args(["affected", "no-such-crate"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

/// The tab-separated rows of a table file, without comments and blank lines.
fn rows(rel: &str) -> Vec<Vec<String>> {
    repo::read(rel)
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').map(str::to_string).collect())
        .collect()
}

/// Shell `case` matching, where `*` matches anything (a `/` too) and nothing else is special.
fn glob(pattern: &str, path: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == path;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !path.starts_with(first)
        || !path[first.len()..].ends_with(last)
        || path.len() < first.len() + last.len()
    {
        return false;
    }
    let mut rest = &path[first.len()..path.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(at) => rest = &rest[at + mid.len()..],
            None => return false,
        }
    }
    true
}

#[test]
fn the_glob_matcher_follows_shell_case_patterns() {
    assert!(glob("docs/*", "docs/a/b.md"));
    assert!(glob("plugins/*/bin/*", "plugins/anthology/bin/atl"));
    assert!(!glob("plugins/*/bin/*", "plugins/anthology/hooks/lib.sh"));
    assert!(glob("ci/jobs/rust-*", "ci/jobs/rust-gate.sh"));
    assert!(glob("rust-toolchain.toml", "rust-toolchain.toml"));
    assert!(!glob("rust-toolchain.toml", "x/rust-toolchain.toml"));
}

/// The files that rust-scope.sh itself places: release metadata, and the Cargo manifests whose
/// versions it reads. They need no rule.
fn placed_by_the_script(path: &str) -> bool {
    path.starts_with("crates/")
        || path.starts_with(".claude/memory/")
        || path.starts_with(".claude/agent-memory/")
        || path.starts_with("changelog.d/")
        || path == "CHANGELOG.md"
        || path == "Cargo.toml"
        || path == "Cargo.lock"
}

#[test]
fn every_tracked_path_is_placed_by_a_rule_or_the_script() {
    let Some(files) = repo::tracked_files() else {
        eprintln!("skipped: git cannot list the files here");
        return;
    };
    let rules = rows(RULES);
    let unplaced: Vec<&String> = files
        .iter()
        .filter(|f| !placed_by_the_script(f) && !rules.iter().any(|r| glob(&r[0], f)))
        .take(10)
        .collect();
    assert!(
        unplaced.is_empty(),
        "no rule in {RULES} matches these (rust-scope.sh would run everything): {unplaced:?}"
    );
}

#[test]
fn every_rule_matches_a_tracked_path_and_names_real_things() {
    let Some(files) = repo::tracked_files() else {
        eprintln!("skipped: git cannot list the files here");
        return;
    };
    let goldens: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let name = f.rsplit('/').next()?;
            f.contains("/tests/golden/")
                .then(|| name.strip_suffix(".jsonl.gz").map(str::to_string))?
        })
        .collect();
    let crates: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix("crates/")?.strip_suffix("/Cargo.toml"))
        .map(|dir| repo::read(&format!("crates/{dir}/Cargo.toml")))
        .filter_map(|t| {
            let line = t.lines().find(|l| l.starts_with("name = "))?;
            Some(
                line.trim_start_matches("name = ")
                    .trim_matches('"')
                    .to_string(),
            )
        })
        .collect();
    assert!(crates.contains(&"repo-tests".to_string()), "{crates:?}");
    for row in rows(RULES) {
        assert_eq!(row.len(), 2, "pattern<TAB>effects: {row:?}");
        // `crate:NAME` is not a path: it names a crate whose build the effects need.
        if let Some(name) = row[0].strip_prefix("crate:") {
            assert!(crates.contains(&name.to_string()), "{row:?}");
            assert!(
                row[1].split(' ').all(|e| e.starts_with("job:")),
                "a crate row only switches jobs on: {row:?}"
            );
        } else {
            assert!(
                files.iter().any(|f| glob(&row[0], f)),
                "no tracked file matches {}",
                row[0]
            );
        }
        for effect in row[1].split(' ') {
            match effect.split_once(':') {
                Some(("pkg", name)) => assert!(crates.contains(&name.to_string()), "{row:?}"),
                Some(("job", name)) => assert!(ALL_JOBS.contains(&name), "{row:?}"),
                Some(("surface", name)) => {
                    assert!(
                        name == "*" || goldens.contains(&name.to_string()),
                        "{row:?}"
                    )
                }
                None => assert!(
                    ["full", "atl", "atl-macos", "fuzz", "macos"].contains(&effect),
                    "{row:?}"
                ),
                _ => panic!("{row:?}"),
            }
        }
    }
}

/// The crates a `crate:NAME` row names for `effect` (`job:NAME`).
fn crate_rows_for(effect: &str) -> BTreeSet<String> {
    rows(RULES)
        .into_iter()
        .filter(|r| r[1].split(' ').any(|e| e == effect))
        .filter_map(|r| r[0].strip_prefix("crate:").map(str::to_string))
        .collect()
}

#[test]
fn the_shims_distros_crate_rows_name_every_workspace_crate_repo_tests_links() {
    // shims-distros builds repo-tests (its dependencies and dev-dependencies, the testkit among
    // them) for musl; a crate it links that has no row would leave a change to it unnoticed.
    let manifest = repo::read("crates/repo-tests/Cargo.toml");
    let linked: BTreeSet<String> = manifest
        .lines()
        .filter_map(|l| l.split_once(".workspace")?.0.strip_prefix("anthology-"))
        .map(|n| format!("anthology-{n}"))
        .collect();
    for krate in [
        "anthology-core",
        "anthology-pycompat",
        "anthology-dev",
        "anthology-memory",
        "anthology-guard",
        "anthology-testkit",
    ] {
        assert!(linked.contains(krate), "{krate} in {linked:?}");
    }
    assert_eq!(
        linked,
        crate_rows_for("job:shims-distros"),
        "crates repo-tests links (left) against the `crate:` rows of shims-distros (right)"
    );
    // repo-tests itself is no row: an edit to any of its tests would run the job.
    assert!(!linked.contains("repo-tests"));
}

#[test]
fn the_startup_budget_and_dev_tool_jobs_name_the_crates_their_commands_run() {
    // `scripts/dev.sh startup-budget` and the smoke, vendor, version-bump, memory-diff and
    // lock-check commands run anthology-dev; the runtime budget tests run anthology-runtime.
    for job in [
        "cli-syntax",
        "vendor-check",
        "version-bump",
        "memory-diff",
        "lock-fresh",
    ] {
        assert!(
            crate_rows_for(&format!("job:{job}")).contains("anthology-dev"),
            "{job}"
        );
    }
    let budget = crate_rows_for("job:startup-budget");
    assert!(budget.contains("anthology-dev") && budget.contains("anthology-runtime"));
}

#[test]
fn the_weights_name_every_workspace_crate_once() {
    let Some(files) = repo::tracked_files() else {
        eprintln!("skipped: git cannot list the files here");
        return;
    };
    let mut crates: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix("crates/")?.strip_suffix("/Cargo.toml"))
        .map(|dir| {
            let t = repo::read(&format!("crates/{dir}/Cargo.toml"));
            let line = t
                .lines()
                .find(|l| l.starts_with("name = "))
                .unwrap()
                .to_string();
            line.trim_start_matches("name = ")
                .trim_matches('"')
                .to_string()
        })
        .collect();
    crates.sort();
    let mut weighed: Vec<String> = Vec::new();
    for row in rows(WEIGHTS) {
        assert_eq!(row.len(), 2, "crate<TAB>seconds: {row:?}");
        assert!(row[1].parse::<u32>().is_ok_and(|n| n > 0), "{row:?}");
        weighed.push(row[0].clone());
    }
    weighed.sort();
    assert_eq!(weighed, crates, "a new crate needs a weight in {WEIGHTS}");
}
