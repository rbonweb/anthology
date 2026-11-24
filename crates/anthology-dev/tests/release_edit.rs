//! Ports of the unit classes of tests/test_release.py (DV2, ruling R1): `ChangelogTests`,
//! `VersionEditTests`, `CheckVersionTests` and `RangeTests` call the release-edit library as the
//! Python tests call `release_edit`. `ReleaseScriptTests` drives RL1's staging release.sh.
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

/// Changelog fragments: `changelog.d/<plugin>/*.md` beside the CHANGELOG (new in the Rust tool).
mod fragment_tests {
    use super::*;
    use anthology_core::Env;
    use anthology_dev::release_edit::run_in;
    use std::ffi::OsString;
    use std::path::Path;

    struct Ran {
        code: i32,
        err: String,
    }

    fn run(cwd: &Path, a: &[&str]) -> Ran {
        let args: Vec<OsString> = a.iter().map(OsString::from).collect();
        let o = run_in(&args, &mut std::io::empty(), &Env::default(), cwd);
        Ran {
            code: o.code,
            err: String::from_utf8_lossy(&o.stderr).into_owned(),
        }
    }

    fn repo_with_changelog() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("CHANGELOG.md"), CHANGELOG).unwrap();
        tmp
    }

    fn fragment(tmp: &tempfile::TempDir, plugin: &str, name: &str, text: &str) {
        let dir = tmp.path().join("changelog.d").join(plugin);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), text).unwrap();
    }

    #[test]
    fn changelog_check_accepts_fragments_only() {
        let tmp = repo_with_changelog();
        // `anthology-superpowers` has an empty [Unreleased]: refused until a fragment has entries.
        let check = ["changelog-check", "CHANGELOG.md", "anthology-superpowers"];
        assert_eq!(run(tmp.path(), &check).code, 1);
        // A heading with nothing under it is still an empty skeleton.
        fragment(
            &tmp,
            "anthology-superpowers",
            "10-skeleton.md",
            "#### Added\n",
        );
        assert_eq!(run(tmp.path(), &check).code, 1);
        // A fragment of another plugin does not count.
        fragment(&tmp, "anthology", "10-other.md", "- Other plugin.\n");
        assert_eq!(run(tmp.path(), &check).code, 1);
        // Neither does a file that is not `.md`.
        fragment(
            &tmp,
            "anthology-superpowers",
            "20-notes.txt",
            "- Not a fragment.\n",
        );
        assert_eq!(run(tmp.path(), &check).code, 1);
        fragment(
            &tmp,
            "anthology-superpowers",
            "20-thing.md",
            "- Fragment thing.\n",
        );
        let ok = run(tmp.path(), &check);
        assert_eq!((ok.code, ok.err.as_str()), (0, ""));
    }

    #[test]
    fn changelog_move_compiles_fragments_in_filename_order_and_removes_them() {
        let tmp = repo_with_changelog();
        fragment(&tmp, "anthology", "20-b.md", "- B.\n");
        fragment(&tmp, "anthology", "10-a.md", "\n- A.\n\n");
        fragment(&tmp, "anthology", "30-c.md", "#### Fixed\n\n- C.\n");
        fragment(&tmp, "anthology", "40-empty.md", "\n");
        fragment(&tmp, "anthology", "notes.txt", "kept\n");
        fragment(
            &tmp,
            "anthology-guardrails",
            "10-g.md",
            "- Guardrails fragment.\n",
        );
        let mv = [
            "changelog-move",
            "CHANGELOG.md",
            "anthology",
            "0.1.0",
            "--date",
            "2026-11-20",
        ];
        let ran = run(tmp.path(), &mv);
        assert_eq!((ran.code, ran.err.as_str()), (0, ""));
        let out = fs::read_to_string(tmp.path().join("CHANGELOG.md")).unwrap();
        assert!(
            out.contains(
                "### [Unreleased]\n\n### [0.1.0] - 2026-11-20\n\n#### Added\n\n- Anthology thing.\n\
                 - A.\n- B.\n\n#### Fixed\n\n- C.\n\n### [0.0.9] - 2026-01-01\n"
            ),
            "{out}"
        );
        // Only this plugin's fragments are consumed, and only the `.md` files.
        let left = |plugin: &str| -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(tmp.path().join("changelog.d").join(plugin))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(left("anthology"), ["notes.txt"]);
        assert_eq!(left("anthology-guardrails"), ["10-g.md"]);
        assert_eq!(
            section(&out, "anthology-guardrails"),
            section(CHANGELOG, "anthology-guardrails")
        );
    }

    #[test]
    fn changelog_move_with_only_fragments_fills_an_empty_unreleased() {
        let tmp = repo_with_changelog();
        fragment(
            &tmp,
            "anthology-superpowers",
            "10-a.md",
            "- From a fragment.\n",
        );
        let mv = [
            "changelog-move",
            "CHANGELOG.md",
            "anthology-superpowers",
            "0.1.0",
            "--date",
            "2026-11-20",
        ];
        assert_eq!(run(tmp.path(), &mv).code, 0);
        let out = fs::read_to_string(tmp.path().join("CHANGELOG.md")).unwrap();
        assert!(
            out.ends_with("### [Unreleased]\n\n### [0.1.0] - 2026-11-20\n\n- From a fragment.\n"),
            "{out}"
        );
    }

    #[test]
    fn a_refused_move_keeps_the_fragments() {
        let tmp = repo_with_changelog();
        fragment(&tmp, "anthology", "10-a.md", "- A.\n");
        // 0.0.9 already has a section: the move fails and nothing is consumed.
        let mv = [
            "changelog-move",
            "CHANGELOG.md",
            "anthology",
            "0.0.9",
            "--date",
            "2026-11-20",
        ];
        assert_eq!(run(tmp.path(), &mv).code, 1);
        assert!(tmp.path().join("changelog.d/anthology/10-a.md").exists());
        assert_eq!(
            fs::read_to_string(tmp.path().join("CHANGELOG.md")).unwrap(),
            CHANGELOG
        );
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

/// `ReleaseScriptTests`: RL1's staging `release.sh` in a throwaway git repository, with a fake
/// `claude` and `ANTHOLOGY_DEV` set to the built `anthology-dev` (so no Docker). The fake
/// `claude` is plain bash: the Python one reads plugin.json with python3, which the Rust image
/// does not have.
mod release_script_tests {
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process::{Command, Stdio};

    use super::*;

    const FAKE_CLAUDE: &str = r##"#!/usr/bin/env bash
# Fake `claude`: records its arguments; for `plugin tag` creates the tag with git.
printf '%s\n' "$*" >> "$FAKE_CLAUDE_LOG"
[ "$1" = plugin ] && [ "$2" = tag ] || exit 0
shift 2
path=""; dry=0; push=0; remote=origin; msg=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry=1 ;;
    --push) push=1 ;;
    --remote) shift; remote="$1" ;;
    -m) shift; msg="$1" ;;
    *) path="$1" ;;
  esac
  shift
