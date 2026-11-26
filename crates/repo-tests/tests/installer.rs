//! The one-command installer (`crates/anthology-installer`): the places where it has to agree
//! with files elsewhere in the repository.

use std::collections::BTreeSet;
use std::process::Command;

use repo_tests::repo;
use repo_tests::yaml::{self, Yaml};

const WORKFLOW: &str = ".github/workflows/installer-release.yml";
const RENDER: &str = "ci/jobs/installer-formula.sh";
const TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];

fn workflow() -> Yaml {
    yaml::load_file(WORKFLOW)
}

fn text<'a>(y: &'a Yaml, key: &str) -> &'a str {
    y[key].as_str().unwrap_or("")
}

fn steps(job: &Yaml) -> &[Yaml] {
    yaml::items(&job["steps"])
}

fn jobs(wf: &Yaml) -> Vec<(String, &Yaml)> {
    wf["jobs"]
        .as_hash()
        .expect("jobs")
        .iter()
        .map(|(k, v)| (k.as_str().unwrap_or("?").to_string(), v))
        .collect()
}

fn job_text(job: &Yaml) -> String {
    format!("{job:?}")
}

fn renders(version: &str, checksums: &str, base: &str) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let sums = dir.path().join("checksums.txt");
    std::fs::write(&sums, checksums).unwrap();
    Command::new("/bin/sh")
        .arg(repo::path(RENDER))
        .arg(version)
        .arg(&sums)
        .arg(base)
        .output()
        .expect("run the renderer")
}

fn sample_sums() -> String {
    TARGETS
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{:064x}  anthology-installer-{t}.tar.gz\n", i + 1))
        .collect()
}

mod release_workflow {
    use super::*;

    #[test]
    fn it_starts_with_no_permissions_and_every_job_names_its_own() {
        let wf = workflow();
        assert_eq!(wf["permissions"].as_hash().map(|h| h.len()), Some(0), "top level");
        for (name, job) in jobs(&wf) {
            assert!(yaml::has(job, "permissions"), "{name} has no permissions block");
        }
        let publish = &wf["jobs"]["publish"]["permissions"];
        assert_eq!(text(publish, "id-token"), "write");
        assert_eq!(text(publish, "attestations"), "write");
        assert_eq!(text(publish, "contents"), "write");
        // Only publish may write to this repository's contents.
        for (name, job) in jobs(&wf) {
            if name != "publish" {
                assert_ne!(text(&job["permissions"], "contents"), "write", "{name}");
            }
        }
    }

    #[test]
    fn it_runs_on_the_installer_tag_a_dispatch_and_a_dry_run_pull_request() {
        let wf = workflow();
        // YAML 1.1 reads a bare `on` as true; yaml-rust2 keeps the key as the string.
        let on = &wf["on"];
        assert_eq!(yaml::strings(&on["push"]["tags"]), ["anthology-installer--v*"]);
        assert!(yaml::has(on, "workflow_dispatch"));
        let paths = yaml::strings(&on["pull_request"]["paths"]);
        for want in [
            ".github/workflows/installer-release.yml",
            "crates/anthology-installer/**",
            "packaging/homebrew/**",
            RENDER,
        ] {
            assert!(paths.iter().any(|p| p == want), "no path {want}: {paths:?}");
        }
    }

    #[test]
    fn the_four_release_targets_are_built_and_no_windows() {
        let wf = workflow();
        let matrix = yaml::items(&wf["jobs"]["build"]["strategy"]["matrix"]["include"]);
        let targets: BTreeSet<&str> = matrix.iter().map(|m| text(m, "target")).collect();
        assert_eq!(targets, BTreeSet::from(TARGETS));
        assert_eq!(matrix.len(), 4);
        assert!(!repo::read(WORKFLOW).to_lowercase().contains("windows"));
    }

    #[test]
    fn the_tag_is_checked_against_the_crate_version_not_the_workspace_one() {
        let text = repo::read(WORKFLOW);
        assert!(text.contains("anthology-installer--v"), "tag form");
        assert!(text.contains("crates/anthology-installer/Cargo.toml"));
        assert!(!text.contains("[workspace.package]"));
    }

    #[test]
    fn publish_attests_and_checksums_the_four_tarballs() {
        let wf = workflow();
        let publish = &wf["jobs"]["publish"];
        assert!(
            steps(publish)
                .iter()
                .any(|s| text(s, "uses").starts_with("actions/attest-build-provenance@")),
            "no attestation"
        );
        let run: String = steps(publish).iter().map(|s| text(s, "run")).collect();
        assert!(run.contains("checksums.txt"));
        assert!(run.contains("!= 4"), "counts the tarballs");
        assert_eq!(text(publish, "if"), "needs.meta.outputs.publish == 'true'");
    }

