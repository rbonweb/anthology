#!/usr/bin/env bash
# Release one plugin: bump its version, move its CHANGELOG entries, commit, tag.
#
# Usage: scripts/release.sh <plugin> <version> [--push] [--remote NAME] [--dry-run]
#
#   <plugin>      a directory under plugins/ (anthology, anthology-guardrails, ...)
#   <version>     X.Y.Z; must be higher than the current version, or equal to it when the
#                 tag <plugin>--v<version> does not exist yet (the first release)
#   --push        also push the branch and the tag in one atomic push (to --remote, default origin)
#   --dry-run     do everything in a throwaway detached worktree, show the commit and the
#                 CHANGELOG diff, then discard it; the checkout, index and tags are untouched
#
# Nothing is ever pushed without --push. The `claude` binary is $CLAUDE_BIN if set.
# $RELEASE_DATE (YYYY-MM-DD) overrides today's date in the new CHANGELOG heading.
# A plugin whose plugin.json has engines.claude-code (">=X.Y.Z") is released only if that `claude`
# is at least that version, checked first, also with --dry-run; otherwise set CLAUDE_BIN.
# The text and JSON edits are `anthology-dev release-edit …`, run through scripts/dev.sh (the
# pinned Rust image, so Docker is needed) or through $ANTHOLOGY_DEV if set. A plugin that ships
# anthology.lock is released only if `anthology-dev lock check-fresh` passes: the binary it pins
# must have been built from the shipped crates as they are now. Works with bash 3.2 (macOS).
#
# The tree must be clean, except for untracked files under .claude/agent-memory/ and
# .claude/memory/ (agent notes are written all day; they are not part of a release).
# The release commit is tagged first (`claude plugin tag`, no push); only then are the branch and
# the tag pushed, together, with `git push --atomic`. If that push fails, the local tag is deleted
# again and the command to finish by hand is printed.
# The CHANGELOG entries are the plugin's `[Unreleased]` section plus the files
# changelog.d/<plugin>/*.md, in filename order; the release commit deletes those files.
# The last line on standard output is always `RELEASE OK <tag>` or `RELEASE FAILED: <step>`
# (arguments, preconditions, edit, commit, tag, push); a dry run ends `RELEASE OK <tag> (dry run: ...)`.
set -euo pipefail

die() { echo "release: $*" >&2; exit 1; }

usage() {
  sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit "${1:-2}"
}

ROOT=$(cd "$(dirname "$0")/.." && pwd)
STEP=arguments
DRY_RUN=0
WORK=$ROOT
TMPDIR_DRY=""
EDITED=""
cleanup() {
  status=$?
  trap - EXIT
  if [ -n "$EDITED" ] && [ "$DRY_RUN" != 1 ]; then
    # Files were rewritten but the release commit was never made: put them back.
    # shellcheck disable=SC2086
    (cd "$WORK" && git reset -q HEAD -- $EDITED && git checkout -q -- $EDITED) || true
    echo "release: failed before committing; restored $EDITED" >&2
  fi
  if [ -n "$TMPDIR_DRY" ]; then
    git -C "$ROOT" worktree remove --force "$TMPDIR_DRY/wt" >/dev/null 2>&1 || true
    git -C "$ROOT" worktree prune >/dev/null 2>&1 || true
    rm -rf "$TMPDIR_DRY"
  fi
  # Whatever failed, the last line says which step it was (--help is not a failure).
  if [ "$status" != 0 ]; then echo "RELEASE FAILED: $STEP"; fi
  exit "$status"
}
trap cleanup EXIT

# A sortable key for X.Y.Z (any suffix after a - or + is ignored): compare keys as text.
version_key() { printf '%s\n' "${1%%[-+]*}" | awk -F. '{ printf "%09d%09d%09d\n", $1, $2, $3 }'; }
# The first X.Y.Z in a string ("2.1.330 (Claude Code)", ">=2.1.330"); empty when there is none.
semver_of() { printf '%s\n' "$1" | sed -n 's/^[^0-9]*\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*$/\1/p' | head -n 1; }

DEV=${ANTHOLOGY_DEV:-$ROOT/scripts/dev.sh}
CLAUDE=${CLAUDE_BIN:-claude}

PLUGIN=""
VERSION=""
PUSH=0
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

# anthology-dev runs at the repository root and is given paths relative to it: scripts/dev.sh
# mounts only the repository into its container.
dev() { (cd "$ROOT" && "$DEV" "$@"); }

# ---- preconditions: every failure leaves the repository exactly as it was -----------

STEP=preconditions
case "$PLUGIN" in
  *[!a-z0-9-]*|-*) die "invalid plugin name '$PLUGIN' (lowercase letters, digits, hyphens)" ;;
