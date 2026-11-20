//! The transforms (vendor.py `rename_plugin`, `strip_connectors`, `rename_skill`).
//!
//! Three patterns have lookaround and run on fancy-regex (plan GC-16): the `rename-plugin`
//! token (a bytes pattern, so fancy-regex's ASCII bytes mode), `IF_TOOLS_CONNECTED` and the
//! `rename-skill` body pattern. A backtrack-limit error fails the transform with the pattern's
//! name (GC-16: dev tools exit 1).

use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use anthology_pycompat::json::{self, Value};
use fancy_regex::{BytesMode, RegexBuilder};
use regex::Regex;

use super::compare::{is_file, is_symlink, os_err, rel_posix, rglob};
use super::py::{self, Piece, S, W};
use super::{Entry, Result, VendorError};

/// The transform names (vendor.py `TRANSFORMS`).
pub const TRANSFORMS: [&str; 3] = ["rename-plugin", "strip-connectors", "rename-skill"];

/// The file `strip-connectors` deletes, and must not see referenced afterwards.
pub const CONNECTORS_FILE: &str = "CONNECTORS.md";

/// Runs the transform `spec` names on `tree` (`TRANSFORMS[spec["name"]](tree, entry, spec)`);
/// `validate_sources` has checked the spec.
pub fn run_transform(tree: &Path, entry: &Entry, spec: &json::Map) -> Result<json::Map> {
    let field = |k: &str| spec.get(k).and_then(Value::as_str).unwrap_or("");
    match field("name") {
        "rename-plugin" => {
            let null = Value::Null;
            let version = entry.get("plugin_version").unwrap_or(&null);
            rename_plugin(tree, field("old"), field("new"), version)
        }
        "strip-connectors" => strip_connectors(tree),
        "rename-skill" => rename_skill(tree, field("old"), field("new")),
        other => Err(VendorError(format!("KeyError: {}", py::repr_str(other)))),
    }
}

fn limit(name: &str, e: &fancy_regex::Error) -> VendorError {
    VendorError(format!("{name}: regex limit ({e})"))
}

/// Python's `re.escape` is not needed for matching: an escaped literal matches the same text.
fn escape_bytes(b: &[u8]) -> String {
    let mut out = String::new();
    for &c in b {
        if c.is_ascii_alphanumeric() || c == b'_' {
            out.push(char::from(c));
        } else {
            out.push_str(&format!("\\x{c:02x}"));
        }
    }
    out
}

