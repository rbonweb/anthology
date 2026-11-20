//! `fsx` against `memory_sync.py`'s helpers: refusals, errnos, cleanup and the fault hook.

use super::*;
use std::os::unix::fs::{MetadataExt, symlink};
use std::process::Command;

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn mode(p: &Path) -> u32 {
    fs::symlink_metadata(p).expect("lstat").mode() & 0o7777
}

fn errno_of(e: &PyExc) -> i32 {
    match e {
        PyExc::OSError { errno, .. } => *errno,
        other => panic!("not an OSError: {other:?}"),
    }
}

fn unsafe_msg(e: &PyExc) -> &str {
    match e {
        PyExc::Other("UnsafeTarget", m) => m,
        other => panic!("not UnsafeTarget: {other:?}"),
    }
}

// ---- atomic_write ---------------------------------------------------------------------------

#[test]
fn atomic_write_creates_parents_sets_the_mode_and_leaves_no_temp() {
    let d = tmp();
    let p = d.path().join("a/b/c.json");
    atomic_write(&p, b"x\n", 0o644, ".tmp-", ".part").expect("write");
    assert_eq!(fs::read(&p).expect("read"), b"x\n");
    assert_eq!(mode(&p), 0o644);
    assert_eq!(names(&d.path().join("a/b")), ["c.json"]);
    atomic_write(&p, b"y", 0o600, ".tmp-", ".part").expect("rewrite");
    assert_eq!(fs::read(&p).expect("read"), b"y");
    assert_eq!(mode(&p), 0o600);
}

#[test]
fn atomic_write_replaces_a_symlink_instead_of_writing_through_it() {
    let d = tmp();
    let target = d.path().join("target");
    fs::write(&target, b"keep").expect("write");
    let link = d.path().join("link");
    symlink(&target, &link).expect("symlink");
    atomic_write(&link, b"new", 0o644, ".tmp-", ".part").expect("write");
    assert_eq!(fs::read(&target).expect("read"), b"keep");
    assert!(fs::symlink_metadata(&link).expect("lstat").is_file());
    assert_eq!(fs::read(&link).expect("read"), b"new");
}

#[test]
fn a_failed_step_unlinks_the_temp_file_and_keeps_the_target() {
    for (op, errno) in [
        ("write", libc::ENOSPC),
        ("chmod", libc::EPERM),
        ("rename", libc::EXDEV),
    ] {
        let d = tmp();
        let p = d.path().join("f");
        fs::write(&p, b"old").expect("write");
        fault::fail_next(op, errno);
        let e = atomic_write(&p, b"new", 0o644, ".tmp-", ".part").expect_err(op);
        assert_eq!(errno_of(&e), errno, "{op}");
        assert_eq!(names(d.path()), ["f"], "{op}: the temp file is gone");
        assert_eq!(fs::read(&p).expect("read"), b"old", "{op}");
        if op != "write" {
            // chmod and os.replace name the temp file, as Python's exception does.
            let PyExc::OSError {
                filename: Some(f), ..
            } = &e
            else {
                panic!("{op}: no filename in {e:?}");
            };
            let name = Path::new(f).file_name().expect("name").to_string_lossy();
            assert!(name.starts_with(".tmp-") && name.ends_with(".part"), "{f}");
            assert_eq!(name.len(), ".tmp-".len() + 8 + ".part".len(), "{f}");
        }
    }
}

#[test]
fn a_failed_cleanup_is_ignored_and_the_first_error_returned() {
    let d = tmp();
    let p = d.path().join("f");
    fault::fail_next("rename", libc::EACCES);
    fault::fail_next("unlink", libc::EBUSY);
    let e = atomic_write(&p, b"new", 0o644, ".tmp-", ".part").expect_err("rename");
    assert_eq!(errno_of(&e), libc::EACCES);
    assert_eq!(
        names(d.path()).len(),
        1,
        "the temp file stays: {:?}",
        names(d.path())
    );
}

#[test]
fn mkstemp_retries_a_taken_name_and_reports_other_open_errors() {
    let d = tmp();
    let p = d.path().join("f");
    fault::fail_next("open", libc::EEXIST);
    atomic_write(&p, b"1", 0o644, ".tmp-", ".part").expect("retried");
    fault::fail_next("open", libc::EACCES);
    let e = atomic_write(&p, b"2", 0o644, ".tmp-", ".part").expect_err("open");
    assert_eq!(errno_of(&e), libc::EACCES);
    assert_eq!(fs::read(&p).expect("read"), b"1");
}

