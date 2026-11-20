#!/bin/sh
# CI job `rust-test-macos` (design §14.1). cargo test --workspace --locked on the macOS runner,
# with the toolchain rust-toolchain.toml names: plain, then with the test-only features (m-R10).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
rustup toolchain install
cargo --version
cargo test --workspace --locked
cargo test --workspace --locked --all-features
