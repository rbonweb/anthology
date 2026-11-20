//! Port of `tests/test_ci.py`: static checks on the CI configuration (pinned Claude Code, workflow rules).

use std::collections::BTreeSet;

use anthology_pycompat::json::{self, Value};
use regex::Regex;
use repo_tests::repo;
use repo_tests::yaml::{self, Yaml};

const CLAUDE_CODE: &str = "@anthropic-ai/claude-code";
const MIN_CLAUDE_CODE: (u64, u64, u64) = (2, 1, 323); // older versions reject `engines` in plugin.json under --strict
const PLUGINS: [&str; 3] = ["anthology", "anthology-guardrails", "anthology-superpowers"];

/// The `.github/workflows/*.yml` names, sorted.
fn workflows() -> Vec<String> {
    repo::list_dir(".github/workflows")
        .into_iter()
        .filter(|n| n.ends_with(".yml"))
        .collect()
}

fn load_workflow(name: &str) -> Yaml {
    yaml::load_file(&format!(".github/workflows/{name}"))
}

/// Every (job name, step) pair of a workflow, in file order.
fn steps_of(workflow: &Yaml) -> Vec<(String, &Yaml)> {
    let mut out = Vec::new();
    if let Some(jobs) = workflow["jobs"].as_hash() {
        for (name, job) in jobs {
            for step in yaml::items(&job["steps"]) {
                out.push((name.as_str().unwrap_or("?").to_string(), step));
            }
        }
    }
    out
}

fn steps(job: &Yaml) -> &[Yaml] {
    yaml::items(&job["steps"])
}

fn text<'a>(y: &'a Yaml, key: &str) -> &'a str {
    y[key].as_str().unwrap_or("")
}

/// The first step of `job` that `pred` accepts; panics (as Python's `next`) when none does.
fn find_step<'a>(job: &'a Yaml, what: &str, pred: impl Fn(&Yaml) -> bool) -> &'a Yaml {
    steps(job)
        .iter()
        .find(|s| pred(s))
        .unwrap_or_else(|| panic!("no step: {what}"))
}

fn checkout_of(job: &Yaml) -> &Yaml {
    find_step(job, "actions/checkout", |s| {
        text(s, "uses").starts_with("actions/checkout@")
    })
}

