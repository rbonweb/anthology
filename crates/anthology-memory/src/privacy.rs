//! The privacy scan for notes that would be written into repo memory, which may be public (MP1;
//! a port of `memory_privacy.py`, plan GC-12).
//!
//! [`privacy_findings`] returns `{kind, line}` pairs: labels and 1-based line numbers only, never
//! the matched text. Every kind blocks a write into repo memory. The kinds are Python's
//! (`home-path`, `email`, `private-ip`, `ip-address`, `private-hostname`, `ssh-target`,
//! `ssh-key-path`, `secret:<label>`), plus `unscannable` (I-9): a line the regex engine could
//! not finish. That is fail-closed (D-7): Rust never reports fewer findings than Python.
//!
//! Semantics are CPython 3.12's: every pattern keeps its Python source ([`SECRET_PATTERNS`] and
//! the module patterns below, verbatim) and goes through one strict translator (`pyre`) that
//! uses CPython's `\w`, `\s`, `\d` and word boundary and its IGNORECASE rules. Patterns with a
//! look-around after translation run on `fancy-regex`; the rest on `regex` (GC-16).
//!
//! **Limits.** The input may be hostile (an imported archive); every pattern is linear, as in
//! Python. fancy-regex counts backtracks over a whole search, about 1 to 9 per byte of a line
//! for these patterns, so a fixed limit would make every long benign line unscannable. A line
//! of `n` bytes gets `max(backtrack_limit, backtrack_per_byte * 2^ceil(log2 n))` (power-of-two
//! tiers, each compiled once): a quadratic blow-up still trips it quickly. fancy-regex also has a
//! fixed backtrack stack (`StackOverflow`), which a greedy run of about a million characters of
//! one class reaches; that line is unscannable too. Any engine error on a line is an
//! `unscannable` finding on it, and every other check on the line still runs.
//!
//! It is an accident guard, not a guarantee: it knows common formats only. `SECRET_PATTERNS` is
//! the one secret-pattern list on the anthology side; a test keeps every guard G6 pattern in it.
//! It uses no other anthology-memory module.

/// CPython 3.12's `\w`, `\s` and `\d` (generated; public for the difftest's table check). Kept
/// exactly as the generator writes it, so a regeneration is byte-for-byte comparable.
#[rustfmt::skip]
pub mod pyclass;
mod pyre;

use std::collections::{BTreeSet, HashMap};
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex, OnceLock};

use anthology_core::Env;

/// One finding: a kind and a 1-based line number, never the matched text.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Finding {
    pub kind: String,
    pub line: usize,
}

/// The kind of a line the scan could not finish (I-9, GC-12).
pub const UNSCANNABLE: &str = "unscannable";

/// Seams and limits (GC-19).
#[derive(Clone, Debug)]
pub struct PrivacyDeps {
    /// `socket.gethostname()`; `None` when the lookup fails (Python then uses "").
    pub hostname: fn() -> Option<String>,
    /// `os.environ.get("HOME")`: used when the `home` argument is `None`.
    pub home: Option<String>,
    pub limits: PrivacyLimits,
}

impl PrivacyDeps {
    /// Production deps: `HOME` from `env`, the machine's host name, the default limits.
    pub fn from_env(env: &Env) -> PrivacyDeps {
        PrivacyDeps {
            hostname: machine_hostname,
            home: env.get("HOME").map(str::to_string),
            limits: PrivacyLimits::default(),
        }
    }
}

/// The backtrack budget of one search over one line (see the module doc).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PrivacyLimits {
    pub backtrack_limit: usize,
    pub backtrack_per_byte: usize,
}

impl Default for PrivacyLimits {
    fn default() -> PrivacyLimits {
        PrivacyLimits {
            // fancy-regex's own default (GC-16).
            backtrack_limit: 1_000_000,
            backtrack_per_byte: 64,
        }
    }
}

impl PrivacyLimits {
    /// The budget of a search over a line of `len` bytes.
    fn for_len(&self, len: usize) -> usize {
        let tier = len.max(1).checked_next_power_of_two().unwrap_or(usize::MAX);
        self.backtrack_limit
            .max(self.backtrack_per_byte.saturating_mul(tier))
    }
}

fn machine_hostname() -> Option<String> {
    anthology_core::sys::hostname().map(|h| h.to_string_lossy().into_owned())
}

