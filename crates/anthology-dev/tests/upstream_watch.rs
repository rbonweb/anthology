//! Port of `tests/test_upstream_watch.py` (DV3): `anthology-dev upstream-watch`.
//!
//! Everything runs against the fixtures (`tests/fixtures/upstream_watch`) and injected fakes
//! (`support/upstream_watch.rs`), except the client tests, which talk to local servers, and the
//! one git test. The Python's generated-command assertions are F-17 (`scripts/dev.sh …`): those
//! tests keep their name with a `_names_dev_sh` suffix and assert the new text.

#[path = "support/upstream_watch.rs"]
mod uws;

#[path = "support/agents.rs"]
mod agents;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;

use anthology_core::Env;
use anthology_dev::json_edit::Uncaught;
use anthology_dev::startup_budget::Root;
use anthology_dev::upstream_watch::{self as uw, Change, Checker, Deps, Error, GitHub, Sections};
use anthology_pycompat::json::{self, PY_DEFAULT, Value};
use uws::*;

fn body_of(change: &Change) -> String {
    uw::render_body(change, &[])
}

fn json_text(change: &Change) -> String {
    json::dumps(&change.to_json(), &PY_DEFAULT)
}

fn is_watch(r: &Result<Option<Change>, Error>) -> bool {
    matches!(r, Err(Error::Watch(_)))
}

mod parse_ls_remote_tests {
    use super::*;

    #[test]
    fn test_parses_refs_and_peeled_entries() {
        let refs = refs_of("ls_remote_tags.txt");
        let get = |k: &str| refs.iter().find(|(r, _)| r == k).map(|(_, v)| v.clone());
        assert_eq!(get("refs/heads/main"), Some(s('1')));
        assert_eq!(get("refs/tags/v7.1.0"), Some(s('6')));
        assert_eq!(get("refs/tags/v7.1.0^{}"), Some(s('7')));
    }

    #[test]
    fn test_ignores_malformed_lines() {
        let text = format!(
            "nothex\trefs/tags/v1.0.0\n{}\n\n{}\trefs/tags/v2.0.0\n{}  refs/tags/x\n",
            s('1'),
            s('2'),
            s('3')
        );
        assert_eq!(
            uw::parse_ls_remote(&text),
            refs(&[("refs/tags/v2.0.0", &s('2'))])
        );
    }
}

mod tag_pinned_tests {
    use super::*;

    fn net(name: &str, drop: &[&str], http: Vec<(String, Vec<u8>)>) -> FakeNet {
        let mut n = FakeNet::new().refs(SP_REPO, drop_refs(refs_of(name), drop));
        for (url, body) in http {
            n = n.http(&url, body);
        }
        n
    }

    #[test]
    fn test_newer_tag_found_with_peeled_commit_names_dev_sh() {
        let net = net(
            "ls_remote_tags.txt",
            &["refs/tags/v7.10.0"],
            vec![(
                compare_url(SP_REPO, &s('5'), &s('7')),
                compare_json(&["skills/a/SKILL.md", "README.md"], 1),
            )],
        );
        let change = check(&tag_entry(vec![]), &net, None).unwrap().unwrap();
        assert_eq!(
            (
                change.key.as_str(),
                change.old.as_str(),
                change.new.as_str()
            ),
            ("superpowers", "v7.0.2", "v7.1.0")
        );
        assert_eq!(change.new_target, "v7.1.0");
        assert!(body_of(&change).contains(&s('7'))); // peeled commit, not the tag object
        assert!(!body_of(&change).contains(&s('6')));
        // F-17: the Python names `python3 scripts/vendor.py sync ...`.
        assert_eq!(
            change.command,
            "scripts/dev.sh vendor sync superpowers --ref v7.1.0"
        );
        assert_eq!(change.files, ["skills/a/SKILL.md", "README.md"]);
    }

    #[test]
    fn test_semver_order_not_string_order_and_lightweight_tag() {
        let net = net(
            "ls_remote_tags.txt",
            &[],
            vec![(
                compare_url(SP_REPO, &s('5'), &s('a')),
                compare_json(&["x.md"], 1),
            )],
        );
        let change = check(&tag_entry(vec![]), &net, None).unwrap().unwrap();
        assert_eq!(change.new, "v7.10.0"); // 7.10 > 7.1
        assert!(body_of(&change).contains(&s('a')));
    }

    #[test]
    fn test_prereleases_and_non_version_tags_ignored() {
        let net = net(
            "ls_remote_tags.txt",
            &[
                "refs/tags/v7.10.0",
                "refs/tags/v7.1.0",
                "refs/tags/v7.1.0^{}",
            ],
            vec![],
        );
        // only rc/beta/nightly are newer
        assert_eq!(check(&tag_entry(vec![]), &net, None), Ok(None));
    }

    #[test]
    fn test_pin_newer_than_every_tag_is_no_change() {
        let net = net("ls_remote_tags.txt", &[], vec![]);
        assert_eq!(
            check(&tag_entry(vec![("ref", st("v8.0.0"))]), &net, None),
            Ok(None)
        );
    }

    #[test]
    fn test_up_to_date() {
        let net = net("ls_remote_up_to_date.txt", &[], vec![]);
        assert_eq!(check(&tag_entry(vec![]), &net, None), Ok(None));
        assert!(net.http_calls.borrow().is_empty());
    }

    #[test]
    fn test_keeps_the_pin_prefix_convention() {
        let net = FakeNet::new()
            .refs(SP_REPO, refs_of("ls_remote_no_prefix.txt"))
            .http(
                &compare_url(SP_REPO, &s('5'), &s('2')),
                compare_json(&[], 1),
            );
        let change = check(&tag_entry(vec![("ref", st("1.2.0"))]), &net, None)
            .unwrap()
            .unwrap();
        assert_eq!(change.new, "1.3"); // not v1.9.0
        assert!(change.command.contains("--ref 1.3"));
    }

    #[test]
    fn test_unprefixed_pin_two_components() {
        let net = FakeNet::new()
            .refs(SP_REPO, refs_of("ls_remote_tags.txt"))
            .http(
                &compare_url(SP_REPO, &s('5'), &s('a')),
                compare_json(&[], 1),
            );
        let change = check(&tag_entry(vec![("ref", st("v7.0"))]), &net, None)
            .unwrap()
            .unwrap();
        assert_eq!(change.new, "v7.10.0");
    }

    #[test]
    fn test_compare_failure_degrades_to_a_note() {
        // no compare response registered
        let net = net("ls_remote_tags.txt", &["refs/tags/v7.10.0"], vec![]);
        let change = check(&tag_entry(vec![]), &net, None).unwrap().unwrap();
        assert_eq!(change.new, "v7.1.0");
        assert!(change.notes.iter().any(|n| n.contains("unavailable")));
    }

    #[test]
    fn test_path_filter_applies_to_tag_diffs() {
        let net = net(
            "ls_remote_tags.txt",
            &["refs/tags/v7.10.0"],
            vec![(
                compare_url(SP_REPO, &s('5'), &s('7')),
                fixture("compare_path_changed.json"),
            )],
        );
        let change = check(
            &tag_entry(vec![("path", st("skills/frontend-design"))]),
            &net,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            change.files,
            [
                "skills/frontend-design/SKILL.md",
                "skills/frontend-design/examples/hero.md"
            ]
        );
    }

    #[test]
    fn test_missing_tag_list_is_an_error() {
        let net = FakeNet::new().refs(SP_REPO, refs(&[("refs/heads/main", &s('1'))]));
        assert!(is_watch(&check(&tag_entry(vec![]), &net, None)));
    }
}

mod branch_pinned_tests {
    use super::*;

    fn net(head: &str, compare: Option<Vec<u8>>) -> FakeNet {
        let mut n = FakeNet::new().refs(SKILLS_REPO, refs(&[("refs/heads/main", head)]));
        if let Some(c) = compare {
            n = n.http(&compare_url(SKILLS_REPO, &s('1'), head), c);
        }
        n
    }

    #[test]
    fn test_moved_with_changes_under_path_names_dev_sh() {
        let net = net(&s('2'), Some(fixture("compare_path_changed.json")));
        let change = check(&git_entry(vec![]), &net, None).unwrap().unwrap();
        assert_eq!(
            (change.old.as_str(), change.new.as_str()),
            (&s('1')[..12], &s('2')[..12])
        );
        assert_eq!(
            change.files,
            [
                "skills/frontend-design/SKILL.md",
                "skills/frontend-design/examples/hero.md"
            ]
        );
        // F-17: the Python names `python3 scripts/vendor.py sync ...`.
        assert_eq!(
            change.command,
            "scripts/dev.sh vendor sync frontend-design --ref main"
        );
        assert!(body_of(&change).contains(&s('2')));
    }

    #[test]
    fn test_files_heading_keeps_the_backticks_around_the_path() {
        let net = net(&s('2'), Some(fixture("compare_path_changed.json")));
        let body = body_of(&check(&git_entry(vec![]), &net, None).unwrap().unwrap());
        assert!(body.contains("Changed files under `skills/frontend-design` (2):"));
    }

