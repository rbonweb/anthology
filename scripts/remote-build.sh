#!/usr/bin/env bash
# Run the Docker-based Rust checks (scripts/rust-test.sh) on a Linux build host over
# ssh, so a long build does not load this machine.
#
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh test -p anthology-guard
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh lint
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-ours
#   ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-all-ours
#
# The host is an ssh alias or name, taken only from the environment: no host detail belongs in the
# repo. Key-based login must already work (ssh -o BatchMode=yes); the script never prompts, never
# elevates, never edits the host's ssh or auth setup and never installs anything there. The host
# needs Docker for the login user, bash 4 or newer, tar and flock (util-linux); rsync is used when
# both ends have it.
#
# The host is shared by many agents at once, so every run is its own: it has its own folder and
# target volume, takes the locks below, and removes only what it made. What we create, and how we
# tell it from the rest:
#   - the folders under ~/anthology-remote (a path nothing else uses), one per run: <checkout>-<run>,
#     where <checkout> is a hash of the checkout's path. With ANTHOLOGY_KEEP_REMOTE=1 the folder is
#     <checkout> alone, so a re-run of that checkout finds it again;
#   - the volumes anthology-rust-target-<n> (one per folder), anthology-cargo, anthology-sccache and
#     anthology-rustup-<digest>. The script creates them first with the label
#     anthology.remote-build=1 (rust-test.sh would create them bare), and treats a volume as ours
#     only if its name matches AND it has the label: a volume with the same name and no label was
#     there before us and is never touched;
#   - the containers that mount one of those volumes (rust-test.sh does not name or label its
#     containers, so the volume is the handle);
#   - the Rust image, only if it was absent when we started a run: then we record it in
#     ~/anthology-remote/.pulled-images. We remove it only from that record, only when no container
#     that is not ours uses it, and never with a forced removal.
# No unfiltered cleanup command is ever used, and nothing outside ~/anthology-remote and the items
# above is touched.
#
# Locks, all flock(1) files under ~/anthology-remote. The kernel drops a flock when its holder dies,
# so a lock that can be taken belongs to a dead run: that is how a crashed run is told from a live
# one, with no PID or clock to go stale.
#   - <folder>.lock: held by a run for its whole life. Whatever sweeps or cleans skips a folder
#     whose lock is held, and reclaims one whose lock is free;
#   - .runs.lock: held shared by every run. --clean-all-ours and the 48-hour cache sweep take it
#     exclusive and no-wait, so they refuse (or skip) while any run is live;
#   - .slots/<i>: a counting semaphore. At most `slots` runs build at once, each with `jobs` cargo
#     jobs (CARGO_BUILD_JOBS), both derived from the host's cores and memory (3 GB per job): more
#     runs than that wait for a slot, so the host is not driven into the OOM killer (exit 137).
#     ANTHOLOGY_REMOTE_SLOTS and ANTHOLOGY_REMOTE_JOBS override them;
#   - .toolchain-<digest>.lock: the toolchain in the anthology-rustup-<digest> volume is installed
#     under the exclusive lock, once (a marker file records it), while every run holds it shared. Two
#     installs into one volume at once leave a half-installed component; a failed install is
#     retried once on a fresh volume.
#
#   1. at its start a run takes its locks. When nothing else is running it also sweeps the shared
#      caches and the pulled image, if nothing of ours has run for 48 hours (the time of
#      ~/anthology-remote/.last-used, written at the start and the end of each run);
#   2. it sweeps the leftovers of other runs: folders (and their volumes) of runs that died, and
#      kept folders and volumes unused for 12 hours (a folder's age is that of its .stamp file, a
#      volume without a folder goes by its creation time), and stopped containers. A volume that a
#      running container uses is left alone;
#   3. it refuses to start when the host has less than 20 GB free, on the home disk or the Docker
#      data disk;
#   4. it copies this checkout to its folder: the tracked files plus the uncommitted and new files
#      that git does not ignore, never target/ or .claude/worktrees. With ANTHOLOGY_KEEP_REMOTE=1 a
#      re-run sends only what changed (rsync) and removes what left the checkout;
#   5. it waits for a build slot, makes sure the toolchain is installed, runs scripts/rust-test.sh
#      there with the arguments given, streams its output and exits with its exit code;
#   6. when the run ends, passed or failed, on Ctrl-C, when the ssh link drops and when this script
#      is killed, the host removes the containers of the run (the ones that mount its target volume)
#      and, unless ANTHOLOGY_KEEP_REMOTE=1 is set, the run's folder and its target volume. The host
#      side does it by itself: this script sends a line every 10 seconds, and when they stop it
#      stops the run and cleans up.
#   --clean-ours removes what this checkout left on the host: its folders, volumes and containers,
#   except those of a run that is still going (--clean is the same).
#   --clean-all-ours removes everything of ours there, as defined above: every folder, our volumes
#   with their containers, the caches and the image we pulled. It refuses while any run is live.
#
# ANTHOLOGY_REMOTE_SYNC=tar forces the tar copy even where rsync exists (the tests use it).
set -eu
set -o pipefail

