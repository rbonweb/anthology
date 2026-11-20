//! Test HTTP and TLS servers (F4, plan §3.7; used by BS1's real downloaders and RT2's fetch).
//!
//! [`TestServer::http`] and [`TestServer::https`] listen on `127.0.0.1:<free port>` and serve
//! each connection on its own thread, one request per connection (`Connection: close`). The
//! TLS mode uses rustls (ring; TLS 1.2 and 1.3) with the committed test certificates in
//! `fixtures/tls/` (made by its `generate.sh`): a test CA, and a server certificate for
//! `localhost`, `127.0.0.1` and `::1`. The server's key is committed as DER only.
//!
//! Clients trust the CA through:
//! - [`TestServer::ca_pem_path`], a PEM file written for the server: `curl --cacert`,
//!   `SSL_CERT_FILE` (curl, GNU wget), `NODE_EXTRA_CA_CERTS` (node), or BusyBox's CA path;
//! - [`TestServer::ca_der`], for a custom rustls/ureq root store.
//!
//! Do not rely on a platform verifier: the certificates are valid for 100 years, and Apple's
//! refuses a server certificate valid for more than 825 days.
//!
//! [`TestServer::route`] maps a path (the request target, or the target without its query) to a
//! [`Route`]: a status, headers, a body and a [`Mode`]:
//! - [`Mode::Normal`]: `Content-Length`, then the body;
//! - [`Mode::Slow`]: `Content-Length`, then the body in chunks with a delay before each;
//! - [`Mode::NeverEnding`]: no `Content-Length`; the body (or zeros), repeated in chunks until
//!   the client goes away or the server is dropped;
//! - [`Route::redirect`]: `302` with an absolute `Location` (http or https).
//!
//! A path with no route gets `404`. Every request is recorded ([`TestServer::requests`]): its
//! method, target and headers. Dropping the server stops accepting at once and ends every
//! streaming response at its next chunk.
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// The test CA certificate, DER.
pub const CA_DER: &[u8] = include_bytes!("../fixtures/tls/ca.der");
/// The test CA certificate, PEM.
pub const CA_PEM: &str = include_str!("../fixtures/tls/ca.pem");
/// The server certificate (`localhost`, `127.0.0.1`, `::1`), DER.
pub const LEAF_DER: &[u8] = include_bytes!("../fixtures/tls/leaf.der");
const LEAF_KEY_DER: &[u8] = include_bytes!("../fixtures/tls/leaf-key.der");

/// How long a connection may sit idle in a read or a write before the server gives it up.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// How a route's body is sent (module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Slow { chunk: usize, delay: Duration },
    NeverEnding { chunk: usize, delay: Duration },
}

/// A response (module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub mode: Mode,
}

impl Route {
    /// `200` with `body`.
    pub fn ok(body: &[u8]) -> Route {
        Route::with_status(200, body)
    }

    /// `status` with `body`.
    pub fn with_status(status: u16, body: &[u8]) -> Route {
        Route {
            status,
            headers: Vec::new(),
            body: body.to_vec(),
            mode: Mode::Normal,
        }
    }

    /// `302` to `location`, an absolute URL.
    pub fn redirect(location: &str) -> Route {
        Route::with_status(302, b"").header("Location", location)
    }

    /// This route with one more header.
    pub fn header(mut self, name: &str, value: &str) -> Route {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// This route sent `chunk` bytes at a time, waiting `delay` before each chunk.
    pub fn slow(mut self, chunk: usize, delay: Duration) -> Route {
        self.mode = Mode::Slow {
            chunk: chunk.max(1),
            delay,
        };
        self
    }

    /// This route sent forever: its body (zeros when empty) repeated, 16 KiB at a time with
    /// 10 ms between chunks.
    pub fn never_ending(self) -> Route {
        self.never_ending_with(16 << 10, Duration::from_millis(10))
    }

    /// [`Route::never_ending`] with another chunk size and delay.
    pub fn never_ending_with(mut self, chunk: usize, delay: Duration) -> Route {
        self.mode = Mode::NeverEnding {
            chunk: chunk.max(1),
            delay,
        };
        self
    }
}

/// A request the server received.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    /// The request target as sent (path and query).
    pub path: String,
    /// The headers in order, as sent.
    pub headers: Vec<(String, String)>,
}

