#!/usr/bin/env bash
# Runs anthology-dev (repository tooling) in the pinned Rust image: there is no host Rust.
#
#   scripts/dev.sh <command> [args]     # e.g. scripts/dev.sh ledger check
#
# The container is the one scripts/rust-test.sh prepares; the repository is mounted at /w.
#
# Stdout carries the tool's own output and nothing else, so callers can capture it
# (`old_pin=$(scripts/dev.sh vendor set-rtk-upstream …)`). The container's setup and the build
# (apt-get, rustup, cargo) go to stderr: the tool writes its stdout to a file under target/,
# which is replayed once the tool exits, with the tool's exit status. Stdin is passed through.
# ANTHOLOGY_RUST_TEST replaces scripts/rust-test.sh (a test seam).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
runner=${ANTHOLOGY_RUST_TEST:-$root/scripts/rust-test.sh}

mkdir -p "$root/target"
out=$(mktemp "$root/target/dev-stdout.XXXXXX")
trap 'rm -f "$out"' EXIT
# The container runs as the checkout's owner, or uid 1000 for a root-owned checkout
# (rust-test.sh); only then may it differ from this file's owner, which must let it write.
owner=$(stat -c %u "$root" 2>/dev/null || stat -f %u "$root")
if [ "$owner" = 0 ] || [ "$owner" != "$(id -u)" ]; then
    chmod 622 "$out"
fi
rel=${out#"$root"/}

status=0
# shellcheck disable=SC2016 # expanded by the container shell, not this one
"$runner" run sh -c 'out=$1; shift; exec cargo run --locked --quiet -p anthology-dev -- "$@" >"$out"' \
    dev "$rel" "$@" >&2 || status=$?
cat "$out"
exit "$status"
