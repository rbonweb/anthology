#!/bin/sh
# CI job `difftest-macos` (design §14.1, m-9): the difftest surfaces against python.org 3.12
# (actions/setup-python, already in the job), natively on the macos-15 runner, gating. The
# CLI-level surfaces join this list as their lanes port them; today it runs the F2/F3 library
# surfaces, which also cover macOS realpaths (/private/var) and file modes.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
rustup toolchain install
cargo --version
python3 --version
cargo build -p difftest --locked
target/debug/difftest --surface pycompat.json,core.state,core.lock --gen 10000 --python python3
