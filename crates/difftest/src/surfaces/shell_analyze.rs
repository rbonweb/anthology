//! The `shell.analyze` difftest surface: `anthology_shell::analyze` against guard_rules.py `_analyze`
//! through `port/difftest/oracle/shell.py` (`Entry` minus `opaque`, plan SH2).
//!
//! Case: `{"op": "analyze", "command": C, "home": H}`. Both sides print the canonical JSON of every entry's
//! `_Entry` fields, then every alternate reading's (`Analysis.alternates`), groups numbered by first
//! appearance among them, and `incomplete` (false in Python, which raises `_TooComplex` instead: an
//! oracle-marked D-7). The reference is the fix-wave H+I `guard_rules_fw.py` (see the oracle). Seeds: the analyze seeds of `port/difftest/seeds/shell.jsonl`, the hand-written `TRICKY`
//! commands, and a sweep of every code point through `lower()` (with Final_Sigma contexts) via the program
//! name. Generated: wrapper/runner/shell/docker/find/cd compositions, mutated seeds, the segments grammar,
//! and odd homes.

use anthology_pycompat::json::Value;
use anthology_shell::{Analysis, Entry, analyze};

use super::shell_segments::{
    Gen, GenRng, Ordinals, field_str, int, load_seeds, mutate, obj, observed, s, strs, sweep_chars,
};
use crate::case::{Case, Rng, Surface};
use crate::observe::Observed;
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "shell.analyze";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(ShellAnalyze))
}

const OPS: &[&str] = &["analyze"];

struct ShellAnalyze;

