#!/bin/sh
# CI jobs `rust-test-linux` and `rust-test-macos`: the gate over their shards in ci.yml, so the
# two job names stay the ones to read. Passes when rust-scope succeeded and either asked for the
# full suite and every shard passed, or found release metadata only and the shards were skipped
# (rust-test-linux then runs repo-tests itself). Anything else fails, with the reason.
#
#   ci/jobs/rust-gate.sh JOB SCOPE_RESULT FULL SHARDS_RESULT
set -eu

if [ "$#" -ne 4 ]; then
    echo "usage: ci/jobs/rust-gate.sh JOB SCOPE_RESULT FULL SHARDS_RESULT" >&2
    exit 2
fi
job=$1
scope=$2
full=$3
shards=$4

fail() {
    echo "$job: $1" >&2
    exit 1
}

[ "$scope" = success ] || fail "rust-scope ended with '$scope'"
case $full in
    true)
        [ "$shards" = success ] || fail "the shards ended with '$shards'"
        echo "$job: every shard passed"
        ;;
    false)
        [ "$shards" = skipped ] || fail "release metadata only, but the shards ended with '$shards'"
        echo "$job: release metadata only; the shards were skipped"
        ;;
    *) fail "rust-scope answered full='$full'" ;;
esac
