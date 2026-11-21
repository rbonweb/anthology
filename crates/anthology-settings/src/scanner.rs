//! The settings.json scanner (ST1): `settings_patch.py`'s position-tracking JSON scanner
//! (`_Scanner`, `_parse`, `_is_blank`, `_member`, `_descend`, `_strings`).
//!
//! Python indexes code points; this scanner records **byte** offsets. Every delimiter it looks
//! at (`{ } [ ] , : "` and `" \t\r\n"`) is ASCII, so each offset falls on the same character
//! boundary Python's index does, and slicing gives the same text.
//!
//! The document is an arena ([`Doc`]): node `0` is the root, and containers hold the indices of
//! their items. The scanner recurses at most `_MAX_DEPTH + 1` levels, because a deeper document
//! is refused before it descends further.

pub(crate) mod py;
pub(crate) mod pyval;

use anthology_pycompat::json::{self, Value};

use self::pyval::{py_eq, type_error};

/// `_WS`: the whitespace `json` and the scanner skip.
pub(crate) const WS: [char; 4] = [' ', '\t', '\r', '\n'];
/// `_MAX_DEPTH`.
pub(crate) const MAX_DEPTH: usize = 100;
/// The scanner's own "not valid JSON" (no detail).
pub(crate) const NOT_VALID: &str = "settings.json is not valid JSON";

/// `_Node.kind`.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) enum Kind {
    Object,
    Array,
    Str,
    Scalar,
}

impl Kind {
    pub(crate) fn is_container(self) -> bool {
        matches!(self, Kind::Object | Kind::Array)
    }
}

/// `_Member`: a key (decoded), the byte span of its quoted text, and its value node.
#[derive(Clone, Debug)]
pub(crate) struct Member {
    pub(crate) key: String,
    pub(crate) kstart: usize,
    pub(crate) kend: usize,
    pub(crate) value: usize,
}

/// `_Node`: `items` for an array, `members` for an object (the other is empty).
#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) kind: Kind,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) items: Vec<usize>,
    pub(crate) members: Vec<Member>,
}

impl Node {
    /// `len(node.items)`.
    pub(crate) fn len(&self) -> usize {
        match self.kind {
            Kind::Object => self.members.len(),
            _ => self.items.len(),
        }
    }

    /// `not node.items`.
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A scanned document. Node `0` is the root.
#[derive(Clone, Debug)]
pub(crate) struct Doc {
    pub(crate) nodes: Vec<Node>,
}

impl Doc {
    pub(crate) const ROOT: usize = 0;

    pub(crate) fn node(&self, i: usize) -> &Node {
        &self.nodes[i]
    }

    /// `_item_start(node, idx)`.
    pub(crate) fn item_start(&self, node: usize, idx: usize) -> usize {
        let n = self.node(node);
        match n.kind {
            Kind::Object => n.members[idx].kstart,
            _ => self.node(n.items[idx]).start,
        }
    }

    /// `_item_end(node, idx)`.
    pub(crate) fn item_end(&self, node: usize, idx: usize) -> usize {
        let n = self.node(node);
        match n.kind {
            Kind::Object => self.node(n.members[idx].value).end,
            _ => self.node(n.items[idx]).end,
        }
    }

    /// `[m.value for m in parent.items].index(node)`: the member holding `node` (by identity).
    pub(crate) fn member_index(&self, parent: usize, node: usize) -> usize {
        self.node(parent)
            .members
            .iter()
            .position(|m| m.value == node)
            .expect("a chain node is a member of its parent")
    }
}

/// `_is_blank(text)`: `text.strip("\ufeff" + _WS) == ""`.
pub(crate) fn is_blank(text: &str) -> bool {
    text.trim_matches(|c| c == '\u{feff}' || WS.contains(&c))
        .is_empty()
}

/// `text[1:] if text.startswith("\ufeff") else text`.
pub(crate) fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

struct Scanner<'a> {
    t: &'a str,
    b: &'a [u8],
    i: usize,
    nodes: Vec<Node>,
}

