#!/bin/sh
# CI job `rust-test-macos` (design §14.1). cargo test --workspace --locked on the macOS runner,
# with the toolchain rust-toolchain.toml names: plain, then with the test-only features (m-R10).
#
# Then the ledger's macOS ground truth (F4a, controller ruling; this F1 file is touched for it):
# what cargo compiles and would skip here, `cargo test -- --list [--ignored]`, plain and with
# --all-features, and `anthology-dev ledger check --listings … --platform macos`, so every ported
# Rust target runs on macOS as well as Linux (ci/jobs/ledger.sh checks Linux).
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
rustup toolchain install
cargo --version
cargo test --workspace --locked
cargo test --workspace --locked --all-features

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
cargo run --locked --quiet -p anthology-dev -- \
    ledger check --report --listings "$listings" --platform macos

# BS1 shims on macOS (controller, BS1 hand-back): the runner's real shells and downloaders, and
# the network cases against the testkit TLS server, with github.com mapped to 127.0.0.1 (the
# shims-distros containers do the same). Last, so nothing above resolves github.com to it.
if [ "${CI:-}" = true ]; then
    grep -qE '^127\.0\.0\.1[[:space:]]+github\.com$' /etc/hosts ||
        printf '127.0.0.1 github.com\n' | sudo tee -a /etc/hosts >/dev/null
    ANTHOLOGY_SHIMS_NET=1 ANTHOLOGY_SHIMS_REQUIRE=dash,bash,macsh,nonroot,curl,node \
        cargo test --locked -p repo-tests --test shims
fi