    #[test]
    fn test_prefix_match_is_by_directory() {
        let net = net(&s('2'), Some(fixture("compare_path_changed.json")));
        let change = check(&git_entry(vec![]), &net, None).unwrap().unwrap();
        assert!(
            !change
                .files
                .contains(&"skills/frontend-designer/SKILL.md".to_string())
        );
    }

    #[test]
    fn test_unchanged_head_makes_no_api_call() {
        let net = net(&s('1'), None);
        assert_eq!(check(&git_entry(vec![]), &net, None), Ok(None));
        assert!(net.http_calls.borrow().is_empty());
    }

    #[test]
    fn test_moved_elsewhere_is_no_change() {
        let net = net(&s('2'), Some(fixture("compare_other_paths.json")));
        assert_eq!(check(&git_entry(vec![]), &net, None), Ok(None));
    }

    #[test]
    fn test_empty_path_means_whole_repo() {
        let net = net(&s('2'), Some(fixture("compare_other_paths.json")));
        let change = check(&git_entry(vec![("path", st(""))]), &net, None)
            .unwrap()
            .unwrap();
        assert_eq!(change.files, ["skills/other-skill/SKILL.md", "README.md"]);
    }

    #[test]
    fn test_truncated_compare_is_a_change_with_a_note() {
        let files: Vec<String> = (0..300).map(|i| format!("elsewhere/f{i}.md")).collect();
        let names: Vec<&str> = files.iter().map(String::as_str).collect();
        let net = net(&s('2'), Some(compare_json(&names, 1)));
        // nothing under path is visible, but 300 means truncated
        let change = check(&git_entry(vec![]), &net, None).unwrap();
        assert!(change.is_some());
        assert!(body_of(&change.unwrap()).contains("diff too large to summarize"));
    }

    #[test]
    fn test_non_github_repo_is_a_change_with_commit_range_only() {
        let repo = "https://git.example.test/team/skills";
        let net = FakeNet::new().refs(repo, refs(&[("refs/heads/main", &s('2'))]));
        let change = check(&git_entry(vec![("repo", st(repo))]), &net, None)
            .unwrap()
            .unwrap();
        assert!(net.http_calls.borrow().is_empty());
        assert!(body_of(&change).contains(&s('1')));
        assert!(body_of(&change).contains(&s('2')));
        assert!(change.files.is_empty());
    }

    #[test]
    fn test_missing_branch_is_an_error() {
        let net = FakeNet::new().refs(SKILLS_REPO, refs(&[("refs/heads/other", &s('2'))]));
        assert!(is_watch(&check(&git_entry(vec![]), &net, None)));
    }

    #[test]
    fn test_compare_is_cached_per_repo_and_commits() {
        let net = net(&s('2'), Some(fixture("compare_path_changed.json")));
        let root = repo_root();
        let mut checker = Checker::new(&root, &net, None);
        let a = checker
            .check_entry(&git_entry(vec![
                ("name", st("a-one")),
                ("path", st("skills/frontend-design")),
            ]))
            .unwrap()
            .unwrap();
        let b = checker
            .check_entry(&git_entry(vec![
                ("name", st("a-two")),
                ("path", st("skills/other-skill")),
            ]))
            .unwrap()
            .unwrap();
        assert_eq!(net.http_calls.borrow().len(), 1);
        assert_eq!(net.ls_calls.borrow().len(), 1);
        assert_eq!(a.files[0], "skills/frontend-design/SKILL.md");
        assert_eq!(b.files, ["skills/other-skill/SKILL.md"]);
    }

    #[test]
    fn test_dot_git_suffix_is_stripped_for_the_api() {
        let repo = format!("{SKILLS_REPO}.git");
        let net = FakeNet::new()
            .refs(&repo, refs(&[("refs/heads/main", &s('2'))]))
            .http(
                &compare_url(SKILLS_REPO, &s('1'), &s('2')),
                compare_json(&["skills/frontend-design/a.md"], 1),
            );
        let change = check(&git_entry(vec![("repo", st(&repo))]), &net, None).unwrap();
        assert!(change.is_some());
    }

    #[test]
    fn test_compare_error_is_an_error_when_it_decides_the_verdict() {
        let net = FakeNet::new().refs(SKILLS_REPO, refs(&[("refs/heads/main", &s('2'))]));
        assert!(is_watch(&check(&git_entry(vec![]), &net, None)));
    }

    #[test]
    fn test_commit_messages_and_authors_never_appear() {
        let net = net(&s('2'), Some(fixture("compare_path_changed.json")));
        let change = check(&git_entry(vec![]), &net, None).unwrap().unwrap();
        let text = uw::render_title(&change) + &body_of(&change) + &json_text(&change);
        for needle in ["SECRET-COMMIT-MESSAGE", "mallory", "Mallory"] {
            assert!(!text.contains(needle), "{needle}");
        }
    }
}

mod ls_remote_failure_cache_tests {
    use super::*;

    #[test]
    fn test_an_unreachable_repo_is_tried_once_per_run() {
        let net = FakeNet::new().refs_fail(SKILLS_REPO, "OSError");
        let root = repo_root();
        let mut checker = Checker::new(&root, &net, None);
        for name in ["one", "two", "three"] {
            let r = checker.check_entry(&git_entry(vec![("name", st(name))]));
            assert_eq!(r, Err(Error::Watch("cannot list refs (OSError)".into())));
        }
        assert_eq!(*net.ls_calls.borrow(), [SKILLS_REPO]);
    }
}

mod url_source_tests {
    use super::*;

    #[test]
    fn test_changed_checksum_with_file_level_diff_names_dev_sh() {
        let tmp = make_root(None, "2.1.329", true);
        let net = FakeNet::new().http(&ui_manifest(), fixture("manifest_new.txt"));
        let change = check(&url_entry(vec![]), &net, Some(tmp.path()))
            .unwrap()
            .unwrap();
        assert_eq!(
            (change.key.clone(), change.old.clone(), change.new.clone()),
            (
                "ui-taste".to_string(),
                old_sum()[..12].to_string(),
                new_sum()[..12].to_string()
            )
        );
        assert_eq!(*net.http_calls.borrow(), [ui_manifest()]);
        assert_eq!(
            change.files,
            [
                "+ references/added.md",
                "- references/removed.md",
                "~ references/changed.md"
            ]
        );
        assert!(body_of(&change).contains(&new_sum()));
        // F-17: the Python names `python3 scripts/vendor.py fetch ... && ... build ...`.
        assert_eq!(
            change.command,
            format!(
                "set \"checksum\" of ui-taste in vendor/sources.json to {}, then run: \
                 scripts/dev.sh vendor fetch ui-taste && scripts/dev.sh vendor build ui-taste",
                new_sum()
            )
        );
    }

    #[test]
    fn test_unchanged() {
        let net = FakeNet::new().http(&ui_manifest(), fixture("manifest_old.txt"));
        assert_eq!(check(&url_entry(vec![]), &net, None), Ok(None));
    }

    #[test]
    fn test_url_already_naming_the_manifest_is_used_as_is() {
        let url = "https://example.test/skills/ui-taste/CHECKSUMS.sha256";
        let net = FakeNet::new().http(url, fixture("manifest_old.txt"));
        assert_eq!(
            check(&url_entry(vec![("url", st(url))]), &net, None),
            Ok(None)
        );
        assert_eq!(*net.http_calls.borrow(), [url]);
    }

    #[test]
    fn test_trailing_slash() {
        let net = FakeNet::new().http(&ui_manifest(), fixture("manifest_old.txt"));
        let entry = url_entry(vec![("url", st(&format!("{UI_URL}/")))]);
        assert_eq!(check(&entry, &net, None), Ok(None));
    }

    #[test]
    fn test_missing_snapshot_is_noted() {
        let tmp = make_root(None, "2.1.329", false);
        let net = FakeNet::new().http(&ui_manifest(), fixture("manifest_new.txt"));
        let change = check(&url_entry(vec![]), &net, Some(tmp.path()))
            .unwrap()
            .unwrap();
        assert!(change.files.is_empty());
        assert!(
            change
                .notes
                .iter()
                .any(|n| n.contains("file-level diff unavailable"))
        );
    }

    #[test]
    fn test_fetch_failure_is_an_error() {
        let net = FakeNet::new().http_fail(&ui_manifest(), "OSError");
        assert_eq!(
            check(&url_entry(vec![]), &net, None),
            Err(Error::Watch("cannot fetch the manifest (OSError)".into()))
        );
    }

    #[test]
    fn test_non_https_url_is_an_error() {
        let entry = url_entry(vec![("url", st("file:///etc/passwd"))]);
        assert!(is_watch(&check(&entry, &FakeNet::new(), None)));
    }