impl<'a> Scanner<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    /// `peek()`: the byte at `i`, or `None` at the end (Python's `""`).
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn push(&mut self, kind: Kind, start: usize) -> usize {
        self.nodes.push(Node {
            kind,
            start,
            end: start,
            items: Vec::new(),
            members: Vec::new(),
        });
        self.nodes.len() - 1
    }

    fn value(&mut self, depth: usize) -> Result<usize, String> {
        if depth > MAX_DEPTH {
            return Err("settings.json nests too deeply".into());
        }
        self.ws();
        match self.peek() {
            Some(b'{') => self.obj(depth),
            Some(b'[') => self.arr(depth),
            Some(b'"') => {
                let end = match_string(self.b, self.i).ok_or(NOT_VALID)?;
                let n = self.push(Kind::Str, self.i);
                self.nodes[n].end = end;
                self.i = end;
                Ok(n)
            }
            _ => {
                let end = match_scalar(self.b, self.i).ok_or(NOT_VALID)?;
                let n = self.push(Kind::Scalar, self.i);
                self.nodes[n].end = end;
                self.i = end;
                Ok(n)
            }
        }
    }

    fn obj(&mut self, depth: usize) -> Result<usize, String> {
        let n = self.push(Kind::Object, self.i);
        self.i += 1;
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            self.nodes[n].end = self.i;
            return Ok(n);
        }
        loop {
            self.ws();
            let kend = if self.peek() == Some(b'"') {
                match_string(self.b, self.i)
            } else {
                None
            }
            .ok_or(NOT_VALID)?;
            let kstart = self.i;
            let key = decode_string(&self.t[kstart..kend])?;
            self.i = kend;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(NOT_VALID.into());
            }
            self.i += 1;
            let value = self.value(depth + 1)?;
            self.nodes[n].members.push(Member {
                key,
                kstart,
                kend,
                value,
            });
            self.ws();
            let c = self.peek();
            self.i += 1;
            if c == Some(b'}') {
                self.nodes[n].end = self.i;
                return Ok(n);
            }
            if c != Some(b',') {
                return Err(NOT_VALID.into());
            }
        }
    }

    fn arr(&mut self, depth: usize) -> Result<usize, String> {
        let n = self.push(Kind::Array, self.i);
        self.i += 1;
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            self.nodes[n].end = self.i;
            return Ok(n);
        }
        loop {
            let item = self.value(depth + 1)?;
            self.nodes[n].items.push(item);
            self.ws();
            let c = self.peek();
            self.i += 1;
            if c == Some(b']') {
                self.nodes[n].end = self.i;
                return Ok(n);
            }
            if c != Some(b',') {
                return Err(NOT_VALID.into());
            }
        }
    }
}

