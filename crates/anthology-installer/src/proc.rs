//! Running other programs, behind a seam: the steps talk to a [`Runner`], so unit tests script the
//! answers and the end-to-end tests put fake programs on PATH.

use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A program, its arguments and where to run it. Arguments are never passed through a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

impl Cmd {
    pub fn new(program: &str, args: &[&str]) -> Cmd {
        Cmd {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
        }
    }

    pub fn cwd(mut self, dir: &Path) -> Cmd {
        self.cwd = Some(dir.to_path_buf());
        self
    }

    /// `program arg arg`, for messages. Not shell-quoted: use [`shell_quote`] for a command the
    /// user is told to paste.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Output {
    /// `None` when a signal ended the process.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

pub trait Runner {
    /// Runs `cmd` to the end with stdin closed and both output streams captured. `Err` only when
    /// the program could not be started.
    fn run(&self, cmd: &Cmd) -> io::Result<Output>;
}

/// The real thing: a child process that inherits this process's environment and PATH.
pub struct SysRunner;

impl Runner for SysRunner {
    fn run(&self, cmd: &Cmd) -> io::Result<Output> {
        let mut command = Command::new(&cmd.program);
        command
            .args(&cmd.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = &cmd.cwd {
            command.current_dir(dir);
        }
        let out = command.output()?;
        Ok(Output {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// The first executable file called `name` in a directory of `path`. Relative and empty entries
/// are skipped: they would resolve against the folder the installer was started in.
pub fn find_in_path(name: &str, path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

/// `value` as one word of a POSIX shell command: single-quoted, so the user can paste it.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn file(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    #[test]
    fn find_in_path_returns_the_first_executable_match() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        file(a.path(), "tool", 0o644);
        let want = file(b.path(), "tool", 0o755);
        let path = std::env::join_paths([a.path(), b.path()]).unwrap();
        assert_eq!(find_in_path("tool", &path), Some(want));
        assert_eq!(find_in_path("absent", &path), None);
    }

    #[test]
    fn find_in_path_ignores_relative_and_empty_entries() {
        let dir = tempfile::tempdir().unwrap();
        file(dir.path(), "tool", 0o755);
        // `:` alone is an empty entry (the current folder); `rel` is a relative one.
        assert_eq!(find_in_path("tool", OsStr::new(":rel")), None);
    }

    #[test]
    fn find_in_path_skips_a_directory_with_the_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("tool")).unwrap();
        assert_eq!(find_in_path("tool", dir.path().as_os_str()), None);
    }

    #[test]
    fn shell_quote_keeps_every_byte_literal() {
        assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$(x) `y` \"z\""), "'$(x) `y` \"z\"'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn sys_runner_captures_output_and_the_exit_code() {
        let out = SysRunner
            .run(&Cmd::new(
                "sh",
                &["-c", "printf out; printf err >&2; exit 3"],
            ))
            .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, "out");
        assert_eq!(out.stderr, "err");
        assert!(!out.success());
    }

    #[test]
    fn sys_runner_runs_in_the_given_folder_with_stdin_closed() {
        let dir = tempfile::tempdir().unwrap();
        let out = SysRunner
            .run(&Cmd::new("sh", &["-c", "pwd; cat"]).cwd(dir.path()))
            .unwrap();
        assert!(out.success());
        let real = fs::canonicalize(dir.path()).unwrap();
        assert_eq!(out.stdout.trim(), real.to_string_lossy());
    }

    #[test]
    fn sys_runner_reports_a_missing_program_as_an_error() {
        assert!(
            SysRunner
                .run(&Cmd::new("anthology-installer-no-such-program", &[]))
                .is_err()
        );
    }
}
