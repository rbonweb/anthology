//! The driver for the official `claude plugin ...` commands, and what their `--json` output means.
//!
//! The shapes below are the ones Claude Code 2.1.x prints (the files in `tests/fixtures/claude`
//! are real output, with the home folder replaced):
//!
//! - `plugin marketplace list --json`: an array of `{name, source, installLocation, ...}` where
//!   `source` is `github` (with `repo`), `git` (with `url` and `ref`) or `directory` (with `path`).
//! - `plugin list --json`: an array of `{id, version, enabled, installPath, ...}`; a plugin that
//!   cannot load also has `errors` (messages) and `errorDetails` (`{type, plugin}`), and
//!   `engine-range-unsatisfied` means Claude Code is older than the plugin's `engines` range.
//! - `plugin install|update <id> --json` and `plugin marketplace add|update|remove --json`: one
//!   result object on stdout, `{command, outcome: "ok"|"failed", message, ...}`, and on failure
//!   the human message on stderr too, with exit code 1.
//!
//! The installer never passes `-y` or `--accept-command`: a command a marketplace declares must
//! be read and confirmed by a person, so it is shown and the step stops.

use std::path::PathBuf;

use anthology_pycompat::json::{self, Map, Value};

use crate::proc::{Cmd, Output, Runner};

/// The marketplace name and the repository it comes from.
pub const MARKETPLACE: &str = "anthology";
pub const MARKETPLACE_SOURCE: &str = "rbonweb/anthology";
/// Installed in this order: the main plugin first, which brings the other two with it.
pub const PLUGINS: [&str; 3] = [
    "anthology@anthology",
    "anthology-guardrails@anthology",
    "anthology-superpowers@anthology",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Github(String),
    Git(String),
    /// Anything else (a local folder, a URL): what to call it in a message.
    Other(String),
}

