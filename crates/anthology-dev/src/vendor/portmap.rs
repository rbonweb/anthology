//! Port maps (vendor.py `load_portmap`, `portmap_scripts`, `portmap_problems`): the script rule
//! applied to the pristine and the shipped tree, and the Chrome-launch pattern.
//!
//! `js_node_regex` is a str pattern: it goes through the Python → fancy-regex translation
//! (plan GC-16; today it has no lookaround, which `tests/vendor/portmap_test.rs` records).
//! `chrome_regex` is compiled as bytes with IGNORECASE, as vendor.py does: fancy-regex's ASCII
//! bytes mode (ASCII classes and case folding, any byte matched).

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::Path;

use anthology_pycompat::json::{self, Value};

use super::compare::{exists, is_file, os_err, os_walk, rel_posix, sort_names};
use super::manifest::type_name;
use super::py;
use super::transforms::suffix;
use super::{Entry, Result, VendorError};

/// `data[k]` as Python evaluates it: the value, or the KeyError/TypeError text.
fn subscript(data: &Value, k: &str) -> std::result::Result<Value, String> {
    match data {
        Value::Object(m) => m.get(k).cloned().ok_or_else(|| py::repr_str(k)),
        Value::Array(_) => Err("list indices must be integers or slices, not str".to_string()),
        Value::Str(_) => Err("string indices must be integers, not 'str'".to_string()),
        other => Err(format!(
            "'{}' object is not subscriptable",
            type_name(other)
        )),
    }
}

/// `re.compile(pattern)` of a JSON value: a str compiles, a list or dict is unhashable (the
/// cache lookup fails first), anything else is not a pattern.
fn compile_value(pattern: &Value) -> std::result::Result<(), String> {
    match pattern {
        Value::Str(text) => py::compile_str(text).map(|_| ()),
        Value::Array(_) | Value::Object(_) => {
            Err(format!("unhashable type: '{}'", type_name(pattern)))
        }
        _ => Err("first argument must be string or compiled pattern".to_string()),
    }
}

/// `load_portmap(root, entry)`: the entry's port map, checked for the keys the rules need.
/// Every check runs in vendor.py's order, so the first failure gives Python's reason.
pub fn load_portmap(root: &Path, entry: &Entry) -> Result<json::Map> {
    let rel = py::str_of(entry.get("portmap"));
    let name = py::str_of(entry.get("name"));
    let path = root.join(&rel);
    if !is_file(&path) {
        return Err(VendorError(format!("{name}: port map {rel} not found")));
    }
    let unusable =
        |why: String| VendorError(format!("{name}: port map {rel} is not usable: {why}"));
    let text = py::read_text(&path).map_err(unusable)?;
    let data = json::loads(&text).map_err(|e| unusable(e.py_str(&text)))?;
    let rule = subscript(&data, "script_rule").map_err(unusable)?;
    let scripts = subscript(&data, "scripts").map_err(unusable)?;
    let wrong = || unusable("scripts, added or extensions have the wrong type".to_string());
    // `isinstance(data["scripts"], dict) and isinstance(data.get("added", {}), dict) and
    // isinstance(rule["extensions"], list)`, short-circuiting (`data` is a dict by now).
    let map = data.as_object().ok_or_else(wrong)?;
    if scripts.as_object().is_none() || map.get("added").is_some_and(|v| v.as_object().is_none()) {
        return Err(wrong());
    }
    if subscript(&rule, "extensions")
        .map_err(unusable)?
        .as_array()
        .is_none()
    {
        return Err(wrong());
    }
    compile_value(&subscript(&rule, "js_node_regex").map_err(unusable)?).map_err(unusable)?;
    // vendor.py compiles `chrome_regex` as a str here; `portmap_problems` compiles it as bytes.
    compile_value(&subscript(&data, "chrome_regex").map_err(unusable)?).map_err(unusable)?;
    Ok(map.clone())
}

fn bad_map(what: &str) -> VendorError {
    VendorError(format!("the port map is not usable: {what}"))
}

/// `portmap_scripts(tree, rule)`: the files the script rule counts as scripts — a listed
/// extension (any case), a `#!` first line, or a `.js` file that reads like Node.
pub fn portmap_scripts(tree: &Path, rule: &json::Map) -> Result<BTreeSet<String>> {
    let extensions: BTreeSet<String> = rule
        .get("extensions")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_map("extensions"))?
        .iter()
        .map(|e| py::lower(&py::str_of(Some(e))))
        .collect();
    let pattern = rule
        .get("js_node_regex")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_map("js_node_regex"))?;
    let node_js = py::compile_str(pattern).map_err(|e| bad_map(&e))?;
    let shebang = py::truthy(rule.get("shebang"));
    let mut found = BTreeSet::new();
    os_walk(tree, |w| {
        w.dirs.retain(|d| d != ".git");
        sort_names(&mut w.dirs);
        for name in &w.files {
            let path = w.path.join(name);
            let rel = rel_posix(&path, tree);
            let ext = py::lower(suffix(&name.to_string_lossy()));
            if extensions.contains(&ext) {
                found.insert(rel);
                continue;
            }
            let raw = if ext == ".js" {
                fs::read(&path)
            } else {
                fs::File::open(&path).and_then(|f| {
                    let mut head = Vec::new();
                    f.take(2).read_to_end(&mut head).map(|_| head)
                })
            };
            let Ok(raw) = raw else { continue };
            let script = (shebang && raw.starts_with(b"#!"))
                || (ext == ".js"
                    && node_js
                        .is_match(String::from_utf8_lossy(&raw).as_ref())
                        .map_err(|e| VendorError(format!("js_node_regex: regex limit ({e})")))?);
            if script {
                found.insert(rel);
            }
        }
        Ok(())
    })?;
    Ok(found)
}

