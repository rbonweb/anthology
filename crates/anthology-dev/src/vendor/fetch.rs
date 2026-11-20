//! Fetching: git (the pinned upstream into `.vendor-cache/`), url sources (the committed
//! snapshot, verified; `vendor fetch` downloads it), and the dispatch between them.
//!
//! Git runs as vendor.py runs it (`git -c core.autocrlf=false …` with the whole environment plus
//! `GIT_TERMINAL_PROMPT=0` and `GIT_CONFIG_NOSYSTEM=1`, no timeout). Downloads go through ureq
//! with the platform verifier, https only, redirects followed by hand so a non-https target is
//! refused by name, and a 10 MiB body cap.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anthology_pycompat::json::{self, Value};
use sha2::{Digest, Sha256};

use super::compare::{
    is_dir, is_file, is_symlink, os_err, os_walk, reject_dir_symlinks, rel_posix, sort_names,
};
use super::manifest::{
    CACHE_DIRNAME, CHECKSUMS_NAME, MAX_DOWNLOAD, SNAPSHOTS, checksum_line, safe_relpath_str,
    source_slug, validate_sources,
};
use super::py;
use super::{Entry, Fetch, Get, Result, VendorError};

// ------------------------------------------------------------------------------------- git

/// A finished git: exit code and text output (decoded as Python's `text=True` does).
pub struct GitResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// `_git(args, cwd)`.
pub fn git(args: &[&str], cwd: Option<&Path>) -> Result<GitResult> {
    let mut cmd = Command::new("git");
    cmd.arg("-c")
        .arg("core.autocrlf=false")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::inherit());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = match cmd.output() {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(VendorError::new(
                "git is required for this command (fetch/sync/patches)",
            ));
        }
        Err(e) => return Err(VendorError(format!("cannot run git: {e}"))),
    };
    let text = |b: &[u8]| py::universal_newlines(&String::from_utf8_lossy(b));
    Ok(GitResult {
        code: out.status.code().unwrap_or(-1),
        stdout: text(&out.stdout),
        stderr: text(&out.stderr),
    })
}

/// `_git_ok(args, cwd)`: the output, or `git <args> failed:\n<stderr>`.
pub fn git_ok(args: &[&str], cwd: Option<&Path>) -> Result<String> {
    let result = git(args, cwd)?;
    if result.code != 0 {
        return Err(VendorError(format!(
            "git {} failed:\n{}",
            args.join(" "),
            py::strip(&result.stderr)
        )));
    }
    Ok(result.stdout)
}

fn name_of(entry: &Entry) -> String {
    py::str_of(entry.get("name"))
}

/// `shutil.rmtree(path, ignore_errors=True)`, never following a symlink.
pub fn rmtree_quiet(path: &Path) {
    if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        let _ = fs::remove_dir_all(path);
    }
}

/// `git_fetcher(cache)(entry)`: a pristine checkout of the repo at the pinned commit, cached
/// as `<cache>/<slug>-<commit>`.
pub fn git_fetch(cache: &Path, entry: &Entry) -> Result<PathBuf> {
    let (repo, commit) = (entry.get("repo"), entry.get("commit"));
    if !py::truthy(repo) || !py::truthy(commit) {
        return Err(VendorError(format!(
            "{}: only git sources with a commit can be fetched",
            name_of(entry)
        )));
    }
    let (repo, commit) = (py::str_of(repo), py::str_of(commit));
    let slug = source_slug(entry);
    let dest = cache.join(format!("{slug}-{commit}"));
    if is_dir(&dest.join(".git")) {
        return Ok(dest);
    }
    fs::create_dir_all(cache).map_err(|e| os_err(&e, cache))?;
    let tmp = cache.join(format!(".tmp-{slug}-{commit}-{}", std::process::id()));
    rmtree_quiet(&tmp);
    let result = (|| -> Result<()> {
        fs::create_dir(&tmp).map_err(|e| os_err(&e, &tmp))?;
        git_ok(&["init", "-q"], Some(&tmp))?;
        git_ok(&["remote", "add", "origin", &repo], Some(&tmp))?;
        git_ok(
            &["fetch", "-q", "--depth", "1", "origin", &commit],
            Some(&tmp),
        )?;
        git_ok(
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "-q",
                "--detach",
                "FETCH_HEAD",
            ],
            Some(&tmp),
        )?;
        let head = git_ok(&["rev-parse", "HEAD"], Some(&tmp))?;
        let head = py::strip(&head);
        if head != commit {
            return Err(VendorError(format!(
                "{}: fetched {head}, expected {commit}",
                name_of(entry)
            )));
        }
        rmtree_quiet(&dest);
        fs::rename(&tmp, &dest).map_err(|e| os_err(&e, &tmp))
    })();
    rmtree_quiet(&tmp);
    result.map(|()| dest)
}

