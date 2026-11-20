//! `Target`, `detect_with`, `uname` (plan §3.3, GC-9): `rtk_install.target()` and `TARGETS`.
#![forbid(unsafe_code)]

use crate::sys;

/// A release target, in `rtk_install.TARGETS` order (the derived `Ord` follows it).
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Debug)]
pub enum Target {
    AppleArm64,
    AppleX86_64,
    LinuxX86_64Musl,
    LinuxArm64Musl,
}

impl Target {
    /// Every target, in `TARGETS` order.
    pub const ALL: [Target; 4] = [
        Target::AppleArm64,
        Target::AppleX86_64,
        Target::LinuxX86_64Musl,
        Target::LinuxArm64Musl,
    ];

    /// The Rust target triple, as in `TARGETS` and the release asset names.
    pub fn triple(self) -> &'static str {
        match self {
            Target::AppleArm64 => "aarch64-apple-darwin",
            Target::AppleX86_64 => "x86_64-apple-darwin",
            Target::LinuxX86_64Musl => "x86_64-unknown-linux-musl",
            Target::LinuxArm64Musl => "aarch64-unknown-linux-musl",
        }
    }

    /// The target whose triple is `s`.
    pub fn from_triple(s: &str) -> Option<Target> {
        Target::ALL.into_iter().find(|t| t.triple() == s)
    }
}

/// `str.isspace()` (CPython 3.12): Unicode `White_Space` plus the separators U+001C-U+001F.
fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `rtk_install.target(system, machine)`: `system` and `machine` are stripped and lowercased
/// (the keys are ASCII, and no non-ASCII character lowercases into one of their letters), then
/// mapped. The errors are GC-9's texts, naming the values as given.
pub fn detect_with(uname: (&str, &str)) -> Result<Target, String> {
    let (system, machine) = uname;
    let s = system.trim_matches(py_space).to_ascii_lowercase();
    let m = machine.trim_matches(py_space).to_ascii_lowercase();
    if ["windows", "cygwin", "msys", "mingw"]
        .iter()
        .any(|p| s.starts_with(p))
    {
        return Err("Windows isn't supported: rtk is installed on macOS and Linux only".into());
    }
    match (s.as_str(), m.as_str()) {
        ("darwin", "arm64" | "aarch64") => Ok(Target::AppleArm64),
        ("darwin", "x86_64") => Ok(Target::AppleX86_64),
        ("linux", "x86_64" | "amd64") => Ok(Target::LinuxX86_64Musl),
        ("linux", "aarch64" | "arm64") => Ok(Target::LinuxArm64Musl),
        _ => Err(format!(
            "unsupported platform {system}/{machine}: rtk is installed on macOS and Linux \
             (x86_64, arm64) only"
        )),
    }
}

/// `(platform.system(), platform.machine())` from `uname(2)`; empty strings if it fails.
pub fn uname() -> (String, String) {
    sys::uname().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_follow_gc9() {
        let triples: Vec<_> = Target::ALL.iter().map(|t| t.triple()).collect();
        assert_eq!(
            triples,
            [
                "aarch64-apple-darwin",
                "x86_64-apple-darwin",
                "x86_64-unknown-linux-musl",
                "aarch64-unknown-linux-musl"
            ]
        );
        let mut sorted = Target::ALL;
        sorted.sort();
        assert_eq!(sorted, Target::ALL);
        for t in Target::ALL {
            assert_eq!(Target::from_triple(t.triple()), Some(t));
        }
        assert_eq!(Target::from_triple("x86_64-pc-windows-msvc"), None);
    }

    #[test]
    fn detect_maps_every_gc9_pair() {
        for (s, m, t) in [
            ("Darwin", "arm64", Target::AppleArm64),
            ("darwin", "aarch64", Target::AppleArm64),
            ("Darwin", "x86_64", Target::AppleX86_64),
            ("Linux", "x86_64", Target::LinuxX86_64Musl),
            ("LINUX", "AMD64", Target::LinuxX86_64Musl),
            ("Linux", "aarch64", Target::LinuxArm64Musl),
            ("linux", "arm64", Target::LinuxArm64Musl),
            (" Linux\n", "\tx86_64 ", Target::LinuxX86_64Musl),
            (
                "\u{1c}Linux\u{85}",
                "\u{a0}x86_64\u{3000}",
                Target::LinuxX86_64Musl,
            ),
        ] {
            assert_eq!(detect_with((s, m)), Ok(t), "{s:?}/{m:?}");
        }
    }

    #[test]
    fn detect_refuses_with_gc9_messages() {
        for s in [
            "Windows",
            "windows_nt",
            "CYGWIN_NT-10.0",
            "MSYS_NT",
            "MINGW64_NT",
            " Windows",
        ] {
            assert_eq!(
                detect_with((s, "x86_64")),
                Err("Windows isn't supported: rtk is installed on macOS and Linux only".into())
            );
        }
        assert_eq!(
            detect_with(("FreeBSD", "amd64")),
            Err(
                "unsupported platform FreeBSD/amd64: rtk is installed on macOS and Linux \
                 (x86_64, arm64) only"
                    .into()
            )
        );
        assert_eq!(
            detect_with((" Linux ", "riscv64")),
            Err(
                "unsupported platform  Linux /riscv64: rtk is installed on macOS and Linux \
                 (x86_64, arm64) only"
                    .into()
            )
        );
        assert!(detect_with(("Darwin", "i386")).is_err());
        assert!(detect_with(("", "")).is_err());
        // The Kelvin sign lowercases to "k" in Python; no key holds a "k", so this stays unknown.
        assert!(detect_with(("Linux", "x86_64\u{212a}")).is_err());
    }

    #[test]
    fn uname_detects_this_machine() {
        let (s, m) = uname();
        assert!(detect_with((&s, &m)).is_ok(), "{s}/{m}");
    }
}
