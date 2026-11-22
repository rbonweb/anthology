//! Port of the compat rows of `tests/test_doctor.py` (RT1a; the `CompatBase` subclasses whose
//! subject is `compat.py`: `PathWrappersTest`, `AgentMemoryPathTest`, `SubagentModelTest`).
//!
//! The Python tests go through `doctor.check_env`; doctor is RT1b's. These drive the compat
//! functions it calls with the same fixtures and assert what decides each check: the
//! `path_wrappers` check fails exactly when `wrapper_problems` names something, the
//! `agent_memory_path` check is soft (None) with bad folders, ok with good ones and None with
//! neither, and `subagent_model` fails exactly when `subagent_model_source` names a place,
//! whose label is the check's `(%s)`. The doctor-only branches and wording are RT1b's rows.

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_runtime::compat::{
    self, REPO_READ_LIMIT, agent_memory_hint, agent_memory_problems, read_json_limited,
    repo_top_with, subagent_model_source, subagent_model_sources, subagent_model_sources_with,
    wrapper_problems,
};
use anthology_testkit::rt_a::{RealTemp, write};

#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;
#[cfg(target_os = "macos")]
const O_NONBLOCK: i32 = 0x0004;
const MODEL: &str = r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#;

/// `EnvCase`: an isolated HOME, config folder and PATH, and a working folder `proj`.
struct Case {
    _tmp: RealTemp,
    tmp: PathBuf,
    config: PathBuf,
    cwd: PathBuf,
}

impl Case {
    fn new() -> Case {
        let t = RealTemp::new();
        let tmp = t.path.clone();
        let config = tmp.join("config");
        fs::create_dir(&config).unwrap();
        let cwd = tmp.join("proj");
        fs::create_dir(&cwd).unwrap();
        Case {
            _tmp: t,
            tmp,
            config,
            cwd,
        }
    }

    fn env(&self) -> Env {
        Env::default()
            .with_os("HOME", &self.tmp)
            .with_os("CLAUDE_CONFIG_DIR", &self.config)
            .with("PATH", "/bin")
    }

    fn source(&self) -> Option<String> {
        subagent_model_source(&self.env(), &self.cwd)
    }