usage() {
    cat >&2 <<'EOF'
usage: ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh <rust-test.sh args...>
       ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-ours
       ANTHOLOGY_BUILD_HOST=<alias> scripts/remote-build.sh --clean-all-ours

  <alias> is the ssh alias of the build host. The arguments are those of scripts/rust-test.sh,
  for example `test -p anthology-guard` or `lint`. After the run the host's copy of this checkout
  and its target volume are removed; ANTHOLOGY_KEEP_REMOTE=1 keeps them for the next run.
  --clean-ours (also --clean) removes what this checkout left on the host, never a run that is
  still going. --clean-all-ours removes everything of ours on the host, caches and the Rust image
  we pulled included, and nothing else; it refuses while any run is live.
  ANTHOLOGY_REMOTE_SLOTS=<n> and ANTHOLOGY_REMOTE_JOBS=<n> set how many runs build at once on the
  host and how many cargo jobs each gets (the defaults follow the host's cores and memory).
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
    --clean | --clean-ours | --clean-all-ours)
        [ "$#" -eq 1 ] || usage
        clean=$1
        ;;
    --clean-all)
        echo "remote-build: --clean-all is now --clean-all-ours" >&2
        exit 2
        ;;
esac

# 1 keeps the checkout's folder and target volume on the host after the run.
keep=0
[ "${ANTHOLOGY_KEEP_REMOTE:-}" != 1 ] || keep=1
# The caps on concurrent builds; empty means the host works them out.
jobs_set=${ANTHOLOGY_REMOTE_JOBS:-}
slots_set=${ANTHOLOGY_REMOTE_SLOTS:-}
for setting in "$jobs_set" "$slots_set"; do
    case $setting in
        *[!0-9]* | 0)
            echo "remote-build: ANTHOLOGY_REMOTE_JOBS and ANTHOLOGY_REMOTE_SLOTS must be whole numbers above 0" >&2
            exit 2
            ;;
    esac
done
# The free space the host must have (20 GB, in KB) and the age at which we sweep our own leftovers
# (12 hours, in seconds).
min_free_kb=20971520
max_age_s=43200
# How long nothing of ours may have run before we remove the shared caches and the image (48 hours).
cache_age_s=172800

