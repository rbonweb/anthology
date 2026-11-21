//! `vendor set-rtk-upstream` (no Python counterpart: it replaces the inline Python in
//! `scripts/sync-rtk.sh`). The old pin on stdout, `"<ref> <commit>\n"`, is DV2's contract.

use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const SOURCES: &str = r#"{"sources": [{"name": "a", "repo": "https://github.com/acme/a"},
 {"name": "rtk", "managed_by": "subtree", "license": "Apache-2.0", "target": "rtk",
  "upstream": {"repo": "https://github.com/rtk-ai/rtk", "ref": "v1.0.0", "commit": "c1", "x": 1.50},
  "note": "für ☃"}]}
"#;

/// `json.dumps(json.loads(SOURCES) with the pin changed, indent=2, ensure_ascii=False) + "\n"`.
const WANT: &str = "{
  \"sources\": [
    {
      \"name\": \"a\",
      \"repo\": \"https://github.com/acme/a\"
    },
    {
      \"name\": \"rtk\",
      \"managed_by\": \"subtree\",
      \"license\": \"Apache-2.0\",
      \"target\": \"rtk\",
      \"upstream\": {
        \"repo\": \"https://github.com/rtk-ai/rtk\",
        \"ref\": \"v2.0.0\",
        \"commit\": \"c2\",
        \"x\": 1.5
      },
      \"note\": \"f\u{fc}r \u{2603}\"
    }
  ]
}
";

fn vendor(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_anthology-dev"))
        .arg("vendor")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn repo(text: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("vendor")).unwrap();
    fs::write(dir.path().join("vendor/sources.json"), text).unwrap();
    dir
}

#[test]
fn set_rtk_upstream_prints_the_old_pin_and_rewrites_the_file() {
    let dir = repo(SOURCES);
    let out = vendor(
        dir.path(),
        &["set-rtk-upstream", "--ref", "v2.0.0", "--commit", "c2"],
    );
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "v1.0.0 c1\n");
    assert!(out.stderr.is_empty());
    assert_eq!(
        fs::read_to_string(dir.path().join("vendor/sources.json")).unwrap(),
        WANT
    );
}

#[test]
fn set_rtk_upstream_without_an_rtk_entry_is_an_error() {
    let dir = repo(r#"{"sources": [{"name": "a"}]}"#);
    let out = vendor(
        dir.path(),
        &["set-rtk-upstream", "--ref", "v", "--commit", "c"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "vendor.py: vendor/sources.json has no rtk entry\n"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn set_rtk_upstream_needs_both_values() {
    let dir = repo(SOURCES);
    let out = vendor(dir.path(), &["set-rtk-upstream", "--ref", "v"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "usage: vendor.py set-rtk-upstream [-h] --ref REF --commit COMMIT\n\
         vendor.py set-rtk-upstream: error: the following arguments are required: --commit\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("vendor/sources.json")).unwrap(),
        SOURCES
    );
}
