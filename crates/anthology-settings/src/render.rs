//! Rendering patched values (ST1): `settings_patch.py`'s `_Style`, `_style`, `_render`,
//! `_line_indent`, `_container_multiline`, `_sep`, `_append`, `_remove_at` and `_nest`.

use anthology_pycompat::json::{self, Map, PY_DEFAULT_UNICODE, Value};
use anthology_pycompat::str::is_space;

use crate::scanner::{Doc, Kind};

/// Values nested deeper than this are refused before rendering (Python's `_render` recursion
/// raises `RecursionError` near 1,000 levels; D-1), so the recursion here stays bounded.
pub(crate) const MAX_RENDER_DEPTH: usize = 900;

/// `_Style`: the file's newline, indent unit, colon and inline comma.
#[derive(Clone, Debug)]
pub(crate) struct Style {
    pub(crate) nl: &'static str,
    pub(crate) unit: String,
    pub(crate) colon: String,
    pub(crate) comma: &'static str,
}

/// The first `[ \t]+` run that starts a line (`(?m)^`, i.e. at 0 or after `\n`) and is
/// followed by a non-whitespace character (`\S`): `re.search(r"(?m)^([ \t]+)\S", text)`.
fn indent_run(text: &str) -> Option<&str> {
    let b = text.as_bytes();
    let mut start = 0;
    loop {
        let mut j = start;
        while j < b.len() && matches!(b[j], b' ' | b'\t') {
            j += 1;
        }
        if j > start
            && let Some(c) = text[j..].chars().next()
            && !is_space(c)
        {
            return Some(&text[start..j]);
        }
        // The next line start: after the next `\n` at or after `j`.
        start = j + text[j..].find('\n')? + 1;
    }
}

/// `_style(text, root)`.
pub(crate) fn style(text: &str, root: Option<&Doc>) -> Style {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let unit = match indent_run(text) {
        Some(run) if run.starts_with('\t') => "\t".to_string(),
        Some(run) => run.to_string(),
        None => "  ".to_string(),
    };
    let mut colon = ": ".to_string();
    if let Some(doc) = root {
        // Breadth first from the root; only arrays add children, and the first non-empty
        // object ends the search.
        let mut queue = std::collections::VecDeque::from([Doc::ROOT]);
        while let Some(n) = queue.pop_front() {
            let node = doc.node(n);
            match node.kind {
                Kind::Object => {
                    if let Some(first) = node.members.first() {
                        colon = text[first.kend..doc.node(first.value).start].to_string();
                        break;
                    }
                }
                Kind::Array => queue.extend(node.items.iter().copied()),
                _ => {}
            }
        }
    }
    let comma = if colon == ":" { "," } else { ", " };
    Style {
        nl,
        unit,
        colon,
        comma,
    }
}

/// `json.dumps(v, ensure_ascii=False)`.
pub(crate) fn dumps_unicode(v: &Value) -> String {
    json::dumps(v, &PY_DEFAULT_UNICODE)
}

/// `json.dumps(key, ensure_ascii=False)` for a str key.
pub(crate) fn dumps_key(k: &str) -> String {
    dumps_unicode(&Value::Str(k.to_string()))
}

/// The nesting depth of `v` (a scalar is 0), iteratively.
pub(crate) fn value_depth(v: &Value) -> usize {
    let mut max = 0;
    let mut stack = vec![(v, 0usize)];
    while let Some((v, d)) = stack.pop() {
        max = max.max(d);
        match v {
            Value::Array(a) => stack.extend(a.iter().map(|x| (x, d + 1))),
            Value::Object(m) => stack.extend(m.iter().map(|(_, x)| (x, d + 1))),
            _ => {}
        }
    }
    max
}

/// Refuse a value `_render` would overflow on.
pub(crate) fn check_render_depth(v: &Value) -> Result<(), String> {
    if value_depth(v) > MAX_RENDER_DEPTH {
        return Err("RecursionError: maximum recursion depth exceeded".into());
    }
    Ok(())
}

/// `_render(value, indent, st, multi)`: the JSON text of `value` whose own line starts with
/// `indent` (dicts keep insertion order). Callers check the depth first.
pub(crate) fn render(value: &Value, indent: &str, st: &Style, multi: bool) -> String {
    match value {
        Value::Object(m) => {
            if m.is_empty() {
                return "{}".into();
            }
            let inner = format!("{indent}{}", st.unit);
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}{}{}",
                        dumps_key(k),
                        st.colon,
                        render(v, &inner, st, multi)
                    )
                })
                .collect();
            wrap("{", "}", &parts, indent, &inner, st, multi)
        }
        Value::Array(a) => {
            if a.is_empty() {
                return "[]".into();
            }
            let inner = format!("{indent}{}", st.unit);
            let parts: Vec<String> = a.iter().map(|v| render(v, &inner, st, multi)).collect();
            wrap("[", "]", &parts, indent, &inner, st, multi)
        }
        scalar => dumps_unicode(scalar),
    }
}

