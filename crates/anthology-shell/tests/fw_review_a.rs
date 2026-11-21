//! SH-FW review, part A (lane SH): command-running options, the caps that drop input, and four wrappers.
//!
//! Every "runs" row is a payload of `task-SH-FW-review.md` (C-1, C-2, I-3, M-3) or a form of it that GNU tar
//! or git reads the same way (an abbreviated long option, a separate or attached value, the key set through
//! `git config` or `GIT_CONFIG_*`). The Python reference misses each of them, so Rust is stricter (D-8).
//! Every "clear" row is a negative control: an everyday form of the same program.
//!
//! The G1 helper below is fw_i.rs's: `analyze()` does not decide G1, the guard does.

use anthology_shell::{Analysis, Entry, Opaque, analyze};

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

// ------------------------------------------------------------------ 1. env -S

/// Each command is too complex: the analysis stops and the guard blocks.
#[track_caller]
fn assert_incomplete(commands: &[&str]) {
    let complete: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            let head: String = command.chars().take(80).collect();
            (!a.incomplete).then(|| format!("{head:?}...: {}", summary(&a)))
        })
        .collect();
    assert!(complete.is_empty(), "complete:\n{}", complete.join("\n"));
}

/// Some entry is marked `UnknownWrapperOption` (`marked`), or no entry is.
#[track_caller]
fn assert_marked(commands: &[&str], marked: bool) {
    let wrong: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            let got = a
                .entries
                .iter()
                .any(|e| e.opaque.contains(&Opaque::UnknownWrapperOption));
            (got != marked).then(|| format!("{command:?}: {:?}", a.entries))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "marked != {marked}:\n{}",
        wrong.join("\n")
    );
}

// ------------------------------------------------------------------ C-1. tar --checkpoint-action

#[test]
fn tar_checkpoint_action_runs_its_exec_in_every_spelling() {
    assert_runs(&[
        "tar -cf b.tar --checkpoint=1 --checkpoint-action exec='npx playwright install' a",
        "tar -cf b.tar --checkpoint=1 --checkpoint-act=exec='npx playwright install' a",
        "tar -cf b.tar --checkpoint=1 --checkpoint-a exec='npx playwright install' a",
        "tar -cf b.tar --checkpoint=1 --checkpoint-=exec='npx playwright install' a",
        "tar -cf b.tar --checkpoint=1 --checkpoint-action=exec='npx playwright install' a",
        "gtar -xf a.tar --checkpoint-action 'exec=npx playwright install'",
    ]);
    assert_clear(&[
        "tar -cf b.tar --checkpoint=1000 --checkpoint-action=dot a",
        "tar -cf b.tar --checkpoint --checkpoint-action echo a",
        "tar --checkpoint=1 -xf a.tar",
        "tar -czf backup.tgz src",
    ]);
    assert_marked(
        &[
            "tar -cf b.tar --checkpoint-action exec=\"$X\" a",
            "tar -cf b.tar --checkpoint-act=exec=\"$X\" a",
        ],
        true,
    );
    assert_marked(
        &[
            "tar -cf b.tar --checkpoint=1000 --checkpoint-action=dot a",
            "tar -cf b.tar --checkpoint --checkpoint-action echo a",
        ],
        false,
    );
}

// ------------------------------------------------------------------ C-2. git

