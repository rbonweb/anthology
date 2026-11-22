//! SH-FW re-review, the lead's part (lane SH): brace readings only for unquoted braces (F-1) and too complex
//! when they change the program word (C-3a), every fed line read (F-2), printf's `--` and filled format
//! (C-4a), `printf -v<name>` and `unset` subscripts (C-5a), compound commands and function calls piped to a
//! shell (I-2a), and linear dedupes (I-cost).
//!
//! Every "runs" row is a re-review payload or a form bash runs the same way (bash 5.2 and 3.2). The Python
//! reference misses each of them, so Rust is stricter (D-8, or the oracle applies the rule too). Every
//! "clear" row is a negative control. The G1 helper below is fw_review_a.rs's.

use std::time::{Duration, Instant};

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

/// Each command is blocked: an install entry or alternate, or too complex (fail-closed families).
#[track_caller]
fn assert_blocked(commands: &[&str]) {
    let missed: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (!a.incomplete && !installs(&a)).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(missed.is_empty(), "allowed:\n{}", missed.join("\n"));
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

/// Each command is too complex to check: the analysis is incomplete (the guard blocks).
#[track_caller]
fn assert_incomplete(commands: &[&str]) {
    let wrong: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (!a.incomplete).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(wrong.is_empty(), "complete:\n{}", wrong.join("\n"));
}

// ------------------------------------------------------------------ C-3a. brace-built program words

#[test]
fn a_brace_reading_that_changes_the_program_word_is_too_complex() {
    assert_incomplete(&[
        "{echo,'npx playwright install'} | sh",
        "{xargs,npx} <<< 'playwright install'",
        "{printf,'npx playwright install'} | bash",
        "sudo {echo,'npx playwright install'} | sh",
    ]);
    assert_clear(&[
        "cp a.{js,ts} dir",
        "{ echo a,b; } | cat",
        "echo {a,b} > out.txt",
        "echo {1,2,3} | xargs -n1 echo",
        "ls {src,test}/*.rs",
    ]);
}

// ------------------------------------------------------------------ C-4a. printf

#[test]
fn printf_text_is_read_without_its_dashes_and_with_its_format_filled() {
    assert_runs(&[
        "printf -- 'npx\\x20playwright install' | sh",
        "printf -- 'npx playwright install' | sh",
        "printf 'npx %s install\\n' playwright | sh",
        "printf 'npx%splaywright install' ' ' | sh",
        "printf '%c%s' n 'px playwright install' | sh",
        "printf '%s %s %s\\n' npx playwright install | bash",
        "printf '%b' 'npx\\x20playwright install' | sh",
        "printf '%s\\n' 'npx playwright install' | xargs -I{} sh -c '{}'",
        "printf -- '%s' 'npx playwright install' | sh",
        "sudo sh -c \"printf 'npx %s install' playwright | sh\"",
    ]);
    assert_clear(&[
        "printf '%s\\n' a b c | xargs -n1 echo",
        "printf '%-10s|%5d\\n' name 3",
        "printf -- '--help\\n' | grep -- --help",
        "printf '%s\\n' 'npx playwright install' > notes.txt",
        "printf '%s\\n' 'npx playwright install' | grep npx",
    ]);
}

// ------------------------------------------------------------------ C-5a. subscripts

#[test]
fn attached_printf_v_and_unset_run_subscripts() {
    assert_blocked(&[
        "printf -v'a[$(npx playwright install)]' x",
        "a=(1); unset 'a[$(npx playwright install)]'",
        "unset -v 'a[$(npx playwright install)]'",
        "test -v'a[$(npx playwright install)]'",
    ]);
    assert_clear(&[
        "unset HOME",
        "unset -f f",
        "unset 'a[1]'",
        "printf -vout '%s' x",
    ]);
}

// ------------------------------------------------------------------ I-2a. compound output piped

#[test]
fn compound_commands_and_function_calls_piped_to_a_shell_run_their_output() {
    assert_runs(&[
        "( echo npx playwright install ) | sh",
        "{ echo npx playwright install; } | sh",
        "if true; then echo npx playwright install; fi | sh",
        "f() { echo npx playwright install; }; f | sh",
        "function f { echo npx playwright install; }; f | sh",
        "for i in 1; do echo npx playwright install; done | sh",
        "while true; do echo npx playwright install; break; done | bash",
        "case x in x) echo npx playwright install;; esac | sh",
        "( printf 'npx %s install' playwright ) | sh",
        "{ echo npx playwright install; } | xargs -I{} sh -c '{}'",
        "( echo npx playwright install ) |\nsh",
        // `function f` bodies are commands too
        "function f { npx playwright install; }; f",
    ]);
    // fail closed: a producer that is not literal text before a shell
    assert_incomplete(&["echo hi; cat x | sh"]);
    assert_clear(&[
        "( cd web && npm ci ) | tee log",
        "{ echo a; echo b; } > out.txt",
        "if true; then echo ok; fi | grep ok",
        "f() { echo hi; }; f | sh",
        "( echo hi ) | sh",
        "git commit -m \"$(cat <<'EOF'\nmsg\nEOF\n)\"",
    ]);
}

// ------------------------------------------------------------------ I-3. the shipped reading of package-manager options

/// `pnpm --silent open -a Chromium https://x`: the value-taking reading reads `x` as the program, so the shipped
/// reading (each unknown option a flag with no value: `open -a Chromium https://x`, which G2 blocks) must be an
/// alternate. npm and bun have no bin reading: the shipped reading is the package manager itself, verbatim.
#[test]
fn shipped_readings_of_package_manager_options_are_alternates() {
    for command in [
        "pnpm --silent open -a Chromium https://x",
        "yarn --silent open -a Chromium https://x",
        "sudo pnpm --silent open -a Chromium https://x",
    ] {
        let a = analyze(command, HOME);
        assert!(
            a.alternates
                .iter()
                .any(|e| e.prog == "open" && e.args == ["-a", "Chromium", "https://x"]),
            "{command}: {}",
            summary(&a)
        );
    }
    let a = analyze("npm --silent open exec playwright install", HOME);
    assert!(
        a.alternates
            .iter()
            .any(|e| e.prog == "npm"
                && e.args == ["--silent", "open", "exec", "playwright", "install"]),
        "{}",
        summary(&a)
    );
    // no misread: the shipped reading is the Rust one, so there is no extra alternate
    let a = analyze("pnpm --silent install", HOME);
    assert!(a.alternates.is_empty(), "{}", summary(&a));
}

// ------------------------------------------------------------------ cost

#[test]
fn re_review_shapes_stay_linear_at_100_kb() {
    let fill = |unit: &str, bytes: usize| unit.repeat(bytes / unit.len());
    let units = [
        "echo -e 'a\\tN' | ",
        "echo 'a N' | ",
        "( echo a ) | sh; ",
        "f() { echo a; }; f | sh; ",
        "fi | sh; ",
        "printf 'a %s\\n' b | sh; ",
        "printf '%s%s%s' a b c d e f | sh; ",
        "echo 'a b' | xargs ",
        "parallel echo ::: a b c d e f g h; ",
        "mkdir -p x/{a,b,c}/{d,e,f}; ",
        "unset 'a[1]'; ",
    ];
    for unit in units {
        let mut best = None;
        for _ in 0..3 {
            let small = {
                let text = fill(unit, 25_000);
                let start = Instant::now();
                analyze(&text, HOME);
                start.elapsed()
            };
            let text = fill(unit, 100_000);
            let start = Instant::now();
            let a = analyze(&text, HOME);
            let large = start.elapsed();
            // 4x input costs less than 10x (floor 0.5 s); 8 s is only a catastrophe guard
            let ok = large < Duration::from_secs(8)
                && large < (small * 10).max(Duration::from_millis(500));
            best = Some((ok, small, large, a.incomplete));
            if ok {
                break;
            }
        }
        let (ok, small, large, incomplete) = best.unwrap();
        assert!(
            ok,
            "{unit:?}: 25 KB {small:?}, 100 KB {large:?} (incomplete {incomplete})"
        );
    }
}
