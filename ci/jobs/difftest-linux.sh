#!/bin/sh
# CI job `difftest-linux` (design §14.1). The difftest in the pinned Python 3.12 image. F4 adds
# `--all --gen 10000` over static musl binaries.
#
# Oracle scripts run as `python -I` (isolated: the script's folder is not on sys.path), because
# port/difftest/oracle/json.py shadows the standard library's json for anything run from there.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
image=$(awk '$1 == "FROM" {print $2; exit}' "$root/ci/images/python.Dockerfile")

# F1's oracle: the guard data, the G6 secret patterns and the roster equal the Python values.
docker run --rm -v "$root":/w:ro -w /w "$image" \
    python -I port/difftest/oracle/data.py

# F2's golden corpus: regenerated from the oracle, it must equal the committed file byte for
# byte (cargo test replays the committed file).
docker run --rm -v "$root":/w:ro -w /w "$image" sh -ec '
    python -I port/difftest/oracle/json.py --golden /tmp/json.jsonl
    golden=crates/anthology-pycompat/tests/golden/json.jsonl
    cmp /tmp/json.jsonl "$golden" || {
        echo "difftest: $golden is stale; regenerate it with" \
            "python3 -I port/difftest/oracle/json.py --golden $golden (3.12 image)" >&2
        exit 1
    }'
echo "difftest-linux: the json golden corpus matches its oracle"
