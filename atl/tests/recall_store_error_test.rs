//! `atl recall` reports why the store cannot be used, with the cause of a
//! failed migration step.

mod common;

use rusqlite::Connection;
use std::path::Path;
use std::process::Output;

fn atl_on(store: &Path, args: &[&str]) -> Output {
    common::atl_command()
        .args(args)
        .env("ATL_RECALL_DB", store)
        .output()
        .expect("atl should run")
}

#[test]
fn recall_names_the_cause_of_a_failed_migration_step() {
    let dir = common::tempdir();
    let store = dir.path().join("recall.db");
    // A recall store stamped at its first version whose `recall` cannot take
    // the column the second step adds.
    common::recall_store(&store, 1, "CREATE VIEW recall AS SELECT 'abc' AS hash;");

    for args in [&["recall", "--list"], &["recall", "abc"]] {
        let output = atl_on(&store, args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("recall database was not upgraded: "),
            "{args:?}: stderr should carry the cause: {stderr}"
        );
        assert!(
            !stderr.contains("Run `atl init`"),
            "{args:?}: `atl init` fails the same step: {stderr}"
        );
    }
}

#[test]
fn recall_says_the_store_is_unavailable_only_when_it_is() {
    let dir = common::tempdir();
    let store = dir.path().join("recall.db");
    let list = || atl_on(&store, &["recall", "--list"]);

    // Another application's file cannot be used at all.
    Connection::open(&store)
        .expect("open should succeed")
        .pragma_update(None, "application_id", 0x1234_5678)
        .expect("the file should be tagged");
    let stderr = String::from_utf8_lossy(&list().stderr).into_owned();
    assert!(stderr.contains("store unavailable"), "{stderr}");
    std::fs::remove_file(&store).expect("the file should go");

    // A failure on the entries of a store that is there is not that: a store stamped
    // at its last version, with no step left to add the columns `--list` needs.
    common::recall_store(&store, 2, "CREATE TABLE recall (hash TEXT PRIMARY KEY);");
    let output = list();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("no such column"), "{stderr}");
    assert!(!stderr.contains("store unavailable"), "{stderr}");
    assert!(
        !stderr.contains("Run `atl init`"),
        "`atl init` does not bring back a column of a table it finds: {stderr}"
    );
}

/// A store at its last version with a table gone, as a partial restore leaves it.
fn store_without_its_table(store: &Path) {
    common::recall_store(store, 2, "CREATE TABLE tee_reads (path TEXT PRIMARY KEY);");
}

const REPAIR_HINT: &str = "Run `atl init` to recreate it.";

#[test]
fn a_store_that_lost_a_table_points_at_atl_init_which_repairs_it() {
    let dir = common::tempdir();
    let store = dir.path().join("recall.db");
    store_without_its_table(&store);
    let list = || atl_on(&store, &["recall", "--list"]);

    let output = list();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("atl: recall database looks corrupted (no such table: recall)"),
        "{stderr}"
    );
    assert!(stderr.contains(REPAIR_HINT), "{stderr}");

    let init = atl_on(&store, &["init", "-g", "--auto-patch"]);
    let init_stderr = String::from_utf8_lossy(&init.stderr);
    assert!(init.status.success(), "{init_stderr}");

    let output = list();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains(REPAIR_HINT), "{stderr}");
}

/// Saving output is where the loss is otherwise silent: the recovery hint just stops
/// appearing.
#[cfg(unix)]
#[test]
fn a_command_that_cannot_save_its_output_points_at_atl_init() {
    let dir = common::tempdir();
    let store = dir.path().join("recall.db");
    store_without_its_table(&store);

    let output = common::atl_command()
        .args(["err", "sh", "-c", "seq 1 400; echo error: x; exit 1"])
        .current_dir(dir.path())
        .env("ATL_RECALL_DB", &store)
        .env("ATL_DB_PATH", dir.path().join("history.db"))
        .output()
        .expect("atl should run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches(REPAIR_HINT).count(), 1, "{stderr}");
}

#[test]
fn the_recall_report_points_at_atl_init_too() {
    let dir = common::tempdir();
    let store = dir.path().join("recall.db");
    store_without_its_table(&store);

    let output = common::atl_command()
        .args(["gain", "--recalls"])
        .env("ATL_RECALL_DB", &store)
        .env("ATL_DB_PATH", dir.path().join("history.db"))
        .output()
        .expect("atl should run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(REPAIR_HINT), "{stderr}");
}

/// `atl init` leaves the store alone while recall is disabled.
#[test]
fn a_disabled_store_does_not_point_at_atl_init() {
    let dir = common::tempdir();
    let store = dir.path().join("recall.db");
    store_without_its_table(&store);

    let output = common::atl_command()
        .args(["recall", "--list"])
        .env("ATL_RECALL_DB", &store)
        .env("ATL_RECALL", "0")
        .output()
        .expect("atl should run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(REPAIR_HINT), "{stderr}");
}
