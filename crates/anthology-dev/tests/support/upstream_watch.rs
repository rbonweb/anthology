//! DV3 test support for `tests/upstream_watch.rs`: the Python tests' fakes, fixtures and entry
//! builders, and a small recording HTTP server (the Python `_Handler`).
#![allow(dead_code, reason = "each test uses only some helpers")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anthology_dev::upstream_watch::{
    self as uw, Change, Checker, Error, GitHub, Net, Refs, parse_ls_remote,
};
use anthology_pycompat::json::{self, Int, Map, PY_DEFAULT, Value};

pub const SP_REPO: &str = "https://github.com/obra/superpowers";
pub const SKILLS_REPO: &str = "https://github.com/anthropics/skills";
pub const RTK_REPO: &str = "https://github.com/rtk-ai/rtk";
pub const NPM_URL: &str = "https://registry.npmjs.org/@anthropic-ai/claude-code/latest";
pub const UI_URL: &str = "https://uizze.sh/.well-known/skills/ui-taste";

pub fn ui_manifest() -> String {
    format!("{UI_URL}/CHECKSUMS.sha256")
}

/// `S[c]`: forty copies of `c`.
pub fn s(c: char) -> String {
    c.to_string().repeat(40)
}

/// The repository checkout (`ROOT`).
pub fn repo_root() -> PathBuf {
    let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&built).unwrap_or(built)
}

pub fn fix_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upstream_watch")
}

pub fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fix_dir().join(name)).expect("fixture")
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn old_sum() -> String {
    sha256_hex(&fixture("manifest_old.txt"))
}

pub fn new_sum() -> String {
    sha256_hex(&fixture("manifest_new.txt"))
}

pub fn refs_of(name: &str) -> Refs {
    parse_ls_remote(&String::from_utf8(fixture(name)).expect("utf-8"))
}

/// `refs` with the named entries removed.
pub fn drop_refs(mut refs: Refs, drop: &[&str]) -> Refs {
    refs.retain(|(k, _)| !drop.contains(&k.as_str()));
    refs
}

pub fn refs(pairs: &[(&str, &str)]) -> Refs {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

pub fn compare_url(repo: &str, old: &str, new: &str) -> String {
    let tail = repo.split("github.com/").nth(1).expect("a GitHub repo");
    format!("https://api.github.com/repos/{tail}/compare/{old}...{new}")
}

pub fn compare_json(files: &[&str], total_commits: i64) -> Vec<u8> {
    let items: Vec<Value> = files
        .iter()
        .map(|f| obj(vec![("filename", st(f)), ("status", st("modified"))]))
        .collect();
    let data = obj(vec![
        ("status", st("ahead")),
        ("ahead_by", int(1)),
        ("total_commits", int(total_commits)),
        ("commits", Value::Array(Vec::new())),
        ("files", Value::Array(items)),
    ]);
    json::dumps(&data, &PY_DEFAULT).into_bytes()
}

// ---- JSON builders -------------------------------------------------------------------------

pub fn st(t: &str) -> Value {
    Value::Str(t.to_string())
}

pub fn int(n: i64) -> Value {
    Value::Int(Int::Small(n))
}

pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

/// `entry.update(kw)`.
pub fn with(mut entry: Value, kw: Vec<(&str, Value)>) -> Value {
    let m = entry.as_object_mut().expect("an object");
    for (k, v) in kw {
        m.insert(k.to_string(), v);
    }
    entry
}

pub fn git_entry(kw: Vec<(&str, Value)>) -> Value {
    let base = obj(vec![
        ("name", st("frontend-design")),
        ("repo", st(SKILLS_REPO)),
        ("path", st("skills/frontend-design")),
        ("ref", st("main")),
        ("commit", st(&s('1'))),
        ("license", st("Apache-2.0")),
        ("target", st("t")),
        ("transform", Value::Array(Vec::new())),
    ]);
    with(base, kw)
}

pub fn tag_entry(kw: Vec<(&str, Value)>) -> Value {
    let base = git_entry(vec![
        ("name", st("superpowers")),
        ("repo", st(SP_REPO)),
        ("path", st("")),
        ("ref", st("v7.0.2")),
        ("commit", st(&s('5'))),
    ]);
    with(base, kw)
}

pub fn url_entry(kw: Vec<(&str, Value)>) -> Value {
    let base = obj(vec![
        ("name", st("ui-taste")),
        ("url", st(UI_URL)),
        ("checksum", st(&old_sum())),
        ("license", st("MIT")),
        ("target", st("t")),
    ]);
    with(base, kw)
}

pub fn rtk_entry(pin: &str, commit: &str) -> Value {
    obj(vec![
        ("name", st("rtk")),
        ("managed_by", st("subtree")),
        ("target", st("rtk")),
        ("license", st("Apache-2.0")),
        (
            "fork",
            obj(vec![
                ("repo", st("https://github.com/rbonweb/rtk")),
                ("ref", st("develop")),
                ("commit", st(&s('a'))),
            ]),
        ),
        (
            "upstream",
            obj(vec![
                ("repo", st(RTK_REPO)),
                ("ref", st(pin)),
                ("commit", st(commit)),
            ]),
        ),
    ])
}

/// The fixture `sources.json`'s entries.
pub fn fixture_sources() -> Value {
    let text = String::from_utf8(fixture("sources.json")).expect("utf-8");
    let mut data = json::loads(&text).expect("json");
    data.as_object_mut()
        .and_then(|m| m.get_mut("sources"))
        .map(Value::take)
        .expect("sources")
}

/// `make_root(sources, claude, snapshot)`: a throwaway repository root.
pub fn make_root(sources: Option<Value>, claude: &str, snapshot: bool) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    std::fs::create_dir_all(root.join("vendor")).expect("mkdir");
    std::fs::create_dir_all(root.join("ci/claude-code")).expect("mkdir");
    let data = sources.unwrap_or_else(fixture_sources);
    std::fs::write(
        root.join("vendor/sources.json"),
        json::dumps(&obj(vec![("sources", data)]), &PY_DEFAULT),
    )
    .expect("write");
    let pkg = obj(vec![(
        "dependencies",
        obj(vec![("@anthropic-ai/claude-code", st(claude))]),
    )]);
    std::fs::write(
        root.join("ci/claude-code/package.json"),
        json::dumps(&pkg, &PY_DEFAULT),
    )
    .expect("write");
    if snapshot {
        let dir = root.join("vendor/snapshots/ui-taste");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::copy(
            fix_dir().join("snapshots/ui-taste/CHECKSUMS.sha256"),
            dir.join("CHECKSUMS.sha256"),
        )
        .expect("copy");
    }
    tmp
}

