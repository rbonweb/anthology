//! `replay` part `report`: the committed `port/replay-reports/<date>.md` (N-I2, design §13.7).
//!
//! **Privacy rule (the controller's ruling).** The report holds only aggregate counts and
//! HMAC-keyed case ids: both commit SHAs, the corpus manifest's sha256, totals, per-bucket and
//! per-runner counts, comparison and divergence-class counts, and the ids of the differences.
//! Never command text, paths, `HOME`, user or host names, tool or MCP server names, or any
//! transcript content. Tools appear only as the buckets `builtin`, `mcp` and `other`.
//!
//! **Shape.** Every line is a `# ` heading or a `* ` item made of fixed [`VOCABULARY`] words,
//! numbers, dates and hex ids, joined by single spaces. So once those are removed, every piece
//! of text left over (a *fragment*) is at most 3 characters long (`"\n* "`), and no corpus string
//! longer than 3 characters can hide in one.
//!
//! **The corpus-string test** ([`check_report`]): after removing the fixed vocabulary, numbers,
//! dates and 16/40/64-digit hex ids, the report must contain no corpus string (any corpus value,
//! or any whitespace-separated word of one) longer than 3 characters, no word outside the
//! vocabulary, and no leftover fragment longer than 3 characters (which also catches
//! punctuation or non-ASCII text that is not in the collected corpus). `replay report` runs it on the real corpus before it writes, and refuses to write
//! a report that fails; CI runs it on a synthetic corpus, including an injected leak.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anthology_pycompat::json::{self, INDENT2_SORTED, Int, Map, Value};

use super::collect;
use super::compare::{CLASSES, KINDS};

/// The tool buckets, in report order.
pub const BUCKETS: [&str; 3] = ["builtin", "mcp", "other"];

/// Claude Code's built-in tools (the GC-17 read-only allowlist plus the tools that change
/// things). Any `mcp__*` tool is `mcp`; every other name is `other`.
pub const BUILTIN_TOOLS: &[&str] = &[
    "Agent",
    "AskUserQuestion",
    "Bash",
    "BashOutput",
    "Edit",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "ExitWorktree",
    "Glob",
    "Grep",
    "KillShell",
    "LS",
    "ListMcpResourcesTool",
    "Monitor",
    "MultiEdit",
    "NotebookEdit",
    "NotebookRead",
    "Read",
    "ReadMcpResourceTool",
    "SendMessage",
    "Skill",
    "SlashCommand",
    "Task",
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "TaskUpdate",
    "TodoWrite",
    "ToolSearch",
    "WebFetch",
    "WebSearch",
    "Write",
];

/// The bucket index of a tool name.
pub fn bucket(tool_name: &str) -> usize {
    if tool_name.starts_with("mcp__") {
        1
    } else if BUILTIN_TOOLS.contains(&tool_name) {
        0
    } else {
        2
    }
}

/// Runner outcome columns.
pub const OUTCOMES: [&str; 3] = ["allowed", "blocked", "other"];

/// Every word the report may contain besides numbers, dates and hex ids.
pub const VOCABULARY: &[&str] = &[
    "Replay",
    "report",
    "python",
    "rust",
    "commit",
    "corpus",
    "manifest",
    "sha256",
    "unknown",
    "calls",
    "total",
    "distinct",
    "texts",
    "Calls",
    "by",
    "bucket",
    "builtin",
    "mcp",
    "other",
    "Runners",
    "runner",
    "allowed",
    "blocked",
    "Comparisons",
    "runners",
    "check",
    "analyze",
    "privacy",
    "compared",
    "same",
    "Differences",
    "unexplained",
    "explained",
    "class",
    "D-1",
    "D-2",
    "D-3",
    "D-4",
    "D-5",
    "D-6",
    "D-7",
    "Difference",
    "ids",
    "stand-ins",
    "used",
];

/// One difference: its case id, its kind and its class (`None`: unexplained).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiffId {
    pub id: String,
    pub kind: String,
    pub class: Option<String>,
}

