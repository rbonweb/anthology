#!/usr/bin/env bash
# Run the Docker-based Rust checks (scripts/rust-test.sh) on a Linux build host over
# ssh, so a long build does not load this machine.
#
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh test -p anthology-guard
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh lint
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean
#
# The host is an ssh alias or name, taken only from the environment: no host detail belongs in the
# repo. Key-based login must already work (ssh -o BatchMode=yes); the script never prompts, never
# elevates, never edits the host's ssh or auth setup and never installs anything there. The host
# needs Docker for the login user, bash 4 or newer and tar (rsync is used when both ends have it).
#
# What it does:
#   1. copies this checkout to ~/anthology-remote/<hash>/ on the host: the tracked files plus the
#      uncommitted and new files that git does not ignore, never target/ or .claude/worktrees. A
#      re-run sends only what changed (rsync) and removes what left the checkout;
#   2. runs scripts/rust-test.sh there with the arguments given, streams its output and exits with
#      its exit code;
#   3. on exit, on Ctrl-C and when the ssh link drops, removes the containers of this checkout (the
#      ones that mount its target volume). rust-test.sh does not name its containers, so the volume
#      is the handle. The next run for the same checkout sweeps the same way, so one build per
#      checkout at a time;
#   --clean removes the checkout's folder on the host and its anthology-rust-target-<n> volume.
#
# ANTHOLOGY_REMOTE_SYNC=tar forces the tar copy even where rsync exists (the tests use it).
set -eu
set -o pipefail

usage() {
    cat >&2 <<'EOF'
usage: ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh <rust-test.sh args...>
       ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean

  <alias> is the ssh alias of the build host. The arguments are those of scripts/rust-test.sh,
  for example `test -p anthology-guard` or `lint`. --clean removes this checkout's remote folder
  and target volume.
EOF
    exit 2
}

host=${ANTHOLOGY_BUILD_HOST:-}
[ -n "$host" ] || usage
case $host in
    -*)
        # Would be read as an ssh option. Not echoed: the value is private.
        echo "remote-build: ANTHOLOGY_BUILD_HOST is not a host name" >&2
        exit 2
        ;;
esac

clean=
if [ "${1:-}" = --clean ]; then
    [ "$#" -eq 1 ] || usage
    clean=1
fi

root=$(cd "$(dirname "$0")/.." && pwd)
# One folder per checkout. The volume name comes from the folder's path on the host (rust-test.sh
# hashes its own root), so that is worked out there, not here.
hash=$(printf '%s' "$root" | cksum | cut -d' ' -f1)
rdir=anthology-remote/$hash

ssh_opts=(-o BatchMode=yes -o LogLevel=ERROR -o ServerAliveInterval=15 -o ServerAliveCountMax=4)

rssh() {
    # shellcheck disable=SC2029 # the command string is built to expand on the host, by design
    ssh "${ssh_opts[@]}" "$host" "$@"
}

# Single-quote one string for the host's shell.
q() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

