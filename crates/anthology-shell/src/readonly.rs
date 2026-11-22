//! `classify_readonly` (P3): proves that a Bash command is read-only, on the shared parser, and fails closed.
//!
//! An allow is the one answer that skips the person, so this module proves and never guesses: the result is
//! [`ReadOnly::Proven`] only for a command made of the shapes below, and [`ReadOnly::NotProven`] for
//! everything else, whatever the reason. A command the module does not know is not read-only.
//!
//! What is proven, in full:
//!
//! - One line of at most 4096 bytes: no newline or other control character, no unquoted non-ASCII.
//! - Simple commands joined by `&&` or `|` only. `;`, `||`, `&`, `|&`, a subshell, a group, `!`, a comment,
//!   a backslash and every compound command are refused.
//! - Words made of letters, digits, `-_./:,+%@=` and quoted text. Outside single quotes there is no `$`,
//!   backquote, backslash or `!`; outside quotes there is no `~`, `^`, `#`, brace, parenthesis, `;`, `<(`,
//!   `>(`, nor `<` that opens a zsh numeric glob (`<1-5>`, `<->`). No word opens with `=` once its quotes
//!   are removed (`=ls`, `''=ls`, `'='ls`: zsh turns it into the path of that command), and no `=` follows
//!   `=` or `:` (MAGIC_EQUAL_SUBST). A word that is, or holds, a `..` component is refused, and so is
//!   a glob that can match `..` (`.*`, `.?`, `.[x]`, `..*`, `[.]*`). A glob whose expansion could start with
//!   `-` (`*.rs`) stands only after `--` or, as a path operand, when it opens with a literal character other
//!   than `-`: a file called `--output=x` must not turn into an option. A glob is never an option value, a
//!   pattern or any other non-path word.
//! - Redirects: `2>&1` and `>&2` (a duplicate of fd 1 or 2), `>/dev/null`, `2>/dev/null`, `&>/dev/null`,
//!   and `< PATH` with a literal path. Every other redirect, any fd form (`>|`, `<>`, `3>f`, `>&f`, `<&N`),
//!   heredoc and here-string is refused.
//! - Every program is in [`READONLY_PROGRAMS`], spelled bare (`ls`, never `/bin/ls`), with only the options
//!   its table row lists. An option the row does not list is refused, so are an abbreviated long option and
//!   an option the row lists as a flag when it is given a value. Options come first: a word that starts
//!   with `-` after the first operand and before any `--` (and `--` itself after an operand) is refused for
//!   every program, because GNU permutes the options and BSD (the macOS tools) reads the word as a file
//!   (`uniq payload.txt -c` writes the file `-c`). `find` is exempt: its expression is its own grammar and
//!   every word of it must be in the table. `git` takes only the subcommands `status
//!   log diff show blame ls-files rev-parse describe branch remote`, `branch` and `remote` with list flags
//!   only, and no global option but `--no-pager` and `--no-optional-locks` (no `-c`, `-C`, `--git-dir`,
//!   `--work-tree`, `--exec-path`, `--config-env`), no option that writes (`--output`) or runs a program
//!   (`--ext-diff`, `--textconv`), no `describe --dirty` or `--broken` (they refresh the index), and no
//!   word with a `:` in it (`HEAD:file` reads a blob; `:(top)` is pathspec magic). A `git` line is marked
//!   `runs_git_config` (see [`ReadOnly::Proven`]). `find` takes no `-exec`, `-execdir`, `-ok`, `-okdir`,
//!   `-delete`, `-fprint`, `-fprint0`, `-fprintf`, `-fls`, `-L`, `-H` or `-follow`. `sort` takes no `-o`,
//!   `--output`, `--compress-program` or `-T`; `tail` no `-f`, `-F` or `--follow`; `uniq` one operand at
//!   most; `rg` no `--pre` or `-z`; `grep` no `-R`, no `-NUM` and a `-d` or `-D` action of exactly `read`,
//!   `skip` or (for `-d`) `recurse`; `diff` no `-r`; `file` no `-p`; `jq` no `-f`, `--arg` or `-L`, and no
//!   filter that names `env`, `$ENV`, `import` or `include`. No `git` word holds `%G` (a signature
//!   placeholder: it runs gpg).
//! - A leading `cd TARGET` (or several) joined by `&&`, with a target [`literal_cd_target`] accepts. `cd`
//!   anywhere else, `cd`, `cd -`, `cd ~…`, `cd $X`, `cd ..` and a bare `cd src` (which `CDPATH` may point
//!   elsewhere) are refused. Every path of the rest is relative to the working directory the `cd` targets
//!   lead to, each target relative to the one before it.
//! - No path of an obvious secret ([`SECRET_PATTERNS`]: `.ssh`, `.env`, `*.pem`, `id_*`, `.aws/credentials`
//!   and so on, in any ASCII case), alone or under the `cd` targets, whatever the command; a recursive path
//!   is refused too when it is a directory such a secret lies in (`.aws`). This is defence in depth: the
//!   path check owns the decision.
//!
//! The programs that run are not only the GNU and BSD ones: the shell snapshot the commands run under puts
//! `bfs` in place of `find` and `ugrep` in place of `grep`, and `rg` may be its own build, on a macOS
//! userland under zsh. A table row lists an option only when it means the same on all of them, and a test pins the options
//! of those programs that run, write or read another file as refused.
//! - No environment assignment before a command (`GIT_PAGER=x git log`, `BASH_ENV=x ls`): it makes the
//!   first word `NAME=value`, which is no table program.
//!
//! The words come from two readers. This module scans the text itself, with a scanner small enough to be
//! sure of, and [`crate::analyze`] reads the same line. The result is `Proven` only when `analyze` finds the
//! same simple commands, the same words and the same redirects, with nothing `opaque`, no `alternates` and
//! no `incomplete`. Two readers that differ on a line are a reason to refuse it (design G-2).
//!
//! `Proven.paths` lists every path the command can read, as written (relative paths are relative to the
//! working directory the `cd` targets lead to, folded in order, or to the working directory with no `cd`),
//! each with whether the command reads everything under it. A path may hold glob characters, may not exist,
//! and, for `git`, may be a revision: git decides, so each operand is reported as a possible path. A command
//! with an implicit path (`ls`, `rg`, `grep PATTERN`, `find`, `du`, `tree`) reports `.`. `Proven.cd` lists the
//! `cd` targets in order. The path check (`anthology-guard`'s `readonly_paths`) decides whether the paths lie
//! under a root, match a secret or a deny rule; `classify_readonly` does not look at the file system.
//!
//! Nothing wires `Proven` to an allow yet. Until the path check (P4) takes `paths`, `recursive`, `cd` and
//! `runs_git_config`, a caller must not skip the person on a `Proven`.

use crate::analyze::analyze;

/// The programs a command may run and still be proven read-only. `git` and `find` are listed here and have
/// their own argument rules below; `cd` is no program of this table (a leading `cd` is read separately).
pub const READONLY_PROGRAMS: &[&str] = &[
    "ls", "cat", "head", "tail", "wc", "grep", "egrep", "fgrep", "rg", "find", "stat", "file",
    "du", "pwd", "which", "basename", "dirname", "realpath", "tree", "sort", "uniq", "cut", "tr",
    "diff", "jq", "git",
];

/// The longest command that is read.
const MAX_LEN: usize = 4096;

/// Names that must never be read on a proof. A path with a component that matches one of these (after any
/// `cd` is folded in) is not proven, whatever the command: this is defence in depth under the path check
/// (P4), which owns the real decision, including the user's `Read()` deny rules, symlinks and containment.
///
/// A pattern is one or more `/`-separated components, in lower case. `*` stands for any run of characters
/// inside a component. A pattern matches when its components equal consecutive components of the path,
/// anywhere in it and whatever the ASCII case (the default macOS volume ignores it), so `.ssh` matches
/// `/srv/app/.ssh`, `a/.SSH/id_rsa` and `.ssh`, and `.aws/credentials` matches `/srv/app/.AWS/credentials`.
///
/// A glob in the path (`.en*`, `*.pem`) is checked against the names these patterns stand for
/// ([`GLOB_SAMPLES`] and each pattern with its `*` read as a character no literal equals). The check cannot
/// see the user's `dotglob` or `GLOB_DOTS`, so a leading wildcard may match a leading `.` (`*env` is `.env`).
/// A glob alone in its component needs two literal characters (`*`, `?`, `i*` are not held, or no `cat ./*`
/// could be proven); in a path with a plain component beside it, as in `.kube/*`, it needs none.
///
/// The path check (P4) must treat a recursive path as able to reach any file under it, a secret included:
/// [`is_secret_tree`] refuses the directories these patterns lie in, but a root above them (`/srv/app` over
/// `/srv/app/.ssh`) is for P4 to judge.
pub const SECRET_PATTERNS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws/credentials",
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "id_*",
    ".netrc",
    ".npmrc",
    ".pypirc",
    ".git-credentials",
    ".config/gh/hosts.yml",
    ".docker/config.json",
    ".kube/config",
    ".claude/.credentials.json",
    ".bash_history",
    ".zsh_history",
    ".histfile",
];

/// Names that a glob in a path is held against, besides the patterns themselves (`*_rsa` is `id_rsa`).
const GLOB_SAMPLES: &[&str] = &["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", ".env.local"];