/// What a run found: the only data a report is made from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub date: String,
    pub python_commit: String,
    pub rust_commit: String,
    pub manifest_sha256: String,
    pub calls_total: u64,
    pub calls_distinct: u64,
    pub texts_total: u64,
    pub texts_distinct: u64,
    /// Per bucket: distinct calls, and their occurrences.
    pub bucket_distinct: [u64; 3],
    pub bucket_total: [u64; 3],
    /// Runner number → per bucket → allowed, blocked, other (distinct calls).
    pub runners: BTreeMap<u8, [[u64; 3]; 3]>,
    /// Kind → (compared, same).
    pub compared: BTreeMap<String, (u64, u64)>,
    pub diffs: Vec<DiffId>,
    /// The run used the SWAP(GD2) stand-ins for the guard and shell.
    pub standins: bool,
}

fn int(n: u64) -> Value {
    Value::Int(Int::Small(i64::try_from(n).unwrap_or(i64::MAX)))
}

fn ints(v: &[u64]) -> Value {
    Value::Array(v.iter().map(|n| int(*n)).collect())
}

fn get_u64(v: Option<&Value>) -> Option<u64> {
    u64::try_from(v?.as_i64()?).ok()
}

fn get_3(v: Option<&Value>) -> Option<[u64; 3]> {
    let a = v?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([get_u64(a.first())?, get_u64(a.get(1))?, get_u64(a.get(2))?])
}

impl Summary {
    /// The run file's JSON.
    pub fn to_json(&self) -> String {
        let mut m = Map::new();
        for (k, v) in [
            ("date", &self.date),
            ("python_commit", &self.python_commit),
            ("rust_commit", &self.rust_commit),
            ("manifest_sha256", &self.manifest_sha256),
        ] {
            m.insert(k.into(), Value::Str(v.clone()));
        }
        m.insert("calls_total".into(), int(self.calls_total));
        m.insert("calls_distinct".into(), int(self.calls_distinct));
        m.insert("texts_total".into(), int(self.texts_total));
        m.insert("texts_distinct".into(), int(self.texts_distinct));
        m.insert("bucket_distinct".into(), ints(&self.bucket_distinct));
        m.insert("bucket_total".into(), ints(&self.bucket_total));
        let mut runners = Map::new();
        for (r, rows) in &self.runners {
            runners.insert(
                r.to_string(),
                Value::Array(rows.iter().map(|row| ints(row)).collect()),
            );
        }
        m.insert("runners".into(), Value::Object(runners));
        let mut compared = Map::new();
        for (k, (n, same)) in &self.compared {
            compared.insert(k.clone(), ints(&[*n, *same]));
        }
        m.insert("compared".into(), Value::Object(compared));
        let diffs = self
            .diffs
            .iter()
            .map(|d| {
                let mut e = Map::new();
                e.insert("id".into(), Value::Str(d.id.clone()));
                e.insert("kind".into(), Value::Str(d.kind.clone()));
                e.insert(
                    "class".into(),
                    d.class.clone().map_or(Value::Null, Value::Str),
                );
                Value::Object(e)
            })
            .collect();
        m.insert("diffs".into(), Value::Array(diffs));
        m.insert("standins".into(), Value::Bool(self.standins));
        let mut text = json::dumps(&Value::Object(m), &INDENT2_SORTED);
        text.push('\n');
        text
    }

