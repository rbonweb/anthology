//! Ports of the unit classes of tests/test_release.py (DV2, ruling R1): `ChangelogTests`,
//! `VersionEditTests`, `CheckVersionTests` and `RangeTests` call the release-edit library as the
//! Python tests call `release_edit`. `ReleaseScriptTests` drives RL1's release.sh and stays `todo`.
//! Also `npm_semver_grid_matches` (R9): the semver ranges against npm's own answers.

use std::fs;
use std::path::PathBuf;

use anthology_dev::release_edit::{
    Failure, changelog_check, changelog_move, check_dependents, check_version, plugin_version,
    set_plugin_version, set_source_plugin_version,
};
use anthology_dev::semver::{self, RangeError};
use anthology_pycompat::json::{self, Value};

const CHANGELOG: &str = "\
# Changelog

Intro text.

## anthology

### [Unreleased]

#### Added

- Anthology thing.

### [0.0.9] - 2026-01-01

#### Added

- Older anthology thing.

## anthology-guardrails

### [Unreleased]

#### Added

- Guardrails thing.

## anthology-superpowers

### [Unreleased]

";

/// The `## plugin` section of a changelog, for before/after comparison.
fn section(text: &str, plugin: &str) -> String {
    let mut out = String::new();
    let mut on = false;
    for line in text.split_inclusive('\n') {
        if line.starts_with("## ") {
            on = line.trim() == format!("## {plugin}");
        }
        if on {
            out.push_str(line);
        }
    }
    out
}

mod changelog_tests {
    use super::*;

    #[test]
    fn test_move_inserts_version_section_below_unreleased() {
        let out = changelog_move(CHANGELOG, "anthology", "0.1.0", "2026-11-20").unwrap();
        assert!(
            out.contains(
                "### [Unreleased]\n\n### [0.1.0] - 2026-11-20\n\n#### Added\n\n\
                 - Anthology thing.\n\n### [0.0.9] - 2026-01-01\n"
            ),
            "{out}"
        );
        assert_eq!(out.matches("- Anthology thing.").count(), 1);
    }

    #[test]
    fn test_move_leaves_other_plugins_and_intro_untouched() {
        let out = changelog_move(CHANGELOG, "anthology", "0.1.0", "2026-11-20").unwrap();
        for other in ["anthology-guardrails", "anthology-superpowers"] {
            assert_eq!(section(&out, other), section(CHANGELOG, other));
        }
        assert!(out.starts_with("# Changelog\n\nIntro text.\n\n## anthology\n"));
    }

    #[test]
    fn test_move_does_not_confuse_plugin_with_shared_prefix() {
        let out = changelog_move(CHANGELOG, "anthology-guardrails", "0.2.0", "2026-11-20").unwrap();
        assert_eq!(section(&out, "anthology"), section(CHANGELOG, "anthology"));
        assert!(
            out.contains("### [0.2.0] - 2026-11-20\n\n#### Added\n\n- Guardrails thing."),
            "{out}"
        );
    }

    #[test]
    fn test_move_last_section_in_file() {
        let text = format!(
            "{}\n- Superpowers thing.\n",
            CHANGELOG.trim_end_matches('\n')
        );
        let out = changelog_move(&text, "anthology-superpowers", "0.1.0", "2026-11-20").unwrap();
        assert!(
            out.ends_with("### [Unreleased]\n\n### [0.1.0] - 2026-11-20\n\n- Superpowers thing.\n"),
            "{out}"
        );
    }

    #[test]
    fn test_empty_unreleased_is_refused() {
        let err =
            changelog_move(CHANGELOG, "anthology-superpowers", "0.1.0", "2026-11-20").unwrap_err();
        assert!(err.0.contains("Unreleased"), "{err}");
        assert!(changelog_check(CHANGELOG, "anthology-superpowers").is_err());
    }

    #[test]
    fn test_heading_only_unreleased_is_refused() {
        let text = CHANGELOG.replace("- Guardrails thing.\n", "");
        assert!(changelog_check(&text, "anthology-guardrails").is_err());
    }

    #[test]
    fn test_missing_plugin_or_unreleased_heading() {
        assert!(changelog_check(CHANGELOG, "nope").is_err());
        assert!(changelog_check("## p\n\n### [1.0.0] - 2026-01-01\n\n- x\n", "p").is_err());
    }

    #[test]
    fn test_existing_version_section_is_refused() {
        assert!(changelog_move(CHANGELOG, "anthology", "0.0.9", "2026-11-20").is_err());
    }

    #[test]
    fn test_check_passes_with_content() {
        changelog_check(CHANGELOG, "anthology").unwrap();
    }
}

mod version_edit_tests {
    use super::*;