impl Source {
    /// True for this project's own repository, however it was written.
    pub fn is_ours(&self) -> bool {
        match self {
            Source::Github(repo) => repo.eq_ignore_ascii_case(MARKETPLACE_SOURCE),
            Source::Git(url) => {
                let lower = url.to_ascii_lowercase();
                let trimmed = lower.trim_end_matches('/');
                let bare = trimmed.strip_suffix(".git").unwrap_or(trimmed);
                [
                    "https://github.com/",
                    "ssh://git@github.com/",
                    "git@github.com:",
                ]
                .iter()
                .any(|prefix| bare.strip_prefix(prefix) == Some(MARKETPLACE_SOURCE))
            }
            Source::Other(_) => false,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Source::Github(repo) => format!("github {repo}"),
            Source::Git(url) => format!("git {url}"),
            Source::Other(what) => what.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marketplace {
    pub name: String,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub id: String,
    pub version: Option<String>,
    pub enabled: bool,
    pub install_path: Option<PathBuf>,
    /// The CLI's own messages about why the plugin did not load.
    pub errors: Vec<String>,
    /// Claude Code is older than the plugin's `engines` range.
    pub engine_unsatisfied: bool,
}

/// What a `--json` action command reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `outcome: ok`. `detail` is the update result (`up to date`, `0.5.4 -> 0.5.5`) when there is
    /// one, else the CLI's message.
    Done {
        detail: String,
    },
    /// The marketplace declares a command that a person has to confirm: its text.
    NeedsConfirm(String),
    Failed(String),
}

fn string_field(map: &Map, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).map(str::to_string)
}

/// The array in a `list --json` output, each element an object.
fn list_items(text: &str, what: &str) -> Result<Vec<Map>, String> {
    let doc = json::loads(text).map_err(|e| format!("{what}: not JSON ({e})"))?;
    let items = doc
        .into_array()
        .ok_or_else(|| format!("{what}: expected a JSON array"))?;
    items
        .into_iter()
        .map(|item| {
            item.into_object()
                .ok_or_else(|| format!("{what}: expected an object in the array"))
        })
        .collect()
}

pub fn parse_marketplaces(text: &str) -> Result<Vec<Marketplace>, String> {
    const WHAT: &str = "claude plugin marketplace list";
    list_items(text, WHAT)?
        .iter()
        .map(|m| {
            let name = string_field(m, "name").ok_or(format!("{WHAT}: an entry has no name"))?;
            let kind = string_field(m, "source").unwrap_or_default();
            let field = |key: &str| string_field(m, key);
            let source = match (kind.as_str(), field("repo"), field("url")) {
                ("github", Some(repo), _) => Source::Github(repo),
                ("git", _, Some(url)) => Source::Git(url),
                _ => {
                    let place = field("path").or(field("url")).or(field("repo"));
                    Source::Other(
                        format!("{kind} {}", place.unwrap_or_default())
                            .trim()
                            .into(),
                    )
                }
            };
            Ok(Marketplace { name, source })
        })
        .collect()
}

pub fn parse_plugins(text: &str) -> Result<Vec<Plugin>, String> {
    const WHAT: &str = "claude plugin list";
    list_items(text, WHAT)?
        .iter()
        .map(|p| {
            let id = string_field(p, "id").ok_or(format!("{WHAT}: an entry has no id"))?;
            let strings = |key: &str| -> Vec<String> {
                p.get(key)
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let engine_unsatisfied =
                p.get("errorDetails")
                    .and_then(Value::as_array)
                    .is_some_and(|details| {
                        details.iter().any(|d| {
                            d.as_object()
                                .and_then(|d| d.get("type"))
                                .and_then(Value::as_str)
                                == Some("engine-range-unsatisfied")
                        })
                    });
            Ok(Plugin {
                id,
                version: string_field(p, "version"),
                enabled: p.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                // A relative path would resolve against the folder the installer runs in.
                install_path: string_field(p, "installPath")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute()),
                errors: strings("errors"),
                engine_unsatisfied,
            })
        })
        .collect()
}

/// The result of an action command: its JSON line, else what the process said.
pub fn parse_outcome(out: &Output) -> Outcome {
    let result = out
        .stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .find_map(|line| {
            let object = json::loads(line).ok()?.into_object()?;
            object.contains_key("outcome").then_some(object)
        });
    let Some(result) = result else {
        return Outcome::Failed(if out.success() {
            "claude printed no result".to_string()
        } else {
            failure_text(out)
        });
    };
    let message = string_field(&result, "message");
    if string_field(&result, "outcome").as_deref() == Some("ok") && out.success() {
        let versions = (
            string_field(&result, "oldVersion"),
            string_field(&result, "newVersion"),
        );
        let detail = match versions {
            (Some(old), Some(new)) if old == new => format!("up to date ({new})"),
            (Some(old), Some(new)) => format!("updated {old} -> {new}"),
            _ => message.unwrap_or_default(),
        };
        return Outcome::Done { detail };
    }
    if let Some(shown) = result.get("shownCommand") {
        let text = match shown {
            Value::Str(s) => s.clone(),
            other => other
                .as_object()
                .and_then(|o| string_field(o, "command"))
                .unwrap_or_else(|| json::dumps(other, &json::PY_DEFAULT)),
        };
        return Outcome::NeedsConfirm(text);
    }
    Outcome::Failed(message.unwrap_or_else(|| failure_text(out)))
}

/// `claude plugin ...` through a [`Runner`].
pub struct Claude<'a> {
    runner: &'a dyn Runner,
}