#[test]
fn mkdir_errors_name_the_folder_that_failed_as_pathlib_does() {
    let d = tmp();
    let file = d.path().join("file");
    fs::write(&file, b"").expect("write");
    // The parent is a file: os.mkdir gives EEXIST and it is not a folder.
    let e = atomic_write(&file.join("x"), b"", 0o644, ".t", "").expect_err("eexist");
    assert_eq!(
        e,
        PyExc::OSError {
            errno: libc::EEXIST,
            filename: Some(path_str(&file))
        }
    );
    // A deeper folder under the file: ENOTDIR on the first mkdir.
    let e = atomic_write(&file.join("sub/x"), b"", 0o644, ".t", "").expect_err("enotdir");
    assert_eq!(
        e,
        PyExc::OSError {
            errno: libc::ENOTDIR,
            filename: Some(path_str(&file.join("sub")))
        }
    );
    // An existing folder (and a symlink to one) is fine.
    let link = d.path().join("dirlink");
    symlink(d.path(), &link).expect("symlink");
    mkdir_parents(&link).expect("a symlink to a folder exists");
    fault::fail_next("mkdir", libc::EROFS);
    let e = mkdir_parents(&d.path().join("new")).expect_err("erofs");
    assert_eq!(errno_of(&e), libc::EROFS);
}

#[test]
fn the_temp_name_is_mkstemps_alphabet() {
    for _ in 0..200 {
        let n = random_name();
        assert_eq!(n.len(), 8);
        assert!(n.bytes().all(|b| TEMP_CHARS.contains(&b)), "{n}");
    }
}

// ---- read_regular ---------------------------------------------------------------------------

#[test]
fn read_regular_reads_caps_and_treats_absent_as_empty() {
    let d = tmp();
    let p = d.path().join("f");
    assert_eq!(read_regular(&p, None).expect("absent"), b"");
    fs::write(&p, b"hello").expect("write");
    assert_eq!(read_regular(&p, None).expect("read"), b"hello");
    assert_eq!(read_regular(&p, Some(3)).expect("capped"), b"hel");
    assert_eq!(read_regular(&p, Some(99)).expect("short"), b"hello");
}

#[test]
fn read_regular_refuses_links_folders_and_fifos() {
    let d = tmp();
    let file = d.path().join("f");
    fs::write(&file, b"x").expect("write");
    let link = d.path().join("link");
    symlink(&file, &link).expect("symlink");
    let dangling = d.path().join("dangling");
    symlink(d.path().join("nowhere"), &dangling).expect("symlink");
    let fifo = d.path().join("fifo");
    let ok = Command::new("mkfifo").arg(&fifo).status().expect("mkfifo");
    assert!(ok.success());
    for p in [&link, &dangling, &fifo, &d.path().to_path_buf()] {
        let e = read_regular(p, None).expect_err("refused");
        assert_eq!(
            unsafe_msg(&e),
            format!("refusing to use {}: it is not a regular file", p.display())
        );
    }
}

#[test]
fn the_open_refuses_a_symlink_with_eloop_when_the_lstat_check_is_raced() {
    let d = tmp();
    let link = d.path().join("link");
    symlink(d.path().join("t"), &link).expect("symlink");
    fs::write(d.path().join("t"), b"x").expect("write");
    let e = open_read_nofollow(&link).expect_err("O_NOFOLLOW");
    assert_eq!(e.raw_os_error(), Some(libc::ELOOP));
}

#[test]
fn an_open_failure_is_unsafe_target_with_the_strerror_text() {
    let d = tmp();
    let p = d.path().join("f");
    fs::write(&p, b"x").expect("write");
    fault::fail_next("open", libc::EACCES);
    let e = read_regular(&p, None).expect_err("open");
    let text = strerror(io::Error::from_raw_os_error(libc::EACCES));
    assert_eq!(
        unsafe_msg(&e),
        format!("refusing to use {}: {text}", p.display())
    );
    assert!(!text.starts_with("[Errno"), "{text}");
    fault::fail_next("read", libc::EIO);
    assert_eq!(
        errno_of(&read_regular(&p, None).expect_err("read")),
        libc::EIO
    );
}

// ---- create_excl ----------------------------------------------------------------------------

