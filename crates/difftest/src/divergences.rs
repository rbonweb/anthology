//! Allowed divergences: `port/difftest/divergences/<surface>.toml` (F4, GC-16, design §13.2).
//!
//! The file is a strict subset of TOML, parsed here by hand:
//!
//! ```text
//! # a comment
//! [[divergence]]
//! class = "D-6"                 # one of D-1 .. D-8 (GC-16); anything else is an error
//! case = "golden:12"            # optional: a case id, or a glob (`*`, `?`)
//! reason = "why this is allowed" # required, not empty
//! ```
//!
//! Values are basic (`"..."`, with `\" \\ \n \t \r \uXXXX \UXXXXXXXX`) or literal (`'...'`)
//! strings on one line. Any other syntax, an unknown or repeated key, a missing `class` or
//! `reason`, or another table is an error, and the run stops (exit 2).
//!
//! How a case is judged (the harness):
//! - an oracle-marked divergence (`"divergence": "D-n"` in the response) passes only if an
//!   entry of class `D-n` matches the case; an entry without `case` matches every case;
//! - a difference between the two sides passes only if an entry **with** a `case` matches it.
//!   An entry without `case` covers oracle marks only, so a surface-wide `D-6` declaration
//!   cannot hide an unrelated diff; write `case = "*"` to waive diffs surface-wide on purpose;
//! - a `D-8` entry ("Rust stricter") waives a diff only where the surface says Rust is the
//!   stricter side (`Surface::rust_stricter`); any other diff on a case a D-8 entry names is
//!   unexplained, whatever other entry matches it;
//! - an entry that matched nothing in the run is reported as `unused` (a warning).
//!
//! A `D-8` entry must have a `case`: it explains named cases only, never a surface.
#![allow(
    dead_code,
    reason = "published for the surface lanes; not every item has a caller yet"
)]

use std::fmt;
use std::path::Path;

/// A GC-16 divergence class.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Class {
    /// Python raised (an exception or `RecursionError`), failing open.
    D1,
    /// Library error text after a fixed prefix.
    D2,
    /// `\b` next to a mark, `No` or `Pc` code point.
    D3,
    /// argparse usage text at exit 2.
    D4,
    /// Byte encodings with equal content (gzip, SQLite).
    D5,
    /// Lone surrogates → U+FFFD (F-19).
    D6,
    /// Resource limits (backtrack limit, guard watchdog).
    D7,
    /// Rust stricter: owner direction (FW-H/FW-I). Case-scoped only.
    D8,
}

impl Class {
    pub const ALL: [Class; 8] = [
        Class::D1,
        Class::D2,
        Class::D3,
        Class::D4,
        Class::D5,
        Class::D6,
        Class::D7,
        Class::D8,
    ];

    /// `"D-1"` .. `"D-8"` exactly.
    pub fn parse(s: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Class::D1 => "D-1",
            Class::D2 => "D-2",
            Class::D3 => "D-3",
            Class::D4 => "D-4",
            Class::D5 => "D-5",
            Class::D6 => "D-6",
            Class::D7 => "D-7",
            Class::D8 => "D-8",
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A key, its value and its line, while an entry is read.
type KeyLine = (String, String, usize);

/// One `[[divergence]]` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub class: Class,
    pub case: Option<String>,
    pub reason: String,
    /// The line of its `[[divergence]]` header.
    pub line: usize,
}

/// A surface's divergence file and which entries a run used.
#[derive(Clone, Debug, Default)]
pub struct Divergences {
    /// The file, for messages.
    pub label: String,
    pub entries: Vec<Entry>,
    used: Vec<usize>,
}

impl Divergences {
    /// `<dir>/<surface>.toml`; no file means no entries.
    pub fn load(dir: &Path, surface: &str) -> Result<Divergences, String> {
        let path = dir.join(format!("{surface}.toml"));
        let label = path.display().to_string();
        match std::fs::read_to_string(&path) {
            Ok(text) => Divergences::parse(&text, &label),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Divergences {
                label,
                ..Divergences::default()
            }),
            Err(e) => Err(format!("{label}: {e}")),
        }
    }