/// `portmap_problems(portmap, pristine, shipped)`: a built tree against its port map;
/// `pristine` is the upstream folder before any patch. Every problem is one line.
pub fn portmap_problems(
    portmap: &json::Map,
    pristine: &Path,
    shipped: &Path,
) -> Result<Vec<String>> {
    let empty = json::Map::new();
    let rule = portmap
        .get("script_rule")
        .and_then(Value::as_object)
        .ok_or_else(|| bad_map("script_rule"))?;
    let scripts = portmap
        .get("scripts")
        .and_then(Value::as_object)
        .ok_or_else(|| bad_map("scripts"))?;
    let added = match portmap.get("added") {
        None => &empty,
        Some(v) => v.as_object().ok_or_else(|| bad_map("added"))?,
    };
    let mut problems = Vec::new();
    let upstream = portmap_scripts(pristine, rule)?;
    let listed: BTreeSet<String> = scripts.iter().map(|(k, _)| k.to_string()).collect();
    for rel in upstream.difference(&listed) {
        problems.push(format!(
            "{rel}: upstream script is not in the port map (add it as keep, port or drop)"
        ));
    }
    for rel in listed.difference(&upstream) {
        problems.push(format!(
            "{rel}: the port map lists it, but it is not upstream"
        ));
    }
    let mut expected: BTreeSet<String> = added.iter().map(|(k, _)| k.to_string()).collect();
    let mut dropped = BTreeSet::new();
    let mut sorted: Vec<(&str, &Value)> = scripts.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (rel, spec) in sorted {
        let action = spec
            .as_object()
            .and_then(|m| m.get("action"))
            .and_then(Value::as_str);
        match action {
            Some("drop") => {
                if exists(&shipped.join(rel)) {
                    dropped.insert(rel.to_string());
                    problems.push(format!("{rel}: drop script is shipped"));
                }
            }
            Some(action @ ("keep" | "port")) => {
                expected.insert(rel.to_string());
                if !is_file(&shipped.join(rel)) {
                    problems.push(format!("{rel}: {action} script is not shipped"));
                } else if action == "keep" {
                    // An unreadable file raises in Python (D-1): an error here too.
                    let read = |p: std::path::PathBuf| fs::read(&p).map_err(|e| os_err(&e, &p));
                    let same = is_file(&pristine.join(rel))
                        && read(shipped.join(rel))? == read(pristine.join(rel))?;
                    if !same {
                        problems.push(format!(
                            "{rel}: keep script is not byte-identical to upstream"
                        ));
                    }
                }
            }
            _ => problems.push(format!("{rel}: action must be keep, port or drop")),
        }
    }
    let mut added_names: Vec<&str> = added.iter().map(|(k, _)| k).collect();
    added_names.sort();
    for rel in added_names {
        if !is_file(&shipped.join(rel)) {
            problems.push(format!("{rel}: added script is not shipped"));
        }
        if upstream.contains(rel) || exists(&pristine.join(rel)) {
            problems.push(format!("{rel}: added script already exists upstream"));
        }
    }
    let have = portmap_scripts(shipped, rule)?;
    for rel in have
        .iter()
        .filter(|r| !expected.contains(*r) && !dropped.contains(*r))
    {
        problems.push(format!(
            "{rel}: shipped script is not keep, port or added in the port map"
        ));
    }
    for rel in expected.difference(&have) {
        if is_file(&shipped.join(rel)) {
            problems.push(format!(
                "{rel}: shipped, but the script rule does not count it as a script"
            ));
        }
    }
    let chrome_pattern = portmap
        .get("chrome_regex")
        .and_then(Value::as_str)
        .ok_or_else(|| bad_map("chrome_regex"))?;
    let chrome = py::compile_bytes_ignorecase(chrome_pattern).map_err(|e| bad_map(&e))?;
    let limit = |e: fancy_regex::Error| VendorError(format!("chrome_regex: regex limit ({e})"));
    os_walk(shipped, |w| {
        sort_names(&mut w.dirs);
        sort_names(&mut w.files);
        for name in &w.files {
            let path = w.path.join(name);
            let rel = rel_posix(&path, shipped);
            if chrome.is_match(rel.as_bytes()).map_err(limit)? {
                problems.push(format!("{rel}: the path matches the Chrome launch pattern"));
            }
            let Ok(data) = fs::read(&path) else { continue };
            if let Some(m) = chrome.find(&data[..]).map_err(limit)? {
                problems.push(format!(
                    "{rel}: the file matches the Chrome launch pattern ({})",
                    py::repr_str(&String::from_utf8_lossy(m.as_bytes()))
                ));
            }
        }
        Ok(())
    })?;
    Ok(problems)
}
