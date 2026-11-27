//! Rust-only rules for `Opaque` (SH2 review rounds 2–5): commands the analysis may have misread, and wrapper
//! options known to be harmless. Python has none of these; they never change an entry's parity fields.

/// Wrapper options known to take no value: past one of these the next word is surely the program.
const WRAPPER_BOOL_FLAGS: &[(&str, &[&str])] = &[
    (
        "sudo",
        &[
            "-E",
            "-n",
            "-H",
            "-S",
            "-i",
            "-s",
            "-k",
            "-K",
            "-b",
            "-P",
            "-A",
            "-B",
            "-N",
            "--preserve-env",
            "--non-interactive",
            "--login",
            "--shell",
            "--set-home",
            "--stdin",
            "--background",
            "--askpass",
            "--reset-timestamp",
        ],
    ),
    ("doas", &["-n", "-s"]),
    (
        "env",
        &[
            "-i",
            "-0",
            "-v",
            "--ignore-environment",
            "--null",
            "--debug",
        ],
    ),
    ("exec", &["-c", "-l"]),
    (
        "time",
        &[
            "-p",
            "-v",
            "-a",
            "-q",
            "--portability",
            "--verbose",
            "--append",
            "--quiet",
        ],
    ),
    (
        "timeout",
        &["--foreground", "--preserve-status", "-v", "--verbose"],
    ),
    (
        "gtimeout",
        &["--foreground", "--preserve-status", "-v", "--verbose"],
    ),
    ("nice", &[]),
    (
        "xargs",
        &[
            "-0",
            "--null",
            "-r",
            "--no-run-if-empty",
            "-t",
            "--verbose",
            "-p",
            "--interactive",
            "-x",
            "--exit",
            "-o",
            "--open-tty",
        ],
    ),
    (
        "xvfb-run",
        &["-a", "--auto-servernum", "-l", "--listen-tcp"],
    ),
    ("caffeinate", &["-d", "-i", "-m", "-s", "-u"]),
    (
        "watch",
        &[
            "-t",
            "-d",
            "-x",
            "-b",
            "-e",
            "-g",
            "-c",
            "-p",
            "-w",
            "--no-title",
            "--differences",
            "--exec",
            "--beep",
            "--errexit",
            "--chgexit",
            "--color",
            "--precise",
            "--no-wrap",
        ],
    ),
    (
        "arch",
        &["-arm64", "-arm64e", "-x86_64", "-x86_64h", "-i386"],
    ),
    ("setsid", &["-f", "-w", "-c", "--fork", "--wait", "--ctty"]),
    ("ionice", &["-t", "--ignore"]),
    ("unbuffer", &["-p"]),
];

/// The options of wrapper `name` known to take no value.
fn wrapper_bools(name: &str) -> &'static [&'static str] {
    WRAPPER_BOOL_FLAGS
        .iter()
        .find(|(w, _)| *w == name)
        .map_or(&[], |(_, bools)| bools)
}

/// `token` is an option of wrapper `name` known not to take the next word: listed, a `--name=value`,
/// stdbuf's attached `-oL`/`-i0`/`-e0` (a detached `-o L` takes the next word, which Python reads as the
/// program), or nice's `-10`/`-n10`.
pub(crate) fn wrapper_known(name: &str, token: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    wrapper_bools(name).contains(&token)
        || (token.starts_with("--") && token.contains('='))
        || (name == "stdbuf"
            && token.len() > 2
            && matches!(token.as_bytes()[1], b'i' | b'o' | b'e'))
        || (name == "nice"
            && token.strip_prefix('-').is_some_and(|rest| {
                digits(rest.strip_prefix('-').unwrap_or(rest))
                    || rest.strip_prefix('n').is_some_and(digits)
            }))
}

/// The only command values exempt from a mark (controller ruling): the WHOLE value, after quote removal,
/// must be one of these. No word-wise or prefix inspection: anything else is opaque, so atl asks.
const HARMLESS_VALUES: &[&str] = &[
    "",
    "true",
    "false",
    "0",
    "1",
    "yes",
    "no",
    "cat",
    "less",
    "less -R",
    "more",
    "vi",
    "vim",
    "nano",
    "ssh",
    "git-lfs",
    "store",
    "cache",
    "osxkeychain",
    "manager",
    "manager-core",
];

