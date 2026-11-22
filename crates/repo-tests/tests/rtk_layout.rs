//! Port of `tests/test_rtk_layout.py`: the atl subtree's sources.json pins, license, named fork
//! tests and `scripts/sync-atl.sh`.

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use anthology_pycompat::json::{self, Value};
use regex::Regex;
use repo_tests::repo;

// The current pins. scripts/sync-atl.sh rewrites the upstream pin in sources.json on every sync,
// so a merged sync bumps UPSTREAM_TAG and UPSTREAM_COMMIT here with it.
const FORK_COMMIT: &str = "9b790c414de16f5fb688126b3945c73aa0e0218e";
const UPSTREAM_TAG: &str = "v1.2.1";
const UPSTREAM_COMMIT: &str = "d0f0399a5ff8cd1d81325a033f0b2d8864d665f6";
const RELEASE_TAG: &str = r"v[0-9]+\.[0-9]+\.[0-9]+";
const FULL_SHA: &str = "[0-9a-f]{40}";
// Fork change 8 (custom `[upstream.*]` headers) lived only in the BUSL engine and was dropped
// with it by Task 9's trim (stream ruling); the other 8 keep their named tests.
const FORK_TESTS: [&str; 8] = [
    "fork_1_auto_allow_hook.rs",
    "fork_2_read_minimal_default.rs",
    "fork_3_node_filter.rs",
    "fork_4_python_filter.rs",
    "fork_5_cd_chain_rewrite.rs",
    "fork_6_biome_lint.rs",
    "fork_7_opencode_run.rs",
    "fork_9_awareness_exact.rs",
];
const SYNC: &str = "scripts/sync-atl.sh";
const ATL_TEST: &str = "scripts/atl-test.sh";

/// `re.fullmatch`: the whole text matches (the pattern may carry its own anchors).
fn fullmatch(pattern: &str, text: &str) -> bool {
    Regex::new(&format!(r"\A(?:{pattern})\z"))
        .unwrap_or_else(|e| panic!("pattern {pattern:?} does not compile: {e}"))
        .is_match(text)
}

/// `os.access(path, os.X_OK)`, as far as the mode bits go.
fn is_executable(rel: &str) -> bool {
    std::fs::metadata(repo::path(rel)).is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
}

fn matches(pattern: &str, text: &str) -> bool {
    Regex::new(pattern).unwrap().is_match(text)
}

/// The one `atl` entry of vendor/sources.json.
fn atl_entry() -> Value {
    let text = repo::read("vendor/sources.json");
    let data = json::loads(&text).unwrap_or_else(|e| panic!("sources.json: {}", e.py_str(&text)));
    let sources = data
        .as_object()
        .and_then(|m| m.get("sources"))
        .and_then(Value::as_array)
        .expect("sources.json has no sources list");
    let mut entries: Vec<&Value> = sources
        .iter()
        .filter(|s| {
            s.as_object()
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                == Some("atl")
        })
        .collect();
    assert!(
        entries.len() == 1,
        "expected one atl entry in sources.json, got {} entries",
        entries.len()
    );
    entries.remove(0).clone()
}

fn field<'a>(entry: &'a Value, key: &str) -> &'a Value {
    entry
        .as_object()
        .and_then(|m| m.get(key))
        .unwrap_or_else(|| panic!("the atl entry has no {key}"))
}

fn str_field<'a>(entry: &'a Value, key: &str) -> &'a str {
    field(entry, key)
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string"))
}

/// `value == {k: v, ...}` for a JSON object of strings.
fn object_eq(v: &Value, expected: &[(&str, &str)]) -> bool {
    match v.as_object() {
        Some(m) => {
            m.len() == expected.len()
                && expected
                    .iter()
                    .all(|(k, want)| m.get(k).and_then(Value::as_str) == Some(*want))
        }
        None => false,
    }
}

mod rtk_source_entry_test {
    // legacy: port/ledger names this module
    use super::*;

    #[test]
    fn test_entry_is_a_subtree_with_its_target_and_license() {
        let entry = atl_entry();
        assert_eq!(str_field(&entry, "managed_by"), "subtree");
        assert_eq!(str_field(&entry, "target"), "atl");
        assert_eq!(str_field(&entry, "license"), "Apache-2.0");
        assert!(!entry.as_object().unwrap().contains_key("repo"));
    }

