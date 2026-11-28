//! Optional scanners run on a tree, only when found on `PATH` (Task 6): `osv-scanner`,
//! `gitleaks` and `zizmor`. Nothing is installed. Each tool runs with the repo's own config
//! switched off (a repo can ship `.gitleaks.toml`, `osv-scanner.toml` or `zizmor.yml` to hide
//! findings), with its working folder set to a scratch folder, without network, and without any
//! token in its environment.
//!
//! `run_tools` reads three variables of the process environment (`PATH`, `HOME` and
//! `OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY`) to build that short environment; nothing else here
//! reads the environment.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anthology_pycompat::json::{self, Value};

use crate::{Finding, Group, Severity};

/// How long one tool may run.
const TIMEOUT: Duration = Duration::from_secs(60);
const OSV: &str = "osv-scanner";
const GITLEAKS: &str = "gitleaks";
const ZIZMOR: &str = "zizmor";
/// The tools in report order.
const TOOLS: [&str; 3] = [OSV, GITLEAKS, ZIZMOR];
/// What `osv-scanner` prints on stderr when the offline database was never downloaded.
const OSV_NO_DB: &str = "no offline version of the OSV database";
const OSV_SKIPPED: &str = "no offline database; to fetch it once run: osv-scanner scan source -r --offline --download-offline-databases <path>";
/// At most this many ids go into one finding's `seen`.
const MAX_IDS: usize = 20;
const MAX_TEXT: usize = 300;

/// How the CLI starts a program. The CLI implements it with a cleared environment; the tests
/// implement it with a recorder.
pub trait Runner: Sync {
    /// The path of `name` on `PATH`, if it is there.
    fn find(&self, name: &str) -> Option<PathBuf>;

    /// Runs `prog` in `cwd` with exactly `env`, killed after `timeout`; returns the exit code,
    /// stdout and stderr.
    fn run(
        &self,
        prog: &Path,
        args: &[OsString],
        cwd: &Path,
        env: &[(OsString, OsString)],
        timeout: Duration,
    ) -> io::Result<(i32, Vec<u8>, Vec<u8>)>;
}

/// What became of one tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolStatus {
    /// Ran and reported this many findings.
    Ran(usize),
    /// Not on `PATH`; the text says what it would add.
    Missing(&'static str),
    Failed(String),
    Skipped(String),
}

/// One tool and its status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRun {
    pub name: &'static str,
    pub status: ToolStatus,
}

/// The arguments of `tool` for `target`; `scratch` holds the config files and reports. Each
/// argument list names a config file of ours in `scratch` (or turns config discovery off), so a
/// config file inside `target` is never read. An unknown `tool` gives an empty list.
pub fn tool_argv(tool: &str, target: &Path, scratch: &Path) -> Vec<OsString> {
    let os = |s: &str| OsString::from(s);
    match tool {
        OSV => {
            let mut config = os("--config=");
            config.push(scratch.join("osv-scanner.toml"));
            vec![
                os("scan"),
                os("source"),
                os("-r"),
                os("--offline"),
                config,
                os("--format"),
                os("json"),
                target.into(),
            ]
        }
        GITLEAKS => vec![
            os("dir"),
            os("--config"),
            scratch.join("gitleaks.toml").into(),
            os("--ignore-gitleaks-allow"),
            os("--no-banner"),
            os("--redact"),
            os("--report-format"),
            os("json"),
            os("--report-path"),
            scratch.join("gitleaks.json").into(),
            os("--exit-code"),
            os("0"),
            target.into(),
        ],
        ZIZMOR => vec![
            os("--offline"),
            os("--no-config"),
            os("--format"),
            os("json"),
            target.into(),
        ],
        _ => Vec::new(),
    }
}

/// Runs the tools found by `r` on `target`, in parallel, each for at most 60 seconds. The
/// result lists every tool in the order `osv-scanner`, `gitleaks`, `zizmor`; a tool that is not
/// on `PATH` is `Missing` and nothing runs for it.
pub fn run_tools(target: &Path, scratch: &Path, r: &dyn Runner) -> (Vec<ToolRun>, Vec<Finding>) {
    // The tools run in `scratch`, so a relative path would point somewhere else for them.
    let target = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    let scratch = std::path::absolute(scratch).unwrap_or_else(|_| scratch.to_path_buf());
    let env = tool_env(
        std::env::var_os("PATH"),
        std::env::var_os("HOME"),
        std::env::var_os("OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY"),
        &scratch,
    );
    let (target, scratch, env) = (&target, &scratch, &env);
    let results: Vec<(ToolStatus, Vec<Finding>)> = std::thread::scope(|s| {
        let handles: Vec<_> = TOOLS
            .iter()
            .map(|&name| {
                r.find(name)
                    .map(|prog| s.spawn(move || run_one(name, &prog, target, scratch, r, env)))
            })
            .collect();
        handles
            .into_iter()
            .zip(TOOLS)
            .map(|(handle, name)| match handle {
                None => (ToolStatus::Missing(adds(name)), Vec::new()),
                Some(h) => h.join().unwrap_or_else(|_| {
                    (
                        ToolStatus::Failed(format!("{name} stopped unexpectedly")),
                        Vec::new(),
                    )
                }),
            })
            .collect()
    });
    let mut runs = Vec::new();
    let mut findings = Vec::new();
    for (name, (status, found)) in TOOLS.into_iter().zip(results) {
        runs.push(ToolRun { name, status });
        findings.extend(found);
    }
    (runs, findings)
}

