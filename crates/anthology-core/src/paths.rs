//! `config_dir`, `anthology_home`, `home` (plan §3.3, GC-1).
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use crate::{Env, sys};

/// `os.path.expanduser("~")` (CPython 3.12 `posixpath`): `HOME` when it is set (even empty or
/// not UTF-8), else the passwd home of the current uid. Trailing `/` are stripped and an empty
/// result is `/`. With no `HOME` and no passwd entry Python returns the path unchanged, so
/// this is `~`.
///
/// This is `expanduser("~")`, normalised. Python sites that use `HOME` as given, such as
/// `os.environ.get("HOME") or os.path.expanduser("~")`, want [`home_raw`] instead.
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

/// `os.environ.get("HOME") or os.path.expanduser("~")`: `HOME` exactly as given (a trailing
/// `/` kept) when it is non-empty, else [`home`]. These Python sites use this form:
/// `guard.py:21` (passed to `guard_rules.check`, which never normalises it) and
/// `settings_patch.py:556` (line numbers informational).
///
/// Sites that treat an unset or empty `HOME` as "no home" (no passwd fallback), such as
/// `memory_privacy.py:154` and `anthology_setup.py:178` and `:215`, read
/// `env.get_os("HOME")` directly instead.
pub fn home_raw(env: &Env) -> PathBuf {
    match env.get_os("HOME") {
        Some(h) if !h.is_empty() => PathBuf::from(h),
        _ => home(env),
    }
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

/// `<ah>` as the bootstrap and the shims resolve it (`lib.sh` `anth_paths`), for every check of
/// the installed binary: `$CLAUDE_CONFIG_DIR/anthology` when that is non-empty, else
/// `$HOME/.claude/anthology`. `None` unless the folder it starts from is an absolute path. A
/// relative one would select a binary under the working folder, which the guard's hot path runs
/// unhashed. An unset or empty `HOME` is refused too, with no passwd fallback, because the shell
/// has none.
pub fn install_home(env: &Env) -> Option<PathBuf> {
    let absolute = |v: &std::ffi::OsStr| v.as_bytes().first() == Some(&b'/');
    match env.get_os("CLAUDE_CONFIG_DIR") {
        Some(c) if !c.is_empty() => absolute(c).then(|| PathBuf::from(c).join("anthology")),
        _ => match env.get_os("HOME") {
            Some(h) if absolute(h) => Some(home(env).join(".claude").join("anthology")),
            _ => None,
        },
    }
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
    fn home_raw_keeps_home_as_given() {
        for (h, want) in [
            ("/home/user/", "/home/user/"),
            ("/a//b", "/a//b"),
            ("//", "//"),
            ("", "/"),
        ] {
            assert_eq!(home_raw(&with_home(h)), PathBuf::from(want), "HOME={h:?}");
        }
        let env = Env::default().with_os("HOME", OsStr::from_bytes(b"/h/\xff/"));
        assert_eq!(home_raw(&env).as_os_str().as_bytes(), b"/h/\xff/");
    }

    #[test]
    fn a_non_utf8_home_is_kept() {
        let env = Env::default().with_os("HOME", OsStr::from_bytes(b"/h/\xff/"));
        assert_eq!(home(&env).as_os_str().as_bytes(), b"/h/\xff");
    }

    #[test]
    fn install_home_needs_an_absolute_folder_and_never_uses_passwd() {
        let ah = |env: &Env| install_home(env);
        assert_eq!(
            ah(&with_home("/h/")),
            Some(PathBuf::from("/h/.claude/anthology"))
        );
        assert_eq!(
            ah(&with_home("/")),
            Some(PathBuf::from("/.claude/anthology"))
        );
        let env = with_home("rel").with("CLAUDE_CONFIG_DIR", "/c");
        assert_eq!(ah(&env), Some(PathBuf::from("/c/anthology")));
        let env = with_home("/h").with("CLAUDE_CONFIG_DIR", "");
        assert_eq!(ah(&env), Some(PathBuf::from("/h/.claude/anthology")));
        for env in [
            Env::default(),
            with_home(""),
            with_home("rel"),
            with_home("./h"),
            with_home("~"),
            with_home("/h").with("CLAUDE_CONFIG_DIR", "config"),
            with_home("/h").with("CLAUDE_CONFIG_DIR", "./c"),
            Env::default().with("CLAUDE_CONFIG_DIR", ""),
        ] {
            assert_eq!(ah(&env), None, "{env:?}");
        }
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