root=$(cd "$(dirname "$0")/.." && pwd)
# One name per checkout; each run adds its own suffix below. The volume name comes from the folder's
# path on the host (rust-test.sh hashes its own root), so that is worked out there, not here.
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
# vol_of FOLDER: the name of the target volume of a run in FOLDER (the runner hashes its own root).
vol_of() {
    printf "anthology-rust-target-%s" "$(printf "%s" "$1" | cksum | cut -d" " -f1)"
}
vol=$(vol_of "$dir")

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
# reclaim D: remove folder D with its volume and containers, unless a run holds the lock of D (the
# run is alive) or a container still uses the volume. The kernel drops a flock when its holder
# dies, so a lock that can be taken belongs to a dead run.
reclaim() {
    (
        exec 5>>"$1.lock" || exit 1
        flock -n -x 5 || exit 1
        drop_volume "$(vol_of "$1")" || exit 1
        cd "$HOME"
        rm -rf -- "$1" "$1.stamp" "$1.lock"
    )
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
# idle_sweep AGE: the shared caches and the image, once nothing of ours has run for AGE seconds. With
# no record of the last run, they stay: the record starts now. Call it only while no run is live.
idle_sweep() {
    last=$root/.last-used
    [ -e "$last" ] || return 0
    [ "$(stat -c %Y "$last")" -lt "$(($(date +%s) - $1))" ] || return 0
    for v in $(docker volume ls -q --filter "label=$label" 2>/dev/null); do
        case $v in anthology-cargo | anthology-sccache | anthology-rustup-*) drop_volume "$v" || true ;; esac
    done
    drop_image "$image"
}
'

# The holder: the first thing a run starts, and the last to end. It takes the run's locks, makes the
# folder, says so, and stays until its stdin ends (the local script sends a line every 10 seconds; no
# line for 90 seconds, or the end of the stream, means the run is over). Then it removes what the run
# made. Because it lives on the host, a killed local script or a dropped link still gets cleaned up.
# The argument is the age for the cache sweep.
# shellcheck disable=SC2016 # expanded on the host, not here
hold_body='
export LC_ALL=C
mkdir -p "$root"
exec 7>>"$root/.runs.lock" || exit 1
# Taken exclusive only when no run holds it: nothing is running, so the caches may be swept.
if flock -n -x 7; then idle_sweep "$1"; fi
flock -s -w 120 7 || { echo "remote-build: the build host is busy with a cleanup; try again" >&2; exit 4; }
exec 6>>"$dir.lock" || exit 1
if ! flock -x -w 5 6; then
    echo "remote-build: another run of this checkout holds its kept folder; wait for it, or run without ANTHOLOGY_KEEP_REMOTE=1" >&2
    exit 4
fi
leave() {
    sweep
    touch "$root/.last-used" 2>/dev/null || true
    if [ "$keep" = 1 ]; then
        touch "$dir.stamp" 2>/dev/null || true
        return 0
    fi
    cd "$HOME"
    if is_ours "$vol"; then docker volume rm "$vol" >/dev/null 2>&1 || true; fi
    rm -rf -- "$dir" "$dir.stamp" "$dir.lock"
}
trap "exit 255" INT TERM HUP
trap leave EXIT
mkdir -p "$dir"
touch "$dir.stamp" "$root/.last-used"
echo remote-build-held
while read -r -t 90 _; do :; done
'

# Before a run: sweep the leftovers of other runs, check the free space, label what we create and say
# whether the host has rsync. Arguments: the free space needed in KB and the age to sweep kept
# folders and volumes at (seconds).
# shellcheck disable=SC2016 # expanded on the host, not here
preflight_body='
export LC_ALL=C
min_kb=$1
now=$(date +%s)
cutoff=$((now - $2))
live=

# due D: should folder D be reclaimed? The folder of a run (a dash in its name) that has a lock file:
# yes, reclaim then decides by the lock. Any other (a kept folder, or one from before the locks):
# only when unused for 12 hours (the age of the .stamp file, else of the folder).
due() {
    case ${1##*/} in
        *-*) [ ! -e "$1.lock" ] || return 0 ;;
    esac
    ref=$1.stamp
    [ -e "$ref" ] || ref=$1
    mtime=$(stat -c %Y "$ref" 2>/dev/null) || return 1
    [ "$mtime" -lt "$cutoff" ]
}

# Folders of other runs. A dead run is removed at once; what stays has a live volume.
for d in "$root"/[0-9]*; do
    [ -d "$d" ] || continue
    v=$(vol_of "$d")
    if [ "$d" != "$dir" ] && due "$d" && reclaim "$d"; then continue; fi
    live="$live $v"
