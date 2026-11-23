//! `port/staging/scripts/release.sh` (RL1; design §8.5): the `check-fresh` precondition and
//! the `anthology-dev release-edit …` calls that replace `release_edit.py`.
//!
//! `release-edit` is DV2's and its behaviour is tested by DV2's port of `tests/test_release.py`;
//! here `$ANTHOLOGY_DEV` is a fake that logs every call (working directory and arguments),
//! passes `lock …` to the real anthology-dev and answers `release-edit` with trivial edits. The
//! repository is a temporary git repository with a small workspace, both plugin locks and the
//! tag `anthology-bin--v0.1.0`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_anthology-dev");
const CARGO: &str = env!("CARGO");
/// Moved to scripts/release.sh by CO2 ([`anthology_testkit::staged`] finds it either way).
const RELEASE_SH: &str = "scripts/release.sh";

const FAKE_DEV: &str = r#"#!/bin/sh
printf '%s|%s\n' "$(pwd -P)" "$*" >> "$DEV_LOG"
if [ "$1" = lock ]; then exec "$REAL_DEV" "$@"; fi
[ "$1" = release-edit ] || exit 70
case $2 in
    plugin-version) sed -n 's/^  "version": "\(.*\)",*$/\1/p' "$3" | head -n 1 ;;
    check-version) echo bump ;;
    changelog-check|check-dependents|set-source-version) : ;;
    changelog-move) printf -- '- released %s %s\n' "$4" "$5" >> "$3" ;;
    set-version)
        sed "s/^  \"version\": \"[^\"]*\"/  \"version\": \"$4\"/" "$3" > "$3.tmp" && mv "$3.tmp" "$3"
        ;;
    *) exit 70 ;;
esac
"#;

const FAKE_CLAUDE: &str = r#"#!/bin/sh
# Fake `claude`: `--version` prints $FAKE_CLAUDE_VERSION (default 9.9.9) and is not logged;
# every other call is recorded; for `plugin tag` it creates the tag with git.
if [ "$1" = --version ]; then echo "${FAKE_CLAUDE_VERSION:-9.9.9} (Claude Code)"; exit 0; fi
printf '%s\n' "$*" >> "$FAKE_CLAUDE_LOG"
[ "$1" = plugin ] && [ "$2" = tag ] || exit 0
shift 2
path=""; dry=0
while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) dry=1 ;;
        --push) ;;
        --remote|-m) shift ;;
        *) path="$1" ;;
    esac
    shift
done
manifest="$path/.claude-plugin/plugin.json"
name=$(sed -n 's/^  "name": "\(.*\)",*$/\1/p' "$manifest" | head -n 1)
ver=$(sed -n 's/^  "version": "\(.*\)",*$/\1/p' "$manifest" | head -n 1)
tag="$name--v$ver"
if [ "$dry" = 1 ]; then echo "$tag"; exit 0; fi
git tag "$tag"
"#;