    const FIRST_PARTY: &str = "{\n  \"name\": \"p\",\n  \"version\": \"0.1.0\",\n  \
         \"author\": { \"name\": \"x\" },\n  \"engines\": { \"claude-code\": \">=2.1.311\" }\n}\n";
    const GENERATED: &str = "{\n  \"name\": \"q\",\n  \"author\": {\n    \"name\": \"A\",\n    \
         \"version\": \"9.9.9\"\n  },\n  \"version\": \"0.1.0\",\n  \"keywords\": [\n    \
         \"version\"\n  ]\n}\n";
    const SOURCES: &str = "{\n  \"sources\": [\n    {\n      \"name\": \"other\",\n      \
         \"target\": \"plugins/anthology/knowledge/x\",\n      \"plugin_version\": \"7.7.7\"\n    \
         },\n    {\n      \"name\": \"superpowers\",\n      \"ref\": \"v7.0.2\",\n      \
         \"target\": \"plugins/anthology-superpowers\",\n      \"transform\": [{\"name\": \
         \"rename-plugin\", \"new\": \"z\"}],\n      \"plugin_version\": \"0.1.0\",\n      \
         \"upstream_version\": \"7.0.2\"\n    }\n  ]\n}\n";

    #[test]
    fn test_replaces_only_the_version_value() {
        let out = set_plugin_version(FIRST_PARTY, "0.2.0").unwrap();
        assert_eq!(
            out,
            FIRST_PARTY.replace("\"version\": \"0.1.0\"", "\"version\": \"0.2.0\"")
        );
    }

    #[test]
    fn test_ignores_nested_version_keys_and_preserves_layout() {
        let out = set_plugin_version(GENERATED, "0.3.0").unwrap();
        assert_eq!(
            out,
            GENERATED.replace("  \"version\": \"0.1.0\"", "  \"version\": \"0.3.0\"")
        );
        assert!(out.contains("\"version\": \"9.9.9\""));
    }

    #[test]
    fn test_missing_or_non_string_version() {
        assert!(set_plugin_version("{\"name\": \"p\"}\n", "0.2.0").is_err());
        assert!(set_plugin_version("{\"version\": 3}\n", "0.2.0").is_err());
    }

    #[test]
    fn test_plugin_version_reads_top_level() {
        assert_eq!(plugin_version(GENERATED).unwrap(), "0.1.0");
    }

    #[test]
    fn test_sources_edit_is_byte_minimal() {
        let out =
            set_source_plugin_version(SOURCES, "plugins/anthology-superpowers", "0.2.0").unwrap();
        assert_eq!(
            out,
            SOURCES.replace(
                "\"plugin_version\": \"0.1.0\"",
                "\"plugin_version\": \"0.2.0\""
            )
        );
    }

    #[test]
    fn test_sources_unknown_target_or_no_plugin_version() {
        assert!(matches!(
            set_source_plugin_version(SOURCES, "plugins/nope", "0.2.0"),
            Err(Failure::Release(_))
        ));
        assert!(matches!(
            set_source_plugin_version("{\"sources\": [{\"target\": \"t\"}]}", "t", "0.2.0"),
            Err(Failure::Release(_))
        ));
    }
}

mod check_version_tests {
    use super::*;

    #[test]
    fn test_higher_is_a_bump() {
        assert_eq!(check_version("0.1.0", "0.1.1", true).unwrap(), "bump");
        assert_eq!(check_version("0.1.0", "0.2.0", false).unwrap(), "bump");
    }

    #[test]
    fn test_equal_without_tag_is_first_release() {
        assert_eq!(check_version("0.1.0", "0.1.0", false).unwrap(), "same");
    }

    #[test]
    fn test_equal_with_tag_is_refused() {
        assert!(check_version("0.1.0", "0.1.0", true).is_err());
    }

    #[test]
    fn test_lower_is_refused() {
        for tag_exists in [true, false] {
            assert!(check_version("0.2.0", "0.1.9", tag_exists).is_err());
        }
    }

    #[test]
    fn test_not_semver_is_refused() {
        for bad in ["1.2", "v1.2.3", "", "1.2.3.4"] {
            assert!(check_version("0.1.0", bad, false).is_err(), "{bad:?}");
        }
    }
}

mod range_tests {
    use super::*;

