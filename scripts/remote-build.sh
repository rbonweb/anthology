#!/usr/bin/env bash
# Run the Docker-based Rust checks (scripts/rust-test.sh) on a Linux build host over
# ssh, so a long build does not load this machine.
#
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh test -p anthology-guard
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh lint
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-all
#
# The host is an ssh alias or name, taken only from the environment: no host detail belongs in the
# repo. Key-based login must already work (ssh -o BatchMode=yes); the script never prompts, never
# elevates, never edits the host's ssh or auth setup and never installs anything there. The host
# needs Docker for the login user, bash 4 or newer and tar (rsync is used when both ends have it).
#
# We leave nothing behind on the host that can fill its disk:
#   1. before a run, it sweeps our own leftovers only: ~/anthology-remote/* folders and
#      anthology-rust-target-* volumes unused for 12 hours (a folder's age is that of its .stamp
#      file, a volume without a folder goes by its creation time), and our stopped containers.
#      A volume that a running container uses is left alone, and so is everything else on the host;
#   2. it refuses to start when the host has less than 20 GB free, on the home disk or the Docker
#      data disk;
#   3. it copies this checkout to ~/anthology-remote/<hash>/: the tracked files plus the
#      uncommitted and new files that git does not ignore, never target/ or .claude/worktrees. With
#      ANTHOLOGY_KEEP_REMOTE=1 a re-run sends only what changed (rsync) and removes what left the
#      checkout;
#   4. it runs scripts/rust-test.sh there with the arguments given, streams its output and exits
#      with its exit code;
#   5. when the run ends, passed or failed, on Ctrl-C and when the ssh link drops, it removes the
#      containers of this checkout (the ones that mount its target volume; rust-test.sh does not
#      name its containers, so the volume is the handle) and, unless ANTHOLOGY_KEEP_REMOTE=1 is
#      set, the checkout's folder and its anthology-rust-target-<n> volume. Keep a run warm only
#      for a session. The next run of the same checkout sweeps its containers first, so one build
#      per checkout at a time.
#   --clean removes this checkout's folder, volume and containers on the host.
#   --clean-all removes everything of ours there: every folder, every anthology-rust-target-*,
#   anthology-cargo and anthology-rustup-* volume, their containers and the Rust image.
#
# ANTHOLOGY_REMOTE_SYNC=tar forces the tar copy even where rsync exists (the tests use it).
set -eu
set -o pipefail

usage() {
    cat >&2 <<'EOF'
usage: ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh <rust-test.sh args...>
       ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean
       ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-all

  <alias> is the ssh alias of the build host. The arguments are those of scripts/rust-test.sh,
  for example `test -p anthology-guard` or `lint`. After the run the host's copy of this checkout
  and its target volume are removed; ANTHOLOGY_KEEP_REMOTE=1 keeps them for the next run.
  --clean removes this checkout's remote folder and target volume. --clean-all removes everything
  of ours on the host, caches and the Rust image included.
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
case ${1:-} in
    --clean | --clean-all)
        [ "$#" -eq 1 ] || usage
        clean=$1
        ;;
esac

# 1 keeps the checkout's folder and target volume on the host after the run.
keep=0
[ "${ANTHOLOGY_KEEP_REMOTE:-}" != 1 ] || keep=1
# The free space the host must have (20 GB, in KB) and the age at which we sweep our own leftovers
# (12 hours, in seconds).
min_free_kb=20971520
max_age_s=43200

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
cd "$HOME"
rm -rf -- "$dir" "$dir.stamp"
'

# Every leftover of ours on the host: all folders, our volumes with their containers, and the image
# (the first argument). Nothing else is touched.
# shellcheck disable=SC2016 # expanded on the host, not here
clean_all_body='
root=${dir%/*}
[ "${root##*/}" = anthology-remote ] || exit 2
image=${1:-}
cd "$HOME"
for v in $(docker volume ls -q 2>/dev/null); do
    case $v in
        anthology-rust-target-* | anthology-cargo | anthology-rustup-*) ;;
        *) continue ;;
    esac
    ids=$(docker ps -aq --filter "volume=$v" 2>/dev/null || true)
    # shellcheck disable=SC2086 # a list of container ids
    [ -z "$ids" ] || docker rm -f $ids >/dev/null 2>&1 || true
    docker volume rm "$v" >/dev/null 2>&1 || true
done
rm -rf -- "$root"
[ -z "$image" ] || docker rmi "$image" >/dev/null 2>&1 || true
'

