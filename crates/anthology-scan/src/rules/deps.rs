//! Dependencies: lockfile hosts, index URLs, name and URL mismatches (Task 10).
//!
//! A lockfile that fetches a package from a host the user never chose, over plain HTTP, or from a
//! path that names another package is how a dependency gets swapped without a visible change to
//! `package.json`. The files are read as text and no URL is ever fetched.

use std::sync::LazyLock;

use anthology_pycompat::json::Value;
use regex::Regex;

use super::agent::{clip, name_of, text_of};
use crate::files::line_of;
use crate::{FileCtx, Finding, Group, Severity, jsonc};

/// The registries a lockfile may name without a finding.
const OFFICIAL_REGISTRIES: &[&str] = &["registry.npmjs.org", "registry.yarnpkg.com"];

/// The URL a lockfile entry downloads from: npm `"resolved": "…"`, yarn `resolved "…"` and pnpm
/// `tarball: …`.
static LOCK_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)(?:"resolved"\s*:\s*"|^[ \t]*resolved[ \t]+"|tarball:[ \t]*['"]?)([^"'\s,}]+)"#,
    )
    .expect("the lock URL pattern compiles")
});

/// An index flag in a requirements file: `--index-url X`, `--extra-index-url=X` or `-i X`.
static PIP_INDEX_FLAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\s)(?:--(?:extra-)?index-url(?:=|[ \t]+)|-i[ \t]+)(\S+)")
        .expect("the index flag pattern compiles")
});

/// An index key in `pip.conf`; the value may be empty and continue on the lines below.
static PIP_CONF_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:extra-)?index[-_]url\s*[=:]\s*(.*)$")
        .expect("the conf key pattern compiles")
});

/// A requirement that names a URL: `pkg @ https://…`, `pkg[extra] @ git+https://…`.
static PIP_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[A-Za-z0-9][A-Za-z0-9._-]*[ \t]*(?:\[[^\]]*\])?[ \t]*@[ \t]*(?:git\+)?https?://\S+",
    )
    .expect("the requirement URL pattern compiles")
});

/// The hits of one rule in one file: the first place and how many in all.
struct Hit {
    rule: &'static str,
    severity: Severity,
    /// Byte offset of the first hit, if known.
    at: Option<usize>,
    seen: String,
    count: usize,
}

/// Hits grouped by rule, so a lockfile with ten thousand mirror URLs gives one finding.
#[derive(Default)]
struct Tally(Vec<Hit>);

impl Tally {
    fn add(&mut self, rule: &'static str, severity: Severity, at: Option<usize>, seen: &str) {
        if let Some(hit) = self.0.iter_mut().find(|h| h.rule == rule) {
            hit.count += 1;
        } else {
            self.0.push(Hit {
                rule,
                severity,
                at,
                seen: clip(seen),
                count: 1,
            });
        }
    }

    fn emit(self, path: &str, text: &str, out: &mut Vec<Finding>) {
        for hit in self.0 {
            let (why, action) = describe(hit.rule);
            out.push(Finding {
                severity: hit.severity,
                group: Group::Deps,
                rule: hit.rule.to_string(),
                path: path.to_string(),
                line: hit.at.map_or(0, |at| line_of(text, at)),
                why: why.to_string(),
                seen: hit.seen,
                action: action.to_string(),
                count: hit.count,
            });
        }
    }
}

fn describe(rule: &str) -> (&'static str, &'static str) {
    match rule {
        "deps.lock-host" => (
            "The lockfile downloads a package from a host other than the official npm registries.",
            "Check that you trust the host before installing.",
        ),
        "deps.lock-http" => (
            "The lockfile downloads a package over plain HTTP, which anyone on the network can alter.",
            "Do not install until the URL uses HTTPS and a host you trust.",
        ),
        "deps.lock-git" => (
            "The lockfile takes a package from a git repo, not from a registry.",
            "Check the repo and the commit before installing.",
        ),
        "deps.lock-tarball" => (
            "The lockfile takes a package from a direct download URL, not from a registry.",
            "Check the URL before installing.",
        ),
        "deps.lock-name-mismatch" => (
            "A package downloads from a registry path that names another package, a sign of a swapped dependency.",
            "Do not install; compare the lockfile with package.json.",
        ),
        "deps.pip-index" => (
            "pip would take packages from an index other than pypi.org.",
            "Check that you trust the index before installing.",
        ),
        "deps.pip-url" => (
            "A requirement downloads from a URL, not from an index.",
            "Check the URL before installing.",
        ),
        _ => (
            "go.mod replaces a module with another source.",
            "Check the replacement before building.",
        ),
    }
}