    #[test]
    fn test_fork_pin() {
        let entry = atl_entry();
        assert!(
            object_eq(
                field(&entry, "fork"),
                &[
                    ("repo", "https://github.com/rbonweb/rtk"),
                    ("ref", "develop"),
                    ("commit", FORK_COMMIT),
                ]
            ),
            "fork pin is {:?}",
            field(&entry, "fork")
        );
    }

    #[test]
    fn test_upstream_pin() {
        let entry = atl_entry();
        assert!(
            object_eq(
                field(&entry, "upstream"),
                &[
                    ("repo", "https://github.com/rtk-ai/rtk"),
                    ("ref", UPSTREAM_TAG),
                    ("commit", UPSTREAM_COMMIT),
                ]
            ),
            "upstream pin is {:?}",
            field(&entry, "upstream")
        );
    }

    #[test]
    fn test_pins_have_the_shape_a_sync_writes() {
        let entry = atl_entry();
        let upstream = field(&entry, "upstream");
        let tag = str_field(upstream, "ref");
        assert!(fullmatch(RELEASE_TAG, tag), "{upstream:?}");
        for side in ["fork", "upstream"] {
            let commit = str_field(field(&entry, side), "commit");
            assert!(fullmatch(FULL_SHA, commit), "{side}");
        }
    }
}

mod rtk_tree_test {
    // legacy: port/ledger names this module
    use super::*;

    #[test]
    fn test_license_is_apache_2() {
        let text = repo::read("atl/LICENSE");
        assert!(text.contains("Apache License"));
        assert!(text.contains("Version 2.0, January 2004"));
    }

    #[test]
    fn test_cargo_manifest_exists() {
        assert!(repo::path("atl/Cargo.toml").is_file());
    }

    #[test]
    fn test_every_fork_change_has_its_named_test() {
        let found: Vec<String> = repo::list_dir("atl/tests")
            .into_iter()
            .filter(|n| n.starts_with("fork_") && n.ends_with(".rs"))
            .collect();
        assert_eq!(found, FORK_TESTS);
        for name in FORK_TESTS {
            assert!(
                repo::read(&format!("atl/tests/{name}")).contains("#[test]"),
                "{name}"
            );
        }
    }
}

mod sync_script_test {
    use super::*;

    #[test]
    fn test_is_executable_with_strict_mode() {
        let text = repo::read(SYNC);
        assert!(is_executable(SYNC));
        assert!(text.starts_with("#!"));
        assert!(matches(r"(?m)^set -eu$", &text));
    }

