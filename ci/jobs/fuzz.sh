#!/bin/sh
# CI job `fuzz` (design §13.5): every cargo-fuzz target, on the nightly fuzz/rust-toolchain.toml
# pins. Weekly in fuzz.yml; `fuzz-build` in ci.yml runs it per PR with FUZZ_RUNS.
#
#   FUZZ_SECONDS  seconds per filled target (default 600)
#   FUZZ_RUNS     when set: a fixed number of runs per target instead (the PR smoke)
#
# A target that is still F1's stub runs for 10 seconds only.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root/fuzz"
seconds=${FUZZ_SECONDS:-600}
rustup toolchain install
cargo install cargo-fuzz --locked --version 0.13.2
targets=$(cargo fuzz list)
[ -n "$targets" ] || { echo "fuzz: no targets" >&2; exit 1; }
for target in $targets; do
    cargo fuzz build "$target"
done
for target in $targets; do
    if [ -n "${FUZZ_RUNS:-}" ]; then
        limit="-runs=$FUZZ_RUNS"
    elif grep -q 'stub until its lane fills it' "fuzz_targets/$target.rs"; then
        limit=-max_total_time=10
    else
        limit="-max_total_time=$seconds"
    fi
    echo "fuzz: $target ($limit)"
    cargo fuzz run "$target" -- "$limit"
done
