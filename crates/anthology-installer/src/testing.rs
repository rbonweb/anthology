//! Test helpers shared by the unit tests (compiled for tests only).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::rc::Rc;

use crate::ctx::{Ask, Ctx};
use crate::proc::{Cmd, Output, Runner};
use crate::report::Report;

/// A `Write` that the test can read back.
#[derive(Clone, Default)]
pub struct Buf(pub Rc<RefCell<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Buf {
    pub fn text(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).unwrap()
    }
}

/// Scripted answers to [`Ask::confirm`]; `asked` keeps the questions.
#[derive(Default)]
pub struct Answers {
    answers: VecDeque<bool>,
    pub asked: Vec<String>,
}

impl Answers {
    pub fn new(answers: &[bool]) -> Answers {
        Answers {
            answers: answers.iter().copied().collect(),
            asked: Vec::new(),
        }
    }
}

impl Ask for Answers {
    fn confirm(&mut self, question: &str, _default_yes: bool) -> bool {
        self.asked.push(question.to_string());
        self.answers
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected question: {question}"))
    }
}

/// Executable files called `names` in `<dir>/bin`, which `ctx` then uses as its PATH.
pub fn programs(dir: &Path, names: &[&str]) {
    let bin = dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for name in names {
        let path = bin.join(name);
        fs::write(&path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A [`Ctx`] for a unit test: not interactive, outside Claude Code, `<dir>/bin` as PATH,
/// `<dir>/anthology` as the installer's folder, text output into `buf`.
pub fn ctx<'a>(runner: &'a dyn Runner, ask: &'a mut dyn Ask, dir: &Path, buf: &Buf) -> Ctx<'a> {
    Ctx {
        runner,
        ask,
        report: Report::new(false, Box::new(buf.clone())),
        ah: dir.join("anthology"),
        path: OsString::from(dir.join("bin")),
        osrelease: dir.join("osrelease"),
        interactive: false,
        in_claude: false,
        yes: false,
    }
}

/// A [`Runner`] that answers from a script, in order. Each reply names the start of the command
/// line it expects (`claude plugin list`), so a step that runs the wrong command, or the right
/// one at the wrong time, fails the test at once. Every command line is kept in `calls`.
pub struct Script {
    replies: RefCell<VecDeque<(String, io::Result<Output>)>>,
    pub calls: RefCell<Vec<String>>,
}

impl Script {
    pub fn new() -> Script {
        Script {
            replies: RefCell::new(VecDeque::new()),
            calls: RefCell::new(Vec::new()),
        }
    }

    pub fn then(self, expect: &str, reply: io::Result<Output>) -> Script {
        self.replies
            .borrow_mut()
            .push_back((expect.to_string(), reply));
        self
    }

    /// Exit 0 with this stdout.
    pub fn ok(self, expect: &str, stdout: &str) -> Script {
        self.then(
            expect,
            Ok(Output {
                code: Some(0),
                stdout: stdout.to_string(),
                stderr: String::new(),
            }),
        )
    }

    /// Exit `code` with this stdout and stderr.
    pub fn exit(self, expect: &str, code: i32, stdout: &str, stderr: &str) -> Script {
        self.then(
            expect,
            Ok(Output {
                code: Some(code),
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            }),
        )
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    /// Panics when a scripted reply was never asked for.
    pub fn assert_done(&self) {
        let left = self.replies.borrow().len();
        assert_eq!(left, 0, "{left} scripted replies were never used");
    }
}

impl Runner for Script {
    fn run(&self, cmd: &Cmd) -> io::Result<Output> {
        let line = cmd.display();
        self.calls.borrow_mut().push(line.clone());
        let (expect, reply) = self
            .replies
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| panic!("unexpected command: {line}"));
        assert!(
            line.starts_with(&expect),
            "expected a command starting with `{expect}`, got `{line}`"
        );
        reply
    }
}
