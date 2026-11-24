//! Ports of tests/test_version_bump.py (DV2): one `mod` per Python class, one test per method.
//!
//! The pure-logic classes call the library; `git_integration_tests` builds throwaway repositories
//! with git and drives `version_bump::run_in`, as the Python tests drive `cvb.main`.

use std::cmp::Ordering;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anthology_core::Env;
use anthology_dev::semver;
use anthology_dev::version_bump::{self, changed_plugins, verdict};
use anthology_pycompat::json::{Int, Value};

const GATE: &str = "scripts/check_version_bump.py";
const ZEROS: &str = "0000000000000000000000000000000000000000";

fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

fn compare(a: &str, b: &str) -> i32 {
    match semver::compare(a, b).expect("both valid") {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

mod semver_tests {
    use super::*;

    #[test]
    fn test_parse_valid() {
        for v in [
            "0.1.0",
            "1.2.3",
            "10.20.30",
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0+build.5",
            "1.0.0-rc.1+build.5",
            "1.0.0-0.3.7",
            "1.0.0-x-y-z",
        ] {
            assert!(semver::parse(v).is_some(), "{v}");
        }
    }

    #[test]
    fn test_parse_invalid() {
        for v in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.3-",
            "1.2.3-01",
            "v1.2.3",
            "1.2.3+",
            "1.2.3-a..b",
            "a.b.c",
            " 1.2.3",
            "1.2.3-alpha_1",
            "1.2.3\n",
            "1.2.3-rc.1\n",
            "1.2.3+b\n",
        ] {
            assert!(semver::parse(v).is_none(), "{v:?}");
        }
        // A value that is not a string (`None`, `3`) is not a version either.
        assert!(semver::parse_value(&Value::Null).is_none());
        assert!(semver::parse_value(&Value::Int(Int::Small(3))).is_none());
    }

    #[test]
    fn test_precedence() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
            "10.0.0",
        ];
        for (i, a) in ordered.iter().enumerate() {
            for (j, b) in ordered.iter().enumerate() {
                let want = i32::from(i > j) - i32::from(i < j);
                assert_eq!(compare(a, b), want, "{a} vs {b}");
            }
        }
    }

    #[test]
    fn test_numeric_not_lexicographic() {
        assert!(compare("0.10.0", "0.9.0") > 0);
    }

    #[test]
    fn test_build_metadata_ignored() {
        assert_eq!(compare("1.0.0+a", "1.0.0+b"), 0);
    }
}

mod changed_plugins_tests {
    use super::*;

    #[test]
    fn test_groups_by_plugin() {
        let paths = [
            "plugins/a/x.md",
            "plugins/a/sub/y.md",
            "plugins/b/z",
            "README.md",
            "scripts/vendor.py",
            "plugins/top-level-file",
        ];
        assert_eq!(changed_plugins(&paths), ["a", "b"]);
    }

    #[test]
    fn test_prefix_is_a_directory_boundary() {
        assert_eq!(
            changed_plugins(&["plugins/anthology-guardrails/x"]),
            ["anthology-guardrails"]
        );
    }

    #[test]
    fn test_nothing() {
        assert!(changed_plugins::<&str>(&[]).is_empty());
        assert!(changed_plugins(&["tests/x.py", ".github/workflows/ci.yml"]).is_empty());
    }
}

mod verdict_tests {
    use super::*;

    fn v(plugin: &str, base: &Value, head: &Value) -> (bool, String) {
        verdict(plugin, base, head, true, true)
    }

    #[test]
    fn test_bumped() {
        let (ok, _msg) = v("p", &s("0.1.0"), &s("0.1.1"));
        assert!(ok);
    }

    #[test]
    fn test_equal_fails() {
        let (ok, msg) = v("p", &s("0.1.0"), &s("0.1.0"));
        assert!(!ok);
        assert_eq!(
            msg,
            "plugins/p changed without a version bump (still 0.1.0)"
        );
    }

    #[test]
    fn test_lower_fails() {
        let (ok, msg) = v("p", &s("0.2.0"), &s("0.1.9"));
        assert!(!ok);
        assert!(msg.contains("0.2.0"), "{msg}");
        assert!(msg.contains("0.1.9"), "{msg}");
    }

