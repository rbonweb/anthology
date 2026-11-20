//! The replay corpus is local only (design §13.7): no tracked path is a copy of it.

use std::path::Path;
use std::process::Command;

/// Whether a tracked path looks like a copy of the replay corpus.
fn is_corpus(path: &str) -> bool {
    path.starts_with("port/replay/")
        || path
            .split('/')
            .any(|part| part.starts_with("replay-corpus"))
}

#[test]
fn no_tracked_path_is_a_replay_corpus() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = match Command::new("git")
        .args(["-c", "core.fsmonitor=false", "ls-files", "-z"])
        .current_dir(&root)
        .output()
    {
        Ok(out) if out.status.success() => out,
        // No git, or a linked worktree whose git directory is outside the container.
        _ => {
            eprintln!("skipped: git ls-files is unavailable here");
            return;
        }
    };
    let listing = String::from_utf8_lossy(&out.stdout);
    let paths: Vec<&str> = listing.split('\0').filter(|p| !p.is_empty()).collect();
    assert!(paths.contains(&"Cargo.toml"), "not the repository root");
    let hits: Vec<&&str> = paths.iter().filter(|p| is_corpus(p)).collect();
    assert!(hits.is_empty(), "replay corpus files are tracked: {hits:?}");
}

#[test]
fn the_corpus_shapes_are_recognized() {
    assert!(is_corpus("port/replay/calls.jsonl"));
    assert!(is_corpus("replay-corpus.jsonl"));
    assert!(is_corpus("tmp/replay-corpus-2026/x"));
    assert!(!is_corpus("port/replay-reports/2026-12-01.md"));
    assert!(!is_corpus("crates/anthology-dev/src/replay/collect.rs"));
}
