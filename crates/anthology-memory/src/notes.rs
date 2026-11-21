//! Note parsing (MS1): a port of `memory_index.parse_text` / `parse_note` (the small frontmatter
//! parser `memory_sync`, the indexer and the portable archive share) and of `memory_sync`'s
//! `note_info`, `normalize_type` and `is_user_note` (plan §3.6, GC-12).
//!
//! The parser reads the YAML subset the notes use (nested maps by indentation, `- item` lists,
//! `[a, b]` flow lists, `>`/`|` block scalars, single- and double-quoted scalars), exactly as
//! the Python does, and never fails: broken frontmatter gives a note listed by its stem.
//!
//! **Divergence (D-1).** Python's `_parse_block` recurses once per nesting level, so a
//! frontmatter nested about a thousand levels deep raises `RecursionError`, which `parse_text`
//! catches, resetting the note to its stem (type `other`). Rust parses any depth (an explicit
//! stack, no recursion), so such a note keeps its fields. That only ever makes Rust stricter:
//! a deep `type: user` note is still recognised as a user note.
//!
//! [`NoteInfo`] field mapping: `kind` is Python's `type` and is always `Some` from the parser
//! (`"other"` when the note names none; consumers that build a `NoteInfo` by hand may use
//! `None`, which every function here reads as `"other"`); `updated` is `None` where Python's is
//! `""`. `relations` keeps the three kinds the indexer reads (`RELATION_KINDS`).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};
use std::sync::OnceLock;

use anthology_pycompat::re::{self, Flags, Regex};
use anthology_pycompat::str::{char_len, char_slice, lower, strip_ws};

/// `memory_sync.USER_REASON`: why a `type: user` note never goes into a repo (GC-12).
pub const USER_REASON: &str = "type: user notes are personal; they never go into a repo";

/// `memory_sync.REPO_EXCLUDE`: the note types a repo or agent index never lists.
pub const REPO_EXCLUDE: &[&str] = &["user"];

/// `memory_index.RELATION_KINDS`.
pub const RELATION_KINDS: [&str; 3] = ["supersedes", "depends_on", "relates_to"];

/// The `metadata.relations` targets of a note, by kind (`info["relations"].get(kind, [])`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Relations {
    pub supersedes: Vec<String>,
    pub depends_on: Vec<String>,
    pub relates_to: Vec<String>,
}

impl Relations {
    /// The targets of `kind` (one of [`RELATION_KINDS`]; any other kind has none).
    pub fn get(&self, kind: &str) -> &[String] {
        match kind {
            "supersedes" => &self.supersedes,
            "depends_on" => &self.depends_on,
            "relates_to" => &self.relates_to,
            _ => &[],
        }
    }
}

/// A parsed note (`memory_index.parse_text`'s dict; see the module docs for the mapping).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoteInfo {
    pub name: String,
    pub description: String,
    /// Python's `type`: `Some("other")` when the note names none.
    pub kind: Option<String>,
    /// Python's `updated`: `None` where Python has `""`.
    pub updated: Option<String>,
    pub relations: Relations,
    pub body: String,
}

impl NoteInfo {
    /// Python's `info["type"]`: the type, `"other"` when there is none.
    pub fn type_str(&self) -> &str {
        match self.kind.as_deref() {
            Some(t) if !t.is_empty() => t,
            _ => "other",
        }
    }

    /// Python's `info["updated"]`: the date text, `""` when there is none.
    pub fn updated_str(&self) -> &str {
        self.updated.as_deref().unwrap_or("")
    }
}

/// `memory_index.parse_text(text, stem)`: never fails; broken frontmatter yields a note listed
/// by `stem`.
pub fn parse_text(text: &str, stem: &str) -> NoteInfo {
    parse_text_ok(text, stem).0
}

