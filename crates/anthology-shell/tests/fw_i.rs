//! Fix-wave I in Rust (lane SH, part I): wrappers, runners, command-running options and alternate readings.
//!
//! Every "runs" row is a payload the FW-I implementer or reviewer ran in bash (`task-FW-I-report.md`,
//! `review-fw-i.md` rounds 0 and 1); the ruling is that those payloads are binding, so where the vendored
//! Python reference (`port/difftest/oracle/guard_rules_fw.py`) misses one, Rust still has to catch it (D-8:
//! R1-I1, R1-M2, R1-M3). Every "clear" row is a negative control from the same files: a command an agent
//! writes every day, which must not read as a browser install.
//!
//! `analyze()` does not decide G1; the guard does. `g1` below is guard_rules.py's `_g1`, cut down to the
//! programs these payloads reach. A row "runs" when some entry or alternate reading (`Analysis.alternates`,
//! which the guard checks against G1 only, lead ruling Q3=A) is a G1 install and the analysis is complete.

use std::time::{Duration, Instant};

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

/// Each command is blocked: an install entry or alternate, or too complex (fail-closed families).
#[track_caller]
fn assert_blocked(commands: &[&str]) {
    let missed: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let a = analyze(command, HOME);
            (!a.incomplete && !installs(&a)).then(|| format!("{command:?}: {}", summary(&a)))
        })
        .collect();
    assert!(missed.is_empty(), "allowed:\n{}", missed.join("\n"));
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

#[test]
fn env_split_string_is_read_in_envs_own_options() {
    assert_runs(&[
        "env A=1 npx playwright install -S x",
        "env -S\"npx playwright install\"",
        "env -S'npx playwright install'",
        "env --split-string='npx playwright install'",
        "env --split-string=\"npx playwright install\"",
        "env --split-string='npx playwright install' ls",
        "env -iS\"PATH=/usr/bin:/bin npx playwright install\"",
        "env -i -S \"PATH=/usr/bin:/bin npx playwright install\"",
        "env -vS'npx playwright install'",
        "env -S 'A=1 npx playwright' install",
        "env -S\"-i PATH=/usr/bin:/bin npx playwright install\"",
        "env --split 'npx playwright install'",
        "env -u X -S 'npx playwright install'",
        "env -C /tmp -S 'npx playwright install'",
        "sudo env -S 'npx playwright install'",
    ]);
    // `-S` after env's options is the program's argument; `--` ends env's options
    assert_clear(&[
        "env -- -S 'npx playwright install'",
        "env -S 'ls -la'",
        "env -u X -S 'ls -la'",
    ]);
}

// ------------------------------------------------------------------ 2. runner and package-manager options

#[test]
fn runner_and_package_manager_value_options_are_skipped() {
    assert_runs(&[
        "npx --registry https://r.example playwright install",
        "npx --cache /tmp/c playwright install",
        "npx --userconfig ./npmrc playwright install",
        "npx --loglevel silent playwright install",
        "npm --loglevel silent exec playwright install",
        "npm exec --registry https://r.example playwright install",
        "npm --userconfig ./npmrc exec playwright install",
        "npm --cache /tmp/c x playwright install",
        "pnpm --reporter silent dlx playwright install",
        "npm --tag beta exec playwright install",
        "npm --node-options=--max-old-space-size=4096 exec playwright install",
        "npm --script-shell bash exec playwright install",
        "uvx --index https://i.example playwright install",
        "uvx --index-url https://i.example playwright install",
        "uvx --extra-index-url https://i.example playwright install",
        "uvx --default-index https://i.example playwright install",
        "uvx --find-links ./wheels playwright install",
        "uvx --with-requirements r.txt playwright install",
        "uvx --with-editable . playwright install",
        "uvx --cache-dir /tmp/c playwright install",
        "uvx --config-file uv.toml playwright install",
    ]);
}

#[test]
fn runner_forms_reach_the_program() {
    // review-fw-i.md round 0: "the runner forms the brief named are all G1"
    assert_runs(&[
        "npm x playwright install",
        "npm x -- playwright install",
        "npm exec -- playwright install",
        "npm exec -p playwright -- playwright install",
        "npm exec --package=playwright -- playwright install",
        "npm exec -c 'playwright install'",
        "npm exec --call='playwright install'",
        "npm exec --no -- playwright install",
        "pnpm exec playwright install",
        "pnpm dlx playwright install",
        "pnpm -F web exec playwright install",
        "pnpm --filter=web exec playwright install",
        "pnpm -C web exec playwright install",
        "pnpm -r exec playwright install",
        "yarn exec playwright install",
        "yarn dlx -p playwright playwright install",
        "yarn workspace w exec playwright install",
        "bunx --bun playwright install",
        "bun x --bun -- playwright install",
        "npx -p playwright playwright install",
        "npx -yp playwright playwright install",
        "npx --package=@playwright/test -- playwright install",
        "corepack pnpm@9 exec playwright install",
        "corepack yarn@4 dlx playwright install",
        "uvx --from playwright playwright install",
        "uvx --with pytest playwright install",
        "uvx --python 3.12 playwright install",
        "uv run --with playwright python -m playwright install",
        "uv run --no-sync -- playwright install",
        "pipx run playwright install",
        "pipx run --spec playwright playwright install",
        "pipx run --python python3.12 playwright install",
        "pipx run --no-cache playwright install",
        "poetry run python -m playwright install",
        "pdm run python -m playwright install",
        "hatch -e test run python -m playwright install",
        "conda run python -m playwright install",
        "pixi run python -m playwright install",
        // nested and clustered wrappers
        "sudo -u x timeout 5 nice npx playwright install",
        "time sudo -E env CI=1 npx playwright install",
        "sudo -nEHu root npx playwright install",
        "sudo -uroot npx playwright install",
        "sudo -uroot apt install chromium",
    ]);
}

