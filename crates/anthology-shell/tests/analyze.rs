//! `analyze()` (SH2): behaviour guard_rules.py's tests rely on (expectations read from the Python oracle),
//! every `Opaque` reason, the fail-closed bounds, stack use, entry-level wrapping monotonicity and a no-panic
//! property. Exact parity with Python is the difftest's job (`shell.analyze`).

use std::time::{Duration, Instant};

use anthology_shell::{
    Analysis, Entry, Limits, MAX_DEPTH, Opaque, analyze, analyze_with, expand_home, normalize_path,
};
use proptest::prelude::*;

const HOME: &str = "/home/tester";

fn progs(a: &Analysis) -> Vec<&str> {
    a.entries.iter().map(|e| e.prog.as_str()).collect()
}

fn entry<'a>(a: &'a Analysis, prog: &str) -> &'a Entry {
    a.entries
        .iter()
        .find(|e| e.prog == prog)
        .unwrap_or_else(|| panic!("no {prog} entry in {:?}", progs(a)))
}

fn has(a: &Analysis, prog: &str, reason: Opaque) -> bool {
    entry(a, prog).opaque.contains(&reason)
}

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn cd_is_tracked_and_chdir_programs_use_their_directory() {
    let a = analyze("cd ~/proj && git -C sub status; cd /tmp; ls x", HOME);
    let cwds: Vec<Option<&str>> = a.entries.iter().map(|e| e.cwd.as_deref()).collect();
    assert_eq!(
        cwds,
        [
            None,
            Some("/home/tester/proj/sub"),
            Some("/home/tester/proj"),
            Some("/tmp")
        ]
    );
    let a = analyze("cd; cd ~bob/x; cd $HOME/a/../b", HOME);
    let cwds: Vec<Option<&str>> = a.entries.iter().map(|e| e.cwd.as_deref()).collect();
    assert_eq!(cwds, [None, Some("/home/tester"), Some("/~bob/x")]);
}

#[test]
fn wrappers_and_runners_are_peeled() {
    let a = analyze(
        "sudo -u bob env FOO=1 nohup npx -y playwright@1.2 install",
        HOME,
    );
    assert_eq!(a.entries.len(), 1);
    let e = &a.entries[0];
    assert_eq!(
        (e.prog.as_str(), e.args.clone()),
        ("playwright", strings(&["install"]))
    );
    assert_eq!(e.raw.len(), 10);
}

#[test]
fn stdin_text_reaches_shells_and_interpreters() {
    let a = analyze("echo 'npx x' | sh", HOME);
    assert_eq!(progs(&a), ["echo", "sh", "x"]);
    assert_eq!(a.entries[0].group, a.entries[1].group);
    let a = analyze("cat <<EOF | python3 -\nprint(1)\nEOF", HOME);
    assert_eq!(entry(&a, "python3").code, strings(&["-", "print(1)\n"]));
}

#[test]
fn docker_shells_are_exempt_and_find_exec_runs() {
    let a = analyze("docker run img bash -c 'npx playwright install'", HOME);
    assert_eq!(progs(&a), ["docker", "playwright"]);
    assert!(a.entries.iter().all(|e| e.exempt));
    let a = analyze("find . -exec rm {} \\; -exec sh -c 'ls' \\;", HOME);
    assert_eq!(progs(&a), ["find", "rm", "sh", "ls"]);
}

#[test]
fn rtk_forms_and_eval() {
    let a = analyze(
        "rtk run -c 'npx x' ; rtk err 'npx y'; rtk proxy -- git push",
        HOME,
    );
    assert_eq!(progs(&a), ["sh", "x", "sh", "y", "git"]);
    assert_eq!(a.entries[2].args, strings(&["-c", "npx y"]));
    let a = analyze("eval 'npx x'; source ./env.sh", "");
    assert_eq!(progs(&a), ["eval", "x", "source"]);
}

