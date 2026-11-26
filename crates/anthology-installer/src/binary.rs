//! The anthology binary: is it installed, and the step that installs it through the guardrails
//! plugin's own `hooks/bootstrap.sh`.

use std::path::Path;

use crate::claude::Plugin;
use crate::ctx::Ctx;
use crate::proc::{Cmd, Output, shell_quote};
use crate::steps::EXIT_FAILED;

pub const GUARDRAILS: &str = "anthology-guardrails@anthology";

/// The arguments after `bootstrap.sh`. They are what `anth_fix_cmd` in the plugin's `lib.sh`
/// builds (a repo test compares the two), which is the one command the guard lets through while
/// the binary is missing.
pub const BOOTSTRAP_ARGS: [&str; 6] = [
    "guard",
    "--plugin",
    "guardrails",
    "--quiet",
    "--budget",
    "300",
];

/// `/bin/sh '<hooks>/bootstrap.sh' guard --plugin guardrails --quiet --budget 300`.
pub fn bootstrap_command(hooks_dir: &str) -> String {
    let script = format!("{hooks_dir}/bootstrap.sh");
    format!(
        "/bin/sh {} {}",
        shell_quote(&script),
        BOOTSTRAP_ARGS.join(" ")
    )
}

/// The bootstrap's `--quiet` answer: `status=present` or `status=installed` is a success,
/// `status=failed:<reason>` a failure. Anything else is not understood.
pub fn parse_status(out: &Output) -> Result<String, String> {
    let status = out
        .stdout
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("status="));
    match status {
        Some(s @ ("present" | "installed")) if out.success() => Ok(s.to_string()),
        Some(failed) if failed.starts_with("failed:") => {
            Err(failed["failed:".len()..].trim().to_string())
        }
        Some(other) => Err(format!("unexpected status `{other}`")),
        None => Err(match out.stderr.lines().find(|l| !l.trim().is_empty()) {
            Some(line) => format!("no status line; {}", line.trim()),
            None => "no status line".to_string(),
        }),
    }
}

/// Step 5: the guard binary, downloaded and checked by the plugin's own bootstrap against the
/// plugin's lock. Returns `present` or `installed`.
pub fn step(ctx: &mut Ctx, plugins: &[Plugin]) -> Result<String, i32> {
    let Some(root) = plugins
        .iter()
        .find(|p| p.id == GUARDRAILS)
        .and_then(|p| p.install_path.as_ref())
    else {
        ctx.report.fail(
            "binary",
            "anthology-guardrails is not installed, so there is no bootstrap to run",
        );
        return Err(EXIT_FAILED);
    };
    let script = root.join("hooks/bootstrap.sh");
    if !script.is_file() {
        ctx.report
            .fail("binary", "the guardrails plugin has no hooks/bootstrap.sh");
        return Err(EXIT_FAILED);
    }
    let script = script.to_string_lossy().into_owned();
    let mut args = vec![script.as_str()];
    args.extend(BOOTSTRAP_ARGS);
    match ctx.runner.run(&Cmd::new("/bin/sh", &args)) {
        Err(e) => {
            ctx.report
                .fail("binary", &format!("cannot run the bootstrap: {e}"));
            Err(EXIT_FAILED)
        }
        Ok(out) => match parse_status(&out) {
            Ok(status) => {
                ctx.report.ok("binary", &status);
                Ok(status)
            }
            Err(reason) => {
                ctx.report
                    .fail("binary", &format!("could not install it: {reason}"));
                Err(EXIT_FAILED)
            }
        },
    }
}