#[test]
fn unknown_options_get_alternate_readings() {
    // Made-up options (`--foo-bar`) pin the construction, not the tables.
    assert_runs(&[
        "npx --foo-bar baz playwright install",
        "npm --foo-bar baz exec playwright install",
        "npm exec --foo-bar baz playwright install",
        "pnpm --foo-bar baz dlx playwright install",
        "yarn --foo-bar baz dlx playwright install",
        "bunx --foo-bar baz playwright install",
        "uvx --foo-bar baz playwright install",
        "sudo --foo-bar baz npx playwright install",
        "timeout --foo-bar baz 5 npx playwright install",
        "nice --foo-bar baz npx playwright install",
        "flock --foo-bar baz /tmp/l npx playwright install",
        "yarn --frozen-lockfile playwright install",
        "sudo --frob x apt-get install -y chromium",
    ]);
}

#[test]
fn a_package_managers_double_dash_ends_the_span_only_after_its_own_verb() {
    // R1-I1 (Rust stricter than the reference, D-8): npm's own end-of-options `--` after a misread value
    // option still runs the program after it. Checked with npm 10 and a local `playwright` bin.
    assert_runs(&[
        "npm exec --omit dev -- playwright install",
        "npm exec --proxy http://p:8080 -- playwright install",
        "npm exec --before 2024-01-01 -- playwright install",
        "npm exec --cafile /etc/ca.pem -- playwright install",
        "pnpm dlx --allow-build esbuild -- playwright install",
        "pnpm dlx --some-flag v -- playwright install",
        "npm x --foo-bar baz -- playwright install",
        "pnpm exec --foo-bar baz -- playwright install",
        "yarn exec --foo-bar baz -- playwright install",
        "yarn dlx --foo-bar baz -- playwright install",
        "bun x --foo-bar baz -- playwright install",
    ]);
    // a script's or test runner's arguments after `--` are not a command
    assert_clear(&[
        "npm run e2e -- playwright test",
        "npm test -- playwright install",
        "pnpm test -- playwright install",
        "npm run test -- --watch=false",
        "pnpm run e2e -- --project=chromium",
        "yarn test -- --watch",
        "bun run dev -- --port 3000",
    ]);
}

#[test]
fn pnpm_yarn_and_bun_script_arguments_after_double_dash_are_not_a_command() {
    // review-fw-i.md round 1, I3 negative controls: `pnpm run e2e -- playwright test` is allowed (the
    // reference allows every row). The main parse reads `pnpm run e2e` as running the bin `e2e`; the R1-I1
    // rule cuts the alternates at `--` for every verb but `exec`/`dlx`/`x`, so `playwright test` is not one.
    assert_clear(&[
        "pnpm run e2e -- playwright test",
        "pnpm run e2e -- playwright install",
        "pnpm e2e -- playwright test",
        "yarn run e2e -- playwright test",
        "yarn e2e -- playwright test",
        "yarn workspace w run e2e -- playwright test",
        "bun run e2e -- playwright test",
    ]);
}

#[test]
fn the_g1_helper_reads_entries_as_the_guard_does() {
    assert!(installs(&analyze("npx playwright install", HOME)));
    assert!(installs(&analyze("pip install 'playwright==1.40'", HOME)));
    assert!(installs(&analyze("brew install --cask chromium", HOME)));
    assert!(installs(&analyze("npm i -D puppeteer", HOME)));
    assert!(!installs(&analyze("npx playwright show-report", HOME)));
    assert!(!installs(&analyze("apt install ripgrep", HOME)));
    assert!(!installs(&analyze(
        "docker run img npx playwright install",
        HOME
    )));
    assert!(!installs(&analyze("ls playwright install", HOME)));
}

