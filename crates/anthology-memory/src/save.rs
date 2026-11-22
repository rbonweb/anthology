//! `save_note` and `SaveError` (MS2): the helper behind `anthology mem save`, a port of
//! `memory_sync.save_note` (plan §3.6, GC-12).
//!
//! It writes `<root>/.claude/agent-memory/anthology-<agent>/<type>-<slug>.md` and regenerates
//! that folder's `MEMORY.md`. Only validated parts build the path, so no argument reaches
//! outside the agent's folder; symlinks are never written through; an existing note needs
//! `replace`. Secrets (privacy's `SECRET_PATTERNS`, and the values of secret-named environment
//! variables) and personal data (the privacy scan) are refused before anything is written, and
//! the refusal names the kind, never the text. Every message is verbatim from `memory_sync.py`.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use anthology_core::clock::{self, Date};
use anthology_core::roster::ROSTER;
use anthology_core::{Env, fsx};
use anthology_pycompat::path::realpath;
use anthology_pycompat::pyerr::PyExc;
use anthology_pycompat::str as pystr;

use crate::notes::REPO_EXCLUDE;
use crate::privacy::{self, Finding, PrivacyDeps};
use crate::render::{Budget, DEFAULT_BUDGET};
use crate::sync::strerror;

/// `SAVE_TYPES`: `TYPE_ORDER` minus `user` (personal memory).
pub const SAVE_TYPES: [&str; 3] = ["project", "feedback", "reference"];
/// `MAX_SLUG`, `MAX_NAME`, `MAX_DESCRIPTION`, `MAX_BODY_BYTES`.
pub const MAX_SLUG: usize = 64;
pub const MAX_NAME: usize = 120;
pub const MAX_DESCRIPTION: usize = 300;
pub const MAX_BODY_BYTES: usize = 16384;
/// `_SECRET_ENV_NAME` (`(?i)` as the IGNORECASE flag).
const SECRET_ENV_NAME: &str = "(TOKEN|SECRET|KEY|PASSWORD|PASSWD|CREDENTIAL)";

/// `SaveError`: a save that was refused; the message is `str(exc)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveError(pub String);

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SaveError {}

/// What `save_note` did: `action` is `created` or `replaced`; `relative` is the note's path
/// below `root` (POSIX form); `root` is the realpath of the root given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub action: String,
    pub relative: String,
    pub root: PathBuf,
    pub index_warning: Option<String>,
}

impl Saved {
    /// Python's `out["path"]`: the note's absolute path (an MS2 addition beyond plan §3.6).
    pub fn path(&self) -> PathBuf {
        self.root.join(&self.relative)
    }
}

/// The seams of `save_note` that tests patch (GC-19). `Default` is production. An MS2
/// addition beyond plan §3.6.
#[derive(Clone)]
pub struct SaveDeps {
    /// `memory_sync.privacy_findings`.
    pub privacy_findings: fn(&str, Option<&str>, Option<&str>, &PrivacyDeps) -> Vec<Finding>,
    /// `memory_sync.regenerate_index`.
    pub regenerate_index: fn(&Path, Budget, &[&str]) -> Result<bool, PyExc>,
}

