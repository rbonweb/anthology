#!/usr/bin/env bash
# $RUST_TEST: the Rust workspace checks in the pinned Rust image. There is no host Rust (GC-18).
#
#   scripts/rust-test.sh                 # lint, then test: the Definition-of-Done run
#   scripts/rust-test.sh lint            # cargo fmt --check, clippy -D warnings (plain and
#                                        #   --all-features)
#   scripts/rust-test.sh test [ARGS]     # cargo test --workspace --locked [ARGS], then the same
#                                        #   with --all-features (the test-only features, m-R10).
#                                        #   When ARGS name a package (-p X, -pX, --package X,
#                                        #   --package=X), --workspace is dropped and cargo
#                                        #   tests only that package, still in both runs.
#   scripts/rust-test.sh run CMD [ARGS]  # any command in the same container (dev.sh uses it)
#   scripts/rust-test.sh session-start NAME   # one long-lived container, set up once (NAME must
#                                        #   start with anthology-); while $ANTHOLOGY_RUST_SESSION
#                                        #   names it, every mode above runs in it with `docker
#                                        #   exec` instead of a fresh container and its setup
#   scripts/rust-test.sh session-stop NAME    # removes that container
#
# The image is the FROM line of ci/images/rust.Dockerfile. As in atl-test.sh, the container
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
    session-start | session-stop)
        if [ "$#" -ne 2 ]; then
            echo "rust-test: $mode needs a container name" >&2
            exit 2
        fi
        # Only our own containers: the name is what `docker rm -f` is given.
        case $2 in
            anthology-*[!A-Za-z0-9_.-]* | anthology-) echo "rust-test: bad session name $2" >&2; exit 2 ;;
            anthology-*) ;;
            *) echo "rust-test: a session name starts with anthology-" >&2; exit 2 ;;
        esac
        ;;
    *)
        echo "usage: scripts/rust-test.sh [all|lint|test [cargo test args]|run CMD [ARGS]|session-start NAME|session-stop NAME]" >&2
        exit 2
        ;;
esac

if [ "$mode" = session-stop ]; then
    exec docker rm -f "$2" >/dev/null
fi

# `test` covers the whole workspace unless the arguments name a package (-p X, -pX,
# --package X or --package=X); then cargo's own package selection applies. Arguments after
# a bare `--` belong to the test binary, so they do not count.
scope=--workspace
if [ "$mode" = test ]; then
    for arg in "$@"; do
        case $arg in
            --) break ;;
            -p | -p?* | --package | --package=*) scope= ;;
        esac
    done
fi

digest=${image##*@sha256:}
checkout=$(printf '%s' "$root" | cksum | cut -d' ' -f1)

# Runs as root in the container, in two parts: `setup` (the packages, the toolchain, the tester
# user: slow, and needed once per container) and `launch` ($1 is the mode, the rest are its
# arguments). A fresh container runs both; a session container (session-start) runs `setup` once
# and `launch` for every command after that.
# shellcheck disable=SC2016 # expanded by the container shell, not this one
setup='
set -eu
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
find /usr/local/cargo /target ! -uid "$uid" -exec chown -h "$uid:$uid" {} +
'
# shellcheck disable=SC2016 # expanded by the container shell, not this one
launch='
set -eu
mode=$1
shift
uid=$(stat -c %u /w)
[ "$uid" -ne 0 ] || uid=1000
user=$(getent passwd "$uid" | cut -d: -f1)
home=$(getent passwd "$uid" | cut -d: -f6)
lint="cargo fmt --all --check && rustfmt --edition 2024 --check fuzz/fuzz_targets/*.rs \
&& cargo clippy --workspace --all-targets --locked -- -D warnings \
&& cargo clippy --workspace --all-targets --locked --all-features -- -D warnings"
# Plain first, so the shipped behaviour (the real watchdog) is tested; then the test features.
tests="cargo test $RUST_TEST_SCOPE --locked \"\$@\" && cargo test $RUST_TEST_SCOPE --locked --all-features \"\$@\""
case $mode in
    lint) set -- sh -c "$lint" ;;
    test) set -- sh -c "$tests" rust-test "$@" ;;
    all) set -- sh -c "$lint && $tests" rust-test ;;
esac
exec runuser -u "$user" -- env -i \
    HOME="$home" USER="$user" LANG=C.UTF-8 \
    PATH="$PATH" CARGO_HOME="$CARGO_HOME" RUSTUP_HOME="$RUSTUP_HOME" \
    CARGO_TARGET_DIR=/target ${CARGO_BUILD_JOBS:+CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS"} \
    "$@"
'
# The fresh-container script: the setup, then the launch with the mode and arguments.
inner="$setup$launch"

interactive=
[ "$mode" != run ] || interactive=-i

# A linked worktree's .git file points into the main checkout's git directory, outside /w.
# Mount that directory read-only at its own path, so git works in the container there too
# (`scripts/dev.sh lock check-fresh` reads the history; RL1).
git_common=$(git -C "$root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)
case $git_common in
    "" | "$root" | "$root"/*) git_common= ;;
esac

volumes=(
    -v "$root":/w
    ${git_common:+-v "$git_common:$git_common:ro"}
    -v anthology-cargo:/usr/local/cargo/registry
    -v "anthology-rustup-${digest:0:12}":/usr/local/rustup
    -v "anthology-rust-target-$checkout":/target
)

if [ "$mode" = session-start ]; then
    # A container that only waits; the setup runs in it once, here, and a failed setup removes it.
    docker run -d --rm --name "$2" "${volumes[@]}" -w /w "$image" tail -f /dev/null >/dev/null
    if ! docker exec "$2" sh -c "$setup" >&2; then
        docker rm -f "$2" >/dev/null 2>&1 || true
        echo "rust-test: the setup of session $2 failed" >&2
        exit 1
    fi
    exit 0
fi

if [ -n "${ANTHOLOGY_RUST_SESSION:-}" ]; then
    exec docker exec $interactive \
        -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-}" \
        -e RUST_TEST_SCOPE="$scope" \
        "$ANTHOLOGY_RUST_SESSION" sh -c "$launch" rust-test "$mode" "$@"
fi

exec docker run --rm $interactive \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-}" \
    -e RUST_TEST_SCOPE="$scope" \
    "${volumes[@]}" \
    -w /w "$image" sh -c "$inner" rust-test "$mode" "$@"
