//! `anthology-dev startup-budget` (scripts/startup_budget.py). Owner: DV2.
//!
//! Per-agent startup budget: how many bytes each agent reads before it starts work, which is
//! its body, plus every `${CLAUDE_PLUGIN_ROOT}/knowledge/.../SKILL.md` listed under its
//! `## Startup` heading, plus every preloaded `skills:` SKILL.md of anthology-superpowers. This
//! module is the single home of that calculation (MT1's test_roster port and DV3's upstream
//! watch call it). Exit codes (GC-5): 0; 1 over or unset; 2 unknown agent (or usage).

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anthology_core::Env;
use anthology_pycompat::argparse::{self, Arg, Nargs};
use anthology_pycompat::pyerr::PyExc;
use regex::Regex;

use crate::Output;
use crate::json_edit::{Uncaught, universal_newlines, utf8_decode_error};
use crate::memory_diff::pystr::splitlines;
use anthology_pycompat::str as pystr;

pub const ANTHOLOGY: &str = "plugins/anthology";
pub const SUPERPOWERS: &str = "plugins/anthology-superpowers";

/// Startup budget in bytes, per agent (plan GC-13, verbatim): the size measured when the roster
/// landed, times 1.15, rounded up. Growth fails the roster test on purpose.
pub const BUDGET: [(&str, u64); 11] = [
    ("researcher", 89267),       // measured 77623
    ("architect", 23612),        // measured 20532
    ("implementer", 25451),      // measured 22131 (karpathy-guidelines)
    ("debugger", 21229),         // measured 18460 (karpathy-guidelines)
    ("reviewer", 12553),         // measured 10915
    ("scout", 5944),             // measured 5168
    ("docs-writer", 31376),      // measured 27283
    ("ui-designer", 128097),     // measured 111388
    ("ui-reviewer", 65986),      // measured 57379
    ("product-planner", 102779), // measured 89373
    ("marketer", 54384),         // measured 47290
];

static STARTUP_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{CLAUDE_PLUGIN_ROOT\}/(knowledge/[A-Za-z0-9_./-]+/SKILL\.md)")
        .expect("STARTUP_PATH")
});
static SAFE_AGENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9._-]*\z").expect("SAFE_AGENT_RE"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^## (.+)$").expect("HEADING"));

/// An exception the Python does not catch (ruling R10): its class and `str()`.
pub type Error = Uncaught;

/// `suggested_budget`: `ceil(measured * 1.15)` in integer arithmetic.
pub fn suggested_budget(measured: u64) -> u64 {
    (measured * 115).div_ceil(100)
}

/// `sections(body)`: the ordered `## ` heading names, and name → text with `dict(zip(...))`
/// semantics (a repeated name keeps its first position and takes the last text).
pub fn sections(body: &str) -> (Vec<String>, Vec<(String, String)>) {
    let mut names = Vec::new();
    let mut texts = Vec::new();
    let mut last = None;
    for caps in HEADING.captures_iter(body) {
        let whole = caps.get(0).expect("match");
        if let Some(prev) = last.take() {
            texts.push(body[prev..whole.start()].to_string());
        }
        names.push(pystr::strip_ws(&caps[1]).to_string());
        last = Some(whole.end());
    }
    if let Some(prev) = last {
        texts.push(body[prev..].to_string());
    }
    let mut map: Vec<(String, String)> = Vec::new();
    for (name, text) in names.iter().zip(texts) {
        match map.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => slot.1 = text,
            None => map.push((name.clone(), text)),
        }
    }
    (names, map)
}

