//! What a repo says about itself, read at session start (never changes a file).
//!
//! - `<repo>/.claude/anthology.json`: the repo's own switches ([`switch`], [`guide_off`]). A key
//!   that is missing, not a JSON boolean, or in a file that does not parse leaves the default.
//! - The repo's own agents and skills ([`section`]): `<repo>/.claude/agents/*.md` and
//!   `<repo>/.claude/skills/*/SKILL.md`, an agent table or list in `<repo>/CLAUDE.md`, then the
//!   user's `agents/` and `skills/` in the Claude config folder. Each is listed by its
//!   frontmatter `name` and a trimmed `description`, at most [`MAX_ENTRIES`] in all, so the lead
//!   uses them before Anthology's agents.
//!
//! Everything here is best effort: a file that cannot be read or parsed is left out, and a
//! folder is looked at only as far as [`MAX_SCAN`] names and [`MAX_FILE`] bytes per file, so a
//! large or odd repo cannot slow session start down.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::Path;

use anthology_memory::notes::parse_text_ok;
use anthology_pycompat::json::{self, Value};

/// The repo's switches file, relative to the repo root.
pub const CONFIG_REL: &str = ".claude/anthology.json";
/// The switch that leaves the routing guide (and what follows it) out of session start.
pub const GUIDE_KEY: &str = "routing_guide";
/// Entries listed in all; the rest are counted, not named.
pub const MAX_ENTRIES: usize = 12;
/// Characters of one description, the ellipsis included.
pub const MAX_DESC: usize = 100;
/// Names looked at in one folder.
const MAX_SCAN: usize = 200;
/// Bytes read of an agent or skill file (the frontmatter is at the top).
const MAX_FILE: u64 = 8192;
/// Bytes read of `CLAUDE.md` and of the switches file.
const MAX_DOC: u64 = 65536;
/// Longest name taken as an agent or skill name.
const MAX_NAME: usize = 64;

/// The heading of the section, and the rule under it.
pub const HEADING: &str = "## This project's own agents and skills";
pub const RULE: &str = "For the jobs they cover, use the project's own agents and skills before Anthology's; Anthology's agents fill the gaps.";

/// Where an entry came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Agent,
    Skill,
    ClaudeMd,
    UserAgent,
    UserSkill,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::Skill => "skill",
            Kind::ClaudeMd => "CLAUDE.md",
            Kind::UserAgent => "user agent",
            Kind::UserSkill => "user skill",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    kind: Kind,
    desc: String,
}

impl Entry {
    fn line(&self) -> String {
        let label = self.kind.label();
        if self.desc.is_empty() {
            format!("- {} ({label})", self.name)
        } else {
            format!("- {} ({label}): {}", self.name, self.desc)
        }
    }
}

/// At most `max` bytes of a regular file, as text (invalid UTF-8 becomes U+FFFD). `None` when it
/// is not a regular file (a pipe would block) or cannot be read.
fn read_prefix(path: &Path, max: u64) -> Option<String> {
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(max)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The boolean `key` of `<repo>/.claude/anthology.json`; `None` for anything else.
pub fn switch(repo: &Path, key: &str) -> Option<bool> {
    let text = read_prefix(&repo.join(CONFIG_REL), MAX_DOC)?;
    let object = json::loads(&text).ok()?.into_object()?;
    match object.get(key)? {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

/// Whether the repo turned the routing guide off.
pub fn guide_off(repo: &Path) -> bool {
    switch(repo, GUIDE_KEY) == Some(false)
}

/// The names of a folder, sorted, at most [`MAX_SCAN`].
fn names_in(dir: &Path) -> Vec<String> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names.truncate(MAX_SCAN);
    names
}

/// A name an agent or skill can have: no spaces, quotes or line breaks.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
}

/// Anthology's own: `anthology`, `anthology:<x>` and `anthology-<x>`.
fn is_ours(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "anthology" || name.starts_with("anthology:") || name.starts_with("anthology-")
}

/// One line, whitespace folded, at most [`MAX_DESC`] characters.
fn trimmed(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= MAX_DESC {
        return one;
    }
    let head: String = one.chars().take(MAX_DESC - 1).collect();
    format!("{}…", head.trim_end())
}

/// The entry a file with frontmatter gives; `stem` names it when the frontmatter does not.
fn from_file(path: &Path, stem: &str, kind: Kind) -> Option<Entry> {
    let (info, ok) = parse_text_ok(&read_prefix(path, MAX_FILE)?, stem);
    let desc = trimmed(&info.description);
    if !ok || desc.is_empty() || !is_name(&info.name) || is_ours(&info.name) || is_ours(stem) {
        return None;
    }
    Some(Entry {
        name: info.name,
        kind,
        desc,
    })
}

fn agents_in(dir: &Path, kind: Kind) -> Vec<Entry> {
    names_in(dir)
        .into_iter()
        .filter_map(|file| {
            let stem = file.strip_suffix(".md")?;
            from_file(&dir.join(&file), stem, kind)
        })
        .collect()
}

fn skills_in(dir: &Path, kind: Kind) -> Vec<Entry> {
    names_in(dir)
        .into_iter()
        .filter_map(|folder| from_file(&dir.join(&folder).join("SKILL.md"), &folder, kind))
        .collect()
}

// --- an agent table or list in CLAUDE.md ----------------------------------------------------

fn table_cells(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim_matches(':');
            !c.is_empty() && c.chars().all(|ch| ch == '-')
        })
}

/// `level` and title of a Markdown heading line.
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let level = t.chars().take_while(|c| *c == '#').count();
    let rest = t[level..].strip_prefix(' ')?;
    (level > 0).then_some((level, rest.trim()))
}

