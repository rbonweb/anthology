//! `revert` (ST1): `settings_patch.py`'s `_revert_op` and `revert`.
//!
//! The stored ops are whatever state.json holds, so every access is Python's duck typing on
//! the decoded value (`scanner::pyval`): where Python succeeds on an odd shape, so does this;
//! where Python raises, this returns an error (lead rulings R2, R4).

use anthology_pycompat::json::Value;

use crate::diff::diff_texts;
use crate::render::remove_at;
use crate::scanner::pyval::{
    get, getitem, py_eq, py_len, py_list, py_reversed, truthy, type_error, type_name,
};
use crate::scanner::{Kind, contains, descend, is_blank, load_raw, parse, strings};

/// `text[:a] + raw + text[b:]` with Python's `str + x` check.
fn splice(text: &str, a: usize, raw: &Value, b: usize) -> Result<String, String> {
    match raw {
        Value::Str(r) => Ok(format!("{}{r}{}", &text[..a], &text[b..])),
        other => Err(type_error(format!(
            "can only concatenate str (not \"{}\") to str",
            type_name(other)
        ))),
    }
}

/// `x or default` for a looked-up value: `None` when Python's result is the empty default.
fn or_empty(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| truthy(x))
}

/// `_revert_op(text, op)`.
fn revert_op(text: &str, op: &Value) -> Result<String, String> {
    let path = py_list(getitem(op, "path")?)?;
    let doc = parse(text)?;
    let chain = descend(&doc, &path)?;
    let found = chain.len() - 1 == path.len();
    let value = getitem(op, "value")?;
    let mut text = text.to_string();
    if py_eq(getitem(op, "op")?, &Value::Str("add".into())) {
        let last = *chain.last().expect("the chain starts at the root");
        if found && doc.node(last).kind == Kind::Array {
            let names = strings(&text, &doc, last)?;
            if contains(&names, value) {
                let idx = names
                    .iter()
                    .rposition(|n| py_eq(value, n))
                    .expect("contains found it");
                text = remove_at(&text, &doc, last, idx);
            }
        }
    } else if found && chain.len() > 1 {
        let node = chain[chain.len() - 1];
        let nd = doc.node(node);
        if py_eq(&load_raw(&text[nd.start..nd.end])?, value) {
            // `prev = op.get("previous") or {}`; `{}.get("present")` is None.
            let prev = or_empty(get(op, "previous")?);
            let present = match prev {
                Some(p) => get(p, "present")?.is_some_and(truthy),
                None => false,
            };
            if present {
                let raw = getitem(prev.expect("present implies a previous"), "raw")?;
                text = splice(&text, nd.start, raw, nd.end)?;
            } else {
                let parent = chain[chain.len() - 2];
                let idx = doc.member_index(parent, node);
                text = remove_at(&text, &doc, parent, idx);
            }
        }
    }
    if let Some(created) = or_empty(get(op, "created")?) {
        for c in py_reversed(created)? {
            // `_descend(_parse(text), list(created))`: the parse runs first.
            let doc = parse(&text)?;
            let cpath = py_list(&c)?;
            let chain = descend(&doc, &cpath)?;
            if chain.len() - 1 != py_len(&c).unwrap_or(cpath.len()) || chain.len() < 2 {
                continue;
            }
            let (node, parent) = (chain[chain.len() - 1], chain[chain.len() - 2]);
            let nd = doc.node(node);
            if !nd.kind.is_container() || !nd.is_empty() {
                break;
            }
            let idx = doc.member_index(parent, node);
            text = remove_at(&text, &doc, parent, idx);
        }
    }
    if let Some(empty) = or_empty(get(op, "empty")?) {
        // `_descend(_parse(text), list(empty["path"]))`: the parse runs first.
        let doc = parse(&text)?;
        let epath_v = getitem(empty, "path")?;
        let epath = py_list(epath_v)?;
        let chain = descend(&doc, &epath)?;
        let node = *chain.last().expect("the chain starts at the root");
        let nd = doc.node(node);
        if chain.len() - 1 == py_len(epath_v).unwrap_or(epath.len())
            && nd.kind.is_container()
            && nd.is_empty()
        {
            if get(empty, "whole")?.is_some_and(truthy) {
                // `return empty["raw"]`: a non-str raw fails in the caller (`_is_blank` or
                // `redact` call `.strip()` on it).
                return match getitem(empty, "raw")? {
                    Value::Str(r) => Ok(r.clone()),
                    other => Err(format!(
                        "AttributeError: '{}' object has no attribute 'strip'",
                        type_name(other)
                    )),
                };
            }
            text = splice(&text, nd.start, getitem(empty, "raw")?, nd.end)?;
        }
    }
    Ok(text)
}

/// `revert(settings, ops)`: `(new_text, diff)` with the stored `ops` removed, last first.
pub fn revert_with_diff(text: &str, stored: &[Value]) -> Result<(String, String), String> {
    if is_blank(text) {
        return Ok((text.to_string(), String::new()));
    }
    parse(text)?;
    let mut out = text.to_string();
    for op in stored.iter().rev() {
        if is_blank(&out) {
            break;
        }
        out = revert_op(&out, op)?;
    }
    let diff = diff_texts(text, &out)?;
    Ok((out, diff))
}

/// `revert(settings, ops)[0]` (plan §3.6); the diff is [`revert_with_diff`]'s.
pub fn revert(text: &str, stored: &[Value]) -> Result<String, String> {
    Ok(revert_with_diff(text, stored)?.0)
}

/// The stored ops as Python's callers hand them over: `list(state.get("settings_ops") or [])`
/// (setup's `reconcile` and `remove`'s `revert` both take `list()` of whatever state.json
/// holds). Missing or falsy is empty; a list gives its items, a str its characters and a dict
/// its keys; any other value is Python's `TypeError`.
pub fn stored_ops(settings_ops: Option<&Value>) -> Result<Vec<Value>, String> {
    match settings_ops {
        Some(v) if truthy(v) => py_list(v),
        _ => Ok(Vec::new()),
    }
}