/// `str(Path(p))`: pathlib's normalised spelling (no `.` parts, no repeated or trailing `/`;
/// exactly two leading slashes are kept; empty is `.`).
pub fn pathlib_str(p: &str) -> String {
    let lead = if p.starts_with("//") && !p.starts_with("///") {
        "//"
    } else if p.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = p
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .collect();
    let joined = format!("{lead}{}", parts.join("/"));
    if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// A repository root as the Python sees it: `shown` is `str(Path(root))` (for messages),
/// `fs` the path to open (resolved against the working folder).
#[derive(Clone, Debug)]
pub struct Root {
    pub shown: String,
    pub fs: PathBuf,
}

impl Root {
    /// `root` as given on the command line, opened relative to `cwd`.
    pub fn new(root: &Path, cwd: &Path) -> Root {
        Root {
            shown: pathlib_str(&root.to_string_lossy()),
            fs: cwd.join(root),
        }
    }

    /// `(str(Path(root) / rel), the path to open)`.
    fn join(&self, rel: &str) -> (String, PathBuf) {
        (
            pathlib_str(&format!("{}/{rel}", self.shown)),
            self.fs.join(rel),
        )
    }
}

fn os_error(e: std::io::Error, shown: &str) -> Error {
    let mut exc = PyExc::from(e);
    if let PyExc::OSError { filename, .. } = &mut exc {
        *filename = Some(shown.to_string());
    }
    Uncaught::from_exc(&exc)
}

/// `Path(shown).read_text(encoding="utf-8")`: strict UTF-8, universal newlines.
fn read_text(shown: &str, fs: &Path) -> Result<String, Error> {
    let bytes = std::fs::read(fs).map_err(|e| os_error(e, shown))?;
    if let Some(msg) = utf8_decode_error(&bytes) {
        return Err(Uncaught::new("UnicodeDecodeError", msg));
    }
    let text = String::from_utf8(bytes).expect("checked");
    Ok(universal_newlines(&text))
}

/// `re.match(r"^skills:\s*$", line)`.
fn is_skills_key(line: &str) -> bool {
    line.strip_prefix("skills:")
        .is_some_and(|rest| rest.chars().all(pystr::is_space))
}

/// `re.match(r"^\s+-\s+(\S+)\s*$", line)`: the item.
fn list_item(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(pystr::is_space);
    if rest.len() == line.len() {
        return None;
    }
    let rest = rest.strip_prefix('-')?;
    let token = rest.trim_start_matches(pystr::is_space);
    if token.len() == rest.len() {
        return None;
    }
    let token = token.trim_end_matches(pystr::is_space);
    (!token.is_empty() && !token.chars().any(pystr::is_space)).then_some(token)
}

/// `split_agent(text)` on the decoded text; `shown` names the file in errors.
pub fn split_agent_text(shown: &str, text: &str) -> Result<(Vec<String>, String), Error> {
    let Some(rest) = text.strip_prefix("---\n") else {
        return Err(Uncaught::new(
            "ValueError",
            format!("{shown}: missing frontmatter"),
        ));
    };
    let Some((head, body)) = rest.split_once("\n---") else {
        return Err(Uncaught::new(
            "ValueError",
            format!("{shown}: unterminated frontmatter"),
        ));
    };
    let mut skills = Vec::new();
    let mut in_list = false;
    for line in splitlines(head, false) {
        if is_skills_key(line) {
            in_list = true;
        } else if in_list {
            match list_item(line) {
                Some(item) => skills.push(item.to_string()),
                None => in_list = false,
            }
        }
    }
    Ok((skills, body.trim_start_matches('\n').to_string()))
}

/// `split_agent(path)`: (preloaded skill names, body) of an agent file, without a YAML parser.
pub fn split_agent(path: &Path) -> Result<(Vec<String>, String), Error> {
    let shown = pathlib_str(&path.to_string_lossy());
    let text = read_text(&shown, path)?;
    split_agent_text(&shown, &text)
}

/// `agent_names(root)`: agent file stems, sorted; names that are not plain identifiers are
/// skipped. `Path.glob("*.md")`: every entry (dot names, folders and dangling links too) whose
/// name ends with `.md`, case-sensitively; an unreadable folder gives none.
pub fn agent_names(root: &Root) -> Vec<String> {
    let folder = root.fs.join(ANTHOLOGY).join("agents");
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".md").map(str::to_string))
        .filter(|stem| SAFE_AGENT_RE.is_match(stem))
        .collect();
    names.sort();
    names
}

