//! Port of tests/test_taste_bridge.py: `neo.mjs` driven over HTTP, with a fake harness standing in
//! for the BrowserOS tab. Needs `node` on the PATH; without it every node test prints a skip line.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anthology_pycompat::json::{self, PY_DEFAULT, Value};
use regex::Regex;
use repo_tests::repo;

const NEO_REL: &str = "plugins/anthology/knowledge/design-taste-frontend/scripts/neo.mjs";
/// Upper bound for waits that succeed at once in the normal case.
const PATIENCE: Duration = Duration::from_secs(20);

/// A minimal blocking HTTP/1.1 client over `TcpStream`: raw requests with any Host and Origin,
/// Content-Length and chunked bodies, no redirects, no decoding.
mod http {
    use std::collections::BTreeSet;
    use std::io::{self, BufRead, BufReader, Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    pub struct Response {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Response {
        /// The value of the header spelled exactly `name` (like `dict(res.getheaders())`).
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        }

        pub fn lower_names(&self) -> BTreeSet<String> {
            self.headers.iter().map(|(k, _)| k.to_lowercase()).collect()
        }

        pub fn text(&self) -> String {
            String::from_utf8_lossy(&self.body).into_owned()
        }
    }

    pub struct Req {
        port: u16,
        method: String,
        path: String,
        host: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        timeout: Duration,
    }

    impl Req {
        pub fn new(port: u16, method: &str, path: &str) -> Req {
            Req {
                port,
                method: method.to_string(),
                path: path.to_string(),
                host: format!("127.0.0.1:{port}"),
                headers: Vec::new(),
                body: None,
                timeout: Duration::from_secs(10),
            }
        }

        pub fn host(mut self, host: &str) -> Req {
            self.host = host.to_string();
            self
        }

        pub fn header(mut self, name: &str, value: &str) -> Req {
            self.headers.push((name.to_string(), value.to_string()));
            self
        }

        /// A body also adds its Content-Length; no body sends none (as `http.client` does).
        pub fn body(mut self, body: impl Into<Vec<u8>>) -> Req {
            self.body = Some(body.into());
            self
        }

        pub fn timeout(mut self, timeout: Duration) -> Req {
            self.timeout = timeout;
            self
        }

        pub fn try_send(self) -> io::Result<Response> {
            let mut head = format!(
                "{} {} HTTP/1.1\r\nHost: {}\r\n",
                self.method, self.path, self.host
            );
            for (k, v) in &self.headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            if let Some(b) = &self.body {
                head.push_str(&format!("Content-Length: {}\r\n", b.len()));
            }
            head.push_str("\r\n");
            let mut stream = connect(self.port, self.timeout)?;
            stream.write_all(head.as_bytes())?;
            if let Some(b) = &self.body {
                stream.write_all(b)?;
            }
            read_response(stream)
        }

        pub fn send(self) -> Response {
            let what = format!("{} {}", self.method, self.path);
            self.try_send()
                .unwrap_or_else(|e| panic!("{what}: request failed: {e}"))
        }
    }

    fn connect(port: u16, timeout: Duration) -> io::Result<TcpStream> {
        let stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(stream)
    }

    /// Send `request` verbatim and read until the server closes (it must say `Connection: close`).
    pub fn raw(port: u16, request: &str) -> Vec<u8> {
        let mut stream = connect(port, Duration::from_secs(10)).expect("connect");
        stream.write_all(request.as_bytes()).expect("write");
        let mut out = Vec::new();
        stream.read_to_end(&mut out).expect("read until close");
        out
    }

    fn eof() -> io::Error {
        io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed early")
    }

    fn line(r: &mut impl BufRead) -> io::Result<String> {
        let mut s = String::new();
        if r.read_line(&mut s)? == 0 {
            return Err(eof());
        }
        Ok(s.trim_end_matches(['\r', '\n']).to_string())
    }

    fn read_response(stream: TcpStream) -> io::Result<Response> {
        let mut r = BufReader::new(stream);
        let status_line = line(&mut r)?;
        let status = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse::<u16>().ok())
            .ok_or_else(|| io::Error::other(format!("bad status line: {status_line:?}")))?;
        let mut headers = Vec::new();
        loop {
            let l = line(&mut r)?;
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                headers.push((k.to_string(), v.trim().to_string()));
            }
        }
        let find = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        };
        let mut body = Vec::new();
        if status < 200 || status == 204 || status == 304 {
            // no body
        } else if find("Transfer-Encoding").is_some_and(|v| v.to_lowercase().contains("chunked")) {
            loop {
                let size_line = line(&mut r)?;
                let hex = size_line.split(';').next().unwrap_or("").trim();
                let size = usize::from_str_radix(hex, 16)
                    .map_err(|_| io::Error::other(format!("bad chunk size {size_line:?}")))?;
                if size == 0 {
                    while !line(&mut r)?.is_empty() {}
                    break;
                }
                let start = body.len();
                body.resize(start + size, 0);
                r.read_exact(&mut body[start..])?;
                line(&mut r)?;
            }
        } else if let Some(n) = find("Content-Length").and_then(|v| v.parse::<usize>().ok()) {
            body.resize(n, 0);
            r.read_exact(&mut body)?;
        } else {
            r.read_to_end(&mut body)?;
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

use http::{Req, Response};

// ---------------------------------------------------------------------------------- json helpers

fn parse(text: &str) -> Value {
    json::loads(text).unwrap_or_else(|e| panic!("bad json ({}): {text}", e.py_str(text)))
}

fn get<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.as_object()
        .and_then(|m| m.get(key))
        .unwrap_or_else(|| panic!("no key {key:?} in {v:?}"))
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v:?}"))
}