impl Surface for ShellAnalyze {
    fn name(&self) -> &'static str {
        NAME
    }

    fn seeds(&self) -> Vec<Case> {
        let mut out: Vec<Case> = load_seeds(OPS)
            .into_iter()
            .enumerate()
            .map(|(k, data)| Case::new(format!("seed:{}", k + 1), data))
            .collect();
        out.extend(
            fixed_cases()
                .into_iter()
                .enumerate()
                .map(|(k, data)| Case::new(format!("sweep:{}", k + 1), data)),
        );
        out
    }

    fn generate(&self, rng: &mut Rng, n: usize) -> Vec<Case> {
        generate(rng.next_u64(), n, &load_seeds(OPS))
            .into_iter()
            .enumerate()
            .map(|(k, data)| Case::new(format!("gen:{}", k + 1), data))
            .collect()
    }

    fn run_python(&self, c: &Case, oracle: &mut Oracle) -> Observed {
        crate::observe::from_oracle(&oracle.call("shell", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        observed(run_rust(&c.data))
    }
}

fn render_entry(e: &Entry, ordinals: &mut Ordinals) -> Value {
    let redirs = e
        .redirs
        .iter()
        .map(|r| Value::Array(vec![s(&r.op), s(&r.target)]))
        .collect();
    obj(vec![
        ("raw", strs(&e.raw)),
        ("redirs", Value::Array(redirs)),
        ("prog_path", s(&e.prog_path)),
        ("prog", s(&e.prog)),
        ("args", strs(&e.args)),
        ("cwd", e.cwd.as_deref().map_or(Value::Null, s)),
        ("exempt", Value::Bool(e.exempt)),
        ("group", int(ordinals.of(e.group))),
        ("code", strs(&e.code)),
    ])
}

/// The oracle's `_analyze` rendering. An incomplete analysis renders as nothing but `incomplete` (what it found
/// before it stopped is not a result: the guard blocks), as the oracle renders the reference's `_TooComplex`.
pub(crate) fn render_analysis(analysis: &Analysis) -> Value {
    if analysis.incomplete {
        return obj(vec![
            ("incomplete", Value::Bool(true)),
            ("entries", Value::Array(Vec::new())),
            ("alternates", Value::Array(Vec::new())),
            ("groups", Value::Array(Vec::new())),
        ]);
    }
    let mut ordinals = Ordinals::default();
    let entries = analysis
        .entries
        .iter()
        .map(|e| render_entry(e, &mut ordinals))
        .collect();
    let alternates = analysis
        .alternates
        .iter()
        .map(|e| render_entry(e, &mut ordinals))
        .collect();
    obj(vec![
        ("incomplete", Value::Bool(analysis.incomplete)),
        ("entries", Value::Array(entries)),
        ("alternates", Value::Array(alternates)),
        ("groups", ordinals.texts(&analysis.groups)),
    ])
}

/// Run one case's data in Rust: the result to print, or `None` for an op this surface does not own.
pub(crate) fn run_rust(data: &Value) -> Option<Value> {
    if field_str(data, "op") != "analyze" {
        return None;
    }
    Some(render_analysis(&analyze(
        field_str(data, "command"),
        field_str(data, "home"),
    )))
}

fn case(command: String, home: &str) -> Value {
    obj(vec![
        ("op", s("analyze")),
        ("command", Value::Str(command)),
        ("home", s(home)),
    ])
}

/// Another user's macOS home, assembled so no literal user path lands in the repository.
fn other_home() -> String {
    format!("/{}ers/someone", "Us")
}

const HOMES: &[&str] = &[
    "/home/tester",
    "/home/tester",
    "/home/tester",
    "",
    "/",
    "//",
    "relative",
    "~",
    "$HOME",
    "/home/tester/",
    "/home/\u{3a3}\u{391}\u{3a3}",
    "/home/K\u{212a}",
    "/h\\o",
    "/home/a b",
];

/// Words that wrap, peel or redirect a command (guard_rules.py `_strip_prefixes` and friends).
const PREFIXES: &[&str] = &[
    "sudo",
    "sudo -u bob",
    "sudo -iu bob",
    "sudo -E",
    "sudo --",
    "doas",
    "doas -u x",
    "env FOO=1",
    "env -i",
    "env -u X",
    "env -S",
    "env --split-string",
    "nohup",
    "time -f x",
    "timeout 5",
    "timeout -s KILL 5",
    "timeout",
    "gtimeout 3",
    "nice -n 5",
    "nice",
    "xargs -I{}",
    "xargs -n1",
    "xargs",
    "watch -n 1",
    "watch",
    "caffeinate -t 3",
    "stdbuf -oL",
    "arch -arm64",
    "setsid",
    "ionice -c 3",
    "unbuffer",
    "exec -a x",
    "exec",
    "command",
    "command -v",
    "command -p",
    "!",
    "if",
    "then",
    "do",
    "{",
    "coproc",
    "FOO=bar",
    "\"\"",
    "$(x)",
    "rtk",
    "rtk -v",
    "rtk run",
    "rtk run -c",
    "rtk run -vc",
    "rtk run --command",
    "rtk run --shell bash -c",
    "rtk run --shell=",
    "rtk err",
    "rtk test --shell zsh",
    "rtk summary --shell=fish",
    "rtk proxy",
    "rtk bypass --",
    "rtk git",
    "rtk npx",
    "rtk -c",
    "npx",
    "npx -y",
    "npx -p x",
    "npx --package=x",
    "npx -c",
    "npx --call",
    "npx --",
    "bunx",
    "pnpx",
    "uvx --from x",
    "uv run",
    "uv tool run",
    "uv --directory e2e run",
    "uv -p 3 run",
    "poetry run",
    "poetry -C sub run",
    "pipenv run",
    "hatch -e test run",
    "pdm run",
    "rye run",
    "conda run -n x",
    "uv pip",
    "npm exec",
    "npm run",
    "npm x",
    "npm i",
    "npm --prefix x exec",
    "pnpm dlx",
    "pnpm exec",
    "pnpm run",
    "pnpm --filter x",
    "pnpm",
    "yarn",
    "yarn run",
    "yarn workspace w",
    "yarn dlx",
    "yarn -w",
    "bun x",
    "bun run",
    "bun",
    "pipx run",
    "pipx",
    "python3 -m",
    "python -m",
    "python3.11 -u -m",
    "python3 x.py -m",
    "PYTHON3 -m",
    "eval",
    "bash -c",
    "sh -euo pipefail -c",
    "fish -c",
    "fish --command",
    "fish -C",
    "zsh -fc",
    "bash",
    "docker run img",
    "docker run -i img bash -s",
    "docker exec -i c",
    "docker-compose run s",
    "docker run img sh -c",
    "find . -exec",
    "find . -execdir",
    "find / -ok",
    "cd",
    "pushd",
];

/// Programs and their arguments.
const PROGRAMS: &[&str] = &[
    "npx playwright install",
    "playwright install chromium",
    "ls -la",
    "cat ~/.config/chromium/x",
    "cat $HOME/.config/x",
    "cat ${HOME}/x",
    "cat ~bob/x",
    "cat ~bob",
    "python3",
    "python3 -",
    "node",
    "node -e 'x'",
    "sh",
    "bash -s",
    "bash -s -- x",
    "echo hi",
    "echo -n -e 'a\\nb'",
    "printf 'x\\n'",
    "open -a 'Google Chrome'",
    "agent-browser open x",
    "pip install playwright",
    "brew install chromium",
    "git push",
    "git -C ~/x status",
    "git -C",
    "tar -C /tmp -x",
    "make --directory x",
    "make -C",
    "rm -rf x",
    "cd ..",
    "cd -",
    "cd",
    "cd ~/a/../b",
    "cd /a/./b//c/",
    "cd //x",
    "cd ///x",
    "cd ../../..",
    "cd \u{130}",
    "cd A\u{3a3}",
    "pushd /tmp",
    "source x.sh",
    ". ./env",
    "source",
    ".",
    "eval \"$x\"",
    "eval ls",
    "$(echo rm) x",
    "$x",
    "*.sh",
    "\u{212a}sh -c ls",
    "PYTHON3 -c 'x'",
    "/usr/bin/Python3.12 x",
    "\u{3a3}BASH",
    "ruby -e x",
    "perl5.36 -e x",
    "deno run x",
    "ts-node x",
    "osascript -e x",
    "rscript x",
    "lua5.4",
    "find . -name x -exec rm {} \\;",
    "find . -exec sh -c 'ls' \\;",
    "find . -exec",
    "-v",
    "--",
    "x@1.2",
    "@scope/pkg@3",
    "@playwright/test",
    "playwright@latest install",
];

const SHAPES: &[&str] = &[
    "{c}",
    "{c}",
    "{c}",
    "{c} | sh",
    "{c} | python3 -",
    "echo '{q}' | sh",
    "printf '{q}' | bash -s",
    "cat <<EOF | sh\n{c}\nEOF",
    "bash <<'EOF'\n{c}\nEOF",
    "python3 - <<EOF\n{c}\nEOF",
    "sh <<< '{q}'",
    "cd sub; {c}",
    "cd ~ && {c} && cd -",
    "{c}; {c}",
    "bash -c '{q}'",
    "sh -c \"sudo bash -c '{q}'\"",
    "echo $({c})",
    "x=`{c}`",
    "( {c} )",
    "find . -exec {c} \\;",
    "find . -exec find . -exec {c} \\; \\;",
    "docker run img bash -c '{q}'",
    "echo '{q}' | docker run -i img sh",
    "watch '{c}'",
    "env -S '{q}'",
    "rtk run -c '{q}'",
    "rtk err '{q}'",
    "npx -c '{q}'",
    "eval '{q}'",
    "{c} 2>&1 >/dev/null",
];

/// A command composed from the vocabulary above.
fn compose(rng: &mut GenRng) -> String {
    let mut inner = String::new();
    for _ in 0..rng.below(3) {
        inner.push_str(rng.pick(PREFIXES));
        inner.push(' ');
    }
    inner.push_str(rng.pick(PROGRAMS));
    let shape = *rng.pick(SHAPES);
    let quoted = inner.replace('\'', "");
    let mut out = shape.replace("{c}", &inner).replace("{q}", &quoted);
    if rng.chance(20) {
        out = format!(
            "{out}{}{}",
            rng.pick(&["; ", " && ", " | ", "\n"]),
            compose_flat(rng)
        );
    }
    out
}

fn compose_flat(rng: &mut GenRng) -> String {
    format!("{} {}", rng.pick(PREFIXES), rng.pick(PROGRAMS))
}

/// Hand-written commands where a port could plausibly differ, and the `lower()` sweep.
pub(crate) fn fixed_cases() -> Vec<Value> {
    const TRICKY: &[&str] = &[
        "cd ~; cd x; cat y",
        "cd; cd -; cd -L /a; ls",
        "git -C a -C b status; tar --directory /x -C",
        "cd /a && cd \"$HOME\" && cd ~u/x && ls",
        "pushd ~/Library/Application\\ Support/Google",
        "find . -exec find . -exec find . -exec ls \\; \\; \\;",
        "find . -exec cd /tmp \\; -exec ls + ; ls",
        "echo 'npx x' | echo 'npx y' | sh",
        "cat <<A | python3 - | sh\nprint(1)\nA",
        "echo -nE 'a\\nb' | node",
        "printf -- '-n x' | sh",
        "docker run img fish -c 'ls' bash -s",
        "docker run img sh -s <<'EOF'\nls\nEOF",
        "rtk run --shell= -c ls; rtk err --shell= ls x",
        "rtk run -c; rtk run --command",
        "rtk proxy -v -- npx x; rtk bypass",
        "rtk 'npx playwright install'",
        "rtk err 'npx playwright install'",
        "env -S; env -S 'npx x' y; env --split-string=x",
        "timeout; timeout -k 3; timeout 5",
        "watch 'a b' c; watch -n 1 ab",
        "command -v npx; command -V x; command -p npx x",
        "uv tool run x; uv run; uv --directory run; poetry -C run",
        "yarn workspace a b c; yarn workspace a; yarn add x; yarn x y",
        "pnpm -C dir exec x; npm run x; npm exec x",
        "python -u -m pip install x; python x -m y; python3 -m",
        "\"\" \"\" x; '' y",
        "!; ! x; if then else",
        "FOO=1 BAR=2; a=1 b",
        "eval; eval ''; eval 'a; b'",
        "sudo -iu bob -- npx x; sudo -u",
        "xargs -I {} sh -c 'echo {}'",
        "fish --comm='npx x'; fish -ic 'y'",
        "a=(x y) b; deps=(\n npx\n)\nls",
    ];
    let mut out: Vec<Value> = Vec::new();
    for text in TRICKY {
        for home in ["/home/tester", "", "/"] {
            out.push(case(text.to_string(), home));
        }
        out.push(case(text.to_string(), &other_home()));
    }
    // Every code point through `lower()` and `prog_name` (Final_Sigma both ways), 2,048 per case.
    for chunk in sweep_chars().chunks(2048) {
        let mut command = String::new();
        for c in chunk {
            command.push_str(&format!("A{c}\u{3a3}; {c}\u{3a3}x; "));
        }
        out.push(case(command, "/home/tester"));
    }
    out
}

/// `n` generated cases for seed `seed`.
pub(crate) fn generate(seed: u64, n: usize, seeds: &[Value]) -> Vec<Value> {
    let mut rng = GenRng::new(seed);
    let commands: Vec<&str> = seeds.iter().map(|c| field_str(c, "command")).collect();
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let home = if rng.chance(5) {
            other_home()
        } else {
            rng.pick(HOMES).to_string()
        };
        let roll = rng.below(100);
        let command = if roll < 45 || commands.is_empty() {
            compose(&mut rng)
        } else if roll < 85 {
            let text = *rng.pick(&commands);
            mutate(&mut rng, text)
        } else if roll < 92 {
            let a = *rng.pick(&commands);
            let b = compose(&mut rng);
            format!("{a}{}{b}", rng.pick(&["\n", "; ", " | ", " && "]))
        } else {
            let mut g = Gen {
                rng: &mut rng,
                left: 3,
            };
            g.command(3)
        };
        out.push(case(command, &home));
    }
    out
}
