//! `reconcile` and `ops_version` (ST1): `settings_patch.py`'s `ops_version`, `_key` and
//! `reconcile`.

use anthology_pycompat::json::{self, COMPACT_SORTED, Int, PY_DEFAULT_SORTED, Value};
use sha2::{Digest, Sha256};

use crate::diff::diff_texts;
use crate::ops::{
    LEGACY_ALLOW_RTK, Op, REPLACEABLE_TAGS, SettingsLimits, TAG_PLUGIN_READ, TAG_TOKEN_LAYER,
};
use crate::plan::{apply, plan};
use crate::revert::revert_with_diff;
use crate::scanner::pyval::{check_hashable, get, getitem, py_eq, py_list};

/// `ops_version(ops)`: `"<OPS_VERSION>-<16 hex>"`, a fingerprint of the op set (GC-8). The
/// plugin-read rule's value is left out: it follows where the plugin lives.
pub fn ops_version(ops: &[Op], limits: &SettingsLimits) -> String {
    let canon: Vec<Value> = ops
        .iter()
        .map(|op| {
            let op = if op.tag() == Some(TAG_PLUGIN_READ) {
                op.clone().with_value(Value::Str("<plugin root>".into()))
            } else {
                op.clone()
            };
            op.to_value()
        })
        .collect();
    let payload = json::dumps(
        &Value::Array(vec![
            Value::Int(Int::Small(limits.ops_version)),
            Value::Array(canon),
        ]),
        &COMPACT_SORTED,
    );
    let digest = Sha256::digest(payload.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}", limits.ops_version, &hex[..16])
}

/// `_key(op)`: `(op["op"], tuple(op["path"]), json.dumps(op.get("value"), sort_keys=True))`,
/// hashed (a list or dict inside is a `TypeError`).
struct Key {
    op: Value,
    path: Vec<Value>,
    value: String,
}

fn key(o: &Value) -> Result<Key, String> {
    let op = getitem(o, "op")?.clone();
    let path = py_list(getitem(o, "path")?)?;
    let value = json::dumps(get(o, "value")?.unwrap_or(&Value::Null), &PY_DEFAULT_SORTED);
    check_hashable(&op)?;
    for p in &path {
        check_hashable(p)?;
    }
    Ok(Key { op, path, value })
}

fn key_eq(a: &Key, b: &Key) -> bool {
    py_eq(&a.op, &b.op)
        && a.path.len() == b.path.len()
        && a.path.iter().zip(&b.path).all(|(x, y)| py_eq(x, y))
        && a.value == b.value
}

/// Whether a stored op is the allow rule releases before the rename recorded: an untagged `add`
/// of [`LEGACY_ALLOW_RTK`] to `permissions.allow`. Only an op setup recorded counts, so a rule
/// the user wrote themselves (never in the record) is not touched.
fn is_legacy_allow(o: &Value) -> Result<bool, String> {
    let (Some(op), Some(path), Some(value)) = (get(o, "op")?, get(o, "path")?, get(o, "value")?)
    else {
        return Ok(false);
    };
    let s = |x: &str| Value::Str(x.into());
    Ok(py_eq(op, &s("add"))
        && py_eq(path, &Value::Array(vec![s("permissions"), s("allow")]))
        && py_eq(value, &s(LEGACY_ALLOW_RTK)))
}

/// `reconcile(settings, desired, previous)`: re-run setup over text that may carry an earlier
/// run's entries. Returns `(new_text, diff, ops_to_store)`. A plugin-read, memory-save or ATL
/// rule added earlier whose value is no longer wanted is replaced (an earlier untagged add of
/// the legacy rtk rule counts as the ATL rule); every other earlier op stays in the record, so
/// remove still undoes everything setup ever added.
pub fn reconcile(
    text: &str,
    desired: &[Op],
    previous: &[Value],
) -> Result<(String, String, Vec<Value>), String> {
    // want = {(tag, value) for desired ops with a replaceable tag} (hashing the value).
    let mut want: Vec<(&str, &Value)> = Vec::new();
    for o in desired {
        if let Some(t) = o.tag().filter(|t| REPLACEABLE_TAGS.contains(t)) {
            check_hashable(o.value())?;
            want.push((t, o.value()));
        }
    }
    let mut stale_idx = Vec::new();
    for (i, o) in previous.iter().enumerate() {
        let tag = get(o, "tag")?;
        let t = match tag {
            Some(Value::Str(t)) => t.as_str(),
            None if is_legacy_allow(o)? => TAG_TOKEN_LAYER,
            _ => continue,
        };
        if !REPLACEABLE_TAGS.contains(&t) {
            continue;
        }
        let v = getitem(o, "value")?;
        check_hashable(v)?;
        if !want.iter().any(|(wt, wv)| *wt == t && py_eq(wv, v)) {
            stale_idx.push(i);
        }
    }
    let stale: Vec<Value> = stale_idx.iter().map(|&i| previous[i].clone()).collect();
    let text1 = if stale.is_empty() {
        text.to_string()
    } else {
        revert_with_diff(text, &stale)?.0
    };
    // `o not in stale`: an op picked as stale is in it by identity, and any other op equal to
    // a stale one is too.
    let kept: Vec<&Value> = previous
        .iter()
        .enumerate()
        .filter(|(i, o)| !stale_idx.contains(i) && !stale.iter().any(|s| py_eq(o, s)))
        .map(|(_, o)| o)
        .collect();
    let new = plan(&text1, desired)?;
    let (text2, _) = apply(&text1, &new.ops)?;
    let new_keys = new.stored.iter().map(key).collect::<Result<Vec<_>, _>>()?;
    let mut stored = Vec::new();
    for o in kept {
        let k = key(o)?;
        if !new_keys.iter().any(|n| key_eq(&k, n)) {
            stored.push(o.clone());
        }
    }
    stored.extend(new.stored);
    let diff = diff_texts(text, &text2)?;
    Ok((text2, diff, stored))
}