impl Default for SaveDeps {
    fn default() -> SaveDeps {
        SaveDeps {
            privacy_findings: privacy::privacy_findings,
            regenerate_index: crate::render::regenerate_index,
        }
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T, SaveError> {
    Err(SaveError(msg.into()))
}

/// `unicodedata.category(c) == "Cc"`.
fn is_cc(c: char) -> bool {
    pystr::unicode_category(c) == "Cc"
}

/// `_has_hidden`: controls, format characters and line/paragraph separators.
fn has_hidden(t: &str) -> bool {
    t.chars()
        .any(|c| matches!(pystr::unicode_category(c), "Cc" | "Cf" | "Zl" | "Zp"))
}

/// `_check_line`.
fn check_line(label: &str, value: &str, limit: usize) -> Result<String, SaveError> {
    let v = pystr::strip_ws(value);
    if v.is_empty() {
        return err(format!("{label} must not be empty"));
    }
    if pystr::char_len(v) > limit {
        return err(format!("{label} is too long (at most {limit} characters)"));
    }
    if has_hidden(v) {
        return err(format!(
            "{label} must be one line without control or invisible formatting characters"
        ));
    }
    Ok(v.to_string())
}

/// `_LEADING_BLANK_LINES.sub("", value)`: `\A(?:[ \t]*\n)+`.
fn drop_leading_blank_lines(mut v: &str) -> &str {
    loop {
        let rest = v.trim_start_matches([' ', '\t']);
        match rest.strip_prefix('\n') {
            Some(r) => v = r,
            None => return v,
        }
    }
}

/// `_check_body`.
fn check_body(value: &str) -> Result<String, SaveError> {
    let v = value.replace("\r\n", "\n").replace('\r', "\n");
    if v.chars().any(|c| is_cc(c) && c != '\n' && c != '\t') {
        return err("body must not contain control characters (other than newline and tab)");
    }
    let v = drop_leading_blank_lines(&v).trim_end_matches(pystr::is_space);
    if v.is_empty() {
        return err("body must not be empty");
    }
    if v.len() > MAX_BODY_BYTES {
        return err(format!("body is too long (at most {MAX_BODY_BYTES} bytes)"));
    }
    Ok(format!("{v}\n"))
}

/// `_SECRET_FIELDS`: the order of the texts `save` scans.
const SECRET_FIELDS: [&str; 4] = ["slug", "name", "description", "body"];

/// One secret finding: `(field index, label, line)`.
type SecretFinding = (usize, &'static str, usize);

/// `_secret_findings`: `(field index, label, line)` for every `SECRET_PATTERNS` match in the
/// fields joined by newlines, sorted and unique; the line is 1-based within the field. Never the
/// matched text. A pattern the engine could not finish is in the second list (fail closed;
/// Python's `re` has no such case).
fn secret_findings(texts: &[&str]) -> (Vec<SecretFinding>, Vec<&'static str>) {
    let joined = texts.join("\n");
    let mut starts = Vec::with_capacity(texts.len());
    let mut pos = 0;
    for text in texts {
        starts.push(pos);
        pos += text.len() + 1;
    }
    let mut found = BTreeSet::new();
    let mut unfinished = BTreeSet::new();
    for (label, at) in privacy::secret_match_starts(&joined) {
        let Some(at) = at else {
            unfinished.insert(label);
            continue;
        };
        let idx = starts.partition_point(|s| *s <= at) - 1;
        let line = joined.as_bytes()[starts[idx]..at]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1;
        found.insert((idx, label, line));
    }
    (
        found.into_iter().collect(),
        unfinished.into_iter().collect(),
    )
}

/// `_MAX_SECRET_FINDINGS`.
const MAX_SECRET_FINDINGS: usize = 5;

/// `_describe_secrets`: `url-credentials on line 3 of the note body, password in the name`: kinds
/// and places, never the text. At most `MAX_SECRET_FINDINGS` findings are listed, then
/// `and N more`.
fn describe_secrets(found: &[SecretFinding], unfinished: &[&str]) -> String {
    let mut groups: Vec<((usize, &str), Vec<usize>)> = Vec::new();
    for &(idx, label, line) in found.iter().take(MAX_SECRET_FINDINGS) {
        match groups.last_mut() {
            Some((key, lines)) if *key == (idx, label) => lines.push(line),
            _ => groups.push(((idx, label), vec![line])),
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for ((idx, label), lines) in groups {
        let field = SECRET_FIELDS[idx];
        parts.push(if field != "body" {
            format!("{label} in the {field}")
        } else {
            let nums: Vec<String> = lines.iter().map(usize::to_string).collect();
            let noun = if lines.len() == 1 { "line" } else { "lines" };
            format!("{label} on {noun} {} of the note body", nums.join(", "))
        });
    }
    let mut text = parts.join(", ");
    if found.len() > MAX_SECRET_FINDINGS {
        text.push_str(&format!(" and {} more", found.len() - MAX_SECRET_FINDINGS));
    }
    for label in unfinished {
        if !text.is_empty() {
            text.push_str(", ");
        }
        text.push_str(&format!("{label} (the scan could not finish)"));
    }
    text
}

/// `_refuse_secrets`.
fn refuse_secrets(texts: &[&str], env: &Env) -> Result<(), SaveError> {
    let joined = texts.join("\n");
    let (found, unfinished) = secret_findings(texts);
    if !found.is_empty() || !unfinished.is_empty() {
        return err(format!(
            "refusing to save: the note looks like it contains a secret; memory is committed to \
             the repo, so remove or replace that value (e.g. refer to the env var name) and save \
             again. Found: {}",
            describe_secrets(&found, &unfinished)
        ));
    }
    let mut vars: Vec<(String, &str)> = env
        .iter()
        .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v.to_str()?)))
        .collect();
    vars.sort();
    for (var, value) in vars {
        if pystr::char_len(value) >= 8
            && privacy::search_any(&[(SECRET_ENV_NAME, true)], &var)
            && joined.contains(value)
        {
            return err(format!(
                "refusing to save: the note contains the value of ${var} (memory is committed to \
                 the repo)"
            ));
        }
    }
    Ok(())
}

/// `_privacy_check`: the one privacy wiring point for `save`.
fn privacy_check(texts: &[&str; 4], env: &Env, deps: &SaveDeps) -> Result<(), SaveError> {
    let pdeps = PrivacyDeps::from_env(env);
    for (label, text) in ["slug", "name", "description", "body"].iter().zip(texts) {
        let scanned = catch_unwind(AssertUnwindSafe(|| {
            (deps.privacy_findings)(text, None, None, &pdeps)
        }));
        let Ok(findings) = scanned else {
            // A scanner bug is a clean refusal, never a crash or a write.
            return err("refusing to save: the privacy scan failed (panic); nothing was written");
        };
        if !findings.is_empty() {
            return err(format!(
                "refusing to save: the {label} contains personal or private data ({}); memory is \
                 committed to the repo, so keep that in personal memory",
                privacy::describe(&findings)
            ));
        }
    }
    Ok(())
}

fn yaml_quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}

/// `SLUG_RE.fullmatch(slug)`: `^[a-z0-9]+(?:-[a-z0-9]+)*$`.
fn slug_ok(slug: &str) -> bool {
    !slug.is_empty()
        && slug.split('-').all(|p| {
            !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

fn rel(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn is_symlink(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// `_ensure_real_dir`: make `path` a real folder (never a symlink), creating it if absent.
fn ensure_real_dir(path: &Path, root: &Path) -> Result<(), SaveError> {
    let r = rel(path, root);
    if fs::symlink_metadata(path).is_err() {
        use std::os::unix::fs::DirBuilderExt;
        match fs::DirBuilder::new().mode(0o755).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return err(format!(
                    "could not create {r}: {}",
                    strerror(&PyExc::from(e))
                ));
            }
        }
    }
    if is_symlink(path) || !path.is_dir() {
        return err(format!(
            "refusing to write: {r} is a symlink or not a folder"
        ));
    }
    Ok(())
}

/// Every check `save_note` runs on the text of a note bound for repo memory, in its order: the
/// name and description are one line within their limits without control or invisible characters,
/// the body has no control characters and is within `MAX_BODY_BYTES`, nothing looks like a secret
/// or holds the value of a secret-named environment variable, and the privacy scan finds nothing
/// (a scanner panic is a clean refusal). Returns the cleaned `(name, description, body)`. Anything
/// that moves a note into a repo's memory must call this, so it never lets in what `save` refuses.
pub fn check_note_text(
    slug: &str,
    name: &str,
    description: &str,
    body: &str,
    env: &Env,
    deps: &SaveDeps,
) -> Result<(String, String, String), SaveError> {
    let name = check_line("name", name, MAX_NAME)?;
    let description = check_line("description", description, MAX_DESCRIPTION)?;
    let body = check_body(body)?;
    refuse_secrets(&[slug, &name, &description, &body], env)?;
    privacy_check(&[slug, &name, &description, &body], env, deps)?;
    Ok((name, description, body))
}

/// `save_note`, with the GC-19 seams (an MS2 addition beyond plan §3.6).
#[allow(
    clippy::too_many_arguments,
    reason = "Python's arguments, as published"
)]
pub fn save_note_with(
    root: &Path,
    agent: &str,
    kind: &str,
    slug: &str,
    name: &str,
    description: &str,
    body: &str,
    replace: bool,
    today: Option<Date>,
    env: &Env,
    deps: &SaveDeps,
) -> Result<Saved, SaveError> {
    if !ROSTER.contains(&agent) {
        return err(format!("unknown agent; use one of: {}", ROSTER.join(", ")));
    }
    if !SAVE_TYPES.contains(&kind) {
        return err(format!("type must be one of: {}", SAVE_TYPES.join(", ")));
    }
    if !(slug_ok(slug) && pystr::char_len(slug) <= MAX_SLUG) {
        return err(format!(
            "slug must be lowercase words joined by single dashes (a-z, 0-9), at most {MAX_SLUG} \
             characters"
        ));
    }
    let (name, description, body) = check_note_text(slug, name, description, body, env, deps)?;
    let mut slug = slug;
    if let Some(rest) = slug.strip_prefix(kind).and_then(|r| r.strip_prefix('-'))
        && !rest.is_empty()
    {
        slug = rest; // `--type project --slug project-x` is project-x.md
    }
    if !root.is_dir() {
        return err(format!("not a folder: {}", root.to_string_lossy()));
    }
    let root = realpath(root);

    let agent_dir = root
        .join(".claude")
        .join("agent-memory")
        .join(format!("anthology-{agent}"));
    for dir in [
        root.join(".claude"),
        root.join(".claude").join("agent-memory"),
        agent_dir.clone(),
    ] {
        ensure_real_dir(&dir, &root)?;
    }
    if realpath(&agent_dir) != agent_dir {
        return err(format!(
            "refusing to write: {} is a symlink or not a folder",
            rel(&agent_dir, &root)
        ));
    }

    let path = agent_dir.join(format!("{kind}-{slug}.md"));
    let relative = rel(&path, &root);
    let modified = today.unwrap_or_else(clock::today_local).iso();
    let text = format!(
        "---\nname: {}\ndescription: {}\nmetadata:\n  type: {kind}\n  modified: {modified}\n---\n\n{body}",
        yaml_quote(&name),
        yaml_quote(&description)
    );
    let write_err = |e: PyExc| match e {
        PyExc::Other("UnsafeTarget", m) => SaveError(m),
        other => SaveError(format!("could not write {relative}: {}", strerror(&other))),
    };
    let mut action = "created";
    if !replace {
        match fsx::create_excl(&path, text.as_bytes(), 0o644, true) {
            Ok(()) => {}
            Err(PyExc::OSError { errno: 17, .. }) => {
                return err(format!(
                    "{relative} already exists; pass --replace to overwrite it"
                ));
            }
            Err(e) => return Err(write_err(e)),
        }
    } else {
        match fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_file() => action = "replaced",
            Ok(_) => {
                return err(format!(
                    "refusing to write: {relative} is a symlink or not a regular file"
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                return err(format!(
                    "refusing to write: {relative} is a symlink or not a regular file"
                ));
            }
        }
        fsx::atomic_write(&path, text.as_bytes(), 0o644, ".tmp-", ".part").map_err(write_err)?;
    }

    let index_warning = (deps.regenerate_index)(&agent_dir, DEFAULT_BUDGET, REPO_EXCLUDE)
        .err()
        .map(|e| e.py_str());
    Ok(Saved {
        action: action.to_string(),
        relative,
        root,
        index_warning,
    })
}

/// `save_note`: write an agent's note into the repo at `root` and regenerate its index.
#[allow(
    clippy::too_many_arguments,
    reason = "Python's arguments, as published"
)]
pub fn save_note(
    root: &Path,
    agent: &str,
    kind: &str,
    slug: &str,
    name: &str,
    description: &str,
    body: &str,
    replace: bool,
    today: Option<Date>,
    env: &Env,
) -> Result<Saved, SaveError> {
    save_note_with(
        root,
        agent,
        kind,
        slug,
        name,
        description,
        body,
        replace,
        today,
        env,
        &SaveDeps::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        for ok in ["a", "a-b", "a1-2b", "x0"] {
            assert!(slug_ok(ok), "{ok}");
        }
        for bad in ["", "-a", "a-", "a--b", "A", "a_b", "a.b", "é", "x\n"] {
            assert!(!slug_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn leading_blank_lines() {
        assert_eq!(drop_leading_blank_lines("\n \t\n  x\n"), "  x\n");
        assert_eq!(drop_leading_blank_lines("  x"), "  x");
    }
}
