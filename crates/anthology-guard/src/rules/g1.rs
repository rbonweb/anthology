//! G1: browser-automation installs on the host (guard_rules.py `_g1` and its helpers).
//!
//! Rust stricter (owner direction, FW-I; difftest class D-8): two readings the Python on `rust`
//! does not have are added, each as a union with the Python reading, so Rust never allows what
//! Python blocks.
//! - A package manager's words are also read as FW-I's `_pm_words` reads them: more value
//!   flags (`--loglevel`, `--reporter`, ...), and before the subcommand a word that follows an
//!   unknown option is that option's value (`npm --loglevel silent i puppeteer`).
//! - The OS installers' value options before the verb are skipped
//!   (`apt-get -o Dpkg::Options::=x install chromium`).

use anthology_pycompat::str as pystr;
use anthology_shell::Entry;

use super::{JS_PACKAGE_MANAGERS, first_arg, positional, strip_version};

const PLAYWRIGHT: &[&str] = &[
    "playwright",
    "playwright-core",
    "@playwright/test",
    "@playwright/cli",
];
const PLAYWRIGHT_SUBCOMMANDS: &[&str] = &[
    "install",
    "install-deps",
    "test",
    "codegen",
    "open",
    "cr",
    "chromium",
    "ff",
    "firefox",
    "wk",
    "webkit",
    "screenshot",
    "pdf",
];
const PUPPETEER: &[&str] = &["puppeteer", "@puppeteer/browsers"];
const PUPPETEER_SUBCOMMANDS: &[&str] = &["browsers", "install"];
const JS_INSTALL_VERBS: &[&str] = &["i", "install", "add"];
const OS_INSTALLERS: &[&str] = &["brew", "apt", "apt-get", "dnf", "yum", "snap", "pacman"];

/// `_PM_VALUE_FLAGS` as the Python on `rust` has it.
const PM_VALUE_FLAGS: &[&str] = &[
    "--filter",
    "--registry",
    "-F",
    "-C",
    "--dir",
    "--prefix",
    "-w",
    "--workspace",
    "--cwd",
];

/// FW-I's `_PM_VALUE_FLAGS`: the Python table plus the flags FW-I added.
const FW_PM_VALUE_FLAGS: &[&str] = &[
    "--filter",
    "-F",
    "-C",
    "--dir",
    "--prefix",
    "-w",
    "--workspace",
    "--cwd",
    "--registry",
    "--loglevel",
    "--userconfig",
    "--cache",
    "--reporter",
    "--tag",
    "--node-options",
    "--script-shell",
];

/// `_PM_VERBS`.
const PM_VERBS: &[&str] = &[
    "add",
    "install",
    "i",
    "remove",
    "rm",
    "uninstall",
    "un",
    "r",
    "run",
    "exec",
    "dlx",
    "x",
    "init",
    "publish",
    "link",
    "unlink",
    "update",
    "upgrade",
    "up",
    "test",
    "start",
    "build",
    "ci",
    "cache",
    "config",
    "info",
    "list",
    "ls",
    "outdated",
    "audit",
    "why",
    "pack",
    "version",
    "workspace",
    "workspaces",
    "global",
    "create",
    "login",
    "logout",
    "whoami",
    "help",
    "prune",
    "store",
    "rebuild",
    "dedupe",
    "import",
    "patch",
    "set",
    "get",
    "plugin",
    "node",
    "bin",
    "dev",
];

/// FW-I's `_APT_VALUE_FLAGS`.
const APT_VALUE_FLAGS: &[&str] = &[
    "-o",
    "--option",
    "-c",
    "--config-file",
    "-t",
    "--target-release",
    "--default-release",
];
/// FW-I's `_DNF_VALUE_FLAGS` (dnf and yum).
const DNF_VALUE_FLAGS: &[&str] = &[
    "-c",
    "--config",
    "--setopt",
    "--releasever",
    "--installroot",
    "-x",
    "--exclude",
    "--repo",
    "--repoid",
    "--enablerepo",
    "--disablerepo",
    "-d",
    "--debuglevel",
    "-e",
    "--errorlevel",
];
const SNAP_VALUE_FLAGS: &[&str] = &["--channel", "--revision"];

/// FW-I's `_OS_INSTALLER_VALUE_FLAGS.get(prog, ())`.
fn os_installer_value_flags(prog: &str) -> &'static [&'static str] {
    match prog {
        "apt" | "apt-get" => APT_VALUE_FLAGS,
        "dnf" | "yum" => DNF_VALUE_FLAGS,
        "snap" => SNAP_VALUE_FLAGS,
        _ => &[],
    }
}

