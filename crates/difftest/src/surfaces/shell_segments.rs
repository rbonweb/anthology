//! The `shell.segments` difftest surface: `anthology_shell::segments` (and the shell option parsers it uses)
//! against guard_rules.py through `port/difftest/oracle/shell.py`.
//!
//! Cases (`{"op": ...}`, oracle module docs): `segments` (text, depth), `shell_parse` (prog, args, i) and
//! `script_starts` (words). Both sides print the canonical JSON of the result (`COMPACT_SORTED`, equal to
//! the oracle's `json.dumps(sort_keys=True, separators=(",", ":"))`), with groups numbered by first
//! appearance. Seeds: `port/difftest/seeds/shell.jsonl` (`port/extract/shell_seeds.py`), the hand-written
//! `TRICKY` cases and sweeps of every code point through the Unicode-sensitive predicates. Generated: a
//! grammar over the lexer's whole surface, mutated and spliced seeds, deep nesting and option lists.
//! `shell.analyze` reuses the helpers and the generator here.

use std::collections::HashMap;

use anthology_pycompat::json::{self, COMPACT_SORTED, Int, Map, Value};
use anthology_shell::{GroupId, Lexed, ShellMode, script_starts, segments, shell_parse};

use crate::case::{Case, Rng, Surface, repo_root};
use crate::observe::{Observed, from_oracle};
use crate::oracle::Oracle;

/// The surface name (plan §1.4).
pub const NAME: &str = "shell.segments";

/// Whether `difftest --all` runs this surface on macOS too (`ci/jobs/difftest-macos.sh`); set
/// `false` only for a surface that cannot run there, and say why.
pub const ON_MACOS: bool = true;

/// The registered implementation.
pub fn surface() -> Option<Box<dyn Surface>> {
    Some(Box::new(ShellSegments))
}

/// The committed seed file (`port/extract/shell_seeds.py`), relative to the repository root.
pub(crate) const SEEDS: &str = "port/difftest/seeds/shell.jsonl";

const OPS: &[&str] = &["segments", "shell_parse", "script_starts"];

struct ShellSegments;

impl Surface for ShellSegments {
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
            sweep_cases()
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
        from_oracle(&oracle.call("shell", &c.data))
    }

    fn run_rust(&self, c: &Case) -> Observed {
        observed(run_rust(&c.data))
    }
}

/// The seed cases of the given ops, in file order. Panics with the path when the file is unreadable.
pub(crate) fn load_seeds(ops: &[&str]) -> Vec<Value> {
    let path = repo_root().join(SEEDS);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("difftest: cannot read {}: {e}", path.display()));
    seed_cases(&text, ops)
}

/// What the Rust side printed: the canonical result on stdout, or exit 2 for an op it does not own.
pub(crate) fn observed(result: Option<Value>) -> Observed {
    match result {
        Some(v) => Observed::text(0, &json::dumps(&v, &COMPACT_SORTED), ""),
        None => Observed::text(2, "", "shell: unknown op"),
    }
}

// ------------------------------------------------------------------ values

pub(crate) fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

pub(crate) fn int(n: usize) -> Value {
    Value::Int(Int::Small(n as i64))
}

pub(crate) fn strs<S: AsRef<str>>(items: &[S]) -> Value {
    Value::Array(items.iter().map(|t| s(t.as_ref())).collect())
}

pub(crate) fn obj(fields: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in fields {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

/// Group ordinals by first appearance (the oracle numbers Python's group objects the same way).
#[derive(Default)]
pub(crate) struct Ordinals {
    ids: HashMap<GroupId, usize>,
    order: Vec<GroupId>,
}

impl Ordinals {
    pub(crate) fn of(&mut self, id: GroupId) -> usize {
        *self.ids.entry(id).or_insert_with(|| {
            self.order.push(id);
            self.order.len() - 1
        })
    }

    /// The groups in ordinal order, each as the list of its texts.
    pub(crate) fn texts(&self, groups: &[anthology_shell::Group]) -> Value {
        let by_id: HashMap<GroupId, &Vec<String>> =
            groups.iter().map(|g| (g.id, &g.texts)).collect();
        Value::Array(
            self.order
                .iter()
                .map(|id| strs(by_id.get(id).map_or(&[][..], |t| t.as_slice())))
                .collect(),
        )
    }
}

pub(crate) fn field<'v>(data: &'v Value, key: &str) -> Option<&'v Value> {
    data.as_object()?.get(key)
}

