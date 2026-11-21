//! `redacted_diff` (ST1), with `redact`: `settings_patch.py`'s `_hidden_leaves`, `redact` and
//! `_diff` (GC-8, F4 redaction).
//!
//! A diff shows values only under `permissions.allow`, `permissions.deny` and
//! `worktree.baseRef` (all of them, the user's own rules included). Every other string or
//! scalar reads `"<redacted>"`, in any layout; keys, layout and line count are kept.

use crate::ops::{REDACTED, SHOWN_PATHS};
use crate::scanner::py::{splitlines, unified_diff};
use crate::scanner::{Doc, Kind, is_blank, parse};

/// `_hidden_leaves(node, path, out)`: the string and scalar nodes outside `SHOWN_PATHS` (an
/// array item adds `[]` to the path). The document is at most `_MAX_DEPTH` deep.
fn hidden_leaves(doc: &Doc, node: usize, path: &mut Vec<String>, out: &mut Vec<usize>) {
    if SHOWN_PATHS.iter().any(|shown| {
        shown.len() == path.len() && shown.iter().zip(path.iter()).all(|(a, b)| a == b)
    }) {
        return;
    }
    let n = doc.node(node);
    match n.kind {
        Kind::Object => {
            for m in &n.members {
                path.push(m.key.clone());
                hidden_leaves(doc, m.value, path, out);
                path.pop();
            }
        }
        Kind::Array => {
            for &item in &n.items {
                path.push("[]".into());
                hidden_leaves(doc, item, path, out);
                path.pop();
            }
        }
        Kind::Str | Kind::Scalar => out.push(node),
    }
}

/// `redact(text)`: `text` with every value outside `SHOWN_PATHS` replaced by
/// `"<redacted>"`. Keys, layout and line count are kept.
pub fn redact(text: &str) -> Result<String, String> {
    if is_blank(text) {
        return Ok(text.to_string());
    }
    let doc = parse(text)?;
    let mut leaves = Vec::new();
    hidden_leaves(&doc, Doc::ROOT, &mut Vec::new(), &mut leaves);
    leaves.sort_by_key(|&n| std::cmp::Reverse(doc.node(n).start));
    let new = format!("\"{REDACTED}\"");
    let mut out = text.to_string();
    for n in leaves {
        let nd = doc.node(n);
        out.replace_range(nd.start..nd.end, &new);
    }
    Ok(out)
}

/// `_diff(old, new)`: the unified diff shown to the user (and Claude), redacted on both sides:
/// `difflib.unified_diff(…, "settings.json", "settings.json", lineterm="", n=0)` joined by
/// `\n`; empty when the texts are equal. An error is what Python's `_diff` raises (a text that
/// is neither blank nor one JSON object).
pub(crate) fn diff_texts(before: &str, after: &str) -> Result<String, String> {
    if before == after {
        return Ok(String::new());
    }
    let (a, b) = (redact(before)?, redact(after)?);
    let lines = unified_diff(
        &splitlines(&a),
        &splitlines(&b),
        "settings.json",
        "settings.json",
        0,
    );
    Ok(lines.join("\n"))
}

/// The redacted unified diff of two settings texts (plan §3.6). Both must be texts `plan`,
/// `apply`, `revert` or `reconcile` accepted or produced (blank, or one JSON object); for any
/// other text it is empty. `apply`, `revert_with_diff` and `reconcile` return the same diff and
/// report such a text as an error, as Python's `_diff` raises.
pub fn redacted_diff(before: &str, after: &str) -> String {
    diff_texts(before, after).unwrap_or_default()
}
