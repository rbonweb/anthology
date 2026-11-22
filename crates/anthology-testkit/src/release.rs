//! Port of `tests/_atl.py` (F4, m-13): fake release tarballs and locks for the tests that drive
//! the real installers.
//!
//! | `_atl.py` | here |
//! |---|---|
//! | `LINUX`, `TARGET`, `ALL_TARGETS`, `VERSION`, `ATL_BODY` | the same constants |
//! | `tarball(body)` | [`tarball`]; [`fake_release`] for another member name, with its sha256 |
//! | `write_lock(path, blob, version)` | [`write_lock`] (returns the [`Lock`]) |
//! | `FakeFetch` | not here: it implements RT2's `Fetch` trait (GC-19 `atl_install` deps), so RT2 adds it next to that trait |
//!
//! A release tarball holds the member (`atl`, or `anthology` for the binary release) and then
//! `LICENSE` (`license\n`), each a regular file with mode `0644`, uid and gid 0 and mtime 0, in
//! the ustar layout Python's `tarfile` writes for such names, gzip-compressed. [`tar_gz`] builds
//! any other layout (symlink members, folders, extra or missing members) for the failure tests.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anthology_core::lock::Lock;
use anthology_core::platform::Target;
use anthology_pycompat::json::{self, INDENT2, Map, Value};
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};

/// `platform.system()`, `platform.machine()` of the Linux test machine.
pub const LINUX: (&str, &str) = ("Linux", "x86_64");
/// The triple of [`LINUX`].
pub const TARGET: &str = "x86_64-unknown-linux-musl";
/// Every release target, in the lock's order.
pub const ALL_TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
];
/// The atl version of the fake locks.
pub const VERSION: &str = "1.2.1-anthology.1";
/// The fake `atl` binary.
pub const ATL_BODY: &[u8] = b"#!/bin/sh\necho fake-atl\n";
/// The `LICENSE` member's content.
pub const LICENSE_BODY: &[u8] = b"license\n";

/// One member of a [`tar_gz`] archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TarEntry {
    File {
        name: String,
        bytes: Vec<u8>,
        mode: u32,
    },
    Dir {
        name: String,
        mode: u32,
    },
    Symlink {
        name: String,
        target: String,
    },
    Hardlink {
        name: String,
        target: String,
    },
}

impl TarEntry {
    /// A regular file with mode `0644`.
    pub fn file(name: &str, bytes: &[u8]) -> TarEntry {
        TarEntry::File {
            name: name.into(),
            bytes: bytes.to_vec(),
            mode: 0o644,
        }
    }
}

/// A gzip-compressed tar of `entries`, in order (uid, gid and mtime 0; gzip mtime 0).
pub fn tar_gz(entries: &[TarEntry]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for entry in entries {
        let mut h = tar::Header::new_ustar();
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        let (name, data): (&str, &[u8]) = match entry {
            TarEntry::File { name, bytes, mode } => {
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(*mode);
                h.set_size(bytes.len() as u64);
                (name, bytes)
            }
            TarEntry::Dir { name, mode } => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(*mode);
                h.set_size(0);
                (name, &[])
            }
            TarEntry::Symlink { name, target } | TarEntry::Hardlink { name, target } => {
                h.set_entry_type(if matches!(entry, TarEntry::Symlink { .. }) {
                    tar::EntryType::Symlink
                } else {
                    tar::EntryType::Link
                });
                h.set_mode(0o777);
                h.set_size(0);
                set_raw(&mut h.as_old_mut().linkname, target.as_bytes(), "link name");
                (name, &[])
            }
        };
        set_name(&mut h, name.as_bytes());
        h.set_cksum();
        builder.append(&h, data).expect("tar: append");
    }
    let tar = builder.into_inner().expect("tar: finish");
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar).expect("gzip");
    gz.finish().expect("gzip")
}

/// Copies `b` into a NUL-padded header field; panics when it does not fit.
fn set_raw(field: &mut [u8], b: &[u8], what: &str) {
    assert!(
        b.len() <= field.len(),
        "tar: the {what} is longer than {} bytes",
        field.len()
    );
    field.fill(0);
    field[..b.len()].copy_from_slice(b);
}

