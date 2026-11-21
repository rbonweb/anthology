//! Port of `tests/test_rtk_install.py` (RT2): rtk_install (target mapping, lock validation,
//! verified per-version install, reconcile, prune, default fetch), hooks.json, and the POSIX sh
//! wrappers `bin/rtk` and `bin/rtk-hook`. Modules are the Python classes; every test keeps the
//! Python method name. No network: every fetch is a fake (the real ureq path is
//! `tests/fetch_tls.rs`). The `CliTest` ports live with the command
//! (`crates/anthology-cli/src/cmd/rtk_install.rs` and `crates/anthology-cli/tests/rtk_install.rs`).
#![forbid(unsafe_code)]

use std::cell::Cell;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anthology_core::lock::Lock;
use anthology_core::platform::Target;
use anthology_core::{Env, state};
use anthology_pycompat::json::{self, Value};
use anthology_pycompat::pyerr::PyExc;
use anthology_runtime::fetch::{self, Fetch, Opener, Response};
use anthology_runtime::rtk_install::{self, RtkDeps, RtkLimits};
use anthology_testkit::rt_b::{
    FakeAnswer, RTK_HOST, RTK_TEST_BODY, RawKind, RtkFakeFetch, raw_tar_gz, rtk_lock,
    rtk_lock_value, rtk_tarball, rtk_url_for,
};
use anthology_testkit::{Row, repo_root, rows, run_program};

const V1: &str = "1.2.1-anthology.1";
const REPO: &str = "rbonweb/anthology";
const HOST: Target = Target::LinuxX86_64Musl;

fn sha(data: &[u8]) -> String {
    anthology_testkit::release::sha256_hex(data)
}

fn make_lock(version: &str, blob: &[u8]) -> Lock {
    rtk_lock(version, blob, REPO)
}

fn linux() -> (String, String) {
    ("Linux".into(), "x86_64".into())
}

fn windows() -> (String, String) {
    ("Windows".into(), "AMD64".into())
}

fn py_exc(type_name: &'static str, msg: String) -> PyExc {
    PyExc::Other(type_name, msg)
}

/// `FakeFetch` as a [`Fetch`].
struct Fake(RtkFakeFetch);

impl Fake {
    fn for_lock(lock: &Lock, blob: &[u8]) -> Fake {
        Fake(RtkFakeFetch::serving(&rtk_url_for(lock, RTK_HOST), blob))
    }

    fn failing(type_name: &'static str, msg: &str) -> Fake {
        Fake(RtkFakeFetch::failing(type_name, msg))
    }

    fn calls(&self) -> Vec<(String, f64)> {
        self.0.calls()
    }
}

impl Fetch for Fake {
    fn fetch(&self, url: &str, dest: &mut dyn Write, timeout: f64) -> Result<(), PyExc> {
        match self.0.call(url, timeout) {
            FakeAnswer::Write(b) => {
                dest.write_all(b)?;
                Ok(())
            }
            FakeAnswer::Raise(t, m) => Err(py_exc(t, m)),
        }
    }
}

/// `Base`: a temporary folder with `data/`, and `CLAUDE_CONFIG_DIR`/`HOME` inside it.
struct Base {
    _tmp: tempfile::TempDir,
    tmp: PathBuf,
    data: PathBuf,
    cfg: PathBuf,
    env: Env,
}

impl Base {
    fn new() -> Base {
        let dir = tempfile::Builder::new()
            .prefix("t11-")
            .tempdir()
            .expect("tmp");
        let tmp = fs::canonicalize(dir.path()).expect("realpath");
        let data = tmp.join("data");
        let cfg = tmp.join("config");
        let env = Env::default()
            .with_os("CLAUDE_CONFIG_DIR", &cfg)
            .with_os("HOME", tmp.join("home"));
        Base {
            _tmp: dir,
            tmp,
            data,
            cfg,
            env,
        }
    }

    fn entries(&self, path: &Path) -> Vec<String> {
        let mut v: Vec<String> = match fs::read_dir(path) {
            Ok(rd) => rd
                .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        v.sort();
        v
    }

    fn state_write(&self, kv: &[(&str, &str)]) {
        state::write(
            &self.env,
            kv.iter()
                .map(|(k, v)| (k.to_string(), Value::Str(v.to_string())))
                .collect(),
        )
        .expect("state.write");
    }

    fn state_read(&self) -> json::Map {
        state::read(&self.env)
    }
}

fn deps(fetch: &dyn Fetch) -> RtkDeps<'_> {
    let mut d = RtkDeps::new(fetch);
    d.uname = linux;
    d
}

fn install(b: &Base, lock: &Lock, fetch: &dyn Fetch) -> Result<PathBuf, PyExc> {
    rtk_install::install(
        &b.data,
        lock,
        rtk_install::DEFAULT_TIMEOUT,
        HOST,
        &deps(fetch),
    )
}

fn install_with(
    b: &Base,
    lock: &Lock,
    fetch: &dyn Fetch,
    d: impl FnOnce(&mut RtkDeps<'_>),
) -> Result<PathBuf, PyExc> {
    let mut deps = deps(fetch);
    d(&mut deps);
    rtk_install::install(&b.data, lock, rtk_install::DEFAULT_TIMEOUT, HOST, &deps)
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("stat").permissions().mode() & 0o777
}

fn set_mode(path: &Path, m: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(m)).expect("chmod");
}

fn set_mtime(path: &Path, secs_ago: f64) {
    let when = SystemTime::now() - Duration::from_secs_f64(secs_ago);
    let f = fs::File::options()
        .read(true)
        .open(path)
        .or_else(|_| fs::File::open(path))
        .expect("open for utime");
    f.set_times(fs::FileTimes::new().set_accessed(when).set_modified(when))
        .expect("utime");
}

fn set_epoch_mtime(path: &Path, epoch: u64) {
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(epoch);
    let f = fs::File::open(path).expect("open for utime");
    f.set_times(fs::FileTimes::new().set_accessed(when).set_modified(when))
        .expect("utime");
}

mod target_test {
    use super::*;

    #[test]
    fn test_supported_platforms() {
        let cases = [
            ("Darwin", "arm64", "aarch64-apple-darwin"),
            ("Darwin", "aarch64", "aarch64-apple-darwin"),
            ("Darwin", "x86_64", "x86_64-apple-darwin"),
            ("Linux", "x86_64", "x86_64-unknown-linux-musl"),
            ("Linux", "amd64", "x86_64-unknown-linux-musl"),
            ("Linux", "aarch64", "aarch64-unknown-linux-musl"),
            ("Linux", "arm64", "aarch64-unknown-linux-musl"),
        ];
        for (system, machine, want) in cases {
            let got = rtk_install::target(system, machine).expect("supported");
            assert_eq!(got.triple(), want, "{system} {machine}");
        }
    }

    #[test]
    fn test_windows_is_refused_in_any_case() {
        for system in ["Windows", "windows", "WINDOWS", "Windows_NT"] {
            let err = rtk_install::target(system, "AMD64").unwrap_err();
            assert!(err.py_str().contains("Windows isn't supported"), "{system}");
        }
    }

    #[test]
    fn test_everything_else_is_refused() {
        for (system, machine) in [
            ("FreeBSD", "amd64"),
            ("Linux", "riscv64"),
            ("Darwin", "ppc"),
            ("", ""),
        ] {
            assert!(rtk_install::target(system, machine).is_err());
        }
    }
}

mod load_lock_test {
    use super::*;

    fn load(b: &Base, text: &str) -> Result<Lock, PyExc> {
        let path = b.tmp.join("rtk.lock");
        fs::write(&path, text).expect("write lock");
        rtk_install::load_lock(&path)
    }