done
# Lock and stamp files of folders that are gone, if their lock is free.
for f in "$root"/*.lock; do
    [ -e "$f" ] || continue
    [ ! -d "${f%.lock}" ] || continue
    ( exec 5>>"$f" && flock -n -x 5 && rm -f -- "$f" "${f%.lock}.stamp" ) 2>/dev/null || true
done

# Stopped containers on our volumes, and volumes of ours that no folder owns and that are 12 hours old.
for v in $(docker volume ls -q --filter "label=$label" --filter name=anthology-rust-target- 2>/dev/null); do
    is_ours "$v" || continue
    ids=$(docker ps -aq --filter status=exited --filter status=dead --filter "volume=$v" 2>/dev/null || true)
    # shellcheck disable=SC2086 # a list of container ids
    [ -z "$ids" ] || docker rm -f $ids >/dev/null 2>&1 || true
    case "$live $vol " in *" $v "*) continue ;; esac
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
        echo "remote-build: the build host has less than $((min_kb / 1048576)) GB free on its $1; free space there or run --clean-all-ours, then try again" >&2
    fi
}
check_free "home disk" "$HOME"
docker_root=$(docker info -f "{{.DockerRootDir}}" 2>/dev/null || true)
[ -z "$docker_root" ] || check_free "Docker disk" "$docker_root"
[ -z "$low" ] || exit 3

mkdir -p "$dir"
touch "$dir.stamp" "$root/.last-used"

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

# The run. The tests run in the background; the ssh link feeds our stdin with a line every 10
# seconds. When the lines stop or the link drops, the watcher stops the run. The holder (hold_body)
# removes the run's containers, folder and volume afterwards.
# shellcheck disable=SC2016 # expanded on the host, not here
run_body='
export LC_ALL=C
cd "$dir"
pid=
watcher=
stop() {
    [ -z "$pid" ] || kill "$pid" 2>/dev/null || true
    [ -z "$watcher" ] || kill "$watcher" 2>/dev/null || true
    exit 255
}
trap stop INT TERM HUP
{ while read -r -t 90 _; do :; done; kill -TERM $$; } <&0 >/dev/null 2>&1 &
watcher=$!

# The cap on concurrent builds: the budget is the number of cargo jobs the host can carry, a job
# being a core and 3 GB. A run takes one of `slots` slots and gets budget/slots jobs.
cpus=$(nproc 2>/dev/null || echo 1)
mem_gb=$(awk "/^MemTotal:/ { print int(\$2 / 1048576) }" /proc/meminfo 2>/dev/null || echo 3)
budget=$((mem_gb / 3))
[ "$budget" -le "$cpus" ] || budget=$cpus
[ "$budget" -ge 1 ] || budget=1
slots=${slots_set:-$((budget / 4))}
[ "$slots" -ge 1 ] || slots=1
jobs=${jobs_set:-$((budget / slots))}
[ "$jobs" -ge 1 ] || jobs=1

# acquire_slot: hold the lock of a free slot on fd 9 until this shell ends.
acquire_slot() {
    mkdir -p "$root/.slots"
    waited=
    while :; do
        i=1
        while [ "$i" -le "$slots" ]; do
            exec 9>>"$root/.slots/$i"
            if flock -n -x 9; then return 0; fi
            exec 9>&-
            i=$((i + 1))
        done
        [ -n "$waited" ] || echo "remote-build: all $slots build slots of the host are busy; waiting for one" >&2
        waited=1
        sleep 2
    done
}

# The toolchain in the anthology-rustup-<digest> volume: installed once, by one run at a time, while
# the others wait. The marker file holds the volume time and the toolchain file it was installed for,
# so a removed volume or a new toolchain is installed again. Every run holds the lock shared on fd 8
# while it builds, so an install (exclusive) never meets a build.
digest=${image##*@sha256:}
rustup_vol=anthology-rustup-${digest:0:12}
marker=$root/.toolchain-${digest:0:12}
installed_for() {
    printf "%s|%s" "$(docker volume inspect -f "{{.CreatedAt}}" "$rustup_vol" 2>/dev/null)" \
        "$({ cksum < rust-toolchain.toml; } 2>/dev/null || true)"
}
toolchain_ready() {
    [ -f "$marker" ] && [ "$(cat "$marker")" = "$(installed_for)" ]
}
install_toolchain() {
    ANTHOLOGY_KEEP_TARGET=1 scripts/rust-test.sh run true < /dev/null
}
ensure_toolchain() {
    [ -n "$image" ] || return 0
    exec 8>>"$marker.lock"
    # Each round: ready? done. Else try for the exclusive lock for 5 seconds; a run that installs
    # holds it, so the others fall back to the shared lock and find the marker when it is done.
    # (Taking the exclusive lock gives up the shared one, so a run that fails to get it holds nothing.)
    rounds=0
    while [ "$rounds" -lt 360 ]; do
        rounds=$((rounds + 1))
        flock -s -w 1800 8 || return 1
        ! toolchain_ready || return 0
        flock -x -w 5 8 || continue
        if ! toolchain_ready; then
            echo "remote-build: installing the toolchain; other runs wait for it" >&2
            if ! install_toolchain; then
                # A killed install leaves a half-installed component that fails every next install:
                # start the volume over (nobody else uses it: we hold the lock exclusive).
                echo "remote-build: the toolchain install failed; trying again on a new volume" >&2
                is_ours "$rustup_vol" || return 1
                docker volume rm "$rustup_vol" >/dev/null 2>&1 || return 1
                docker volume create --label "$label" "$rustup_vol" >/dev/null 2>&1 || return 1
                install_toolchain || return 1
            fi
            installed_for > "$marker"
        fi
        flock -s -w 1800 8
        return
    done
    return 1
}

acquire_slot
ensure_toolchain || { echo "remote-build: the toolchain could not be installed" >&2; exit 5; }
cd "$dir" || { echo "remote-build: the run folder is gone" >&2; exit 1; }
sweep
# rust-test.sh removes the target volume when a one-shot run ends, unless told to keep it: keep it
# exactly when we were asked to keep the run warm (otherwise the holder removes it).
export ANTHOLOGY_KEEP_TARGET=$keep CARGO_BUILD_JOBS=$jobs
scripts/rust-test.sh "$@" &
pid=$!
rc=0
wait "$pid" || rc=$?
kill "$watcher" 2>/dev/null || true
if [ "$rc" = 137 ]; then
    echo "remote-build: the host killed the run (exit 137), most likely for memory; set ANTHOLOGY_REMOTE_SLOTS=1 or a lower ANTHOLOGY_REMOTE_JOBS" >&2
fi
exit "$rc"
'

# --clean-ours: what this checkout left on the host: every folder named for it, with its volume and
# containers, except where a run still holds the lock of the folder.
# shellcheck disable=SC2016 # expanded on the host, not here
clean_ours_body='
[ "${root##*/}" = anthology-remote ] || exit 2
cd "$HOME"
for d in "$dir" "$dir"-*; do
    [ -d "$d" ] || continue
    reclaim "$d" || echo "remote-build: left ${d##*/} alone: a run is still using it" >&2
