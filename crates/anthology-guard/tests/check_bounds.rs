//! GD1 acceptance beyond the Python table: fail-closed limits, the work budget, verdict
//! monotonicity, and the FW-I payloads where G1 is stricter than the Python on `rust`
//! (owner direction; difftest class D-8).

#[path = "support/gd.rs"]
mod gd;

use anthology_guard::{Rule, Verdict, check, check_with, check_without_d8};
use anthology_pycompat::json::Value;
use anthology_shell::Limits;
use proptest::prelude::*;

use gd::{HOME, bash, gh_token, obj};

fn verdict(command: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, HOME)
}

fn low_limit() -> Limits {
    Limits { backtrack_limit: 0 }
}

#[test]
fn a_backtrack_limit_never_allows() {
    // `$HOME` goes through `_HOME_VAR` (fancy-regex): at limit 0 the path cannot be read.
    let (tool, input) = bash("cat $HOME/notes.txt");
    assert_eq!(check(&tool, &input, HOME), Verdict::Allow);
    assert_eq!(
        check_with(&tool, &input, HOME, &low_limit()),
        Verdict::Incomplete
    );
    // a file tool's path, and G6's look-ahead patterns
    let read = obj(&[("file_path", Value::Str("~/notes.txt".into()))]);
    assert_eq!(
        check_with(&Value::Str("Read".into()), &read, HOME, &low_limit()),
        Verdict::Incomplete
    );
    let write = obj(&[
        ("file_path", Value::Str(".claude/memory/a.md".into())),
        ("content", Value::Str(["pass", "word: hunter2xyz"].concat())),
    ]);
    assert_eq!(
        check_with(&Value::Str("Write".into()), &write, HOME, &low_limit()),
        Verdict::Incomplete
    );
}

#[test]
fn a_rule_that_blocks_wins_over_a_limit() {
    let (tool, input) = bash("npx playwright install; cat $HOME/x");
    assert_eq!(
        check_with(&tool, &input, HOME, &low_limit()),
        Verdict::Block(Rule::G1)
    );
    // every path is read through `_TILDE_USER`/`_HOME_VAR`, so under the limit a profile path
    // is unknown: it blocks, as the limit block
    let (tool, input) = bash(&format!("cat {HOME}/.config/chromium/x"));
    assert_eq!(
        check_with(&tool, &input, HOME, &low_limit()),
        Verdict::Incomplete
    );
    // a memory path read under the limit is unknown, so a secret written there is not allowed
    let write = obj(&[
        ("file_path", Value::Str(".claude/memory/a.md".into())),
        ("content", Value::Str(gh_token())),
    ]);
    assert_ne!(
        check_with(&Value::Str("Write".into()), &write, HOME, &low_limit()),
        Verdict::Allow
    );
    // G5 needs no analysis
    assert_eq!(
        check_with(
            &Value::Str("mcp__playwright__x".into()),
            &obj(&[]),
            HOME,
            &low_limit()
        ),
        Verdict::Block(Rule::G5)
    );
}

#[test]
fn an_analysis_over_the_work_budget_is_too_complex() {
    // anthology-shell's own budget test input: quadratic option rescans stop the analysis.
    let command = format!("docker run img{}", " fish -cX".repeat(4000));
    assert_eq!(verdict(&command), Verdict::TooComplex);
    assert!(anthology_guard::TOO_COMPLEX.contains("too complex to check safely"));
    assert!(anthology_guard::TOO_COMPLEX.contains("/plugin"));
}

#[test]
fn deep_find_exec_nesting_fails_closed() {
    // Python raises RecursionError here and allows (D-1); Rust walks it and stops at its cap.
    let command = format!("{}ls ;", "find . -exec ".repeat(1500));
    assert_ne!(verdict(&command), Verdict::Allow);
}