#[test]
fn git_transport_options_run_their_command() {
    assert_runs(&[
        "git clone -u 'npx playwright install' . ../c",
        "git clone -u'npx playwright install' . ../c",
        "git clone -qu 'npx playwright install' . ../c",
        "git clone . ../c -u 'npx playwright install'",
        "git clone --upload-pack='npx playwright install' . ../c",
        "git clone --upload-pack 'npx playwright install' . ../c",
        "git clone --upl 'npx playwright install' . ../c",
        "git clone --u='npx playwright install' . ../c",
        "git -C repo clone --upload-pack='npx playwright install' . ../c",
        "git fetch --upload-pack 'npx playwright install' origin",
        "git fetch --upload-p='npx playwright install' origin",
        "git pull --upload-pack='npx playwright install' origin main",
        "git ls-remote --upload-pack='npx playwright install' .",
        "git ls-remote --exec='npx playwright install' .",
        "git fetch-pack --upload-pack='npx playwright install' . --all",
        "git fetch-pack --exec='npx playwright install' . HEAD",
        "git push --receive-pack='npx playwright install' . HEAD:refs/heads/z",
        "git push --receive-p 'npx playwright install' . HEAD:refs/heads/z",
        "git push --exec='npx playwright install' . HEAD:refs/heads/z",
        "git send-pack --receive-pack='npx playwright install' . HEAD",
        "git send-pack --exec 'npx playwright install' . HEAD",
        "git archive --remote=. --exec='npx playwright install' HEAD",
        "git archive --exec 'npx playwright install' --remote=. HEAD",
    ]);
    assert_clear(&[
        "git clone https://github.com/a/b.git",
        "git clone -b main --depth 1 https://github.com/a/b.git",
        "git clone -bupstream https://github.com/a/b.git",
        "git fetch origin",
        "git fetch -u origin main:main",
        "git pull --rebase origin main",
        "git push -u origin HEAD",
        "git push --force-with-lease origin HEAD",
        "git add -u",
        "git ls-remote --tags origin",
        "git archive --format=tar.gz -o out.tgz HEAD",
    ]);
    assert_marked(
        &[
            "git clone -u \"$X\" . ../c",
            "git push --receive-pack=\"$X\" . HEAD",
            "git archive --remote=. --exec \"$X\" HEAD",
        ],
        true,
    );
    assert_marked(
        &[
            "git clone -b main https://github.com/a/b.git",
            "git clone -bupstream https://github.com/a/b.git",
            "git push -u origin HEAD",
            "git add -u",
            "git fetch -u origin main:main",
            "git archive --format=tar HEAD",
        ],
        false,
    );
}

#[test]
fn git_keys_that_run_a_command() {
    assert_runs(&[
        "git -c remote.o.url=. -c remote.o.uploadpack='npx playwright install' fetch o",
        "git -c remote.o.receivepack='npx playwright install' push o HEAD",
        "git -c remote.o.receivePack='npx playwright install' push o HEAD",
        "git -c difftool.z.cmd='npx playwright install' difftool --cached -y --tool=z",
        "git -c mergetool.z.cmd='npx playwright install' mergetool --tool=z",
        "git config difftool.z.cmd 'npx playwright install' && git difftool -y --tool=z",
        "git config remote.origin.uploadpack 'npx playwright install' && git fetch",
        "git config --global mergetool.z.cmd 'npx playwright install'",
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=remote.o.uploadpack GIT_CONFIG_VALUE_0='npx playwright install' git fetch o",
        "GIT_CONFIG_PARAMETERS=\"'mergetool.z.cmd'='npx playwright install'\" git mergetool --tool=z",
    ]);
    assert_clear(&[
        "git -c difftool.prompt=false difftool",
        "git config remote.origin.url git@github.com:a/b.git",
        "git config --global mergetool.keepBackup false",
        "git -c remote.origin.fetch='+refs/heads/*:refs/remotes/origin/*' fetch",
    ]);
    assert_marked(
        &[
            "git -c remote.o.uploadpack=\"$X\" fetch o",
            "git -c difftool.z.cmd=\"$X\" difftool -y --tool=z",
            "git --config-env=mergetool.z.cmd=X mergetool",
        ],
        true,
    );
    assert_marked(
        &[
            "git -c difftool.prompt=false difftool",
            "git -c remote.origin.url=git@github.com:a/b.git fetch origin",
        ],
        false,
    );
}