#[test]
fn path_helpers_match_python() {
    assert_eq!(expand_home("~/a", HOME), Ok("/home/tester/a".to_string()));
    assert_eq!(expand_home("~bob/a", HOME), Ok("/~bob/a".to_string()));
    assert_eq!(
        expand_home("$HOMEx/${HOME}", HOME),
        Ok("$HOMEx//home/tester".to_string())
    );
    assert_eq!(expand_home("a\\b", HOME), Ok("a/b".to_string()));
    assert_eq!(
        normalize_path("//A/./B/../c", HOME, None),
        Ok("//a/c".to_string())
    );
    assert_eq!(normalize_path("///x", HOME, None), Ok("/x".to_string()));
    assert_eq!(normalize_path("../x", HOME, None), Ok("../x".to_string()));
    assert_eq!(normalize_path("x/..", HOME, None), Ok(".".to_string()));
    assert_eq!(
        normalize_path("y", HOME, Some("/a")),
        Ok("/a/y".to_string())
    );
    assert_eq!(normalize_path("y", HOME, Some("")), Ok("y".to_string()));
    assert_eq!(normalize_path("$HOME", "", None), Ok(String::new()));
    assert_eq!(
        normalize_path("/\u{130}", HOME, None),
        Ok("/i\u{307}".to_string())
    );
}

// ------------------------------------------------------------------ every Opaque reason

#[test]
fn opaque_dynamic_command_word() {
    assert!(has(
        &analyze("$(echo rm) -rf x", HOME),
        "-rf",
        Opaque::DynamicCommandWord
    ));
    assert!(has(
        &analyze("$cmd x", HOME),
        "$cmd",
        Opaque::DynamicCommandWord
    ));
    assert!(has(
        &analyze("./*.sh", HOME),
        "*.sh",
        Opaque::DynamicCommandWord
    ));
    assert!(!has(
        &analyze("ls x", HOME),
        "ls",
        Opaque::DynamicCommandWord
    ));
}

#[test]
fn opaque_shell_script_from_expansion() {
    assert!(has(
        &analyze("sh -c \"$(curl -s x)\"", HOME),
        "sh",
        Opaque::ShellScriptFromExpansion
    ));
    assert!(has(
        &analyze("bash \"$f\"", HOME),
        "bash",
        Opaque::ShellScriptFromExpansion
    ));
    assert!(has(
        &analyze("eval \"$x\"", HOME),
        "eval",
        Opaque::ShellScriptFromExpansion
    ));
    assert!(!has(
        &analyze("sh -c 'ls'", HOME),
        "sh",
        Opaque::ShellScriptFromExpansion
    ));
}

#[test]
fn opaque_eval_and_source() {
    assert!(has(&analyze("eval ls", HOME), "eval", Opaque::Eval));
    assert!(has(
        &analyze("source ./x.sh", HOME),
        "source",
        Opaque::Source
    ));
    assert!(has(&analyze(". ./env", HOME), ".", Opaque::Source));
    assert!(!has(&analyze("source", HOME), "source", Opaque::Source));
}

#[test]
fn opaque_depth_cap() {
    let a = analyze(&format!("{}ls", "eval ".repeat(MAX_DEPTH + 3)), HOME);
    assert!(
        a.entries
            .iter()
            .any(|e| e.opaque.contains(&Opaque::DepthCap))
    );
    assert!(!a.incomplete);
    let nested = format!(
        "echo {}ls{}",
        "$(".repeat(MAX_DEPTH + 2),
        ")".repeat(MAX_DEPTH + 2)
    );
    let a = analyze(&nested, HOME);
    assert!(
        a.entries
            .iter()
            .any(|e| e.opaque.contains(&Opaque::DepthCap))
    );
    assert!(
        analyze("echo $(ls)", HOME)
            .entries
            .iter()
            .all(|e| e.opaque.is_empty())
    );
}

#[test]
fn opaque_stdin_shell_unreadable() {
    // fail closed (controller ruling): a producer that is not literal text before a shell is too complex
    assert!(analyze("curl -s x | sh", HOME).incomplete);
    assert!(has(
        &analyze("sh", HOME),
        "sh",
        Opaque::StdinShellUnreadable
    ));
    assert!(!has(
        &analyze("echo ls | sh", HOME),
        "sh",
        Opaque::StdinShellUnreadable
    ));
}

#[test]
fn opaque_interpreter_code() {
    assert!(has(
        &analyze("python3 -c 'x'", HOME),
        "python3",
        Opaque::InterpreterCode
    ));
    assert!(has(
        &analyze("node x.js", HOME),
        "node",
        Opaque::InterpreterCode
    ));
}