/// Writes a member name as given (`../atl`, `/atl` included: the `tar` crate's `set_path`
/// refuses those). A name over 100 bytes is split at a `/` into the ustar prefix (155 bytes)
/// and name (100 bytes), as Python's `tarfile` does for the ustar format.
fn set_name(h: &mut tar::Header, name: &[u8]) {
    if name.len() <= 100 {
        set_raw(&mut h.as_old_mut().name, name, "member name");
        return;
    }
    let split = (0..name.len())
        .rev()
        .find(|&i| name[i] == b'/' && i <= 155 && name.len() - i - 1 <= 100 && i > 0)
        .unwrap_or_else(|| {
            panic!(
                "tar: {:?} does not fit a ustar header",
                String::from_utf8_lossy(name)
            )
        });
    let ustar = h.as_ustar_mut().expect("a ustar header");
    set_raw(&mut ustar.prefix, &name[..split], "name prefix");
    set_raw(&mut ustar.name, &name[split + 1..], "member name");
}

/// `tarball(body)`: an atl release tarball, `atl` then `LICENSE`.
pub fn tarball(body: &[u8]) -> Vec<u8> {
    tar_gz(&[
        TarEntry::file("atl", body),
        TarEntry::file("LICENSE", LICENSE_BODY),
    ])
}

/// A release tarball for `target` holding `member` (`atl` or `anthology`) with `bytes`, then
/// `LICENSE`; and its sha256 (lowercase hex), as a lock records it. The bytes are the same for
/// every target, as Python's `tarball()` is; [`asset_name`] gives the file name.
pub fn fake_release(target: Target, member: &str, bytes: &[u8]) -> (Vec<u8>, String) {
    let _ = target;
    let blob = tar_gz(&[
        TarEntry::file(member, bytes),
        TarEntry::file("LICENSE", LICENSE_BODY),
    ]);
    let digest = sha256_hex(&blob);
    (blob, digest)
}

/// The release asset name: `<member>-<triple>.tar.gz` (`atl_install.asset_url`).
pub fn asset_name(member: &str, target: Target) -> String {
    format!("{member}-{}.tar.gz", target.triple())
}