    /// `PathWrappersTest.fake_plugin`, with F-9's `bin/anthology` too.
    fn fake_plugin(&self) -> PathBuf {
        let root = self.tmp.join("plug");
        for name in ["anthology", "anthology-mem", "rtk", "rtk-hook"] {
            let path = write(&root.join("bin").join(name), "#!/bin/sh\n");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        root
    }

    /// `AgentMemoryPathTest.setUp`: a repo at `proj`; returns its agent-memory folder.
    fn repo_memory(&self) -> PathBuf {
        fs::create_dir(self.cwd.join(".git")).unwrap();
        self.cwd.join(".claude/agent-memory")
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

fn mkfifo(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let ok = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap()
        .success();
    assert!(ok, "mkfifo");
}

// --- PathWrappersTest

#[test]
fn test_all_present_and_executable_outside_claude_code() {
    let c = Case::new();
    let root = c.fake_plugin();
    assert_eq!(wrapper_problems(&root), Vec::<String>::new());
}

#[test]
fn test_missing_and_non_executable_are_named() {
    let c = Case::new();
    let root = c.fake_plugin();
    fs::remove_file(root.join("bin/rtk")).unwrap();
    fs::set_permissions(root.join("bin/rtk-hook"), fs::Permissions::from_mode(0o644)).unwrap();
    let bad = wrapper_problems(&root);
    assert_eq!(bad, names(&["rtk", "rtk-hook"]));
    assert!(!bad.iter().any(|n| n == "anthology-mem"));
}

// --- AgentMemoryPathTest

#[test]
fn test_none_is_none() {
    let c = Case::new();
    let mem = c.repo_memory();
    assert_eq!(agent_memory_problems(&c.cwd), (vec![], vec![]));
    fs::create_dir_all(&mem).unwrap();
    assert_eq!(agent_memory_problems(&c.cwd), (vec![], vec![]));
}

#[test]
fn test_expected_folders_are_ok() {
    let c = Case::new();
    let mem = c.repo_memory();
    fs::create_dir_all(mem.join("anthology-implementer")).unwrap();
    assert_eq!(
        agent_memory_problems(&c.cwd),
        (vec![], names(&["anthology-implementer"]))
    );
}

#[test]
fn test_bare_agent_name_alone_is_a_soft_warning_not_a_failure() {
    let c = Case::new();
    let mem = c.repo_memory();
    fs::create_dir_all(mem.join("implementer")).unwrap();
    let (bad, good) = agent_memory_problems(&c.cwd);
    assert_eq!((bad.clone(), good), (names(&["implementer"]), vec![]));
    let hint = agent_memory_hint(&bad[0]);
    assert!(hint.contains("agent-memory/implementer/"), "{hint}");
    assert!(hint.contains("no `anthology-implementer/`"), "{hint}");
    assert!(hint.contains("agent of your own"), "{hint}");
}

#[test]
fn test_a_user_agent_next_to_anthology_folders_is_fine() {
    let c = Case::new();
    let mem = c.repo_memory();
    fs::create_dir_all(mem.join("reviewer")).unwrap();
    fs::create_dir(mem.join("anthology-implementer")).unwrap();
    let (bad, good) = agent_memory_problems(&c.cwd);
    assert!(bad.is_empty());
    assert_eq!(good, names(&["anthology-implementer"]));
}

#[test]
fn test_colon_and_underscore_forms_alone_are_soft() {
    let c = Case::new();
    let mem = c.repo_memory();
    for name in ["anthology:implementer", "anthology_debugger"] {
        fs::create_dir_all(mem.join(name)).unwrap();
        assert_eq!(
            agent_memory_problems(&c.cwd),
            (names(&[name]), vec![]),
            "{name}"
        );
        fs::remove_dir(mem.join(name)).unwrap();
    }
}

#[test]
fn test_repo_top_survives_an_unreadable_parent() {
    let c = Case::new();
    c.repo_memory();
    let denied = |_: &Path| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    assert_eq!(repo_top_with(&c.cwd, &denied), None);
    assert_eq!(compat::repo_top(&c.cwd), Some(c.cwd.clone()));
}

#[test]
fn test_found_from_a_subfolder_and_unrelated_folders_ignored() {
    let c = Case::new();
    let mem = c.repo_memory();
    let sub = c.cwd.join("pkg/x");
    fs::create_dir_all(&sub).unwrap();
    fs::create_dir_all(mem.join("notes")).unwrap();
    fs::create_dir(mem.join("anthology-implementer")).unwrap();
    assert_eq!(
        agent_memory_problems(&sub),
        (vec![], names(&["anthology-implementer"]))
    );
}

// --- SubagentModelTest

#[test]
fn test_unset_is_ok() {
    assert_eq!(Case::new().source(), None);
}

#[test]
fn test_set_in_process_env() {
    let c = Case::new();
    let env = c.env().with("CLAUDE_CODE_SUBAGENT_MODEL", "SECRETMODEL");
    let label = subagent_model_source(&env, &c.cwd).unwrap();
    assert!(label.contains("process environment"), "{label}");
    assert!(!label.contains("SECRETMODEL"));
}

#[test]
fn test_empty_env_value_is_unset() {
    let c = Case::new();
    let env = c.env().with("CLAUDE_CODE_SUBAGENT_MODEL", "");
    assert_eq!(subagent_model_source(&env, &c.cwd), None);
}

#[test]
fn test_set_only_in_user_settings() {
    let c = Case::new();
    write(
        &c.config.join("settings.json"),
        r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "SECRETMODEL"}}"#,
    );
    let label = c.source().unwrap();
    assert!(label.contains("settings.json"), "{label}");
    assert!(!label.contains("SECRETMODEL"));
}

#[test]
fn test_process_env_copied_from_a_settings_file_names_only_the_file() {
    // Claude Code copies a settings file's env into the commands it runs
    let c = Case::new();
    write(&c.config.join("settings.json"), MODEL);
    let env = c.env().with("CLAUDE_CODE_SUBAGENT_MODEL", "m");
    assert_eq!(subagent_model_sources(&env, &c.cwd), ["user settings.json"]);
    let env = c.env().with("CLAUDE_CODE_SUBAGENT_MODEL", "x");
    assert_eq!(
        subagent_model_sources(&env, &c.cwd),
        ["user settings.json", "the process environment"]
    );
}

#[test]
fn test_every_place_is_listed_in_order() {
    let c = Case::new();
    let managed = c.tmp.join("managed-settings.json");
    for path in [
        c.config.join("settings.json"),
        c.cwd.join(".claude/settings.local.json"),
        managed.clone(),
    ] {
        write(&path, MODEL);
    }
    let found = subagent_model_sources_with(&c.env(), &c.cwd, std::slice::from_ref(&managed));
    assert_eq!(
        found,
        [
            "user settings.json",
            "the project's .claude/settings.local.json",
            "managed settings"
        ]
    );
    // doctor's `(%s)` is every place, joined
    assert_eq!(
        found.join(", "),
        "user settings.json, the project's .claude/settings.local.json, managed settings"
    );
    for label in found {
        assert!(
            compat::subagent_model_hint(label).contains("CLAUDE_CODE_SUBAGENT_MODEL"),
            "{label}"
        );
    }
}

#[test]
fn test_set_in_managed_settings() {
    let c = Case::new();
    let managed = c.tmp.join("managed-settings.json");
    write(
        &managed,
        r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "SECRETMODEL"}}"#,
    );
    let found = subagent_model_sources_with(&c.env(), &c.cwd, &[managed]);
    assert_eq!(found, ["managed settings"]);
    assert!(!found.join(", ").contains("SECRETMODEL"));
}