// ---- fakes ---------------------------------------------------------------------------------

/// `FakeNet`: ls-remote results and HTTP bodies from maps; records the calls. A key it does not
/// know raises `KeyError`, as the Python dict lookup does.
#[derive(Default)]
pub struct FakeNet {
    pub refs: HashMap<String, Result<Refs, Error>>,
    pub http: HashMap<String, Result<Vec<u8>, Error>>,
    pub ls_calls: RefCell<Vec<String>>,
    pub http_calls: RefCell<Vec<String>>,
}

impl FakeNet {
    pub fn new() -> FakeNet {
        FakeNet::default()
    }

    pub fn refs(mut self, repo: &str, refs: Refs) -> FakeNet {
        self.refs.insert(repo.to_string(), Ok(refs));
        self
    }

    pub fn refs_fail(mut self, repo: &str, class: &str) -> FakeNet {
        self.refs
            .insert(repo.to_string(), Err(Error::Other(class.to_string())));
        self
    }

    pub fn http(mut self, url: &str, body: Vec<u8>) -> FakeNet {
        self.http.insert(url.to_string(), Ok(body));
        self
    }

    pub fn http_fail(mut self, url: &str, class: &str) -> FakeNet {
        self.http
            .insert(url.to_string(), Err(Error::Other(class.to_string())));
        self
    }
}

impl Net for FakeNet {
    fn ls_remote(&self, repo: &str) -> Result<Refs, Error> {
        self.ls_calls.borrow_mut().push(repo.to_string());
        self.refs
            .get(repo)
            .cloned()
            .unwrap_or_else(|| Err(Error::Other("KeyError".into())))
    }

    fn http_get(&self, url: &str, _headers: &[(String, String)]) -> Result<Vec<u8>, Error> {
        self.http_calls.borrow_mut().push(url.to_string());
        self.http
            .get(url)
            .cloned()
            .unwrap_or_else(|| Err(Error::Other("KeyError".into())))
    }
}

/// One `FakeGitHub` call.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    List,
    Create(String, String),
    Update(Value, String, String),
}

/// `FakeGitHub`: records every call; `issues` is what the listing returns. A title containing
/// `fail_title` makes create raise `OSError` (the Python `Flaky` subclass).
pub struct FakeGitHub {
    pub issues: Result<Vec<Value>, Error>,
    pub calls: RefCell<Vec<Call>>,
    pub fail_title: Option<String>,
}

