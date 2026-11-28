#!/bin/sh
# CI job `check-repo-corpus` (check-repo plan, Task 15): `anthology check-repo` scans every
# repository in ci/check-repo-corpus.tsv, each pinned to a commit, and must find no HIGH or
# CRITICAL finding and take no more than 5000 ms ("no false alarms", "speed": plan Global
# Constraints). Run by .github/workflows/check-repo-corpus.yml.
#
#   ci/jobs/check-repo-corpus.sh                    build the release binary in the pinned Rust
#                                                   image (scripts/rust-test.sh) and scan the corpus
#                                                   there
#   ci/jobs/check-repo-corpus.sh --run BIN [TSV]    scan the corpus with BIN (what the first form
#                                                   runs in the image); TSV defaults to the file above
#
# The scan is `check-repo --json --no-tools --rev <rev> --into <work>/<n> <url>`: a shallow fetch into
# a quarantine folder and a read of its files. The corpus repos are data: nothing of them is
# installed, built or run, and no optional tool runs. Every row is scanned before the job fails, so
# one run shows all the bad rows. Output lines are indented or start with `corpus:`, so nothing a
# repo contains can start a workflow command.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
limit_ms=5000

usage() {
    echo "usage: ci/jobs/check-repo-corpus.sh [--run BIN [TSV]]" >&2
    exit 2
}

# Reads one JSON report ($1) and prints a summary, then every MEDIUM and worse finding. Exits 1 when
# the report holds a HIGH or CRITICAL finding or took longer than $2 ms, 2 when it is not JSON.
summarize='
const fs = require("fs");
const file = process.argv[1];
const limit = Number(process.argv[2]);
let j;
try {
  j = JSON.parse(fs.readFileSync(file, "utf8"));
} catch (e) {
  console.log("  no JSON report: " + String(e.message).split("\n")[0]);
  process.exit(2);
}
const c = j.counts || {};
const crit = Number(c.CRITICAL) || 0;
const high = Number(c.HIGH) || 0;
const ms = Number(j.scan_ms);
console.log("  files=" + Number(j.files) + " scan_ms=" + ms + " CRITICAL=" + crit + " HIGH=" + high +
  " MEDIUM=" + (Number(c.MEDIUM) || 0) + " exit=" + Number(j.exit));
for (const f of j.findings || []) {
  if (["CRITICAL", "HIGH", "MEDIUM"].includes(f.severity)) {
    console.log("  " + f.severity + " " + String(f.id) + " " + String(f.rule) + " " +
      JSON.stringify(String(f.path)) + ":" + Number(f.line));
  }
}
let bad = 0;
if (crit + high > 0) {
  console.log("  FAIL: " + (crit + high) + " HIGH or CRITICAL finding(s)");
  bad = 1;
}
if (!(ms <= limit)) {
  console.log("  FAIL: scan_ms " + ms + " is over " + limit);
  bad = 1;
}
process.exit(bad);
'

run_corpus() {
    bin=$1
    tsv=$2
    [ -x "$bin" ] || { echo "check-repo-corpus: $bin is not an executable" >&2; exit 1; }
    [ -f "$tsv" ] || { echo "check-repo-corpus: $tsv is missing" >&2; exit 1; }
    tab=$(printf '\t')
    # Every row is checked before any scan starts, so a typo costs no network time.
    rows=0
    while IFS="$tab" read -r url rev extra; do
        case $url in '' | '#'*) continue ;; esac
        case $url in
            https://*[!A-Za-z0-9._/:@~+%-]* | https://) echo "check-repo-corpus: odd URL: $url" >&2; exit 1 ;;
            https://*) ;;
            *) echo "check-repo-corpus: not an https URL: $url" >&2; exit 1 ;;
        esac
        case $rev in
            *[!0-9a-f]* | '') echo "check-repo-corpus: $url: the rev is not a lowercase SHA" >&2; exit 1 ;;
        esac
        if [ "${#rev}" -ne 40 ] || [ -n "$extra" ]; then
            echo "check-repo-corpus: $url: a row is <url><tab><40-character commit>" >&2
            exit 1
        fi
        rows=$((rows + 1))
    done <"$tsv"
    if [ "$rows" -eq 0 ]; then
        echo "check-repo-corpus: $tsv has no rows" >&2
        exit 1
    fi

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    n=0
    failed=0
    while IFS="$tab" read -r url rev _; do
        case $url in '' | '#'*) continue ;; esac
        n=$((n + 1))
        echo "corpus: $url at $rev"
        code=0
        "$bin" check-repo --json --no-tools --rev "$rev" --into "$work/$n" "$url" \
            >"$work/$n.json" 2>"$work/$n.err" || code=$?
        if [ -s "$work/$n.err" ]; then
            head -n 5 "$work/$n.err" | sed 's/^/  stderr: /'
        fi
        bad=0
        if [ "$code" -ne 0 ]; then
            echo "  FAIL: exit $code"
            bad=1
        fi
        node -e "$summarize" "$work/$n.json" "$limit_ms" || bad=1
        failed=$((failed + bad))
    done <"$tsv"

    if [ "$failed" -ne 0 ]; then
        echo "check-repo-corpus: $failed of $rows repositories failed"
        exit 1
    fi
    echo "check-repo-corpus: $rows repositories clean"
}

case ${1:-} in
    --run)
        [ "$#" -ge 2 ] && [ "$#" -le 3 ] || usage
        run_corpus "$2" "${3:-$root/ci/check-repo-corpus.tsv}"
        exit 0
        ;;
    '') ;;
    *) usage ;;
esac

# The image has git, node and the pinned toolchain; the scan runs there, outside this host.
# shellcheck disable=SC2016 # expanded by the container shell, not this one
exec "$root/scripts/rust-test.sh" run sh -c '
set -eu
cargo build --release --locked -p anthology-cli
exec sh ci/jobs/check-repo-corpus.sh --run /target/release/anthology
'
