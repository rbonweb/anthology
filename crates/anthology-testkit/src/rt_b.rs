//! Test helpers of lane RT-B (plan §0.2): the pieces of `tests/test_rtk_install.py` that RT2's
//! tests share.
//!
//! - [`RtkFakeFetch`]: `FakeFetch`, writing the blob registered for a URL and recording every
//!   call. It implements `anthology_runtime::fetch::Fetch` through a closure the caller wraps
//!   (testkit does not depend on the runtime), see [`RtkFakeFetch::call`].
//! - [`raw_tar_gz`]: a `.tar.gz` with member names written as given (`../rtk`, `/rtk`), which
//!   the `tar` builder refuses to write.
//! - [`rtk_lock`] / [`rtk_lock_value`]: `make_lock(version, blob, repo)`.
//! - [`ConnectProxy`]: a minimal HTTP `CONNECT` proxy that records each tunnel's target.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anthology_core::lock::Lock;
use anthology_core::platform::Target;
use anthology_pycompat::json::{Map, Value};
use flate2::Compression;
use flate2::write::GzEncoder;

use crate::release::{ALL_TARGETS, sha256_hex};

/// `HOST` of the Python tests.
pub const RTK_HOST: &str = "x86_64-unknown-linux-musl";
/// `RTK_BODY` of the Python tests.
pub const RTK_TEST_BODY: &[u8] = b"#!/bin/sh\necho fake-rtk\n";

/// What a raw tar member is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawKind {
    File(Vec<u8>),
    Symlink(String),
    Hardlink(String),
    Dir,
}

/// A gzip-compressed tar whose member names are written byte for byte (ustar, `name` field).
pub fn raw_tar_gz(members: &[(&str, RawKind)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, kind) in members {
        let mut h = tar::Header::new_ustar();
        h.set_uid(0);
        h.set_gid(0);
        h.set_mtime(0);
        let bytes = name.as_bytes();
        assert!(bytes.len() < 100, "raw_tar_gz: name too long");
        {
            let old = h.as_old_mut();
            old.name = [0; 100];
            old.name[..bytes.len()].copy_from_slice(bytes);
        }
        let data: &[u8] = match kind {
            RawKind::File(payload) => {
                h.set_entry_type(tar::EntryType::Regular);
                h.set_mode(0o644);
                h.set_size(payload.len() as u64);
                payload
            }
            RawKind::Symlink(target) | RawKind::Hardlink(target) => {
                h.set_entry_type(if matches!(kind, RawKind::Symlink(_)) {
                    tar::EntryType::Symlink
                } else {
                    tar::EntryType::Link
                });
                h.set_mode(0o777);
                h.set_size(0);
                h.set_link_name(target).expect("raw_tar_gz: link name");
                &[]
            }
            RawKind::Dir => {
                h.set_entry_type(tar::EntryType::Directory);
                h.set_mode(0o755);
                h.set_size(0);
                &[]
            }
        };
        h.set_cksum();
        builder.append(&h, data).expect("raw_tar_gz: append");
    }
    let tar = builder.into_inner().expect("raw_tar_gz: finish");
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar).expect("gzip");
    gz.finish().expect("gzip")
}

/// `tarball()` of the Python tests: `rtk` ([`RTK_TEST_BODY`]) then `LICENSE`.
pub fn rtk_tarball() -> Vec<u8> {
    raw_tar_gz(&[
        ("rtk", RawKind::File(RTK_TEST_BODY.to_vec())),
        ("LICENSE", RawKind::File(b"license text\n".to_vec())),
    ])
}

/// `make_lock(version, blob, repo)` as JSON: every target hashes its own name, except
/// [`RTK_HOST`], which hashes `blob`.
pub fn rtk_lock_value(version: &str, blob: &[u8], repo: &str) -> Value {
    let mut digests = Map::new();
    for t in ALL_TARGETS {
        let d = if t == RTK_HOST {
            sha256_hex(blob)
        } else {
            sha256_hex(t.as_bytes())
        };
        digests.insert(t.to_string(), Value::Str(d));
    }
    let mut m = Map::new();
    m.insert("version".into(), Value::Str(version.into()));
    m.insert("tag".into(), Value::Str(format!("rtk--v{version}")));
    m.insert("repo".into(), Value::Str(repo.into()));
    m.insert("sha256".into(), Value::Object(digests));
    Value::Object(m)
}