/// The lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `write_lock(path, blob, version)`: a valid `atl.lock` whose [`TARGET`] digest is `blob`'s
/// and whose other digests are the sha256 of each target's name; repo `example/anthology`.
/// Written as `json.dumps(lock, indent=2) + "\n"` (parent folders created); returns the lock.
pub fn write_lock(path: &Path, blob: &[u8], version: &str) -> Lock {
    let mut digests = Map::new();
    let mut sha256 = BTreeMap::new();
    for t in ALL_TARGETS {
        let d = if t == TARGET {
            sha256_hex(blob)
        } else {
            sha256_hex(t.as_bytes())
        };
        digests.insert(t.to_string(), Value::Str(d.clone()));
        sha256.insert(Target::from_triple(t).expect("a release target"), d);
    }
    let tag = format!("atl--v{version}");
    let repo = "example/anthology";
    let mut lock = Map::new();
    lock.insert("version".into(), Value::Str(version.into()));
    lock.insert("tag".into(), Value::Str(tag.clone()));
    lock.insert("repo".into(), Value::Str(repo.into()));
    lock.insert("sha256".into(), Value::Object(digests));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("write_lock: mkdir");
    }
    let text = json::dumps(&Value::Object(lock), &INDENT2) + "\n";
    std::fs::write(path, text).expect("write_lock: write");
    Lock {
        version: version.into(),
        tag,
        repo: repo.into(),
        sha256,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::Read;

    fn members(blob: &[u8]) -> Vec<(String, tar::EntryType, u32, u64, u64, Vec<u8>)> {
        let mut out = Vec::new();
        let mut ar = tar::Archive::new(GzDecoder::new(blob));
        for e in ar.entries().expect("entries") {
            let mut e = e.expect("entry");
            let h = e.header().clone();
            let mut data = Vec::new();
            e.read_to_end(&mut data).expect("read");
            out.push((
                e.path().expect("path").to_string_lossy().into_owned(),
                h.entry_type(),
                h.mode().expect("mode"),
                h.uid().expect("uid"),
                h.mtime().expect("mtime"),
                data,
            ));
        }
        out
    }

    #[test]
    fn tarball_holds_atl_then_license() {
        let got = members(&tarball(ATL_BODY));
        assert_eq!(
            got,
            [
                (
                    "atl".into(),
                    tar::EntryType::Regular,
                    0o644,
                    0,
                    0,
                    ATL_BODY.to_vec()
                ),
                (
                    "LICENSE".into(),
                    tar::EntryType::Regular,
                    0o644,
                    0,
                    0,
                    b"license\n".to_vec()
                ),
            ]
        );
    }

    #[test]
    fn fake_release_names_the_member_and_hashes_the_blob() {
        let (blob, digest) = fake_release(Target::LinuxArm64Musl, "anthology", b"\x7fELF");
        assert_eq!(digest, sha256_hex(&blob));
        assert_eq!(digest.len(), 64);
        let got = members(&blob);
        assert_eq!(got[0].0, "anthology");
        assert_eq!(got[0].5, b"\x7fELF");
        assert_eq!(got[1].0, "LICENSE");
        // Deterministic: the same input gives the same bytes.
        assert_eq!(
            fake_release(Target::AppleArm64, "anthology", b"\x7fELF").0,
            blob
        );
        assert_eq!(
            asset_name("atl", Target::LinuxX86_64Musl),
            "atl-x86_64-unknown-linux-musl.tar.gz"
        );
    }

    #[test]
    fn tar_gz_builds_links_and_folders() {
        let blob = tar_gz(&[
            TarEntry::Dir {
                name: "d".into(),
                mode: 0o755,
            },
            TarEntry::Symlink {
                name: "atl".into(),
                target: "/etc/passwd".into(),
            },
            TarEntry::Hardlink {
                name: "h".into(),
                target: "atl".into(),
            },
        ]);
        let got = members(&blob);
        assert_eq!(got[0].1, tar::EntryType::Directory);
        assert_eq!(got[1].1, tar::EntryType::Symlink);
        assert_eq!(got[2].1, tar::EntryType::Link);
    }

    #[test]
    fn tar_gz_writes_hostile_and_long_names_as_given() {
        let long = format!("{}/{}", "d".repeat(120), "f".repeat(90));
        let blob = tar_gz(&[
            TarEntry::file("../atl", b"up"),
            TarEntry::file("/atl", b"abs"),
            TarEntry::file("bin/atl", b"nested"),
            TarEntry::file("./atl", b"dot"),
            TarEntry::file(&long, b"long"),
            TarEntry::Symlink {
                name: "l".into(),
                target: "../../etc/passwd".into(),
            },
        ]);
        // The raw header names, as an extractor sees them (tar's reader would also refuse
        // nothing here: it reports paths, unpack is what checks them).
        let mut names = Vec::new();
        let mut ar = tar::Archive::new(GzDecoder::new(&blob[..]));
        for e in ar.entries().expect("entries") {
            let e = e.expect("entry");
            names.push(String::from_utf8_lossy(&e.path_bytes()).into_owned());
            if e.header().entry_type() == tar::EntryType::Symlink {
                let link = e.link_name_bytes().expect("link");
                assert_eq!(&*link, b"../../etc/passwd");
            }
        }
        assert_eq!(
            names,
            ["../atl", "/atl", "bin/atl", "./atl", long.as_str(), "l"]
        );
        let too_long =
            std::panic::catch_unwind(|| tar_gz(&[TarEntry::file(&"x".repeat(101), b"")]));
        assert!(too_long.is_err());
    }

    #[test]
    fn sha256_hex_is_the_known_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn write_lock_writes_a_valid_atl_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("plugin/atl.lock");
        let blob = tarball(ATL_BODY);
        let lock = write_lock(&path, &blob, VERSION);
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.starts_with("{\n  \"version\": \"1.2.1-anthology.1\",\n  \"tag\": \"atl--v1.2.1-anthology.1\",\n  \"repo\": \"example/anthology\",\n  \"sha256\": {\n    \"aarch64-apple-darwin\": \""));
        assert!(text.ends_with("}\n}\n"));
        let parsed = anthology_core::lock::parse(&text, "atl--v").expect("valid");
        assert_eq!(parsed, lock);
        assert_eq!(lock.sha256[&Target::LinuxX86_64Musl], sha256_hex(&blob));
        assert_eq!(
            lock.sha256[&Target::AppleArm64],
            sha256_hex(b"aarch64-apple-darwin")
        );
        assert_eq!(LINUX, ("Linux", "x86_64"));
        assert_eq!(
            anthology_core::platform::detect_with(LINUX).map(|t| t.triple()),
            Ok(TARGET)
        );
    }
}