#[test]
fn opaque_unknown_wrapper_option() {
    assert!(has(
        &analyze("sudo --frob npx x", HOME),
        "x",
        Opaque::UnknownWrapperOption
    ));
    assert!(has(
        &analyze("nohup --x ls", HOME),
        "ls",
        Opaque::UnknownWrapperOption
    ));
    assert!(!has(
        &analyze("sudo -u bob ls", HOME),
        "ls",
        Opaque::UnknownWrapperOption
    ));
}

/// Some entry of `command` has `reason`.
fn any_has(command: &str, reason: Opaque) -> bool {
    analyze(command, HOME)
        .entries
        .iter()
        .any(|e| e.opaque.contains(&reason))
}

/// No entry of `command` is opaque.
fn clear(command: &str) -> bool {
    analyze(command, HOME)
        .entries
        .iter()
        .all(|e| e.opaque.is_empty())
}

#[test]
fn opaque_marks_the_forms_python_misreads() {
    // Each runs `npx playwright install` where the analysis may misread or not parse it (fix-wave I reads
    // many of them now, and `npx --registry X` is a known value option); the entry is still marked so
    // rtk's gate asks.
    let unknown = [
        "uv --color always run playwright install",
        "rtk --frob x npx playwright install",
        "rtk proxy --frob x npx playwright install",
        "rtk err --frob x npx playwright install",
        "unshare -r npx playwright install",
        "ssh host npx playwright install",
        // review round 2
        "trap 'npx playwright install' EXIT",
        "trap -- 'npx playwright install' EXIT",
        "fd . -x npx playwright install",
        "fd -e js --exec-batch npx playwright install",
        "git -c alias.x='!npx playwright install' x",
        "git -c core.pager='npx playwright install' log",
        "git -c core.sshCommand='npx playwright install' fetch",
        "tar --to-command='npx playwright install' -xf a.tar",
        "tar -I 'npx playwright install' -xf a.tar",
        "tar --checkpoint=1 --checkpoint-action=exec='npx playwright install' -cf a.tar x",
        "rsync -e 'npx playwright install' a b:c",
        "rsync --rsync-path='npx playwright install' a b:c",
        "GIT_PAGER='npx playwright install' git log",
        "BASH_ENV=./x.sh bash -c ls",
        // review round 3
        "rsync -avze 'npx playwright install' a b:c",
        "rsync -e'npx playwright install' a b:c",
        "fd -Hx npx playwright install",
        "tar -cI 'npx playwright install' -f a.tar x",
        "GIT_CONFIG_PARAMETERS=\"'core.pager'='npx playwright install'\" git log",
        "git --config-env=core.pager=X log",
        "export GIT_PAGER='npx playwright install'",
        // review round 4
        "GIT_PAGER='cat && npx playwright install' git log",
        "EDITOR='true || npx playwright install' git commit",
        "git -c core.pager='cat && npx playwright install' log",
        "GIT_SSH_COMMAND='ssh -o ProxyCommand=\"npx playwright install\"' git fetch",
        "GIT_SSH_COMMAND='ssh -F ./cfg' git fetch",
        "rsync -e 'ssh -o ProxyCommand=npx' a b:c",
        // review round 5
        "git -c core.pager='git -c alias.x=!npx x playwright install' log",
    ];
    let missed: Vec<&str> = unknown
        .into_iter()
        // `git --config-env` is too complex now (fail-closed families): blocked either way
        .filter(|command| {
            !any_has(command, Opaque::UnknownWrapperOption) && !analyze(command, HOME).incomplete
        })
        .collect();
    assert!(missed.is_empty(), "{missed:#?}");
    // fix-wave I reads these (known value options, wrapper operands and script options): the command is
    // an entry of its own
    let missed: Vec<&str> = [
        "npm --loglevel silent exec playwright install",
        "builtin eval 'npx playwright install'",
        "su -c 'npx playwright install' user",
        "script -qc 'npx playwright install' /dev/null",
        "flock /tmp/l npx playwright install",
        "busybox sh -c 'npx playwright install'",
        "chroot / npx playwright install",
        "nsenter -t 1 npx playwright install",
        "stdbuf -o L npx playwright install",
    ]
    .into_iter()
    .filter(|command| {
        !analyze(command, HOME)
            .entries
            .iter()
            .any(|e| e.prog == "playwright" && e.args == ["install"])
    })
    .collect();
    assert!(missed.is_empty(), "{missed:#?}");
    let missed: Vec<&str> = [
        "echo playwright install | xargs npx",
        "echo 'npx playwright install' | xargs -I{} sh -c '{}'",
        "parallel ::: 'npx playwright install'",
        "npx",
    ]
    .into_iter()
    .filter(|command| !any_has(command, Opaque::DynamicCommandWord))
    .collect();
    assert!(missed.is_empty(), "{missed:#?}");
    assert!(has(
        &analyze("bash ./install.sh", HOME),
        "bash",
        Opaque::Source
    ));
    assert!(has(&analyze("sh install.sh", HOME), "sh", Opaque::Source));
    // fail closed (controller ruling): curl's output before a shell is too complex
    assert!(analyze("curl -s x | bash -s -- --version 1.2.3", HOME).incomplete);
    for command in [
        "sh <(echo npx playwright install)",
        "cat <(echo npx playwright install) | sh",
        "exec 3<<<'npx playwright install'; sh <&3",
    ] {
        assert!(
            analyze(command, HOME)
                .entries
                .iter()
                .any(|e| e.prog == "sh" && !e.opaque.is_empty()),
            "{command}"
        );
    }
}