impl FakeGitHub {
    pub fn new(issues: Vec<Value>) -> FakeGitHub {
        FakeGitHub {
            issues: Ok(issues),
            calls: RefCell::new(Vec::new()),
            fail_title: None,
        }
    }

    pub fn failing(class: &str) -> FakeGitHub {
        FakeGitHub {
            issues: Err(Error::Other(class.to_string())),
            calls: RefCell::new(Vec::new()),
            fail_title: None,
        }
    }

    pub fn writes(&self) -> Vec<Call> {
        self.calls
            .borrow()
            .iter()
            .filter(|c| !matches!(c, Call::List))
            .cloned()
            .collect()
    }

    pub fn kinds(&self) -> Vec<&'static str> {
        self.calls
            .borrow()
            .iter()
            .map(|c| match c {
                Call::List => "list",
                Call::Create(..) => "create",
                Call::Update(..) => "update",
            })
            .collect()
    }

    pub fn write_kinds(&self) -> Vec<&'static str> {
        self.kinds().into_iter().filter(|k| *k != "list").collect()
    }
}

impl GitHub for FakeGitHub {
    fn list_issues(&self) -> Result<Vec<Value>, Error> {
        self.calls.borrow_mut().push(Call::List);
        self.issues.clone()
    }

    fn create_issue(&self, title: &str, body: &str) -> Result<Value, Error> {
        if self
            .fail_title
            .as_deref()
            .is_some_and(|f| title.contains(f))
        {
            return Err(Error::Other("OSError".into()));
        }
        self.calls
            .borrow_mut()
            .push(Call::Create(title.into(), body.into()));
        let n = 900 + self.calls.borrow().len() as i64;
        Ok(obj(vec![("number", int(n))]))
    }

    fn update_issue(&self, number: &Value, title: &str, body: &str) -> Result<Value, Error> {
        self.calls
            .borrow_mut()
            .push(Call::Update(number.clone(), title.into(), body.into()));
        Ok(obj(vec![("number", number.clone())]))
    }
}

/// `bot_issue(change, number, **kw)`: an issue as the API returns one the watcher created.
pub fn bot_issue(change: &Change, number: i64, kw: Vec<(&str, Value)>) -> Value {
    let base = obj(vec![
        ("number", int(number)),
        ("state", st("open")),
        ("title", st(&uw::render_title(change))),
        ("body", st(&uw::render_body(change, &[]))),
        (
            "labels",
            Value::Array(vec![obj(vec![("name", st("upstream-watch"))])]),
        ),
        ("user", obj(vec![("login", st("github-actions[bot]"))])),
    ]);
    with(base, kw)
}

/// `check(entry, net, root)`.
pub fn check(entry: &Value, net: &FakeNet, root: Option<&Path>) -> Result<Option<Change>, Error> {
    let default = repo_root();
    Checker::new(root.unwrap_or(&default), net, None).check_entry(entry)
}

// ---- a recording HTTP server (the Python `_Handler`) ----------------------------------------

/// One request the server received: method, target, headers (lower-cased names), JSON body.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Value>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// GET answers `pages[page]` (else `[]`); POST and PATCH answer `{"number": 42}`. Each
/// connection carries one request.
pub struct Recorder {
    pub port: u16,
    pub log: Arc<Mutex<Vec<Seen>>>,
}

impl Recorder {
    pub fn start(pages: HashMap<u32, Value>) -> Recorder {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&log);
        let pages: HashMap<u32, String> = pages
            .into_iter()
            .map(|(k, v)| (k, json::dumps(&v, &PY_DEFAULT)))
            .collect();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = Vec::new();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
                    }
                }
                let length: usize = headers
                    .iter()
                    .find(|(k, _)| k == "content-length")
                    .and_then(|(_, v)| v.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let body = (length > 0)
                    .then(|| json::loads(&String::from_utf8_lossy(&body)).unwrap_or(Value::Null));
                let reply = if method == "GET" {
                    let page = path
                        .split("page=")
                        .last()
                        .and_then(|p| p.parse::<u32>().ok())
                        .unwrap_or(1);
                    pages.get(&page).cloned().unwrap_or_else(|| "[]".into())
                } else {
                    "{\"number\": 42}".into()
                };
                shared.lock().expect("log").push(Seen {
                    method,
                    path,
                    headers,
                    body,
                });
                let mut stream = reader.into_inner();
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Recorder { port, log }
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.log.lock().expect("log").clone()
    }
}