done
field() { sed -n "s/^ *\"$1\": *\"\\([^\"]*\\)\".*/\\1/p" "$path/.claude-plugin/plugin.json" | head -1; }
name=$(field name)
ver=$(field version)
tag="$name--v$ver"
if [ "$dry" = 1 ]; then echo "$tag"; exit 0; fi
git tag -a -m "${msg//%s/$ver}" "$tag"
[ "$push" = 1 ] && git push "$remote" "refs/tags/$tag"
exit 0
"##;

    const SOURCES: &str = "{\n  \"sources\": [\n    {\n      \"name\": \"superpowers\",\n      \
         \"target\": \"plugins/anthology-superpowers\",\n      \"plugin_version\": \"0.1.0\",\n      \
         \"upstream_version\": \"7.0.2\"\n    },\n    {\n      \"name\": \"x\",\n      \
         \"target\": \"plugins/anthology/knowledge/x\"\n    }\n  ]\n}\n";

    const MANIFEST_ANTHOLOGY: &str = "{\n  \"name\": \"anthology\",\n  \"version\": \"0.1.0\",\n  \
         \"dependencies\": [\n    { \"name\": \"anthology-guardrails\", \"version\": \"~0.1\" },\n    \
         { \"name\": \"anthology-superpowers\", \"version\": \"~0.1\" }\n  ]\n}\n";
    const MANIFEST_GUARDRAILS: &str =
        "{\n  \"name\": \"anthology-guardrails\",\n  \"version\": \"0.1.0\"\n}\n";
    const MANIFEST_SUPERPOWERS: &str =
        "{\n  \"name\": \"anthology-superpowers\",\n  \"version\": \"0.1.0\"\n}\n";

    const FAKE_CHANGELOG: &str = "\
# Changelog

## anthology

### [Unreleased]

#### Added

- A.