pub(crate) fn field_str<'v>(data: &'v Value, key: &str) -> &'v str {
    field(data, key).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn field_usize(data: &Value, key: &str) -> usize {
    field(data, key)
        .and_then(Value::as_i64)
        .map_or(0, |n| n.max(0) as usize)
}

pub(crate) fn field_strings(data: &Value, key: &str) -> Vec<String> {
    field(data, key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|v| v.as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ------------------------------------------------------------------ the Rust side

fn mode_name(mode: ShellMode) -> &'static str {
    match mode {
        ShellMode::Command => "c",
        ShellMode::Stdin => "stdin",
        ShellMode::File => "file",
    }
}

fn pairs(items: &[(usize, String)]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|(k, t)| Value::Array(vec![int(*k), s(t)]))
            .collect(),
    )
}

/// The oracle's `_segments` rendering: groups numbered by first appearance.
pub(crate) fn render_lexed(lexed: &Lexed) -> Value {
    let mut ordinals = Ordinals::default();
    let mut segments = Vec::new();
    for seg in &lexed.segments {
        let ordinal = ordinals.of(seg.group);
        let redirs = seg
            .redirs
            .iter()
            .map(|r| Value::Array(vec![s(&r.op), s(&r.target)]))
            .collect();
        segments.push(obj(vec![
            ("tokens", strs(&seg.tokens)),
            ("redirs", Value::Array(redirs)),
            ("group", int(ordinal)),
        ]));
    }
    obj(vec![
        ("lenient", Value::Bool(lexed.lenient)),
        ("incomplete", Value::Bool(lexed.incomplete)),
        ("segments", Value::Array(segments)),
        ("groups", ordinals.texts(&lexed.groups)),
    ])
}

/// Run one case's data in Rust: the result to print, or `None` for an op this surface does not own.
pub(crate) fn run_rust(data: &Value) -> Option<Value> {
    let result = match field_str(data, "op") {
        "segments" => render_lexed(&segments(
            field_str(data, "text"),
            field_usize(data, "depth"),
        )),
        "shell_parse" => {
            let args = field_strings(data, "args");
            let i = field_usize(data, "i").min(args.len());
            let parsed = shell_parse(field_str(data, "prog"), &args, i);
            obj(vec![
                ("mode", s(mode_name(parsed.mode))),
                ("start", int(parsed.start)),
                ("scripts", pairs(&parsed.scripts)),
            ])
        }
        "script_starts" => obj(vec![(
            "starts",
            pairs(&script_starts(&field_strings(data, "words"))),
        )]),
        _ => return None,
    };
    Some(result)
}

// ------------------------------------------------------------------ randomness

/// SplitMix64: small, deterministic, good enough to drive a generator.
pub(crate) struct GenRng(u64);