#[test]
fn only_exactly_listed_command_values_are_exempt() {
    // Controller ruling: a command value is exempt only when the whole value is in a small literal list, so
    // these everyday forms ask (an extra ask is fine; a hole is not).
    for command in [
        "GIT_SEQUENCE_EDITOR=: git rebase -i HEAD~3",
        "GIT_SSH_COMMAND='ssh -i key' git fetch",
        "git -c filter.lfs.smudge='git-lfs smudge -- %f' -c filter.lfs.required=true clone x",
        "git -c core.hooksPath=/dev/null commit -m x",
        "git -c credential.helper='cache --timeout=3600' push",
        "GIT_SSH_COMMAND='ssh -o StrictHostKeyChecking=no' git fetch",
        "rsync -e 'ssh -p 2222' a b:c",
        "rsync -avz -e 'ssh -p 22' a b:c",
        "tar -I 'zstd -19 -T0' -cf a.tar.zst x",
    ] {
        assert!(any_has(command, Opaque::UnknownWrapperOption), "{command}");
    }
    for command in [
        "GIT_PAGER=cat git log",
        "GIT_PAGER='less -R' git log",
        "EDITOR=nano git commit",
        "git -c core.pager=less log",
        "git -c credential.helper=osxkeychain push",
        "GIT_SSH_COMMAND=ssh git fetch",
        "rsync -e ssh a b:c",
        "tar -I zstd -xf a.tar.zst",
    ] {
        assert!(clear(command), "{command}");
    }
}

#[test]
fn common_forms_stay_clear() {
    // rtk asks on any reason: the everyday forms must not get one
    for command in [
        "npx -y playwright install",
        "npx --yes create-next-app@latest app",
        "npm -s run build",
        "npm --silent exec tsc",
        "pnpm -r --filter web build",
        "yarn workspace web build",
        "uv run --frozen pytest",
        "uvx --from ruff ruff check",
        "rtk -v git status",
        "rtk -vv --skip-env npm test",
        "rtk proxy --skip-env ls",
        "rtk run -c 'ls -la'",
        "rtk err --shell bash cargo test",
        "sudo --user=root ls",
        "env -S 'ls -la'",
        "env -u X -S 'ls -la'",
        "[ -f x ] && ls",
        "[[ -n $x ]] || ls",
        "sh -c 'ls'",
        "cd /tmp && git -C repo status",
        "A=1 B=2",
        // review round 2
        "sudo -E npm test",
        "sudo -n ls",
        "env -i PATH=/bin ls",
        "time -p make",
        "timeout --foreground 5 make",
        "stdbuf -oL make",
        "nohup make",
        "arch -arm64 make",
        "npm --workspaces run build",
        "npm -ws test",
        "pnpm --parallel -r dev",
        "pnpm --stream -r build",
        "bun --watch run dev",
        "bun install",
        "bun test",
        "bun add zod",
        "uv run -m pytest",
        "bash --version",
        "trap - EXIT",
        "trap '' INT",
        "fd -e rs main",
        "git -c user.name=x commit -m y",
        "git commit -c HEAD",
        "tar -I zstd -xf a.tar.zst",
        "tar -xzf a.tgz -C out",
        "rsync -a --rsync-path=rsync a b:c",
        "PAGER= git log",
        "GIT_DIR=.git git status",
        // review round 3
        "GIT_PAGER=cat git log",
        "PAGER=cat git log",
        "GIT_EDITOR=true git commit",
        "EDITOR=vim git commit",
        "ENV=production npm run build",
        "git -c core.pager=cat log",
        "git -c pager.log=false log",
        "git -c credential.helper= fetch",
        "git -c credential.useHttpPath=true fetch",
        "git -c core.fsmonitor=false status",
        "nice -10 make",
        "nice -n10 make",
        "rsync -avze ssh a b:c",
        "export PATH=/x:$PATH",
        "tar -xvzf a.tgz",
        "tar -cJf a.tar.xz x",
        "tar -Izstd -xf a.tar.zst",
        // review round 4
        "git -c credential.helper=store push",
        "git -c credential.helper=osxkeychain push",
    ] {
        assert!(clear(command), "{command}: {:?}", analyze(command, HOME));
    }
}

