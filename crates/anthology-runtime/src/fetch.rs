//! HTTPS fetch over ureq (RT2): `rtk_install.default_fetch` and its `HttpsOnlyRedirect`.
//!
//! The OS trust store (design §11.2) is **not** ureq's default: ureq 3 uses
//! `RootCerts::WebPki` unless the agent's TLS config sets `RootCerts::PlatformVerifier`.
//! [`UreqFetch::new`] sets it explicitly, and a test asserts it on the built agent. Only the
//! test constructor [`UreqFetch::with_test_roots`] trusts anything else (the testkit CA).
//!
//! [`download`] is `default_fetch`'s body, written against the [`Opener`] seam (Python's
//! `urllib.request.build_opener` patch site) so the Python tests port branch for branch:
//! - a URL that is not `https` is refused before any opener exists;
//! - the overall deadline is `4 × timeout` on `monotonic`, checked after every receive (a
//!   trickling server cannot stretch it), on top of ureq's own per-read `timeout` and global
//!   `4 × timeout`;
//! - a `Content-Length` over the cap is refused before the first read, and the streamed body is
//!   capped too;
//! - the final URL must be `https`.
//!
//! [`UreqOpener`] follows redirects itself (ureq's own are off), so every hop is checked by
//! [`redirect_target`] (https only, as `HttpsOnlyRedirect.redirect_request`), at most
//! [`MAX_REDIRECTS`] of them (urllib's `max_redirections`), each with its own proxy decision.
//! The proxy comes from the [`Env`], never the process environment (GC-18), with urllib's
//! rules for an `https` URL: `https_proxy` (lower case wins), else `HTTPS_PROXY`; `no_proxy`,
//! else `NO_PROXY`, matched as `proxy_bypass_environment` does. The proxy is always reached
//! with plain HTTP `CONNECT`, as urllib does. There are no retries.
//!
//! [`fetch_to_path`] is `default_fetch`'s file handling: it creates the file, and unlinks it
//! on any error, so no partial download is left behind.
#![forbid(unsafe_code)]

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anthology_core::Env;
use anthology_pycompat::pyerr::PyExc;
use ureq::Agent;
use ureq::config::Config;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

/// The download cap (`rtk_install.MAX_DOWNLOAD_BYTES`, GC-9).
pub const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// The `User-Agent` of every request (GC-9).
pub const USER_AGENT: &str = "anthology-rtk-installer";
/// urllib's `HTTPRedirectHandler.max_redirections`.
pub const MAX_REDIRECTS: usize = 10;
/// The size of each read (`resp.read1(1 << 14)`).
const CHUNK: usize = 1 << 14;

/// Downloads `url` into `dest` (plan §3.6). `timeout` is per socket operation, in seconds.
pub trait Fetch {
    fn fetch(&self, url: &str, dest: &mut dyn Write, timeout: f64) -> Result<(), PyExc>;
}

/// `InstallError(msg)`: an error whose message is shown to the user.
pub fn install_error(msg: impl Into<String>) -> PyExc {
    PyExc::Other("InstallError", msg.into())
}

/// The scheme of `url`, lower-cased (`urlsplit(url).scheme.lower()`); empty when there is
/// none.
pub fn scheme(url: &str) -> String {
    match url.find(':') {
        Some(i)
            if i > 0
                && url[..i].starts_with(|c: char| c.is_ascii_alphabetic())
                && url[..i]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            url[..i].to_ascii_lowercase()
        }
        _ => String::new(),
    }
}

fn is_https(url: &str) -> bool {
    scheme(url) == "https"
}

/// A response as `default_fetch` reads it.
pub trait Response {
    /// The URL the response came from, after redirects (`resp.geturl()`).
    fn final_url(&self) -> String;
    /// The raw `Content-Length` header, if any.
    fn content_length(&self) -> Option<String>;
    /// Whatever body bytes have arrived, at most `buf.len()`; 0 at the end (`read1`).
    fn read1(&mut self, buf: &mut [u8]) -> Result<usize, PyExc>;
}

