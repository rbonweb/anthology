#!/bin/sh
# CI job `rust-test-linux` (design §14.1): cargo test in the pinned Rust image (with git, busybox
# and node), as parallel shards that ci.yml builds from the matrices of ci/jobs/rust-scope.sh.
#
#   PACKAGES='a b' ci/jobs/rust-test-linux.sh plain|all rest    # the tests of those crates
#   SURFACES=a,b   ci/jobs/rust-test-linux.sh plain|all replay  # the difftest golden replay
#
# `plain` is cargo test --locked and `all` the same with --all-features (the test-only features,
# m-R10), as scripts/rust-test.sh test runs them one after the other.
# `rest` runs every test of the named crates (the whole workspace when PACKAGES is empty) except
# the golden replay, which is one test per surface (`golden_replay::<surface>`, in
# crates/difftest/tests/golden.rs); `replay` runs the named surfaces and nothing else.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)

usage() {
    echo "usage: [PACKAGES='a b'] ci/jobs/rust-test-linux.sh plain|all rest" >&2
    echo "       SURFACES=a,b ci/jobs/rust-test-linux.sh plain|all replay" >&2
    exit 2
}

case ${1:-} in
    plain) features= ;;
    all) features=--all-features ;;
    *) usage ;;
esac
[ "$#" -eq 2 ] || usage
shard_features=$1

case $2 in
    rest)
        # The crates by name, so the tests of the crates a change reaches run, no more.
        set -- cargo test --locked
        if [ -n "${PACKAGES:-}" ]; then
            for p in $PACKAGES; do
                set -- "$@" -p "$p"
            done
        else
            set -- "$@" --workspace
        fi
        # shellcheck disable=SC2086 # $features is empty or one option
        exec "$root/scripts/rust-test.sh" run "$@" $features -- --skip golden_replay::
        ;;
    replay)
        [ -n "${SURFACES:-}" ] || usage
        tests=
        for s in $(printf '%s' "$SURFACES" | tr ',' ' '); do
            tests="$tests golden_replay::$(printf '%s' "$s" | tr '.-' '__')"
        done
        # The whole workspace stays selected, so --all-features unifies every member's features
        # as in the full run. A name that matches no test fails instead of passing.
        # shellcheck disable=SC2016,SC2086 # expanded by the container shell; $tests are words
        exec "$root/scripts/rust-test.sh" run sh -c '
set -eu
features=$1
shift
f=
if [ "$features" = all ]; then f=--all-features; fi
listing=$(cargo test --workspace --locked $f --test golden -- --list)
for t in "$@"; do
    n=$(printf "%s\n" "$listing" | grep -c "^$t: test\$" || true)
    if [ "$n" -ne 1 ]; then
        echo "rust-test-linux: $t is listed $n times, not once" >&2
        exit 1
    fi
done
exec cargo test --workspace --locked $f --test golden -- --exact "$@"
' rust-test-linux "$shard_features" $tests
        ;;
    *) usage ;;
esac
