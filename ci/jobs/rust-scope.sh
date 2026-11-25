#!/bin/sh
# CI job `rust-scope`: whether the Rust test jobs run the full suite. Prints `full=true` or
# `full=false` (a line for $GITHUB_OUTPUT) and the reason on stderr. Runs in the repository root.
#
#   ci/jobs/rust-scope.sh BASE HEAD
#
# full=false only when every file the pull request changes (BASE...HEAD, from the merge base) is
# release metadata:
#   - memory notes: .claude/memory/** and .claude/agent-memory/**;
#   - the changelog: CHANGELOG.md and changelog.d/**;
#   - a pinned binary: plugins/<plugin>/anthology.lock;
#   - a version and nothing else in plugins/<plugin>/.claude-plugin/plugin.json (the top-level
#     "version"), Cargo.toml (the version of a [package] or [workspace.package] table), and
#     Cargo.lock or fuzz/Cargo.lock (quoted package versions: a registry or git dependency that
#     moves also changes its checksum or source line, so it still counts as code).
# Anything else, a push run (no BASE), an unknown commit or an empty diff gives full=true. Then
# the rust-test-linux and rust-test-macos shards run every test; otherwise they are skipped and
# rust-test-linux runs repo-tests (the anthology_lock and atl_lock checks among them).
set -eu

full() {
    echo "rust-scope: the full suite: $1" >&2
    echo full=true
    exit 0
}

base=${1:-}
head=${2:-}
[ -n "$base" ] && [ -n "$head" ] || full "no pull request range (a push run)"
git cat-file -e "$base^{commit}" 2>/dev/null || full "unknown base commit $base"
git cat-file -e "$head^{commit}" 2>/dev/null || full "unknown head commit $head"
from=$(git merge-base "$base" "$head") || full "no merge base of $base and $head"
files=$(git diff --name-only --no-renames "$from" "$head" --) || full "git diff failed"
[ -n "$files" ] || full "no file changed"

# The text of file $2 at revision $1 with the lines a version bump may change ($3: toml, lock or
# json) each replaced by one placeholder line: a changed version compares equal, but a version
# line that is removed or added (or duplicated) leaves a different number of lines.
versionless() {
    git show "$1:$2" | awk -v kind="$3" '
        /^\[/ { table = $0 }
        kind == "toml" && (table == "[package]" || table == "[workspace.package]") &&
            /^version = "[^"]*"$/ { print "@version-line@"; next }
        kind == "lock" && /^version = "[^"]*"$/ { print "@version-line@"; next }
        kind == "json" && /^  "version": "[^"]*",?$/ { print "@version-line@"; next }
        { print }'
}

# Whether file $1 exists on both sides and differs in nothing but versions ($2 as above).
version_only() {
    git cat-file -e "$from:$1" 2>/dev/null && git cat-file -e "$head:$1" 2>/dev/null &&
        [ "$(versionless "$from" "$1" "$2")" = "$(versionless "$head" "$1" "$2")" ]
}

# Whether $1 is plugins/<one folder>/$2.
in_a_plugin() {
    name=${1#plugins/}
    [ "plugins/${name%%/*}/$2" = "$1" ]
}

set -f
IFS='
'
for f in $files; do
    case $f in
        .claude/memory/* | .claude/agent-memory/* | CHANGELOG.md | changelog.d/*) continue ;;
        plugins/*/anthology.lock)
            in_a_plugin "$f" anthology.lock && continue
            ;;
        plugins/*/.claude-plugin/plugin.json)
            in_a_plugin "$f" .claude-plugin/plugin.json && version_only "$f" json && continue
            ;;
        Cargo.toml) version_only "$f" toml && continue ;;
        Cargo.lock | fuzz/Cargo.lock) version_only "$f" lock && continue ;;
    esac
    full "$f is not release metadata"
done

echo "rust-scope: release metadata only: repo-tests instead of the full suite" >&2
echo full=false