/// Opens a URL (`build_opener(HttpsOnlyRedirect).open(request, timeout=timeout)`).
pub trait Opener {
    fn open(&self, url: &str, timeout: f64) -> Result<Box<dyn Response>, PyExc>;
}

/// `default_fetch(url, dest, timeout)` over `opener` (module docs). `monotonic` is
/// `time.monotonic`; `max_bytes` is `MAX_DOWNLOAD_BYTES`.
pub fn download(
    opener: &dyn Opener,
    url: &str,
    dest: &mut dyn Write,
    timeout: f64,
    max_bytes: u64,
    monotonic: fn() -> f64,
) -> Result<(), PyExc> {
    if !is_https(url) {
        return Err(install_error(format!(
            "refusing to download over a non-https URL: {url}"
        )));
    }
    let deadline = monotonic() + 4.0 * timeout;
    let mut resp = opener.open(url, timeout)?;
    if !is_https(&resp.final_url()) {
        return Err(install_error("download ended on a non-https URL"));
    }
    let too_large = || install_error(format!("download is larger than {max_bytes} bytes"));
    // `length.isdigit() and int(length) > cap`: all digits, and a value too large for u64 is
    // over the cap too.
    if let Some(length) = resp.content_length()
        && !length.is_empty()
        && length.bytes().all(|b| b.is_ascii_digit())
        && length.parse::<u64>().map_or(true, |n| n > max_bytes)
    {
        return Err(too_large());
    }
    let mut total: u64 = 0;
    let mut buf = vec![0u8; CHUNK];
    loop {
        if monotonic() > deadline {
            return Err(install_error("download timed out"));
        }
        let n = resp.read1(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        total += n as u64;
        if total > max_bytes {
            return Err(too_large());
        }
        dest.write_all(&buf[..n])?;
    }
}

/// `default_fetch`'s file handling: creates (truncates) `path`, fetches into it, and unlinks
/// it on any error (module docs).
pub fn fetch_to_path(
    fetch: &dyn Fetch,
    url: &str,
    path: &Path,
    timeout: f64,
) -> Result<(), PyExc> {
    let result = File::create(path)
        .map_err(PyExc::from)
        .and_then(|mut file| {
            fetch.fetch(url, &mut file, timeout)?;
            file.flush()?;
            Ok(())
        });
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

/// `HttpsOnlyRedirect.redirect_request`: the absolute URL a `Location` header names, relative
/// to `current`; an error unless it is `https`.
pub fn redirect_target(current: &str, location: &str) -> Result<String, PyExc> {
    let target = join_url(current, location.trim());
    if !is_https(&target) {
        return Err(url_error("refusing redirect to a non-https URL"));
    }
    Ok(target)
}

/// `urljoin(base, reference)` for the shapes a `Location` header takes.
fn join_url(base: &str, reference: &str) -> String {
    if !scheme(reference).is_empty() {
        return reference.to_string();
    }
    let base_scheme = scheme(base);
    let after_scheme = &base[base_scheme.len() + 1..];
    if let Some(rest) = reference.strip_prefix("//") {
        return format!("{base_scheme}://{rest}");
    }
    let (authority, path) = match after_scheme.strip_prefix("//") {
        Some(rest) => {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        }
        None => ("", after_scheme),
    };
    let origin = format!("{base_scheme}://{authority}");
    if reference.starts_with('/') {
        return format!("{origin}{reference}");
    }
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    if reference.is_empty() {
        return format!("{origin}{path}");
    }
    if reference.starts_with('?') || reference.starts_with('#') {
        return format!("{origin}{path}{reference}");
    }
    let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
    let dir = if dir.is_empty() { "/" } else { dir };
    format!("{origin}{}", remove_dot_segments(&format!("{dir}{reference}")))
}

/// RFC 3986 §5.2.4 on a path (query and fragment kept after the path).
fn remove_dot_segments(path_and_rest: &str) -> String {
    let split = path_and_rest
        .find(['?', '#'])
        .unwrap_or(path_and_rest.len());
    let (path, rest) = path_and_rest.split_at(split);
    let mut out: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    for (i, seg) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        match *seg {
            "." => {
                if last {
                    out.push("");
                }
            }
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    let mut joined = out.join("/");
    if !joined.starts_with('/') {
        joined.insert(0, '/');
    }
    joined + rest
}

/// `urllib.error.URLError(reason)`, as `str()` shows it.
fn url_error(reason: &str) -> PyExc {
    PyExc::Other("URLError", format!("<urlopen error {reason}>"))
}

// --- proxies (urllib's getproxies_environment / proxy_bypass_environment) ----------------

/// The proxy settings an `https` request uses, read from an [`Env`] (module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyEnv {
    /// The `https` proxy, as given.
    pub https: Option<String>,
    /// The `no_proxy` list, as given.
    pub no_proxy: Option<String>,
}

impl ProxyEnv {
    /// `getproxies_environment()`'s `https` and `no` entries: first every `<x>_proxy` variable
    /// in any case with a non-empty value, then the lower-case spellings again, where an empty
    /// value removes the entry.
    pub fn from_env(env: &Env) -> ProxyEnv {
        let mut out = ProxyEnv::default();
        for (name, value) in env.iter() {
            let (Some(name), Some(value)) = (name.to_str(), value.to_str()) else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            match name.to_ascii_lowercase().as_str() {
                "https_proxy" => out.https = Some(value.to_string()),
                "no_proxy" => out.no_proxy = Some(value.to_string()),
                _ => {}
            }
        }
        for (name, slot) in [("https_proxy", &mut out.https), ("no_proxy", &mut out.no_proxy)] {
            if let Some(value) = env.get(name) {
                *slot = if value.is_empty() {
                    None
                } else {
                    Some(value.to_string())
                };
            }
        }
        out
    }

    /// `proxy_bypass_environment(host)`: `host` is the URL's `host[:port]`.
    pub fn bypass(&self, host: &str) -> bool {
        let Some(no_proxy) = &self.no_proxy else {
            return false;
        };
        if no_proxy == "*" {
            return true;
        }
        let host = host.to_lowercase();
        let hostonly = split_port(&host);
        for name in no_proxy.split(',') {
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let name = name.trim_start_matches('.').to_lowercase();
            if hostonly == name || host == name {
                return true;
            }
            let dotted = format!(".{name}");
            if hostonly.ends_with(&dotted) || host.ends_with(&dotted) {
                return true;
            }
        }
        false
    }

    /// The `host:port` (and credentials) of the proxy for a request to `host`, or `None` for a
    /// direct connection.
    pub fn proxy_for(&self, host: &str) -> Option<String> {
        let proxy = self.https.as_deref()?;
        if self.bypass(host) {
            return None;
        }
        // urllib always tunnels through the proxy with plain CONNECT, whatever its scheme.
        let rest = match proxy.find("://") {
            Some(i) => &proxy[i + 3..],
            None => proxy,
        };
        let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
        (!authority.is_empty()).then(|| authority.to_string())
    }
}

/// `_splitport(host)`'s host part.
fn split_port(host: &str) -> &str {
    match host.rfind(':') {
        Some(i) if host[i + 1..].bytes().all(|b| b.is_ascii_digit()) && !host.ends_with(']') => {
            &host[..i]
        }
        _ => host,
    }
}

/// The `host[:port]` of an absolute URL.
fn url_host(url: &str) -> &str {
    let after = url.find("://").map_or(url, |i| &url[i + 3..]);
    let authority = &after[..after.find(['/', '?', '#']).unwrap_or(after.len())];
    authority.rsplit('@').next().unwrap_or(authority)
}

// --- ureq ----------------------------------------------------------------------------------

/// The production [`Fetch`]: ureq with rustls and the OS trust store (module docs).
#[derive(Clone)]
pub struct UreqFetch {
    roots: RootCerts,
    proxy: ProxyEnv,
    max_bytes: u64,
    monotonic: fn() -> f64,
}

/// Seconds on a monotonic clock (`time.monotonic`).
pub fn monotonic() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() + 1.0
}

impl UreqFetch {
    /// The OS trust store, the proxy settings of `env`, [`MAX_DOWNLOAD_BYTES`].
    pub fn new(env: &Env) -> UreqFetch {
        UreqFetch {
            roots: RootCerts::PlatformVerifier,
            proxy: ProxyEnv::from_env(env),
            max_bytes: MAX_DOWNLOAD_BYTES,
            monotonic,
        }
    }

    /// Test only: trust exactly the DER certificates `cas` instead of the OS store.
    pub fn with_test_roots(mut self, cas: &[&[u8]]) -> UreqFetch {
        let certs = cas
            .iter()
            .map(|der| Certificate::from_der(der).to_owned())
            .collect();
        self.roots = RootCerts::Specific(Arc::new(certs));
        self
    }

    /// Another download cap (`MAX_DOWNLOAD_BYTES` patches).
    pub fn with_max_bytes(mut self, max_bytes: u64) -> UreqFetch {
        self.max_bytes = max_bytes;
        self
    }

    /// Another clock (`time.monotonic` patches).
    pub fn with_monotonic(mut self, monotonic: fn() -> f64) -> UreqFetch {
        self.monotonic = monotonic;
        self
    }

    /// The opener this fetch uses.
    pub fn opener(&self) -> UreqOpener {
        UreqOpener {
            roots: self.roots.clone(),
            proxy: self.proxy.clone(),
        }
    }
}

impl Fetch for UreqFetch {
    fn fetch(&self, url: &str, dest: &mut dyn Write, timeout: f64) -> Result<(), PyExc> {
        download(
            &self.opener(),
            url,
            dest,
            timeout,
            self.max_bytes,
            self.monotonic,
        )
    }
}

/// The ureq [`Opener`]: one agent per hop (module docs).
#[derive(Clone)]
pub struct UreqOpener {
    roots: RootCerts,
    proxy: ProxyEnv,
}

/// `timeout` seconds as a [`Duration`]; `socket.settimeout`'s error otherwise.
fn duration(timeout: f64) -> Result<Duration, PyExc> {
    Duration::try_from_secs_f64(timeout)
        .map_err(|_| PyExc::ValueError("Timeout value out of range".into()))
}

impl UreqOpener {
    /// The agent configuration for one request to `host` with `remaining` of the global
    /// budget left.
    pub fn config(&self, host: &str, timeout: Duration, remaining: Duration) -> Config {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(self.roots.clone())
            .build();
        let proxy = self.proxy.proxy_for(host).and_then(|authority| {
            ureq::Proxy::new(&format!("http://{authority}")).ok()
        });
        Agent::config_builder()
            .tls_config(tls)
            .https_only(true)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .proxy(proxy)
            .timeout_resolve(Some(timeout))
            .timeout_connect(Some(timeout))
            .timeout_per_read(Some(timeout))
            .timeout_per_write(Some(timeout))
            .timeout_global(Some(remaining))
            .build()
    }
}

impl Opener for UreqOpener {
    fn open(&self, url: &str, timeout: f64) -> Result<Box<dyn Response>, PyExc> {
        let per_op = duration(timeout)?;
        let total = duration(4.0 * timeout)?;
        let start = Instant::now();
        let mut current = url.to_string();
        let mut hops = 0;
        loop {
            let remaining = total.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Err(timed_out());
            }
            let agent: Agent = self
                .config(url_host(&current), per_op, remaining)
                .new_agent();
            let resp = agent.get(&current).call().map_err(ureq_error)?;
            let status = resp.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = resp
                    .headers()
                    .get("location")
                    .or_else(|| resp.headers().get("uri"))
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let Some(location) = location else {
                    return Err(http_error(status));
                };
                hops += 1;
                if hops > MAX_REDIRECTS {
                    return Err(PyExc::Other(
                        "HTTPError",
                        format!(
                            "HTTP Error {status}: The HTTP server returned a redirect error \
                             that would lead to an infinite loop.\n\
                             The last 30x error message was:\n{}",
                            reason(status)
                        ),
                    ));
                }
                current = redirect_target(&current, &location)?;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(http_error(status));
            }
            let length = resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let reader = resp.into_body().into_reader();
            return Ok(Box::new(UreqResponse {
                url: current,
                length,
                reader: Box::new(reader),
            }));
        }
    }
}