    #[test]
    fn test_bad_checksum_in_sources_is_an_error() {
        let net = FakeNet::new().http(&ui_manifest(), Vec::new());
        assert!(is_watch(&check(
            &url_entry(vec![("checksum", st("zz"))]),
            &net,
            None
        )));
    }
}

mod subtree_tests {
    use super::*;

    #[test]
    fn test_newer_upstream_tag() {
        let r = refs(&[
            ("refs/tags/v1.2.1", &s('3')),
            ("refs/tags/v1.3.0", &s('8')),
            ("refs/tags/v1.3.0^{}", &s('9')),
            ("refs/tags/v1.4.0-rc.1", &s('a')),
            ("refs/heads/develop", &s('b')),
        ]);
        let net = FakeNet::new().refs(ATL_REPO, r).http(
            &compare_url(ATL_REPO, &s('3'), &s('9')),
            compare_json(&["src/main.rs"], 1),
        );
        let change = check(&atl_entry("v1.2.1", &s('3')), &net, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                change.key.as_str(),
                change.old.as_str(),
                change.new.as_str()
            ),
            ("atl", "v1.2.1", "v1.3.0")
        );
        assert_eq!(change.command, "scripts/sync-atl.sh v1.3.0");
        assert_eq!(*net.ls_calls.borrow(), [ATL_REPO]); // the fork is ours: never queried
        assert_eq!(change.files, ["src/main.rs"]);
    }

    #[test]
    fn test_up_to_date() {
        let net = FakeNet::new().refs(ATL_REPO, refs(&[("refs/tags/v1.2.1", &s('3'))]));
        assert_eq!(check(&atl_entry("v1.2.1", &s('3')), &net, None), Ok(None));
    }
}

mod claude_code_tests {
    use super::*;

    fn check_cc(npm: &[u8], pinned: &str) -> Result<Option<Change>, Error> {
        let tmp = make_root(None, pinned, true);
        let net = FakeNet::new().http(NPM_URL, npm.to_vec());
        Checker::new(tmp.path(), &net, None).check_claude_code()
    }

    #[test]
    fn test_newer() {
        let change = check_cc(&fixture("npm_latest.json"), "2.1.329")
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                change.key.as_str(),
                change.old.as_str(),
                change.new.as_str()
            ),
            ("claude-code", "2.1.329", "2.1.400")
        );
        assert_eq!(
            change.command,
            "set \"@anthropic-ai/claude-code\" in ci/claude-code/package.json to 2.1.400, run \
             npm install --package-lock-only in ci/claude-code, and let CI (plugin-validate) and \
             evals run on the PR"
        );
        assert!(
            body_of(&change)
                .contains("https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md")
        );
    }

    #[test]
    fn test_equal_and_older() {
        assert_eq!(check_cc(&fixture("npm_same.json"), "2.1.329"), Ok(None));
        assert_eq!(check_cc(&fixture("npm_older.json"), "2.1.329"), Ok(None));
    }

    #[test]
    fn test_invalid_version_from_registry_is_an_error() {
        assert!(is_watch(&check_cc(
            b"{\"version\": \"2.1.400`x`\"}",
            "2.1.329"
        )));
        assert_eq!(
            check_cc(b"not json", "2.1.329"),
            Err(Error::Watch("request failed (JSONDecodeError)".into()))
        );
    }

    #[test]
    fn test_overlong_or_newline_terminated_version_is_an_error() {
        for version in [
            format!("2.1.400-{}", "a".repeat(64)),
            "2.1.400\n".to_string(),
        ] {
            let npm = json::dumps(&obj(vec![("version", st(&version))]), &PY_DEFAULT);
            assert!(
                is_watch(&check_cc(npm.as_bytes(), "2.1.329")),
                "{version:?}"
            );
        }
    }

    #[test]
    fn test_prerelease_latest_is_not_newer_than_its_release() {
        assert_eq!(
            check_cc(b"{\"version\": \"2.1.329-beta.1\"}", "2.1.329"),
            Ok(None)
        );
    }
}

mod check_all_tests {
    use super::*;

    fn keys(changes: &[Change]) -> Vec<&str> {
        changes.iter().map(|c| c.key.as_str()).collect()
    }

    fn error_keys(errors: &[(String, String)]) -> Vec<&str> {
        errors.iter().map(|(k, _)| k.as_str()).collect()
    }

    #[test]
    fn test_unknown_shape_errors_and_the_rest_continue() {
        let tmp = make_root(None, "2.1.329", true);
        // frontend-design's pinned commit in the fixture differs from S["1"]; keep it unmoved
        let sources = fixture_sources();
        let pinned = sources.as_array().unwrap()[1]
            .as_object()
            .unwrap()
            .get("commit")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        let net = FakeNet::new()
            .refs(SP_REPO, refs_of("ls_remote_tags.txt"))
            .refs(SKILLS_REPO, refs(&[("refs/heads/main", &pinned)]))
            .refs(ATL_REPO, refs(&[("refs/tags/v1.2.1", &s('3'))]))
            .http(
                &compare_url(SP_REPO, &s('5'), &s('a')),
                compare_json(&["x.md"], 1),
            )
            .http(&ui_manifest(), fixture("manifest_new.txt"))
            .http(NPM_URL, fixture("npm_latest.json"));
        let (changes, errors) = uw::check_all(tmp.path(), &net, None);
        assert_eq!(keys(&changes), ["superpowers", "ui-taste", "claude-code"]);
        assert_eq!(error_keys(&errors), ["mystery"]);
        assert!(errors[0].1.contains("unrecognized source shape"));
    }

    #[test]
    fn test_one_failing_network_entry_does_not_stop_the_others() {
        let sources = Value::Array(vec![tag_entry(vec![]), url_entry(vec![])]);
        let tmp = make_root(Some(sources), "2.1.329", true);
        let net = FakeNet::new()
            .refs_fail(SP_REPO, "OSError")
            .http(&ui_manifest(), fixture("manifest_new.txt"))
            .http(NPM_URL, fixture("npm_same.json"));
        let (changes, errors) = uw::check_all(tmp.path(), &net, None);
        assert_eq!(keys(&changes), ["ui-taste"]);
        assert_eq!(error_keys(&errors), ["superpowers"]);
    }

    #[test]
    fn test_claude_code_failure_is_recorded_under_its_key() {
        let tmp = make_root(Some(Value::Array(vec![])), "2.1.329", true);
        let net = FakeNet::new().http_fail(NPM_URL, "OSError");
        let (changes, errors) = uw::check_all(tmp.path(), &net, None);
        assert!(changes.is_empty());
        assert_eq!(error_keys(&errors), ["claude-code"]);
    }

    #[test]
    fn test_the_real_sources_file_is_readable_generically() {
        let text = std::fs::read_to_string(repo_root().join("vendor/sources.json")).unwrap();
        let data = json::loads(&text).unwrap();
        let entries = data.as_object().unwrap().get("sources").unwrap();
        for entry in entries.as_array().unwrap() {
            let name = entry
                .as_object()
                .unwrap()
                .get("name")
                .and_then(Value::as_str);
            assert!(uw::classify(entry).is_ok(), "{name:?}");
        }
    }
}

mod hostile_input_tests {
    use super::*;

    #[test]
    fn test_hostile_tags_are_never_selected_or_rendered() {
        let net = FakeNet::new()
            .refs(SP_REPO, refs_of("ls_remote_tags.txt"))
            .http(
                &compare_url(SP_REPO, &s('5'), &s('a')),
                compare_json(&["x.md"], 1),
            );
        let change = check(&tag_entry(vec![]), &net, None).unwrap().unwrap();
        let text = uw::render_title(&change) + &body_of(&change) + &json_text(&change);
        for needle in ["touch-x", "$(id)", "octocat", "`touch"] {
            assert!(!text.contains(needle), "{needle}");
        }
    }

    #[test]
    fn test_hostile_pin_ref_errors() {
        for r in [
            "v1.0.0`x`",
            "main\nINJECT",
            "$(id)",
            "a b",
            "@octocat",
            "-rf",
            "",
        ] {
            let net = FakeNet::new().refs(SKILLS_REPO, refs(&[("refs/heads/main", &s('1'))]));
            assert!(
                is_watch(&check(&git_entry(vec![("ref", st(r))]), &net, None)),
                "{r:?}"
            );
        }
    }

    #[test]
    fn test_hostile_name_errors() {
        for name in ["a`b", "a b", "x\ny", "../x", "a$(id)", ""] {
            assert!(
                is_watch(&check(
                    &git_entry(vec![("name", st(name))]),
                    &FakeNet::new(),
                    None
                )),
                "{name:?}"
            );
        }
    }