    fn dumps(v: &Value) -> String {
        json::dumps(v, &json::PY_DEFAULT)
    }

    #[test]
    fn test_valid_lock_round_trips() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = load(&b, &dumps(&rtk_lock_value(V1, &blob, REPO))).expect("valid");
        assert_eq!(lock, make_lock(V1, &blob));
    }

    #[test]
    fn test_release_without_prerelease_is_valid() {
        let b = Base::new();
        let lock = load(&b, &dumps(&rtk_lock_value("2.0.0", b"x", REPO))).expect("valid");
        assert_eq!(lock.tag, "rtk--v2.0.0");
    }

    /// `mutate(fn)` on a fresh `make_lock()` value.
    fn mutate(f: impl FnOnce(&mut json::Map)) -> Value {
        let mut v = rtk_lock_value(V1, &rtk_tarball(), REPO);
        f(v.as_object_mut().expect("object"));
        v
    }

    fn sha_map(m: &mut json::Map) -> &mut json::Map {
        m.get_mut("sha256")
            .and_then(Value::as_object_mut)
            .expect("sha256")
    }

    fn set(m: &mut json::Map, k: &str, v: &str) {
        m.insert(k.into(), Value::Str(v.into()));
    }

    fn bad_lock(name: &str) -> Value {
        match name {
            "uppercase hex" => mutate(|l| set(sha_map(l), RTK_HOST, &"A".repeat(64))),
            "short hex" => mutate(|l| set(sha_map(l), RTK_HOST, &"a".repeat(63))),
            "non-hex" => mutate(|l| set(sha_map(l), RTK_HOST, &"g".repeat(64))),
            "trailing newline" => {
                mutate(|l| set(sha_map(l), RTK_HOST, &format!("{}\n", "a".repeat(64))))
            }
            "wrong tag" => mutate(|l| set(l, "tag", "rtk--v9.9.9")),
            "no tag prefix" => mutate(|l| set(l, "tag", "v1.2.1-anthology.1")),
            "extra target" => {
                mutate(|l| set(sha_map(l), "x86_64-pc-windows-msvc", &"a".repeat(64)))
            }
            "missing target" => mutate(|l| {
                sha_map(l).remove("x86_64-apple-darwin");
            }),
            "bad version" => mutate(|l| {
                set(l, "version", "1.2");
                set(l, "tag", "rtk--v1.2");
            }),
            "path version" => mutate(|l| {
                set(l, "version", "../1.0.0");
                set(l, "tag", "rtk--v../1.0.0");
            }),
            "unsafe repo" => mutate(|l| set(l, "repo", "a/b/../../c")),
            "repo with space" => mutate(|l| set(l, "repo", "a b/c")),
            "repo with query" => mutate(|l| set(l, "repo", "a/b?x=1")),
            "repo no slash" => mutate(|l| set(l, "repo", "rbonweb")),
            "unknown key" => mutate(|l| set(l, "extra", "x")),
            "missing key" => mutate(|l| {
                l.remove("repo");
            }),
            "sha not a map" => mutate(|l| {
                l.insert("sha256".into(), Value::Array(Vec::new()));
            }),
            "not an object" => Value::Array(Vec::new()),
            other => panic!("no case {other:?}"),
        }
    }

    #[test]
    fn test_rejects_invalid_locks() {
        let b = Base::new();
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.LoadLockTest.test_rejects_invalid_locks",
        );
        assert_eq!(table.len(), 18);
        for row in table {
            let name = row.str("$msg");
            let lock = bad_lock(name);
            assert!(load(&b, &dumps(&lock)).is_err(), "{name}");
        }
    }

    #[test]
    fn test_rejects_non_json() {
        let b = Base::new();
        let err = load(&b, "not json").unwrap_err();
        assert!(
            err.py_str().ends_with("is not valid JSON"),
            "{}",
            err.py_str()
        );
    }
}

mod verify_test {
    use super::*;

    #[test]
    fn test_verify() {
        let b = Base::new();
        let path = b.tmp.join("blob");
        fs::write(&path, b"hello").expect("write");
        assert!(rtk_install::verify(&path, &sha(b"hello")));
        assert!(!rtk_install::verify(&path, &sha(b"other")));
        assert!(!rtk_install::verify(&b.tmp.join("missing"), &sha(b"hello")));
    }
}

mod install_test {
    use super::*;

    #[test]
    fn test_good_hash_installs_per_version() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        let path = install(&b, &lock, &fetch).expect("install");
        assert_eq!(path, b.data.join("rtk").join(V1).join("rtk"));
        assert_eq!(fs::read(&path).expect("read"), RTK_TEST_BODY);
        assert_eq!(mode(&path), 0o755);
        assert_eq!(fetch.calls()[0].0, rtk_url_for(&lock, RTK_HOST));
        let vdir = path.parent().expect("vdir");
        assert_eq!(
            fs::read_to_string(vdir.join(".sha256"))
                .expect("marker")
                .trim(),
            lock.sha256[&HOST]
        );
        assert_eq!(b.entries(vdir), [".last-used", ".sha256", "rtk"]);
        assert_eq!(b.entries(&b.data.join("rtk")), [V1]);
    }

    #[test]
    fn test_bad_hash_leaves_nothing_behind() {
        let b = Base::new();
        let blob = rtk_tarball();
        let mut lock = make_lock(V1, &blob);
        lock.sha256.insert(HOST, "0".repeat(64));
        let fetch = Fake::for_lock(&lock, &blob);
        assert!(install(&b, &lock, &fetch).is_err());
        assert_eq!(fetch.calls().len(), 1);
        assert!(b.entries(&b.data.join("rtk")).is_empty());
    }

    #[test]
    fn test_second_run_does_not_fetch() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        let first = install(&b, &lock, &fetch).expect("first");
        let second = install(&b, &lock, &fetch).expect("second");
        assert_eq!(first, second);
        assert_eq!(fetch.calls().len(), 1);
    }

    #[test]
    fn test_reinstalls_when_marker_disagrees_with_lock() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        let path = install(&b, &lock, &fetch).expect("install");
        let marker = path.parent().expect("vdir").join(".sha256");
        fs::write(&marker, format!("{}\n", "f".repeat(64))).expect("marker");
        install(&b, &lock, &fetch).expect("reinstall");
        assert_eq!(fetch.calls().len(), 2);
        assert_eq!(
            fs::read_to_string(&marker).expect("marker").trim(),
            lock.sha256[&HOST]
        );
    }

    #[test]
    fn test_reinstalls_when_binary_is_not_executable() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        let path = install(&b, &lock, &fetch).expect("install");
        set_mode(&path, 0o644);
        install(&b, &lock, &fetch).expect("reinstall");
        assert_eq!(fetch.calls().len(), 2);
        assert_eq!(mode(&path) & 0o111, 0o111);
    }

    fn rejected(name: &str) -> Vec<(&'static str, RawKind)> {
        let body = || RawKind::File(RTK_TEST_BODY.to_vec());
        let x = || RawKind::File(b"x".to_vec());
        match name {
            "symlink rtk" => vec![("rtk", RawKind::Symlink("/bin/sh".into()))],
            "hardlink rtk" => vec![
                ("rtk", RawKind::Hardlink("LICENSE".into())),
                ("LICENSE", x()),
            ],
            "dotdot member" => vec![("../rtk", body()), ("LICENSE", x())],
            "nested member" => vec![("bin/rtk", body())],
            "absolute member" => vec![("/rtk", body())],
            "no rtk member" => vec![("LICENSE", x())],
            "duplicate rtk" => vec![("rtk", body()), ("rtk", RawKind::File(b"other".to_vec()))],
            other => panic!("no case {other:?}"),
        }
    }

    #[test]
    fn test_rejected_tarballs_leave_nothing_behind() {
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.InstallTest.test_rejected_tarballs_leave_nothing_behind",
        );
        assert_eq!(table.len(), 7);
        let b = Base::new();
        for row in table {
            let name = row.str("$msg");
            let blob = raw_tar_gz(&rejected(name));
            let lock = make_lock(V1, &blob);
            let err = install(&b, &lock, &Fake::for_lock(&lock, &blob));
            assert!(err.is_err(), "{name}");
            assert!(b.entries(&b.data.join("rtk")).is_empty(), "{name}");
        }
    }

    #[test]
    fn test_oversized_member_is_rejected() {
        let b = Base::new();
        let blob = raw_tar_gz(&[("rtk", RawKind::File(vec![b'x'; 2048]))]);
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        let err = install_with(&b, &lock, &fetch, |d| {
            d.limits = RtkLimits {
                max_binary_bytes: 1024,
                ..RtkLimits::default()
            }
        })
        .unwrap_err();
        assert_eq!(err.py_str(), "the rtk member is too large");
        assert!(b.entries(&b.data.join("rtk")).is_empty());
    }

    #[test]
    fn test_garbage_that_hashes_correctly_is_rejected() {
        let b = Base::new();
        let blob = b"this is not a tarball";
        let lock = make_lock(V1, blob);
        let err = install(&b, &lock, &Fake::for_lock(&lock, blob)).unwrap_err();
        assert!(
            err.py_str().starts_with("cannot read the rtk tarball: "),
            "{}",
            err.py_str()
        );
        assert!(b.entries(&b.data.join("rtk")).is_empty());
    }

    #[test]
    fn test_failed_fetch_leaves_nothing_behind() {
        let b = Base::new();
        let lock = make_lock(V1, &rtk_tarball());
        assert!(install(&b, &lock, &Fake::failing("OSError", "boom")).is_err());
        assert!(b.entries(&b.data.join("rtk")).is_empty());
    }

    fn tar_open_is_an_assertion() -> Result<(), PyExc> {
        Err(PyExc::Other("AssertionError", "opened".into()))
    }

    #[test]
    fn test_the_hash_is_checked_before_extraction() {
        let b = Base::new();
        let blob = rtk_tarball();
        let mut lock = make_lock(V1, &blob);
        lock.sha256.insert(HOST, "1".repeat(64));
        let fetch = Fake::for_lock(&lock, &blob);
        let err = install_with(&b, &lock, &fetch, |d| {
            d.tar_open = Some(tar_open_is_an_assertion)
        })
        .unwrap_err();
        assert_ne!(err.type_name(), "AssertionError");
        assert!(err.py_str().contains("does not match rtk.lock"));
    }
}

