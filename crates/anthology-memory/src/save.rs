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

use crate::notes::{REPO_EXCLUDE, USER_REASON};
use crate::privacy::{self, Finding, PrivacyDeps};
use crate::render::{Budget, DEFAULT_BUDGET, try_list_notes};
use crate::repo::unsafe_dir;
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

/// The share of words two texts must have in common (Jaccard, in percent) to count as similar.
const SIMILAR_PERCENT: usize = 60;
/// At most this many similar notes are named in a warning.
const MAX_SIMILAR: usize = 3;

/// The lowercase alphanumeric words of `text`.
fn words(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Do two names or descriptions share at least `SIMILAR_PERCENT` of their words?
fn similar(a: &str, b: &str) -> bool {
    let (a, b) = (words(a), words(b));
    let shared = a.intersection(&b).count();
    let all = a.union(&b).count();
    all > 0 && shared * 100 >= all * SIMILAR_PERCENT
}

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
/// below `root` (POSIX form); `root` is the realpath of the root given; `similar` holds the
/// file names of the agent's other notes whose name or description is very similar (at most
/// `MAX_SIMILAR`; a warning, never a refusal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    pub action: String,
    pub relative: String,
    pub root: PathBuf,
    pub index_warning: Option<String>,
    pub similar: Vec<String>,
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

/// The most characters of a slug derived from a note name (under [`MAX_SLUG`]).
pub const DERIVED_SLUG_CHARS: usize = 60;

/// The slug `save` uses when `--slug` is left out: `name` in ASCII lowercase, every run of
/// characters that are not ASCII letters or digits made one `-`, cut to [`DERIVED_SLUG_CHARS`]
/// characters and trimmed of `-`. It is empty when `name` holds no ASCII letter or digit, and
/// the caller then asks for an explicit `--slug`. The result still goes through the same slug and
/// privacy checks as an explicit slug.
pub fn slug_from_name(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(DERIVED_SLUG_CHARS);
    slug.trim_end_matches('-').to_string()
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
    check_slug(slug)?;
    let (name, description, body) = check_note_text(slug, name, description, body, env, deps)?;
    if !root.is_dir() {
        return err(format!("not a folder: {}", root.to_string_lossy()));
    }
    let root = realpath(root);
    write_note(
        &root,
        &[".claude", "agent-memory", &format!("anthology-{agent}")],
        kind,
        slug,
        &name,
        &description,
        &body,
        replace,
        today,
        deps,
    )
}

/// Save a note into the repo's own memory, `<root>/.claude/memory/<type>-<slug>.md`, and
/// regenerate that folder's `MEMORY.md` (`anthology-mem save --repo`). It takes the same checks as
/// an agent note ([`check_note_text`]); a `user` note is refused, because the repo is committed.
#[allow(
    clippy::too_many_arguments,
    reason = "the arguments of `save_note_with`, minus the agent"
)]
pub fn save_repo_note_with(
    root: &Path,
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
    if kind == "user" {
        return err(format!("refusing to save: {USER_REASON}"));
    }
    if !SAVE_TYPES.contains(&kind) {
        return err(format!("type must be one of: {}", SAVE_TYPES.join(", ")));
    }
    check_slug(slug)?;
    let (name, description, body) = check_note_text(slug, name, description, body, env, deps)?;
    if !root.is_dir() {
        return err(format!("not a folder: {}", root.to_string_lossy()));
    }
    let root = realpath(root);
    write_note(
        &root,
        &[".claude", "memory"],
        kind,
        slug,
        &name,
        &description,
        &body,
        replace,
        today,
        deps,
    )
}

/// The slug rule shared by both save paths.
fn check_slug(slug: &str) -> Result<(), SaveError> {
    if slug_ok(slug) && pystr::char_len(slug) <= MAX_SLUG {
        return Ok(());
    }
    err(format!(
        "slug must be lowercase words joined by single dashes (a-z, 0-9), at most {MAX_SLUG} \
         characters"
    ))
}