    #[test]
    fn test_trailing_newline_is_rejected_everywhere() {
        let net = FakeNet::new()
            .refs(SKILLS_REPO, refs(&[("refs/heads/main", &s('1'))]))
            .refs(SP_REPO, refs_of("ls_remote_tags.txt"))
            .http(&ui_manifest(), fixture("manifest_old.txt"));
        let cases = [
            git_entry(vec![("name", st("demo\n"))]),
            git_entry(vec![("ref", st("main\n"))]),
            git_entry(vec![("commit", st(&format!("{}\n", s('1'))))]),
            git_entry(vec![("repo", st(&format!("{SKILLS_REPO}\n")))]),
            git_entry(vec![("path", st("skills\n"))]),
            tag_entry(vec![("ref", st("v7.0.2\n"))]),
            url_entry(vec![("checksum", st(&format!("{}\n", old_sum())))]),
            url_entry(vec![("url", st(&format!("{UI_URL}\n")))]),
        ];
        for entry in &cases {
            assert!(is_watch(&check(entry, &net, None)), "{entry:?}");
        }
    }

    #[test]
    fn test_hostile_commit_sha_errors() {
        let net = FakeNet::new().refs(SKILLS_REPO, refs(&[("refs/heads/main", &s('2'))]));
        assert!(is_watch(&check(
            &git_entry(vec![("commit", st("not-a-sha"))]),
            &net,
            None
        )));
    }

    #[test]
    fn test_hostile_head_sha_from_upstream_errors() {
        let net = FakeNet::new().refs(SKILLS_REPO, refs(&[("refs/heads/main", "zzz")]));
        assert!(is_watch(&check(&git_entry(vec![]), &net, None)));
    }

    #[test]
    fn test_hostile_repo_url_errors() {
        for repo in [
            "--upload-pack=touch x",
            "file:///etc",
            "https://x/y z",
            "ssh://h/r",
        ] {
            assert!(
                is_watch(&check(
                    &git_entry(vec![("repo", st(repo))]),
                    &FakeNet::new(),
                    None
                )),
                "{repo}"
            );
        }
    }

    #[test]
    fn test_hostile_file_paths_are_sanitised_inside_the_fence() {
        let net = FakeNet::new()
            .refs(SKILLS_REPO, refs(&[("refs/heads/main", &s('2'))]))
            .http(
                &compare_url(SKILLS_REPO, &s('1'), &s('2')),
                fixture("compare_hostile_paths.json"),
            );
        let change = check(&git_entry(vec![("path", st("docs"))]), &net, None)
            .unwrap()
            .unwrap();
        let body = body_of(&change);
        let fence = body
            .split("```text\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap();
        assert_eq!(fence.matches('\n').count(), 1); // two paths, no injected newline
        assert!(!fence.contains('`'));
        assert!(!body.contains('\x07'));
        assert_eq!(fence.lines().nth(1), Some("docs/ok.md"));
    }

    #[test]
    fn test_sanitise_helpers() {
        assert_eq!(uw::sanitize_path("a`b\nc\x00d\u{202e}e"), "abcde");
        assert_eq!(
            uw::sanitize_path(&"x".repeat(1000)).chars().count(),
            uw::MAX_PATH_LEN
        );
    }
}

mod render_tests {
    use super::*;

    fn change(files: Vec<String>, files_total: usize) -> Change {
        let mut c = Change::new(
            "demo",
            "git",
            "v1.0.0",
            "v1.1.0",
            "v1.1.0",
            "scripts/dev.sh vendor sync demo --ref v1.1.0",
        )
        .with_details(&[
            ("Source", "https://github.com/o/r"),
            ("New commit", &s('7')),
        ]);
        c.files = files;
        c.files_total = files_total;
        c
    }

    #[test]
    fn test_title_and_marker() {
        let c = change(vec![], 0);
        assert_eq!(
            uw::render_title(&c),
            "Upstream: demo v1.0.0 \u{2192} v1.1.0"
        );
        assert!(body_of(&c).starts_with("<!-- upstream-watch:demo -->\n"));
    }

    #[test]
    fn test_body_has_command_and_gate_text() {
        let body = body_of(&change(vec![], 0));
        assert!(body.contains("scripts/dev.sh vendor sync demo --ref v1.1.0"));
        assert!(body.contains("on a branch"));
        assert!(body.contains("CI and the evals are the gate"));
    }

    #[test]
    fn test_body_is_deterministic() {
        assert_eq!(body_of(&change(vec![], 0)), body_of(&change(vec![], 0)));
    }

    #[test]
    fn test_extra_sections_come_before_the_command() {
        let extra = vec![("Startup budget".to_string(), "+3 tokens".to_string())];
        let body = uw::render_body(&change(vec![], 0), &extra);
        assert!(body.contains("### Startup budget\n+3 tokens"));
        assert!(body.find("### Startup budget") < body.find("scripts/dev.sh vendor sync"));
        assert!(!body_of(&change(vec![], 0)).contains("Startup budget"));
    }

    #[test]
    fn test_at_most_50_files_and_a_remainder_note() {
        let files: Vec<String> = (0..80).map(|i| format!("f{i:03}.md")).collect();
        let body = body_of(&change(files, 80));
        assert!(body.contains("f049.md"));
        assert!(!body.contains("f050.md"));
        assert!(body.contains("30 more"));
    }

    #[test]
    fn test_json_shape() {
        let data = change(vec![], 0).to_json();
        let m = data.as_object().unwrap();
        for key in [
            "key",
            "kind",
            "old",
            "new",
            "new_target",
            "command",
            "files",
            "notes",
            "title",
        ] {
            assert!(m.contains_key(key), "{key}");
        }
        json::dumps(&data, &PY_DEFAULT);
    }
}

mod sync_issues_tests {
    use super::*;

    pub fn change(key: &str, new: &str) -> Change {
        Change::new(
            key,
            "git",
            "v1.0.0",
            new,
            new,
            &format!("scripts/dev.sh vendor sync {key} --ref {new}"),
        )
        .with_details(&[("Source", "https://github.com/o/r")])
    }

    fn demo() -> Change {
        change("demo", "v1.1.0")
    }

    fn sync(changes: &[Change], gh: &FakeGitHub, dry_run: bool) -> Vec<json::Map> {
        uw::sync_issues(changes, Some(gh as &dyn GitHub), dry_run, &HashMap::new()).unwrap()
    }

    fn action(a: &json::Map) -> &str {
        a.get("action").and_then(Value::as_str).unwrap()
    }

    #[test]
    fn test_creates_when_absent() {
        let gh = FakeGitHub::new(vec![]);
        let actions = sync(&[demo()], &gh, false);
        assert_eq!(gh.kinds(), ["list", "create"]);
        assert_eq!(
            gh.calls.borrow()[1],
            Call::Create(uw::render_title(&demo()), body_of(&demo()))
        );
        assert_eq!(
            actions
                .iter()
                .map(|a| (a.get("key").and_then(Value::as_str).unwrap(), action(a)))
                .collect::<Vec<_>>(),
            [("demo", "create")]
        );
    }

    #[test]
    fn test_noop_when_identical() {
        let gh = FakeGitHub::new(vec![bot_issue(&demo(), 7, vec![])]);
        let actions = sync(&[demo()], &gh, false);
        assert!(gh.writes().is_empty());
        assert_eq!(action(&actions[0]), "unchanged");
        assert_eq!(actions[0].get("number"), Some(&int(7)));
    }

    #[test]
    fn test_noop_tolerates_crlf_and_trailing_whitespace() {
        let body = body_of(&demo()).replace('\n', "\r\n") + "\r\n";
        let gh = FakeGitHub::new(vec![bot_issue(&demo(), 7, vec![("body", st(&body))])]);
        sync(&[demo()], &gh, false);
        assert!(gh.writes().is_empty());
    }

    #[test]
    fn test_patches_when_upstream_moved() {
        let gh = FakeGitHub::new(vec![bot_issue(&change("demo", "v1.0.5"), 12, vec![])]);
        let actions = sync(&[demo()], &gh, false);
        assert_eq!(
            gh.writes(),
            [Call::Update(
                int(12),
                uw::render_title(&demo()),
                body_of(&demo())
            )]
        );
        assert_eq!(action(&actions[0]), "update");
    }

    #[test]
    fn test_ignores_other_keys_and_pull_requests() {
        let other = bot_issue(&change("other", "v1.1.0"), 3, vec![]);
        let pull = bot_issue(
            &demo(),
            4,
            vec![("pull_request", obj(vec![("url", st("x"))]))],
        );
        let unmarked = bot_issue(&demo(), 5, vec![("body", st("no marker here"))]);
        let gh = FakeGitHub::new(vec![other, pull, unmarked]);
        let actions = sync(&[demo()], &gh, false);
        assert_eq!(gh.write_kinds(), ["create"]);
        assert_eq!(action(&actions[0]), "create");
    }

    #[test]
    fn test_human_authored_or_unlabelled_issues_are_ignored() {
        let human = bot_issue(
            &demo(),
            8,
            vec![("user", obj(vec![("login", st("mallory"))]))],
        );
        let no_user = bot_issue(&demo(), 9, vec![("user", Value::Null)]);
        let unlabeled = bot_issue(
            &demo(),
            10,
            vec![("labels", Value::Array(vec![obj(vec![("name", st("bug"))])]))],
        );
        let gh = FakeGitHub::new(vec![human, no_user, unlabeled]);
        let actions = sync(&[demo()], &gh, false);
        assert_eq!(gh.write_kinds(), ["create"]);
        assert_eq!(action(&actions[0]), "create");
    }

