#!/bin/sh
# CI jobs `rust-test-linux` and `rust-test-macos`: the gate over their shards in ci.yml, so the
# two job names stay the ones to read (the required checks). Passes when rust-scope succeeded and
# either asked for the shards (RUN true) and every shard passed, or needed none (RUN false: the
# pull request changed nothing the platform's tests cover) and they were skipped. Anything else
# fails, with the reason.
#
#   ci/jobs/rust-gate.sh JOB SCOPE_RESULT RUN SHARDS_RESULT
set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: ci/jobs/rust-gate.sh JOB SCOPE_RESULT RUN SHARDS_RESULT" >&2
    exit 2
fi
job=$1
scope=$2
run=$3
shards=$4

fail() {
    echo "$job: $1" >&2
    exit 1
}

[ "$scope" = success ] || fail "rust-scope ended with '$scope'"
case $run in
    true)
        [ "$shards" = success ] || fail "the shards ended with '$shards'"
        echo "$job: every shard passed"
        ;;
    false)
        [ "$shards" = skipped ] || fail "no shard was needed, but the shards ended with '$shards'"
        echo "$job: no shard was needed for this change; the shards were skipped"
        ;;
    *) fail "rust-scope answered run='$run'" ;;
esac
