//! The anthology binary's release pipeline (RL1): both plugins' `anthology.lock`, and the shape
//! of `.github/workflows/anthology-release.yml` (design §8.2, §10; GC-17).
//!
//! The lock tests are active once the locks exist: CO1 writes them into `port/staging/plugins/`
//! and CO2 moves them to `plugins/`. Until then they check that neither plugin has one alone.
//!
//! The workflow tests port `tests/test_rtk_release_workflow.py` to the new workflow: every
//! action pinned by SHA, exactly the four targets, no Windows, least privilege with the
//! attestation permissions on the publish job only, the tag checked as a whole string under
//! bash, and the shipped-graph-only test step (m-16).

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use anthology_core::lock::{ANTHOLOGY_TAG_PREFIX, parse};
use repo_tests::repo;
use yaml_rust2::{Yaml, YamlLoader};

const WORKFLOW: &str = ".github/workflows/anthology-release.yml";
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

// ------------------------------------------------------------------------------------------
// anthology.lock

/// Checks the pair of locks under `dir` (relative to the root); `false` when neither exists.
fn check_locks(dir: &str) -> bool {
    let paths = [
        format!("{dir}/anthology/anthology.lock"),
        format!("{dir}/anthology-guardrails/anthology.lock"),
    ];
    let present: Vec<bool> = paths.iter().map(|p| repo::path(p).is_file()).collect();
    if present.iter().all(|p| !p) {
        return false;
    }
    assert!(
        present.iter().all(|p| *p),
        "{dir}: both plugins must ship anthology.lock, or neither: {paths:?}"
    );
    let texts: Vec<String> = paths.iter().map(|p| repo::read(p)).collect();
    assert_eq!(
        texts[0], texts[1],
        "{dir}: the two anthology.lock files differ"
    );
    let lock = parse(&texts[0], ANTHOLOGY_TAG_PREFIX)
        .unwrap_or_else(|e| panic!("{}: anthology.lock is not valid ({e})", paths[0]));
    assert!(lock.tag.starts_with(ANTHOLOGY_TAG_PREFIX));
    assert_eq!(lock.repo, "rbonweb/anthology", "{dir}");
    assert!(
        texts[0].ends_with("}\n"),
        "{dir}: lock write ends the file with a newline"
    );
    true
}

#[test]
fn the_plugin_locks_are_valid_and_identical_once_they_exist() {
    let mut active = Vec::new();
    for dir in ["plugins", "port/staging/plugins"] {
        if check_locks(dir) {
            active.push(dir);
        }
    }
    if active.is_empty() {
        eprintln!("no anthology.lock yet (CO1 writes them); nothing to check");
    }
}

// ------------------------------------------------------------------------------------------
// The workflow

fn workflow_text() -> String {
    repo::read(WORKFLOW)
}

fn workflow() -> Yaml {
    let mut docs = YamlLoader::load_from_str(&workflow_text()).expect("the workflow is YAML");
    assert_eq!(docs.len(), 1);
    docs.remove(0)
}

fn s(y: &Yaml) -> &str {
    y.as_str().unwrap_or_else(|| panic!("not a string: {y:?}"))
}

fn jobs(wf: &Yaml) -> Vec<(String, Yaml)> {
    wf["jobs"]
        .as_hash()
        .expect("jobs")
        .iter()
        .map(|(k, v)| (s(k).to_string(), v.clone()))
        .collect()
}

fn steps(job: &Yaml) -> Vec<Yaml> {
    job["steps"].as_vec().cloned().unwrap_or_default()
}