/// A glob needs this many literal characters to be held against the names above: with fewer (`i*`) nearly
/// every name could match, and the check would refuse `ls i*` for a file that is not there.
const GLOB_MIN_LITERALS: usize = 2;

/// One path a proven command can read, as written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadPath {
    /// The path as the user typed it, quotes removed: relative to the working directory after the `cd`
    /// targets, which are folded in order (`cd ./a && cd ./b` leaves `./b` relative to `./a`). It may hold
    /// glob characters, may not exist, and, for `git`, may be a revision.
    pub path: String,
    /// The command reads everything under the path, not just the path: `rg`, `find`, `du`, `tree`,
    /// `grep -r`, `ls -R`, a `git` pathspec, the two directories of a `diff`, and any path with a `**` in
    /// it. A name match on the path alone (`.`) cannot see a secret below it, so the path check must treat
    /// the whole tree as read, and as able to reach a secret file by name.
    pub recursive: bool,
}

/// What [`classify_readonly`] proved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadOnly {
    /// Read-only: every program is one of [`READONLY_PROGRAMS`], with only the options of its table, and
    /// no file a person could name is written. It is not a claim that nothing else runs or changes: `git`
    /// runs the programs its configuration names (see `runs_git_config`), `git status` refreshes the stat
    /// cache in `.git/index` unless `--no-optional-locks` is given, and a program may update access times.
    ///
    /// `paths` are the paths it can read, in order, and `cd` the leading `cd` targets, in order. A path
    /// marked `recursive` stands for every file under it: the path check must treat it as able to reach a
    /// secret file by name, whatever its own name says.
    ///
    /// `runs_git_config` is true when any command of the line is `git`. Git then reads the whole repository
    /// at the working directory (history, index, work tree, submodules), whatever `paths` says, and it runs
    /// the programs that the repository's and the user's configuration names. The caller must not allow the
    /// line on the strength of `paths` alone.
    ///
    /// The path check (P4) must decide this with an ALLOWLIST: it allows the line only when every
    /// configuration file git can reach holds nothing but keys on a short list of safe repository-local ones
    /// (such as `core.repositoryFormatVersion`, `core.bare`, `core.filemode`, `core.ignorecase`,
    /// `core.logAllRefUpdates`, `user.name`, `user.email`, `remote.<name>.url`, `remote.<name>.fetch` and
    /// `branch.<name>.remote` / `.merge`), and asks on any other key, section or include. A denylist keeps
    /// missing vectors. These are known misses of the first denylist, as examples of what an allowlist
    /// catches:
    ///
    /// - a partial clone: `remote.<name>.promisor`, `remote.<name>.partialclonefilter` and `extensions.*`
    ///   make plain `git log -p`, `show` and `diff` fetch missing blobs on demand, and the fetch runs
    ///   `remote.<name>.uploadpack` (or `receivepack`), and can reach `credential.helper`,
    ///   `core.sshCommand`, `core.gitProxy` and `protocol.*`;
    /// - `core.pager`, `pager.*`, `core.fsmonitor`, `core.hooksPath`, `core.askPass`, `core.editor`,
    ///   `core.attributesFile`, `core.excludesFile`;
    /// - `diff.external`, `diff.<driver>.command`, `diff.<driver>.textconv`, `diff.<driver>.cachetextconv`
    ///   (a `.gitattributes` file in the work tree selects the driver), and the same for `merge.*.driver`;
    /// - `gpg.program`, `gpg.<format>.program`, with `log.showSignature` or `gpg.*` set;
    /// - `filter.<name>.smudge`, `filter.<name>.clean` and `filter.<name>.process`;
    /// - `submodule.*` (`submodule.recurse`, `submodule.<name>.update`), and `status.submoduleSummary`;
    /// - `alias.*` and `include.path` / `includeIf.*`.
    ///
    /// The configuration to read is that of the repository git finds the way git does: from the working
    /// directory after the `cd` targets, up through the parents (`GIT_DIR`, `GIT_CEILING_DIRECTORIES` and
    /// `safe.directory` apply), a bare repository reached by a `cd` included, a `.git` file that points
    /// elsewhere, a worktree's own configuration, plus the system, global and `GIT_CONFIG_*` sources and
    /// the submodules' repositories. `git` with `%G` in a format is not proven (it runs gpg).
    Proven {
        paths: Vec<ReadPath>,
        cd: Vec<String>,
        runs_git_config: bool,
    },
    /// Not proven: the reason is for a log line, never for a decision.
    NotProven(&'static str),
}

type Why = &'static str;

/// The characters of one path component, tokenised for a glob match: `*` is any run, `?` and a bracket
/// class (read as any one character: it can only widen the match) are one character.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Tok {
    Star,
    One,
    Lit(char),
}

fn tokens(component: &str) -> Vec<Tok> {
    let chars: Vec<char> = component.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => out.push(Tok::Star),
            '?' => out.push(Tok::One),
            '[' => match chars[i + 1..].iter().position(|&c| c == ']') {
                // the first `]` closes the class; read as one character it can only widen the match
                Some(end) => {
                    out.push(Tok::One);
                    i += 1 + end;
                }
                None => out.push(Tok::Lit('[')),
            },
            c => out.push(Tok::Lit(c)),
        }
        i += 1;
    }
    out
}

/// Whether the glob `pattern` matches the whole of `text`. A wildcard at the front matches a leading `.`,
/// as it does with `dotglob` (bash) or `GLOB_DOTS` (zsh): the check cannot see the user's options, so it
/// assumes the wider reading. Linear in the product of the lengths: one backtrack point.
fn glob_matches(pattern: &[Tok], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(Tok::Star) => {
                backtrack = Some((p, t));
                p += 1;
            }
            Some(Tok::One) => {
                p += 1;
                t += 1;
            }
            Some(Tok::Lit(c)) if *c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match backtrack {
                Some((star, at)) => {
                    backtrack = Some((star, at + 1));
                    p = star + 1;
                    t = at + 1;
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|tok| *tok == Tok::Star)
}

fn has_glob_char(component: &str) -> bool {
    component.contains(['*', '?', '['])
}

/// Whether one path component (already in lower case) is the one a secret pattern's component names. A
/// glob needs `min_literals` literal characters to be held against the names the pattern stands for.
fn component_is_secret(component: &str, pattern: &str, min_literals: usize) -> bool {
    if !has_glob_char(component) {
        let name: Vec<char> = component.chars().collect();
        return glob_matches(&tokens(pattern), &name);
    }
    // a glob in the path: it names a secret when it can expand to a name the pattern stands for
    let glob = tokens(component);
    if glob.iter().filter(|tok| matches!(tok, Tok::Lit(_))).count() < min_literals {
        return false;
    }
    // the pattern's own `*` becomes a character no literal of a glob equals
    let sample: Vec<char> = pattern.replace('*', "\u{1}").chars().collect();
    glob_matches(&glob, &sample)
        || GLOB_SAMPLES.iter().any(|name| {
            let name: Vec<char> = name.chars().collect();
            glob_matches(&tokens(pattern), &name) && glob_matches(&glob, &name)
        })
}

/// Whether `components` (lower case) hold a secret pattern. With `tree`, also when they end in the leading
/// components of a pattern of several: a recursive read of `.aws` reaches `.aws/credentials`.
fn holds_secret(components: &[String], tree: bool) -> bool {
    // a window of one component needs literals in a glob; a window of several needs a plain component, so
    // `./*/*` is no match for `.aws/credentials` but `.aws/*` is
    let window_matches = |have: &[String], want: &[&str]| {
        let min_literals = if want.len() == 1 {
            GLOB_MIN_LITERALS
        } else {
            0
        };
        have.iter()
            .zip(want)
            .all(|(have, want)| component_is_secret(have, want, min_literals))
            && (want.len() == 1 || have.iter().any(|c| !has_glob_char(c)))
    };
    SECRET_PATTERNS.iter().any(|pattern| {
        let want: Vec<&str> = pattern.split('/').collect();
        components
            .windows(want.len())
            .any(|window| window_matches(window, &want))
            || tree
                && (1..want.len()).any(|k| {
                    components.len() >= k
                        && window_matches(&components[components.len() - k..], &want[..k])
                })
    })
}

/// The path's components in ASCII lower case, without `.` and empty ones: `./a//.ENV` is `a/.env`. The
/// default macOS volume ignores case, so `.ENV` is `.env`.
fn lower_components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Whether `path` names an obvious secret (see [`SECRET_PATTERNS`]), ignoring ASCII case. `.` and empty
/// components are skipped, so `./a//.env` is read as `a/.env`.
pub fn is_secret_path(path: &str) -> bool {
    holds_secret(&lower_components(path), false)
}

/// Whether a read of everything under `path` reaches an obvious secret: [`is_secret_path`], or `path` is a
/// directory a secret pattern lies in (`.aws`, `.config/gh`).
pub fn is_secret_tree(path: &str) -> bool {
    holds_secret(&lower_components(path), true)
}

/// `word` when it is a `cd` target that is safe to follow without the shell: an absolute path or a `./…`
/// path, from letters, digits and `/._+,:@%=-`, with no `..` component, at most 4096 bytes.
///
/// A target that is not rooted (`cd src`) can be looked up through `CDPATH`, which a hook cannot see (the
/// user's shell snapshot may set it); `cd -`, `cd ~…`, `cd $X`, a quote, space, glob or brace character and
/// any non-ASCII byte are not literal either.
pub fn literal_cd_target(word: &str) -> Option<&str> {
    if word.len() > MAX_LEN || !(word.starts_with('/') || word.starts_with("./")) {
        return None;
    }
    let literal = word
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"/._+,:@%=-".contains(&b));
    (literal && !has_parent_component(word)).then_some(word)
}

