//! `plan` (ST1), with `apply`: `settings_patch.py`'s `_create`, `_apply_op`, `_run`, `plan`
//! and `apply`.

use anthology_pycompat::json::{Map, Value};

use crate::diff::diff_texts;
use crate::ops::Op;
use crate::render::{append, check_render_depth, dumps_unicode, nest, remove_at, render, style};
use crate::scanner::pyval::py_eq;
use crate::scanner::{Doc, Kind, contains, descend, dotted, is_blank, load_raw, parse, strings};

/// What `plan` returns: the ops that still have to be applied (`ops`) and, for each, the JSON
/// record `revert` needs to undo it exactly (`stored`; Python's `plan` result: the op's keys,
/// then `created`, `empty` and `previous`).
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub ops: Vec<Op>,
    pub stored: Vec<Value>,
}

impl Planned {
    /// Nothing left to apply.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

pub(crate) fn path_values(path: &[String]) -> Vec<Value> {
    path.iter().map(|p| Value::Str(p.clone())).collect()
}

/// `[path[:k] for k in range(from, upto + 1)]`.
fn prefixes(path: &[String], from: usize, upto: isize) -> Value {
    let mut out = Vec::new();
    let mut k = from as isize;
    while k <= upto {
        out.push(Value::Array(path_values(&path[..k as usize])));
        k += 1;
    }
    Value::Array(out)
}

fn obj(pairs: Vec<(&str, Value)>) -> Map {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    m
}

/// `_create(text, op, path, leaf, created_upto, existing)`: insert `leaf` at `path`, creating
/// the missing containers.
pub(crate) fn create(
    text: &str,
    path: &[String],
    leaf: Value,
    created_upto: isize,
    existing: Option<&Doc>,
) -> Result<(String, Map), String> {
    if is_blank(text) {
        let st = style(text, None);
        let nested = nest(path, leaf);
        check_render_depth(&nested)?;
        let mut doc = render(&nested, "", &st, true) + st.nl;
        if text.starts_with('\u{feff}') {
            doc.insert(0, '\u{feff}');
        }
        let empty = obj(vec![
            ("path", Value::Array(Vec::new())),
            ("raw", Value::Str(text.to_string())),
            ("whole", Value::Bool(true)),
        ]);
        let info = obj(vec![
            ("created", prefixes(path, 1, created_upto)),
            ("empty", Value::Object(empty)),
        ]);
        return Ok((doc, info));
    }
    let parsed;
    let root = match existing {
        Some(d) => d,
        None => {
            parsed = parse(text)?;
            &parsed
        }
    };
    let chain = descend(root, &path_values(path))?;
    let n = chain.len() - 1;
    let node = *chain.last().expect("the chain starts at the root");
    let mut info = obj(vec![("created", prefixes(path, n + 1, created_upto))]);
    let nd = root.node(node);
    if nd.is_empty() {
        info.insert(
            "empty".into(),
            Value::Object(obj(vec![
                ("path", Value::Array(path_values(&path[..n]))),
                ("raw", Value::Str(text[nd.start..nd.end].to_string())),
            ])),
        );
    }
    let nested = nest(&path[n + 1..], leaf);
    check_render_depth(&nested)?;
    let out = append(text, root, &chain, node, Some(&path[n]), &nested);
    Ok((out, info))
}

/// `_apply_op(text, op)`: `(text, info)`; `info` is `None` when the op was already satisfied.
fn apply_op(text: &str, op: &Op) -> Result<(String, Option<Map>), String> {
    let path = op.path();
    let value = op.value();
    let pv = path_values(path);
    match op {
        Op::Add { .. } => {
            if is_blank(text) {
                let (t, i) = create(
                    text,
                    path,
                    Value::Array(vec![value.clone()]),
                    path.len() as isize,
                    None,
                )?;
                return Ok((t, Some(i)));
            }
            let root = parse(text)?;
            let chain = descend(&root, &pv)?;
            if chain.len() - 1 == path.len() {
                let arr = *chain.last().expect("the chain starts at the root");
                if root.node(arr).kind != Kind::Array {
                    return Err(format!("{} is not a list", dotted(&pv)?));
                }
                if contains(&strings(text, &root, arr)?, value) {
                    return Ok((text.to_string(), None));
                }
                let mut info = obj(vec![("created", Value::Array(Vec::new()))]);
                let nd = root.node(arr);
                if nd.is_empty() {
                    info.insert(
                        "empty".into(),
                        Value::Object(obj(vec![
                            ("path", Value::Array(pv.clone())),
                            ("raw", Value::Str(text[nd.start..nd.end].to_string())),
                        ])),
                    );
                }
                check_render_depth(value)?;
                return Ok((append(text, &root, &chain, arr, None, value), Some(info)));
            }
            let (t, i) = create(
                text,
                path,
                Value::Array(vec![value.clone()]),
                path.len() as isize,
                Some(&root),
            )?;
            Ok((t, Some(i)))
        }
        Op::Set { .. } => {
            let upto = path.len() as isize - 1;
            let (out, mut info) = if is_blank(text) {
                create(text, path, value.clone(), upto, None)?
            } else {
                let root = parse(text)?;
                let chain = descend(&root, &pv)?;
                if chain.len() - 1 == path.len() {
                    let nd = root.node(*chain.last().expect("the chain starts at the root"));
                    let raw = &text[nd.start..nd.end];
                    if py_eq(&load_raw(raw)?, value) {
                        return Ok((text.to_string(), None));
                    }
                    let new = dumps_unicode(value);
                    let previous = obj(vec![
                        ("present", Value::Bool(true)),
                        ("raw", Value::Str(raw.to_string())),
                    ]);
                    let info = obj(vec![
                        ("created", Value::Array(Vec::new())),
                        ("previous", Value::Object(previous)),
                    ]);
                    let out = format!("{}{new}{}", &text[..nd.start], &text[nd.end..]);
                    return Ok((out, Some(info)));
                }
                create(text, path, value.clone(), upto, Some(&root))?
            };
            info.insert(
                "previous".into(),
                Value::Object(obj(vec![("present", Value::Bool(false))])),
            );
            Ok((out, Some(info)))
        }
        Op::Unset { .. } => {
            if path.is_empty() || is_blank(text) {
                return Ok((text.to_string(), None));
            }
            let root = parse(text)?;
            // A parent on the path that is not an object: nothing to drop.
            let Ok(chain) = descend(&root, &pv) else {
                return Ok((text.to_string(), None));
            };
            if chain.len() - 1 != path.len() {
                return Ok((text.to_string(), None));
            }
            let (node, parent) = (chain[chain.len() - 1], chain[chain.len() - 2]);
            let key = path.last().expect("the path is not empty");
            if root
                .node(parent)
                .members
                .iter()
                .filter(|m| &m.key == key)
                .count()
                > 1
            {
                // Set twice: dropping one copy would leave the other in force.
                return Ok((text.to_string(), None));
            }
            let nd = root.node(node);
            let previous = obj(vec![
                ("present", Value::Bool(true)),
                ("raw", Value::Str(text[nd.start..nd.end].to_string())),
            ]);
            let info = obj(vec![
                ("created", Value::Array(Vec::new())),
                ("previous", Value::Object(previous)),
            ]);
            let idx = root.member_index(parent, node);
            Ok((remove_at(text, &root, parent, idx), Some(info)))
        }
    }
}

/// `_run(text, ops)`: the new text, the records of the ops that changed it, and those ops.
pub(crate) fn run(text: &str, ops: &[Op]) -> Result<(String, Planned), String> {
    let mut text = text.to_string();
    let mut planned = Planned {
        ops: Vec::new(),
        stored: Vec::new(),
    };
    for op in ops {
        let (t, info) = apply_op(&text, op)?;
        text = t;
        if let Some(info) = info {
            let mut base = op.to_value().into_object().expect("an op is a dict");
            for (k, v) in info.iter() {
                base.insert(k.to_string(), v.clone());
            }
            planned.stored.push(Value::Object(base));
            planned.ops.push(op.clone());
        }
    }
    Ok((text, planned))
}

/// `plan(settings, ops)`: the ops that still have to be applied to `text`, with what is
/// needed to undo them.
pub fn plan(text: &str, ops: &[Op]) -> Result<Planned, String> {
    if ops.is_empty() && !is_blank(text) {
        parse(text)?;
    }
    Ok(run(text, ops)?.1)
}

/// `apply(settings, ops)`: `(new_text, diff)`, the diff redacted (GC-8). Applying an op twice
/// is a no-op.
pub fn apply(text: &str, ops: &[Op]) -> Result<(String, String), String> {
    let (new, _) = run(text, ops)?;
    let diff = diff_texts(text, &new)?;
    Ok((new, diff))
}
