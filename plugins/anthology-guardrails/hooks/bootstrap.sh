#!/bin/sh
# bootstrap.sh: install the anthology binary this plugin's anthology.lock pins (design §8.4).
#
#   bootstrap.sh <session|guard|wrapper> --plugin <anthology|guardrails> [--quiet] [--budget SECONDS]
#
# With --plugin guardrails, guard and session modes take the archive from a background download
# (`bootstrap.sh fetch --plugin guardrails`, started detached) and wait for it up to the budget.
#
# --quiet prints one line, status=present|installed|failed:<reason>, and exits 0 or 1. Without
# it, session mode prints nothing on success and the plugin's systemMessage on failure, and
# exits 0. The logic is in lib.sh, next to this file; this is the command-line entry point.
case $0 in
    */*) anth_hooks=${0%/*} ;;
    *) anth_hooks=. ;;
esac
if [ ! -f "$anth_hooks/lib.sh" ] || ! (command . "$anth_hooks/lib.sh") >/dev/null 2>&1; then
    printf '%s\n' 'bootstrap.sh: cannot load hooks/lib.sh' >&2
    exit 1
fi
# shellcheck source=lib.sh
command . "$anth_hooks/lib.sh" || exit 1
anth_paths "$anth_hooks/.."
# The installer command the guardrails' session message names.
anth_fix_cmd "$anth_hooks"
# The staging folder goes on every exit, including a hook timeout's signal.
trap '[ -z "$ANTH_TMP" ] || rm -rf "$ANTH_TMP"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
anth_bootstrap_main "$@"
exit "$?"
