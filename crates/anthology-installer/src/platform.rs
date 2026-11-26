//! Which machine this is: the platform table of the design (Darwin or Linux, arm64 or x86_64).

use std::path::Path;

use crate::proc::{Cmd, Runner};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Darwin,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    Arm64,
    X86_64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
    /// Linux inside Windows Subsystem for Linux.
    pub wsl: bool,
}

impl Platform {
    /// `Darwin arm64`, `Linux x86_64 (WSL)`.
    pub fn describe(&self) -> String {
        let os = match self.os {
            Os::Darwin => "Darwin",
            Os::Linux => "Linux",
        };
        let arch = match self.arch {
            Arch::Arm64 => "arm64",
            Arch::X86_64 => "x86_64",
        };
        let wsl = if self.wsl { " (WSL)" } else { "" };
        format!("{os} {arch}{wsl}")
    }
}

/// Why a machine is refused. Every case exits 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    /// A native Windows shell (MINGW, MSYS, CYGWIN).
    WindowsNative(String),
    Os(String),
    Arch(String),
}

impl Unsupported {
    pub fn message(&self) -> String {
        match self {
            Unsupported::WindowsNative(name) => format!(
                "{name} is native Windows, which Anthology does not support; use WSL \
                 (https://learn.microsoft.com/windows/wsl/install) and run the installer there"
            ),
            Unsupported::Os(name) => {
                format!("{name} is not supported; Anthology runs on macOS and Linux")
            }
            Unsupported::Arch(name) => {
                format!("the {name} processor is not supported; Anthology runs on arm64 and x86_64")
            }
        }
    }
}

/// The platform for the output of `uname -s` and `uname -m`.
pub fn parse(uname_s: &str, uname_m: &str, wsl: bool) -> Result<Platform, Unsupported> {
    let upper = uname_s.to_ascii_uppercase();
    if ["MINGW", "MSYS", "CYGWIN"]
        .iter()
        .any(|prefix| upper.starts_with(prefix))
    {
        return Err(Unsupported::WindowsNative(uname_s.to_string()));
    }
    let os = match uname_s {
        "Darwin" => Os::Darwin,
        "Linux" => Os::Linux,
        other => return Err(Unsupported::Os(other.to_string())),
    };
    let arch = match uname_m {
        "arm64" | "aarch64" => Arch::Arm64,
        "x86_64" | "amd64" => Arch::X86_64,
        other => return Err(Unsupported::Arch(other.to_string())),
    };
    Ok(Platform { os, arch, wsl })
}

/// `/proc/sys/kernel/osrelease` of a WSL kernel names Microsoft.
pub fn is_wsl(osrelease: &str) -> bool {
    osrelease.to_ascii_lowercase().contains("microsoft")
}

/// Asks `uname`, and reads `osrelease` (a missing file is not WSL).
pub fn detect(runner: &dyn Runner, osrelease: &Path) -> Result<Platform, Unsupported> {
    let ask = |flag: &str| -> Result<String, Unsupported> {
        match runner.run(&Cmd::new("uname", &[flag])) {
            Ok(out) if out.success() => Ok(out.stdout.trim().to_string()),
            _ => Err(Unsupported::Os(format!(
                "an unknown system (`uname {flag}` failed)"
            ))),
        }
    };
    let (s, m) = (ask("-s")?, ask("-m")?);
    let wsl = std::fs::read_to_string(osrelease).is_ok_and(|text| is_wsl(&text));
    parse(&s, &m, wsl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::Output;
    use std::io;

    #[test]
    fn the_four_supported_pairs_parse() {
        for (s, m, os, arch) in [
            ("Darwin", "arm64", Os::Darwin, Arch::Arm64),
            ("Darwin", "x86_64", Os::Darwin, Arch::X86_64),
            ("Linux", "aarch64", Os::Linux, Arch::Arm64),
            ("Linux", "amd64", Os::Linux, Arch::X86_64),
        ] {
            let got = parse(s, m, false).unwrap();
            assert_eq!((got.os, got.arch, got.wsl), (os, arch, false), "{s} {m}");
        }
    }

    #[test]
    fn native_windows_shells_are_refused_with_a_pointer_to_wsl() {
        for name in [
            "MINGW64_NT-10.0-22631",
            "MSYS_NT-10.0",
            "CYGWIN_NT-10.0",
            "mingw32_nt-6.1",
        ] {
            let err = parse(name, "x86_64", false).unwrap_err();
            assert!(matches!(err, Unsupported::WindowsNative(_)), "{name}");
            assert!(err.message().contains("WSL"), "{name}");
        }
    }

    #[test]
    fn other_systems_and_processors_are_refused() {
        assert_eq!(
            parse("FreeBSD", "amd64", false),
            Err(Unsupported::Os("FreeBSD".into()))
        );
        assert_eq!(
            parse("Linux", "riscv64", false),
            Err(Unsupported::Arch("riscv64".into()))
        );
        assert!(
            parse("Linux", "i686", false)
                .unwrap_err()
                .message()
                .contains("i686")
        );
    }

    #[test]
    fn wsl_is_a_kernel_that_names_microsoft() {
        assert!(is_wsl("5.15.153.1-microsoft-standard-WSL2\n"));
        assert!(is_wsl("4.4.0-19041-Microsoft"));
        assert!(!is_wsl("6.8.0-45-generic"));
        assert!(!is_wsl(""));
    }

    #[test]
    fn describe_names_wsl() {
        let p = parse("Linux", "x86_64", true).unwrap();
        assert_eq!(p.describe(), "Linux x86_64 (WSL)");
        assert_eq!(
            parse("Darwin", "arm64", false).unwrap().describe(),
            "Darwin arm64"
        );
    }

    struct Uname(&'static str, &'static str);

    impl Runner for Uname {
        fn run(&self, cmd: &Cmd) -> io::Result<Output> {
            assert_eq!(cmd.program, "uname");
            let out = if cmd.args == ["-s"] { self.0 } else { self.1 };
            Ok(Output {
                code: Some(0),
                stdout: format!("{out}\n"),
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn detect_asks_uname_and_reads_osrelease() {
        let dir = tempfile::tempdir().unwrap();
        let rel = dir.path().join("osrelease");
        std::fs::write(&rel, "5.15.0-microsoft-standard-WSL2\n").unwrap();
        let p = detect(&Uname("Linux", "x86_64"), &rel).unwrap();
        assert_eq!(p.describe(), "Linux x86_64 (WSL)");
        let p = detect(&Uname("Linux", "x86_64"), &dir.path().join("missing")).unwrap();
        assert!(!p.wsl);
    }

    #[test]
    fn detect_refuses_when_uname_cannot_run() {
        struct Broken;
        impl Runner for Broken {
            fn run(&self, _: &Cmd) -> io::Result<Output> {
                Err(io::Error::new(io::ErrorKind::NotFound, "no uname"))
            }
        }
        let err = detect(&Broken, Path::new("/nonexistent")).unwrap_err();
        assert!(matches!(err, Unsupported::Os(_)));
    }
}
