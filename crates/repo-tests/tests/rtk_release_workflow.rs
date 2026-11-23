//! Port of `tests/test_rtk_release_workflow.py`: shape of the atl release workflow, its toolchain pin and version.

use std::collections::BTreeSet;
use std::process::Command;

use regex::Regex;
use repo_tests::repo;
use repo_tests::yaml::{self, Yaml};

const WORKFLOW: &str = ".github/workflows/atl-release.yml";
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

fn load() -> Yaml {
    yaml::load_file(WORKFLOW)
}

fn text<'a>(y: &'a Yaml, key: &str) -> &'a str {
    y[key].as_str().unwrap_or("")
}

fn steps(job: &Yaml) -> &[Yaml] {
    yaml::items(&job["steps"])
}

/// Every (job name, job) pair in file order.
fn jobs(wf: &Yaml) -> Vec<(String, &Yaml)> {
    wf["jobs"]
        .as_hash()
        .expect("jobs")
        .iter()
        .map(|(k, v)| (k.as_str().unwrap_or("?").to_string(), v))
        .collect()
}

fn matrix_include(wf: &Yaml) -> &[Yaml] {
    yaml::items(&wf["jobs"]["build"]["strategy"]["matrix"]["include"])
}

/// The `id: tag` step of the `meta` job.
fn tag_step(wf: &Yaml) -> &Yaml {
    steps(&wf["jobs"]["meta"])
        .iter()
        .find(|s| s["id"].as_str() == Some("tag"))
        .expect("the tag step")
}

