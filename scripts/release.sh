#!/usr/bin/env bash
# Release one plugin: bump its version, move its CHANGELOG entries, commit, tag.
#
# Usage: scripts/release.sh <plugin> <version> [--push] [--remote NAME] [--dry-run]
#
#   <plugin>      a directory under plugins/ (anthology, anthology-guardrails, ...)
#   <version>     X.Y.Z; must be higher than the current version, or equal to it when the
#                 tag <plugin>--v<version> does not exist yet (the first release)
#   --push        also push the branch and the tag (to --remote, default origin)
#   --dry-run     do everything in a throwaway detached worktree, show the commit and the
#                 CHANGELOG diff, then discard it; the checkout, index and tags are untouched
#
# Nothing is ever pushed without --push. The `claude` binary is $CLAUDE_BIN if set.
# The text and JSON edits live in scripts/release_edit.py. Works with bash 3.2 (macOS).
set -euo pipefail

die() { echo "release: $*" >&2; exit 1; }

usage() {
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit "${1:-2}"
}

ROOT=$(cd "$(dirname "$0")/.." && pwd)
EDIT="$ROOT/scripts/release_edit.py"
CLAUDE=${CLAUDE_BIN:-claude}

PLUGIN=""
VERSION=""
PUSH=0
DRY_RUN=0
REMOTE=origin
POSITIONAL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --push) PUSH=1 ;;
    --dry-run) DRY_RUN=1 ;;
    --remote)
      [ $# -ge 2 ] || die "--remote needs a remote name"
      REMOTE=$2
      shift
      ;;
    -h|--help) usage 0 ;;
    -*) echo "release: unknown option $1" >&2; usage ;;
    *)
      POSITIONAL=$((POSITIONAL + 1))
      case "$POSITIONAL" in
        1) PLUGIN=$1 ;;
        2) VERSION=$1 ;;
        *) echo "release: unexpected argument $1" >&2; usage ;;
      esac
      ;;
  esac
  shift
done
[ -n "$PLUGIN" ] && [ -n "$VERSION" ] || usage

if [ "$DRY_RUN" = 1 ] && [ "$PUSH" = 1 ]; then
  die "--dry-run and --push cannot be combined (a dry run never pushes)"
fi

# ---- preconditions: every failure leaves the repository exactly as it was -----------

case "$PLUGIN" in
  *[!a-z0-9-]*|-*) die "invalid plugin name '$PLUGIN' (lowercase letters, digits, hyphens)" ;;
esac
MANIFEST_REL="plugins/$PLUGIN/.claude-plugin/plugin.json"
TAG="$PLUGIN--v$VERSION"

cd "$ROOT"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "$ROOT is not a git work tree"
[ -f "$MANIFEST_REL" ] || die "no such plugin: $MANIFEST_REL does not exist"
command -v python3 >/dev/null 2>&1 || die "python3 is required"
command -v "$CLAUDE" >/dev/null 2>&1 || die "'$CLAUDE' not found (install Claude Code, or set CLAUDE_BIN)"
if [ "$PUSH" = 1 ]; then
  git remote get-url "$REMOTE" >/dev/null 2>&1 || die "no git remote named '$REMOTE'"
fi

if [ -n "$(git status --porcelain)" ]; then
  git status --short >&2
  die "working tree is not clean; commit or stash your changes first"
fi

CURRENT=$(python3 "$EDIT" plugin-version "$MANIFEST_REL")
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  die "tag $TAG already exists"
fi
MODE=$(python3 "$EDIT" check-version "$CURRENT" "$VERSION" 0)
python3 "$EDIT" changelog-check CHANGELOG.md "$PLUGIN"
python3 "$EDIT" check-dependents "$ROOT" "$PLUGIN" "$VERSION"

# ---- work area: this checkout, or (dry run) a throwaway detached worktree -----------

