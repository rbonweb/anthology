#!/usr/bin/env bash
# Run the Docker-based Rust checks (scripts/rust-test.sh) on a Linux build host over
# ssh, so a long build does not load this machine.
#
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh test -p anthology-guard
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh lint
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-ours
#
# The host is an ssh alias or name, taken only from the environment: no host detail belongs in the
# repo. Key-based login must already work (ssh -o BatchMode=yes); the script never prompts, never
# elevates, never edits the host's ssh or auth setup and never installs anything there. The host
# needs Docker for the login user, bash 4 or newer and tar (rsync is used when both ends have it).
#
# The host is shared: we remove only what we created, and leave nothing of it behind that can fill
# the disk. What we create, and how we tell it from the rest:
#   - the folders under ~/anthology-remote (a path nothing else uses);
#   - the volumes anthology-rust-target-<n>, anthology-cargo, anthology-sccache and
#     anthology-rustup-<digest>. The
#     script creates them first with the label anthology.remote-build=1 (rust-test.sh would create
#     them bare), and treats a volume as ours only if its name matches AND it has the label: a
#     volume with the same name and no label was there before us and is never touched;
#   - the containers that mount one of those volumes (rust-test.sh does not name or label its
#     containers, so the volume is the handle);
#   - the Rust image, only if it was absent when we started a run: then we record it in
#     ~/anthology-remote/.pulled-images. We remove it only from that record, only when no container
#     that is not ours uses it, and never with a forced removal.
# No unfiltered cleanup command is ever used, and nothing outside ~/anthology-remote and the items
# above is touched.
#
#   1. before a run, it sweeps our own leftovers: folders and anthology-rust-target-* volumes
#      unused for 12 hours (a folder's age is that of its .stamp file, a volume without a folder
#      goes by its creation time), our stopped containers, and, when nothing of ours has run for 48
#      hours (the time of ~/anthology-remote/.last-used, written at the start and the end of each
#      run), the anthology-cargo, anthology-sccache and anthology-rustup-* caches and the pulled
#      image. A volume that
#      a running container uses is left alone;
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
#   --clean-ours removes everything of ours there, as defined above: every folder, our volumes
#   with their containers and the image we pulled.
#
# ANTHOLOGY_REMOTE_SYNC=tar forces the tar copy even where rsync exists (the tests use it).
set -eu
set -o pipefail

usage() {
    cat >&2 <<'EOF'
usage: ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh <rust-test.sh args...>
       ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean
       ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-ours

  <alias> is the ssh alias of the build host. The arguments are those of scripts/rust-test.sh,
  for example `test -p anthology-guard` or `lint`. After the run the host's copy of this checkout
  and its target volume are removed; ANTHOLOGY_KEEP_REMOTE=1 keeps them for the next run.
  --clean removes this checkout's remote folder and target volume. --clean-ours removes
  everything of ours on the host, caches and the Rust image we pulled included, and nothing else.
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
    --clean | --clean-ours)
        [ "$#" -eq 1 ] || usage
        clean=$1
        ;;
    --clean-all)
        echo "remote-build: --clean-all is now --clean-ours" >&2
        exit 2
        ;;
esac

# 1 keeps the checkout's folder and target volume on the host after the run.
keep=0
[ "${ANTHOLOGY_KEEP_REMOTE:-}" != 1 ] || keep=1
# The free space the host must have (20 GB, in KB) and the age at which we sweep our own leftovers
# (12 hours, in seconds).
min_free_kb=20971520
max_age_s=43200
# How long nothing of ours may have run before we remove the shared caches and the image (48 hours).
cache_age_s=172800

root=$(cd "$(dirname "$0")/.." && pwd)
# One folder per checkout. The volume name comes from the folder's path on the host (rust-test.sh
# hashes its own root), so that is worked out there, not here.
hash=$(printf '%s' "$root" | cksum | cut -d' ' -f1)
rdir=anthology-remote/$hash
# The image rust-test.sh runs: the FROM line of the Dockerfile it reads.
image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile" 2>/dev/null || true)

ssh_opts=(-o BatchMode=yes -o LogLevel=ERROR -o ServerAliveInterval=15 -o ServerAliveCountMax=4)

rssh() {
    # shellcheck disable=SC2029 # the command string is built to expand on the host, by design
    ssh "${ssh_opts[@]}" "$host" "$@"
}