impl GenRng {
    pub(crate) fn new(seed: u64) -> GenRng {
        GenRng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (`n > 0`).
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    pub(crate) fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    pub(crate) fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

// ------------------------------------------------------------------ the generator

/// Characters the lexer treats specially, and Python-vs-Rust Unicode traps.
pub(crate) const ALPHABET: &[&str] = &[
    "'",
    "\"",
    "\\",
    "`",
    "$",
    "(",
    ")",
    "{",
    "}",
    "[",
    "]",
    "<",
    ">",
    "|",
    "&",
    ";",
    "#",
    "\n",
    "\r",
    "\t",
    " ",
    "-",
    "=",
    "~",
    "*",
    "?",
    "!",
    "+",
    "/",
    ".",
    "_",
    "0",
    "2",
    "9",
    "a",
    "E",
    "c",
    "s",
    "o",
    "O",
    "\u{a0}",
    "\u{1c}",
    "\u{0b}",
    "\u{0c}",
    "\u{85}",
    "\u{3000}",
    "\u{2028}",
    "\0",
    "\u{3a3}",
    "\u{130}",
    "\u{212a}",
    "\u{b2}",
    "\u{663}",
    "\u{ff13}",
    "\u{e9}",
    "\u{1f600}",
    "\u{10d50}",
    "\u{1e900}",
    "\u{200b}",
    "<<",
    "<<-",
    "<<<",
    "$(",
    "${",
    "$'",
    "$\"",
    "\\\n",
    "EOF",
    "&>",
    ">&",
    "2>",
    "|&",
    "&&",
    "||",
    "a=(",
    "\r\n",
];

/// Words: programs, shells, wrappers, options and odd spellings.
pub(crate) const WORDS: &[&str] = &[
    "ls",
    "echo",
    "printf",
    "cat",
    "cd",
    "pushd",
    "git",
    "tar",
    "make",
    "find",
    "npx",
    "playwright",
    "install",
    "chromium",
    "bash",
    "sh",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "/bin/bash",
    "BASH",
    "\u{212a}sh",
    "Sh",
    "fi\u{17f}h",
    "sudo",
    "env",
    "nohup",
    "timeout",
    "10",
    "xargs",
    "rtk",
    "run",
    "err",
    "proxy",
    "docker",
    "exec",
    "python3",
    "-c",
    "-e",
    "-s",
    "-euo",
    "pipefail",
    "-o",
    "-O",
    "+O",
    "--rcfile",
    "--command",
    "--comm=x",
    "-ic",
    "-C",
    "--",
    "-",
    "x.sh",
    "~",
    "~/x",
    "$HOME",
    "${HOME}/a",
    "~user/x",
    "a\u{a0}b",
    "*.txt",
    "{a,b}",
    "file:///x",
    "\u{3a3}",
    "\u{391}\u{3a3}",
    "\u{130}",
    "2",
    "\u{b2}",
    "\u{663}",
    "a=1",
    "FOO=bar",
    "!",
    "if",
    "then",
    "do",
    "{",
    "}",
    "coproc",
    "eval",
    "source",
    ".",
];

const HEREDOC_NAMES: &[&str] = &["EOF", "X", "_a.b-c", "END", "eof", "9x", "A B"];

pub(crate) struct Gen<'r> {
    pub(crate) rng: &'r mut GenRng,
    /// Nesting levels left: a nested command at 0 is a plain word.
    pub(crate) left: usize,
}

impl Gen<'_> {
    /// A nested command one level down (bounded by `left`).
    fn sub(&mut self) -> String {
        if self.left == 0 {
            return self.plain();
        }
        self.left -= 1;
        let budget = self.left;
        let out = self.command(budget);
        self.left += 1;
        out
    }

    fn word(&mut self) -> String {
        match self.rng.below(20) {
            0 => format!("'{}'", self.words(2)),
            1 => format!("\"{}\"", self.dq_body()),
            2 => format!("$'{}\\'{}'", self.plain(), self.plain()),
            3 => format!("$({})", self.sub()),
            4 => format!("`{}`", self.sub()),
            5 => format!("<({})", self.sub()),
            6 => format!("${{{}}}", self.plain()),
            7 => format!("{}\\ {}", self.plain(), self.plain()),
            8 => {
                let c = self.rng.pick(ALPHABET).to_string();
                format!("{}{c}{}", self.plain(), self.plain())
            }
            _ => self.plain(),
        }
    }

    fn plain(&mut self) -> String {
        self.rng.pick(WORDS).to_string()
    }

    fn words(&mut self, max: usize) -> String {
        let n = 1 + self.rng.below(max);
        (0..n).map(|_| self.word()).collect::<Vec<_>>().join(" ")
    }

    fn dq_body(&mut self) -> String {
        match self.rng.below(8) {
            0 => format!("a $({}) b", self.sub()),
            1 => format!("`{}`", self.sub()),
            2 => "${x#\\\"}".to_string(),
            3 => "\\\"\\$\\\\\\`\\n".to_string(),
            4 => format!("{}\\\n{}", self.plain(), self.plain()),
            5 => "${unterminated".to_string(),
            _ => self.words(3),
        }
    }

