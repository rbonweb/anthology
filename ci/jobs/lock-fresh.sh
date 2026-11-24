#!/bin/sh
# CI job `lock-fresh` (informational): does this PR change what the pinned anthology binary was
# built from? Runs `lock check-fresh`, which compares the shipped set between the tag the
# plugins' anthology.lock pins and HEAD. A stale result is a warning and a job summary, never a
# failure: the job has no write token, comments nothing on the PR, and always exits 0. The
# release (scripts/release.sh) is where a stale lock is a hard stop.
#
#   ANTHOLOGY_DEV_SH   replaces scripts/dev.sh (a test seam)
#   GITHUB_STEP_SUMMARY  where the summary goes (set by GitHub Actions)
set -u

root=$(cd "$(dirname "$0")/../.." && pwd)
dev=${ANTHOLOGY_DEV_SH:-$root/scripts/dev.sh}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
out=$(mktemp)
trap 'rm -f "$out"' EXIT

status=0
"$dev" lock check-fresh >"$out" 2>&1 || status=$?

# The tool's lines are printed indented: none can start a workflow command (`::warning` and so
# on), whatever a path or crate name in them says.
indented=$(tail -n 40 "$out" | sed 's/^/    /')

case $status in
    0)
        echo "lock-fresh: the shipped set matches the lock"
        printf '### lock-fresh\n\nThe shipped set matches the lock.\n' >>"$summary"
        ;;
    1)
        message="this PR needs a new anthology-bin release before the next plugin release"
        echo "::warning title=lock-fresh::$message"
        printf '%s\n' "$indented"
        printf '### lock-fresh\n\nWarning: %s.\n\n%s\n' "$message" "$indented" >>"$summary"
        ;;
    *)
        message="lock-fresh could not run the check (exit $status); this job is informational"
        echo "::warning title=lock-fresh::$message"
        printf '%s\n' "$indented"
        printf '### lock-fresh\n\n%s.\n\n%s\n' "$message" "$indented" >>"$summary"
        ;;
esac
exit 0