# Single-quote one string for the host's shell.
q() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

# Runs on the host as `bash -c <script> remote-build <folder> [args]`. The shared head holds the
# one definition of "ours": a volume is ours only if its name matches AND it carries our label; a
# container is ours only if it mounts one of our volumes; the image only if we recorded pulling it.
# Everything that removes something goes through these functions.
# shellcheck disable=SC2016 # expanded on the host, not here
remote_head='
set -u
dir=$1
shift
case $dir in */anthology-remote/*) ;; *) echo "remote-build: bad folder" >&2; exit 2 ;; esac
root=${dir%/*}
label=anthology.remote-build=1
pulled=$root/.pulled-images
vol=anthology-rust-target-$(printf "%s" "$dir" | cksum | cut -d" " -f1)

# is_ours V: our name and our label. Never true for a volume that was there before us.
is_ours() {
    case $1 in
        anthology-rust-target-* | anthology-cargo | anthology-sccache | anthology-rustup-*) ;;
        *) return 1 ;;
    esac
    docker volume ls -q --filter "label=$label" --filter "name=$1" 2>/dev/null | grep -qxF -- "$1"
}
# in_use V: a running container mounts V.
in_use() {
    [ -n "$(docker ps -q --filter "volume=$1" 2>/dev/null)" ]
}
# drop_containers V: remove the containers that mount V, if V is ours.
drop_containers() {
    is_ours "$1" || return 0
    ids=$(docker ps -aq --filter "volume=$1" 2>/dev/null || true)
    # shellcheck disable=SC2086 # a list of container ids
    [ -z "$ids" ] || docker rm -f $ids >/dev/null 2>&1 || true
}
# drop_volume V: remove V and its containers, if V is ours and no running container uses it.
drop_volume() {
    is_ours "$1" || return 0
    ! in_use "$1" || return 1
    drop_containers "$1"
    docker volume rm "$1" >/dev/null 2>&1 || true
}
sweep() {
    drop_containers "$vol"
}
# drop_image IMAGE: remove it, only if we recorded that we pulled it and every container that uses
# it is ours. Never forced: docker refuses while any container still uses it.
drop_image() {
    [ -n "$1" ] || return 0
    grep -qxF -- "$1" "$pulled" 2>/dev/null || return 0
    for c in $(docker ps -aq --filter "ancestor=$1" 2>/dev/null); do
        mine=
        for m in $(docker inspect -f "{{range .Mounts}}{{.Name}} {{end}}" "$c" 2>/dev/null); do
            ! is_ours "$m" || mine=1
        done
        [ -n "$mine" ] || return 0
    done
    if docker rmi "$1" >/dev/null 2>&1; then
        grep -vxF -- "$1" "$pulled" > "$pulled.new" || true
        mv "$pulled.new" "$pulled"
    fi
}
'

# shellcheck disable=SC2016 # expanded on the host, not here
sweep_body='
sweep
'

# shellcheck disable=SC2016 # expanded on the host, not here
clean_body='
sweep
if is_ours "$vol"; then docker volume rm "$vol" >/dev/null 2>&1 || true; fi
cd "$HOME"
rm -rf -- "$dir" "$dir.stamp"
'

# Everything of ours on the host, as the head defines it: every folder, our volumes with their
# containers, and the image (the first argument) if we pulled it. Nothing else is touched.
# shellcheck disable=SC2016 # expanded on the host, not here
clean_ours_body='
[ "${root##*/}" = anthology-remote ] || exit 2
image=${1:-}
cd "$HOME"
for v in $(docker volume ls -q --filter "label=$label" 2>/dev/null); do
    is_ours "$v" || continue
    drop_containers "$v"
    docker volume rm "$v" >/dev/null 2>&1 || true
done
drop_image "$image"
rm -rf -- "$root"
'

# Before a run: sweep our own old leftovers, check the free space, make the folder, label what we
# create and say whether the host has rsync. Arguments: the free space needed in KB, the age to
# sweep folders and volumes at and the age to sweep the caches and image at (seconds), the image.
# shellcheck disable=SC2016 # expanded on the host, not here
preflight_body='
export LC_ALL=C
min_kb=$1
now=$(date +%s)
cutoff=$((now - $2))
cache_cutoff=$((now - $3))
image=${4:-}
known=

