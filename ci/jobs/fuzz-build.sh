#!/bin/sh
# CI job `fuzz-build` (owner: F1): every fuzz target compiles on the pinned nightly and survives a
# short smoke run, on every PR. The long runs are fuzz.yml's (weekly).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
FUZZ_RUNS=${FUZZ_RUNS:-1000} exec "$root/ci/jobs/fuzz.sh"