/// [`rtk_lock_value`] as a [`Lock`].
pub fn rtk_lock(version: &str, blob: &[u8], repo: &str) -> Lock {
    let mut sha256 = BTreeMap::new();
    for t in ALL_TARGETS {
        let d = if t == RTK_HOST {
            sha256_hex(blob)
        } else {
            sha256_hex(t.as_bytes())
        };
        sha256.insert(Target::from_triple(t).expect("a target"), d);
    }
    Lock {
        version: version.into(),
        tag: format!("rtk--v{version}"),
        repo: repo.into(),
        sha256,
    }
}

/// `url_for(lock, target)`.
pub fn rtk_url_for(lock: &Lock, target: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/rtk-{target}.tar.gz",
        lock.repo, lock.tag
    )
}

/// `FakeFetch(blobs, error)`: the state behind a fake fetch (module docs).
#[derive(Default)]
pub struct RtkFakeFetch {
    pub blobs: HashMap<String, Vec<u8>>,
    /// `(type name, message)` of the error every call raises, if any.
    pub error: Option<(&'static str, String)>,
    pub calls: Mutex<Vec<(String, f64)>>,
}

/// What one fake call does.
pub enum FakeAnswer<'a> {
    Write(&'a [u8]),
    Raise(&'static str, String),
}

impl RtkFakeFetch {
    /// A fetch that serves `blob` at `url`.
    pub fn serving(url: &str, blob: &[u8]) -> RtkFakeFetch {
        let mut f = RtkFakeFetch::default();
        f.blobs.insert(url.to_string(), blob.to_vec());
        f
    }

    /// A fetch whose every call raises `type_name: message`.
    pub fn failing(type_name: &'static str, message: &str) -> RtkFakeFetch {
        RtkFakeFetch {
            error: Some((type_name, message.to_string())),
            ..RtkFakeFetch::default()
        }
    }

    /// Records the call and says what to do (`self.blobs[url]` raises `KeyError`).
    pub fn call(&self, url: &str, timeout: f64) -> FakeAnswer<'_> {
        self.calls
            .lock()
            .expect("calls")
            .push((url.to_string(), timeout));
        if let Some((t, m)) = &self.error {
            return FakeAnswer::Raise(t, m.clone());
        }
        match self.blobs.get(url) {
            Some(b) => FakeAnswer::Write(b),
            None => FakeAnswer::Raise("KeyError", format!("'{url}'")),
        }
    }

    /// The calls so far.
    pub fn calls(&self) -> Vec<(String, f64)> {
        self.calls.lock().expect("calls").clone()
    }
}

/// A minimal HTTP `CONNECT` proxy on `127.0.0.1:<free port>` (module docs). Stops on drop.
pub struct ConnectProxy {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    targets: Arc<Mutex<Vec<String>>>,
}

impl ConnectProxy {
    /// Starts the proxy.
    pub fn start() -> ConnectProxy {
        let listener = TcpListener::bind("127.0.0.1:0").expect("proxy: bind");
        listener.set_nonblocking(true).expect("proxy: nonblocking");
        let addr = listener.local_addr().expect("proxy: addr");
        let stop = Arc::new(AtomicBool::new(false));
        let targets = Arc::new(Mutex::new(Vec::new()));
        let (s, t) = (Arc::clone(&stop), Arc::clone(&targets));
        thread::spawn(move || {
            while !s.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let t = Arc::clone(&t);
                        thread::spawn(move || {
                            let _ = tunnel(conn, &t);
                        });
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        ConnectProxy {
            addr,
            stop,
            targets,
        }
    }

    /// `http://127.0.0.1:<port>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The `CONNECT` targets so far.
    pub fn targets(&self) -> Vec<String> {
        self.targets.lock().expect("targets").clone()
    }
}

impl Drop for ConnectProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn tunnel(conn: TcpStream, targets: &Mutex<Vec<String>>) -> io::Result<()> {
    conn.set_nonblocking(false)?;
    conn.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut parts = first.split_whitespace();
    let (Some("CONNECT"), Some(target)) = (parts.next(), parts.next()) else {
        let mut c = conn;
        c.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")?;
        return Ok(());
    };
    targets.lock().expect("targets").push(target.to_string());
    let upstream = TcpStream::connect(target)?;
    let mut client = conn;
    client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
    // Bytes the reader already buffered belong to the tunnel.
    let buffered = reader.buffer().to_vec();
    let mut up_w = upstream.try_clone()?;
    up_w.write_all(&buffered)?;
    let mut up_r = upstream;
    let mut client_r = reader.into_inner();
    let mut client_w = client;
    let a = thread::spawn(move || {
        let _ = io::copy(&mut client_r, &mut up_w);
        let _ = up_w.shutdown(Shutdown::Write);
    });
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match up_r.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if client_w.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = client_w.shutdown(Shutdown::Both);
    let _ = a.join();
    Ok(())
}