#[test]
fn an_alternate_marks_its_entry_and_differs_from_it() {
    let a = analyze("npx --foo-bar baz playwright install", HOME);
    assert!(!a.entries.iter().any(g1), "{}", summary(&a));
    let hit = a
        .alternates
        .iter()
        .find(|e| g1(e))
        .expect("an alternate is G1");
    assert_eq!(
        (hit.prog.as_str(), hit.args.clone()),
        ("playwright", vec!["install".to_string()])
    );
    let main = &a.entries[0];
    assert_eq!(main.prog, "baz");
    assert!(main.opaque.contains(&Opaque::UnknownWrapperOption));
    assert_eq!(hit.group, main.group);
    assert_eq!(hit.raw, main.raw);
    for alternate in &a.alternates {
        assert!(
            a.entries
                .iter()
                .all(|e| (&e.prog, &e.args) != (&alternate.prog, &alternate.args)),
            "{}",
            summary(&a)
        );
    }
    // a known reading has no alternate that differs
    let a = analyze("npx -y playwright install", HOME);
    assert!(a.alternates.is_empty(), "{}", summary(&a));
    assert!(
        a.entries.iter().all(|e| e.opaque.is_empty()),
        "{}",
        summary(&a)
    );
    // an alternate of a docker command is exempt, as the entry would be
    let a = analyze("sudo --frob x docker run img npx playwright install", HOME);
    assert!(!installs(&a), "{}", summary(&a));
}

#[test]
fn alternates_are_skipped_after_quiet_programs_and_stop_at_remote_ones() {
    assert_clear(&[
        "sudo echo playwright install",
        "rtk grep playwright install",
        "sudo rm -rf playwright install",
        "sudo ssh host npx playwright install",
        "sudo podman run img npx playwright install",
        "nohup orb run npx playwright install",
        "sudo kubectl exec pod -- npx playwright install",
        "sudo docker run img npx playwright test",
        "grep -rn 'playwright install' .",
        "npx playwright show-report",
        "npx prettier --write .",
        "npm i -D @playwright/test",
        "npm i -D playwright @playwright/test",
        "docker compose run e2e npx playwright install",
    ]);
    // a positional operand count can be misread as easily as an option: after any known wrapper (ruling)
    assert_runs(&["timeout 5 mytool npx playwright install"]);
}

#[test]
fn alternate_readings_are_capped() {
    // R1 ruling: at most 256 words per alternate, at most 64 alternates (both only for a misread command;
    // the main parse is never truncated). SH-FW review I-3: a cap that would drop input is too complex
    // (fw_review_a.rs).
    let pkgs: String = (0..300).map(|k| format!(" p{k}")).collect();
    assert_runs(&[&format!("sudo apt-get install -y{pkgs} chromium")]);
    let command = format!(
        "timeout 5 mytool{} npx playwright install",
        " sh".repeat(70)
    );
    for command in [
        format!("sudo -uroot apt-get install -y{pkgs} chromium"),
        command,
        format!("timeout 5 mytool npx{pkgs}"),
    ] {
        let a = analyze(&command, HOME);
        assert!(a.incomplete, "{}", summary(&a));
    }
    let a = analyze(&format!("timeout 5 mytool npx{}", &pkgs[..200]), HOME);
    assert!(!a.incomplete);
    assert!(!a.alternates.is_empty());
}

// ------------------------------------------------------------------ 3. xargs and parallel

#[test]
fn xargs_runs_its_literal_stdin() {
    assert_runs(&[
        "echo playwright install | xargs npx",
        "echo install | xargs npx playwright",
        "xargs npx <<< 'playwright install'",
        "xargs -a /dev/stdin npx <<< 'playwright install'",
        "echo 'npx playwright install' | xargs -I{} sh -c '{}'",
        "echo playwright install | xargs -I{} sh -c 'npx {}'",
        "echo x | xargs --replace npx playwright install",
        "echo x | xargs -i npx playwright install",
        "echo x | xargs -J % npx playwright install",
        "echo install | xargs -I {} npx playwright {}",
        "echo install | xargs -I% npx playwright %",
        "echo install | xargs -n1 -I{} npx playwright {}",
        "echo install | xargs -I {} -t npx playwright {}",
        "echo install | xargs -i npx playwright {}",
        "echo install | xargs -t -i npx playwright {}",
        "printf 'playwright install\\n' | xargs npx",
        "xargs npx <<EOF\nplaywright install\nEOF",
        // I1 (round 1): `--replace` means `{}`; short clusters are read letter by letter
        "echo install | xargs --replace npx playwright {}",
        "echo install | xargs -tI{} npx playwright {}",
        "echo install | xargs -tI {} npx playwright {}",
        "echo install | xargs -0tI{} npx playwright {}",
        "echo install | xargs -ti npx playwright {}",
        "echo install | xargs -iX npx playwright X",
        "echo install | xargs --replace=@ npx playwright @",
        "echo playwright | xargs --replace npx {} install",
        // each xargs layer counts once per command
        "echo playwright install | xargs echo | xargs npx",
        // accepted over-block: xargs input is split on whitespace even with -0 / -d (fails closed)
        "echo 'playwright install' | xargs -0 npx",
    ]);
    assert_clear(&[
        // with a replace string, each line is ONE argument (argc=1: npx gets "playwright install")
        "echo 'playwright install' | xargs --replace npx {}",
        "echo a.log b.log | xargs --replace echo {}",
        "echo a b | xargs -tI{} mkdir -p out/{}",
        "git diff --name-only | xargs npx eslint",
        "echo src test | xargs npx prettier --check",
    ]);
}

