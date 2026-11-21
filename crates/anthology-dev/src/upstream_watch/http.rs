//! The upstream watch's HTTP transport: `urllib.request.build_opener(_DropAuthOffHost())`.
//!
//! - **TLS.** ureq with rustls and the OS trust store: [`Http::new`] sets
//!   `RootCerts::PlatformVerifier` explicitly (ureq's default is WebPki). Only the test seam
//!   [`Http::with_test_roots`] trusts anything else (the testkit CA).
//! - **Redirects** are followed here (ureq's own are off), as urllib's `HTTPRedirectHandler`:
//!   301, 302, 303, 307 and 308 for `GET`; 301, 302 and 303 for `POST`, which becomes a `GET`
//!   without its body; anything else (a `PATCH`, a `POST` on 307/308, a redirect without
//!   `Location`, to a scheme other than http or https, or a loop past 4 visits of one URL or
//!   10 redirects) is an `HTTPError`. The content headers are dropped on every hop, and
//!   [`redirect_headers`] drops `Authorization` whenever the scheme or the host (`netloc`)
//!   changes (`_DropAuthOffHost`).
//! - **Proxies** come from the [`Env`], with urllib's rules: `<scheme>_proxy` (lower case
//!   wins), and `no_proxy` as `proxy_bypass_environment` reads it. The proxy is reached with
//!   plain HTTP. This is RT2's `fetch::ProxyEnv`, for both schemes; anthology-dev cannot depend
//!   on anthology-runtime (F1's `Cargo.toml`), so the logic is repeated here.
//! - **Timeouts.** 30 s for each socket operation (`timeout=HTTP_TIMEOUT`), no overall cap.
//! - **Errors** carry the Python class name `type(exc).__name__` would show: `HTTPError` for a
//!   status outside 2xx, `URLError` for a failure before the request was sent (DNS, connect,
//!   TLS, proxy), `TimeoutError` for a timeout after it, and the `OSError` subclass of an I/O
//!   error otherwise. Ruling DV3-R6: the real-network classes are an approximation; the fakes
//!   the tests and the difftest inject carry exact class names.
#![forbid(unsafe_code)]

use std::io::{self, Read};
use std::sync::Arc;
use std::time::Duration;

use anthology_core::Env;
use anthology_pycompat::pyerr::PyExc;
use ureq::Agent;
use ureq::config::Config;
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

use super::{Error, HTTP_TIMEOUT, USER_AGENT};

/// urllib's `HTTPRedirectHandler.max_repeats`: the same URL at most this many times.
pub const MAX_REPEATS: usize = 4;
/// urllib's `HTTPRedirectHandler.max_redirections`.
pub const MAX_REDIRECTIONS: usize = 10;

/// A request method the watcher uses.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Patch,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Patch => "PATCH",
        }
    }
}

/// The opener (module docs).
#[derive(Clone)]
pub struct Http {
    roots: RootCerts,
    proxy: ProxyEnv,
}

impl Http {
    /// The OS trust store and the proxy settings of `env`.
    pub fn new(env: &Env) -> Http {
        Http {
            roots: RootCerts::PlatformVerifier,
            proxy: ProxyEnv::from_env(env),
        }
    }

    /// **Test only**: trust exactly the DER certificates `cas` (the testkit CA,
    /// `TestServer::ca_der()`) instead of the OS store. macOS's verifier refuses the testkit's
    /// 100-year leaf, so the TLS tests need this seam; production keeps
    /// `RootCerts::PlatformVerifier`.
    #[doc(hidden)]
    pub fn with_test_roots(mut self, cas: &[&[u8]]) -> Http {
        let certs = cas
            .iter()
            .map(|der| Certificate::from_der(der).to_owned())
            .collect();
        self.roots = RootCerts::Specific(Arc::new(certs));
        self
    }

