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
#   scripts/rust-test.sh session-start NAME   # one container, set up once, that ends by itself
#                                        #   after 4 hours (NAME must start with anthology-release-);
#                                        #   while $ANTHOLOGY_RUST_SESSION names it, every mode
#                                        #   above runs in it with `docker exec` instead of a fresh
#                                        #   container and its setup. Runs `gc` first.
#   scripts/rust-test.sh session-stop NAME    # removes that container and this checkout's target volume
#   scripts/rust-test.sh gc              # removes leaked target volumes (see below)
#
# The image is the FROM line of ci/images/rust.Dockerfile. As in atl-test.sh, the container
# installs test-only tools from the image's own Debian release (never shipped) and runs the
# command as a non-root user. Named volumes keep the cargo registry, the toolchain components
# from rust-toolchain.toml (per image digest) and a target directory per checkout, so parallel
# checkouts do not share build artifacts. CARGO_BUILD_JOBS passes through when it is set.
#
# The target volume of a checkout (anthology-rust-target-N) is kept for as long as the checkout
# exists, so that repeated runs from it stay incremental: downloading and rebuilding it costs
# minutes each time. A one-shot run (no $ANTHOLOGY_RUST_SESSION) keeps it unless
# ANTHOLOGY_KEEP_TARGET=0 asks for it to go when the run ends (remote-build.sh does, for its
# per-run folders). session-stop removes it, unless ANTHOLOGY_KEEP_TARGET=1. A volume still
# mounted by another container stays: Docker refuses to remove it.
# `gc` removes the leftovers: dangling volumes (no container) named anthology-rust-target-* whose
# checkout is no longer a git worktree of this repository, and, when git cannot list the
# worktrees (so "gone" is unknown), those and anthology-atl-target created more than
# $ANTHOLOGY_GC_HOURS hours ago (default 48); anthology-atl-target always goes by that age. A
# target volume of a live worktree is never removed by age. It never touches anthology-cargo,
# anthology-rustup-* or any other volume: those are shared caches that later runs reuse.
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
            anthology-release-*[!A-Za-z0-9_.-]* | anthology-release-) echo "rust-test: bad session name $2" >&2; exit 2 ;;
            anthology-release-*) ;;
            *) echo "rust-test: a session name starts with anthology-release-" >&2; exit 2 ;;
        esac
        ;;
    gc)
        if [ "$#" -ne 1 ]; then
            echo "rust-test: gc takes no arguments" >&2
            exit 2
        fi
        ;;
    *)
        echo "usage: scripts/rust-test.sh [all|lint|test [cargo test args]|run CMD [ARGS]|session-start NAME|session-stop NAME|gc]" >&2
        exit 2
        ;;
esac

digest=${image##*@sha256:}
checkout=$(printf '%s' "$root" | cksum | cut -d' ' -f1)
target_volume="anthology-rust-target-$checkout"

# Removes this checkout's target volume unless the user keeps it (ANTHOLOGY_KEEP_TARGET set and not
# 0): for the end of a session and a failed session setup. Docker refuses while a container still
# mounts it (another session of this checkout), and there is nothing to do when it is gone:
# neither is an error.
drop_target() {
    case ${ANTHOLOGY_KEEP_TARGET:-} in
        "" | 0) docker volume rm "$target_volume" >/dev/null 2>&1 || true ;;
    esac
}
# The end of a one-shot run: the volume stays, so the next run is incremental, unless the caller
# asks for it to go with ANTHOLOGY_KEEP_TARGET=0.
# shellcheck disable=SC2329 # run by the EXIT trap
drop_target_if_asked() {
    [ "${ANTHOLOGY_KEEP_TARGET:-}" != 0 ] || docker volume rm "$target_volume" >/dev/null 2>&1 || true
}