    /// Parses the subset (module docs); `label` names the file in errors.
    pub fn parse(text: &str, label: &str) -> Result<Divergences, String> {
        let mut entries = Vec::new();
        let mut current: Option<(usize, Vec<KeyLine>)> = None;
        let err = |n: usize, msg: &str| format!("{label}:{n}: {msg}");
        for (i, raw) in text.lines().enumerate() {
            let n = i + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') {
                let header = strip_comment(line).map_err(|m| err(n, &m))?;
                if header.trim() != "[[divergence]]" {
                    return Err(err(n, &format!("unknown table {:?}", header.trim())));
                }
                if let Some((at, keys)) = current.take() {
                    entries.push(finish(at, keys, label)?);
                }
                current = Some((n, Vec::new()));
                continue;
            }
            let Some((_, keys)) = current.as_mut() else {
                return Err(err(n, "a key outside a [[divergence]] entry"));
            };
            let (key, rest) = line
                .split_once('=')
                .ok_or_else(|| err(n, "expected key = \"value\""))?;
            let key = key.trim();
            if !["class", "case", "reason"].contains(&key) {
                return Err(err(
                    n,
                    &format!("unknown key {key:?} (class, case, reason)"),
                ));
            }
            if keys.iter().any(|(k, _, _)| k == key) {
                return Err(err(n, &format!("{key:?} is given twice")));
            }
            let value = string_value(rest.trim()).map_err(|m| err(n, &m))?;
            keys.push((key.to_string(), value, n));
        }
        if let Some((at, keys)) = current.take() {
            entries.push(finish(at, keys, label)?);
        }
        let used = vec![0; entries.len()];
        Ok(Divergences {
            label: label.to_string(),
            entries,
            used,
        })
    }

    /// The first entry of `class` matching `case_id` (any entry without `case` matches);
    /// counted as used.
    pub fn explain_mark(&mut self, class: Class, case_id: &str) -> Option<&Entry> {
        let i = self
            .entries
            .iter()
            .position(|e| e.class == class && e.case.as_deref().is_none_or(|p| glob(p, case_id)))?;
        self.used[i] += 1;
        Some(&self.entries[i])
    }

    /// The first entry with a `case` matching `case_id`; counted as used. A `D-8` entry is
    /// skipped unless `stricter` (Rust is the stricter side of the diff).
    pub fn explain_diff(&mut self, case_id: &str, stricter: bool) -> Option<&Entry> {
        let i = self.entries.iter().position(|e| {
            (stricter || e.class != Class::D8)
                && e.case.as_deref().is_some_and(|p| glob(p, case_id))
        })?;
        self.used[i] += 1;
        Some(&self.entries[i])
    }

    /// The first `D-8` entry naming `case_id` (not counted as used).
    pub fn d8_entry(&self, case_id: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.class == Class::D8 && e.case.as_deref().is_some_and(|p| glob(p, case_id)))
    }

    /// Entries no case used.
    pub fn unused(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .zip(&self.used)
            .filter(|(_, n)| **n == 0)
            .map(|(e, _)| e)
            .collect()
    }
}

fn finish(at: usize, keys: Vec<KeyLine>, label: &str) -> Result<Entry, String> {
    let get = |k: &str| keys.iter().find(|(key, _, _)| key == k);
    let class = match get("class") {
        Some((_, v, n)) => Class::parse(v)
            .ok_or_else(|| format!("{label}:{n}: class {v:?} is not a GC-16 class (D-1 .. D-8)"))?,
        None => return Err(format!("{label}:{at}: the entry has no class")),
    };
    let reason = match get("reason") {
        Some((_, v, _)) if !v.trim().is_empty() => v.clone(),
        Some((_, _, n)) => return Err(format!("{label}:{n}: the reason is empty")),
        None => return Err(format!("{label}:{at}: the entry has no reason")),
    };
    let case = match get("case") {
        Some((_, v, n)) if v.is_empty() => {
            return Err(format!(
                "{label}:{n}: case is empty (leave it out to match any case)"
            ));
        }
        Some((_, v, _)) => Some(v.clone()),
        None if class == Class::D8 => {
            return Err(format!(
                "{label}:{at}: a D-8 entry needs a case (it is never surface-wide)"
            ));
        }
        None => None,
    };
    Ok(Entry {
        class,
        case,
        reason,
        line: at,
    })
}