impl Request {
    /// The first header named `name` (any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

struct Shared {
    routes: Mutex<HashMap<String, Route>>,
    requests: Mutex<Vec<Request>>,
    stop: AtomicBool,
    tls: Option<Arc<rustls::ServerConfig>>,
}

/// A local HTTP or HTTPS server (module docs). Stops on drop.
pub struct TestServer {
    addr: SocketAddr,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
    ca_dir: tempfile::TempDir,
}

impl TestServer {
    /// A plain HTTP server.
    pub fn http() -> TestServer {
        TestServer::start(None)
    }

    /// An HTTPS server with the test certificates.
    pub fn https() -> TestServer {
        TestServer::start(Some(Arc::new(server_config())))
    }

    fn start(tls: Option<Arc<rustls::ServerConfig>>) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("TestServer: bind");
        listener
            .set_nonblocking(true)
            .expect("TestServer: nonblocking");
        let addr = listener.local_addr().expect("TestServer: addr");
        let shared = Arc::new(Shared {
            routes: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            stop: AtomicBool::new(false),
            tls,
        });
        let ca_dir = tempfile::tempdir().expect("TestServer: tempdir");
        std::fs::write(ca_dir.path().join("ca.pem"), CA_PEM).expect("TestServer: ca.pem");
        let worker = Arc::clone(&shared);
        let accept = thread::spawn(move || accept_loop(&listener, &worker));
        TestServer {
            addr,
            shared,
            accept: Some(accept),
            ca_dir,
        }
    }

    /// Serves `route` at `path` (replacing an earlier route there).
    pub fn route(&self, path: &str, route: Route) -> &TestServer {
        lock(&self.shared.routes).insert(path.to_string(), route);
        self
    }

    /// Whether this is the HTTPS server.
    pub fn is_tls(&self) -> bool {
        self.shared.tls.is_some()
    }

    /// The listening address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The listening port.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// `http://127.0.0.1:<port><path>` or `https://...` (`path` starts with `/`).
    pub fn url(&self, path: &str) -> String {
        let scheme = if self.is_tls() { "https" } else { "http" };
        format!("{scheme}://127.0.0.1:{}{path}", self.addr.port())
    }

    /// Every request so far, in arrival order.
    pub fn requests(&self) -> Vec<Request> {
        lock(&self.shared.requests).clone()
    }

    /// A PEM file holding the test CA (module docs).
    pub fn ca_pem_path(&self) -> PathBuf {
        self.ca_dir.path().join("ca.pem")
    }

    /// The test CA, DER, for a custom root store.
    pub fn ca_der(&self) -> Vec<u8> {
        CA_DER.to_vec()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
    }
}

/// A rustls root store that trusts only the test CA.
pub fn test_root_store() -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(CA_DER.to_vec()))
        .expect("the test CA");
    roots
}