/// The count of the arguments `-n N`, `-nN` or `-N` of `head` or `tail`, and nothing else: no file, no other
/// option, no sign, no `+`, no unit, and only ASCII digits (the count is not expanded: the caller has the
/// words after the shell's quote removal).
pub fn head_tail_count(args: &[String]) -> Option<u64> {
    let count = |text: &str| {
        let digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        digits.then(|| text.parse::<u64>().ok()).flatten()
    };
    match args {
        [flag, n] if flag == "-n" => count(n),
        [one] => count(one.strip_prefix("-n").or_else(|| one.strip_prefix('-'))?),
        _ => None,
    }
}

/// Prove `command` read-only, or say it is not proven. `home` is the home directory `analyze` expands `~`
/// with (no `~` is ever proven, so it only has to be the one the caller's `analyze` would use).
pub fn classify_readonly(command: &str, home: &str) -> ReadOnly {
    match prove(command, home) {
        Ok(proof) => ReadOnly::Proven {
            paths: proof.paths,
            cd: proof.cd,
            runs_git_config: proof.runs_git_config,
        },
        Err(why) => ReadOnly::NotProven(why),
    }
}

/// What `prove` collected.
#[derive(Debug, Eq, PartialEq)]
struct Proof {
    paths: Vec<ReadPath>,
    cd: Vec<String>,
    runs_git_config: bool,
}

fn prove(command: &str, home: &str) -> Result<Proof, Why> {
    prove_with(command, home, scan)
}

/// `prove`, with the scanner given: a test hands it a scanner that is wrong, to show that the cross-check
/// with `analyze` is what refuses the line.
fn prove_with(
    command: &str,
    home: &str,
    scanner: fn(&str) -> Result<Vec<Stage>, Why>,
) -> Result<Proof, Why> {
    if command.len() > MAX_LEN {
        return Err("command too long");
    }
    if command
        .chars()
        .any(|c| c.is_control() && c != '\t' || c == '\u{7f}')
    {
        return Err("control character");
    }
    let stages = scanner(command)?;
    agrees_with_analyze(command, home, &stages)?;
    let mut paths = Vec::new();
    let mut cd = Vec::new();
    let mut runs_git_config = false;
    let mut leading = true;
    for (index, stage) in stages.iter().enumerate() {
        let program = stage.args[0].text.as_str();
        runs_git_config |= program == "git";
        if program == "cd" {
            if !leading {
                return Err("cd after a command");
            }
            let [_, target] = stage.args.as_slice() else {
                return Err("cd with other than one target");
            };
            if !stage.redirs.is_empty() {
                return Err("cd with a redirect");
            }
            let target =
                literal_cd_target(&target.text).ok_or("cd target is not a literal path")?;
            if target.contains(['*', '?', '['])
                || stages.get(index + 1).map(|s| s.join) != Some(Some(Join::And))
            {
                return Err("cd must lead an && chain");
            }
            cd.push(target.to_string());
            continue;
        }
        leading = false;
        redirects(stage, &mut paths)?;
        command_paths(program, &stage.args[1..], &mut paths)?;
    }
    // The paths are read after the `cd` targets, which are folded in order, so a secret is judged on the
    // joined path as well (`cd ./.aws && cat credentials`); an absolute path only adds components.
    // A recursive path also reaches what lies under it, so the directory a secret lies in counts.
    let prefix = cd.join("/");
    let judged = cd
        .iter()
        .map(|target| (target.as_str(), false))
        .chain(paths.iter().map(|p| (p.path.as_str(), p.recursive)));
    for (path, recursive) in judged {
        let secret = |path: &str| {
            if recursive {
                is_secret_tree(path)
            } else {
                is_secret_path(path)
            }
        };
        if secret(path) || secret(&format!("{prefix}/{path}")) {
            return Err("path of a secret");
        }
    }
    Ok(Proof {
        paths,
        cd,
        runs_git_config,
    })
}

// --- the scanner ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Join {
    And,
    Pipe,
}

/// One word, quotes removed.
#[derive(Clone, Debug)]
struct Arg {
    text: String,
    /// It holds an unquoted `*`, `?` or `[`: the shell expands it.
    globby: bool,
    /// Its expansion may start with `-` (it does, or its first character is a glob character).
    risky: bool,
}

/// One simple command: its words (the program first) and its redirects, with how it is joined to the one
/// before it.
#[derive(Debug)]
struct Stage {
    join: Option<Join>,
    args: Vec<Arg>,
    redirs: Vec<(&'static str, String)>,
}

impl Stage {
    fn new(join: Option<Join>) -> Stage {
        Stage {
            join,
            args: Vec::new(),
            redirs: Vec::new(),
        }
    }
}

/// A word being built: its text, and for each character whether it is an unquoted glob character.
#[derive(Default)]
struct Building {
    text: String,
    mask: Vec<bool>,
    quoted: bool,
}

impl Building {
    fn push(&mut self, c: char, glob: bool) {
        self.text.push(c);
        self.mask.push(glob);
    }
}

struct Scanner {
    stages: Vec<Stage>,
    current: Stage,
    word: Option<Building>,
    /// A redirect operator waiting for its target word.
    pending: Option<&'static str>,
}

impl Scanner {
    /// A word is complete: check it, then it is a redirect target or an argument.
    fn end_word(&mut self) -> Result<(), Why> {
        let Some(word) = self.word.take() else {
            return Ok(());
        };
        check_word(&word)?;
        let globby = word.mask.iter().any(|&g| g);
        let risky = globby && (word.text.starts_with('-') || word.mask.first() == Some(&true));
        match self.pending.take() {
            Some(op) => {
                if globby {
                    return Err("glob in a redirect target");
                }
                self.current.redirs.push((op, word.text));
            }
            None => self.current.args.push(Arg {
                text: word.text,
                globby,
                risky,
            }),
        }
        Ok(())
    }

    /// An operator ends the command: it needs words, and no redirect may be left without its target.
    fn end_stage(&mut self, next: Option<Join>) -> Result<(), Why> {
        self.end_word()?;
        if self.pending.is_some() {
            return Err("redirect without a target");
        }
        if self.current.args.is_empty() {
            return Err("empty command");
        }
        let done = std::mem::replace(&mut self.current, Stage::new(next));
        self.stages.push(done);
        Ok(())
    }

    /// A redirect operator: any word before it ends. A word of digits (unquoted, glued) is the fd and goes.
    fn redirect(&mut self, op: &'static str, fd_allowed: bool) -> Result<(), Why> {
        if fd_allowed
            && self.word.as_ref().is_some_and(|w| {
                !w.quoted && !w.text.is_empty() && w.text.bytes().all(|b| b.is_ascii_digit())
            })
        {
            self.word = None;
        } else {
            self.end_word()?;
        }
        if self.pending.is_some() {
            return Err("two redirect operators in a row");
        }
        self.pending = Some(op);
        Ok(())
    }
}

/// The stages of `command`, or why the scanner refuses it. Quoting, for this scanner, is only `'…'` and
/// `"…"`: nothing is escaped, and every character the shell could read in a quote it does not read as
/// literal is refused.
fn scan(command: &str) -> Result<Vec<Stage>, Why> {
    let chars: Vec<char> = command.chars().collect();
    let mut s = Scanner {
        stages: Vec::new(),
        current: Stage::new(None),
        word: None,
        pending: None,
    };
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if q == '"' && matches!(c, '$' | '`' | '\\' | '!') {
                return Err("expansion inside double quotes");
            } else if let Some(word) = s.word.as_mut() {
                word.push(c, false);
            }
            i += 1;
            continue;
        }
        match c {
            ' ' | '\t' => s.end_word()?,
            '\'' | '"' => {
                s.word.get_or_insert_with(Building::default).quoted = true;
                quote = Some(c);
            }
            '&' => match next {
                Some('&') => {
                    s.end_stage(Some(Join::And))?;
                    i += 1;
                }
                Some('>') => {
                    if chars.get(i + 2) == Some(&'>') {
                        return Err("append redirect");
                    }
                    s.redirect("&>", false)?;
                    i += 1;
                }
                _ => return Err("background job"),
            },
            '|' => match next {
                Some('|' | '&') => return Err("|| or |&"),
                _ => s.end_stage(Some(Join::Pipe))?,
            },
            '>' => match next {
                Some('>' | '|' | '(') => return Err("append, clobber or process substitution"),
                Some('&') => {
                    s.redirect(">&", true)?;
                    i += 1;
                }
                _ => s.redirect(">", true)?,
            },
            '<' if numeric_glob(&chars, i) => return Err("numeric glob"),
            '<' => match next {
                Some('<' | '&' | '>' | '(') => {
                    return Err("here-document, fd, read-write or process substitution");
                }
                _ => s.redirect("<", true)?,
            },
            ';' | '(' | ')' | '{' | '}' | '`' | '$' | '\\' | '!' | '~' | '^' | '#' => {
                return Err("shell metacharacter");
            }
            // zsh expands a word that opens with `=` to the path of that command (EQUALS), also when the word
            // opened with an empty quote (`''=ls`), and, with MAGIC_EQUAL_SUBST, an `=` after `=` or `:`
            '=' if s
                .word
                .as_ref()
                .is_none_or(|w| w.text.is_empty() || w.text.ends_with(['=', ':'])) =>
            {
                return Err("word opens with =");
            }
            '*' | '?' | '[' => s.word.get_or_insert_with(Building::default).push(c, true),
            c if c.is_ascii_alphanumeric() || "-_./:,+%@=]".contains(c) => {
                s.word.get_or_insert_with(Building::default).push(c, false);
            }
            _ => return Err("character outside the plain set"),
        }
        i += 1;
    }
    if quote.is_some() {
        return Err("unterminated quote");
    }
    s.end_stage(None)?;
    Ok(s.stages)
}