#[test]
fn parallel_runs_its_template_and_inputs() {
    assert_runs(&[
        "parallel ::: 'npx playwright install'",
        "parallel npx playwright ::: install",
        "parallel -j1 npx playwright ::: install",
        "parallel 'npx playwright {}' ::: install",
        "echo 'npx playwright install' | parallel",
        "parallel -j2 ::: 'npx playwright install'",
        "parallel --jobs 2 'npx playwright {}' ::: install",
        "parallel -k npx playwright ::: install",
        "parallel --bar npx playwright ::: install",
        "parallel npx playwright {} ::: install",
        // M3 (round 1): other replacement strings and positional `{N}` over the product of the sources
        "parallel -I@@ npx playwright @@ ::: install",
        "parallel npx {1} {2} ::: playwright ::: install",
    ]);
    // with `:::`, parallel ignores stdin
    assert_clear(&[
        "echo 'npx playwright install' | parallel ::: x",
        "parallel gzip ::: a b c",
    ]);
}

// ------------------------------------------------------------------ 4. wrappers and runners

#[test]
fn wrappers_run_the_command_after_them() {
    assert_runs(&[
        "builtin eval 'npx playwright install'",
        "su -c 'npx playwright install' root",
        "su root -c 'npx playwright install'",
        "su - root -c 'npx playwright install'",
        "su -lc 'npx playwright install'",
        "su --command='npx playwright install'",
        "su --session-command='npx playwright install'",
        "runuser -u root -- npx playwright install",
        "runuser -c 'npx playwright install' root",
        "runuser root -c 'npx playwright install'",
        "sg root 'npx playwright install'",
        "sg root -c 'npx playwright install'",
        "script -qc 'npx playwright install' /dev/null",
        "script -q -c 'npx playwright install' /dev/null",
        "script --command 'npx playwright install' /dev/null",
        "script -q /dev/null npx playwright install",
        "flock /tmp/l npx playwright install",
        "flock -x /tmp/l npx playwright install",
        "flock -w 5 /tmp/l npx playwright install",
        "flock /tmp/l -c 'npx playwright install'",
        "flock -n /tmp/l --command 'npx playwright install'",
        "busybox sh -c 'npx playwright install'",
        "busybox env npx playwright install",
        "busybox timeout 5 npx playwright install",
        "chroot / npx playwright install",
        "chroot --userspec=0:0 / npx playwright install",
        "nsenter -a -t 1 npx playwright install",
        "nsenter --target 1 --mount npx playwright install",
        "unshare -r npx playwright install",
        "unshare --map-root-user npx playwright install",
        "unshare --fork --pid -- npx playwright install",
        "taskset -c 0 npx playwright install",
        "taskset 1 npx playwright install",
        "chrt -o 0 npx playwright install",
        "setpriv --reuid 0 npx playwright install",
        "prlimit --nofile=1024 npx playwright install",
        "fakeroot -- npx playwright install",
        "strace -f -o /dev/null npx playwright install",
        "dbus-run-session -- npx playwright install",
        "setarch \"$(uname -m)\" -R npx playwright install",
        "stdbuf -o L npx playwright install",
        "stdbuf -oL npx playwright install",
        "stdbuf --output=L npx playwright install",
        "arch -arch arm64 npx playwright install",
        "sudo --close-from 3 npx playwright install",
        "entr -n npx playwright install",
        "entr -n -s 'npx playwright install'",
        "watch 'npx\tplaywright\tinstall'",
        "watch -n1 'npx playwright install;true'",
        "watch -x npx playwright install",
        "watch npx playwright install",
        "watch -- npx playwright install",
        "timeout 5 npx playwright install",
        "timeout -k 5 10 npx playwright install",
        "gtimeout 5 npx playwright install",
        "nice npx playwright install",
        "nohup npx playwright install",
        "command -p npx playwright install",
        "command -- npx playwright install",
        "exec -- npx playwright install",
        "time -p npx playwright install",
        "setsid -w npx playwright install",
        "setsid -- npx playwright install",
        "sudo -u x -i -- npx playwright install",
        // M8 (round 1): every listed wrapper has a row
        "dumb-init npx playwright install",
        "tini -- npx playwright install",
        "pkexec npx playwright install",
        "cross-env CI=1 npx playwright install",
        "dotenv -- npx playwright install",
        "env-cmd -f .env npx playwright install",
        // I3 (round 1): more wrappers
        "systemd-run --user npx playwright install",
        "systemd-run --user --scope npx playwright install",
        "systemd-run -p CPUQuota=50% --user npx playwright install",
        "dbus-launch npx playwright install",
        "ssh-agent npx playwright install",
        "ltrace -f -o /dev/null npx playwright install",
        "valgrind npx playwright install",
        "chpst -u x npx playwright install",
        "cpulimit -l 50 -- npx playwright install",
        "gdb --args npx playwright install",
        "perf stat npx playwright install",
        "perf record npx playwright install",
        "perf trace npx playwright install",
        // M2 (round 1): busybox's own shell names
        "busybox ash -c 'npx playwright install'",
        "mksh -c 'npx playwright install'",
        "ash -c 'npx playwright install'",
        "hush -c 'npx playwright install'",
        "yash -c 'npx playwright install'",
        "csh -c 'npx playwright install'",
        "tcsh -c 'npx playwright install'",
    ]);
    // `sudo -s|-i 'one word'`: sudo escapes the spaces and execs one word, which is not found (verified)
    assert_clear(&[
        "sudo -s 'npx playwright install'",
        "sudo -i 'npx playwright install'",
        "flock /tmp/build.lock make",
        "su -c 'systemctl restart x' root",
        "timeout --foreground 5 make",
        "watch -n1 date",
        "busybox --help",
        "nice -n10 make",
    ]);
}

