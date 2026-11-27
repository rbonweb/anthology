#!/usr/bin/env bash
# atl makes no network calls: run the release binary with the network off.
#
#   scripts/atl-net-test.sh
#
# 1. builds atl/ with `cargo build --release --locked` in the pinned image (the FROM line of
#    ci/images/rust.Dockerfile), with the repository root mounted (atl depends on
#    ../crates/anthology-shell); the target dir is the anthology-atl-release volume;
# 2. builds a throwaway test image FROM that same image plus git (test-only, unpinned Debian
#    package, never shipped; the Dockerfile comes on stdin, so ci/images/ is unchanged);
# 3. runs the binary there with `docker run --network none` on a fixture repo and asserts
#    the exact outputs: the network is off (only `lo`), `atl git status`, `atl rewrite`,
#    `atl hook claude` with and without a rewrite, the `atl hook` subcommands (claude and
#    check only), and that `atl telemetry` does not exist.
#
# Exits non-zero on any mismatch. ci.yml's atl-test job runs it after scripts/atl-test.sh.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
if [ -z "$image" ]; then
    echo "atl-net-test: no FROM line in ci/images/rust.Dockerfile" >&2
    exit 1
fi
net_image=anthology-atl-net

echo "atl-net-test: building the release binary"
docker run --rm \
    -v "$root":/w \
    -v anthology-cargo:/usr/local/cargo/registry \
    -v anthology-atl-release:/target \
    -e CARGO_TARGET_DIR=/target \
    -w /w/atl "$image" cargo build --release --locked

echo "atl-net-test: building the test image ($net_image)"
docker build -q -t "$net_image" - >/dev/null <<EOF
FROM $image
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends git >/dev/null && rm -rf /var/lib/apt/lists/*
EOF

# Runs inside the container, with the network off and the release build at /target.
inner='
set -eu
ATL=/target/release/atl
w=$(mktemp -d)
fail=0
: >"$w/in"

# check NAME WANT_EXIT CMD...: CMD (stdin from $w/in) must exit WANT_EXIT and print
# exactly the bytes of $w/want on stdout.
check() {
    name=$1 want_exit=$2
    shift 2
    set +e
    "$@" <"$w/in" >"$w/got" 2>"$w/err"
    got_exit=$?
    set -e
    if [ "$got_exit" = "$want_exit" ] && cmp -s "$w/want" "$w/got"; then
        echo "ok   $name"
    else
        fail=1
        echo "FAIL $name: exit $got_exit (want $want_exit)"
        echo "  stdout:"; cat -A "$w/got"; echo
        echo "  want:";   cat -A "$w/want"; echo
        echo "  stderr:"; cat "$w/err"
    fi
    : >"$w/in"
}

interfaces() { awk -F: '"'"'NR > 2 {gsub(/ /, "", $1); print $1}'"'"' /proc/net/dev; }
hook_commands() {
    "$ATL" hook --help | awk '"'"'/^Commands:/ {on = 1; next} on && /^$/ {exit} on {print $1}'"'"'
}
telemetry() { "$ATL" telemetry status 2>&1 | head -n 1; }

export HOME=/tmp/home
mkdir -p "$HOME"
git config --global user.email t@example.invalid
git config --global user.name t
git config --global init.defaultBranch main
mkdir -p /tmp/repo
cd /tmp/repo
git init -q
echo a >a.txt
git add a.txt
git commit -qm init
echo b >b.txt
echo changed >a.txt

printf "lo\n" >"$w/want"
check "the network is off (only lo)" 0 interfaces

printf "* main\n M a.txt\n?? b.txt\n" >"$w/want"
check "atl git status" 0 "$ATL" git status

printf "atl git log -5" >"$w/want"
check "atl rewrite \"git log -5\" (exit 3: rewrite, host prompts)" 3 "$ATL" rewrite "git log -5"

printf "%s" "{\"session_id\":\"s1\",\"tool_use_id\":\"t1\",\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"git status\"},\"cwd\":\"/tmp/repo\"}" >"$w/in"
: >"$w/want"
check "atl hook claude (git status is left alone: no allow rule, no proof)" 0 "$ATL" hook claude

# ATL never widens permissions: it rewrites only what the user allows (or a read-only
# proof shows), so the rewrite needs an allow rule of the user.
mkdir -p "$HOME/.claude"
printf "%s" "{\"permissions\":{\"allow\":[\"Bash(git status)\"]}}" >"$HOME/.claude/settings.json"
printf "%s" "{\"session_id\":\"s1\",\"tool_use_id\":\"t1\",\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"git status\"},\"cwd\":\"/tmp/repo\"}" >"$w/in"
printf "%s\n" "{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"permissionDecisionReason\":\"ATL auto-rewrite\",\"updatedInput\":{\"command\":\"atl git status\"},\"permissionDecision\":\"allow\"}}" >"$w/want"
check "atl hook claude (an allowed git status is rewritten)" 0 "$ATL" hook claude

printf "%s" "{\"session_id\":\"s1\",\"tool_use_id\":\"t2\",\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"echo hi\"},\"cwd\":\"/tmp/repo\"}" >"$w/in"
: >"$w/want"
check "atl hook claude (echo hi passes through)" 0 "$ATL" hook claude

printf "claude\ncheck\nhelp\n" >"$w/want"
check "atl hook subcommands are claude and check" 0 hook_commands

printf "%s\n" "error: unrecognized subcommand '"'"'telemetry'"'"'" >"$w/want"
check "atl telemetry does not exist" 0 telemetry

if [ "$fail" -ne 0 ]; then
    echo "atl-net-test: FAIL"
    exit 1
fi
echo "atl-net-test: PASS"
'

docker run --rm --network none \
    -v anthology-atl-release:/target:ro \
    "$net_image" sh -c "$inner"