    /// The sync merges, but only on its own review branch: nothing is pushed, and no merge or
    /// switch happens before the branch is created or onto the integration branches.
    #[test]
    fn test_never_pushes_or_merges() {
        let text = repo::read(SYNC);
        let code: Vec<&str> = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect();
        let code = code.join("\n");
        assert!(!matches(r"\bgit\s+push\b", &code));
        assert!(!matches(
            r"\bgit\s+(checkout|switch)\s+(build|main)\b",
            &code
        ));
        let branch_made = code
            .find(r#"git switch -c "$branch""#)
            .expect("a review branch");
        let first_merge = code.find("git merge").expect("a merge");
        assert!(
            branch_made < first_merge,
            "a merge runs before the review branch exists"
        );
    }

    #[test]
    fn test_pulls_upstream_into_the_subtree_on_a_review_branch() {
        let text = repo::read(SYNC);
        for needle in [
            "UPSTREAM=${ATL_SYNC_UPSTREAM:-https://github.com/rtk-ai/rtk}",
            r#"scripts/atl-rename.sh --root "$dir""#,
            r#"git merge -q -s ours --allow-unrelated-histories"#,
            r#"git merge --no-edit -m "$merge_message" "$renamed_new""#,
            r#"branch="atl-sync/$tag""#,
            "git status --porcelain",
            "scripts/atl-trim.sh",
            r#""$root/scripts/atl-test.sh""#,
        ] {
            assert!(text.contains(needle), "{needle}");
        }
        assert!(
            !text.contains("subtree pull"),
            "a plain subtree pull cannot see the rename"
        );
    }

    #[test]
    fn test_validates_the_tag() {
        let text = repo::read(SYNC);
        let m = Regex::new(r"(?m)^tag_pattern='([^']+)'$")
            .unwrap()
            .captures(&text)
            .expect("sync-atl.sh has no tag_pattern='...' line");
        assert!(text.contains("[[ $tag =~ $tag_pattern ]]"));
        // The data is compiled as given. A pattern Python's `re` accepts but the regex crate
        // cannot (lookaround, backreference) fails here with this message.
        let source = &m[1];
        if let Err(e) = Regex::new(source) {
            panic!(
                "tag_pattern {source:?} does not compile with the regex crate (Python's re may \
                 accept it): {e}"
            );
        }
        let full = |s: &str| fullmatch(source, s);
        assert!(full("v1.2.1"));
        for bad in ["1.2.1", "v1.2", "v1.2.1; rm -rf /", "main", "v1.2.1-rc1"] {
            assert!(!full(bad), "{bad}");
        }
    }

    // Tag validation runs before any git call, so this needs neither git nor a repo.
    #[test]
    fn test_rejects_a_bad_tag_before_touching_the_repo() {
        if Command::new("bash").arg("--version").output().is_err() {
            eprintln!("skipped: bash is not installed");
            return;
        }
        let script = repo::path(SYNC);
        for bad in ["v1.2", "v1.2.1\nx", "main"] {
            let out = Command::new("bash")
                .arg(&script)
                .arg(bad)
                .output()
                .expect("bash ran");
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(2), "{bad:?}: {stderr}");
            assert!(
                stderr.contains("usage: scripts/sync-atl.sh <tag>"),
                "{bad:?}: {stderr}"
            );
        }
    }

    #[test]
    fn test_records_the_merged_upstream_pin() {
        let text = repo::read(SYNC);
        for needle in [
            r#"git fetch --no-tags --quiet "$UPSTREAM" "refs/tags/$tag""#,
            r#"git rev-parse "FETCH_HEAD^{commit}""#,
            r#"echo "  pin:      $pin""#,
        ] {
            assert!(text.contains(needle), "{needle}");
        }
        // omitted: the embedded python that rewrites the pin (`path = Path("vendor/sources.json")`,
        // `json.dumps(data, indent=2, ensure_ascii=False)`) -> CO2 replacement: cutover_ci.rs
        // asserts sync-atl.sh records the pin through scripts/dev.sh and embeds no python3
    }

    #[test]
    fn test_a_failed_trim_still_reaches_the_summary() {
        let text = repo::read(SYNC);
        assert!(text.contains("if bash scripts/atl-trim.sh; then"));
        assert!(text.contains(r#"exit "$status""#));
    }
}

/// `$ATL_TEST` is scripts/atl-test.sh, used locally, by CI and by sync-atl.sh.
mod rtk_test_script_test {
    // legacy: port/ledger names this module
    use super::*;

    #[test]
    fn test_is_executable_with_strict_mode() {
        let text = repo::read(ATL_TEST);
        assert!(is_executable(ATL_TEST));
        assert!(text.starts_with("#!"));
        assert!(matches(r"(?m)^set -eu$", &text));
    }

    #[test]
    fn test_reads_the_image_from_the_rust_dockerfile() {
        let text = repo::read(ATL_TEST);
        assert!(
            text.contains(
                r#"awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile""#
            )
        );
        assert!(!matches(r"rust:[0-9.]+-slim", &text));
    }

    #[test]
    fn test_runs_locked_cargo_test_as_a_non_root_user() {
        let text = repo::read(ATL_TEST);
        assert!(text.contains(r#"cargo test --locked "$@""#));
        assert!(text.contains("runuser -u"));
        assert!(text.contains("useradd"));
        assert!(text.contains("git curl jq procps"));
        assert!(!text.contains("--skip"));
    }

    #[test]
    fn test_ci_and_sync_use_it() {
        let ci = repo::read(".github/workflows/ci.yml");
        assert!(matches(r"(?m)^  atl-test:$", &ci));
        assert!(ci.contains("run: scripts/atl-test.sh"));
        assert!(repo::read(SYNC).contains(r#""$root/scripts/atl-test.sh""#));
    }
}