#[test]
fn script_options_and_subcommand_wrappers_run_commands() {
    assert_runs(&[
        "nodemon -x 'npx playwright install'",
        "nodemon --exec 'npx playwright install'",
        "nix-shell --run 'npx playwright install'",
        "nix-shell -p nodejs --run 'npx playwright install'",
        "nix-shell --command 'npx playwright install'",
        "nix develop -c npx playwright install",
        "nix develop --command npx playwright install",
        "nix shell nixpkgs#nodejs -c npx playwright install",
        "mise exec -- npx playwright install",
        "mise x node@20 -- npx playwright install",
        "rtx exec -- npx playwright install",
        "asdf exec npx playwright install",
        "direnv exec . npx playwright install",
        "devbox run npx playwright install",
        "volta run npx playwright install",
        "fnm exec -- npx playwright install",
        "nvm exec 20 -- npx playwright install",
        "pixi run npx playwright install",
        "pixi exec npx playwright install",
        "concurrently 'npm run dev' 'npx playwright install'",
        "hyperfine 'npx playwright install'",
        "su -c 'echo a' -c 'npx playwright install'",
        // R1-M3 (Rust stricter, D-8): the secret runners' `--command` and `--`
        "doppler run --command 'npx playwright install'",
        "doppler run -- npx playwright install",
        "doppler run -- -- npx playwright install",
        "op run -- npx playwright install",
        "op run --env-file=.env -- npx playwright install",
        "dotenvx run -- npx playwright install",
        "dotenvx run -f .env -- npx playwright install",
        "infisical run -- npx playwright install",
        "infisical run --command 'npx playwright install'",
        // the first `-c`-style scan of each program counts once; a nested layer is still peeled
        "sg grp watch 'npx playwright install'",
        "watch sg grp 'npx playwright install'",
        "watch -n1 nice 'npx playwright install'",
    ]);
    assert_clear(&[
        "doppler run -- npm test",
        "nodemon --exec 'node server.js'",
        "nix develop -c cargo build",
        "concurrently 'npm run dev' 'npm run api'",
        "hyperfine 'sleep 0.1'",
        "mise exec -- node -v",
        "direnv exec . make",
    ]);
}

// ------------------------------------------------------------------ 5. settings and arguments that run commands

#[test]
fn git_settings_and_subcommands_run_commands() {
    assert_blocked(&[
        // an alias from GIT_CONFIG_* is too complex (fail-closed families)
        "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=alias.x GIT_CONFIG_VALUE_0='!npx playwright install' git x",
    ]);
    assert_runs(&[
        "git -c alias.x='!npx playwright install' x",
        "git -c 'alias.x=!npx playwright install' x",
        "git -c core.sshCommand='npx playwright install' ls-remote ssh://h/x",
        "GIT_SSH_COMMAND='npx playwright install' git ls-remote ssh://h/x",
        "git -c core.fsmonitor='npx playwright install' status",
        "git -c core.pager='npx playwright install' log",
        "GIT_PAGER='npx playwright install' git log",
        "GIT_CONFIG_PARAMETERS=\"'alias.x=!npx playwright install'\" git x",
        "GIT_CONFIG_PARAMETERS=\"'alias.x'='!npx playwright install'\" git x",
        "git config alias.pw '!npx playwright install' && git pw",
        "GIT_EDITOR='npx playwright install' git commit --allow-empty",
        "git -c core.editor='npx playwright install' commit",
        // M8 (round 1): the other variables and keys
        "GIT_SEQUENCE_EDITOR='npx playwright install' git rebase -i HEAD~1",
        "GIT_EXTERNAL_DIFF='npx playwright install' git diff",
        "GIT_ASKPASS='npx playwright install' git fetch",
        "GIT_SSH='npx playwright install' git fetch",
        "PAGER='npx playwright install' git log",
        "VISUAL='npx playwright install' git commit",
        "EDITOR='npx playwright install' git commit",
        "git -c pager.log='npx playwright install' log",
        "git -c merge.x.driver='npx playwright install' merge b",
        "git -c filter.x.smudge='npx playwright install' checkout .",
        "git -c sequence.editor='npx playwright install' rebase -i HEAD~1",
        "git -c diff.external='npx playwright install' diff",
        "git -c interactive.diffFilter='npx playwright install' add -p",
        "git -c core.askpass='npx playwright install' fetch",
        "git -c credential.helper='!npx playwright install' fetch",
        // I2 (round 1): git's own command-running subcommands
        "git bisect run npx playwright install",
        "git bisect run npx playwright test e2e/x.spec.ts",
        "git bisect run sh -c 'npx playwright install'",
        "git rebase -x 'npx playwright install' HEAD~1",
        "git rebase --exec 'npx playwright install' HEAD~1",
        "git rebase --exec='npx playwright install' HEAD~1",
        "git rebase -i --exec='npx playwright install' HEAD~1",
        "git rebase -x'npx playwright install' HEAD~1",
        "git rebase -x 'echo a' -x 'npx playwright install' HEAD~1",
        "git submodule foreach 'npx playwright install'",
        "git submodule --quiet foreach --recursive 'npx playwright install'",
        "git difftool -y -x 'npx playwright install' HEAD~1",
        "git difftool --extcmd='npx playwright install' HEAD~1",
        "git -C a bisect run echo; git -C b bisect run npx playwright install",
    ]);
    assert_clear(&[
        "git bisect run make test",
        "git bisect run npm test",
        "git rebase -i HEAD~3",
        "git rebase -x 'cargo test' main",
        "git rebase -x 'npm test' main",
        "git submodule foreach git pull",
        "git difftool -y",
        "git difftool -x meld",
        "git -c core.editor=true commit",
        "GIT_SSH_COMMAND='ssh -i ~/.ssh/k' git clone git@h:x",
        "git config alias.co checkout",
        "GIT_PAGER=cat git log",
    ]);
}