/// `rename_plugin(tree, old, new, plugin_version)`: rename a Claude Code plugin and the
/// `old:<skill>` references to its skills.
///
/// - `.claude-plugin/plugin.json`: `name` → new, `version` → plugin_version.
/// - `.claude-plugin/marketplace.json` (upstream's dev marketplace) is deleted.
/// - Every text file (no NUL byte): the token `old:` followed by `[a-z0-9-]` becomes `new:`,
///   unless it is part of a longer name, a path or URL (`/old:`, `.old:`), or not directly
///   followed by a skill name.
pub fn rename_plugin(
    tree: &Path,
    old: &str,
    new: &str,
    plugin_version: &Value,
) -> Result<json::Map> {
    let manifest = tree.join(".claude-plugin").join("plugin.json");
    if !is_file(&manifest) {
        return Err(VendorError::new(
            "rename-plugin: .claude-plugin/plugin.json not found",
        ));
    }
    let text = py::read_text(&manifest).map_err(VendorError)?;
    let mut data = json::loads(&text).map_err(|e| {
        VendorError(format!(
            "rename-plugin: plugin.json: {}: {}",
            e.type_name(),
            e.py_str(&text)
        ))
    })?;
    let Some(map) = data.as_object_mut() else {
        return Err(VendorError::new(
            "rename-plugin: plugin.json is not an object (AttributeError)",
        ));
    };
    if map.get("name").and_then(Value::as_str) != Some(old) {
        return Err(VendorError(format!(
            "rename-plugin: plugin.json name is {}, expected {}",
            py::repr_opt(map.get("name")),
            py::repr_str(old)
        )));
    }
    map.insert("name".to_string(), Value::Str(new.to_string()));
    map.insert("version".to_string(), plugin_version.clone());
    let opts = json::DumpOpts {
        ensure_ascii: false,
        ..json::INDENT2
    };
    let out = json::dumps(&data, &opts) + "\n";
    fs::write(&manifest, out).map_err(|e| os_err(&e, &manifest))?;
    let market = tree.join(".claude-plugin").join("marketplace.json");
    match fs::remove_file(&market) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(os_err(&e, &market)),
        _ => {}
    }

    // rb"(?<![\w./-])" + re.escape(old.encode()) + rb":(?=[a-z0-9-])", ASCII \w.
    let pattern = format!(
        r"(?<![0-9A-Za-z_./\-]){}:(?=[a-z0-9\-])",
        escape_bytes(old.as_bytes())
    );
    let token = RegexBuilder::new(&pattern)
        .bytes_mode(BytesMode::Ascii)
        .build()
        .map_err(|e| VendorError(format!("rename-plugin: bad token pattern: {e}")))?;
    let mut replacement = new.as_bytes().to_vec();
    replacement.push(b':');
    let mut template: Option<Vec<Piece>> = None;
    let (mut files, mut references) = (0i64, 0i64);
    for path in rglob(tree) {
        if !is_file(&path) || is_symlink(&path) {
            continue;
        }
        let raw = fs::read(&path).map_err(|e| os_err(&e, &path))?;
        if raw.contains(&0) {
            continue;
        }
        if template.is_none() {
            let parsed = py::parse_template(&py::latin1(&replacement))
                .map_err(|e| VendorError(format!("rename-plugin: re.error: {e}")))?;
            template = Some(parsed);
        }
        let pieces = template.as_deref().unwrap_or(&[]);
        let mut changed = Vec::with_capacity(raw.len());
        let mut last = 0;
        let mut count = 0i64;
        for m in token.find_iter(&raw[..]) {
            let m = m.map_err(|e| limit("rename-plugin token", &e))?;
            changed.extend_from_slice(&raw[last..m.start()]);
            let whole = py::latin1(m.as_bytes());
            changed.extend(py::unlatin1(&py::expand(pieces, &whole)));
            last = m.end();
            count += 1;
        }
        if count > 0 {
            changed.extend_from_slice(&raw[last..]);
            fs::write(&path, &changed).map_err(|e| os_err(&e, &path))?;
            files += 1;
            references += count;
        }
    }
    Ok(py::stats(&[("files", files), ("references", references)]))
}

// ------------------------------------------------------------------------- strip-connectors

/// The patterns of `strip_connectors`, compiled once.
struct Patterns {
    connector_heading: Regex,
    md_heading: Regex,
    md_fence: Regex,
    fallback: Regex,
    list_marker: Regex,
    if_placeholder_connected: Regex,
    if_tools_connected: fancy_regex::Regex,
    list_if_connected: Regex,
    connect_via_mcp: Regex,
    supercharged_box: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let re = |p: String| Regex::new(&p).unwrap_or_else(|e| unreachable!("{p}: {e}"));
        Patterns {
            // Connector setup sections: "Connectors", "If Connectors Available", "Tool
            // Integration", "Connect <Product> (optional)", "... Connected Tools".
            connector_heading: re(format!(
                r"(?i)\A(?:if connectors available|connectors?|tool integrations?|connect {W}+(?: \(optional\))?|.*\bconnected tools)\n?\z"
            )),
            md_heading: re(r"\A(#{1,6})[ \t]+(.*?)(?:[ \t]+#+)?\n?\z".to_string()),
            md_fence: re(format!(r"\A{S}*(`{{3,}}|~{{3,}})(.*)\n?\z")),
            // What stays when the connector setup around it goes.
            fallback: re(format!(
                r"(?i)\A{S}*(?:[-*+]{S}+)?(?:\*\*)?(?:if{S}+(?:no|none|neither)\b.*\bconnected\b|if\b.*\bnot{S}+connected\b|without\b.*\btools?\b)"
            )),
            list_marker: re(format!(r"\A{S}*(?:[-*+]|\d+[.)]){S}")),
            if_placeholder_connected: re(format!(r"\A{S}*(?:\*\*)?If\b.*~~.*\bconnected\b")),
            if_tools_connected: fancy_regex::Regex::new(&format!(
                r"\A{S}*(?:[-*+]{S}+)?(?:\*\*)?If{S}+(?!no\b).*\btools?{S}+(?:is|are|weren't|were){S}+connected\b"
            ))
            .unwrap_or_else(|e| unreachable!("IF_TOOLS_CONNECTED: {e}")),
            list_if_connected: re(format!(r"\A{S}*[-*+]{S}.*~~.*\(if connected\)")),
            connect_via_mcp: re(format!(r"\A{S}*[-*+]{S}.*\bconnect\b.*\bMCP\b")),
            supercharged_box: re(r"SUPERCHARGED.*\bconnect\b".to_string()),
        }
    })
}