#[test]
fn a_budget_stop_is_incomplete_but_not_a_regex_limit() {
    let a = analyze(&format!("find . {}", "-exec find . ".repeat(5000)), HOME);
    assert!(a.incomplete);
    assert!(
        a.entries
            .iter()
            .all(|e| !e.opaque.contains(&Opaque::RegexLimit))
    );
}

#[test]
fn opaque_unbalanced() {
    let a = analyze("echo it's; npx playwright install", HOME);
    assert!(
        a.entries
            .iter()
            .all(|e| e.opaque.contains(&Opaque::Unbalanced))
    );
    assert!(!a.incomplete);
}

#[test]
fn env_split_strings_are_read_as_env_does() {
    // fix-wave I: only env's own options hold -S, and its words become env's arguments
    for (command, args) in [
        ("env A=1 npx playwright install -S x", "install -S x"),
        ("sudo env npx playwright install -S ls", "install -S ls"),
        ("env -S\"npx playwright install\"", "install"),
        (
            "env --split-string='npx playwright install' ls",
            "install ls",
        ),
        ("env -S'npx playwright install' ls", "install ls"),
        ("env -u X -S 'npx playwright install'", "install"),
    ] {
        let a = analyze(command, HOME);
        assert!(
            a.entries
                .iter()
                .any(|e| e.prog == "playwright" && e.args.join(" ") == args),
            "{command}: {a:?}"
        );
    }
}

#[test]
fn a_path_regex_limit_sets_incomplete() {
    // the lexer has no regex since fix-wave H; `_TILDE_USER` and `_HOME_VAR` keep fancy-regex (GC-16)
    let text = "cd ~other/x && ls";
    assert!(!analyze(text, HOME).incomplete);
    assert!(analyze_with(text, HOME, &Limits { backtrack_limit: 0 }).incomplete);
}

// ------------------------------------------------------------------ fail-closed bounds

/// A catastrophe guard: a bound below 8 s is raised to it, because a slow runner is not a regression.
fn assert_quick(what: &str, bound: Duration, f: impl FnOnce()) {
    let bound = bound.max(Duration::from_secs(8));
    let start = Instant::now();
    f();
    let took = start.elapsed();
    assert!(took < bound, "{what}: {took:?} (bound {bound:?})");
}

#[test]
fn deep_find_exec_is_walked_without_recursion_and_capped() {
    // Python recurses here and raises RecursionError near 500 levels (D-1); Rust walks it with a stack.
    // Each level copies the rest, so the work is quadratic: the linear budget stops it (fail closed).
    let deep = |n: usize| format!("{}ls ;", "find . -exec ".repeat(n));
    let a = analyze(&deep(40), HOME);
    assert!(!a.incomplete);
    assert_eq!(a.entries.len(), 41);
    assert_quick("find cap", Duration::from_secs(1), || {
        assert!(analyze(&deep(400), HOME).incomplete);
        assert!(analyze(&deep(1500), HOME).incomplete);
    });
}

