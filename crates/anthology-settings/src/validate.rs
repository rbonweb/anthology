//! Validation and `plugin_rule_status` (ST1): `settings_patch.py`'s `validate_plugin_root`
//! (F21) and `plugin_rule_status`.

use std::path::Path;

use anthology_core::Env;
use anthology_core::paths::home_raw;
use anthology_pycompat::json::{self, Value};

use crate::ops::plugin_read_rule;
use crate::render::value_depth;
use crate::scanner::{is_blank, py, strip_bom};
use crate::write::{read_text, settings_path};

/// `_RULE_META`: characters permission rules read as a pattern.
const RULE_META: &[char] = &['*', '?', '[', ']', '{', '}', '(', ')', '\\'];

/// A manifest nested deeper than this is refused as Python's `json.load` would crash on it
/// (`RecursionError`, which `validate_plugin_root` does not catch, so setup stops). CPython
/// 3.12.16 gives up near 10,000 levels; this bound is far below, so Rust refuses every manifest
/// Python crashes on, and also the absurd ones between (lead ruling; no plugin.json nests).
pub(crate) const MAX_MANIFEST_DEPTH: usize = 1000;

/// `json.load(open(<real>/.claude-plugin/plugin.json, encoding="utf-8")).get("name")`, with
/// `OSError`, `ValueError` and `AttributeError` read as `None`. A manifest too deep to load is
/// an error, as Python's `RecursionError` is.
fn manifest_name(real: &str) -> Result<Option<String>, String> {
    let path = if real == "/" {
        "/.claude-plugin/plugin.json".to_string()
    } else {
        format!("{real}/.claude-plugin/plugin.json")
    };
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(None);
    };
    // Text mode, utf-8 (not utf-8-sig): a BOM reaches json.loads and is refused.
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    let Ok(data) = json::loads(&text) else {
        return Ok(None);
    };
    if value_depth(&data) >= MAX_MANIFEST_DEPTH {
        return Err(
            "RecursionError: maximum recursion depth exceeded while decoding a JSON object from a unicode string"
                .into(),
        );
    }
    Ok(data
        .as_object()
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string))
}

/// `validate_plugin_root(plugin_root, env)` (F21): refuse a plugin root the plugin-read allow
/// rule cannot safely be built from.
pub fn validate_plugin_root(plugin_root: &Path, env: &Env) -> Result<(), String> {
    let raw = plugin_root.to_string_lossy();
    if raw.is_empty() || !raw.starts_with('/') {
        return Err(format!(
            "the plugin root must be an absolute path (got {})",
            py::repr_str(&raw)
        ));
    }
    let real = py::realpath(&raw);
    if !Path::new(&real).is_dir() {
        return Err(format!("the plugin root {real} is not a folder"));
    }
    if manifest_name(&real)?.as_deref() != Some("anthology") {
        return Err(format!(
            "{real} is not the anthology plugin (no .claude-plugin/plugin.json named anthology)"
        ));
    }
    let parent = py::dirname(&real);
    let home = py::realpath_os(home_raw(env).as_os_str())
        .to_string_lossy()
        .into_owned();
    if parent == "/"
        || home == parent
        || home.starts_with(&format!("{}/", parent.trim_end_matches('/')))
    {
        return Err(format!(
            "refusing to allow reads of {parent}: it is the home folder, one of its parents or the root, so the plugin must live in its own folder"
        ));
    }
    if real.contains(RULE_META) {
        return Err(format!(
            "the plugin path {real} has a character (* ? [ ] {{ }} ( ) or a backslash) that permission rules would read as a pattern"
        ));
    }
    Ok(())
}

/// `plugin_rule_status(plugin_root, env)`: `"ok"`, `"missing"` (not in `permissions.allow`)
/// or `"unreadable"` for the plugin-read rule in the user's settings.json.
pub fn plugin_rule_status(env: &Env, plugin_root: &Path) -> &'static str {
    let text = match read_text(&settings_path(env)) {
        Ok(t) => t.unwrap_or_default(),
        Err(_) => return "unreadable",
    };
    if is_blank(&text) {
        return "missing";
    }
    let data = match json::loads(strip_bom(&text)) {
        Ok(d) => d,
        Err(_) => return "unreadable",
    };
    let allow = data
        .as_object()
        .and_then(|d| d.get("permissions"))
        .and_then(Value::as_object)
        .and_then(|p| p.get("allow"));
    if let Some(Value::Array(items)) = allow {
        let rule = plugin_read_rule(plugin_root);
        if items.iter().any(|i| i.as_str() == Some(rule.as_str())) {
            return "ok";
        }
    }
    "missing"
}