    /// A run file back; `None` when it is not one.
    pub fn from_json(text: &str) -> Option<Summary> {
        let v = json::loads(text).ok()?;
        let m = v.as_object()?;
        let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
        let mut runners = BTreeMap::new();
        for (r, rows) in m.get("runners")?.as_object()?.iter() {
            let rows = rows.as_array()?;
            if rows.len() != 3 {
                return None;
            }
            let r: u8 = r.parse().ok()?;
            runners.insert(
                r,
                [
                    get_3(rows.first())?,
                    get_3(rows.get(1))?,
                    get_3(rows.get(2))?,
                ],
            );
        }
        let mut compared = BTreeMap::new();
        for (k, pair) in m.get("compared")?.as_object()?.iter() {
            let p = pair.as_array()?;
            compared.insert(k.to_string(), (get_u64(p.first())?, get_u64(p.get(1))?));
        }
        let mut diffs = Vec::new();
        for d in m.get("diffs")?.as_array()? {
            let d = d.as_object()?;
            diffs.push(DiffId {
                id: d.get("id")?.as_str()?.to_string(),
                kind: d.get("kind")?.as_str()?.to_string(),
                class: d.get("class")?.as_str().map(str::to_string),
            });
        }
        Some(Summary {
            date: s("date")?,
            python_commit: s("python_commit")?,
            rust_commit: s("rust_commit")?,
            manifest_sha256: s("manifest_sha256")?,
            calls_total: get_u64(m.get("calls_total"))?,
            calls_distinct: get_u64(m.get("calls_distinct"))?,
            texts_total: get_u64(m.get("texts_total"))?,
            texts_distinct: get_u64(m.get("texts_distinct"))?,
            bucket_distinct: get_3(m.get("bucket_distinct"))?,
            bucket_total: get_3(m.get("bucket_total"))?,
            runners,
            compared,
            diffs,
            standins: m.get("standins")?.as_bool()?,
        })
    }

    /// How many differences have no class.
    pub fn unexplained(&self) -> usize {
        self.diffs.iter().filter(|d| d.class.is_none()).count()
    }
}

fn hex_of(s: &str, len: usize) -> String {
    if s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        s.to_string()
    } else {
        "unknown".to_string()
    }
}

fn date_ok(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        })
}