    const TABLE: &[(&str, &str, bool)] = &[
        ("~0.1", "0.1.0", true),
        ("~0.1", "0.1.5", true),
        ("~0.1", "0.2.0", false),
        ("~0.1", "0.0.9", false),
        ("~1.2", "1.2.9", true),
        ("~1.2", "1.3.0", false),
        ("~0.1.2", "0.1.1", false),
        ("~0.1.2", "0.1.9", true),
        ("~0.1.2", "0.2.0", false),
        ("^0.1.2", "0.1.2", true),
        ("^0.1.2", "0.1.9", true),
        ("^0.1.2", "0.2.0", false),
        ("^0.1.2", "0.1.1", false),
        ("^0.0.3", "0.0.3", true),
        ("^0.0.3", "0.0.4", false),
        ("^1.2.3", "1.9.0", true),
        ("^1.2.3", "2.0.0", false),
        ("^1.2.3", "1.2.2", false),
        ("1.2.3", "1.2.3", true),
        ("1.2.3", "1.2.4", false),
        (">=1.2.3", "1.2.3", true),
        (">=1.2.3", "3.0.0", true),
        (">=1.2.3", "1.2.2", false),
        // npm: a prerelease only satisfies a range that names a prerelease of that version
        ("~0.1", "0.1.5-beta.1", false),
        ("^0.1.2", "0.1.5-rc.1", false),
        ("^0.1.2-beta.1", "0.1.2-beta.2", true),
        ("^0.1.2-beta.1", "0.1.2", true),
        ("^0.1.2-beta.2", "0.1.2-beta.1", false),
        (">=1.0.0", "2.0.0-rc.1", false),
        // npm partial ranges: ^X.Y, ^X, ~X
        ("^1.0", "1.0.0", true),
        ("^1.0", "1.9.3", true),
        ("^1.0", "2.0.0", false),
        ("^1.0", "0.9.9", false),
        ("^1.0", "1.0.1-rc.1", false),
        ("^1.0", "1.1.0-rc.1", false),
        ("^1.2", "1.2.0", true),
        ("^1.2", "1.1.9", false),
        ("^1.2", "1.9.0", true),
        ("^0.1", "0.1.0", true),
        ("^0.1", "0.1.9", true),
        ("^0.1", "0.2.0", false),
        ("^0.1", "0.0.9", false),
        ("^0.1", "0.1.1-beta.1", false),
        ("^0.0", "0.0.0", true),
        ("^0.0", "0.0.9", true),
        ("^0.0", "0.1.0", false),
        ("^0.0", "1.0.0", false),
        ("^1", "1.0.0", true),
        ("^1", "1.9.9", true),
        ("^1", "2.0.0", false),
        ("^1", "0.9.9", false),
        ("^1", "1.0.0-rc.1", false),
        ("^0", "0.0.0", true),
        ("^0", "0.9.9", true),
        ("^0", "1.0.0", false),
        ("^0", "0.1.0-rc.1", false),
        ("~1", "1.0.0", true),
        ("~1", "1.9.9", true),
        ("~1", "2.0.0", false),
        ("~1", "0.9.9", false),
        ("~1", "1.0.0-rc.1", false),
        ("~0", "0.0.0", true),
        ("~0", "0.9.9", true),
        ("~0", "1.0.0", false),
        ("~0", "0.1.0-rc.1", false),
        ("^10", "10.4.0", true),
        ("^10", "11.0.0", false),
    ];

    fn st(text: &str) -> Value {
        Value::Str(text.to_string())
    }

    #[test]
    fn test_acceptance_table() {
        for &(rng, version, want) in TABLE {
            assert_eq!(
                semver::range_accepts(&st(rng), version),
                Ok(want),
                "{rng} vs {version}"
            );
        }
    }

    #[test]
    fn test_unsupported_forms_raise() {
        let strs = [
            "*",
            "latest",
            "1.x",
            ">1.2.3",
            "1.2 - 2.0",
            "",
            "~",
            "^",
            ">=1.2",
            ">=1",
            "||",
            "~0.1 || ~0.2",
            "v1.2.3",
            "=1.2.3",
            "<2.0.0",
            "1.2",
            "1",
            "^1.x",
            "~1.x",
            "^1.2.3+build",
            "^1.2+build",
            " ^1.0",
            "^1.0 ",
            "^01",
            "^1.02",
            "^1.2.",
            "^-1",
        ];
        let mut all: Vec<Value> = strs.iter().map(|r| st(r)).collect();
        all.push(Value::Null);
        for rng in all {
            assert!(
                matches!(
                    semver::range_accepts(&rng, "1.2.3"),
                    Err(RangeError::Unsupported(_))
                ),
                "{rng:?}"
            );
        }
    }

