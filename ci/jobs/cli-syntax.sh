#!/bin/sh
# CI job `cli-syntax` (design §14.2; CO2 moves this to ci/jobs/ and points the job at it): the
# Claude Code CLI surface the plugins and the evals rely on (U4), checked by
# `anthology-dev smoke cli-syntax` with the pinned Claude Code (npm ci --prefix ci/claude-code
# runs first). No model is called and no credentials are needed. anthology-dev is built here
# with the pinned toolchain unless $ANTHOLOGY_DEV names one.
set -eu

# This file is written for ci/jobs/; before the cutover it lives in port/staging/ci/jobs/.
root=$(cd "$(dirname "$0")/../.." && pwd)
case $root in
    */port/staging) root=${root%/port/staging} ;;
esac
cd "$root"

dev=${ANTHOLOGY_DEV:-}
if [ -z "$dev" ]; then
    cargo build --release --locked -p anthology-dev >&2
    dev=${CARGO_TARGET_DIR:-$root/target}/release/anthology-dev
fi
exec "$dev" smoke --claude ci/claude-code/node_modules/.bin/claude cli-syntax
