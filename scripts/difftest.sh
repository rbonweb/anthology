#!/bin/sh
# Runs the difftest (design §13.2) against the Python oracle in the pinned Python 3.12 image.
#
#   scripts/difftest.sh --list
#   scripts/difftest.sh --surface pycompat.json,core.state --gen 10000
#   scripts/difftest.sh --all --gen 10000 --report /out/report.json
#
# 1. A static musl `difftest` is built in the pinned Rust image (scripts/rust-test.sh run), for
#    the Docker host's architecture. There is no host Rust (GC-18); CARGO_BUILD_JOBS passes
#    through. The image has no musl-gcc, so the C parts (ring, SQLite) are compiled by its gcc
#    (CC_<target>=gcc), which works for a static musl link.
# 2. It runs in the image of ci/images/python.Dockerfile with the repository mounted read-only
#    at /w, the build volume of scripts/rust-test.sh read-only at /target, and
#    target/difftest/ writable at /out (for --report and temporary --divergences folders).
#    The oracle is that image's python3. Every argument goes to difftest.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
case $(docker version --format '{{.Server.Arch}}') in
    amd64 | x86_64) triple=x86_64-unknown-linux-musl ;;
    arm64 | aarch64) triple=aarch64-unknown-linux-musl ;;
    *)
        echo "difftest: unsupported Docker architecture" >&2
        exit 1
        ;;
esac
cc_var="CC_$(printf '%s' "$triple" | tr - _)"
"$root/scripts/rust-test.sh" run env "$cc_var=gcc" \
    cargo build --locked --quiet -p difftest --target "$triple"

# The target volume scripts/rust-test.sh uses for this checkout (same name rule).
checkout=$(printf '%s' "$root" | cksum | cut -d' ' -f1)
image=$(awk '$1 == "FROM" {print $2; exit}' "$root/ci/images/python.Dockerfile")
mkdir -p "$root/target/difftest"
exec docker run --rm \
    -v "$root":/w:ro \
    -v "anthology-rust-target-$checkout":/target:ro \
    -v "$root/target/difftest":/out \
    -w /w "$image" "/target/$triple/debug/difftest" --python python3 "$@"
