//! The command line: `anthology-installer [install|uninstall|status] [--json] [--yes]`.

use std::ffi::OsString;

/// What the installer was asked to do. `install` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Install,
    Uninstall,
    Status,
}

impl Command {
    pub fn name(self) -> &'static str {
        match self {
            Command::Install => "install",
            Command::Uninstall => "uninstall",
            Command::Status => "status",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub command: Command,
    /// One JSON document on stdout instead of the `ok|skip|fail` lines. Never prompts.
    pub json: bool,
    /// Uninstall only: answer yes to every confirmation.
    pub yes: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Args),
    Help,
    Version,
}

pub const USAGE: &str = "\
usage: anthology-installer [install|uninstall|status] [--json] [--yes]

  install     add the Anthology marketplace and plugins, install the anthology binary and offer
              the one-time setup (the default; running it again upgrades)
  uninstall   undo it, after you confirm each part (--yes answers yes to all of them)
  status      show what is installed

  --json      print one JSON document instead of one `ok|skip|fail <step>: <detail>` line per step
  --yes, -y   uninstall only: do not ask
  --help, -h  this text
  --version, -V

exit codes: 0 done, 1 a step failed, 2 unsupported platform, 3 done but Claude Code is too old,
64 bad command line";

pub fn parse<I, T>(argv: I) -> Result<Parsed, String>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    use lexopt::prelude::*;
    let mut parser = lexopt::Parser::from_args(argv);
    let mut command = None;
    let (mut json, mut yes) = (false, false);
    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Long("help") | Short('h') => return Ok(Parsed::Help),
            Long("version") | Short('V') => return Ok(Parsed::Version),
            Long("json") => json = true,
            Long("yes") | Short('y') => yes = true,
            Value(word) => {
                if command.is_some() {
                    return Err(format!("unexpected argument {word:?}"));
                }
                command = Some(match word.to_str() {
                    Some("install") => Command::Install,
                    Some("uninstall") => Command::Uninstall,
                    Some("status") => Command::Status,
                    _ => {
                        return Err(format!(
                            "unknown command {word:?}; expected install, uninstall or status"
                        ));
                    }
                });
            }
            other => return Err(other.unexpected().to_string()),
        }
    }
    let command = command.unwrap_or(Command::Install);
    if yes && command != Command::Uninstall {
        return Err("--yes only applies to uninstall".to_string());
    }
    Ok(Parsed::Run(Args { command, json, yes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Parsed, String> {
        parse(args.iter().map(OsString::from))
    }

    fn run(command: Command, json: bool, yes: bool) -> Result<Parsed, String> {
        Ok(Parsed::Run(Args { command, json, yes }))
    }

    #[test]
    fn no_arguments_means_install() {
        assert_eq!(p(&[]), run(Command::Install, false, false));
    }

    #[test]
    fn each_subcommand_is_named_by_its_word() {
        assert_eq!(p(&["install"]), run(Command::Install, false, false));
        assert_eq!(p(&["uninstall"]), run(Command::Uninstall, false, false));
        assert_eq!(p(&["status"]), run(Command::Status, false, false));
    }

    #[test]
    fn json_flag_can_come_before_or_after_the_subcommand() {
        assert_eq!(p(&["--json"]), run(Command::Install, true, false));
        assert_eq!(p(&["status", "--json"]), run(Command::Status, true, false));
        assert_eq!(p(&["--json", "status"]), run(Command::Status, true, false));
    }

    #[test]
    fn yes_is_for_uninstall_only() {
        assert_eq!(
            p(&["uninstall", "--yes"]),
            run(Command::Uninstall, false, true)
        );
        assert_eq!(
            p(&["uninstall", "-y"]),
            run(Command::Uninstall, false, true)
        );
        assert!(p(&["--yes"]).unwrap_err().contains("uninstall"));
        assert!(p(&["status", "-y"]).unwrap_err().contains("uninstall"));
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(p(&["--help"]), Ok(Parsed::Help));
        assert_eq!(p(&["-h"]), Ok(Parsed::Help));
        assert_eq!(p(&["status", "--version"]), Ok(Parsed::Version));
        assert_eq!(p(&["-V"]), Ok(Parsed::Version));
    }

    #[test]
    fn unknown_words_and_options_are_refused() {
        assert!(p(&["frobnicate"]).unwrap_err().contains("frobnicate"));
        assert!(p(&["--no-such"]).unwrap_err().contains("--no-such"));
        assert!(p(&["install", "status"]).is_err());
    }
}
