#!/bin/sh
# Interim differential run for the shell.* surfaces until F4's harness lands (SH lead ruling R8); the lead
# deletes it once crates/difftest/src/surfaces/shell_*.rs implement F4's Surface.
#
#   port/difftest/shell_interim.sh <surface> <seed> <n> [--no-seeds]
#
# Builds the Rust driver (host cargo, or RUST_IN_DOCKER=1 for the pinned rust image), writes the cases, runs
# guard_rules.py on them in the pinned Python 3.12 image (always: the host Python's Unicode differs) and Rust
# on the host, and compares the two result streams byte for byte. Prints the first differences readably and
# exits 1 when there is any. Work files go to target/shell-interim/.
set -eu

[ "$#" -ge 3 ] || { echo "usage: $0 <surface> <seed> <n> [--no-seeds]" >&2; exit 2; }
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
image=$(awk '$1 == "FROM" {print $2; exit}' ci/images/python.Dockerfile)
work=target/shell-interim
mkdir -p "$work"
tag="$1-$2-$3"

if [ "${RUST_IN_DOCKER:-}" = 1 ]; then
    scripts/rust-test.sh run cargo build --locked --quiet -p difftest --example shell_interim
    bin=target/debug/examples/shell_interim
    run_rust() { scripts/rust-test.sh run sh -c "$bin $*"; }
else
    CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2} cargo build --locked --quiet --release -p difftest \
        --example shell_interim
    bin=target/release/examples/shell_interim
    run_rust() { "$bin" "$@"; }
fi

start=$(date +%s)
"$bin" gen "$@" > "$work/$tag.cases.jsonl"
cases=$(wc -l < "$work/$tag.cases.jsonl" | tr -d ' ')
docker run --rm -i -e PYTHONDONTWRITEBYTECODE=1 -v "$root":/w:ro -w /w "$image" \
    python -I port/difftest/oracle/shell.py --batch < "$work/$tag.cases.jsonl" > "$work/$tag.py.jsonl"
"$bin" run < "$work/$tag.cases.jsonl" > "$work/$tag.rs.jsonl"
end=$(date +%s)

if cmp -s "$work/$tag.py.jsonl" "$work/$tag.rs.jsonl"; then
    echo "shell_interim: $1 seed $2: $cases cases, 0 diffs ($((end - start)) s)"
    exit 0
fi
docker run --rm -i -v "$root":/w:ro -w /w "$image" python -I -c '
import json, sys
cases, py, rs = (open(p, encoding="utf-8").read().splitlines() for p in sys.argv[1:4])
diffs = [(c, p, r) for c, p, r in zip(cases, py, rs) if p != r]
print("shell_interim: %d cases, %d diffs" % (len(cases), len(diffs)))
for c, p, r in diffs[:int(sys.argv[4])]:
    c, p, r = json.loads(c), json.loads(p)["out"], json.loads(r)["out"]
    print("--- %s" % c["id"])
    print("case:   %s" % ascii(c["data"]))
    for name, out in (("python", p), ("rust", r)):
        body = json.loads(out["stdout"]) if out.get("stdout") else out
        print("%-7s %s" % (name + ":", ascii(body)))
' "$work/$tag.cases.jsonl" "$work/$tag.py.jsonl" "$work/$tag.rs.jsonl" "${SHOW:-5}"
exit 1
