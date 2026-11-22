//! `binary-missing` (F-13, no model; replaces `python-missing` at CO2): with no `anthology`
//! binary installed and no way to download one, the staged BS1 shims fail closed and say why.
//!
//! A copy of the staged anthology and anthology-guardrails plugins gets a valid
//! `anthology.lock`, and every hook runs with a `PATH` that has every tool of the caller's but
//! the downloaders (curl, wget, node, BusyBox), and a fresh config dir each, so each reason is
//! the first-attempt one:
//! - the anthology SessionStart hook and the guardrails SessionStart bootstrap each exit 0 with
//!   one JSON line whose `systemMessage` names the missing downloader (GC-14);
//! - the guard shim blocks a Bash call (exit 2, the `(setup)` no-downloader message) and still
//!   allows a read-only tool (exit 0);
//! - the `bin/anthology` and `bin/anthology-mem` wrappers exit 127 with their GC-14 message.

use std::collections::BTreeMap;
use std::path::Path;

use anthology_core::lock::ANTHOLOGY_TAG_PREFIX;
use anthology_core::platform::Target;

use crate::smoke::runner::*;
use crate::smoke::staged::{
    DOWNLOADERS, bash, envelope, hook_command, hook_env, narrowed_path, system_message,
};

/// What one shim run must give.
struct Want {
    code: i32,
    stdout_message: Option<&'static str>,
    stderr_prefix: Option<&'static str>,
}

const NO_DOWNLOADER: &str = "(no downloader (curl, wget or node); download https://github.com/";

fn check(label: &str, p: &Proc, want: &Want) -> Option<String> {
    let mut wrong = Vec::new();
    if p.code != want.code {
        wrong.push(format!("exit {} (want {})", p.code, want.code));
    }
    match want.stdout_message {
        Some(prefix) => match system_message(&p.out) {
            Some(msg) if msg.starts_with(prefix) && msg.contains(NO_DOWNLOADER) => {}
            Some(msg) => wrong.push(format!("systemMessage {}", py_repr_str(&msg))),
            None => wrong.push(format!(
                "stdout is not one JSON object with a systemMessage: {}",
                py_repr_str(&p.out)
            )),
        },
        None if !p.out.is_empty() => wrong.push(format!("stdout {}", py_repr_str(&p.out))),
        None => {}
    }
    match want.stderr_prefix {
        Some(prefix) if !p.err.starts_with(prefix) => {
            wrong.push(format!("stderr {}", py_repr_str(&p.err)));
        }
        None if !p.err.is_empty() => wrong.push(format!("stderr {}", py_repr_str(&p.err))),
        _ => {}
    }
    (!wrong.is_empty()).then(|| format!("{label}: {}", wrong.join(", ")))
}

pub fn run(ctx: &Ctx) -> Res<Outcome> {
    let staged = ctx.staged_plugins()?;
    let tmp = scratch()?;
    let plugins = tmp.join("plugins");
    let version = env!("CARGO_PKG_VERSION");
    let digests: BTreeMap<Target, String> =
        Target::ALL.iter().map(|t| (*t, "0".repeat(64))).collect();
    let lock = crate::lock::render(
        &format!("{ANTHOLOGY_TAG_PREFIX}{version}"),
        crate::lock::DEFAULT_REPO,
        digests,
    )?;
    for p in ["anthology", "anthology-guardrails"] {
        copy_tree(&staged.join(p), &plugins.join(p))?;
        write_text(&plugins.join(p).join("anthology.lock"), &lock)?;
    }
    let anthology = plugins.join("anthology");
    let guardrails = plugins.join("anthology-guardrails");
    let bin = tmp.join("bin");
    narrowed_path(ctx.env.get("PATH").unwrap_or(""), &bin, &DOWNLOADERS)?;
    let path = bin.to_string_lossy().into_owned();
    let home = tmp.join("home");
    let tmpdir = tmp.join("t");
    let project = tmp.join("project");
    for d in [&home, &tmpdir, &project] {
        mkdirs(d)?;
    }
    let mut fresh = 0;
    let mut shim = |plugin_root: &Path, argv: &[String], stdin: Option<&str>| -> Proc {
        fresh += 1;
        let config = tmp.join(format!("cfg-{fresh}"));
        let env = hook_env(&path, &home, &config, &tmpdir, plugin_root, &project);
        ctx.run(argv, &project, &env, 60, stdin)
    };

    let mut problems: Vec<String> = Vec::new();
    let session = hook_command(&anthology, "SessionStart", "session-start.sh")?;
    let p = shim(&anthology, &session, Some("{}"));
    problems.extend(check(
        "anthology session-start.sh",
        &p,
        &Want {
            code: 0,
            stdout_message: Some("anthology: could not install the anthology binary "),
            stderr_prefix: None,
        },
    ));
    let boot = hook_command(&guardrails, "SessionStart", "bootstrap.sh")?;
    let p = shim(&guardrails, &boot, Some("{}"));
    problems.extend(check(
        "anthology-guardrails bootstrap.sh session",
        &p,
        &Want {
            code: 0,
            stdout_message: Some("anthology-guardrails: could not install the guard binary "),
            stderr_prefix: None,
        },
    ));
    let guard = hook_command(&guardrails, "PreToolUse", "guard.sh")?;
    let p = shim(&guardrails, &guard, Some(&bash("ls", &project)));
    problems.extend(check(
        "guard.sh on Bash",
        &p,
        &Want {
            code: 2,
            stdout_message: None,
            stderr_prefix: Some(
                "Blocked by anthology-guardrails (setup): the guard binary is not installed and no downloader (curl, wget or node) was found",
            ),
        },
    ));
    let read = envelope(
        "Read",
        jobj(vec![(
            "file_path",
            jstr(&project.join("README.md").to_string_lossy()),
        )]),
        &project,
    );
    let p = shim(&guardrails, &guard, Some(&read));
    problems.extend(check(
        "guard.sh on Read",
        &p,
        &Want {
            code: 0,
            stdout_message: None,
            stderr_prefix: None,
        },
    ));
    for (name, args) in [
        ("anthology", vec!["--version"]),
        ("anthology-mem", vec!["search", "probe"]),
    ] {
        let mut argv = vec![
            anthology
                .join("bin")
                .join(name)
                .to_string_lossy()
                .into_owned(),
        ];
        argv.extend(args.iter().map(|a| a.to_string()));
        let p = shim(&anthology, &argv, None);
        let prefix: &'static str = if name == "anthology" {
            "anthology: the anthology binary is not installed ("
        } else {
            "anthology-mem: the anthology binary is not installed ("
        };
        problems.extend(check(
            &format!("bin/{name}"),
            &p,
            &Want {
                code: 127,
                stdout_message: None,
                stderr_prefix: Some(prefix),
            },
        ));
    }
    Ok(if problems.is_empty() {
        (
            Status::Pass,
            "without a binary or a downloader, both SessionStart hooks warn, the guard blocks Bash \
             and allows Read, and both wrappers exit 127"
                .to_string(),
        )
    } else {
        (
            Status::Fail,
            problems
                .iter()
                .map(|p| ctx.snippet(p, 160))
                .collect::<Vec<_>>()
                .join("; "),
        )
    })
}