mod check_installed_test {
    use super::*;

    fn install5(b: &Base, lock: &Lock) -> PathBuf {
        let blob = rtk_tarball();
        rtk_install::install(
            &b.data,
            lock,
            5.0,
            HOST,
            &deps(&Fake::for_lock(lock, &blob)),
        )
        .expect("install")
    }

    #[test]
    fn test_true_after_install_and_false_before() {
        let b = Base::new();
        let lock = make_lock(V1, &rtk_tarball());
        assert!(rtk_install::check_installed_for(&b.data, &lock, HOST).is_err());
        let path = install5(&b, &lock);
        assert_eq!(
            rtk_install::check_installed_for(&b.data, &lock, HOST),
            Ok(path)
        );
    }

    #[test]
    fn test_default_target_is_this_machine() {
        let b = Base::new();
        let lock = make_lock(V1, &rtk_tarball());
        install5(&b, &lock);
        assert!(rtk_install::check_installed_with(&b.data, &lock, ("Linux", "x86_64")).is_ok());
    }

    #[test]
    fn test_false_for_a_different_lock_hash_or_version() {
        let b = Base::new();
        let lock = make_lock(V1, &rtk_tarball());
        install5(&b, &lock);
        let other_hash = make_lock(
            V1,
            &raw_tar_gz(&[("rtk", RawKind::File(b"other".to_vec()))]),
        );
        assert!(rtk_install::check_installed_for(&b.data, &other_hash, HOST).is_err());
        let other_version = make_lock("1.9.0", &rtk_tarball());
        assert!(rtk_install::check_installed_for(&b.data, &other_version, HOST).is_err());
    }

    #[test]
    fn test_false_when_the_binary_is_not_executable_or_the_marker_is_gone() {
        let b = Base::new();
        let lock = make_lock(V1, &rtk_tarball());
        let path = install5(&b, &lock);
        set_mode(&path, 0o600);
        assert!(rtk_install::check_installed_for(&b.data, &lock, HOST).is_err());
        set_mode(&path, 0o755);
        fs::remove_file(path.parent().expect("vdir").join(".sha256")).expect("unlink");
        assert!(rtk_install::check_installed_for(&b.data, &lock, HOST).is_err());
    }

    #[test]
    fn test_never_raises() {
        let b = Base::new();
        // An invalid lock cannot be a `Lock`: `validate_lock` refuses it, which is the "False".
        assert!(rtk_install::validate_lock(&json::loads("{\"nope\": 1}").expect("json")).is_err());
        // A target the lock has no hash for.
        let mut lock = make_lock(V1, &rtk_tarball());
        lock.sha256.remove(&HOST);
        assert!(rtk_install::check_installed_for(&b.data, &lock, HOST).is_err());
        let lock = make_lock(V1, &rtk_tarball());
        let err = rtk_install::check_installed_with(&b.data, &lock, ("Windows", "AMD64"));
        assert!(err.unwrap_err().contains("Windows isn't supported"));
    }
}

mod prune_test {
    use super::*;

    fn make_version(b: &Base, name: &str, age_days: f64, marker: bool) -> PathBuf {
        let vdir = b.data.join("rtk").join(name);
        fs::create_dir_all(&vdir).expect("mkdir");
        fs::write(vdir.join("rtk"), b"x").expect("rtk");
        let ago = age_days * 86400.0;
        if marker {
            fs::write(vdir.join(".last-used"), b"").expect("marker");
            set_mtime(&vdir.join(".last-used"), ago);
        }
        set_mtime(&vdir, ago);
        vdir
    }

    #[test]
    fn test_install_prunes_old_versions_but_not_current_or_recent() {
        let b = Base::new();
        let old = make_version(&b, "0.9.0", 45.0, true);
        let recent = make_version(&b, "1.0.0", 5.0, true);
        let no_marker_old = make_version(&b, "0.8.0", 90.0, false);
        let stray = b.data.join("rtk").join("notes");
        fs::create_dir(&stray).expect("stray");
        set_epoch_mtime(&stray, 1);
        let link_target = b.tmp.join("elsewhere");
        fs::create_dir(&link_target).expect("elsewhere");
        fs::write(link_target.join("keep"), b"x").expect("keep");
        set_epoch_mtime(&link_target, 1);
        let link = b.data.join("rtk").join("0.1.0");
        std::os::unix::fs::symlink(&link_target, &link).expect("symlink");

        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let current = b.data.join("rtk").join(&lock.version);
        fs::create_dir(&current).expect("current");
        fs::write(current.join("rtk"), b"x").expect("rtk");
        fs::write(current.join(".last-used"), b"").expect("marker");
        set_epoch_mtime(&current.join(".last-used"), 1); // ancient, but it is the current version
        set_epoch_mtime(&current, 1);
        install(&b, &lock, &Fake::for_lock(&lock, &blob)).expect("install");

        assert!(!old.exists());
        assert!(!no_marker_old.exists());
        assert!(recent.exists());
        assert!(current.exists());
        assert!(stray.exists());
        assert!(
            fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        assert!(link_target.join("keep").exists());
    }

    #[test]
    fn test_install_touches_last_used_on_the_current_version() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        let path = install(&b, &lock, &fetch).expect("install");
        let marker = path.parent().expect("vdir").join(".last-used");
        set_epoch_mtime(&marker, 1);
        install(&b, &lock, &fetch).expect("again");
        let mtime = fs::metadata(&marker)
            .expect("stat")
            .modified()
            .expect("mtime");
        assert!(mtime > SystemTime::now() - Duration::from_secs(60));
        assert_eq!(fetch.calls().len(), 1);
    }