## anthology-guardrails

### [Unreleased]

#### Added

- G.

## anthology-superpowers

### [Unreleased]

#### Added

- S.
";

    const GUARDRAILS_JSON: &str = "plugins/anthology-guardrails/.claude-plugin/plugin.json";

    struct Fx {
        tmp: tempfile::TempDir,
        repo: PathBuf,
        remote: PathBuf,
        log: PathBuf,
        fake: PathBuf,
    }

    /// The process environment without `GIT_*`.
    fn base_env() -> Vec<(OsString, OsString)> {
        std::env::vars_os()
            .filter(|(k, _)| !k.to_string_lossy().starts_with("GIT_"))
            .collect()
    }

    fn run_git(dir: &Path, env: &[(OsString, OsString)], args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env_clear()
            .envs(env.iter().cloned())
            .stdin(Stdio::null())
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    impl Fx {
        fn new() -> Fx {
            let tmp = tempfile::tempdir().expect("temp dir");
            let base = tmp.path().to_path_buf();
            let fx = Fx {
                repo: base.join("repo"),
                remote: base.join("remote.git"),
                log: base.join("claude.log"),
                fake: base.join("fake-claude"),
                tmp,
            };
            fs::write(&fx.fake, FAKE_CLAUDE).unwrap();
            fs::set_permissions(&fx.fake, fs::Permissions::from_mode(0o755)).unwrap();
            fs::write(&fx.log, "").unwrap();
            fs::create_dir(&fx.repo).unwrap();
            fx.git(&["init", "-q", "-b", "main"]);
            fx.git(&["config", "user.name", "Test"]);
            fx.git(&["config", "user.email", "test@users.noreply.github.com"]);
            fx.git(&["config", "commit.gpgsign", "false"]);
            fx.git(&["config", "tag.gpgsign", "false"]);
            // The script under test is RL1's staging release.sh (it becomes scripts/release.sh).
            let script = anthology_testkit::staged("scripts/release.sh");
            fs::create_dir(fx.repo.join("scripts")).unwrap();
            fs::copy(&script, fx.repo.join("scripts/release.sh")).unwrap();
            fx.write(
                "plugins/anthology/.claude-plugin/plugin.json",
                MANIFEST_ANTHOLOGY,
            );
            fx.write(GUARDRAILS_JSON, MANIFEST_GUARDRAILS);
            fx.write(
                "plugins/anthology-superpowers/.claude-plugin/plugin.json",
                MANIFEST_SUPERPOWERS,
            );
            fx.write("vendor/sources.json", SOURCES);
            fx.write("CHANGELOG.md", FAKE_CHANGELOG);
            fx.git(&["add", "-A"]);
            fx.git(&["commit", "-q", "-m", "initial"]);
            let remote = fx.remote.to_str().unwrap().to_string();
            let out = Command::new("git")
                .args(["init", "-q", "--bare", "-b", "main", &remote])
                .env_clear()
                .envs(fx.env())
                .output()
                .unwrap();
            assert!(out.status.success());
            fx.git(&["remote", "add", "origin", &remote]);
            fx.git(&["push", "-q", "origin", "main"]);
            fx
        }

        fn env(&self) -> Vec<(OsString, OsString)> {
            let mut env = base_env();
            env.push(("CLAUDE_BIN".into(), self.fake.clone().into_os_string()));
            env.push(("FAKE_CLAUDE_LOG".into(), self.log.clone().into_os_string()));
            env.push((
                "ANTHOLOGY_DEV".into(),
                env!("CARGO_BIN_EXE_anthology-dev").into(),
            ));
            env
        }

        fn git(&self, args: &[&str]) -> String {
            run_git(&self.repo, &self.env(), args)
        }

        fn write(&self, rel: &str, text: &str) {
            let p = self.repo.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }

        fn read(&self, rel: &str) -> String {
            fs::read_to_string(self.repo.join(rel)).unwrap()
        }

        fn release_with(&self, env: &[(OsString, OsString)], args: &[&str]) -> (i32, String) {
            // stderr is merged into stdout, as the Python test does.
            let out = Command::new("bash")
                .args(["-c", "exec bash \"$0\" \"$@\" 2>&1"])
                .arg(self.repo.join("scripts/release.sh"))
                .args(args)
                .current_dir(&self.repo)
                .env_clear()
                .envs(env.iter().cloned())
                .stdin(Stdio::null())
                .output()
                .expect("bash runs");
            (
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )
        }

        fn release(&self, args: &[&str]) -> (i32, String) {
            self.release_with(&self.env(), args)
        }

        fn state(&self) -> (String, String, String, String) {
            (
                self.git(&["rev-parse", "HEAD"]),
                self.git(&["status", "--porcelain"]),
                self.git(&["tag", "-l"]),
                self.git(&["worktree", "list"]),
            )
        }

        fn remote_refs(&self) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.remote)
                .arg("for-each-ref")
                .env_clear()
                .envs(base_env())
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8_lossy(&out.stdout).into_owned()
        }

        fn claude_log(&self) -> String {
            fs::read_to_string(&self.log).unwrap()
        }
    }

    fn lines(s: &str) -> Vec<&str> {
        s.lines().collect()
    }

    #[test]
    fn test_script_is_executable_in_the_real_repo() {
        // The staging script becomes scripts/release.sh at CO2, so it must be executable now.
        let script = anthology_testkit::staged("scripts/release.sh");
        let mode = fs::metadata(script).unwrap().permissions().mode();
        assert!(mode & 0o100 != 0, "mode {mode:o}");
    }

    #[test]
    fn test_normal_release() {
        let fx = Fx::new();
        let remote_before = fx.remote_refs();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.1"]);
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            fx.git(&["log", "-1", "--format=%s"]),
            "release: anthology-guardrails 0.1.1"
        );
        assert!(
            !fx.git(&["log", "-1", "--format=%B"])
                .contains("Co-Authored-By")
        );
        assert_eq!(
            lines(&fx.git(&["show", "--name-only", "--format=", "HEAD"])),
            ["CHANGELOG.md", GUARDRAILS_JSON]
        );
        assert_eq!(
            fx.read(GUARDRAILS_JSON),
            MANIFEST_GUARDRAILS.replace("0.1.0", "0.1.1")
        );
        let log = fx.read("CHANGELOG.md");
        assert!(log.contains("### [Unreleased]\n\n### [0.1.1] - "), "{log}");
        assert_eq!(log.matches("- G.").count(), 1);
        assert!(
            fx.git(&["tag", "-l"])
                .contains("anthology-guardrails--v0.1.1")
        );
        assert_eq!(
            fx.git(&["tag", "-l", "-n1", "anthology-guardrails--v0.1.1"]),
            "anthology-guardrails--v0.1.1 anthology-guardrails 0.1.1"
        );
        assert_eq!(fx.git(&["status", "--porcelain"]), "");
        assert_eq!(fx.remote_refs(), remote_before);
        assert!(
            fx.claude_log()
                .contains("plugin tag plugins/anthology-guardrails -m anthology-guardrails %s")
        );
        // prints how to publish: one atomic push of the branch and the tag
        assert!(
            out.contains("git push --atomic origin HEAD refs/tags/anthology-guardrails--v0.1.1"),
            "{out}"
        );
    }

    #[test]
    fn test_dry_run_leaves_the_checkout_untouched() {
        let fx = Fx::new();
        let before = fx.state();
        let remote_before = fx.remote_refs();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.1", "--dry-run"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("anthology-guardrails--v0.1.1"), "{out}");
        assert!(out.contains("release: anthology-guardrails 0.1.1"), "{out}");
        assert!(out.contains("CHANGELOG.md"), "{out}");
        assert!(out.contains("+### [0.1.1] - "), "{out}");
        assert_eq!(fx.state(), before);
        assert_eq!(fx.git(&["worktree", "list"]).lines().count(), 1);
        assert_eq!(fx.remote_refs(), remote_before);
        assert!(fx.claude_log().contains("--dry-run"));
    }

    #[test]
    fn test_dry_run_with_push_is_refused() {
        let fx = Fx::new();
        let before = fx.state();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.1", "--dry-run", "--push"]);
        assert_ne!(code, 0);
        assert!(out.contains("--dry-run"), "{out}");
        assert_eq!(fx.state(), before);
    }

    #[test]
    fn test_dirty_tree_is_refused() {
        let fx = Fx::new();
        fx.write("plugins/anthology/note.txt", "x\n");
        let before = fx.state();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.1"]);
        assert_ne!(code, 0);
        assert!(out.contains("not clean"), "{out}");
        assert_eq!(fx.state(), before);
        assert_eq!(fx.claude_log(), "");
    }

    #[test]
    fn test_equal_version_without_tag_is_the_first_release() {
        let fx = Fx::new();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.0"]);
        assert_eq!(code, 0, "{out}");
        assert_eq!(fx.read(GUARDRAILS_JSON), MANIFEST_GUARDRAILS);
        assert_eq!(
            lines(&fx.git(&["show", "--name-only", "--format=", "HEAD"])),
            ["CHANGELOG.md"]
        );
        assert!(fx.read("CHANGELOG.md").contains("### [0.1.0] - "));
        assert!(
            fx.git(&["tag", "-l"])
                .contains("anthology-guardrails--v0.1.0")
        );
    }

    #[test]
    fn test_equal_version_with_existing_tag_is_refused() {
        let fx = Fx::new();
        fx.git(&["tag", "anthology-guardrails--v0.1.0"]);
        let before = fx.state();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.0"]);
        assert_ne!(code, 0);
        assert!(out.contains("already exists"), "{out}");
        assert_eq!(fx.state(), before);
    }

    #[test]
    fn test_lower_version_is_refused() {
        let fx = Fx::new();
        let before = fx.state();
        let (code, out) = fx.release(&["anthology-guardrails", "0.0.9"]);
        assert_ne!(code, 0);
        assert!(out.contains("0.0.9"), "{out}");
        assert_eq!(fx.state(), before);
    }

    #[test]
    fn test_invalid_semver_and_unknown_plugin_are_refused() {
        let fx = Fx::new();
        let before = fx.state();
        assert_ne!(fx.release(&["anthology-guardrails", "0.2"]).0, 0);
        let (code, out) = fx.release(&["nonesuch", "0.1.0"]);
        assert_ne!(code, 0);
        assert!(out.contains("plugins/nonesuch"), "{out}");
        assert_ne!(fx.release(&["../x", "0.1.0"]).0, 0);
        assert_ne!(fx.release(&["anthology-guardrails"]).0, 0);
        assert_eq!(fx.state(), before);
    }

    #[test]
    fn test_empty_unreleased_is_refused() {
        let fx = Fx::new();
        fx.write(
            "CHANGELOG.md",
            &FAKE_CHANGELOG.replace("#### Added\n\n- S.\n", ""),
        );
        fx.git(&["commit", "-q", "-a", "-m", "empty"]);
        let before = fx.state();
        let (code, out) = fx.release(&["anthology-superpowers", "0.1.1"]);
        assert_ne!(code, 0);
        assert!(out.contains("Unreleased"), "{out}");
        assert_eq!(fx.state(), before);
    }

    #[test]
    fn test_dependent_range_rejecting_the_version_is_refused() {
        let fx = Fx::new();
        let before = fx.state();
        let (code, out) = fx.release(&["anthology-guardrails", "0.2.0"]);
        assert_ne!(code, 0);
        assert!(
            out.contains("plugins/anthology/.claude-plugin/plugin.json"),
            "{out}"
        );
        assert!(out.contains("~0.1"), "{out}");
        assert_eq!(fx.state(), before);
        assert_eq!(fx.claude_log(), "");
    }

    #[test]
    fn test_push_from_detached_head_is_refused_with_nothing_changed() {
        let fx = Fx::new();
        fx.git(&["checkout", "-q", "--detach"]);
        let before = fx.state();
        let remote_before = fx.remote_refs();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.1", "--push"]);
        assert_ne!(code, 0);
        assert!(out.contains("checked-out branch"), "{out}");
        assert_eq!(fx.state(), before);
        assert_eq!(fx.remote_refs(), remote_before);
        assert_eq!(fx.claude_log(), "");
    }

    #[test]
    fn test_rejected_branch_push_gives_a_recovery_hint_and_no_tag() {
        let fx = Fx::new();
        let other = fx.tmp.path().join("other");
        let remote = fx.remote.to_str().unwrap();
        let out = Command::new("git")
            .args(["clone", "-q", "-b", "main", remote, other.to_str().unwrap()])
            .env_clear()
            .envs(base_env())
            .output()
            .unwrap();
        assert!(out.status.success());
        let env = fx.env();
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@users.noreply.github.com"),
            ("commit.gpgsign", "false"),
        ] {
            run_git(&other, &env, &["config", key, value]);
        }
        fs::write(other.join("diverge.txt"), "x\n").unwrap();
        run_git(&other, &env, &["add", "diverge.txt"]);
        run_git(&other, &env, &["commit", "-q", "-m", "diverge"]);
        run_git(&other, &env, &["push", "-q", "origin", "main"]);
        let remote_before = fx.remote_refs();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.1", "--push"]);
        assert_ne!(code, 0);
        assert!(out.contains("release commit exists locally"), "{out}");
        assert!(
            out.contains("git push --atomic origin HEAD refs/tags/anthology-guardrails--v0.1.1"),
            "{out}"
        );
        assert!(out.contains("git reset --hard HEAD~1"), "{out}");
        // The tag was made before the push; the rejected push takes it away again.
        assert_eq!(fx.git(&["tag", "-l"]), "");
        assert_eq!(fx.remote_refs(), remote_before);
        assert_eq!(
            fx.git(&["log", "-1", "--format=%s"]),
            "release: anthology-guardrails 0.1.1"
        );
    }

    #[test]
    fn test_dry_run_failure_after_the_worktree_exists_cleans_up() {
        let fx = Fx::new();
        fx.write(
            "CHANGELOG.md",
            &FAKE_CHANGELOG.replace("- G.\n", "- G.\n\n### [0.1.1] - 2026-01-01\n\n- old\n"),
        );
        fx.git(&["commit", "-q", "-a", "-m", "changelog"]);
        let before = fx.state();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.1", "--dry-run"]);
        assert_ne!(code, 0);
        assert!(out.contains("already has a 0.1.1 section"), "{out}");
        assert!(!out.contains("restored"), "{out}");
        assert_eq!(fx.state(), before);
    }

    #[test]
    fn test_dry_run_needs_no_identity_and_skips_hooks() {
        // No user.name/user.email anywhere, and a pre-commit hook that would leave a marker.
        let fx = Fx::new();
        let home = fx.tmp.path().join("home");
        fs::create_dir(&home).unwrap();
        fx.git(&["config", "--unset", "user.name"]);
        fx.git(&["config", "--unset", "user.email"]);
        let marker = fx.tmp.path().join("hook-ran");
        let hook = fx.repo.join(".git/hooks/pre-commit");
        fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let mut env: Vec<(OsString, OsString)> = fx
            .env()
            .into_iter()
            .filter(|(k, _)| !matches!(k.to_str(), Some("HOME" | "XDG_CONFIG_HOME")))
            .collect();
        env.push(("HOME".into(), home.clone().into_os_string()));
        env.push(("XDG_CONFIG_HOME".into(), home.into_os_string()));
        env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
        let (code, out) = fx.release_with(&env, &["anthology-guardrails", "0.1.1", "--dry-run"]);
        assert_eq!(code, 0, "{out}");
        assert!(!marker.exists());
        // The placeholder identity was never written.
        let local = Command::new("git")
            .arg("-C")
            .arg(&fx.repo)
            .args(["config", "--local", "--get-regexp", "^user[.]"])
            .env_clear()
            .envs(base_env())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&local.stdout), "");
    }

    #[test]
    fn test_push_publishes_branch_and_tag() {
        let fx = Fx::new();
        let (code, out) = fx.release(&["anthology-guardrails", "0.1.1", "--push"]);
        assert_eq!(code, 0, "{out}");
        let head = fx.git(&["rev-parse", "HEAD"]);
        let refs = fx.remote_refs();
        assert!(
            refs.contains(&format!("{head} commit\trefs/heads/main")),
            "{refs}"
        );
        assert!(
            refs.contains("refs/tags/anthology-guardrails--v0.1.1"),
            "{refs}"
        );
        // claude only tags; the script pushes the branch and the tag together.
        assert!(!fx.claude_log().contains("--push"));
    }

    #[test]
    fn test_superpowers_release_also_updates_sources_json() {
        let fx = Fx::new();
        let (code, out) = fx.release(&["anthology-superpowers", "0.1.1"]);
        assert_eq!(code, 0, "{out}");
        assert_eq!(
            fx.read("vendor/sources.json"),
            SOURCES.replace(
                "\"plugin_version\": \"0.1.0\"",
                "\"plugin_version\": \"0.1.1\""
            )
        );
        assert_eq!(
            lines(&fx.git(&["show", "--name-only", "--format=", "HEAD"])),
            [
                "CHANGELOG.md",
                "plugins/anthology-superpowers/.claude-plugin/plugin.json",
                "vendor/sources.json"
            ]
        );
        assert_eq!(
            fx.read("plugins/anthology-superpowers/.claude-plugin/plugin.json"),
            MANIFEST_SUPERPOWERS.replace("0.1.0", "0.1.1")
        );
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
