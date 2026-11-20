//! `anthology-dev lock write|check|check-fresh` (RL1; design §8.2, §8.5, §10).
//!
//! The `check-fresh` cases build a small workspace that mirrors the real one in a temporary git
//! repository: `anthology-cli` (a binary with a build script) on `anthology-core` (which pulls
//! in a file from outside `src/`, has a dev-dependency on `anthology-testkit` and a vendored
//! registry dependency `foo`), plus `anthology-testkit` (on the vendored `bar`), `difftest` and
//! `anthology-dev`. The registry is a directory source, so cargo never needs the network. The
//! tag `anthology-bin--v0.1.0` and its locks live in that repository only.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_anthology-dev");
const CARGO: &str = env!("CARGO");
const TAG: &str = "anthology-bin--v0.1.0";

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn hex(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn lock_text(version: &str) -> String {
    format!(
        "{{\n  \"version\": \"{version}\",\n  \"tag\": \"anthology-bin--v{version}\",\n  \"repo\": \"rbonweb/anthology\",\n  \"sha256\": {{\n    \"aarch64-apple-darwin\": \"{}\",\n    \"x86_64-apple-darwin\": \"{}\",\n    \"x86_64-unknown-linux-musl\": \"{}\",\n    \"aarch64-unknown-linux-musl\": \"{}\"\n  }}\n}}\n",
        hex('a'),
        hex('b'),
        hex('c'),
        hex('d')
    )
}

fn checksums_text() -> String {
    format!(
        "{}  anthology-aarch64-apple-darwin.tar.gz\n{}  anthology-aarch64-unknown-linux-musl.tar.gz\n\
         {}  anthology-x86_64-apple-darwin.tar.gz\n{}  anthology-x86_64-unknown-linux-musl.tar.gz\n",
        hex('a'),
        hex('d'),
        hex('b'),
        hex('c')
    )
}

/// Runs anthology-dev in `cwd` with a clean environment (cargo offline).
fn dev(cwd: &Path, args: &[&str]) -> Output {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", path)
        .env("CARGO", CARGO)
        .env("CARGO_NET_OFFLINE", "true")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null());
    for key in ["HOME", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    cmd.output().expect("run anthology-dev")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

// ------------------------------------------------------------------------------------------
// lock write and lock check

/// A plugins folder with both plugin directories.
fn plugins(dir: &Path) -> PathBuf {
    let p = dir.join("plugins");
    for name in ["anthology", "anthology-guardrails"] {
        std::fs::create_dir_all(p.join(name)).unwrap();
    }
    p
}

#[test]
fn lock_write_writes_both_locks_in_the_design_layout() {
    let tmp = tempfile::tempdir().unwrap();
    let p = plugins(tmp.path());
    std::fs::write(tmp.path().join("checksums.txt"), checksums_text()).unwrap();
    let out = dev(
        tmp.path(),
        &[
            "lock",
            "write",
            "--tag",
            TAG,
            "--checksums",
            "checksums.txt",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    for name in ["anthology", "anthology-guardrails"] {
        let got = std::fs::read_to_string(p.join(name).join("anthology.lock")).unwrap();
        assert_eq!(got, lock_text("0.1.0"), "{name}");
    }
    let out = dev(tmp.path(), &["lock", "check"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    // No temporary file is left behind.
    let names: Vec<_> = std::fs::read_dir(p.join("anthology"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["anthology.lock"]);
}

#[test]
fn lock_write_takes_the_repo_and_the_plugins_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let staging = tmp.path().join("port/staging");
    let p = plugins(&staging);
    std::fs::write(tmp.path().join("sums"), checksums_text()).unwrap();
    let out = dev(
        tmp.path(),
        &[
            "lock",
            "write",
            "--tag",
            "anthology-bin--v1.2.3-rc.1",
            "--checksums",
            "sums",
            "--repo",
            "someone/fork",
            "--plugins-dir",
            "port/staging/plugins",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let got = std::fs::read_to_string(p.join("anthology-guardrails/anthology.lock")).unwrap();
    assert!(got.contains("\"repo\": \"someone/fork\""), "{got}");
    assert!(
        got.contains("\"tag\": \"anthology-bin--v1.2.3-rc.1\""),
        "{got}"
    );
    let out = dev(
        tmp.path(),
        &["lock", "check", "--plugins-dir", "port/staging/plugins"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn lock_write_refuses_bad_input_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let p = plugins(tmp.path());
    std::fs::write(tmp.path().join("good"), checksums_text()).unwrap();
    let three: String = checksums_text()
        .lines()
        .take(3)
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(tmp.path().join("three"), three).unwrap();
    for args in [
        &["--tag", "anthology--v0.1.0", "--checksums", "good"][..],
        &["--tag", "rtk--v0.1.0", "--checksums", "good"],
        &["--tag", "anthology-bin--v0.1", "--checksums", "good"],
        &["--tag", TAG, "--checksums", "three"],
        &["--tag", TAG, "--checksums", "missing"],
        &["--tag", TAG, "--checksums", "good", "--repo", "../x"],
    ] {
        let out = dev(tmp.path(), &[&["lock", "write"][..], args].concat());
        assert_eq!(
            out.status.code(),
            Some(1),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert!(!p.join("anthology/anthology.lock").exists(), "{args:?}");
    }
    for args in [
        &["--checksums", "good"][..],
        &["--tag", TAG],
        &["--tag", TAG, "--checksums", "good", "extra"],
    ] {
        let out = dev(tmp.path(), &[&["lock", "write"][..], args].concat());
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
    std::fs::remove_dir(p.join("anthology-guardrails")).unwrap();
    let out = dev(
        tmp.path(),
        &["lock", "write", "--tag", TAG, "--checksums", "good"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("no plugin folder"),
        "{}",
        text(&out.stderr)
    );
}

/// `lock check` on locks with these texts; returns (code, stderr).
fn check_with(anthology: Option<&str>, guardrails: Option<&str>) -> (i32, String) {
    let tmp = tempfile::tempdir().unwrap();
    let p = plugins(tmp.path());
    for (name, t) in [
        ("anthology", anthology),
        ("anthology-guardrails", guardrails),
    ] {
        if let Some(t) = t {
            std::fs::write(p.join(name).join("anthology.lock"), t).unwrap();
        }
    }
    let out = dev(tmp.path(), &["lock", "check"]);
    (out.status.code().unwrap(), text(&out.stderr))
}

#[test]
fn lock_check_validates_every_field_the_layout_and_equality() {
    let good = lock_text("0.1.0");
    assert_eq!(check_with(Some(&good), Some(&good)).0, 0);
    // Without the final newline is still the layout.
    let bare = good.trim_end().to_string();
    assert_eq!(check_with(Some(&bare), Some(&bare)).0, 0);
    let bad = [
        good.replace("\"0.1.0\"", "\"0.1\""),        // version
        good.replace("\"0.1.0\"", "\"0.1.0-rc_1\""), // version
        good.replace("anthology-bin--v0.1.0", "anthology-bin--v0.1.1"), // tag
        good.replace("anthology-bin--v", "anthology--v"), // tag prefix
        good.replace("anthology-bin--v", "rtk--v"),  // tag prefix
        good.replace("rbonweb/anthology", "rbonweb/.."), // repo
        good.replace("rbonweb/anthology", "rbonweb"), // repo
        good.replace(&hex('a'), &hex('A')),          // hash
        good.replace(&hex('a'), &hex('a')[..63]),    // hash
        good.replace("aarch64-apple-darwin", "aarch64-pc-windows-msvc"), // target
        good.replace("  \"repo\"", "  \"extra\": 1,\n  \"repo\""), // key
        good.replace("\n    ", "\n   "),             // layout
        good.replace(",\n  \"tag\"", ", \"tag\""),   // layout
        "{}".to_string(),
        "not json".to_string(),
        String::new(),
    ];
    for b in &bad {
        let (code, err) = check_with(Some(b), Some(b));
        assert_eq!(code, 1, "{b}");
        assert!(err.contains("anthology.lock is not valid"), "{err}");
    }
    let other = lock_text("0.1.0").replace(&hex('a'), &hex('e'));
    let (code, err) = check_with(Some(&good), Some(&other));
    assert_eq!(code, 1);
    assert!(err.contains("differ"), "{err}");
    let (code, err) = check_with(Some(&good), Some(&bare));
    assert_eq!(code, 1, "byte-identical, not just equal");
    assert!(err.contains("differ"), "{err}");
    let (code, err) = check_with(Some(&good), None);
    assert_eq!(code, 1);
    assert!(err.contains("anthology-guardrails"), "{err}");
}

// ------------------------------------------------------------------------------------------
// check-fresh

struct Repo {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

const ROOT_TOML: &str = r#"[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "Apache-2.0"

[workspace.dependencies]
anthology-core = { path = "crates/anthology-core" }
anthology-testkit = { path = "crates/anthology-testkit" }
foo = "1"
bar = "1"
# Declared, but no crate uses it.
unused = "1"

[profile.release]
lto = "thin"
strip = true

[profile.dev]
opt-level = 0
"#;

fn member(name: &str, extra: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion.workspace = true\nedition.workspace = true\n\
         publish = false\n{extra}"
    )
}

fn vendored(name: &str, version: &str, checksum: char) -> [(String, String); 3] {
    let dir = format!("vendor/{name}-{version}");
    [
        (
            format!("{dir}/Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        ),
        (format!("{dir}/src/lib.rs"), "pub fn f() {}\n".into()),
        (
            format!("{dir}/.cargo-checksum.json"),
            format!("{{\"files\":{{}},\"package\":\"{}\"}}", hex(checksum)),
        ),
    ]
}

impl Repo {
    fn new() -> Repo {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let repo = Repo { _tmp: tmp, root };
        let mut files: Vec<(String, String)> = vec![
            ("Cargo.toml".into(), ROOT_TOML.into()),
            ("rust-toolchain.toml".into(), "[toolchain]\nchannel = \"1.100.0\"\n".into()),
            (
                ".cargo/config.toml".into(),
                "[source.crates-io]\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"vendor\"\n"
                    .into(),
            ),
            ("README.md".into(), "readme\n".into()),
            ("data/shipped.txt".into(), "shipped data\n".into()),
            ("data/other.txt".into(), "not shipped\n".into()),
            (
                "crates/anthology-cli/Cargo.toml".into(),
                member(
                    "anthology-cli",
                    "build = \"build.rs\"\n\n[[bin]]\nname = \"anthology\"\npath = \"src/main.rs\"\n\n\
                     [dependencies]\nanthology-core.workspace = true\n",
                ),
            ),
            ("crates/anthology-cli/src/main.rs".into(), "fn main() {}\n".into()),
            (
                "crates/anthology-cli/build.rs".into(),
                "fn main() { println!(\"cargo:rerun-if-changed=build.rs\"); }\n".into(),
            ),
            ("crates/anthology-cli/tests/cli.rs".into(), "#[test]\nfn t() {}\n".into()),
            (
                "crates/anthology-core/Cargo.toml".into(),
                member(
                    "anthology-core",
                    "\n[dependencies]\nfoo.workspace = true\n\n\
                     [target.'cfg(target_os = \"macos\")'.dependencies]\nbaz = \"1\"\n\n\
                     [dev-dependencies]\nanthology-testkit.workspace = true\n",
                ),
            ),
            (
                "crates/anthology-core/src/lib.rs".into(),
                "pub mod inner;\npub const DATA: &str = include_str!(\"../../../data/shipped.txt\");\n"
                    .into(),
            ),
            ("crates/anthology-core/src/inner.rs".into(), "pub fn g() {}\n".into()),
            ("crates/anthology-core/tests/core.rs".into(), "#[test]\nfn t() {}\n".into()),
            (
                "crates/anthology-testkit/Cargo.toml".into(),
                member("anthology-testkit", "\n[dependencies]\nbar.workspace = true\n"),
            ),
            ("crates/anthology-testkit/src/lib.rs".into(), "pub fn k() {}\n".into()),
            (
                "crates/difftest/Cargo.toml".into(),
                member("difftest", "\n[dependencies]\nanthology-core.workspace = true\n"),
            ),
            ("crates/difftest/src/lib.rs".into(), "pub fn d() {}\n".into()),
            (
                "crates/anthology-dev/Cargo.toml".into(),
                member("anthology-dev", "\n[dependencies]\nanthology-core.workspace = true\n"),
            ),
            ("crates/anthology-dev/src/lib.rs".into(), "pub fn v() {}\n".into()),
            ("plugins/anthology/anthology.lock".into(), lock_text("0.1.0")),
            ("plugins/anthology-guardrails/anthology.lock".into(), lock_text("0.1.0")),
        ];
        files.extend(vendored("foo", "1.0.0", '1'));
        files.extend(vendored("bar", "1.0.0", '2'));
        files.extend(vendored("baz", "1.0.0", '5'));
        for (rel, t) in &files {
            repo.write(rel, t);
        }
        repo.git(&["init", "-q", "-b", "main"]);
        repo.cargo(&["generate-lockfile"]);
        repo.commit("initial");
        repo.git(&["tag", TAG]);
        repo
    }

    fn write(&self, rel: &str, t: &str) {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, t).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap()
    }

    fn edit(&self, rel: &str, from: &str, to: &str) {
        let t = self.read(rel);
        assert!(t.contains(from), "{rel} has no {from:?}");
        self.write(rel, &t.replacen(from, to, 1));
    }

    fn run(&self, program: &str, args: &[&str]) {
        let out = Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .env("CARGO_NET_OFFLINE", "true")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{program} {args:?}: {}",
            text(&out.stderr)
        );
    }

    fn git(&self, args: &[&str]) {
        let mut all = vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@users.noreply.github.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
        ];
        all.extend_from_slice(args);
        self.run("git", &all);
    }

    fn cargo(&self, args: &[&str]) {
        self.run(CARGO, args);
    }

    fn commit(&self, msg: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", msg]);
    }

    /// `lock check-fresh` in the repository: (exit code, stdout + stderr).
    fn fresh(&self) -> (i32, String) {
        let out = dev(&self.root, &["lock", "check-fresh"]);
        (
            out.status.code().unwrap(),
            format!("{}{}", text(&out.stdout), text(&out.stderr)),
        )
    }

    fn assert_fresh(&self) {
        let (code, out) = self.fresh();
        assert_eq!(code, 0, "{out}");
        assert!(
            out.contains("the shipped set is unchanged since anthology-bin--v0.1.0"),
            "{out}"
        );
    }

    fn assert_stale(&self, item: &str) {
        let (code, out) = self.fresh();
        assert_eq!(code, 1, "{out}");
        assert!(
            out.contains("the shipped set changed since anthology-bin--v0.1.0"),
            "{out}"
        );
        assert!(
            out.lines().any(|l| l.trim() == item),
            "{item:?} is not listed:\n{out}"
        );
    }
}

/// Builds the repository, or `None` (with a note) where git is missing.
fn repo() -> Option<Repo> {
    if !have_git() {
        eprintln!("skipped: git is not installed");
        return None;
    }
    Some(Repo::new())
}

#[test]
fn fresh_at_the_tag_itself() {
    let Some(r) = repo() else { return };
    r.assert_fresh();
}

#[test]
fn a_test_only_change_passes() {
    let Some(r) = repo() else { return };
    r.write(
        "crates/anthology-core/tests/core.rs",
        "#[test]\nfn t2() {}\n",
    );
    r.write(
        "crates/anthology-cli/tests/more.rs",
        "#[test]\nfn t3() {}\n",
    );
    r.write("crates/anthology-core/benches/b.rs", "fn main() {}\n");
    r.write("README.md", "changed\n");
    r.write("data/other.txt", "changed\n");
    r.commit("tests");
    r.assert_fresh();
}

#[test]
fn a_difftest_change_passes() {
    let Some(r) = repo() else { return };
    r.write("crates/difftest/src/lib.rs", "pub fn d2() {}\n");
    r.edit(
        "crates/difftest/Cargo.toml",
        "[dependencies]",
        "# c\n[dependencies]",
    );
    r.commit("difftest");
    r.assert_fresh();
}

#[test]
fn an_anthology_dev_change_passes() {
    let Some(r) = repo() else { return };
    r.write("crates/anthology-dev/src/lib.rs", "pub fn v2() {}\n");
    r.write("crates/anthology-dev/src/more.rs", "pub fn m() {}\n");
    r.commit("dev");
    r.assert_fresh();
}

#[test]
fn a_dev_dependency_change_passes() {
    let Some(r) = repo() else { return };
    // anthology-testkit is only a dev-dependency of the shipped anthology-core; bar is its
    // registry dependency. Neither ships.
    r.write("crates/anthology-testkit/src/lib.rs", "pub fn k2() {}\n");
    for (rel, t) in vendored("bar", "1.0.1", '3') {
        r.write(&rel, &t);
    }
    r.cargo(&["update", "-p", "bar"]);
    assert!(r.read("Cargo.lock").contains("1.0.1"));
    r.edit("Cargo.toml", "bar = \"1\"", "bar = \"1.0\"");
    r.commit("testkit");
    r.assert_fresh();
}

#[test]
fn unused_dependencies_and_other_profiles_pass() {
    let Some(r) = repo() else { return };
    r.edit("Cargo.toml", "unused = \"1\"", "unused = \"2\"");
    r.edit("Cargo.toml", "opt-level = 0", "opt-level = 1");
    // Comments and blank lines in compared tables do not count.
    r.edit(
        "Cargo.toml",
        "lto = \"thin\"",
        "# why\nlto = \"thin\" # thin\n",
    );
    r.commit("toml");
    r.assert_fresh();
}

#[test]
fn a_shipped_src_change_fails() {
    let Some(r) = repo() else { return };
    r.write("crates/anthology-core/src/inner.rs", "pub fn g2() {}\n");
    r.commit("src");
    r.assert_stale("crates/anthology-core/src/inner.rs");
}

#[test]
fn a_new_shipped_src_file_fails() {
    let Some(r) = repo() else { return };
    r.write("crates/anthology-cli/src/cmd.rs", "pub fn c() {}\n");
    r.commit("src");
    r.assert_stale("crates/anthology-cli/src/cmd.rs");
}

#[test]
fn a_shipped_manifest_change_fails() {
    let Some(r) = repo() else { return };
    r.edit(
        "crates/anthology-cli/Cargo.toml",
        "[dependencies]",
        "[features]\nx = []\n\n[dependencies]",
    );
    r.commit("manifest");
    r.assert_stale("crates/anthology-cli/Cargo.toml");
}

#[test]
fn a_build_script_change_fails() {
    let Some(r) = repo() else { return };
    r.write(
        "crates/anthology-cli/build.rs",
        "fn main() { println!(\"cargo:rustc-env=X=1\"); }\n",
    );
    r.commit("build.rs");
    r.assert_stale("crates/anthology-cli/build.rs");
}

#[test]
fn a_release_profile_change_fails() {
    let Some(r) = repo() else { return };
    r.edit("Cargo.toml", "strip = true", "strip = false");
    r.commit("profile");
    r.assert_stale("Cargo.toml [profile.release]");
}

#[test]
fn a_workspace_version_change_fails() {
    let Some(r) = repo() else { return };
    r.edit("Cargo.toml", "version = \"0.1.0\"", "version = \"0.2.0\"");
    r.cargo(&["update", "--workspace"]);
    r.commit("version");
    r.assert_stale("Cargo.toml [workspace.package] version");
    r.assert_stale("Cargo.lock: anthology-cli");
}

#[test]
fn a_used_workspace_dependency_change_fails() {
    let Some(r) = repo() else { return };
    r.edit(
        "Cargo.toml",
        "foo = \"1\"",
        "foo = { version = \"1\", default-features = false }",
    );
    r.commit("dep");
    r.assert_stale("Cargo.toml [workspace.dependencies] foo");
}

#[test]
fn a_toolchain_or_cargo_config_change_fails() {
    let Some(r) = repo() else { return };
    r.edit(
        "rust-toolchain.toml",
        "[toolchain]",
        "# pinned\n[toolchain]",
    );
    r.edit(
        ".cargo/config.toml",
        "[source.vendored]",
        "# vendored\n[source.vendored]",
    );
    r.commit("toolchain");
    r.assert_stale("rust-toolchain.toml");
    r.assert_stale(".cargo/config.toml");
}

#[test]
fn a_shipped_cargo_lock_block_change_fails() {
    let Some(r) = repo() else { return };
    for (rel, t) in vendored("foo", "1.0.1", '4') {
        r.write(&rel, &t);
    }
    r.cargo(&["update", "-p", "foo"]);
    r.commit("lock");
    r.assert_stale("Cargo.lock: foo");
}

#[test]
fn an_included_file_outside_src_fails() {
    let Some(r) = repo() else { return };
    r.write("data/shipped.txt", "changed\n");
    r.commit("data");
    r.assert_stale("data/shipped.txt");
}

#[test]
fn a_missing_tag_is_an_error() {
    let Some(r) = repo() else { return };
    r.git(&["tag", "-d", TAG]);
    let (code, out) = r.fresh();
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("git fetch --tags"), "{out}");
}

#[test]
fn uncommitted_shipped_changes_are_an_error() {
    let Some(r) = repo() else { return };
    r.write("README.md", "dirty but not shipped\n");
    r.assert_fresh();
    r.write("crates/anthology-core/src/new.rs", "pub fn n() {}\n");
    let (code, out) = r.fresh();
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("uncommitted changes in the shipped set"),
        "{out}"
    );
    assert!(out.contains("crates/anthology-core/src/new.rs"), "{out}");
}

#[test]
fn check_fresh_checks_the_locks_first() {
    let Some(r) = repo() else { return };
    r.write(
        "plugins/anthology/anthology.lock",
        &lock_text("0.1.0").replace(&hex('a'), &hex('f')),
    );
    let (code, out) = r.fresh();
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("differ"), "{out}");
}

#[test]
fn an_unfollowable_include_is_an_error() {
    let Some(r) = repo() else { return };
    r.write(
        "crates/anthology-core/src/inner.rs",
        "pub const X: &str = include_str!(PATH);\n",
    );
    r.commit("include");
    let (code, out) = r.fresh();
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("not a string literal"), "{out}");
}

#[test]
fn a_macos_only_dependency_counts() {
    // check-fresh runs on Linux here; the --target flags must still see a macOS-only crate.
    let Some(r) = repo() else { return };
    for (rel, t) in vendored("baz", "1.0.1", '6') {
        r.write(&rel, &t);
    }
    r.cargo(&["update", "-p", "baz"]);
    r.commit("baz");
    r.assert_stale("Cargo.lock: baz");
}

#[test]
fn a_path_module_outside_src_counts() {
    let Some(r) = repo() else { return };
    r.write("shared/x.rs", "pub fn x() {}\n");
    r.edit(
        "crates/anthology-core/src/lib.rs",
        "pub mod inner;",
        "pub mod inner;\n#[path = \"../../../shared/x.rs\"]\nmod x;",
    );
    r.commit("path module");
    r.write("shared/x.rs", "pub fn x2() {}\n");
    r.commit("change it");
    r.assert_stale("shared/x.rs");
}

#[test]
fn a_build_script_input_counts() {
    let Some(r) = repo() else { return };
    r.write("data/gen.txt", "one\n");
    r.write(
        "crates/anthology-cli/build.rs",
        "fn main() {\n    println!(\"cargo:rerun-if-changed=build.rs\");\n    \
         println!(\"cargo:rerun-if-changed=../../data/gen.txt\");\n    \
         let _ = std::fs::read_to_string(\"../../data/gen.txt\");\n}\n",
    );
    r.commit("build input");
    r.git(&["tag", "-f", TAG]);
    r.write("data/gen.txt", "two\n");
    r.commit("change it");
    r.assert_stale("data/gen.txt");
}

#[test]
fn a_build_script_with_undeclared_inputs_is_an_error() {
    let Some(r) = repo() else { return };
    r.write(
        "crates/anthology-cli/build.rs",
        "fn main() { let _ = std::fs::read_to_string(\"x\"); }\n",
    );
    r.commit("reads");
    let (code, out) = r.fresh();
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("rerun-if-changed"), "{out}");
}

#[test]
fn the_resolver_and_quoted_profile_headers_count() {
    let Some(r) = repo() else { return };
    r.edit("Cargo.toml", "resolver = \"3\"", "resolver = \"2\"");
    r.commit("resolver");
    r.assert_stale("Cargo.toml [workspace]");

    let Some(r) = repo() else { return };
    r.edit(
        "Cargo.toml",
        "[profile.dev]",
        "[profile.\"release\".package.anthology-core]\nopt-level = 0\n\n[profile.dev]",
    );
    r.commit("quoted");
    r.assert_stale("Cargo.toml [profile.release.package.anthology-core]");
}

#[test]
fn a_path_package_outside_the_repository_is_an_error() {
    let Some(r) = repo() else { return };
    let outside = r.root.parent().unwrap().join("outside");
    std::fs::create_dir_all(outside.join("src")).unwrap();
    std::fs::write(
        outside.join("Cargo.toml"),
        "[package]\nname = \"outside\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(outside.join("src/lib.rs"), "pub fn o() {}\n").unwrap();
    r.edit(
        "crates/anthology-core/Cargo.toml",
        "foo.workspace = true",
        "foo.workspace = true\noutside = { path = \"../../../outside\" }",
    );
    r.cargo(&["update", "-p", "anthology-core"]);
    r.commit("outside");
    let (code, out) = r.fresh();
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("outside the repository"), "{out}");
}

#[test]
fn a_crate_root_outside_src_is_an_error() {
    let Some(r) = repo() else { return };
    r.edit(
        "crates/anthology-cli/Cargo.toml",
        "path = \"src/main.rs\"",
        "path = \"main.rs\"",
    );
    r.write("crates/anthology-cli/main.rs", "fn main() {}\n");
    r.commit("root");
    let (code, out) = r.fresh();
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("outside src/"), "{out}");
}
