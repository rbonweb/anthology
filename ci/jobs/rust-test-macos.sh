#!/bin/sh
# CI job `rust-test-macos` (design §14.1). cargo test --workspace --locked on the macOS runner,
# with the toolchain rust-toolchain.toml names.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
rustup toolchain install
cargo --version
cargo test --workspace --locked
