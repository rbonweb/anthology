//! urllib's redirect handling for `http_get`: Python 3.12's
//! `HTTPRedirectHandler.http_error_302` and the `urllib.parse` functions it calls (`urlparse`,
//! `urlunparse`, `quote`, `urljoin`), plus vendor.py's `_HttpsOnly` check.
//!
//! The parts this port does not reproduce are listed in the report (D-2): `_checknetloc`'s NFKC
//! test of a non-ASCII host, `_check_bracketed_netloc`'s IPv6 checks beyond a missing bracket,
//! and the reason phrase (the canonical one is used, not the server's).

use super::VendorError;

const USES_RELATIVE: &[&str] = &[
    "", "ftp", "http", "gopher", "nntp", "imap", "wais", "file", "https", "shttp", "mms",
    "prospero", "rtsp", "rtsps", "rtspu", "sftp", "svn", "svn+ssh", "ws", "wss",
];
const USES_NETLOC: &[&str] = &[
    "",
    "ftp",
    "http",
    "gopher",
    "nntp",
    "telnet",
    "imap",
    "wais",
    "file",
    "mms",
    "https",
    "shttp",
    "snews",
    "prospero",
    "rtsp",
    "rtsps",
    "rtspu",
    "rsync",
    "svn",
    "svn+ssh",
    "sftp",
    "nfs",
    "git",
    "git+ssh",
    "ws",
    "wss",
    "itms-services",
];
const USES_PARAMS: &[&str] = &[
    "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp", "rtsps", "rtspu",
    "sip", "sips", "mms", "sftp", "tel",
];

/// `HTTPRedirectHandler.max_repeats` and `.max_redirections`.
pub const MAX_REPEATS: usize = 4;
pub const MAX_REDIRECTIONS: usize = 10;
/// `HTTPRedirectHandler.inf_msg`.
pub const INF_MSG: &str = "The HTTP server returned a redirect error that would lead to an \
                           infinite loop.\nThe last 30x error message was:\n";

/// A `urlparse` result.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parts {
    pub scheme: String,
    pub netloc: String,
    pub path: String,
    pub params: String,
    pub query: String,
    pub fragment: String,
}

/// `urlsplit(url, scheme)` (params left in the path). `Err` is the ValueError text.
fn urlsplit(url: &str, default_scheme: &str) -> Result<Parts, String> {
    let unsafe_chars = ['\t', '\r', '\n'];
    let url = url.trim_start_matches(|c: char| c <= ' ');
    let mut url: String = url.replace(unsafe_chars, "");
    let mut scheme = default_scheme
        .trim_matches(|c: char| c <= ' ')
        .replace(unsafe_chars, "");
    if let Some(i) = url.find(':') {
        let head = &url[..i];
        let scheme_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.');
        if head.starts_with(|c: char| c.is_ascii_alphabetic()) && head.chars().all(scheme_char) {
            scheme = head.to_ascii_lowercase();
            url = url[i + 1..].to_string();
        }
    }
    let mut netloc = String::new();
    if let Some(rest) = url.strip_prefix("//") {
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        netloc = rest[..end].to_string();
        url = rest[end..].to_string();
        if netloc.contains('[') != netloc.contains(']') {
            return Err("Invalid IPv6 URL".to_string());
        }
    }
    let mut fragment = String::new();
    if let Some((a, b)) = url.split_once('#') {
        fragment = b.to_string();
        url = a.to_string();
    }
    let mut query = String::new();
    if let Some((a, b)) = url.split_once('?') {
        query = b.to_string();
        url = a.to_string();
    }
    Ok(Parts {
        scheme,
        netloc,
        path: url,
        params: String::new(),
        query,
        fragment,
    })
}

/// `urlparse(url, scheme)`.
pub fn urlparse(url: &str, default_scheme: &str) -> Result<Parts, String> {
    let mut parts = urlsplit(url, default_scheme)?;
    if USES_PARAMS.contains(&parts.scheme.as_str()) && parts.path.contains(';') {
        // `_splitparams`: the params start at the first `;` of the last segment.
        let i = match parts.path.rfind('/') {
            Some(slash) => parts.path[slash..].find(';').map(|j| slash + j),
            None => parts.path.find(';'),
        };
        if let Some(i) = i {
            parts.params = parts.path[i + 1..].to_string();
            parts.path.truncate(i);
        }
    }
    Ok(parts)
}

