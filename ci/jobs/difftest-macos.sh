#!/bin/sh
# CI job `difftest-macos` (design §14.1, m-9): every ported difftest surface against python.org
# 3.12 (actions/setup-python, already in the job), natively on the macos-15 runner, gating.
# `--all` picks up each surface as its lane ports it, so lanes never edit this script; a surface
# that cannot run on macOS sets `ON_MACOS = false` in its own module (printed as `skip`).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
rustup toolchain install
cargo --version
python3 --version
python3 -I port/difftest/oracle/_driver.py --self-test
cargo build -p difftest --locked
target/debug/difftest --all --gen "${DIFFTEST_GEN:-10000}" --python python3