#[test]
fn quadratic_option_rescans_stop_at_the_budget() {
    let command = format!("docker run img{}", " fish -cX".repeat(4000));
    assert_quick("docker fish rescan", Duration::from_secs(5), || {
        assert!(analyze(&command, HOME).incomplete);
    });
}

#[test]
fn peeling_64_prefixes_does_not_copy_the_words_each_time() {
    let peeled_command = format!("{}ls{}", "sudo ".repeat(64), " a".repeat(200_000));
    let plain_command = format!("ls{}", " a".repeat(200_000));
    let best = |command: &str| {
        (0..3)
            .map(|_| {
                let start = Instant::now();
                analyze(command, HOME);
                start.elapsed()
            })
            .min()
            .unwrap()
    };
    let (peeled, plain) = (best(&peeled_command), best(&plain_command));
    assert_eq!(analyze(&peeled_command, HOME).entries[0].prog, "ls");
    // Copying every word on each of the 64 peels made this about 12 times the plain cost; never judged
    // below a 0.5 s floor (shared runners are noisy).
    assert!(
        peeled < (plain * 10).max(Duration::from_millis(500)),
        "{peeled:?} vs {plain:?}"
    );
}

#[test]
fn worst_case_analysis_fits_a_one_mebibyte_stack() {
    let mut texts = vec![
        format!("{}ls ;", "find . -exec ".repeat(5000)),
        format!("{}ls", "eval ".repeat(40)),
        format!("echo {}ls{}", "$(".repeat(200), ")".repeat(200)),
        format!("{}ls", "sudo env nohup ".repeat(30)),
    ];
    let mut heredoc = String::from("ls");
    for _ in 0..20 {
        heredoc = format!("cat <<EOF | sh\n$({heredoc})\nEOF\n");
    }
    texts.push(heredoc);
    let handle = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || {
            for text in &texts {
                analyze(text, HOME);
            }
        })
        .expect("spawn");
    assert!(handle.join().is_ok());
}

// ------------------------------------------------------------------ properties

fn simple_command() -> impl Strategy<Value = String> {
    let prog = prop::sample::select(vec![
        "ls",
        "git",
        "grep",
        "cat",
        "make",
        "playwright",
        "agent-browser",
    ]);
    let arg = prop::sample::select(vec!["a", "b", "-x", "install", "x.txt", "push", "--flag"]);
    (prog, prop::collection::vec(arg, 0..4)).prop_map(|(p, args)| {
        let mut words = vec![p];
        words.extend(args);
        words.join(" ")
    })
}

/// A simple command whose arguments collide with wrapper, runner and shell options.
fn hostile_command() -> impl Strategy<Value = String> {
    let prog = prop::sample::select(vec!["ls", "git", "playwright", "make", "cat"]);
    let arg = prop::sample::select(vec![
        "a",
        "-S",
        "-c",
        "-u",
        "-n",
        "-m",
        "-C",
        "--",
        "-s",
        "-k",
        "-a",
        "-I",
        "x.txt",
        "--split-string",
        "-v",
        "-V",
        "-x",
    ]);
    (prog, prop::collection::vec(arg, 0..5)).prop_map(|(p, args)| {
        let mut words = vec![p];
        words.extend(args);
        words.join(" ")
    })
}

/// Wrappers that take one simple command (prefixes and `find -exec`), not shell text.
fn simple_only(wrapper: &str) -> bool {
    const SIMPLE: &[&str] = &[
        "sudo {}",
        "sudo -u bob {}",
        "nohup {}",
        "env A=1 {}",
        "timeout 5 {}",
        "nice -n 3 {}",
        "xargs {}",
        "command {}",
        "FOO=1 {}",
        "rtk proxy {}",
        "find . -exec {} \\;",
    ];
    SIMPLE.contains(&wrapper)
}

