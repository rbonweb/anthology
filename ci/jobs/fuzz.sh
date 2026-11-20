#!/bin/sh
# CI job `fuzz` (design §14.1). Every cargo-fuzz target for a bounded time (weekly, fuzz.yml),
# on the nightly fuzz/rust-toolchain.toml pins.
set -eu

# FUZZ_SECONDS: per target (default 600, design §13.5).
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root/fuzz"
seconds=${FUZZ_SECONDS:-600}
rustup toolchain install
cargo install cargo-fuzz --locked --version 0.13.2
targets=$(cargo fuzz list)
[ -n "$targets" ] || { echo "fuzz: no targets" >&2; exit 1; }
for target in $targets; do
    echo "fuzz: $target for ${seconds}s"
    cargo fuzz run "$target" -- -max_total_time="$seconds"
done
