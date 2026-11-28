#!/bin/sh
# commit-privacy.sh: the anthology PreToolUse hook for Bash that checks every `git commit` for
# the owner's private details. A command that mentions both `git` and `commit` has its hook input
# (stdin) handed to `anthology privacy hook`, which refuses a commit whose staged changes or
# message hold the owner's home path, handles, git email, machine or tailnet name or a banned name.
# Only that binary's exit 10 blocks: this script then prints the binary's stderr (the message that
# names the kind and the place, never the value) and exits 2. Every other outcome exits 0 with no
# output, so the check fails open: a missing, old, broken or crashing binary, a lib.sh that does
# not load and a missing lock never stop a command. It never installs the binary (session start
# does) and writes nothing to stdout.
#
# A command that is no commit costs no process: the input is read with the shell's own `read`
# and looked at with `case` before anything is loaded or started. The environment variable
# ANTHOLOGY_COMMIT_PRIVACY set to exactly 0 turns the check off: the script exits 0 first.
trap 'exit 0' HUP INT TERM
# ANTHOLOGY_COMMIT_PRIVACY=0 (exactly 0) switches the check off: exit 0 before anything else runs.
if [ "${ANTHOLOGY_COMMIT_PRIVACY-}" = 0 ]; then
    exit 0
fi
anth_eol='
'
anth_in=
while IFS= read -r anth_line || [ -n "$anth_line" ]; do
    anth_in=$anth_in$anth_line$anth_eol
done
# The input is JSON: `git` and `commit` in this order is all it takes to look closer.
case $anth_in in
    *git*commit*) ;;
    *) exit 0 ;;
esac
case $0 in
    */*) anth_hooks=${0%/*} ;;
    *) anth_hooks=. ;;
esac
if [ ! -f "$anth_hooks/lib.sh" ] || ! (command . "$anth_hooks/lib.sh") >/dev/null 2>&1; then
    exit 0
fi
# shellcheck source=lib.sh
command . "$anth_hooks/lib.sh"
anth_paths "$anth_hooks/.."
if ! anth_resolve "$anth_hooks/.." || ! anth_installed; then
    exit 0
fi
# The binary reads stdin; its stdout is dropped and its stderr is kept, with its exit code
# after a sentinel.
anth_err=$(printf '%s' "$anth_in" | "$ANTH_BIN" privacy hook 2>&1 >/dev/null; printf 'x%s' "$?")
anth_rc=${anth_err##*x}
anth_err=${anth_err%x*}
# Digits only: a late writer after the sentinel must not decide the outcome.
anth_only "$anth_rc" 0123456789 || exit 0
[ "$anth_rc" = 10 ] || exit 0
while :; do
    case $anth_err in
        *"$ANTH_NL") anth_err=${anth_err%"$ANTH_NL"} ;;
        *) break ;;
    esac
done
printf '%s\n' "$anth_err" >&2
exit 2
