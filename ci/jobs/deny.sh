#!/bin/sh
# CI job `deny` (design §14.1): `cargo deny --locked check` (advisories, bans, licenses, sources;
# deny.toml), then the serde_json tree check (m-11). Runs in the pinned Rust image through
# scripts/rust-test.sh. cargo-deny is a pinned release binary, checked against its sha256.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
if [ "${1:-}" != --inside ]; then
    exec "$root/scripts/rust-test.sh" run sh ci/jobs/deny.sh --inside
fi

version=0.22.0
case $(uname -m) in
    x86_64) arch=x86_64 sha=98c56e6a408bfa5c3c60744b6015aaa6aa08f743fdeb88386d06fca035bb1786 ;;
    aarch64) arch=aarch64 sha=4818d44cdcac285776336504fca722493ab634d3377b94a462e98aad3f41484a ;;
    *) echo "deny: no pinned cargo-deny for $(uname -m)" >&2; exit 1 ;;
esac
name="cargo-deny-$version-$arch-unknown-linux-musl"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --proto '=https' --proto-redir '=https' -o "$tmp/deny.tar.gz" \
    "https://github.com/EmbarkStudios/cargo-deny/releases/download/$version/$name.tar.gz"
echo "$sha  $tmp/deny.tar.gz" | sha256sum -c - >/dev/null
tar -xzf "$tmp/deny.tar.gz" -C "$tmp"

"$tmp/$name/cargo-deny" --locked check

shipped="--target aarch64-apple-darwin --target x86_64-apple-darwin \
--target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl"
# shellcheck disable=SC2086 # $shipped is a list of options
if cargo tree --locked -p anthology-cli -e normal $shipped --prefix none | grep '^serde_json '; then
    echo "deny: serde_json is in the shipped graph (m-11)" >&2
    exit 1
fi
echo "deny: no serde_json in the anthology-cli graph"