/// `git_fetcher(cache)`.
pub fn git_fetcher(cache: PathBuf) -> impl Fn(&Entry) -> Result<PathBuf> {
    move |entry| git_fetch(&cache, entry)
}

/// `git_resolve_ref(entry, ref)`: a tag (peeled) or branch of the entry's repo → its commit.
pub fn git_resolve_ref(entry: &Entry, git_ref: &str) -> Result<String> {
    let repo = py::str_of(entry.get("repo"));
    let tags = format!("refs/tags/{git_ref}");
    let peeled = format!("refs/tags/{git_ref}^{{}}");
    let heads = format!("refs/heads/{git_ref}");
    let out = git_ok(&["ls-remote", &repo, &tags, &peeled, &heads], None)?;
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    for line in py::splitlines(&out) {
        let (sha, name) = line.split_once('\t').unwrap_or((line, ""));
        found.insert(name.to_string(), sha.to_string());
    }
    for name in [&peeled, &tags, &heads] {
        if let Some(sha) = found.get(name.as_str()) {
            return Ok(sha.clone());
        }
    }
    Err(VendorError(format!(
        "{}: ref {} not found in {repo}",
        name_of(entry),
        py::repr_str(git_ref)
    )))
}

// ------------------------------------------------------------------------------ url sources

/// `_sha256(data)`.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `parse_checksums(data)`: `<sha256>  <path>` lines → {path: sha256}. Paths must be plain
/// relative paths: they become file names on disk and in URLs.
pub fn parse_checksums(data: &[u8]) -> Result<BTreeMap<String, String>> {
    let Ok(text) = std::str::from_utf8(data) else {
        return Err(VendorError(format!("{CHECKSUMS_NAME} is not UTF-8")));
    };
    let mut listed = BTreeMap::new();
    for (i, line) in py::splitlines(text).into_iter().enumerate() {
        let number = i + 1;
        let Some((digest, rel)) = checksum_line(line) else {
            return Err(VendorError(format!(
                "{CHECKSUMS_NAME}:{number}: not a `<sha256>  <path>` line"
            )));
        };
        if !safe_relpath_str(rel, false)
            || rel == CHECKSUMS_NAME
            || rel.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
        {
            return Err(VendorError(format!(
                "{CHECKSUMS_NAME}:{number}: unsafe path {}",
                py::repr_str(rel)
            )));
        }
        if listed.contains_key(rel) {
            return Err(VendorError(format!(
                "{CHECKSUMS_NAME}:{number}: {rel} is listed twice"
            )));
        }
        listed.insert(rel.to_string(), digest.to_string());
    }
    if listed.is_empty() {
        return Err(VendorError(format!("{CHECKSUMS_NAME} lists no files")));
    }
    Ok(listed)
}

/// `_snapshot_files(tree)`: the relative path of every file; symlinks are not allowed.
fn snapshot_files(tree: &Path) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    os_walk(tree, |w| {
        sort_names(&mut w.dirs);
        reject_dir_symlinks(&w.path, &w.dirs, tree)?;
        sort_names(&mut w.files);
        for name in &w.files {
            let p = w.path.join(name);
            if is_symlink(&p) {
                return Err(VendorError(format!(
                    "{}: symlinks are not allowed in a snapshot",
                    rel_posix(&p, tree)
                )));
            }
            found.insert(rel_posix(&p, tree));
        }
        Ok(())
    })?;
    Ok(found)
}