/// `line` without a trailing `# comment` (a `#` inside a string is kept).
fn strip_comment(line: &str) -> Result<&str, String> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if c == '\\' => escaped = true,
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' => return Ok(&line[..i]),
            None => {}
        }
    }
    if quote.is_some() {
        return Err("an unterminated string".into());
    }
    Ok(line)
}

/// A one-line TOML basic or literal string, then only an optional comment.
fn string_value(s: &str) -> Result<String, String> {
    let mut chars = s.char_indices();
    let open = match chars.next() {
        Some((_, c @ ('"' | '\''))) => c,
        _ => return Err("a value must be a \"string\"".into()),
    };
    let mut out = String::new();
    let mut end = None;
    while let Some((i, c)) = chars.next() {
        if c == open {
            end = Some(i + 1);
            break;
        }
        if c == '\\' && open == '"' {
            let (_, e) = chars.next().ok_or("an unterminated string")?;
            match e {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' | 'U' => {
                    let len = if e == 'u' { 4 } else { 8 };
                    let hex: String = (0..len)
                        .filter_map(|_| chars.next().map(|(_, c)| c))
                        .collect();
                    let code = u32::from_str_radix(&hex, 16)
                        .ok()
                        .filter(|_| hex.len() == len)
                        .and_then(char::from_u32)
                        .ok_or_else(|| format!("a bad \\{e} escape"))?;
                    out.push(code);
                }
                other => return Err(format!("an unknown escape \\{other}")),
            }
            continue;
        }
        if c.is_control() && c != '\t' {
            return Err("a control character in a string".into());
        }
        out.push(c);
    }
    let end = end.ok_or("an unterminated string")?;
    let rest = s[end..].trim_start();
    if !(rest.is_empty() || rest.starts_with('#')) {
        return Err(format!("unexpected text after the value: {rest:?}"));
    }
    Ok(out)
}

/// Whether `text` matches `pattern`: `*` is any run of characters, `?` one character.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
# lone surrogates
[[divergence]]
class = "D-6"   # a comment
reason = "F-19: U+FFFD # not a comment"