#[test]
fn git_ext_urls_run_their_command() {
    assert_runs(&[
        "git -c protocol.ext.allow=always ls-remote 'ext::npx playwright install'",
        "git -c protocol.ext.allow=always clone 'ext::npx playwright install' c",
        "git -c protocol.ext.allow=always clone 'ext::sh -c npx% playwright% install' c",
        "git -c protocol.ext.allow=always -c remote.o.url='ext::npx playwright install' fetch o",
        "git config remote.o.url 'ext::npx playwright install' && git fetch o",
        "git remote add o 'ext::npx playwright install' && git fetch o",
        "git -c protocol.ext.allow=always -c url.'ext::npx playwright install'.insteadOf=zz: ls-remote zz:x",
    ]);
    assert_clear(&[
        "git remote add origin https://github.com/a/b.git",
        "git -c url.git@github.com:.insteadOf=https://github.com/ fetch",
        "git ls-remote origin",
    ]);
    assert_marked(
        &[
            "git ls-remote \"ext::$X\"",
            "git -c remote.o.url=\"ext::$X\" fetch o",
        ],
        true,
    );
    assert_marked(
        &[
            "git remote add origin https://github.com/a/b.git",
            "git -c url.git@github.com:.insteadOf=https://github.com/ fetch",
        ],
        false,
    );
}

// ------------------------------------------------------------------ I-3. caps that drop input

#[test]
fn a_cap_that_drops_input_is_too_complex() {
    let inputs = " true".repeat(64);
    let pkgs: String = (0..300).map(|k| format!(" p{k}")).collect();
    let nine = " a".repeat(9);
    let eight = " b".repeat(8);
    // SH-FW re-review F-2: GNU parallel's inputs and combinations past MAX_FED are read, not too complex
    assert_runs(&[&format!("parallel :::{inputs} 'npx playwright install'")]);
    assert_clear(&[&format!("parallel echo {{1}} {{2}} :::{nine} :::{eight}")]);
    assert_incomplete(&[
        // more than MAX_ALTERNATES alternates
        &format!(
            "timeout 5 mytool{} npx playwright install",
            " sh".repeat(70)
        ),
        // an alternate cut at MAX_ALTERNATE_WORDS before the main program
        &format!("sudo -uroot apt-get install -y{pkgs} chromium"),
        &format!("timeout 5 mytool npx{pkgs}"),
    ]);
    // the main program's own words are never cut, and the caps hold everyday forms
    assert_runs(&[
        &format!("sudo apt-get install -y{pkgs} chromium"),
        &format!(
            "parallel :::{} 'npx playwright install'",
            " true".repeat(63)
        ),
        &format!(
            "timeout 5 mytool{} npx playwright install",
            " sh".repeat(60)
        ),
    ]);
    assert_clear(&[
        "parallel gzip ::: a b c",
        "parallel echo {1} {2} ::: a b ::: c d",
        &format!("parallel echo {{1}} {{2}} :::{} :::{eight}", " a".repeat(8)),
        &format!("rm -f{pkgs}"),
        &format!("npx eslint{pkgs}"),
        &format!("sudo npm i -D{pkgs}"),
        // a cut alternate before the main program that reads as it drops nothing
        &format!("sudo -u x timeout 5 nice npx eslint{pkgs}"),
        &format!("watch -n1 npx eslint{pkgs}"),
        &format!("doppler run -- npx eslint{pkgs}"),
        &format!("{}ls", "sudo env nohup ".repeat(21)),
    ]);
}

// ------------------------------------------------------------------ M-3. wrappers

#[test]
fn more_wrappers_run_the_command_after_them() {
    assert_runs(&[
        "gosu root npx playwright install",
        "gosu app:app npx playwright install",
        "su-exec root npx playwright install",
        "chronic npx playwright install",
        "torsocks npx playwright install",
        "torsocks -u me -p pw npx playwright install",
        "nodenv exec npx playwright install",
        "micromamba run -n x npx playwright install",
        "micromamba run -n x playwright install",
        "micromamba run --name x pip install playwright",
    ]);
    assert_clear(&[
        "gosu root id",
        "su-exec app ls -la",
        "chronic make",
        "torsocks curl https://example.com",
        "nodenv exec node -v",
        "micromamba run -n x pytest",
        "micromamba create -n x python",
    ]);
    assert_marked(
        &[
            "gosu root ls",
            "su-exec app ls",
            "chronic make",
            "nodenv exec node -v",
            "micromamba run -n x pytest",
        ],
        false,
    );
}