/// `_BROWSER_PACKAGES` as `fnmatch.fnmatchcase` reads them: a trailing `*` is a prefix match,
/// and `*` matches any text (newlines included), so each pattern is a prefix or an exact name.
fn is_browser_package(name: &str) -> bool {
    ["google-chrome", "chromium", "playwright"]
        .iter()
        .any(|p| name.starts_with(p))
        || name == "ungoogled-chromium"
        || name == "chromedriver"
}

/// `_skip_flags(tokens, i, value_flags)` (FW-I's `_pm_verb_index` uses it). A cluster such as
/// `-iu` takes a value when `-` plus its last character is a value flag; every value flag here
/// is `-` plus an ASCII letter, so Python's `isalpha()` test adds nothing.
fn skip_flags(tokens: &[&str], mut i: usize, value_flags: &[&str]) -> usize {
    while i < tokens.len() {
        let token = tokens[i];
        if token == "--" {
            return i + 1;
        }
        if pystr::char_len(token) > 1 && token.starts_with('-') {
            let last = token.chars().last().map(String::from).unwrap_or_default();
            let takes_value = value_flags.contains(&token)
                || (!token[1..].starts_with('-')
                    && pystr::char_len(token) > 2
                    && value_flags.contains(&format!("-{last}").as_str()));
            i += if takes_value { 2 } else { 1 };
        } else {
            break;
        }
    }
    i
}

/// FW-I's `_unknown_option_before(toks, i)`.
fn unknown_option_before(toks: &[&str], i: usize) -> bool {
    let prev = if i > 0 { toks[i - 1] } else { "" };
    pystr::char_len(prev) > 1
        && prev.starts_with('-')
        && prev != "--"
        && !prev.contains('=')
        && !FW_PM_VALUE_FLAGS.contains(&prev)
}

/// FW-I's `_pm_verb_index(toks)`.
fn pm_verb_index(toks: &[&str]) -> usize {
    let n = toks.len();
    let mut i = skip_flags(toks, 1, FW_PM_VALUE_FLAGS);
    while i < n && !PM_VERBS.contains(&toks[i]) && unknown_option_before(toks, i) {
        i = skip_flags(toks, i + 1, FW_PM_VALUE_FLAGS);
    }
    if i < n {
        i
    } else {
        skip_flags(toks, 1, FW_PM_VALUE_FLAGS)
    }
}

/// FW-I's `_pm_words(args)`: the positional words, the subcommand first.
fn pm_words(args: &[String]) -> Vec<String> {
    let mut toks: Vec<&str> = vec![""];
    toks.extend(args.iter().map(String::as_str));
    // `skip_flags` starts at 1, so `i >= 1`; it may step past the end (`toks[i:]` is then empty).
    let i = pm_verb_index(&toks).min(toks.len());
    let mut words: Vec<String> = positional(&toks[1..i], FW_PM_VALUE_FLAGS)
        .into_iter()
        .filter(|w| PM_VERBS.contains(w))
        .map(str::to_string)
        .collect();
    words.extend(
        positional(&toks[i..], FW_PM_VALUE_FLAGS)
            .into_iter()
            .map(str::to_string),
    );
    words
}

/// The package-manager readings: Python's `_positional(args, _PM_VALUE_FLAGS)`, then FW-I's
/// unless `fw` is off.
fn pm_readings(args: &[String], fw: bool) -> Vec<Vec<String>> {
    let python = positional(args, PM_VALUE_FLAGS)
        .into_iter()
        .map(str::to_string)
        .collect();
    if fw {
        vec![python, pm_words(args)]
    } else {
        vec![python]
    }
}

/// `_js_installs_puppeteer(entry)` for one reading of the words.
fn installs_puppeteer(words: &[String]) -> bool {
    let words = match words.first() {
        Some(w) if w == "global" => &words[1..],
        _ => words,
    };
    match words.split_first() {
        Some((verb, rest)) if JS_INSTALL_VERBS.contains(&verb.as_str()) => rest
            .iter()
            .any(|w| pystr::lower(strip_version(w)) == "puppeteer"),
        _ => false,
    }
}

/// `_js_scaffolds_playwright(entry)` for one reading of the words: `npm init playwright`,
/// `yarn create playwright` download browsers unless told not to.
fn scaffolds_playwright(words: &[String], args: &[String]) -> bool {
    words.len() > 1
        && (words[0] == "init" || words[0] == "create")
        && pystr::lower(strip_version(&words[1])) == "playwright"
        && !args.iter().any(|a| a == "--no-browsers")
}

/// `_pip_installs_playwright(entry)`.
fn pip_installs_playwright(entry: &Entry) -> bool {
    let mut words = positional(&entry.args, &[]);
    if entry.prog == "uv" {
        if words.first() != Some(&"pip") {
            return false;
        }
        words.remove(0);
    }
    if words.first() != Some(&"install") {
        return false;
    }
    words[1..].iter().any(|spec| {
        let spec = pystr::lower(spec);
        let name = spec
            .split(['=', '<', '>', '!', '~', '[', ';', '@', ' '])
            .next()
            .unwrap_or("");
        name.replace('_', "-") == "playwright"
    })
}