/// Review finding: long ordinary inputs must not hit a search-wide backtrack limit (the G6
/// patterns go through `pycompat::re`, with a budget per start; `_ESCAPED_SPACE` is a scan).
#[test]
fn long_ordinary_inputs_are_allowed() {
    let prose = "The meeting notes say we keep the cache warm and ship on Friday. ".repeat(16_000);
    assert!(prose.len() > 1_000_000);
    let write = obj(&[
        ("file_path", Value::Str("~/.claude/memory/notes.md".into())),
        ("content", Value::Str(prose.clone())),
    ]);
    assert_eq!(
        check(&Value::Str("Write".into()), &write, HOME),
        Verdict::Allow
    );
    let code = format!("{}cd my\\ dir", "x = compute(a, b) + 1\n".repeat(25_000));
    let run = obj(&[("code", Value::Str(code))]);
    assert_eq!(
        check(&Value::Str("mcp__browseros-neo__run".into()), &run, HOME),
        Verdict::Allow
    );
    // a secret at the end of the long text is still found
    let write = obj(&[
        ("file_path", Value::Str("~/.claude/memory/notes.md".into())),
        ("content", Value::Str(format!("{prose}{}", gh_token()))),
    ]);
    assert_eq!(
        check(&Value::Str("Write".into()), &write, HOME),
        Verdict::Block(Rule::G6)
    );
}