fn have(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn lock_text() -> String {
    let h = |c: char| -> String { std::iter::repeat_n(c, 64).collect() };
    format!(
        "{{\n  \"version\": \"0.1.0\",\n  \"tag\": \"anthology-bin--v0.1.0\",\n  \"repo\": \"rbonweb/anthology\",\n  \"sha256\": {{\n    \"aarch64-apple-darwin\": \"{}\",\n    \"x86_64-apple-darwin\": \"{}\",\n    \"x86_64-unknown-linux-musl\": \"{}\",\n    \"aarch64-unknown-linux-musl\": \"{}\"\n  }}\n}}\n",
        h('a'),
        h('b'),
        h('c'),
        h('d')
    )
}

fn manifest(name: &str) -> String {
    format!("{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\"\n}}\n")
}

/// A manifest in the shape of plugins/*/.claude-plugin/plugin.json: `engines` after `version`.
fn manifest_with_engines(name: &str, requirement: &str) -> String {
    format!(
        "{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\",\n  \"engines\": {{ \"claude-code\": \"{requirement}\" }}\n}}\n"
    )
}

struct Repo {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
}

/// The repository's observable state: HEAD, status, tags, worktrees.
type State = (String, String, String, String);

impl Repo {
    fn new() -> Repo {
        let tmp = tempfile::tempdir().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("repo");
        let r = Repo {
            _tmp: tmp,
            base,
            root,
        };
        let pkg = |name: &str, extra: &str| {
            format!(
                "[package]\nname = \"{name}\"\nversion.workspace = true\nedition.workspace = true\n{extra}"
            )
        };
        let files: Vec<(&str, String)> =
            vec![
            (
                "Cargo.toml",
                "[workspace]\nresolver = \"3\"\nmembers = [\"crates/*\"]\n\n[workspace.package]\n\
                 version = \"0.1.0\"\nedition = \"2024\"\n\n[workspace.dependencies]\n\
                 anthology-core = { path = \"crates/anthology-core\" }\n"
                    .into(),
            ),
            ("rust-toolchain.toml", "[toolchain]\nchannel = \"1.100.0\"\n".into()),
            (".gitignore", "/target\n".into()),
            (
                "crates/anthology-cli/Cargo.toml",
                pkg("anthology-cli", "\n[dependencies]\nanthology-core.workspace = true\n"),
            ),
            ("crates/anthology-cli/src/main.rs", "fn main() {}\n".into()),
            ("crates/anthology-core/Cargo.toml", pkg("anthology-core", "")),
            ("crates/anthology-core/src/lib.rs", "pub fn f() {}\n".into()),
            ("CHANGELOG.md", "# Changelog\n".into()),
            ("vendor/sources.json", "{}\n".into()),
            ("plugins/anthology/.claude-plugin/plugin.json", manifest("anthology")),
            (
                "plugins/anthology-guardrails/.claude-plugin/plugin.json",
                manifest("anthology-guardrails"),
            ),
            (
                "plugins/anthology-superpowers/.claude-plugin/plugin.json",
                manifest("anthology-superpowers"),
            ),
            ("plugins/anthology/anthology.lock", lock_text()),
            ("plugins/anthology-guardrails/anthology.lock", lock_text()),
        ];
        for (rel, t) in &files {
            r.write(rel, t);
        }
        let script = r.root.join("scripts/release.sh");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::copy(anthology_testkit::staged(RELEASE_SH), &script).unwrap();
        for (name, body) in [("fake-dev", FAKE_DEV), ("fake-claude", FAKE_CLAUDE)] {
            let p = r.base.join(name);
            std::fs::write(&p, body).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            anthology_testkit::rl::settle_exec(
                &p,
                &[("DEV_LOG", "/dev/null"), ("FAKE_CLAUDE_LOG", "/dev/null")],
            );
        }
        r.git(&["init", "-q", "-b", "main"]);
        for kv in [
            "user.name=t",
            "user.email=t@users.noreply.github.com",
            "commit.gpgsign=false",
            "tag.gpgsign=false",
        ] {
            let (k, v) = kv.split_once('=').unwrap();
            r.git(&["config", k, v]);
        }
        let out = Command::new(CARGO)
            .arg("generate-lockfile")
            .current_dir(&r.root)
            .env("CARGO_NET_OFFLINE", "true")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        r.commit("initial");
        r.git(&["tag", "anthology-bin--v0.1.0"]);
        r
    }

    fn write(&self, rel: &str, t: &str) {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, t).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap()
    }

    fn git_out(&self, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", text(&out.stderr));
        text(&out.stdout).trim().to_string()
    }

    fn git(&self, args: &[&str]) {
        if !self.root.exists() {
            std::fs::create_dir_all(&self.root).unwrap();
        }
        self.git_out(args);
    }

    fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
    }

    fn state(&self) -> State {
        (
            self.git_out(&["rev-parse", "HEAD"]),
            self.git_out(&["status", "--porcelain"]),
            self.git_out(&["tag", "-l"]),
            self.git_out(&["worktree", "list", "--porcelain"]),
        )
    }

    fn release_with(&self, dev: &Path, args: &[&str]) -> Output {
        self.script(dev, args).output().unwrap()
    }

    /// Runs the release with a fake `claude` that reports `claude_version` for `--version`.
    fn release_with_claude(&self, claude_version: &str, args: &[&str]) -> Output {
        self.script(&self.base.join("fake-dev"), args)
            .env("FAKE_CLAUDE_VERSION", claude_version)
            .output()
            .unwrap()
    }

    fn script(&self, dev: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new("bash");
        cmd.arg(self.root.join("scripts/release.sh"))
            .args(args)
            .current_dir(&self.root)
            .env("ANTHOLOGY_DEV", dev)
            .env("REAL_DEV", BIN)
            .env("DEV_LOG", self.base.join("dev.log"))
            .env("CLAUDE_BIN", self.base.join("fake-claude"))
            .env("FAKE_CLAUDE_LOG", self.base.join("claude.log"))
            .env("CARGO", CARGO)
            .env("CARGO_NET_OFFLINE", "true")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null());
        cmd
    }

    fn release(&self, args: &[&str]) -> Output {
        self.release_with(&self.base.join("fake-dev"), args)
    }

    /// The fake dev's calls: (working directory, arguments).
    fn dev_calls(&self) -> Vec<(String, String)> {
        std::fs::read_to_string(self.base.join("dev.log"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let (cwd, args) = l.split_once('|').unwrap();
                (cwd.to_string(), args.to_string())
            })
            .collect()
    }

    /// Every dev call ran at the repository root with relative paths only.
    fn assert_calls_are_relative(&self) {
        let root = self.root.display().to_string();
        let calls = self.dev_calls();
        assert!(!calls.is_empty());
        for (cwd, args) in &calls {
            assert_eq!(cwd, &root, "{args}");
            assert!(
                !args.split(' ').any(|a| a.starts_with('/')),
                "an absolute path reaches anthology-dev: {args}"
            );
        }
    }
}

