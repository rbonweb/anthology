//! SH-FW review, part B (collection): I-1, I-3 (collection part), C-3, C-4, C-5 of the review of the FW-H/FW-I
//! hardening.
//!
//! Every "runs" row is a command the review ran in bash 5.2 (most also in 3.2); the reference allows each, so
//! Rust is stricter here (D-8). Every "clear" row is an everyday form of the same programs, which must stay
//! complete and must not read as a browser install. The G1 helpers are copied from `fw_i.rs`.

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

/// Each command is too complex to check (a cap would drop input): the analysis is incomplete.
#[track_caller]
fn assert_incomplete(commands: &[&str]) {
    let missed: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (!a.incomplete).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(missed.is_empty(), "complete:\n{}", missed.join("\n"));
}

// ------------------------------------------------------------------ I-1. fd short clusters

#[test]
fn fd_clusters_ending_in_x_run_their_command() {
    assert_runs(&[
        "fdfind -Hx npx playwright install",
        "fdfind -HX npx playwright install",
        "fd -Hx npx playwright install",
        "fd -HIx npx playwright install",
        "fd -0X npx playwright install",
        "fd . -Hx echo hi \\; -HX npx playwright install",
    ]);
    // `-tx` is a file type, not exec; a cluster with a value letter is not an exec option
    assert_clear(&[
        "fd -H pattern",
        "fd -Hx echo {}",
        "fd -tx playwright install",
        "fd -HI -e rs playwright install",
        "fdfind -Hx rm {} \\;",
    ]);
}

// ------------------------------------------------------------------ C-4. escapes in echo / printf text

#[test]
fn echo_and_printf_escapes_are_decoded_for_shells_and_xargs() {
    assert_runs(&[
        "echo -e 'npx\\x20playwright install' | sh",
        "printf 'npx\\x20playwright install' | sh",
        "printf '%b' 'npx\\0040playwright install' | sh",
        "printf 'npx\\040playwright install' | bash -s",
        "echo -e 'npx\\tplaywright install' | sh",
        "printf '%s' 'npx playwright install' | sh",
        "echo -e 'playwright\\x20install' | xargs npx",
        "printf 'npx\\x20playwright install\\n' | xargs -I{} sh -c '{}'",
        "sh <(printf 'npx\\x20playwright install')",
        "cat <(echo -e 'npx\\x20playwright install') | sh",
    ]);
    assert_clear(&[
        "echo -e 'a\\tb' | sh",
        "printf '%s\\n' hello | xargs echo",
        "echo -e 'line1\\nline2'",
        "printf 'x\\x20y\\n' | sh",
        "echo 'ls -la' | sh",
    ]);
}

#[test]
fn decoded_echo_text_is_not_interpreter_code() {
    // `code` is a parity field: an interpreter gets the text as the reference reads it
    let a = analyze("printf 'x\\x20y' | python3 -", HOME);
    let python = a.entries.iter().find(|e| e.prog == "python3").unwrap();
    assert_eq!(python.code, vec!["-".to_string(), "x\\x20y".to_string()]);
}

// ------------------------------------------------------------------ C-5. subscripts run substitutions

#[test]
fn subscripts_in_single_quotes_run_their_substitutions() {
    // too complex now (fail-closed families: a subscript holding `$(` in any word), blocked either way
    assert_blocked(&[
        "printf 'a[$(npx playwright install)]'",
        "[ -f 'a[$(npx playwright install)]' ]",
        "let 'a[$(npx playwright install)]=1'",
        "let 'a[`npx playwright install`]=1'",
        "[[ 'a[$(npx playwright install)]' -eq 0 ]]",
        "printf -v 'a[$(npx playwright install)]' x",
        "read 'a[$(npx playwright install)]' <<< x",
        "declare 'a[$(npx playwright install)]=1'",
        "declare -i x='a[$(npx playwright install)]'",
        "typeset 'a[$(npx playwright install)]=1'",
        "local 'a[$(npx playwright install)]=1'",
        "readonly 'a[$(npx playwright install)]=1'",
        "export 'a[$(npx playwright install)]=1'",
        "test -v 'a[$(npx playwright install)]'",
        "[ -v 'a[$(npx playwright install)]' ]",
    ]);
    assert_clear(&[
        "let 'a[1]=2'",
        "let i++",
        "declare -a arr=(1 2)",
        "[[ $x == a[b] ]]",
        "read -r line",
        "printf -v out '%s' x",
        "test -v HOME",
    ]);
}

// ------------------------------------------------------------------ C-3. brace expansion

#[test]
fn brace_expansion_gives_alternate_readings() {
    assert_runs(&[
        "npx playwright {install,}",
        "npx playwright {install,x}",
        "echo {a,b}; npx playwright install",
    ]);
    // re-review C-3a: a reading whose program word differs is too complex (`{echo,X} | sh` is X's input)
    assert_incomplete(&[
        "{npx,playwright,install}",
        "{npx,} playwright install",
        "npx {playwright,} {install,}",
        "sudo {npx,playwright,install}",
        "{sh,-c,'npx playwright install'}",
        "{{npx,},playwright} install",
        "{echo,'npx playwright install'} | sh",
        "{xargs,npx} <<< 'playwright install'",
        "echo x; {echo,'npx playwright install'} | bash",
        "sh -c \"{echo,'npx playwright install'} | sh\"",
        // fail-closed families: a brace sequence
        "echo {a..z}",
    ]);
    assert_clear(&[
        "cp file{,.bak}",
        "mkdir -p src/{a,b,c}/{d,e,f}",
        "awk '{print $1, $2}' f",
        "find . -exec rm {} \\;",
        "echo ${x,,}",
        "git log --format='%h {%s,%an}'",
    ]);
}