/// The environment of a tool call: `PATH`, `HOME` and the offline database folder, never a token.
/// `HOME` falls back to `scratch` so a tool cannot reach the real home through it.
fn tool_env(
    path: Option<OsString>,
    home: Option<OsString>,
    osv_db: Option<OsString>,
    scratch: &Path,
) -> Vec<(OsString, OsString)> {
    let mut env = Vec::new();
    if let Some(path) = path {
        env.push((OsString::from("PATH"), path));
    }
    env.push((
        OsString::from("HOME"),
        home.unwrap_or_else(|| scratch.as_os_str().to_owned()),
    ));
    if let Some(db) = osv_db {
        env.push((OsString::from("OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY"), db));
    }
    env
}

/// What a missing tool would add to the report.
fn adds(tool: &str) -> &'static str {
    match tool {
        OSV => {
            "would match lockfile packages against known-malicious and vulnerable versions, offline"
        }
        GITLEAKS => "would look for planted or leaked secrets",
        _ => "would check GitHub Actions workflows for risky patterns",
    }
}

/// Writes the config files of `name` into `scratch`.
fn prepare(name: &str, scratch: &Path) -> io::Result<()> {
    match name {
        OSV => std::fs::write(scratch.join("osv-scanner.toml"), ""),
        GITLEAKS => {
            std::fs::write(
                scratch.join("gitleaks.toml"),
                "[extend]\nuseDefault = true\n",
            )?;
            // A report left by an earlier run must not pass for this one.
            match std::fs::remove_file(scratch.join("gitleaks.json")) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        }
        _ => Ok(()),
    }
}

/// Runs one tool and maps what it printed to a status and findings.
fn run_one(
    name: &'static str,
    prog: &Path,
    target: &Path,
    scratch: &Path,
    r: &dyn Runner,
    env: &[(OsString, OsString)],
) -> (ToolStatus, Vec<Finding>) {
    let failed = |why: String| (ToolStatus::Failed(why), Vec::new());
    if let Err(e) = prepare(name, scratch) {
        return failed(format!("cannot write the {name} config: {e}"));
    }
    let args = tool_argv(name, target, scratch);
    let (code, stdout, stderr) = match r.run(prog, &args, scratch, env, TIMEOUT) {
        Ok(done) => done,
        Err(e) if e.kind() == io::ErrorKind::TimedOut => {
            return failed(format!("{name} timed out after {} s", TIMEOUT.as_secs()));
        }
        Err(e) => return failed(format!("{name} did not run: {e}")),
    };
    let exited = || format!("{name} exited {code}: {}", last_line(&stderr));
    let ran = |found: Vec<Finding>| (ToolStatus::Ran(found.len()), found);
    match name {
        OSV => match code {
            // 0: nothing found; 1: something found.
            0 | 1 => match report(&stdout) {
                Some(doc) => ran(osv_findings(&doc, target)),
                None => failed(format!("{name} printed no JSON report")),
            },
            // No lockfile or manifest in the tree.
            128 => ran(Vec::new()),
            127 if String::from_utf8_lossy(&stderr).contains(OSV_NO_DB) => {
                (ToolStatus::Skipped(OSV_SKIPPED.to_string()), Vec::new())
            }
            _ => failed(exited()),
        },
        GITLEAKS => {
            if code != 0 {
                return failed(exited());
            }
            let Ok(text) = std::fs::read(scratch.join("gitleaks.json")) else {
                return failed(format!("{name} wrote no report"));
            };
            match report(&text) {
                Some(doc) => ran(gitleaks_findings(&doc, target)),
                None => failed(format!("{name} wrote a report that is not JSON")),
            }
        }
        _ => match code {
            // 0: nothing found; 10 to 14: findings, by their highest severity.
            0 | 10..=14 => match report(&stdout) {
                Some(doc) => ran(zizmor_findings(&doc, target)),
                None => failed(format!("{name} printed no JSON report")),
            },
            // No workflow in the tree.
            3 if String::from_utf8_lossy(&stderr).contains("no inputs collected") => {
                ran(Vec::new())
            }
            _ => failed(exited()),
        },
    }
}

/// osv-scanner: a package with a `MAL-` record is CRITICAL and its other advisories (the same
/// malware, filed under other ids) are dropped; every other advisory is one LOW finding per
/// lockfile with the ids in `seen`.
fn osv_findings(doc: &Value, target: &Path) -> Vec<Finding> {
    let mut out = Vec::new();
    for result in items(get(doc, "results")) {
        let path = rel(target, text(get(result, "source"), "path").unwrap_or(""));
        let mut malicious = Hits::default();
        let mut known = Hits::default();
        for pkg in items(get(result, "packages")) {
            let package = get(pkg, "package");
            let label = format!(
                "{}@{}",
                text(package, "name").unwrap_or("?"),
                text(package, "version").unwrap_or("?")
            );
            let vulns = items(get(pkg, "vulnerabilities"));
            let ids: Vec<&str> = vulns.iter().filter_map(|v| text(Some(v), "id")).collect();
            let is_mal = |s: &&str| s.starts_with("MAL-");
            let mal_ids: Vec<&str> = if ids.iter().any(|i| i.starts_with("MAL-")) {
                ids.iter().copied().filter(is_mal).collect()
            } else {
                vulns
                    .iter()
                    .flat_map(|v| items(get(v, "aliases")))
                    .filter_map(Value::as_str)
                    .filter(is_mal)
                    .collect()
            };
            if mal_ids.is_empty() {
                known.add(&label, ids);
            } else {
                malicious.add(&label, mal_ids);
            }
        }
        if !malicious.ids.is_empty() {
            out.push(Finding {
                severity: Severity::Critical,
                group: Group::Deps,
                rule: "deps.osv-malicious".to_string(),
                path: path.clone(),
                line: 0,
                why: format!(
                    "osv-scanner matched a known-malicious package record: {}",
                    malicious.packages()
                ),
                seen: malicious.seen(),
                action: "Do not install, build or open this repo until the dependency is removed."
                    .to_string(),
                count: malicious.ids.len(),
            });
        }
        if !known.ids.is_empty() {
            out.push(Finding {
                severity: Severity::Low,
                group: Group::Deps,
                rule: "deps.osv-known-vuln".to_string(),
                path,
                line: 0,
                why: format!(
                    "osv-scanner found known vulnerabilities in: {}",
                    known.packages()
                ),
                seen: known.seen(),
                action: "Update the dependency before you ship this code.".to_string(),
                count: known.ids.len(),
            });
        }
    }
    out
}