#[test]
fn trap_tar_rsync_and_fd_run_commands() {
    assert_runs(&[
        "trap 'npx playwright install' EXIT",
        "trap -- 'npx playwright install' EXIT",
        "trap 'npx playwright install' ERR; false",
        "tar -xf a.tar --to-command='npx playwright install'",
        "tar -xf a.tar --to-command 'npx playwright install'",
        "tar -xf a.tar --to-com='npx playwright install'",
        "tar -cf b.tar -I 'npx playwright install' f",
        "tar -cf b.tar -I'npx playwright install' f",
        "tar -cf b.tar -F 'npx playwright install' f",
        "tar -cf b.tar -F'npx playwright install' f",
        "tar -cf b.tar --use-compress-program='npx playwright install' f",
        "tar -cf b.tar --use-compress-program 'npx playwright install' f",
        "tar -cf b.tar --checkpoint=1 --checkpoint-action=exec='npx playwright install' f",
        "tar -cf b.tar --info-script='npx playwright install' f",
        "tar -cf b.tar --new-volume-script='npx playwright install' f",
        "tar -cf b.tar --rsh-command='npx playwright install' h:/f",
        "gtar -cf b.tar -I 'npx playwright install' f",
        "bsdtar -cf b.tar --use-compress-program 'npx playwright install' f",
        "rsync -e 'npx playwright install' f h:/tmp/",
        "rsync --rsh='npx playwright install' f h:/tmp/",
        "rsync --rsh 'npx playwright install' f h:/tmp/",
        "rsync -avze 'npx playwright install' f h:/tmp/",
        "RSYNC_RSH='npx playwright install' rsync f h:/tmp/",
        "fdfind hostname /etc -x npx playwright install",
        "fd . -x npx playwright install",
        "fd . --exec npx playwright install",
        "fd . -X npx playwright install",
        "fd . --exec-batch npx playwright install",
        "fd -x npx playwright install \\; .",
        "find . -exec npx playwright install \\;",
        "find . -ok npx playwright install \\;",
        // R1-M2 (Rust stricter, D-8): every fd -x group up to its `;`
        "fdfind hostname /etc -x echo hi \\; -x npx playwright install",
        "fd . --exec echo a \\; --exec npx playwright install",
    ]);
    assert_clear(&[
        "trap 'rm -rf \"$tmp\"' EXIT",
        "trap - EXIT",
        "trap '' INT",
        "tar -xf a.tar --to='npx playwright install'",
        "tar -I zstd -xf a.tar.zst",
        "tar --use-compress-program=zstd -cf a.tar.zst x",
        "rsync -avz -e 'ssh -p 2222' a h:/b",
        "fd -e ts -x prettier --check",
        "fd -x echo hi \\; npx playwright install",
    ]);
}

// ------------------------------------------------------------------ 6. fed shells