/// `urlunparse(parts)` (3.12's `urlunsplit`).
pub fn urlunparse(p: &Parts) -> String {
    let mut url = p.path.clone();
    if !p.params.is_empty() {
        url = format!("{url};{}", p.params);
    }
    if !p.netloc.is_empty() {
        if !url.is_empty() && !url.starts_with('/') {
            url.insert(0, '/');
        }
        url = format!("//{}{url}", p.netloc);
    } else if url.starts_with("//")
        || (!p.scheme.is_empty()
            && USES_NETLOC.contains(&p.scheme.as_str())
            && (url.is_empty() || url.starts_with('/')))
    {
        url = format!("//{url}");
    }
    if !p.scheme.is_empty() {
        url = format!("{}:{url}", p.scheme);
    }
    if !p.query.is_empty() {
        url = format!("{url}?{}", p.query);
    }
    if !p.fragment.is_empty() {
        url = format!("{url}#{}", p.fragment);
    }
    url
}

/// `urljoin(base, url)`.
pub fn urljoin(base: &str, url: &str) -> Result<String, String> {
    if base.is_empty() {
        return Ok(url.to_string());
    }
    if url.is_empty() {
        return Ok(base.to_string());
    }
    let b = urlparse(base, "")?;
    let mut u = urlparse(url, &b.scheme)?;
    if u.scheme != b.scheme || !USES_RELATIVE.contains(&u.scheme.as_str()) {
        return Ok(url.to_string());
    }
    if USES_NETLOC.contains(&u.scheme.as_str()) {
        if !u.netloc.is_empty() {
            return Ok(urlunparse(&u));
        }
        u.netloc = b.netloc.clone();
    }
    if u.path.is_empty() && u.params.is_empty() {
        u.path = b.path.clone();
        u.params = b.params.clone();
        if u.query.is_empty() {
            u.query = b.query.clone();
        }
        return Ok(urlunparse(&u));
    }
    let mut base_parts: Vec<&str> = b.path.split('/').collect();
    if base_parts.last() != Some(&"") {
        base_parts.pop();
    }
    let segments: Vec<&str> = if u.path.starts_with('/') {
        u.path.split('/').collect()
    } else {
        let mut all: Vec<&str> = base_parts;
        all.extend(u.path.split('/'));
        // `segments[1:-1] = filter(None, segments[1:-1])`
        if all.len() > 2 {
            let last = all.len() - 1;
            let middle: Vec<&str> = all[1..last]
                .iter()
                .copied()
                .filter(|s| !s.is_empty())
                .collect();
            let mut out = vec![all[0]];
            out.extend(middle);
            out.push(all[last]);
            out
        } else {
            all
        }
    };
    let mut resolved: Vec<&str> = Vec::new();
    for seg in &segments {
        match *seg {
            ".." => {
                resolved.pop();
            }
            "." => {}
            s => resolved.push(s),
        }
    }
    if matches!(segments.last(), Some(&".") | Some(&"..")) {
        resolved.push("");
    }
    let path = resolved.join("/");
    u.path = if path.is_empty() {
        "/".to_string()
    } else {
        path
    };
    Ok(urlunparse(&u))
}

/// `quote(s, encoding="iso-8859-1", safe=string.punctuation)` of a str whose chars are all
/// below U+0100 (a header value decoded as Latin-1).
fn quote_latin1(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c.is_ascii_punctuation() {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", c as u32 & 0xff));
        }
    }
    out
}

/// Why a redirect is not followed.
pub enum Stop {
    /// An `OSError`/`ValueError` inside urlopen: `cannot download <url>: <text>`.
    Failed(String),
    /// vendor.py's own `VendorError`, raised as is.
    Refused(VendorError),
}

