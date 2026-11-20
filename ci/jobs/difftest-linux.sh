#!/bin/sh
# CI job `difftest-linux` (design §14.1). The difftest in the pinned Python 3.12 image. F4 adds
# `--all --gen 10000` over static musl binaries.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
image=$(awk '$1 == "FROM" {print $2; exit}' "$root/ci/images/python.Dockerfile")

# F1's oracle: the guard data, the G6 secret patterns and the roster equal the Python values.
docker run --rm -e PYTHONDONTWRITEBYTECODE=1 -v "$root":/w:ro -w /w "$image" \
    python port/difftest/oracle/data.py