/// `verify_snapshot(entry, tree)`: the tree is exactly the files CHECKSUMS.sha256 lists plus
/// that file, every file matches its digest, and CHECKSUMS.sha256 matches the pin.
pub fn verify_snapshot(entry: &Entry, tree: &Path) -> Result<()> {
    let name = name_of(entry);
    let sums = tree.join(CHECKSUMS_NAME);
    if !is_file(&sums) || is_symlink(&sums) {
        return Err(VendorError(format!(
            "{name}: no snapshot at {SNAPSHOTS}/{name}; run `vendor.py fetch {name}`"
        )));
    }
    let raw = fs::read(&sums).map_err(|e| os_err(&e, &sums))?;
    if Some(sha256_hex(&raw).as_str()) != entry.get("checksum").and_then(Value::as_str) {
        return Err(VendorError(format!(
            "{name}: {CHECKSUMS_NAME} does not match the pinned checksum in vendor/sources.json"
        )));
    }
    let listed = parse_checksums(&raw)?;
    let mut present = snapshot_files(tree)?;
    present.remove(CHECKSUMS_NAME);
    let mut problems: Vec<String> = Vec::new();
    for rel in listed.keys().filter(|r| !present.contains(*r)) {
        problems.push(format!("missing {rel}"));
    }
    for rel in present.iter().filter(|r| !listed.contains_key(*r)) {
        problems.push(format!("unlisted file {rel}"));
    }
    for rel in present.iter().filter(|r| listed.contains_key(*r)) {
        let p = tree.join(rel);
        let data = fs::read(&p).map_err(|e| os_err(&e, &p))?;
        if sha256_hex(&data) != listed[rel] {
            problems.push(format!("{rel} does not match its checksum"));
        }
    }
    if !problems.is_empty() {
        return Err(VendorError(format!(
            "{name}: snapshot {SNAPSHOTS}/{name} is not what {CHECKSUMS_NAME} lists: {}",
            problems.join("; ")
        )));
    }
    Ok(())
}

/// `snapshot_fetcher(root)(entry)`: the committed snapshot of a url source, verified (no
/// network).
pub fn snapshot_fetch(root: &Path, entry: &Entry) -> Result<PathBuf> {
    if !py::truthy(entry.get("url")) {
        return Err(VendorError(format!("{}: not a url source", name_of(entry))));
    }
    let tree = root.join(SNAPSHOTS).join(name_of(entry));
    verify_snapshot(entry, &tree)?;
    Ok(tree)
}

/// `snapshot_fetcher(root)`.
pub fn snapshot_fetcher(root: &Path) -> impl Fn(&Entry) -> Result<PathBuf> + '_ {
    move |entry| snapshot_fetch(root, entry)
}

/// How [`http_get_with`] verifies TLS: the platform verifier, or (tests) these DER roots.
#[derive(Clone, Debug, Default)]
pub struct HttpOpts {
    /// Extra trust roots (DER). Empty: `RootCerts::PlatformVerifier`, as production uses.
    pub roots: Vec<Vec<u8>>,
    pub timeout: Option<Duration>,
}

/// `http_get(url)`: GET over https only (no other scheme, no redirect to one); the body is
/// size-limited.
pub fn http_get(url: &str) -> Result<Vec<u8>> {
    http_get_with(url, &HttpOpts::default())
}

/// The redirect statuses urllib follows.
const REDIRECTS: [u16; 5] = [301, 302, 303, 307, 308];
/// `HTTPRedirectHandler.max_redirections`.
const MAX_REDIRECTIONS: usize = 10;