pub(crate) fn check(f: &FileCtx, out: &mut Vec<Finding>) {
    // ASCII case is ignored: macOS and Windows open `Package-Lock.json` as `package-lock.json`.
    let name = name_of(f.path).to_ascii_lowercase();
    let name = name.as_str();
    let known = matches!(
        name,
        "package-lock.json"
            | "npm-shrinkwrap.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "pip.conf"
            | "go.mod"
    ) || (name.starts_with("requirements") && name.ends_with(".txt"));
    if !known {
        return;
    }
    // A known name is read whatever its bytes are: invalid UTF-8 or a NUL must not skip the file.
    let text = text_of(f);
    let text: &str = &text;
    let mut tally = Tally::default();
    match name {
        "package-lock.json" | "npm-shrinkwrap.json" => {
            lock_urls(text, &mut tally);
            npm_names(text, &mut tally);
        }
        "yarn.lock" => {
            lock_urls(text, &mut tally);
            yarn_names(text, &mut tally);
        }
        "pnpm-lock.yaml" => lock_urls(text, &mut tally),
        "pip.conf" => pip_conf(text, &mut tally),
        "go.mod" => go_mod(text, &mut tally),
        n if n.starts_with("requirements") && n.ends_with(".txt") => requirements(text, &mut tally),
        _ => {}
    }
    tally.emit(f.path, text, out);
}

// ---- URLs ----

struct Url {
    scheme: String,
    host: String,
    path: String,
}

/// The parts of `url`, or `None` when it has no `scheme://` (a `file:` spec, a range, a name).
fn split_url(url: &str) -> Option<Url> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty()
        || !scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
    {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host_port.strip_prefix('[') {
        Some(inner) => inner.split(']').next().unwrap_or(inner),
        None => host_port.split(':').next().unwrap_or(host_port),
    };
    let path = tail.split(['?', '#']).next().unwrap_or(tail);
    Some(Url {
        scheme: scheme.to_ascii_lowercase(),
        host: host.to_ascii_lowercase(),
        path: path.to_string(),
    })
}

/// `s` with `%XX` escapes decoded, repeated until nothing changes, so a double-encoded `..` shows.
fn percent_decode(s: &str) -> String {
    let mut current = s.to_string();
    for _ in 0..3 {
        let bytes = current.as_bytes();
        let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
        let mut i = 0;
        let mut changed = false;
        while i < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if bytes[i] == b'%'
                && i + 2 < bytes.len()
                && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
            {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                changed = true;
                continue;
            }
            out.push(bytes[i]);
            i += 1;
        }
        if !changed {
            break;
        }
        current = String::from_utf8_lossy(&out).into_owned();
    }
    current
}

/// `path` with `.`, empty and `..` parts resolved, as an absolute path.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Whether a path has the registry layout `<name>/-/<file>`, before or after resolving `..`.
fn has_registry_layout(path: &str) -> bool {
    let decoded = percent_decode(path);
    decoded.contains("/-/") || normalize(&decoded).contains("/-/")
}

/// The rule and severity for a lockfile URL, or `None` when it is fine or not a download URL.
fn classify_url(url: &str) -> Option<(&'static str, Severity)> {
    let u = split_url(url)?;
    match u.scheme.as_str() {
        "http" | "git+http" => Some(("deps.lock-http", Severity::High)),
        "git" | "ssh" => Some(("deps.lock-git", Severity::Low)),
        s if s.starts_with("git+") => Some(("deps.lock-git", Severity::Low)),
        "https" if OFFICIAL_REGISTRIES.contains(&u.host.as_str()) => None,
        "https" if has_registry_layout(&u.path) => Some(("deps.lock-host", Severity::Medium)),
        "https" => Some(("deps.lock-tarball", Severity::Low)),
        _ => None,
    }
}