fn server_config() -> rustls::ServerConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(LEAF_DER.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(LEAF_KEY_DER.to_vec())),
        )
        .expect("the test certificate and key")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let shared = Arc::clone(shared);
                thread::spawn(move || {
                    let _ = serve(stream, &shared);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(_) => thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn serve(stream: TcpStream, shared: &Shared) -> io::Result<()> {
    // An accepted socket inherits O_NONBLOCK on macOS (not on Linux).
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let _ = stream.set_nodelay(true);
    match &shared.tls {
        None => {
            let mut s = stream;
            respond(&mut s, shared)?;
            s.flush()?;
            linger(&s);
        }
        Some(cfg) => {
            let conn = rustls::ServerConnection::new(Arc::clone(cfg)).map_err(io::Error::other)?;
            let mut s = rustls::StreamOwned::new(conn, stream);
            respond(&mut s, shared)?;
            s.conn.send_close_notify();
            s.flush()?;
            linger(&s.sock);
        }
    }
    Ok(())
}

/// A graceful close: no more writes, then read (and drop) what the client still sends until it
/// closes, for at most a second. Closing a socket with unread input sends a reset, which can
/// destroy the response before the client has read it (BusyBox wget over TLS does that).
fn linger(sock: &TcpStream) {
    let _ = sock.shutdown(std::net::Shutdown::Write);
    let _ = sock.set_read_timeout(Some(Duration::from_secs(1)));
    let mut sink = [0u8; 4096];
    let mut sock = sock;
    while matches!(sock.read(&mut sink), Ok(n) if n > 0) {}
}

fn read_head(s: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && !head.ends_with(b"\n\n") {
        if head.len() > 64 << 10 {
            return Err(io::Error::other("request head too large"));
        }
        if s.read(&mut byte)? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        head.push(byte[0]);
    }
    Ok(head)
}

fn parse_head(head: &[u8]) -> Option<Request> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l));
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let headers = lines
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    Some(Request {
        method,
        path,
        headers,
    })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}

fn respond<S: Read + Write>(s: &mut S, shared: &Shared) -> io::Result<()> {
    let head = read_head(s)?;
    let Some(req) = parse_head(&head) else {
        s.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    };
    let route = {
        let routes = lock(&shared.routes);
        let bare = req.path.split('?').next().unwrap_or("");
        routes
            .get(&req.path)
            .or_else(|| routes.get(bare))
            .cloned()
            .unwrap_or_else(|| Route::with_status(404, b"not found\n"))
    };
    let head_only = req.method.eq_ignore_ascii_case("HEAD");
    lock(&shared.requests).push(req);
    let mut out = format!("HTTP/1.1 {} {}\r\n", route.status, reason(route.status));
    for (k, v) in &route.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    if !matches!(route.mode, Mode::NeverEnding { .. }) {
        out.push_str(&format!("Content-Length: {}\r\n", route.body.len()));
    }
    out.push_str("Connection: close\r\n\r\n");
    s.write_all(out.as_bytes())?;
    s.flush()?;
    if head_only {
        return Ok(());
    }
    let stopped = || shared.stop.load(Ordering::SeqCst);
    match route.mode {
        Mode::Normal => s.write_all(&route.body)?,
        Mode::Slow { chunk, delay } => {
            for part in route.body.chunks(chunk) {
                thread::sleep(delay);
                if stopped() {
                    return Ok(());
                }
                s.write_all(part)?;
                s.flush()?;
            }
        }
        Mode::NeverEnding { chunk, delay } => {
            let unit = if route.body.is_empty() {
                vec![0u8; chunk]
            } else {
                route.body.iter().copied().cycle().take(chunk).collect()
            };
            while !stopped() {
                s.write_all(&unit)?;
                s.flush()?;
                thread::sleep(delay);
            }
        }
    }
    s.flush()
}

/// A raw request over plain TCP or TLS (tests and simple clients): sends `GET <path>` and
/// returns the whole response (head and body) once the server closes.
pub fn raw_get(
    server: &TestServer,
    path: &str,
    roots: Option<rustls::RootCertStore>,
) -> io::Result<Vec<u8>> {
    let stream = TcpStream::connect(server.addr())?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: testkit\r\n\r\n");
    let mut out = Vec::new();
    if server.is_tls() {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_root_certificates(roots.unwrap_or_else(test_root_store))
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::IpAddress(std::net::Ipv4Addr::LOCALHOST.into());
        let conn = rustls::ClientConnection::new(Arc::new(cfg), name).map_err(io::Error::other)?;
        let mut tls = rustls::StreamOwned::new(conn, stream);
        tls.write_all(req.as_bytes())?;
        tls.flush()?;
        tls.read_to_end(&mut out)?;
    } else {
        let mut stream = stream;
        stream.write_all(req.as_bytes())?;
        stream.read_to_end(&mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
