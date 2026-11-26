//! What every step needs, in one place: how to run programs, how to ask the person at the
//! terminal, where the installer's own folder is and where to report.

use std::ffi::OsString;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use crate::claude::Claude;
use crate::proc::Runner;
use crate::report::Report;

/// A yes/no question to the person at the terminal.
pub trait Ask {
    /// `default_yes` is what an empty answer means. End of input is always no.
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool;
}

pub struct Ctx<'a> {
    pub runner: &'a dyn Runner,
    pub ask: &'a mut dyn Ask,
    pub report: Report,
    /// `<Claude config folder>/anthology`: the lock, `installer.json` and the `bin` folder.
    pub ah: PathBuf,
    /// The PATH the installer looks programs up in.
    pub path: OsString,
    /// `/proc/sys/kernel/osrelease`, where WSL names itself.
    pub osrelease: PathBuf,
    /// Standard input and output are a terminal and `--json` is off: prompts are allowed.
    pub interactive: bool,
    /// Started from inside a Claude Code session (`CLAUDECODE` is set).
    pub in_claude: bool,
    /// `uninstall --yes`.
    pub yes: bool,
}

impl<'a> Ctx<'a> {
    pub fn claude(&self) -> Claude<'a> {
        Claude::new(self.runner)
    }
}

/// What a typed answer means: `y`/`yes` and `n`/`no` in any case, empty is the default, and
/// anything else is no (an unclear answer never agrees to a change).
pub fn interpret(answer: &str, default_yes: bool) -> bool {
    match answer.trim().to_ascii_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    }
}

/// Asks on standard output and reads the answer from standard input.
pub struct StdinAsk;

impl Ask for StdinAsk {
    fn confirm(&mut self, question: &str, default_yes: bool) -> bool {
        let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
        print!("{question} {hint} ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => false,
            Ok(_) => interpret(&line, default_yes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_mean_what_a_person_expects() {
        assert!(interpret("", true));
        assert!(!interpret("", false));
        assert!(interpret(" Y \n", false));
        assert!(interpret("yes", false));
        assert!(!interpret("n", true));
        assert!(!interpret("NO", true));
        assert!(
            !interpret("maybe", true),
            "an unclear answer is no, even when yes is the default"
        );
        assert!(!interpret("yep", true));
    }
}
