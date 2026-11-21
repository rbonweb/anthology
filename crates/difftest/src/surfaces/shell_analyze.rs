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
        let mut out: Vec<Case> = fw_cases()
            .into_iter()
            .enumerate()
            .map(|(k, data)| Case::new(format!("fw:{}", k + 1), data))
            .collect();
        out.extend(
            load_seeds(OPS)
                .into_iter()
                .enumerate()
                .map(|(k, data)| Case::new(format!("seed:{}", k + 1), data)),
        );
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
    // fix-wave I
    "su -c",
    "su root -c",
    "runuser -u x --",
    "script -qc",
    "flock /tmp/l",
    "flock /tmp/l -c",
    "chroot /",
    "nsenter -t 1",
    "unshare -r",
    "setsid -f",
    "stdbuf -o L",
    "busybox",
    "parallel",
    "parallel -j2 -I@@",
    "sg grp",
    "sg - grp -c",
    "watch -n1",
    "entr -s",
    "gdb -q --args",
    "nix develop -c",
    "nix-shell --run",
    "mise exec --",
    "mise x",
    "direnv exec .",
    "devbox run",
    "pixi run",
    "perf stat",
    "concurrently -n a",
    "hyperfine -w 3",
    "nodemon --exec",
    "xargs -0",
    "xargs --replace",
    "xargs -i",
    "xargs -L1 -I%",
    "dotenv -e .env",
    "env-cmd -f x",
    "systemd-run --user",
    "ltrace -o x",
    "cpulimit -l 5",
    "chpst -u x",
    "corepack",
    "cross-env A=1",
    "GIT_PAGER=x",
    "GIT_SSH_COMMAND='ssh -i k'",
    "GIT_CONFIG_PARAMETERS=\"'core.pager'='x'\"",
    "npm --loglevel x exec",
    "npm exec --omit dev --",
    "pnpm dlx --foo x --",
    "pnpm run e2e --",
    "yarn --frozen-lockfile",
    "bun x --",
    "uvx --with x",
    "pipx run --spec x",
    "python3 -Im",
    "python3 -W ignore -m",
    "env -u X -S",
    "env -S'sudo -u x'",
    "sudo --",
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
    // fix-wave I
    "git -c core.pager='less -R' log",
    "git -c alias.x='!ls' x",
    "git config core.pager 'x y'",
    "git bisect run make test",
    "git submodule foreach 'git pull'",
    "git rebase -x 'make' main",
    "git difftool --extcmd=x",
    "tar --to-command='cat' -xf a",
    "tar --use-comp=zstd -cf a x",
    "tar -I 'zstd -19' -cf a x",
    "rsync -e 'ssh -p 2' a b:c",
    "rsync -avze ssh a b:c",
    "trap 'ls' EXIT",
    "trap -- 'x' INT",
    "trap '' INT",
    "parallel echo ::: a b ::: c",
    "parallel '{1} {2}' ::: a ::: b",
    "sh /dev/stdin",
    "source /dev/stdin",
    "sh <&3",
    "bash /dev/fd/3 3<<<'ls'",
    "exec 3<<<'ls'",
    "sh <(echo ls)",
    "fd . -x ls",
    "fd -e js --exec-batch wc",
    "nodemon -x 'npm test'",
    "concurrently 'a b' 'c'",
    "$'\\x6epx' x",
    "x=$(( 2 * 3 ))",
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
    // fix-wave I
    "echo '{q}' | xargs sh -c",
    "printf '{q}\\n' | xargs -I{} sh -c '{}'",
    "echo '{q}' | xargs",
    "echo '{q}' | parallel",
    "sh <(echo '{q}')",
    "cat <(echo '{q}') | sh",
    "exec 3<<<'{q}'; sh <&3",
    "watch -n1 {c}",
    "sg grp '{q}'",
    "su -c '{q}' root",
    "env -S'{q}'",
    "git -c core.pager='{q}' log",
    "trap '{q}' EXIT",
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