fn harmless_value(value: &str) -> bool {
    HARMLESS_VALUES.contains(&value)
}

/// Assignments whose value the shell or a program later runs as a command (`GIT_PAGER='...' git log`,
/// `BASH_ENV=f bash -c ...`), unless the whole value is in `HARMLESS_VALUES` (`GIT_PAGER=cat`).
pub(crate) fn runs_assigned_command(word: &str) -> bool {
    /// Run as a command unless the whole value is harmless.
    const COMMAND_VARS: &[&str] = &[
        "GIT_SSH_COMMAND",
        "GIT_SSH",
        "RSYNC_RSH",
        "GIT_PAGER",
        "GIT_EDITOR",
        "GIT_SEQUENCE_EDITOR",
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GIT_EXTERNAL_DIFF",
        "GIT_PROXY_COMMAND",
        "PAGER",
        "MANPAGER",
        "EDITOR",
        "VISUAL",
    ];
    /// Always run something (a file, a library, git config) when set.
    const RUN_VARS: &[&str] = &[
        "BASH_ENV",
        "PROMPT_COMMAND",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "GIT_CONFIG_PARAMETERS",
        "LESSOPEN",
        "LESSCLOSE",
        "RIPGREP_CONFIG_PATH",
    ];
    let Some((name, value)) = word.split_once('=') else {
        return false;
    };
    if value.is_empty() {
        return false;
    }
    if RUN_VARS.contains(&name) || name.starts_with("GIT_CONFIG_VALUE_") {
        return true;
    }
    COMMAND_VARS.contains(&name) && !harmless_value(value)
}

/// The values of options `long` (`--long=v` or `--long v`) and of the short options `shorts`. With
/// `clusters`, a short option may end a cluster (`-avze v`) or hold its value attached (`-ev`, `-ave'v'`).
fn option_values<'a>(
    args: &'a [String],
    long: &[&str],
    shorts: &[char],
    clusters: bool,
) -> Vec<&'a str> {
    let mut values = Vec::new();
    for (k, arg) in args.iter().enumerate() {
        let next = args.get(k + 1).map(String::as_str);
        if long.contains(&arg.as_str()) {
            values.extend(next);
            continue;
        }
        if let Some((name, value)) = arg.split_once('=')
            && long.contains(&name)
        {
            values.push(value);
            continue;
        }
        let Some(letters) = arg.strip_prefix('-').filter(|l| !l.starts_with('-')) else {
            continue;
        };
        for (at, c) in letters.char_indices() {
            if shorts.contains(&c) {
                let attached = &letters[at + c.len_utf8()..];
                if attached.is_empty() {
                    values.extend(next);
                } else if clusters {
                    values.push(attached);
                }
                break;
            }
            if !clusters {
                break;
            }
        }
    }
    values
}

/// `git -c key=value` keys whose value git runs as a command.
fn git_key_runs_command(key: &str) -> bool {
    const KEYS: &[&str] = &[
        "core.pager",
        "core.sshcommand",
        "core.editor",
        "core.fsmonitor",
        "core.hookspath",
        "core.askpass",
        "core.gitproxy",
        "sequence.editor",
        "diff.external",
        "credential.helper",
        "gpg.program",
        "gpg.ssh.program",
        "uploadpack.packobjectshook",
    ];
    let key = key.to_ascii_lowercase();
    KEYS.contains(&key.as_str())
        || key.starts_with("pager.")
        || key.starts_with("filter.")
        || (key.starts_with("credential.") && key.ends_with(".helper"))
        || key.ends_with(".textconv")
        || key.ends_with(".command")
        || (key.starts_with("merge.") && key.ends_with(".driver"))
        || (key.starts_with("remote.")
            && (key.ends_with(".uploadpack") || key.ends_with(".receivepack")))
        || ((key.starts_with("difftool.") || key.starts_with("mergetool."))
            && key.ends_with(".cmd"))
        || key.starts_with("url.ext::")
}