    #[test]
    fn the_tap_job_uses_the_release_environment_and_skips_without_the_key() {
        let wf = workflow();
        let tap = &wf["jobs"]["tap"];
        assert_eq!(text(tap, "environment"), "release");
        assert!(tap["needs"].as_vec().is_some_and(|n| n.iter().any(|x| x.as_str() == Some("publish"))));
        let whole = job_text(tap);
        assert!(whole.contains("TAP_DEPLOY_KEY"));
        assert!(whole.contains("rbonweb/homebrew-anthology"));
        assert!(whole.contains("Formula/anthology-installer.rb"));
        // The key is only ever read into an env var, never into a run script.
        for s in steps(tap) {
            assert!(!text(s, "run").contains("secrets."), "{s:?}");
        }
        // A missing secret is a notice and a clean end, not a failure.
        let skip = steps(tap)
            .iter()
            .find(|s| text(s, "id") == "key")
            .expect("a step with id key decides whether the secret exists");
        let run = text(skip, "run");
        assert!(run.contains("present=false"), "{run}");
        assert!(run.contains("::notice"), "{run}");
        assert!(!run.contains("exit 1"), "{run}");
        for s in steps(tap).iter().filter(|s| text(s, "id") != "key") {
            if text(s, "uses").starts_with("actions/checkout@") {
                continue;
            }
            assert!(
                text(s, "if").contains("steps.key.outputs.present == 'true'"),
                "a tap step runs without the key: {s:?}"
            );
        }
    }

    #[test]
    fn the_tap_job_pins_github_host_key_and_never_puts_the_key_on_a_command_line() {
        let wf = workflow();
        let tap = &wf["jobs"]["tap"];
        let run: String = steps(tap).iter().map(|s| text(s, "run")).collect();
        assert!(run.contains("StrictHostKeyChecking=yes"));
        assert!(run.contains("ssh-ed25519"));
        assert!(run.contains("IdentitiesOnly=yes"));
        assert!(!run.contains("StrictHostKeyChecking=no"));
        assert!(!run.contains("echo \"$TAP_DEPLOY_KEY\" |"));
    }

    #[test]
    fn a_pull_request_renders_the_formula_and_runs_brew_audit_and_test_on_macos_and_linux() {
        let wf = workflow();
        let check = &wf["jobs"]["formula-check"];
        let os: BTreeSet<&str> = yaml::items(&check["strategy"]["matrix"]["os"])
            .iter()
            .filter_map(Yaml::as_str)
            .collect();
        assert!(os.iter().any(|o| o.starts_with("macos-")), "{os:?}");
        assert!(os.iter().any(|o| o.starts_with("ubuntu-")), "{os:?}");
        let run: String = steps(check).iter().map(|s| text(s, "run")).collect();
        assert!(run.contains(RENDER));
        assert!(run.contains("brew audit --strict"));
        assert!(run.contains("brew test"));
        assert!(run.contains("brew install"));
        // It must not publish anything.
        assert!(!job_text(check).contains("TAP_DEPLOY_KEY"));
        assert_ne!(text(&check["permissions"], "contents"), "write");
    }
}

mod formula {
    use super::*;

    #[test]
    fn it_names_each_target_with_its_checksum_and_url() {
        let out = renders("1.2.3", &sample_sums(), "https://example.test/dl");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let rb = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(rb.contains("class AnthologyInstaller < Formula"), "{rb}");
        assert!(rb.contains(r#"version "1.2.3""#));
        assert!(rb.contains(r#"license "Apache-2.0""#));
        for (i, t) in TARGETS.iter().enumerate() {
            assert!(
                rb.contains(&format!(r#"url "https://example.test/dl/anthology-installer-{t}.tar.gz""#)),
                "{t}\n{rb}"
            );
            assert!(rb.contains(&format!(r#"sha256 "{:064x}""#, i + 1)), "{t}");
        }
        assert!(rb.contains("on_macos do") && rb.contains("on_linux do"));
        assert!(rb.contains(r#"bin.install "anthology-installer""#));
        assert!(rb.contains("test do"));
        assert!(!rb.contains('@'), "an unfilled placeholder: {rb}");
    }

    #[test]
    fn a_missing_target_or_a_malformed_checksum_is_refused() {
        let sums = sample_sums();
        let without: String = sums.lines().skip(1).map(|l| format!("{l}\n")).collect();
        assert!(!renders("1.2.3", &without, "https://x.test").status.success());
        let bad = sums.replacen("0000000000000000000000000000000000000000000000000000000000000001", "xyz", 1);
        assert!(!renders("1.2.3", &bad, "https://x.test").status.success());
        assert!(!renders("not a version", &sums, "https://x.test").status.success());
        assert!(!renders("1.2.3", &sums, "http://x.test").status.success(), "plain http");
    }

    #[test]
    fn a_local_file_base_is_allowed_for_the_pull_request_dry_run() {
        assert!(renders("1.2.3", &sample_sums(), "file:///tmp/dist").status.success());
    }
}

/// The command the installer runs for the guard binary is the one `anth_fix_cmd` builds in each
/// plugin's `lib.sh`, which is the only command the guard lets through while the binary is
/// missing.
#[test]
fn the_bootstrap_command_matches_what_anth_fix_cmd_builds() {
    for plugin in ["anthology", "anthology-guardrails"] {
        let lib = repo::path(&format!("plugins/{plugin}/hooks/lib.sh"));
        let hooks = "/opt/plugin/hooks";
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(r#". "$1" && anth_fix_cmd "$2" && printf '%s' "$ANTH_FIX""#)
            .arg("sh")
            .arg(&lib)
            .arg(hooks)
            .output()
            .expect("run sh");
        assert!(
            out.status.success(),
            "{plugin}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            anthology_installer::binary::bootstrap_command(hooks),
            "{plugin}: lib.sh and the installer disagree"
        );
    }
}
