#!/bin/sh
# CI job `ledger` (design §13.3, §14.1): `anthology-dev ledger check --report` against a fresh
# extraction, so a Python test or a lookaround pattern can never be dropped silently.
#
#   ci/jobs/ledger.sh [CHECK ARGS]   # e.g. --files tests/test_setup.py [--task RT4]: a lane's DoD
#   ci/jobs/ledger.sh --write        # regenerate the id list, row counts and regex inventory and
#                                    #   add `todo` rows (after a build→rust merge changes tests/)
#
# 1. The pinned Python 3.12 image (with tests/requirements.txt) runs port/extract/: the run ids
#    (test_ids.py), the subTest row counts (rows.py runs the suite once) and the regex inventory
#    (regex_inventory.py), into target/ledger-fresh/.
# 2. The pinned Rust image runs `anthology-dev ledger` (scripts/dev.sh), which compares them with
#    the committed port/ files and checks every port/ledger/<module>.tsv.
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
python port/extract/test_ids.py --out "$FRESH/python-test-ids.txt"
python port/extract/rows.py --counts "$FRESH/python-test-rows.tsv"
python port/extract/regex_inventory.py --out "$FRESH/regex-inventory.tsv"
chown -R "$HOST_UID:$HOST_GID" "$FRESH"
'

if [ "${1:-}" = "--write" ]; then
    shift
    exec "$root/scripts/dev.sh" ledger ids --fresh "$fresh" --write "$@"
fi
exec "$root/scripts/dev.sh" ledger check --report --fresh "$fresh" "$@"