/// Fix-wave H and I reviewer payloads (`fw:<n>`, in this order: `shell.analyze.toml` names the first seven).
/// 1-7 are Rust stricter than the reference (D-8); the rest the oracle reads as Rust does.
pub(crate) fn fw_cases() -> Vec<Value> {
    const FW: &[&str] = &[
        // FW-I review R1-M2: every fd -x group
        "fdfind hostname /etc -x echo hi \\; -x npx playwright install",
        "fd . -x echo a \\; --exec npx playwright install",
        // FW-I review R1-M3: secret loaders run their command
        "doppler run --command 'npx playwright install'",
        "doppler run -- npx playwright install",
        "op run --env-file .env -- npx playwright install",
        "dotenvx run -f .env -- npx playwright install",
        "infisical run --command='npx playwright install'",
        // FW-I review R1-I1 and its negative control
        "npm exec --omit dev -- playwright install",
        "pnpm dlx --some-flag v -- playwright install",
        "pnpm run e2e -- playwright test",
        // FW-H review R1-M1
        "$'\\x6epx' playwright install\n'",
        "npx $'pl\\x61ywright' install\necho '",
        // the negative control the owner named
        "git commit -m \"$(cat <<'EOF'\nmsg\nEOF\n)\"",
        // SH-FW review (one Opus review of the hardening): C-1 tar --checkpoint-action
        "tar -cf b.tar --checkpoint=1 --checkpoint-action exec='npx playwright install' a",
        "tar -cf b.tar --checkpoint=1 --checkpoint-act=exec='npx playwright install' a",
        // C-2 git options, keys and ext:: URLs that run a command
        "git clone -u 'npx playwright install' . ../c",
        "git push --receive-pack='npx playwright install' . HEAD:refs/heads/z",
        "git archive --remote=. --exec='npx playwright install' HEAD",
        "git -c remote.o.url=. -c remote.o.uploadpack='npx playwright install' fetch o",
        "git -c difftool.z.cmd='npx playwright install' difftool --cached -y --tool=z",
        "git -c protocol.ext.allow=always ls-remote 'ext::npx playwright install'",
        // M-3 wrappers
        "gosu root npx playwright install",
        "micromamba run -n x npx playwright install",
        // I-1 fd clusters
        "fdfind -Hx npx playwright install",
        "fd -HIx npx playwright install",
        // C-5 subscripts
        "let 'a[$(npx playwright install)]=1'",
        "[[ 'a[$(npx playwright install)]' -eq 0 ]]",
        "printf -v 'a[$(npx playwright install)]' x",
        "test -v 'a[$(npx playwright install)]'",
        // C-3 brace expansion, C-4 decoded echo/printf text, I-2 a newline after a pipe
        "{npx,playwright,install}",
        "echo -e 'npx\\x20playwright install' | sh",
        "printf 'npx\\x20playwright install' | sh",
        "echo npx playwright install |\nsh",
        // M-4 everyday controls
        "eval \"$(fnm env)\"",
        "bash <(curl -fsSL https://example.com/install.sh)",
        "npx prettier --check src playwright test",
    ];
    let mut out: Vec<Value> = FW
        .iter()
        .map(|text| case(text.to_string(), "/home/tester"))
        .collect();
    // I-3: caps that drop input (the re-review lifted the fed and parallel ones, F-2)
    out.push(case(
        format!(
            "printf '{}npx playwright install\\n' | xargs -I{{}} sh -c '{{}}'",
            "true\\n".repeat(64)
        ),
        "/home/tester",
    ));
    out.push(case(
        format!(
            "parallel ::: {}'npx playwright install'",
            "true ".repeat(64)
        ),
        "/home/tester",
    ));
    out.push(case(
        format!(
            "timeout 5 mytool{} npx playwright install",
            " sh".repeat(70)
        ),
        "/home/tester",
    ));
    // SH-FW re-review: fw:40 on, in this order (`shell.analyze.toml` names the Rust-only ones)
    const REREVIEW: &[&str] = &[
        // C-2a, C-1a (Rust-only)
        "git -c alias.cl=clone cl -u 'npx playwright install' . ../c",
        "TAR_OPTIONS='--checkpoint=1 --checkpoint-action=exec=npx\\ playwright\\ install' tar -cf b.tar a",
        // C-3a, C-4a (the oracle applies them too)
        "{echo,'npx playwright install'} | sh",
        "{xargs,npx} <<< 'playwright install'",
        "printf -- 'npx\\x20playwright install' | sh",
        "printf 'npx %s install\\n' playwright | sh",
        "printf '%c%s' n 'px playwright install' | sh",
        // C-5a (the fail-closed families: the oracle applies them too)
        "printf -v'a[$(npx playwright install)]' x",
        "a=(1); unset 'a[$(npx playwright install)]'",
        // I-2a (the oracle applies it too), and `function f` bodies (Rust-only)
        "( echo npx playwright install ) | sh",
        "if true; then echo npx playwright install; fi | sh",
        "f() { echo npx playwright install; }; f | sh",
        "function f { npx playwright install; }; f",
        // F-1 controls: quoted braces are not brace expressions, 70-word lists are fine
        "awk '/a/{print $1,$2,$3,$4} /b/{print $1,$2,$3,$4} /c/{print $1,$2,$3,$4,$5}' log.txt",
        "mkdir -p app/{models,views,controllers,helpers,mailers,jobs,channels}/{admin,api,web,shared,internal,public,legacy,v2,v3,v4}",
        "rm -f build/{a,b,c,d,e,f,g,h}.{js,map,d.ts,css,html,json,txt,md,yml}",
        "echo '{a,b}' \"{c,d}\" \\{e,f}",
    ];
    out.extend(
        REREVIEW
            .iter()
            .map(|text| case(text.to_string(), "/home/tester")),
    );
    // F-2 controls: more than 64 inputs or lines are read, not too complex (Rust-only past the 64th)
    let hosts: String = (0..70).map(|k| format!(" host{k}")).collect();
    out.push(case(
        format!("parallel ssh {{}} uptime :::{hosts}"),
        "/home/tester",
    ));
    let lines: String = (0..70).map(|k| format!("echo {k}\n")).collect();
    out.push(case(
        format!("xargs -n1 echo <<'EOF'\n{lines}EOF"),
        "/home/tester",
    ));
    // SH-seq review C-1, I-1, M-2 (the oracle applies them too)
    const SEQ: &[&str] = &[
        "npm --prefix \"{1..2}\" {-1..-1} i puppeteer",
        "npm --prefix '{1..2}' i {puppeteer,x}",
        "npm --prefix {a\",\"b} i {puppeteer,x}",
        "npm --prefix {1\"..\"2} i {puppeteer,x}",
        "npm {-1..{1,}} i puppeteer",
        "npm {-1..-1} i puppeteer",
        "npm --prefix ${HOME:+x} {-1..-1} i puppeteer",
        "echo {0..20..5} {-2..1}",
        "ls ${HOME}/{a,b}",
    ];
    out.extend(
        SEQ.iter()
            .map(|text| case(text.to_string(), "/home/tester")),
    );
    // paramsub: `$(...)` in `${...}`; a backtick holding `)` and a `case` inside `$(` (the oracle applies them)
    const PARAMSUB: &[&str] = &[
        "echo ${x:-$(npx playwright install chromium)}",
        "echo \"${x:-'$(npx playwright install)'}\"",
        "echo $(( ${x:-$(npx playwright install)} + 0 ))",
        "echo ${x:-$(echo `echo )`; npx playwright install)}",
        "echo $(case a in a) npx playwright install;; esac)",
        "echo \"$(case a in (a) echo hi;; esac)\"",
        "echo $(echo case)",
    ];
    out.extend(
        PARAMSUB
            .iter()
            .map(|text| case(text.to_string(), "/home/tester")),
    );
    // SH-seq review I-2: the stdin-shell family, named shells' and launchers' (the oracle applies it too)
    const STDIN_SHELL: &[&str] = &[
        r#"echo npx playwright install | sh -c "$(cat)""#,
        r#"echo npx playwright install | bash -c "$(cat -)""#,
        r#"echo npx playwright install | eval "$(cat)""#,
        r#"echo npx playwright install | sh -c "$(</dev/stdin)""#,
        r#"echo npx playwright install | sh -c "$(cat /dev/stdin)""#,
        "echo npx playwright install | sh -c \"`cat`\"",
        r#"echo npx playwright install | sh -c '$(cat)'"#,
        r#"echo npx playwright install | eval '$(cat)'"#,
        "echo npx playwright install | sudo -s",
        "echo npx playwright install | sudo -i",
        "echo npx playwright install | su",
        "echo npx playwright install | su root",
        "echo npx playwright install | script -q /dev/null",
        "echo npx playwright install | sg root",
        "echo npx playwright install | newgrp root",
        r#"curl x | sh -c "$(cat)""#,
        r#"curl x | eval "$(cat)""#,
        "curl x | sudo -s",
        "sudo -s",
        "bash",
        r#"sh -c "$(cat)""#,
        r#"eval "$(ssh-agent -s)""#,
        r#"echo hi | sh -c "$(date)""#,
        "echo hi | sudo -s",
        "cat x | sudo tee f",
        "echo npx playwright install | sudo -s -- ls",
        "echo npx playwright install | sg docker -c 'cat'",
        "echo npx playwright install | script -c 'cat'",
        "echo npx playwright install | su root ls",
        "echo npx playwright install | make -f -",
        "echo npx playwright install | sed e",
    ];
    out.extend(
        STDIN_SHELL
            .iter()
            .map(|text| case(text.to_string(), "/home/tester")),
    );
    // SH-FW final review I-3: the shipped reading of a package manager's unknown options (an alternate)
    const I3: &[&str] = &[
        "pnpm --silent open -a Chromium https://x",
        "yarn --silent open -a Chromium https://x",
        "sudo pnpm --silent open -a Chromium https://x",
        "npm --silent open exec playwright install",
        "pnpm --silent install",
    ];
    out.extend(I3.iter().map(|text| case(text.to_string(), "/home/tester")));
    // SH-R2 (R-2): a process substitution is one word of its command, and its body a command of its own
    const R2: &[&str] = &[
        "npm --userconfig <(echo) i puppeteer",
        "npm --userconfig >(cat) i puppeteer",
        "npm --userconfig <( echo ) install puppeteer",
        "npm --userconfig x<(echo) i puppeteer",
        "npm --userconfig <(cat <(echo)) i puppeteer",
        "npm --userconfig ${x:-<(echo)} i puppeteer",
        "npx --yes <(true) playwright install",
        "npx --yes >(cat) playwright install",
        "(npm --userconfig <(echo) i puppeteer)",
        "echo a >(touch /tmp/m) b",
        "f <(true)b c",
        "f a=<(touch /tmp/m) z",
        "f 2<(true) z",
        "f > >(touch /tmp/m) z",
        "f <(true)<f",
        "cat <(touch /tmp/m)",
        r#"f "<(touch /tmp/m)" z"#,
        "f ${x:-<(touch /tmp/m)} z",
        r#"f "${x:-<(touch /tmp/m)}" z"#,
        "sh < <(echo npx playwright install)",
        "cat <(echo npx playwright install) | sh",
        "diff <(sort a) <(sort b)",
        "comm -12 <(ls a) <(ls b)",
        "tee >(gzip > f.gz) < x",
        "while read l; do :; done < <(ls)",
    ];
    out.extend(R2.iter().map(|text| case(text.to_string(), "/home/tester")));
    // #89 review F-1, F-2, F-4 (the oracle applies them too)
    const F124: &[&str] = &[
        "npm --userconfig <(echo) i puppeteer\necho \"x",
        "npm --userconfig >(cat) i puppeteer\n`",
        "echo npx playwright install | sudo -s\necho \"x",
        "echo npx playwright install | sh\necho \"x",
        "curl -s x | sh\necho \"x",
        "echo \"$(case\\\n a in a) npm i puppeteer;; esac)\"",
        "echo $(case\\\n a in a) npx playwright install;; esac)",
        "echo ${x:-$(case\\\n a in a) npx playwright install;; esac)}",
        "npm --userconfig <(case\\\n x in x) echo;; esac) i puppeteer",
        "npm --userconfig <(cas\\\ne x in x) echo;; esac) i puppeteer",
        "npm --prefix ${x:-\"}\"{a,b}} {-x,} i puppeteer",
        "npm --prefix ${x:-\\}{a,b}} {-x,} i puppeteer",
        "npm --prefix ${x:-'}'{a,b}} {-x,} i puppeteer",
    ];
    out.extend(
        F124.iter()
            .map(|text| case(text.to_string(), "/home/tester")),
    );
    // SH-F3: the stdin-reader variants (runtime substitutions, launchers as consumers, readers nested in
    // a script or a compound body, missing launchers); the payloads of sh_fw_verify_payloads.txt
    const F3: &[&str] = &[
        r#"echo npx playwright install | sh -c "$(cat) ""#,
        r#"echo npx playwright install | sh -c "$(cat);""#,
        r#"echo npx playwright install | eval " $(cat)""#,
        r#"echo npx playwright install | eval "$(cat);""#,
        r#"echo npx playwright install | sh -c "`cat` ""#,
        r#"echo npx playwright install > >(su)"#,
        r#"echo npx playwright install | tee >(sudo -s)"#,
        r#"su < <(echo npx playwright install)"#,
        r#"sudo -s < <(echo npx playwright install)"#,
        r#"echo npx playwright install > >(eval "$(cat)")"#,
        r#"echo npx playwright install | bash -c sh"#,
        r#"echo npx playwright install | sh -c 'eval "$(cat)"'"#,
        r#"echo npx playwright install | su -c sh"#,
        r#"echo npx playwright install | script -qc sh /dev/null"#,
        r#"echo npx playwright install | if true; then sh; fi"#,
        r#"echo npx playwright install | doas -s"#,
        r#"echo npx playwright install | runuser -l root"#,
        r#"echo npx playwright install | su root -s /bin/bash"#,
        r#"echo npx playwright install | chroot /"#,
        r#"echo npx playwright install | unshare"#,
        r#"echo npx playwright install | nsenter -t 1"#,
        r#"echo npx playwright install | toybox sh"#,
        r#"echo npx playwright install | posh"#,
        r#"echo npx playwright install | rbash"#,
        r#"echo npx playwright install | unshare -r"#,
        r#"echo npx playwright install | sh -c "$(cat) " > /dev/null"#,
        r#"echo npx playwright install | find . -maxdepth 0 -exec sh \;"#,
        r#"echo npx playwright install | find . -maxdepth 0 -exec sh -c "$(cat) " \;"#,
        r#"echo npx playwright install | sh -c "$(cat) " 2>/dev/null"#,
    ];
    out.extend(F3.iter().map(|text| case(text.to_string(), "/home/tester")));
    // SH-FW final F-8: a program word that is a default or alternate, or a substitution (payloads), and the
    // everyday forms that must stay allowed (controls)
    const F8: &[&str] = &[
        "${x:-npx} playwright install",
        "${x-npx} playwright install",
        "x=1; ${x:+npx} playwright install",
        "${x:=npm} i puppeteer",
        "$(echo npx playwright install)",
        "`echo npx` playwright install",
        r#""$(echo npx)" playwright install"#,
        "sudo $(echo npx) playwright install",
        "$(which npx) playwright install",
        "npm ${x:-i} puppeteer",
        "$(which python3) -m venv .venv",
        r#""$(command -v node)" --version"#,
        "${EDITOR:-vi} file.txt",
        "${PYTHON:-python3} script.py",
        "echo $(date)",
        "x=$(git rev-parse HEAD)",
    ];
    out.extend(F8.iter().map(|text| case(text.to_string(), "/home/tester")));
    out
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
