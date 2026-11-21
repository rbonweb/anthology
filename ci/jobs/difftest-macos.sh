#!/bin/sh
# CI job `difftest-macos` (design §14.1, m-9): every ported difftest surface against CPython
# 3.12, natively on the macos-15 runner, gating.
#
# The oracle must be the same CPython 3.12 patch release as the Linux image's
# (ci/images/python.Dockerfile, 3.12.16): tarfile's header parsing changed in the 3.12 security
# releases, and the port follows 3.12.16. actions/setup-python stops at 3.12.10 on macOS
# (python.org ships no macOS build of a security-only release), so this job fetches the
# python-build-standalone build of 3.12.16, pinned by sha256, and checks its version.
#
# `--all` picks up each surface as its lane ports it, so lanes never edit this script; a surface
# that cannot run on macOS sets `ON_MACOS = false` in its own module (printed as `skip`).
set -eu

PY_VERSION=3.12.16
PY_URL=https://github.com/astral-sh/python-build-standalone/releases/download/20261110/cpython-3.12.16%2B20261110-aarch64-apple-darwin-install_only.tar.gz
PY_SHA256=e956e60bc9465d577ef8062059aa05cd3a2665b4b8678a2ed24e432368330b8f

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"

if [ "$(uname -s) $(uname -m)" != "Darwin arm64" ]; then
    echo "difftest-macos: needs macOS on arm64" >&2
    exit 1
fi
pydir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/oracle-python.XXXXXX")
curl -fsSL --retry 3 -o "$pydir/python.tar.gz" "$PY_URL"
echo "$PY_SHA256  $pydir/python.tar.gz" | shasum -a 256 -c -
tar -xzf "$pydir/python.tar.gz" -C "$pydir"
# First on PATH, as setup-python put its own, for anything that runs python3.
PATH=$pydir/python/bin:$PATH
export PATH
python=$pydir/python/bin/python3
got=$("$python" -I -c 'import platform; print(platform.python_version())')
if [ "$got" != "$PY_VERSION" ]; then
    echo "difftest-macos: oracle is Python $got, expected $PY_VERSION" >&2
    exit 1
fi

rustup toolchain install
cargo --version
"$python" --version
"$python" -I port/difftest/oracle/_driver.py --self-test
cargo build -p difftest --locked
target/debug/difftest --all --gen "${DIFFTEST_GEN:-10000}" --python "$python"
