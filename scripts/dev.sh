#!/usr/bin/env bash
# Runs anthology-dev (repository tooling) in the pinned Rust image: there is no host Rust.
#
#   scripts/dev.sh <command> [args]     # e.g. scripts/dev.sh ledger check
#
# The container is the one scripts/rust-test.sh prepares; the repository is mounted at /w.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
exec "$root/scripts/rust-test.sh" run cargo run --locked --quiet -p anthology-dev -- "$@"