/// [`parse_text`] and Python's `info["ok"]`: whether the frontmatter was read.
pub fn parse_text_ok(text: &str, stem: &str) -> (NoteInfo, bool) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.replace("\r\n", "\n");
    let mut info = NoteInfo {
        name: stem.to_string(),
        description: String::new(),
        kind: Some("other".to_string()),
        updated: None,
        relations: Relations::default(),
        body: text.clone(),
    };
    let Some(rest) = text.strip_prefix("---\n") else {
        return (info, false);
    };
    let Some((start, end)) = front_end(rest) else {
        return (info, false);
    };
    info.body = rest[end..].trim_start_matches('\n').to_string();
    let data = parse_yaml(&rest[..start]);
    let empty = YMap::default();
    let meta = match data.get("metadata") {
        Some(Y::Map(m)) => m,
        _ => &empty,
    };
    let name = text_of(data.get("name"));
    info.name = if name.is_empty() {
        stem.to_string()
    } else {
        name
    };
    info.description = text_of(data.get("description"));
    let kind = first_text(&[meta.get("type"), data.get("type")]);
    info.kind = Some(if kind.is_empty() {
        "other".to_string()
    } else {
        kind
    });
    let updated = first_text(&[
        meta.get("modified"),
        meta.get("updated"),
        data.get("modified"),
        data.get("updated"),
    ]);
    info.updated = (!updated.is_empty()).then_some(updated);
    if let Some(Y::Map(rel)) = meta.get("relations") {
        // `{_text(k): _targets(v) for k, v in relations.items() if _targets(v)}`: per kind, the
        // last key that strips to it and has targets wins.
        for (k, v) in &rel.0 {
            let targets = targets_of(v);
            if targets.is_empty() {
                continue;
            }
            match strip_ws(k) {
                "supersedes" => info.relations.supersedes = targets,
                "depends_on" => info.relations.depends_on = targets,
                "relates_to" => info.relations.relates_to = targets,
                _ => {}
            }
        }
    }
    (info, true)
}

/// `memory_index.parse_note(path)`: the note at `path`, parsed with its stem (`Path.stem`);
/// `None` when the file cannot be read (Python raises `OSError`). Follows symlinks, as
/// `read_bytes` does.
pub fn parse_note(path: &Path) -> Option<NoteInfo> {
    let data = std::fs::read(path).ok()?;
    Some(parse_text(&decode_replace(&data), &path_stem(path)))
}

/// `memory_sync.note_info(name, data)`: `parse_text(data.decode("utf-8", errors="replace"),
/// name[:-3])`.
pub fn note_info(name: &str, data: &[u8]) -> NoteInfo {
    let stem = char_slice(name, 0, char_len(name).saturating_sub(3));
    parse_text(&decode_replace(data), stem)
}

/// `memory_sync.is_user_note(info)`: a `type: user` note, in any case, with or without a
/// trailing `# comment`.
pub fn is_user_note(info: &NoteInfo) -> bool {
    normalize_type(info.kind.as_deref()) == "user"
}

/// `memory_sync.normalize_type(value)`: lower case, a trailing `# comment` and the surrounding
/// whitespace dropped (`str(value or "").split("#", 1)[0].strip().lower()`).
pub fn normalize_type(value: Option<&str>) -> String {
    let v = value.unwrap_or("");
    let head = v.split_once('#').map_or(v, |(h, _)| h);
    lower(strip_ws(head))
}

/// `bytes.decode("utf-8", errors="replace")`: CPython replaces each maximal invalid subpart
/// with one U+FFFD, as Rust's lossy decoding does.
pub fn decode_replace(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

/// `PurePath.stem` (CPython 3.12): the last component without its last suffix; a name whose
/// only dot is the first or the last character keeps it. A name that is not UTF-8 is decoded
/// lossily (F-19).
pub fn path_stem(path: &Path) -> String {
    let name = match path.components().next_back() {
        Some(Component::Normal(n)) => n,
        Some(Component::ParentDir) => OsStr::new(".."),
        _ => OsStr::new(""),
    };
    let b = name.as_bytes();
    let stem = match b.iter().rposition(|&c| c == b'.') {
        Some(i) if i > 0 && i < b.len() - 1 => &b[..i],
        _ => b,
    };
    String::from_utf8_lossy(stem).into_owned()
}

/// Python's sort key for a file name (`str` order of the surrogate-escaped name): code points,
/// with each byte that is not UTF-8 as `U+DC80..U+DCFF`. For a UTF-8 name it is byte order.
pub(crate) fn py_name_key(name: &OsStr) -> Vec<u32> {
    let mut out = Vec::with_capacity(name.len());
    for chunk in name.as_bytes().utf8_chunks() {
        out.extend(chunk.valid().chars().map(u32::from));
        out.extend(chunk.invalid().iter().map(|&b| 0xDC00 + u32::from(b)));
    }
    out
}

// ------------------------------------------------------------------------------------------
// The frontmatter end and the YAML subset
// ------------------------------------------------------------------------------------------

/// `_FRONT_END.search(rest)` (`^---[ \t]*$`, MULTILINE): the span of the closing line.
fn front_end(rest: &str) -> Option<(usize, usize)> {
    static RX: OnceLock<Regex> = OnceLock::new();
    let rx = RX.get_or_init(|| {
        re::compile_with(
            r"^---[ \t]*$",
            Flags {
                multiline: true,
                ..Flags::default()
            },
        )
        .expect("_FRONT_END compiles")
    });
    // A plain (non-fancy) pattern cannot hit a backtrack limit.
    rx.find_at(rest, 0).ok().flatten()
}

/// A parsed YAML value: a scalar, a list of scalars, or a map.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Y {
    Str(String),
    List(Vec<String>),
    Map(YMap),
}

