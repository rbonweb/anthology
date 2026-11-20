//! `config_dir`, `anthology_home`, `home` (plan §3.3, GC-1).

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use crate::{Env, sys};

/// `os.path.expanduser("~")` (CPython 3.12 `posixpath`): `HOME` when it is set (even empty or
/// not UTF-8), else the passwd home of the current uid. Trailing `/` are stripped and an empty
/// result is `/`. With no `HOME` and no passwd entry Python returns the path unchanged, so
/// this is `~`.
pub fn home(env: &Env) -> PathBuf {
    let raw = match env.get_os("HOME") {
        Some(h) => h.to_os_string(),
        None => match sys::passwd_home() {
            Some(h) => h,
            None => return PathBuf::from("~"),
        },
    };
    let bytes = raw.as_bytes();
    let end = bytes.iter().rposition(|&b| b != b'/').map_or(0, |i| i + 1);
    if end == 0 {
        return PathBuf::from("/");
    }
    PathBuf::from(OsString::from_vec(bytes[..end].to_vec()))
}

/// `$CLAUDE_CONFIG_DIR` when it is non-empty, else `os.path.join(home, ".claude")`.
pub fn config_dir(env: &Env) -> PathBuf {
    match env.get_os("CLAUDE_CONFIG_DIR") {
        Some(c) if !c.is_empty() => PathBuf::from(c),
        _ => home(env).join(".claude"),
    }
}

/// `<config>/anthology`.
pub fn anthology_home(env: &Env) -> PathBuf {
    config_dir(env).join("anthology")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn with_home(h: &str) -> Env {
        Env::default().with("HOME", h)
    }

    #[test]
    fn home_follows_expanduser() {
        for (h, want) in [
            ("/home/user", "/home/user"),
            ("/home/user/", "/home/user"),
            ("/home/user//", "/home/user"),
            ("/", "/"),
            ("///", "/"),
            ("", "/"),
            ("rel/", "rel"),
            ("/a//b", "/a//b"),
        ] {
            assert_eq!(home(&with_home(h)), PathBuf::from(want), "HOME={h:?}");
        }
    }

    #[test]
    fn a_non_utf8_home_is_kept() {
        let env = Env::default().with_os("HOME", OsStr::from_bytes(b"/h/\xff/"));
        assert_eq!(home(&env).as_os_str().as_bytes(), b"/h/\xff");
    }

    #[test]
    fn config_dir_prefers_a_non_empty_claude_config_dir() {
        let env = with_home("/h").with("CLAUDE_CONFIG_DIR", "/c");
        assert_eq!(config_dir(&env), PathBuf::from("/c"));
        assert_eq!(anthology_home(&env), PathBuf::from("/c/anthology"));
        let env = with_home("/h").with("CLAUDE_CONFIG_DIR", "");
        assert_eq!(config_dir(&env), PathBuf::from("/h/.claude"));
        assert_eq!(config_dir(&with_home("")), PathBuf::from("/.claude"));
        assert_eq!(
            anthology_home(&with_home("/h/")),
            PathBuf::from("/h/.claude/anthology")
        );
    }
}