#[test]
fn test_run_in_the_home_folder_the_user_file_is_listed_once() {
    let c = Case::new();
    let env = c.env().with_os("CLAUDE_CONFIG_DIR", c.cwd.join(".claude"));
    write(&c.cwd.join(".claude/settings.json"), MODEL);
    assert_eq!(subagent_model_sources(&env, &c.cwd), ["user settings.json"]);
}

#[test]
fn test_set_in_project_settings_and_local() {
    let c = Case::new();
    write(&c.cwd.join(".claude/settings.json"), MODEL);
    assert!(c.source().is_some());
    fs::remove_file(c.cwd.join(".claude/settings.json")).unwrap();
    write(&c.cwd.join(".claude/settings.local.json"), MODEL);
    let label = c.source().unwrap();
    assert!(label.contains("settings.local.json"), "{label}");
}

#[test]
fn test_unrelated_settings_and_garbage_are_ok() {
    let c = Case::new();
    write(
        &c.config.join("settings.json"),
        r#"{"env": {"OTHER": "1"}}"#,
    );
    write(&c.cwd.join(".claude/settings.json"), "{nope");
    write(
        &c.cwd.join(".claude/settings.local.json"),
        r#"{"env": "bad"}"#,
    );
    assert_eq!(c.source(), None);
}

fn model_with_pad(pad: usize) -> String {
    format!(
        r#"{{"env": {{"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}, "pad": "{}"}}"#,
        "A".repeat(pad)
    )
}

#[test]
fn test_oversized_repo_settings_are_skipped_like_an_absent_file() {
    let c = Case::new();
    let pad = REPO_READ_LIMIT as usize + 10;
    for name in ["settings.json", "settings.local.json"] {
        write(&c.cwd.join(".claude").join(name), model_with_pad(pad));
    }
    assert_eq!(c.source(), None);
}

#[test]
fn test_repo_settings_just_under_the_cap_are_read() {
    let c = Case::new();
    let text = model_with_pad(0);
    let pad = REPO_READ_LIMIT as usize - text.len();
    let path = write(&c.cwd.join(".claude/settings.json"), model_with_pad(pad));
    assert_eq!(fs::metadata(&path).unwrap().len(), REPO_READ_LIMIT);
    assert!(c.source().is_some());
}

