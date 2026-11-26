//! The `anthology-installer` binary: reads the environment once, builds a [`Ctx`] and runs the
//! command. Nothing else in the crate reads the environment.
#![forbid(unsafe_code)]

use std::io::IsTerminal;
use std::path::PathBuf;

use anthology_installer::cli::{self, Args, Command, Parsed, USAGE};
use anthology_installer::ctx::{Ctx, StdinAsk};
use anthology_installer::proc::SysRunner;
use anthology_installer::report::Report;
use anthology_installer::{install, status, uninstall};

/// 64, `EX_USAGE`: the command line was wrong. Not one of the four documented outcomes.
const EXIT_USAGE: i32 = 64;

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    match cli::parse(std::env::args_os().skip(1)) {
        Err(e) => {
            eprintln!("anthology-installer: {e}\n{USAGE}");
            EXIT_USAGE
        }
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            0
        }
        Ok(Parsed::Version) => {
            println!("anthology-installer {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Ok(Parsed::Run(args)) => run(&args),
    }
}

/// `$CLAUDE_CONFIG_DIR`, else `$HOME/.claude`: where Claude Code (and the plugins' bootstrap, which
/// reads the same two variables) keep their files. Always absolute.
fn config_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    let dir = match var("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(var("HOME")?).join(".claude"),
    };
    dir.is_absolute().then_some(dir)
}

fn run(args: &Args) -> i32 {
    let mut report = Report::new(args.json, Box::new(std::io::stdout()));
    let Some(config) = config_dir() else {
        report.fail(
            "preflight",
            "cannot tell where Claude Code keeps its files: set HOME (or CLAUDE_CONFIG_DIR to an absolute path)",
        );
        report.finish(args.command.name(), 1);
        return 1;
    };
    let interactive =
        !args.json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let mut ask = StdinAsk;
    let mut ctx = Ctx {
        runner: &SysRunner,
        ask: &mut ask,
        report,
        ah: config.join("anthology"),
        path: std::env::var_os("PATH").unwrap_or_default(),
        osrelease: PathBuf::from("/proc/sys/kernel/osrelease"),
        interactive,
        in_claude: std::env::var_os("CLAUDECODE").is_some_and(|v| !v.is_empty()),
        yes: args.yes,
    };
    let code = match args.command {
        Command::Install => install::run(&mut ctx),
        Command::Uninstall => uninstall::run(&mut ctx),
        Command::Status => status::run(&mut ctx),
    };
    ctx.report.finish(args.command.name(), code);
    code
}