    #[test]
    fn stale_temp_folders_are_pruned_and_fresh_ones_kept() {
        let b = Base::new();
        let root = b.data.join("rtk");
        fs::create_dir_all(root.join(".tmp-old")).expect("old");
        fs::create_dir_all(root.join(".tmp-new")).expect("new");
        set_mtime(&root.join(".tmp-old"), 25.0 * 3600.0);
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("now")
            .as_secs_f64();
        assert_eq!(rtk_install::prune(&root, V1, now), [".tmp-old"]);
        assert!(root.join(".tmp-new").exists());
    }
}

mod reconcile_test {
    use super::*;

    fn reconcile(b: &Base, lock: &Lock, fetch: &dyn Fetch) -> rtk_install::Reconciled {
        rtk_install::reconcile(&b.env, &b.data, lock, 5.0, &deps(fetch))
    }

    #[test]
    fn test_installs_and_records_the_path() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let res = reconcile(&b, &lock, &Fake::for_lock(&lock, &blob));
        let want = b.data.join("rtk").join(V1).join("rtk");
        assert_eq!(
            res,
            rtk_install::Reconciled {
                rtk_path: Some(want.clone()),
                warning: None,
                installed: true
            }
        );
        assert_eq!(
            b.state_read().get("rtk_path").and_then(Value::as_str),
            want.to_str()
        );
    }

    #[test]
    fn test_already_installed_does_not_fetch() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        reconcile(&b, &lock, &fetch);
        let res = reconcile(&b, &lock, &fetch);
        assert!(!res.installed);
        assert_eq!(res.warning, None);
        assert_eq!(fetch.calls().len(), 1);
    }

    #[test]
    fn test_lock_change_installs_new_version_keeps_old_and_updates_state() {
        let b = Base::new();
        let blob1 = rtk_tarball();
        let old_lock = make_lock(V1, &blob1);
        reconcile(&b, &old_lock, &Fake::for_lock(&old_lock, &blob1));
        let blob2 = raw_tar_gz(&[("rtk", RawKind::File(b"#!/bin/sh\necho v2\n".to_vec()))]);
        let new_lock = make_lock("1.3.0-anthology.1", &blob2);
        let res = reconcile(&b, &new_lock, &Fake::for_lock(&new_lock, &blob2));
        assert!(res.installed);
        assert_eq!(b.entries(&b.data.join("rtk")), [V1, "1.3.0-anthology.1"]);
        let want = b.data.join("rtk").join("1.3.0-anthology.1").join("rtk");
        assert_eq!(
            b.state_read().get("rtk_path").and_then(Value::as_str),
            want.to_str()
        );
        assert!(b.data.join("rtk").join(V1).join("rtk").exists());
    }

    #[test]
    fn test_fetch_failure_returns_a_warning_and_keeps_state() {
        let b = Base::new();
        let lock = make_lock(V1, &rtk_tarball());
        b.state_write(&[("rtk_path", "/previous/rtk"), ("other", "kept")]);
        for (t, msg) in [
            ("OSError", "offline"),
            ("URLError", "<urlopen error dns>"),
            ("RuntimeError", "weird"),
        ] {
            let res = reconcile(&b, &lock, &Fake::failing(t, msg));
            assert_eq!(res.rtk_path, None);
            assert!(!res.installed);
            let w = res.warning.expect("warning");
            assert!(w.contains("rtk"), "{w}");
            assert!(w.contains(msg), "{w}");
        }
        let st = b.state_read();
        assert_eq!(st.len(), 2);
        assert_eq!(
            st.get("rtk_path").and_then(Value::as_str),
            Some("/previous/rtk")
        );
        assert_eq!(st.get("other").and_then(Value::as_str), Some("kept"));
        assert!(b.entries(&b.data.join("rtk")).is_empty());
    }

    #[test]
    fn test_bad_hash_returns_a_warning() {
        let b = Base::new();
        let blob = rtk_tarball();
        let mut lock = make_lock(V1, &blob);
        lock.sha256.insert(HOST, "2".repeat(64));
        let res = reconcile(&b, &lock, &Fake::for_lock(&lock, &blob));
        assert_eq!(res.rtk_path, None);
        assert!(res.warning.expect("warning").contains("sha256"));
        assert!(b.state_read().get("rtk_path").is_none());
    }

    #[test]
    fn test_unsupported_platform_returns_a_warning() {
        let b = Base::new();
        let lock = make_lock(V1, &rtk_tarball());
        let fetch = Fake(RtkFakeFetch::default());
        let mut d = deps(&fetch);
        d.uname = windows;
        let res = rtk_install::reconcile(&b.env, &b.data, &lock, 5.0, &d);
        assert!(
            res.warning
                .expect("warning")
                .contains("Windows isn't supported")
        );
        assert!(fetch.calls().is_empty());
    }

    #[test]
    fn test_uses_the_running_platform_by_default() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        // The `platform` seam: `RtkDeps::new` reads `platform::uname`; the test pins it, as
        // Python patches `platform.system`/`machine`.
        let mut d = RtkDeps::new(&fetch);
        d.uname = linux;
        let res = rtk_install::reconcile(&b.env, &b.data, &lock, 5.0, &d);
        assert!(res.installed, "{res:?}");
    }

    #[test]
    fn the_warning_names_the_error_as_python_does() {
        let exc = PyExc::Other("OSError", "offline".into());
        assert_eq!(
            rtk_install::warning_text(&exc),
            "rtk is not installed or up to date (offline); commands run without it. It is \
             retried at the next session start."
        );
    }

    #[test]
    fn a_marker_that_is_not_utf8_is_a_warning_as_in_python() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let fetch = Fake::for_lock(&lock, &blob);
        reconcile(&b, &lock, &fetch);
        fs::write(b.data.join("rtk").join(V1).join(".sha256"), b"\xff\n").expect("marker");
        let res = reconcile(&b, &lock, &fetch);
        assert_eq!(
            res.warning.expect("warning"),
            "rtk is not installed or up to date ('utf-8' codec can't decode byte 0xff in \
             position 0: invalid start byte); commands run without it. It is retried at the next \
             session start."
        );
        assert_eq!(fetch.calls().len(), 1);
    }

    #[test]
    fn a_version_folder_that_is_a_symlink_is_refused() {
        let b = Base::new();
        let blob = rtk_tarball();
        let lock = make_lock(V1, &blob);
        let elsewhere = b.tmp.join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("elsewhere");
        fs::create_dir_all(b.data.join("rtk")).expect("rtk");
        std::os::unix::fs::symlink(&elsewhere, b.data.join("rtk").join(V1)).expect("link");
        let res = reconcile(&b, &lock, &Fake::for_lock(&lock, &blob));
        let w = res.warning.expect("warning");
        assert!(w.contains(" is a symlink"), "{w}");
        assert!(b.entries(&elsewhere).is_empty());
        assert_eq!(b.entries(&b.data.join("rtk")), [V1]);
    }
}

