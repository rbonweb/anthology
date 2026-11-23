//! `scripts/sync-atl.sh` over a fixture: a local "upstream" repository with two releases, and
//! a fork whose `atl/` is the first release, renamed, plus a change of the fork's own. The
//! tools the sync calls (`scripts/dev.sh`, the trim, the test run) are stubs in the fixture;
//! the sync script and the rename script are the real ones.
//!
//! The old name is built from the constants below, so no fixture line has to carry it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use repo_tests::repo;
use tempfile::TempDir;

const LOW: &str = "rtk"; // legacy
const UP: &str = "RTK"; // legacy

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .args(["-c", "init.defaultBranch=main", "-c", "core.autocrlf=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .expect("git ran");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git_text(dir: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&git(dir, args).stdout)
        .trim()
        .to_string()
}

fn put(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn get(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

const LICENSE: &str = "Apache License\nCopyright 2026 RTK AI Labs\n"; // legacy

/// The first upstream release: the old name in a source file, a manifest and a file name.
fn release_one(root: &Path) {
    put(
        root,
        "src/main.rs",
        &format!(
            "fn main() {{\n    println!(\"{LOW} 1.0\");\n}}\n\n// section one\nfn one() {{}}\n\n\
             // section two\nfn two() {{}}\n"
        ),
    );
    put(
        root,
        "Cargo.toml",
        &format!("[package]\nname = \"{LOW}\"\n"),
    );
    put(
        root,
        &format!("docs/{LOW}-notes.md"),
        &format!("{UP} notes\n"),
    );
    put(root, "LICENSE", LICENSE);
}

/// The second: a change near the top of the source, a new file named for the old name, and a
/// change to the notes.
fn release_two(root: &Path) {
    put(
        root,
        "src/main.rs",
        &format!(
            "fn main() {{\n    println!(\"{LOW} 1.1\");\n}}\n\n// section one\nfn one() {{}}\n\n\
             // section two\nfn two() {{}}\n\nfn {LOW}_extra() {{}}\n"
        ),
    );
    put(
        root,
        &format!("src/{LOW}_new.rs"),
        &format!("pub fn {LOW}_new() {{}}\n"),
    );
    put(
        root,
        &format!("docs/{LOW}-notes.md"),
        &format!("{UP} notes, second edition\n"),
    );
}

struct Fixture {
    _dir: TempDir,
    upstream: PathBuf,
    fork: PathBuf,
}

/// An upstream with the tags v1.0.0 and v1.0.1, and a fork on `main` at v1.0.0.
/// The fork's own change to `atl/src/main.rs` replaces `from` (the renamed text) by `to`.
fn fixture(from: &str, to: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let upstream = dir.path().join("upstream");
    let fork = dir.path().join("fork");
    fs::create_dir_all(&upstream).unwrap();
    fs::create_dir_all(&fork).unwrap();

    git(&upstream, &["init", "-q"]);
    release_one(&upstream);
    git(&upstream, &["add", "-A"]);
    git(&upstream, &["commit", "-q", "-m", "release one"]);
    git(&upstream, &["tag", "v1.0.0"]);
    let one = git_text(&upstream, &["rev-parse", "HEAD"]);
    release_two(&upstream);
    git(&upstream, &["add", "-A"]);
    git(&upstream, &["commit", "-q", "-m", "release two"]);
    git(&upstream, &["tag", "v1.0.1"]);

    // The fork: the scripts (two real, three stubs), the pin, and atl/ = release one, renamed.
    git(&fork, &["init", "-q"]);
    for script in ["atl-rename.sh", "sync-atl.sh"] {
        put(
            &fork,
            &format!("scripts/{script}"),
            &repo::read(&format!("scripts/{script}")),
        );
    }
    put(
        &fork,
        "scripts/dev.sh",
        "#!/usr/bin/env bash\n\
         set -eu\n\
         case $1 in\n\
         vendor)\n\
             ref=$4; commit=$6\n\
             old=$(cat vendor/pin)\n\
             echo \"$ref $commit\" > vendor/pin\n\
             echo \"$old\"\n\
             ;;\n\
         atl-trim) cat >/dev/null ;;\n\
         esac\n",
    );
    put(
        &fork,
        "scripts/atl-trim.sh",
        "#!/usr/bin/env bash\necho trimmed\n",
    );
    put(
        &fork,
        "scripts/atl-test.sh",
        "#!/usr/bin/env bash\necho tested\n",
    );
    for script in [
        "atl-rename.sh",
        "sync-atl.sh",
        "dev.sh",
        "atl-trim.sh",
        "atl-test.sh",
    ] {
        let status = Command::new("chmod")
            .arg("+x")
            .arg(fork.join("scripts").join(script))
            .status()
            .unwrap();
        assert!(status.success());
    }
    put(&fork, "vendor/pin", &format!("v1.0.0 {one}\n"));
    release_one(&fork.join(LOW));
    let renamed = Command::new("bash")
        .arg(fork.join("scripts/atl-rename.sh"))
        .arg("--root")
        .arg(&fork)
        .arg(LOW)
        .output()
        .unwrap();
    assert!(renamed.status.success());
    let main = get(&fork, "atl/src/main.rs").replacen(from, to, 1);
    put(&fork, "atl/src/main.rs", &main);
    git(&fork, &["add", "-A"]);
    git(&fork, &["commit", "-q", "-m", "fork at release one"]);

    Fixture {
        _dir: dir,
        upstream,
        fork,
    }
}

fn sync(fx: &Fixture, tag: &str) -> Output {
    Command::new("bash")
        .arg(fx.fork.join("scripts/sync-atl.sh"))
        .arg(tag)
        .current_dir(&fx.fork)
        .env("ATL_SYNC_UPSTREAM", &fx.upstream)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .expect("bash ran")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn a_new_release_lands_renamed_and_the_forks_own_change_stays() {
    // The fork changed section one; upstream changed the top and added to the end.
    let fx = fixture("fn one() {}", "fn one() { /* the fork's change */ }");
    let out = sync(&fx, "v1.0.1");
    assert!(out.status.success(), "{}", text(&out));

    let main = get(&fx.fork, "atl/src/main.rs");
    assert!(
        main.contains("atl 1.1"),
        "upstream's change, renamed: {main}"
    );
    assert!(main.contains("fn atl_extra() {}"), "{main}");
    assert!(
        main.contains("the fork's change"),
        "the fork's change stays: {main}"
    );
    assert_eq!(get(&fx.fork, "atl/src/atl_new.rs"), "pub fn atl_new() {}\n");
    assert_eq!(
        get(&fx.fork, "atl/docs/atl-notes.md"),
        "ATL notes, second edition\n"
    );
    assert_eq!(
        get(&fx.fork, "atl/LICENSE"),
        LICENSE,
        "the license text is never edited"
    );
    assert!(!fx.fork.join(LOW).exists(), "no folder with the old name");
    for file in git_text(&fx.fork, &["ls-files", "atl"]).lines() {
        assert!(!file.to_lowercase().contains(LOW), "{file}");
    }

    assert_eq!(
        git_text(&fx.fork, &["branch", "--show-current"]),
        "atl-sync/v1.0.1"
    );
    let parents = git_text(&fx.fork, &["log", "-1", "--format=%P"]);
    assert_eq!(
        parents.split(' ').count(),
        2,
        "the last commit is a merge: {parents}"
    );
    let subject = git_text(&fx.fork, &["log", "-1", "--format=%s"]);
    assert!(subject.contains("v1.0.1"), "{subject}");
    assert!(!git_text(&fx.fork, &["log", "-1", "--format=%B"]).contains("Co-Authored-By"));

    // The pin is recorded and left uncommitted for review.
    let pin = get(&fx.fork, "vendor/pin");
    assert!(pin.starts_with("v1.0.1 "), "{pin}");
    assert!(git_text(&fx.fork, &["status", "--porcelain"]).contains("vendor/pin"));
    let summary = text(&out);
    assert!(summary.contains("conflicts: none"), "{summary}");
    assert!(
        summary.contains("trim:     ran scripts/atl-trim.sh"),
        "{summary}"
    );
    assert!(summary.contains("tests:    PASS"), "{summary}");
}

#[test]
fn a_clash_with_the_forks_change_stops_for_review() {
    // The fork changed a line upstream changed too.
    let fx = fixture("println!(\"atl 1.0\");", "println!(\"atl fork\");");
    let out = sync(&fx, "v1.0.1");
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let said = text(&out);
    assert!(said.contains("stopped on conflicts in:"), "{said}");
    assert!(said.contains("atl/src/main.rs"), "{said}");
    assert!(get(&fx.fork, "atl/src/main.rs").contains("<<<<<<<"));
    assert_eq!(
        git_text(&fx.fork, &["branch", "--show-current"]),
        "atl-sync/v1.0.1"
    );
    assert!(git_text(&fx.fork, &["status", "--porcelain"]).contains("UU atl/src/main.rs"));
}

#[test]
fn it_refuses_a_dirty_tree_and_an_existing_branch() {
    let fx = fixture("fn one() {}", "fn one() { /* x */ }");
    put(&fx.fork, "stray.txt", "x\n");
    let out = sync(&fx, "v1.0.1");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out).contains("the working tree is not clean"),
        "{}",
        text(&out)
    );

    fs::remove_file(fx.fork.join("stray.txt")).unwrap();
    git(&fx.fork, &["branch", "atl-sync/v1.0.1"]);
    let out = sync(&fx, "v1.0.1");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).contains("already exists"), "{}", text(&out));
}

#[test]
fn it_never_pushes() {
    let script = repo::read("scripts/sync-atl.sh");
    for line in script.lines().filter(|l| !l.trim_start().starts_with('#')) {
        assert!(!line.contains("git push"), "{line}");
    }
}