esac
MANIFEST_REL="plugins/$PLUGIN/.claude-plugin/plugin.json"
LOCK_REL="plugins/$PLUGIN/anthology.lock"
TAG="$PLUGIN--v$VERSION"

cd "$ROOT"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "$ROOT is not a git work tree"
[ -f "$MANIFEST_REL" ] || die "no such plugin: $MANIFEST_REL does not exist"
if [ ! -x "$DEV" ] && ! command -v "$DEV" >/dev/null 2>&1; then
  die "'$DEV' not found (scripts/dev.sh needs Docker; or set ANTHOLOGY_DEV to an anthology-dev binary)"
fi
command -v "$CLAUDE" >/dev/null 2>&1 || die "'$CLAUDE' not found (install Claude Code, or set CLAUDE_BIN)"
# The claude that will tag the release must be new enough for the plugin (engines.claude-code in
# its manifest); an older one fails `claude plugin tag` validation after the release commit and
# the push, leaving a half release. A plugin without the field is not checked.
ENGINES=$(sed -n 's/.*"claude-code"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$MANIFEST_REL" | head -n 1)
if [ -n "$ENGINES" ]; then
  case "$ENGINES" in
    ">="*) ;;
    *) die "$MANIFEST_REL: engines.claude-code '$ENGINES' is not of the form '>=X.Y.Z'" ;;
  esac
  ENGINES_MIN=$(semver_of "${ENGINES#>=}")
  [ -n "$ENGINES_MIN" ] || die "$MANIFEST_REL: cannot read a version from engines.claude-code '$ENGINES'"
  CLAUDE_OUT=$("$CLAUDE" --version 2>&1 | head -n 1 || true)
  CLAUDE_VERSION=$(semver_of "$CLAUDE_OUT")
  [ -n "$CLAUDE_VERSION" ] || die "cannot read the version of '$CLAUDE' ('$CLAUDE --version' printed '$CLAUDE_OUT'); set CLAUDE_BIN to a claude of at least $ENGINES_MIN"
  if [[ "$(version_key "$CLAUDE_VERSION")" < "$(version_key "$ENGINES_MIN")" ]]; then
    die "plugin $PLUGIN needs claude-code $ENGINES, but '$CLAUDE' is $CLAUDE_VERSION; set CLAUDE_BIN to a claude of at least $ENGINES_MIN (nothing was changed)"
  fi
fi
if [ "$PUSH" = 1 ]; then
  git remote get-url "$REMOTE" >/dev/null 2>&1 || die "no git remote named '$REMOTE'"
  git symbolic-ref -q HEAD >/dev/null 2>&1 || die "--push needs a checked-out branch (HEAD is detached)"
fi

# Untracked agent notes (.claude/agent-memory/, .claude/memory/) are the one thing let through;
# a changed tracked file there, or any other untracked file, still blocks.
DIRTY=$(git status --porcelain --untracked-files=all | grep -v -E '^\?\? "?\.claude/(agent-memory|memory)/' || true)
if [ -n "$DIRTY" ]; then
  printf '%s\n' "$DIRTY" >&2
  die "working tree is not clean; commit or stash your changes first"
fi

if [ -f "$LOCK_REL" ]; then
  # The binary that anthology.lock pins must not be older than the workspace: a plugin must not
  # ship with a guard binary that predates the crates it was tested with (for example one that
  # cannot look inside `atl` commands).
  LOCK_BIN=$(sed -n 's/^  "version": "\([^"]*\)",*$/\1/p' "$LOCK_REL" | head -n 1)
  WS_VERSION=$(awk '/^\[/ { p = ($0 == "[workspace.package]") } p && $1 == "version" { gsub(/"/, "", $3); print $3; exit }' Cargo.toml)
  [ -n "$LOCK_BIN" ] || die "cannot read the anthology-bin version from $LOCK_REL"
  [ -n "$WS_VERSION" ] || die "cannot read the workspace version from Cargo.toml"
  if [[ "$(version_key "$LOCK_BIN")" < "$(version_key "$WS_VERSION")" ]]; then
    die "$LOCK_REL pins anthology-bin $LOCK_BIN, older than the workspace version $WS_VERSION: release anthology-bin--v$WS_VERSION and run 'scripts/dev.sh lock write' first"
  fi
  # The binary that anthology.lock pins must match the shipped crates at HEAD (design §8.5).
  dev lock check-fresh >&2 || die "the anthology binary is stale: release a new one (tag anthology-bin--v<next>) and run 'scripts/dev.sh lock write', or see the lines above"
fi

CURRENT=$(dev release-edit plugin-version "$MANIFEST_REL")
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  die "tag $TAG already exists"
fi
MODE=$(dev release-edit check-version "$CURRENT" "$VERSION" 0)
dev release-edit changelog-check CHANGELOG.md "$PLUGIN"
dev release-edit check-dependents . "$PLUGIN" "$VERSION"