/// True when `<ah>/bin/<version>/anthology` exists for some version. Hidden folders
/// (`.tmp-*`, `.fetch-*`, `.broken-*`) are the bootstrap's own scratch space.
pub fn installed(ah: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(ah.join("bin")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        !entry.file_name().to_string_lossy().starts_with('.')
            && std::fs::metadata(entry.path().join("anthology")).is_ok_and(|m| m.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::parse_plugins;
    use crate::testing::{self, Answers, Buf, Script};
    use std::fs;
    use std::path::Path;

    fn out(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn the_bootstrap_command_is_the_one_the_guard_allows() {
        assert_eq!(
            bootstrap_command("/h/hooks"),
            "/bin/sh '/h/hooks/bootstrap.sh' guard --plugin guardrails --quiet --budget 300"
        );
    }

    #[test]
    fn status_present_and_installed_are_successes() {
        assert_eq!(
            parse_status(&out(0, "status=present\n", "")),
            Ok("present".into())
        );
        assert_eq!(
            parse_status(&out(0, "noise\nstatus=installed\n", "")),
            Ok("installed".into())
        );
    }

    #[test]
    fn a_failed_status_gives_the_reason_and_a_bad_exit_never_counts_as_success() {
        assert_eq!(
            parse_status(&out(1, "status=failed:no network\n", "")),
            Err("no network".into())
        );
        assert!(parse_status(&out(1, "status=installed\n", "")).is_err());
        assert!(
            parse_status(&out(0, "status=weird\n", ""))
                .unwrap_err()
                .contains("weird")
        );
        assert_eq!(parse_status(&out(0, "", "")), Err("no status line".into()));
        assert!(
            parse_status(&out(2, "", "usage: x\n"))
                .unwrap_err()
                .contains("usage: x")
        );
    }

    /// A guardrails plugin folder with (or without) hooks/bootstrap.sh.
    fn guardrails(dir: &Path, with_script: bool) -> Vec<Plugin> {
        let root = dir.join("plugins/anthology-guardrails");
        fs::create_dir_all(root.join("hooks")).unwrap();
        if with_script {
            fs::write(root.join("hooks/bootstrap.sh"), "#!/bin/sh\n").unwrap();
        }
        let text = format!(
            r#"[{{"id":"anthology-guardrails@anthology","version":"1","installPath":"{}"}}]"#,
            root.display()
        );
        parse_plugins(&text).unwrap()
    }

    #[test]
    fn the_step_runs_the_plugins_own_bootstrap_with_the_pinned_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = guardrails(dir.path(), true);
        let want = format!(
            "/bin/sh {}/plugins/anthology-guardrails/hooks/bootstrap.sh guard --plugin guardrails --quiet --budget 300",
            dir.path().display()
        );
        let script = Script::new().ok(&want, "status=installed\n");
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(step(&mut ctx, &plugins), Ok("installed".to_string()));
        assert_eq!(buf.text(), "ok binary: installed\n");
        script.assert_done();
    }

    #[test]
    fn a_failing_bootstrap_fails_the_step_with_its_reason() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = guardrails(dir.path(), true);
        let script = Script::new().exit("/bin/sh", 1, "status=failed:checksum mismatch\n", "");
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(step(&mut ctx, &plugins), Err(EXIT_FAILED));
        assert_eq!(
            buf.text(),
            "fail binary: could not install it: checksum mismatch\n"
        );
    }

    #[test]
    fn without_the_plugin_or_its_script_nothing_runs() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new();
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(step(&mut ctx, &[]), Err(EXIT_FAILED));
        let plugins = guardrails(dir.path(), false);
        assert_eq!(step(&mut ctx, &plugins), Err(EXIT_FAILED));
        assert!(script.calls().is_empty());
    }

    #[test]
    fn a_version_folder_with_the_binary_counts() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!installed(dir.path()), "no bin folder at all");
        fs::create_dir_all(dir.path().join("bin/0.2.5")).unwrap();
        assert!(!installed(dir.path()), "an empty version folder");
        fs::write(dir.path().join("bin/0.2.5/anthology"), "x").unwrap();
        assert!(installed(dir.path()));
    }

    #[test]
    fn scratch_folders_do_not_count() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("bin/.tmp-abc")).unwrap();
        fs::write(dir.path().join("bin/.tmp-abc/anthology"), "x").unwrap();
        fs::create_dir_all(dir.path().join("bin/.broken-0.2.5-1-2")).unwrap();
        fs::write(dir.path().join("bin/.broken-0.2.5-1-2/anthology"), "x").unwrap();
        assert!(!installed(dir.path()));
    }
}