    fn redirect(&mut self) -> String {
        const OPS: &[&str] = &[
            ">", ">>", ">|", ">&", "<", "<>", "<&", "&>", "&>>", "2>", "2>&", "1>>", "9<",
            "\u{b2}>", "\u{663}>", "\"2\">", "'1'<", "x2>", "<<<", "2<<<", ">(", "<(",
        ];
        let op = *self.rng.pick(OPS);
        if op.ends_with('(') {
            return format!("{op}{})", self.sub());
        }
        let sp = if self.rng.chance(30) { " " } else { "" };
        format!("{op}{sp}{}", self.word())
    }

    fn heredoc(&mut self) -> String {
        let name = *self.rng.pick(HEREDOC_NAMES);
        let quote = *self.rng.pick(&["", "", "'", "\"", "'\""]);
        let close = if quote == "'\"" { "'" } else { quote };
        let dash = if self.rng.chance(30) { "-" } else { "" };
        let sp = if self.rng.chance(30) { " \t" } else { "" };
        let mut body = String::new();
        for _ in 0..self.rng.below(3) {
            match self.rng.below(5) {
                0 => body.push_str(&format!("x $({}) y\n", self.sub())),
                1 => body.push_str(&format!("`{}`\n", self.sub())),
                2 => body.push_str(&format!("{name}x\n")),
                _ => body.push_str(&format!("{}\n", self.words(3))),
            }
        }
        let pad = *self.rng.pick(&[
            "", " ", "\t", "\u{a0}", "\u{1c}", "\u{85}", "\u{200b}", "\r",
        ]);
        let end = if self.rng.chance(15) {
            String::new() // unterminated
        } else {
            format!("{pad}{name}{pad}\n")
        };
        format!("cat <<{dash}{sp}{quote}{name}{close}\n{body}{end}")
    }

    fn array(&mut self) -> String {
        let name = *self.rng.pick(&["a", "deps", "_x1", "1a", "a+", "a.b"]);
        let op = if self.rng.chance(20) { "+=" } else { "=" };
        let mut inner = Vec::new();
        for _ in 0..self.rng.below(4) {
            match self.rng.below(6) {
                0 => inner.push("# it's (\n".to_string()),
                1 => inner.push(format!("$({})", self.sub())),
                2 => inner.push("\n".to_string()),
                3 => inner.push(self.heredoc()),
                _ => inner.push(self.word()),
            }
        }
        let close = if self.rng.chance(10) { "" } else { ")" };
        format!("{name}{op}({}{close}", inner.join(" "))
    }

    fn simple(&mut self, budget: usize) -> String {
        let mut parts = Vec::new();
        if self.rng.chance(15) {
            parts.push("FOO=1".to_string());
        }
        parts.push(self.plain());
        for _ in 0..self.rng.below(5) {
            match self.rng.below(14) {
                0 => parts.push(self.redirect()),
                1 if budget > 0 => parts.push(format!("$({})", self.sub())),
                2 if budget > 0 => parts.push(format!("\"$({})\"", self.sub())),
                3 => parts.push("# it's a comment".to_string()),
                4 => parts.push(self.array()),
                _ => parts.push(self.word()),
            }
        }
        parts.join(" ")
    }

    /// A command line with up to `budget` more levels of nesting.
    pub(crate) fn command(&mut self, budget: usize) -> String {
        const SEPS: &[&str] = &[
            "; ", ";", " && ", "||", " | ", "|", "|&", " & ", "&", "\n", "\r\n", " &\n", "\n\n",
        ];
        let mut out = String::new();
        let n = 1 + self.rng.below(3);
        for k in 0..n {
            if k > 0 {
                out.push_str(self.rng.pick(SEPS));
            }
            match self.rng.below(12) {
                0 => out.push_str(&self.heredoc()),
                1 if budget > 0 => out.push_str(&format!("( {} )", self.sub())),
                2 if budget > 0 => out.push_str(&format!("{{ {}; }}", self.sub())),
                3 if budget > 0 => {
                    let script = self.sub().replace('\'', "");
                    let shell = self.shell_prefix();
                    out.push_str(&format!("{shell} '{script}'"));
                }
                4 => out.push_str(&self.array()),
                _ => out.push_str(&self.simple(budget)),
            }
        }
        out
    }