/// A Python dict: insertion order, a repeated key replaces the value in its first position.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct YMap(Vec<(String, Y)>);

impl YMap {
    fn get(&self, k: &str) -> Option<&Y> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v)
    }

    fn insert(&mut self, k: String, v: Y) {
        match self.0.iter_mut().find(|(key, _)| *key == k) {
            Some(slot) => slot.1 = v,
            None => self.0.push((k, v)),
        }
    }
}

/// `_text(value)`: a stripped string, or `""` for anything else.
fn text_of(v: Option<&Y>) -> String {
    match v {
        Some(Y::Str(s)) => strip_ws(s).to_string(),
        _ => String::new(),
    }
}

/// `_text(a) or _text(b) or ...`.
fn first_text(vals: &[Option<&Y>]) -> String {
    vals.iter()
        .map(|v| text_of(*v))
        .find(|t| !t.is_empty())
        .unwrap_or_default()
}

/// `_targets(value)`: the non-empty stripped strings of a string or a list.
fn targets_of(v: &Y) -> Vec<String> {
    let items: Vec<&str> = match v {
        Y::Str(s) => vec![s.as_str()],
        Y::List(l) => l.iter().map(String::as_str).collect(),
        Y::Map(_) => return Vec::new(),
    };
    items
        .into_iter()
        .map(strip_ws)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// `_scalar(value)`: stripped; one pair of matching quotes removed (`''` in single quotes is
/// `'`; `\n`, `\"` and `\\` in double quotes are unescaped, in that order).
fn scalar(value: &str) -> String {
    let v = strip_ws(value);
    let quoted = char_len(v) >= 2 && {
        let first = v.as_bytes()[0];
        (first == b'\'' || first == b'"') && v.as_bytes()[v.len() - 1] == first
    };
    if !quoted {
        return v.to_string();
    }
    let inner = &v[1..v.len() - 1];
    if v.starts_with('\'') {
        inner.replace("''", "'")
    } else {
        inner
            .replace("\\n", "\n")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    }
}

/// `_flow_list(value)`: `[a, "b"]` is a list; without the closing `]` the text stays a string.
fn flow_list(value: &str) -> Y {
    let v = strip_ws(value);
    if !v.ends_with(']') {
        return Y::Str(v.to_string());
    }
    let inner = char_slice(v, 1, char_len(v).saturating_sub(1));
    Y::List(
        inner
            .split(',')
            .filter(|p| !strip_ws(p).is_empty())
            .map(scalar)
            .collect(),
    )
}

/// `_is_item(text)`.
fn is_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// `_parse_yaml(head)`: the top-level map, or an empty one.
fn parse_yaml(head: &str) -> YMap {
    let lines: Vec<(usize, &str)> = head
        .split('\n')
        .filter_map(|raw| {
            let stripped = strip_ws(raw);
            if stripped.is_empty() || stripped.starts_with('#') {
                return None;
            }
            Some((raw.len() - raw.trim_start_matches(' ').len(), stripped))
        })
        .collect();
    if lines.is_empty() {
        return YMap::default();
    }
    match parse_block(&lines, 0, lines[0].0) {
        Y::Map(m) => m,
        _ => YMap::default(),
    }
}

/// The `- item` lines at `indent` from `i`: `_parse_block`'s list case.
fn parse_items(lines: &[(usize, &str)], mut i: usize, indent: usize) -> (Y, usize) {
    let mut items = Vec::new();
    while i < lines.len() && lines[i].0 == indent && is_item(lines[i].1) {
        items.push(scalar(&lines[i].1[1..]));
        i += 1;
    }
    (Y::List(items), i)
}

/// A map being read: its entries, its indent, and the key waiting for a nested map.
struct Frame {
    out: YMap,
    indent: usize,
    pending: Option<String>,
}

/// `_parse_block(lines, start, indent)`'s value, without recursion: a nested map is a new
/// frame on an explicit stack, finished into its parent's pending key.
fn parse_block(lines: &[(usize, &str)], start: usize, indent: usize) -> Y {
    if is_item(lines[start].1) {
        return parse_items(lines, start, indent).0;
    }
    let mut i = start;
    let mut stack = vec![Frame {
        out: YMap::default(),
        indent,
        pending: None,
    }];
    loop {
        let top = stack.last_mut().expect("a frame");
        let mut nested = None;
        while i < lines.len() {
            let (ind, text) = lines[i];
            if ind < top.indent {
                break;
            }
            if ind > top.indent || is_item(text) {
                i += 1;
                continue;
            }
            i += 1;
            let Some((key, val)) = text.split_once(':') else {
                continue;
            };
            let key = scalar(key);
            let val = strip_ws(val);
            if val.is_empty() {
                match lines.get(i) {
                    Some(&(n_ind, n_text))
                        if n_ind > top.indent || (n_ind == top.indent && is_item(n_text)) =>
                    {
                        if is_item(n_text) {
                            let (items, next) = parse_items(lines, i, n_ind);
                            top.out.insert(key, items);
                            i = next;
                        } else {
                            top.pending = Some(key);
                            nested = Some(n_ind);
                            break;
                        }
                    }
                    _ => top.out.insert(key, Y::Str(String::new())),
                }
            } else if is_block_scalar(val) {
                let mut parts = Vec::new();
                while i < lines.len() && lines[i].0 > top.indent {
                    parts.push(lines[i].1);
                    i += 1;
                }
                top.out.insert(key, Y::Str(parts.join(" ")));
            } else if val.starts_with('[') {
                top.out.insert(key, flow_list(val));
            } else {
                top.out.insert(key, Y::Str(scalar(val)));
            }
        }
        if let Some(indent) = nested {
            stack.push(Frame {
                out: YMap::default(),
                indent,
                pending: None,
            });
            continue;
        }
        let done = stack.pop().expect("a frame");
        match stack.last_mut() {
            None => return Y::Map(done.out),
            Some(parent) => {
                let key = parent.pending.take().expect("a pending key");
                parent.out.insert(key, Y::Map(done.out));
            }
        }
    }
}

/// `val[0] in ">|" and not val[1:].strip("+-0123456789")`.
fn is_block_scalar(val: &str) -> bool {
    let mut chars = val.chars();
    matches!(chars.next(), Some('>' | '|'))
        && chars.all(|c| c == '+' || c == '-' || c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_follow_purepath() {
        for (p, want) in [
            ("a/b.md", "b"),
            ("b.tar.gz", "b.tar"),
            (".md", ".md"),
            ("a.", "a."),
            ("a", "a"),
            ("/", ""),
            ("x/..", ".."),
            ("x/./", "x"),
        ] {
            assert_eq!(path_stem(Path::new(p)), want, "{p}");
        }
    }

    #[test]
    fn note_info_drops_three_code_points() {
        assert_eq!(note_info("ab", b"x").name, "");
        assert_eq!(note_info("é.md", b"x").name, "é");
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let mut text = String::from("---\nmetadata:\n");
        for d in 1..3_000 {
            text.push_str(&" ".repeat(d));
            text.push_str("k:\n");
        }
        text.push_str("---\nbody\n");
        let info = parse_text(&text, "deep");
        assert_eq!(info.name, "deep");
        assert_eq!(info.body, "body\n");
    }

    #[test]
    fn name_key_orders_like_python() {
        let a = py_name_key(OsStr::from_bytes(b"\xff"));
        let b = py_name_key(OsStr::new("\u{e000}"));
        assert!(a < b, "U+DCFF sorts before U+E000");
    }
}
