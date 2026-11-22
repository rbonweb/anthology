//! The guard against the SHIPPED Python guard (final review C-1, I-3): wherever
//! `plugins/anthology-guardrails/scripts/guard_rules.py` (as on `rust` before the Python removal)
//! blocks, Rust blocks too (the D-7 direction). Rust may be stricter; it may never allow more.
//!
//! `data/shipped_wrapper_cross.tsv` is frozen: one `<rule or ->\t<command>` line per case of the
//! reviewer's cross product (28 runners x 36 options x 4 G1 targets, 4,032 cases), each verdict from
//! the shipped `guard_rules.check("Bash", {"command": c}, "/home/tester")`. It was generated once:
//!
//! ```text
//! for r, o, t in itertools.product(RUNNERS, OPTIONS, TARGETS):
//!     c = " ".join(x for x in [r, o, t] if x)
//!     if r.endswith(":"):
//!         c = r + t if not o else r.split(" npm:")[0] + " " + o + " npm:" + t
//!     v = guard_rules.check("Bash", {"command": c}, "/home/tester")
//!     print((v[0] if v else "-") + "\t" + c)
//! ```
//!
//! The lists below are the same lists; the test rebuilds the commands and checks they are the
//! table's, line by line, so the table cannot drift from the product it claims to be.

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Rule, Verdict, check};

use gd::{HOME, bash, gh_token};

const RUNNERS: &[&str] = &[
    "npx",
    "yarn",
    "pnpm",
    "bunx",
    "bun x",
    "pnpm dlx",
    "pnpm exec",
    "npm exec",
    "yarn dlx",
    "yarn exec",
    "npm x",
    "sudo",
    "env",
    "nice",
    "timeout 5",
    "time",
    "command",
    "exec",
    "nohup",
    "xargs",
    "doas",
    "uvx",
    "pipx run",
    "corepack yarn",
    "corepack pnpm",
    "npx -p playwright",
    "bun",
    "deno run -A npm:",
];

const OPTIONS: &[&str] = &[
    "",
    "--silent",
    "--frozen-lockfile",
    "--verbose",
    "-s",
    "-q",
    "--yes",
    "-y",
    "--prefer-offline",
    "--no-install",
    "--ignore-scripts",
    "--foo",
    "--foo x",
    "-x",
    "--registry=https://r",
    "--registry https://r",
    "--cwd web",
    "-C web",
    "--filter web",
    "-w",
    "--offline",
    "--json",
    "--color=always",
    "--no-color",
    "--loglevel silent",
    "--quiet",
    "--package playwright",
    "-p playwright",
    "-n 5",
    "-u root",
    "-E",
    "-i",
    "-k 5",
    "--preserve-status",
    "--",
    "-v",
];

const TARGETS: &[&str] = &[
    "playwright install",
    "playwright install chromium",
    "puppeteer browsers install chrome",
    "@playwright/test install",
];

const TABLE: &str = include_str!("data/shipped_wrapper_cross.tsv");

