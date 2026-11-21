//! The `guard.check` difftest surface: `anthology_guard::check` against guard_rules.py `check` through
//! `port/difftest/oracle/guard.py` (plan GD1).
//!
//! Case: `{"op": "check", "tool": T, "input": I, "home": H}`. Both sides print the canonical JSON of
//! `[rule, message]`, or `null` for an allow. Rust's own blocks print `["internal", null]` (a backtrack
//! limit, D-7) and `["G0", <too complex>]` (the work budget, D-7); Python has neither.
//!
//! Seeds: every `case` row of `crates/anthology-guard/tests/data/test_guard_rules.jsonl` (`rows:*`), the
//! shell seeds and hand-written commands of `shell.analyze` as Bash commands (`shell:*`), hand-written tool
//! inputs (`tool:*`), and the FW-I payloads where Rust's G1 is stricter than Python (`fw:*`, D-8, owner
//! direction). Generated (`gen:*`): `shell.analyze`'s command generator, secret-into-memory commands, and
//! tool inputs built from path, code and secret vocabularies, all under a mix of homes and tool names.

use anthology_guard::{TOO_COMPLEX, TOO_COMPLEX_ID, Verdict, check};
use anthology_pycompat::json::{self, Map, Value};

use super::shell_analyze;
use super::shell_segments::{GenRng, field, field_str, load_seeds, mutate, obj, observed, s};
use crate::case::{Case, Rng, Surface, repo_root};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "guard.check";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(GuardCheck))
}

/// The rule table rows (GD1's port of `test_guard_rules`), relative to the repository root.
const ROWS: &str = "crates/anthology-guard/tests/data/test_guard_rules.jsonl";

/// The Python tests' home.
const HOME: &str = "/home/tester";

struct GuardCheck;

impl Surface for GuardCheck {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out = row_cases();
        let shell: Vec<Value> = load_seeds(&["analyze"])
            .into_iter()
            .chain(shell_analyze::fixed_cases())
            .collect();
        for (k, data) in shell.iter().enumerate() {
            out.push(Case::new(
                format!("shell:{}", k + 1),
                bash_case(field_str(data, "command"), field_str(data, "home")),
            ));
        }
        for (k, data) in tool_cases().into_iter().enumerate() {
            out.push(Case::new(format!("tool:{}", k + 1), data));
        }
        for (k, command) in FW_STRICTER.iter().enumerate() {
            out.push(Case::new(format!("fw:{}", k + 1), bash_case(command, HOME)));
        }
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        generate(rng.next_u64(), n)
            .into_iter()
            .enumerate()
            .map(|(k, data)| Case::new(format!("gen:{}", k + 1), data))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        from_oracle(&oracle.call("guard", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        observed(run_rust(&c.data))
    }
}

/// Run one case's data in Rust: the result to print, or `None` for an op this surface does not own.
pub(crate) fn run_rust(data: &Value) -> Option<Value> {
    if field_str(data, "op") != "check" {
        return None;
    }
    let null = Value::Null;
    let tool = field(data, "tool").unwrap_or(&null);
    let input = field(data, "input").unwrap_or(&null);
    Some(render(check(tool, input, field_str(data, "home"))))
}

/// The oracle's rendering of a verdict.
pub(crate) fn render(verdict: Verdict) -> Value {
    match verdict {
        Verdict::Allow => Value::Null,
        Verdict::Block(rule) => Value::Array(vec![s(rule.id()), s(rule.message())]),
        Verdict::Incomplete => Value::Array(vec![s("internal"), Value::Null]),
        Verdict::TooComplex => Value::Array(vec![s(TOO_COMPLEX_ID), s(TOO_COMPLEX)]),
    }
}

fn check_case(tool: Value, input: Value, home: &str) -> Value {
    obj(vec![
        ("op", s("check")),
        ("tool", tool),
        ("input", input),
        ("home", s(home)),
    ])
}

fn bash_case(command: &str, home: &str) -> Value {
    check_case(s("Bash"), obj(vec![("command", s(command))]), home)
}

/// Every row of the rule table that carries a `case` (`[tool, input]`).
fn row_cases() -> Vec<Case> {
    let path = repo_root().join(ROWS);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("difftest: cannot read {}: {e}", path.display()));
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let row = json::loads(line)
            .unwrap_or_else(|e| panic!("difftest: {}: not JSON: {e}", path.display()));
        let case = field(&row, "params").and_then(|p| field(p, "case"));
        let Some(Value::Array(pair)) = case else {
            continue;
        };
        if pair.len() != 2 {
            continue;
        }
        let py = field_str(&row, "py")
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_string();
        let n = field(&row, "row").and_then(Value::as_i64).unwrap_or(0);
        out.push(Case::new(
            format!("rows:{py}:{n}"),
            check_case(pair[0].clone(), pair[1].clone(), HOME),
        ));
    }
    out
}