# The creation time of volume $1 in epoch seconds, read from Docker's RFC 3339 `CreatedAt` (a zone
# offset or Z, with or without a fraction of a second); nothing when it is missing or unreadable.
created_epoch() {
    docker volume inspect --format '{{.CreatedAt}}' "$1" 2>/dev/null | awk '
        {
            if (!match($0, /^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9]/)) exit
            y = substr($0, 1, 4) + 0; m = substr($0, 6, 2) + 0; d = substr($0, 9, 2) + 0
            h = substr($0, 12, 2) + 0; mi = substr($0, 15, 2) + 0; s = substr($0, 18, 2) + 0
            rest = substr($0, 20)
            off = 0
            if (match(rest, /[+-][0-9][0-9]:[0-9][0-9]$/)) {
                off = (substr(rest, RSTART + 1, 2) * 60 + substr(rest, RSTART + 4, 2)) * 60
                if (substr(rest, RSTART, 1) == "-") off = -off
            } else if (rest !~ /Z$/) exit
            # Days since 1970-01-01 of a civil date (Howard Hinnant: days_from_civil).
            if (m <= 2) y -= 1
            era = int(y / 400); yoe = y - era * 400
            doy = int((153 * (m + (m > 2 ? -3 : 9)) + 2) / 5) + d - 1
            doe = yoe * 365 + int(yoe / 4) - int(yoe / 100) + doy
            print (era * 146097 + doe - 719468) * 86400 + h * 3600 + mi * 60 + s - off
            exit
        }'
}

# gc: removes the dangling anthology-rust-target-* volumes whose checkout is gone, and those and
# anthology-atl-target older than $ANTHOLOGY_GC_HOURS (see the header). Only those names are ever
# removed. Messages go to stderr; a Docker that will not list or remove is not an error.
gc_volumes() {
    local hours=${ANTHOLOGY_GC_HOURS:-48} listing line path live='' have_live=0
    local names name id stale created cutoff
    case $hours in
        *[!0-9]*)
            echo "rust-test: ANTHOLOGY_GC_HOURS must be a whole number of hours, not $hours" >&2
            return 2
            ;;
    esac
    # The checkouts that exist: git's worktree list, each folder still there, plus this one. Without
    # that list (no git, not a repository) only the age rule applies.
    if listing=$(git -C "$root" worktree list --porcelain 2>/dev/null); then
        have_live=1
        live=" $checkout"
        while IFS= read -r line; do
            case $line in
                "worktree "*)
                    path=${line#worktree }
                    [ ! -d "$path" ] || live="$live $(printf '%s' "$path" | cksum | cut -d' ' -f1)"
                    ;;
            esac
        done <<<"$listing"
    fi
    cutoff=$(($(date +%s) - hours * 3600))
    names=$(docker volume ls --quiet --filter dangling=true 2>/dev/null) || return 0
    while IFS= read -r name; do
        case $name in
            anthology-rust-target-* | anthology-atl-target) ;;
            *) continue ;;
        esac
        case $name in
            *[!A-Za-z0-9_.-]*) continue ;;
        esac
        stale=0
        if [ "$have_live" = 1 ] && [ "$name" != anthology-atl-target ]; then
            id=${name#anthology-rust-target-}
            case $id in
                "" | *[!0-9]*) ;;
                *) case "$live " in *" $id "*) ;; *) stale=1 ;; esac ;;
            esac
        fi
        # A target volume of a live worktree is kept whatever its age; age only judges the one
        # volume that has no worktree, and every volume when the worktrees cannot be listed.
        if [ "$stale" = 0 ] && { [ "$have_live" = 0 ] || [ "$name" = anthology-atl-target ]; }; then
            created=$(created_epoch "$name")
            [ -z "$created" ] || [ "$created" -gt "$cutoff" ] || stale=1
        fi
        if [ "$stale" = 1 ] && docker volume rm "$name" >/dev/null 2>&1; then
            echo "rust-test: gc removed volume $name" >&2
        fi
    done <<<"$names"
}

if [ "$mode" = gc ]; then
    gc_volumes || exit $?
    exit 0