/// `git -c key=value` that makes git run a command: a `!` alias, or a command-valued key (or one whose command
/// `git_command_value` reads, such as an `ext::` URL in `remote.*.url`) whose whole value is not in
/// `HARMLESS_VALUES`.
fn git_config_runs_command(config: &str) -> bool {
    let Some((key, value)) = config.split_once('=') else {
        return false;
    };
    if key.to_ascii_lowercase().starts_with("alias.") {
        return value.trim_start().starts_with('!');
    }
    (git_key_runs_command(key) && !harmless_value(value))
        || super::git_command_value(key, value).is_some_and(|v| !harmless_value(&v))
}

/// The program runs a command given in its arguments that the analysis does not parse: `trap 'cmd' EXIT`,
/// `fd -x cmd`, `git -c alias.x='!cmd'`, `git --config-env=core.pager=V`, git's transport options and `ext::`
/// URLs (`git clone -u cmd`), `tar --to-command=cmd`, `tar --checkpoint-action exec=cmd`, `tar -I cmd`
/// (other than exactly a compressor's name), `rsync -e cmd` (other than exactly `ssh`), `rsync --rsync-path=cmd`
/// (other than exactly `rsync`), `export GIT_PAGER=cmd` (as `runs_assigned_command`), and the options of
/// `option_commands` (`rg --pre`, `ugrep --filter`, `sort --compress-program`; as `runs_unread_command`).
pub(crate) fn runs_option_command(prog: &str, args: &[String]) -> bool {
    match prog {
        "trap" => {
            let rest = match args.first().map(String::as_str) {
                Some("--") => &args[1..],
                _ => args,
            };
            rest.len() >= 2 && !matches!(rest[0].as_str(), "" | "-" | "-l" | "-p")
        }
        "export" | "declare" | "typeset" | "local" | "readonly" => {
            args.iter().any(|a| runs_assigned_command(a))
        }
        "fd" | "fdfind" => {
            args.iter().any(|a| {
                a == "--exec"
                    || a == "--exec-batch"
                    || a.starts_with("--exec=")
                    || a.starts_with("--exec-batch=")
            }) || args.iter().any(|a| {
                // `-x`, `-X`, or a cluster of fd's no-value flags ending in one (`-Hx`)
                a.strip_prefix('-')
                    .filter(|l| !l.starts_with('-'))
                    .and_then(|l| l.strip_suffix(['x', 'X']))
                    .is_some_and(|l| l.bytes().all(|b| b"HIsigFalLpu01q".contains(&b)))
            })
        }
        "git" => {
            option_values(args, &[], &['c'], false)
                .iter()
                .any(|c| git_config_runs_command(c))
                || option_values(args, &["--config-env"], &[], false)
                    .iter()
                    .any(|c| {
                        c.split_once('=').is_some_and(|(key, _)| {
                            git_key_runs_command(key)
                                || key.to_ascii_lowercase().starts_with("alias.")
                        })
                    })
                || super::git_transport_scripts(args)
                    .iter()
                    .any(|v| !harmless_value(v))
        }
        "tar" | "gtar" | "bsdtar" => {
            !option_values(
                args,
                &[
                    "--to-command",
                    "--info-script",
                    "--new-volume-script",
                    "--rsh-command",
                ],
                &['F'],
                true,
            )
            .is_empty()
                || args.iter().enumerate().any(|(k, a)| {
                    // `--checkpoint-action exec=cmd`, `--checkpoint-act=exec=cmd`
                    let (name, value) = a
                        .split_once('=')
                        .map_or((a.as_str(), None), |(n, v)| (n, Some(v)));
                    super::is_checkpoint_action(name)
                        && value
                            .or(args.get(k + 1).map(String::as_str))
                            .is_some_and(|v| v.starts_with("exec"))
                })
                || option_values(args, &["--use-compress-program"], &['I'], true)
                    .iter()
                    .any(|v| !COMPRESSORS.contains(v))
        }
        "rsync" => {
            option_values(args, &["--rsh"], &['e'], true)
                .iter()
                .any(|v| *v != "ssh")
                || option_values(args, &["--rsync-path"], &[], false)
                    .iter()
                    .any(|v| *v != "rsync")
        }
        _ => runs_unread_command(prog, args),
    }
}