    #[test]
    fn test_prerelease_below_release() {
        assert!(!v("p", &s("1.0.0"), &s("1.0.0-rc.1")).0);
        assert!(v("p", &s("1.0.0-rc.1"), &s("1.0.0")).0);
    }

    #[test]
    fn test_invalid_head_fails() {
        for bad in [
            s("banana"),
            s("1.2"),
            Value::Null,
            Value::Int(Int::Small(7)),
        ] {
            let (ok, msg) = v("p", &s("0.1.0"), &bad);
            assert!(!ok, "{bad:?}");
            assert!(msg.contains("not valid semver"), "{msg}");
        }
        // The message carries the Python repr of the value.
        assert_eq!(
            v("p", &s("0.1.0"), &Value::Null).1,
            "plugins/p version None is not valid semver"
        );
        assert_eq!(
            v("p", &s("0.1.0"), &s("banana")).1,
            "plugins/p version 'banana' is not valid semver"
        );
    }

    #[test]
    fn test_new_or_removed_plugin_passes() {
        assert!(verdict("p", &Value::Null, &s("0.1.0"), false, true).0);
        assert!(verdict("p", &s("0.1.0"), &Value::Null, true, false).0);
    }

    #[test]
    fn test_invalid_base_version_passes_if_head_valid() {
        // A broken base version must not lock the plugin out of ever fixing it.
        let (ok, msg) = v("p", &s("garbage"), &s("0.1.0"));
        assert!(ok);
        assert_eq!(msg, "plugins/p 0.1.0 (base version 'garbage' was invalid)");
    }
}

mod git_integration_tests {
    use super::*;

    /// A throwaway repository, as `GitIntegrationTests.setUp` builds it.
    struct Fx {
        _tmp: tempfile::TempDir,
        repo: PathBuf,
        stderr: String,
    }

    /// The process environment without `GIT_*`, as the Python helper passes to git.
    fn clean_env() -> Env {
        let mut env = Env::from_process();
        let names: Vec<String> = std::env::vars_os()
            .filter_map(|(k, _)| k.into_string().ok())
            .filter(|k| k.starts_with("GIT_"))
            .collect();
        for k in names {
            env = env.without(&k);
        }
        env
    }

    impl Fx {
        fn new() -> Fx {
            let tmp = tempfile::tempdir().expect("temp dir");
            let repo = tmp.path().to_path_buf();
            let fx = Fx {
                _tmp: tmp,
                repo,
                stderr: String::new(),
            };
            fx.git(&["init", "-q", "-b", "main"]);
            fx.git(&["config", "user.name", "Test"]);
            fx.git(&["config", "user.email", "test@users.noreply.github.com"]);
            fx.git(&["config", "commit.gpgsign", "false"]);
            fx
        }

        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.repo)
                .args(args)
                .env_clear()
                .envs(clean_env().iter())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .expect("git runs");
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        fn write(&self, rel: &str, text: &str) {
            let p = self.repo.join(rel);
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(p, text).expect("write");
        }

        fn set_version(&self, plugin: &str, version: &str) {
            self.write(
                &format!("plugins/{plugin}/.claude-plugin/plugin.json"),
                &format!("{{\"name\": \"{plugin}\", \"version\": \"{version}\"}}\n"),
            );
        }

        fn install_gate(&self) {
            self.write(GATE, "# gate marker\n");
        }

        fn commit(&self, msg: &str) -> String {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", msg]);
            self.git(&["rev-parse", "HEAD"])
        }

        fn run_gate(&mut self, base: &str, head: &str) -> (i32, String) {
            let args: Vec<OsString> = vec![
                base.into(),
                head.into(),
                "--repo".into(),
                self.repo.clone().into_os_string(),
            ];
            let out =
                version_bump::run_in(&args, &mut std::io::empty(), &clean_env(), Path::new("."));
            self.stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            (out.code, String::from_utf8_lossy(&out.stdout).into_owned())
        }