fn step_named(job: &Yaml, name: &str) -> Yaml {
    steps(job)
        .into_iter()
        .find(|st| st["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("no step {name:?}"))
}

/// A mapping as `key -> scalar text`.
fn map(y: &Yaml) -> BTreeMap<String, String> {
    y.as_hash()
        .map(|h| {
            h.iter()
                .map(|(k, v)| {
                    let v = match v {
                        Yaml::String(t) => t.clone(),
                        Yaml::Boolean(b) => b.to_string(),
                        Yaml::Integer(i) => i.to_string(),
                        other => format!("{other:?}"),
                    };
                    (s(k).to_string(), v)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn list(y: &Yaml) -> Vec<String> {
    match y {
        Yaml::String(t) => vec![t.clone()],
        Yaml::Array(a) => a.iter().map(|v| s(v).to_string()).collect(),
        _ => Vec::new(),
    }
}

/// `owner/repo[/path]@<40 hex>`.
fn pinned(uses: &str) -> bool {
    let Some((action, sha)) = uses.split_once('@') else {
        return false;
    };
    let mut parts = action.split('/');
    let owner_ok = parts.next().is_some_and(|o| {
        !o.is_empty() && o.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c))
    });
    let rest: Vec<&str> = parts.collect();
    owner_ok
        && !rest.is_empty()
        && rest
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c)))
        && sha.len() == 40
        && sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

#[test]
fn every_action_is_pinned_by_sha() {
    let wf = workflow();
    let mut count = 0;
    for (name, job) in jobs(&wf) {
        for st in steps(&job) {
            if let Some(uses) = st["uses"].as_str() {
                assert!(pinned(uses), "{name}: {uses}");
                count += 1;
            }
        }
    }
    assert!(count >= 5, "only {count} actions");
    // Each pin carries its version as a comment, as in rtk-release.yml.
    for line in workflow_text().lines().filter(|l| l.contains("uses: ")) {
        assert!(line.contains(" # v"), "{line}");
    }
    assert!(!pinned("actions/checkout@v7"));
    assert!(!pinned(
        "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b"
    ));
}

#[test]
fn the_targets_are_exactly_the_four() {
    let wf = workflow();
    let build = &wf["jobs"]["build"];
    let matrix = build["strategy"]["matrix"]["include"]
        .as_vec()
        .expect("matrix");
    let got: BTreeSet<&str> = matrix.iter().map(|m| s(&m["target"])).collect();
    assert_eq!(got, TARGETS.iter().copied().collect());
    assert_eq!(matrix.len(), 4);
    assert_eq!(build["strategy"]["fail-fast"].as_bool(), Some(false));
    let os: BTreeMap<&str, &str> = matrix
        .iter()
        .map(|m| (s(&m["target"]), s(&m["os"])))
        .collect();
    assert_eq!(os["aarch64-apple-darwin"], "macos-15");
    assert_eq!(os["x86_64-apple-darwin"], "macos-15-intel");
    assert_eq!(os["x86_64-unknown-linux-musl"], "ubuntu-24.04");
    assert_eq!(os["aarch64-unknown-linux-musl"], "ubuntu-24.04-arm");
}

#[test]
fn no_windows() {
    assert!(!workflow_text().to_lowercase().contains("windows"));
}

#[test]
fn the_publish_job_holds_the_attestation_permissions() {
    let wf = workflow();
    let publish = &wf["jobs"]["publish"];
    let perms = map(&publish["permissions"]);
    assert_eq!(
        perms,
        [
            ("attestations", "write"),
            ("contents", "write"),
            ("id-token", "write")
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    );
    let st = steps(publish);
    let attest = st
        .iter()
        .position(|x| {
            x["uses"]
                .as_str()
                .is_some_and(|u| u.starts_with("actions/attest-build-provenance@"))
        })
        .expect("attestation step");
    assert_eq!(
        s(&st[attest]["with"]["subject-path"]),
        "dist/anthology-*.tar.gz"
    );
    let sums = st
        .iter()
        .position(|x| x["name"].as_str() == Some("Checksums"))
        .unwrap();
    let release = st
        .iter()
        .position(|x| {
            x["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("Create the release"))
        })
        .unwrap();
    assert!(
        sums < attest && attest < release,
        "checksums, then attest, then release"
    );
}

#[test]
fn permissions_are_least_privilege() {
    let wf = workflow();
    assert_eq!(
        wf["permissions"].as_hash().map(|h| h.len()),
        Some(0),
        "permissions: {{}}"
    );
    for (name, job) in jobs(&wf) {
        if name != "publish" {
            assert_eq!(
                map(&job["permissions"]),
                BTreeMap::from([("contents".to_string(), "read".to_string())]),
                "{name}"
            );
        }
    }
}

#[test]
fn the_triggers() {
    let wf = workflow();
    let on = &wf["on"];
    assert_eq!(list(&on["push"]["tags"]), ["anthology-bin--v*"]);
    assert_eq!(
        on["workflow_dispatch"]["inputs"]["tag"]["required"].as_bool(),
        Some(true)
    );
    let paths: BTreeSet<String> = list(&on["pull_request"]["paths"]).into_iter().collect();
    assert_eq!(
        paths,
        [WORKFLOW, "rust-toolchain.toml", "deny.toml"]
            .iter()
            .map(|p| p.to_string())
            .collect()
    );
    assert!(on["push"]["branches"].is_badvalue(), "no branch pushes");
}

fn tag_check(wf: &Yaml) -> String {
    let step = steps(&wf["jobs"]["meta"])
        .into_iter()
        .find(|st| st["id"].as_str() == Some("tag"))
        .expect("the tag step");
    assert_eq!(step["shell"].as_str(), Some("bash"));
    let run = s(&step["run"]).to_string();
    assert!(
        run.contains(r"[[ $tag =~ ^anthology-bin--v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]"),
        "{run}"
    );
    assert!(
        !run.contains("grep"),
        "a line-based grep accepts a multi-line dispatch input"
    );
    run
}

#[test]
fn the_tag_is_validated_as_a_whole_string_under_bash() {
    tag_check(&workflow());
}

#[test]
fn the_tag_check_rejects_multi_line_input() {
    let run = tag_check(&workflow());
    if Command::new("bash").arg("--version").output().is_err() {
        eprintln!("skipped: bash is not installed");
        return;
    }
    let line = run
        .lines()
        .map(str::trim)
        .find(|l| l.contains("$tag =~"))
        .unwrap();
    let cond = line
        .strip_prefix("if ! ")
        .and_then(|l| l.strip_suffix("; then"))
        .unwrap();
    for (tag, ok) in [
        ("anthology-bin--v0.1.0", true),
        ("anthology-bin--v0.1.0-rc.1", true),
        ("anthology-bin--v12.0.3-1.a", true),
        ("anthology-bin--v0.1.0\nanything", false),
        ("evil\nanthology-bin--v0.1.0", false),
        ("anthology-bin--v0.1", false),
        ("anthology-bin--v0.1.0-", false),
        ("anthology-bin--v0.1.0-rc_1", false),
        ("anthology--v0.1.0", false),
        ("rtk--v1.2.1", false),
        ("", false),
    ] {
        let status = Command::new("bash")
            .args(["-c", &format!("tag=$1; {cond}"), "bash", tag])
            .status()
            .unwrap();
        assert_eq!(status.success(), ok, "{tag:?}");
    }
}

#[test]
fn the_version_comes_from_the_workspace_package_table() {
    let wf = workflow();
    let st = steps(&wf["jobs"]["meta"])
        .into_iter()
        .find(|x| x["id"].as_str() == Some("version"))
        .expect("the version step");
    let run = s(&st["run"]);
    assert!(run.contains("[workspace.package]"), "{run}");
    assert!(run.contains("\"anthology-bin--v$version\""), "{run}");
    assert_eq!(st["shell"].as_str(), Some("bash"));
    assert!(run.contains(r"[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]"));
    // The tag's prefix is the lock's (anthology_core::lock).
    assert!(run.contains(ANTHOLOGY_TAG_PREFIX));
}

#[test]
fn every_job_has_a_timeout() {
    for (name, job) in jobs(&workflow()) {
        assert!(job["timeout-minutes"].as_i64().is_some(), "{name}");
    }
}

#[test]
fn no_expression_inside_run_scripts() {
    for (name, job) in jobs(&workflow()) {
        for st in steps(&job) {
            if let Some(run) = st["run"].as_str() {
                assert!(!run.contains("${{"), "{name}: {:?}", st["name"]);
            }
        }
    }
}

#[test]
fn checkouts_do_not_persist_credentials() {
    let mut count = 0;
    for (name, job) in jobs(&workflow()) {
        for st in steps(&job) {
            if st["uses"]
                .as_str()
                .is_some_and(|u| u.starts_with("actions/checkout@"))
            {
                assert_eq!(
                    st["with"]["persist-credentials"].as_bool(),
                    Some(false),
                    "{name}"
                );
                count += 1;
            }
        }
    }
    assert_eq!(count, 3, "meta, deny and build check out");
}

#[test]
fn the_dry_run_stops_before_publishing() {
    let wf = workflow();
    let publish = &wf["jobs"]["publish"];
    assert_eq!(s(&publish["if"]), "needs.meta.outputs.publish == 'true'");
    assert_eq!(list(&publish["needs"]), ["meta", "deny", "build"]);
    assert_eq!(list(&wf["jobs"]["build"]["needs"]), ["meta", "deny"]);
    assert_eq!(list(&wf["jobs"]["deny"]["needs"]), ["meta"]);
    let meta =
        s(&step_named(&wf["jobs"]["meta"], "Resolve and validate the tag")["run"]).to_string();
    assert!(
        meta.contains("printf 'tag=\\nref=\\npublish=false\\n'"),
        "{meta}"
    );
    assert_eq!(
        jobs(&wf).into_iter().map(|j| j.0).collect::<Vec<_>>(),
        ["meta", "deny", "build", "publish"]
    );
}

#[test]
fn cargo_deny_runs_the_pinned_job_script() {
    let wf = workflow();
    let deny = &wf["jobs"]["deny"];
    assert!(
        steps(deny)
            .iter()
            .any(|st| st["run"].as_str().map(str::trim) == Some("ci/jobs/deny.sh"))
    );
    assert!(repo::path("ci/jobs/deny.sh").is_file());
}

/// The workspace packages that `anthology-cli` reaches through normal and build dependencies,
/// read from `crates/*/Cargo.toml`.
fn shipped_workspace_packages() -> BTreeSet<String> {
    let mut deps: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for entry in std::fs::read_dir(repo::path("crates")).unwrap() {
        let manifest = entry.unwrap().path().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let mut name = None;
        let mut section = String::new();
        let mut found = BTreeSet::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).replace(' ', "");
                // `[dependencies.foo]` and `[target.'cfg'.build-dependencies.foo]`.
                for kind in ["dependencies.", "build-dependencies."] {
                    if let Some(at) = section.find(kind) {
                        let head = &section[..at];
                        if (head.is_empty() || head.starts_with("target."))
                            && !head.ends_with("dev-")
                        {
                            found.insert(section[at + kind.len()..].trim_matches('"').to_string());
                        }
                    }
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key
                .trim()
                .split('.')
                .next()
                .unwrap_or("")
                .trim_matches('"')
                .to_string();
            if section == "package" && key == "name" {
                name = Some(value.trim().trim_matches('"').to_string());
            }
            let normal = section == "dependencies"
                || section == "build-dependencies"
                || (section.starts_with("target.")
                    && (section.ends_with(".dependencies")
                        && !section.ends_with(".dev-dependencies")
                        || section.ends_with(".build-dependencies")));
            if normal {
                let renamed = value
                    .split("package")
                    .nth(1)
                    .and_then(|r| r.trim_start().strip_prefix('='))
                    .and_then(|r| r.trim().strip_prefix('"'))
                    .and_then(|r| r.split('"').next())
                    .map(str::to_string);
                found.insert(renamed.unwrap_or(key));
            }
        }
        if let Some(name) = name {
            deps.insert(name, found);
        }
    }
    let members: BTreeSet<String> = deps.keys().cloned().collect();
    let mut shipped = BTreeSet::new();
    let mut todo = vec!["anthology-cli".to_string()];
    while let Some(p) = todo.pop() {
        if shipped.insert(p.clone()) {
            for d in deps.get(&p).into_iter().flatten() {
                if members.contains(d) {
                    todo.push(d.clone());
                }
            }
        }
    }
    shipped
}

#[test]
fn the_test_step_runs_exactly_the_shipped_graph() {
    let wf = workflow();
    let run = s(&step_named(&wf["jobs"]["build"], "Test the shipped crates")["run"]).to_string();
    let words: Vec<&str> = run.split_whitespace().collect();
    assert_eq!(
        &words[..5],
        ["cargo", "test", "--locked", "--target", "\"$TARGET\""]
    );
    let mut packages = BTreeSet::new();
    let mut rest = words[5..].iter();
    while let Some(w) = rest.next() {
        assert_eq!(*w, "-p", "{run}");
        packages.insert(rest.next().expect("-p needs a package").to_string());
    }
    let shipped = shipped_workspace_packages();
    assert_eq!(packages, shipped);
    for not in [
        "anthology-dev",
        "difftest",
        "anthology-testkit",
        "repo-tests",
    ] {
        assert!(!packages.contains(not), "{not}");
    }
    assert!(shipped.contains("anthology-shell") && shipped.contains("anthology-pycompat"));
}

#[test]
fn the_binary_is_built_checked_and_packaged() {
    let wf = workflow();
    let build = &wf["jobs"]["build"];
    assert_eq!(
        s(&step_named(build, "Build")["run"]).trim(),
        "cargo build --release --locked -p anthology-cli --target \"$TARGET\""
    );
    let st = step_named(build, "Check the binary is static (Linux)");
    assert_eq!(s(&st["if"]), "runner.os == 'Linux'");
    let r = s(&st["run"]);
    assert!(
        r.contains("readelf -lW \"$bin\" > \"$RUNNER_TEMP/program-headers.txt\"")
            && r.contains("readelf -dW \"$bin\" > \"$RUNNER_TEMP/dynamic.txt\"")
            && r.contains("if grep INTERP \"$RUNNER_TEMP/program-headers.txt\"; then")
            && r.contains("if grep NEEDED \"$RUNNER_TEMP/dynamic.txt\"; then")
            && !r.contains("| grep")
    );
    let version = step_named(build, "Check --version");
    assert!(version["if"].is_badvalue(), "on every OS");
    let r = s(&version["run"]);
    assert!(r.contains("want=\"anthology $VERSION ($TARGET)\""), "{r}");
    assert!(r.contains("[ \"$got\" != \"$want\" ]"), "{r}");
    assert_eq!(
        s(&build["env"]["VERSION"]),
        "${{ needs.meta.outputs.version }}"
    );
    let package = step_named(build, "Package");
    let r = s(&package["run"]);
    assert!(
        r.contains("COPYFILE_DISABLE=1 tar -czf \"dist/anthology-$TARGET.tar.gz\" -C \"$stage\" anthology LICENSE\n"),
        "{r}"
    );
    assert!(!workflow_text().contains("NOTICE"));
    let linux = step_named(build, "Install musl-tools (Linux)");
    assert!(s(&linux["run"]).contains("=musl-gcc"));
}

#[test]
fn the_release_publishes_four_tarballs_and_checksums() {
    let wf = workflow();
    let publish = &wf["jobs"]["publish"];
    let sums = s(&step_named(publish, "Checksums")["run"]).to_string();
    assert!(sums.contains("if [ \"$count\" != 4 ]; then"), "{sums}");
    assert!(sums.contains("sha256sum anthology-*.tar.gz | LC_ALL=C sort -k2 > checksums.txt"));
    let create = s(&step_named(publish, "Create the release (fails if it already exists)")["run"])
        .to_string();
    assert!(create.contains("gh release view \"$TAG\""), "{create}");
    assert!(create.contains("--verify-tag"), "{create}");
    assert!(create.contains("dist/checksums.txt"), "{create}");
    for t in TARGETS {
        assert!(
            create.contains(&format!("dist/anthology-{t}.tar.gz")),
            "{t}"
        );
    }
    assert!(create.contains("gh attestation verify <file> --repo rbonweb/anthology"));
}