/// The compressors `tar -I` and `sort --compress-program` may name: they run no command of their own.
const COMPRESSORS: &[&str] = &[
    "gzip", "pigz", "zstd", "unzstd", "xz", "pixz", "bzip2", "pbzip2", "lbzip2", "lz4", "lzip",
    "plzip", "lzma", "lzop", "brotli", "compress",
];

/// The commands `prog` runs from the options it takes them in, the ones that hand the command a file name the
/// text may not show: `rg --pre` and `--hostname-bin`, ugrep's `--filter` (a comma list of `exts:command`; the
/// Claude Code shell `grep` is ugrep) and `--pager=`, and `sort --compress-program`. The long option may be
/// abbreviated to a prefix at least as long as the listed minimum, as getopt reads it. The value is the one
/// attached with `=` or the next word (only the attached one for `--pager`, whose value is optional).
pub(crate) fn option_commands(prog: &str, args: &[String]) -> Vec<String> {
    // (long option, shortest abbreviation read, whether the next word may be the value)
    let options: &[(&str, usize, bool)] = match prog {
        "rg" | "ripgrep" => &[("--pre", 4, true), ("--hostname-bin", 4, true)],
        "grep" | "egrep" | "fgrep" | "ugrep" | "ug" => {
            &[("--filter", 8, true), ("--pager", 7, false)]
        }
        "sort" | "gsort" => &[("--compress-program", 4, true)],
        _ => return Vec::new(),
    };
    let mut commands = Vec::new();
    for (k, arg) in args.iter().enumerate() {
        let (name, attached) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (arg.as_str(), None),
        };
        let Some(&(long, _, next)) = options
            .iter()
            .find(|(long, min, _)| name.len() >= *min && long.starts_with(name))
        else {
            continue;
        };
        let Some(value) = attached.or(args.get(k + 1).map(String::as_str).filter(|_| next)) else {
            continue;
        };
        if long == "--filter" {
            // `exts:command`, or a command alone
            commands.extend(
                value
                    .split(',')
                    .map(|item| item.split_once(':').map_or(item, |(_, command)| command))
                    .map(str::to_string),
            );
        } else {
            commands.push(value.to_string());
        }
    }
    commands
}

/// The values of `option_commands` that run nothing when handed a file: not `HARMLESS_VALUES`, which holds
/// editors (vim runs the `:!` lines of the file it is given) and `ssh`.
const FILE_FED_HARMLESS: &[&str] = &["", "true", "false", "cat", "less", "less -R", "more"];

/// The program runs something the text does not show: `option_commands` has a command that is not a known
/// harmless value (`cat`, a pager, a compressor for `sort`; no editor), a config file can hold such an option
/// (`ug` and `ugrep --config`/`---FILE` read ugrep options from files), or an awk program runs a command.
pub(crate) fn runs_unread_command(prog: &str, args: &[String]) -> bool {
    let compressor = matches!(prog, "sort" | "gsort");
    option_commands(prog, args).iter().any(|c| {
        !(FILE_FED_HARMLESS.contains(&c.as_str())
            || (compressor && COMPRESSORS.contains(&c.as_str())))
    }) || reads_ugrep_config(prog, args)
        || awk_runs_commands(prog, args)
}

/// A ugrep-family program that loads option files: `ug` and `ug+` read `.ugrep` from the working folder and the
/// home folder, `--config` reads the same, and `---FILE` reads FILE.
fn reads_ugrep_config(prog: &str, args: &[String]) -> bool {
    match prog {
        "ug" | "ug+" => true,
        "grep" | "egrep" | "fgrep" | "ugrep" => args
            .iter()
            .any(|a| a == "--config" || a.starts_with("--config=") || a.starts_with("---")),
        _ => false,
    }
}