/// True when `url` is a registry download whose path does not name package `name`. An official
/// registry always has the layout; a mirror is checked only when it uses it.
fn name_mismatch(name: &str, url: &str) -> bool {
    let Some(u) = split_url(url) else {
        return false;
    };
    if u.scheme != "https" && u.scheme != "http" {
        return false;
    }
    let official = OFFICIAL_REGISTRIES.contains(&u.host.as_str());
    if !official && !has_registry_layout(&u.path) {
        return false;
    }
    let path = normalize(&percent_decode(&u.path));
    let needle = format!("/{name}/-/");
    let Some(at) = path.find(&needle) else {
        return true;
    };
    let unscoped = name.rsplit('/').next().unwrap_or(name);
    !path[at + needle.len()..].starts_with(&format!("{unscoped}-"))
}

/// Every download URL in a lockfile, by host and scheme. Read as lines of text, so a lockfile cut
/// at the size cap still gets this check.
fn lock_urls(text: &str, tally: &mut Tally) {
    for caps in LOCK_URL.captures_iter(text) {
        let Some(m) = caps.get(1) else {
            continue;
        };
        if let Some((rule, severity)) = classify_url(m.as_str()) {
            tally.add(rule, severity, Some(m.start()), m.as_str());
        }
    }
}

fn mismatch(tally: &mut Tally, text: &str, name: &str, url: &str) {
    if name_mismatch(name, url) {
        let at = text.find(url);
        tally.add(
            "deps.lock-name-mismatch",
            Severity::High,
            at,
            &format!("{name}: {url}"),
        );
    }
}

/// The name and download URL of each entry of an npm lockfile, checked against each other.
fn npm_names(text: &str, tally: &mut Tally) {
    let Ok(doc) = jsonc::parse(text) else {
        return;
    };
    let Some(root) = doc.as_object() else {
        return;
    };
    // Lockfile version 2 and 3: `packages`, keyed by `node_modules/<name>` paths. Version 1 and the
    // legacy copy in version 2: `dependencies`, nested. The legacy copy is skipped when
    // `packages` is there.
    let mut entries: Vec<(&str, &Value)> = Vec::new();
    if let Some(packages) = root.get("packages").and_then(Value::as_object) {
        for (key, entry) in packages.iter() {
            if let Some(i) = key.rfind("node_modules/") {
                entries.push((&key[i + "node_modules/".len()..], entry));
            }
        }
    } else if let Some(deps) = root.get("dependencies").and_then(Value::as_object) {
        let mut stack: Vec<(&str, &Value)> = deps.iter().collect();
        while let Some((name, entry)) = stack.pop() {
            entries.push((name, entry));
            if let Some(nested) = entry
                .as_object()
                .and_then(|o| o.get("dependencies"))
                .and_then(Value::as_object)
            {
                stack.extend(nested.iter());
            }
        }
    }
    for (key_name, entry) in entries {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(resolved) = entry.get("resolved").and_then(Value::as_str) else {
            continue;
        };
        // An alias (`"version": "npm:real@1.0.0"`) downloads another name on purpose.
        if entry
            .get("version")
            .and_then(Value::as_str)
            .is_some_and(|v| v.starts_with("npm:"))
        {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(key_name);
        mismatch(tally, text, name, resolved);
    }
}

/// The name in each header of a yarn v1 lockfile, checked against the `resolved` URL below it.
fn yarn_names(text: &str, tally: &mut Tally) {
    let mut name: Option<String> = None;
    for line in text.lines() {
        if line.starts_with(|c: char| c.is_whitespace() || c == '#') || line.trim().is_empty() {
            let trimmed = line.trim();
            if let (Some(name), Some(rest)) = (&name, trimmed.strip_prefix("resolved")) {
                let url = rest.trim().trim_matches('"');
                mismatch(tally, text, name, url);
            }
            continue;
        }
        // A header: `name@range, name@range:`; the first spec names the package.
        let first = line
            .trim_end()
            .trim_end_matches(':')
            .split(',')
            .next()
            .unwrap_or("");
        let spec = first.trim().trim_matches('"');
        name = if spec.contains("@npm:") {
            None
        } else {
            spec.rfind('@')
                .filter(|&i| i > 0)
                .map(|i| spec[..i].to_string())
        };
    }
}

// ---- Python ----

/// True when `url` is the PyPI index over HTTPS.
fn is_pypi(url: &str) -> bool {
    split_url(url).is_some_and(|u| u.scheme == "https" && u.host == "pypi.org")
}

fn pip_index(tally: &mut Tally, offset: usize, url: &str) {
    // A value that is not a URL (an environment variable, say) names no host to judge.
    if split_url(url).is_some() && !is_pypi(url) {
        tally.add("deps.pip-index", Severity::Medium, Some(offset), url);
    }
}

fn requirements(text: &str, tally: &mut Tally) {
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let start = offset;
        offset += raw.len();
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(caps) = PIP_INDEX_FLAG.captures(line) {
            pip_index(tally, start, &caps[1]);
        } else if PIP_URL.is_match(line) {
            tally.add("deps.pip-url", Severity::Low, Some(start), line);
        }
    }
}