/// gitleaks: one MEDIUM finding per file, with the rule ids (the values are redacted by the
/// `--redact` flag, and no value is read here).
fn gitleaks_findings(doc: &Value, target: &Path) -> Vec<Finding> {
    struct FileHits {
        path: String,
        line: usize,
        rules: Vec<String>,
        count: usize,
        first: String,
    }
    let mut files: Vec<FileHits> = Vec::new();
    for hit in items(Some(doc)) {
        let path = rel(target, text(Some(hit), "File").unwrap_or(""));
        let rule = id_text(text(Some(hit), "RuleID").unwrap_or("?"));
        let line = number(get(hit, "StartLine"));
        let desc = clean(text(Some(hit), "Description").unwrap_or(""));
        match files.iter_mut().find(|f| f.path == path) {
            Some(f) => {
                f.count += 1;
                if !f.rules.contains(&rule) {
                    f.rules.push(rule);
                }
            }
            None => files.push(FileHits {
                path,
                line,
                rules: vec![rule],
                count: 1,
                first: desc,
            }),
        }
    }
    files
        .into_iter()
        .map(|f| Finding {
            severity: Severity::Medium,
            group: Group::Tool,
            rule: "secrets.gitleaks".to_string(),
            path: f.path,
            line: f.line,
            why: if f.count == 1 {
                format!("gitleaks: {}", f.first)
            } else {
                format!(
                    "gitleaks matched {} secret patterns in this file; the values are redacted",
                    f.count
                )
            },
            seen: f.rules.join(", "),
            action: "Check whether the key is real. Do not use it: a planted key can be bait."
                .to_string(),
            count: f.count,
        })
        .collect()
}

/// zizmor: one finding per (check, workflow file); High is MEDIUM, Medium is LOW, anything
/// else INFO (a cloner is not the maintainer these risks are for).
fn zizmor_findings(doc: &Value, target: &Path) -> Vec<Finding> {
    let mut out: Vec<Finding> = Vec::new();
    for item in items(Some(doc)) {
        let ident = id_text(text(Some(item), "ident").unwrap_or(""));
        if ident.is_empty() {
            continue;
        }
        let locations = items(get(item, "locations"));
        let primary = locations
            .iter()
            .find(|l| text(get(l, "symbolic"), "kind") == Some("Primary"))
            .or_else(|| locations.first());
        let key = get(
            primary.and_then(|l| get(l, "symbolic")).unwrap_or(item),
            "key",
        );
        let local = get(key.unwrap_or(item), "Local");
        let raw_path = text(local, "verbatim_path")
            .or_else(|| text(local, "given_path"))
            .unwrap_or("");
        let path = rel(target, raw_path);
        let rule = format!("ci.zizmor.{ident}");
        if let Some(f) = out.iter_mut().find(|f| f.rule == rule && f.path == path) {
            f.count += 1;
            continue;
        }
        let severity = match text(get(item, "determinations"), "severity") {
            Some("High") => Severity::Medium,
            Some("Medium") => Severity::Low,
            _ => Severity::Info,
        };
        let row = primary
            .and_then(|l| get(l, "concrete"))
            .and_then(|c| get(c, "location"))
            .and_then(|l| get(l, "start_point"))
            .map(|p| number(get(p, "row")) + 1)
            .unwrap_or(0);
        let desc = clean(text(Some(item), "desc").unwrap_or(&ident));
        let note =
            clean(text(primary.and_then(|l| get(l, "symbolic")), "annotation").unwrap_or(""));
        let seen = text(primary.and_then(|l| get(l, "concrete")), "feature")
            .and_then(|s| s.lines().find(|l| !l.trim().is_empty()))
            .map(|l| cap(l.trim(), 200))
            .unwrap_or_default();
        out.push(Finding {
            severity,
            group: Group::Tool,
            rule,
            path,
            line: row,
            why: if note.is_empty() {
                format!("zizmor: {desc}")
            } else {
                format!("zizmor: {desc} ({note})")
            },
            seen,
            action:
                "Read the workflow before CI runs it; these risks matter most to its maintainers."
                    .to_string(),
            count: 1,
        });
    }
    out
}

/// The packages and ids of one kind of osv hit in one lockfile.
#[derive(Default)]
struct Hits {
    packages: Vec<String>,
    ids: Vec<String>,
}

impl Hits {
    fn add(&mut self, package: &str, ids: Vec<&str>) {
        if ids.is_empty() {
            return;
        }
        self.packages.push(package.to_string());
        for id in ids {
            let id = id_text(id);
            if !self.ids.contains(&id) {
                self.ids.push(id);
            }
        }
    }

    fn packages(&self) -> String {
        more(&self.packages, 5)
    }

    fn seen(&self) -> String {
        more(&self.ids, MAX_IDS)
    }
}

/// The first `max` items joined, then `and N more`.
fn more(items: &[String], max: usize) -> String {
    let mut s = items
        .iter()
        .take(max)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > max {
        s.push_str(&format!(", and {} more", items.len() - max));
    }
    s
}

/// The JSON document a tool printed.
fn report(bytes: &[u8]) -> Option<Value> {
    json::loads_bytes_lossy(bytes).ok()
}

fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.as_object()?.get(key)
}