    fn shell_prefix(&mut self) -> String {
        const PREFIXES: &[&str] = &[
            "bash -c",
            "bash -euo pipefail -c",
            "sh -ec",
            "zsh --emulate sh -c",
            "fish -c",
            "fish --comm",
            "fish -ic",
            "bash -s <<<",
            "sudo bash -c",
            "env FOO=1 sh -c",
            "rtk run -c",
            "npx -c",
            "/bin/sh -xc",
            "\u{212a}sh -c",
        ];
        self.rng.pick(PREFIXES).to_string()
    }

    /// A deeply nested command (past MAX_DEPTH sometimes).
    fn deep(&mut self) -> String {
        let depth = 1 + self.rng.below(16);
        let (open, close) = *self.rng.pick(&[
            ("$(", ")"),
            ("\"$(", ")\""),
            ("(", ")"),
            ("a=(", ")"),
            ("<(", ")"),
        ]);
        let inner = self.simple(0);
        let close_all = !self.rng.chance(20);
        let mut out = open.repeat(depth);
        out.push_str(&inner);
        if close_all {
            out.push_str(&close.repeat(depth));
        }
        out
    }
}

/// Mutate a seed text: 1-4 random edits with metachar-heavy insertions.
pub(crate) fn mutate(rng: &mut GenRng, text: &str) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    for _ in 0..1 + rng.below(4) {
        let at = rng.below(chars.len() + 1);
        match rng.below(6) {
            0 | 1 => {
                let ins: Vec<char> = rng.pick(ALPHABET).chars().collect();
                chars.splice(at..at, ins);
            }
            2 if at < chars.len() => {
                chars.remove(at);
            }
            3 if at < chars.len() => {
                let ins: Vec<char> = rng.pick(ALPHABET).chars().collect();
                chars.splice(at..at + 1, ins);
            }
            4 if !chars.is_empty() => {
                let a = rng.below(chars.len());
                let b = (a + 1 + rng.below(12)).min(chars.len());
                let piece: Vec<char> = chars[a..b].to_vec();
                let at = rng.below(chars.len() + 1);
                chars.splice(at..at, piece);
            }
            _ => {
                let unbalanced = *rng.pick(&["'", "\"", "`", "$(", "${", "(", ")", "\\"]);
                chars.splice(at..at, unbalanced.chars());
            }
        }
    }
    chars.into_iter().collect()
}

/// Seed cases (`port/difftest/seeds/shell.jsonl`) for the given ops, in file order.
pub(crate) fn seed_cases(seeds_jsonl: &str, ops: &[&str]) -> Vec<Value> {
    seeds_jsonl
        .lines()
        .filter_map(|line| json::loads(line).ok())
        .filter(|case| ops.contains(&field_str(case, "op")))
        .collect()
}

fn segments_case(text: String, depth: usize) -> Value {
    obj(vec![
        ("op", s("segments")),
        ("text", Value::Str(text)),
        ("depth", int(depth)),
    ])
}

pub(crate) fn sweep_chars() -> Vec<char> {
    // Every non-surrogate code point above ASCII, plus the ASCII letters and digits.
    let ascii = ('0'..='9').chain('a'..='z').chain('A'..='Z');
    ascii
        .chain((0x80u32..=0x10FFFF).filter_map(char::from_u32))
        .collect()
}