fn perms(y: &Yaml) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = y
        .as_hash()
        .map(|h| {
            h.iter()
                .map(|(k, v)| {
                    (
                        k.as_str().unwrap_or("").to_string(),
                        v.as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

mod rtk_release_workflow_test {
    // legacy: port/ledger names this module
    use super::*;

    #[test]
    fn test_every_action_is_pinned_by_sha() {
        let wf = load();
        let re = Regex::new(r"^[\w.-]+/[\w./-]+@[0-9a-f]{40}$").unwrap();
        let uses: Vec<&str> = jobs(&wf)
            .into_iter()
            .flat_map(|(_, job)| steps(job).iter())
            .filter(|s| yaml::has(s, "uses"))
            .map(|s| text(s, "uses"))
            .collect();
        assert!(!uses.is_empty());
        for u in uses {
            assert!(re.is_match(u), "{u}");
        }
    }

    #[test]
    fn test_targets_are_exactly_the_four() {
        let wf = load();
        let matrix = matrix_include(&wf);
        let targets: BTreeSet<&str> = matrix.iter().map(|m| text(m, "target")).collect();
        assert_eq!(targets, BTreeSet::from(TARGETS));
        assert_eq!(matrix.len(), TARGETS.len());
        assert_eq!(
            wf["jobs"]["build"]["strategy"]["fail-fast"].as_bool(),
            Some(false)
        );
    }

    #[test]
    fn test_no_windows() {
        assert!(!repo::read(WORKFLOW).to_lowercase().contains("windows"));
        let wf = load();
        for m in matrix_include(&wf) {
            assert!(!text(m, "os").contains("windows"));
        }
    }

    #[test]
    fn test_publish_job_permissions_and_attestation() {
        let wf = load();
        let publish = &wf["jobs"]["publish"];
        let perms = &publish["permissions"];
        assert_eq!(text(perms, "id-token"), "write");
        assert_eq!(text(perms, "attestations"), "write");
        assert_eq!(text(perms, "contents"), "write");
        assert!(
            steps(publish)
                .iter()
                .any(|s| text(s, "uses").starts_with("actions/attest-build-provenance@"))
        );
    }

    #[test]
    fn test_permissions_are_least_privilege() {
        let wf = load();
        assert_eq!(wf["permissions"].as_hash().map(|h| h.len()), Some(0));
        let want = vec![("contents".to_string(), "read".to_string())];
        for (name, job) in jobs(&wf) {
            if name != "publish" {
                assert_eq!(perms(&job["permissions"]), want, "{name}");
            }
        }
    }

    #[test]
    fn test_triggers() {
        let wf = load();
        // yaml-rust2 (YAML 1.2) keeps `on` as the string key; PyYAML read it as True.
        let on = &wf["on"];
        assert!(yaml::strings(&on["push"]["tags"]).contains(&"atl--v*".to_string()));
        assert_eq!(
            on["workflow_dispatch"]["inputs"]["tag"]["required"].as_bool(),
            Some(true)
        );
        assert!(
            yaml::strings(&on["pull_request"]["paths"]).contains(&"atl/rust-toolchain.toml".into())
        );
    }

    #[test]
    fn test_tag_is_validated_whole_string_under_bash() {
        let wf = load();
        let step = tag_step(&wf);
        assert_eq!(step["shell"].as_str(), Some("bash"));
        let run = text(step, "run");
        assert!(
            run.contains("[[ $tag =~ ^atl--v[0-9]+\\.[0-9]+\\.[0-9]+(-anthology\\.[0-9]+)?$ ]]"),
            "{run}"
        );
        // a line-based grep accepts a multi-line dispatch input
        assert!(!run.contains("grep"));
    }

    #[test]
    fn test_the_tag_check_rejects_multi_line_input() {
        if Command::new("bash").arg("--version").output().is_err() {
            eprintln!("skipped: bash not found");
            return;
        }
        let wf = load();
        let run = text(tag_step(&wf), "run");
        let check = run
            .lines()
            .map(str::trim)
            .find(|l| l.contains("=~"))
            .expect("a line with =~");
        let cond = check
            .strip_prefix("if ! ")
            .and_then(|c| c.strip_suffix("; then"))
            .unwrap_or_else(|| panic!("unexpected check line: {check}"));
        let cases = [
            ("atl--v1.2.1", true),
            ("atl--v1.2.1-anthology.3", true),
            ("rtk--v1.2.1-anthology.3", false), // legacy: a published tag is never rebuilt
            ("atl--v1.2.1\nanything", false),
            ("evil\natl--v1.2.1", false),
            ("atl--v1.2", false),
            ("atl--v1.2.1-x", false),
        ];
        for (tag, ok) in cases {
            let out = Command::new("bash")
                .args(["-c", &format!("tag=$1; {cond}"), "bash", tag])
                .output()
                .expect("run bash");
            assert_eq!(out.status.success(), ok, "{tag:?}");
        }
    }

    #[test]
    fn test_every_job_has_a_timeout() {
        let wf = load();
        for (name, job) in jobs(&wf) {
            assert!(yaml::has(job, "timeout-minutes"), "{name}");
        }
    }

    #[test]
    fn test_no_untrusted_expression_in_run_scripts() {
        let wf = load();
        for (name, job) in jobs(&wf) {
            for s in steps(job) {
                assert!(
                    !text(s, "run").contains("${{"),
                    "{name}: {}",
                    text(s, "name")
                );
            }
        }
    }

    #[test]
    fn test_toolchain_matches_the_rust_image() {
        let toolchain = repo::read("atl/rust-toolchain.toml");
        let channel = Regex::new(r#"(?m)^channel\s*=\s*"([^"]+)""#)
            .unwrap()
            .captures(&toolchain)
            .map(|c| c[1].to_string())
            .expect("a channel line");
        assert!(!toolchain.contains("targets"));
        let dockerfile = repo::read("ci/images/rust.Dockerfile");
        let image = Regex::new(r"(?m)^FROM\s+rust:(\d+(?:\.\d+)*)")
            .unwrap()
            .captures(&dockerfile)
            .map(|c| c[1].to_string())
            .expect("a FROM rust: line");
        assert!(
            channel == image || channel.starts_with(&format!("{image}.")),
            "toolchain {channel} vs image rust:{image}"
        );
    }

    #[test]
    fn test_version_is_the_anthology_edition() {
        let cargo = repo::read("atl/Cargo.toml");
        let version = Regex::new(r#"(?m)^version = "([^"]+)""#)
            .unwrap()
            .captures(&cargo)
            .map(|c| c[1].to_string())
            .expect("a version line");
        assert!(
            Regex::new(r"^\d+\.\d+\.\d+-anthology\.\d+$")
                .unwrap()
                .is_match(&version),
            "{version}"
        );
        let lock = repo::read("atl/Cargo.lock");
        assert!(
            lock.contains(&format!("name = \"atl\"\nversion = \"{version}\"\n")),
            "atl/Cargo.lock does not pin {version}"
        );
    }
}