struct UreqResponse {
    url: String,
    length: Option<String>,
    reader: Box<dyn Read>,
}

impl Response for UreqResponse {
    fn final_url(&self) -> String {
        self.url.clone()
    }

    fn content_length(&self) -> Option<String> {
        self.length.clone()
    }

    fn read1(&mut self, buf: &mut [u8]) -> Result<usize, PyExc> {
        loop {
            match self.reader.read(buf) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(io_error(e)),
            }
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        407 => "Proxy Authentication Required",
        410 => "Gone",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

/// `urllib.error.HTTPError`, as `str()` shows it.
fn http_error(status: u16) -> PyExc {
    PyExc::Other("HTTPError", format!("HTTP Error {status}: {}", reason(status)))
}

fn timed_out() -> PyExc {
    PyExc::Other("TimeoutError", "timed out".into())
}

fn io_error(e: io::Error) -> PyExc {
    if e.kind() == io::ErrorKind::TimedOut {
        return timed_out();
    }
    // A ureq error surfaced through the body reader.
    match e.get_ref().and_then(|inner| inner.downcast_ref::<ureq::Error>()) {
        Some(inner) => describe(inner),
        None => PyExc::Other("OSError", e.to_string()),
    }
}

fn ureq_error(e: ureq::Error) -> PyExc {
    describe(&e)
}

fn describe(e: &ureq::Error) -> PyExc {
    match e {
        ureq::Error::Timeout(_) => timed_out(),
        ureq::Error::StatusCode(code) => http_error(*code),
        ureq::Error::Io(io) if io.kind() == io::ErrorKind::TimedOut => timed_out(),
        other => url_error(&other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_agent_uses_the_platform_verifier() {
        let fetch = UreqFetch::new(&Env::default());
        let config = fetch.opener().config(
            "github.com",
            Duration::from_secs(5),
            Duration::from_secs(20),
        );
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::PlatformVerifier
        ));
        assert_eq!(config.tls_config().provider(), TlsProvider::Rustls);
        assert!(config.https_only());
        assert_eq!(config.max_redirects(), 0, "every hop is checked here");
        assert!(!config.http_status_as_error());
        assert_eq!(config.timeout_per_read(), Some(Duration::from_secs(5)));
        assert_eq!(config.timeouts().global, Some(Duration::from_secs(20)));
        assert_eq!(config.timeouts().connect, Some(Duration::from_secs(5)));
        assert!(config.proxy().is_none(), "no proxy unless the Env names one");
    }

    #[test]
    fn test_roots_replace_the_platform_verifier_only_when_asked() {
        let fetch = UreqFetch::new(&Env::default()).with_test_roots(&[b"\x30\x00"]);
        let config =
            fetch
                .opener()
                .config("h", Duration::from_secs(1), Duration::from_secs(4));
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::Specific(_)
        ));
    }