# Folders of ours not used for 12 hours (the age of the .stamp file, else of the folder).
if [ -d "$root" ]; then
    for d in "$root"/*; do
        [ -d "$d" ] || continue
        v=anthology-rust-target-$(printf "%s" "$d" | cksum | cut -d" " -f1)
        known="$known $v"
        ref=$d.stamp
        [ -e "$ref" ] || ref=$d
        [ "$(stat -c %Y "$ref")" -lt "$cutoff" ] || continue
        ! in_use "$v" || continue
        drop_volume "$v" || continue
        rm -rf -- "$d" "$d.stamp"
    done
fi

# Our stopped containers, and volumes of ours that no folder owns and that are 12 hours old.
for v in $(docker volume ls -q --filter "label=$label" --filter name=anthology-rust-target- 2>/dev/null); do
    is_ours "$v" || continue
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

# The shared caches and the image, once nothing of ours has run for 48 hours. With no record of the
# last run, they stay: the record starts now.
last=$root/.last-used
if [ -e "$last" ] && [ "$(stat -c %Y "$last")" -lt "$cache_cutoff" ]; then
    for v in $(docker volume ls -q --filter "label=$label" 2>/dev/null); do
        case $v in anthology-cargo | anthology-sccache | anthology-rustup-*) drop_volume "$v" || true ;; esac
    done
    drop_image "$image"
fi

# The free space, on the home disk and on the Docker data disk.
low=
# check_free LABEL PATH: note it in `low` when the disk of PATH has too little room.
check_free() {
    [ -e "$2" ] || return 0
    free=$(df -Pk "$2" | awk "NR == 2 { print \$4 }")
    if [ -z "$free" ] || [ "$free" -lt "$min_kb" ]; then
        low=1
        echo "remote-build: the build host has less than $((min_kb / 1048576)) GB free on its $1; free space there or run --clean-ours, then try again" >&2
    fi
}
check_free "home disk" "$HOME"
docker_root=$(docker info -f "{{.DockerRootDir}}" 2>/dev/null || true)
[ -z "$docker_root" ] || check_free "Docker disk" "$docker_root"
[ -z "$low" ] || exit 3

mkdir -p "$dir"
touch "$dir.stamp" "$last"

# Create our volumes ourselves, with our label: rust-test.sh would create them bare. A volume that
# is already there stays as it is (and is not ours unless it already has the label).
make_volume() {
    docker volume inspect "$1" >/dev/null 2>&1 || docker volume create --label "$label" "$1" >/dev/null 2>&1 || true
}
make_volume "$vol"
make_volume anthology-cargo
make_volume anthology-sccache
digest=${image##*@sha256:}
[ -z "$image" ] || make_volume "anthology-rustup-${digest:0:12}"

# An image that is not on the host yet is pulled by the run: record that it is ours to remove.
if [ -n "$image" ] && ! docker image inspect "$image" >/dev/null 2>&1; then
    grep -qxF -- "$image" "$pulled" 2>/dev/null || echo "$image" >> "$pulled"
fi
if command -v rsync >/dev/null 2>&1; then echo remote-build-rsync; fi
'

# Reads the NUL-separated list of the files that belong in the folder from stdin and removes the
# rest, then the empty folders. An empty list removes nothing: it means the copy failed.
# shellcheck disable=SC2016 # expanded on the host, not here
trim_body='
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
    touch "$root/.last-used" 2>/dev/null || true
    if [ "$keep" = 1 ]; then
        touch "$dir.stamp" 2>/dev/null || true
        return 0
    fi
    cd "$HOME"
    if is_ours "$vol"; then docker volume rm "$vol" >/dev/null 2>&1 || true; fi
    rm -rf -- "$dir" "$dir.stamp"
}
trap stop INT TERM HUP
trap leave EXIT
sweep
# rust-test.sh removes the target volume when a one-shot run ends, unless told to keep it: keep it
# exactly when we were asked to keep the run warm (otherwise `leave` removes it).
export ANTHOLOGY_KEEP_TARGET=$keep
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
    --clean-ours)
        rssh "$(remote_command "$clean_ours_body" "$image")"
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
probe=$(rssh "$(remote_command "$preflight_body" "$min_free_kb" "$max_age_s" "$cache_age_s" "$image")")
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
rssh "$(remote_command "$trim_body")" < "$list"

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
