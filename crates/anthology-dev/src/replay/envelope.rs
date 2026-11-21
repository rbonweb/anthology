//! `replay` part `envelope`: the PreToolUse hook input a replayed call is fed as (design §13.7).
//!
//! - The envelope is `{"session_id","transcript_path","cwd","permission_mode",
//!   "hook_event_name":"PreToolUse","tool_name","tool_input","tool_use_id"}`, serialized the way
//!   Claude Code does (`JSON.stringify`: [`json::js_stringify`]).
//! - `tool_input` is **spliced as the raw bytes** from the transcript line, so the real encodings
//!   (escapes, raw non-ASCII, number spellings) reach the guards' decoders.
//! - The **canonical** form of a call, `{"tool_input":…,"tool_name":…}` as
//!   `json.dumps(sort_keys=True, separators=(",",":"))`, is both the corpus's deduplication key
//!   and the message the case id is the HMAC of. It ignores the session fields, so a case keeps
//!   its id across collections.
//! - The corpus stores each call as one JSON line whose `tool_input` is the same raw splice;
//!   [`span`] finds a value's bytes inside a JSON text.

use std::ops::Range;

use anthology_pycompat::json::{self, COMPACT_SORTED, Map, Value};

/// One distinct tool call of the corpus, with the metadata of its first occurrence.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Call {
    /// How many times the call occurs in the transcripts.
    pub count: u64,
    pub tool_name: String,
    pub session_id: String,
    pub transcript_path: String,
    pub cwd: String,
    pub permission_mode: String,
    pub tool_use_id: String,
    /// The `input` value's bytes, exactly as in the transcript line.
    pub raw_input: Vec<u8>,
}

impl Call {
    /// The parsed `tool_input` (`Null` when the bytes do not parse, which a transcript line
    /// that parsed cannot produce).
    pub fn input(&self) -> Value {
        json::loads_bytes_lossy(&self.raw_input).unwrap_or(Value::Null)
    }

    /// The hook input Claude Code would send for this call (module docs).
    pub fn envelope(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.raw_input.len() + 256);
        let field = |out: &mut Vec<u8>, first: bool, key: &str, value: &str| {
            out.extend_from_slice(if first { b"{" } else { b"," });
            out.extend_from_slice(js_str(key).as_bytes());
            out.push(b':');
            out.extend_from_slice(js_str(value).as_bytes());
        };
        field(&mut out, true, "session_id", &self.session_id);
        field(&mut out, false, "transcript_path", &self.transcript_path);
        field(&mut out, false, "cwd", &self.cwd);
        field(&mut out, false, "permission_mode", &self.permission_mode);
        field(&mut out, false, "hook_event_name", "PreToolUse");
        field(&mut out, false, "tool_name", &self.tool_name);
        out.extend_from_slice(b",\"tool_input\":");
        out.extend_from_slice(&self.raw_input);
        field(&mut out, false, "tool_use_id", &self.tool_use_id);
        out.push(b'}');
        out
    }

    /// The canonical form (module docs).
    pub fn canonical(&self) -> String {
        canonical(&self.tool_name, &self.input())
    }

    /// The corpus line (no newline): the metadata, then `"tool_input":` and the raw bytes.
    pub fn corpus_line(&self) -> Vec<u8> {
        let mut m = Map::new();
        m.insert(
            "count".into(),
            Value::Int(json::Int::Small(self.count as i64)),
        );
        for (k, v) in [
            ("tool_name", &self.tool_name),
            ("session_id", &self.session_id),
            ("transcript_path", &self.transcript_path),
            ("cwd", &self.cwd),
            ("permission_mode", &self.permission_mode),
            ("tool_use_id", &self.tool_use_id),
        ] {
            m.insert(k.into(), Value::Str(v.clone()));
        }
        let mut out = json::js_stringify(&Value::Object(m)).into_bytes();
        out.pop(); // the closing brace
        out.extend_from_slice(b",\"tool_input\":");
        out.extend_from_slice(&self.raw_input);
        out.push(b'}');
        out
    }

    /// A corpus line back into a call; `None` when it is not one.
    pub fn from_corpus_line(line: &[u8]) -> Option<Call> {
        let value = json::loads_bytes_lossy(line).ok()?;
        let m = value.as_object()?;
        let s = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_string);
        let raw = span(line, &[Step::Key("tool_input")])?;
        Some(Call {
            count: u64::try_from(m.get("count")?.as_i64()?).ok()?,
            tool_name: s("tool_name")?,
            session_id: s("session_id")?,
            transcript_path: s("transcript_path")?,
            cwd: s("cwd")?,
            permission_mode: s("permission_mode")?,
            tool_use_id: s("tool_use_id")?,
            raw_input: line[raw].to_vec(),
        })
    }
}