fi

if [ "$mode" = session-stop ]; then
    stopped=0
    docker rm -f "$2" >/dev/null || stopped=$?
    drop_target
    exit "$stopped"
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

# sccache caches rustc's output for the dependencies in one volume shared by every checkout, so a
# fresh checkout (an empty target volume) does not compile them again. The workspace crates stay
# incremental in the target volume: sccache cannot cache an incremental compile, so it leaves them
# alone. Off with ANTHOLOGY_SCCACHE=0, and in CI (a runner starts empty) unless ANTHOLOGY_SCCACHE=1.
# `gc` never removes the volume; the cache evicts its oldest entries beyond SCCACHE_CACHE_SIZE.
sccache=1
case ${ANTHOLOGY_SCCACHE:-${CI:+0}} in
    0) sccache= ;;
esac

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
if [ -n "$sccache" ]; then
    # Pinned to the upstream release; Debian may move its revision (-N) within the image's release.
    # shellcheck disable=SC2016 # expanded by the container shell, not this one
    setup="$setup"'apt-get install -y -qq --no-install-recommends "sccache=0.10.0-*" >/dev/null
find /sccache ! -uid "$uid" -exec chown -h "$uid:$uid" {} +
'
fi
# shellcheck disable=SC2016 # expanded by the container shell, not this one
launch='
set -eu
mode=$1
shift
# A session container set up without sccache has none to run: go without.
[ -z "${SCCACHE_ON:-}" ] || command -v sccache >/dev/null || SCCACHE_ON=
uid=$(stat -c %u /w)
[ "$uid" -ne 0 ] || uid=1000
user=$(getent passwd "$uid" | cut -d: -f1)
home=$(getent passwd "$uid" | cut -d: -f6)
lint="cargo fmt --all --check && rustfmt --edition 2024 --check fuzz/fuzz_targets/*.rs \
&& cargo clippy --workspace --all-targets --locked -- -D warnings \
&& cargo clippy --workspace --all-targets --locked --all-features -- -D warnings"
# Plain first, so the shipped behavior (the real watchdog) is tested; then the test features.
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
    ${SCCACHE_ON:+RUSTC_WRAPPER=sccache SCCACHE_DIR=/sccache SCCACHE_CACHE_SIZE=5G} \
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
    -v "$target_volume":/target
    ${sccache:+-v anthology-sccache:/sccache}
)

if [ "$mode" = session-start ]; then
    gc_volumes || true
    # A container that only waits, and only for 4 hours, so one that its owner never removed (a
    # killed release, a crashed host) ends by itself; --rm then deletes it. The setup runs in it
    # once, here, and a failed setup removes it.
    docker run -d --rm --name "$2" "${volumes[@]}" -w /w "$image" sleep 14400 >/dev/null
    if ! docker exec "$2" sh -c "$setup" >&2; then
        docker rm -f "$2" >/dev/null 2>&1 || true
        drop_target
        echo "rust-test: the setup of session $2 failed" >&2
        exit 1
    fi
    exit 0
fi

if [ -n "${ANTHOLOGY_RUST_SESSION:-}" ]; then
    exec docker exec $interactive \
        -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-}" \
        -e RUST_TEST_SCOPE="$scope" \
        -e SCCACHE_ON="$sccache" \
        "$ANTHOLOGY_RUST_SESSION" sh -c "$launch" rust-test "$mode" "$@"
fi

# A one-shot run: its target volume stays when it ends, whatever way it ends, unless the caller
# set ANTHOLOGY_KEEP_TARGET=0. Not `exec`: this shell has to be there to remove the volume then.
trap drop_target_if_asked EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
ran=0
docker run --rm $interactive \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-}" \
    -e RUST_TEST_SCOPE="$scope" \
    -e SCCACHE_ON="$sccache" \
    "${volumes[@]}" \
    -w /w "$image" sh -c "$inner" rust-test "$mode" "$@" || ran=$?
exit "$ran"