    #[test]
    fn test_the_newest_issue_per_key_decides() {
        let stale_open = bot_issue(&change("demo", "v1.0.5"), 3, vec![]);
        let current_closed = bot_issue(&demo(), 9, vec![("state", st("closed"))]);
        let gh = FakeGitHub::new(vec![stale_open, current_closed]);
        let actions = sync(&[demo()], &gh, false);
        assert!(gh.writes().is_empty());
        assert_eq!(action(&actions[0]), "dismissed");
    }

    #[test]
    fn test_closed_issue_with_the_same_body_is_dismissed() {
        let gh = FakeGitHub::new(vec![bot_issue(&demo(), 4, vec![("state", st("closed"))])]);
        let actions = sync(&[demo()], &gh, false);
        assert!(gh.writes().is_empty());
        assert_eq!(action(&actions[0]), "dismissed");
        assert_eq!(actions[0].get("number"), Some(&int(4)));
    }

    #[test]
    fn test_closed_issue_with_a_different_body_gets_a_new_issue() {
        let gh = FakeGitHub::new(vec![bot_issue(
            &change("demo", "v1.0.5"),
            4,
            vec![("state", st("closed"))],
        )]);
        let actions = sync(&[demo()], &gh, false);
        assert_eq!(gh.write_kinds(), ["create"]);
        assert_eq!(action(&actions[0]), "create");
    }

    #[test]
    fn test_a_newer_open_issue_is_patched_not_the_closed_one() {
        let closed = bot_issue(&change("demo", "v1.0.4"), 2, vec![("state", st("closed"))]);
        let opened = bot_issue(&change("demo", "v1.0.5"), 6, vec![]);
        let gh = FakeGitHub::new(vec![closed, opened]);
        sync(&[demo()], &gh, false);
        let writes = gh.writes();
        assert_eq!(writes.len(), 1);
        assert!(matches!(&writes[0], Call::Update(n, _, _) if *n == int(6)));
    }

    #[test]
    fn test_marker_must_start_the_body() {
        let quoted = bot_issue(
            &demo(),
            6,
            vec![("body", st("see <!-- upstream-watch:demo --> in the text"))],
        );
        let gh = FakeGitHub::new(vec![quoted]);
        sync(&[demo()], &gh, false);
        assert_eq!(gh.write_kinds(), ["create"]);
    }

    #[test]
    fn test_dry_run_reads_but_never_writes() {
        let gh = FakeGitHub::new(vec![bot_issue(&change("demo", "v1.0.5"), 12, vec![])]);
        let actions = sync(&[demo(), change("fresh", "v1.1.0")], &gh, true);
        assert!(gh.writes().is_empty());
        assert_eq!(
            actions.iter().map(action).collect::<Vec<_>>(),
            ["update", "create"]
        );
    }

    #[test]
    fn test_dry_run_without_a_client() {
        let actions = uw::sync_issues(&[demo()], None, true, &HashMap::new()).unwrap();
        assert_eq!(action(&actions[0]), "create-or-update");
    }

    #[test]
    fn test_write_failure_is_recorded_and_the_rest_continue() {
        let mut gh = FakeGitHub::new(vec![]);
        gh.fail_title = Some("bad".into());
        let actions = sync(
            &[change("bad", "v1.1.0"), change("good", "v1.1.0")],
            &gh,
            false,
        );
        assert_eq!(
            actions
                .iter()
                .map(|a| (a.get("key").and_then(Value::as_str).unwrap(), action(a)))
                .collect::<Vec<_>>(),
            [("bad", "error"), ("good", "create")]
        );
    }
}

/// `MainTests.run_main`: `main(argv, env, net, gh)` in a scratch working folder.
fn run_main(
    argv: &[&str],
    env: &[(&str, &str)],
    net: &FakeNet,
    gh: Option<&FakeGitHub>,
    sections: Option<uw::SectionsFn>,
) -> (i32, String) {
    let mut e = Env::default();
    for (k, v) in env {
        e = e.with(k, v);
    }
    let mut deps = Deps {
        net: Some(net),
        gh: gh.map(|g| g as &dyn GitHub),
        ..Deps::default()
    };
    if let Some(f) = sections {
        deps.startup_budget_sections = f;
    }
    let args: Vec<OsString> = argv.iter().map(OsString::from).collect();
    let cwd = std::env::temp_dir();
    let out = uw::main_with(&args, &e, &cwd, &deps);
    (out.code, String::from_utf8(out.stdout).unwrap())
}

mod main_tests {
    use super::*;

    pub fn fake_world(claude_new: bool, sources: Option<Value>) -> (tempfile::TempDir, FakeNet) {
        let sources =
            sources.unwrap_or_else(|| Value::Array(vec![tag_entry(vec![]), url_entry(vec![])]));
        let tmp = make_root(Some(sources), "2.1.329", true);
        let net = FakeNet::new()
            .refs(SP_REPO, refs_of("ls_remote_tags.txt"))
            .http(
                &compare_url(SP_REPO, &s('5'), &s('a')),
                compare_json(&["x.md"], 1),
            )
            .http(&ui_manifest(), fixture("manifest_new.txt"))
            .http(
                NPM_URL,
                fixture(if claude_new {
                    "npm_latest.json"
                } else {
                    "npm_same.json"
                }),
            );
        (tmp, net)
    }

    fn root_arg(tmp: &tempfile::TempDir) -> String {
        tmp.path().to_string_lossy().into_owned()
    }

    const REAL: [(&str, &str); 2] = [("GITHUB_TOKEN", "t"), ("GITHUB_REPOSITORY", "o/r")];

    fn real_env(out_file: &Path) -> Vec<(&'static str, String)> {
        vec![
            ("GITHUB_TOKEN", "t".into()),
            ("GITHUB_REPOSITORY", "o/r".into()),
            ("GITHUB_OUTPUT", out_file.to_string_lossy().into_owned()),
        ]
    }

    fn as_refs<'a>(env: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
        env.iter().map(|(k, v)| (*k, v.as_str())).collect()
    }

    #[test]
    fn test_dry_run_prints_issues_and_makes_no_writes() {
        let (tmp, net) = fake_world(true, None);
        let gh = FakeGitHub::new(vec![]);
        let (code, out) = run_main(
            &["--dry-run", "--root", &root_arg(&tmp)],
            &[],
            &net,
            Some(&gh),
            None,
        );
        assert_eq!(code, 0);
        assert!(gh.writes().is_empty());
        for needle in [
            "Upstream: superpowers v7.0.2",
            "Upstream: ui-taste",
            "Upstream: claude-code 2.1.329",
            "<!-- upstream-watch:superpowers -->",
        ] {
            assert!(out.contains(needle), "{needle}");
        }
    }

    #[test]
    fn test_dry_run_needs_no_token_or_repo() {
        let (tmp, net) = fake_world(true, None);
        let (code, _) = run_main(
            &["--dry-run", "--root", &root_arg(&tmp)],
            &[],
            &net,
            None,
            None,
        );
        assert_eq!(code, 0);
    }

    #[test]
    fn test_dry_run_does_not_write_github_output() {
        let (tmp, net) = fake_world(true, None);
        let d = tempfile::tempdir().unwrap();
        let out_file = d.path().join("out");
        let path = out_file.to_string_lossy().into_owned();
        run_main(
            &["--dry-run", "--root", &root_arg(&tmp)],
            &[("GITHUB_OUTPUT", &path)],
            &net,
            None,
            None,
        );
        assert!(!out_file.exists());
    }

    #[test]
    fn test_real_mode_requires_token_and_repo() {
        let (tmp, net) = fake_world(true, None);
        let (code, out) = run_main(&["--root", &root_arg(&tmp)], &[], &net, None, None);
        assert_eq!(code, 2);
        assert!(out.contains("GITHUB_TOKEN"));
    }

    #[test]
    fn test_real_mode_syncs_and_flags_claude_code() {
        let (tmp, net) = fake_world(true, None);
        let gh = FakeGitHub::new(vec![]);
        let d = tempfile::tempdir().unwrap();
        let out_file = d.path().join("out");
        let env = real_env(&out_file);
        let (code, _) = run_main(
            &["--root", &root_arg(&tmp)],
            &as_refs(&env),
            &net,
            Some(&gh),
            None,
        );
        assert_eq!(code, 0);
        assert_eq!(gh.writes().len(), 3);
        assert_eq!(
            std::fs::read_to_string(&out_file).unwrap(),
            "claude_code_changed=true\n"
        );
    }