WORK=$ROOT
TMPDIR_DRY=""
EDITED=""
cleanup() {
  status=$?
  if [ -n "$EDITED" ]; then
    # Edits were made but never committed: put the files back.
    # shellcheck disable=SC2086
    (cd "$WORK" && git reset -q HEAD -- $EDITED && git checkout -q -- $EDITED) || true
    echo "release: failed before committing; reverted $EDITED" >&2
  fi
  if [ -n "$TMPDIR_DRY" ]; then
    git -C "$ROOT" worktree remove --force "$TMPDIR_DRY/wt" >/dev/null 2>&1 || true
    git -C "$ROOT" worktree prune >/dev/null 2>&1 || true
    rm -rf "$TMPDIR_DRY"
  fi
  exit "$status"
}
trap cleanup EXIT

GIT_COMMIT_OPTS=""
if [ "$DRY_RUN" = 1 ]; then
  TMPDIR_DRY=$(mktemp -d "${TMPDIR:-/tmp}/anthology-release.XXXXXX")
  git worktree add -q --detach "$TMPDIR_DRY/wt" HEAD
  WORK="$TMPDIR_DRY/wt"
  GIT_COMMIT_OPTS="-c commit.gpgsign=false"
  if [ -z "$(git config user.email 2>/dev/null || true)" ]; then
    export GIT_AUTHOR_NAME="release dry run" GIT_AUTHOR_EMAIL="noreply@example.invalid"
    export GIT_COMMITTER_NAME="release dry run" GIT_COMMITTER_EMAIL="noreply@example.invalid"
  fi
fi

# ---- edits ---------------------------------------------------------------------------

FILES="CHANGELOG.md"
EDITED="CHANGELOG.md"
python3 "$EDIT" changelog-move "$WORK/CHANGELOG.md" "$PLUGIN" "$VERSION" \
  ${RELEASE_DATE:+--date "$RELEASE_DATE"}

if [ "$MODE" = bump ]; then
  python3 "$EDIT" set-version "$WORK/$MANIFEST_REL" "$VERSION"
  FILES="$FILES $MANIFEST_REL"
  EDITED="$FILES"
  if [ "$PLUGIN" = anthology-superpowers ]; then
    # Generated plugin: vendor.py takes its version from sources.json, so keep them in step.
    python3 "$EDIT" set-source-version "$WORK/vendor/sources.json" \
      "plugins/anthology-superpowers" "$VERSION"
    FILES="$FILES vendor/sources.json"
    EDITED="$FILES"
  fi
fi

# ---- commit ----------------------------------------------------------------------------

# shellcheck disable=SC2086
git -C "$WORK" add -- $FILES
# shellcheck disable=SC2086
git -C "$WORK" $GIT_COMMIT_OPTS commit -q -m "release: $PLUGIN $VERSION" -- $FILES
EDITED=""

# ---- tag -------------------------------------------------------------------------------

TAG_MSG="$PLUGIN %s"
if [ "$DRY_RUN" = 1 ]; then
  echo "== dry run: nothing in this checkout was changed =="
  echo
  git -C "$WORK" show --stat --format='commit %h%n    %s' HEAD
  echo
  echo "== CHANGELOG.md diff =="
  git -C "$WORK" diff HEAD~1 HEAD -- CHANGELOG.md
  echo
  echo "== claude plugin tag --dry-run =="
  (cd "$WORK" && "$CLAUDE" plugin tag --dry-run "$WORK/plugins/$PLUGIN" -m "$TAG_MSG")
  exit 0
fi

if [ "$PUSH" = 1 ]; then
  git push "$REMOTE" HEAD
  "$CLAUDE" plugin tag "plugins/$PLUGIN" -m "$TAG_MSG" --push --remote "$REMOTE" \
    || die "the release commit was pushed but tagging failed; fix the cause and run: $CLAUDE plugin tag plugins/$PLUGIN -m '$TAG_MSG' --push --remote $REMOTE"
  echo "released and pushed $TAG to $REMOTE"
else
  "$CLAUDE" plugin tag "plugins/$PLUGIN" -m "$TAG_MSG" \
    || die "the release commit exists but tagging failed; fix the cause and run: $CLAUDE plugin tag plugins/$PLUGIN -m '$TAG_MSG'  (or undo the commit with: git reset --hard HEAD~1)"
  echo
  echo "released $TAG locally; nothing was pushed. To publish:"
  echo "  git push $REMOTE HEAD"
  echo "  git push $REMOTE refs/tags/$TAG"
fi
