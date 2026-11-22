#!/bin/sh
# CI job `rust-test-linux` (design §14.1). cargo test --workspace --locked in the pinned Rust
# image (with git, busybox and node).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
exec "$root/scripts/rust-test.sh" test
