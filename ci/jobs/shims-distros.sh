#!/bin/sh
# CI job `shims-distros` (design §8.4, §13.4, §14.1; BS1 lead ruling T6). The repo-tests shim
# suite in the pinned alpine, debian-slim (+ nodejs) and ubuntu (+ curl, wget) images, with the
# real downloaders against the testkit TLS server standing in for github.com, then
# `claude plugin validate --strict` on the plugins with the staging overlay.
#
#   (a) Build tests/shims.rs as one static musl binary in the pinned Rust image.
#   (b) In each image: install the test CA, add the job-start packages, create a non-root user
#       and run the binary as that user, with ANTHOLOGY_SHIMS_NET=1 and the image's REQUIRE
#       list. github.com resolves to 127.0.0.1 and port 443 may be bound unprivileged.
#   (c) On this host: npm ci of ci/claude-code into a scratch folder, then validate copies of
#       plugins/anthology and plugins/anthology-guardrails with port/staging overlaid.
#
# It runs the same on an arm64 Docker VM. Nothing here pipes test output through tail: the log
# is kept whole, and failures are found with grep.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
from() {
    awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/$1.Dockerfile"
}
rust_image=$(from rust)
digest=${rust_image##*@sha256:}
digest=$(printf '%.12s' "$digest")
checkout=$(printf '%s' "$root" | cksum | cut -d' ' -f1)
out=target/shims-distros
mkdir -p "$root/$out"

# --- (a) the static test binary ---------------------------------------------------------------
# shellcheck disable=SC2016 # expanded by the container shell
build='
set -eu
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends musl-tools >/dev/null
rustup toolchain install >/dev/null 2>&1 || rustup toolchain install
triple=$(uname -m)-unknown-linux-musl
cc_var=CC_$(printf %s "$triple" | tr - _)
export "$cc_var=musl-gcc"
cargo test --locked -p repo-tests --test shims --no-run --target "$triple" 2>build.err ||
    { cat build.err >&2; exit 1; }
exe=$(sed -n "s/^ *Executable tests\/shims.rs (\(.*\))$/\1/p" build.err | head -n 1)
rm -f build.err
[ -n "$exe" ] && [ -x "$exe" ] || { echo "shims-distros: no test executable" >&2; exit 1; }
cp "$exe" "/w/'"$out"'/shims"
'
echo "shims-distros: building the static shim tests in $rust_image"
docker run --rm \
    -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-}" \
    -e CARGO_TARGET_DIR=/target \
    -v "$root":/w \
    -v anthology-cargo:/usr/local/cargo/registry \
    -v "anthology-rustup-$digest":/usr/local/rustup \
    -v "anthology-shims-target-$checkout":/target \
    -w /tmp "$rust_image" sh -c "cd /w && $build"

# --- (b) the images -------------------------------------------------------------------------
# shellcheck disable=SC2016 # expanded by the container shell
run_tests='
set -eu
ca=/w/crates/anthology-testkit/fixtures/bs/ca.pem
case $1 in
    alpine)
        cat "$ca" >>/etc/ssl/certs/ca-certificates.crt
        adduser -D -s /bin/sh tester
        ;;
    debian-slim)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y -qq --no-install-recommends nodejs >/dev/null
        useradd -m -s /bin/sh tester
        ;;
    ubuntu)
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq
        apt-get install -y -qq --no-install-recommends curl wget ca-certificates >/dev/null
        cp "$ca" /usr/local/share/ca-certificates/anthology-shims-test.crt
        update-ca-certificates >/dev/null
        useradd -m -s /bin/sh tester
        ;;
esac
cp /w/'"$out"'/shims /tmp/shims
chmod 755 /tmp/shims
exec su tester -s /bin/sh -c "cd /tmp && exec env -i HOME=/home/tester PATH=$PATH LANG=C \
    ANTHOLOGY_SHIMS_NET=1 ANTHOLOGY_SHIMS_REQUIRE=$2 /tmp/shims --test-threads=12"
'
status=0
for image in alpine debian-slim ubuntu; do
    case $image in
        alpine) require=busybox,busybox-wget,net,nonroot ;;
        debian-slim) require=dash,bash,node,net,nonroot ;;
        ubuntu) require=dash,bash,curl,gnu-wget,net,nonroot ;;
    esac
    ref=$(from "$image")
    log="$root/$out/$image.log"
    echo "shims-distros: $image ($ref), requiring $require"
    if docker run --rm \
        --add-host github.com:127.0.0.1 \
        --sysctl net.ipv4.ip_unprivileged_port_start=0 \
        -v "$root":/w:ro \
        "$ref" sh -c "$run_tests" shims "$image" "$require" >"$log" 2>&1; then
        rc=0
    else
        rc=$?
    fi
    grep -E '^test result:|^failures:|^    [a-z_:]+$|panicked|^skip:' "$log" || true
    if [ "$rc" -ne 0 ] || ! grep -q '^test result: ok\.' "$log" || grep -q 'FAILED' "$log"; then
        echo "shims-distros: $image FAILED (exit $rc); the full log follows" >&2
        cat "$log" >&2
        status=1
    else
        echo "shims-distros: $image ok"
    fi
done

# --- (c) claude plugin validate --strict with the staging overlay ---------------------------
echo "shims-distros: claude plugin validate --strict (staging overlay)"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/shims-validate.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
# The pinned Claude Code, installed into the scratch folder so the checkout stays clean.
mkdir "$scratch/claude-code"
cp "$root/ci/claude-code/package.json" "$root/ci/claude-code/package-lock.json" \
    "$scratch/claude-code/"
npm ci --prefix "$scratch/claude-code" >/dev/null
claude=$scratch/claude-code/node_modules/.bin/claude
"$claude" --version
for plugin in anthology anthology-guardrails; do
    cp -R "$root/plugins/$plugin" "$scratch/$plugin"
    cp -R "$root/port/staging/plugins/$plugin/." "$scratch/$plugin/"
    if DISABLE_AUTOUPDATER=1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
        "$claude" plugin validate --strict "$scratch/$plugin"; then
        echo "shims-distros: $plugin validates"
    else
        echo "shims-distros: $plugin does not validate" >&2
        status=1
    fi
done
exit "$status"
