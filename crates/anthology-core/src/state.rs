//! `state.json`, byte for byte as `plugins/anthology/scripts/state.py` writes it (plan §3.3,
//! GC-3).
//!
//! - [`read`]: a missing or unreadable file, invalid UTF-8 (a BOM included), invalid JSON or a
//!   non-object reads as an empty object.
//! - [`write`]: `mkdir -p` the folder, read, merge the pairs in order (`dict.update`), then
//!   `json.dumps(indent=2, sort_keys=True, ensure_ascii=False) + "\n"` into a `mkstemp` file
//!   (`.state.XXXXXXXX.tmp`, mode 0600) renamed onto `state.json`; the temp file is unlinked
//!   on failure. No fsync, no lock. Python's `ensure_ascii=True` fallback only ever applied to
//!   lone surrogates, which Rust never holds (they are U+FFFD, D-6).
//! - Each key is on its own line, so `"atl_path"` is `  "atl_path": "<path>",` (or without the
//!   comma) for the `sed` in `bin/atl`.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use anthology_pycompat::json::{self, Map, Value};
use anthology_pycompat::pyerr::PyExc;

use crate::{Env, fsx, paths};

/// `<config>/anthology/state.json`.
pub fn path(env: &Env) -> PathBuf {
    paths::anthology_home(env).join("state.json")
}

/// atl's history, recall store and tee spool: `<state.json's folder>/atl`
/// (`state.atl_data_dir()`). `bin/atl` and `bin/atl-hook` point atl there, and
/// `anthology setup remove` deletes it.
pub fn atl_data_dir(env: &Env) -> PathBuf {
    paths::anthology_home(env).join("atl")
}

/// The history folder of the legacy rtk install: `<state.json's folder>/rtk`. Nothing writes
/// there any more; setup deletes it, and nothing else ever touches it.
pub fn legacy_rtk_data_dir(env: &Env) -> PathBuf {
    paths::anthology_home(env).join("rtk") // legacy folder name
}

/// The state, or an empty object when it cannot be read (`state.read()`).
pub fn read(env: &Env) -> Map {
    let Ok(bytes) = std::fs::read(path(env)) else {
        return Map::new();
    };
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Map::new();
    };
    json::loads(text)
        .ok()
        .and_then(Value::into_object)
        .unwrap_or_default()
}

/// Merges `kv` into the state and replaces the file (`state.write(**kv)`).
pub fn write(env: &Env, kv: Vec<(String, Value)>) -> Result<(), PyExc> {
    let path = path(env);
    if let Some(dir) = path.parent() {
        fsx::mkdir_parents(dir)?;
    }
    let mut data = read(env);
    for (k, v) in kv {
        data.insert(k, v);
    }
    let mut text = json::dumps(&Value::Object(data), &json::STATE);
    text.push('\n');
    fsx::replace_via_temp(&path, text.as_bytes(), ".state.", ".tmp", None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthology_pycompat::json::Int;
    use std::os::unix::fs::MetadataExt;

    fn sandbox() -> (tempfile::TempDir, Env) {
        let d = tempfile::tempdir().expect("tempdir");
        let env = Env::default()
            .with("HOME", "/nonexistent")
            .with_os("CLAUDE_CONFIG_DIR", d.path().join("cfg"));
        (d, env)
    }

    fn s(v: &str) -> Value {
        Value::Str(v.into())
    }

    #[test]
    fn write_creates_merges_and_keeps_atl_path_on_its_own_line() {
        let (_d, env) = sandbox();
        assert!(read(&env).is_empty());
        write(&env, vec![("atl_path".into(), s("/p/atl"))]).expect("write");
        write(
            &env,
            vec![
                ("data_dir".into(), s("/d")),
                ("settings_created".into(), Value::Bool(true)),
                ("n".into(), Value::Int(Int::Small(1))),
            ],
        )
        .expect("write");
        let text = std::fs::read_to_string(path(&env)).expect("read");
        assert_eq!(
            text,
            "{\n  \"atl_path\": \"/p/atl\",\n  \"data_dir\": \"/d\",\n  \"n\": 1,\n  \"settings_created\": true\n}\n"
        );
        assert!(text.lines().any(|l| l == "  \"atl_path\": \"/p/atl\","));
        let meta = std::fs::metadata(path(&env)).expect("stat");
        assert_eq!(meta.mode() & 0o777, 0o600);
        let names: Vec<_> = std::fs::read_dir(path(&env).parent().expect("dir"))
            .expect("ls")
            .map(|e| e.expect("e").file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
        assert_eq!(
            read(&env).get("data_dir").and_then(Value::as_str),
            Some("/d")
        );
    }

    #[test]
    fn the_atl_and_legacy_rtk_data_dirs_sit_beside_state_json() {
        let (_d, env) = sandbox();
        let home = path(&env).parent().expect("dir").to_path_buf();
        assert_eq!(atl_data_dir(&env), home.join("atl"));
        assert_eq!(legacy_rtk_data_dir(&env), home.join("rtk"));
    }

    #[test]
    fn unreadable_states_read_as_empty() {
        let (_d, env) = sandbox();
        std::fs::create_dir_all(path(&env).parent().expect("dir")).expect("mkdir");
        for bad in [
            &b"\xef\xbb\xbf{\"a\": 1}"[..],
            b"{\"a\": \"\xff\"}",
            b"[1]",
            b"\"s\"",
            b"null",
            b"{",
            b"",
        ] {
            std::fs::write(path(&env), bad).expect("write");
            assert!(read(&env).is_empty(), "{bad:?}");
        }
        std::fs::remove_file(path(&env)).expect("rm");
        std::fs::create_dir(path(&env)).expect("a folder named state.json");
        assert!(read(&env).is_empty());
    }

    #[test]
    fn a_failed_write_removes_the_temp_file_and_keeps_the_old_state() {
        let (_d, env) = sandbox();
        write(&env, vec![("a".into(), s("1"))]).expect("write");
        let before = std::fs::read(path(&env)).expect("read");
        crate::fsx::fault::fail_next("rename", libc::EXDEV);
        let e = write(&env, vec![("a".into(), s("2"))]).expect_err("rename");
        assert!(matches!(e, PyExc::OSError { errno, .. } if errno == libc::EXDEV));
        assert_eq!(std::fs::read(path(&env)).expect("read"), before);
        let n = std::fs::read_dir(path(&env).parent().expect("dir"))
            .expect("ls")
            .count();
        assert_eq!(n, 1);
        crate::fsx::fault::fail_next("mkdir", libc::EACCES);
        let (_d2, env2) = sandbox();
        let e = write(&env2, vec![]).expect_err("mkdir");
        assert_eq!(e.type_name(), "PermissionError");
    }

    #[test]
    fn a_folder_in_the_way_is_an_is_a_directory_error() {
        let (_d, env) = sandbox();
        std::fs::create_dir_all(path(&env)).expect("mkdir");
        let e = write(&env, vec![("a".into(), s("1"))]).expect_err("rename onto a folder");
        assert_eq!(e.type_name(), "IsADirectoryError");
        let n = std::fs::read_dir(path(&env).parent().expect("dir"))
            .expect("ls")
            .count();
        assert_eq!(n, 1, "only the folder is left");
    }
}
