//! Fuzz target `portable_import` (MX1, design §13.5): `portable::load::load_archive` never
//! panics on any file; when an archive loads, `plan_import` writes nothing, and
//! `apply_import` into an empty second machine (whose working folder is not a repo) creates
//! nothing outside `<anthology home>/personal-memory/` (and the folders leading to it).
//!
//! Input: a first byte, then the archive. An even first byte wraps the rest in gzip (stored
//! blocks, so the fuzzer works on the tar stream); an odd one uses the rest as the file.
#![no_main]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anthology_core::Env;
use anthology_core::git::{Git, GitOut};
use anthology_memory::portable::{self, load};
use anthology_pycompat::json::Value;

/// git is missing: the working folder is never a repo, so only personal notes are written.
struct NoGit;

impl Git for NoGit {
    fn run(&self, _env: &Env, _cwd: &Path, _args: &[&str]) -> Option<GitOut> {
        None
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// `data` as one gzip member of stored deflate blocks.
fn gzip_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];
    let mut chunks = data.chunks(0xffff).peekable();
    if chunks.peek().is_none() {
        out.extend([1, 0, 0, 0xff, 0xff]);
    }
    while let Some(chunk) = chunks.next() {
        let last = u8::from(chunks.peek().is_none());
        let len = chunk.len() as u16;
        out.push(last);
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend(chunk);
    }
    out.extend(crc32(data).to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out
}

/// Every path under `root`, symlinks not followed.
fn paths(root: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(p.clone());
            }
            out.insert(p);
        }
    }
    out
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let blob = if mode % 2 == 0 {
        gzip_stored(rest)
    } else {
        rest.to_vec()
    };
    let dir = tempfile::tempdir().expect("a scratch folder");
    let root = dir.path().canonicalize().expect("canonical scratch folder");
    let archive = root.join("archive.tar.gz");
    fs::write(&archive, &blob).expect("write the archive");
    if load::load_archive(&archive).is_err() {
        return;
    }
    let home = root.join("home");
    let cwd = root.join("cwd");
    fs::create_dir(&home).expect("mkdir home");
    fs::create_dir(&cwd).expect("mkdir cwd");
    let env = Env::default()
        .with_os("HOME", &home)
        .with_os("CLAUDE_CONFIG_DIR", home.join(".claude"));
    let before = paths(&root);
    let plan = portable::plan_import(&archive, &cwd, Some("2026-01-01"), &env, &NoGit);
    assert_eq!(paths(&root), before, "plan_import wrote something");
    let Ok(plan) = plan else {
        return;
    };
    let _ = portable::apply_import_env(&Value::Object(plan), &env, &NoGit);
    let personal = home
        .join(".claude")
        .join("anthology")
        .join("personal-memory");
    for p in paths(&root).difference(&before) {
        assert!(
            p.starts_with(&personal) || personal.starts_with(p),
            "apply_import created {} outside {}",
            p.display(),
            personal.display()
        );
    }
});
