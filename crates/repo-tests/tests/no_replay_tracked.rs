//! The replay corpus, its HMAC key and the runner-4 list are local only (design §13.7, m2-8):
//! no tracked path is a copy of them.

use repo_tests::repo;

/// Whether a tracked path looks like a local-only replay file.
fn is_local_only(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.starts_with("port/replay/")
        || path
            .split('/')
            .any(|part| part.starts_with("replay-corpus"))
        || name == "hmac.key"
        || (name.starts_with("runner4-") && name.ends_with(".tsv"))
}

#[test]
fn no_tracked_path_is_a_local_replay_file() {
    let Some(paths) = repo::tracked_files() else {
        eprintln!("skipped: git ls-files is unavailable here");
        return;
    };
    assert!(
        paths.iter().any(|p| p == "Cargo.toml"),
        "not the repository root"
    );
    let hits: Vec<&String> = paths.iter().filter(|p| is_local_only(p)).collect();
    assert!(
        hits.is_empty(),
        "local-only replay files are tracked: {hits:?}"
    );
}

#[test]
fn the_local_only_shapes_are_recognized() {
    for hit in [
        "port/replay/calls.jsonl",
        "replay-corpus.jsonl",
        "tmp/replay-corpus-2026/x",
        "hmac.key",
        "port/replay-reports/hmac.key",
        "runner4-2026-12-01.tsv",
        "notes/runner4-x.tsv",
    ] {
        assert!(is_local_only(hit), "{hit}");
    }
    for miss in [
        "port/replay-reports/2026-12-01.md",
        "crates/anthology-dev/src/replay/collect.rs",
        "crates/anthology-dev/src/replay/hmac.rs",
        "hmac.key.md",
        "runner4-2026.txt",
    ] {
        assert!(!is_local_only(miss), "{miss}");
    }
}
