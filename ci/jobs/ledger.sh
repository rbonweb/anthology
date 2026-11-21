#!/bin/sh
# CI job `ledger` (design §13.3, §14.1): `anthology-dev ledger check --report` against what cargo
# really runs, so a ported test, a Rust target or a regex pattern can never be dropped silently.
#
#   ci/jobs/ledger.sh [CHECK ARGS]   # e.g. --files tests/test_setup.py --task RT4
#
# The id list, row counts and regex inventory under port/ are final: CO2 deleted the Python
# suite and its extractor (port/extract/), so there is nothing left to extract. The pinned Rust
# image lists what cargo compiles and runs, `cargo test --workspace --locked -- --list` and what
# it would skip, `-- --list --ignored`, each plain and with --all-features (as rust-test.sh runs
# them), into target/ledger-fresh/, then runs `anthology-dev ledger check --listings DIR
# --platform linux`: it requires every `ported` Rust target in a listing and in no ignored
# listing, and checks every port/ledger/<module>.tsv and port/regex-resolved.tsv (every
# `unresolved` inventory row has a resolution there whose Rust test is listed).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

if [ "${1:-}" = "--write" ]; then
    echo "ledger: --write needs port/extract/, which the cutover deleted; the id list is final" >&2
    exit 2
fi

fresh=target/ledger-fresh
rm -rf "$fresh"
mkdir -p "$fresh"

against=--listings

# shellcheck disable=SC2016 # expanded by the container shell, not this one
exec "$root/scripts/rust-test.sh" run sh -c '
set -eu
fresh=$1
against=$2
shift 2
for mode in plain all ignored-plain ignored-all; do
    out="$fresh/rust-tests-$mode.txt"
    features=
    ignored=
    case $mode in *all) features=--all-features ;; esac
    case $mode in ignored-*) ignored=--ignored ;; esac
    # shellcheck disable=SC2086 # $features and $ignored are empty or one option each
    if ! cargo test --workspace --locked $features -- --list $ignored >"$out" 2>&1; then
        cat "$out" >&2
        echo "ledger: cargo test --list ($mode) failed" >&2
        exit 1
    fi
done
if [ "$against" = --listings ]; then
    set -- --platform linux "$@"
fi
exec cargo run --locked --quiet -p anthology-dev -- ledger check --report "$against" "$fresh" "$@"
' ledger "$fresh" "$against" "$@"
