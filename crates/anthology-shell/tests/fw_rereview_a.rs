//! SH-FW re-review, part A (lane SH): a git alias without `!` (C-2a) and `TAR_OPTIONS` (C-1a).
//!
//! Every "runs" row is a re-review payload or a form of it that git or GNU tar reads the same way (a chained
//! or case-changed alias, options inside the alias, `env` / `sudo` / `sh -c` around it). The Python
//! reference misses each of them, so Rust is stricter (D-8). Every "clear" row is a negative control.
//!
//! The G1 helper below is fw_review_a.rs's: `analyze()` does not decide G1, the guard does.

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

/// Each command runs a browser install in bash: the analysis must show it (complete, not by a budget stop).
#[track_caller]
fn assert_runs(commands: &[&str]) {
    let missed: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (a.incomplete || !installs(&a)).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(missed.is_empty(), "missed installs:\n{}", missed.join("\n"));
}

/// No command is a browser install (negative controls): no entry and no alternate reading is G1.
#[track_caller]
fn assert_clear(commands: &[&str]) {
    let wrong: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (a.incomplete || installs(&a)).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(wrong.is_empty(), "false installs:\n{}", wrong.join("\n"));
}

// ------------------------------------------------------------------ C-2a. a git alias without `!`

#[test]
fn a_git_alias_without_bang_is_read_as_its_expansion() {
    assert_runs(&[
        "git -c alias.cl=clone cl -u 'npx playwright install' . ../c",
        "sudo git -c alias.cl=clone cl -u 'npx playwright install' . ../c",
        "sh -c \"git -c alias.cl=clone cl -u 'npx playwright install' . ../c\"",
        "git -c alias.CL=clone cl -u 'npx playwright install' . ../c",
        "git -c alias.cl=clone CL -u 'npx playwright install' . ../c",
        "git -c alias.cl='clone -u \"npx playwright install\"' cl . ../c",
        "git -c alias.f=fetch f --upload-pack='npx playwright install' origin",
        "git -c alias.p=push p --receive-pack='npx playwright install' . HEAD",
        "git -c alias.a=b -c alias.b=clone a -u 'npx playwright install' . ../c",
        "git -c alias.f='-c remote.o.uploadpack=npx\\ playwright\\ install fetch' f o",
        "git -c alias.r='rebase -x' r 'npx playwright install' main",
    ]);
    assert_clear(&[
        "git -c alias.st=status st",
        "git -c alias.co=checkout co main",
        "git -c alias.cl=clone cl https://example.com/r.git",
        "git -c alias.cl=cl cl -v",
        "git clone https://example.com/r.git",
        "git push -u origin main",
        "git -c alias.p=push p -u origin main",
    ]);
}

// ------------------------------------------------------------------ C-1a. TAR_OPTIONS

#[test]
fn tar_options_are_read_as_tar_arguments() {
    assert_runs(&[
        "TAR_OPTIONS='--checkpoint=1 --checkpoint-action=exec=npx\\ playwright\\ install' tar -cf b.tar a",
        "sudo TAR_OPTIONS='--checkpoint=1 --checkpoint-action=exec=npx\\ playwright\\ install' tar -cf b.tar a",
        "sh -c \"TAR_OPTIONS='--checkpoint=1 --checkpoint-action=exec=npx\\ playwright\\ install' tar -cf b.tar a\"",
        "env TAR_OPTIONS='--checkpoint=1 --checkpoint-action=exec=npx\\ playwright\\ install' tar -cf b.tar a",
        "TAR_OPTIONS=\"--to-command='npx playwright install'\" tar -xf a.tar",
        "TAR_OPTIONS='-I \"npx playwright install\"' gtar -xf a.tar",
    ]);
    assert_clear(&[
        "TAR_OPTIONS=--verbose tar -xf a.tar",
        "tar --checkpoint=1000 -xf a.tar",
        "TAR_OPTIONS='--checkpoint=1000 --checkpoint-action=dot' tar -cf b.tar a",
        "env TAR_OPTIONS=--verbose tar -xf a.tar",
    ]);
}
