//! Rust-only rules for `Opaque` (SH2 review rounds 2–3): commands the analysis may have misread, and wrapper
//! options known to be harmless. Python has none of these; they never change an entry's parity fields.

use crate::shell_args::prog_name;

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
pub(crate) fn wrapper_bools(name: &str) -> &'static [&'static str] {
    WRAPPER_BOOL_FLAGS
        .iter()
        .find(|(w, _)| *w == name)
        .map_or(&[], |(_, bools)| bools)
}

/// `token` is an option of wrapper `name` known not to take the next word: listed, a `--name=value`, or
/// stdbuf's attached `-oL`/`-i0`/`-e0` (a detached `-o L` takes the next word, which Python reads as the
/// program).
pub(crate) fn wrapper_known(name: &str, token: &str) -> bool {
    wrapper_bools(name).contains(&token)
        || (token.starts_with("--") && token.contains('='))
        || (name == "stdbuf"
            && token.len() > 2
            && matches!(token.as_bytes()[1], b'i' | b'o' | b'e'))
}

/// An `env` option that holds its script attached (`-S'...'`, `--split-string=...`): Python does not see it
/// and reads the next word as the program.
pub(crate) fn env_attached_split(token: &str) -> bool {
    token.starts_with("--split-string=") || (token.starts_with("-S") && token.len() > 2)
}

/// Assignments whose value the shell or a program later runs as a command (`GIT_PAGER='...' git log`,
/// `BASH_ENV=f bash -c ...`).
pub(crate) fn runs_assigned_command(word: &str) -> bool {
    const VARS: &[&str] = &[
        "GIT_SSH_COMMAND",
        "GIT_SSH",
        "GIT_PAGER",
        "GIT_EDITOR",
        "GIT_SEQUENCE_EDITOR",
        "GIT_ASKPASS",
        "GIT_EXTERNAL_DIFF",
        "GIT_PROXY_COMMAND",
        "SSH_ASKPASS",
        "PAGER",
        "MANPAGER",
        "EDITOR",
        "VISUAL",
        "BASH_ENV",
        "ENV",
        "PROMPT_COMMAND",
        "LD_PRELOAD",
        "DYLD_INSERT_LIBRARIES",
        "RSYNC_RSH",
    ];
    word.split_once('=')
        .is_some_and(|(name, value)| !value.is_empty() && VARS.contains(&name))
}

/// The first word of an option's command value, as a program name (`"ssh -p 22"` → `ssh`).
fn value_program(value: &str) -> String {
    prog_name(value.split_whitespace().next().unwrap_or(""))
}

/// The values of option `long` (`--long=v` or `--long v`) and of each short option in `shorts` (`-x v`).
fn option_values<'a>(args: &'a [String], long: &[&str], shorts: &[&str]) -> Vec<&'a str> {
    let mut values = Vec::new();
    for (k, arg) in args.iter().enumerate() {
        let next = args.get(k + 1).map(String::as_str);
        if long.contains(&arg.as_str()) || shorts.contains(&arg.as_str()) {
            values.extend(next);
        } else if let Some((name, value)) = arg.split_once('=')
            && long.contains(&name)
        {
            values.push(value);
        }
    }
    values
}

/// `git -c key=value` keys whose value git runs as a command.
fn git_config_runs_command(config: &str) -> bool {
    let Some((key, value)) = config.split_once('=') else {
        return false;
    };
    let key = key.to_ascii_lowercase();
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
    (key.starts_with("alias.") && value.trim_start().starts_with('!'))
        || KEYS.contains(&key.as_str())
        || key.starts_with("pager.")
        || key.starts_with("filter.")
        || key.starts_with("credential.")
        || key.ends_with(".textconv")
        || key.ends_with(".command")
        || (key.starts_with("merge.") && key.ends_with(".driver"))
}

/// The program runs a command given in its arguments that the analysis does not parse: `trap 'cmd' EXIT`,
/// `fd -x cmd`, `git -c alias.x='!cmd'`, `tar --to-command=cmd`, `tar -I cmd` (other than a compressor),
/// `rsync -e cmd` (other than ssh), `rsync --rsync-path=cmd`.
pub(crate) fn runs_option_command(prog: &str, args: &[String]) -> bool {
    match prog {
        "trap" => {
            let rest = match args.first().map(String::as_str) {
                Some("--") => &args[1..],
                _ => args,
            };
            rest.len() >= 2 && !matches!(rest[0].as_str(), "" | "-" | "-l" | "-p")
        }
        "fd" | "fdfind" => args.iter().any(|a| {
            matches!(a.as_str(), "-x" | "-X" | "--exec" | "--exec-batch")
                || a.starts_with("--exec=")
                || a.starts_with("--exec-batch=")
        }),
        "git" => option_values(args, &[], &["-c"])
            .iter()
            .any(|c| git_config_runs_command(c)),
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
                &["-F"],
            )
            .is_empty()
                || args
                    .iter()
                    .any(|a| a.starts_with("--checkpoint-action=exec"))
                || option_values(args, &["--use-compress-program"], &["-I"])
                    .iter()
                    .any(|v| !COMPRESSORS.contains(&value_program(v).as_str()))
        }
        "rsync" => {
            option_values(args, &["--rsh"], &["-e"])
                .iter()
                .any(|v| value_program(v) != "ssh")
                || option_values(args, &["--rsync-path"], &[])
                    .iter()
                    .any(|v| value_program(v) != "rsync")
        }
        _ => false,
    }
}