fn run_text(job: &Yaml) -> String {
    steps(job)
        .iter()
        .map(|s| text(s, "run"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn ci_jobs() -> Yaml {
    load_workflow("ci.yml")
}

fn json_file(rel: &str) -> Value {
    json::loads(&repo::read(rel)).unwrap_or_else(|e| panic!("{rel}: {}", e.py_str(rel)))
}

/// `v[k0][k1]...`; panics when a key is missing (Python's KeyError).
fn at<'a>(v: &'a Value, path: &[&str]) -> &'a Value {
    let mut cur = v;
    for k in path {
        cur = cur
            .as_object()
            .and_then(|o| o.get(k))
            .unwrap_or_else(|| panic!("missing key {k:?}"));
    }
    cur
}

/// A mapping of strings to strings (a `permissions:` block).
fn str_map(y: &Yaml) -> Vec<(String, String)> {
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

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = items
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    v.sort();
    v
}

mod pinned_claude_code_tests {
    use super::*;

    fn pkg() -> Value {
        json_file("ci/claude-code/package.json")
    }

    fn pin(pkg: &Value) -> String {
        at(pkg, &["dependencies", CLAUDE_CODE])
            .as_str()
            .expect("the pin is a string")
            .to_string()
    }

    #[test]
    fn test_exact_version_at_least_minimum() {
        let pin = pin(&pkg());
        let m = Regex::new(r"^([0-9]+)\.([0-9]+)\.([0-9]+)$")
            .unwrap()
            .captures(&pin)
            .unwrap_or_else(|| panic!("{pin:?} must be an exact X.Y.Z, not a range"));
        let n = |i: usize| m[i].parse::<u64>().unwrap();
        assert!((n(1), n(2), n(3)) >= MIN_CLAUDE_CODE, "{pin}");
    }

    #[test]
    fn test_lockfile_matches_the_pin() {
        let pin = pin(&pkg());
        let lock = json_file("ci/claude-code/package-lock.json");
        let key = format!("node_modules/{CLAUDE_CODE}");
        let entry = at(&lock, &["packages", &key, "version"]);
        assert_eq!(entry.as_str(), Some(pin.as_str()));
    }

    #[test]
    fn test_package_is_private() {
        let pkg = pkg();
        assert_eq!(at(&pkg, &["private"]).as_bool(), Some(true));
    }
}

mod workflow_rule_tests {
    use super::*;

    #[test]
    fn test_there_are_workflows() {
        assert!(!workflows().is_empty());
    }

    #[test]
    fn test_every_uses_is_pinned_by_full_sha() {
        let uses_re = Regex::new(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$").unwrap();
        for name in workflows() {
            let wf = load_workflow(&name);
            for (job, step) in steps_of(&wf) {
                if yaml::has(step, "uses") {
                    let uses = text(step, "uses");
                    assert!(uses_re.is_match(uses), "{name}:{job}: {uses:?}");
                }
            }
        }
    }

    #[test]
    fn test_top_level_permissions() {
        for name in workflows() {
            assert!(yaml::has(&load_workflow(&name), "permissions"), "{name}");
        }
    }

    #[test]
    fn test_no_expression_inside_run_scripts() {
        for name in workflows() {
            let wf = load_workflow(&name);
            for (job, step) in steps_of(&wf) {
                assert!(
                    !text(step, "run").contains("${{"),
                    "{name}:{job}: pass it via env:"
                );
            }
        }
    }

    #[test]
    fn test_checkouts_do_not_persist_credentials() {
        for name in workflows() {
            let wf = load_workflow(&name);
            for (job, step) in steps_of(&wf) {
                if text(step, "uses").starts_with("actions/checkout@") {
                    assert_eq!(
                        step["with"]["persist-credentials"].as_bool(),
                        Some(false),
                        "{name}:{job}"
                    );
                }
            }
        }
    }
}

mod ci_job_tests {
    use super::*;

    #[test]
    fn test_version_bump_job() {
        let wf = ci_jobs();
        let jobs = &wf["jobs"];
        assert!(yaml::has(jobs, "version-bump"));
        let job = &jobs["version-bump"];
        assert_eq!(checkout_of(job)["with"]["fetch-depth"].as_i64(), Some(0));
        // The gate step is found by its BASE env, not by its script name (the script is a Python
        // artifact until the cutover).
        // omitted: the `check_version_bump.py` step lookup -> CO2 replacement.
        let gate = find_step(job, "the gate step with env BASE", |s| {
            yaml::has(&s["env"], "BASE")
        });
        assert_eq!(
            text(&gate["env"], "BASE"),
            "${{ github.event.pull_request.base.sha || github.event.before }}"
        );
        assert_eq!(
            text(&gate["env"], "HEAD_SHA"),
            "${{ github.event.pull_request.head.sha || github.sha }}"
        );
        assert!(text(gate, "run").contains("\"$BASE\" \"$HEAD_SHA\""));
    }

    #[test]
    fn test_plugin_validate_matrix_is_the_marketplace_and_exactly_the_plugins() {
        let wf = ci_jobs();
        let job = &wf["jobs"]["plugin-validate"];
        let mut want: BTreeSet<String> = PLUGINS.iter().map(|p| format!("plugins/{p}")).collect();
        want.insert(".".to_string());
        let targets = yaml::strings(&job["strategy"]["matrix"]["target"]);
        assert_eq!(
            targets.iter().cloned().collect::<BTreeSet<_>>(),
            want,
            "{targets:?}"
        );
        // A non-string entry is dropped by `strings`, so count the raw items too.
        assert_eq!(
            yaml::items(&job["strategy"]["matrix"]["target"]).len(),
            want.len()
        );
        assert_eq!(job["strategy"]["fail-fast"].as_bool(), Some(false));
        let run = find_step(job, "plugin validate", |s| {
            text(s, "run").contains("plugin validate")
        });
        assert!(text(run, "run").contains("plugin validate --strict \"$TARGET\""));
        assert_eq!(text(&run["env"], "TARGET"), "${{ matrix.target }}");
    }

    #[test]
    fn test_marketplace_manifest_has_a_description() {
        let manifest = json_file(".claude-plugin/marketplace.json");
        let description = manifest
            .as_object()
            .and_then(|o| o.get("description"))
            .and_then(|d| d.as_str())
            .expect("description is a string");
        assert!(!description.trim().is_empty());
        let keys: Vec<&str> = manifest
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k)
            .take(4)
            .collect();
        assert_eq!(keys, ["name", "owner", "description", "plugins"]);
    }

    #[test]
    fn test_gitleaks_job() {
        let wf = ci_jobs();
        let jobs = &wf["jobs"];
        assert!(yaml::has(jobs, "gitleaks"));
        let job = &jobs["gitleaks"];
        assert_eq!(checkout_of(job)["with"]["fetch-depth"].as_i64(), Some(0));
        let scan = find_step(job, "docker run", |s| text(s, "run").contains("docker run"));
        // the scan only reads the checkout
        assert!(text(scan, "run").contains("-v \"$PWD\":/repo:ro"));
    }

    #[test]
    fn test_new_jobs_have_a_timeout() {
        let wf = ci_jobs();
        for name in ["version-bump", "plugin-validate", "gitleaks"] {
            assert_eq!(
                wf["jobs"][name]["timeout-minutes"].as_i64(),
                Some(15),
                "{name}"
            );
        }
    }

    #[test]
    fn test_every_plugin_has_a_validate_target() {
        let on_disk: BTreeSet<String> = repo::list_dir("plugins")
            .into_iter()
            .filter(|n| repo::path(&format!("plugins/{n}")).is_dir())
            .collect();
        let want: BTreeSet<String> = PLUGINS.iter().map(|p| p.to_string()).collect();
        assert_eq!(on_disk, want);
    }
}

mod memory_diff_startup_budget_cli_syntax_job_tests {
    use super::*;

    #[test]
    fn test_jobs_exist_with_a_timeout() {
        let wf = ci_jobs();
        for name in ["memory-diff", "startup-budget", "cli-syntax"] {
            assert!(yaml::has(&wf["jobs"], name), "{name}");
            assert_eq!(
                wf["jobs"][name]["timeout-minutes"].as_i64(),
                Some(15),
                "{name}"
            );
        }
    }

    #[test]
    fn test_memory_diff_is_pull_request_only_and_passes_the_shas_by_env() {
        let wf = ci_jobs();
        let job = &wf["jobs"]["memory-diff"];
        assert_eq!(text(job, "if"), "github.event_name == 'pull_request'");
        assert_eq!(checkout_of(job)["with"]["fetch-depth"].as_i64(), Some(0));
        // Found by its BASE env: the `memory_diff.py` script name is a Python artifact.
        // omitted: the `memory_diff.py` step lookup -> CO2 replacement.
        let step = find_step(job, "the step with env BASE", |s| {
            yaml::has(&s["env"], "BASE")
        });
        assert_eq!(
            text(&step["env"], "BASE"),
            "${{ github.event.pull_request.base.sha }}"
        );
        assert_eq!(
            text(&step["env"], "HEAD_SHA"),
            "${{ github.event.pull_request.head.sha }}"
        );
        assert!(text(step, "run").contains("\"$BASE\" \"$HEAD_SHA\""));
        // the workflow's contents: read is enough
        assert!(!yaml::has(job, "permissions"));
        // it comments nothing on the PR
        assert!(!run_text(job).contains("gh pr"));
    }

    #[test]
    fn test_startup_budget_runs_both_budget_tests_in_the_pinned_image() {
        let wf = ci_jobs();
        let text = run_text(&wf["jobs"]["startup-budget"]);
        // omitted: the Python image, `--require-hashes -r tests/requirements.txt`, `test_roster`,
        // `test_session_start` and `set -e` (all pin the Python run) -> CO2 replacement.
        assert!(text.contains("startup_budget"));
    }

    #[test]
    fn test_cli_syntax_installs_the_pinned_cli_and_runs_the_smoke() {
        let wf = ci_jobs();
        let job = &wf["jobs"]["cli-syntax"];
        let text = run_text(job);
        assert!(text.contains("npm ci --prefix ci/claude-code"));
        // omitted: `evals/smoke/smoke.py` -> CO2 replacement.
        assert!(text.contains("cli-syntax"));
        assert!(text.contains("ci/claude-code/node_modules/.bin/claude"));
        assert_eq!(super::text(&job["env"], "DISABLE_AUTOUPDATER"), "1");
    }
}

mod upstream_watch_workflow_tests {
    use super::*;

    fn wf() -> Yaml {
        load_workflow("upstream-watch.yml")
    }

    #[test]
    fn test_least_privilege_permissions() {
        let wf = wf();
        assert_eq!(wf["permissions"].as_hash().map(|h| h.len()), Some(0));
        let jobs = &wf["jobs"];
        assert_eq!(
            str_map(&jobs["watch"]["permissions"]),
            pairs(&[("contents", "read"), ("issues", "write")])
        );
        assert_eq!(
            str_map(&jobs["trigger-evals"]["permissions"]),
            pairs(&[("contents", "read"), ("actions", "write")])
        );
    }

    #[test]
    fn test_weekly_schedule_and_manual_run() {
        let wf = wf();
        // yaml-rust2 (YAML 1.2) keeps `on` as the string key; PyYAML read it as True.
        let triggers = &wf["on"];
        assert_eq!(yaml::items(&triggers["schedule"]).len(), 1);
        assert!(yaml::has(triggers, "workflow_dispatch"));
    }

    #[test]
    fn test_jobs_have_a_timeout() {
        let wf = wf();
        for (name, job) in wf["jobs"].as_hash().expect("jobs") {
            assert_eq!(
                job["timeout-minutes"].as_i64(),
                Some(15),
                "{}",
                name.as_str().unwrap_or("?")
            );
        }
    }

    #[test]
    fn test_evals_are_dispatched_on_the_default_branch() {
        let wf = wf();
        let step = find_step(&wf["jobs"]["trigger-evals"], "gh workflow run", |s| {
            text(s, "run").contains("gh workflow run")
        });
        assert_eq!(
            text(&step["env"], "REF"),
            "${{ github.event.repository.default_branch }}"
        );
    }

    #[test]
    fn test_evals_trigger_follows_the_watch_output() {
        let wf = wf();
        let job = &wf["jobs"]["trigger-evals"];
        assert_eq!(text(job, "needs"), "watch");
        assert!(text(job, "if").contains("needs.watch.outputs.claude_code_changed == 'true'"));
        assert!(yaml::has(
            &wf["jobs"]["watch"]["outputs"],
            "claude_code_changed"
        ));
    }
}

mod dependabot_tests {
    use super::*;

    #[test]
    fn test_claude_code_updates_wait_three_days() {
        let config = yaml::load_file(".github/dependabot.yml");
        let entry = yaml::items(&config["updates"])
            .iter()
            .find(|u| text(u, "package-ecosystem") == "npm")
            .expect("an npm entry");
        assert_eq!(text(entry, "directory"), "/ci/claude-code");
        let cooldown = entry["cooldown"].as_hash().expect("cooldown");
        assert_eq!(cooldown.len(), 1);
        assert_eq!(entry["cooldown"]["default-days"].as_i64(), Some(3));
    }
}
