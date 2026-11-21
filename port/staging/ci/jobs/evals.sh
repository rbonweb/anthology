#!/bin/sh
# CI job `evals` (design §14.2; CO2 moves this to ci/jobs/ and makes evals.yml's run step call
# it): builds the `anthology` binary and `anthology-dev` with the pinned toolchain (static musl
# on Linux, so the binary matches anthology.lock's target; musl-gcc builds the bundled SQLite),
# then runs scripts/run-evals.sh with the arguments given (`--ci --max-cost-usd "$MAX_COST_USD"`
# in CI). The plugins under test are a scratch copy with that binary seeded. Exit status is
# run-evals.sh's; status 3 (no baseline committed) also prints the CI error line.
#
# The build runs without the API credentials the step's environment holds for the evals, so no
# third-party build script sees them. (CO2 may move the build into a step of its own that has no
# secret and exports ANTHOLOGY_BIN and ANTHOLOGY_DEV; this script then skips it.)
set -eu

# This file is written for ci/jobs/; before the cutover it lives in port/staging/ci/jobs/.
root=$(cd "$(dirname "$0")/../.." && pwd)
case $root in
    */port/staging) root=${root%/port/staging} ;;
esac
cd "$root"

if [ -z "${ANTHOLOGY_BIN:-}" ] || [ -z "${ANTHOLOGY_DEV:-}" ]; then
    target_dir=${CARGO_TARGET_DIR:-$root/target}
    triple=
    case $(uname -s) in
        Linux)
            case $(uname -m) in
                x86_64 | amd64) triple=x86_64-unknown-linux-musl ;;
                aarch64 | arm64) triple=aarch64-unknown-linux-musl ;;
                *)
                    echo "evals: no anthology build for $(uname -m)" >&2
                    exit 2
                    ;;
            esac
            if ! command -v musl-gcc >/dev/null 2>&1; then
                sudo apt-get update -qq >&2
                sudo apt-get install -y -qq musl-tools >&2
            fi
            built=$target_dir/$triple/release
            ;;
        *) built=$target_dir/release ;;
    esac
    (
        unset ANTHROPIC_API_KEY ANTHROPIC_AUTH_TOKEN ANTHROPIC_BASE_URL ANTHROPIC_BASE_URL_SECRET
        if [ -n "$triple" ]; then
            cc_var=CC_$(printf '%s' "$triple" | tr '-' '_')
            export "$cc_var=musl-gcc"
            rustup target add "$triple" >&2
            cargo build --release --locked -p anthology-cli -p anthology-dev --target "$triple" >&2
        else
            cargo build --release --locked -p anthology-cli -p anthology-dev >&2
        fi
    ) || {
        echo "evals: the build of anthology and anthology-dev failed" >&2
        exit 2
    }
    ANTHOLOGY_BIN=${ANTHOLOGY_BIN:-$built/anthology}
    ANTHOLOGY_DEV=${ANTHOLOGY_DEV:-$built/anthology-dev}
    export ANTHOLOGY_BIN ANTHOLOGY_DEV
fi

status=0
scripts/run-evals.sh "$@" || status=$?
if [ "$status" = 3 ]; then
    echo "::error::No evals/baseline.json is committed; the results and a candidate baseline are in the evals-results artifact"
fi
exit "$status"