/// zsh reads `<N-M>` (digits, a dash, digits, `>`; either number may be missing, so `<->` too) as one glob
/// word, not as a `<` redirect: `cat <1-5>/dev/null` reads the files `1` to `5`, and `<1-5>&2` is a word and
/// a background job. A `<` that starts that shape is refused, wherever it stands in the word.
fn numeric_glob(chars: &[char], at: usize) -> bool {
    let digits = |mut i: usize| {
        while chars.get(i).is_some_and(char::is_ascii_digit) {
            i += 1;
        }
        i
    };
    let dash = digits(at + 1);
    chars.get(dash) == Some(&'-') && chars.get(digits(dash + 1)) == Some(&'>')
}

fn has_parent_component(text: &str) -> bool {
    text.split('/').any(|component| component == "..")
}

/// A word that is, or reaches, a parent directory, or a glob that can match `..`, or that opens with `=`
/// once its quotes are removed.
fn check_word(word: &Building) -> Result<(), Why> {
    if word.text.starts_with('=') {
        return Err("word opens with =");
    }
    if has_parent_component(&word.text) {
        return Err("parent directory component");
    }
    // `--name=../x`: the value of a glued option is read as a path too
    if word.text.starts_with('-')
        && word
            .text
            .split_once('=')
            .is_some_and(|(_, value)| has_parent_component(value))
    {
        return Err("parent directory component");
    }
    let chars: Vec<char> = word.text.chars().collect();
    let mut start = 0;
    for component in word.text.split('/') {
        let len = component.chars().count();
        let (text, mask) = (&chars[start..start + len], &word.mask[start..start + len]);
        start += len + 1;
        let Some(first_glob) = mask.iter().position(|&g| g) else {
            continue;
        };
        // the literal characters before the first glob character: `.`, `..` or nothing can still match `..`
        let prefix: String = text[..first_glob].iter().collect();
        if prefix == "." || prefix == ".." {
            return Err("glob can match ..");
        }
        if prefix.is_empty()
            && text[first_glob] == '['
            && text[first_glob + 1..].iter().any(|c| ".-:[=".contains(*c))
        {
            return Err("bracket glob can match ..");
        }
    }
    Ok(())
}

// --- the shared parser must read the line the same way -------------------------------------------------------

fn agrees_with_analyze(command: &str, home: &str, stages: &[Stage]) -> Result<(), Why> {
    let analysis = analyze(command, home);
    if analysis.incomplete {
        return Err("analysis incomplete");
    }
    if !analysis.alternates.is_empty() {
        return Err("analysis has alternate readings");
    }
    if analysis.entries.len() != stages.len() {
        return Err("analyze reads other commands");
    }
    for (entry, stage) in analysis.entries.iter().zip(stages) {
        if !entry.opaque.is_empty() {
            return Err("analyze finds something opaque");
        }
        let words_agree = entry.raw.len() == stage.args.len()
            && entry
                .raw
                .iter()
                .zip(&stage.args)
                .all(|(raw, arg)| *raw == arg.text);
        let redirs_agree = entry.redirs.len() == stage.redirs.len()
            && entry
                .redirs
                .iter()
                .zip(&stage.redirs)
                .all(|(r, (op, target))| r.op == *op && r.target == *target);
        if !words_agree || !redirs_agree {
            return Err("analyze reads other words");
        }
    }
    Ok(())
}

// --- redirects -----------------------------------------------------------------------------------------------

/// A path the command reads, as written. `recursive`: everything under it. A `**` in the path is a
/// recursive glob (`src/**/x`, `**`, `***`): the shell walks the tree under it, whatever the program.
fn read_path(paths: &mut Vec<ReadPath>, path: &str, recursive: bool) {
    paths.push(ReadPath {
        path: path.to_string(),
        recursive: recursive || path.contains("**"),
    });
}

fn redirects(stage: &Stage, paths: &mut Vec<ReadPath>) -> Result<(), Why> {
    for (op, target) in &stage.redirs {
        match *op {
            ">&" if target == "1" || target == "2" => {}
            ">" | "&>" if target == "/dev/null" => {}
            "<" if !target.is_empty() && target != "-" => read_path(paths, target, false),
            _ => return Err("redirect that can write or reads a descriptor"),
        }
    }
    Ok(())
}

// --- programs and their options --------------------------------------------------------------------------------

/// How a program reads its operands.
#[derive(Clone, Copy)]
enum Operands {
    /// Every operand is a path; with none, `default_dot` says whether `.` is the path.
    Paths,
    /// grep and rg: the first operand is the pattern unless an option gave it; the rest are paths.
    Pattern,
    /// jq: the first operand is the filter, the rest are paths.
    Filter,
    /// Names or strings, never paths (`basename`, `dirname`, `which`).
    Names,
    /// `tr`: one or two character sets.
    Sets,
    /// `uniq`: at most one path (a second operand is the output file).
    Single,
    /// `diff`: at least two paths.
    Two,
    /// No operand at all (`pwd`, `git branch`, `git remote`).
    NoneAllowed,
}

/// One row of the program table. Every list is exact: an option not listed is not proven.
struct Spec {
    /// Short flags, no value; they may cluster (`-la`).
    short: &'static str,
    /// Short options with a value: the rest of the word, else the next word.
    value: &'static str,
    /// Short options whose value is glued, if any (`-uno`, `-U3`); they never take the next word.
    glued: &'static str,
    /// Of `value`, the options whose value is a path.
    path_short: &'static str,
    /// `-NUM`.
    digits: bool,
    /// `--flag`, exactly.
    long: &'static [&'static str],
    /// `--name` or `--name=value`, never the next word.
    long_eq: &'static [&'static str],
    /// `--name=value` or `--name value`.
    long_value: &'static [&'static str],
    /// Of `long_value` and `long_eq`, the options whose value is a path.
    path_long: &'static [&'static str],
    /// Options that give the pattern (`Operands::Pattern`).
    pattern_short: &'static str,
    pattern_long: &'static [&'static str],
    /// Flags after which there is no pattern operand (`rg --files`).
    no_pattern_long: &'static [&'static str],
    operands: Operands,
    /// With no path operand, `.` is the path.
    default_dot: bool,
    /// The program always walks the directories it is given (`rg`, `du`, `tree`).
    walks: bool,
    /// Flags that make it walk (`-r`, `-R`), and `--recursive`.
    recursive_short: &'static str,
    recursive_long: &'static [&'static str],
    /// `-d ACTION`, `--directories=ACTION`, `-D ACTION` and `--devices=ACTION` exist (`grep`): the action is
    /// exactly `read` or `skip`, and `recurse` for directories, which walks ([`grep_action`]).
    directories: bool,
}

const NONE: Spec = Spec {
    short: "",
    value: "",
    glued: "",
    path_short: "",
    digits: false,
    long: &[],
    long_eq: &[],
    long_value: &[],
    path_long: &[],
    pattern_short: "",
    pattern_long: &[],
    no_pattern_long: &[],
    operands: Operands::Paths,
    default_dot: false,
    walks: false,
    recursive_short: "",
    recursive_long: &[],
    directories: false,
};

const LS: Spec = Spec {
    short: "1ABCFGHPRSUXabcdfghiklmnopqrstuvx",
    recursive_short: "R",
    recursive_long: &["--recursive"],
    long: &[
        "--all",
        "--almost-all",
        "--classify",
        "--directory",
        "--human-readable",
        "--inode",
        "--reverse",
        "--recursive",
        "--size",
        "--numeric-uid-gid",
        "--group-directories-first",
        "--literal",
        "--no-group",
        "--si",
        "--file-type",
        "--hide-control-chars",
        "--show-control-chars",
        "--escape",
        "--kibibytes",
        "--author",
    ],
    long_eq: &["--color"],
    long_value: &[
        "--sort",
        "--time",
        "--time-style",
        "--format",
        "--block-size",
        "--quoting-style",
        "--indicator-style",
    ],
    default_dot: true,
    ..NONE
};

const CAT: Spec = Spec {
    short: "AbeEnstTuv",
    long: &[
        "--show-all",
        "--number-nonblank",
        "--show-ends",
        "--number",
        "--squeeze-blank",
        "--show-tabs",
        "--show-nonprinting",
    ],
    ..NONE
};

const HEAD: Spec = Spec {
    short: "qv",
    value: "nc",
    digits: true,
    long: &["--quiet", "--silent", "--verbose"],
    long_value: &["--lines", "--bytes"],
    ..NONE
};

const WC: Spec = Spec {
    short: "clmwL",
    long: &[
        "--bytes",
        "--chars",
        "--lines",
        "--words",
        "--max-line-length",
    ],
    ..NONE
};