    /// The agent configuration for one request to `url`. A proxy setting ureq cannot parse is
    /// an error, never a direct connection.
    pub fn config(&self, url: &str) -> Result<Config, Error> {
        let timeout = Some(Duration::from_secs(HTTP_TIMEOUT));
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(self.roots.clone())
            .build();
        let (scheme, netloc) = scheme_netloc(url);
        let proxy = match self.proxy.proxy_for(&scheme, host_port(&netloc)) {
            None => None,
            Some(authority) => Some(
                ureq::Proxy::new(&format!("http://{authority}"))
                    .map_err(|_| Error::Other("URLError".into()))?,
            ),
        };
        // `config_builder()` starts from ureq's defaults, which read the process environment's
        // proxy variables; `.proxy(...)` always replaces that, so only the `Env` decides.
        Ok(Agent::config_builder()
            .tls_config(tls)
            .https_only(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .proxy(proxy)
            .timeout_resolve(timeout)
            .timeout_connect(timeout)
            .timeout_per_read(timeout)
            .timeout_per_write(timeout)
            .build())
    }

    /// `opener.open(Request(url, data, headers, method), timeout=30).read(limit)`: the body of
    /// the final response, at most `limit` bytes (the rest is not read).
    pub fn open(
        &self,
        method: Method,
        url: &str,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
        limit: u64,
    ) -> Result<Vec<u8>, Error> {
        let mut method = method;
        let mut url = url.to_string();
        let mut headers = headers;
        let mut body = body;
        // `redirect_dict`: how often each redirect target was visited.
        let mut visited: Vec<(String, usize)> = Vec::new();
        loop {
            let agent: Agent = self.config(&url)?.new_agent();
            let resp = send(&agent, method, &url, &headers, body.as_deref())?;
            let status = resp.status().as_u16();
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                let location = resp
                    .headers()
                    .get("location")
                    .or_else(|| resp.headers().get("uri"))
                    .map(|v| location_text(v.as_bytes()));
                let Some(location) = location else {
                    return Err(http_error());
                };
                let target = redirect_target(&url, &location)?;
                method = redirect_method(method, status)?;
                if !visited.is_empty() {
                    let seen = visited
                        .iter()
                        .find(|(u, _)| *u == target)
                        .map_or(0, |(_, n)| *n);
                    if seen >= MAX_REPEATS || visited.len() >= MAX_REDIRECTIONS {
                        return Err(http_error());
                    }
                }
                match visited.iter_mut().find(|(u, _)| *u == target) {
                    Some((_, n)) => *n += 1,
                    None => visited.push((target.clone(), 1)),
                }
                headers = redirect_headers(&url, &target, headers);
                body = None;
                url = target;
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(http_error());
            }
            let mut data = Vec::new();
            resp.into_body()
                .into_reader()
                .take(limit)
                .read_to_end(&mut data)
                .map_err(|e| io_error(&e))?;
            return Ok(data);
        }
    }
}