/// The leading name of a list item and what follows it. A name in `` ` `` or `**` is taken as
/// written. A plain word counts only when a `:` or a dash follows it (or, for a hyphenated word,
/// nothing does), so a sentence in an agents section is not read as an agent.
fn name_and_rest(text: &str) -> Option<(String, &str)> {
    let t = text.trim();
    for quote in ["`", "**"] {
        if let Some(inner) = t.strip_prefix(quote)
            && let Some(end) = inner.find(quote)
        {
            return Some((inner[..end].to_string(), &inner[end + quote.len()..]));
        }
    }
    let end = t
        .find(|c: char| c.is_whitespace() || c == ':')
        .unwrap_or(t.len());
    let (name, rest) = (&t[..end], &t[end..]);
    let after = rest.trim_start();
    let separated = after.starts_with(':')
        || ["- ", "– ", "— "].iter().any(|d| after.starts_with(d))
        || (after.is_empty() && name.contains('-'));
    separated.then(|| (name.to_string(), rest))
}

fn claude_md_entry(name: &str, desc: &str) -> Option<Entry> {
    let name = name.trim_matches(|c| c == '`' || c == '*').trim();
    if !is_name(name) || is_ours(name) || name.eq_ignore_ascii_case("agent") {
        return None;
    }
    Some(Entry {
        name: name.to_string(),
        kind: Kind::ClaudeMd,
        desc: trimmed(desc),
    })
}

/// The agents a `CLAUDE.md` names: under each heading that has "agent" in it, the rows of a table
/// (the column whose header says "agent", else the first) and the items of a list (`` `name`: …``,
/// `**name** - …`, `name - …`), up to the next heading of the same or a higher level. Fenced code
/// is skipped.
fn claude_md_entries(text: &str) -> Vec<Entry> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut section: Option<usize> = None;
    let mut fenced = false;
    let mut name_col = 0;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some((level, title)) = heading(line) {
            if section.is_some_and(|s| level <= s) {
                section = None;
            }
            if section.is_none() && title.to_ascii_lowercase().contains("agent") {
                section = Some(level);
            }
            name_col = 0;
            continue;
        }
        if section.is_none() {
            continue;
        }
        if t.starts_with('|') {
            let cells = table_cells(line);
            let next_is_rule = lines
                .get(i + 1)
                .is_some_and(|n| n.trim_start().starts_with('|') && is_separator(&table_cells(n)));
            if next_is_rule {
                name_col = cells
                    .iter()
                    .position(|c| c.to_ascii_lowercase().contains("agent"))
                    .unwrap_or(0);
                continue;
            }
            if is_separator(&cells) {
                continue;
            }
            let Some(name) = cells.get(name_col) else {
                continue;
            };
            let desc: Vec<&str> = cells
                .iter()
                .enumerate()
                .filter(|(c, v)| *c != name_col && !v.is_empty())
                .map(|(_, v)| v.as_str())
                .collect();
            out.extend(claude_md_entry(name, &desc.join("; ")));
        } else if let Some(item) = ["- ", "* ", "+ "].iter().find_map(|p| t.strip_prefix(p))
            && let Some((name, rest)) = name_and_rest(item)
        {
            let desc = rest.trim_start_matches(|c: char| {
                c.is_whitespace() || matches!(c, ':' | '-' | '–' | '—')
            });
            out.extend(claude_md_entry(&name, desc));
        }
    }
    out
}

fn claude_md(repo: &Path) -> Vec<Entry> {
    ["CLAUDE.md", ".claude/CLAUDE.md"]
        .iter()
        .filter_map(|rel| read_prefix(&repo.join(rel), MAX_DOC))
        .flat_map(|text| claude_md_entries(&text))
        .collect()
}

/// The section for the injected context, or `None` when the repo and the user have none to list.
/// `config` is the Claude config folder (the user's own `agents/` and `skills/` are in it).
/// Order: project agents, project skills, `CLAUDE.md`, then the user's; a name listed once.
pub fn section(repo: &Path, config: &Path) -> Option<String> {
    let mut seen = HashSet::new();
    let found: Vec<Entry> = agents_in(&repo.join(".claude/agents"), Kind::Agent)
        .into_iter()
        .chain(skills_in(&repo.join(".claude/skills"), Kind::Skill))
        .chain(claude_md(repo))
        .chain(agents_in(&config.join("agents"), Kind::UserAgent))
        .chain(skills_in(&config.join("skills"), Kind::UserSkill))
        .filter(|e| seen.insert(e.name.to_ascii_lowercase()))
        .collect();
    if found.is_empty() {
        return None;
    }
    let mut lines = vec![HEADING.to_string(), RULE.to_string()];
    lines.extend(found.iter().take(MAX_ENTRIES).map(Entry::line));
    if found.len() > MAX_ENTRIES {
        lines.push(format!("({} more not listed)", found.len() - MAX_ENTRIES));
    }
    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_fold_whitespace_and_stop_at_the_cap() {
        assert_eq!(trimmed("  a \n b\t c "), "a b c");
        let long = trimmed(&"ab ".repeat(100));
        assert_eq!(long.chars().count(), MAX_DESC - 1);
        assert!(long.ends_with('…'));
        assert_eq!(trimmed(&"é".repeat(100)).chars().count(), 100);
    }

    #[test]
    fn only_plain_names_pass() {
        for ok in ["a", "code-reviewer", "x_y", "p:q", "v1.2"] {
            assert!(is_name(ok), "{ok}");
        }
        for bad in ["", "two words", "a`b", "x\ny", &"a".repeat(65)] {
            assert!(!is_name(bad), "{bad}");
        }
    }

    #[test]
    fn heading_levels() {
        assert_eq!(heading("## Agents"), Some((2, "Agents")));
        assert_eq!(heading("#NoSpace"), None);
        assert_eq!(heading("plain"), None);
    }
}