/// `FakeResponse` / `FakeOpener` behind the [`Opener`] seam (`build_opener`).
mod default_fetch_test {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    thread_local! {
        static NOW: Cell<f64> = const { Cell::new(1000.0) };
    }

    fn clock() -> f64 {
        NOW.with(Cell::get)
    }

    #[derive(Clone, Default)]
    struct Shared {
        reads: Rc<Cell<usize>>,
        requests: Rc<RefCell<Vec<(String, f64)>>>,
    }

    struct FakeResponse {
        body: Vec<u8>,
        url: String,
        length: Option<String>,
        chunk: usize,
        trickle: bool,
        fail_after: Option<usize>,
        pos: usize,
        shared: Shared,
    }

    fn response(body: &[u8]) -> FakeResponse {
        FakeResponse {
            body: body.to_vec(),
            url: "https://objects.example.invalid/rtk.tar.gz".into(),
            length: None,
            chunk: 1000,
            trickle: false,
            fail_after: None,
            pos: 0,
            shared: Shared::default(),
        }
    }

    impl Response for FakeResponse {
        fn final_url(&self) -> String {
            self.url.clone()
        }
        fn content_length(&self) -> Option<String> {
            self.length.clone()
        }
        fn read1(&mut self, buf: &mut [u8]) -> Result<usize, PyExc> {
            self.shared.reads.set(self.shared.reads.get() + 1);
            if self.trickle {
                // Each read takes 4 s; the overall budget is 4 * 5 = 20 s.
                NOW.with(|n| n.set(n.get() + 4.0));
            }
            if self.fail_after.is_some_and(|f| self.pos >= f) {
                return Err(PyExc::Other(
                    "ConnectionResetError",
                    "connection reset".into(),
                ));
            }
            let end = (self.pos + self.chunk.min(buf.len())).min(self.body.len());
            let n = end - self.pos;
            buf[..n].copy_from_slice(&self.body[self.pos..end]);
            self.pos = end;
            Ok(n)
        }
    }

    struct FakeOpener {
        response: RefCell<Option<FakeResponse>>,
        shared: Shared,
    }

    impl FakeOpener {
        fn new(r: FakeResponse) -> FakeOpener {
            let shared = r.shared.clone();
            FakeOpener {
                response: RefCell::new(Some(r)),
                shared,
            }
        }
    }

    impl Opener for FakeOpener {
        fn open(&self, url: &str, timeout: f64) -> Result<Box<dyn Response>, PyExc> {
            self.shared
                .requests
                .borrow_mut()
                .push((url.to_string(), timeout));
            Ok(Box::new(
                self.response.borrow_mut().take().expect("one open"),
            ))
        }
    }

    /// `default_fetch` over a fake opener.
    struct OpenerFetch<'a> {
        opener: &'a dyn Opener,
        max: u64,
    }

    impl Fetch for OpenerFetch<'_> {
        fn fetch(&self, url: &str, dest: &mut dyn Write, timeout: f64) -> Result<(), PyExc> {
            fetch::download(self.opener, url, dest, timeout, self.max, clock)
        }
    }

    const URL: &str = "https://github.com/o/r/releases/download/t/f.tar.gz";

    fn run(b: &Base, opener: &FakeOpener, url: &str, timeout: f64, max: u64) -> Result<(), PyExc> {
        let f = OpenerFetch { opener, max };
        fetch::fetch_to_path(&f, url, &b.tmp.join("out"), timeout)
    }

    struct NoOpener;

    impl Opener for NoOpener {
        fn open(&self, _: &str, _: f64) -> Result<Box<dyn Response>, PyExc> {
            panic!("opener built");
        }
    }

    #[test]
    fn test_refuses_non_https_urls() {
        let b = Base::new();
        for url in [
            "http://example.invalid/x",
            "file:///etc/passwd",
            "ftp://example.invalid/x",
        ] {
            let f = OpenerFetch {
                opener: &NoOpener,
                max: fetch::MAX_DOWNLOAD_BYTES,
            };
            let err = fetch::fetch_to_path(&f, url, &b.tmp.join("out"), 5.0).unwrap_err();
            assert_eq!(err.type_name(), "InstallError");
            assert!(!b.tmp.join("out").exists());
        }
    }

    #[test]
    fn test_uses_the_https_only_redirect_handler() {
        // ureq follows no redirect itself (every hop goes through `redirect_target`), and the
        // agent refuses plain http.
        let opener = anthology_runtime::fetch::UreqFetch::new(&Env::default()).opener();
        let config = opener
            .config(
                "github.com",
                Duration::from_secs(5),
                Duration::from_secs(20),
            )
            .expect("config");
        assert_eq!(config.max_redirects(), 0);
        assert!(config.https_only());
        assert!(matches!(
            config.tls_config().root_certs(),
            ureq::tls::RootCerts::PlatformVerifier
        ));
    }

    #[test]
    fn test_downloads_the_body_and_passes_the_timeout() {
        let b = Base::new();
        let body = b"abc".repeat(5000);
        let mut r = response(&body);
        r.length = Some(body.len().to_string());
        let opener = FakeOpener::new(r);
        run(&b, &opener, URL, 7.0, fetch::MAX_DOWNLOAD_BYTES).expect("fetch");
        assert_eq!(fs::read(b.tmp.join("out")).expect("out"), body);
        assert_eq!(opener.shared.requests.borrow()[0].1, 7.0);
    }

    #[test]
    fn test_refuses_a_final_url_that_is_not_https() {
        let b = Base::new();
        let mut r = response(b"x");
        r.url = "http://evil.example.invalid/x".into();
        let err = run(&b, &FakeOpener::new(r), URL, 5.0, fetch::MAX_DOWNLOAD_BYTES).unwrap_err();
        assert_eq!(err.type_name(), "InstallError");
        assert!(!b.tmp.join("out").exists());
    }

    #[test]
    fn test_streamed_size_cap_removes_the_partial_file() {
        let b = Base::new();
        // No Content-Length: only the stream can tell.
        let err = run(
            &b,
            &FakeOpener::new(response(&[b'x'; 5000])),
            URL,
            5.0,
            1024,
        )
        .unwrap_err();
        assert_eq!(err.py_str(), "download is larger than 1024 bytes");
        assert!(!b.tmp.join("out").exists());
    }

    #[test]
    fn test_content_length_cap_refuses_before_reading() {
        let b = Base::new();
        let mut r = response(&[b'x'; 10]);
        r.length = Some("1000000000".into());
        let opener = FakeOpener::new(r);
        let err = run(&b, &opener, URL, 5.0, fetch::MAX_DOWNLOAD_BYTES).unwrap_err();
        assert_eq!(err.type_name(), "InstallError");
        assert_eq!(opener.shared.reads.get(), 0);
        assert!(!b.tmp.join("out").exists());
    }

    #[test]
    fn test_failure_midway_removes_the_partial_file() {
        let b = Base::new();
        let mut r = response(&[b'x'; 5000]);
        r.chunk = 500;
        r.fail_after = Some(1500);
        let err = run(&b, &FakeOpener::new(r), URL, 5.0, fetch::MAX_DOWNLOAD_BYTES).unwrap_err();
        assert_eq!(err.type_name(), "ConnectionResetError");
        assert!(!b.tmp.join("out").exists());
    }

    #[test]
    fn test_deadline_is_checked_after_every_receive() {
        let b = Base::new();
        NOW.with(|n| n.set(1000.0));
        let mut r = response(&[b'x'; 100_000]);
        r.chunk = 10;
        r.trickle = true;
        let opener = FakeOpener::new(r);
        let err = run(&b, &opener, URL, 5.0, fetch::MAX_DOWNLOAD_BYTES).unwrap_err();
        assert!(err.py_str().contains("timed out"), "{}", err.py_str());
        assert!(
            opener.shared.reads.get() <= 7,
            "{}",
            opener.shared.reads.get()
        );
        assert!(!b.tmp.join("out").exists());
    }

    #[test]
    fn test_refuses_redirects_to_non_https() {
        let err = fetch::redirect_target("https://example.invalid/a", "http://example.invalid/b")
            .unwrap_err();
        assert_eq!(err.type_name(), "URLError");
    }

    #[test]
    fn test_https_redirect_is_followed() {
        let new = fetch::redirect_target(
            "https://example.invalid/a",
            "https://objects.example.invalid/b",
        )
        .expect("followed");
        assert_eq!(new, "https://objects.example.invalid/b");
    }
}