/// FW-I payloads where Rust's G1 reads a package manager's or an OS installer's options as FW-I
/// does (bash-verified installs Python allows). Declared case by case as D-8 in
/// `port/difftest/divergences/guard.check.toml`.
pub(crate) const FW_STRICTER: &[&str] = &[
    "npm --loglevel silent i puppeteer",
    "npm --loglevel silent init playwright",
    "npm --foo-bar silent i puppeteer",
    "npm --foo-bar silent init playwright",
    "sudo apt-get -o Dpkg::Options::=--force-confnew install chromium",
    "apt-get --option x=y install chromium",
    "dnf --setopt x=y install chromium",
];

// ------------------------------------------------------------------ vocabularies

/// Another user's macOS home, assembled so no literal user path lands in the repository.
fn other_home() -> String {
    format!("/{}ers/someone", "Us")
}

const HOMES: &[&str] = &[
    "/home/tester",
    "/home/tester",
    "/home/tester",
    "/home/tester/",
    "",
    "/",
    "//",
    "relative",
    "~",
    "$HOME",
    "/root",
    "/home/\u{3a3}\u{391}",
    "/home/K\u{212a}",
    "/h\\o",
    "/home/a b",
];

/// Secret shapes, assembled at run time so the repository never holds one.
fn secrets() -> Vec<String> {
    vec![
        ["gh", "p_", "abc123"].concat(),
        ["AK", "IA", "IOSFODNN7EXAMPLE"].concat(),
        ["github", "_pat_", "abcdef"].concat(),
        ["sk", "-", "a1b2c3d4e5f6g7h8i9"].concat(),
        ["xo", "xb-", "123"].concat(),
        ["-----BEGIN OPENSSH ", "PRIVATE KEY-----"].concat(),
        ["pass", "word=", "hunter2xyz"].concat(),
        ["PASS", "WORD: ", "\"hunter2xyz\""].concat(),
        ["\"pass", "word\": \"", "<your-password>\""].concat(),
        ["api", "_key = ", "null"].concat(),
        ["API", "-KEY=", "${API_KEY}"].concat(),
        ["\\\"api", "Key\\\": \\\"", "s3cr3t\\\""].concat(),
        "ghp prefixes mark GitHub tokens".to_string(),
        "AKIA prefix marks AWS keys".to_string(),
        "a note".to_string(),
    ]
}

const PATHS: &[&str] = &[
    "~/Library/Application Support/Google/Chrome/Default/Cookies",
    "~/library/application support/google",
    "~/Library/Application\\ Support/Chromium",
    "~/Library/Application\\\\ Support/Google",
    "Library/Application Support/Google/Chrome",
    "~/.config/chromium/Default/Cookies",
    "~/.config/google-chrome-beta/x",
    "~/.config/google-chrome-canary",
    "~/.config/chromium-browser",
    "~/.config/chrom*",
    "~/.config/[c]hromium/x",
    "~/.config/[z-a]*/x",
    "~/*/*/*",
    "**/*",
    "*/*",
    "/home/tester/.config/chromium",
    "/home/user/.config/chromium",
    "/root/.config/chromium",
    "/opt/.config/chromium",
    "$HOME/.config/chromium",
    "${HOME}/.config/google-chrome",
    "$HOMEx/.config/chromium",
    "~bob/.config/chromium",
    "~bob",
    "../.config/chromium",
    "$X/../.config/chromium",
    "./.config/chromium",
    ".config/chromium",
    "sub/.config/chromium",
    "a/{b,.config/chromium}/c",
    "{x,y}/{a,b}/{c,d}",
    "file:///home/tester/.config%2Fchromium",
    "file:%2Fhome%2Ftester%2F.config%2Fchromium",
    "FILE:%2E%2Fx",
    "https://x/?q=.config%2Fchromium",
    "x=~/.config/chromium",
    "a:~/.config/chromium:b",
    "~\\.config\\chromium",
    ".claude/memory/a.md",
    ".claude/agent-memory/x/MEMORY.md",
    "~/.claude/memory/x",
    "/repo/.claude/memory",
    ".claude/memories/a.md",
    "src/chrome-extension/manifest.json",
    "docs/chromium.md",
    "\u{130}.config/chromium",
    "~/.CONFIG/CHROMIUM",
    "",
];

