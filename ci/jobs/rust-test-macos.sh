#!/bin/sh
# CI job `rust-test-macos` (design §14.1). cargo test --workspace --locked on the macOS runner,
# with the toolchain rust-toolchain.toml names, as two parallel shards that ci.yml gates on:
#
#   ci/jobs/rust-test-macos.sh plain   # the plain run, then the BS1 shims on macOS
#   ci/jobs/rust-test-macos.sh all     # the run with the test-only features (m-R10), then the
#                                      # ledger's macOS ground truth
#
# With PACKAGES='a b' (a pull request that touched platform-sensitive paths; ci/jobs/rust-scope.sh
# names the crates its change reaches) only those crates are tested, and the ledger's listings
# are left to the full run on `build`, which has no PACKAGES.
#
# The ledger's macOS ground truth (F4a, controller ruling; this F1 file is touched for it):
# what cargo compiles and would skip here, `cargo test -- --list [--ignored]`, plain and with
# --all-features, and `anthology-dev ledger check --listings … --platform macos`, so every ported
# Rust target runs on macOS as well as Linux (ci/jobs/ledger.sh checks Linux).
set -eu

case ${1:-} in
    plain) features= ;;
    all) features=--all-features ;;
    *)
        echo "usage: ci/jobs/rust-test-macos.sh plain|all" >&2
        exit 2
        ;;
esac

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
# The crates whose tests run the built `anthology` binary (anthology_testkit::run::run_bin).
# crates/repo-tests/tests/rust_test_jobs.rs checks this list against the source.
needs_binary="anthology-dev anthology-memory anthology-runtime"
rustup toolchain install
cargo --version
scope=--workspace
if [ -n "${PACKAGES:-}" ]; then
    scope=
    build_first=false
    for p in $PACKAGES; do
        scope="$scope -p $p"
    done
    # A crate that runs the built binary needs it, and a run of the whole workspace builds it
    # only because cli is in it: build it first unless cli's own tests are in the run.
    case " $PACKAGES " in
        *" anthology-cli "*) ;;
        *)
            for p in $needs_binary; do
                case " $PACKAGES " in *" $p "*) build_first=true ;; esac
            done
            ;;
    esac
    if [ "$build_first" = true ]; then
        # shellcheck disable=SC2086 # $features is empty or one option
        cargo build -p anthology-cli --locked $features
    fi
fi
# shellcheck disable=SC2086 # $scope is --workspace or -p options; $features is empty or one option
cargo test $scope --locked $features

if [ "$1" = all ] && [ "$scope" = --workspace ]; then
    listings=target/ledger-macos
    rm -rf "$listings"
    mkdir -p "$listings"
    for mode in plain all ignored-plain ignored-all; do
        out="$listings/rust-tests-$mode.txt"
        features=
        ignored=
        case $mode in *all) features=--all-features ;; esac
        case $mode in ignored-*) ignored=--ignored ;; esac
        # shellcheck disable=SC2086 # $features and $ignored are empty or one option each
        if ! cargo test --workspace --locked $features -- --list $ignored >"$out" 2>&1; then
            cat "$out" >&2
            echo "rust-test-macos: cargo test --list ($mode) failed" >&2
            exit 1
        fi
    done
    exec cargo run --locked --quiet -p anthology-dev -- \
        ledger check --report --listings "$listings" --platform macos
fi

# BS1 shims on macOS (controller, BS1 hand-back): the runner's real shells and downloaders, and
# the network cases against the testkit TLS server, with github.com mapped to 127.0.0.1 (the
# shims-distros containers do the same). Last, so nothing above resolves github.com to it.
if [ "${CI:-}" = true ]; then
    grep -qE '^127\.0\.0\.1[[:space:]]+github\.com$' /etc/hosts ||
        printf '127.0.0.1 github.com\n' | sudo tee -a /etc/hosts >/dev/null
    # A non-root bind to port 443 works on macOS only on the wildcard address.
    ANTHOLOGY_SHIMS_BIND=0.0.0.0:443 ANTHOLOGY_SHIMS_NET=1 ANTHOLOGY_SHIMS_REQUIRE=dash,bash,macsh,nonroot,curl,node \
        cargo test --locked -p repo-tests --test shims
fi
