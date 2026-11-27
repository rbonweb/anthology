#!/bin/sh
# Keeps the Linux Rust shards' build state between runs (ci.yml, actions/cache). scripts/rust-test.sh
# runs cargo in a container with three named docker volumes: the cargo registry, the toolchain
# components of rust-toolchain.toml and the target folder. A fresh runner has none of them, so every
# shard downloaded the registry and the toolchain and compiled every dependency again. This binds
# the same three volumes to folders on the runner that actions/cache restores and saves.
#
#   ci/jobs/rust-cache.sh dirs DIR      # make DIR/cargo, DIR/rustup and DIR/target
#   ci/jobs/rust-cache.sh volumes DIR   # create the three volumes, bound to those folders; run it
#                                       # after the restore and before scripts/rust-test.sh
#   ci/jobs/rust-cache.sh trim DIR      # cut DIR/target to what third-party crates built
#
# The names are the ones scripts/rust-test.sh computes (crates/repo-tests/tests/rust_test_jobs.rs
# runs both and compares them), so a change there fails that test rather than silently emptying
# the cache. A cache is only ever speed: cargo's fingerprints rebuild whatever does not match, so a
# stale or partial restore never changes a result.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)

usage() {
    echo "usage: ci/jobs/rust-cache.sh dirs|volumes|trim DIR" >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
mode=$1
dir=$2

case $mode in
    dirs)
        mkdir -p "$dir/cargo" "$dir/rustup" "$dir/target"
        ;;
    volumes)
        image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
        if [ -z "$image" ]; then
            echo "rust-cache: no FROM line in ci/images/rust.Dockerfile" >&2
            exit 1
        fi
        digest=${image##*@sha256:}
        checkout=$(printf '%s' "$root" | cksum | cut -d' ' -f1)
        for pair in \
            "anthology-cargo cargo" \
            "anthology-rustup-$(printf '%.12s' "$digest") rustup" \
            "anthology-rust-target-$checkout target"; do
            # shellcheck disable=SC2086 # $pair is a volume name and a folder name
            set -- $pair
            docker volume create --driver local --opt type=none --opt o=bind \
                --opt "device=$dir/$2" "$1" >/dev/null
        done
        ;;
    trim)
        debug=$dir/target/debug
        [ -d "$debug" ] || exit 0
        # Incremental state, examples, and every program (the test binaries and the crates' own
        # binaries have no extension, a library or metadata file has one).
        rm -rf "$debug/incremental" "$debug/examples"
        find "$debug" -maxdepth 2 -type f ! -name '*.*' -exec rm -f {} +
        # The workspace's own crates: their libraries, metadata and build records are rebuilt on
        # every change anyway, and leave only what the registry's crates built.
        meta=$(mktemp)
        trap 'rm -f "$meta"' EXIT
        if ${CARGO:-cargo} metadata --no-deps --format-version 1 --offline >"$meta" 2>/dev/null; then
            names=$(node "$root/ci/jobs/rust-graph.mjs" "$meta" dirs | cut -f2)
            for n in $names; do
                u=$(printf '%s' "$n" | tr '-' '_')
                rm -f "$debug"/deps/lib"$u"-* "$debug"/deps/"$u"-*
                rm -rf "$debug"/.fingerprint/"$n"-* "$debug"/build/"$n"-*
            done
        fi
        ;;
    *) usage ;;
esac
