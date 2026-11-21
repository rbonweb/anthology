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
/// must be one of these. No word-wise or prefix inspection: anything else is opaque, so rtk asks.
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
}

/// `git -c key=value` that makes git run a command: a `!` alias, or a command-valued key whose whole value is
/// not in `HARMLESS_VALUES`.
fn git_config_runs_command(config: &str) -> bool {
    let Some((key, value)) = config.split_once('=') else {
        return false;
    };
    if key.to_ascii_lowercase().starts_with("alias.") {
        return value.trim_start().starts_with('!');
    }
    git_key_runs_command(key) && !harmless_value(value)
}

/// The program runs a command given in its arguments that the analysis does not parse: `trap 'cmd' EXIT`,
/// `fd -x cmd`, `git -c alias.x='!cmd'`, `git --config-env=core.pager=V`, `tar --to-command=cmd`, `tar -I cmd`
/// (other than exactly a compressor's name), `rsync -e cmd` (other than exactly `ssh`), `rsync --rsync-path=cmd`
/// (other than exactly `rsync`), and
/// `export GIT_PAGER=cmd` (as `runs_assigned_command`).
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
        }
        "tar" | "gtar" | "bsdtar" => {
            const COMPRESSORS: &[&str] = &[
                "gzip", "pigz", "zstd", "unzstd", "xz", "pixz", "bzip2", "pbzip2", "lbzip2", "lz4",
                "lzip", "plzip", "lzma", "lzop", "brotli", "compress",
            ];
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
                || args
                    .iter()
                    .any(|a| a.starts_with("--checkpoint-action=exec"))
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
        _ => false,
    }
}