/// `http_error_302` up to `redirect_request`: the next URL of a redirect from `current` with
/// a `Location` (or `URI`) header value decoded as Latin-1, then `_HttpsOnly`'s check.
pub fn next_url(current: &str, location: &str, code: u16, msg: &str) -> Result<String, Stop> {
    let mut parts = urlparse(location, "").map_err(Stop::Failed)?;
    if !matches!(parts.scheme.as_str(), "http" | "https" | "ftp" | "") {
        return Err(Stop::Failed(format!(
            "HTTP Error {code}: {msg} - Redirection to url '{location}' is not allowed"
        )));
    }
    if parts.path.is_empty() && !parts.netloc.is_empty() {
        parts.path = "/".to_string();
    }
    let quoted = quote_latin1(&urlunparse(&parts));
    let next = urljoin(current, &quoted).map_err(Stop::Failed)?;
    if !next.to_lowercase().starts_with("https://") {
        return Err(Stop::Refused(VendorError(format!(
            "refusing a redirect to a non-https URL: {next}"
        ))));
    }
    Ok(next.replace(' ', "%20"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (base, location) → what Python 3.12's handler does (`python:3.12-slim`, `python3 -I`):
    /// the next URL, `F:` + the urlopen error text, or `R:` + the VendorError text.
    const CASES: &[(&str, &str, &str)] = &[
        (
            "https://h.test/a/b/file?x=1",
            "/p?next=http://evil",
            "https://h.test/p?next=http://evil",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "p?u=https://x",
            "https://h.test/a/b/p?u=https://x",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "?q=1",
            "https://h.test/a/b/file?q=1",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "#f",
            "https://h.test/a/b/file?x=1#f",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "../up",
            "https://h.test/a/up",
        ),
        ("https://h.test/a/b/file?x=1", "./", "https://h.test/a/b/"),
        (
            "https://h.test/a/b/file?x=1",
            "../../../../x",
            "https://h.test/x",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "a//b/./c/..",
            "https://h.test/a/b/a/b/",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "http:/foo",
            "R:refusing a redirect to a non-https URL: http:///foo",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "http://h.test/x",
            "R:refusing a redirect to a non-https URL: http://h.test/x",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "a b",
            "https://h.test/a/b/a%20b",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "caf\u{e9}",
            "https://h.test/a/b/caf%E9",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "file:///x",
            "F:HTTP Error 302: Found - Redirection to url 'file:///x' is not allowed",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "javascript:x",
            "F:HTTP Error 302: Found - Redirection to url 'javascript:x' is not allowed",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "//other.test",
            "https://other.test/",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "https://other.test",
            "https://other.test/",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "HTTPS://Other.test/P",
            "https://Other.test/P",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "https:rel",
            "https://h.test/a/b/rel",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "  /lead",
            "https://h.test/lead",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "/t\tab\n",
            "https://h.test/tab",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "x;p",
            "https://h.test/a/b/x;p",
        ),
        ("https://h.test/a/b/file?x=1", "x;", "https://h.test/a/b/x"),
        ("https://h.test/a/b/file?x=1", "/p?", "https://h.test/p"),
        (
            "https://h.test/a/b/file?x=1",
            "https://[::1",
            "F:Invalid IPv6 URL",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "ftp://h.test/x",
            "R:refusing a redirect to a non-https URL: ftp://h.test/x",
        ),
        (
            "https://h.test/a/b/file?x=1",
            "1abc:x",
            "https://h.test/a/b/1abc:x",
        ),
        ("https://h.test", "x", "https://h.test/x"),
        ("https://h.test/dir/", "..", "https://h.test/"),
        ("https://h.test/a;p/b;q", ";z", "https://h.test/a;p/;z"),
        ("https://h.test/a;p/b;q", "", "https://h.test/a;p/b;q"),
        (
            "https://h.test/a/b",
            "//u@h2.test:8443?x#y",
            "https://u@h2.test:8443/?x#y",
        ),
        ("https://h.test/a/b", "///triple", "https://h.test/triple"),
        ("https://h.test/a/b", "https:///x", "https://h.test/x"),
        ("https://h.test/a/b", "https:", "https://h.test/a/b"),
        ("https://h.test/a/b", "https:?q", "https://h.test/a/b?q"),
        ("https://h.test/a/b", "\u{1}\u{2}/ctl", "https://h.test/ctl"),
        ("https://h.test/a/b", "/%41%zz", "https://h.test/%41%zz"),
        (
            "https://h.test/a/b",
            "/x\u{7f}\u{ff}",
            "https://h.test/x%7F%FF",
        ),
        ("https://h.test/a/b/", "../../../", "https://h.test/"),
        ("https://h.test/a/b/", "c/../..", "https://h.test/a/"),
    ];

    #[test]
    fn next_url_matches_python_312() {
        for (base, location, want) in CASES {
            let got = match next_url(base, location, 302, "Found") {
                Ok(u) => u,
                Err(Stop::Failed(t)) => format!("F:{t}"),
                Err(Stop::Refused(e)) => format!("R:{}", e.0),
            };
            assert_eq!(&got, want, "{base} + {location:?}");
        }
    }
}