fn send(
    agent: &Agent,
    method: Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> Result<ureq::http::Response<ureq::Body>, Error> {
    let result = match (method, body) {
        (Method::Get, _) => {
            let mut req = agent.get(url);
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.call()
        }
        (Method::Post | Method::Patch, body) => {
            let mut req = if method == Method::Post {
                agent.post(url)
            } else {
                agent.patch(url)
            };
            for (k, v) in headers {
                req = req.header(k.as_str(), v.as_str());
            }
            req.send(body.unwrap_or_default())
        }
    };
    result.map_err(|e| describe(&e))
}

/// A `Location` header's bytes as urllib reads them (Latin-1, then `quote(...,
/// encoding="iso-8859-1", safe=punctuation)`): surrounding whitespace trimmed, and every byte
/// outside visible ASCII percent-encoded, so a non-ASCII target is followed, not refused.
pub fn location_text(raw: &[u8]) -> String {
    let start = raw
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(raw.len());
    let end = raw
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |i| i + 1);
    let mut out = String::with_capacity(end - start);
    for &b in &raw[start..end] {
        if b.is_ascii_graphic() {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `redirect_request`'s method rule: `GET` stays `GET`; a `POST` on 301, 302 or 303 becomes a
/// `GET`; anything else raises `HTTPError`.
pub fn redirect_method(method: Method, status: u16) -> Result<Method, Error> {
    match (method, status) {
        (Method::Get, 301 | 302 | 303 | 307 | 308) => Ok(Method::Get),
        (Method::Post, 301..=303) => Ok(Method::Get),
        _ => Err(http_error()),
    }
}

/// `http_error_302`'s target: the `Location` resolved against `current` (`urljoin`), with
/// spaces and bytes outside printable ASCII percent-encoded (`quote(..., safe=punctuation)`).
/// A scheme other than http or https is an `HTTPError`.
pub fn redirect_target(current: &str, location: &str) -> Result<String, Error> {
    let scheme = scheme(location);
    if !scheme.is_empty() && scheme != "http" && scheme != "https" {
        return Err(http_error());
    }
    let joined = join_url(current, location);
    let mut out = String::with_capacity(joined.len());
    for b in joined.bytes() {
        if b.is_ascii_graphic() {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(out)
}

/// The headers of the next hop (`redirect_request` with `_DropAuthOffHost`): the content
/// headers are dropped, and so is `Authorization` (any case) when the scheme or the `netloc`
/// of `new` differs from `old`'s.
pub fn redirect_headers(
    old: &str,
    new: &str,
    headers: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let off_host = scheme_netloc(old) != scheme_netloc(new);
    headers
        .into_iter()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            k != "content-length" && k != "content-type" && !(off_host && k == "authorization")
        })
        .collect()
}

/// `(urlparse(url).scheme, urlparse(url).netloc)`: the scheme lower-cased, the authority as
/// written.
pub fn scheme_netloc(url: &str) -> (String, String) {
    let s = scheme(url);
    let rest = if s.is_empty() {
        url
    } else {
        &url[s.len() + 1..]
    };
    let netloc = match rest.strip_prefix("//") {
        Some(r) => &r[..r.find(['/', '?', '#']).unwrap_or(r.len())],
        None => "",
    };
    (s, netloc.to_string())
}

/// The `host[:port]` of a `netloc` (credentials dropped).
fn host_port(netloc: &str) -> &str {
    netloc.rsplit('@').next().unwrap_or(netloc)
}

/// The scheme of `url`, lower-cased; empty when there is none.
fn scheme(url: &str) -> String {
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
        return format!("{origin}{}", remove_dot_segments(reference));
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
    format!(
        "{origin}{}",
        remove_dot_segments(&format!("{dir}{reference}"))
    )
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

fn http_error() -> Error {
    Error::Other("HTTPError".into())
}

fn io_error(e: &io::Error) -> Error {
    if e.kind() == io::ErrorKind::TimedOut {
        return Error::Other("TimeoutError".into());
    }
    match e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
    {
        Some(inner) => describe(inner),
        None => Error::Other(PyExc::from(io::Error::new(e.kind(), "")).type_name().into()),
    }
}

/// The Python class of a ureq failure (module docs).
fn describe(e: &ureq::Error) -> Error {
    use ureq::Timeout as T;
    let name = match e {
        ureq::Error::Timeout(
            T::Resolve | T::Connect | T::SendRequest | T::SendBody | T::PerWrite,
        ) => "URLError",
        ureq::Error::Timeout(_) => "TimeoutError",
        ureq::Error::Io(io) => {
            return match io.kind() {
                io::ErrorKind::TimedOut => Error::Other("TimeoutError".into()),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound => {
                    Error::Other("URLError".into())
                }
                _ => io_error(io),
            };
        }
        ureq::Error::StatusCode(_) => "HTTPError",
        ureq::Error::Protocol(_) => "RemoteDisconnected",
        _ => "URLError",
    };
    Error::Other(name.into())
}

// --- proxies (urllib's getproxies_environment / proxy_bypass_environment) ------------------

/// The proxy settings, read from an [`Env`] (module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxyEnv {
    /// The `http` proxy, as given.
    pub http: Option<String>,
    /// The `https` proxy, as given.
    pub https: Option<String>,
    /// The `no_proxy` list, as given.
    pub no_proxy: Option<String>,
}

impl ProxyEnv {
    /// `getproxies_environment()`'s `http`, `https` and `no` entries: first every `<x>_proxy`
    /// variable in any case with a non-empty value, then again every name ending in exactly
    /// `_proxy`, where an empty value removes the entry.
    pub fn from_env(env: &Env) -> ProxyEnv {
        let mut out = ProxyEnv::default();
        for (name, value) in env.iter() {
            let (Some(name), Some(value)) = (name.to_str(), value.to_str()) else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            if let Some(slot) = out.slot(&name.to_ascii_lowercase()) {
                *slot = Some(value.to_string());
            }
        }
        for (name, value) in env.iter() {
            let (Some(name), Some(value)) = (name.to_str(), value.to_str()) else {
                continue;
            };
            if !name.ends_with("_proxy") {
                continue;
            }
            if let Some(slot) = out.slot(&name.to_ascii_lowercase()) {
                *slot = (!value.is_empty()).then(|| value.to_string());
            }
        }
        out
    }

    fn slot(&mut self, lower: &str) -> Option<&mut Option<String>> {
        match lower {
            "http_proxy" => Some(&mut self.http),
            "https_proxy" => Some(&mut self.https),
            "no_proxy" => Some(&mut self.no_proxy),
            _ => None,
        }
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

    /// The `host:port` (and credentials) of the proxy for a `scheme` request to `host`, or
    /// `None` for a direct connection.
    pub fn proxy_for(&self, scheme: &str, host: &str) -> Option<String> {
        let proxy = match scheme {
            "http" => self.http.as_deref()?,
            "https" => self.https.as_deref()?,
            _ => return None,
        };
        if self.bypass(host) {
            return None;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_agent_uses_the_platform_verifier() {
        let config = Http::new(&Env::default())
            .config("https://api.github.com/x")
            .expect("config");
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::PlatformVerifier
        ));
        assert_eq!(config.tls_config().provider(), TlsProvider::Rustls);
        assert_eq!(config.max_redirects(), 0, "every hop is checked here");
        assert!(!config.http_status_as_error());
        assert_eq!(config.timeout_per_read(), Some(Duration::from_secs(30)));
        assert_eq!(config.timeouts().connect, Some(Duration::from_secs(30)));
        assert!(
            config.proxy().is_none(),
            "no proxy unless the Env names one"
        );
    }

    #[test]
    fn test_roots_replace_the_platform_verifier_only_when_asked() {
        let config = Http::new(&Env::default())
            .with_test_roots(&[b"\x30\x00"])
            .config("https://h/x")
            .expect("config");
        assert!(matches!(
            config.tls_config().root_certs(),
            RootCerts::Specific(_)
        ));
    }

    #[test]
    fn proxies_follow_the_scheme_and_no_proxy() {
        let env = Env::default()
            .with("https_proxy", "http://p:3128")
            .with("HTTP_PROXY", "http://q:8080/")
            .with("no_proxy", "example.test, .internal");
        let p = ProxyEnv::from_env(&env);
        assert_eq!(
            p.proxy_for("https", "api.github.com"),
            Some("p:3128".into())
        );
        assert_eq!(p.proxy_for("http", "api.github.com"), Some("q:8080".into()));
        assert_eq!(p.proxy_for("https", "example.test:443"), None);
        assert_eq!(p.proxy_for("https", "a.internal"), None);
        let config = Http::new(&env)
            .config("https://api.github.com/")
            .expect("config");
        assert!(config.proxy().is_some());
        let bad = Env::default().with("HTTPS_PROXY", "http://[bad");
        assert!(Http::new(&bad).config("https://h/").is_err());
    }

    #[test]
    fn redirect_rules() {
        assert_eq!(redirect_method(Method::Get, 307), Ok(Method::Get));
        assert_eq!(redirect_method(Method::Post, 303), Ok(Method::Get));
        assert!(redirect_method(Method::Post, 307).is_err());
        assert!(redirect_method(Method::Patch, 302).is_err());
        assert_eq!(
            redirect_target("https://a.test/x/y", "../z?q=1").as_deref(),
            Ok("https://a.test/z?q=1")
        );
        assert_eq!(
            redirect_target("https://a.test/x", "//b.test/p q").as_deref(),
            Ok("https://b.test/p%20q")
        );
        assert!(redirect_target("https://a.test/", "file:///etc/passwd").is_err());
        assert_eq!(location_text(b" /caf\xe9 x\t"), "/caf%E9%20x");
        assert_eq!(
            scheme_netloc("HTTPS://u@h.test:8443/p?q"),
            ("https".to_string(), "u@h.test:8443".to_string())
        );
        let h = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "x".to_string()),
        ];
        assert_eq!(
            redirect_headers("https://a/", "https://a/b", h),
            vec![("Accept".to_string(), "x".to_string())]
        );
    }
}
