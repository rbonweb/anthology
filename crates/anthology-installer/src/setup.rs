//! Step 7: the one-time machine setup, offered only at a terminal.
//!
//! The installer never writes `settings.json`. It runs the anthology binary's own
//! `setup preview`, shows the diff, asks, and passes the previewed checksum back to
//! `setup apply`, which refuses when the file changed in between.

use std::path::Path;

use anthology_pycompat::json::{self, Value};

use crate::claude::Plugin;
use crate::ctx::Ctx;
use crate::proc::{Cmd, Output, shell_quote};
use crate::steps::EXIT_FAILED;

pub const ANTHOLOGY: &str = "anthology@anthology";

/// How the setup step ended, for `installer.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupState {
    Applied,
    /// Already as setup wants it.
    Current,
    Declined,
    /// No terminal, or nothing to run it with.
    NotRun,
}

impl SetupState {
    pub fn word(self) -> &'static str {
        match self {
            SetupState::Applied => "applied",
            SetupState::Current => "current",
            SetupState::Declined => "declined",
            SetupState::NotRun => "not-run",
        }
    }
}

/// What `anthology setup preview` said about `settings.json`.
#[derive(Debug, PartialEq, Eq)]
pub struct Preview {
    pub supported: bool,
    pub changed: bool,
    pub path: String,
    pub diff: String,
    pub sha256: String,
    /// The preview could not plan the change.
    pub error: Option<String>,
}

/// The JSON object a setup command printed, or why there is none. `{"ok": false, "error": ...}`
/// is how setup refuses, with any exit code, so it is checked first.
pub(crate) fn result_object(out: &Output, what: &str) -> Result<json::Map, String> {
    let object = json::loads(&out.stdout)
        .ok()
        .and_then(Value::into_object)
        .ok_or_else(|| {
            let line = out
                .stderr
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            format!("{what} printed no JSON result {}", line.trim())
                .trim()
                .to_string()
        })?;
    if object.get("ok").and_then(Value::as_bool) == Some(false) {
        let error = object
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("refused");
        return Err(error.to_string());
    }
    Ok(object)
}

pub fn parse_preview(out: &Output) -> Result<Preview, String> {
    let object = result_object(out, "setup preview")?;
    let text = |m: &json::Map, key: &str| m.get(key).and_then(Value::as_str).map(str::to_string);
    if object.get("supported").and_then(Value::as_bool) == Some(false) {
        return Ok(Preview {
            supported: false,
            changed: false,
            path: String::new(),
            diff: String::new(),
            sha256: String::new(),
            error: text(&object, "message"),
        });
    }
    let settings = object
        .get("settings")
        .and_then(Value::as_object)
        .ok_or("setup preview has no settings block")?;
    let missing = |key: &str| format!("setup preview has no settings.{key}");
    Ok(Preview {
        supported: true,
        changed: settings
            .get("changed")
            .and_then(Value::as_bool)
            .ok_or_else(|| missing("changed"))?,
        path: text(settings, "path").ok_or_else(|| missing("path"))?,
        diff: text(settings, "diff").unwrap_or_default(),
        sha256: text(settings, "sha256").ok_or_else(|| missing("sha256"))?,
        error: text(settings, "error"),
    })
}

/// The `anthology setup ...` command line for a person to paste, with `shim` as the program.
pub fn preview_command(shim: &Path, root: &Path) -> String {
    format!(
        "{} setup preview --plugin-root {}",
        shell_quote(&shim.to_string_lossy()),
        shell_quote(&root.to_string_lossy())
    )
}