/// awk runs commands from its program: `system(`, a pipe to or from a string (`print | "cmd"`,
/// `"cmd" | getline`, `|&`), a gawk `@` directive (`@load`, `@include`), or text the analysis cannot see: a program
/// file (`-f`, `-E`), an extension (`-l`) or an include other than `inplace` (`-i`). The program is the first
/// operand, or the value of `-e`/`--source`; operands after it, and the values of `-F` and `-v`, are not read as
/// program text.
fn awk_runs_commands(prog: &str, args: &[String]) -> bool {
    if !matches!(prog, "awk" | "gawk" | "mawk" | "nawk" | "original-awk") {
        return false;
    }
    let mut programs: Vec<&str> = Vec::new();
    let mut operands = false;
    let mut k = 0;
    while k < args.len() {
        let arg = args[k].as_str();
        let next = args.get(k + 1).map(String::as_str);
        k += 1;
        if operands || !arg.starts_with('-') || arg == "-" {
            // the first operand is the program (when `-e` gave none), the rest are files and assignments
            if programs.is_empty() {
                programs.push(arg);
            }
            break;
        }
        if arg == "--" {
            operands = true;
        } else if let Some(long) = arg.strip_prefix("--") {
            let (name, attached) = long
                .split_once('=')
                .map_or((long, None), |(n, v)| (n, Some(v)));
            let abbreviates = |full: &str, min: usize| name.len() >= min && full.starts_with(name);
            if abbreviates("file", 3) || abbreviates("exec", 3) || abbreviates("load", 3) {
                return true;
            }
            if abbreviates("include", 3) {
                if attached.or(next) != Some("inplace") {
                    return true;
                }
                k += usize::from(attached.is_none());
            } else if abbreviates("source", 3) {
                programs.extend(attached.or(next));
                k += usize::from(attached.is_none());
            }
        } else {
            for (at, c) in arg.char_indices().skip(1) {
                let rest = &arg[at + c.len_utf8()..];
                match c {
                    'f' | 'E' | 'l' => return true,
                    'i' | 'e' | 'F' | 'v' | 'W' => {
                        let value = if rest.is_empty() { next } else { Some(rest) };
                        k += usize::from(rest.is_empty());
                        match (c, value) {
                            ('i', value) if value != Some("inplace") => return true,
                            ('e', Some(text)) => programs.push(text),
                            _ => {}
                        }
                        break;
                    }
                    // the rest of the word is the value of an option like `-ofile`, `-Dfile`, `-pfile`
                    'o' | 'D' | 'p' | 'd' => break,
                    _ => {}
                }
            }
        }
    }
    programs.into_iter().any(awk_text_runs)
}

/// Whether awk program text holds `system(`, an `@`, or a `|` that is not `||` outside a string, a comment and a
/// regular expression. A `/` opens a regular expression only where an operand can start (after nothing, `(`, `,`,
/// `~`, `!`, `&`, `|`, `{`, `;`, `}` or a newline); after anything else (`n++ / 2`) it divides, so a pipe after it
/// still counts.
fn awk_text_runs(text: &str) -> bool {
    let b = text.as_bytes();
    let mut prev = b'\n';
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                prev = b'"';
                continue;
            }
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'/' if matches!(
                prev,
                b'\n' | b'(' | b',' | b'~' | b'!' | b'&' | b'|' | b'{' | b';' | b'}'
            ) =>
            {
                i += 1;
                while i < b.len() && b[i] != b'/' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                prev = b'/';
                continue;
            }
            b'|' => {
                if b.get(i + 1) != Some(&b'|') {
                    return true;
                }
                i += 2;
                prev = b'|';
                continue;
            }
            b'@' => return true,
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let after = text[i..].trim_start_matches([' ', '\t']);
                if &text[start..i] == "system" && after.starts_with('(') {
                    return true;
                }
                prev = b'a';
                continue;
            }
            _ => {}
        }
        prev = c;
        i += 1;
    }
    false
}

/// The variables `less` runs as a command on every file it opens: `value` with the mode a leading `|`, `||` or
/// `-` sets taken off, when `name` is one of them.
pub(crate) fn less_command<'a>(name: &str, value: &'a str) -> Option<&'a str> {
    matches!(name, "LESSOPEN" | "LESSCLOSE").then(|| value.trim_start_matches(['|', '-']))
}