/// `MD_HEADING.match(line)`: `(hashes, text)`.
fn heading(line: &str) -> Option<(usize, &str)> {
    let c = patterns().md_heading.captures(line)?;
    Some((
        c.get(1)?.as_str().len(),
        c.get(2).map_or("", |m| m.as_str()),
    ))
}

/// `MD_FENCE.match(line)`: `(marker, rest)`.
fn fence(line: &str) -> Option<(&str, &str)> {
    let c = patterns().md_fence.captures(line)?;
    Some((c.get(1)?.as_str(), c.get(2).map_or("", |m| m.as_str())))
}

fn is_blank(s: &str) -> bool {
    py::strip(s).is_empty()
}

fn is_connector_heading(text: &str) -> bool {
    let p = patterns();
    if p.fallback.is_match(text) {
        return false;
    }
    p.connector_heading.is_match(py::strip(text)) || text.contains("~~")
}

/// `_fence_end`: the index of the line closing the fence opened at `start` (the last line if
/// unclosed).
fn fence_end(lines: &[&str], start: usize, marker: &str) -> usize {
    let first = marker.chars().next();
    for (j, line) in lines.iter().enumerate().skip(start + 1) {
        if let Some((m, rest)) = fence(py::rstrip(line))
            && m.chars().next() == first
            && m.len() >= marker.len()
            && is_blank(rest)
        {
            return j;
        }
    }
    lines.len().saturating_sub(1)
}

/// `_section_end`: the next heading (outside code fences) at `level` or higher, else the end.
fn section_end(lines: &[&str], start: usize, level: usize) -> usize {
    let mut j = start + 1;
    while j < lines.len() {
        let line = py::rstrip(lines[j]);
        if let Some((m, _)) = fence(line) {
            j = fence_end(lines, j, m) + 1;
            continue;
        }
        if let Some((hashes, _)) = heading(line)
            && hashes <= level
        {
            return j;
        }
        j += 1;
    }
    lines.len()
}

/// `_if_block_end`: a list item is one line; a lead-in ending in a colon owns the list under it
/// (also after one blank line); anything else is its paragraph.
fn if_block_end(lines: &[&str], i: usize) -> usize {
    let p = patterns();
    let line = py::rstrip(lines[i]);
    if p.list_marker.is_match(line) {
        return i + 1;
    }
    let mut j = i + 1;
    if (line.ends_with(':') || line.ends_with(":**"))
        && j + 1 < lines.len()
        && is_blank(lines[j])
        && p.list_marker.is_match(lines[j + 1])
    {
        j += 1;
    }
    while j < lines.len() && !is_blank(lines[j]) {
        j += 1;
    }
    j
}

/// `_fallbacks_in`: the fallback text inside a connector section that is about to be removed.
fn fallbacks_in<'a>(
    lines: &[&'a str],
    start: usize,
    end: usize,
    level: usize,
) -> Vec<Vec<&'a str>> {
    let p = patterns();
    let mut chunks = Vec::new();
    let mut j = start + 1;
    while j < end {
        let line = py::rstrip(lines[j]);
        if let Some((m, _)) = fence(line) {
            j = fence_end(lines, j, m) + 1;
            continue;
        }
        let h = heading(line);
        let stop = match h {
            Some((hashes, text)) if hashes > level && p.fallback.is_match(text) => {
                section_end(lines, j, hashes).min(end)
            }
            None if p.fallback.is_match(line) => if_block_end(lines, j).min(end),
            _ => {
                j += 1;
                continue;
            }
        };
        let mut chunk: Vec<&str> = lines[j..stop].to_vec();
        while chunk.last().is_some_and(|l| is_blank(l)) {
            chunk.pop();
        }
        chunks.push(chunk);
        j = stop;
    }
    chunks
}