#[test]
fn shells_fed_from_substitutions_fds_and_stdin_paths_run_the_text() {
    assert_runs(&[
        "sh <(echo npx playwright install)",
        "bash <(echo npx playwright install)",
        "cat <(echo npx playwright install) | sh",
        "source <(echo npx playwright install)",
        ". <(echo npx playwright install)",
        "bash <(printf 'npx playwright install\\n')",
        "exec 3<<<'npx playwright install'; sh <&3",
        "exec 3<<<'npx playwright install'; bash <&3",
        "exec 3<<<'npx playwright install'; sh /dev/fd/3",
        "exec 3< <(echo npx playwright install); sh <&3",
        "exec 3<<EOF\nnpx playwright install\nEOF\nsh <&3",
        "sh /dev/stdin <<< 'npx playwright install'",
        "bash /dev/fd/0 <<< 'npx playwright install'",
        "bash /proc/self/fd/0 <<< 'npx playwright install'",
        // M1 (round 1)
        "bash /dev/fd/3 3<<<'npx playwright install'",
        "sh /dev/fd/3 3<<'EOF'\nnpx playwright install\nEOF",
        "source /dev/stdin <<< 'npx playwright install'",
        ". /dev/stdin <<< 'npx playwright install'",
        "exec 3<<<'npx playwright install'; source /dev/fd/3",
        // accepted over-block: any shell in the text feeds `<(...)` (ruling)
        "diff <(echo npx playwright install) x; sh run.sh",
    ]);
    assert_clear(&[
        "diff <(sort a) <(sort b)",
        "diff <(echo npx playwright install) x",
        "exec 3<<<'npx playwright install'",
        "cat /dev/stdin <<< 'npx playwright install'",
    ]);
}

// ------------------------------------------------------------------ 7. the `--` of an unknown program

#[test]
fn the_words_after_an_unknown_programs_double_dash_are_read_again() {
    assert_runs(&[
        "aws-vault exec prof -- npx playwright install",
        "lerna exec -- npx playwright install",
        "nx exec -- npx playwright install",
        "npx nx exec -- npx playwright install",
        "with-secrets -- npx playwright install",
        "mytool -- npx playwright install",
        // accepted over-blocks (round 1)
        "npx eslint -- playwright install",
        "pre-commit run -- playwright install",
    ]);
    assert_clear(&[
        "git checkout -- playwright install",
        "git diff HEAD -- playwright install",
        "git checkout -- src/app.ts",
        "git restore --staged -- .",
        "cargo test -- playwright install",
        "cargo run -- playwright install",
        "cargo run -- npx playwright install",
        "cargo test -- --nocapture",
        "go test ./... -- -v",
        "make -- test",
        "make -- npx playwright install",
        "just test -- -k foo",
        "terraform apply -- -auto-approve",
        "gh pr create -- --title x",
        "docker run --rm img -- npx playwright install",
        "docker compose exec web -- npm test",
        "kubectl exec pod -- npx playwright install",
        "rg -- playwright install",
        "grep -r -- playwright install",
        "node script.js -- playwright install",
        "bash run.sh -- playwright install",
        "npx eslint -- src",
        "mytool -- --help",
        "deno task dev -- --port 3000",
        "bazel run //app -- --port 8080",
    ]);
}

// ------------------------------------------------------------------ 8. python -m, OS installers

#[test]
fn python_module_options_and_os_installer_value_options() {
    assert_runs(&[
        "python3 -m playwright install",
        "python3 -Bm playwright install",
        "python3 -mplaywright install",
        "python3 -X dev -m playwright install",
        "python3 -W ignore -m playwright install",
        "python3 -B -m playwright install",
        "sudo apt-get -o Dpkg::Options::=--force-confnew install chromium",
        "apt-get --option x=y install chromium",
        "apt -t bookworm-backports install chromium",
        "dnf --setopt x=y install chromium",
        "yum -c /etc/y.conf install chromium",
        "snap --channel beta install chromium",
    ]);
    assert_clear(&[
        "python3 -c 'import playwright' install",
        "python3 -m pytest -k install",
        "python3 - playwright install",
        "python3 script.py -m playwright install",
        "apt-get -o x=y update",
        "sudo apt install ripgrep",
    ]);
}

// ------------------------------------------------------------------ 9. run once

#[test]
fn a_text_seen_again_is_not_analyzed_again_but_keeps_its_directory() {
    // the memo is keyed on (text, docker-exempt flag, starting cwd)
    let a = analyze("sh -c 'ls x'; cd /tmp; sh -c 'ls x'", HOME);
    let ls: Vec<Option<&str>> = a
        .entries
        .iter()
        .filter(|e| e.prog == "ls")
        .map(|e| e.cwd.as_deref())
        .collect();
    assert_eq!(ls, [None, Some("/tmp")], "{}", summary(&a));
    let a = analyze("sh -c 'cd /a'; ls; sh -c 'cd /a'; ls", HOME);
    let ls: Vec<Option<&str>> = a
        .entries
        .iter()
        .filter(|e| e.prog == "ls")
        .map(|e| e.cwd.as_deref())
        .collect();
    assert_eq!(ls, [Some("/a"), Some("/a")], "{}", summary(&a));
    // the docker-exempt copy and the host copy of one script are both analyzed
    assert_runs(&["docker run img sh -c 'npx playwright install'; sh -c 'npx playwright install'"]);
}

// ------------------------------------------------------------------ 10. cost

/// Seconds for one analysis of `text`, and whether it finished (an incomplete one is a fail-closed block).
fn timed(text: &str) -> (Duration, bool) {
    let start = Instant::now();
    let a = analyze(text, HOME);
    (start.elapsed(), !a.incomplete)
}