/// Hand-written corner cases: each is a place where a port could plausibly differ from guard_rules.py.
pub(crate) const TRICKY: &[&str] = &[
    // a quoted fd word is dropped but leaves `quoted` set: the heredoc then counts as quoted
    "\"2\"<<EOF\n$(npx playwright install)\nEOF",
    "'2'<<EOF\n`npx x`\nEOF",
    "\u{b2}<<EOF\n$(a)\nEOF",
    "2<<EOF\n$(a)\nEOF",
    // escapes before multibyte characters, at the end, and before a newline
    "echo \\\u{e9}x \\\u{1f600} \\",
    "echo \"a\\\u{e9}b\" \"\\\n\"",
    "$'a\\\u{e9}' $'\\'' $'x\\",
    "echo ${x\u{e9}",
    "echo \"${x\u{e9}",
    "`a\\\u{e9}`",
    "$(a \\\u{e9})",
    // heredocs: several per line, unterminated, CRLF, strip(), inside substitutions and arrays
    "cat <<A <<B\na\nA\nb\nB\nls",
    "cat <<A <<A\nx\nA\ny\nA",
    "cat <<X\ncat <<X\nls",
    "cat <<EOF\r\nx\r\nEOF\r\nls",
    "cat <<EOF\nx\n\u{85}EOF\u{a0}\nls",
    "cat <<-'EOF'\n\t$(a)\n\tEOF\nb",
    "cat <<\"E-1.x\"\n$(a)\nE-1.x\n",
    "cat << 'A B'\nx\nA B\n",
    "x=$(cat <<'EOF'\n)\nEOF\n)",
    "x=$(cat <<EOF\n$(echo ))\nEOF\n); ls",
    "x=\"$(cat <<EOF\n\"\nEOF\n)\"",
    "a=(cat <<EOF\nb\nEOF\n c)",
    "a=( <<EOF\n$(x)\nEOF\n)",
    "a=(\n<(npx playwright install)\n)",
    "a=(x) b=(y)z c+=(1 2",
    "a=(x # (\n) (y)",
    "\"a\"=(b)",
    "a\\=(b)",
    "f() { a=( # it's\n x\n ) ; }",
    // redirect forms
    "a 2>&1 &>f &>>g >|h <>i <&0 >&- 3<<<w 9>(x) <(y)",
    "a&>(b)",
    "a <<<",
    "a >",
    "a |& b || c && d | e",
    "a & b &",
    "2>f",
    "\"\" 2>f",
    // comments and words
    "a#b # c\nd",
    "#!/bin/sh\nls",
    "a=(# c\n)",
    "echo $\"x\" $ $$ ${#a[@]} ${x:-a #b}",
    // nesting at and past MAX_DEPTH
    "$($($($($($($($($($($($($(ls))))))))))))",
    "\"$(\"$(\"$(\"$(\"$(\"$(\"$(\"$(\"$(\"$(\"$(\"$(\"$(ls)\")\")\")\")\")\")\")\")\")\")\")\")\"",
    "`a \\`b \\\\\\`c\\\\\\` \\``",
    // unbalanced: the lenient fallback, with embedded shell scripts
    "echo it's; bash -c -e 'npx playwright install'",
    "echo it's; fish --command='npx playwright install'",
    "echo \"x; \u{212a}sh -c ls",
    "a $(b",
    "a `b",
    "a $'b",
    "a=(x 'y",
];

/// Deterministic sweeps of every code point through the Unicode-sensitive predicates the lexer uses:
/// `isdigit` (an fd word before `>`), `split()` (the lenient path) and `strip()` (heredoc terminators),
/// plus the hand-written `TRICKY` cases at depths 0, 11 and 12.
pub(crate) fn sweep_cases() -> Vec<Value> {
    let chars = sweep_chars();
    let mut out = Vec::new();
    for depth in [0, 11, 12] {
        for text in TRICKY {
            out.push(segments_case(text.to_string(), depth));
        }
    }
    for chunk in chars.chunks(4096) {
        let digits: String = chunk.iter().map(|c| format!("{c}>f ")).collect();
        out.push(segments_case(digits, 0));
        let mut lenient = String::from("'");
        for c in chunk {
            lenient.push('a');
            lenient.push(*c);
            lenient.push_str("b ");
        }
        out.push(segments_case(lenient, 0));
    }
    for chunk in chars.chunks(256) {
        let mut heredocs = String::new();
        for c in chunk {
            heredocs.push_str(&format!("cat <<EOF\n{c}EOF{c}\nEOF\n"));
        }
        out.push(segments_case(heredocs, 0));
    }
    out
}