/// `_parts(root, name)`: (skills, body, startup files).
fn parts(root: &Root, name: &str) -> Result<(Vec<String>, String, Vec<String>), Error> {
    let (shown, fs) = root.join(&format!("{ANTHOLOGY}/agents/{name}.md"));
    let text = read_text(&shown, &fs)?;
    let (skills, body) = split_agent_text(&shown, &text)?;
    let (_, map) = sections(&body);
    let startup = map
        .iter()
        .find(|(k, _)| k == "Startup")
        .map_or("", |(_, t)| t.as_str());
    let files = STARTUP_PATH
        .captures_iter(startup)
        .map(|c| c[1].to_string())
        .collect();
    Ok((skills, body, files))
}

/// `startup_files(root, name)`: the knowledge files of the Startup section, relative to the
/// anthology plugin.
pub fn startup_files(root: &Root, name: &str) -> Result<Vec<String>, Error> {
    Ok(parts(root, name)?.2)
}

fn paths_of(skills: &[String], files: &[String]) -> Vec<String> {
    let mut paths: Vec<String> = files.iter().map(|f| format!("{ANTHOLOGY}/{f}")).collect();
    for skill in skills {
        let short = skill.split_once(':').map_or(skill.as_str(), |(_, s)| s);
        paths.push(format!("{SUPERPOWERS}/skills/{short}/SKILL.md"));
    }
    paths
}

/// `startup_paths(root, name)`: every counted file, repo-relative (Startup files, then preloads).
pub fn startup_paths(root: &Root, name: &str) -> Result<Vec<String>, Error> {
    let (skills, _, files) = parts(root, name)?;
    Ok(paths_of(&skills, &files))
}

/// `startup_bytes(root, name)`: body + every Startup file + every preloaded SKILL.md, in bytes.
pub fn startup_bytes(root: &Root, name: &str) -> Result<u64, Error> {
    let (skills, body, files) = parts(root, name)?;
    let mut total = body.len() as u64;
    for rel in paths_of(&skills, &files) {
        let (shown, fs) = root.join(&rel);
        total += std::fs::metadata(&fs)
            .map_err(|e| os_error(e, &shown))?
            .len();
    }
    Ok(total)
}

/// `agents_reading(root, target)`: agents with a startup file or preload at or under `target`.
pub fn agents_reading(root: &Root, target: &str) -> Result<Vec<String>, Error> {
    let target = target.trim_matches('/');
    if target.is_empty() {
        return Ok(Vec::new());
    }
    let under = format!("{target}/");
    let mut found = Vec::new();
    for name in agent_names(root) {
        if startup_paths(root, &name)?
            .iter()
            .any(|p| p == target || p.starts_with(&under))
        {
            found.push(name);
        }
    }
    Ok(found)
}

/// One `report` record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub agent: String,
    pub measured: u64,
    pub budget: Option<u64>,
}

/// `report(root, names)` against `budget` (the `BUDGET` dict; a parameter so tests replace it).
pub fn report(
    root: &Root,
    names: Option<&[String]>,
    budget: &[(&str, u64)],
) -> Result<Vec<Record>, Error> {
    let all;
    let names = match names {
        Some(n) => n,
        None => {
            all = agent_names(root);
            &all
        }
    };
    names
        .iter()
        .map(|n| {
            Ok(Record {
                agent: n.clone(),
                measured: startup_bytes(root, n)?,
                budget: budget.iter().find(|(k, _)| k == n).map(|&(_, v)| v),
            })
        })
        .collect()
}

/// The checkout this binary was built from (the Python's `__file__`-relative default root).
pub fn default_root() -> PathBuf {
    let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&built).unwrap_or(built)
}

/// `main`'s `ArgumentParser` (`prog` is the script's name, as `sys.argv[0]` gives it).
/// `COLUMNS` sets the help width, as `shutil.get_terminal_size` reads it.
fn spec(env: &Env) -> argparse::Spec {
    argparse::Spec::new("startup_budget.py")
        .description("Print each agent's startup size against its budget.")
        .width(argparse::help_width(env.get("COLUMNS"), None))
        .arg(
            Arg::positional("agents")
                .nargs(Nargs::ZeroOrMore)
                .help("agent names (default: all)"),
        )
        .arg(Arg::option(&["--root"]).help("repository root (default: this checkout)"))
}