/// The part of a save after the checks: create each folder of `chain` below `root` (never through
/// a symlink), write `<kind>-<slug>.md` in the last one, and regenerate its `MEMORY.md`.
#[allow(
    clippy::too_many_arguments,
    reason = "the cleaned note text and the save's switches are all separate"
)]
fn write_note(
    root: &Path,
    chain: &[&str],
    kind: &str,
    slug: &str,
    name: &str,
    description: &str,
    body: &str,
    replace: bool,
    today: Option<Date>,
    deps: &SaveDeps,
) -> Result<Saved, SaveError> {
    let mut slug = slug;
    if let Some(rest) = slug.strip_prefix(kind).and_then(|r| r.strip_prefix('-'))
        && !rest.is_empty()
    {
        slug = rest; // `--type project --slug project-x` is project-x.md
    }
    let mut dir = root.to_path_buf();
    for part in chain {
        dir.push(part);
        ensure_real_dir(&dir, root)?;
    }
    if realpath(&dir) != dir {
        return err(format!(
            "refusing to write: {} is a symlink or not a folder",
            rel(&dir, root)
        ));
    }

    let file = format!("{kind}-{slug}.md");
    let similar_notes: Vec<String> = try_list_notes(&dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|(f, n)| {
            *f != file && (similar(name, &n.name) || similar(description, &n.description))
        })
        .map(|(f, _)| f)
        .take(MAX_SIMILAR)
        .collect();
    let path = dir.join(&file);
    let relative = rel(&path, root);
    let modified = today.unwrap_or_else(clock::today_local).iso();
    let text = format!(
        "---\nname: {}\ndescription: {}\nmetadata:\n  type: {kind}\n  modified: {modified}\n---\n\n{body}",
        yaml_quote(name),
        yaml_quote(description)
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

    let index_warning = (deps.regenerate_index)(&dir, DEFAULT_BUDGET, REPO_EXCLUDE)
        .err()
        .map(|e| e.py_str());
    Ok(Saved {
        action: action.to_string(),
        relative,
        root: root.to_path_buf(),
        index_warning,
        similar: similar_notes,
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

/// Which note folders `reindex` rebuilds the `MEMORY.md` of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReindexTarget<'a> {
    /// One roster agent's folder.
    Agent(&'a str),
    /// The repo's own memory folder.
    Repo,
    /// The repo's folder and every roster agent's folder.
    All,
}

/// `reindex`: rewrite `MEMORY.md` of the targeted note folders below `root` from the notes alone,
/// whatever the old file holds (a merge conflict, say). Returns `(path of the MEMORY.md below root,
/// whether it was rewritten)` for each folder that exists; a folder that does not exist is skipped.
/// Every folder on the way is checked first, so a symlink anywhere refuses the whole run before
/// anything is written.
pub fn reindex(
    root: &Path,
    target: ReindexTarget<'_>,
    deps: &SaveDeps,
) -> Result<Vec<(String, bool)>, SaveError> {
    if let ReindexTarget::Agent(agent) = target
        && !ROSTER.contains(&agent)
    {
        return err(format!("unknown agent; use one of: {}", ROSTER.join(", ")));
    }
    if !root.is_dir() {
        return err(format!("not a folder: {}", root.to_string_lossy()));
    }
    let root = realpath(root);
    let claude = root.join(".claude");
    let agents = claude.join("agent-memory");
    let mut dirs: Vec<PathBuf> = Vec::new();
    if matches!(target, ReindexTarget::Repo | ReindexTarget::All) {
        dirs.push(claude.join("memory"));
    }
    match target {
        ReindexTarget::Agent(agent) => dirs.push(agents.join(format!("anthology-{agent}"))),
        // Only the roster's folder names: a name read from disk is never listed or printed.
        ReindexTarget::All => {
            dirs.extend(ROSTER.iter().map(|a| agents.join(format!("anthology-{a}"))));
        }
        ReindexTarget::Repo => {}
    }
    let mut present = Vec::new();
    for dir in dirs {
        let mut at = root.clone();
        for part in dir.strip_prefix(&root).unwrap_or(&dir).components() {
            at.push(part);
            if let Some(bad) = unsafe_dir(&at) {
                return err(format!(
                    "refusing to write: {} is a symlink or not a folder",
                    rel(&bad.path, &root)
                ));
            }
        }
        if fs::symlink_metadata(&dir).is_ok() {
            present.push(dir);
        }
    }
    let mut done = Vec::new();
    for dir in present {
        let wrote = (deps.regenerate_index)(&dir, DEFAULT_BUDGET, REPO_EXCLUDE).map_err(|e| {
            SaveError(format!(
                "could not rewrite {}: {}",
                rel(&dir.join("MEMORY.md"), &root),
                strerror(&e)
            ))
        })?;
        done.push((rel(&dir.join("MEMORY.md"), &root), wrote));
    }
    Ok(done)
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
    fn similar_texts_share_most_of_their_words() {
        assert!(similar("Eval probe", "Eval probe notes"));
        assert!(similar("Eval-Probe!", "eval probe"));
        assert!(similar("How the probe works", "how the probe works"));
        assert!(!similar("Eval probe", "Release checklist"));
        assert!(!similar("Eval probe", "Eval probe results and more"));
        assert!(!similar("", ""));
        assert!(!similar("!!", "??"));
    }

    #[test]
    fn leading_blank_lines() {
        assert_eq!(drop_leading_blank_lines("\n \t\n  x\n"), "  x\n");
        assert_eq!(drop_leading_blank_lines("  x"), "  x");
    }
}
