//! `anthology guard mask`: stdin in, the masked text out (the `masking.ts` mod runs it on Bash
//! output). Over 4 MiB or not UTF-8 is exit 3 with nothing on stdout, so the caller keeps the
//! text as it was.

use std::path::Path;
use std::time::Duration;

use anthology_core::Env;
use anthology_testkit::run::{Output, run_program_timeout};

const BIN: &str = env!("CARGO_BIN_EXE_anthology");
const MIB: usize = 1024 * 1024;

fn mask(stdin: &[u8]) -> Output {
    run_program_timeout(
        Path::new(BIN),
        &["guard", "mask"],
        stdin,
        &Env::default().with("PATH", "/usr/bin:/bin"),
        Path::new("/"),
        Some(Duration::from_secs(60)),
    )
}

#[test]
fn guard_mask_prints_masked_text() {
    let key = ["AKIA", "IOSFODNN7", "EXAMPLE"].concat();
    let out = mask(format!("id {key}\nnext line\n").as_bytes());
    assert_eq!(out.code, 0, "{}", out.stderr_text());
    assert_eq!(
        out.stdout_text(),
        "id [masked by anthology: aws-access-key]\nnext line\n"
    );
}

#[test]
fn guard_mask_leaves_clean_text_as_it_was() {
    let out = mask(b"total 8\r\ndrwxr-xr-x  2 dev dev 4096 src\r\nno newline");
    assert_eq!(out.code, 0);
    assert_eq!(
        out.stdout_text(),
        "total 8\r\ndrwxr-xr-x  2 dev dev 4096 src\r\nno newline"
    );
    let empty = mask(b"");
    assert_eq!(empty.code, 0);
    assert!(empty.stdout.is_empty());
}

#[test]
fn guard_mask_refuses_over_4_mib() {
    let out = mask(&vec![b'a'; 4 * MIB + 1]);
    assert_eq!(out.code, 3);
    assert!(out.stdout.is_empty());
    // Exactly 4 MiB is still accepted.
    let edge = mask(&vec![b'a'; 4 * MIB]);
    assert_eq!(edge.code, 0, "{}", edge.stderr_text());
    assert_eq!(edge.stdout.len(), 4 * MIB);
}

#[test]
fn guard_mask_refuses_text_that_is_not_utf8() {
    let out = mask(&[b'o', b'k', 0xff, 0xfe, b'\n']);
    assert_eq!(out.code, 3);
    assert!(out.stdout.is_empty());
}

#[test]
fn guard_mask_takes_no_arguments() {
    let out = run_program_timeout(
        Path::new(BIN),
        &["guard", "mask", "extra"],
        b"",
        &Env::default().with("PATH", "/usr/bin:/bin"),
        Path::new("/"),
        Some(Duration::from_secs(60)),
    );
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty());
    assert!(
        out.stderr_text()
            .contains("usage: anthology guard hook [--shim] | anthology guard mask"),
        "{}",
        out.stderr_text()
    );
}
