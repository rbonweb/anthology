//! `anthology switch <key> <dir>`: whether the repo holding `<dir>` has turned a feature off in
//! `.claude/anthology.json` and is allowed to (approved in `/anthology:setup`). The delegation mod
//! asks this, so it and session start read the same file at the same place by the same rule
//! (`anthology_runtime::project::state`).
//!
//! `<key>` is `routing_guide` or `delegation`. The answer is one word on stdout and exit 0: `off`
//! (the file says `false` and the repo is approved), `ignored` (it says `false` but the repo is
//! not approved) or `on`. Anything else is exit 2 with a usage line.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;

use anthology_core::Env;
use anthology_runtime::project::{self, Switch};

use super::Output;

/// Runs `anthology switch …`; `args` are the arguments after `switch`.
pub fn run(args: &[OsString], _stdin: &mut dyn Read) -> Output {
    run_with(args, &Env::from_process())
}

fn run_with(args: &[OsString], env: &Env) -> Output {
    let (Some(key), Some(dir), None) = (
        args.first().and_then(|a| a.to_str()),
        args.get(1),
        args.get(2),
    ) else {
        return Output::usage("switch needs a key and a folder");
    };
    if ![project::GUIDE_KEY, project::DELEGATION_KEY].contains(&key) {
        return Output::usage(&format!("unknown switch: {key}"));
    }
    let word = match project::state(env, Path::new(dir), key) {
        Switch::On => "on",
        Switch::Off => "off",
        Switch::Ignored => "ignored",
    };
    Output {
        stdout: format!("{word}\n").into_bytes(),
        ..Output::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_folder_without_the_file_is_on() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::default().with_os("HOME", dir.path());
        let out = run_with(&args(&["delegation", dir.path().to_str().unwrap()]), &env);
        assert_eq!((out.code, out.stdout), (0, b"on\n".to_vec()));
    }

    #[test]
    fn a_file_that_asks_in_an_unapproved_folder_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/anthology.json"),
            r#"{"delegation": false}"#,
        )
        .unwrap();
        let env = Env::default().with_os("HOME", dir.path());
        let out = run_with(&args(&["delegation", dir.path().to_str().unwrap()]), &env);
        assert_eq!(out.stdout, b"ignored\n");
        // the other key is not asked for
        let out = run_with(
            &args(&["routing_guide", dir.path().to_str().unwrap()]),
            &env,
        );
        assert_eq!(out.stdout, b"on\n");
    }

    #[test]
    fn bad_arguments_are_usage() {
        let env = Env::default();
        for bad in [
            &[][..],
            &["delegation"],
            &["nope", "/x"],
            &["delegation", "/x", "y"],
        ] {
            assert_eq!(run_with(&args(bad), &env).code, 2, "{bad:?}");
        }
    }
}