fn repo() -> Option<Repo> {
    for tool in ["bash", "git"] {
        if !have(tool) {
            eprintln!("skipped: {tool} is not installed");
            return None;
        }
    }
    Some(Repo::new())
}

#[test]
fn the_staging_script_is_executable_and_runs_no_python() {
    let path = anthology_testkit::staged(RELEASE_SH);
    let script = std::fs::read_to_string(&path).unwrap();
    assert!(script.starts_with("#!/usr/bin/env bash\n"));
    assert!(
        !script.to_lowercase().contains("python"),
        "release.sh mentions python"
    );
    assert!(script.contains("lock check-fresh"));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "{mode:o}");
    for sub in [
        "plugin-version",
        "check-version",
        "changelog-check",
        "check-dependents",
        "changelog-move",
        "set-version",
        "set-source-version",
    ] {
        assert!(
            script.contains(&format!("dev release-edit {sub} ")),
            "release-edit {sub} is not called through dev"
        );
    }
}

#[test]
fn a_fresh_lock_lets_the_release_through() {
    let Some(r) = repo() else { return };
    let out = r.release(&["anthology", "0.2.0"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(r.git_out(&["tag", "-l"]).contains("anthology--v0.2.0"));
    assert!(
        r.read("plugins/anthology/.claude-plugin/plugin.json")
            .contains("\"0.2.0\"")
    );
    assert!(
        r.read("CHANGELOG.md")
            .contains("- released anthology 0.2.0")
    );
    let calls: Vec<String> = r.dev_calls().into_iter().map(|c| c.1).collect();
    assert_eq!(
        calls,
        [
            "lock check-fresh",
            "release-edit plugin-version plugins/anthology/.claude-plugin/plugin.json",
            "release-edit check-version 0.1.0 0.2.0 0",
            "release-edit changelog-check CHANGELOG.md anthology",
            "release-edit check-dependents . anthology 0.2.0",
            "release-edit changelog-move ./CHANGELOG.md anthology 0.2.0",
            "release-edit set-version ./plugins/anthology/.claude-plugin/plugin.json 0.2.0",
        ]
    );
    r.assert_calls_are_relative();
}

#[test]
fn a_stale_lock_stops_the_release_before_any_edit() {
    let Some(r) = repo() else { return };
    r.write("crates/anthology-core/src/lib.rs", "pub fn f2() {}\n");
    r.commit("shipped change");
    let before = r.state();
    let out = r.release(&["anthology-guardrails", "0.2.0"]);
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "{all}");
    assert!(
        all.contains("release: the anthology binary is stale"),
        "{all}"
    );
    assert!(all.contains("crates/anthology-core/src/lib.rs"), "{all}");
    assert_eq!(r.state(), before);
    let calls: Vec<String> = r.dev_calls().into_iter().map(|c| c.1).collect();
    assert_eq!(calls, ["lock check-fresh"]);
}

#[test]
fn a_lock_pinning_a_binary_older_than_the_workspace_stops_the_release() {
    // A plugin must not ship beside an older guard binary than the crates it was tested with
    // (for example one that cannot look inside `atl` commands).
    let Some(r) = repo() else { return };
    let cargo = r
        .read("Cargo.toml")
        .replace("version = \"0.1.0\"", "version = \"0.2.0\"");
    r.write("Cargo.toml", &cargo);
    r.commit("bump the workspace");
    let before = r.state();
    for plugin in ["anthology", "anthology-guardrails"] {
        let out = r.release(&[plugin, "0.2.0"]);
        let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
        assert_eq!(out.status.code(), Some(1), "{plugin}: {all}");
        assert!(
            all.contains(&format!(
                "release: plugins/{plugin}/anthology.lock pins anthology-bin 0.1.0, older than the workspace version 0.2.0"
            )),
            "{plugin}: {all}"
        );
        assert_eq!(r.state(), before, "{plugin}");
    }
    assert!(r.dev_calls().is_empty(), "{:?}", r.dev_calls());
    // A plugin without a lock is not held back by it.
    let out = r.release(&["anthology-superpowers", "0.2.0"]);
    assert!(
        !text(&out.stderr).contains("older than the workspace version"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn a_missing_binary_tag_stops_the_release() {
    let Some(r) = repo() else { return };
    r.git(&["tag", "-d", "anthology-bin--v0.1.0"]);
    let before = r.state();
    let out = r.release(&["anthology", "0.2.0"]);
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(1), "{all}");
    assert!(all.contains("git fetch --tags"), "{all}");
    assert_eq!(r.state(), before);
}

#[test]
fn a_plugin_without_a_lock_skips_check_fresh() {
    let Some(r) = repo() else { return };
    // Stale for the binary, but anthology-superpowers ships no anthology.lock.
    r.write("crates/anthology-core/src/lib.rs", "pub fn f2() {}\n");
    r.commit("shipped change");
    let out = r.release(&["anthology-superpowers", "0.2.0"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(r.dev_calls().iter().all(|c| !c.1.starts_with("lock")));
    assert!(
        r.git_out(&["tag", "-l"])
            .contains("anthology-superpowers--v0.2.0")
    );
}

#[test]
fn the_dry_run_edits_a_worktree_under_target_and_leaves_the_checkout() {
    let Some(r) = repo() else { return };
    let before = r.state();
    let out = r.release(&["anthology", "0.2.0", "--dry-run"]);
    let all = format!("{}{}", text(&out.stdout), text(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{all}");
    assert!(
        all.contains("== dry run: nothing in this checkout was changed =="),
        "{all}"
    );
    assert!(all.contains("- released anthology 0.2.0"), "{all}");
    assert_eq!(r.state(), before);
    let calls = r.dev_calls();
    let mv = calls
        .iter()
        .find(|c| c.1.starts_with("release-edit changelog-move "))
        .unwrap();
    assert!(
        mv.1.starts_with("release-edit changelog-move target/release-dry-run."),
        "{mv:?}"
    );
    assert!(mv.1.contains("/wt/CHANGELOG.md anthology 0.2.0"), "{mv:?}");
    r.assert_calls_are_relative();
    let left: Vec<_> = std::fs::read_dir(r.root.join("target"))
        .map(|d| d.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{left:?}");
    assert!(!r.read("CHANGELOG.md").contains("released"));
}

#[test]
fn a_missing_anthology_dev_is_refused() {
    let Some(r) = repo() else { return };
    let before = r.state();
    let out = r.release_with(&r.base.join("no-such-dev"), &["anthology", "0.2.0"]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("ANTHOLOGY_DEV"), "{err}");
    assert_eq!(r.state(), before);
}

/// A repository whose `anthology` plugin requires claude-code `>=2.1.330`.
fn repo_with_engines() -> Option<Repo> {
    let r = repo()?;
    r.write(
        "plugins/anthology/.claude-plugin/plugin.json",
        &manifest_with_engines("anthology", ">=2.1.330"),
    );
    r.commit("require claude-code 2.1.330");
    Some(r)
}

fn output_text(out: &Output) -> String {
    format!("{}{}", text(&out.stdout), text(&out.stderr))
}

#[test]
fn an_older_claude_than_the_engines_requirement_stops_the_release_before_any_edit() {
    let Some(r) = repo_with_engines() else { return };
    let before = r.state();
    for args in [
        &["anthology", "0.2.0"][..],
        &["anthology", "0.2.0", "--dry-run"][..],
    ] {
        let out = r.release_with_claude("2.1.329", args);
        let all = output_text(&out);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {all}");
        assert!(
            all.contains("release: plugin anthology needs claude-code >=2.1.330, but"),
            "{args:?}: {all}"
        );
        assert!(all.contains("2.1.329"), "{args:?}: {all}");
        assert!(all.contains("CLAUDE_BIN"), "{args:?}: {all}");
        assert_eq!(r.state(), before, "{args:?}");
    }
    // Refused before the lock check, the edits, the commit, the push and the tag.
    assert!(r.dev_calls().is_empty(), "{:?}", r.dev_calls());
    let claude_log = std::fs::read_to_string(r.base.join("claude.log")).unwrap_or_default();
    assert!(claude_log.is_empty(), "{claude_log}");
}

#[test]
fn a_claude_equal_to_the_engines_requirement_lets_the_release_through() {
    let Some(r) = repo_with_engines() else { return };
    let out = r.release_with_claude("2.1.330", &["anthology", "0.2.0"]);
    assert_eq!(out.status.code(), Some(0), "{}", output_text(&out));
    assert!(r.git_out(&["tag", "-l"]).contains("anthology--v0.2.0"));
}

#[test]
fn a_claude_newer_than_the_engines_requirement_lets_the_release_through() {
    let Some(r) = repo_with_engines() else { return };
    let out = r.release_with_claude("2.2.0", &["anthology", "0.2.0"]);
    assert_eq!(out.status.code(), Some(0), "{}", output_text(&out));
    assert!(r.git_out(&["tag", "-l"]).contains("anthology--v0.2.0"));
}

#[test]
fn claude_versions_are_compared_by_number_not_by_text() {
    let Some(r) = repo_with_engines() else { return };
    let before = r.state();
    // (reported version, accepted); `>=2.1.330` is the requirement. "2.1.99" is text-greater
    // than "2.1.330" and "10.0.0" text-smaller than "2.1.330": both would pass a text compare.
    for (version, accepted) in [
        ("2.1.99", false),
        ("2.0.1000", false),
        ("1.99.999", false),
        ("2.1.329-beta.1", false),
        ("2.1.330", true),
        ("2.1.330-beta.1", true),
        ("2.1.1000", true),
        ("2.10.0", true),
        ("10.0.0", true),
    ] {
        let out = r.release_with_claude(version, &["anthology", "0.2.0", "--dry-run"]);
        let all = output_text(&out);
        let want = if accepted { 0 } else { 1 };
        assert_eq!(out.status.code(), Some(want), "claude {version}: {all}");
        assert_eq!(r.state(), before, "claude {version}");
    }
}

#[test]
fn a_plugin_without_engines_skips_the_claude_check() {
    let Some(r) = repo() else { return };
    // The manifest has no `engines`: even a very old claude is let through.
    let out = r.release_with_claude("0.0.1", &["anthology", "0.2.0"]);
    assert_eq!(out.status.code(), Some(0), "{}", output_text(&out));
    assert!(r.git_out(&["tag", "-l"]).contains("anthology--v0.2.0"));
}

#[test]
fn a_claude_that_reports_no_version_is_refused_when_the_plugin_has_engines() {
    let Some(r) = repo_with_engines() else { return };
    let before = r.state();
    let out = r.release_with_claude("unknown", &["anthology", "0.2.0"]);
    let all = output_text(&out);
    assert_eq!(out.status.code(), Some(1), "{all}");
    assert!(all.contains("cannot read the version of"), "{all}");
    assert!(all.contains("CLAUDE_BIN"), "{all}");
    assert_eq!(r.state(), before);
}
