#!/usr/bin/env bash
# $RTK_TEST: rtk's cargo test suite in the pinned Rust image.
#
#   scripts/rtk-test.sh                  # the canonical run
#   scripts/rtk-test.sh --no-fail-fast   # extra arguments go to `cargo test --locked`
#
# The image is the FROM line of ci/images/rust.Dockerfile. The slim image lacks tools
# rtk's tests run (git, curl, jq, kill) and runs as root, where a read-only directory
# is still writable. So, inside the container only, this installs those test-only
# tools from the image's own Debian release (never shipped), then runs the tests as
# a non-root user. Local runs, ci.yml's rtk-test job and sync-rtk.sh all use it.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/rust.Dockerfile")
if [ -z "$image" ]; then
    echo "rtk-test: no FROM line in ci/images/rust.Dockerfile" >&2
    exit 1
fi

# Runs as root in the container; "$@" are the extra cargo test arguments.
inner='
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends git curl jq procps ca-certificates >/dev/null
# The tester owns the source tree when the bind mount maps a non-root owner
# (Linux CI); otherwise it gets uid 1000.
uid=$(stat -c %u /w)
[ "$uid" -ne 0 ] || uid=1000
getent group "$uid" >/dev/null || groupadd -g "$uid" rtktest
getent passwd "$uid" >/dev/null || useradd -m -u "$uid" -g "$uid" -s /bin/sh rtktest
user=$(getent passwd "$uid" | cut -d: -f1)
home=$(getent passwd "$uid" | cut -d: -f6)
# The cargo home (with the registry volume) and the target volume persist between
# runs: hand over only what is not the tester'\''s yet.
find /usr/local/cargo /target ! -uid "$uid" -exec chown -h "$uid:$uid" {} +
exec runuser -u "$user" -- env -i \
    HOME="$home" USER="$user" LANG=C.UTF-8 \
    PATH="$PATH" CARGO_HOME="$CARGO_HOME" RUSTUP_HOME="$RUSTUP_HOME" \
    CARGO_TARGET_DIR=/target \
    cargo test --locked "$@"
'

exec docker run --rm \
    -v "$root/rtk":/w \
    -v anthology-cargo:/usr/local/cargo/registry \
    -v anthology-rtk-target:/target \
    -w /w "$image" sh -c "$inner" rtk-test "$@"