    #[test]
    fn scheme_is_lower_cased_and_needs_a_letter_first() {
        assert_eq!(scheme("HTTPS://x/y"), "https");
        assert_eq!(scheme("file:///etc/passwd"), "file");
        assert_eq!(scheme("/relative"), "");
        assert_eq!(scheme("1http://x"), "");
        assert_eq!(scheme("no-scheme"), "");
    }

    #[test]
    fn join_url_resolves_location_shapes() {
        let base = "https://github.com/o/r/releases/download/t/f.tar.gz?x=1";
        assert_eq!(join_url(base, "https://cdn.example/a"), "https://cdn.example/a");
        assert_eq!(join_url(base, "//cdn.example/a"), "https://cdn.example/a");
        assert_eq!(join_url(base, "/a/b"), "https://github.com/a/b");
        assert_eq!(
            join_url(base, "g.tar.gz"),
            "https://github.com/o/r/releases/download/t/g.tar.gz"
        );
        assert_eq!(
            join_url(base, "../x"),
            "https://github.com/o/r/releases/download/x"
        );
        assert_eq!(join_url("https://h", "a"), "https://h/a");
        assert_eq!(join_url(base, "http://evil/x"), "http://evil/x");
    }

    #[test]
    fn redirect_target_refuses_every_non_https_hop() {
        for loc in ["http://e/b", "ftp://e/b", "file:///etc/passwd", "HTTP://e/b"] {
            let err = redirect_target("https://e/a", loc).unwrap_err();
            assert_eq!(
                err.py_str(),
                "<urlopen error refusing redirect to a non-https URL>"
            );
        }
        assert_eq!(
            redirect_target("https://e/a", "/b").unwrap(),
            "https://e/b"
        );
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs
            .iter()
            .fold(Env::default(), |e, (k, v)| e.with(k, v))
    }