/// FW-I bash-verified installs that the Python on `rust` allows (task-FW-I-report.md, rules 12
/// and 27). Each blocks here as G1.
#[test]
fn fw_i_package_manager_and_installer_options_block() {
    for command in [
        "npm --loglevel silent i puppeteer",
        "npm --loglevel silent init playwright",
        "npm --foo-bar silent i puppeteer",
        "npm --foo-bar silent init playwright",
        "sudo apt-get -o Dpkg::Options::=--force-confnew install chromium",
        "apt-get --option x=y install chromium",
        "dnf --setopt x=y install chromium",
        "yum -c /etc/yum.conf install chromium",
        "snap install --channel beta chromium",
        // the Python reading is kept too (union): Python blocks these, so Rust must
        "npm i --tag puppeteer",
        "npm install --save-dev puppeteer@21.0.0",
        "apt install google-chrome-stable",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
    for command in [
        "npm --loglevel silent install",
        "npm --loglevel silent i playwright",
        "apt-get -o Dpkg::Options::=--force-confnew install jq",
        "npm i -D playwright @playwright/test",
        "dnf --setopt x=y install jq",
    ] {
        assert_eq!(verdict(command), Verdict::Allow, "{command}");
    }
}

/// Commands each rule blocks, for the monotonicity property below.
const BLOCKED: &[&str] = &[
    "npx playwright install",
    "pip install playwright",
    "brew install chromium",
    "google-chrome --headless",
    "open -a Chromium",
    "cat ~/.config/chromium/Default/Cookies",
    "ls ~/Library/Application\\ Support/Google",
    "agent-browser open x",
    "python3 -c \"open('~/.config/google-chrome/x')\"",
];

/// Wrappers and neighbours that do not exempt anything (docker would exempt G1 and G2).
const WRAPS: &[&str] = &[
    "{c}",
    "sudo {c}",
    "timeout 5 {c}",
    "env A=1 {c}",
    "nohup {c}",
    "{c}; ls",
    "ls && {c}",
    "true || {c}",
    "echo $({c})",
    "( {c} )",
    "{ {c}; }",
    "bash -c '{q}'",
    "sh -euo pipefail -c '{q}'",
    "echo '{q}' | sh",
    "cd /tmp && {c}",
    "atl run {c}",
    "rtk run {c}", // legacy
    "x=$({c})",
    "{c} 2>&1 | tee log",
];

proptest! {
    /// Wrapping a blocked command, or chaining it with others, never turns it into an allow.
    #[test]
    fn verdict_monotonicity(base in 0..BLOCKED.len(), outer in 0..WRAPS.len(), inner in 0..WRAPS.len()) {
        let c = BLOCKED[base];
        let q = c.replace('\'', "\"");
        let once = WRAPS[inner].replace("{q}", &q).replace("{c}", c);
        let q2 = once.replace('\'', "\"");
        let twice = WRAPS[outer].replace("{q}", &q2).replace("{c}", &once);
        prop_assert_ne!(verdict(c), Verdict::Allow);
        prop_assert_ne!(verdict(&once), Verdict::Allow, "{}", once);
        prop_assert_ne!(verdict(&twice), Verdict::Allow, "{}", twice);
    }

    /// `check` never panics, whatever the command.
    #[test]
    fn check_never_panics(command in ".{0,200}") {
        let _ = verdict(&command);
    }
}

#[test]
fn an_alternate_reading_is_checked_against_g1() {
    // the brace reading `npx playwright install` is an alternate (SH lead ruling Q3=A): G1 blocks it
    assert_eq!(
        verdict("npx playwright {install,}"),
        Verdict::Block(Rule::G1)
    );
    assert_eq!(verdict("cp a.{js,ts} dir"), Verdict::Allow);
    // SH-seq review M-2: only the sequence alternate shows the install (bash runs `npm -1 i puppeteer`)
    for command in [
        "npm {-1..-1} i puppeteer",
        "npm --prefix ${HOME:+x} {-1..-1} i puppeteer",
    ] {
        assert_eq!(verdict(command), Verdict::Block(Rule::G1), "{command}");
    }
    // C-1, I-1: a reading that may not be bash's is too complex, never allowed
    for command in [
        "npm --prefix \"{1..2}\" {-1..-1} i puppeteer",
        "npm --prefix '{1..2}' i {puppeteer,x}",
        "npm --prefix \"{a,b}\" {-x,} i puppeteer",
        "npm {-1..{1,}} i puppeteer",
        "echo \"npm --prefix '{1..2}'\" {-1..-1} i puppeteer | sh",
        // re-review R-1: bash leaves braces inside `${...}` alone
        "npm --prefix ${x:-{a,b}} {-x,} i puppeteer",
        "npm --prefix ${x:+{a,b}} {-x,} i puppeteer",
        "npm --prefix ${x:-\\{a,b\\}} {-x,} i puppeteer",
        "npm --prefix ${x:-{1..2}} {-1..-1} i puppeteer",
        "npm --prefix \"${x:-{a,b}}\" {-x,} i puppeteer",
        // #89 review F-4: quotes and escapes inside `${...}` hide braces
        "npm --prefix ${x:-\"}\"{a,b}} {-x,} i puppeteer",
        "npm --prefix ${x:-\\}{a,b}} {-x,} i puppeteer",
        "npm --prefix ${x:-'}'{a,b}} {-x,} i puppeteer",
    ] {
        assert_ne!(verdict(command), Verdict::Allow, "{command}");
    }
}

// ------------------------------------------- LR-G: past Python's caps the guard blocks (D-8)

fn tool(name: &str, input: Value) -> (Value, Value) {
    (Value::Str(name.into()), input)
}

/// `check` and `check_without_d8` (Python's reading) on one input.
fn both(tool: &(Value, Value), home: &str) -> (Verdict, Verdict) {
    (
        check(&tool.0, &tool.1, home),
        check_without_d8(&tool.0, &tool.1, home),
    )
}

fn verdict_at(command: &str, home: &str) -> Verdict {
    let (tool, input) = bash(command);
    check(&tool, &input, home)
}

/// `command` fed to `levels` nested shells, each reading the next from a here-document (no
/// quoting, so the text grows linearly).
fn nested_bash(command: &str, levels: usize) -> String {
    let mut c = command.to_string();
    for k in 0..levels {
        c = format!("bash <<'E{k}'\n{c}\nE{k}");
    }
    c
}

#[test]
fn a_command_nested_past_the_depth_cap_is_too_complex() {
    let deep = nested_bash("npx playwright install chromium", 13);
    assert_eq!(
        both(&bash(&deep), HOME),
        (Verdict::TooComplex, Verdict::Allow)
    );
    // at the cap the rule still sees it
    let at = nested_bash("npx playwright install chromium", 12);
    assert_eq!(verdict(&at), Verdict::Block(Rule::G1));
    // the lexer's own cap: `$(...)` nested past it
    let subs = format!("{}ls{}", "echo $(".repeat(40), ")".repeat(40));
    assert_eq!(verdict(&subs), Verdict::TooComplex);
    // a harmless command nested past the cap is not read, so it is not allowed either
    assert_eq!(verdict(&nested_bash("ls", 13)), Verdict::TooComplex);
    assert_eq!(verdict(&nested_bash("ls", 12)), Verdict::Allow);
}

#[test]
fn a_cd_that_cd_tracking_drops_is_too_complex() {
    let deep = format!("/{}.config", "a/".repeat(64));
    let command = format!("cd {deep} && cat chromium/Default/Cookies");
    assert_eq!(
        both(&bash(&command), HOME),
        (Verdict::TooComplex, Verdict::Allow)
    );
    for cd in ["pushd", "cd --", "cd -P"] {
        let command = format!("{cd} {deep} && cat chromium/Default/Cookies");
        assert_eq!(verdict(&command), Verdict::TooComplex, "{cd}");
    }
    // a deep directory `cd` tracking follows is read as before
    let shallow = format!("/{}.config", "a/".repeat(60));
    let command = format!("cd {shallow} && cat chromium/Default/Cookies");
    assert_eq!(verdict(&command), Verdict::Block(Rule::G3));
    assert_eq!(verdict("cd - && ls"), Verdict::Allow);
    assert_eq!(verdict("cd && ls"), Verdict::Allow);
}

#[test]
fn a_profile_path_under_deep_tool_input_nesting_blocks() {
    let mut deep = Value::Str("~/.config/chromium/Default/Cookies".into());
    for _ in 0..13 {
        deep = obj(&[("path", deep)]);
    }
    let t = tool("mcp__x__y", deep);
    assert_eq!(both(&t, HOME), (Verdict::Block(Rule::G3), Verdict::Allow));
    let mut code = Value::Str("open('~/.config/chromium/x')".into());
    for _ in 0..13 {
        code = Value::Array(vec![code]);
    }
    let t = tool("mcp__browseros-neo__run", obj(&[("code", code)]));
    assert_eq!(both(&t, HOME), (Verdict::Block(Rule::G3), Verdict::Allow));
}

#[test]
fn a_long_profile_path_blocks() {
    let padded = format!("~/{}.config/chromium/Default/Cookies", "a/../".repeat(900));
    assert!(padded.chars().count() > 4096);
    let read = tool("Read", obj(&[("file_path", Value::Str(padded.clone()))]));
    assert_eq!(
        both(&read, HOME),
        (Verdict::Block(Rule::G3), Verdict::Allow)
    );
    assert_eq!(
        both(&bash(&format!("cat {padded}")), HOME),
        (Verdict::Block(Rule::G3), Verdict::Allow)
    );
    // long words without `~`, `$` or a glob take the one-scan path; it must still find these
    let pad = "a/".repeat(3000);
    for (command, home) in [
        (format!("cat /{pad}.config/chromium/x"), HOME),
        (
            format!("cat /{}.config\\\\chromium", "a\\\\".repeat(3000)),
            HOME,
        ),
        (format!("cat /{pad}.config/google-chrome-beta"), HOME),
        (
            format!("cat /{pad}.claude/memory/../../.config/chromium"),
            HOME,
        ),
        (format!("cd /opt/.config && cat chromium/{pad}x"), HOME),
        (
            format!("cd /opt/.config/chromium && cat {}x", "b/../".repeat(1000)),
            HOME,
        ),
        (format!("cat /{pad}.config/chromium/x"), "relative"),
        (
            format!("cat '/{pad}Library/Application Support/Google/x'"),
            HOME,
        ),
    ] {
        assert_eq!(
            verdict_at(&command, home),
            Verdict::Block(Rule::G3),
            "{command:.60}"
        );
    }
    // a long secret write into a long memory path (G6 reads memory paths the same way)
    let write = obj(&[
        (
            "file_path",
            Value::Str(format!("/{pad}repo/.claude/memory/a.md")),
        ),
        ("content", Value::Str(gh_token())),
    ]);
    assert_eq!(
        check(&Value::Str("Write".into()), &write, HOME),
        Verdict::Block(Rule::G6)
    );
    // the protected text inside a component is not a component
    assert_eq!(
        verdict(&format!("cat /{pad}x.config%2fchromium/y")),
        Verdict::Allow
    );
}

#[test]
fn braces_python_drops_are_expanded() {
    // the 33rd alternative, past Python's 32
    let alts: Vec<String> = (0..32).map(|i| format!("a{i}")).collect();
    let wide = format!("cat ~/{{{},.config/chromium}}/x", alts.join(","));
    assert_eq!(
        both(&bash(&wide), HOME),
        (Verdict::Block(Rule::G3), Verdict::Allow)
    );
    // a fifth group, past Python's four rounds
    let five = "cat ~/{a,b}/{a,b}/{a,b}/{a,b}/../../../../{.config,x}/chromium/x";
    assert_eq!(
        both(&bash(five), HOME),
        (Verdict::Block(Rule::G3), Verdict::Allow)
    );
    // what Python blocks still blocks (the union)
    assert_eq!(
        verdict("cat ~/{.config,x}/chromium/x"),
        Verdict::Block(Rule::G3)
    );
}

#[test]
fn a_brace_expansion_too_large_to_check_is_too_complex() {
    // Quoted, so anthology-shell's own bash brace reading (unquoted `{` only) leaves the word
    // alone and the guard's path reading is what is tested.
    let command = format!("cat \"/home/tester/{}\"", "{a,b}".repeat(13));
    assert_eq!(
        both(&bash(&command), HOME),
        (Verdict::TooComplex, Verdict::Allow)
    );
    let read = tool(
        "Read",
        obj(&[("file_path", Value::Str(format!("~/{}", "{a,b}".repeat(13))))]),
    );
    assert_eq!(both(&read, HOME), (Verdict::TooComplex, Verdict::Allow));
    // a rule that blocks on what was read still wins
    let command = format!(
        "npx playwright install; cat \"/home/tester/{}\"",
        "{a,b}".repeat(13)
    );
    assert_eq!(verdict(&command), Verdict::Block(Rule::G1));
    assert_eq!(
        verdict(&format!("cat \"/home/tester/{}\"", "{a,b}".repeat(12))),
        Verdict::Allow
    );
    // a piece with no separator is still expanded after a `cd` that could complete a sequence
    let command = format!(
        "cd '/opt/Library/Application Support' && cat 'google{}'",
        "{,}".repeat(13)
    );
    assert_eq!(
        both(&bash(&command), HOME),
        (Verdict::TooComplex, Verdict::Allow)
    );
    let command = format!(
        "cd '/opt/Library/Application Support' && cat 'googl{{e,x}}{}'",
        "{,}".repeat(3)
    );
    assert_eq!(verdict(&command), Verdict::Block(Rule::G3));
}

#[test]
fn g3_reads_a_relative_or_empty_home_as_an_absolute_one() {
    let read = |path: &str| tool("Read", obj(&[("file_path", Value::Str(path.into()))]));
    let profile = read("~/.config/chromium/x");
    for home in ["relative/home", "~", "./h"] {
        assert_eq!(
            both(&profile, home),
            (Verdict::Block(Rule::G3), Verdict::Allow),
            "{home}"
        );
    }
    // `~/*/*` names the profile only right under the home
    let glob = read("~/*/*");
    assert_eq!(both(&glob, ""), (Verdict::Block(Rule::G3), Verdict::Allow));
    assert_eq!(check(&glob.0, &glob.1, HOME), Verdict::Block(Rule::G3));
    // what Python blocks under such a home still blocks as G3 (an empty home makes `~/` `/`)
    assert_eq!(check(&profile.0, &profile.1, ""), Verdict::Block(Rule::G3));
    assert_eq!(
        check(&profile.0, &profile.1, "$HOME"),
        Verdict::Block(Rule::G3)
    );
    // a `cd` into the home, then a relative path
    let cd = bash("cd ~/.config && cat chromium/x");
    assert_eq!(
        both(&cd, "relative"),
        (Verdict::Block(Rule::G3), Verdict::Allow)
    );
    // climbing out of the home leads somewhere unknown
    let out = read("~/../../x");
    assert_eq!(
        both(&out, "relative"),
        (Verdict::Incomplete, Verdict::Allow)
    );
    // ordinary paths under such a home are read as before
    for home in ["relative", "", "~"] {
        for path in ["/tmp/x", "~/notes.txt", "~", "src/main.rs"] {
            let t = read(path);
            assert_eq!(check(&t.0, &t.1, home), Verdict::Allow, "{path} {home}");
        }
        for command in [
            "cat notes.txt",
            "cd ~ && ls .config",
            "cd ~; cd x; cat y",
            "git -C a -C b status",
            "cd ~/projects && ls",
            "cd .. && git status",
            "cd ../x && cargo test",
        ] {
            assert_eq!(
                verdict_at(command, home),
                Verdict::Allow,
                "{command} {home}"
            );
        }
        assert_eq!(
            verdict_at("cat /opt/.config/chromium/x", home),
            Verdict::Block(Rule::G3)
        );
    }
}

#[test]
fn g6_reads_a_relative_or_tilde_home_as_an_absolute_one() {
    let secret = gh_token();
    let write = |path: &str| {
        tool(
            "Write",
            obj(&[
                ("file_path", Value::Str(path.into())),
                ("content", Value::Str(secret.clone())),
            ]),
        )
    };
    for home in ["relative", "~", "./h"] {
        for command in [
            format!("echo {secret} > ~/.claude/agent-memory/a.md"),
            format!("cd ~/.claude && echo {secret} > memory/a.md"),
            format!("echo {secret} > $HOME/x/.claude/memory/a.md"),
        ] {
            assert_eq!(
                both(&bash(&command), home),
                (Verdict::Block(Rule::G6), Verdict::Allow),
                "{command} {home}"
            );
        }
        let t = write("~/.claude/agent-memory/a.md");
        assert_eq!(
            both(&t, home),
            (Verdict::Block(Rule::G6), Verdict::Allow),
            "{home}"
        );
        // no secret, or not a memory path: allowed as before
        let note = tool(
            "Write",
            obj(&[
                ("file_path", Value::Str("~/.claude/memory/a.md".into())),
                ("content", Value::Str("a note".into())),
            ]),
        );
        assert_eq!(check(&note.0, &note.1, home), Verdict::Allow, "{home}");
        let t = write("~/notes/a.md");
        assert_eq!(check(&t.0, &t.1, home), Verdict::Allow, "{home}");
        assert_eq!(
            verdict_at(&format!("cd .. && echo {secret} > notes.md"), home),
            Verdict::Allow
        );
    }
}

/// `n` JSON objects of `keys` keys each, joined by commas.
fn objects(n: usize, keys: usize) -> String {
    (0..n)
        .map(|i| {
            let fields: Vec<String> = (0..keys).map(|k| format!("\"k{k}\":{i}")).collect();
            format!("{{{}}}", fields.join(","))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The new readings must not touch ordinary payloads: long commit and PR texts, long file
/// contents, scripts with a few dicts, JSON request bodies, ordinary file tools.
#[test]
fn ordinary_payloads_past_the_old_caps_are_allowed() {
    let body =
        "This change keeps the cache warm; see docs/config/notes.md for the details.\n".repeat(200);
    assert!(body.len() > 10_000);
    let commands: Vec<String> = vec![
        format!("git commit -m \"{body}\""),
        format!("gh pr create --title x --body \"$(cat <<'EOF'\n{body}\nEOF\n)\""),
        format!("cat > notes.md <<'EOF'\n{body}\nEOF"),
        "python3 -c \"d = {'a': 1, 'b': 2, 'c': 3}; e = {'x': 1, 'y': 2}; print(d, e)\"".into(),
        "curl -s -X POST -d '{\"name\": \"a\", \"tags\": [\"x\", \"y\"], \"n\": 3}' https://example.com/api".into(),
        "jq '{name: .name, id: .id, tags: .tags}' data.json".into(),
        // review I-1: JSON and dict literals that multiply past the brace cap
        format!("curl -s -X POST -d '[{}]' https://example.com/api", objects(7, 4)),
        format!("curl -d '[{}]' https://x", objects(8, 3)),
        format!("gh api repos/o/r/issues -f body='[{}]'", objects(7, 4)),
        format!("git commit -m 'fix: accept [{}]'", objects(7, 4)),
        format!(
            "python3 -c \"rows=[{}]; print(rows)\"",
            ["{'a': 1, 'b': 2}"; 13].join(", ")
        ),
        format!("jq -n '[{}]'", ["{a:1,b:2,c:3,d:4}"; 7].join(",")),
        format!("echo payload=[{}]", objects(7, 4)),
        format!("cd /tmp/proj && curl -d '[{}]' https://x", objects(7, 4)),
        "mkdir -p src/{app,lib,test}/{a,b,c}".into(),
        "cd /tmp && ls -la".into(),
        "cd ~/projects/app && cargo test".into(),
    ];
    for command in &commands {
        assert_eq!(verdict(command), Verdict::Allow, "{command:.80}");
    }
    let mut nested = Value::Str("x".into());
    for _ in 0..40 {
        nested = obj(&[("child", nested)]);
    }
    let inputs = [
        (
            "Read",
            obj(&[(
                "file_path",
                Value::Str("/home/tester/project/src/main.rs".into()),
            )]),
        ),
        (
            "Write",
            obj(&[
                (
                    "file_path",
                    Value::Str("/home/tester/project/README.md".into()),
                ),
                ("content", Value::Str(body.clone())),
            ]),
        ),
        (
            "Edit",
            obj(&[
                ("file_path", Value::Str("src/lib.rs".into())),
                ("old_string", Value::Str(body.clone())),
                ("new_string", Value::Str(body.clone())),
            ]),
        ),
        (
            "Glob",
            obj(&[("pattern", Value::Str("src/**/*.{rs,toml}".into()))]),
        ),
        ("mcp__x__y", obj(&[("tree", nested)])),
    ];
    for (name, input) in inputs {
        let t = tool(name, input);
        assert_eq!(check(&t.0, &t.1, HOME), Verdict::Allow, "{name}");
    }
    // a word of any length is read, in linear time, without a backtrack limit
    let long_word = format!("echo {}", "word/".repeat(400_000));
    let start = std::time::Instant::now();
    assert_eq!(verdict(&long_word), Verdict::Allow);
    assert!(start.elapsed() < gd::bound(2000), "{:?}", start.elapsed());
    let with_path = format!("{long_word}/../{}.config/chromium", "../".repeat(400_000));
    assert_eq!(
        both(&bash(&with_path), HOME),
        (Verdict::Block(Rule::G3), Verdict::Allow)
    );
}