# ---- work area: this checkout, or (dry run) a throwaway detached worktree -----------

STEP=edit
WORK_REL=.
GIT_COMMIT_OPTS=""
if [ "$DRY_RUN" = 1 ]; then
  # Inside the repository (target/ is ignored), so the scripts/dev.sh container sees it.
  mkdir -p "$ROOT/target"
  TMPDIR_DRY=$(mktemp -d "$ROOT/target/release-dry-run.XXXXXX")
  git worktree add -q --detach "$TMPDIR_DRY/wt" HEAD
  WORK="$TMPDIR_DRY/wt"
  WORK_REL=${WORK#"$ROOT"/}
  # The throwaway commit must not sign, run hooks or need a configured identity; the
  # placeholder identity is passed per command and never written to any config.
  GIT_COMMIT_OPTS="-c commit.gpgsign=false"
  if [ -z "$(git config user.name 2>/dev/null || true)" ]; then
    GIT_COMMIT_OPTS="$GIT_COMMIT_OPTS -c user.name=release-dry-run"
  fi
  if [ -z "$(git config user.email 2>/dev/null || true)" ]; then
    GIT_COMMIT_OPTS="$GIT_COMMIT_OPTS -c user.email=noreply@example.invalid"
  fi
fi

# ---- edits ---------------------------------------------------------------------------

FILES="CHANGELOG.md"
# changelog-move compiles changelog.d/<plugin>/*.md into the new section and deletes those files;
# the release commit takes the deletions with it.
if [ -n "$(git -C "$WORK" ls-files -- "changelog.d/$PLUGIN")" ]; then
  FILES="$FILES changelog.d/$PLUGIN"
fi
EDITED="$FILES"
dev release-edit changelog-move "$WORK_REL/CHANGELOG.md" "$PLUGIN" "$VERSION" \
  ${RELEASE_DATE:+--date "$RELEASE_DATE"}

if [ "$MODE" = bump ]; then
  dev release-edit set-version "$WORK_REL/$MANIFEST_REL" "$VERSION"
  FILES="$FILES $MANIFEST_REL"
  EDITED="$FILES"
  if [ "$PLUGIN" = anthology-superpowers ]; then
    # Generated plugin: vendor sync takes its version from sources.json, so keep them in step.
    dev release-edit set-source-version "$WORK_REL/vendor/sources.json" \
      "plugins/anthology-superpowers" "$VERSION"
    FILES="$FILES vendor/sources.json"
    EDITED="$FILES"
  fi
fi

# ---- commit ----------------------------------------------------------------------------

STEP=commit
NO_VERIFY=""
if [ "$DRY_RUN" = 1 ]; then NO_VERIFY="--no-verify"; fi

# shellcheck disable=SC2086
git -C "$WORK" add -- $FILES
# shellcheck disable=SC2086
git -C "$WORK" $GIT_COMMIT_OPTS commit -q $NO_VERIFY -m "release: $PLUGIN $VERSION" -- $FILES
EDITED=""

# ---- tag, then push --------------------------------------------------------------------

STEP=tag
TAG_MSG="$PLUGIN %s"
TAG_CMD="$CLAUDE plugin tag plugins/$PLUGIN -m '$TAG_MSG'"
PUSH_CMD="git push --atomic $REMOTE HEAD refs/tags/$TAG"
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
  echo
  echo "RELEASE OK $TAG (dry run: nothing was released)"
  exit 0
fi

# The tag is made locally first, so a failing `claude plugin tag` leaves the remote untouched.
"$CLAUDE" plugin tag "plugins/$PLUGIN" -m "$TAG_MSG" \
  || die "the release commit exists locally but tagging failed; nothing was pushed. Fix the cause and run: $TAG_CMD  (or undo the release commit with: git reset --hard HEAD~1)"
git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1 \
  || die "'$CLAUDE plugin tag' succeeded but tag $TAG does not exist; nothing was pushed. Look at the tags with: git tag -l '$PLUGIN--v*'"

if [ "$PUSH" = 1 ]; then
  STEP=push
  # One transaction: the branch and the tag both arrive, or neither does.
  if ! git push --atomic "$REMOTE" HEAD "refs/tags/$TAG"; then
    # The tag names a commit that a `git pull --rebase` may replace: take it away again.
    git tag -d "$TAG" >/dev/null 2>&1 || true
    die "the release commit exists locally but the push failed; nothing was pushed and the local tag $TAG was deleted. Fix the cause (e.g. git pull --rebase $REMOTE), then run: $TAG_CMD && $PUSH_CMD   -- or undo the release commit with: git reset --hard HEAD~1"
  fi
  echo "released and pushed $TAG to $REMOTE"
else
  echo
  echo "released $TAG locally; nothing was pushed. To publish, in one atomic push:"
  echo "  $PUSH_CMD"
fi
echo "RELEASE OK $TAG"
