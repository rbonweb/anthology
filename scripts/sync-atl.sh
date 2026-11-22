#!/usr/bin/env bash
# Merge an upstream release (https://github.com/rtk-ai/rtk) into atl/ on a review branch.
#
#   scripts/sync-atl.sh v1.2.3
#
# The upstream tree is not named ATL, so a plain merge would conflict on every file. This sync
# merges over renamed bases instead: it builds the pinned upstream release and the new one,
# runs scripts/atl-rename.sh over both (the same rule that renamed atl/), and merges the
# renamed new release into the branch with the renamed pinned release as the merge base. Git then
# sees only what upstream changed between the two releases.
#
# 1. refuses to run on a dirty tree;
# 2. creates branch atl-sync/<tag> from HEAD;
# 3. records the new tag and its commit as the upstream pin in vendor/sources.json
#    (`scripts/dev.sh vendor set-atl-upstream`, which prints the old pin as `<ref> <commit>`);
# 4. builds the renamed old and new releases as commits under atl/ (they hold nothing else),
#    records the old one as an ancestor (`git merge -s ours`: the tree does not change), and
#    merges the new one (upstream edits to files the trim deletes are modify/delete conflicts,
#    "deleted by us": they are resolved by deletion, as the trim would delete them again; on any
#    other conflict it prints the conflicted files and stops, leaving the branch for review);
# 5. runs scripts/atl-trim.sh (required: the sync fails without it);
# 6. runs $ATL_TEST (scripts/atl-test.sh: the pinned image from ci/images/rust.Dockerfile);
# 7. prints a summary.
#
# The pin and the trim's changes are left uncommitted on the branch, for review with it.
# It never pushes and never merges into another branch: the branch is left for review.
# The tools run as `scripts/dev.sh <subcommand>` (anthology-dev in the pinned Rust image; F-17).
# ATL_SYNC_UPSTREAM replaces the upstream URL (a test seam: a local repository).
set -eu