#[test]
fn create_excl_creates_once_and_refuses_existing_paths_and_symlinks() {
    let d = tmp();
    let p = d.path().join("note.md");
    create_excl(&p, b"body", 0o644, true).expect("create");
    assert_eq!(fs::read(&p).expect("read"), b"body");
    assert_eq!(mode(&p) & !0o644, 0, "at most 0644 (umask applies)");
    let e = create_excl(&p, b"again", 0o644, true).expect_err("exists");
    assert_eq!(
        e,
        PyExc::OSError {
            errno: libc::EEXIST,
            filename: Some(path_str(&p))
        }
    );
    assert_eq!(e.type_name(), "FileExistsError");
    let link = d.path().join("link.md");
    symlink(d.path().join("elsewhere"), &link).expect("symlink");
    for nofollow in [true, false] {
        let e = create_excl(&link, b"x", 0o644, nofollow).expect_err("symlink");
        assert_eq!(errno_of(&e), libc::EEXIST);
    }
    assert!(
        !d.path().join("elsewhere").exists(),
        "never written through"
    );
}

#[test]
fn create_excl_unlinks_the_file_when_the_write_fails() {
    let d = tmp();
    let p = d.path().join("note.md");
    fault::fail_next("write", libc::ENOSPC);
    let e = create_excl(&p, b"body", 0o644, true).expect_err("write");
    assert_eq!(errno_of(&e), libc::ENOSPC);
    assert!(!p.exists());
}

// ---- append_line ----------------------------------------------------------------------------

#[test]
fn append_line_adds_a_missing_newline_first() {
    let d = tmp();
    for (before, after) in [
        (None, "line\n"),
        (Some(""), "line\n"),
        (Some("a"), "a\nline\n"),
        (Some("a\n"), "a\nline\n"),
        (Some("a\n\n"), "a\n\nline\n"),
        (Some("é"), "é\nline\n"),
    ] {
        let p = d.path().join("sub/exclude");
        let _ = fs::remove_file(&p);
        if let Some(b) = before {
            fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            fs::write(&p, b).expect("write");
        }
        append_line(&p, "line").expect("append");
        assert_eq!(fs::read_to_string(&p).expect("read"), after, "{before:?}");
    }
    let p = d.path().join("new/deeper/f");
    append_line(&p, "x").expect("creates parents");
    assert_eq!(mode(&p) & !0o644, 0);
}

#[test]
fn append_line_refuses_symlinks_and_non_regular_files() {
    let d = tmp();
    let target = d.path().join("t");
    fs::write(&target, b"keep").expect("write");
    let link = d.path().join("link");
    symlink(&target, &link).expect("symlink");
    for p in [&link, &d.path().to_path_buf()] {
        let e = append_line(p, "x").expect_err("refused");
        assert_eq!(
            unsafe_msg(&e),
            format!(
                "refusing to write {}: it is not a regular file",
                p.display()
            )
        );
    }
    assert_eq!(fs::read(&target).expect("read"), b"keep");
    fault::fail_next("open", libc::ELOOP);
    let e = append_line(&d.path().join("raced"), "x").expect_err("raced");
    let text = strerror(io::Error::from_raw_os_error(libc::ELOOP));
    assert_eq!(
        unsafe_msg(&e),
        format!(
            "refusing to write {}: {text}",
            d.path().join("raced").display()
        )
    );
}

// ---- is_real_dir and the hook -----------------------------------------------------------------

#[test]
fn is_real_dir_is_a_folder_that_is_not_a_link() {
    let d = tmp();
    let file = d.path().join("f");
    fs::write(&file, b"").expect("write");
    let link = d.path().join("l");
    symlink(d.path(), &link).expect("symlink");
    assert!(is_real_dir(d.path()));
    assert!(!is_real_dir(&link));
    assert!(!is_real_dir(&file));
    assert!(!is_real_dir(&d.path().join("absent")));
}

#[test]
fn faults_are_per_thread_and_fire_once() {
    let d = tmp();
    let p = d.path().join("f");
    std::thread::spawn(|| fault::fail_next("write", libc::EIO))
        .join()
        .expect("thread");
    atomic_write(&p, b"1", 0o644, ".t", "").expect("another thread's fault is not ours");
    fault::fail_next("write", libc::EIO);
    assert!(atomic_write(&p, b"2", 0o644, ".t", "").is_err());
    atomic_write(&p, b"3", 0o644, ".t", "").expect("it fired once");
}

#[test]
#[should_panic(expected = "unknown op")]
fn an_unknown_fault_op_panics() {
    fault::fail_next("opne", libc::EIO);
}
