#!/bin/sh
# CI job `atl-test-macos`: atl's own cargo test suite on the macOS runner, with the toolchain
# atl/rust-toolchain.toml names. atl-test (scripts/atl-test.sh) runs the same suite on Linux in the
# pinned image, so it cannot see behavior that only macOS has: the /private/var symlinks behind
# temporary folders failed three atl tests in atl-release.yml's macOS builds after a pull request
# had passed. Runs when atl/ changes and on every push to `build` and `main` (ci.yml).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root/atl"
rustup toolchain install
cargo --version
cargo test --locked