/// `(label, Python pattern source, re.IGNORECASE)`: the anthology-guardrails G6 patterns, copied
/// exactly (a test checks it), then the export/import patterns, then a few more common token
/// formats. Verbatim from `memory_privacy.SECRET_PATTERNS`, as Python concatenates them.
pub const SECRET_PATTERNS: &[(&str, &str, bool)] = &[
    ("aws-access-key", r#"(?:AKIA|ASIA)[0-9A-Z]{16}"#, false),
    ("github-token", r#"\bgh[pousr]_[A-Za-z0-9]+"#, false),
    ("github-token", r#"github_pat_"#, false),
    ("api-key", r#"\bsk-[A-Za-z0-9_-]{16,}"#, false),
    ("slack-token", r#"\bxox[baprs]-"#, false),
    (
        "private-key",
        r#"-----BEGIN [A-Z ]*PRIVATE KEY-----"#,
        false,
    ),
    (
        "credential-assignment",
        r#"password(?:\s*[:=](?![ \t]*(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))|\\?[\"']\s*[:=]\s*(?!(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))(?:\\?[\"'])?(?![\\\"'{\[\]},\s])\S)"#,
        true,
    ),
    (
        "credential-assignment",
        r#"api[_-]?key(?:\s*[:=](?![ \t]*(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))|\\?[\"']\s*[:=]\s*(?!(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))(?:\\?[\"'])?(?![\\\"'{\[\]},\s])\S)"#,
        true,
    ),
    ("aws-access-key", r#"AKIA[0-9A-Z]{16}"#, false),
    ("github-token", r#"gh[pousr]_[A-Za-z0-9]{36,}"#, false),
    ("github-token", r#"github_pat_[A-Za-z0-9_]{20,}"#, false),
    ("slack-token", r#"xox[abprs]-[A-Za-z0-9-]{10,}"#, false),
    ("api-key", r#"sk-(?:ant-)?[A-Za-z0-9_-]{20,}"#, false),
    (
        "credential-assignment",
        r#"(password|passwd|secret|api[_-]?key|token)(?:\s*[:=](?![ \t]*(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))\s*\S{8,}|\\?[\"']\s*[:=]\s*(?!(?:\\?[\"'](?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+)\\?[\"']|(?:null|none|true|false|<[^<>\r\n]*>|\$\{[^{}\r\n]*\}|[*.xX]+))[ \t]*(?:[,}\]\r\n]|$))(?:\\?[\"'])?(?![\\\"'{\[\]},\s])\S{8,})"#,
        true,
    ),
    // A look-behind, not \b: `-` is in the class, so \b would start a scan at every `-eyJ`.
    (
        "jwt",
        r#"(?<![A-Za-z0-9_-])eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"#,
        false,
    ),
    (
        "cloud-secret-key",
        r#"(?:secret|access)[_-]?(?:access[_-]?)?key[\x22']?\s*[:=]\s*\S{8,}"#,
        true,
    ),
    ("url-credentials", r#"://[^\s/:@]*:[^\s/@]+@"#, false),
    ("anthropic-key", r#"sk-ant-[A-Za-z0-9_-]{8,}"#, false),
    ("google-api-key", r#"AIza[0-9A-Za-z_-]{35}"#, false),
    ("stripe-live-key", r#"\b[sr]k_live_[0-9A-Za-z]{8,}"#, false),
    (
        "bearer-token",
        r#"\bBearer\s+[A-Za-z0-9._~+/=-]{20,}"#,
        false,
    ),
    (
        "basic-auth",
        r#"Authorization:\s*Basic\s+[A-Za-z0-9+/=]{8,}"#,
        true,
    ),
    ("gitlab-token", r#"\bglpat-[A-Za-z0-9_-]{20,}"#, false),
    ("huggingface-token", r#"\bhf_[A-Za-z0-9]{30,}"#, false),
    ("npm-token", r#"\bnpm_[A-Za-z0-9]{36,}"#, false),
];

// The module patterns, verbatim from memory_privacy.py (as Python concatenates them).
const HOME_PATH: &str = r"/(?:Users|home)/([A-Za-z0-9._-]+)";
const EMAIL: &str =
    r"(?<![\w.%+-])([A-Za-z0-9._%+-]+)@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,})(?![\w-])";
const IPV4: &str = r"(?<![\w.])(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})(?!\d|\.\d)";
const IPV6: &str = r"(?<![\w:.])[0-9A-Fa-f]{1,4}(?::[0-9A-Fa-f]{0,4}){2,7}(?:%[\w.-]+)?(?![\w:])";
const PRIVATE_HOST: &str = r"(?<![\w.-])(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+(?:local|lan|internal|intranet|corp|home|localdomain|home\.arpa|ts\.net)(?![\w(-]|\.[\w-])";
const SSH_COMMAND: &str = r"(?<![\w-])(?:ssh|scp|sftp|rsync|mosh)(?![\w-])";
const USER_AT_HOST: &str =
    r"(?<![\w.%+-])([A-Za-z0-9._-]+)@([A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?)";
const SSH_PATH: &str = r"/\.ssh/[A-Za-z0-9._@+-]";
const KEY_NAME: &str = r"(?<![\w-])id_(?:rsa|ed25519|ecdsa|dsa)(?![\w-])";
/// `_home_literal` and `_hostname_patterns`: the parts around the escaped value.
const HOME_TAIL: &str = r"(?![\w-])";
const HOST_HEAD: &str = r"(?<![\w-])";
const HOST_TAIL: &str = r"(?![\w-])";

/// `(kind, label)` for `describe`.
const LABELS: &[(&str, &str)] = &[
    ("home-path", "home path"),
    ("email", "email address"),
    ("private-ip", "private IP address"),
    ("ip-address", "IP address"),
    ("private-hostname", "private hostname"),
    ("ssh-target", "ssh user@host"),
    ("ssh-key-path", "ssh key path"),
    (UNSCANNABLE, "text the privacy scan could not finish"),
];

/// Characters of a path run before a home segment (`_PATH_CHARS`; ASCII).
fn is_path_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"._~$@%+/-".contains(&b)
}

/// `_SCHEME_CHARS` (ASCII).
fn is_scheme_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"+.-".contains(&b)
}

const RESERVED_DOMAINS: &[&str] = &["example.com", "example.org", "example.net"];
const RESERVED_TLDS: &[&str] = &["example", "test", "invalid", "localhost"];
/// `git@<forge>` is the shared ssh user of a public forge, not a person or a private host.
const PUBLIC_FORGES: &[&str] = &[
    "github.com",
    "gitlab.com",
    "bitbucket.org",
    "codeberg.org",
    "ssh.dev.azure.com",
    "git.sr.ht",
];
/// Default or generic machine names: never a finding on their own.
const GENERIC_HOSTNAMES: &[&str] = &[
    "ubuntu",
    "debian",
    "fedora",
    "arch",
    "alpine",
    "linux",
    "macbook",
    "macbook-pro",
    "macbook-air",
    "imac",
    "mac-mini",
    "mac",
    "localhost",
    "raspberrypi",
    "docker",
    "container",
    "server",
    "workstation",
    "desktop",
    "laptop",
];

/// A search hit the engine's limit (or its backtrack stack): the line is unscannable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unfinished;

/// One compiled pattern.
#[derive(Debug)]
enum Rx {
    Plain(regex::Regex),
    Fancy(fancy_regex::Regex),
}

impl Rx {
    /// Compiles a translated pattern; a pattern that does not compile is a bug (tests compile
    /// every one), so it panics with the pattern's Python source.
    fn build(py: &str, t: &pyre::Translated, limit: usize) -> Rx {
        if t.fancy {
            let rx = fancy_regex::RegexBuilder::new(&t.pattern)
                .backtrack_limit(limit)
                .build()
                .unwrap_or_else(|e| panic!("privacy: {py:?} does not compile: {e}"));
            Rx::Fancy(rx)
        } else {
            let rx = regex::RegexBuilder::new(&t.pattern)
                .size_limit(64 << 20)
                .build()
                .unwrap_or_else(|e| panic!("privacy: {py:?} does not compile: {e}"));
            Rx::Plain(rx)
        }
    }

    fn compile(py: &str, ignorecase: bool, limit: usize) -> Rx {
        let t = pyre::translate(py, ignorecase)
            .unwrap_or_else(|e| panic!("privacy: {py:?} does not translate: {e}"));
        Rx::build(py, &t, limit)
    }

    /// `rx.search(line) is not None`.
    fn search(&self, line: &str) -> Result<bool, Unfinished> {
        match self {
            Rx::Plain(rx) => Ok(rx.is_match(line)),
            Rx::Fancy(rx) => rx.is_match(line).map_err(|_| Unfinished),
        }
    }

    /// `for m in rx.finditer(line)`, with each match's groups (`None` for an unset one), until
    /// `f` breaks.
    fn for_each<F>(&self, line: &str, mut f: F) -> Result<(), Unfinished>
    where
        F: FnMut(&[Option<&str>]) -> ControlFlow<()>,
    {
        let mut groups: Vec<Option<&str>> = Vec::new();
        match self {
            Rx::Plain(rx) => {
                for caps in rx.captures_iter(line) {
                    groups.clear();
                    groups.extend(caps.iter().map(|m| m.map(|m| m.as_str())));
                    if f(&groups).is_break() {
                        break;
                    }
                }
            }
            Rx::Fancy(rx) => {
                for caps in rx.captures_iter(line) {
                    let caps = caps.map_err(|_| Unfinished)?;
                    groups.clear();
                    groups.extend(caps.iter().map(|m| m.map(|m| m.as_str())));
                    if f(&groups).is_break() {
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// The `regex` of a pattern with no look-around (`_HOME_PATH`, whose walk needs offsets).
    fn plain(&self) -> &regex::Regex {
        match self {
            Rx::Plain(rx) => rx,
            Rx::Fancy(_) => unreachable!("privacy: _HOME_PATH is a plain pattern"),
        }
    }
}

/// Every static pattern, compiled for one backtrack budget.
#[derive(Debug)]
struct Patterns {
    home_path: Rx,
    email: Rx,
    ipv4: Rx,
    ipv6: Rx,
    private_host: Rx,
    ssh_command: Rx,
    user_at_host: Rx,
    ssh_path: Rx,
    key_name: Rx,
    secrets: Vec<(&'static str, Rx)>,
}

impl Patterns {
    fn compile(limit: usize) -> Patterns {
        Patterns {
            home_path: Rx::compile(HOME_PATH, false, limit),
            email: Rx::compile(EMAIL, false, limit),
            ipv4: Rx::compile(IPV4, false, limit),
            ipv6: Rx::compile(IPV6, false, limit),
            private_host: Rx::compile(PRIVATE_HOST, true, limit),
            ssh_command: Rx::compile(SSH_COMMAND, false, limit),
            user_at_host: Rx::compile(USER_AT_HOST, false, limit),
            ssh_path: Rx::compile(SSH_PATH, false, limit),
            key_name: Rx::compile(KEY_NAME, false, limit),
            secrets: SECRET_PATTERNS
                .iter()
                .map(|&(label, py, ic)| (label, Rx::compile(py, ic, limit)))
                .collect(),
        }
    }
}

/// A bounded cache: compiled sets and dynamic patterns are built once per key, as Python's `re`
/// cache does for the dynamic ones.
struct Cache<K, V> {
    map: Mutex<HashMap<K, Arc<V>>>,
}

impl<K: std::hash::Hash + Eq + Clone, V> Cache<K, V> {
    const MAX: usize = 256;

    fn new() -> Cache<K, V> {
        Cache {
            map: Mutex::new(HashMap::new()),
        }
    }

    fn get(&self, key: &K, build: impl FnOnce() -> V) -> Arc<V> {
        if let Some(v) = self.lock().get(key) {
            return Arc::clone(v);
        }
        // Built outside the lock: a slow compile never blocks another thread's lookup.
        let v = Arc::new(build());
        let mut map = self.lock();
        if map.len() >= Self::MAX {
            map.clear();
        }
        Arc::clone(map.entry(key.clone()).or_insert(v))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<K, Arc<V>>> {
        // A panic elsewhere cannot leave the map half-written (insert and clear are atomic).
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn patterns_for(limit: usize) -> Arc<Patterns> {
    static CACHE: OnceLock<Cache<usize, Patterns>> = OnceLock::new();
    CACHE
        .get_or_init(Cache::new)
        .get(&limit, || Patterns::compile(limit))
}

/// Which dynamic pattern a cached entry is.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Dynamic {
    Home(String),
    Host(String),
    /// A Python pattern source and its IGNORECASE flag ([`search_any`]).
    Python(String, bool),
}

fn dynamic_for(which: &Dynamic, limit: usize) -> Arc<Rx> {
    static CACHE: OnceLock<Cache<(Dynamic, usize), Rx>> = OnceLock::new();
    CACHE
        .get_or_init(Cache::new)
        .get(&(which.clone(), limit), || build_dynamic(which, limit))
}

fn build_dynamic(which: &Dynamic, limit: usize) -> Rx {
    let part = |py: &str, ic: bool| {
        pyre::translate(py, ic)
            .unwrap_or_else(|e| panic!("privacy: {py:?} does not translate: {e}"))
            .pattern
    };
    let (py, pattern) = match which {
        Dynamic::Python(py, ic) => return Rx::compile(py, *ic, limit),
        // `re.escape(home) + r"(?![\w-])"`.
        Dynamic::Home(home) => {
            let lit = pyre::literal(home, false, pyre::NonAsciiCase::Refuse)
                .unwrap_or_else(|e| panic!("privacy: home literal: {e}"));
            (HOME_TAIL, format!("{lit}{}", part(HOME_TAIL, false)))
        }
        // `r"(?<![\w-])" + re.escape(n) + r"(?![\w-])"`, re.IGNORECASE. A hostname is data: a
        // non-ASCII cased letter in it uses the engine's own case folding (documented residual).
        Dynamic::Host(name) => {
            let lit = pyre::literal(name, true, pyre::NonAsciiCase::EngineFold)
                .unwrap_or_else(|e| panic!("privacy: hostname literal: {e}"));
            (
                HOST_HEAD,
                format!("{}{lit}{}", part(HOST_HEAD, true), part(HOST_TAIL, true)),
            )
        }
    };
    let translated = pyre::Translated {
        pattern,
        fancy: true,
    };
    Rx::build(py, &translated, limit)
}

/// The patterns of one scan, per line budget.
struct Scan {
    limits: PrivacyLimits,
    home: Option<String>,
    hosts: Vec<String>,
    by_limit: HashMap<usize, LinePatterns>,
}

struct LinePatterns {
    statics: Arc<Patterns>,
    home: Option<Arc<Rx>>,
    hosts: Vec<Arc<Rx>>,
}

impl Scan {
    fn patterns(&mut self, len: usize) -> &LinePatterns {
        let limit = self.limits.for_len(len);
        let (home, hosts) = (&self.home, &self.hosts);
        self.by_limit.entry(limit).or_insert_with(|| LinePatterns {
            statics: patterns_for(limit),
            home: home
                .as_ref()
                .map(|h| dynamic_for(&Dynamic::Home(h.clone()), limit)),
            hosts: hosts
                .iter()
                .map(|n| dynamic_for(&Dynamic::Host(n.clone()), limit))
                .collect(),
        })
    }
}

/// `[{kind, line}]` for `text`, sorted by line then kind, one per (kind, line); never the matched
/// text. `home: None` means `deps.home` (`$HOME`); `hostname: None` means `deps.hostname()`.
pub fn privacy_findings(
    text: &str,
    home: Option<&str>,
    hostname: Option<&str>,
    deps: &PrivacyDeps,
) -> Vec<Finding> {
    let home_value = match home {
        Some(h) => h.to_string(),
        None => deps.home.clone().unwrap_or_default(),
    };
    let host_value = match hostname {
        Some(h) => h.to_string(),
        None => (deps.hostname)().unwrap_or_default(),
    };
    let mut scan = Scan {
        limits: deps.limits,
        home: home_literal(&home_value),
        hosts: hostname_names(&host_value),
        by_limit: HashMap::new(),
    };
    let mut found: BTreeSet<(usize, String)> = BTreeSet::new();
    for (index, line) in text.split('\n').enumerate() {
        let pats = scan.patterns(line.len());
        for kind in line_kinds(line, pats) {
            found.insert((index + 1, kind));
        }
    }
    found
        .into_iter()
        .map(|(line, kind)| Finding { kind, line })
        .collect()
}

/// `_home_literal`: the value to search for, or `None` when it is one character or less.
fn home_literal(home: &str) -> Option<String> {
    let home = home.trim_end_matches('/');
    (home.chars().count() > 1).then(|| home.to_string())
}

/// `_hostname_patterns`: the names to search for (the patterns are built per budget).
fn hostname_names(hostname: &str) -> Vec<String> {
    // `str(hostname or "").strip().rstrip(".")`, with Python's whitespace.
    let mut name = hostname.trim_matches(is_py_space).trim_end_matches('.');
    // `name.lower().endswith(".local")`: only ASCII lowers into ".local" (İ lowers to two
    // characters, the Kelvin sign to `k`), so this is an ASCII test on the last six bytes.
    if name.len() >= 6
        && name.is_char_boundary(name.len() - 6)
        && name[name.len() - 6..].eq_ignore_ascii_case(".local")
    {
        name = &name[..name.len() - 6];
    }
    let mut names: BTreeSet<&str> = BTreeSet::new();
    names.insert(name);
    if let Some((short, _)) = name.split_once('.') {
        names.insert(short); // the short name of an FQDN
    }
    names
        .into_iter()
        .filter(|n| n.chars().count() >= 4)
        // `n.lower()` is the full Unicode lowercase (the Kelvin sign is a `k`).
        .filter(|n| !GENERIC_HOSTNAMES.contains(&n.to_lowercase().as_str()))
        .map(str::to_string)
        .collect()
}

/// Python's `str.isspace()` (the `\s` class).
fn is_py_space(c: char) -> bool {
    let cp = c as u32;
    pyclass::S.iter().any(|&(lo, hi)| lo <= cp && cp <= hi)
}

/// The value of a decimal digit (Python's `int()` of one Nd character).
fn digit_value(c: char) -> Option<u32> {
    let cp = c as u32;
    let k = pyclass::DIGIT_ZEROS.partition_point(|&z| z <= cp);
    let zero = *pyclass::DIGIT_ZEROS.get(k.checked_sub(1)?)?;
    (cp - zero < 10).then_some(cp - zero)
}

/// Python's `int()` of a `\d{1,3}` group.
fn py_int(digits: &str) -> Option<u32> {
    digits
        .chars()
        .try_fold(0u32, |acc, c| Some(acc * 10 + digit_value(c)?))
}

fn reserved_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    let tld = host.rsplit('.').next().unwrap_or(host);
    if host == "localhost" || RESERVED_TLDS.contains(&tld) {
        return true;
    }
    RESERVED_DOMAINS
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

fn forge_git_user(local: &str, host: &str) -> bool {
    local.eq_ignore_ascii_case("git")
        && PUBLIC_FORGES.contains(&host.to_ascii_lowercase().trim_end_matches('.'))
}

/// The email groups are ASCII (the pattern's classes), so `.lower()` is ASCII lowercase.
fn exempt_email(local: &str, domain: &str) -> bool {
    if forge_git_user(local, domain) {
        return true;
    }
    let (local, domain) = (local.to_ascii_lowercase(), domain.to_ascii_lowercase());
    if local == "noreply" || local == "no-reply" || domain == "users.noreply.github.com" {
        return true;
    }
    reserved_host(&domain)
}

/// `_ipv4_kind`.
fn ipv4_kind(groups: &[Option<&str>]) -> Option<&'static str> {
    let mut o = [0u32; 4];
    for (k, slot) in o.iter_mut().enumerate() {
        *slot = py_int(groups.get(k + 1).copied().flatten()?)?;
    }
    if o.iter().any(|&x| x > 255) {
        return None;
    }
    let addr = (o[0] << 24) | (o[1] << 16) | (o[2] << 8) | o[3];
    let within = |net: u32, bits: u32| {
        let mask = if bits == 0 {
            0
        } else {
            u32::MAX << (32 - bits)
        };
        addr & mask == net & mask
    };
    let exempt = [
        (0x7f00_0000, 8),
        (0, 32),
        (0xffff_ffff, 32),
        (0xc000_0200, 24),
        (0xc633_6400, 24),
        (0xcb00_7100, 24),
    ];
    if exempt.iter().any(|&(n, b)| within(n, b)) {
        return None;
    }
    let private = [
        (0x0a00_0000, 8),
        (0xac10_0000, 12),
        (0xc0a8_0000, 16),
        (0x6440_0000, 10),
        (0xa9fe_0000, 16),
    ];
    if private.iter().any(|&(n, b)| within(n, b)) {
        return Some("private-ip");
    }
    Some("ip-address")
}

/// `ipaddress.IPv6Address(text)` (CPython 3.12 `_ip_int_from_string`, without a zone), as the
/// first hextet, or `None` when Python raises. Only the first hextet decides the two private
/// networks, but every part is checked as Python does.
fn ipv6_first_hextet(text: &str) -> Option<u16> {
    const HEXTETS: usize = 8;
    if text.is_empty() || text.chars().count() > 45 {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() < 3 || parts.len() > HEXTETS + 1 {
        return None;
    }
    // A `.` in the last part (an IPv4 suffix) cannot come out of `_IPV6`.
    if parts.last().is_some_and(|p| p.contains('.')) {
        return None;
    }
    let mut skip = None;
    for (i, part) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if part.is_empty() {
            if skip.is_some() {
                return None;
            }
            skip = Some(i);
        }
    }
    let (hi, lo) = if let Some(skip) = skip {
        let mut hi = skip;
        let mut lo = parts.len() - skip - 1;
        if parts[0].is_empty() {
            hi -= 1;
            if hi != 0 {
                return None;
            }
        }
        if parts[parts.len() - 1].is_empty() {
            lo -= 1;
            if lo != 0 {
                return None;
            }
        }
        if HEXTETS < hi + lo + 1 {
            return None;
        }
        (hi, lo)
    } else {
        if parts.len() != HEXTETS || parts[0].is_empty() || parts[HEXTETS - 1].is_empty() {
            return None;
        }
        (parts.len(), 0)
    };
    // `_parse_hextet`: hex digits only, at most four, and not empty (`int("", 16)` raises).
    let hextet = |s: &str| -> Option<u16> {
        if s.is_empty() || s.len() > 4 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u16::from_str_radix(s, 16).ok()
    };
    let high: Vec<u16> = parts[..hi]
        .iter()
        .map(|p| hextet(p))
        .collect::<Option<_>>()?;
    for part in &parts[parts.len() - lo..] {
        hextet(part)?;
    }
    // With no part before the `::`, the address starts with the skipped zeros.
    Some(high.first().copied().unwrap_or(0))
}

/// `fc00::/7` or `fe80::/10`.
fn private_v6(first: u16) -> bool {
    first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
}

/// `_has_scheme`: a scheme (`[A-Za-z][A-Za-z0-9+.-]*`) ends at byte `p`, within the token that
/// starts at `start`. Byte offsets are exact here: every byte compared is ASCII.
fn has_scheme(line: &[u8], start: usize, p: usize) -> bool {
    let mut k = p;
    while k > start && is_scheme_char(line[k - 1]) {
        k -= 1;
        if line[k].is_ascii_alphabetic() {
            return true;
        }
    }
    false
}

/// `_first_url_end`: the end of the first `scheme://h` in the token at `start`, as a byte offset
/// one past the host's first byte (only compared with a match start, which is a `/`: the same
/// order as Python's code-point offsets, since `p + 3` is a character boundary). Reads the token
/// only, so all tokens together stay linear.
fn first_url_end(line: &[u8], start: usize) -> Option<usize> {
    let end = line[start..]
        .iter()
        .position(|&b| b == b' ' || b == b'\t')
        .map_or(line.len(), |k| start + k);
    let find = |from: usize| -> Option<usize> {
        line.get(from..end)?
            .windows(3)
            .position(|w| w == b"://")
            .map(|k| from + k)
    };
    let mut p = find(start);
    while let Some(at) = p {
        if at + 3 < end && line[at + 3] != b'/' && has_scheme(line, start, at) {
            return Some(at + 4);
        }
        p = find(at + 1);
    }
    None
}

/// `_home_path_found`: `/Users/<name>` or `/home/<name>` as a segment of an absolute path,
/// outside a URL with a host. Linear: each backwards walk stops where the previous match's walk
/// started, and each token's URL is looked up once. Byte offsets are exact: the walks compare
/// ASCII bytes only (`_PATH_CHARS`, ` `, `\t`), and a match starts with `/`.
fn home_path_found(line: &str, home_path: &regex::Regex) -> bool {
    let b = line.as_bytes();
    let (mut prev_at, mut prev_run): (Option<usize>, usize) = (None, 0);
    let mut tok: Option<(usize, usize)> = None; // the previous match's start and its token's start
    let mut url_end: Option<usize> = None;
    for caps in home_path.captures_iter(line) {
        let name = caps.get(1).map_or("", |m| m.as_str()).trim_end_matches('.');
        if name.is_empty() || name == "Shared" {
            continue;
        }
        let i = caps.get(0).map_or(0, |m| m.start());
        let mut j = i;
        while j > 0 && is_path_char(b[j - 1]) {
            j -= 1;
            if Some(j) == prev_at {
                j = prev_run; // joined the previous match's run: reuse its start
                break;
            }
        }
        prev_at = Some(i);
        prev_run = j;
        if j < i && b[j] != b'/' {
            continue; // a relative path such as `src/home/view.py`
        }
        let lo = tok.map_or(0, |(at, _)| at);
        let ws = b[lo..i]
            .iter()
            .rposition(|&c| c == b' ' || c == b'\t')
            .map(|k| lo + k);
        let start = match (ws, tok) {
            (Some(ws), _) => ws + 1,
            (None, Some((_, tok_start))) => tok_start,
            (None, None) => 0,
        };
        if tok.is_none_or(|(_, tok_start)| tok_start != start) {
            url_end = first_url_end(b, start);
        }
        tok = Some((i, start));
        if url_end.is_none_or(|end| end > i) {
            return true;
        }
    }
    false
}

/// The kinds on one line; an engine error on any search adds `unscannable`.
fn line_kinds(line: &str, pats: &LinePatterns) -> BTreeSet<String> {
    fn add(kinds: &mut BTreeSet<String>, kind: &str) {
        kinds.insert(kind.to_string());
    }
    let mut kinds = BTreeSet::new();
    let mut unfinished = false;
    let p = &pats.statics;
    if home_path_found(line, p.home_path.plain()) {
        add(&mut kinds, "home-path");
    }
    if let Some(rx) = &pats.home {
        match rx.search(line) {
            Ok(true) => add(&mut kinds, "home-path"),
            Ok(false) => {}
            Err(Unfinished) => unfinished = true,
        }
    }
    let r = p.email.for_each(line, |g| {
        let (local, domain) = (g[1].unwrap_or(""), g[2].unwrap_or(""));
        if !exempt_email(local, domain) {
            kinds.insert("email".to_string());
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    });
    unfinished |= r.is_err();
    // The look-behind skips `v1.2.3.4` and `1.2.3.4.5`.
    let r = p.ipv4.for_each(line, |g| {
        if let Some(kind) = ipv4_kind(g) {
            kinds.insert(kind.to_string());
        }
        ControlFlow::Continue(())
    });
    unfinished |= r.is_err();
    let r = p.ipv6.for_each(line, |g| {
        let text = g[0].unwrap_or("");
        let addr = text.split_once('%').map_or(text, |(a, _)| a);
        if ipv6_first_hextet(addr).is_some_and(private_v6) {
            kinds.insert("private-ip".to_string());
        }
        ControlFlow::Continue(())
    });
    unfinished |= r.is_err();
    // `_PRIVATE_HOST.search(line) or any(rx.search(line) for rx in host_rxs)`; an error on the
    // first still runs the rest.
    let mut private_host = false;
    match p.private_host.search(line) {
        Ok(found) => private_host = found,
        Err(Unfinished) => unfinished = true,
    }
    if !private_host {
        for rx in &pats.hosts {
            match rx.search(line) {
                Ok(true) => {
                    private_host = true;
                    break;
                }
                Ok(false) => {}
                Err(Unfinished) => unfinished = true,
            }
        }
    }
    if private_host {
        add(&mut kinds, "private-hostname");
    }
    // The target may come before or after the command word.
    match p.ssh_command.search(line) {
        Ok(true) => {
            let r = p.user_at_host.for_each(line, |g| {
                let (user, host) = (g[1].unwrap_or(""), g[2].unwrap_or(""));
                if !forge_git_user(user, host) && !reserved_host(host) {
                    kinds.insert("ssh-target".to_string());
                    return ControlFlow::Break(());
                }
                ControlFlow::Continue(())
            });
            unfinished |= r.is_err();
        }
        Ok(false) => {}
        Err(Unfinished) => unfinished = true,
    }
    let mut key_path = false;
    match p.ssh_path.search(line) {
        Ok(found) => key_path = found,
        Err(Unfinished) => unfinished = true,
    }
    if !key_path {
        match p.key_name.search(line) {
            Ok(found) => key_path = found,
            Err(Unfinished) => unfinished = true,
        }
    }
    if key_path {
        add(&mut kinds, "ssh-key-path");
    }
    for (label, rx) in &p.secrets {
        match rx.search(line) {
            Ok(true) => {
                kinds.insert(format!("secret:{label}"));
            }
            Ok(false) => {}
            Err(Unfinished) => unfinished = true,
        }
    }
    if unfinished {
        add(&mut kinds, UNSCANNABLE);
    }
    kinds
}

/// A human-readable summary, e.g. `home path (line 3), email address (lines 7, 9)`.
pub fn describe(findings: &[Finding]) -> String {
    let mut order: Vec<&str> = Vec::new();
    let mut lines: HashMap<&str, Vec<usize>> = HashMap::new();
    for f in findings {
        let kind = f.kind.as_str();
        let at = lines.entry(kind).or_insert_with(|| {
            order.push(kind);
            Vec::new()
        });
        if !at.contains(&f.line) {
            at.push(f.line);
        }
    }
    let parts: Vec<String> = order
        .iter()
        .map(|kind| {
            let label = match kind.strip_prefix("secret:") {
                Some(rest) => format!("secret-looking {rest}"),
                None => LABELS
                    .iter()
                    .find(|(k, _)| k == kind)
                    .map_or_else(|| (*kind).to_string(), |(_, l)| (*l).to_string()),
            };
            let nums = &lines[kind];
            let at = if nums.len() == 1 {
                format!("line {}", nums[0])
            } else {
                let list: Vec<String> = nums.iter().map(usize::to_string).collect();
                format!("lines {}", list.join(", "))
            };
            format!("{label} ({at})")
        })
        .collect();
    parts.join(", ")
}

/// Python's `any(rx.search(text) for rx in patterns)` for `(pattern source, re.IGNORECASE)`
/// pairs such as [`SECRET_PATTERNS`] or the guard's G6 list, with CPython 3.12's semantics and
/// the default budget for `text`'s length. An engine error counts as a match (fail closed).
/// Each pattern is compiled once per process.
pub fn search_any(patterns: &[(&str, bool)], text: &str) -> bool {
    let limit = PrivacyLimits::default().for_len(text.len());
    patterns.iter().any(|&(py, ic)| {
        let which = Dynamic::Python(py.to_string(), ic);
        dynamic_for(&which, limit).search(text).unwrap_or(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_translates_and_compiles() {
        let limit = PrivacyLimits::default().backtrack_limit;
        let _ = Patterns::compile(limit);
        for d in [
            Dynamic::Home("/srv/x".into()),
            Dynamic::Host("Box-Ünï.k".into()),
        ] {
            let _ = build_dynamic(&d, limit);
        }
        for (py, ic) in anthology_guard::secrets::GUARD_SECRET_PATTERNS {
            pyre::translate(py, *ic).unwrap();
        }
    }

    #[test]
    fn digits_have_their_python_values() {
        assert_eq!(py_int("123"), Some(123));
        assert_eq!(py_int("\u{661}\u{662}"), Some(12)); // Arabic-Indic
        assert_eq!(py_int("\u{1d7ce}"), Some(0)); // mathematical bold zero
        assert_eq!(py_int("x"), None);
    }

    #[test]
    fn ipv6_parsing_is_pythons() {
        for ok in [
            "fd12:3456:789a::1",
            "fe80::1",
            "::1:2",
            "1::",
            "1:2:3:4:5:6:7:8",
            "1::8",
        ] {
            assert!(ipv6_first_hextet(ok).is_some(), "{ok}");
        }
        for bad in [
            "1:2",
            "1:::2",
            "1::2::3",
            ":1:2:3:4:5:6:7",
            "1:2:3:4:5:6:7:",
            "12345::1",
        ] {
            assert!(ipv6_first_hextet(bad).is_none(), "{bad}");
        }
        assert_eq!(ipv6_first_hextet("::1:2"), Some(0));
        assert_eq!(ipv6_first_hextet("fe80::1"), Some(0xfe80));
        assert!(ipv6_first_hextet("1:2:3:4:5:6:7:8:9").is_none());
        assert!(
            ipv6_first_hextet("1:2:3:4::5:6:7:8").is_none(),
            "`::` must skip a part"
        );
    }

    #[test]
    fn limit_tiers() {
        let l = PrivacyLimits::default();
        assert_eq!(l.for_len(0), 1_000_000);
        assert_eq!(l.for_len(8_192), 1_000_000);
        assert_eq!(l.for_len(8_193), 1 << 20);
        assert_eq!(l.for_len(2_240_000), 64 << 22);
        let low = PrivacyLimits {
            backtrack_limit: 0,
            backtrack_per_byte: 0,
        };
        assert_eq!(low.for_len(1 << 20), 0);
    }
}
