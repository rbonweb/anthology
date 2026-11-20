#!/usr/bin/env bash
# Merge an upstream rtk release into rtk/ on a review branch.
#
#   scripts/sync-rtk.sh v1.2.3
#
# 1. refuses to run on a dirty tree;
# 2. creates branch rtk-sync/<tag> from HEAD;
# 3. git subtree pull --prefix=rtk https://github.com/rtk-ai/rtk <tag>
#    (on conflicts: prints the conflicted files and stops, leaving the branch for review);
# 4. runs scripts/rtk-trim.sh when it exists;
# 5. runs $RTK_TEST in the pinned Rust image (the FROM line of ci/images/rust.Dockerfile);
# 6. prints a summary.
#
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
git switch -c "$branch"
echo "sync-rtk: on $branch (from $base)"

message="feat: merge upstream rtk $tag

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"

if ! git subtree pull --prefix="$PREFIX" "$UPSTREAM" "$tag" -m "$message"; then
    conflicts=$(git diff --name-only --diff-filter=U)
    echo
    if [ -n "$conflicts" ]; then
        echo "sync-rtk: the merge of $tag stopped on conflicts in:"
        printf '%s\n' "$conflicts" | sed 's/^/  /'
    else
        echo "sync-rtk: git subtree pull of $tag failed (see above)"
    fi
    echo "Resolve them on $branch (keep upstream's design, re-apply the fork's intent),"
    echo "then commit the merge and re-run the tests. Nothing was pushed or merged."
    exit 1
fi
merge=$(git rev-parse --short HEAD)

if [ -f scripts/rtk-trim.sh ]; then
    echo "sync-rtk: running scripts/rtk-trim.sh"
    bash scripts/rtk-trim.sh
    trim="ran scripts/rtk-trim.sh"
else
    echo "sync-rtk: scripts/rtk-trim.sh does not exist yet; trim skipped"
    trim="skipped (scripts/rtk-trim.sh does not exist)"
fi

image=$(awk '$1 == "FROM" {print $2}' ci/images/rust.Dockerfile)
echo "sync-rtk: running the rtk tests in $image"
if docker run --rm -v "$root/$PREFIX":/w -v anthology-cargo:/usr/local/cargo/registry -w /w \
    "$image" cargo test --locked; then
    tests=PASS
else
    tests=FAIL
fi

echo
echo "sync-rtk summary"
echo "  upstream: $UPSTREAM $tag"
echo "  branch:   $branch (from $base), merge commit $merge"
echo "  conflicts: none"
echo "  trim:     $trim"
echo "  tests:    $tests"
changed=$(git status --porcelain -- "$PREFIX")
if [ -n "$changed" ]; then
    echo "  uncommitted changes under $PREFIX/ (from the trim): review and commit them on $branch"
fi
echo "Nothing was pushed or merged. Review $branch, then merge it yourself."
[ "$tests" = PASS ]