/// [`http_get`] with a TLS seam (tests trust the testkit CA through `opts.roots`).
pub fn http_get_with(url: &str, opts: &HttpOpts) -> Result<Vec<u8>> {
    use ureq::tls::{Certificate, RootCerts, TlsConfig};
    if !url.starts_with("https://") {
        return Err(VendorError(format!("refusing a non-https URL: {url}")));
    }
    let roots = if opts.roots.is_empty() {
        RootCerts::PlatformVerifier
    } else {
        RootCerts::Specific(Arc::new(
            opts.roots
                .iter()
                .map(|der| Certificate::from_der(der).to_owned())
                .collect(),
        ))
    };
    let timeout = opts.timeout.unwrap_or(Duration::from_secs(30));
    let config = ureq::Agent::config_builder()
        .tls_config(TlsConfig::builder().root_certs(roots).build())
        .https_only(true)
        .max_redirects(0)
        .http_status_as_error(false)
        .user_agent("anthology-vendor")
        .timeout_connect(Some(timeout))
        .timeout_recv_response(Some(timeout))
        .timeout_recv_body(Some(timeout))
        .build();
    let agent: ureq::Agent = config.into();
    let fail = |detail: String| VendorError(format!("cannot download {url}: {detail}"));
    let mut current = url.to_string();
    let mut hops = 0;
    loop {
        let response = agent
            .get(&current)
            .call()
            .map_err(|e| fail(e.to_string()))?;
        let status = response.status().as_u16();
        if REDIRECTS.contains(&status) {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let Some(location) = location else {
                return Err(fail(format!(
                    "HTTP Error {status}: redirect without a Location"
                )));
            };
            let next = join_url(&current, &location);
            if !next.to_ascii_lowercase().starts_with("https://") {
                return Err(VendorError(format!(
                    "refusing a redirect to a non-https URL: {next}"
                )));
            }
            hops += 1;
            if hops > MAX_REDIRECTIONS {
                return Err(fail(format!(
                    "HTTP Error {status}: The HTTP server returned a redirect error that would \
                     lead to an infinite loop."
                )));
            }
            current = next;
            continue;
        }
        if !(200..300).contains(&status) {
            let reason = response.status().canonical_reason().unwrap_or("");
            return Err(fail(format!("HTTP Error {status}: {reason}")));
        }
        let mut data = Vec::new();
        response
            .into_body()
            .into_reader()
            .take(MAX_DOWNLOAD as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|e| fail(e.to_string()))?;
        if data.len() > MAX_DOWNLOAD {
            return Err(VendorError(format!(
                "{url} is larger than {MAX_DOWNLOAD} bytes"
            )));
        }
        return Ok(data);
    }
}

/// `urllib.parse.urljoin(base, location)` for the forms a `Location` header takes.
pub fn join_url(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if let Some(net) = location.strip_prefix("//") {
        return format!("{scheme}://{net}");
    }
    if location.starts_with('/') {
        return format!("{scheme}://{authority}{location}");
    }
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let dir = &path[..path.rfind('/').map_or(0, |i| i + 1)];
    format!("{scheme}://{authority}{dir}{location}")
}

/// `_index_files(entry, get)`: the file list the host's `index.json` gives for the skill.
fn index_files(entry: &Entry, get: &Get) -> Result<BTreeSet<String>> {
    let url = py::str_of(entry.get("url"));
    let base = url.rsplitn(3, '/').last().unwrap_or("");
    let index_url = format!("{base}/index.json");
    let unusable = || {
        VendorError(format!(
            "{}: {index_url} has no usable file list for this skill",
            name_of(entry)
        ))
    };
    let raw = get(&index_url)?;
    let text = std::str::from_utf8(&raw).map_err(|_| unusable())?;
    let data = json::loads(text).map_err(|_| unusable())?;
    // A non-list `skills` or a non-dict skill raises AttributeError or TypeError in Python
    // (D-1): unusable here too.
    let skills = data
        .as_object()
        .and_then(|m| m.get("skills"))
        .and_then(Value::as_array)
        .ok_or_else(unusable)?;
    let name = entry.get("name").and_then(Value::as_str);
    for s in skills {
        let Some(s) = s.as_object() else {
            return Err(unusable());
        };
        if s.get("name").and_then(Value::as_str) == name && name.is_some() {
            let files = s
                .get("files")
                .and_then(Value::as_array)
                .ok_or_else(unusable)?;
            let mut out = BTreeSet::new();
            for f in files {
                out.insert(f.as_str().ok_or_else(unusable)?.to_string());
            }
            return Ok(out);
        }
    }
    Err(unusable())
}