        /// Commit plugin `p`; by default tag it `p--v<version>` (the gate only binds released
        /// plugins).
        fn base_with_plugin(&self, version: &str, released: bool) -> String {
            self.install_gate();
            self.set_version("p", version);
            self.write("plugins/p/skill.md", "one\n");
            let sha = self.commit("base");
            if released {
                self.git(&["tag", &format!("p--v{version}")]);
            }
            sha
        }
    }

    #[test]
    fn test_a_change_without_bump_fails() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.write("plugins/p/skill.md", "two\n");
        let head = fx.commit("edit");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("FAIL"), "{out}");
        assert!(
            out.contains("plugins/p changed without a version bump (still 0.1.0)"),
            "{out}"
        );
    }

    #[test]
    fn test_unreleased_plugin_is_skipped() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", false);
        fx.write("plugins/p/skill.md", "two\n");
        let head = fx.commit("edit without bump");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(
            out.contains("p: unreleased (no p--v* tag) \u{2014} version bump not required"),
            "{out}"
        );
        assert!(!out.contains("FAIL"), "{out}");
    }

    #[test]
    fn test_released_plugin_is_enforced() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", false);
        fx.git(&["tag", "p--v0.1.0"]);
        fx.write("plugins/p/skill.md", "two\n");
        let head = fx.commit("edit without bump");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p changed without a version bump (still 0.1.0)"),
            "{out}"
        );
    }

    #[test]
    fn test_another_plugins_tag_does_not_release_this_one() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", false);
        fx.git(&["tag", "pp--v0.1.0"]);
        fx.git(&["tag", "p-v0.1.0"]);
        fx.write("plugins/p/skill.md", "two\n");
        let head = fx.commit("edit without bump");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("unreleased"), "{out}");
    }

    #[test]
    fn test_b_change_with_bump_passes() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.1.1");
        let head = fx.commit("edit+bump");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("ok"), "{out}");
        assert!(out.contains("plugins/p 0.1.0 -> 0.1.1"), "{out}");
    }

    #[test]
    fn test_c_no_plugin_change_passes() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.write("docs.md", "x\n");
        let head = fx.commit("docs");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("version gate: no plugin changed"), "{out}");
    }

    #[test]
    fn test_d_base_without_script_is_noop() {
        let mut fx = Fx::new();
        fx.set_version("p", "0.1.0");
        let base = fx.commit("base without gate");
        fx.install_gate();
        fx.write("plugins/p/skill.md", "two\n");
        let head = fx.commit("add gate and edit plugin");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(
            out.contains("version gate: base predates the gate; skipping"),
            "{out}"
        );
    }

    #[test]
    fn test_e_new_plugin_passes() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.set_version("q", "0.1.0");
        let head = fx.commit("new plugin");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
    }

    #[test]
    fn test_removed_plugin_passes() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fs::remove_dir_all(fx.repo.join("plugins").join("p")).expect("rmtree");
        let head = fx.commit("remove plugin");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plugins/p was removed"), "{out}");
    }

    #[test]
    fn test_f_version_decrease_fails() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.2.0", true);
        fx.set_version("p", "0.1.0");
        let head = fx.commit("downgrade");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(out.contains("FAIL"), "{out}");
        assert!(
            out.contains("plugins/p version went down (0.2.0 -> 0.1.0)"),
            "{out}"
        );
    }

    #[test]
    fn test_invalid_head_version_fails() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.set_version("p", "banana");
        let head = fx.commit("bad version");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p version 'banana' is not valid semver"),
            "{out}"
        );
    }

    #[test]
    fn test_g_merge_base_semantics() {
        let mut fx = Fx::new();
        fx.base_with_plugin("0.1.0", true);
        // The base branch advances with its own bump after the branch point.
        fx.git(&["checkout", "-q", "-b", "feature"]);
        fx.write("plugins/p/skill.md", "feature edit\n");
        fx.set_version("p", "0.1.1");
        let feature_head = fx.commit("feature edit+bump");
        fx.git(&["checkout", "-q", "main"]);
        fx.write("plugins/p/other.md", "main edit\n");
        fx.set_version("p", "0.2.0");
        let main_tip = fx.commit("main edit+bump");
        // 0.1.1 < 0.2.0 on main, but the feature branch is judged against the merge base.
        let (code, out) = fx.run_gate(&main_tip, &feature_head);
        assert_eq!(code, 0, "{out}");
        // And a feature branch without a bump still fails whatever main did.
        fx.git(&["checkout", "-q", "feature"]);
        fx.set_version("p", "0.1.0");
        fx.write("plugins/p/skill.md", "feature edit 2\n");
        let unbumped = fx.commit("feature without bump");
        let (code, out) = fx.run_gate(&main_tip, &unbumped);
        assert_eq!(code, 1, "{out}");
    }

    #[test]
    fn test_zero_base_is_noop() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        let (code, out) = fx.run_gate(ZEROS, &base);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("new branch"), "{out}");
    }

    #[test]
    fn test_unresolvable_ref_exits_2() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        let (code, out) = fx.run_gate(&"f".repeat(40), &base);
        assert_eq!(code, 2, "{out}");
        assert!(
            fx.stderr.contains("version gate: cannot resolve"),
            "{}",
            fx.stderr
        );
    }

    #[test]
    fn test_no_merge_base_exits_2() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.git(&["checkout", "-q", "--orphan", "other"]);
        fx.git(&["rm", "-rqf", "--cached", "."]);
        fx.write("unrelated.txt", "x\n");
        let other = fx.commit("unrelated history");
        let (code, out) = fx.run_gate(&base, &other);
        assert_eq!(code, 2, "{out}");
        assert!(
            fx.stderr.contains("version gate: no merge base"),
            "{}",
            fx.stderr
        );
    }

    #[test]
    fn test_non_ascii_path_under_a_plugin_still_needs_a_bump() {
        // git quotes non-ASCII paths ("plugins/p/caf\303\251.md") unless told not to; the
        // gate must still see them as plugin files.
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.write("plugins/p/caf\u{e9}.md", "one\n");
        let head = fx.commit("add a non-ascii file");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p changed without a version bump"),
            "{out}"
        );
    }

    // The bump gate judges a head against the latest release tag, and against the merge base only
    // to refuse a version that went down.

    #[test]
    fn an_unreleased_bump_on_the_base_counts() {
        // Another change already raised the version on the base branch; this branch need not
        // bump it again.
        let mut fx = Fx::new();
        fx.base_with_plugin("0.1.0", true);
        fx.set_version("p", "0.2.0");
        let base = fx.commit("bump on the base");
        fx.write("plugins/p/skill.md", "two\n");
        let head = fx.commit("edit without a new bump");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(
            out.contains("plugins/p 0.2.0 (bump already on the base; latest release 0.1.0)"),
            "{out}"
        );
    }

    #[test]
    fn head_must_exceed_the_highest_release() {
        // The branch was cut at 0.1.0, then 0.2.0 was released elsewhere. 0.1.5 is above the
        // merge base but not above the release.
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.git(&["tag", "-a", "-m", "release", "p--v0.2.0"]);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.1.5");
        let head = fx.commit("edit+bump below the release");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p version 0.1.5 is not above the latest release 0.2.0"),
            "{out}"
        );
    }

    #[test]
    fn head_below_base_fails() {
        // 0.4.0 is above the release (0.1.0) but below the version already on the base.
        let mut fx = Fx::new();
        fx.base_with_plugin("0.1.0", true);
        fx.set_version("p", "0.5.0");
        let base = fx.commit("bump on the base");
        fx.set_version("p", "0.4.0");
        let head = fx.commit("lower the version");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p version went down (0.5.0 -> 0.4.0)"),
            "{out}"
        );
    }

    #[test]
    fn the_highest_tag_is_semver_not_lexical() {
        // Lexically `0.9.0` sorts above `0.10.0`; as semver it is the other way round.
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.9.0", true);
        fx.git(&["tag", "p--v0.10.0"]);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.9.5");
        let below = fx.commit("bump below the release");
        let (code, out) = fx.run_gate(&base, &below);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p version 0.9.5 is not above the latest release 0.10.0"),
            "{out}"
        );
        fx.set_version("p", "0.10.1");
        let above = fx.commit("bump above the release");
        let (code, out) = fx.run_gate(&base, &above);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plugins/p 0.9.0 -> 0.10.1"), "{out}");
    }

    #[test]
    fn a_plugin_name_prefix_does_not_match_another() {
        // `pp` and `p-q` are other plugins; their tags are not releases of `p`.
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.git(&["tag", "pp--v9.0.0"]);
        fx.git(&["tag", "p-q--v9.0.0"]);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.1.1");
        let head = fx.commit("edit+bump");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plugins/p 0.1.0 -> 0.1.1"), "{out}");
    }

    #[test]
    fn a_tag_that_is_not_semver_is_ignored() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.git(&["tag", "p--vbanana"]);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.1.1");
        let head = fx.commit("edit+bump");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plugins/p 0.1.0 -> 0.1.1"), "{out}");
    }

    #[test]
    fn a_plugin_released_only_under_an_invalid_tag_falls_back_to_the_base() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", false);
        fx.git(&["tag", "p--vbanana"]);
        fx.write("plugins/p/skill.md", "two\n");
        let unbumped = fx.commit("edit without bump");
        let (code, out) = fx.run_gate(&base, &unbumped);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p changed without a version bump (still 0.1.0)"),
            "{out}"
        );
        fx.set_version("p", "0.1.1");
        let bumped = fx.commit("bump");
        let (code, out) = fx.run_gate(&base, &bumped);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plugins/p 0.1.0 -> 0.1.1"), "{out}");
    }

    // The gate also runs on a push to the base branch, after a release: the release commit then
    // carries the new tag. A tag made inside base..head is not a release the head must exceed.

    #[test]
    fn a_release_tag_made_in_the_pushed_range_is_ignored() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.2.0");
        let head = fx.commit("release 0.2.0");
        // An annotated tag, as `claude plugin tag` makes one.
        fx.git(&["tag", "-a", "-m", "release", "p--v0.2.0"]);
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plugins/p 0.1.0 -> 0.2.0"), "{out}");
    }

    #[test]
    fn a_release_tag_in_the_range_does_not_excuse_a_missing_bump() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.write("plugins/p/skill.md", "two\n");
        let head = fx.commit("edit without bump");
        fx.git(&["tag", "-a", "-m", "release", "p--v0.2.0"]);
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p changed without a version bump (still 0.1.0)"),
            "{out}"
        );
    }

    #[test]
    fn an_invalid_base_version_still_needs_head_above_the_release() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("garbage", false);
        fx.git(&["tag", "p--v0.2.0"]);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.1.0");
        let below = fx.commit("fix the version, below the release");
        let (code, out) = fx.run_gate(&base, &below);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p version 0.1.0 is not above the latest release 0.2.0"),
            "{out}"
        );
        fx.set_version("p", "0.3.0");
        let above = fx.commit("fix the version, above the release");
        let (code, out) = fx.run_gate(&base, &above);
        assert_eq!(code, 0, "{out}");
        assert!(
            out.contains("plugins/p 0.3.0 (base version 'garbage' was invalid)"),
            "{out}"
        );
    }

    #[test]
    fn a_head_equal_to_the_release_but_above_the_base_fails() {
        // The base is below the release (the branch predates it); head == release is no bump.
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.git(&["tag", "p--v0.2.0"]);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.2.0");
        let head = fx.commit("bump to the released version");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p version 0.2.0 is not above the latest release 0.2.0"),
            "{out}"
        );
    }

    #[test]
    fn a_prerelease_is_below_its_release() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.git(&["tag", "p--v0.2.0"]);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.2.0-x.1");
        let head = fx.commit("bump to a prerelease of the released version");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("plugins/p version 0.2.0-x.1 is not above the latest release 0.2.0"),
            "{out}"
        );
    }

    #[test]
    fn a_release_is_above_its_prerelease_tag() {
        let mut fx = Fx::new();
        let base = fx.base_with_plugin("0.1.0", true);
        fx.git(&["tag", "p--v0.2.0-x.1"]);
        fx.write("plugins/p/skill.md", "two\n");
        fx.set_version("p", "0.2.0");
        let head = fx.commit("bump to the release");
        let (code, out) = fx.run_gate(&base, &head);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("plugins/p 0.1.0 -> 0.2.0"), "{out}");
    }
}