[[divergence]]
class = 'D-1'
case = "golden:1?"
reason = "deep \"text\" \u00e9"
"#;

    #[test]
    fn parses_the_subset() {
        let d = Divergences::parse(GOOD, "x.toml").expect("valid");
        assert_eq!(
            d.entries,
            [
                Entry {
                    class: Class::D6,
                    case: None,
                    reason: "F-19: U+FFFD # not a comment".into(),
                    line: 3
                },
                Entry {
                    class: Class::D1,
                    case: Some("golden:1?".into()),
                    reason: "deep \"text\" é".into(),
                    line: 7
                }
            ]
        );
        assert!(
            Divergences::parse("", "e")
                .expect("empty")
                .entries
                .is_empty()
        );
    }

    #[test]
    fn everything_else_is_an_error() {
        for (text, want) in [
            ("class = \"D-6\"", "x:1: a key outside"),
            (
                "[[divergence]]\nclass = \"D-9\"\nreason = \"r\"",
                "x:2: class \"D-9\" is not a GC-16",
            ),
            (
                "[[divergence]]\nclass = \"d-6\"\nreason = \"r\"",
                "not a GC-16",
            ),
            (
                "[[divergence]]\nclass = \"D-6\"\nreason = \"r\"\nnote = \"n\"",
                "x:4: unknown key \"note\"",
            ),
            (
                "[[divergence]]\nclass = \"D-6\"\nclass = \"D-6\"\nreason = \"r\"",
                "given twice",
            ),
            (
                "[[divergence]]\nreason = \"r\"",
                "x:1: the entry has no class",
            ),
            (
                "[[divergence]]\nclass = \"D-6\"",
                "x:1: the entry has no reason",
            ),
            (
                "[[divergence]]\nclass = \"D-6\"\nreason = \"  \"",
                "the reason is empty",
            ),
            (
                "[[divergence]]\nclass = \"D-6\"\ncase = \"\"\nreason = \"r\"",
                "case is empty",
            ),
            ("[divergence]", "unknown table"),
            ("[[other]]", "unknown table"),
            ("[[divergence]]\nclass = D-6", "must be a \"string\""),
            ("[[divergence]]\nclass = \"D-6", "unterminated"),
            ("[[divergence]]\nclass = \"D-6\" x", "unexpected text"),
            ("[[divergence]]\nclass \"D-6\"", "expected key"),
            ("[[divergence]]\nreason = \"\\q\"", "unknown escape"),
            ("[[divergence]]\nreason = \"\\u12\"", "bad \\u escape"),
            ("[[divergence]]\nreason = '''r'''", "unexpected text"),
        ] {
            let e = Divergences::parse(text, "x").expect_err(text);
            assert!(e.contains(want), "{text:?}: {e}");
        }
    }

    #[test]
    fn marks_need_their_class_and_diffs_need_a_case() {
        let mut d = Divergences::parse(GOOD, "x").expect("valid");
        assert_eq!(d.explain_mark(Class::D6, "gen:5").map(|e| e.line), Some(3));
        assert!(d.explain_mark(Class::D2, "gen:5").is_none());
        // The D-1 entry needs its case; the D-6 one has none, so it explains no diff.
        assert!(d.explain_mark(Class::D1, "golden:2").is_none());
        assert!(d.explain_diff("gen:5", true).is_none());
        assert_eq!(d.unused().len(), 1);
        assert_eq!(
            d.explain_diff("golden:12", false).map(|e| e.class),
            Some(Class::D1)
        );
        assert!(d.unused().is_empty());
    }

    #[test]
    fn a_missing_file_has_no_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = Divergences::load(dir.path(), "core.lock").expect("ok");
        assert!(d.entries.is_empty());
        assert!(d.label.ends_with("core.lock.toml"));
        std::fs::write(dir.path().join("core.lock.toml"), "junk").expect("write");
        assert!(Divergences::load(dir.path(), "core.lock").is_err());
    }

    #[test]
    fn glob_matches_stars_and_marks() {
        for (p, t, want) in [
            ("*", "", true),
            ("*", "golden:1", true),
            ("golden:*", "golden:17", true),
            ("golden:*", "gen:17", false),
            ("gen:?", "gen:7", true),
            ("gen:?", "gen:17", false),
            ("*:1*", "golden:17", true),
            ("a*b*c", "axxbyyc", true),
            ("a*b*c", "axxbyy", false),
            ("exact", "exact", true),
            ("exact", "exactly", false),
        ] {
            assert_eq!(glob(p, t), want, "{p} {t}");
        }
        assert_eq!(Class::parse("D-4"), Some(Class::D4));
        assert_eq!(Class::D7.to_string(), "D-7");
        assert_eq!(Class::parse("D-8"), Some(Class::D8));
        assert_eq!(Class::D8.to_string(), "D-8");
        assert_eq!(Class::parse("D-9"), None);
    }

    #[test]
    fn d8_is_case_scoped_only() {
        let e = Divergences::parse("[[divergence]]\nclass = \"D-8\"\nreason = \"r\"", "x")
            .expect_err("D-8 without a case");
        assert!(e.contains("needs a case"), "{e}");
        let mut d = Divergences::parse(
            "[[divergence]]\nclass = \"D-8\"\ncase = \"fw:1\"\nreason = \"r\"",
            "x",
        )
        .expect("valid");
        assert_eq!(
            d.explain_diff("fw:1", true).map(|e| e.class),
            Some(Class::D8)
        );
        assert!(d.explain_diff("fw:2", true).is_none());
        // a diff where Rust is not stricter: the D-8 entry does not waive it
        assert!(d.explain_diff("fw:1", false).is_none());
        assert_eq!(d.d8_entry("fw:1").map(|e| e.line), Some(1));
        assert!(d.d8_entry("fw:2").is_none());
    }
}