fn shell_parse_case(rng: &mut GenRng) -> Value {
    const ARGS: &[&str] = &[
        "-c",
        "-e",
        "-euo",
        "pipefail",
        "-o",
        "-O",
        "+O",
        "extglob",
        "--rcfile",
        "--rcfile=x",
        "x",
        "--init-file",
        "--emulate",
        "sh",
        "--norc",
        "-s",
        "-",
        "--",
        "-xs",
        "script.sh",
        "echo hi",
        "-cx",
        "-ic",
        "-C",
        "-i",
        "-l",
        "-d",
        "3",
        "--command",
        "--command=ls",
        "--comm",
        "--comm=ls",
        "--co",
        "--init-command",
        "--ini=ls",
        "--debug",
        "--debug=x",
        "--private",
        "--unknown",
        "--unknown=1",
        "-icX",
        "-cX",
        "-Cfoo",
        "-n",
        "-p",
        "-dD",
        "-f",
        "file.fish",
        "--profile",
        "--pro",
        "--de",
        "--interactive",
        "--int",
        "--=",
        "--c",
        "-\u{e9}c",
        "-o\u{e9}",
        "\u{212a}sh",
        "",
    ];
    let prog = *rng.pick(&["bash", "sh", "zsh", "dash", "ksh", "fish", "fish"]);
    let n = rng.below(7);
    let args: Vec<&str> = (0..n).map(|_| *rng.pick(ARGS)).collect();
    obj(vec![
        ("op", s("shell_parse")),
        ("prog", s(prog)),
        ("args", strs(&args)),
        ("i", int(rng.below(n + 1))),
    ])
}

fn script_starts_case(rng: &mut GenRng) -> Value {
    const PROGS: &[&str] = &[
        "bash",
        "sh",
        "fish",
        "zsh",
        "/bin/bash",
        "BASH",
        "\u{212a}sh",
        "@scope/sh",
        "sh/",
        "sudo",
        "npx",
        "env",
        "docker",
        "run",
        "img",
        "ls",
        "-c",
        "-xc",
        "-ec",
        "-c1",
        "-C",
        "--command=x",
        "-cX",
        "x",
        "echo hi",
        "--",
        "-",
        "-s",
        "-euo",
        "pipefail",
        "--rcfile",
        "--comm",
        "--unknown",
    ];
    let n = rng.below(8);
    let words: Vec<&str> = (0..n).map(|_| *rng.pick(PROGS)).collect();
    obj(vec![("op", s("script_starts")), ("words", strs(&words))])
}

/// `n` generated cases for seed `seed`: grammar commands, mutated and spliced seeds, deep nesting, and
/// shell argument lists.
pub(crate) fn generate(seed: u64, n: usize, seeds: &[Value]) -> Vec<Value> {
    let mut rng = GenRng::new(seed);
    let texts: Vec<&str> = seeds
        .iter()
        .filter(|c| field_str(c, "op") == "segments")
        .map(|c| field_str(c, "text"))
        .collect();
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let roll = rng.below(100);
        let depth = if rng.chance(10) { rng.below(15) } else { 0 };
        let case = if roll < 35 || texts.is_empty() {
            let mut g = Gen {
                rng: &mut rng,
                left: 4,
            };
            let text = if roll < 5 { g.deep() } else { g.command(3) };
            segments_case(text, depth)
        } else if roll < 75 {
            let text = *rng.pick(&texts);
            segments_case(mutate(&mut rng, text), depth)
        } else if roll < 85 {
            let a = *rng.pick(&texts);
            let b = *rng.pick(&texts);
            let sep = *rng.pick(&["\n", "; ", " | ", " ", "$(", "<<EOF\n", "'"]);
            segments_case(format!("{a}{sep}{b}"), depth)
        } else if roll < 93 {
            shell_parse_case(&mut rng)
        } else {
            script_starts_case(&mut rng)
        };
        out.push(case);
    }
    out
}
