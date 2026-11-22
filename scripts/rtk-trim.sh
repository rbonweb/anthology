#!/usr/bin/env bash
# Trim rtk/ to the Apache-2.0 edition, Claude Code only, without telemetry.
#
#   scripts/rtk-trim.sh
#
# 1. `scripts/dev.sh rtk-trim` (anthology-dev, in the pinned Rust image; no Python) applies
#    the ordered, idempotent edits (rm/sub/cut/drop). An anchor that no longer matches stops
#    it with exit 1, naming the file and the anchor, so an upstream refactor of a trimmed
#    region is noticed.
#    `scripts/dev.sh` builds anthology-dev in the pinned image, so each run (the trim and the
#    lock guard) needs the network on a cold cache (apt and the cargo registry).
# 2. Cargo.lock is pruned by `cargo metadata --offline` in the pinned image (the FROM line of
#    ci/images/rust.Dockerfile), online only when the registry volume lacks a crate. The whole
#    repository is mounted, because rtk/Cargo.toml may name path dependencies under
#    ../crates/. With an existing lockfile cargo only drops unused packages; a guard fails
#    (and restores the old lockfile) if any package is added or any locked version changes.
#    The one exception is a
#    path dependency of rtk/Cargo.toml on a workspace crate under ../crates/ and its dependency
#    closure, which the guard accepts (`scripts/dev.sh rtk-trim --lock-guard`).
#
# Running it again prints "no changes". sync-rtk.sh runs it after every upstream merge.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
if [ -z "$image" ]; then
    echo "rtk-trim: no FROM line in ci/images/rust.Dockerfile" >&2
    exit 1
fi

# The container mounts the repository at /w and runs there, so it gets repo-relative paths.
ops=$("$root/scripts/dev.sh" rtk-trim --root rtk)
if [ "$ops" != "no changes" ]; then
    printf '%s\n' "$ops"
fi

lock="$root/rtk/Cargo.lock"
# The copy of the lockfile stays inside the repository (in the git-ignored target/), so the
# container can read it and it is never committed.
before_rel="target/rtk-trim/Cargo.lock.before.$$"
before="$root/$before_rel"
mkdir -p "$root/target/rtk-trim"
trap 'rm -f "$before"' EXIT
cp "$lock" "$before"

# The container runs as root so it can fill a cold registry volume. On Linux the bind
# mount keeps that owner, so the lockfile is handed back to the caller afterwards.
owner=
if [ "$(uname -s)" = Linux ]; then
    owner="$(id -u):$(id -g)"
fi
metadata() {
    docker run --rm \
        -v "$root":/w \
        -v anthology-cargo:/usr/local/cargo/registry \
        -e OWNER="$owner" \
        -w /w/rtk "$image" sh -c '
            status=0
            cargo metadata "$@" --format-version 1 >/dev/null || status=$?
            [ -z "$OWNER" ] || chown "$OWNER" Cargo.lock
            exit "$status"
        ' rtk-trim "$@"
}
if ! metadata --offline; then
    echo "rtk-trim: the registry volume lacks a crate; resolving online (locked versions stay)"
    metadata
fi

if ! "$root/scripts/dev.sh" rtk-trim --lock-guard "$before_rel" rtk/Cargo.lock --root rtk; then
    cp "$before" "$lock"
    echo "rtk-trim: rtk/Cargo.lock restored to its state before the lockfile step" >&2
    exit 1
fi

if [ "$ops" = "no changes" ] && cmp -s "$before" "$lock"; then
    echo "rtk-trim: no changes"
else
    echo "rtk-trim: done (review the changes under rtk/)"
fi
