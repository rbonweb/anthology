#!/usr/bin/env bash
# $RUST_TEST: the Rust workspace checks in the pinned Rust image. There is no host Rust (GC-18).
#
#   scripts/rust-test.sh                 # lint, then test: the Definition-of-Done run
#   scripts/rust-test.sh lint            # cargo fmt --check, clippy -D warnings
#   scripts/rust-test.sh test [ARGS]     # cargo test --workspace --locked [ARGS]
#   scripts/rust-test.sh run CMD [ARGS]  # any command in the same container (dev.sh uses it)
#
# The image is the FROM line of ci/images/rust.Dockerfile. As in rtk-test.sh, the container
# installs test-only tools from the image's own Debian release (never shipped) and runs the
# command as a non-root user. Named volumes keep the cargo registry, the toolchain components
# from rust-toolchain.toml (per image digest) and a target directory per checkout, so parallel
# checkouts do not share build artifacts. CARGO_BUILD_JOBS passes through when it is set.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
if [ -z "$image" ]; then
    echo "rust-test: no FROM line in ci/images/rust.Dockerfile" >&2
    exit 1
fi

mode=${1:-all}
case "$mode" in
    all | lint | test) [ "$#" -eq 0 ] || shift ;;
    run)
        shift
        if [ "$#" -eq 0 ]; then
            echo "rust-test: run needs a command" >&2
            exit 2
        fi
        ;;
    *)
        echo "usage: scripts/rust-test.sh [all|lint|test [cargo test args]|run CMD [ARGS]]" >&2
        exit 2
        ;;
esac

digest=${image##*@sha256:}
checkout=$(printf '%s' "$root" | cksum | cut -d' ' -f1)

# Runs as root in the container; $1 is the mode, the rest are its arguments.
# shellcheck disable=SC2016 # expanded by the container shell, not this one
inner='
set -eu
mode=$1
shift
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
    git curl ca-certificates procps busybox nodejs >/dev/null
# The components and targets rust-toolchain.toml names, installed once into the volume.
rustup toolchain install >/dev/null 2>&1 || rustup toolchain install
# The tester owns the source tree when the bind mount maps a non-root owner (Linux CI);
# otherwise it gets uid 1000.
uid=$(stat -c %u /w)
[ "$uid" -ne 0 ] || uid=1000
getent group "$uid" >/dev/null || groupadd -g "$uid" rusttest
getent passwd "$uid" >/dev/null || useradd -m -u "$uid" -g "$uid" -s /bin/sh rusttest
user=$(getent passwd "$uid" | cut -d: -f1)
home=$(getent passwd "$uid" | cut -d: -f6)
find /usr/local/cargo /target ! -uid "$uid" -exec chown -h "$uid:$uid" {} +
lint="cargo fmt --all --check && rustfmt --edition 2024 --check fuzz/fuzz_targets/*.rs \
&& cargo clippy --workspace --all-targets --locked -- -D warnings"
case $mode in
    lint) set -- sh -c "$lint" ;;
    test) set -- cargo test --workspace --locked "$@" ;;
    all) set -- sh -c "$lint && cargo test --workspace --locked" ;;
esac
exec runuser -u "$user" -- env -i \
    HOME="$home" USER="$user" LANG=C.UTF-8 \
    PATH="$PATH" CARGO_HOME="$CARGO_HOME" RUSTUP_HOME="$RUSTUP_HOME" \
    CARGO_TARGET_DIR=/target ${CARGO_BUILD_JOBS:+CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS"} \
    "$@"
'

interactive=
[ "$mode" != run ] || interactive=-i

exec docker run --rm $interactive \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-}" \
    -v "$root":/w \
    -v anthology-cargo:/usr/local/cargo/registry \
    -v "anthology-rustup-${digest:0:12}":/usr/local/rustup \
    -v "anthology-rust-target-$checkout":/target \
    -w /w "$image" sh -c "$inner" rust-test "$mode" "$@"
