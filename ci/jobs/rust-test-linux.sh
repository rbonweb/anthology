#!/bin/sh
# CI job `rust-test-linux` (design §14.1): cargo test --workspace --locked in the pinned Rust
# image (with git, busybox and node), as four parallel shards that ci.yml gates on.
#
#   ci/jobs/rust-test-linux.sh plain|all replay|rest   # one shard
#   ci/jobs/rust-test-linux.sh release-metadata        # repo-tests only (ci/jobs/rust-scope.sh)
#
# `plain` is cargo test --workspace --locked and `all` the same with --all-features (the
# test-only features, m-R10), as scripts/rust-test.sh test runs them one after the other.
# `replay` runs only the difftest golden replay, about 9 of the 16 minutes of one run; `rest`
# runs every other test. The four shards together run what scripts/rust-test.sh test runs.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
replay=the_committed_goldens_replay_clean

usage() {
    echo "usage: ci/jobs/rust-test-linux.sh plain|all replay|rest | release-metadata" >&2
    exit 2
}

case ${1:-} in
    plain) features= ;;
    all) features=--all-features ;;
    release-metadata)
        [ "$#" -eq 1 ] || usage
        exec "$root/scripts/rust-test.sh" run cargo test --locked -p repo-tests
        ;;
    *) usage ;;
esac
[ "$#" -eq 2 ] || usage

case $2 in
    rest)
        # --exact makes --skip drop that one test name, not every name that contains it.
        # shellcheck disable=SC2086 # $features is empty or one option
        exec "$root/scripts/rust-test.sh" run cargo test --workspace --locked $features \
            -- --exact --skip "$replay"
        ;;
    replay)
        # The whole workspace stays selected, so --all-features unifies every member's
        # features as in the full run. A filter that matches no test fails instead of passing.
        # shellcheck disable=SC2016,SC2086 # expanded by the container shell; $features as above
        exec "$root/scripts/rust-test.sh" run sh -c '
set -eu
replay=$1
shift
n=$(cargo test --workspace --locked "$@" --test golden -- --list --exact "$replay" |
    grep -c "^$replay: test$" || true)
if [ "$n" -ne 1 ]; then
    echo "rust-test-linux: $replay is listed $n times, not once" >&2
    exit 1
fi
exec cargo test --workspace --locked "$@" --test golden -- --exact "$replay"
' rust-test-linux "$replay" $features
        ;;
    *) usage ;;
esac