# Before a run: sweep our own old leftovers, check the free space, make the folder and say whether
# the host has rsync. Arguments: the free space needed in KB, the age to sweep at in seconds.
# shellcheck disable=SC2016 # expanded on the host, not here
preflight_body='
export LC_ALL=C
min_kb=$1
cutoff=$(( $(date +%s) - $2 ))
root=${dir%/*}
known=

# drop_volume V: remove its containers and the volume, unless a running container uses it.
drop_volume() {
    [ -z "$(docker ps -q --filter "volume=$1" 2>/dev/null)" ] || return 1
    ids=$(docker ps -aq --filter "volume=$1" 2>/dev/null || true)
    # shellcheck disable=SC2086 # a list of container ids
    [ -z "$ids" ] || docker rm -f $ids >/dev/null 2>&1 || true
    docker volume rm "$1" >/dev/null 2>&1 || true
}

# Folders of ours not used for 12 hours (the age of the .stamp file, else of the folder).
if [ -d "$root" ]; then
    for d in "$root"/*; do
        [ -d "$d" ] || continue
        v=anthology-rust-target-$(printf "%s" "$d" | cksum | cut -d" " -f1)
        known="$known $v"
        ref=$d.stamp
        [ -e "$ref" ] || ref=$d
        [ "$(stat -c %Y "$ref")" -lt "$cutoff" ] || continue
        drop_volume "$v" || continue
        rm -rf -- "$d" "$d.stamp"
    done
fi

# Our stopped containers, and volumes of ours that no folder owns and that are 12 hours old.
for v in $(docker volume ls -q --filter name=anthology-rust-target- 2>/dev/null); do
    case $v in anthology-rust-target-*) ;; *) continue ;; esac
    ids=$(docker ps -aq --filter status=exited --filter status=created --filter status=dead \
        --filter "volume=$v" 2>/dev/null || true)
    # shellcheck disable=SC2086 # a list of container ids
    [ -z "$ids" ] || docker rm -f $ids >/dev/null 2>&1 || true
    case "$known " in *" $v "*) continue ;; esac
    created=$(docker volume inspect -f "{{.CreatedAt}}" "$v" 2>/dev/null || true)
    created=$(date -d "$created" +%s 2>/dev/null) || continue
    [ "$created" -lt "$cutoff" ] || continue
    drop_volume "$v" || true
done

# The free space, on the home disk and on the Docker data disk.
low=
# check_free LABEL PATH: note it in `low` when the disk of PATH has too little room.
check_free() {
    [ -e "$2" ] || return 0
    free=$(df -Pk "$2" | awk "NR == 2 { print \$4 }")
    if [ -z "$free" ] || [ "$free" -lt "$min_kb" ]; then
        low=1
        echo "remote-build: the build host has less than $((min_kb / 1048576)) GB free on its $1; free space there or run --clean-all, then try again" >&2
    fi
}
check_free "home disk" "$HOME"
docker_root=$(docker info -f "{{.DockerRootDir}}" 2>/dev/null || true)
[ -z "$docker_root" ] || check_free "Docker disk" "$docker_root"
[ -z "$low" ] || exit 3

mkdir -p "$dir"
touch "$dir.stamp"
if command -v rsync >/dev/null 2>&1; then echo remote-build-rsync; fi
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
# ends, and the watcher stops the run. Either way, on the way out the containers are swept and,
# unless keep is 1, the folder and the target volume are removed.
# shellcheck disable=SC2016 # expanded on the host, not here
run_body='
cd "$dir"
pid=
stop() {
    [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
    exit 255
}
leave() {
    sweep
    if [ "$keep" = 1 ]; then
        touch "$dir.stamp" 2>/dev/null || true
        return 0
    fi
    cd "$HOME"
    docker volume rm "$vol" >/dev/null 2>&1 || true
    rm -rf -- "$dir" "$dir.stamp"
}
trap stop INT TERM HUP
trap leave EXIT
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
    cmd="bash -c $(q "keep=$keep$remote_head$body") remote-build \"\$HOME/$rdir\""
    local arg
    for arg in "$@"; do
        cmd="$cmd $(q "$arg")"
    done
    printf '%s' "$cmd"
}

case $clean in
    --clean)
        rssh "$(remote_command "$clean_body")"
        echo "remote-build: cleaned this checkout on the build host" >&2
        exit 0
        ;;
    --clean-all)
        # The image rust-test.sh pulls: the FROM line of the Dockerfile it reads.
        image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
        rssh "$(remote_command "$clean_all_body" "$image")"
        echo "remote-build: removed everything of ours from the build host" >&2
        exit 0
        ;;
esac

tmp=$(mktemp -d)
hold_pid=
ssh_pid=
touched_host=
ran=
# shellcheck disable=SC2329 # run by the EXIT trap
finish() {
    [ -z "$hold_pid" ] || kill "$hold_pid" 2>/dev/null || true
    # A step before the run failed: the host's run script, which cleans up, never started.
    if [ -n "$touched_host" ] && [ -z "$ran" ] && [ "$keep" != 1 ]; then
        rssh "$(remote_command "$clean_body")" >/dev/null 2>&1 || true
    fi
    rm -rf "$tmp"
}
# shellcheck disable=SC2329 # run by the signal trap
on_signal() {
    [ -z "$ssh_pid" ] || kill "$ssh_pid" 2>/dev/null || true
    body=$sweep_body
    [ "$keep" = 1 ] || body=$clean_body
    rssh "$(remote_command "$body")" >/dev/null 2>&1 || true
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

# Sweep the host's old leftovers of ours, refuse a host short of disk (its message and exit code 3
# come through), make the folder and see whether the host has rsync.
touched_host=1
probe=$(rssh "$(remote_command "$preflight_body" "$min_free_kb" "$max_age_s")")
have_rsync=
if [ "${ANTHOLOGY_REMOTE_SYNC:-}" != tar ] && command -v rsync >/dev/null 2>&1; then
    case $probe in *remote-build-rsync*) have_rsync=1 ;; esac
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
ran=1
mkfifo "$tmp/hold"
sleep 21600 > "$tmp/hold" &
hold_pid=$!
rssh "$(remote_command "$run_body" "$@")" < "$tmp/hold" &
ssh_pid=$!
rc=0
wait "$ssh_pid" || rc=$?
ssh_pid=
exit "$rc"