impl<'a> Claude<'a> {
    pub fn new(runner: &'a dyn Runner) -> Claude<'a> {
        Claude { runner }
    }

    fn run(&self, args: &[&str]) -> Result<Output, String> {
        let mut all = vec!["plugin"];
        all.extend_from_slice(args);
        self.runner
            .run(&Cmd::new("claude", &all))
            .map_err(|e| format!("cannot run claude: {e}"))
    }

    fn list_with<T>(
        &self,
        args: &[&str],
        parse: fn(&str) -> Result<Vec<T>, String>,
    ) -> Result<Vec<T>, String> {
        let out = self.run(args)?;
        if !out.success() {
            return Err(failure_text(&out));
        }
        parse(&out.stdout)
    }

    pub fn marketplaces(&self) -> Result<Vec<Marketplace>, String> {
        self.list_with(&["marketplace", "list", "--json"], parse_marketplaces)
    }

    pub fn plugins(&self) -> Result<Vec<Plugin>, String> {
        self.list_with(&["list", "--json"], parse_plugins)
    }

    fn action(&self, args: &[&str]) -> Outcome {
        match self.run(args) {
            Ok(out) => parse_outcome(&out),
            Err(e) => Outcome::Failed(e),
        }
    }

    pub fn marketplace_add(&self, source: &str) -> Outcome {
        self.action(&["marketplace", "add", source, "--json"])
    }

    pub fn marketplace_update(&self, name: &str) -> Outcome {
        self.action(&["marketplace", "update", name, "--json"])
    }

    pub fn marketplace_remove(&self, name: &str) -> Outcome {
        self.action(&["marketplace", "remove", name, "--json"])
    }

    pub fn plugin_install(&self, id: &str) -> Outcome {
        self.action(&["install", id, "--json"])
    }

    pub fn plugin_update(&self, id: &str) -> Outcome {
        self.action(&["update", id, "--json"])
    }

    pub fn plugin_uninstall(&self, id: &str) -> Outcome {
        self.action(&["uninstall", id, "--json"])
    }

    /// `uninstall <id> --prune -y`. `--json` does not work with `--prune`, so only the exit code
    /// and the human message are there to read.
    pub fn plugin_uninstall_prune(&self, id: &str) -> Outcome {
        match self.run(&["uninstall", id, "--prune", "-y"]) {
            Ok(out) if out.success() => Outcome::Done {
                detail: first_line(&out.stdout),
            },
            Ok(out) => Outcome::Failed(failure_text(&out)),
            Err(e) => Outcome::Failed(e),
        }
    }
}

fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Why a command that exited non-zero failed: its first line of stderr, else of stdout.
fn failure_text(out: &Output) -> String {
    let line = first_line(&out.stderr);
    let line = if line.is_empty() {
        first_line(&out.stdout)
    } else {
        line
    };
    match out.code {
        Some(code) if line.is_empty() => format!("exit code {code}"),
        Some(code) => format!("{line} (exit code {code})"),
        None => format!("{line} (ended by a signal)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Script;

    const MARKETPLACES: &str = include_str!("../tests/fixtures/claude/marketplace-list.json");
    const PLUGINS_OLD_CLAUDE: &str =
        include_str!("../tests/fixtures/claude/plugin-list-engine-unsatisfied.json");
    const INSTALL_OK: &str = include_str!("../tests/fixtures/claude/plugin-install-ok.json");
    const INSTALL_FAILED: &str =
        include_str!("../tests/fixtures/claude/plugin-install-failed.json");
    const UPDATE_UP_TO_DATE: &str =
        include_str!("../tests/fixtures/claude/plugin-update-up-to-date.json");
    const MARKETPLACE_ADD_OK: &str =
        include_str!("../tests/fixtures/claude/marketplace-add-ok.json");

    fn output(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            code: Some(code),
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn marketplaces_parse_each_source_kind() {
        let list = parse_marketplaces(MARKETPLACES).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, "anthology");
        assert_eq!(
            list[0].source,
            Source::Git("https://github.com/rbonweb/anthology.git".into())
        );
        assert_eq!(
            list[1].source,
            Source::Github("anthropics/claude-plugins-official".into())
        );
        assert_eq!(
            list[2].source,
            Source::Other("directory /home/user/work/plugins".into())
        );
        assert_eq!(parse_marketplaces("[]").unwrap(), vec![]);
    }

    #[test]
    fn only_this_projects_repository_is_ours() {
        for ours in [
            Source::Github("rbonweb/anthology".into()),
            Source::Github("RBonWeb/Anthology".into()),
            Source::Git("https://github.com/rbonweb/anthology.git".into()),
            Source::Git("https://github.com/rbonweb/anthology".into()),
            Source::Git("git@github.com:rbonweb/anthology.git".into()),
            Source::Git("ssh://git@github.com/rbonweb/anthology.git/".into()),
        ] {
            assert!(ours.is_ours(), "{ours:?}");
        }
        for other in [
            Source::Github("someone/anthology".into()),
            Source::Github("rbonweb/anthology-fork".into()),
            Source::Git("https://github.com/rbonweb/anthology-fork.git".into()),
            Source::Git("https://example.com/github.com/rbonweb/anthology.git".into()),
            Source::Other("directory /home/user/anthology".into()),
        ] {
            assert!(!other.is_ours(), "{other:?}");
        }
    }

    #[test]
    fn marketplace_output_that_is_not_a_list_is_an_error() {
        assert!(parse_marketplaces("{}").is_err());
        assert!(parse_marketplaces("not json").is_err());
        assert!(parse_marketplaces("[1]").is_err());
        assert!(parse_marketplaces(r#"[{"source":"github"}]"#).is_err());
    }

    #[test]
    fn plugins_parse_with_the_engine_problem_of_an_old_claude() {
        let list = parse_plugins(PLUGINS_OLD_CLAUDE).unwrap();
        assert_eq!(list.len(), 4);
        let anthology = list.iter().find(|p| p.id == "anthology@anthology").unwrap();
        assert_eq!(anthology.version.as_deref(), Some("0.5.5"));
        assert!(anthology.enabled);
        assert_eq!(
            anthology.install_path,
            Some(PathBuf::from(
                "/home/user/.claude/plugins/cache/anthology/anthology/0.5.5"
            ))
        );
        assert!(anthology.engine_unsatisfied);
        assert_eq!(anthology.errors.len(), 1);
        assert!(anthology.errors[0].contains("needs Claude Code >=2.1.330"));
        let guardrails = list
            .iter()
            .find(|p| p.id == "anthology-guardrails@anthology")
            .unwrap();
        assert!(!guardrails.engine_unsatisfied);
        assert!(guardrails.errors.is_empty());
        let off = list.iter().find(|p| p.id.starts_with("frontend")).unwrap();
        assert!(!off.enabled);
    }

    #[test]
    fn other_error_types_are_not_an_engine_problem() {
        let text = r#"[{"id":"a@b","version":"1","enabled":true,"installPath":"/x",
            "errors":["broken"],"errorDetails":[{"type":"manifest-invalid","plugin":"a"}]}]"#;
        let list = parse_plugins(text).unwrap();
        assert!(!list[0].engine_unsatisfied);
        assert_eq!(list[0].errors, vec!["broken".to_string()]);
    }

    #[test]
    fn a_relative_install_path_is_not_trusted() {
        let text = r#"[{"id":"a@b","enabled":true,"installPath":"relative/dir"}]"#;
        assert_eq!(parse_plugins(text).unwrap()[0].install_path, None);
    }

    #[test]
    fn ok_results_say_what_happened() {
        assert_eq!(
            parse_outcome(&output(0, INSTALL_OK, "")),
            Outcome::Done {
                detail: "Successfully installed plugin: anthology@anthology (scope: user) (+ 2 dependencies: anthology-guardrails, anthology-superpowers)".into()
            }
        );
        assert_eq!(
            parse_outcome(&output(0, UPDATE_UP_TO_DATE, "")),
            Outcome::Done {
                detail: "up to date (0.5.5)".into()
            }
        );
        let updated = r#"{"command":"update","outcome":"ok","message":"m","updateOutcome":"updated","oldVersion":"0.5.4","newVersion":"0.5.5"}"#;
        assert_eq!(
            parse_outcome(&output(0, updated, "")),
            Outcome::Done {
                detail: "updated 0.5.4 -> 0.5.5".into()
            }
        );
        assert!(matches!(
            parse_outcome(&output(0, MARKETPLACE_ADD_OK, "")),
            Outcome::Done { .. }
        ));
    }

    #[test]
    fn a_failed_result_carries_the_cli_message_even_with_text_on_stderr() {
        let got = parse_outcome(&output(1, INSTALL_FAILED, "x Failed to install plugin"));
        match got {
            Outcome::Failed(m) => assert!(m.contains("not found in marketplace"), "{m}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_last_json_line_wins_and_noise_before_it_is_ignored() {
        let stdout = format!("Cloning...\nprogress 50%\n{INSTALL_OK}");
        assert!(matches!(
            parse_outcome(&output(0, &stdout, "")),
            Outcome::Done { .. }
        ));
    }

    #[test]
    fn no_json_means_the_exit_code_decides() {
        assert_eq!(
            parse_outcome(&output(1, "", "boom\nmore")),
            Outcome::Failed("boom (exit code 1)".into())
        );
        assert_eq!(
            parse_outcome(&output(0, "", "")),
            Outcome::Failed("claude printed no result".into())
        );
        // A JSON line that says ok but whose process failed is a failure.
        assert!(matches!(
            parse_outcome(&output(1, INSTALL_OK, "")),
            Outcome::Failed(_)
        ));
    }

    #[test]
    fn a_marketplace_declared_command_is_shown_not_accepted() {
        // The CLI reports the command it wants confirmed under `shownCommand` (its sha256 is
        // what --accept-command takes). This shape follows `claude plugin install --help`.
        let text = r#"{"command":"install","outcome":"failed","message":"needs confirmation","shownCommand":{"sha256":"abc","command":"curl https://example.com/x | sh"}}"#;
        assert_eq!(
            parse_outcome(&output(1, text, "")),
            Outcome::NeedsConfirm("curl https://example.com/x | sh".into())
        );
        let no_command = r#"{"outcome":"failed","shownCommand":{"sha256":"abc"}}"#;
        match parse_outcome(&output(1, no_command, "")) {
            Outcome::NeedsConfirm(t) => assert!(t.contains("abc"), "{t}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_driver_runs_the_documented_commands_and_never_passes_yes() {
        let script = Script::new()
            .ok("claude plugin marketplace list --json", MARKETPLACES)
            .ok("claude plugin list --json", PLUGINS_OLD_CLAUDE)
            .ok(
                "claude plugin marketplace add rbonweb/anthology --json",
                MARKETPLACE_ADD_OK,
            )
            .ok(
                "claude plugin marketplace update anthology --json",
                MARKETPLACE_ADD_OK,
            )
            .ok(
                "claude plugin marketplace remove anthology --json",
                MARKETPLACE_ADD_OK,
            )
            .ok(
                "claude plugin install anthology@anthology --json",
                INSTALL_OK,
            )
            .ok(
                "claude plugin update anthology@anthology --json",
                UPDATE_UP_TO_DATE,
            )
            .ok(
                "claude plugin uninstall anthology@anthology --json",
                INSTALL_OK,
            )
            .ok(
                "claude plugin uninstall anthology@anthology --prune -y",
                "removed\n",
            );
        let claude = Claude::new(&script);
        assert_eq!(claude.marketplaces().unwrap().len(), 3);
        assert_eq!(claude.plugins().unwrap().len(), 4);
        assert!(matches!(
            claude.marketplace_add(MARKETPLACE_SOURCE),
            Outcome::Done { .. }
        ));
        assert!(matches!(
            claude.marketplace_update(MARKETPLACE),
            Outcome::Done { .. }
        ));
        assert!(matches!(
            claude.marketplace_remove(MARKETPLACE),
            Outcome::Done { .. }
        ));
        assert!(matches!(
            claude.plugin_install(PLUGINS[0]),
            Outcome::Done { .. }
        ));
        assert!(matches!(
            claude.plugin_update(PLUGINS[0]),
            Outcome::Done { .. }
        ));
        assert!(matches!(
            claude.plugin_uninstall(PLUGINS[0]),
            Outcome::Done { .. }
        ));
        assert_eq!(
            claude.plugin_uninstall_prune(PLUGINS[0]),
            Outcome::Done {
                detail: "removed".into()
            }
        );
        script.assert_done();
        for call in script.calls() {
            assert!(!call.contains("--accept-command"), "{call}");
            let yes = call.split(' ').any(|w| w == "-y" || w == "--yes");
            assert_eq!(yes, call.contains("--prune"), "{call}");
        }
    }

    #[test]
    fn a_failing_list_command_is_an_error_with_its_first_line() {
        let script = Script::new().exit("claude plugin list", 1, "", "no config\nsecond");
        let err = Claude::new(&script).plugins().unwrap_err();
        assert_eq!(err, "no config (exit code 1)");
    }

    #[test]
    fn a_claude_that_cannot_start_is_an_error() {
        let script = Script::new().then(
            "claude plugin list",
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "gone")),
        );
        let err = Claude::new(&script).plugins().unwrap_err();
        assert!(err.contains("cannot run claude"), "{err}");
    }
}