/// `download_snapshot(root, entry, get)`: download a url source into
/// `vendor/snapshots/<name>/`. The CHECKSUMS.sha256 must match the pin and the host's
/// index.json; every file must match its digest. The old snapshot is replaced only after all
/// of that.
pub fn download_snapshot(root: &Path, entry: &Entry, get: Option<&Get>) -> Result<PathBuf> {
    let errors = validate_sources(&[Value::Object(entry.clone())]);
    if !errors.is_empty() {
        return Err(VendorError(format!(
            "invalid source:\n  {}",
            errors.join("\n  ")
        )));
    }
    let default_get = |u: &str| http_get(u);
    let get: &Get = get.unwrap_or(&default_get);
    let name = name_of(entry);
    let base = py::str_of(entry.get("url"));
    let raw = get(&format!("{base}{CHECKSUMS_NAME}"))?;
    if Some(sha256_hex(&raw).as_str()) != entry.get("checksum").and_then(Value::as_str) {
        return Err(VendorError(format!(
            "{name}: downloaded {CHECKSUMS_NAME} does not match the pinned checksum; nothing was \
             written"
        )));
    }
    let listed = parse_checksums(&raw)?;
    let advertised = index_files(entry, get)?;
    let mut expected: BTreeSet<String> = listed.keys().cloned().collect();
    expected.insert(CHECKSUMS_NAME.to_string());
    if advertised != expected {
        let diff: Vec<&String> = advertised.symmetric_difference(&expected).collect();
        let diff: Vec<&str> = {
            let mut d: Vec<&str> = diff.iter().map(|s| s.as_str()).collect();
            d.sort();
            d
        };
        return Err(VendorError(format!(
            "{name}: index.json and {CHECKSUMS_NAME} disagree about: {}",
            diff.join(", ")
        )));
    }
    let dest = root.join(SNAPSHOTS).join(&name);
    let parent = root.join(SNAPSHOTS);
    fs::create_dir_all(&parent).map_err(|e| os_err(&e, &parent))?;
    let pid = std::process::id();
    let tmp = parent.join(format!(".tmp-{name}-{pid}"));
    let old = parent.join(format!(".old-{name}-{pid}"));
    rmtree_quiet(&tmp);
    rmtree_quiet(&old);
    let result = (|| -> Result<()> {
        fs::create_dir(&tmp).map_err(|e| os_err(&e, &tmp))?;
        let write = |path: &Path, data: &[u8]| -> Result<()> {
            if let Some(dir) = path.parent() {
                fs::create_dir_all(dir).map_err(|e| os_err(&e, dir))?;
            }
            fs::write(path, data).map_err(|e| os_err(&e, path))?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o644))
                .map_err(|e| os_err(&e, path))
        };
        for (rel, digest) in &listed {
            let data = get(&format!("{base}{}", py::quote(rel, "/")))?;
            if &sha256_hex(&data) != digest {
                return Err(VendorError(format!(
                    "{name}: {rel} does not match its checksum; nothing was written"
                )));
            }
            write(&tmp.join(rel), &data)?;
        }
        write(&tmp.join(CHECKSUMS_NAME), &raw)?;
        verify_snapshot(entry, &tmp)?;
        if dest.exists() {
            fs::rename(&dest, &old).map_err(|e| os_err(&e, &dest))?;
        }
        if let Err(e) = fs::rename(&tmp, &dest) {
            if old.exists() {
                let _ = fs::rename(&old, &dest);
            }
            return Err(os_err(&e, &tmp));
        }
        Ok(())
    })();
    rmtree_quiet(&tmp);
    rmtree_quiet(&old);
    result.map(|()| dest)
}

/// `fetch_entry(root, entry, fetch, get)`: what `vendor fetch` does for one source — url
/// sources are downloaded into their snapshot, git sources fetched into the cache.
pub fn fetch_entry(
    root: &Path,
    entry: &Entry,
    fetch: Option<&Fetch>,
    get: Option<&Get>,
) -> Result<PathBuf> {
    if py::truthy(entry.get("url")) {
        return download_snapshot(root, entry, get);
    }
    match fetch {
        Some(f) => f(entry),
        None => git_fetch(&root.join(CACHE_DIRNAME), entry),
    }
}

/// `dispatch_fetcher(root, git_fetch)`: url sources come from their committed snapshot,
/// everything else from `git_fetch`.
pub fn dispatch_fetcher<'a>(root: &'a Path, git_fetch: Box<Fetch<'a>>) -> Box<Fetch<'a>> {
    Box::new(move |entry: &Entry| {
        if py::truthy(entry.get("url")) {
            snapshot_fetch(root, entry)
        } else {
            git_fetch(entry)
        }
    })
}

/// `_fetcher(root, fetch)`: `fetch`, or the default dispatch over `<root>/.vendor-cache`.
pub fn fetcher<'a>(root: &'a Path, fetch: Option<&'a Fetch<'a>>) -> Box<Fetch<'a>> {
    match fetch {
        Some(f) => Box::new(f),
        None => dispatch_fetcher(root, Box::new(git_fetcher(root.join(CACHE_DIRNAME)))),
    }
}