const GREP: Spec = Spec {
    short: "EFGHILPTUabchilnoqrsuvwxy",
    value: "emABCdDf",
    path_short: "f",
    // no `-NUM`: in the grep that runs (ugrep) it is a recursion depth that turns on `-r`; GNU reads context
    long: &[
        "--extended-regexp",
        "--fixed-strings",
        "--basic-regexp",
        "--perl-regexp",
        "--ignore-case",
        "--no-ignore-case",
        "--word-regexp",
        "--line-regexp",
        "--invert-match",
        "--count",
        "--files-with-matches",
        "--files-without-match",
        "--only-matching",
        "--quiet",
        "--silent",
        "--no-messages",
        "--line-number",
        "--with-filename",
        "--no-filename",
        "--byte-offset",
        "--null",
        "--text",
        "--recursive",
        "--initial-tab",
        "--line-buffered",
        "--no-group-separator",
    ],
    long_eq: &["--color"],
    long_value: &[
        "--regexp",
        "--file",
        "--max-count",
        "--after-context",
        "--before-context",
        "--context",
        "--include",
        "--exclude",
        "--exclude-dir",
        "--directories",
        "--devices",
        "--binary-files",
        "--label",
        "--group-separator",
    ],
    path_long: &["--file"],
    pattern_short: "ef",
    pattern_long: &["--regexp", "--file"],
    operands: Operands::Pattern,
    default_dot: true,
    recursive_short: "r",
    recursive_long: &["--recursive"],
    directories: true,
    ..NONE
};

const RG: Spec = Spec {
    short: "FHINPSUaclinopqsuvwx",
    value: "egtTABCmjdErM",
    long: &[
        "--files",
        "--type-list",
        "--hidden",
        "--no-ignore",
        "--no-ignore-vcs",
        "--no-ignore-parent",
        "--no-ignore-dot",
        "--no-ignore-global",
        "--no-ignore-exclude",
        "--no-messages",
        "--no-heading",
        "--heading",
        "--line-number",
        "--no-line-number",
        "--with-filename",
        "--no-filename",
        "--column",
        "--vimgrep",
        "--stats",
        "--count",
        "--count-matches",
        "--files-with-matches",
        "--files-without-match",
        "--only-matching",
        "--fixed-strings",
        "--word-regexp",
        "--line-regexp",
        "--ignore-case",
        "--smart-case",
        "--case-sensitive",
        "--invert-match",
        "--multiline",
        "--multiline-dotall",
        "--text",
        "--trim",
        "--json",
        "--null",
        "--no-config",
        "--quiet",
        "--unrestricted",
        "--pcre2",
        "--no-pcre2",
        "--sort-files",
    ],
    long_value: &[
        "--regexp",
        "--file",
        "--glob",
        "--iglob",
        "--type",
        "--type-not",
        "--max-count",
        "--max-depth",
        "--after-context",
        "--before-context",
        "--context",
        "--threads",
        "--replace",
        "--sort",
        "--sortr",
        "--max-filesize",
        "--max-columns",
        "--color",
        "--colors",
        "--encoding",
    ],
    path_long: &["--file"],
    pattern_short: "ef",
    pattern_long: &["--regexp", "--file"],
    no_pattern_long: &["--files", "--type-list"],
    operands: Operands::Pattern,
    default_dot: true,
    walks: true,
    ..NONE
};

const STAT: Spec = Spec {
    short: "srxnqlF",
    value: "c",
    long_value: &["--format", "--printf"],
    ..NONE
};

// no `-p`/`--preserve-date`: it sets the access time back with `utimes(2)`, a write to the file's metadata
const FILE: Spec = Spec {
    short: "bhiInNr",
    long: &[
        "--brief",
        "--mime",
        "--mime-type",
        "--mime-encoding",
        "--no-pad",
        "--keep-going",
        "--raw",
    ],
    ..NONE
};

const DU: Spec = Spec {
    short: "AHPSabchkmgsx",
    value: "dtB",
    long: &[
        "--all",
        "--apparent-size",
        "--bytes",
        "--total",
        "--human-readable",
        "--summarize",
        "--one-file-system",
        "--si",
        "--separate-dirs",
        "--count-links",
        "--dereference-args",
    ],
    long_value: &["--max-depth", "--threshold", "--block-size", "--exclude"],
    default_dot: true,
    walks: true,
    ..NONE
};

const PWD: Spec = Spec {
    short: "LP",
    operands: Operands::NoneAllowed,
    ..NONE
};

const WHICH: Spec = Spec {
    short: "as",
    operands: Operands::Names,
    ..NONE
};

const BASENAME: Spec = Spec {
    short: "az",
    value: "s",
    long: &["--multiple", "--zero"],
    long_value: &["--suffix"],
    operands: Operands::Names,
    ..NONE
};

const DIRNAME: Spec = Spec {
    short: "z",
    long: &["--zero"],
    operands: Operands::Names,
    ..NONE
};

const REALPATH: Spec = Spec {
    short: "emsqzLP",
    long: &[
        "--canonicalize-existing",
        "--canonicalize-missing",
        "--no-symlinks",
        "--quiet",
        "--strip",
        "--zero",
        "--logical",
        "--physical",
    ],
    long_value: &["--relative-to", "--relative-base"],
    path_long: &["--relative-to", "--relative-base"],
    ..NONE
};

const TREE: Spec = Spec {
    short: "CDFNQadfghinpqrstuvx",
    value: "LPI",
    long: &[
        "--noreport",
        "--dirsfirst",
        "--prune",
        "--du",
        "--inodes",
        "--device",
        "--gitignore",
        "--filesfirst",
        "--matchdirs",
        "--ignore-case",
    ],
    long_value: &["--filelimit", "--charset", "--sort", "--timefmt"],
    default_dot: true,
    walks: true,
    ..NONE
};

const SORT: Spec = Spec {
    short: "bcCdfghiMmnrsuVz",
    value: "kSt",
    long: &[
        "--ignore-leading-blanks",
        "--dictionary-order",
        "--ignore-case",
        "--general-numeric-sort",
        "--ignore-nonprinting",
        "--month-sort",
        "--human-numeric-sort",
        "--numeric-sort",
        "--reverse",
        "--version-sort",
        "--stable",
        "--unique",
        "--merge",
        "--zero-terminated",
        "--debug",
    ],
    long_eq: &["--check"],
    long_value: &[
        "--key",
        "--field-separator",
        "--buffer-size",
        "--sort",
        "--parallel",
    ],
    ..NONE
};

const UNIQ: Spec = Spec {
    short: "cdDiuz",
    value: "fsw",
    long: &[
        "--count",
        "--repeated",
        "--unique",
        "--ignore-case",
        "--zero-terminated",
    ],
    long_eq: &["--all-repeated", "--group"],
    long_value: &["--skip-fields", "--skip-chars", "--check-chars"],
    operands: Operands::Single,
    ..NONE
};

const CUT: Spec = Spec {
    short: "snz",
    value: "bcdf",
    long: &["--complement", "--only-delimited", "--zero-terminated"],
    long_value: &[
        "--bytes",
        "--characters",
        "--delimiter",
        "--fields",
        "--output-delimiter",
    ],
    ..NONE
};

const TR: Spec = Spec {
    short: "cCdst",
    long: &[
        "--complement",
        "--delete",
        "--squeeze-repeats",
        "--truncate-set1",
    ],
    operands: Operands::Sets,
    ..NONE
};

const DIFF: Spec = Spec {
    short: "abBdEeiNpqstTuwy",
    value: "UCLxW",
    long: &[
        "--brief",
        "--ignore-case",
        "--ignore-space-change",
        "--ignore-all-space",
        "--ignore-blank-lines",
        "--side-by-side",
        "--report-identical-files",
        "--new-file",
        "--text",
        "--ignore-trailing-space",
        "--expand-tabs",
        "--initial-tab",
        "--minimal",
        "--speed-large-files",
        "--strip-trailing-cr",
        "--no-dereference",
        "--left-column",
        "--suppress-common-lines",
    ],
    long_eq: &["--unified", "--context", "--color"],
    long_value: &["--label", "--width", "--exclude", "--ignore-matching-lines"],
    operands: Operands::Two,
    ..NONE
};

const JQ: Spec = Spec {
    short: "cCrsSneRajM",
    long: &[
        "--compact-output",
        "--raw-output",
        "--join-output",
        "--slurp",
        "--null-input",
        "--raw-input",
        "--sort-keys",
        "--tab",
        "--exit-status",
        "--ascii-output",
        "--color-output",
        "--monochrome-output",
        "--raw-output0",
        "--seq",
    ],
    long_value: &["--indent"],
    operands: Operands::Filter,
    ..NONE
};

/// The words after `--`, and the words before it, split.
struct Walked<'a> {
    /// The operands, in order, each with whether it came after `--`.
    operands: Vec<(&'a Arg, bool)>,
    pattern_given: bool,
    no_pattern: bool,
    /// An option made the command walk the directories it reads (`-r`, `-R`, `-d recurse`).
    recursive: bool,
}

/// An option value: the next word, which no glob may stand for.
fn take_value<'a>(args: &'a [Arg], at: &mut usize) -> Result<&'a Arg, Why> {
    *at += 1;
    let value = args.get(*at).ok_or("option without its value")?;
    if value.globby {
        return Err("glob as an option value");
    }
    Ok(value)
}