fn text<'a>(v: Option<&'a Value>, key: &str) -> Option<&'a str> {
    get(v?, key)?.as_str()
}

fn items(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array).map_or(&[], |a| a.as_slice())
}

/// A non-negative integer field; anything else is 0.
fn number(v: Option<&Value>) -> usize {
    v.and_then(Value::as_i64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(0)
}

/// `p` relative to `target` with `/` separators; a path outside it stays as given.
fn rel(target: &Path, p: &str) -> String {
    match Path::new(p).strip_prefix(target) {
        Ok(r) if !r.as_os_str().is_empty() => r
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        _ => p.to_string(),
    }
}

/// An id or rule name from a tool: only `[A-Za-z0-9._-]` survives, so it is safe inside a rule
/// name and a report line.
fn id_text(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .take(100)
        .collect()
}

/// Text from a tool on one line without control characters, cut to `MAX_TEXT` characters.
fn clean(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    cap(flat.trim(), MAX_TEXT)
}

fn cap(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The last non-empty line of a tool's stderr, safe to print.
fn last_line(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("no error text");
    clean(line)
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::fake::{Call, Fake};
    use super::*;
    use crate::{Group, Severity};

    type Reply = io::Result<(i32, Vec<u8>, Vec<u8>)>;

    fn out(code: i32, stdout: &str, stderr: &str) -> Reply {
        Ok((code, stdout.as_bytes().to_vec(), stderr.as_bytes().to_vec()))
    }

    /// `osv-scanner scan source -r --offline --format json` on a Cargo.lock with `time 0.1.43`
    /// and on `mal/Cargo.lock` with `rustdecimal 1.26.1`, recorded from
    /// `ghcr.io/google/osv-scanner` with the crates.io database (exit 1). Long `details`,
    /// `affected` and `references` fields are cut; every key the mapping reads is as recorded.
    const OSV: &str = r#"{
  "results": [
    {
      "source": {"path": "@T@/Cargo.lock", "type": "lockfile"},
      "packages": [
        {
          "package": {"name": "time", "version": "0.1.43", "ecosystem": "crates.io", "purl": "pkg:cargo/time@0.1.43"},
          "groups": [{"ids": ["RUSTSEC-2020-0071", "GHSA-wcg3-cvx6-7396"], "aliases": ["CVE-2020-26235", "GHSA-wcg3-cvx6-7396", "RUSTSEC-2020-0071"], "max_severity": "6.2"}],
          "vulnerabilities": [
            {"aliases": ["CVE-2020-26235", "GHSA-wcg3-cvx6-7396"], "id": "RUSTSEC-2020-0071", "modified": "2026-02-04T02:31:56.682937Z", "schema_version": "1.7.3", "summary": "Potential segfault in the time crate"},
            {"aliases": ["CVE-2020-26235", "RUSTSEC-2020-0071"], "id": "GHSA-wcg3-cvx6-7396", "modified": "2026-09-10T03:49:08.359875406Z", "schema_version": "1.9.0", "summary": "Segmentation fault in time"}
          ]
        }
      ]
    },
    {
      "source": {"path": "@T@/mal/Cargo.lock", "type": "lockfile"},
      "packages": [
        {
          "package": {"name": "rustdecimal", "version": "1.26.1", "ecosystem": "crates.io", "purl": "pkg:cargo/rustdecimal@1.26.1"},
          "groups": [{"ids": ["GHSA-7pwq-f4pq-78gm", "MAL-2022-1", "RUSTSEC-2022-0042"], "aliases": ["GHSA-7pwq-f4pq-78gm", "MAL-2022-1", "RUSTSEC-2022-0042"], "max_severity": "0.0"}],
          "vulnerabilities": [
            {"aliases": ["MAL-2022-1", "RUSTSEC-2022-0042"], "id": "GHSA-7pwq-f4pq-78gm", "summary": "Malicious code in rustdecimal"},
            {"aliases": ["GHSA-7pwq-f4pq-78gm", "RUSTSEC-2022-0042"], "id": "MAL-2022-1", "summary": "Malicious code in rustdecimal (crates.io)"},
            {"aliases": ["GHSA-7pwq-f4pq-78gm", "MAL-2022-1"], "id": "RUSTSEC-2022-0042", "summary": "Crate rustdecimal is malicious"}
          ]
        }
      ]
    }
  ],
  "experimental_config": {"licenses": {"summary": false, "allowlist": null}}
}"#;

    /// `gitleaks dir ... --redact --report-format json` on a folder with three planted tokens,
    /// recorded from the pinned image (exit 0). The first two hits are moved into one file, a
    /// third into another, to cover the grouping.
    const GITLEAKS: &str = r#"[
 {"RuleID": "slack-bot-token", "Description": "Identified a Slack Bot token, which may compromise bot integrations and communication channel security.", "StartLine": 1, "EndLine": 1, "StartColumn": 13, "EndColumn": 68, "Match": "REDACTED", "Secret": "REDACTED", "File": "@T@/config.txt", "SymlinkFile": "", "Commit": "", "Entropy": 4.9254966, "Author": "", "Email": "", "Date": "", "Message": "", "Tags": [], "Fingerprint": "@T@/config.txt:slack-bot-token:1"},
 {"RuleID": "github-pat", "Description": "Uncovered a GitHub Personal Access Token, potentially leading to unauthorized repository access and sensitive content exposure.", "StartLine": 3, "EndLine": 3, "StartColumn": 17, "EndColumn": 56, "Match": "REDACTED", "Secret": "REDACTED", "File": "@T@/config.txt", "SymlinkFile": "", "Commit": "", "Entropy": 5.171928, "Author": "", "Email": "", "Date": "", "Message": "", "Tags": [], "Fingerprint": "@T@/config.txt:github-pat:3"},
 {"RuleID": "stripe-access-token", "Description": "Found a Stripe Access Token, posing a risk to payment processing services and sensitive financial data.", "StartLine": 2, "EndLine": 2, "StartColumn": 11, "EndColumn": 50, "Match": "REDACTED", "Secret": "REDACTED", "File": "@T@/sub/env.txt", "SymlinkFile": "", "Commit": "", "Entropy": 4.871928, "Author": "", "Email": "", "Date": "", "Message": "", "Tags": [], "Fingerprint": "@T@/sub/env.txt:stripe-access-token:2"}
]"#;

    /// `zizmor --offline --no-config --format json` on one workflow, recorded from
    /// `ghcr.io/zizmorcore/zizmor` v1.33.1 (exit 14). `route`, `fixes` and `offset_span` are cut.
    /// The `excessive-permissions` and `obfuscation` severities were edited to `Low` and
    /// `Informational` to cover the whole mapping.
    const ZIZMOR: &str = r#"[
  {
    "ident": "artipacked", "desc": "credential persistence through GitHub Actions artifacts",
    "url": "https://docs.zizmor.sh/audits/#artipacked",
    "determinations": {"confidence": "Low", "severity": "Medium", "persona": "Regular"},
    "locations": [
      {"symbolic": {"key": {"Local": {"verbatim_path": "@T@/.github/workflows/ci.yml"}}, "annotation": "does not set persist-credentials: false", "feature_kind": "Normal", "kind": "Primary"},
       "concrete": {"location": {"start_point": {"row": 6, "column": 8}, "end_point": {"row": 6, "column": 33}}, "feature": "uses: actions/checkout@v4", "comments": []}}
    ],
    "ignored": false
  },
  {
    "ident": "excessive-permissions", "desc": "overly broad permissions",
    "url": "https://docs.zizmor.sh/audits/#excessive-permissions",
    "determinations": {"confidence": "Medium", "severity": "Low", "persona": "Regular"},
    "locations": [
      {"symbolic": {"key": {"Local": {"verbatim_path": "@T@/.github/workflows/ci.yml"}}, "annotation": "this job", "feature_kind": "Normal", "kind": "Related"},
       "concrete": {"location": {"start_point": {"row": 3, "column": 2}, "end_point": {"row": 8, "column": 0}}, "feature": "  x:\n    runs-on: ubuntu-latest\n", "comments": []}},
      {"symbolic": {"key": {"Local": {"verbatim_path": "@T@/.github/workflows/ci.yml"}}, "annotation": "default permissions used due to no permissions: block", "feature_kind": "Normal", "kind": "Primary"},
       "concrete": {"location": {"start_point": {"row": 3, "column": 2}, "end_point": {"row": 8, "column": 0}}, "feature": "  x:\n    runs-on: ubuntu-latest\n", "comments": []}}
    ],
    "ignored": false
  },
  {
    "ident": "template-injection", "desc": "code injection via template expansion",
    "url": "https://docs.zizmor.sh/audits/#template-injection",
    "determinations": {"confidence": "High", "severity": "High", "persona": "Regular"},
    "locations": [
      {"symbolic": {"key": {"Local": {"verbatim_path": "@T@/.github/workflows/ci.yml"}}, "annotation": "this step", "feature_kind": "Normal", "kind": "Hidden"},
       "concrete": {"location": {"start_point": {"row": 7, "column": 8}, "end_point": {"row": 8, "column": 0}}, "feature": "run: echo \"${{ github.event.pull_request.title }}\"\n", "comments": []}},
      {"symbolic": {"key": {"Local": {"verbatim_path": "@T@/.github/workflows/ci.yml"}}, "annotation": "may expand into attacker-controllable code", "feature_kind": {"Subfeature": {"after": 7, "fragment": {"Raw": "github.event.pull_request.title"}}}, "kind": "Primary"},
       "concrete": {"location": {"start_point": {"row": 7, "column": 23}, "end_point": {"row": 7, "column": 54}}, "feature": "echo \"${{ github.event.pull_request.title }}\"", "comments": []}}
    ],
    "ignored": false
  },
  {
    "ident": "obfuscation", "desc": "obfuscated usage of GitHub Actions features",
    "url": "https://docs.zizmor.sh/audits/#obfuscation",
    "determinations": {"confidence": "High", "severity": "Informational", "persona": "Regular"},
    "locations": [
      {"symbolic": {"key": {"Local": {"verbatim_path": "@T@/.github/workflows/ci.yml"}}, "annotation": "can be replaced by a constant", "feature_kind": "Normal", "kind": "Primary"},
       "concrete": {"location": {"start_point": {"row": 9, "column": 4}, "end_point": {"row": 9, "column": 20}}, "feature": "if: ${{ 'a' }}", "comments": []}}
    ],
    "ignored": false
  }
]"#;

    struct Rig {
        _tmp: tempfile::TempDir,
        target: PathBuf,
        scratch: PathBuf,
    }

    fn rig() -> Rig {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("repo");
        let scratch = tmp.path().join("scratch");
        std::fs::create_dir(&target).unwrap();
        std::fs::create_dir(&scratch).unwrap();
        Rig {
            _tmp: tmp,
            target,
            scratch,
        }
    }

    fn words(argv: &[OsString]) -> Vec<String> {
        argv.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn prog(c: &Call) -> String {
        c.prog.file_name().unwrap().to_string_lossy().into_owned()
    }

    /// Writes `body` where gitleaks was told to put its report.
    fn write_gitleaks_report(c: &Call, body: &str) {
        let w = c.words();
        let i = w.iter().position(|a| a == "--report-path").unwrap();
        std::fs::write(&w[i + 1], body).unwrap();
    }

    /// Every tool answers with its recorded sample.
    fn samples(target: &Path) -> Fake {
        let t = target.to_string_lossy().into_owned();
        Fake::new(
            &["osv-scanner", "gitleaks", "zizmor"],
            move |c| match prog(c).as_str() {
                "osv-scanner" => out(1, &OSV.replace("@T@", &t), "Loaded crates.io local db\n"),
                "zizmor" => out(14, &ZIZMOR.replace("@T@", &t), " INFO zizmor: v1.33.1\n"),
                "gitleaks" => {
                    write_gitleaks_report(c, &GITLEAKS.replace("@T@", &t));
                    out(0, "", "WRN leaks found: 3\n")
                }
                other => panic!("unexpected program {other}"),
            },
        )
    }

    /// Every tool finds nothing.
    fn quiet() -> Fake {
        Fake::new(&["osv-scanner", "gitleaks", "zizmor"], |c| {
            match prog(c).as_str() {
                "osv-scanner" => out(0, "{\"results\": []}", ""),
                "zizmor" => out(0, "[]", ""),
                _ => {
                    write_gitleaks_report(c, "[]");
                    out(0, "", "")
                }
            }
        })
    }

    fn status_of(runs: &[ToolRun], name: &str) -> ToolStatus {
        runs.iter().find(|r| r.name == name).unwrap().status.clone()
    }

    #[test]
    fn tool_argv_ignores_repo_config() {
        let t = Path::new("/t/repo");
        let s = Path::new("/t/scratch");
        assert_eq!(
            words(&tool_argv("osv-scanner", t, s)),
            [
                "scan",
                "source",
                "-r",
                "--offline",
                "--config=/t/scratch/osv-scanner.toml",
                "--format",
                "json",
                "/t/repo"
            ]
        );
        assert_eq!(
            words(&tool_argv("gitleaks", t, s)),
            [
                "dir",
                "--config",
                "/t/scratch/gitleaks.toml",
                "--ignore-gitleaks-allow",
                "--no-banner",
                "--redact",
                "--report-format",
                "json",
                "--report-path",
                "/t/scratch/gitleaks.json",
                "--exit-code",
                "0",
                "/t/repo"
            ]
        );
        assert_eq!(
            words(&tool_argv("zizmor", t, s)),
            ["--offline", "--no-config", "--format", "json", "/t/repo"]
        );
        assert!(tool_argv("trufflehog", t, s).is_empty());
    }

    #[test]
    fn tools_run_in_scratch_with_the_plan_argv_and_a_short_env() {
        let rig = rig();
        // The repo tries to steer every tool.
        for name in [".gitleaks.toml", "zizmor.yml", "osv-scanner.toml"] {
            std::fs::write(rig.target.join(name), "trap").unwrap();
        }
        let fake = quiet();
        let (runs, findings) = run_tools(&rig.target, &rig.scratch, &fake);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(
            runs.iter().map(|r| r.name).collect::<Vec<_>>(),
            ["osv-scanner", "gitleaks", "zizmor"]
        );
        let calls = fake.calls();
        assert_eq!(calls.len(), 3);
        for c in &calls {
            let name = prog(c);
            assert_eq!(c.cwd, rig.scratch, "{name}: cwd is the scratch folder");
            assert_eq!(c.timeout, Duration::from_secs(60), "{name}");
            assert_eq!(
                words(&c.args),
                words(&tool_argv(&name, &rig.target, &rig.scratch)),
                "{name}"
            );
            // The target is only ever the last argument: no repo file is named as config.
            let last = c.args.last().unwrap();
            assert_eq!(Path::new(last), rig.target, "{name}");
            assert!(
                c.args[..c.args.len() - 1]
                    .iter()
                    .all(|a| !Path::new(a).starts_with(&rig.target)),
                "{name}: {:?}",
                c.args
            );
            for (k, _) in &c.env {
                let k = k.to_string_lossy();
                assert!(
                    ["PATH", "HOME", "OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY"].contains(&&*k),
                    "{name}: {k} must not reach a tool"
                );
            }
        }
        // The empty and default configs are ours, in scratch.
        assert_eq!(
            std::fs::read_to_string(rig.scratch.join("osv-scanner.toml")).unwrap(),
            ""
        );
        assert_eq!(
            std::fs::read_to_string(rig.scratch.join("gitleaks.toml")).unwrap(),
            "[extend]\nuseDefault = true\n"
        );
        // The repo's own files were left alone.
        assert_eq!(
            std::fs::read_to_string(rig.target.join(".gitleaks.toml")).unwrap(),
            "trap"
        );
    }

    #[test]
    fn tool_env_is_a_short_list() {
        let scratch = Path::new("/t/scratch");
        let pairs = |v: Vec<(OsString, OsString)>| -> Vec<(String, String)> {
            v.into_iter()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
                .collect()
        };
        let want = |v: &[(&str, &str)]| -> Vec<(String, String)> {
            v.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        assert_eq!(
            pairs(tool_env(
                Some("/usr/bin".into()),
                Some("/home/u".into()),
                Some("/db".into()),
                scratch
            )),
            want(&[
                ("PATH", "/usr/bin"),
                ("HOME", "/home/u"),
                ("OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY", "/db"),
            ])
        );
        assert_eq!(
            pairs(tool_env(Some("/usr/bin".into()), None, None, scratch)),
            want(&[("PATH", "/usr/bin"), ("HOME", "/t/scratch")])
        );
    }

    #[test]
    fn tool_output_maps_to_findings() {
        let rig = rig();
        let fake = samples(&rig.target);
        let (runs, findings) = run_tools(&rig.target, &rig.scratch, &fake);
        assert_eq!(status_of(&runs, "osv-scanner"), ToolStatus::Ran(2));
        assert_eq!(status_of(&runs, "gitleaks"), ToolStatus::Ran(2));
        assert_eq!(status_of(&runs, "zizmor"), ToolStatus::Ran(4));
        assert_eq!(findings.len(), 8, "{findings:#?}");

        let one = |rule: &str, path: &str| -> &Finding {
            let hits: Vec<&Finding> = findings
                .iter()
                .filter(|f| f.rule == rule && f.path == path)
                .collect();
            assert_eq!(hits.len(), 1, "{rule} {path}: {findings:#?}");
            hits[0]
        };

        // osv-scanner: a MAL- record is CRITICAL and hides the package's other advisories;
        // the rest are one LOW finding per lockfile with the ids in `seen`.
        let mal = one("deps.osv-malicious", "mal/Cargo.lock");
        assert_eq!(mal.severity, Severity::Critical);
        assert_eq!(mal.group, Group::Deps);
        assert_eq!(mal.line, 0);
        assert_eq!(mal.count, 1);
        assert_eq!(mal.seen, "MAL-2022-1");
        assert!(mal.why.contains("rustdecimal@1.26.1"), "{}", mal.why);
        let vuln = one("deps.osv-known-vuln", "Cargo.lock");
        assert_eq!(vuln.severity, Severity::Low);
        assert_eq!(vuln.group, Group::Deps);
        assert_eq!(vuln.count, 2);
        assert_eq!(vuln.seen, "RUSTSEC-2020-0071, GHSA-wcg3-cvx6-7396");
        assert!(vuln.why.contains("time@0.1.43"), "{}", vuln.why);
        assert!(
            !findings
                .iter()
                .any(|f| f.rule == "deps.osv-known-vuln" && f.path == "mal/Cargo.lock"),
            "a malicious package is not also listed as a known vulnerability"
        );

        // gitleaks: MEDIUM, one per file, the first line, rule ids but never a secret.
        let cfg = one("secrets.gitleaks", "config.txt");
        assert_eq!(cfg.severity, Severity::Medium);
        assert_eq!(cfg.group, Group::Tool);
        assert_eq!((cfg.line, cfg.count), (1, 2));
        assert_eq!(cfg.seen, "slack-bot-token, github-pat");
        let env = one("secrets.gitleaks", "sub/env.txt");
        assert_eq!((env.line, env.count), (2, 1));
        assert_eq!(env.seen, "stripe-access-token");

        // zizmor: High -> MEDIUM, Medium -> LOW, anything else INFO; the Primary location.
        let inj = one("ci.zizmor.template-injection", ".github/workflows/ci.yml");
        assert_eq!(inj.severity, Severity::Medium);
        assert_eq!(inj.group, Group::Tool);
        assert_eq!(inj.line, 8);
        assert!(
            inj.why.contains("code injection via template expansion"),
            "{}",
            inj.why
        );
        assert!(
            inj.why
                .contains("may expand into attacker-controllable code"),
            "{}",
            inj.why
        );
        assert!(
            inj.seen.contains("github.event.pull_request.title"),
            "{}",
            inj.seen
        );
        let art = one("ci.zizmor.artipacked", ".github/workflows/ci.yml");
        assert_eq!((art.severity, art.line), (Severity::Low, 7));
        let perm = one(
            "ci.zizmor.excessive-permissions",
            ".github/workflows/ci.yml",
        );
        assert_eq!((perm.severity, perm.line), (Severity::Info, 4));
        let obf = one("ci.zizmor.obfuscation", ".github/workflows/ci.yml");
        assert_eq!(obf.severity, Severity::Info);

        for f in &findings {
            assert!(!f.action.is_empty(), "{}", f.rule);
            assert!(!f.path.starts_with('/'), "{} {}", f.rule, f.path);
        }
    }

    #[test]
    fn missing_tools_are_listed_not_installed() {
        let rig = rig();
        let fake = Fake::new(&[], |_| panic!("nothing is found, so nothing runs"));
        let (runs, findings) = run_tools(&rig.target, &rig.scratch, &fake);
        assert!(findings.is_empty());
        assert!(fake.calls().is_empty());
        assert_eq!(
            runs.iter().map(|r| r.name).collect::<Vec<_>>(),
            ["osv-scanner", "gitleaks", "zizmor"]
        );
        let mut texts = Vec::new();
        for r in &runs {
            match r.status {
                ToolStatus::Missing(what) => {
                    assert!(!what.is_empty() && !what.contains('\n'), "{}", r.name);
                    texts.push(what);
                }
                ref other => panic!("{}: {other:?}", r.name),
            }
        }
        texts.sort_unstable();
        texts.dedup();
        assert_eq!(texts.len(), 3, "each tool says what it would add");
        assert_eq!(std::fs::read_dir(&rig.scratch).unwrap().count(), 0);

        // One tool present: the others stay missing and leave no config behind.
        let only = Fake::new(&["zizmor"], |_| out(0, "[]", ""));
        let (runs, _) = run_tools(&rig.target, &rig.scratch, &only);
        assert!(matches!(
            status_of(&runs, "osv-scanner"),
            ToolStatus::Missing(_)
        ));
        assert!(matches!(
            status_of(&runs, "gitleaks"),
            ToolStatus::Missing(_)
        ));
        assert_eq!(status_of(&runs, "zizmor"), ToolStatus::Ran(0));
        assert_eq!(only.calls().len(), 1);
        assert!(!rig.scratch.join("osv-scanner.toml").exists());
        assert!(!rig.scratch.join("gitleaks.toml").exists());
    }

    #[test]
    fn osv_without_a_database_is_skipped() {
        let rig = rig();
        let fake = Fake::new(&["osv-scanner"], |_| {
            out(
                127,
                "{\"results\": []}",
                "Scanned /fx/Cargo.lock file and found 2 packages\n\
                 could not load db for crates.io ecosystem: unable to fetch OSV database: no offline version of the OSV database is available\n",
            )
        });
        let (runs, findings) = run_tools(&rig.target, &rig.scratch, &fake);
        assert!(findings.is_empty());
        assert_eq!(
            status_of(&runs, "osv-scanner"),
            ToolStatus::Skipped(
                "no offline database; to fetch it once run: osv-scanner scan source -r --offline --download-offline-databases <path>"
                    .to_string()
            )
        );
    }

    #[test]
    fn a_tool_with_nothing_to_look_at_ran_with_no_findings() {
        let rig = rig();
        let fake = Fake::new(&["osv-scanner", "zizmor"], |c| {
            if prog(c) == "osv-scanner" {
                out(
                    128,
                    "",
                    "No package sources found, --help for usage information.\n",
                )
            } else {
                out(
                    3,
                    "",
                    "fatal: no audit was performed\nerror: no inputs collected\n",
                )
            }
        });
        let (runs, findings) = run_tools(&rig.target, &rig.scratch, &fake);
        assert!(findings.is_empty());
        assert_eq!(status_of(&runs, "osv-scanner"), ToolStatus::Ran(0));
        assert_eq!(status_of(&runs, "zizmor"), ToolStatus::Ran(0));
    }

    #[test]
    fn a_failing_tool_is_failed_and_the_others_still_run() {
        let rig = rig();
        let target = rig.target.to_string_lossy().into_owned();
        let fake = Fake::new(
            &["osv-scanner", "gitleaks", "zizmor"],
            move |c| match prog(c).as_str() {
                "osv-scanner" => Err(io::Error::new(io::ErrorKind::TimedOut, "killed")),
                "gitleaks" => out(1, "", "ERR boom\n"),
                _ => out(1, "not json", &format!("error in {target}\n")),
            },
        );
        let (runs, findings) = run_tools(&rig.target, &rig.scratch, &fake);
        assert!(findings.is_empty());
        let failed = |name: &str| match status_of(&runs, name) {
            ToolStatus::Failed(why) => why,
            other => panic!("{name}: {other:?}"),
        };
        assert!(failed("osv-scanner").contains("timed out"));
        assert!(failed("gitleaks").contains("boom"));
        assert!(failed("zizmor").contains("exit"));
        assert_eq!(fake.calls().len(), 3);
    }

    #[test]
    fn a_gitleaks_run_that_wrote_no_report_is_failed() {
        let rig = rig();
        let fake = Fake::new(&["gitleaks"], |_| out(0, "", ""));
        let (runs, findings) = run_tools(&rig.target, &rig.scratch, &fake);
        assert!(findings.is_empty());
        assert!(matches!(
            status_of(&runs, "gitleaks"),
            ToolStatus::Failed(_)
        ));
    }

    #[test]
    fn the_tools_run_at_the_same_time() {
        let rig = rig();
        let live = std::sync::Arc::new(AtomicUsize::new(0));
        let peak = std::sync::Arc::new(AtomicUsize::new(0));
        let (l, p) = (live.clone(), peak.clone());
        let fake = Fake::new(&["osv-scanner", "gitleaks", "zizmor"], move |c| {
            let now = l.fetch_add(1, Ordering::SeqCst) + 1;
            p.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(200));
            l.fetch_sub(1, Ordering::SeqCst);
            match prog(c).as_str() {
                "osv-scanner" => out(0, "{\"results\": []}", ""),
                "zizmor" => out(0, "[]", ""),
                _ => {
                    write_gitleaks_report(c, "[]");
                    out(0, "", "")
                }
            }
        });
        let _ = run_tools(&rig.target, &rig.scratch, &fake);
        assert!(peak.load(Ordering::SeqCst) >= 2, "the tools ran one by one");
    }
}