const CODE: &[&str] = &[
    "open('~/.config/chromium/Default/Cookies')",
    "fs.readFileSync(`${process.env.HOME}/Library/Application Support/Google/Chrome/x`)",
    "os.path.expanduser('~/Library/Application\\\\ Support/Google')",
    "# Chrome's bookmarks\npath = '~/.config/google-chrome/x'",
    "() => document.title",
    "location.href",
    "x = '[z-a]/b/c'",
    "f\"{home}/.config/chromium\"",
    "Path.home() / 'Library' / 'Application Support'",
    "fetch('file:%2Fhome%2Ftester%2F.config%2Fchromium')",
    "print('a\\\\ b')",
];

const TOOLS: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Glob",
    "Grep",
    "LS",
    "WebFetch",
    "Monitor",
    "Task",
    "TodoWrite",
    "mcp__browseros-neo__run",
    "mcp__browseros-neo__evaluate_in_page",
    "mcp__browseros-neo__save_skill",
    "mcp__browseros-neo__navigate",
    "mcp__playwright__browser_navigate",
    "mcp__Chrome_DevTools__x",
    "mcp__claude_in_chrome__y",
    "mcp__puppeteer",
    "mcp__terminal__run",
    "MCP__playwright",
];

const KEYS: &[&str] = &[
    "file_path",
    "path",
    "notebook_path",
    "pattern",
    "glob",
    "url",
    "uri",
    "dir",
    "cwd",
    "source",
    "dest",
    "target",
    "root",
    "location",
    "FilePath",
    "query",
    "prompt",
    "description",
    "content",
    "new_string",
    "new_source",
    "old_string",
    "edits",
    "command",
    "script",
    "code",
    "expression",
    "func",
    "fn",
    "Code",
    "flow",
    "page",
];

/// Commands that write into memory, with `{s}` for a secret and `{m}` for a memory path.
const G6_SHAPES: &[&str] = &[
    "echo {s} > {m}",
    "echo {s} >> {m}",
    "printf '%s' '{s}' &> {m}",
    "cat >> {m} <<'EOF'\n{s}\nEOF",
    "tee -a {m} <<< '{s}'",
    "echo '{s}' | tee {m}",
    "cp notes.txt {m} && echo {s}",
    "sed -i 's/x/{s}/' {m}",
    "perl -pi -e 's/x/{s}/' {m}",
    "sed -n 's/x/{s}/p' {m}",
    "cd .claude && echo {s} > memory/a.md",
    "cd ~ && echo {s} > .claude/memory/a.md",
    "echo {s} > notes.md",
    "install -m 600 f {m} # {s}",
    "dd of={m} <<< {s}",
];

const MEMORY_PATHS: &[&str] = &[
    ".claude/memory/a.md",
    ".claude/agent-memory/x/MEMORY.md",
    "~/.claude/memory/x.md",
    "$HOME/.claude/memory/y",
    "./.claude/memory/z",
    "../.claude/memory/w",
    ".claude/memories/a.md",
    "/tmp/a.md",
];

// ------------------------------------------------------------------ seeds and generator