mod state_write_test {
    use super::*;

    #[test]
    fn test_non_ascii_is_written_literally() {
        let b = Base::new();
        b.state_write(&[("rtk_path", "/h\u{e9}llo/rtk")]);
        let text = fs::read_to_string(state::path(&b.env)).expect("state");
        assert!(text.contains("/h\u{e9}llo/rtk"), "{text}");
        assert_eq!(
            b.state_read().get("rtk_path").and_then(Value::as_str),
            Some("/h\u{e9}llo/rtk")
        );
    }
}

mod files_test {
    use super::*;

    fn plugin() -> PathBuf {
        repo_root().join("plugins").join("anthology")
    }

    #[test]
    fn test_hooks_json_shape() {
        let text = fs::read_to_string(plugin().join("hooks").join("hooks.json")).expect("hooks");
        let hooks = json::loads(&text).expect("json");
        let top = hooks.as_object().expect("object");
        let keys: Vec<&str> = top.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["hooks"]);
        let inner = top.get("hooks").and_then(Value::as_object).expect("hooks");
        let mut names: Vec<&str> = inner.iter().map(|(k, _)| k).collect();
        names.sort_unstable();
        assert_eq!(names, ["PreToolUse", "SessionStart"]);
        let want = json::loads(
            r#"[{"matcher": "Bash", "hooks": [
                {"type": "command", "command": "sh", "args": ["${CLAUDE_PLUGIN_ROOT}/bin/rtk-hook"],
                 "timeout": 60, "onFailure": "block"}]}]"#,
        )
        .expect("want");
        assert_eq!(inner.get("PreToolUse"), Some(&want));
    }

    #[test]
    fn test_wrappers_are_executable_posix_sh() {
        for name in ["rtk", "rtk-hook"] {
            let path = plugin().join("bin").join(name);
            assert_eq!(mode(&path) & 0o111, 0o111, "{name}");
            let text = fs::read_to_string(&path).expect("wrapper");
            assert_eq!(text.lines().next(), Some("#!/bin/sh"), "{name}");
        }
    }
}

mod wrapper_test {
    use super::*;

    struct W {
        b: Base,
        fake: PathBuf,
    }

    fn setup() -> W {
        let b = Base::new();
        let fake = b.tmp.join("fake-rtk");
        fs::write(
            &fake,
            "#!/bin/sh\necho \"args:$*\"\necho \"suppress:$RTK_SUPPRESS_HOOK_WARNING\"\n\
             if [ \"$1\" = hook ]; then cat; fi\n",
        )
        .expect("fake");
        set_mode(&fake, 0o755);
        W { b, fake }
    }

    fn path_var() -> String {
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
    }

    fn wrapper(name: &str) -> PathBuf {
        repo_root()
            .join("plugins")
            .join("anthology")
            .join("bin")
            .join(name)
    }

    impl W {
        fn env(&self) -> Env {
            Env::default()
                .with("PATH", &path_var())
                .with_os("HOME", self.b.tmp.join("home"))
                .with_os("CLAUDE_CONFIG_DIR", &self.b.cfg)
        }

        fn run_bin(&self, name: &str, args: &[&str], stdin: &str) -> anthology_testkit::Output {
            run_program(
                &wrapper(name),
                args,
                stdin.as_bytes(),
                &self.env(),
                &self.b.tmp,
            )
        }

        fn copy_fake(&self, dest: &Path) -> PathBuf {
            fs::create_dir_all(dest.parent().expect("parent")).expect("mkdir");
            fs::copy(&self.fake, dest).expect("copy");
            set_mode(dest, 0o755);
            dest.to_path_buf()
        }

        fn fake_str(&self) -> &str {
            self.fake.to_str().expect("utf-8 tmp")
        }
    }

    fn lines(o: &anthology_testkit::Output) -> Vec<String> {
        o.stdout_text().lines().map(str::to_string).collect()
    }

    #[test]
    fn test_rtk_execs_the_recorded_binary() {
        let w = setup();
        w.b.state_write(&[("rtk_path", w.fake_str())]);
        let res = w.run_bin("rtk", &["git", "status"], "");
        assert_eq!(res.code, 0, "{}", res.stderr_text());
        assert_eq!(lines(&res), ["args:git status", "suppress:1"]);
    }

    #[test]
    fn test_rtk_uses_home_when_config_dir_is_unset() {
        let w = setup();
        let home = w.b.tmp.join("home");
        fs::create_dir_all(home.join(".claude").join("anthology")).expect("mkdir");
        let mut m = json::Map::new();
        m.insert("rtk_path".into(), Value::Str(w.fake_str().into()));
        let text = json::dumps(&Value::Object(m), &json::STATE) + "\n";
        fs::write(
            home.join(".claude").join("anthology").join("state.json"),
            text,
        )
        .expect("state");
        let env = Env::default()
            .with("PATH", &path_var())
            .with_os("HOME", &home);
        let res = run_program(&wrapper("rtk"), &["x"], b"", &env, &w.b.tmp);
        assert_eq!(lines(&res)[0], "args:x");
    }

    #[test]
    fn test_rtk_finds_the_key_wherever_it_sits() {
        let w = setup();
        // rtk_path mid-file.
        w.b.state_write(&[
            ("data_dir", "/d"),
            ("rtk_path", w.fake_str()),
            ("zzz", "after"),
        ]);
        assert_eq!(lines(&w.run_bin("rtk", &["a"], ""))[0], "args:a");
        // The sole (last) key.
        let mut m = json::Map::new();
        m.insert("rtk_path".into(), Value::Str(w.fake_str().into()));
        fs::write(
            state::path(&w.b.env),
            json::dumps(&Value::Object(m), &json::STATE) + "\n",
        )
        .expect("state");
        assert_eq!(lines(&w.run_bin("rtk", &["b"], ""))[0], "args:b");
    }

    #[test]
    fn test_rtk_handles_a_path_with_spaces() {
        let w = setup();
        let p = w.copy_fake(&w.b.tmp.join("with space").join("rtk"));
        w.b.state_write(&[("rtk_path", p.to_str().expect("utf-8"))]);
        assert_eq!(lines(&w.run_bin("rtk", &["ok"], ""))[0], "args:ok");
    }

    #[test]
    fn test_rtk_non_ascii_path_round_trips() {
        let w = setup();
        let p = w.copy_fake(&w.b.tmp.join("h\u{e9}llo").join("rtk"));
        w.b.state_write(&[("rtk_path", p.to_str().expect("utf-8"))]);
        assert_eq!(lines(&w.run_bin("rtk", &["ok"], ""))[0], "args:ok");
    }

    #[test]
    fn test_rtk_missing_state_exits_127_with_the_message() {
        let w = setup();
        let res = w.run_bin("rtk", &["git", "status"], "");
        assert_eq!(res.code, 127);
        assert!(
            res.stderr_text()
                .contains("rtk is not installed: run /anthology:setup")
        );
        assert_eq!(res.stdout_text(), "");
    }