#[test]
fn brace_expansion_over_its_bound_is_too_complex() {
    let long = format!("echo {}", "{a,b}".repeat(5000));
    // 2^13 words, over MAX_BRACE_WORDS
    assert_incomplete(&[
        "echo {a,b}{c,d}{e,f}{g,h}{i,j}{k,l}{m,n}{a,b}{c,d}{e,f}{g,h}{i,j}{k,l}",
        &long,
    ]);
    // re-review F-1: 2^7 words in one command are fine
    let a = analyze("echo {a,b}{c,d}{e,f}{g,h}{i,j}{k,l}{m,n}", HOME);
    assert!(!a.incomplete, "{}", summary(&a));
}

// ------------------------------------------------------------------ re-review F-1. quoted braces

#[test]
fn only_unquoted_braces_are_expanded() {
    assert_clear(&[
        "awk '/a/{print $1,$2,$3,$4} /b/{print $1,$2,$3,$4} /c/{print $1,$2,$3,$4,$5}' log.txt",
        "node -e 'const {a, b} = require(\"./x\"); const {c, d, e} = require(\"./y\"); const {f, g} = \
         require(\"./z\"); const {h, i, j} = require(\"./w\"); const {k, l} = require(\"./v\"); console.log(a)'",
        "mkdir -p app/{models,views,controllers,helpers,mailers,jobs,channels}/{admin,api,web,shared,internal,public,legacy,v2,v3,v4}",
        "rm -f build/{a,b,c,d,e,f,g,h}.{js,map,d.ts,css,html,json,txt,md,yml}",
        "echo '{npx,playwright,install}'",
        "echo \"{npx,playwright,install}\"",
        "echo \\{npx,playwright,install}",
    ]);
    // a quoted brace expression is not one: no alternate reading
    for command in ["echo '{a,b}'", "echo \"{a,b}\"", "echo \\{a,b}"] {
        let a = analyze(command, HOME);
        assert!(a.alternates.is_empty(), "{command}: {}", summary(&a));
    }
    let a = analyze("echo {a,b}", HOME);
    assert_eq!(a.alternates.len(), 1, "{}", summary(&a));
}

// ------------------------------------------------------------------ I-3. fed lines over the cap

#[test]
fn stdin_lines_over_the_cap_are_read() {
    // SH-FW re-review F-2: every line is read (past MAX_FED on the Rust-only budget)
    let printf = format!(
        "printf '{}npx playwright install\\n' | xargs -I{{}} sh -c '{{}}'",
        "true\\n".repeat(64)
    );
    let heredoc = format!(
        "xargs -I{{}} sh -c '{{}}' <<'EOF'\n{}npx playwright install\nEOF",
        "true\n".repeat(64)
    );
    let plain = format!(
        "xargs npx <<'EOF'\n{}playwright install\nEOF",
        "--yes\n".repeat(70)
    );
    let parallel = format!(
        "parallel <<'EOF'\n{}npx playwright install\nEOF",
        "true\n".repeat(70)
    );
    assert_runs(&[&printf, &heredoc, &plain, &parallel]);
    let hosts: String = (0..70).map(|k| format!(" host{k}")).collect();
    let lines: String = (0..70).map(|k| format!("echo {k}\n")).collect();
    let tabs: String = (0..40).map(|k| format!("a\\tb{k}\\n")).collect();
    assert_clear(&[
        &format!("parallel ssh {{}} uptime :::{hosts}"),
        &format!("xargs -n1 echo <<'EOF'\n{lines}EOF"),
        &format!("python3 - <<'EOF' | xargs -n1 echo\n{lines}EOF"),
        &format!("echo -e '{tabs}' | xargs -n2 echo"),
    ]);
    // under the cap, the install is still seen
    let short = format!(
        "printf '{}npx playwright install\\n' | xargs -I{{}} sh -c '{{}}'",
        "true\\n".repeat(10)
    );
    assert_runs(&[&short]);
}

// ------------------------------------------------------------------ budget

#[test]
fn part_b_shapes_stay_quick_at_100_kb() {
    let fill = |unit: &str, tail: &str| format!("{}{tail}", unit.repeat(100_000 / unit.len()));
    let shapes = [
        fill("{a,b}", ""),
        fill("{a,b} ", ""),
        fill("{", "a,b}"),
        fill("{a,", "b"),
        format!("echo {}{}", "{a,".repeat(5000), "}".repeat(5000)),
        format!(
            "curl -d '{{\"k\":1,\"v\":[{}]}}' x",
            "{\"a\":1,\"b\":2},".repeat(5000)
        ),
        fill("let 'a[$(ls)]=1'; ", ""),
        fill("declare 'a[", "$(ls)]'"),
        format!("echo -e '{}' | sh", "\\x20\\0040".repeat(10_000)),
        format!("printf '%b' {} | xargs echo", "'\\0040a' ".repeat(10_000)),
    ];
    for shape in &shapes {
        let start = Instant::now();
        let a = analyze(shape, HOME);
        let took = start.elapsed();
        assert!(
            took < Duration::from_secs(8),
            "{:?}... took {took:?} (incomplete {})",
            &shape[..40.min(shape.len())],
            a.incomplete
        );
    }
    // a JSON body is quoted text, not brace expressions: it stays complete
    let json = format!(
        "curl -d '[{}]' https://api.example",
        "{\"id\":1,\"n\":\"a\"},".repeat(200)
    );
    assert!(!analyze(&json, HOME).incomplete);
}
