#!/bin/sh
# CI job `mods`: the anthology plugin's Claude Code mods (plugins/anthology/hooks/mods/), checked
# with the Claude Code version pinned in ci/claude-code, in the pinned node image
# (ci/images/node.Dockerfile). `claude plugin validate --strict` reads the hooks module the way
# the engine loads it (and type-checks it); `claude plugin test` runs tests/mods/*.test.ts.
#
#   ci/jobs/mods.sh        validate and test (CI)
#   ci/jobs/mods.sh tsc    dev only: lay Claude Code's declarations for the pinned version into
#                          plugins/anthology/.claude-plugin/types/ (gitignored) and run tsc
#
# The worktree is the only mount, read-only except for `tsc`. Claude Code is installed under
# /tmp inside the container, so nothing lands in the worktree but the gitignored types.
set -eu

mode=${1:-check}
case $mode in
    check | tsc) ;;
    *)
        echo "usage: ci/jobs/mods.sh [tsc]" >&2
        exit 2
        ;;
esac

if [ "${ANTHOLOGY_MODS_IN_CONTAINER:-}" != 1 ]; then
    root=$(cd "$(dirname "$0")/../.." && pwd)
    image=$(awk '$1 == "FROM" {print $NF; exit}' "$root/ci/images/node.Dockerfile")
    if [ -z "$image" ]; then
        echo "mods: no FROM line in ci/images/node.Dockerfile" >&2
        exit 1
    fi
    mount="$root:/w:ro"
    [ "$mode" = check ] || mount="$root:/w"
    exec docker run --rm --name "anthology-mods-$(basename "$root")-$$" \
        -v "$mount" -w /w -e ANTHOLOGY_MODS_IN_CONTAINER=1 \
        -e DISABLE_AUTOUPDATER=1 -e CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
        "$image" timeout 1200 sh ci/jobs/mods.sh "$mode"
fi

# Inside the container from here on.
export HOME=/tmp/home
mkdir -p /tmp/cc "$HOME"
cp ci/claude-code/package.json ci/claude-code/package-lock.json /tmp/cc/
npm ci --prefix /tmp/cc --no-audit --no-fund >&2
claude=/tmp/cc/node_modules/.bin/claude
"$claude" --version

plugin=plugins/anthology
if [ "$mode" = check ]; then
    report=$("$claude" plugin validate --strict "$plugin")
    printf '%s\n' "$report"
    # A hook that fails without a .catch is skipped, which the kit cannot tell from the .catch
    # replaying the verdict; validate names each gating hook either way, so hold it to the .catch.
    if printf '%s\n' "$report" | grep -q 'gating hook without \.catch'; then
        echo "mods: a gating hook has no .catch; give it .catch((\$, e, next) => next(e))" >&2
        exit 1
    fi
    "$claude" plugin test "$plugin"
    exit 0
fi

# tsc (dev only). The npm package ships the native binary, which holds the declarations as an
# embedded compressed file; scripts/mods-types.mjs takes them out.
version=$(node -p 'require("/tmp/cc/node_modules/@anthropic-ai/claude-code/package.json").version')
native=$(find /tmp/cc/node_modules/@anthropic-ai -path '*claude-code-linux-*/claude' -type f | head -n 1)
node scripts/mods-types.mjs "$native" "$version" "$plugin/.claude-plugin/types/claude-code/index.d.ts"
npx -y -p typescript@5.9.3 tsc -p "$plugin"
echo "mods: tsc clean"