    #[test]
    fn test_output_false_when_the_claude_code_issue_already_matches() {
        let (tmp, net) = fake_world(true, None);
        let current = Checker::new(tmp.path(), &net, None)
            .check_claude_code()
            .unwrap()
            .unwrap();
        let gh = FakeGitHub::new(vec![bot_issue(&current, 9, vec![])]);
        let d = tempfile::tempdir().unwrap();
        let out_file = d.path().join("out");
        let env = real_env(&out_file);
        run_main(
            &["--root", &root_arg(&tmp)],
            &as_refs(&env),
            &net,
            Some(&gh),
            None,
        );
        assert_eq!(
            std::fs::read_to_string(&out_file).unwrap(),
            "claude_code_changed=false\n"
        );
        assert_eq!(gh.writes().len(), 2); // the other two were created
    }

    #[test]
    fn test_output_true_when_the_claude_code_issue_moved() {
        let (tmp, net) = fake_world(true, None);
        let older = FakeNet::new().http(NPM_URL, b"{\"version\": \"2.1.350\"}".to_vec());
        let old = Checker::new(tmp.path(), &older, None)
            .check_claude_code()
            .unwrap()
            .unwrap();
        let gh = FakeGitHub::new(vec![bot_issue(&old, 9, vec![])]);
        let d = tempfile::tempdir().unwrap();
        let out_file = d.path().join("out");
        let env = real_env(&out_file);
        run_main(
            &["--root", &root_arg(&tmp)],
            &as_refs(&env),
            &net,
            Some(&gh),
            None,
        );
        assert_eq!(
            std::fs::read_to_string(&out_file).unwrap(),
            "claude_code_changed=true\n"
        );
        assert!(gh.kinds().contains(&"update"));
    }

    #[test]
    fn test_output_false_when_the_claude_code_issue_was_closed_as_dismissed() {
        let (tmp, net) = fake_world(true, None);
        let current = Checker::new(tmp.path(), &net, None)
            .check_claude_code()
            .unwrap()
            .unwrap();
        let gh = FakeGitHub::new(vec![bot_issue(&current, 9, vec![("state", st("closed"))])]);
        let d = tempfile::tempdir().unwrap();
        let out_file = d.path().join("out");
        let env = real_env(&out_file);
        run_main(
            &["--root", &root_arg(&tmp)],
            &as_refs(&env),
            &net,
            Some(&gh),
            None,
        );
        assert_eq!(
            std::fs::read_to_string(&out_file).unwrap(),
            "claude_code_changed=false\n"
        );
        assert_eq!(gh.writes().len(), 2); // superpowers and ui-taste only
    }

    #[test]
    fn test_dry_run_survives_a_failing_issue_listing() {
        let (tmp, net) = fake_world(true, None);
        let gh = FakeGitHub::failing("OSError");
        let (code, out) = run_main(
            &["--dry-run", "--root", &root_arg(&tmp)],
            &[],
            &net,
            Some(&gh),
            None,
        );
        assert_eq!(code, 1);
        assert!(out.contains("cannot list issues"));
        assert!(gh.writes().is_empty());
        assert!(out.contains("<!-- upstream-watch:superpowers -->"));
    }

    #[test]
    fn test_output_false_when_no_new_release() {
        let (tmp, net) = fake_world(false, None);
        let d = tempfile::tempdir().unwrap();
        let out_file = d.path().join("out");
        let env = real_env(&out_file);
        let gh = FakeGitHub::new(vec![]);
        run_main(
            &["--root", &root_arg(&tmp)],
            &as_refs(&env),
            &net,
            Some(&gh),
            None,
        );
        assert_eq!(
            std::fs::read_to_string(&out_file).unwrap(),
            "claude_code_changed=false\n"
        );
    }

    fn with_mystery() -> Value {
        Value::Array(vec![
            tag_entry(vec![]),
            obj(vec![("name", st("mystery")), ("target", st("t"))]),
        ])
    }

    #[test]
    fn test_errors_exit_1_after_issues_are_synced() {
        let (tmp, net) = fake_world(true, Some(with_mystery()));
        let gh = FakeGitHub::new(vec![]);
        let (code, out) = run_main(&["--root", &root_arg(&tmp)], &REAL, &net, Some(&gh), None);
        assert_eq!(code, 1);
        assert_eq!(gh.writes().len(), 2); // superpowers + claude-code were still synced
        assert!(out.contains("mystery"));
    }

    #[test]
    fn test_json_output() {
        let (tmp, net) = fake_world(true, Some(with_mystery()));
        let d = tempfile::tempdir().unwrap();
        let target = d.path().join("watch.json");
        let t = target.to_string_lossy().into_owned();
        let (code, _) = run_main(
            &["--dry-run", "--json", &t, "--root", &root_arg(&tmp)],
            &[],
            &net,
            None,
            None,
        );
        let data = json::loads(&std::fs::read_to_string(&target).unwrap()).unwrap();
        assert_eq!(code, 1);
        let keys = |field: &str| -> Vec<String> {
            data.as_object()
                .unwrap()
                .get(field)
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    c.as_object()
                        .unwrap()
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap()
                        .to_string()
                })
                .collect()
        };
        assert_eq!(keys("changes"), ["superpowers", "claude-code"]);
        assert_eq!(keys("errors"), ["mystery"]);
    }

    #[test]
    fn test_no_changes_prints_so_and_exits_0() {
        let (tmp, net) = fake_world(false, Some(Value::Array(vec![])));
        let (code, out) = run_main(
            &["--dry-run", "--root", &root_arg(&tmp)],
            &[],
            &net,
            None,
            None,
        );
        assert_eq!(code, 0);
        assert!(out.to_lowercase().contains("no upstream changes"));
    }
}

mod git_hub_client_tests {
    use super::*;
    use anthology_dev::upstream_watch::GitHubClient;

    fn client(server: &Recorder) -> GitHubClient {
        // No proxy: the Env names none (the Python clears the proxy variables).
        GitHubClient::new("o/r", "tok", &server.base(), &Env::default())
    }

    #[test]
    fn test_paginates_open_issues_with_the_label_filter() {
        let first: Vec<Value> = (0..100).map(|i| obj(vec![("number", int(i))])).collect();
        let pages = HashMap::from([
            (1, Value::Array(first)),
            (2, Value::Array(vec![obj(vec![("number", int(100))])])),
            (3, Value::Array(vec![])),
        ]);
        let server = Recorder::start(pages);
        let issues = client(&server).list_issues().unwrap();
        assert_eq!(issues.len(), 101);
        let seen = server.seen();
        assert!(seen.iter().all(|r| {
            r.path
                .contains("/repos/o/r/issues?labels=upstream-watch&state=all&per_page=100")
        }));
        assert_eq!(seen.len(), 2); // a short second page ends it
        assert_eq!(seen[0].header("authorization"), Some("Bearer tok"));
        assert_eq!(
            seen[0].header("accept"),
            Some("application/vnd.github+json")
        );
        assert_eq!(seen[0].header("x-github-api-version"), Some("2022-11-28"));
        assert_eq!(
            seen[0].header("user-agent"),
            Some("anthology-upstream-watch")
        );
    }

    #[test]
    fn test_create_and_update_send_json() {
        let server = Recorder::start(HashMap::new());
        let c = client(&server);
        c.create_issue("T", "B").unwrap();
        c.update_issue(&int(7), "T2", "B2").unwrap();
        let seen = server.seen();
        assert_eq!(seen.len(), 2);
        assert_eq!(
            (seen[0].method.as_str(), seen[0].path.as_str()),
            ("POST", "/repos/o/r/issues")
        );
        assert_eq!(
            seen[0].body,
            Some(obj(vec![
                ("title", st("T")),
                ("body", st("B")),
                ("labels", Value::Array(vec![st("upstream-watch")])),
            ]))
        );
        assert_eq!(
            (seen[1].method.as_str(), seen[1].path.as_str()),
            ("PATCH", "/repos/o/r/issues/7")
        );
        assert_eq!(
            seen[1].body,
            Some(obj(vec![("title", st("T2")), ("body", st("B2"))]))
        );
    }
}

mod redirect_tests {
    use super::*;
    use anthology_dev::upstream_watch::http::{Http, Method, redirect_headers};
    use anthology_testkit::http::{Route, TestServer};

    fn auth() -> Vec<(String, String)> {
        vec![
            ("Authorization".to_string(), "Bearer secret".to_string()),
            ("Accept".to_string(), "x".to_string()),
        ]
    }

    fn has_auth(h: &[(String, String)]) -> bool {
        h.iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    }

    /// Follows a redirect from `from` (route `/a`) to `to`'s `/b`, sending `Authorization`;
    /// returns whether `/b` received it.
    fn end_to_end(from: &TestServer, to: &TestServer) -> bool {
        to.route("/b", Route::ok(b"ok"));
        from.route("/a", Route::redirect(&to.url("/b")));
        let http = Http::new(&Env::default()).with_test_roots(&[&from.ca_der()]);
        let body = http
            .open(Method::Get, &from.url("/a"), auth(), None, 100)
            .expect("the redirect is followed");
        assert_eq!(body, b"ok");
        let first = &from.requests()[0];
        assert_eq!(first.header("authorization"), Some("Bearer secret"));
        to.requests()
            .iter()
            .find(|r| r.path == "/b")
            .expect("/b was requested")
            .header("authorization")
            .is_some()
    }

