#!/bin/sh
# CI job `ledger` (design §13.3, §14.1): `anthology-dev ledger check --report` against a fresh
# extraction, so a Python test, a Rust target or a regex pattern can never be dropped silently.
#
#   ci/jobs/ledger.sh [CHECK ARGS]   # e.g. --files tests/test_setup.py --task RT4: a lane's DoD
#   ci/jobs/ledger.sh --write        # regenerate the id list, row counts and regex inventory and
#                                    #   add `todo` rows (after a build→rust merge changes tests/)
#
# 1. The pinned Python 3.12 image (with tests/requirements.txt) runs port/extract/: the run ids
#    (test_ids.py), the subTest row counts (rows.py runs the suite once) and the regex inventory
#    (regex_inventory.py, after its self-test), into target/ledger-fresh/.
# 2. The pinned Rust image lists what cargo compiles and runs, `cargo test --workspace --locked
#    -- --list`, plain and with --all-features (as rust-test.sh runs them), into the same
#    directory, then runs `anthology-dev ledger check`: it compares the extraction with the
#    committed port/ files, requires every `ported` Rust target in a listing, and checks every
#    port/ledger/<module>.tsv.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

image=$(awk '$1 == "FROM" {print $2; exit}' ci/images/python.Dockerfile)
if [ -z "$image" ]; then
    echo "ledger: no FROM line in ci/images/python.Dockerfile" >&2
    exit 1
fi

fresh=target/ledger-fresh
rm -rf "$fresh"
mkdir -p "$fresh"

# shellcheck disable=SC2016 # expanded by the container shell, not this one
docker run --rm -e PYTHONDONTWRITEBYTECODE=1 \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -e FRESH="$fresh" \
    -v "$root":/w -w /w "$image" sh -c '
set -eu
pip install -q --disable-pip-version-check --root-user-action=ignore --require-hashes -r tests/requirements.txt
python port/extract/regex_inventory.py --self-test
python port/extract/test_ids.py --out "$FRESH/python-test-ids.txt"
python port/extract/rows.py --counts "$FRESH/python-test-rows.tsv"
python port/extract/regex_inventory.py --out "$FRESH/regex-inventory.tsv"
chown -R "$HOST_UID:$HOST_GID" "$FRESH"
'

if [ "${1:-}" = "--write" ]; then
    shift
    exec "$root/scripts/dev.sh" ledger ids --fresh "$fresh" --write "$@"
fi

# shellcheck disable=SC2016 # expanded by the container shell, not this one
exec "$root/scripts/rust-test.sh" run sh -c '
set -eu
fresh=$1
shift
for mode in plain all; do
    out="$fresh/rust-tests-$mode.txt"
    features=
    [ "$mode" = plain ] || features=--all-features
    # shellcheck disable=SC2086 # $features is empty or one option
    if ! cargo test --workspace --locked $features -- --list >"$out" 2>&1; then
        cat "$out" >&2
        echo "ledger: cargo test --list ($mode) failed" >&2
        exit 1
    fi
done
exec cargo run --locked --quiet -p anthology-dev -- ledger check --report --fresh "$fresh" "$@"
' ledger "$fresh" "$@"
