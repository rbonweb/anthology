#!/bin/sh
# CI job `difftest-linux` (design §14.1). The difftest in the pinned Python 3.12 image: the
# oracle checks and golden corpora, then `--all --gen 10000` over a static musl difftest.
#
# Oracle scripts run as `python -I` (isolated: the script's folder is not on sys.path), because
# port/difftest/oracle/json.py shadows the standard library's json for anything run from there.
#
# After the cutover (CO2 deletes port/difftest/oracle/ and the Python images) there is no oracle:
# the job replays the frozen goldens instead (`anthology-dev golden check --all` in the Rust
# image), which fails for any ported surface without one. Design §14.2 removes this job at CO2;
# cargo test keeps the same replay (crates/difftest/tests/golden.rs).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
if [ ! -f "$root/port/difftest/oracle/_driver.py" ]; then
    echo "difftest-linux: no Python oracle (after the cutover): replaying the frozen goldens"
    exec "$root/scripts/dev.sh" golden check --all
fi
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

# F3's golden corpora (core.state, core.lock), checked the same way. The mount is read-only:
# core_state.py writes only in its own temporary folders, core_lock.py chdirs into them.
docker run --rm -v "$root":/w:ro -w /w "$image" sh -ec '
    for name in state lock; do
        python -I "port/difftest/oracle/core_$name.py" --golden "/tmp/$name.jsonl"
        golden="crates/anthology-core/tests/golden/$name.jsonl"
        cmp "/tmp/$name.jsonl" "$golden" || {
            echo "difftest: $golden is stale; regenerate it with" \
                "python3 -I port/difftest/oracle/core_$name.py --golden $golden (3.12 image)" >&2
            exit 1
        }
    done'
echo "difftest-linux: the core state and lock golden corpora match their oracles"

# F4: the oracle driver's own rules (a lone surrogate outside "python" is an error, a module's
# SystemExit or stdin read cannot break the protocol).
docker run --rm -v "$root":/w:ro -w /w "$image" \
    python -I port/difftest/oracle/_driver.py --self-test

# F4: every ported surface, its seeds and 10,000 generated cases, through the static musl
# difftest in the same 3.12 image (scripts/difftest.sh). 0 unexplained diffs or the job fails.
# Pull requests run fewer generated cases (CI sets DIFFTEST_GEN); batch merges and build/main run 10,000.
"$root/scripts/difftest.sh" --all --gen "${DIFFTEST_GEN:-10000}" --report /out/report.json
echo "difftest-linux: 0 unexplained diffs (report: target/difftest/report.json)"

# The harness self-test: a diff injected into core.lock's first case (golden:1) must fail
# (exit 1) with the real divergence files, and pass once a matching entry is added. The
# temporary divergences folder is target/difftest/inject, /out/inject in the container.
inject="$root/target/difftest/inject"
rm -rf "$inject"
mkdir -p "$inject"
cp "$root"/port/difftest/divergences/*.toml "$inject/"
log="$root/target/difftest/inject.log"
set +e
"$root/scripts/difftest.sh" --surface core.lock --gen 0 --inject-diff core.lock \
    --divergences /out/inject >"$log" 2>&1
code=$?
set -e
if [ "$code" -ne 1 ] || ! grep -q '^FAIL core.lock golden:1: unexplained diff' "$log"; then
    cat "$log" >&2
    echo "difftest-linux: an injected diff did not fail (exit $code)" >&2
    exit 1
fi
cat >>"$inject/core.lock.toml" <<'TOML'

[[divergence]]
class = "D-2"
case = "golden:1"
reason = "the injected diff of the harness self-test"
TOML
if ! "$root/scripts/difftest.sh" --surface core.lock --gen 0 --inject-diff core.lock \
    --divergences /out/inject >"$log" 2>&1; then
    cat "$log" >&2
    echo "difftest-linux: a matching divergence entry did not explain the injected diff" >&2
    exit 1
fi
echo "difftest-linux: an injected diff fails, and a matching divergence entry passes"

# CO2's tools against the real oracle, on a small sample: `difftest freeze` writes goldens from
# Python, `golden check` replays them with no oracle (0 unexplained), and an injected diff fails
# there too. The goldens go to target/difftest/golden-selftest (/out/golden-selftest), never to
# the committed crates/*/tests/golden/ (the real freeze is a CO2 step).
selftest="$root/target/difftest/golden-selftest"
rm -rf "$selftest"
if ! "$root/scripts/difftest.sh" freeze --sample --surface core.lock,core.state --gen 200 \
    --golden /out/golden-selftest >"$log" 2>&1 \
    || ! "$root/scripts/difftest.sh" golden check --surface core.lock,core.state \
        --golden /out/golden-selftest >>"$log" 2>&1; then
    cat "$log" >&2
    echo "difftest-linux: freeze, then golden check, did not pass on the sample" >&2
    exit 1
fi
set +e
"$root/scripts/difftest.sh" golden check --surface core.lock --golden /out/golden-selftest \
    --inject-diff core.lock >"$log" 2>&1
code=$?
set -e
if [ "$code" -ne 1 ] || ! grep -q '^FAIL core.lock golden:1: unexplained diff' "$log"; then
    cat "$log" >&2
    echo "difftest-linux: an injected diff did not fail golden check (exit $code)" >&2
    exit 1
fi
echo "difftest-linux: freeze and golden check agree on the sample, and an injected diff fails"