/// The action of grep's `-d`/`--directories` (`dirs`) or `-D`/`--devices` (`devices`): exactly `read` or
/// `skip`, and for directories `recurse`, which walks (the result). GNU accepts abbreviations (`-d rec`) and
/// ugrep adds `dereference-recurse`, which also follows symlinks: any other value is refused, not guessed.
fn grep_action(dirs: bool, devices: bool, value: &str) -> Result<bool, Why> {
    match value {
        "recurse" if dirs => Ok(true),
        "read" | "skip" => Ok(false),
        _ if dirs || devices => Err("grep action other than read, skip or recurse"),
        _ => Ok(false),
    }
}

/// Walk `args` by `spec`: every option must be listed. Paths named by options go to `paths`.
///
/// Options come first. A word that starts with `-` after the first operand, and before any `--`, is not
/// proven for any program: GNU `getopt_long` permutes and reads it as an option, BSD `getopt` (macOS `cat`,
/// `head`, `uniq`, ...) stops at the first operand and reads it as a file, and the two differ in what is
/// read and, for `uniq a -c`, in what is written. The same goes for a `--` after an operand.
fn walk<'a>(spec: &Spec, args: &'a [Arg], paths: &mut Vec<ReadPath>) -> Result<Walked<'a>, Why> {
    let mut walked = Walked {
        operands: Vec::new(),
        pattern_given: false,
        no_pattern: false,
        recursive: false,
    };
    let mut after_dashes = false;
    let mut at = 0;
    while at < args.len() {
        let arg = &args[at];
        let text = arg.text.as_str();
        if after_dashes {
            walked.operands.push((arg, true));
        } else if text.starts_with('-') && !walked.operands.is_empty() {
            return Err("option after an operand");
        } else if text == "--" {
            after_dashes = true;
        } else if let Some(long) = text.strip_prefix("--") {
            if arg.globby {
                return Err("glob in an option");
            }
            let (name, glued) = match long.split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value)),
                None => (format!("--{long}"), None),
            };
            let name = name.as_str();
            if spec.long.contains(&name) && glued.is_none() {
                walked.no_pattern |= spec.no_pattern_long.contains(&name);
                walked.recursive |= spec.recursive_long.contains(&name);
            } else if spec.long_eq.contains(&name) {
                if let Some(value) = glued.filter(|_| spec.path_long.contains(&name)) {
                    read_path(paths, value, false);
                }
            } else if spec.long_value.contains(&name) {
                let value = match glued {
                    Some(value) => value,
                    None => &take_value(args, &mut at)?.text,
                };
                if spec.path_long.contains(&name) {
                    read_path(paths, value, false);
                }
                walked.pattern_given |= spec.pattern_long.contains(&name);
                if spec.directories {
                    walked.recursive |=
                        grep_action(name == "--directories", name == "--devices", value)?;
                }
            } else {
                return Err("option not in the table");
            }
        } else if text.starts_with('-') && text.len() > 1 {
            if arg.globby {
                return Err("glob in an option");
            }
            let body = &text[1..];
            if spec.digits && body.bytes().all(|b| b.is_ascii_digit()) {
                at += 1;
                continue;
            }
            for (k, c) in body.char_indices() {
                if spec.short.contains(c) {
                    walked.recursive |= spec.recursive_short.contains(c);
                    continue;
                }
                let rest = &body[k + c.len_utf8()..];
                if spec.value.contains(c) {
                    let value = if rest.is_empty() {
                        take_value(args, &mut at)?.text.as_str()
                    } else {
                        rest
                    };
                    if spec.path_short.contains(c) {
                        read_path(paths, value, false);
                    }
                    walked.pattern_given |= spec.pattern_short.contains(c);
                    if spec.directories {
                        walked.recursive |= grep_action(c == 'd', c == 'D', value)?;
                    }
                } else if !spec.glued.contains(c) {
                    return Err("option not in the table");
                }
                break;
            }
        } else if text == "-" {
            return Err("standard input operand");
        } else {
            walked.operands.push((arg, false));
        }
        at += 1;
    }
    Ok(walked)
}

/// jq can print the environment (`env`, `$ENV`) and load modules from the file system (`import`, `include`,
/// `modulemeta`). A filter that holds any of them as a whole identifier, in any case, is refused: a read-only
/// command does not hand the environment's secrets to the reader. A field of that name (`.env`) is refused
/// too; `.environment` is not.
fn names_a_risky_jq_builtin(filter: &str) -> bool {
    let lower = filter.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    ["env", "import", "include", "modulemeta"]
        .iter()
        .any(|word| {
            lower.match_indices(word).any(|(at, _)| {
                let before = at == 0 || !identifier(bytes[at - 1]);
                let after = bytes.get(at + word.len()).is_none_or(|b| !identifier(*b));
                before && after
            })
        })
}

/// A path operand: a glob in it may not start with `-` unless it follows `--`.
fn path_operand(
    arg: &Arg,
    after_dashes: bool,
    recursive: bool,
    paths: &mut Vec<ReadPath>,
) -> Result<(), Why> {
    if arg.risky && !after_dashes {
        return Err("glob that can expand to an option");
    }
    read_path(paths, &arg.text, recursive);
    Ok(())
}

/// A word that is not a path: no glob may stand for it.
fn plain_operand(arg: &Arg) -> Result<&str, Why> {
    if arg.globby {
        return Err("glob where a plain word is read");
    }
    Ok(arg.text.as_str())
}

/// The paths of one simple command, or why it is not proven read-only.
fn command_paths(program: &str, args: &[Arg], paths: &mut Vec<ReadPath>) -> Result<(), Why> {
    let spec = match program {
        "git" => return git_paths(args, paths),
        "find" => return find_paths(args, paths),
        "ls" => &LS,
        "cat" => &CAT,
        "head" | "tail" => &HEAD,
        "wc" => &WC,
        "grep" | "egrep" | "fgrep" => &GREP,
        "rg" => &RG,
        "stat" => &STAT,
        "file" => &FILE,
        "du" => &DU,
        "pwd" => &PWD,
        "which" => &WHICH,
        "basename" => &BASENAME,
        "dirname" => &DIRNAME,
        "realpath" => &REALPATH,
        "tree" => &TREE,
        "sort" => &SORT,
        "uniq" => &UNIQ,
        "cut" => &CUT,
        "tr" => &TR,
        "diff" => &DIFF,
        "jq" => &JQ,
        _ => return Err("program not in the table"),
    };
    let walked = walk(spec, args, paths)?;
    let recursive = spec.walks || walked.recursive;
    let mut operands = walked.operands.into_iter();
    match spec.operands {
        Operands::NoneAllowed => {
            if operands.next().is_some() {
                return Err("operand where there is none");
            }
        }
        Operands::Names => {
            let names: Vec<_> = operands.collect();
            if names.is_empty() {
                return Err("no name to look at");
            }
            for (arg, _) in names {
                plain_operand(arg)?;
            }
        }
        Operands::Sets => {
            let sets: Vec<_> = operands.collect();
            if sets.is_empty() || sets.len() > 2 {
                return Err("tr takes one or two sets");
            }
            for (arg, _) in sets {
                plain_operand(arg)?;
            }
        }
        Operands::Single => {
            let rest: Vec<_> = operands.collect();
            if rest.len() > 1 {
                return Err("second operand is an output file");
            }
            for (arg, dashes) in rest {
                path_operand(arg, dashes, recursive, paths)?;
            }
        }
        Operands::Two => {
            let rest: Vec<_> = operands.collect();
            if rest.len() < 2 {
                return Err("diff needs two paths");
            }
            // two directories are compared file by file: every file they share is read
            for (arg, dashes) in rest {
                path_operand(arg, dashes, true, paths)?;
            }
        }
        Operands::Filter => {
            let (first, _) = operands.next().ok_or("jq needs a filter")?;
            if names_a_risky_jq_builtin(plain_operand(first)?) {
                return Err("jq filter reads the environment or loads a module");
            }
            for (arg, dashes) in operands {
                path_operand(arg, dashes, recursive, paths)?;
            }
        }
        Operands::Pattern => {
            if !walked.pattern_given && !walked.no_pattern {
                let (first, _) = operands.next().ok_or("no pattern")?;
                plain_operand(first)?;
            }
            let mut any = false;
            for (arg, dashes) in operands {
                any = true;
                path_operand(arg, dashes, recursive, paths)?;
            }
            if !any && spec.default_dot {
                read_path(paths, ".", recursive);
            }
        }
        Operands::Paths => {
            let mut any = false;
            for (arg, dashes) in operands {
                any = true;
                path_operand(arg, dashes, recursive, paths)?;
            }
            if !any && spec.default_dot {
                read_path(paths, ".", recursive);
            }
        }
    }
    Ok(())
}

// --- find ----------------------------------------------------------------------------------------------------

