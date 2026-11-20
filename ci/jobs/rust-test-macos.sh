#!/bin/sh
# CI job `rust-test-macos` (design §14.1). cargo test --workspace --locked on the macOS runner,
# with the toolchain rust-toolchain.toml names: plain, then with the test-only features (m-R10).
#
# Then the ledger's macOS ground truth (F4a, controller ruling; this F1 file is touched for it):
# what cargo compiles and would skip here, `cargo test -- --list [--ignored]`, plain and with
# --all-features, and `anthology-dev ledger check --listings … --platform macos`, so every ported
# Rust target runs on macOS as well as Linux (ci/jobs/ledger.sh checks Linux).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
rustup toolchain install
cargo --version
cargo test --workspace --locked
cargo test --workspace --locked --all-features

listings=target/ledger-macos
rm -rf "$listings"
mkdir -p "$listings"
for mode in plain all ignored-plain ignored-all; do
    out="$listings/rust-tests-$mode.txt"
    features=
    ignored=
    case $mode in *all) features=--all-features ;; esac
    case $mode in ignored-*) ignored=--ignored ;; esac
    # shellcheck disable=SC2086 # $features and $ignored are empty or one option each
    if ! cargo test --workspace --locked $features -- --list $ignored >"$out" 2>&1; then
        cat "$out" >&2
        echo "rust-test-macos: cargo test --list ($mode) failed" >&2
        exit 1
    fi
done
cargo run --locked --quiet -p anthology-dev -- \
    ledger check --report --listings "$listings" --platform macos
