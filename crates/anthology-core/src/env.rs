//! `Env`: the environment, passed in by the CLI (plan §3.3, GC-2).
//!
//! Only `anthology-cli` reads the process environment, once, with [`Env::from_process`]; every
//! other crate takes `&Env`. Child processes get exactly an `Env` (see `git`), never the real
//! process environment.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

/// A snapshot of environment variables, sorted by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Env(BTreeMap<OsString, OsString>);

impl Env {
    /// The process environment, as is (`HOME` is not filled in; `paths::home` does the passwd
    /// fallback).
    pub fn from_process() -> Env {
        // A name listed twice keeps its first value, as `os.environ` and `getenv` do.
        let mut m = BTreeMap::new();
        for (k, v) in std::env::vars_os() {
            m.entry(k).or_insert(v);
        }
        Env(m)
    }

    /// The value of `k`; `None` when it is unset or not UTF-8.
    pub fn get(&self, k: &str) -> Option<&str> {
        self.0.get(OsStr::new(k)).and_then(|v| v.to_str())
    }

    /// The value of `k` as raw bytes, also when it is not UTF-8 (Python's `os.environ` keeps
    /// such a value, surrogate-escaped).
    pub fn get_os(&self, k: &str) -> Option<&OsStr> {
        self.0.get(OsStr::new(k)).map(OsString::as_os_str)
    }

    /// This environment with `k` set to `v`.
    pub fn with(self, k: &str, v: &str) -> Env {
        self.with_os(k, v)
    }

    /// This environment with `k` set to `v`, for values that need not be UTF-8 (paths).
    pub fn with_os(mut self, k: impl AsRef<OsStr>, v: impl AsRef<OsStr>) -> Env {
        self.0
            .insert(k.as_ref().to_os_string(), v.as_ref().to_os_string());
        self
    }

    /// This environment without `k`.
    pub fn without(mut self, k: &str) -> Env {
        self.0.remove(OsStr::new(k));
        self
    }

    /// Every variable, sorted by name (for a child process's environment).
    pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.0.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn with_without_and_get() {
        let env = Env::default().with("A", "1").with("B", "").without("C");
        assert_eq!(env.get("A"), Some("1"));
        assert_eq!(env.get("B"), Some(""));
        assert_eq!(env.get("C"), None);
        let env = env.without("A");
        assert_eq!(env.get("A"), None);
        let names: Vec<_> = env.iter().map(|(k, _)| k.to_owned()).collect();
        assert_eq!(names, vec![OsString::from("B")]);
    }

    #[test]
    fn a_non_utf8_value_is_kept_but_not_returned_as_text() {
        let raw = OsStr::from_bytes(b"/home/\xff");
        let env = Env::default().with_os("HOME", raw);
        assert_eq!(env.get("HOME"), None);
        assert_eq!(env.get_os("HOME"), Some(raw));
    }

    #[test]
    fn from_process_snapshots_without_adding_anything() {
        let env = Env::from_process();
        assert_eq!(env.iter().count(), std::env::vars_os().count());
    }
}