    #[test]
    fn test_authorization_is_dropped_when_the_host_changes() {
        let new = redirect_headers(
            "https://api.example.test/a",
            "https://other.example.test/b",
            auth(),
        );
        assert!(!has_auth(&new));
        assert!(new.iter().any(|(k, _)| k == "Accept"));
        // And on the wire: another port is another host (`netloc`).
        let (a, b) = (TestServer::http(), TestServer::http());
        assert!(!end_to_end(&a, &b));
    }

    #[test]
    fn test_authorization_is_dropped_on_a_scheme_downgrade() {
        let new = redirect_headers(
            "https://api.example.test/a",
            "http://api.example.test/b",
            auth(),
        );
        assert!(!has_auth(&new));
        // And on the wire, from TLS (the testkit CA through the test-root seam) to plain HTTP.
        let (a, b) = (TestServer::https(), TestServer::http());
        assert!(!end_to_end(&a, &b));
    }

    #[test]
    fn test_authorization_is_kept_on_the_same_host() {
        let new = redirect_headers(
            "https://api.example.test/a",
            "https://api.example.test/b",
            auth(),
        );
        assert!(has_auth(&new));
        let a = TestServer::http();
        assert!(end_to_end(&a, &a));
    }
}

mod git_env_tests {
    use super::*;
    use anthology_dev::upstream_watch::{Net, RealNet, RunOut};
    use std::sync::Mutex;
    use std::time::Duration;

    #[test]
    fn test_git_env_is_minimal() {
        let hostile = Env::default()
            .with("GITHUB_TOKEN", "t")
            .with("GH_TOKEN", "t")
            .with("GIT_ASKPASS", "x")
            .with("AWS_SECRET_ACCESS_KEY", "s")
            .with("PATH", "/usr/bin")
            .with("HOME", "/home/x");
        let mut env = uw::git_env(&hostile);
        env.sort();
        let mut want: Vec<(String, String)> = [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/x"),
            ("LANG", "C"),
            ("GIT_TERMINAL_PROMPT", "0"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
        want.sort();
        assert_eq!(env, want);
    }

    type Seen = (Vec<String>, Vec<(String, String)>);
    static CALL: Mutex<Option<Seen>> = Mutex::new(None);

    fn fake_run(
        argv: &[String],
        env: &[(String, String)],
        _timeout: Duration,
    ) -> Result<RunOut, String> {
        *CALL.lock().unwrap() = Some((argv.to_vec(), env.to_vec()));
        Ok(RunOut {
            code: 0,
            stdout: format!("{}\trefs/heads/main\n", s('1')).into_bytes(),
            stderr: Vec::new(),
        })
    }

    #[test]
    fn test_ls_remote_runs_git_with_that_env_and_no_shell() {
        let env = Env::default()
            .with("GITHUB_TOKEN", "t")
            .with("PATH", "/usr/bin");
        let mut net = RealNet::new(&env);
        net.run = fake_run;
        let r = net.ls_remote(SKILLS_REPO).unwrap();
        assert_eq!(r, refs(&[("refs/heads/main", &s('1'))]));
        let (argv, child_env) = CALL.lock().unwrap().clone().unwrap();
        assert_eq!(argv[..4], ["git", "ls-remote", "--", SKILLS_REPO]);
        assert!(!child_env.iter().any(|(k, _)| k == "GITHUB_TOKEN"));
        // No shell: argv[0] is git itself, and the patterns reach it as literal arguments.
        assert_eq!(argv[4..], ["refs/tags/*", "refs/heads/*"]);
    }
}

mod real_git_tests {
    use super::*;
    use anthology_dev::upstream_watch::{Net, RealNet};
    use std::process::Command;

    #[test]
    fn test_ls_remote_against_a_local_repository() {
        let d = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(d.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "one"]);
        git(&["tag", "-a", "v1.0.0", "-m", "annotated"]);
        git(&["tag", "v1.1.0"]);
        let env = Env::default().with("PATH", &std::env::var("PATH").unwrap_or_default());
        let r = RealNet::new(&env)
            .ls_remote(&d.path().to_string_lossy())
            .unwrap();
        let has = |k: &str| r.iter().any(|(n, _)| n == k);
        assert!(has("refs/heads/main"));
        assert!(has("refs/tags/v1.0.0^{}")); // annotated tag: peeled entry present
        assert!(has("refs/tags/v1.1.0"));
        assert!(!has("refs/tags/v1.1.0^{}"));
    }
}

mod startup_budget_section_tests {
    use super::*;

    const BUDGETS: [(&str, u64); 3] = [("reader", 1000), ("skilled", 1000), ("bare", 1000)];

    struct World {
        tmp: tempfile::TempDir,
    }

    impl World {
        fn new() -> World {
            let sources = Value::Array(vec![
                git_entry(vec![
                    ("name", st("alpha")),
                    ("target", st("plugins/anthology/knowledge/alpha")),
                ]),
                git_entry(vec![
                    ("name", st("beta")),
                    ("target", st("plugins/anthology/knowledge/beta")),
                ]),
                tag_entry(vec![("target", st("plugins/anthology-superpowers"))]),
                git_entry(vec![
                    ("name", st("unread")),
                    ("target", st("plugins/anthology/knowledge/unread")),
                ]),
                obj(vec![
                    ("name", st("atl")),
                    ("target", st("atl")),
                    ("upstream", obj(vec![("repo", st(ATL_REPO))])),
                ]),
            ]);
            let tmp = make_root(Some(sources), "2.1.329", true);
            agents::make_repo(tmp.path());
            World { tmp }
        }

        fn root(&self) -> Root {
            Root::new(self.tmp.path(), Path::new("/"))
        }

        fn section_with(&self, key: &str, kind: &str, budget: &[(&str, u64)]) -> Option<Sections> {
            let found = uw::startup_budget_sections(&self.root(), &[change(key, kind)], budget)
                .expect("sections");
            found.get(key).cloned()
        }

        fn section(&self, key: &str) -> Option<Sections> {
            self.section_with(key, "git", &BUDGETS)
        }

        fn text(&self, key: &str) -> String {
            self.section(key).expect("a section")[0].1.clone()
        }
    }

    pub fn change(key: &str, kind: &str) -> Change {
        Change::new(
            key,
            kind,
            "v1",
            "v2",
            "v2",
            &format!("scripts/dev.sh vendor sync {key} --ref v2"),
        )
    }

    #[test]
    fn test_a_startup_file_source_lists_current_size_against_budget() {
        let w = World::new();
        let section = w.section("alpha").unwrap();
        let (heading, text) = &section[0];
        assert_eq!(heading, "Startup budget");
        let now = anthology_dev::startup_budget::startup_bytes(&w.root(), "reader").unwrap();
        assert!(text.contains(&format!(
            "| `reader` | {now} | 1000 | {}% |",
            now * 100 / 1000
        )));
        assert!(!text.contains("`skilled`"));
        assert!(!text.contains("`bare`"));
    }

    #[test]
    fn test_every_agent_reading_the_source_is_listed() {
        let text = World::new().text("beta");
        assert!(text.contains("| `reader` |"));
        assert!(text.contains("| `skilled` |"));
    }

    #[test]
    fn test_a_preloaded_skill_source_counts() {
        let text = World::new().text("superpowers");
        assert!(text.contains("| `skilled` |"));
        assert!(!text.contains("`reader`"));
    }

    #[test]
    fn test_the_sync_command_gives_the_size_after_the_sync_names_dev_sh() {
        let text = World::new().text("beta");
        assert!(text.contains("Size after the sync: not computed here"));
        // F-17: the Python names `python3 scripts/startup_budget.py ...`.
        assert!(text.contains("```sh\nscripts/dev.sh startup-budget reader skilled\n```"));
    }