/// `_strip_markdown(text)`: the text with its connector markup removed (see
/// [`strip_connectors`]); the text itself when nothing goes.
pub fn strip_markdown(text: &str) -> Result<String> {
    let p = patterns();
    let lines: Vec<&str> = text.split('\n').collect();
    let n = lines.len();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;

    // A removed block takes its trailing blank lines unless text stands right before it.
    let skip_blanks = |out: &[&str], mut j: usize| -> usize {
        if out.last().is_some_and(|l| !is_blank(l)) {
            return j;
        }
        while j < n && is_blank(lines[j]) {
            j += 1;
        }
        j
    };
    let trim_blanks = |out: &[&str]| -> usize {
        let mut k = out.len();
        while k > 0 && is_blank(out[k - 1]) {
            k -= 1;
        }
        k
    };

    while i < n {
        let line = lines[i];
        let probe = py::rstrip(line);
        if let Some((marker, _)) = fence(probe) {
            let end = fence_end(&lines, i, marker);
            let block = &lines[i..=end.max(i).min(n - 1)];
            if !block.iter().any(|l| p.supercharged_box.is_match(l)) {
                out.extend_from_slice(block);
                i = end + 1;
                continue;
            }
            // The "STANDALONE / SUPERCHARGED (when you connect your tools)" box goes, with a
            // line that only introduces it and a heading it leaves without any content.
            let k = trim_blanks(&out);
            if k > 0 {
                let above = py::rstrip(out[k - 1]);
                if above.ends_with(':') && heading(above).is_none() {
                    out.truncate(k - 1);
                }
            }
            let mut j = end + 1;
            while j < n && is_blank(lines[j]) {
                j += 1;
            }
            let k = trim_blanks(&out);
            let above = if k > 0 {
                heading(py::rstrip(out[k - 1]))
            } else {
                None
            };
            let below = if j < n {
                heading(py::rstrip(lines[j]))
            } else {
                None
            };
            if let Some((above_level, _)) = above
                && (j >= n || below.is_some_and(|(b, _)| b <= above_level))
            {
                out.truncate(k - 1);
            }
            i = if out.last().is_some_and(|l| is_blank(l)) {
                j
            } else {
                end + 1
            };
            continue;
        }
        if let Some((level, text)) = heading(probe)
            && is_connector_heading(text)
        {
            let end = section_end(&lines, i, level);
            let kept = fallbacks_in(&lines, i, end, level);
            if !kept.is_empty() {
                // The section goes, its fallbacks stay under the section's own heading.
                out.push(line);
                out.push("");
                for chunk in kept {
                    out.extend(chunk);
                    out.push("");
                }
            }
            i = end;
            continue;
        }
        if py::lstrip(probe).starts_with('>') {
            let mut j = i;
            while j < n && py::lstrip(lines[j]).starts_with('>') {
                j += 1;
            }
            if lines[i..j].iter().any(|l| l.contains(CONNECTORS_FILE)) {
                i = skip_blanks(&out, j);
                continue;
            }
        }
        let tools = p
            .if_tools_connected
            .is_match(probe)
            .map_err(|e| limit("strip-connectors IF_TOOLS_CONNECTED", &e))?;
        if p.if_placeholder_connected.is_match(probe) || tools {
            i = skip_blanks(&out, if_block_end(&lines, i));
            continue;
        }
        if p.list_if_connected.is_match(probe) || p.connect_via_mcp.is_match(probe) {
            i += 1;
            continue;
        }
        out.push(line);
        i += 1;
    }
    if out == lines {
        return Ok(text.to_string());
    }
    while out.last().is_some_and(|l| is_blank(l)) {
        out.pop();
    }
    Ok(out.join("\n") + "\n")
}