fn verdict(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

fn commands() -> Vec<String> {
    let mut out = Vec::new();
    for r in RUNNERS {
        for o in OPTIONS {
            for t in TARGETS {
                let c = if let Some(base) = r.strip_suffix(" npm:") {
                    if o.is_empty() {
                        format!("{r}{t}")
                    } else {
                        format!("{base} {o} npm:{t}")
                    }
                } else {
                    [*r, *o, *t]
                        .iter()
                        .filter(|x| !x.is_empty())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                out.push(c);
            }
        }
    }
    out
}

#[test]
fn rust_blocks_every_wrapper_cross_case_the_shipped_python_blocks() {
    let rows: Vec<(&str, &str)> = TABLE
        .lines()
        .map(|l| l.split_once('\t').expect("a `rule\\tcommand` line"))
        .collect();
    let commands = commands();
    assert_eq!(rows.len(), 4032);
    assert_eq!(commands.len(), rows.len());
    let mut allowed = Vec::new();
    let mut blocked = 0;
    for ((rule, command), expected) in rows.iter().zip(&commands) {
        assert_eq!(command, expected, "the table is not the product it claims");
        if *rule == "-" {
            continue;
        }
        blocked += 1;
        if verdict(command) == Verdict::Allow {
            allowed.push(format!("{rule}\t{command}"));
        }
    }
    assert_eq!(blocked, 2460, "the frozen table's shipped blocks");
    assert!(
        allowed.is_empty(),
        "Rust allows {} case(s) the shipped Python blocks:\n{}",
        allowed.len(),
        allowed.join("\n")
    );
}

#[test]
fn an_unknown_option_before_a_g1_target_still_blocks() {
    // final review C-1: the shipped Python reads an unknown option as a flag with no value; that
    // reading is an alternate in Rust, and G1 on it blocks
    for command in [
        "yarn --frozen-lockfile playwright install",
        "pnpm --silent playwright install chromium",
        "yarn -s puppeteer browsers install chrome",
        "pnpm --verbose @playwright/test install",
        "yarn --silent brew install chromium",
        "pnpm --silent apt-get -y install chromium",
        "yarn --silent pip install playwright",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
}

#[test]
fn an_unknown_option_bypass_the_fw_reference_found_still_blocks() {
    // final review C-1: the shipped Python allows these (D-8 direction); the alternate closes them
    for command in [
        "npx --foo X playwright install",
        "env --frob X npx playwright install",
        "npm exec --foo x -- playwright install",
    ] {
        assert_ne!(verdict(command), Verdict::Allow, "{command}");
    }
}

#[test]
fn an_unknown_option_before_a_g2_launch_still_blocks() {
    // the shipped Python blocks both as G2
    for command in [
        "pnpm --silent open -a Chromium https://x",
        "yarn --silent open -a Chromium https://x",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G2), "{command}");
    }
}

#[test]
fn g3_and_g4_survive_an_unknown_option() {
    // final review I-3: not affected, pinned so they stay so
    assert_eq!(
        verdict("pnpm --silent cat ~/Library/Application\\ Support/Google/Chrome/Default/Cookies"),
        Verdict::Block(Rule::G3)
    );
    assert_eq!(
        verdict("yarn -s agent-browser open x"),
        Verdict::Block(Rule::G4)
    );
}

// ------------------------------------------------------------------ prefixes x targets (G1 to G4)

/// `data/shipped_prefix_cross.tsv`: the reviewer's second product, 55 prefixes x 29 targets
/// (1,595 cases) over G1 to G4, frozen the same way (`bash -c`, `sh -c` and `nix-shell --run` take the
/// target single-quoted, its own single quotes dropped).
const PREFIX_TABLE: &str = include_str!("data/shipped_prefix_cross.tsv");

const PREFIXES: &[&str] = &[
    "",
    "sudo",
    "sudo -E",
    "env",
    "env A=1",
    "A=1",
    "nice -n 5",
    "time",
    "command",
    "exec",
    "nohup",
    "timeout 5",
    "xargs",
    "npx",
    "pnpm",
    "yarn",
    "bunx",
    "npm exec --",
    "pnpm exec",
    "yarn exec",
    "bash -c",
    "sh -c",
    "eval",
    "watch",
    "caffeinate",
    "arch -arm64",
    "stdbuf -oL",
    "chrt 1",
    "ionice -c3",
    "taskset 1",
    "unbuffer",
    "script -q /dev/null",
    "doas",
    "dotenv --",
    "cross-env A=1",
    "npm-run-all",
    "concurrently",
    "rtk",
    "rtk run",
    "rtk bypass",
    "mise exec --",
    "asdf exec",
    "direnv exec .",
    "nix-shell --run",
    "devbox run",
    "pipx run",
    "uv run",
    "poetry run",
    "bundle exec",
    "pnpm --silent",
    "yarn --silent",
    "npm --silent exec",
    "pnpm -s dlx",
    "yarn -s dlx",
    "npm -s x",
];

const PREFIX_TARGETS: &[&str] = &[
    "npm i -g puppeteer",
    "npm install --save-dev playwright",
    "pnpm add -D @playwright/test",
    "yarn add puppeteer",
    "yarn global add playwright",
    "bun add puppeteer",
    "brew install chromium",
    "brew install --cask google-chrome",
    "apt-get -y install chromium",
    "apt install chromium-browser",
    "pip install playwright",
    "pip3 install pyppeteer",
    "python3 -m playwright install",
    "playwright install",
    "npx playwright install",
    "open -a \"Google Chrome\"",
    "open -a Chromium https://x",
    "\"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome\" --headless",
    "google-chrome --headless",
    "agent-browser open x",
    "cat ~/Library/Application\\ Support/Google/Chrome/Default/Cookies",
    "ls \"$HOME/Library/Application Support/Google/Chrome\"",
    "cp -r ~/.config/google-chrome /tmp/x",
    "uv pip install playwright",
    "uvx playwright install",
    "deno run -A npm:playwright install",
    "npm init playwright@latest",
    "pnpm create playwright",
    "yarn create playwright",
];

/// Shipped blocks Rust still allows until SH keeps the shipped reading of an unknown option as an
/// alternate (final review I-3). Remove each once SH lands; the list may only shrink.
const PENDING_SH: &[&str] = &[];

fn prefix_commands() -> Vec<String> {
    let mut out = Vec::new();
    for p in PREFIXES {
        for t in PREFIX_TARGETS {
            out.push(if ["bash -c", "sh -c", "nix-shell --run"].contains(p) {
                format!("{p} '{}'", t.replace('\'', ""))
            } else {
                format!("{p} {t}").trim().to_string()
            });
        }
    }
    out
}

#[test]
fn rust_blocks_every_prefix_cross_case_the_shipped_python_blocks() {
    let rows: Vec<(&str, &str)> = PREFIX_TABLE
        .lines()
        .map(|l| l.split_once('\t').expect("a `rule\\tcommand` line"))
        .collect();
    let commands = prefix_commands();
    assert_eq!(rows.len(), 1595);
    assert_eq!(commands.len(), rows.len());
    let mut allowed = Vec::new();
    let mut pending = Vec::new();
    for ((rule, command), expected) in rows.iter().zip(&commands) {
        assert_eq!(command, expected, "the table is not the product it claims");
        if *rule == "-" || verdict(command) != Verdict::Allow {
            continue;
        }
        if PENDING_SH.contains(command) {
            pending.push(*command);
        } else {
            allowed.push(format!("{rule}\t{command}"));
        }
    }
    assert_eq!(
        rows.iter().filter(|(r, _)| *r != "-").count(),
        1054,
        "the frozen table's shipped blocks"
    );
    assert!(
        allowed.is_empty(),
        "Rust allows {} case(s) the shipped Python blocks:\n{}",
        allowed.len(),
        allowed.join("\n")
    );
    assert_eq!(
        pending.len(),
        PENDING_SH.len(),
        "a PENDING_SH case now blocks: remove it from the list"
    );
}

// ------------------------------------------------------------------ runners x options x G6 writes

/// `data/shipped_g6_cross.tsv` (review-gd-final I-1): `echo {T} | <runner> <option> <write>` for the
/// runners above but `deno run -A npm:`, every option above, and three writes into memory (27 x 36 x 3 =
/// 2,916 cases), frozen the same way under HOME `/home/tester`. `{T}` stands for [`gh_token`], put in
/// at run time (and when the table was generated), so the repository holds no token shape.
const G6_TABLE: &str = include_str!("data/shipped_g6_cross.tsv");

/// `data/shipped_g6_pending_sh.txt`: the shipped G6 blocks Rust still allows until SH (#89,
/// port-sh-final) keeps the shipped reading of an unknown option as an alternate (`yarn --silent tee P`
/// read as `tee P`); G6 already reads alternates. Empty the file once #89 lands; it may only shrink.
const G6_PENDING_SH: &str = include_str!("data/shipped_g6_pending_sh.txt");

const G6_WRITES: &[&str] = &[
    "tee ~/.claude/agent-memory/a.md",
    "tee -a ~/.claude/agent-memory/a.md",
    "dd of=/home/tester/.claude/agent-memory/a.md",
];

#[test]
fn rust_blocks_every_g6_cross_case_the_shipped_python_blocks() {
    let rows: Vec<(&str, &str)> = G6_TABLE
        .lines()
        .map(|l| l.split_once('\t').expect("a `rule\\tcommand` line"))
        .collect();
    let mut commands = Vec::new();
    for r in RUNNERS.iter().filter(|r| !r.ends_with(" npm:")) {
        for o in OPTIONS {
            for w in G6_WRITES {
                let run = [*r, *o, *w]
                    .iter()
                    .filter(|x| !x.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" ");
                commands.push(format!("echo {{T}} | {run}"));
            }
        }
    }
    assert_eq!(rows.len(), 2916);
    assert_eq!(commands.len(), rows.len());
    let token = gh_token();
    let pending_sh: Vec<&str> = G6_PENDING_SH.lines().collect();
    let mut allowed = Vec::new();
    let mut pending = 0;
    for ((rule, command), expected) in rows.iter().zip(&commands) {
        assert_eq!(command, expected, "the table is not the product it claims");
        if *rule == "-" || verdict(&command.replace("{T}", &token)) != Verdict::Allow {
            continue;
        }
        if pending_sh.contains(command) {
            pending += 1;
        } else {
            allowed.push(format!("{rule}\t{command}"));
        }
    }
    assert_eq!(
        rows.iter().filter(|(r, _)| *r != "-").count(),
        1845,
        "the frozen table's shipped blocks"
    );
    assert!(
        allowed.is_empty(),
        "Rust allows {} case(s) the shipped Python blocks:\n{}",
        allowed.len(),
        allowed.join("\n")
    );
    assert_eq!(
        pending,
        pending_sh.len(),
        "a shipped_g6_pending_sh.txt case now blocks: remove it from the file"
    );
}

#[test]
fn a_secret_written_to_memory_behind_an_unknown_option_is_g6() {
    // review-gd-final I-1: the shipped reading (an alternate) writes memory
    let token = gh_token();
    for command in [
        "echo {T} | pnpm --silent tee ~/.claude/agent-memory/a.md",
        "echo {T} | yarn --frozen-lockfile tee ~/.claude/agent-memory/a.md",
        "echo {T} | pnpm -s dd of=/home/tester/.claude/agent-memory/a.md",
    ] {
        let command = command.replace("{T}", &token);
        assert_eq!(verdict(&command), Verdict::Block(Rule::G6), "{command}");
    }
}