const WRAPPERS: &[&str] = &[
    "sudo {}",
    "sudo -u bob {}",
    "nohup {}",
    "env A=1 {}",
    "timeout 5 {}",
    "nice -n 3 {}",
    "xargs {}",
    "command {}",
    "FOO=1 {}",
    "sh -c '{}'",
    "bash -euo pipefail -c '{}'",
    "fish -c '{}'",
    "echo $({})",
    "x=`{}`",
    "({})",
    "true && {}",
    "{} | cat",
    "false || {}",
    "cat <<EOF | sh\n{}\nEOF",
    "sh <<< '{}'",
    "echo '{}' | bash",
    "rtk run -c '{}'",
    "rtk proxy {}",
    "docker run img sh -c '{}'",
    "find . -exec {} \\;",
    "eval '{}'",
    "if {}; then :; fi",
    // not `watch '{}'`: guard_rules.py reads watch's argument as a script only when it holds a space
];

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1500,
        rng_seed: proptest::test_runner::RngSeed::Fixed(0x5348_3232),
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Wrapping a command never hides it: every entry of `X` is still an entry (same program and
    /// arguments) of `W(X)`, for every wrapper, nested twice.
    #[test]
    fn wrapping_is_monotone(
        command in simple_command(),
        outer in 0..WRAPPERS.len(),
        inner in 0..WRAPPERS.len(),
    ) {
        let base = analyze(&command, HOME);
        let once = WRAPPERS[inner].replace("{}", &command);
        // A prefix wrapper takes one simple command, so it only wraps another prefix wrapper's output; a
        // single-quoted wrapper cannot hold another one's quotes.
        let twice = if (simple_only(WRAPPERS[outer]) && !simple_only(WRAPPERS[inner]))
            || ["'", "`", "EOF"]
                .iter()
                .any(|d| WRAPPERS[outer].contains(d) && once.contains(d))
        {
            once.clone()
        } else {
            WRAPPERS[outer].replace("{}", &once)
        };
        for wrapped in [once, twice] {
            let got = analyze(&wrapped, HOME);
            for e in &base.entries {
                prop_assert!(
                    got.entries.iter().any(|g| g.prog == e.prog && g.args == e.args),
                    "{wrapped:?} hides {} {:?}", e.prog, e.args
                );
            }
        }
    }

    /// With arguments that collide with wrapper and shell options (`-S`, `-c`, `-u`, `--`, ...), Python's
    /// analysis can lose a wrapped command (`env A=1 ls -S x` reads `x` as env's script; FW-I fixes it), so
    /// the claim is weaker: a wrapped command is still an entry, or some entry is marked opaque, so rtk's
    /// gate asks.
    #[test]
    fn wrapping_never_hides_silently(
        command in hostile_command(),
        outer in 0..WRAPPERS.len(),
        inner in 0..WRAPPERS.len(),
    ) {
        let base = analyze(&command, HOME);
        let once = WRAPPERS[inner].replace("{}", &command);
        let twice = if (simple_only(WRAPPERS[outer]) && !simple_only(WRAPPERS[inner]))
            || ["'", "`", "EOF"]
                .iter()
                .any(|d| WRAPPERS[outer].contains(d) && once.contains(d))
        {
            once.clone()
        } else {
            WRAPPERS[outer].replace("{}", &once)
        };
        for wrapped in [once, twice] {
            let got = analyze(&wrapped, HOME);
            let flagged = got.incomplete || got.entries.iter().any(|g| !g.opaque.is_empty());
            for e in &base.entries {
                prop_assert!(
                    flagged || got.entries.iter().any(|g| g.prog == e.prog && g.args == e.args),
                    "{wrapped:?} hides {} {:?} silently", e.prog, e.args
                );
            }
        }
    }

    #[test]
    fn analyze_never_panics(text in any::<String>(), home in "[a-z/~$\u{130}]{0,8}") {
        let a = analyze(&text, &home);
        for e in &a.entries {
            prop_assert!(a.groups.iter().any(|g| g.id == e.group));
        }
    }

    #[test]
    fn analyze_never_panics_on_shell_like_text(
        parts in prop::collection::vec(
            prop::sample::select(vec![
                "sudo ", "env -S ", "npx ", "-c ", "'", "\"", "$(", ")", "`", "| sh", "<<EOF\n", "EOF\n",
                "find . -exec ", ";", "\\;", "cd ", "~", "$HOME", "fish ", "--comm=", "rtk run ", "docker run i ",
                "eval ", "python3 -m ", "yarn workspace ", "\n", " ", "x", "\u{3a3}", "\u{e9}",
            ]),
            0..40,
        ),
    ) {
        let text = parts.concat();
        analyze(&text, HOME);
        analyze_with(&text, "", &Limits { backtrack_limit: 0 });
    }
}
