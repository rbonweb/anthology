#!/usr/bin/env bash
# Merge an upstream rtk release into rtk/ on a review branch.
#
#   scripts/sync-rtk.sh v1.2.3
#
# 1. refuses to run on a dirty tree;
# 2. creates branch rtk-sync/<tag> from HEAD;
# 3. git subtree pull --prefix=rtk https://github.com/rtk-ai/rtk <tag>
#    (upstream edits to files the trim deletes are modify/delete conflicts, "deleted by us":
#    they are resolved by deletion, as the trim would delete them again; on any other
#    conflict it prints the conflicted files and stops, leaving the branch for review);
# 4. records the merged tag and its commit as the upstream pin in vendor/sources.json;
# 5. runs scripts/rtk-trim.sh (required: the sync fails without it);
# 6. runs $RTK_TEST (scripts/rtk-test.sh: the pinned image from ci/images/rust.Dockerfile);
# 7. prints a summary.
#
# The pin and the trim's changes are left uncommitted on the branch, for review with it.
# It never pushes and never merges into another branch: the branch is left for review.
set -eu

UPSTREAM=https://github.com/rtk-ai/rtk
PREFIX=rtk

usage() {
    echo "usage: scripts/sync-rtk.sh <tag>   (an upstream release tag such as v1.2.3)" >&2
    exit 2
}

[ "$#" -eq 1 ] || usage
tag=$1
tag_pattern='^v[0-9]+\.[0-9]+\.[0-9]+$'
if ! [[ $tag =~ $tag_pattern ]]; then
    echo "sync-rtk: '$tag' is not a release tag like v1.2.3" >&2
    usage
fi

root=$(git rev-parse --show-toplevel)
cd "$root"

if [ -n "$(git status --porcelain)" ]; then
    echo "sync-rtk: the working tree is not clean; commit or set aside your changes first" >&2
    exit 1
fi

branch="rtk-sync/$tag"
if git show-ref --verify --quiet "refs/heads/$branch"; then
    echo "sync-rtk: branch $branch already exists; review or delete it first" >&2
    exit 1
fi

base=$(git rev-parse --short HEAD)
start=$(git symbolic-ref --short -q HEAD || echo "$base")
git switch -c "$branch"
echo "sync-rtk: on $branch (from $base)"

message="feat: merge upstream rtk $tag

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"

resolved=0
if ! git subtree pull --prefix="$PREFIX" "$UPSTREAM" "$tag" -m "$message"; then
    conflicts=$(git diff --name-only --diff-filter=U)
    # Upstream edited files the trim deletes ("deleted by us"): delete them again.
    trimmed=
    if [ -n "$conflicts" ]; then
        trimmed=$(git status --porcelain | python3 scripts/rtk_trim.py --trimmed-conflicts)
    fi
    if [ -n "$trimmed" ]; then
        while IFS= read -r path; do
            git rm -q -- "$path"
            resolved=$((resolved + 1))
        done <<EOF
$trimmed
EOF
        echo "sync-rtk: resolved $resolved deleted-by-us conflicts by deletion (the trim removes them)"
        conflicts=$(git diff --name-only --diff-filter=U)
    fi
    if [ -n "$trimmed" ] && [ -z "$conflicts" ]; then
        # strip: git's commented "# Conflicts:" list stays out of the merge message.
        git commit -q --no-edit --cleanup=strip
    else
        echo
        if [ -n "$conflicts" ]; then
            echo "sync-rtk: the merge of $tag stopped on conflicts in:"
            printf '%s\n' "$conflicts" | sed 's/^/  /'
            echo "Resolve them on $branch (keep upstream's design, re-apply the fork's intent),"
            echo "then commit the merge and re-run the tests. Nothing was pushed or merged."
        else
            echo "sync-rtk: git subtree pull of $tag failed (see above); nothing was merged."
            echo "To retry, go back and delete the empty branch:"
            echo "  git switch $start && git branch -D $branch"
        fi
        exit 1
    fi
fi
merge=$(git rev-parse --short HEAD)

# The pin: the commit the tag names (an annotated tag is peeled to its commit).
git fetch --no-tags --quiet "$UPSTREAM" "refs/tags/$tag"
upstream_commit=$(git rev-parse "FETCH_HEAD^{commit}")
old_pin=$(python3 - "$tag" "$upstream_commit" <<'PY'
import json
import sys
from pathlib import Path

path = Path("vendor/sources.json")
data = json.loads(path.read_text(encoding="utf-8"))
entry = next(e for e in data["sources"] if e.get("name") == "rtk")
upstream = entry["upstream"]
print(f"{upstream['ref']} {upstream['commit']}")
upstream["ref"], upstream["commit"] = sys.argv[1], sys.argv[2]
# The same layout as vendor.py's save_sources.
path.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
PY
)
if [ "$old_pin" = "$tag $upstream_commit" ]; then
    pin="unchanged ($tag $upstream_commit)"
else
    pin="$old_pin -> $tag $upstream_commit (uncommitted)"
fi

status=0
if [ ! -f scripts/rtk-trim.sh ]; then
    echo "sync-rtk: scripts/rtk-trim.sh is missing; the trim is required" >&2
    trim="FAILED (scripts/rtk-trim.sh is missing)"
    status=1
else
    echo "sync-rtk: running scripts/rtk-trim.sh"
    if bash scripts/rtk-trim.sh; then
        trim="ran scripts/rtk-trim.sh"
    else
        trim="FAILED (scripts/rtk-trim.sh, see above)"
        status=1
    fi
fi

if [ "$status" -ne 0 ]; then
    tests="not run (the trim failed)"
else
    echo "sync-rtk: running the rtk tests (scripts/rtk-test.sh)"
    if "$root/scripts/rtk-test.sh"; then
        tests=PASS
    else
        tests=FAIL
        status=1
    fi
fi

echo
echo "sync-rtk summary"
echo "  upstream: $UPSTREAM $tag"
echo "  branch:   $branch (from $base), merge commit $merge"
if [ "$resolved" -gt 0 ]; then
    echo "  conflicts: $resolved deleted-by-us, resolved by deletion (no other conflicts)"
else
    echo "  conflicts: none"
fi
echo "  pin:      $pin"
echo "  trim:     $trim"
echo "  tests:    $tests"
changed=$(git status --porcelain -- "$PREFIX" vendor/sources.json)
if [ -n "$changed" ]; then
    echo "  uncommitted changes (the pin, the trim): review and commit them on $branch"
    printf '%s\n' "$changed" | sed 's/^/    /'
fi
echo "Nothing was pushed or merged. Review $branch, then merge it yourself."
exit "$status"