    #[test]
    fn test_rtk_unusable_paths_exit_127() {
        let w = setup();
        let notexec = w.b.tmp.join("notexec");
        fs::write(&notexec, "#!/bin/sh\n").expect("notexec");
        let tmp = w.b.tmp.to_str().expect("utf-8 tmp");
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_rtk_unusable_paths_exit_127",
        );
        assert_eq!(table.len(), 8);
        for row in table {
            let value = row.str("value").replace("<tmp>", tmp);
            w.b.state_write(&[("rtk_path", &value)]);
            let res = w.run_bin("rtk", &["x"], "");
            assert_eq!(res.code, 127, "{value}");
            assert!(
                res.stderr_text().contains("run /anthology:setup"),
                "{value}"
            );
        }
    }

    impl W {
        fn env_fake(&self) -> PathBuf {
            let fake = self.b.tmp.join("env-rtk");
            fs::write(
                &fake,
                "#!/bin/sh\necho \"db:$RTK_DB_PATH\"\necho \"recall:$RTK_RECALL_DB\"\n\
                 echo \"tee:$RTK_TEE_DIR\"\n",
            )
            .expect("env-rtk");
            set_mode(&fake, 0o755);
            fake
        }
    }

    #[test]
    fn test_both_wrappers_keep_rtk_history_next_to_state_json() {
        // I4: rtk's tracking db (command lines), recall store and tee spool live under
        // state.rtk_data_dir(), which remove deletes, never in rtk's default data folder.
        let w = setup();
        let fake = w.env_fake();
        w.b.state_write(&[("rtk_path", fake.to_str().expect("utf-8 tmp"))]);
        let base = state::path(&w.b.env)
            .parent()
            .expect("state dir")
            .join("rtk");
        assert_eq!(base, w.b.cfg.join("anthology").join("rtk"));
        let want = [
            format!("db:{}", base.join("history.db").display()),
            format!("recall:{}", base.join("recall.db").display()),
            format!("tee:{}", base.join("tee").display()),
        ];
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_both_wrappers_keep_rtk_history_next_to_state_json",
        );
        assert_eq!(table.len(), 2);
        for row in table {
            let name = row.str("name");
            let res = w.run_bin(name, &["x"], "{}");
            assert_eq!(res.code, 0, "{name}: {}", res.stderr_text());
            assert_eq!(lines(&res), want, "{name}");
        }
    }

    #[test]
    fn test_wrappers_history_follows_home_when_config_dir_is_unset() {
        let w = setup();
        let fake = w.env_fake();
        let home = w.b.tmp.join("home");
        fs::create_dir_all(home.join(".claude").join("anthology")).expect("mkdir");
        let mut m = json::Map::new();
        m.insert(
            "rtk_path".into(),
            Value::Str(fake.to_str().expect("utf-8 tmp").into()),
        );
        let text = json::dumps(&Value::Object(m), &json::STATE) + "\n";
        fs::write(
            home.join(".claude").join("anthology").join("state.json"),
            text,
        )
        .expect("state");
        let env = Env::default()
            .with("PATH", &path_var())
            .with_os("HOME", &home);
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_wrappers_history_follows_home_when_config_dir_is_unset",
        );
        assert_eq!(table.len(), 2);
        for row in table {
            let name = row.str("name");
            let res = run_program(&wrapper(name), &[], b"{}", &env, &w.b.tmp);
            assert_eq!(
                lines(&res)[0],
                format!(
                    "db:{}",
                    home.join(".claude")
                        .join("anthology")
                        .join("rtk")
                        .join("history.db")
                        .display()
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn test_hook_runs_rtk_hook_claude_with_stdin_passed_through() {
        let w = setup();
        w.b.state_write(&[("rtk_path", w.fake_str())]);
        let res = w.run_bin("rtk-hook", &[], r#"{"tool_name":"Bash"}"#);
        assert_eq!(res.code, 0, "{}", res.stderr_text());
        assert_eq!(
            lines(&res),
            ["args:hook claude", "suppress:1", r#"{"tool_name":"Bash"}"#]
        );
    }

    const ASK: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecisionReason":"RTK auto-rewrite","updatedInput":{"command":"node --version; bun --version"},"permissionDecision":"ask"}}"#;
    const REWRITE: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecisionReason":"RTK auto-rewrite","updatedInput":{"command":"rtk git status --short"}}}"#;
    const DENY: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"RTK: a deny rule matches"}}"#;

    /// The answer a row names.
    fn answer(name: &str) -> &'static str {
        match name {
            "ask" => ASK,
            "rewrite" => REWRITE,
            "deny" => DENY,
            "not json" => "not json",
            other => panic!("unknown answer {other:?}"),
        }
    }

    impl W {
        /// A fake rtk that saves its stdin, prints one hook answer and exits with `rc`.
        fn answer_fake(&self, answer: &str, rc: i64) {
            let fake = self.b.tmp.join("answer-rtk");
            let seen = self.b.tmp.join("stdin-seen");
            fs::write(
                &fake,
                format!(
                    "#!/bin/sh\ncat > '{}'\nprintf '%s\\n' '{answer}'\nexit {rc}\n",
                    seen.display()
                ),
            )
            .expect("answer-rtk");
            set_mode(&fake, 0o755);
            self.b
                .state_write(&[("rtk_path", fake.to_str().expect("utf-8 tmp"))]);
        }
    }

    /// `json.dumps` of a Bash PreToolUse input running `command`.
    fn hook_input(command: &str) -> String {
        let mut input = json::Map::new();
        input.insert("command".into(), Value::Str(command.into()));
        let mut m = json::Map::new();
        m.insert("tool_name".into(), Value::Str("Bash".into()));
        m.insert("cwd".into(), Value::Str("/w".into()));
        m.insert("tool_input".into(), Value::Object(input));
        json::dumps(&Value::Object(m), &json::PY_DEFAULT)
    }

    #[test]
    fn test_hook_drops_an_ask_so_the_command_runs_as_typed() {
        // A hook ask prompts in every mode (bypass, auto), so it is dropped: no output, exit 0.
        let w = setup();
        w.answer_fake(ASK, 0);
        let stdin = hook_input("node --version; bun --version");
        let res = w.run_bin("rtk-hook", &[], &stdin);
        assert_eq!(
            (res.code, res.stdout_text(), res.stderr_text()),
            (0, String::new(), String::new())
        );
        let seen = fs::read_to_string(w.b.tmp.join("stdin-seen")).expect("stdin-seen");
        assert_eq!(seen, stdin, "rtk still read the call");
    }

    #[test]
    fn test_hook_keeps_an_ask_on_a_line_that_mentions_rtk() {
        // `Bash(rtk *)` would allow these as typed, so the ask stands.
        let w = setup();
        w.answer_fake(ASK, 0);
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_hook_keeps_an_ask_on_a_line_that_mentions_rtk",
        );
        assert_eq!(table.len(), 28);
        for row in table {
            let command = row.str("command");
            let res = w.run_bin("rtk-hook", &[], &hook_input(command));
            assert_eq!(
                (res.code, res.stdout_text()),
                (0, format!("{ASK}\n")),
                "{command:?}"
            );
        }
        let escaped = hook_input("rtk x").replace("rtk", "\\u0072tk");
        let res = w.run_bin("rtk-hook", &[], &escaped);
        assert_eq!(
            (res.code, res.stdout_text()),
            (0, format!("{ASK}\n")),
            "a JSON escape counts"
        );
    }

    #[test]
    fn test_hook_drops_an_ask_on_a_dollar_quote_that_cannot_spell_rtk() {
        // An ANSI-C `$'...'` is never a mention: `Bash(rtk *)` never auto-allows one, and a `$'` in a
        // heredoc body is not decoded by the shell. So the ask is dropped and the command runs as typed.
        let w = setup();
        w.answer_fake(ASK, 0);
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_hook_drops_an_ask_on_a_dollar_quote_that_cannot_spell_rtk",
        );
        assert_eq!(table.len(), 22);
        for row in table {
            let command = row.str("command");
            let res = w.run_bin("rtk-hook", &[], &hook_input(command));
            assert_eq!(
                (res.code, res.stdout_text(), res.stderr_text()),
                (0, String::new(), String::new()),
                "{command:?}"
            );
        }
    }

    #[test]
    fn test_hook_drops_an_ask_when_rtk_is_only_inside_a_longer_word() {
        // `rtk` glued after a letter or underscore cannot invoke rtk, so the ask is dropped.
        let w = setup();
        w.answer_fake(ASK, 0);
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_hook_drops_an_ask_when_rtk_is_only_inside_a_longer_word",
        );
        assert_eq!(table.len(), 4);
        for row in table {
            let command = row.str("command");
            let res = w.run_bin("rtk-hook", &[], &hook_input(command));
            assert_eq!(
                (res.code, res.stdout_text(), res.stderr_text()),
                (0, String::new(), String::new()),
                "{command:?}"
            );
        }
    }

    #[test]
    fn test_hook_passes_rewrites_and_denies_through() {
        let w = setup();
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_hook_passes_rewrites_and_denies_through",
        );
        assert_eq!(table.len(), 2);
        for row in table {
            let answer = answer(row.str("answer"));
            w.answer_fake(answer, 0);
            let res = w.run_bin("rtk-hook", &[], &hook_input("git status --short"));
            assert_eq!((res.code, res.stdout_text()), (0, format!("{answer}\n")));
        }
    }

    #[test]
    fn test_hook_keeps_rtk_exit_status_and_output_on_failure() {
        let w = setup();
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_hook_keeps_rtk_exit_status_and_output_on_failure",
        );
        assert_eq!(table.len(), 4);
        for row in table {
            let answer = answer(row.str("answer"));
            let rc: i64 = row.get("rc");
            w.answer_fake(answer, rc);
            let res = w.run_bin("rtk-hook", &[], &hook_input("node --version"));
            assert_eq!(
                (i64::from(res.code), res.stdout_text()),
                (rc, format!("{answer}\n"))
            );
        }
    }

    #[test]
    fn test_hook_silent_rtk_stays_silent() {
        let w = setup();
        w.answer_fake("", 0);
        let res = w.run_bin("rtk-hook", &[], &hook_input("ls; pwd"));
        assert_eq!((res.code, res.stdout_text()), (0, String::new()));
    }

    #[test]
    fn test_hook_drops_the_rewrite_with_the_ask() {
        // Nothing is printed, so the rewrite rtk carried on its ask is never applied.
        let w = setup();
        w.answer_fake(
            &ASK.replace("node --version; bun --version", "rtk git status; rtk ls"),
            0,
        );
        let res = w.run_bin("rtk-hook", &[], &hook_input("git status; ls"));
        assert_eq!(
            (res.code, res.stdout_text(), res.stderr_text()),
            (0, String::new(), String::new())
        );
    }

    #[test]
    fn test_hook_looks_for_rtk_in_the_command_only() {
        // A working folder, transcript or description that says rtk does not bring the prompt back.
        let w = setup();
        w.answer_fake(ASK, 0);
        let stdin = r#"{"session_id": "s", "transcript_path": "/x/rtk/t.jsonl", "cwd": "/x/rtk-proj", "tool_name": "Bash", "tool_input": {"command": "node --version", "description": "rtk check"}}"#;
        let res = w.run_bin("rtk-hook", &[], stdin);
        assert_eq!((res.code, res.stdout_text()), (0, String::new()));
        // Spacing around the key is read; an input with no "command" key is matched whole.
        let table: Vec<Row> = rows(
            "test_rtk_install",
            "test_rtk_install.WrapperTest.test_hook_looks_for_rtk_in_the_command_only",
        );
        assert_eq!(table.len(), 2);
        for row in table {
            let stdin = row.str("stdin");
            let res = w.run_bin("rtk-hook", &[], stdin);
            assert_eq!(
                (res.code, res.stdout_text()),
                (0, format!("{ASK}\n")),
                "{stdin:?}"
            );
        }
    }

    #[test]
    fn test_hook_via_sh_in_exec_form() {
        let w = setup();
        w.b.state_write(&[("rtk_path", w.fake_str())]);
        let env = Env::default()
            .with("PATH", &path_var())
            .with_os("CLAUDE_CONFIG_DIR", &w.b.cfg);
        let script = wrapper("rtk-hook");
        let res = run_program(
            Path::new("/bin/sh"),
            &[script.to_str().expect("utf-8")],
            b"{}",
            &env,
            &w.b.tmp,
        );
        assert_eq!(lines(&res)[0], "args:hook claude");
    }

    /// `os.geteuid() == 0`, without libc: root can read a file with mode 0.
    fn is_root(dir: &Path) -> bool {
        let probe = dir.join(".root-probe");
        fs::write(&probe, b"x").expect("probe");
        set_mode(&probe, 0);
        let readable = fs::read(&probe).is_ok();
        let _ = fs::remove_file(&probe);
        readable
    }

    #[test]
    fn test_unreadable_state_is_quiet_for_the_hook_and_missing_for_rtk() {
        let w = setup();
        if is_root(&w.b.tmp) {
            // Python: skipIf(geteuid() == 0, "root can read any file").
            return;
        }
        w.b.state_write(&[("rtk_path", w.fake_str())]);
        let st = state::path(&w.b.env);
        set_mode(&st, 0);
        let hook = w.run_bin("rtk-hook", &[], "{}");
        let res = w.run_bin("rtk", &["x"], "");
        set_mode(&st, 0o600);
        assert_eq!(
            (hook.code, hook.stdout_text(), hook.stderr_text()),
            (0, String::new(), String::new())
        );
        assert_eq!(res.code, 127);
        assert_eq!(
            res.stderr_text().trim(),
            "rtk is not installed: run /anthology:setup"
        );
    }

    #[test]
    fn test_state_that_is_a_directory_is_missing() {
        let w = setup();
        fs::create_dir_all(state::path(&w.b.env)).expect("mkdir state.json");
        assert_eq!(w.run_bin("rtk", &[], "").code, 127);
        let hook = w.run_bin("rtk-hook", &[], "");
        assert_eq!(
            (hook.code, hook.stdout_text(), hook.stderr_text()),
            (0, String::new(), String::new())
        );
    }

    #[test]
    fn test_hook_missing_exits_0_silently() {
        let w = setup();
        let res = w.run_bin("rtk-hook", &[], r#"{"tool_name":"Bash"}"#);
        assert_eq!(
            (res.code, res.stdout_text(), res.stderr_text()),
            (0, String::new(), String::new())
        );
    }

    #[test]
    fn test_hook_unusable_paths_exit_0_silently() {
        let w = setup();
        let gone = w.b.tmp.join("gone");
        for value in [gone.to_str().expect("utf-8"), "bad\\path", ""] {
            w.b.state_write(&[("rtk_path", value)]);
            let res = w.run_bin("rtk-hook", &[], "{}");
            assert_eq!(
                (res.code, res.stdout_text(), res.stderr_text()),
                (0, String::new(), String::new()),
                "{value}"
            );
        }
    }
}