const FIND_FLAGS: &[&str] = &[
    "-print",
    "-print0",
    "-prune",
    "-a",
    "-and",
    "-o",
    "-or",
    "-not",
    "-true",
    "-false",
    "-empty",
    "-readable",
    "-writable",
    "-executable",
    "-depth",
    "-xdev",
    "-mount",
    "-noleaf",
    "-nogroup",
    "-nouser",
    "-daystart",
    "-ls",
    "(",
    ")",
    "!",
];
const FIND_VALUES: &[&str] = &[
    "-name",
    "-iname",
    "-path",
    "-ipath",
    "-wholename",
    "-iwholename",
    "-regex",
    "-iregex",
    "-type",
    "-xtype",
    "-size",
    "-mtime",
    "-atime",
    "-ctime",
    "-mmin",
    "-amin",
    "-cmin",
    "-perm",
    "-user",
    "-group",
    "-uid",
    "-gid",
    "-links",
    "-inum",
    "-maxdepth",
    "-mindepth",
    "-lname",
    "-ilname",
    "-regextype",
    "-newermt",
    "-printf",
];
const FIND_PATH_VALUES: &[&str] = &["-newer", "-anewer", "-cnewer", "-samefile"];

/// `find`: start points, then an expression made only of the predicates above. `-exec`, `-execdir`, `-ok`,
/// `-okdir`, `-delete`, `-fprint`, `-fprint0`, `-fprintf`, `-fls`, `-L`, `-H`, `-follow`, `-files0-from` and
/// every other predicate are refused because they are not listed. Every start point is read recursively. The
/// option-after-an-operand rule of [`walk`] has nothing to add here: the expression is `find`'s own grammar,
/// and a word of it that the table does not list is refused.
fn find_paths(args: &[Arg], paths: &mut Vec<ReadPath>) -> Result<(), Why> {
    let mut at = 0;
    if args.first().is_some_and(|a| a.text == "-P") {
        at = 1;
    }
    let starts = at;
    while let Some(arg) = args.get(at) {
        if arg.text.starts_with('-') || matches!(arg.text.as_str(), "(" | ")" | "!") {
            break;
        }
        path_operand(arg, false, true, paths)?;
        at += 1;
    }
    if at == starts {
        read_path(paths, ".", true);
    }
    while let Some(arg) = args.get(at) {
        let name = arg.text.as_str();
        if arg.globby {
            return Err("glob in a find expression");
        }
        if FIND_FLAGS.contains(&name) {
            at += 1;
        } else if FIND_VALUES.contains(&name) {
            take_value(args, &mut at)?;
            at += 1;
        } else if FIND_PATH_VALUES.contains(&name) {
            let value = take_value(args, &mut at)?;
            read_path(paths, &value.text, false);
            at += 1;
        } else {
            return Err("find predicate not in the table");
        }
    }
    Ok(())
}

// --- git -----------------------------------------------------------------------------------------------------

const GIT_LOG: Spec = Spec {
    short: "pusWbwqmrtzaREFiP",
    value: "n",
    glued: "UMCB",
    digits: true,
    long: &[
        "--oneline",
        "--graph",
        "--no-decorate",
        "--shortstat",
        "--numstat",
        "--name-only",
        "--name-status",
        "--abbrev-commit",
        "--no-abbrev-commit",
        "--no-merges",
        "--merges",
        "--first-parent",
        "--all",
        "--branches",
        "--tags",
        "--remotes",
        "--reverse",
        "--follow",
        "--patch",
        "--no-patch",
        "--date-order",
        "--topo-order",
        "--author-date-order",
        "--left-right",
        "--cherry-pick",
        "--cherry-mark",
        "--cherry",
        "--boundary",
        "--full-history",
        "--sparse",
        "--simplify-merges",
        "--ancestry-path",
        "--notes",
        "--no-notes",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-all-space",
        "--ignore-space-change",
        "--ignore-space-at-eol",
        "--ignore-cr-at-eol",
        "--text",
        "--summary",
        "--full-index",
        "--binary",
        "--minimal",
        "--patience",
        "--histogram",
        "--raw",
        "--no-walk",
        "--do-walk",
        "--merge",
        "--ignore-missing",
        "--exit-code",
        "--quiet",
        "--check",
        "--root",
        "--no-renames",
        "--source",
        "--children",
        "--parents",
        "--all-match",
        "--invert-grep",
        "--regexp-ignore-case",
        "--extended-regexp",
        "--fixed-strings",
        "--perl-regexp",
    ],
    long_eq: &[
        "--decorate",
        "--stat",
        "--abbrev",
        "--color",
        "--pretty",
        "--format",
        "--date",
        "--word-diff",
        "--color-words",
        "--find-renames",
        "--find-copies",
        "--diff-filter",
        "--max-count",
        "--skip",
        "--since",
        "--after",
        "--until",
        "--before",
        "--author",
        "--committer",
        "--grep",
        "--min-parents",
        "--max-parents",
        "--unified",
        "--dirstat",
        "--submodule",
        "--ignore-submodules",
        "--encoding",
    ],
    operands: Operands::Paths,
    ..NONE
};

const GIT_DIFF: Spec = Spec {
    short: "puwbRaqz",
    glued: "UMCB",
    long: &[
        "--cached",
        "--staged",
        "--shortstat",
        "--numstat",
        "--name-only",
        "--name-status",
        "--exit-code",
        "--quiet",
        "--check",
        "--minimal",
        "--patience",
        "--histogram",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-space-at-eol",
        "--ignore-space-change",
        "--ignore-all-space",
        "--ignore-cr-at-eol",
        "--summary",
        "--full-index",
        "--binary",
        "--no-renames",
        "--text",
        "--raw",
        "--patch",
        "--no-patch",
        "--ignore-blank-lines",
        "--function-context",
    ],
    long_eq: &[
        "--stat",
        "--color",
        "--word-diff",
        "--color-words",
        "--find-renames",
        "--find-copies",
        "--diff-filter",
        "--unified",
        "--abbrev",
        "--dirstat",
        "--submodule",
        "--ignore-submodules",
    ],
    ..NONE
};

const GIT_STATUS: Spec = Spec {
    short: "sbvz",
    glued: "u",
    long: &[
        "--short",
        "--branch",
        "--long",
        "--verbose",
        "--no-renames",
        "--renames",
        "--ahead-behind",
        "--no-ahead-behind",
        "--show-stash",
        "--no-column",
    ],
    long_eq: &[
        "--porcelain",
        "--untracked-files",
        "--ignored",
        "--ignore-submodules",
        "--column",
        "--find-renames",
    ],
    ..NONE
};

const GIT_BLAME: Spec = Spec {
    short: "wefnpstlb",
    value: "L",
    glued: "MC",
    long: &[
        "--porcelain",
        "--line-porcelain",
        "--incremental",
        "--show-email",
        "--show-name",
        "--show-number",
        "--root",
        "--first-parent",
        "--minimal",
    ],
    long_eq: &["--abbrev", "--since", "--date"],
    ..NONE
};

const GIT_LS_FILES: Spec = Spec {
    short: "cdmoiskzftvu",
    value: "x",
    long: &[
        "--cached",
        "--deleted",
        "--modified",
        "--others",
        "--ignored",
        "--stage",
        "--unmerged",
        "--killed",
        "--directory",
        "--no-empty-directory",
        "--eol",
        "--full-name",
        "--error-unmatch",
        "--exclude-standard",
        "--deduplicate",
        "--recurse-submodules",
        "--debug",
        "--sparse",
        "--resolve-undo",
    ],
    long_eq: &["--abbrev"],
    long_value: &["--exclude"],
    ..NONE
};

const GIT_REV_PARSE: Spec = Spec {
    short: "q",
    long: &[
        "--show-toplevel",
        "--show-prefix",
        "--show-cdup",
        "--show-superproject-working-tree",
        "--is-inside-work-tree",
        "--is-inside-git-dir",
        "--is-bare-repository",
        "--is-shallow-repository",
        "--git-dir",
        "--absolute-git-dir",
        "--git-common-dir",
        "--verify",
        "--quiet",
        "--symbolic",
        "--symbolic-full-name",
        "--all",
        "--branches",
        "--tags",
        "--remotes",
    ],
    long_eq: &["--short", "--abbrev-ref"],
    ..NONE
};

const GIT_DESCRIBE: Spec = Spec {
    long: &[
        "--tags",
        "--all",
        "--long",
        "--always",
        "--exact-match",
        "--first-parent",
        "--contains",
    ],
    // no `--dirty` or `--broken`: both refresh the index, a write, and run the configured fsmonitor
    long_eq: &["--abbrev", "--candidates"],
    long_value: &["--match", "--exclude"],
    ..NONE
};

const GIT_BRANCH: Spec = Spec {
    short: "arv",
    long: &[
        "--all",
        "--remotes",
        "--verbose",
        "--list",
        "--show-current",
        "--no-color",
        "--no-column",
        "--omit-empty",
        "--no-abbrev",
        "--ignore-case",
    ],
    long_eq: &["--color", "--column", "--abbrev"],
    long_value: &[
        "--sort",
        "--contains",
        "--no-contains",
        "--merged",
        "--no-merged",
        "--points-at",
        "--format",
    ],
    operands: Operands::NoneAllowed,
    ..NONE
};

const GIT_REMOTE: Spec = Spec {
    short: "v",
    long: &["--verbose"],
    operands: Operands::NoneAllowed,
    ..NONE
};