fn wrap(
    open: &str,
    close: &str,
    parts: &[String],
    indent: &str,
    inner: &str,
    st: &Style,
    multi: bool,
) -> String {
    if multi {
        let sep = format!(",{}{inner}", st.nl);
        format!(
            "{open}{}{inner}{}{}{indent}{close}",
            st.nl,
            parts.join(&sep),
            st.nl
        )
    } else {
        format!("{open}{}{close}", parts.join(st.comma))
    }
}

/// `_line_indent(text, pos)`: the `[ \t]*` run at the start of `pos`'s line, up to `pos`.
pub(crate) fn line_indent(text: &str, pos: usize) -> &str {
    let start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let b = text.as_bytes();
    let mut j = start;
    while j < pos && matches!(b[j], b' ' | b'\t') {
        j += 1;
    }
    &text[start..j]
}

/// `_container_multiline(text, chain)`: is the (empty) container at the end of `chain` laid
/// out over several lines? It follows its parent.
pub(crate) fn container_multiline(text: &str, doc: &Doc, chain: &[usize]) -> bool {
    if chain.len() < 2 {
        return true;
    }
    let parent = chain[chain.len() - 2];
    text[doc.node(parent).start + 1..doc.item_start(parent, 0)].contains('\n')
}

/// `_sep(text, node)`: the whitespace before the last item of `node` (after its comma or
/// bracket).
pub(crate) fn sep<'t>(text: &'t str, doc: &Doc, node: usize) -> &'t str {
    let n = doc.node(node).len();
    let prev_end = if n > 1 {
        doc.item_end(node, n - 2)
    } else {
        doc.node(node).start + 1
    };
    let gap = &text[prev_end..doc.item_start(node, n - 1)];
    if n > 1 {
        gap.rsplit_once(',').map_or(gap, |(_, after)| after)
    } else {
        gap
    }
}

/// `_append(text, root, chain, node, key, value)`: add `value` to the array `node` (`key` None)
/// or `key: value` to the object `node`.
pub(crate) fn append(
    text: &str,
    doc: &Doc,
    chain: &[usize],
    node: usize,
    key: Option<&str>,
    value: &Value,
) -> String {
    let st = style(text, Some(doc));
    let n = doc.node(node);
    if !n.is_empty() {
        let mut sep = sep(text, doc, node).to_string();
        let multi = sep.contains('\n');
        if !multi && sep.is_empty() && n.len() == 1 {
            // Inline: keep the file's `, ` style (the gap is empty when there is one item).
            sep = st.comma[1..].to_string();
        }
        let indent = if multi {
            sep.rsplit_once('\n')
                .map_or(sep.as_str(), |(_, after)| after)
                .to_string()
        } else {
            String::new()
        };
        let item = match key {
            None => render(value, &indent, &st, multi),
            Some(k) => {
                let last = n.members.last().expect("a non-empty object has members");
                format!(
                    "{}{}{}",
                    dumps_key(k),
                    &text[last.kend..doc.node(last.value).start],
                    render(value, &indent, &st, multi)
                )
            }
        };
        let at = doc.item_end(node, n.len() - 1);
        return format!("{},{sep}{item}{}", &text[..at], &text[at..]);
    }
    let multi = container_multiline(text, doc, chain);
    let base = line_indent(text, n.start);
    let wrapped = match key {
        None => Value::Array(vec![value.clone()]),
        Some(k) => {
            let mut m = Map::new();
            m.insert(k.to_string(), value.clone());
            Value::Object(m)
        }
    };
    let new = render(&wrapped, base, &st, multi);
    format!("{}{new}{}", &text[..n.start], &text[n.end..])
}

/// `_remove_at(text, node, idx)`: drop item `idx` of `node` (the inverse of `_append` for the
/// last item).
pub(crate) fn remove_at(text: &str, doc: &Doc, node: usize, idx: usize) -> String {
    let nd = doc.node(node);
    let n = nd.len();
    if n == 1 {
        let empty = if nd.kind == Kind::Object { "{}" } else { "[]" };
        return format!("{}{empty}{}", &text[..nd.start], &text[nd.end..]);
    }
    if idx == n - 1 {
        return format!(
            "{}{}",
            &text[..doc.item_end(node, n - 2)],
            &text[doc.item_end(node, n - 1)..]
        );
    }
    format!(
        "{}{}",
        &text[..doc.item_start(node, idx)],
        &text[doc.item_start(node, idx + 1)..]
    )
}

/// `_nest(keys, leaf)`.
pub(crate) fn nest(keys: &[String], leaf: Value) -> Value {
    let mut leaf = leaf;
    for key in keys.iter().rev() {
        let mut m = Map::new();
        m.insert(key.clone(), leaf);
        leaf = Value::Object(m);
    }
    leaf
}