fn quote(s: &str) -> String {
    json::dumps(&Value::Str(s.to_string()), &PY_DEFAULT)
}

fn body_json(r: &Response) -> Value {
    parse(&r.text())
}

// -------------------------------------------------------------------------------------- processes

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn node_available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| {
        Command::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

fn neo() -> PathBuf {
    std::fs::canonicalize(repo::path(NEO_REL)).expect("neo.mjs exists")
}

/// `Path.as_uri()` for an absolute path: `file://` and the path percent-encoded like
/// `urllib.parse.quote` (alphanumerics, `/` and `_.-~` stay).
fn file_uri(p: &Path) -> String {
    let mut out = String::from("file://");
    for b in p.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"/_.-~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A child process that never outlives the test: SIGTERM first (so the bridge cleans up), then
/// SIGKILL after 10 s.
struct ChildGuard(Mutex<Child>);

impl ChildGuard {
    fn pid(&self) -> u32 {
        lock(&self.0).id()
    }

    fn terminate(&self) {
        let pid = self.pid().to_string();
        let _ = Command::new("kill")
            .args(["-TERM", &pid])
            .stderr(Stdio::null())
            .status();
    }

    fn wait_exit(&self, secs: u64) -> Option<ExitStatus> {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Ok(Some(s)) = lock(&self.0).try_wait() {
                return Some(s);
            }
            if Instant::now() >= end {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(lock(&self.0).try_wait(), Ok(None)) {
            return;
        }
        self.terminate();
        if self.wait_exit(10).is_none() {
            let mut child = lock(&self.0);
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// A spawned node process: stdout lines arrive on a channel, stderr is collected.
struct Node {
    child: ChildGuard,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    stdin: Option<ChildStdin>,
}

impl Node {
    fn spawn(args: &[&str], cwd: &Path, with_stdin: bool) -> Node {
        let mut child = Command::new("node")
            .args(args)
            .current_dir(cwd)
            .stdin(if with_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn node");
        let stdin = child.stdin.take();
        let mut out = child.stdout.take().expect("stdout");
        let mut err = child.stderr.take().expect("stderr");
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            let mut pending = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                pending.extend_from_slice(&buf[..n]);
                while let Some(i) = pending.iter().position(|&b| b == b'\n') {
                    let rest = pending.split_off(i + 1);
                    let l = String::from_utf8_lossy(&pending[..i]).into_owned();
                    pending = rest;
                    if tx.send(l).is_err() {
                        return;
                    }
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr);
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                lock(&sink).push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
        Node {
            child: ChildGuard(Mutex::new(child)),
            lines,
            stderr,
            stdin,
        }
    }

    fn next_line(&self) -> String {
        self.lines.recv_timeout(PATIENCE).unwrap_or_else(|e| {
            panic!(
                "no output line from node ({e}); stderr: {}",
                lock(&self.stderr)
            )
        })
    }

    fn stderr(&self) -> String {
        lock(&self.stderr).clone()
    }
}

struct Done {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// `subprocess.run(["node", *args], cwd=cwd, capture_output=True, timeout=secs)`.
fn run(args: &[&str], cwd: &Path, secs: u64) -> Done {
    let mut child = Command::new("node")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn node");
    let mut out = child.stdout.take().expect("stdout");
    let mut err = child.stderr.take().expect("stderr");
    let o = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        String::from_utf8_lossy(&b).into_owned()
    });
    let e = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        String::from_utf8_lossy(&b).into_owned()
    });
    let guard = ChildGuard(Mutex::new(child));
    let Some(status) = guard.wait_exit(secs) else {
        let _ = lock(&guard.0).kill();
        panic!("node {args:?} timed out after {secs} s");
    };
    Done {
        code: status.code(),
        stdout: o.join().expect("stdout reader"),
        stderr: e.join().expect("stderr reader"),
    }
}

// ----------------------------------------------------------------------------------- the bridge

/// The bridge picks its port from 5300-5309, so at most this many run at once in this binary
/// (cargo runs tests in threads); the margin leaves room for a port taken by something else.
const MAX_BRIDGES: usize = 6;
static BRIDGES: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

struct Permit;

impl Permit {
    fn take() -> Permit {
        let (count, freed) = &BRIDGES;
        let mut n = lock(count);
        while *n >= MAX_BRIDGES {
            n = freed.wait(n).unwrap_or_else(PoisonError::into_inner);
        }
        *n += 1;
        Permit
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let (count, freed) = &BRIDGES;
        *lock(count) -= 1;
        freed.notify_one();
    }
}

/// How to reach a running bridge; cheap to clone into a thread.
#[derive(Clone)]
struct Hub {
    port: u16,
    token: String,
    save_token: String,
    origin: String,
}

impl Hub {
    fn req(&self, method: &str, path: &str) -> Req {
        Req::new(self.port, method, path)
    }

    fn call(&self, method: &str, path: &str) -> Response {
        self.req(method, path).send()
    }

    /// An API call with the bridge token; a body is JSON text.
    fn api(&self, method: &str, path: &str, body: Option<&str>) -> Response {
        let req = self.req(method, path).header("X-Neo-Token", &self.token);
        match body {
            Some(b) => req.header("Content-Type", "application/json").body(b),
            None => req,
        }
        .send()
    }

    fn status(&self) -> Value {
        let r = self.api("GET", "/__neo/api/status", None);
        assert_eq!(r.status, 200);
        body_json(&r)
    }
}

/// `neo.mjs serve` running in a folder.
struct Serve {
    hub: Hub,
    info: Value,
    lines: Vec<String>,
    node: Node,
    _permit: Permit,
}

impl Deref for Serve {
    type Target = Hub;
    fn deref(&self) -> &Hub {
        &self.hub
    }
}

impl Serve {
    fn start(dir: &Path, extra: &[&str]) -> Serve {
        let permit = Permit::take();
        let neo = neo();
        let mut args = vec![neo.to_str().expect("utf-8 path"), "serve"];
        args.extend_from_slice(extra);
        let node = Node::spawn(&args, dir, false);
        let lines = vec![node.next_line(), node.next_line()];
        let file = dir.join(".taste-skill").join("neo.json");
        assert!(file.is_file(), "no info file: {lines:?}");
        let info = parse(&std::fs::read_to_string(&file).expect("info file"));
        let port = get(&info, "port").as_i64().expect("port") as u16;
        let hub = Hub {
            port,
            token: text(get(&info, "token")).to_string(),
            save_token: text(get(&info, "saveToken")).to_string(),
            origin: text(get(&info, "origin")).to_string(),
        };
        Serve {
            hub,
            info,
            lines,
            node,
            _permit: permit,
        }
    }
}

/// `neo.mjs serve --root site` on a scratch folder (class `Bridge` in the Python).
struct Bridge {
    serve: Serve, // first: the process stops before the folder goes
    dir: PathBuf,
    _tmp: tempfile::TempDir,
}

impl Deref for Bridge {
    type Target = Serve;
    fn deref(&self) -> &Serve {
        &self.serve
    }
}

impl Bridge {
    /// `None` (after printing the skip line) when node is not installed.
    fn start() -> Option<Bridge> {
        if !node_available() {
            eprintln!("skipped: node not found");
            return None;
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = std::fs::canonicalize(tmp.path()).expect("canonical tempdir");
        std::fs::create_dir(dir.join("site")).expect("site");
        std::fs::write(
            dir.join("site/index.html"),
            "<!doctype html><html><head><title>t</title></head><body>hi</body></html>\n",
        )
        .expect("index.html");
        std::fs::write(dir.join("site/app.js"), "var a = 1;\n").expect("app.js");
        std::fs::write(dir.join("secret.txt"), "outside the root").expect("secret.txt");
        let serve = Serve::start(&dir, &["--root", "site"]);
        Some(Bridge {
            serve,
            dir,
            _tmp: tmp,
        })
    }

    fn site(&self) -> PathBuf {
        self.dir.join("site")
    }
}

// ----------------------------------------------------------------------------------- the harness

type Answer = Box<dyn Fn(&Value) -> String + Send>;

/// What the BrowserOS tab does: poll /__neo/next, answer with /__neo/done. `answer` returns the
/// JSON text of the reply to a job.
struct Harness {
    jobs: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Harness {
    fn start(hub: &Hub, visible: &str, raf: u32, answer: Answer) -> Harness {
        let hello = format!("/__neo/hello?t={}", hub.token);
        hub.req("POST", &hello).body("{}").send();
        let jobs = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (hub, jobs, stop) = (hub.clone(), Arc::clone(&jobs), Arc::clone(&stop));
            let next = format!("/__neo/next?t={}&v={visible}&r={raf}&wait=1", hub.token);
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let Ok(r) = hub
                        .req("GET", &next)
                        .timeout(Duration::from_secs(40))
                        .try_send()
                    else {
                        return;
                    };
                    if r.status != 200 {
                        continue;
                    }
                    let job = body_json(&r);
                    let id = text(get(&job, "id")).to_string();
                    lock(&jobs).push(job.clone());
                    let reply = answer(&job);
                    let done = format!("/__neo/done?t={}&id={id}", hub.token);
                    if hub.req("POST", &done).body(reply).try_send().is_err() {
                        return;
                    }
                }
            })
        };
        let harness = Harness {
            jobs,
            stop,
            thread: Some(thread),
        };
        let deadline = Instant::now() + PATIENCE;
        while Instant::now() < deadline {
            let r = hub.api("GET", "/__neo/api/status", None);
            if r.status == 200 && get(&body_json(&r), "harness").as_bool() == Some(true) {
                return harness;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("the harness never showed up");
    }

    /// Stops polling; the thread ends within the bridge's 1 s poll.
    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.join().expect("harness thread");
        }
    }

    fn jobs(&self) -> Vec<Value> {
        lock(&self.jobs).clone()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn answer_null() -> Answer {
    Box::new(|_| r#"{"ok": true, "value": null}"#.to_string())
}

// ------------------------------------------------------------------------------------------ tests

mod startup_test {
    use super::*;

    #[test]
    fn test_prints_the_two_lines_and_writes_the_info_file() {
        let Some(b) = Bridge::start() else { return };
        let root = b.site().to_string_lossy().into_owned();
        let port = b.port;
        assert_eq!(
            b.lines[0],
            format!("NEO: serving {root} at http://127.0.0.1:{port}/")
        );
        assert_eq!(
            b.lines[1],
            format!("NEO: open the harness in a BrowserOS Neo tab: http://127.0.0.1:{port}/__neo/")
        );
        assert!((5300..=5309).contains(&port), "port {port}");
        assert_eq!(text(get(&b.info, "root")), root);
        assert_eq!(
            text(get(&b.info, "origin")),
            format!("http://127.0.0.1:{port}")
        );
        assert_eq!(
            get(&b.info, "pid").as_i64(),
            Some(i64::from(b.node.child.pid()))
        );
        let hex32 = Regex::new(r"^[0-9a-f]{32}$").unwrap();
        for key in ["token", "saveToken"] {
            assert!(hex32.is_match(text(get(&b.info, key))), "{key}");
        }
        assert_ne!(text(get(&b.info, "token")), text(get(&b.info, "saveToken")));
    }

    #[test]
    fn test_the_root_must_be_inside_the_working_folder() {
        let Some(b) = Bridge::start() else { return };
        let neo = neo();
        for bad in ["..", "/tmp"] {
            let out = run(
                &[neo.to_str().unwrap(), "serve", "--root", bad],
                &b.site(),
                20,
            );
            assert_ne!(out.code, Some(0), "{bad}");
            assert!(out.stderr.contains("inside"), "{bad}: {}", out.stderr);
        }
    }
}

mod security_test {
    use super::*;

    #[test]
    fn test_wrong_host_gets_421() {
        let Some(b) = Bridge::start() else { return };
        let port = b.port;
        let paths = [
            "/".to_string(),
            "/__neo/".to_string(),
            format!("/__neo/next?t={}", b.token),
        ];
        let hosts = [
            format!("evil.example:{port}"),
            "127.0.0.1".to_string(),
            format!("127.0.0.1.evil.example:{port}"),
        ];
        for path in &paths {
            for host in &hosts {
                let status = b.req("GET", path).host(host).send().status;
                assert_eq!(status, 421, "{path} {host}");
            }
        }
        let ok = b.req("GET", "/").host(&format!("localhost:{port}")).send();
        assert_eq!(ok.status, 200);
    }

    #[test]
    fn test_foreign_origin_gets_403_and_there_is_no_cors() {
        let Some(b) = Bridge::start() else { return };
        for (method, path) in [
            ("GET", "/index.html"),
            ("GET", "/__neo/api/status"),
            ("POST", "/__neo/api/rpc"),
        ] {
            let r = b
                .req(method, path)
                .header("Origin", "http://evil.example")
                .header("X-Neo-Token", &b.token)
                .send();
            assert_eq!(r.status, 403, "{path}");
            assert!(!r.lower_names().contains("access-control-allow-origin"));
        }
        let r = b
            .req("GET", "/index.html")
            .header("Origin", &b.origin)
            .send();
        assert_eq!(r.status, 200);
        assert!(!r.lower_names().contains("access-control-allow-origin"));
        let r = b
            .req("OPTIONS", "/__neo/api/rpc")
            .header("Origin", &b.origin)
            .send();
        assert!(!r.lower_names().contains("access-control-allow-origin"));
    }

    #[test]
    fn test_the_api_needs_the_token() {
        let Some(b) = Bridge::start() else { return };
        let zeros = "0".repeat(32);
        let cases = [
            ("GET", "/__neo/api/status".to_string()),
            ("POST", "/__neo/api/rpc".to_string()),
            ("GET", "/__neo/api/requests?tab=a".to_string()),
            ("GET", "/__neo/next".to_string()),
            ("GET", format!("/__neo/next?t={zeros}")),
            ("POST", "/__neo/done?t=nope&id=1".to_string()),
            ("POST", "/__neo/hello?t=nope".to_string()),
        ];
        for (method, path) in &cases {
            let mut req = b.req(method, path);
            if *method == "POST" {
                req = req.body("{}");
            }
            assert_eq!(req.send().status, 401, "{path}");
        }
        let with = |token: &str| {
            b.req("GET", "/__neo/api/status")
                .header("X-Neo-Token", token)
                .send()
                .status
        };
        assert_eq!(with(&zeros), 401);
        assert_eq!(with(&b.save_token), 401);
        assert_eq!(b.api("GET", "/__neo/api/status", None).status, 200);
    }

    #[test]
    fn test_save_is_limited_to_images_under_shots_and_the_save_token() {
        let Some(b) = Bridge::start() else { return };
        let save = "/__neo/api/save?file=";
        let png = "iVBORw0KGgo=";
        for rel in [
            "index.html",
            "../x.png",
            "shots/../../x.png",
            "shots/a.html",
            "shots/a.svg",
            "/etc/x.png",
            "other/a.png",
            "shots/%2e%2e/a.png",
            "shots/a.png/../../b.js",
            "",
            "shots/\\x.png",
            ".taste-skillx/a.png",
        ] {
            let r = b
                .req("POST", &format!("{save}{rel}"))
                .header("X-Neo-Token", &b.save_token)
                .body(png)
                .send();
            assert!([400, 403].contains(&r.status), "{rel}: {}", r.status);
        }
        let r = b
            .req("POST", &format!("{save}shots/a.png"))
            .header("X-Neo-Token", &b.token)
            .body(png)
            .send();
        assert_eq!(r.status, 401);
        assert!(!b.site().join("shots").exists());
        assert!(!b.dir.join("x.png").exists());
        let r = b
            .req("POST", &format!("{save}shots/1440-01.jpg"))
            .header("X-Neo-Token", &b.save_token)
            .body("data:image/jpeg;base64,/9j/4AAQ")
            .send();
        assert_eq!(r.status, 200);
        assert_eq!(
            body_json(&r),
            parse(r#"{"file": "shots/1440-01.jpg", "bytes": 6}"#)
        );
        assert_eq!(
            std::fs::read(b.site().join("shots/1440-01.jpg")).unwrap(),
            [0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10]
        );
        let r = b
            .req("POST", &format!("{save}.taste-skill/x.png"))
            .header("X-Neo-Token", &b.save_token)
            .body(png)
            .send();
        assert_eq!(r.status, 200);
        assert!(b.site().join(".taste-skill/x.png").is_file());
    }

    #[test]
    fn test_static_files_never_leave_the_root() {
        let Some(b) = Bridge::start() else { return };
        for path in [
            "/../secret.txt",
            "/%2e%2e/secret.txt",
            "/..%2fsecret.txt",
            "/a/../../secret.txt",
        ] {
            let status = b.call("GET", path).status;
            assert!([400, 403, 404].contains(&status), "{path}: {status}");
        }
        let raw = http::raw(
            b.port,
            &format!(
                "GET /../secret.txt HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                b.port
            ),
        );
        let leaked = raw
            .windows(b"outside the root".len())
            .any(|w| w == b"outside the root");
        assert!(!leaked, "the raw response leaked the file");
    }
}

mod static_test {
    use super::*;

    #[test]
    fn test_a_page_is_served_without_the_init_blocks_unless_asked() {
        let Some(b) = Bridge::start() else { return };
        let r = b.call("GET", "/index.html");
        assert_eq!(r.status, 200);
        assert!(!r.text().contains("data-neo-init"));
        assert_eq!(r.header("Cache-Control"), Some("no-store"));
        let r = b.call("GET", "/");
        assert_eq!(r.status, 200);
        assert!(r.text().contains("hi"));
        assert!(!r.text().contains("data-neo-init"));
        assert_eq!(b.call("GET", "/nope.html").status, 404);
    }

    #[test]
    fn test_the_init_blocks_go_in_only_with_the_neo_parameter() {
        let Some(b) = Bridge::start() else { return };
        let r = b.call("GET", "/index.html?__neo=tab1");
        let page = r.text();
        assert_eq!(r.status, 200);
        assert_eq!(page.matches("<script data-neo-init>").count(), 2);
        assert!(page.find("data-neo-init").unwrap() < page.find("<title>").unwrap());
        assert!(page.starts_with("<!doctype html><html><head><script data-neo-init>"));
        assert!(page.contains("__neoLogs"));
        assert!(page.contains("HeadlessChrome"));
        assert_eq!(b.call("GET", "/app.js?__neo=tab1").body, b"var a = 1;\n");
    }

    #[test]
    fn test_registered_init_scripts_are_injected_for_that_tab_only() {
        let Some(b) = Bridge::start() else { return };
        let r = b.api(
            "POST",
            "/__neo/api/rpc",
            Some(r#"{"op": "init", "tab": "tab9", "source": "window.__MARK = 'zz-init-9';"}"#),
        );
        assert_eq!(r.status, 200);
        let page = b.call("GET", "/index.html?__neo=tab9").text();
        assert!(page.contains("window.__MARK = 'zz-init-9';"));
        assert_eq!(page.matches("<script data-neo-init>").count(), 3);
        let other = b.call("GET", "/index.html?__neo=tab8").text();
        assert!(!other.contains("zz-init-9"));
    }

    #[test]
    fn test_gzip_for_text_types() {
        let Some(b) = Bridge::start() else { return };
        let r = b
            .req("GET", "/app.js")
            .header("Accept-Encoding", "gzip")
            .send();
        assert_eq!(r.status, 200);
        assert_eq!(r.header("Content-Encoding"), Some("gzip"));
        let mut plain = Vec::new();
        flate2::read::GzDecoder::new(&r.body[..])
            .read_to_end(&mut plain)
            .expect("gunzip");
        assert_eq!(plain, b"var a = 1;\n");
    }

    #[test]
    fn test_the_request_log_records_own_requests_per_tab() {
        let Some(b) = Bridge::start() else { return };
        b.call("GET", "/index.html?__neo=tabL");
        b.req("GET", "/app.js")
            .header("Referer", &format!("{}/index.html?__neo=tabL", b.origin))
            .send();
        b.call("GET", "/missing.png?__neo=tabL");
        let r = b.api("GET", "/__neo/api/requests?tab=tabL", None);
        assert_eq!(r.status, 200);
        let log = body_json(&r);
        let mut by_url: BTreeMap<String, &Value> = BTreeMap::new();
        for entry in log.as_array().expect("a list") {
            let url = text(get(entry, "url"));
            by_url.insert(url.split('?').next().unwrap().to_string(), entry);
        }
        let at = |name: &str| {
            by_url
                .get(&format!("{}{name}", b.origin))
                .copied()
                .unwrap_or_else(|| panic!("{name} is not in the log: {by_url:?}"))
        };
        assert_eq!(text(get(at("/index.html"), "type")), "Document");
        assert_eq!(text(get(at("/app.js"), "type")), "Script");
        assert_eq!(get(at("/missing.png"), "status").as_i64(), Some(404));
        assert_eq!(get(at("/missing.png"), "failed").as_bool(), Some(true));
        assert_eq!(
            get(at("/app.js"), "decoded").as_i64(),
            Some(b"var a = 1;\n".len() as i64)
        );
    }
}

mod root_secrets_test {
    use super::*;

    #[test]
    fn test_the_bridge_info_file_is_not_served_when_the_root_is_the_working_folder() {
        if !node_available() {
            eprintln!("skipped: node not found");
            return;
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let work = std::fs::canonicalize(tmp.path()).expect("canonical tempdir");
        std::fs::write(
            work.join("index.html"),
            "<html><head></head><body>x</body></html>\n",
        )
        .unwrap();
        let s = Serve::start(&work, &[]);
        assert_eq!(text(get(&s.info, "root")), work.to_string_lossy());
        assert_eq!(
            std::fs::read_to_string(work.join(".taste-skill/.gitignore")).unwrap(),
            "*\n"
        );
        for path in [
            "/.taste-skill/neo.json",
            "/.taste-skill/",
            "/.taste-skill/.gitignore",
            "/.TASTE-SKILL/neo.json",
            "/.Taste-Skill/neo.json",
            "/.TASTE-SKILL/",
            "/.TASTE-SKILL/.gitignore",
        ] {
            let r = s.call("GET", path);
            assert_eq!(r.status, 404, "{path}");
            let leaked = r
                .body
                .windows(s.token.len())
                .any(|w| w == s.token.as_bytes());
            assert!(!leaked, "{path} leaked the token");
        }
    }
}

mod job_round_trip_test {
    use super::*;

    #[test]
    fn test_one_eval_job_round_trips_between_a_client_and_the_harness() {
        let Some(b) = Bridge::start() else { return };
        let answer: Answer = Box::new(|job| {
            format!(
                r#"{{"ok": true, "value": {{"echo": {}, "tab": {}}}}}"#,
                quote(text(get(job, "expr"))),
                quote(text(get(job, "tab")))
            )
        });
        let h = Harness::start(&b, "visible", 60, answer);
        let r = b.api(
            "POST",
            "/__neo/api/rpc",
            Some(r#"{"op": "eval", "tab": "t1", "expr": "1 + 1", "ms": 5000}"#),
        );
        assert_eq!(r.status, 200);
        assert_eq!(
            body_json(&r),
            parse(r#"{"ok": true, "value": {"echo": "1 + 1", "tab": "t1"}}"#)
        );
        let jobs = h.jobs();
        assert_eq!(text(get(jobs.last().unwrap(), "op")), "eval");
        let info = b.status();
        assert_eq!(get(&info, "harness").as_bool(), Some(true));
        assert_eq!(get(&info, "visible").as_bool(), Some(true));
        assert_eq!(get(&info, "raf").as_i64(), Some(60));
        assert_eq!(text(get(&info, "root")), b.site().to_string_lossy());
    }

    #[test]
    fn test_the_heartbeat_keeps_the_harness_status_fresh_without_taking_jobs() {
        let Some(b) = Bridge::start() else { return };
        let beat = format!("/__neo/beat?t={}&v=visible&r=58", b.token);
        assert_eq!(b.call("GET", &beat).status, 204);
        assert_eq!(
            b.call("GET", "/__neo/beat?t=nope&v=visible&r=58").status,
            401
        );
        let info = b.status();
        assert_eq!(get(&info, "harness").as_bool(), Some(true));
        assert_eq!(get(&info, "visible").as_bool(), Some(true));
        assert_eq!(get(&info, "raf").as_i64(), Some(58));
        assert!(
            b.call("GET", "/__neo/")
                .text()
                .contains("neo harness: connected")
        );
    }

    #[test]
    fn test_a_job_with_no_harness_times_out_with_504() {
        let Some(b) = Bridge::start() else { return };
        let started = Instant::now();
        let r = b.api(
            "POST",
            "/__neo/api/rpc",
            Some(r#"{"op": "ping", "tab": "x", "ms": 100}"#),
        );
        assert_eq!(r.status, 504);
        assert_eq!(get(&body_json(&r), "ok").as_bool(), Some(false));
        assert!(started.elapsed() < Duration::from_secs(20));
    }

    #[test]
    fn test_a_harness_error_comes_back_as_ok_false() {
        let Some(b) = Bridge::start() else { return };
        let answer: Answer =
            Box::new(|_| r#"{"ok": false, "error": "ReferenceError: x is not defined"}"#.into());
        let _h = Harness::start(&b, "visible", 60, answer);
        let r = b.api(
            "POST",
            "/__neo/api/rpc",
            Some(r#"{"op": "eval", "tab": "t", "expr": "x", "ms": 5000}"#),
        );
        assert_eq!(r.status, 200);
        assert_eq!(
            body_json(&r),
            parse(r#"{"ok": false, "error": "ReferenceError: x is not defined"}"#)
        );
    }

    #[test]
    fn test_hello_fails_every_pending_job() {
        let Some(b) = Bridge::start() else { return };
        let hub = Hub::clone(&b);
        let client = thread::spawn(move || {
            hub.api(
                "POST",
                "/__neo/api/rpc",
                Some(r#"{"op": "eval", "tab": "t", "expr": "1", "ms": 20000}"#),
            )
        });
        thread::sleep(Duration::from_millis(500));
        b.req("POST", &format!("/__neo/hello?t={}", b.token))
            .body("{}")
            .send();
        let r = client.join().expect("the rpc call");
        assert_eq!(r.status, 200);
        assert_eq!(
            body_json(&r),
            parse(r#"{"ok": false, "error": "harness reloaded"}"#)
        );
    }

    #[test]
    fn test_the_harness_page_carries_the_token_and_the_title() {
        let Some(b) = Bridge::start() else { return };
        let r = b.call("GET", "/__neo/");
        let page = r.text();
        assert_eq!(r.status, 200);
        assert!(r.header("Content-Type").unwrap().contains("text/html"));
        assert!(page.contains(&b.token));
        assert!(!page.contains(&b.save_token));
        assert!(page.contains("neo harness"));
        assert!(!page.contains("data-neo-init"));
    }
}

const CLIENT: &str = r#"
import { connect, status, NeoTab, NeoError } from %(neo)s;
const out = {};
const s0 = await status({ cwd: process.cwd() });
out.status = { ok: s0.ok, why: s0.why };
const hub = await connect({ cwd: process.cwd(), wait: 3000 });
out.origin = hub.origin;
out.url = hub.urlFor(hub.root + '/dir with space/a b.html');
try { hub.urlFor('/etc/passwd'); } catch (e) { out.outside = [e instanceof NeoError, e.neo === true, e.message]; }
const tab = await NeoTab.open(hub, { width: 390, height: 844 }, ['window.__X = 1;']);
out.vw = [tab.vw, tab.vh];
await tab.goto(hub.urlFor(hub.root + '/index.html'), 5000);
out.value = await tab.eval('document.title', 5000);
out.shot = await tab.shot();
out.noop = await tab.send('Page.enable', {});
out.resize = await tab.send('Emulation.setDeviceMetricsOverride', { width: 768, height: 1024 });
out.vw2 = [tab.vw, tab.vh];
out.evalViaSend = await tab.send('Runtime.evaluate', { expression: '2 + 2' });
try { await tab.send('Page.captureScreenshot', {}); } catch (e) { out.cdp = [e instanceof NeoError, e.neo === true, e.message]; }
try { await tab.eval('throw new Error("boom")', 5000); } catch (e) { out.thrown = e.message; }
out.logs = await tab.logs();
await tab.syncFailed();
out.failed = tab.failed;
out.on = typeof hub.on('x', () => {});
out.hubSend = await hub.send('Browser.close');
await tab.close();
await hub.close();
console.log(JSON.stringify(out));
"#;

mod client_test {
    use super::*;

    fn node_module(script: &str, dir: &Path, secs: u64) -> Done {
        run(&["--input-type=module", "-e", script], dir, secs)
    }

    #[test]
    fn test_the_client_module_drives_the_hub_through_a_fake_harness() {
        let Some(b) = Bridge::start() else { return };
        let answer: Answer = Box::new(|job| {
            let expr = job
                .as_object()
                .and_then(|m| m.get("expr"))
                .and_then(Value::as_str)
                .unwrap_or("");
            match text(get(job, "op")) {
                "eval" if expr.contains("boom") => {
                    r#"{"ok": false, "error": "Error: boom\n    at <anonymous>"}"#.into()
                }
                "eval" if expr.contains("title") => r#"{"ok": true, "value": "TITLE"}"#.into(),
                "eval" => r#"{"ok": true, "value": 4}"#.into(),
                "goto" => format!(
                    r#"{{"ok": true, "value": {{"url": {}, "status": "load"}}}}"#,
                    quote(text(get(job, "url")))
                ),
                "logs" => r#"{"ok": true, "value": ["error: x"]}"#.into(),
                _ => r#"{"ok": true, "value": null}"#.into(),
            }
        });
        let h = Harness::start(&b, "visible", 60, answer);
        let script = CLIENT.replace("%(neo)s", &quote(&file_uri(&neo())));
        let done = node_module(&script, &b.dir, 60);
        assert_eq!(done.code, Some(0), "{}", done.stderr);
        let out = parse(done.stdout.trim().lines().last().expect("output"));
        let eq = |key: &str, expected: &str| {
            assert_eq!(get(&out, key), &parse(expected), "{key}");
        };
        eq("status", r#"{"ok": true, "why": null}"#);
        assert_eq!(
            text(get(&out, "url")),
            format!(
                "{}/dir%20with%20space/a%20b.html",
                text(get(&out, "origin"))
            )
        );
        let outside = get(&out, "outside").as_array().expect("outside");
        assert_eq!(outside[..2], [Value::Bool(true), Value::Bool(true)]);
        assert!(text(&outside[2]).contains("outside the bridge's root"));
        eq("vw", "[390, 844]");
        eq("value", r#""TITLE""#);
        assert!(get(&out, "shot").is_null());
        eq("noop", "{}");
        eq("vw2", "[768, 1024]");
        eq("evalViaSend", r#"{"result": {"value": 4}}"#);
        let cdp = get(&out, "cdp").as_array().expect("cdp");
        assert_eq!(cdp[..2], [Value::Bool(true), Value::Bool(true)]);
        assert_eq!(
            text(&cdp[2]),
            "Page.captureScreenshot is not available on BrowserOS Neo"
        );
        assert!(text(get(&out, "thrown")).starts_with("page: Error: boom"));
        eq("logs", r#"["error: x"]"#);
        eq("failed", "[]");
        assert_eq!(text(get(&out, "on")), "function");
        eq("hubSend", "{}");
        let jobs = h.jobs();
        let ops: Vec<&str> = jobs.iter().map(|j| text(get(j, "op"))).collect();
        assert_eq!(ops[0], "open");
        assert!(ops.contains(&"close"), "{ops:?}");
        let goto = jobs
            .iter()
            .find(|j| text(get(j, "op")) == "goto")
            .expect("a goto job");
        assert_eq!(
            text(get(goto, "url")),
            format!("{}/index.html", text(get(&out, "origin")))
        );
    }

    #[test]
    fn test_a_bridge_timeout_is_not_an_unavailable_method() {
        let Some(b) = Bridge::start() else { return };
        let mut h = Harness::start(&b, "visible", 60, answer_null());
        let script = format!(
            "import {{ connect, NeoTab, NeoError }} from {};\
             const hub = await connect({{ cwd: process.cwd(), wait: 3000 }});\
             const tab = await NeoTab.open(hub, {{ width: 390, height: 844 }});\
             const out = {{}};\
             try {{ await tab.send('Page.captureScreenshot', {{}}); }} catch (e) {{ out.unavailable = [e instanceof NeoError, e.unavailable === true]; }}\
             await new Promise((r) => setTimeout(r, 100));\
             process.stdout.write('READY\\n');\
             await new Promise((r) => process.stdin.once('data', r));\
             try {{ await tab.eval('1', 200); }} catch (e) {{ out.timeout = [e instanceof NeoError, e.unavailable === true, e.message]; }}\
             console.log(JSON.stringify(out)); process.exit(0);",
            quote(&file_uri(&neo()))
        );
        let mut client = Node::spawn(&["--input-type=module", "-e", &script], &b.dir, true);
        let ready = client.next_line();
        assert_eq!(ready.trim(), "READY", "stderr: {}", client.stderr());
        h.stop();
        let mut stdin = client.stdin.take().expect("stdin");
        stdin.write_all(b"go\n").expect("write go");
        stdin.flush().expect("flush");
        let out = parse(&client.next_line());
        assert_eq!(get(&out, "unavailable"), &parse("[true, true]"));
        let timeout = get(&out, "timeout").as_array().expect("timeout");
        assert_eq!(timeout[..2], [Value::Bool(true), Value::Bool(false)]);
        assert!(text(&timeout[2]).contains("did not answer"));
    }

    #[test]
    fn test_status_says_what_to_do_when_the_harness_is_missing_or_hidden() {
        let Some(b) = Bridge::start() else { return };
        let script = format!(
            "import {{ status }} from {}; const s = await status({{ cwd: process.cwd() }});\
             console.log(JSON.stringify({{ ok: s.ok, why: s.why }}));",
            quote(&file_uri(&neo()))
        );
        let status_now = || {
            let done = node_module(&script, &b.dir, 30);
            parse(&done.stdout)
        };
        let missing = status_now();
        assert_eq!(get(&missing, "ok").as_bool(), Some(false));
        assert!(
            text(get(&missing, "why")).contains(&format!(
                "open http://127.0.0.1:{}/__neo/ in a BrowserOS Neo tab",
                b.port
            )),
            "{missing:?}"
        );
        let _h = Harness::start(&b, "hidden", 0, answer_null());
        let hidden = status_now();
        assert_eq!(get(&hidden, "ok").as_bool(), Some(false));
        assert!(text(get(&hidden, "why")).contains("the harness tab is hidden"));
        assert!(text(get(&hidden, "why")).contains("mcp__browseros-neo__run"));
    }

    #[test]
    fn test_status_cli_prints_json_and_exits_3_when_not_ready() {
        let Some(b) = Bridge::start() else { return };
        let neo = neo();
        let neo = neo.to_str().unwrap();
        let done = run(&[neo, "status"], &b.dir, 30);
        assert_eq!(done.code, Some(3));
        let data = parse(&done.stdout);
        assert_eq!(get(&data, "ok").as_bool(), Some(false));
        assert_eq!(get(&data, "port").as_i64(), Some(i64::from(b.port)));
        assert_eq!(text(get(&data, "root")), b.site().to_string_lossy());
        let _h = Harness::start(&b, "visible", 60, answer_null());
        let done = run(&[neo, "status"], &b.dir, 30);
        assert_eq!(done.code, Some(0), "{}", done.stdout);
        assert_eq!(get(&parse(&done.stdout), "ok").as_bool(), Some(true));
    }

    #[test]
    fn test_status_with_no_bridge_says_how_to_start_it() {
        // The Python class starts a bridge for every test; keep it running, as there.
        let Some(_b) = Bridge::start() else { return };
        let empty = tempfile::tempdir().expect("tempdir");
        let neo = neo();
        let done = run(&[neo.to_str().unwrap(), "status"], empty.path(), 30);
        assert_eq!(done.code, Some(3));
        let why = get(&parse(&done.stdout), "why").clone();
        let re =
            Regex::new(r"the BrowserOS Neo bridge is not running: run `node .*neo\.mjs serve`")
                .unwrap();
        assert!(re.is_match(text(&why)), "{why:?}");
    }
}

mod stop_test {
    use super::*;

    #[test]
    fn test_stop_ends_the_process_and_removes_the_info_file() {
        let Some(b) = Bridge::start() else { return };
        let neo = neo();
        let neo = neo.to_str().unwrap();
        let done = run(&[neo, "stop"], &b.dir, 30);
        assert_eq!(done.code, Some(0), "{}", done.stderr);
        assert!(
            b.node.child.wait_exit(10).is_some(),
            "the bridge is still running"
        );
        assert!(!b.dir.join(".taste-skill/neo.json").exists());
        let again = run(&[neo, "stop"], &b.dir, 30);
        assert_eq!(again.code, Some(0));
    }

    #[test]
    fn test_sigterm_removes_the_info_file() {
        let Some(b) = Bridge::start() else { return };
        b.node.child.terminate();
        assert!(
            b.node.child.wait_exit(10).is_some(),
            "the bridge is still running"
        );
        assert!(!b.dir.join(".taste-skill/neo.json").exists());
    }
}

mod source_test {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn test_neo_mjs_uses_only_node_builtins_and_is_not_executable() {
        let src = repo::read(NEO_REL);
        for m in Regex::new(r"from '([^']+)'").unwrap().captures_iter(&src) {
            assert!(m[1].starts_with("node:"), "{}", &m[1]);
        }
        let banned = Regex::new(r"\brequire\(|child_process|spawn|exec\(").unwrap();
        assert!(!banned.is_match(&src), "{:?}", banned.find(&src));
        let mode = std::fs::metadata(repo::path(NEO_REL))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0, "neo.mjs must not be executable");
    }
}
