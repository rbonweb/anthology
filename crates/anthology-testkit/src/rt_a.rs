//! Test helpers of lane RT-A (plan §0.2): layouts for `guardrails_loc` and `compat`.
//!
//! [`materialize`] builds a folder tree from a JSON layout spec. The `runtime.guardrails_loc`
//! difftest surface and its oracle (`port/difftest/oracle/guardrails_loc.py`, which has a twin
//! of this function) both use it, so the two sides see the same files. A spec is a list of
//! entries applied in order; `p` is relative to the root:
//!
//! - `{"t": "dir", "p": P}`: `mkdir -p`;
//! - `{"t": "file", "p": P, "text": S, "enc": E}`: `S` with the placeholders below replaced,
//!   encoded as `E`: `utf8` (default), `utf8-bom`, `utf16le-bom` or `utf16be-bom`; optional
//!   `"tail": HEX` appends raw bytes;
//! - `{"t": "file", "p": P, "hex": HEX}`: raw bytes;
//! - `{"t": "guard", "p": P, "kind": K}`: the guard of a plugin folder, as BOTH Python's
//!   `scripts/guard.py` and Rust's `hooks/guard.sh` (F-6), so the two sides agree. `K` is
//!   `file` (default), `dir` (both are folders), `link` (both link to a regular file) or
//!   `dangling` (both are dangling links);
//! - `{"t": "link", "p": P, "to": T}`: a symlink to `T` (placeholders replaced);
//! - `{"t": "fifo", "p": P}`: a FIFO (`mkfifo`);
//! - `{"t": "chmod", "p": P, "mode": N}`: `chmod`.
//!
//! Every entry creates its missing parent folders. Placeholders: `@ROOT@` is the root,
//! `@PAD@` is `"A"` × the entry's `"pad"`, `@DEEP@` is `"["` × `"deep"` then `"]"` × `"deep"`.

#![forbid(unsafe_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anthology_pycompat::json::Value;

/// `@ROOT@`, `@PAD@` and `@DEEP@` replaced in `s`.
pub fn expand(s: &str, root: &Path, entry: Option<&Value>) -> String {
    let mut out = s.replace("@ROOT@", &root.to_string_lossy());
    let num = |k: &str| {
        entry
            .and_then(Value::as_object)
            .and_then(|o| o.get(k))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0) as usize
    };
    if out.contains("@PAD@") {
        out = out.replace("@PAD@", &"A".repeat(num("pad")));
    }
    if out.contains("@DEEP@") {
        let n = num("deep");
        out = out.replace("@DEEP@", &format!("{}{}", "[".repeat(n), "]".repeat(n)));
    }
    out
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

/// `text` encoded as `enc` (module docs).
pub fn encode(text: &str, enc: &str) -> Vec<u8> {
    let utf16 = |le: bool| {
        let mut out: Vec<u8> = if le { vec![0xff, 0xfe] } else { vec![0xfe, 0xff] };
        for u in text.encode_utf16() {
            out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
        }
        out
    };
    match enc {
        "utf8" => text.as_bytes().to_vec(),
        "utf8-bom" => [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat(),
        "utf16le-bom" => utf16(true),
        "utf16be-bom" => utf16(false),
        other => panic!("rt_a: unknown encoding {other:?}"),
    }
}

fn parent_dirs(path: &Path) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir -p");
    }
}

fn str_field<'a>(entry: &'a Value, k: &str) -> Option<&'a str> {
    entry.as_object()?.get(k)?.as_str()
}

/// Builds the layout `spec` (module docs) under `root`.
pub fn materialize(root: &Path, spec: &[Value]) {
    for entry in spec {
        let kind = str_field(entry, "t").expect("entry t");
        let rel = str_field(entry, "p").expect("entry p");
        assert!(!rel.starts_with('/'), "rt_a: {rel:?} is not relative");
        let path = root.join(rel);
        match kind {
            "dir" => fs::create_dir_all(&path).expect("mkdir"),
            "file" => {
                parent_dirs(&path);
                let mut bytes = match (str_field(entry, "hex"), str_field(entry, "text")) {
                    (Some(h), _) => unhex(h),
                    (None, Some(t)) => encode(
                        &expand(t, root, Some(entry)),
                        str_field(entry, "enc").unwrap_or("utf8"),
                    ),
                    (None, None) => Vec::new(),
                };
                if let Some(tail) = str_field(entry, "tail") {
                    bytes.extend(unhex(tail));
                }
                fs::write(&path, bytes).expect("write");
            }
            "guard" => {
                let how = str_field(entry, "kind").unwrap_or("file");
                for rel in ["scripts/guard.py", "hooks/guard.sh"] {
                    let file = path.join(rel);
                    parent_dirs(&file);
                    match how {
                        "file" => fs::write(&file, b"#!/bin/sh\nexit 2\n").expect("guard"),
                        "dir" => fs::create_dir_all(&file).expect("guard dir"),
                        "link" => {
                            let real = path.join("guard-real");
                            fs::write(&real, b"#!/bin/sh\nexit 2\n").expect("guard real");
                            std::os::unix::fs::symlink(&real, &file).expect("guard link");
                        }
                        "dangling" => std::os::unix::fs::symlink(path.join("no-such-guard"), &file)
                            .expect("guard dangling"),
                        other => panic!("rt_a: unknown guard kind {other:?}"),
                    }
                }
            }
            "link" => {
                parent_dirs(&path);
                let to = expand(str_field(entry, "to").expect("link to"), root, None);
                std::os::unix::fs::symlink(to, &path).expect("symlink");
            }
            "fifo" => {
                parent_dirs(&path);
                let ok = std::process::Command::new("mkfifo")
                    .arg(&path)
                    .status()
                    .is_ok_and(|s| s.success());
                assert!(ok, "mkfifo {}", path.display());
            }
            "chmod" => {
                let mode = entry
                    .as_object()
                    .and_then(|o| o.get("mode"))
                    .and_then(Value::as_i64)
                    .expect("chmod mode") as u32;
                fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
            }
            other => panic!("rt_a: unknown entry {other:?}"),
        }
    }
}

/// Gives every folder under `root` back its owner permissions (after a `chmod` entry), so the
/// tree can be removed.
pub fn unlock(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(e.path());
                }
            }
        }
    }
}

/// A temporary folder at its real path (macOS `/var` → `/private/var`), as the Python tests
/// use `os.path.realpath(tmp)`.
pub struct RealTemp {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
}

impl RealTemp {
    pub fn new() -> RealTemp {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = fs::canonicalize(dir.path()).expect("realpath");
        RealTemp { _dir: dir, path }
    }
}

impl Default for RealTemp {
    fn default() -> RealTemp {
        RealTemp::new()
    }
}

impl Drop for RealTemp {
    fn drop(&mut self) {
        unlock(&self.path);
    }
}

/// `write(path, text)` of `tests/_mem.py`: creates the parent folders.
pub fn write(path: &Path, text: impl AsRef<[u8]>) -> PathBuf {
    parent_dirs(path);
    fs::write(path, text).expect("write");
    path.to_path_buf()
}