    #[test]
    fn proxy_env_prefers_lower_case_and_an_empty_lower_case_removes() {
        let p = ProxyEnv::from_env(&env(&[("HTTPS_PROXY", "http://up:1")]));
        assert_eq!(p.https.as_deref(), Some("http://up:1"));
        let p = ProxyEnv::from_env(&env(&[
            ("HTTPS_PROXY", "http://up:1"),
            ("https_proxy", "http://low:2"),
        ]));
        assert_eq!(p.https.as_deref(), Some("http://low:2"));
        let p = ProxyEnv::from_env(&env(&[("HTTPS_PROXY", "http://up:1"), ("https_proxy", "")]));
        assert_eq!(p.https, None);
        let p = ProxyEnv::from_env(&env(&[("HTTP_PROXY", "http://h:3"), ("ALL_PROXY", "x:4")]));
        assert_eq!(p.https, None, "only the https entry applies to an https URL");
        let p = ProxyEnv::from_env(&env(&[("NO_PROXY", "a"), ("no_proxy", "b")]));
        assert_eq!(p.no_proxy.as_deref(), Some("b"));
    }

    #[test]
    fn no_proxy_matches_as_urllib_does() {
        let p = ProxyEnv {
            https: Some("proxy:3128".into()),
            no_proxy: Some(" .Example.com, localhost:8443 ,10.0.0.1".into()),
        };
        assert!(p.bypass("example.com"));
        assert!(p.bypass("cdn.EXAMPLE.com"));
        assert!(p.bypass("cdn.example.com:443"));
        assert!(!p.bypass("badexample.com"));
        assert!(p.bypass("localhost:8443"));
        assert!(!p.bypass("localhost:9"));
        assert!(p.bypass("10.0.0.1"));
        assert!(!p.bypass("github.com"));
        assert_eq!(p.proxy_for("github.com").as_deref(), Some("proxy:3128"));
        assert_eq!(p.proxy_for("x.example.com"), None);
        let star = ProxyEnv {
            https: Some("http://u:p@proxy:3128/".into()),
            no_proxy: Some("*".into()),
        };
        assert_eq!(star.proxy_for("github.com"), None);
        let no_list = ProxyEnv {
            https: Some("https://u:p@proxy:3128/".into()),
            no_proxy: None,
        };
        assert_eq!(no_list.proxy_for("github.com").as_deref(), Some("u:p@proxy:3128"));
    }

    #[test]
    fn url_host_drops_credentials_and_path() {
        assert_eq!(url_host("https://u:p@h:8443/x?y"), "h:8443");
        assert_eq!(url_host("https://h"), "h");
    }

    #[test]
    fn bad_timeouts_are_value_errors() {
        for t in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                duration(t).unwrap_err().py_str(),
                "Timeout value out of range"
            );
        }
    }
}