#[test]
fn test_the_users_own_settings_are_not_capped() {
    let c = Case::new();
    write(
        &c.config.join("settings.json"),
        model_with_pad(REPO_READ_LIMIT as usize + 10),
    );
    assert!(c.source().is_some());
}

#[test]
fn test_the_repo_limit_is_claude_codes_own_two_mib() {
    assert_eq!(REPO_READ_LIMIT, 2 * 1024 * 1024);
}

#[test]
fn test_a_fifo_with_valid_json_is_refused_though_it_reads_fine() {
    // only the regular-file check can refuse this: the data is small and valid
    let c = Case::new();
    let fifo = c.tmp.join("settings.json");
    mkfifo(&fifo);
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(&fifo)
        .unwrap();
    std::io::Write::write_all(&mut f, MODEL.as_bytes()).unwrap();
    assert_eq!(read_json_limited(&fifo, REPO_READ_LIMIT), None);
}

#[test]
fn test_settings_are_decoded_like_claude_code() {
    let c = Case::new();
    let flag = r#"{"env": {"CLAUDE_CODE_SUBAGENT_MODEL": "m"}}"#;
    let path = c.cwd.join(".claude/settings.json");
    fs::create_dir(c.cwd.join(".claude")).unwrap();
    let utf16 = |le: bool| {
        let mut out: Vec<u8> = if le {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for u in flag.encode_utf16() {
            out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        out
    };
    let truncated = &flag[..flag.len() - 2];
    for raw in [
        utf16(true),
        utf16(false),
        [b"\xef\xbb\xbf".as_slice(), flag.as_bytes()].concat(),
        [truncated.as_bytes(), b", \"x\": \"a\xff\"}}".as_slice()].concat(),
    ] {
        fs::write(&path, &raw).unwrap();
        assert!(c.source().is_some(), "{:?}", &raw[..6]);
    }
}

#[test]
fn test_a_symlink_to_a_regular_file_is_still_read() {
    let c = Case::new();
    let real = write(&c.tmp.join("real.json"), MODEL);
    fs::create_dir(c.cwd.join(".claude")).unwrap();
    symlink(&real, c.cwd.join(".claude/settings.json")).unwrap();
    assert!(c.source().is_some());
}

/// The compat half of `test_a_device_symlink_is_skipped_without_reading_it` (the
/// `doctor.find_mcp_servers` half is RT1b's row).
#[test]
fn a_device_symlink_settings_file_is_skipped_without_reading_it() {
    let c = Case::new();
    fs::create_dir(c.cwd.join(".claude")).unwrap();
    symlink("/dev/zero", c.cwd.join(".claude/settings.json")).unwrap();
    let start = Instant::now();
    assert_eq!(c.source(), None);
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn test_a_fifo_is_skipped_without_blocking() {
    let c = Case::new();
    mkfifo(&c.cwd.join(".claude/settings.json"));
    let (env, cwd) = (c.env(), c.cwd.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(subagent_model_source(&env, &cwd));
    });
    let got = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("subagent_model_source blocked on a FIFO");
    assert_eq!(got, None);
}

#[test]
fn test_the_owners_settings_are_never_written() {
    let c = Case::new();
    let path = write(&c.config.join("settings.json"), MODEL);
    let before = fs::read(&path).unwrap();
    assert!(c.source().is_some());
    assert_eq!(fs::read(&path).unwrap(), before);
}

// --- Beyond the Python tests (RT1a review). Not ledger targets.

#[test]
fn a_deep_user_file_is_malformed_past_pythons_limit() {
    let c = Case::new();
    let deep = |n: usize| format!(r#"{{"a": {}{}}}"#, "[".repeat(n), "]".repeat(n));
    let path = write(&c.tmp.join("deep.json"), deep(9_996));
    assert!(compat::read_json(&path).is_some());
    write(&path, deep(9_997));
    assert_eq!(compat::read_json(&path), None);
    // a repo-controlled read stops far earlier (REPO_JSON_DEPTH)
    write(&path, deep(600));
    assert_eq!(read_json_limited(&path, REPO_READ_LIMIT), None);
    assert!(compat::read_json(&path).is_some());
}