/// A `Runner` that records every call and answers from a closure, shared by the tests of
/// `fetch.rs` and this file.
#[cfg(test)]
pub(crate) mod fake {
    use std::ffi::OsString;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    use super::Runner;

    /// One `Runner::run` call.
    #[derive(Clone, Debug)]
    pub(crate) struct Call {
        pub prog: PathBuf,
        pub args: Vec<OsString>,
        pub cwd: PathBuf,
        pub env: Vec<(OsString, OsString)>,
        pub timeout: Duration,
    }

    impl Call {
        /// The arguments as text, for assertions.
        pub(crate) fn words(&self) -> Vec<String> {
            self.args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        }
    }

    pub(crate) type Reply = dyn Fn(&Call) -> io::Result<(i32, Vec<u8>, Vec<u8>)> + Send + Sync;

    pub(crate) struct Fake {
        /// The names `find` knows.
        pub tools: Vec<&'static str>,
        pub calls: Mutex<Vec<Call>>,
        pub reply: Box<Reply>,
    }

    impl Fake {
        pub(crate) fn new(
            tools: &[&'static str],
            reply: impl Fn(&Call) -> io::Result<(i32, Vec<u8>, Vec<u8>)> + Send + Sync + 'static,
        ) -> Fake {
            Fake {
                tools: tools.to_vec(),
                calls: Mutex::new(Vec::new()),
                reply: Box::new(reply),
            }
        }

        pub(crate) fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for Fake {
        fn find(&self, name: &str) -> Option<PathBuf> {
            self.tools
                .iter()
                .find(|t| **t == name)
                .map(|t| Path::new("/fake/bin").join(t))
        }

        fn run(
            &self,
            prog: &Path,
            args: &[OsString],
            cwd: &Path,
            env: &[(OsString, OsString)],
            timeout: Duration,
        ) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
            let call = Call {
                prog: prog.to_path_buf(),
                args: args.to_vec(),
                cwd: cwd.to_path_buf(),
                env: env.to_vec(),
                timeout,
            };
            self.calls.lock().unwrap().push(call.clone());
            (self.reply)(&call)
        }
    }
}