/// `unit` repeated to `bytes`, then `tail`.
fn shape(unit: &str, bytes: usize, tail: &str) -> String {
    let mut text = unit.repeat(bytes / unit.len());
    text.push_str(tail);
    text
}

const SHAPES: &[(&str, &str)] = &[
    // review-fw-i.md round 0 (C1) and round 1 (R1-M1), and the report's COST_SHAPES
    ("watch ", "ls"),
    ("sg x ", "ls"),
    ("parallel ", "ls"),
    ("sudo watch -n1 ", "ls"),
    ("watch 'a b' ", "ls"),
    ("watch sg x parallel ", "ls"),
    ("fd -x a ", ""),
    ("fd -x a \\; ", ""),
    ("echo 'a b' | xargs ", "xargs ls"),
    ("xargs ", "ls"),
    ("sudo npx ", "ls"),
    ("su -c x ", "ls"),
    ("env -S 'a b' ", "ls"),
    ("cd a; sh -c 'cd b'; ", "ls"),
    ("sh -c 'npx a'; ", "ls"),
    ("doppler run -- ", "ls"),
    ("x -- ", "ls"),
    ("git bisect run ", "ls"),
    ("git submodule foreach ", "ls"),
    ("parallel echo ::: a ::: b; ", "ls"),
    ("sh <(echo a); ", "ls"),
    ("exec 3<<<'a'; sh <&3; ", "ls"),
    ("trap 'a b' EXIT; ", "ls"),
    ("nix develop -c ", "ls"),
    ("gdb --args ", "ls"),
    ("systemd-run --user ", "ls"),
    ("busybox ash -c 'a'; ", "ls"),
    ("tar -I'a b' -cf x y; ", "ls"),
    ("apt-get -o x=y ", "install chromium"),
    ("git -c alias.x='!a b' ", "x"),
    ("timeout 5 mytool ", "npx playwright install"),
    ("npx --foo-bar baz ", "playwright install"),
    ("echo a | xargs -I{} sh -c '{}'; ", "ls"),
    ("rsync -e 'a b' f h:/t; ", "ls"),
];

#[test]
fn every_review_shape_is_linear_and_quick_at_100_kb() {
    // C1: <= 1 s at 100 KB (or a fail-closed budget stop within it), and 4x input < 6x time
    let mut slow = Vec::new();
    for &(unit, tail) in SHAPES {
        let (small, _) = timed(&shape(unit, 25_000, tail));
        let (large, done) = timed(&shape(unit, 100_000, tail));
        if large > Duration::from_secs(1) || large > small * 6 + Duration::from_millis(50) {
            slow.push(format!(
                "{unit:?}: 25 KB {small:?}, 100 KB {large:?} (complete: {done})"
            ));
        }
    }
    assert!(slow.is_empty(), "{}", slow.join("\n"));
}

#[test]
fn long_everyday_commands_finish_within_the_budget() {
    // the budget stops only absurd shapes (chains of more than about 64 wrappers); a long file or package
    // list after wrappers, runners, xargs, parallel, watch or fd must still be analyzed in full
    let files: String = (0..5000).map(|k| format!(" src/file{k}.ts")).collect();
    let pkgs: String = (0..300).map(|k| format!(" pkg{k}")).collect();
    for command in [
        format!("npx eslint{files}"),
        format!("sudo -u x timeout 5 nice npx eslint{files}"),
        format!("timeout 600 mytool{files}"),
        format!("npx --foo-bar baz{files}"),
        format!("sudo npm i -D{pkgs}"),
        // SH-FW review I-3: more than 64 `:::` inputs are too complex (fw_review_a.rs)
        format!("parallel gzip :::{}", &files[..886]),
        format!("echo{files} | xargs npx prettier --check"),
        format!("env -S 'npx eslint'{files}"),
        format!("watch -n1 npx eslint{files}"),
        format!("fd -e ts -x npx prettier --check{files}"),
        format!("doppler run -- npx eslint{files}"),
        format!("sudo{} ls", " sudo".repeat(63)),
        // 63 wrappers: more than 64 alternates would be too complex (SH-FW review I-3)
        format!("{}ls", "sudo env nohup ".repeat(21)),
    ] {
        let (took, done) = timed(&command);
        assert!(done, "{}...: incomplete", &command[..60]);
        assert!(
            took < Duration::from_secs(1),
            "{}...: {took:?}",
            &command[..60]
        );
    }
}

#[test]
fn long_wrapper_chains_still_show_the_install() {
    // round 0 took 32 s on the 20-word `watch` chain; it must stay G1 and quick
    for unit in [
        "watch ",
        "sg x ",
        "parallel ",
        "sudo watch -n1 ",
        "nice ",
        "sudo ",
        "timeout 5 ",
    ] {
        let command = format!("{}npx playwright install", unit.repeat(20));
        let start = Instant::now();
        assert_runs(&[&command]);
        assert!(start.elapsed() < Duration::from_secs(1), "{command}");
    }
}