# Runs on the host as `bash -c <script> remote-build <folder> [args]`: the shared head sets the
# folder, its target volume (the name rust-test.sh gives it) and `sweep`, which removes every
# container that mounts that volume.
# shellcheck disable=SC2016 # expanded on the host, not here
remote_head='
set -u
dir=$1
shift
case $dir in */anthology-remote/*) ;; *) echo "remote-build: bad folder" >&2; exit 2 ;; esac
vol=anthology-rust-target-$(printf "%s" "$dir" | cksum | cut -d" " -f1)
sweep() {
    ids=$(docker ps -aq --filter "volume=$vol" 2>/dev/null || true)
    # shellcheck disable=SC2086 # a list of container ids
    [ -z "$ids" ] || docker rm -f $ids >/dev/null 2>&1 || true
}
'

# shellcheck disable=SC2016 # expanded on the host, not here
sweep_body='
sweep
'

# shellcheck disable=SC2016 # expanded on the host, not here
clean_body='
sweep
docker volume rm "$vol" >/dev/null 2>&1 || true
rm -rf -- "$dir"
'

# Reads the NUL-separated list of the files that belong in the folder from stdin and removes the
# rest, then the empty folders. An empty list removes nothing: it means the copy failed.
# shellcheck disable=SC2016 # expanded on the host, not here
prune_body='
export LC_ALL=C
cd "$dir"
tmp=$(mktemp -d)
trap "rm -rf \"$tmp\"" EXIT
sort -z > "$tmp/want"
[ -s "$tmp/want" ] || { echo "remote-build: empty file list" >&2; exit 1; }
find . \( -type f -o -type l \) -printf "%P\\0" | sort -z > "$tmp/have"
comm -z -23 "$tmp/have" "$tmp/want" | xargs -0 -r rm -f --
find . -mindepth 1 -type d -empty -delete
'

# The run. The tests run in the background; the ssh link feeds our stdin. When the link drops, stdin
# ends, and the watcher stops the run. Either way the containers are swept on the way out.
# shellcheck disable=SC2016 # expanded on the host, not here
run_body='
cd "$dir"
pid=
stop() {
    [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
    exit 255
}
trap stop INT TERM HUP
trap sweep EXIT
sweep
scripts/rust-test.sh "$@" &
pid=$!
{ cat; kill -TERM $$; } <&0 >/dev/null 2>&1 &
watcher=$!
rc=0
wait "$pid" || rc=$?
kill "$watcher" 2>/dev/null || true
exit "$rc"
'

# The command string for the host: one of the scripts above, its folder under the host's $HOME
# (expanded there) and the arguments, each quoted.
remote_command() {
    local body=$1 cmd
    shift
    cmd="bash -c $(q "$remote_head$body") remote-build \"\$HOME/$rdir\""
    local arg
    for arg in "$@"; do
        cmd="$cmd $(q "$arg")"
    done
    printf '%s' "$cmd"
}

if [ -n "$clean" ]; then
    rssh "$(remote_command "$clean_body")"
    echo "remote-build: cleaned this checkout on the build host" >&2
    exit 0
fi

tmp=$(mktemp -d)
hold_pid=
ssh_pid=
# shellcheck disable=SC2329 # run by the EXIT trap
finish() {
    [ -z "$hold_pid" ] || kill "$hold_pid" 2>/dev/null || true
    rm -rf "$tmp"
}
# shellcheck disable=SC2329 # run by the signal trap
on_signal() {
    [ -z "$ssh_pid" ] || kill "$ssh_pid" 2>/dev/null || true
    rssh "$(remote_command "$sweep_body")" >/dev/null 2>&1 || true
    exit 130
}
trap finish EXIT
trap on_signal INT TERM HUP

# The files of this checkout that belong on the host: tracked and new-but-not-ignored, minus the
# ones deleted on disk, target/ and the agent worktrees.
list=$tmp/files
git -C "$root" ls-files -z --cached --others --exclude-standard |
    while IFS= read -r -d '' file; do
        case $file in target/* | .claude/worktrees/*) continue ;; esac
        if [ -f "$root/$file" ] || [ -L "$root/$file" ]; then
            printf '%s\0' "$file"
        fi
    done > "$list"
[ -s "$list" ] || { echo "remote-build: no files to send from $root" >&2; exit 1; }

# Make the folder, and see whether the host has rsync.
have_rsync=
if [ "${ANTHOLOGY_REMOTE_SYNC:-}" != tar ] && command -v rsync >/dev/null 2>&1; then
    probe=$(rssh "mkdir -p \"\$HOME/$rdir\" && if command -v rsync >/dev/null 2>&1; then echo remote-build-rsync; fi")
    case $probe in *remote-build-rsync*) have_rsync=1 ;; esac
else
    rssh "mkdir -p \"\$HOME/$rdir\""
fi

if [ -n "$have_rsync" ]; then
    rsync -a --from0 --files-from="$list" -e "ssh ${ssh_opts[*]}" "$root/" "$host:$rdir/"
else
    COPYFILE_DISABLE=1 tar -C "$root" --null -T "$list" -czf - |
        rssh "tar -C \"\$HOME/$rdir\" -xzf -"
fi
rssh "$(remote_command "$prune_body")" < "$list"

# The run. A fifo keeps the host-side stdin open until we stop; if we die or the link drops, the host
# sees the end of its stdin and stops the tests.
mkfifo "$tmp/hold"
sleep 21600 > "$tmp/hold" &
hold_pid=$!
rssh "$(remote_command "$run_body" "$@")" < "$tmp/hold" &
ssh_pid=$!
rc=0
wait "$ssh_pid" || rc=$?
ssh_pid=
exit "$rc"