/// The report (module docs). Only vocabulary words, numbers, the date and hex ids.
pub fn render(s: &Summary) -> String {
    let mut out = String::new();
    let date = if date_ok(&s.date) {
        s.date.as_str()
    } else {
        "unknown"
    };
    let _ = writeln!(out, "# Replay report {date}");
    let _ = writeln!(out, "* python commit {}", hex_of(&s.python_commit, 40));
    let _ = writeln!(out, "* rust commit {}", hex_of(&s.rust_commit, 40));
    let _ = writeln!(
        out,
        "* corpus manifest sha256 {}",
        hex_of(&s.manifest_sha256, 64)
    );
    if s.standins {
        let _ = writeln!(out, "* stand-ins used");
    }
    let _ = writeln!(out, "* calls total {}", s.calls_total);
    let _ = writeln!(out, "* calls distinct {}", s.calls_distinct);
    let _ = writeln!(out, "* texts total {}", s.texts_total);
    let _ = writeln!(out, "* texts distinct {}", s.texts_distinct);
    let _ = writeln!(out, "# Calls by bucket");
    for (i, b) in BUCKETS.iter().enumerate() {
        let _ = writeln!(out, "* {b} distinct {}", s.bucket_distinct[i]);
        let _ = writeln!(out, "* {b} total {}", s.bucket_total[i]);
    }
    let _ = writeln!(out, "# Runners");
    for (r, rows) in &s.runners {
        for (i, b) in BUCKETS.iter().enumerate() {
            for (j, o) in OUTCOMES.iter().enumerate() {
                let _ = writeln!(out, "* runner {r} {b} {o} {}", rows[i][j]);
            }
        }
    }
    let _ = writeln!(out, "# Comparisons");
    for kind in KINDS {
        if let Some((n, same)) = s.compared.get(kind) {
            let _ = writeln!(out, "* {kind} compared {n}");
            let _ = writeln!(out, "* {kind} same {same}");
        }
    }
    let _ = writeln!(out, "# Differences");
    let _ = writeln!(out, "* unexplained {}", s.unexplained());
    let _ = writeln!(out, "* explained {}", s.diffs.len() - s.unexplained());
    for class in CLASSES {
        let n = s
            .diffs
            .iter()
            .filter(|d| d.class.as_deref() == Some(class))
            .count();
        let _ = writeln!(out, "* class {class} {n}");
    }
    let _ = writeln!(out, "# Difference ids");
    let mut diffs = s.diffs.clone();
    diffs.sort();
    for d in &diffs {
        let id = hex_of(&d.id, 16);
        let kind = if KINDS.contains(&d.kind.as_str()) {
            d.kind.as_str()
        } else {
            "unknown"
        };
        let class = match d.class.as_deref() {
            Some(c) if CLASSES.contains(&c) => c,
            Some(_) => "unknown",
            None => "unexplained",
        };
        let _ = writeln!(out, "* {id} {kind} {class}");
    }
    out
}

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Whether a report token is fixed vocabulary, a number, a date or a hex id.
fn removable(token: &str) -> bool {
    VOCABULARY.contains(&token)
        || token.bytes().all(|b| b.is_ascii_digit())
        || date_ok(token)
        || ([16, 40, 64].contains(&token.len())
            && token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// The report's unknown tokens and its fragments (module docs).
pub fn scan(report: &str) -> (Vec<String>, Vec<String>) {
    let mut unknown = Vec::new();
    let mut fragments = Vec::new();
    let mut fragment = String::new();
    let mut rest = report;
    while let Some(c) = rest.chars().next() {
        if token_char(c) {
            let end = rest.find(|c: char| !token_char(c)).unwrap_or(rest.len());
            let token = &rest[..end];
            if removable(token) {
                if !fragment.is_empty() {
                    fragments.push(std::mem::take(&mut fragment));
                }
            } else {
                unknown.push(token.to_string());
                fragment.push_str(token);
            }
            rest = &rest[end..];
        } else {
            fragment.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    if !fragment.is_empty() {
        fragments.push(fragment);
    }
    (unknown, fragments)
}

/// What [`check_report`] found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportCheck {
    /// Words outside the vocabulary.
    pub unknown: Vec<String>,
    /// Corpus strings longer than 3 characters found in the report.
    pub hits: Vec<String>,
    /// Leftover fragments longer than 3 characters.
    pub long_fragments: usize,
}

impl ReportCheck {
    pub fn ok(&self) -> bool {
        self.unknown.is_empty() && self.hits.is_empty() && self.long_fragments == 0
    }
}

/// The corpus-string test (module docs) over `corpus` strings.
pub fn check_report(report: &str, corpus: &mut dyn Iterator<Item = String>) -> ReportCheck {
    let (unknown, fragments) = scan(report);
    let long: Vec<&String> = fragments.iter().filter(|f| f.chars().count() > 3).collect();
    let mut hits = Vec::new();
    if !long.is_empty() {
        for s in corpus {
            let words = std::iter::once(s.as_str()).chain(s.split_whitespace());
            for w in words {
                if w.chars().count() > 3
                    && long.iter().any(|f| f.contains(w))
                    && !hits.iter().any(|h| h == w)
                {
                    hits.push(w.to_string());
                }
            }
        }
    }
    ReportCheck {
        unknown,
        hits,
        long_fragments: long.len(),
    }
}

/// Every string of the corpus in `dir` (call fields, input keys and values, texts), plus
/// `extra` (the real `HOME` and its parts, user and host names).
pub fn corpus_strings(dir: &Path, extra: Vec<String>) -> Result<Vec<String>, String> {
    let mut out = extra;
    for call in collect::read_calls(dir)? {
        out.extend([
            call.tool_name.clone(),
            call.session_id.clone(),
            call.transcript_path.clone(),
            call.cwd.clone(),
            call.permission_mode.clone(),
            call.tool_use_id.clone(),
        ]);
        let input = call.input();
        let mut stack = vec![&input];
        while let Some(v) = stack.pop() {
            match v {
                Value::Str(s) => out.push(s.clone()),
                Value::Array(items) => stack.extend(items.iter()),
                Value::Object(m) => {
                    for (k, v) in m.iter() {
                        out.push(k.to_string());
                        stack.push(v);
                    }
                }
                _ => {}
            }
        }
    }
    out.extend(collect::read_texts(dir)?.into_iter().map(|(_, t)| t));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Summary {
        let mut s = Summary {
            date: "2026-11-21".into(),
            python_commit: "a".repeat(40),
            rust_commit: "b".repeat(40),
            manifest_sha256: "c".repeat(64),
            calls_total: 12,
            calls_distinct: 9,
            texts_total: 30,
            texts_distinct: 20,
            bucket_distinct: [6, 2, 1],
            bucket_total: [8, 3, 1],
            standins: true,
            ..Summary::default()
        };
        s.runners.insert(1, [[5, 1, 0], [2, 0, 0], [1, 0, 0]]);
        s.runners.insert(4, [[3, 3, 0], [0, 2, 0], [0, 1, 0]]);
        s.compared.insert("runners".into(), (9, 8));
        s.compared.insert("privacy".into(), (20, 20));
        s.diffs.push(DiffId {
            id: "0123456789abcdef".into(),
            kind: "runners".into(),
            class: None,
        });
        s.diffs.push(DiffId {
            id: "fedcba9876543210".into(),
            kind: "check".into(),
            class: Some("D-7".into()),
        });
        s
    }

    #[test]
    fn the_summary_round_trips() {
        let s = sample();
        assert_eq!(Summary::from_json(&s.to_json()), Some(s));
        assert_eq!(Summary::from_json("{}"), None);
    }

    #[test]
    fn the_report_is_only_vocabulary_numbers_and_ids() {
        let text = render(&sample());
        assert!(
            text.starts_with("# Replay report 2026-11-21\n* python commit aaaa"),
            "{text}"
        );
        assert!(text.contains("\n* runner 4 mcp blocked 2\n"), "{text}");
        assert!(
            text.contains("\n* unexplained 1\n* explained 1\n"),
            "{text}"
        );
        assert!(
            text.contains("\n* 0123456789abcdef runners unexplained\n"),
            "{text}"
        );
        assert!(text.contains("\n* fedcba9876543210 check D-7\n"), "{text}");
        let (unknown, fragments) = scan(&text);
        assert!(unknown.is_empty(), "{unknown:?}");
        assert!(
            fragments.iter().all(|f| f.chars().count() <= 3),
            "{fragments:?}"
        );
        let check = check_report(&text, &mut vec!["anything at all".to_string()].into_iter());
        assert!(check.ok(), "{check:?}");
    }

    #[test]
    fn a_leak_is_caught() {
        let mut text = render(&sample());
        text.push_str("* cat /secret/path\n");
        let corpus = vec!["cat /secret/path".to_string(), "unrelated".to_string()];
        let check = check_report(&text, &mut corpus.into_iter());
        assert_eq!(check.unknown, ["cat", "secret", "path"]);
        assert!(
            check.hits.contains(&"cat /secret/path".to_string()),
            "{check:?}"
        );
        assert!(
            check.hits.contains(&"/secret/path".to_string()),
            "{check:?}"
        );
        // A vocabulary word the corpus also holds is not a hit, and short strings never are.
        let check = check_report(
            &render(&sample()),
            &mut vec!["calls".to_string(), "mcp".to_string()].into_iter(),
        );
        assert!(check.ok());
        // Non-token text alone is caught as a long fragment.
        let leak = format!("{}* ~/.ssh/é\n", render(&sample()));
        let check = check_report(&leak, &mut vec!["/.ssh/é".to_string()].into_iter());
        assert!(check.unknown.contains(&"ssh".to_string()));
        assert_eq!(check.hits, ["/.ssh/é"]);
        // Text with no word in it, and not in the corpus, still fails: a long fragment.
        let odd = format!("{}* ~~~~ é\n", render(&sample()));
        let check = check_report(&odd, &mut std::iter::empty());
        assert!(check.unknown.is_empty() && check.hits.is_empty());
        assert_eq!(check.long_fragments, 1);
        assert!(!check.ok());
    }

    #[test]
    fn bad_values_render_as_unknown() {
        let mut s = sample();
        s.python_commit = "/home/someone".into();
        s.diffs[0].id = "not-an-id".into();
        s.diffs[0].kind = "Bash".into();
        s.diffs[1].class = Some("secret".into());
        s.date = "today".into();
        let text = render(&s);
        assert!(scan(&text).0.is_empty(), "{text}");
        assert!(!text.contains("someone") && !text.contains("Bash") && !text.contains("secret"));
    }

    #[test]
    fn tools_are_bucketed() {
        assert_eq!(bucket("Bash"), 0);
        assert_eq!(bucket("mcp__server__tool"), 1);
        assert_eq!(bucket("SomethingNew"), 2);
        assert_eq!(bucket("mcp_"), 2);
    }
}