fn pip_conf(text: &str, tally: &mut Tally) {
    let mut offset = 0;
    let mut continued = false;
    for raw in text.split_inclusive('\n') {
        let start = offset;
        offset += raw.len();
        let line = raw.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if continued && raw.starts_with(char::is_whitespace) {
            pip_index(tally, start, line);
            continue;
        }
        continued = false;
        if let Some(caps) = PIP_CONF_KEY.captures(line) {
            let value = caps[1].trim();
            if value.is_empty() {
                continued = true;
            }
            for url in value.split_whitespace() {
                pip_index(tally, start, url);
            }
        }
    }
}

// ---- Go ----

fn go_mod(text: &str, tally: &mut Tally) {
    let mut offset = 0;
    for raw in text.split_inclusive('\n') {
        let start = offset;
        offset += raw.len();
        let code = raw.split("//").next().unwrap_or(raw).trim();
        if code.contains("=>") {
            tally.add("deps.go-replace", Severity::Low, Some(start), code);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileKind, Group, Severity};

    fn run(path: &str, text: &str) -> Vec<Finding> {
        let f = FileCtx {
            path,
            bytes: text.as_bytes(),
            text: Some(text),
            kind: FileKind::Config,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        out
    }

    /// A `package-lock.json` (lockfile version 3) with one entry per `(path key, resolved)`.
    fn npm_lock(entries: &[(&str, &str)]) -> String {
        let mut text = String::from(
            "{\n  \"name\": \"app\",\n  \"lockfileVersion\": 3,\n  \"packages\": {\n    \"\": {\"name\": \"app\"}",
        );
        for (key, resolved) in entries {
            text.push_str(&format!(
                ",\n    \"{key}\": {{\n      \"version\": \"1.0.0\",\n      \"resolved\": \"{resolved}\",\n      \"integrity\": \"sha512-x\"\n    }}"
            ));
        }
        text.push_str("\n  }\n}\n");
        text
    }

    fn only(out: &[Finding], rule: &str) -> Finding {
        let hits: Vec<&Finding> = out.iter().filter(|f| f.rule == rule).collect();
        assert_eq!(hits.len(), 1, "{rule} in {out:?}");
        hits[0].clone()
    }

    const NPM: &str = "https://registry.npmjs.org";

    #[test]
    fn official_registries_are_clean() {
        let lock = npm_lock(&[
            (
                "node_modules/lodash",
                &format!("{NPM}/lodash/-/lodash-4.17.21.tgz"),
            ),
            (
                "node_modules/@types/node",
                &format!("{NPM}/@types/node/-/node-20.0.0.tgz"),
            ),
            (
                "node_modules/a/node_modules/yarned",
                "https://registry.yarnpkg.com/yarned/-/yarned-1.0.0.tgz",
            ),
        ]);
        assert!(run("package-lock.json", &lock).is_empty());
    }

    #[test]
    fn a_resolved_host_outside_the_official_registries_is_medium() {
        let lock = npm_lock(&[(
            "node_modules/foo",
            "https://registry.example.invalid/foo/-/foo-1.0.0.tgz",
        )]);
        let hit = only(&run("package-lock.json", &lock), "deps.lock-host");
        assert_eq!(hit.severity, Severity::Medium);
        assert_eq!(hit.group, Group::Deps);
        assert_eq!(hit.path, "package-lock.json");
        assert_eq!(hit.line, 8);
        assert_eq!(
            hit.seen,
            "https://registry.example.invalid/foo/-/foo-1.0.0.tgz"
        );
        assert_eq!(hit.count, 1);
    }

    #[test]
    fn a_resolved_http_url_is_high() {
        let lock = npm_lock(&[(
            "node_modules/foo",
            "http://registry.npmjs.org/foo/-/foo-1.0.0.tgz",
        )]);
        let hit = only(&run("package-lock.json", &lock), "deps.lock-http");
        assert_eq!(hit.severity, Severity::High);
    }

    #[test]
    fn git_and_bare_tarball_urls_are_low() {
        let lock = npm_lock(&[
            (
                "node_modules/a",
                "git+ssh://git@github.com/acme/a.git#abc123",
            ),
            ("node_modules/b", "git+https://github.com/acme/b.git#abc123"),
            (
                "node_modules/c",
                "https://example.invalid/downloads/c-1.0.0.tgz",
            ),
        ]);
        let out = run("package-lock.json", &lock);
        let git = only(&out, "deps.lock-git");
        assert_eq!(git.severity, Severity::Low);
        assert_eq!(git.count, 2);
        let tar = only(&out, "deps.lock-tarball");
        assert_eq!(tar.severity, Severity::Low);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn local_and_link_specs_are_not_registry_urls() {
        let lock = npm_lock(&[
            ("node_modules/a", "file:../a"),
            ("node_modules/b", "link:../b"),
            ("node_modules/c", "workspace:*"),
        ]);
        assert!(run("package-lock.json", &lock).is_empty());
    }

    #[test]
    fn a_name_that_does_not_match_its_url_is_high() {
        let lock = npm_lock(&[(
            "node_modules/lodash",
            &format!("{NPM}/evil-pkg/-/evil-pkg-1.0.0.tgz"),
        )]);
        let hit = only(&run("package-lock.json", &lock), "deps.lock-name-mismatch");
        assert_eq!(hit.severity, Severity::High);
        assert_eq!(hit.line, 8);
    }

    #[test]
    fn a_tarball_file_that_names_another_package_is_a_mismatch() {
        let lock = npm_lock(&[(
            "node_modules/lodash",
            &format!("{NPM}/lodash/-/evil-1.0.0.tgz"),
        )]);
        only(&run("package-lock.json", &lock), "deps.lock-name-mismatch");
    }

    #[test]
    fn percent_encoding_and_dot_dot_cannot_hide_a_mismatch() {
        for url in [
            format!("{NPM}/lodash/%2e%2e/evil/-/evil-1.0.0.tgz"),
            format!("{NPM}/lodash/-/../../evil/-/evil-1.0.0.tgz"),
            format!("{NPM}/lodash/..%2fevil/-/evil-1.0.0.tgz"),
            // The start looks right: only decoding and resolving the tail shows the other package.
            format!("{NPM}/lodash/-/lodash-1.0.0.tgz/..%2f..%2f..%2fevil/-/evil-1.0.0.tgz"),
            format!(
                "{NPM}/lodash/-/lodash-1.0.0.tgz/%252e%252e/%252e%252e/%252e%252e/evil/-/evil-1.0.0.tgz"
            ),
        ] {
            let lock = npm_lock(&[("node_modules/lodash", &url)]);
            let out = run("package-lock.json", &lock);
            only(&out, "deps.lock-name-mismatch");
        }
    }

    #[test]
    fn an_encoded_scope_matches_its_package() {
        let lock = npm_lock(&[(
            "node_modules/@types/node",
            &format!("{NPM}/@types%2fnode/-/node-20.0.0.tgz"),
        )]);
        assert!(run("package-lock.json", &lock).is_empty());
    }

    #[test]
    fn an_official_host_without_the_registry_layout_is_a_mismatch() {
        let lock = npm_lock(&[("node_modules/lodash", &format!("{NPM}/lodash.tgz"))]);
        only(&run("package-lock.json", &lock), "deps.lock-name-mismatch");
    }

    #[test]
    fn a_mirror_with_the_registry_layout_is_only_a_host_finding() {
        let lock = npm_lock(&[(
            "node_modules/lodash",
            "https://artifactory.example.invalid/api/npm/npm/lodash/-/lodash-4.17.21.tgz",
        )]);
        let out = run("package-lock.json", &lock);
        only(&out, "deps.lock-host");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn an_alias_is_checked_against_its_real_name() {
        let text = format!(
            "{{\"lockfileVersion\": 3, \"packages\": {{\"node_modules/old\": {{\"name\": \"real\", \"version\": \"1.0.0\", \"resolved\": \"{NPM}/real/-/real-1.0.0.tgz\"}}}}}}"
        );
        assert!(run("package-lock.json", &text).is_empty());
    }

    #[test]
    fn a_workspace_entry_without_node_modules_is_not_checked() {
        let text = format!(
            "{{\"lockfileVersion\": 3, \"packages\": {{\"packages/app\": {{\"resolved\": \"{NPM}/other/-/other-1.0.0.tgz\"}}}}}}"
        );
        assert!(run("package-lock.json", &text).is_empty());
    }

    #[test]
    fn lockfile_version_1_nested_dependencies_are_checked() {
        let text = format!(
            "{{\"lockfileVersion\": 1, \"dependencies\": {{\"a\": {{\"version\": \"1.0.0\", \"resolved\": \"{NPM}/a/-/a-1.0.0.tgz\", \"dependencies\": {{\"b\": {{\"version\": \"1.0.0\", \"resolved\": \"{NPM}/evil/-/evil-1.0.0.tgz\"}}, \"c\": {{\"version\": \"npm:d@1.0.0\", \"resolved\": \"{NPM}/d/-/d-1.0.0.tgz\"}}}}}}}}}}"
        );
        let out = run("npm-shrinkwrap.json", &text);
        let hit = only(&out, "deps.lock-name-mismatch");
        assert_eq!(hit.seen, format!("b: {NPM}/evil/-/evil-1.0.0.tgz"));
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn a_truncated_lockfile_still_gets_the_host_check() {
        let lock = npm_lock(&[(
            "node_modules/foo",
            "http://example.invalid/foo/-/foo-1.0.0.tgz",
        )]);
        let cut = &lock[..lock.len() - 12];
        only(&run("package-lock.json", cut), "deps.lock-http");
    }

    #[test]
    fn many_hits_make_one_finding_with_a_count() {
        let lock = npm_lock(&[
            (
                "node_modules/a",
                "https://m.example.invalid/a/-/a-1.0.0.tgz",
            ),
            (
                "node_modules/b",
                "https://m.example.invalid/b/-/b-1.0.0.tgz",
            ),
            (
                "node_modules/c",
                "https://m.example.invalid/c/-/c-1.0.0.tgz",
            ),
        ]);
        let hit = only(&run("package-lock.json", &lock), "deps.lock-host");
        assert_eq!(hit.count, 3);
        assert_eq!(hit.line, 8);
        assert!(hit.seen.contains("/a/-/a-1.0.0.tgz"));
    }

    #[test]
    fn yarn_lock_hosts_and_names_are_checked() {
        let text = "# yarn lockfile v1\n\n\nlodash@^4.17.21:\n  version \"4.17.21\"\n  resolved \"https://registry.yarnpkg.com/lodash/-/lodash-4.17.21.tgz#abc\"\n  integrity sha512-x\n\n\"@types/node@^20.0.0\":\n  version \"20.0.0\"\n  resolved \"https://registry.yarnpkg.com/@types/node/-/node-20.0.0.tgz#abc\"\n";
        assert!(run("yarn.lock", text).is_empty());
        let bad = "foo@^1.0.0:\n  version \"1.0.0\"\n  resolved \"https://registry.yarnpkg.com/bar/-/bar-1.0.0.tgz#abc\"\n\nbaz@^1.0.0:\n  version \"1.0.0\"\n  resolved \"https://mirror.example.invalid/baz/-/baz-1.0.0.tgz#abc\"\n\nplain@^1.0.0:\n  resolved \"http://registry.yarnpkg.com/plain/-/plain-1.0.0.tgz#abc\"\n";
        let out = run("yarn.lock", bad);
        assert_eq!(only(&out, "deps.lock-name-mismatch").line, 3);
        assert_eq!(only(&out, "deps.lock-host").line, 7);
        assert_eq!(only(&out, "deps.lock-http").line, 10);
    }

    #[test]
    fn a_yarn_alias_is_not_a_mismatch() {
        let text = format!(
            "old@npm:real@^1.0.0:\n  version \"1.0.0\"\n  resolved \"{NPM}/real/-/real-1.0.0.tgz#abc\"\n"
        );
        assert!(run("yarn.lock", &text).is_empty());
    }

    #[test]
    fn pnpm_tarball_urls_are_checked() {
        let text = "packages:\n  /a/1.0.0:\n    resolution: {tarball: https://codeload.github.com/acme/a/tar.gz/abc}\n  /b/1.0.0:\n    resolution: {tarball: http://example.invalid/b.tgz}\n  /c/1.0.0:\n    resolution: {integrity: sha512-x}\n";
        let out = run("pnpm-lock.yaml", text);
        only(&out, "deps.lock-http");
        only(&out, "deps.lock-tarball");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn resolved_text_in_other_files_is_not_a_lockfile() {
        let lock = npm_lock(&[(
            "node_modules/foo",
            "http://example.invalid/foo/-/foo-1.0.0.tgz",
        )]);
        assert!(run("fixtures/lock.json", &lock).is_empty());
        assert!(run("package.json", &lock).is_empty());
        assert!(run("README.md", &lock).is_empty());
    }

    #[test]
    fn case_changes_do_not_hide_a_lockfile_or_a_dependency_file() {
        let lock = npm_lock(&[(
            "node_modules/foo",
            "http://example.invalid/foo/-/foo-1.0.0.tgz",
        )]);
        for path in [
            "Package-Lock.json",
            "PACKAGE-LOCK.JSON",
            "App/NPM-Shrinkwrap.json",
        ] {
            only(&run(path, &lock), "deps.lock-http");
        }
        let yarn =
            "foo@^1.0.0:\n  resolved \"http://registry.yarnpkg.com/foo/-/foo-1.0.0.tgz#a\"\n";
        only(&run("YARN.LOCK", yarn), "deps.lock-http");
        let pnpm = "resolution: {tarball: http://example.invalid/b.tgz}\n";
        only(&run("Pnpm-Lock.yaml", pnpm), "deps.lock-http");
        let req = "--index-url https://example.invalid/simple\n";
        only(&run("Requirements.TXT", req), "deps.pip-index");
        only(&run("REQUIREMENTS-DEV.txt", req), "deps.pip-index");
        only(
            &run("PIP.CONF", "index-url = https://example.invalid/simple\n"),
            "deps.pip-index",
        );
        only(&run("GO.MOD", "replace a => ../a\n"), "deps.go-replace");
        assert!(run("MY-REQUIREMENTS.txt", req).is_empty());
    }

    #[test]
    fn nested_lockfiles_are_read() {
        let lock = npm_lock(&[(
            "node_modules/foo",
            "http://example.invalid/foo/-/foo-1.0.0.tgz",
        )]);
        let out = run("apps/web/package-lock.json", &lock);
        assert_eq!(
            only(&out, "deps.lock-http").path,
            "apps/web/package-lock.json"
        );
    }

    // ---- pip ----

    #[test]
    fn a_pip_index_outside_pypi_is_medium() {
        for text in [
            "--index-url https://example.invalid/simple\nrequests==2.0\n",
            "requests==2.0\n--extra-index-url https://example.invalid/simple\n",
            "--index-url=https://example.invalid/simple\n",
            "-i https://example.invalid/simple\n",
            "--index-url http://pypi.org/simple\n",
        ] {
            let out = run("requirements.txt", text);
            let hit = only(&out, "deps.pip-index");
            assert_eq!(hit.severity, Severity::Medium, "{text}");
            assert_eq!(hit.group, Group::Deps);
        }
    }

    #[test]
    fn the_pypi_index_is_clean() {
        let text = "--index-url https://pypi.org/simple\n--extra-index-url https://user:pw@pypi.org/simple/\nflask>=2\n";
        assert!(run("requirements.txt", text).is_empty());
    }

    #[test]
    fn requirements_variants_and_pip_conf_are_read() {
        let line = "--extra-index-url https://example.invalid/simple\n";
        for path in [
            "requirements-dev.txt",
            "requirements_test.txt",
            "sub/requirements.txt",
        ] {
            only(&run(path, line), "deps.pip-index");
        }
        assert!(run("notes.txt", line).is_empty());
        assert!(run("my-requirements.txt", line).is_empty());
        let conf = "[global]\nindex-url = https://example.invalid/simple\n";
        only(&run("pip.conf", conf), "deps.pip-index");
        only(&run("home/pip.conf", conf), "deps.pip-index");
        let multi = "[global]\nextra-index-url =\n    https://pypi.org/simple\n    https://example.invalid/simple\n";
        let hit = only(&run("pip.conf", multi), "deps.pip-index");
        assert_eq!(hit.line, 4);
        let clean = "[global]\nindex-url = https://pypi.org/simple\ntimeout = 60\n";
        assert!(run("pip.conf", clean).is_empty());
    }

    #[test]
    fn a_requirement_from_a_url_is_low() {
        let text = "flask>=2\nmypkg @ https://example.invalid/mypkg-1.0.tar.gz\nother[extra] @ git+https://example.invalid/o.git\n";
        let hit = only(&run("requirements.txt", text), "deps.pip-url");
        assert_eq!(hit.severity, Severity::Low);
        assert_eq!(hit.line, 2);
        assert_eq!(hit.count, 2);
    }

    #[test]
    fn ordinary_requirements_are_clean() {
        let text = "# pins\nflask==2.0.1\nrequests[security]>=2 ; python_version > '3'\n-r base.txt\n-e .\nemail @ example.invalid\n";
        assert!(run("requirements.txt", text).is_empty());
    }

    // ---- go.mod ----

    #[test]
    fn a_go_replace_is_low() {
        let single = "module x\n\nrequire a v1.0.0\n\nreplace a => ../a\n";
        let hit = only(&run("go.mod", single), "deps.go-replace");
        assert_eq!(hit.severity, Severity::Low);
        assert_eq!(hit.line, 5);
        let block = "module x\n\nreplace (\n    a v1.0.0 => example.invalid/fork/a v1.0.1\n    b => ./b\n)\n";
        let hit = only(&run("sub/go.mod", block), "deps.go-replace");
        assert_eq!(hit.count, 2);
        assert_eq!(hit.line, 4);
    }

    #[test]
    fn a_go_mod_without_replace_is_clean() {
        let text = "module x\n\ngo 1.22\n\nrequire (\n    a v1.0.0\n    b v1.2.3 // => not a replace\n)\n// replace a => b\n";
        assert!(run("go.mod", text).is_empty());
        assert!(run("go.sum", "replace a => b\n").is_empty());
    }

    fn run_bytes(path: &str, bytes: &[u8]) -> Vec<Finding> {
        let f = FileCtx {
            path,
            bytes,
            text: None,
            kind: FileKind::Binary,
            truncated: false,
        };
        let mut out = Vec::new();
        check(&f, &mut out);
        out
    }

    #[test]
    fn a_file_with_no_lockfile_name_gives_nothing_even_if_it_is_not_text() {
        assert!(run_bytes("blob.bin", &[0, 1]).is_empty());
        assert!(run_bytes("package-lock.json", &[0, 1]).is_empty());
    }

    #[test]
    fn a_stray_invalid_byte_or_nul_does_not_hide_a_lockfile_host() {
        let lock = npm_lock(&[(
            "node_modules/foo",
            "http://example.invalid/foo/-/foo-1.0.0.tgz",
        )]);
        let mut with_ff = lock.clone().into_bytes();
        with_ff.splice(2..2, [0xff]);
        only(&run_bytes("package-lock.json", &with_ff), "deps.lock-http");
        let mut with_nul = lock.into_bytes();
        with_nul.splice(2..2, [0u8]);
        only(&run_bytes("package-lock.json", &with_nul), "deps.lock-http");
        let req = b"\xff\n--index-url https://example.invalid/simple\n";
        only(&run_bytes("requirements.txt", req), "deps.pip-index");
        let go = b"\xff\nreplace a => ../a\n";
        only(&run_bytes("go.mod", go), "deps.go-replace");
    }
}