/// `JSON.stringify` of a string.
pub fn js_str(s: &str) -> String {
    json::js_stringify(&Value::Str(s.to_string()))
}

/// The canonical form of a call (module docs).
pub fn canonical(tool_name: &str, input: &Value) -> String {
    let mut m = Map::new();
    m.insert("tool_input".into(), input.clone());
    m.insert("tool_name".into(), Value::Str(tool_name.to_string()));
    json::dumps(&Value::Object(m), &COMPACT_SORTED)
}

/// One step of a path into a JSON text.
#[derive(Clone, Copy, Debug)]
pub enum Step<'a> {
    /// An object member; with duplicate keys the last one wins, as in `json.loads`.
    Key(&'a str),
    /// An array element.
    Index(usize),
}

/// The byte range of the value at `path` in the JSON text `text` (whitespace excluded), or
/// `None` when the text is not well-formed along the way or the path is absent.
pub fn span(text: &[u8], path: &[Step<'_>]) -> Option<Range<usize>> {
    let mut start = ws(text, 0);
    let mut end = skip_value(text, start)?;
    for step in path {
        let found = match *step {
            Step::Key(key) => member(text, start, key)?,
            Step::Index(n) => element(text, start, n)?,
        };
        start = found.start;
        end = found.end;
    }
    Some(start..end)
}

fn ws(text: &[u8], mut i: usize) -> usize {
    while i < text.len() && matches!(text[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// The members of the object at `i`: the value range of the last `key`.
fn member(text: &[u8], i: usize, key: &str) -> Option<Range<usize>> {
    if text.get(i) != Some(&b'{') {
        return None;
    }
    let mut found = None;
    let mut i = ws(text, i + 1);
    if text.get(i) == Some(&b'}') {
        return None;
    }
    loop {
        let key_end = skip_string(text, i)?;
        let name = json::loads_bytes_lossy(&text[i..key_end]).ok()?;
        i = ws(text, key_end);
        if text.get(i) != Some(&b':') {
            return None;
        }
        let vs = ws(text, i + 1);
        let ve = skip_value(text, vs)?;
        if name.as_str() == Some(key) {
            found = Some(vs..ve);
        }
        i = ws(text, ve);
        match text.get(i) {
            Some(b',') => i = ws(text, i + 1),
            Some(b'}') => return found,
            _ => return None,
        }
    }
}

/// The `n`th element of the array at `i`.
fn element(text: &[u8], i: usize, n: usize) -> Option<Range<usize>> {
    if text.get(i) != Some(&b'[') {
        return None;
    }
    let mut i = ws(text, i + 1);
    if text.get(i) == Some(&b']') {
        return None;
    }
    let mut k = 0;
    loop {
        let end = skip_value(text, i)?;
        if k == n {
            return Some(i..end);
        }
        k += 1;
        i = ws(text, end);
        match text.get(i) {
            Some(b',') => i = ws(text, i + 1),
            _ => return None,
        }
    }
}

/// The end of the string starting at `i`.
fn skip_string(text: &[u8], i: usize) -> Option<usize> {
    if text.get(i) != Some(&b'"') {
        return None;
    }
    let mut j = i + 1;
    while j < text.len() {
        match text[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// The end of the value starting at `i` (iterative: any depth). Containers are skipped by
/// bracket counting outside strings; their contents are checked by the caller's parse.
fn skip_value(text: &[u8], i: usize) -> Option<usize> {
    match text.get(i)? {
        b'"' => skip_string(text, i),
        b'{' | b'[' => {
            let mut depth = 0usize;
            let mut j = i;
            while j < text.len() {
                match text[j] {
                    b'"' => {
                        j = skip_string(text, j)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            None
        }
        _ => {
            let mut j = i;
            while j < text.len()
                && !matches!(text[j], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                j += 1;
            }
            (j > i).then_some(j)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(raw: &str) -> Call {
        Call {
            count: 2,
            tool_name: "Bash".into(),
            session_id: "s-1".into(),
            transcript_path: "/t/p.jsonl".into(),
            cwd: "/w".into(),
            permission_mode: "default".into(),
            tool_use_id: "toolu_1".into(),
            raw_input: raw.as_bytes().to_vec(),
        }
    }

    #[test]
    fn the_envelope_splices_the_raw_input_bytes() {
        let c = call(r#"{"command": "echo é é",  "timeout":1.0e3}"#);
        let env = String::from_utf8(c.envelope()).expect("utf8");
        assert_eq!(
            env,
            concat!(
                r#"{"session_id":"s-1","transcript_path":"/t/p.jsonl","cwd":"/w","#,
                r#""permission_mode":"default","hook_event_name":"PreToolUse","tool_name":"Bash","#,
                r#""tool_input":{"command": "echo é é",  "timeout":1.0e3},"#,
                r#""tool_use_id":"toolu_1"}"#
            )
        );
        // It is one JSON object whose tool_input parses to the same value.
        let v = json::loads(&env).expect("json");
        let m = v.as_object().expect("object");
        assert_eq!(m.get("tool_input"), Some(&c.input()));
        assert_eq!(
            m.get("hook_event_name").and_then(Value::as_str),
            Some("PreToolUse")
        );
    }

    #[test]
    fn metadata_strings_are_js_stringified() {
        let mut c = call("{}");
        c.cwd = "/w/\u{7}\"é\u{2028}".into();
        let env = String::from_utf8(c.envelope()).expect("utf8");
        assert!(env.contains(r#""cwd":"/w/\u0007\"é"#), "{env}");
        assert!(env.contains('\u{2028}'), "raw, as JSON.stringify writes it");
    }

    #[test]
    fn the_canonical_form_ignores_spelling_and_session_fields() {
        let a = call(r#"{"b": 1, "a": "é"}"#);
        let mut b = call(r#"{"a":"é","b":1}"#);
        b.session_id = "other".into();
        b.cwd = "/elsewhere".into();
        assert_eq!(a.canonical(), b.canonical());
        assert_eq!(
            a.canonical(),
            r#"{"tool_input":{"a":"\u00e9","b":1},"tool_name":"Bash"}"#
        );
        let mut c = call(r#"{"a":"é","b":1}"#);
        c.tool_name = "Read".into();
        assert_ne!(a.canonical(), c.canonical());
    }

    #[test]
    fn a_corpus_line_round_trips_byte_for_byte() {
        let c = call("{ \"command\" : \"ls\\n\" , \"x\" : [1, {\"y\": \"}\"}] }");
        let line = c.corpus_line();
        assert_eq!(Call::from_corpus_line(&line), Some(c.clone()));
        // Invalid UTF-8 in the input survives too.
        let mut raw = b"{\"command\":\"a".to_vec();
        raw.extend_from_slice(&[0xff, 0xfe]);
        raw.extend_from_slice(b"\"}");
        let bad = Call {
            raw_input: raw,
            ..c
        };
        assert_eq!(Call::from_corpus_line(&bad.corpus_line()), Some(bad));
        assert_eq!(Call::from_corpus_line(b"[1]"), None);
        assert_eq!(Call::from_corpus_line(b"{\"count\":1}"), None);
    }

    #[test]
    fn span_finds_values_by_path() {
        let text = br#" {"message": {"content": [ {"type":"text","text":"a\"]}"},
            {"type": "tool_use", "input": {"k": [1, 2]} , "input": {"k": "last"}} ]}} "#;
        let at = |path: &[Step<'_>]| span(text, path).map(|r| &text[r]);
        assert_eq!(
            at(&[
                Step::Key("message"),
                Step::Key("content"),
                Step::Index(1),
                Step::Key("input")
            ]),
            Some(&br#"{"k": "last"}"#[..])
        );
        assert_eq!(
            at(&[
                Step::Key("message"),
                Step::Key("content"),
                Step::Index(0),
                Step::Key("text")
            ]),
            Some(&br#""a\"]}""#[..])
        );
        assert_eq!(
            at(&[Step::Key("message"), Step::Key("content"), Step::Index(2)]),
            None
        );
        assert_eq!(at(&[Step::Key("nope")]), None);
        assert_eq!(at(&[Step::Key("message"), Step::Index(0)]), None);
        // An escaped key matches its decoded name.
        let esc = br#"{"tool\u005finput": 7}"#;
        assert_eq!(
            span(esc, &[Step::Key("tool_input")]).map(|r| &esc[r]),
            Some(&b"7"[..])
        );
        // Unterminated texts give None.
        assert_eq!(span(br#"{"a": "#, &[Step::Key("a")]), None);
        assert_eq!(span(br#"{"a": [1, 2"#, &[Step::Key("a")]), None);
        assert_eq!(span(br#"{"a": "x"#, &[Step::Key("a")]), None);
    }

    #[test]
    fn span_is_iterative_on_deep_nesting() {
        let depth = 200_000;
        let mut text = b"{\"a\":".to_vec();
        text.extend(std::iter::repeat_n(b'[', depth));
        text.extend(std::iter::repeat_n(b']', depth));
        text.push(b'}');
        assert_eq!(
            span(&text, &[Step::Key("a")]).map(|r| r.len()),
            Some(2 * depth)
        );
    }
}
