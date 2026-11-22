#!/bin/sh
# CI job `rust-lint` (design §14.1). cargo fmt --check and clippy -D warnings, in the pinned
# Rust image.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
exec "$root/scripts/rust-test.sh" lint