UPSTREAM=${ATL_SYNC_UPSTREAM:-https://github.com/rtk-ai/rtk}
PREFIX=atl

usage() {
    echo "usage: scripts/sync-atl.sh <tag>   (an upstream release tag such as v1.2.3)" >&2
    exit 2
}

[ "$#" -eq 1 ] || usage
tag=$1
tag_pattern='^v[0-9]+\.[0-9]+\.[0-9]+$'
if ! [[ $tag =~ $tag_pattern ]]; then
    echo "sync-atl: '$tag' is not a release tag like v1.2.3" >&2
    usage
fi

root=$(git rev-parse --show-toplevel)
cd "$root"

if [ -n "$(git status --porcelain)" ]; then
    echo "sync-atl: the working tree is not clean; commit or set aside your changes first" >&2
    exit 1
fi

branch="atl-sync/$tag"
if git show-ref --verify --quiet "refs/heads/$branch"; then
    echo "sync-atl: branch $branch already exists; review or delete it first" >&2
    exit 1
fi

base=$(git rev-parse --short HEAD)
start=$(git symbolic-ref --short -q HEAD || echo "$base")
git switch -c "$branch"
echo "sync-atl: on $branch (from $base)"

# The new release and the pin it replaces.
git fetch --no-tags --quiet "$UPSTREAM" "refs/tags/$tag"
upstream_commit=$(git rev-parse "FETCH_HEAD^{commit}")
old_pin=$(scripts/dev.sh vendor set-atl-upstream --ref "$tag" --commit "$upstream_commit")
old_ref=${old_pin% *}
old_commit=${old_pin#* }
git fetch --no-tags --quiet "$UPSTREAM" "refs/tags/$old_ref"
if [ "$(git rev-parse "FETCH_HEAD^{commit}")" != "$old_commit" ]; then
    echo "sync-atl: the pinned tag $old_ref no longer names $old_commit upstream" >&2
    exit 1
fi

gitdir=$(git rev-parse --absolute-git-dir)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/atl-sync.XXXXXX")
trap 'rm -rf "$scratch"' EXIT

# renamed_commit <upstream commit> <message> [parent]: a commit whose tree is the upstream
# release, under atl/ and renamed by scripts/atl-rename.sh. Prints its id.
renamed_commit() {
    local commit=$1 message=$2 parent=${3:-} dir index tree
    dir=$(mktemp -d "$scratch/tree.XXXXXX")
    index="$dir.index"
    GIT_INDEX_FILE=$index git read-tree --prefix="$PREFIX/" "$commit"
    GIT_INDEX_FILE=$index git checkout-index -a -f --prefix="$dir/"
    rm -f "$index"
    scripts/atl-rename.sh --root "$dir" >/dev/null
    GIT_INDEX_FILE=$index git --git-dir="$gitdir" --work-tree="$dir" -C "$dir" add -A -f
    tree=$(GIT_INDEX_FILE=$index git --git-dir="$gitdir" write-tree)
    rm -f "$index"
    if [ -n "$parent" ]; then
        git commit-tree "$tree" -p "$parent" -m "$message"
    else
        git commit-tree "$tree" -m "$message"
    fi
}

renamed_old=$(renamed_commit "$old_commit" "upstream $old_ref, renamed to ATL (merge base)")
renamed_new=$(renamed_commit "$upstream_commit" "upstream $tag, renamed to ATL" "$renamed_old")

# The old release becomes an ancestor without changing the tree; then the real merge has it as
# its base.
git merge -q -s ours --allow-unrelated-histories -m "chore: record upstream $old_ref (renamed) as the merge base" "$renamed_old"
merge_message="feat: merge upstream $tag into atl/ (renamed to ATL)"

resolved=0
if ! git merge --no-edit -m "$merge_message" "$renamed_new"; then
    conflicts=$(git diff --name-only --diff-filter=U)
    # Upstream edited files the trim deletes ("deleted by us"): delete them again.
    trimmed=
    if [ -n "$conflicts" ]; then
        trimmed=$(git status --porcelain | scripts/dev.sh atl-trim --trimmed-conflicts)
    fi
    if [ -n "$trimmed" ]; then
        while IFS= read -r path; do
            git rm -q -- "$path"
            resolved=$((resolved + 1))
        done <<EOF
$trimmed
EOF
        echo "sync-atl: resolved $resolved deleted-by-us conflicts by deletion (the trim removes them)"
        conflicts=$(git diff --name-only --diff-filter=U)
    fi
    if [ -n "$trimmed" ] && [ -z "$conflicts" ]; then
        # strip: git's commented "# Conflicts:" list stays out of the merge message.
        git commit -q --no-edit --cleanup=strip
    else
        echo
        if [ -n "$conflicts" ]; then
            echo "sync-atl: the merge of $tag stopped on conflicts in:"
            printf '%s\n' "$conflicts" | sed 's/^/  /'
            echo "Resolve them on $branch (keep upstream's design, re-apply the fork's intent),"
            echo "then commit the merge and re-run the tests. Nothing was pushed or merged."
        else
            echo "sync-atl: the merge of $tag failed (see above); nothing was merged."
            echo "To retry, go back and delete the branch:"
            echo "  git switch $start && git branch -D $branch"
        fi
        exit 1
    fi
fi
merge=$(git rev-parse --short HEAD)

if [ "$old_pin" = "$tag $upstream_commit" ]; then
    pin="unchanged ($tag $upstream_commit)"
else
    pin="$old_pin -> $tag $upstream_commit (uncommitted)"
fi

status=0
if [ ! -f scripts/atl-trim.sh ]; then
    echo "sync-atl: scripts/atl-trim.sh is missing; the trim is required" >&2
    trim="FAILED (scripts/atl-trim.sh is missing)"
    status=1
else
    echo "sync-atl: running scripts/atl-trim.sh"
    if bash scripts/atl-trim.sh; then
        trim="ran scripts/atl-trim.sh"
    else
        trim="FAILED (scripts/atl-trim.sh, see above)"
        status=1
    fi
fi

if [ "$status" -ne 0 ]; then
    tests="not run (the trim failed)"
else
    echo "sync-atl: running the atl tests (scripts/atl-test.sh)"
    if "$root/scripts/atl-test.sh"; then
        tests=PASS
    else
        tests=FAIL
        status=1
    fi
fi

echo
echo "sync-atl summary"
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
echo "Nothing was pushed or merged into another branch. Review $branch, then merge it yourself."
exit "$status"