/// `git`: no global option but two, and only the subcommands above, each with its own options. Every
/// operand may name a path (git decides between a revision and a path), so each is reported, except that a
/// word holding `:` is refused (`REV:path` reads a blob, `:(top)` is pathspec magic). An operand may be a
/// directory pathspec, so each is reported as read recursively. `Proven.runs_git_config` tells the caller
/// that git ran at all.
fn git_paths(args: &[Arg], paths: &mut Vec<ReadPath>) -> Result<(), Why> {
    // `%G?`, `%GS`, `%GK` and the other `%G` placeholders of a format verify the signature of a commit, which
    // runs gpg or ssh-keygen: no word of a proven git line holds one
    if args.iter().any(|arg| arg.text.contains("%G")) {
        return Err("git format that verifies a signature");
    }
    let mut at = 0;
    while let Some(arg) = args.get(at) {
        match arg.text.as_str() {
            "--no-pager" | "--no-optional-locks" => at += 1,
            text if text.starts_with('-') => return Err("git global option"),
            _ => break,
        }
    }
    let subcommand = args.get(at).ok_or("git without a subcommand")?;
    let spec = match subcommand.text.as_str() {
        "status" => &GIT_STATUS,
        "log" | "show" => &GIT_LOG,
        "diff" => &GIT_DIFF,
        "blame" => &GIT_BLAME,
        "ls-files" => &GIT_LS_FILES,
        "rev-parse" => &GIT_REV_PARSE,
        "describe" => &GIT_DESCRIBE,
        "branch" => &GIT_BRANCH,
        "remote" => &GIT_REMOTE,
        _ => return Err("git subcommand not in the table"),
    };
    let walked = walk(spec, &args[at + 1..], paths)?;
    for (arg, after_dashes) in walked.operands {
        if matches!(spec.operands, Operands::NoneAllowed) {
            return Err("operand where there is none");
        }
        if arg.text.contains(':') {
            return Err("git word with a colon");
        }
        path_operand(arg, after_dashes, true, paths)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The layers alone: the scanner, and the cross-check with `analyze`. `classify_readonly` refuses a
    //! line when either layer does, so the table tests cannot show that each layer works on its own.
    use super::*;

    const HOME: &str = "/home/tester";

    #[test]
    fn the_scanner_alone_refuses_what_it_cannot_read_plainly() {
        for (command, why) in [
            ("ls \"$X\"", "expansion inside double quotes"),
            ("ls \"a`b\"", "expansion inside double quotes"),
            ("ls \"a\\b\"", "expansion inside double quotes"),
            ("ls \"a!b\"", "expansion inside double quotes"),
            ("ls \u{e9}", "character outside the plain set"),
            ("ls 'abc", "unterminated quote"),
            ("ls \"abc", "unterminated quote"),
            ("ls =ls", "word opens with ="),
            ("ls ''=ls", "word opens with ="),
            ("ls \"\"=ls", "word opens with ="),
            ("ls '='ls", "word opens with ="),
            ("ls a==ls", "word opens with ="),
            ("ls a:=ls", "word opens with ="),
            ("cat <1-5>/dev/null", "numeric glob"),
            ("cat <1-5>&2 x", "numeric glob"),
            ("cat <-9>x", "numeric glob"),
            ("cat <->x", "numeric glob"),
            ("cat a<10-20>b", "numeric glob"),
            ("ls > > x", "two redirect operators in a row"),
            ("ls < < x", "two redirect operators in a row"),
            ("ls >> x", "append, clobber or process substitution"),
            ("ls >| x", "append, clobber or process substitution"),
            ("ls >(x)", "append, clobber or process substitution"),
            (
                "ls << x",
                "here-document, fd, read-write or process substitution",
            ),
            (
                "ls <& 3",
                "here-document, fd, read-write or process substitution",
            ),
            (
                "ls <> x",
                "here-document, fd, read-write or process substitution",
            ),
            (
                "ls <(x)",
                "here-document, fd, read-write or process substitution",
            ),
            ("ls &>> x", "append redirect"),
            ("ls || ls", "|| or |&"),
            ("ls |& cat", "|| or |&"),
            ("ls & ls", "background job"),
            ("ls &", "background job"),
            ("ls $x", "shell metacharacter"),
            ("ls ;", "shell metacharacter"),
            ("ls ~", "shell metacharacter"),
            ("ls >", "redirect without a target"),
            ("ls &&", "empty command"),
            ("&& ls", "empty command"),
            ("", "empty command"),
            ("ls > *", "glob in a redirect target"),
            ("ls .*", "glob can match .."),
            ("ls ..*", "glob can match .."),
            ("ls [.]*", "bracket glob can match .."),
            ("ls ./[a-c]*", "bracket glob can match .."),
            ("ls ./[[:punct:]]*", "bracket glob can match .."),
            ("ls ..", "parent directory component"),
            ("ls a/../b", "parent directory component"),
            ("ls --color=../x", "parent directory component"),
        ] {
            assert_eq!(scan(command).expect_err(command), why, "{command}");
        }
    }

    #[test]
    fn the_scanner_reads_stages_words_and_redirects() {
        let stages = scan("cd /a && ls -la 'b c' 2>&1 | head -n 5 > /dev/null").expect("scans");
        let words: Vec<Vec<&str>> = stages
            .iter()
            .map(|s| s.args.iter().map(|a| a.text.as_str()).collect())
            .collect();
        assert_eq!(
            words,
            [
                vec!["cd", "/a"],
                vec!["ls", "-la", "b c"],
                vec!["head", "-n", "5"]
            ]
        );
        let joins: Vec<_> = stages.iter().map(|s| s.join).collect();
        assert_eq!(joins, [None, Some(Join::And), Some(Join::Pipe)]);
        assert_eq!(stages[1].redirs, [(">&", "1".to_string())]);
        assert_eq!(stages[2].redirs, [(">", "/dev/null".to_string())]);
        // an fd number is no word; a quoted one is
        let stages = scan("ls 2>&1 '2'>&1").expect("scans");
        let words: Vec<&str> = stages[0].args.iter().map(|a| a.text.as_str()).collect();
        assert_eq!(words, ["ls", "2"]);
    }

    fn agree(scanned: &str, analyzed: &str) -> Result<(), Why> {
        agrees_with_analyze(analyzed, HOME, &scan(scanned).expect("scans"))
    }

    #[test]
    fn the_cross_check_alone_refuses_a_line_analyze_reads_differently() {
        assert_eq!(agree("ls -la src", "ls -la src"), Ok(()));
        assert_eq!(agree("cd /a && ls | wc -l", "cd /a && ls | wc -l"), Ok(()));
        // other words, other commands, other redirects
        assert_eq!(agree("ls a", "ls b"), Err("analyze reads other words"));
        assert_eq!(agree("ls a b", "ls a"), Err("analyze reads other words"));
        assert_eq!(agree("ls", "ls; ls"), Err("analyze reads other commands"));
        assert_eq!(agree("ls | ls", "ls"), Err("analyze reads other commands"));
        assert_eq!(
            agree("ls", "ls $(pwd)"),
            Err("analyze reads other commands")
        );
        assert_eq!(
            agree("ls > /dev/null", "ls"),
            Err("analyze reads other words")
        );
        assert_eq!(agree("ls", "ls 2>&1"), Err("analyze reads other words"));
        assert_eq!(agree("ls 2>&1", "ls >&2"), Err("analyze reads other words"));
        // something opaque in an entry whose words match
        assert_eq!(
            agree("bash x.sh", "bash x.sh"),
            Err("analyze finds something opaque")
        );
        assert_eq!(
            agree("python3 -c x", "python3 -c x"),
            Err("analyze finds something opaque")
        );
        // another reading of a wrapped command
        assert_eq!(
            agree("ls a", "ls ${x:-a}"),
            Err("analysis has alternate readings")
        );
        // the work budget ran out
        let long = "ls;".repeat(1_000_000);
        assert_eq!(agree("ls", &long), Err("analysis incomplete"));
    }

    /// A scanner that is wrong on purpose: whatever line it is given, it reports `ls a`.
    fn scanner_that_always_reads_ls_a(_: &str) -> Result<Vec<Stage>, Why> {
        scan("ls a")
    }

    #[test]
    fn prove_refuses_what_the_scanner_read_and_analyze_did_not() {
        // The table accepts every line below as the scanner reports it (`ls a`), so only the cross-check
        // against `analyze` stands between a scanner bug and a proof. Remove the call to
        // `agrees_with_analyze` from `prove_with` and each of these is proven.
        assert!(prove_with("ls a", HOME, scanner_that_always_reads_ls_a).is_ok());
        for (line, why) in [
            ("ls b", "analyze reads other words"),
            ("ls a b", "analyze reads other words"),
            ("ls a > /dev/null", "analyze reads other words"),
            ("ls a; ls a", "analyze reads other commands"),
            ("ls a | ls a", "analyze reads other commands"),
            ("ls a $(pwd)", "analyze reads other commands"),
            ("ls ${x:-a}", "analysis has alternate readings"),
        ] {
            assert_eq!(
                prove_with(line, HOME, scanner_that_always_reads_ls_a).err(),
                Some(why),
                "{line}"
            );
        }
        // and `prove` itself is `prove_with` the real scanner
        assert_eq!(prove("ls b", HOME), prove_with("ls b", HOME, scan));
    }

    #[test]
    fn a_cd_with_nothing_after_it_is_refused() {
        // the last stage of a proven command is never a `cd`: a `cd` needs an `&&` stage after it
        assert_eq!(prove("cd /a", HOME), Err("cd must lead an && chain"));
        assert_eq!(
            prove("cd /a && cd ./b", HOME),
            Err("cd must lead an && chain")
        );
    }
}