/// Hand-written tool inputs: every path, code and secret entry under the keys that read it.
fn tool_cases() -> Vec<Value> {
    let mut out = Vec::new();
    for path in PATHS {
        for (tool, key) in [
            ("Read", "file_path"),
            ("Glob", "pattern"),
            ("Grep", "pattern"),
            ("mcp__x__y", "url"),
            ("Task", "prompt"),
        ] {
            out.push(check_case(s(tool), obj(vec![(key, s(path))]), HOME));
        }
        out.push(check_case(
            s("Glob"),
            obj(vec![("pattern", s("Default/**")), ("path", s(path))]),
            HOME,
        ));
        out.push(check_case(
            s("Grep"),
            obj(vec![
                ("pattern", s("x")),
                ("glob", s("*/Cookies")),
                ("path", s(path)),
            ]),
            HOME,
        ));
    }
    for code in CODE {
        for key in ["code", "func", "script", "expression", "fn", "content"] {
            out.push(check_case(
                s("mcp__browseros-neo__run"),
                obj(vec![(key, s(code))]),
                HOME,
            ));
        }
    }
    for secret in secrets() {
        for path in MEMORY_PATHS {
            out.push(check_case(
                s("Write"),
                obj(vec![("file_path", s(path)), ("content", s(&secret))]),
                HOME,
            ));
            out.push(check_case(
                s("MultiEdit"),
                obj(vec![
                    ("file_path", s(path)),
                    (
                        "edits",
                        Value::Array(vec![obj(vec![("new_string", s(&secret))])]),
                    ),
                ]),
                HOME,
            ));
        }
    }
    for tool in TOOLS {
        out.push(check_case(s(tool), obj(vec![]), HOME));
    }
    out
}

/// A random JSON value for a tool input, `depth` levels at most.
fn value(rng: &mut GenRng, depth: usize, secrets: &[String]) -> Value {
    match rng.below(if depth == 0 { 6 } else { 9 }) {
        0..=2 => s(rng.pick(PATHS)),
        3 => s(rng.pick(CODE)),
        4 => s(rng.pick(secrets)),
        5 => match rng.below(4) {
            0 => Value::Null,
            1 => Value::Bool(true),
            2 => Value::Int(json::Int::Small(rng.below(10) as i64)),
            _ => {
                let path = rng_pick_owned(rng, PATHS);
                s(&mutate(rng, path))
            }
        },
        6 | 7 => {
            let mut map = Map::new();
            for _ in 0..1 + rng.below(3) {
                let key = rng.pick(KEYS).to_string();
                let v = value(rng, depth - 1, secrets);
                map.insert(key, v);
            }
            Value::Object(map)
        }
        _ => Value::Array(
            (0..rng.below(3))
                .map(|_| value(rng, depth - 1, secrets))
                .collect(),
        ),
    }
}

fn rng_pick_owned(rng: &mut GenRng, items: &[&'static str]) -> &'static str {
    items[rng.below(items.len())]
}

fn home(rng: &mut GenRng) -> String {
    if rng.chance(5) {
        other_home()
    } else {
        rng.pick(HOMES).to_string()
    }
}

/// `n` generated cases for seed `seed`.
pub(crate) fn generate(seed: u64, n: usize) -> Vec<Value> {
    let mut rng = GenRng::new(seed);
    let secrets = secrets();
    let shell_seeds = load_seeds(&["analyze"]);
    // The shell generator's commands, made in one batch (it is deterministic for its seed).
    let mut commands = shell_analyze::generate(rng.next_u64(), n, &shell_seeds).into_iter();
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let roll = rng.below(100);
        let case = if roll < 50 {
            let data = commands.next().unwrap_or(Value::Null);
            let tool = if rng.chance(90) {
                "Bash"
            } else {
                *rng.pick(TOOLS)
            };
            check_case(
                s(tool),
                obj(vec![("command", s(field_str(&data, "command")))]),
                field_str(&data, "home"),
            )
        } else if roll < 65 {
            let shape = *rng.pick(G6_SHAPES);
            let command = shape
                .replace("{s}", rng.pick(&secrets))
                .replace("{m}", rng.pick(MEMORY_PATHS));
            let command = if rng.chance(20) {
                mutate(&mut rng, &command)
            } else {
                command
            };
            let h = home(&mut rng);
            bash_case(&command, &h)
        } else {
            let tool = if rng.chance(85) {
                rng.pick(TOOLS).to_string()
            } else {
                let tool = rng_pick_owned(&mut rng, TOOLS);
                mutate(&mut rng, tool)
            };
            let mut map = Map::new();
            for _ in 0..1 + rng.below(4) {
                let key = rng.pick(KEYS).to_string();
                let v = value(&mut rng, 3, &secrets);
                map.insert(key, v);
            }
            let input = if rng.chance(3) {
                value(&mut rng, 1, &secrets)
            } else {
                Value::Object(map)
            };
            let h = home(&mut rng);
            check_case(s(&tool), input, &h)
        };
        out.push(case);
    }
    out
}
