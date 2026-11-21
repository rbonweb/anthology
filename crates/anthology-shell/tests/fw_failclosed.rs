//! SH-FW narrow verification (controller ruling: close each family by failing closed). The payloads are the
//! verifier's probe set (`fixtures/sh_fw_verify_payloads.txt`: brace sequences and quoted brace items,
//! subscripts holding `$(`, command-running environment, compound or non-literal producers before a shell,
//! printf forms); each must be blocked: an install entry or alternate, an `rm` it shows, or too complex.
//! The verifier's benign list (`fixtures/sh_fw_verify_benign.txt`) must stay complete with no install.
//! One probe is left out: `printf '%b' 'npx\x20%s install' playwright | sh` prints `npx %s installplaywright`
//! in bash, which installs nothing.

use anthology_shell::{Analysis, Entry, analyze};

const HOME: &str = "/home/tester";

// ------------------------------------------------------------------ G1, as the guard reads an entry

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
const OS_INSTALLERS: &[&str] = &["brew", "apt", "apt-get", "dnf", "yum", "snap", "pacman"];
const APT_VALUE_FLAGS: &[&str] = &[
    "-o",
    "--option",
    "-c",
    "--config-file",
    "-t",
    "--target-release",
    "--default-release",
];
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

fn strip_version(word: &str) -> &str {
    match word.rfind('@') {
        Some(at) if at > 0 => &word[..at],
        _ => word,
    }
}

fn first_arg(args: &[String]) -> Option<&str> {
    args.iter()
        .map(String::as_str)
        .find(|a| !a.starts_with('-'))
}

fn positional<'a>(args: &'a [String], value_flags: &[&str]) -> Vec<&'a str> {
    let mut words = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if value_flags.contains(&args[i].as_str()) {
            i += 1;
        } else if !args[i].starts_with('-') {
            words.push(args[i].as_str());
        }
        i += 1;
    }
    words
}

fn is_pip(prog: &str) -> bool {
    prog.strip_prefix("pip")
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// guard_rules.py `_g1` for the programs below (playwright, puppeteer, pip, the OS installers).
fn g1(e: &Entry) -> bool {
    if e.exempt {
        return false;
    }
    let prog = e.prog.as_str();
    let sub = first_arg(&e.args);
    if PLAYWRIGHT.contains(&prog) {
        return sub.is_some_and(|s| PLAYWRIGHT_SUBCOMMANDS.contains(&s));
    }
    if prog == "puppeteer" || prog == "@puppeteer/browsers" {
        return sub.is_some_and(|s| s == "browsers" || s == "install");
    }
    if ["npm", "pnpm", "yarn", "bun"].contains(&prog) {
        let words = positional(&e.args, &[]);
        return words
            .first()
            .is_some_and(|v| ["i", "install", "add"].contains(v))
            && words[1..]
                .iter()
                .any(|w| strip_version(w).eq_ignore_ascii_case("puppeteer"));
    }
    if is_pip(prog) || prog == "pipx" || prog == "uv" {
        let mut words = positional(&e.args, &[]);
        if prog == "uv" {
            if words.first() != Some(&"pip") {
                return false;
            }
            words.remove(0);
        }
        return words.first() == Some(&"install")
            && words[1..].iter().any(|spec| {
                let spec = spec.to_lowercase();
                let base = spec
                    .split(['=', '<', '>', '!', '~', '[', ';', '@', ' '])
                    .next()
                    .unwrap_or("");
                base.replace('_', "-") == "playwright"
            });
    }
    if OS_INSTALLERS.contains(&prog) {
        let flags: &[&str] = match prog {
            "apt" | "apt-get" => APT_VALUE_FLAGS,
            "dnf" | "yum" => DNF_VALUE_FLAGS,
            "snap" => &["--channel", "--revision"],
            _ => &[],
        };
        let words = positional(&e.args, flags);
        let packages = if prog == "pacman" {
            if !e.args.iter().any(|a| {
                a.strip_prefix("-S")
                    .is_some_and(|r| r.chars().all(|c| "yuw".contains(c)))
            }) {
                return false;
            }
            &words[..]
        } else {
            let verbs: &[&str] = if prog == "brew" {
                &["install", "reinstall", "upgrade"]
            } else {
                &["install"]
            };
            if !words.first().is_some_and(|v| verbs.contains(v)) {
                return false;
            }
            &words[1..]
        };
        return packages.iter().any(|p| {
            let p = p.to_lowercase();
            let name = strip_version(p.rsplit('/').next().unwrap_or(""));
            name.starts_with("google-chrome")
                || name.starts_with("chromium")
                || name.starts_with("playwright")
                || name == "ungoogled-chromium"
                || name == "chromedriver"
        });
    }
    false
}

/// An entry or an alternate reading of `a` is a G1 install.
fn installs(a: &Analysis) -> bool {
    a.entries.iter().chain(&a.alternates).any(g1)
}

fn summary(a: &Analysis) -> String {
    let show = |e: &Entry| {
        format!(
            "{} {:?}{}",
            e.prog,
            e.args,
            if e.exempt { " (exempt)" } else { "" }
        )
    };
    format!(
        "entries {:?}, alternates {:?}, incomplete {}",
        a.entries.iter().map(show).collect::<Vec<_>>(),
        a.alternates.iter().map(show).collect::<Vec<_>>(),
        a.incomplete
    )
}

const SEP: &str = "\n#---\n";

fn cases(text: &str) -> Vec<&str> {
    text.split(SEP)
        .map(|c| c.trim_end_matches('\n'))
        .filter(|c| !c.trim().is_empty())
        .collect()
}

#[test]
fn every_verified_payload_is_blocked() {
    let missed: Vec<String> = cases(include_str!("fixtures/sh_fw_verify_payloads.txt"))
        .into_iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            let shown = installs(&a)
                || a.entries
                    .iter()
                    .chain(&a.alternates)
                    .any(|e| e.prog == "rm");
            (!a.incomplete && !shown).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(missed.is_empty(), "allowed:\n{}", missed.join("\n"));
}

#[test]
fn the_benign_list_stays_allowed() {
    let wrong: Vec<String> = cases(include_str!("fixtures/sh_fw_verify_benign.txt"))
        .into_iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (a.incomplete || installs(&a)).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(wrong.is_empty(), "blocked:\n{}", wrong.join("\n"));
}