/// Runs `anthology-dev startup-budget …`; `args` are the arguments after `startup-budget`.
pub fn run(args: &[OsString], stdin: &mut dyn Read) -> Output {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_in(args, stdin, &Env::from_process(), &cwd)
}

/// `startup_budget.main(args)` in `cwd`, with the real `BUDGET`.
pub fn run_in(args: &[OsString], stdin: &mut dyn Read, env: &Env, cwd: &Path) -> Output {
    let _ = stdin;
    main_with(args, env, cwd, &BUDGET)
}

/// `main(argv)` with `budget` in place of `BUDGET` (the `mock.patch.dict` seam).
pub fn main_with(args: &[OsString], env: &Env, cwd: &Path, budget: &[(&str, u64)]) -> Output {
    let parsed = match argparse::parse(&spec(env), args) {
        Ok(p) => p,
        Err(e) => {
            return Output {
                stdout: e.stdout.into_bytes(),
                stderr: e.stderr.into_bytes(),
                code: e.code,
            };
        }
    };
    let root = match parsed.os_str("root") {
        Some(r) => Root::new(Path::new(r), cwd),
        None => Root::new(&default_root(), cwd),
    };
    let agents: Vec<String> = parsed
        .list("agents")
        .iter()
        .map(|a| {
            a.as_os_str()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned())
        })
        .collect();
    let known = agent_names(&root);
    let unknown: Vec<&str> = agents
        .iter()
        .filter(|a| !known.contains(a))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Output {
            stdout: format!(
                "unknown agent(s): {}; known: {}\n",
                unknown.join(", "),
                known.join(", ")
            )
            .into_bytes(),
            code: 2,
            ..Output::default()
        };
    }
    let names = (!agents.is_empty()).then_some(agents.as_slice());
    let records = match report(&root, names, budget) {
        Ok(r) => r,
        Err(e) => return e.into_output("startup-budget", Vec::new()),
    };
    let mut out = String::new();
    let mut bad = 0;
    for rec in records {
        let status = match rec.budget {
            None => {
                bad += 1;
                "no budget set".to_string()
            }
            Some(b) if rec.measured > b => {
                bad += 1;
                format!(
                    "OVER; if intended set BUDGET to {}",
                    suggested_budget(rec.measured)
                )
            }
            Some(_) => "ok".to_string(),
        };
        let budget = rec.budget.map_or("None".to_string(), |b| b.to_string());
        out.push_str(&format!(
            "{}: {} bytes of {budget} budget ({status})\n",
            rec.agent, rec.measured
        ));
    }
    Output {
        stdout: out.into_bytes(),
        stderr: Vec::new(),
        code: i32::from(bad > 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pathlib_spelling() {
        assert_eq!(pathlib_str(""), ".");
        assert_eq!(pathlib_str("./a//b/./c/"), "a/b/c");
        assert_eq!(pathlib_str("//x/y"), "//x/y");
        assert_eq!(pathlib_str("///x"), "/x");
        assert_eq!(pathlib_str("a/../b"), "a/../b");
    }

    #[test]
    fn sections_follow_re_split_and_dict_zip() {
        let (names, map) = sections("intro\n## A \nx\n## B\ny\n## A\nz");
        assert_eq!(names, ["A", "B", "A"]);
        assert_eq!(
            map,
            [
                ("A".to_string(), "\nz".to_string()),
                ("B".to_string(), "\ny\n".to_string())
            ]
        );
    }

    #[test]
    fn frontmatter_list_lines() {
        assert!(is_skills_key("skills:") && is_skills_key("skills:\u{a0} "));
        assert!(!is_skills_key("skills: x") && !is_skills_key(" skills:"));
        assert_eq!(list_item("  - a:b "), Some("a:b"));
        assert_eq!(list_item("- a"), None);
        assert_eq!(list_item("  -a"), None);
        assert_eq!(list_item("  - a b"), None);
        assert_eq!(list_item("\u{3000}-\u{85}x"), Some("x"));
    }
}