/// `strip_connectors(tree)`: remove everything that tells an agent to use connected tools.
///
/// In every `.md` file, outside code fences: connector setup sections, `If ... connected`
/// blocks, `~~<category> (if connected)` list items, "connect via MCP" list items, the
/// SUPERCHARGED box and blockquotes pointing to `CONNECTORS.md`. A file named `CONNECTORS.md`
/// is deleted. A `~~` placeholder or `CONNECTORS.md` reference left over is an error.
pub fn strip_connectors(tree: &Path) -> Result<json::Map> {
    let (mut files, mut deleted) = (0i64, 0i64);
    for path in rglob(tree) {
        if is_symlink(&path) || !is_file(&path) {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        if name.as_deref() == Some(CONNECTORS_FILE) {
            fs::remove_file(&path).map_err(|e| os_err(&e, &path))?;
            deleted += 1;
            continue;
        }
        if !name
            .as_deref()
            .is_some_and(|n| py::lower(suffix(n)) == ".md")
        {
            continue;
        }
        let rel = rel_posix(&path, tree);
        let raw = fs::read(&path).map_err(|e| os_err(&e, &path))?;
        let Ok(text) = String::from_utf8(raw) else {
            return Err(VendorError(format!("strip-connectors: {rel} is not UTF-8")));
        };
        let stripped = strip_markdown(&text)?;
        for (number, line) in stripped.split('\n').enumerate() {
            if line.contains("~~") || line.contains(CONNECTORS_FILE) {
                return Err(VendorError(format!(
                    "strip-connectors: {rel}:{}: connector markup the transform does not handle \
                     (failing on purpose, so nothing slips through; extend strip-connectors): {}",
                    number + 1,
                    py::prefix(py::strip(line), 100)
                )));
            }
        }
        if stripped != text {
            fs::write(&path, stripped.as_bytes()).map_err(|e| os_err(&e, &path))?;
            files += 1;
        }
    }
    Ok(py::stats(&[("files", files), ("deleted", deleted)]))
}

/// `PurePath.suffix` of a file name.
pub fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[i..],
        _ => "",
    }
}

// ----------------------------------------------------------------------------- rename-skill

/// `rename_skill(tree, old, new)`: the frontmatter `name:` goes from old to new, and `/old` in
/// the body becomes `new`. A longer name such as `/old-checklist` is not matched.
pub fn rename_skill(tree: &Path, old: &str, new: &str) -> Result<json::Map> {
    let path = tree.join("SKILL.md");
    if !is_file(&path) {
        return Err(VendorError::new("rename-skill: SKILL.md not found"));
    }
    let text = py::read_text(&path).map_err(VendorError)?;
    // text.find("\n---", 4) if text.startswith("---\n") else -1
    let end = text
        .strip_prefix("---\n")
        .and_then(|after| after.find("\n---"))
        .map(|i| i + 4);
    let Some(end) = end else {
        return Err(VendorError::new(
            "rename-skill: SKILL.md has no frontmatter",
        ));
    };
    let (front, rest) = text.split_at(end);
    static NAME_LINE: OnceLock<Regex> = OnceLock::new();
    let name_line = NAME_LINE.get_or_init(|| {
        Regex::new(r"(?m)^name:[ \t]*(.*?)[ \t]*$").unwrap_or_else(|e| unreachable!("{e}"))
    });
    let Some(caps) = name_line.captures(front) else {
        return Err(VendorError::new(
            "rename-skill: SKILL.md frontmatter has no name",
        ));
    };
    let (whole, value) = (caps.get(0), caps.get(1).map_or("", |m| m.as_str()));
    if value.trim_matches(|c| c == '\'' || c == '"') != old {
        return Err(VendorError(format!(
            "rename-skill: the skill is named {}, expected {}",
            py::repr_str(value),
            py::repr_str(old)
        )));
    }
    let head = match whole {
        Some(m) => format!("{}name: {new}{}", &front[..m.start()], &front[m.end()..]),
        None => front.to_string(),
    };
    let body_pattern = format!(
        r"(?<!{W})/{}(?![\p{{L}}\p{{N}}_\-])",
        fancy_regex::escape(old)
    );
    let body_re = fancy_regex::Regex::new(&body_pattern)
        .map_err(|e| VendorError(format!("rename-skill: bad pattern: {e}")))?;
    let pieces =
        py::parse_template(new).map_err(|e| VendorError(format!("rename-skill: re.error: {e}")))?;
    let mut body = String::with_capacity(rest.len());
    let mut last = 0;
    let mut count = 0i64;
    for m in body_re.find_iter(rest) {
        let m = m.map_err(|e| limit("rename-skill body", &e))?;
        body.push_str(&rest[last..m.start()]);
        body.push_str(&py::expand(&pieces, m.as_str()));
        last = m.end();
        count += 1;
    }
    body.push_str(&rest[last..]);
    let out = head + &body;
    fs::write(&path, out).map_err(|e| os_err(&e, &path))?;
    let mut stats = json::Map::new();
    stats.insert("name".to_string(), Value::Str(new.to_string()));
    stats.insert(
        "references".to_string(),
        Value::Int(json::Int::Small(count)),
    );
    Ok(stats)
}