    #[test]
    fn test_sources_no_agent_reads_get_no_section() {
        let w = World::new();
        assert_eq!(w.section("unread"), None);
        assert_eq!(w.section_with("atl", "subtree", &BUDGETS), None);
        assert_eq!(w.section_with("claude-code", "claude-code", &BUDGETS), None);
        assert!(
            uw::startup_budget_sections(&w.root(), &[], &BUDGETS)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_over_budget_is_marked() {
        let w = World::new();
        let budget = [("reader", 5), ("skilled", 1000), ("bare", 1000)];
        let text = w.section_with("alpha", "git", &budget).unwrap()[0]
            .1
            .clone();
        let re = regex::Regex::new(r"\| `reader` \| \d+ \| 5 \| \d+% OVER \|").unwrap();
        assert!(re.is_match(&text), "{text}");
    }

    #[test]
    fn test_missing_budget_and_unreadable_files_are_shown_not_raised() {
        let w = World::new();
        let text = w.section_with("alpha", "git", &[]).unwrap()[0].1.clone();
        assert!(text.contains("no budget"));
        std::fs::remove_file(
            w.tmp
                .path()
                .join("plugins/anthology/knowledge/alpha/SKILL.md"),
        )
        .unwrap();
        assert!(w.text("alpha").contains("| `reader` | unreadable |"));
    }

    #[test]
    fn test_the_section_lands_in_the_issue_body_before_the_command() {
        let w = World::new();
        let c = change("alpha", "git");
        let extra =
            uw::startup_budget_sections(&w.root(), std::slice::from_ref(&c), &BUDGETS).unwrap();
        let body = uw::render_body(&c, &extra["alpha"]);
        assert!(body.contains("### Startup budget\n"));
        assert!(body.find("### Startup budget") < body.find("### Update"));
        assert!(!body_of(&c).contains("Startup budget"));
    }

    #[test]
    fn test_sync_issues_writes_the_section_into_the_issue() {
        let w = World::new();
        let c = change("alpha", "git");
        let gh = FakeGitHub::new(vec![]);
        let extra =
            uw::startup_budget_sections(&w.root(), std::slice::from_ref(&c), &BUDGETS).unwrap();
        uw::sync_issues(&[c], Some(&gh as &dyn GitHub), false, &extra).unwrap();
        let writes = gh.writes();
        assert!(matches!(&writes[0], Call::Create(_, body) if body.contains("### Startup budget")));
    }

    #[test]
    fn test_sanitisation_leaves_no_markdown_html_or_mentions() {
        let hostile =
            "a@octocat <img src=x onerror=alert(1)> [x](http://e.example) `` ```\n# h | * _";
        let rows = vec![
            (hostile.to_string(), Some(5), Some(10)),
            ("@team\n### Update".to_string(), None, None),
        ];
        let text = uw::render_startup_budget(hostile, &rows);
        for bad in ["@", "<", ">", "[", "]", "\\", "*", "!", "&"] {
            assert!(!text.contains(bad), "{bad}");
        }
        // only the two code fences this function writes itself are present
        assert_eq!(text.matches("```").count(), 2);
        assert_eq!(text.matches("\n### ").count(), 0);
        // each hostile agent name stays on its own table row
        assert_eq!(text.lines().filter(|l| l.starts_with("| `")).count(), 2);
    }

    #[test]
    fn test_sanitise_prose_whitelists_one_line_of_plain_text() {
        assert_eq!(
            uw::sanitize_prose("@octocat <b>hi</b> [a](b)\nnext", 80),
            "octocat bhi/b a(b)next"
        );
        assert_eq!(uw::sanitize_prose(&"x".repeat(500), 80).chars().count(), 80);
    }

    #[test]
    fn test_agent_files_with_hostile_names_are_ignored() {
        let w = World::new();
        agents::make_repo(w.tmp.path());
        let agents_dir = w.tmp.path().join("plugins/anthology/agents");
        std::fs::copy(
            agents_dir.join("reader.md"),
            agents_dir.join("evil@octocat.md"),
        )
        .unwrap();
        assert!(!w.text("alpha").contains("octocat"));
    }
}

mod startup_budget_main_tests {
    use super::*;

    fn world() -> (tempfile::TempDir, FakeNet) {
        let sources = Value::Array(vec![
            tag_entry(vec![("target", st("plugins/anthology-superpowers"))]),
            url_entry(vec![]),
        ]);
        let tmp = make_root(Some(sources), "2.1.329", true);
        agents::make_repo(tmp.path());
        let net = FakeNet::new()
            .refs(SP_REPO, refs_of("ls_remote_tags.txt"))
            .http(
                &compare_url(SP_REPO, &s('5'), &s('a')),
                compare_json(&["x.md"], 1),
            )
            .http(&ui_manifest(), fixture("manifest_new.txt"))
            .http(NPM_URL, fixture("npm_same.json"));
        (tmp, net)
    }

    const ENV: [(&str, &str); 2] = [("GITHUB_TOKEN", "t"), ("GITHUB_REPOSITORY", "o/r")];

    fn main(
        tmp: &tempfile::TempDir,
        net: &FakeNet,
        gh: &FakeGitHub,
        dry_run: bool,
    ) -> (i32, String) {
        let root = tmp.path().to_string_lossy().into_owned();
        let mut argv = vec!["--root", root.as_str()];
        if dry_run {
            argv.push("--dry-run");
        }
        run_main(&argv, &ENV, net, Some(gh), None)
    }

    fn superpowers_change(tmp: &tempfile::TempDir, net: &FakeNet) -> Change {
        let sources = fixture_sources_of(tmp);
        Checker::new(tmp.path(), net, None)
            .check_entry(&sources.as_array().unwrap()[0])
            .unwrap()
            .unwrap()
    }

    fn fixture_sources_of(tmp: &tempfile::TempDir) -> Value {
        let text = std::fs::read_to_string(tmp.path().join("vendor/sources.json")).unwrap();
        let mut data = json::loads(&text).unwrap();
        data.as_object_mut()
            .unwrap()
            .get_mut("sources")
            .unwrap()
            .take()
    }

    fn bodies(gh: &FakeGitHub) -> Vec<(String, String)> {
        gh.writes()
            .into_iter()
            .filter_map(|c| match c {
                Call::Create(t, b) => Some((t, b)),
                Call::Update(_, t, b) => Some((t, b)),
                Call::List => None,
            })
            .collect()
    }

    #[test]
    fn test_main_creates_issues_with_the_section_only_where_agents_read_the_source() {
        let (tmp, net) = world();
        let gh = FakeGitHub::new(vec![]);
        let (code, _) = main(&tmp, &net, &gh, false);
        assert_eq!(code, 0);
        let all = bodies(&gh);
        let sp = &all
            .iter()
            .find(|(t, _)| t.contains("superpowers"))
            .unwrap()
            .1;
        let ui = &all.iter().find(|(t, _)| t.contains("ui-taste")).unwrap().1;
        assert!(sp.contains("### Startup budget"));
        assert!(sp.contains("| `skilled` |"));
        assert!(!ui.contains("Startup budget"));
    }

    #[test]
    fn test_dry_run_prints_the_section() {
        let (tmp, net) = world();
        let gh = FakeGitHub::new(vec![]);
        let (code, out) = main(&tmp, &net, &gh, true);
        assert_eq!(code, 0);
        assert!(out.contains("### Startup budget"));
        assert!(gh.writes().is_empty());
    }

    fn boom(
        _: &Root,
        _: &[Change],
        _: &[(&str, u64)],
    ) -> Result<HashMap<String, Sections>, Uncaught> {
        Err(Uncaught::new("OSError", "boom"))
    }

    #[test]
    fn test_a_failing_budget_calculation_never_blocks_the_issues() {
        let (tmp, net) = world();
        let gh = FakeGitHub::new(vec![]);
        let root = tmp.path().to_string_lossy().into_owned();
        let (code, out) = run_main(&["--root", &root], &ENV, &net, Some(&gh), Some(boom));
        assert_eq!(code, 0);
        assert!(out.contains("WARNING startup budget not computed (OSError)"));
        let all = bodies(&gh);
        assert_eq!(all.len(), 2);
        assert!(!all[0].1.contains("Startup budget"));
    }

    #[test]
    fn test_a_dismissed_release_stays_dismissed_when_only_the_sizes_moved() {
        let (tmp, net) = world();
        let change = superpowers_change(&tmp, &net);
        let old = vec![(
            "Startup budget".to_string(),
            "| `skilled` | 1 | 2 | 50% |".to_string(),
        )];
        let closed = bot_issue(
            &change,
            9,
            vec![
                ("state", st("closed")),
                ("body", st(&uw::render_body(&change, &old))),
            ],
        );
        let gh = FakeGitHub::new(vec![closed]);
        let (code, _) = main(&tmp, &net, &gh, false);
        assert_eq!(code, 0);
        assert!(bodies(&gh).iter().all(|(t, _)| !t.contains("superpowers")));
    }

    #[test]
    fn test_an_open_issue_is_updated_when_the_sizes_moved() {
        let (tmp, net) = world();
        let change = superpowers_change(&tmp, &net);
        let stale_sections = vec![("Startup budget".to_string(), "old".to_string())];
        let stale = bot_issue(
            &change,
            9,
            vec![("body", st(&uw::render_body(&change, &stale_sections)))],
        );
        let gh = FakeGitHub::new(vec![stale]);
        main(&tmp, &net, &gh, false);
        let updates: Vec<Call> = gh
            .writes()
            .into_iter()
            .filter(|c| matches!(c, Call::Update(..)))
            .collect();
        assert_eq!(updates.len(), 1);
        assert!(
            matches!(&updates[0], Call::Update(n, _, b) if *n == int(9) && b.contains("| `skilled` |"))
        );
    }
}