pub fn step(ctx: &mut Ctx, plugins: &[Plugin]) -> Result<SetupState, i32> {
    let fail = |ctx: &mut Ctx, detail: &str| {
        ctx.report.fail("setup", detail);
        Err(EXIT_FAILED)
    };
    let Some(root) = plugins
        .iter()
        .find(|p| p.id == ANTHOLOGY)
        .and_then(|p| p.install_path.clone())
    else {
        return fail(
            ctx,
            "the anthology plugin is not installed, so there is no setup to run",
        );
    };
    let shim = root.join("bin/anthology");
    if !shim.is_file() {
        return fail(
            ctx,
            "the anthology plugin has no bin/anthology to run setup with",
        );
    }
    if !ctx.interactive {
        let detail = format!(
            "no terminal, so nothing was changed; run /anthology:setup in Claude Code, or \
             review the change with: {}",
            preview_command(&shim, &root)
        );
        ctx.report.skip("setup", &detail);
        return Ok(SetupState::NotRun);
    }
    // Setup looks at the repo it runs in; the installer's own folder is no repo.
    let _ = std::fs::create_dir_all(&ctx.ah);
    let shim_arg = shim.to_string_lossy().into_owned();
    let root_arg = root.to_string_lossy().into_owned();
    let run = |ctx: &Ctx, extra: &[&str]| {
        let mut args = vec!["setup", extra[0], "--plugin-root", root_arg.as_str()];
        args.extend_from_slice(&extra[1..]);
        ctx.runner.run(&Cmd::new(&shim_arg, &args).cwd(&ctx.ah))
    };
    let preview = match run(ctx, &["preview"]) {
        Ok(out) => parse_preview(&out),
        Err(e) => Err(format!("cannot run the anthology binary: {e}")),
    };
    let preview = match preview {
        Ok(p) => p,
        Err(e) => return fail(ctx, &format!("setup preview failed: {e}")),
    };
    if !preview.supported {
        let why = preview.error.unwrap_or_default();
        ctx.report
            .skip("setup", &format!("not supported here: {why}"));
        return Ok(SetupState::NotRun);
    }
    if let Some(error) = preview.error {
        return fail(ctx, &format!("cannot plan the settings change: {error}"));
    }
    if !preview.changed {
        ctx.report.ok("setup", "settings are already set up");
        return Ok(SetupState::Current);
    }
    ctx.report
        .note(&format!("{}\n{}\n", preview.path, preview.diff));
    if !ctx.ask.confirm("Apply?", true) {
        ctx.report.skip(
            "setup",
            "declined; run /anthology:setup in Claude Code when you want it",
        );
        return Ok(SetupState::Declined);
    }
    let applied = match run(
        ctx,
        &[
            "apply",
            "--expect-sha256",
            &preview.sha256,
            "--skip-approve",
        ],
    ) {
        Ok(out) => result_object(&out, "setup apply").and_then(|_| {
            out.success()
                .then_some(())
                .ok_or_else(|| "setup apply failed".to_string())
        }),
        Err(e) => Err(format!("cannot run the anthology binary: {e}")),
    };
    match applied {
        Ok(()) => {
            ctx.report.ok("setup", "applied");
            Ok(SetupState::Applied)
        }
        Err(e) => fail(ctx, &format!("setup apply failed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::parse_plugins;
    use crate::testing::{self, Answers, Buf, Script};
    use std::fs;

    fn out(code: i32, stdout: &str) -> Output {
        Output {
            code: Some(code),
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn preview_json(changed: bool) -> String {
        format!(
            r#"{{"supported":true,"settings":{{"path":"/c/settings.json","sha256":"abc123","changed":{changed},"diff":"+  \"x\": 1\n","bypass_note":"n"}},"memory":{{}}}}"#
        )
    }

    #[test]
    fn a_preview_is_read_from_the_settings_block() {
        let p = parse_preview(&out(0, &preview_json(true))).unwrap();
        assert_eq!(
            p,
            Preview {
                supported: true,
                changed: true,
                path: "/c/settings.json".into(),
                diff: "+  \"x\": 1\n".into(),
                sha256: "abc123".into(),
                error: None,
            }
        );
        assert!(
            !parse_preview(&out(0, &preview_json(false)))
                .unwrap()
                .changed
        );
    }

    #[test]
    fn a_preview_that_could_not_plan_carries_its_error() {
        let text = r#"{"supported":true,"settings":{"path":"p","sha256":"s","changed":false,"diff":"","error":"bad json"}}"#;
        assert_eq!(
            parse_preview(&out(0, text)).unwrap().error.as_deref(),
            Some("bad json")
        );
        let refused = r#"{"ok":false,"error":"ValueError: nope"}"#;
        assert!(
            parse_preview(&out(1, refused))
                .unwrap_err()
                .contains("nope")
        );
        assert!(parse_preview(&out(0, "not json")).is_err());
        assert!(parse_preview(&out(0, r#"{"supported":true}"#)).is_err());
        let unsupported = r#"{"supported":false,"message":"no windows"}"#;
        assert!(!parse_preview(&out(0, unsupported)).unwrap().supported);
    }

    #[test]
    fn the_command_to_paste_quotes_every_path() {
        assert_eq!(
            preview_command(Path::new("/p q/bin/anthology"), Path::new("/r/it's")),
            r"'/p q/bin/anthology' setup preview --plugin-root '/r/it'\''s'"
        );
    }

    /// An anthology plugin folder with its shim; returns the plugins and the shim path.
    fn anthology(dir: &Path, with_shim: bool) -> (Vec<Plugin>, String) {
        let root = dir.join("plugins/anthology");
        fs::create_dir_all(root.join("bin")).unwrap();
        if with_shim {
            fs::write(root.join("bin/anthology"), "#!/bin/sh\n").unwrap();
        }
        let text = format!(
            r#"[{{"id":"anthology@anthology","version":"1","installPath":"{}"}}]"#,
            root.display()
        );
        (parse_plugins(&text).unwrap(), root.display().to_string())
    }

    #[test]
    fn without_a_terminal_it_prints_the_command_and_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (plugins, root) = anthology(dir.path(), true);
        let script = Script::new();
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(step(&mut ctx, &plugins), Ok(SetupState::NotRun));
        let text = buf.text();
        assert!(text.starts_with("skip setup: "), "{text}");
        assert!(
            text.contains(&format!("setup preview --plugin-root '{root}'")),
            "{text}"
        );
        assert!(script.calls().is_empty());
    }

    #[test]
    fn at_a_terminal_it_shows_the_diff_asks_and_applies_with_the_previewed_checksum() {
        let dir = tempfile::tempdir().unwrap();
        let (plugins, root) = anthology(dir.path(), true);
        let shim = format!("{root}/bin/anthology");
        let script = Script::new()
            .ok(
                &format!("{shim} setup preview --plugin-root {root}"),
                &preview_json(true),
            )
            .ok(
                &format!(
                    "{shim} setup apply --plugin-root {root} --expect-sha256 abc123 --skip-approve"
                ),
                r#"{"ok":true}"#,
            );
        let (mut ask, buf) = (Answers::new(&[true]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.interactive = true;
        assert_eq!(step(&mut ctx, &plugins), Ok(SetupState::Applied));
        script.assert_done();
        let text = buf.text();
        assert!(text.contains("/c/settings.json"), "{text}");
        assert!(text.contains("+  \"x\": 1"), "{text}");
        assert!(text.ends_with("ok setup: applied\n"), "{text}");
        assert_eq!(ask.asked, ["Apply?"]);
    }

    #[test]
    fn declining_runs_no_apply() {
        let dir = tempfile::tempdir().unwrap();
        let (plugins, root) = anthology(dir.path(), true);
        let script = Script::new().ok("", &preview_json(true));
        let _ = root;
        let (mut ask, buf) = (Answers::new(&[false]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.interactive = true;
        assert_eq!(step(&mut ctx, &plugins), Ok(SetupState::Declined));
        assert_eq!(script.calls().len(), 1);
        assert!(
            buf.text().contains("skip setup: declined"),
            "{}",
            buf.text()
        );
    }

    #[test]
    fn settings_that_are_already_right_are_not_offered_again() {
        let dir = tempfile::tempdir().unwrap();
        let (plugins, _) = anthology(dir.path(), true);
        let script = Script::new().ok("", &preview_json(false));
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.interactive = true;
        assert_eq!(step(&mut ctx, &plugins), Ok(SetupState::Current));
        assert!(buf.text().starts_with("ok setup: "), "{}", buf.text());
    }

    #[test]
    fn a_refused_apply_fails_the_step() {
        let dir = tempfile::tempdir().unwrap();
        let (plugins, _) = anthology(dir.path(), true);
        let script = Script::new().ok("", &preview_json(true)).exit(
            "",
            1,
            r#"{"ok":false,"error":"settings.json changed since the preview"}"#,
            "",
        );
        let (mut ask, buf) = (Answers::new(&[true]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.interactive = true;
        assert_eq!(step(&mut ctx, &plugins), Err(EXIT_FAILED));
        assert!(buf.text().contains("fail setup: "), "{}", buf.text());
        assert!(
            buf.text().contains("changed since the preview"),
            "{}",
            buf.text()
        );
    }

    #[test]
    fn a_preview_that_cannot_plan_is_a_failure_and_nothing_is_applied() {
        let dir = tempfile::tempdir().unwrap();
        let (plugins, _) = anthology(dir.path(), true);
        let bad = r#"{"supported":true,"settings":{"path":"p","sha256":"s","changed":false,"diff":"","error":"settings.json is not valid JSON"}}"#;
        let script = Script::new().ok("", bad);
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        ctx.interactive = true;
        assert_eq!(step(&mut ctx, &plugins), Err(EXIT_FAILED));
        assert!(buf.text().contains("not valid JSON"), "{}", buf.text());
    }

    #[test]
    fn without_the_plugin_or_its_shim_the_step_fails_before_running_anything() {
        let dir = tempfile::tempdir().unwrap();
        let script = Script::new();
        let (mut ask, buf) = (Answers::new(&[]), Buf::default());
        let mut ctx = testing::ctx(&script, &mut ask, dir.path(), &buf);
        assert_eq!(step(&mut ctx, &[]), Err(EXIT_FAILED));
        let (plugins, _) = anthology(dir.path(), false);
        assert_eq!(step(&mut ctx, &plugins), Err(EXIT_FAILED));
        assert!(script.calls().is_empty());
    }
}