    /// `_tree`: a throwaway checkout with `plugins/<name>/.claude-plugin/plugin.json` per entry;
    /// `deps` is the JSON text of its `dependencies`, if any.
    fn tree(entries: &[(&str, Option<&str>)]) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("temp dir");
        let root = tmp.path().to_path_buf();
        for (name, deps) in entries {
            let dir = root.join("plugins").join(name).join(".claude-plugin");
            fs::create_dir_all(&dir).expect("mkdir");
            let manifest = match deps {
                Some(d) => format!(
                    "{{\"name\": \"{name}\", \"version\": \"0.1.0\", \"dependencies\": {d}}}"
                ),
                None => format!("{{\"name\": \"{name}\", \"version\": \"0.1.0\"}}"),
            };
            // The text must be what `json.dumps` would write; check it parses.
            assert!(json::loads(&manifest).is_ok());
            fs::write(dir.join("plugin.json"), manifest).expect("write");
        }
        (tmp, root)
    }

    #[test]
    fn test_dependents_accepting() {
        let (_tmp, root) = tree(&[
            ("lib", None),
            ("app", Some(r#"[{"name": "lib", "version": "~0.1"}]"#)),
            (
                "other",
                Some(r#"[{"name": "unrelated", "version": "9.9.9"}]"#),
            ),
        ]);
        assert_eq!(
            check_dependents(&root, "lib", "0.1.7").unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_dependents_rejecting_names_the_dependent() {
        let (_tmp, root) = tree(&[
            ("lib", None),
            ("app", Some(r#"[{"name": "lib", "version": "~0.1"}]"#)),
        ]);
        let problems = check_dependents(&root, "lib", "0.2.0").unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("plugins/app/.claude-plugin/plugin.json"));
        assert!(problems[0].contains("~0.1"));
        assert!(problems[0].contains("widen"));
        assert_eq!(
            problems[0],
            "plugins/app/.claude-plugin/plugin.json depends on lib ~0.1, which does not accept \
             0.2.0; widen that range first (it is an app change and needs its own version bump)"
        );
    }

    #[test]
    fn test_dependents_unsupported_range_fails_closed() {
        let (_tmp, root) = tree(&[
            ("lib", None),
            ("app", Some(r#"[{"name": "lib", "version": "*"}]"#)),
        ]);
        let problems = check_dependents(&root, "lib", "0.1.0").unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("can't check range"));
        assert_eq!(
            problems[0],
            "plugins/app/.claude-plugin/plugin.json depends on lib with range '*': can't check \
             range (supported: ~X, ~X.Y, ~X.Y.Z, ^X, ^X.Y, ^X.Y.Z, X.Y.Z, >=X.Y.Z)"
        );
    }

    #[test]
    fn test_dependents_caret_partial_range() {
        let (_tmp, root) = tree(&[
            ("lib", None),
            ("app", Some(r#"[{"name": "lib", "version": "^1.0"}]"#)),
        ]);
        assert!(check_dependents(&root, "lib", "1.0.0").unwrap().is_empty());
        let problems = check_dependents(&root, "lib", "2.0.0").unwrap();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("plugins/app/.claude-plugin/plugin.json"));
        assert!(problems[0].contains("^1.0"));
        assert!(problems[0].contains("does not accept 2.0.0"));
    }

    #[test]
    fn test_dependency_without_range_or_as_string_accepts_anything() {
        let (_tmp, root) = tree(&[
            ("lib", None),
            ("a", Some(r#"[{"name": "lib"}]"#)),
            ("b", Some(r#"["lib"]"#)),
        ]);
        assert!(check_dependents(&root, "lib", "5.0.0").unwrap().is_empty());
    }
}

/// R9: every (range, version) pair of the npm grid gets npm's answer. A pair the tool declines
/// to evaluate is not a mismatch only if npm's range form is outside the supported forms; the
/// grid holds supported forms only, so any `Err` is a mismatch.
#[test]
fn npm_semver_grid_matches() {
    let text = fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/npm_semver_ranges.json"
    ))
    .expect("grid fixture");
    let grid = json::loads(&text).expect("grid json");
    let obj = grid.as_object().expect("object");
    let strings = |key: &str| -> Vec<String> {
        obj.get(key)
            .and_then(Value::as_array)
            .expect(key)
            .iter()
            .map(|v| v.as_str().expect("string").to_string())
            .collect()
    };
    let ranges = strings("ranges");
    let versions = strings("versions");
    let accepts = strings("accepts");
    assert_eq!(accepts.len(), ranges.len());
    let (mut pairs, mut mismatches) = (0usize, Vec::new());
    for (rng, row) in ranges.iter().zip(&accepts) {
        assert_eq!(row.len(), versions.len(), "{rng}");
        for (version, want) in versions.iter().zip(row.chars()) {
            pairs += 1;
            let want = want == '1';
            let got = semver::range_accepts(&Value::Str(rng.clone()), version);
            if got != Ok(want) {
                mismatches.push(format!("{rng} vs {version}: got {got:?}, npm says {want}"));
            }
        }
    }
    assert_eq!(pairs, 5576);
    assert!(
        mismatches.is_empty(),
        "{} mismatches of {pairs}: {:?}",
        mismatches.len(),
        &mismatches[..mismatches.len().min(10)]
    );
}