done
'

# --clean-all-ours: everything of ours on the host, as the head defines it: every folder, our volumes
# with their containers, and the image if we pulled it. Nothing else is touched. It takes the runs
# lock exclusive and refuses when any run holds it. The lock file stays: a run that is starting may
# already have it open.
# shellcheck disable=SC2016 # expanded on the host, not here
clean_all_body='
[ "${root##*/}" = anthology-remote ] || exit 2
cd "$HOME"
if [ -d "$root" ]; then
    exec 7>>"$root/.runs.lock" || exit 1
    if ! flock -n -x 7; then
        echo "remote-build: refusing --clean-all-ours: another run on the build host is still going; wait for it, or use --clean-ours for this checkout" >&2
        exit 4
    fi
fi
for v in $(docker volume ls -q --filter "label=$label" 2>/dev/null); do
    is_ours "$v" || continue
    drop_containers "$v"
    docker volume rm "$v" >/dev/null 2>&1 || true
done
drop_image "$image"
[ ! -d "$root" ] || find "$root" -mindepth 1 ! -name .runs.lock -delete
'

# The command string for the host: one of the scripts above, its folder under the host's $HOME
# (expanded there) and the arguments, each quoted.
remote_command() {
    local body=$1 cmd prefix arg
    shift
    prefix="keep=$keep
image=$(q "$image")
jobs_set=$jobs_set
slots_set=$slots_set"
    cmd="bash -c $(q "$prefix$remote_head$body") remote-build \"\$HOME/$rdir\""
    for arg in "$@"; do
        cmd="$cmd $(q "$arg")"
    done
    printf '%s' "$cmd"
}