/// `_STRING.match(t, i)` (`"(?:[^"\\\x00-\x1f]|\\.)*"`, DOTALL): the end of the match.
fn match_string(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    loop {
        match *b.get(j)? {
            b'"' => return Some(j + 1),
            b'\\' => {
                // `\\.` with DOTALL: any one character (a UTF-8 sequence) after the backslash.
                let next = *b.get(j + 1)?;
                j += 1 + utf8_len(next);
            }
            c if c < 0x20 => return None,
            c => j += utf8_len(c),
        }
    }
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// `_SCALAR.match(t, i)`: `-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?|true|false|null`.
///
fn match_scalar(b: &[u8], i: usize) -> Option<usize> {
    if let Some(end) = match_number(b, i) {
        return Some(end);
    }
    for word in [&b"true"[..], b"false", b"null"] {
        if b[i.min(b.len())..].starts_with(word) {
            return Some(i + word.len());
        }
    }
    None
}

fn match_number(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    if b.get(j) == Some(&b'-') {
        j += 1;
    }
    match b.get(j) {
        Some(b'0') => j += 1,
        Some(b'1'..=b'9') => {
            j += 1;
            j = digits(b, j);
        }
        _ => return None,
    }
    if b.get(j) == Some(&b'.') {
        let k = digits(b, j + 1);
        if k > j + 1 {
            j = k;
        }
    }
    if matches!(b.get(j), Some(b'e' | b'E')) {
        let mut k = j + 1;
        if matches!(b.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        let m = digits(b, k);
        if m > k {
            j = m;
        }
    }
    Some(j)
}

/// The end of a run of `\d` starting at `j`. ASCII digits are enough: the scanner runs only
/// on text `json.loads` accepted, and there a number is ASCII and is followed by a delimiter,
/// whitespace or the end, never by another (Unicode) digit.
fn digits(b: &[u8], mut j: usize) -> usize {
    while b.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    j
}

/// `json.loads(<quoted text>)` for a key or string node the scanner matched.
pub(crate) fn decode_string(quoted: &str) -> Result<String, String> {
    match json::loads(quoted) {
        Ok(v) => Ok(v.into_string().unwrap_or_default()),
        Err(e) => Err(format!("{}: {}", e.type_name(), e.py_str(quoted))),
    }
}

/// `json.loads(text)` of a node's raw text (always valid: the document parsed).
pub(crate) fn load_raw(raw: &str) -> Result<Value, String> {
    json::loads(raw).map_err(|e| format!("{}: {}", e.type_name(), e.py_str(raw)))
}

/// `_parse(text)`: the scanned document, whose root is one JSON object.
pub(crate) fn parse(text: &str) -> Result<Doc, String> {
    let body = strip_bom(text);
    if let Err(e) = json::loads(body) {
        return Err(format!(
            "settings.json is not valid JSON: {}",
            e.py_str(body)
        ));
    }
    let mut sc = Scanner {
        t: text,
        b: text.as_bytes(),
        i: if text.starts_with('\u{feff}') { 3 } else { 0 },
        nodes: Vec::new(),
    };
    let root = sc.value(0)?;
    debug_assert_eq!(root, Doc::ROOT);
    sc.ws();
    if sc.i != text.len() {
        return Err(NOT_VALID.into());
    }
    let doc = Doc { nodes: sc.nodes };
    if doc.node(Doc::ROOT).kind != Kind::Object {
        return Err("settings.json is not a JSON object".into());
    }
    Ok(doc)
}

/// `_member(node, key)`: the LAST member whose key equals `key` (str == x).
pub(crate) fn member<'d>(doc: &'d Doc, node: usize, key: &Value) -> Option<&'d Member> {
    let Value::Str(k) = key else { return None };
    doc.node(node).members.iter().rev().find(|m| &m.key == k)
}

/// `_dotted(path)`: `".".join(path) or "settings.json"`.
pub(crate) fn dotted(path: &[Value]) -> Result<String, String> {
    let mut parts = Vec::with_capacity(path.len());
    for (i, p) in path.iter().enumerate() {
        match p {
            Value::Str(s) => parts.push(s.as_str()),
            other => {
                return Err(type_error(format!(
                    "sequence item {i}: expected str instance, {} found",
                    pyval::type_name(other)
                )));
            }
        }
    }
    let joined = parts.join(".");
    Ok(if joined.is_empty() {
        "settings.json".into()
    } else {
        joined
    })
}

/// `_descend(root, path)`: the chain of nodes `[root, …]` for the leading part of `path` that
/// exists. A non-object where a key must be looked up is a ValueError.
pub(crate) fn descend(doc: &Doc, path: &[Value]) -> Result<Vec<usize>, String> {
    let mut chain = vec![Doc::ROOT];
    for (i, key) in path.iter().enumerate() {
        let node = *chain.last().expect("the chain starts at the root");
        if doc.node(node).kind != Kind::Object {
            return Err(format!("{} is not an object", dotted(&path[..i])?));
        }
        match member(doc, node, key) {
            None => break,
            Some(m) => chain.push(m.value),
        }
    }
    Ok(chain)
}

/// `_strings(text, arr)`: each item's decoded string, or `None` (`Value::Null`) for a
/// non-string item.
pub(crate) fn strings(text: &str, doc: &Doc, arr: usize) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for &item in &doc.node(arr).items {
        let n = doc.node(item);
        out.push(if n.kind == Kind::Str {
            Value::Str(decode_string(&text[n.start..n.end])?)
        } else {
            Value::Null
        });
    }
    Ok(out)
}

/// `value in names` (`x is e or x == e`; `value` is never one of the fresh `names`).
pub(crate) fn contains(names: &[Value], value: &Value) -> bool {
    names.iter().any(|n| py_eq(value, n))
}
