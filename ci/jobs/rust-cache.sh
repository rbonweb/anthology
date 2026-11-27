#!/bin/sh
# Keeps the Linux Rust shards' build state between runs (ci.yml, actions/cache). scripts/rust-test.sh
# runs cargo in a container with named docker volumes: the cargo registry, the toolchain components
# of rust-toolchain.toml and, with ANTHOLOGY_SCCACHE=1, an sccache of what the dependencies
# compiled (its target volume lasts for one run). A fresh runner has none of them, so every shard
# downloaded the registry and the toolchain and compiled every dependency again. This binds the
# three volumes to folders on the runner that actions/cache restores and saves.
#
#   ci/jobs/rust-cache.sh dirs DIR      # make DIR/cargo, DIR/rustup and DIR/sccache
#   ci/jobs/rust-cache.sh volumes DIR   # create the three volumes, bound to those folders; run it
#                                       # after the restore and before scripts/rust-test.sh
#
# The names are the ones scripts/rust-test.sh uses (crates/repo-tests/tests/rust_test_jobs.rs runs
# both and compares them), so a change there fails that test rather than silently emptying the
# cache. A cache is only ever speed: cargo and sccache rebuild whatever does not match, so a stale
# or partial restore never changes a result.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)

usage() {
    echo "usage: ci/jobs/rust-cache.sh dirs|volumes DIR" >&2
    exit 2
}

[ "$#" -eq 2 ] || usage
mode=$1
dir=$2

case $mode in
    dirs)
        mkdir -p "$dir/cargo" "$dir/rustup" "$dir/sccache"
        ;;
    volumes)
        image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
        if [ -z "$image" ]; then
            echo "rust-cache: no FROM line in ci/images/rust.Dockerfile" >&2
            exit 1
        fi
        digest=${image##*@sha256:}
        for pair in \
            "anthology-cargo cargo" \
            "anthology-rustup-$(printf '%.12s' "$digest") rustup" \
            "anthology-sccache sccache"; do
            # shellcheck disable=SC2086 # $pair is a volume name and a folder name
            set -- $pair
            docker volume create --driver local --opt type=none --opt o=bind \
                --opt "device=$dir/$2" "$1" >/dev/null
        done
        ;;
    *) usage ;;
esac
