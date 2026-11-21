//! The `git` the `setup` and `hook.session-start` surfaces put on `PATH` (not a surface).
//!
//! The oracle image (`ci/images/python.Dockerfile`, python:3.12-slim) has no git, so without
//! this every repo case would be "not a git repo" on both sides. Both sides run this one script
//! as `git` (it is a tree entry of the case, and both prepend `@ROOT@/gitbin` to `PATH`), so the
//! memory code that calls git is real on both sides: Python's `memory_sync._git` and
//! `anthology_core::git::RealGit` spawn it, and their parsing of its output is compared.
//!
//! It answers the git commands the memory code runs, from the work tree's files:
//! - a folder under the case root (`@ROOT@`, expanded by the materializer) holding `.git` (a
//!   folder or a file) is a work tree; `--show-toplevel` is its real path and
//!   `--git-common-dir` its `.git`. Nothing outside the case root is a repo, so a command run
//!   from the test's own checkout never finds, or writes to, that checkout;
//! - `.git/remotes` lists `<name> <url>` lines (`remote`, `remote get-url`,
//!   `config --get remote.<name>.url`);
//! - `.git/tracked` lists the tracked paths (`ls-files --error-unmatch`);
//! - `.git/info/exclude` lines `<path>` or `/<path>` ignore a path (`check-ignore -q`).
//!
//! Anything else exits 129 with `fake git: unsupported` on stderr.

use anthology_pycompat::json::{Int, Map, Value};

/// The folder, under the case root, that holds the script.
pub const DIR: &str = "gitbin";

/// The script (POSIX sh: dash in the oracle image).
pub const SCRIPT: &str = r#"#!/bin/sh
while [ "$1" = -c ]; do shift 2; done
top=$(pwd -P)
case "$top" in
'@ROOT@'/*) ;;
*) top=/ ;;
esac
while [ ! -e "$top/.git" ]; do
    if [ "$top" = '@ROOT@' ] || [ "$top" = / ]; then
        echo "fatal: not a git repository (or any of the parent directories): .git" >&2
        exit 128
    fi
    top=$(dirname "$top")
done
url() {
    [ -f "$top/.git/remotes" ] || return 1
    while read -r n u; do
        if [ "$n" = "$1" ]; then
            printf '%s\n' "$u"
            return 0
        fi
    done < "$top/.git/remotes"
    return 1
}
case "$*" in
"rev-parse --show-toplevel") printf '%s\n' "$top" ;;
"rev-parse --path-format=absolute --git-common-dir" | "rev-parse --git-common-dir")
    printf '%s\n' "$top/.git" ;;
"rev-parse --git-path info/exclude") printf '%s\n' "$top/.git/info/exclude" ;;
remote)
    if [ -f "$top/.git/remotes" ]; then
        while read -r n u; do printf '%s\n' "$n"; done < "$top/.git/remotes"
    fi ;;
"remote get-url "*)
    url "$3" || { echo "error: No such remote '$3'" >&2; exit 2; } ;;
"config --get remote."*.url)
    name=${3#remote.}
    url "${name%.url}" || exit 1 ;;
"ls-files --error-unmatch -- "*)
    if [ -f "$top/.git/tracked" ] && grep -qxF -- "$4" "$top/.git/tracked"; then
        printf '%s\n' "$4"
    else
        echo "error: pathspec '$4' did not match any file(s) known to git" >&2
        exit 1
    fi ;;
"check-ignore -q -- "*)
    ex="$top/.git/info/exclude"
    if [ -f "$ex" ] && { grep -qxF -- "$4" "$ex" || grep -qxF -- "/$4" "$ex"; }; then
        exit 0
    fi
    exit 1 ;;
*) echo "fake git: unsupported: $*" >&2; exit 129 ;;
esac
"#;

fn entry(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_string(), v);
    }
    Value::Object(m)
}

/// The tree entries that install the script at `<root>/gitbin/git`.
pub fn entries() -> Vec<Value> {
    let p = Value::Str(format!("{DIR}/git"));
    vec![
        entry(vec![
            ("t", Value::Str("file".into())),
            ("p", p.clone()),
            ("text", Value::Str(SCRIPT.into())),
        ]),
        entry(vec![
            ("t", Value::Str("chmod".into())),
            ("p", p),
            ("mode", Value::Int(Int::Small(0o755))),
        ]),
    ]
}

/// The case's `PATH`: the script's folder first, then `rest`.
pub fn path(root: &str, rest: &str) -> String {
    format!("{root}/{DIR}:{rest}")
}