/// `_PACMAN_SYNC.match(arg)`: `-S[yuw]*\Z`.
fn is_pacman_sync(arg: &str) -> bool {
    arg.strip_prefix("-S")
        .is_some_and(|rest| rest.chars().all(|c| matches!(c, 'y' | 'u' | 'w')))
}

/// `_os_installs_browser(entry)` for one reading (`value_flags` skipped before the verb).
fn os_installs_browser(entry: &Entry, value_flags: &[&str]) -> bool {
    let words = positional(&entry.args, value_flags);
    let packages: &[&str] = if entry.prog == "pacman" {
        if !entry.args.iter().any(|a| is_pacman_sync(a)) {
            return false;
        }
        &words
    } else {
        let verbs: &[&str] = if entry.prog == "brew" {
            &["install", "reinstall", "upgrade"]
        } else {
            &["install"]
        };
        match words.first() {
            Some(verb) if verbs.contains(verb) => &words[1..],
            _ => return false,
        }
    };
    packages.iter().any(|package| {
        let lowered = pystr::lower(package);
        let last = lowered.rsplit('/').next().unwrap_or("");
        is_browser_package(strip_version(last))
    })
}

/// `_PIP.match(prog)`: `pip[0-9.]*\Z`.
fn is_pip(prog: &str) -> bool {
    prog.strip_prefix("pip")
        .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}

/// `_g1(entry)`, with FW-I's readings added (module docs).
pub fn g1(entry: &Entry) -> bool {
    g1_with(entry, true)
}

/// `_g1(entry)`; `fw` off leaves out FW-I's readings (Python's G1 exactly; the difftest generator
/// uses it to keep D-8 shapes out of generated cases).
pub(crate) fn g1_with(entry: &Entry, fw: bool) -> bool {
    if entry.exempt {
        return false;
    }
    let prog = entry.prog.as_str();
    let sub = first_arg(&entry.args);
    if PLAYWRIGHT.contains(&prog) {
        return sub.is_some_and(|s| PLAYWRIGHT_SUBCOMMANDS.contains(&s));
    }
    if PUPPETEER.contains(&prog) {
        return sub.is_some_and(|s| PUPPETEER_SUBCOMMANDS.contains(&s));
    }
    if prog == "create-playwright" {
        return !entry.args.iter().any(|a| a == "--no-browsers");
    }
    if JS_PACKAGE_MANAGERS.contains(&prog) {
        return pm_readings(&entry.args, fw)
            .iter()
            .any(|words| installs_puppeteer(words) || scaffolds_playwright(words, &entry.args));
    }
    if is_pip(prog) || prog == "pipx" || prog == "uv" {
        return pip_installs_playwright(entry);
    }
    if OS_INSTALLERS.contains(&prog) {
        return os_installs_browser(entry, &[])
            || (fw && os_installs_browser(entry, os_installer_value_flags(prog)));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(ws: &[&str]) -> Vec<String> {
        ws.iter().map(|w| (*w).to_string()).collect()
    }

    #[test]
    fn pm_words_reads_an_unknown_options_value_before_the_verb() {
        assert_eq!(
            pm_words(&words(&["--loglevel-x", "silent", "i", "puppeteer"])),
            ["i", "puppeteer"]
        );
        assert_eq!(
            pm_words(&words(&["--foo-bar", "silent", "init", "playwright"])),
            ["init", "playwright"]
        );
        // a known verb after an unknown option is the verb
        assert_eq!(
            pm_words(&words(&["--frozen-lockfile", "install"])),
            ["install"]
        );
        // running off the end falls back to the plain skip
        assert_eq!(pm_words(&words(&["--x", "y"])), ["y"]);
        assert_eq!(pm_words(&words(&[])), Vec::<String>::new());
    }

    #[test]
    fn skip_flags_reads_clusters_ending_in_a_value_flag() {
        let toks = ["", "-xF", "v", "run"];
        assert_eq!(skip_flags(&toks, 1, FW_PM_VALUE_FLAGS), 3);
        let toks = ["", "--", "run"];
        assert_eq!(skip_flags(&toks, 1, FW_PM_VALUE_FLAGS), 2);
    }

    #[test]
    fn browser_packages_follow_fnmatch() {
        for name in [
            "google-chrome",
            "google-chrome-stable",
            "chromium-browser",
            "chromium",
            "ungoogled-chromium",
            "chromedriver",
            "playwright",
            "playwright-core",
        ] {
            assert!(is_browser_package(name), "{name}");
        }
        for name in [
            "chromedriver2",
            "ungoogled-chromium-x",
            "jq",
            "google-chrom",
        ] {
            assert!(!is_browser_package(name), "{name}");
        }
    }
}
