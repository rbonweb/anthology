//! What every step needs, in one place: how to run programs, how to ask the person at the
//! terminal, where the installer's own folder is and where to report.

use std::ffi::OsString;
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