case $clean in
    --clean | --clean-ours)
        rssh "$(remote_command "$clean_ours_body")"
        echo "remote-build: removed what this checkout left on the build host" >&2
        exit 0
        ;;
    --clean-all-ours)
        rssh "$(remote_command "$clean_all_body")"
        echo "remote-build: removed everything of ours from the build host" >&2
        exit 0
        ;;
esac

# This run's own folder, so its target volume and everything else is its own too. A kept run uses the
# checkout's name alone: the next run finds it again.
[ "$keep" = 1 ] || rdir=$rdir-$(date +%s)-$$-$RANDOM

tmp=$(mktemp -d)
beat_hold=
beat_run=
hold_pid=
ssh_pid=
# shellcheck disable=SC2329 # run by the EXIT trap
finish() {
    [ -z "$beat_run" ] || kill "$beat_run" 2>/dev/null || true
    [ -z "$beat_hold" ] || kill "$beat_hold" 2>/dev/null || true
    # The holder on the host sees its stdin end, stops what is left and removes the run's folder and
    # volume; wait for that, so nothing of the run is on the host when this script returns.
    [ -z "$hold_pid" ] || wait "$hold_pid" 2>/dev/null || true
    rm -rf "$tmp"
}
# shellcheck disable=SC2329 # run by the signal trap
on_signal() {
    [ -z "$ssh_pid" ] || kill "$ssh_pid" 2>/dev/null || true
    exit 130
}
trap finish EXIT
trap on_signal INT TERM HUP

# beat: a line every 10 seconds for as long as this script lives. The host stops the run and cleans
# up when they stop, even if this script is killed outright.
beat() {
    local parent=$$
    while kill -0 "$parent" 2>/dev/null; do
        printf '\n'
        sleep 10 > /dev/null
    done
}

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

# Start the holder: it takes the run's locks and makes the folder, and we go on once it says so.
mkfifo "$tmp/hold" "$tmp/run"
beat > "$tmp/hold" &
beat_hold=$!
rssh "$(remote_command "$hold_body" "$cache_age_s")" < "$tmp/hold" > "$tmp/held" &
hold_pid=$!
until grep -qx remote-build-held "$tmp/held" 2>/dev/null; do
    if ! kill -0 "$hold_pid" 2>/dev/null; then
        rc=0
        wait "$hold_pid" || rc=$?
        hold_pid=
        [ "$rc" -ne 0 ] || rc=1
        exit "$rc"
    fi
    sleep 0.2
done

# Sweep the leftovers of other runs, refuse a host short of disk (its message and exit code 3 come
# through), and see whether the host has rsync.
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
rssh "$(remote_command "$trim_body")" < "$list"

# The run. If we die or the link drops, the host stops getting lines and stops the tests.
beat > "$tmp/run" &
beat_run=$!
rssh "$(remote_command "$run_body" "$@")" < "$tmp/run" &
ssh_pid=$!
rc=0
wait "$ssh_pid" || rc=$?
ssh_pid=
exit "$rc"
