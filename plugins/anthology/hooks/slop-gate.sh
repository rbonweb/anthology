#!/bin/sh
# slop-gate.sh: the anthology PreToolUse hook for Write, Edit and MultiEdit. It hands the hook
# input (stdin) to `anthology slop hook`, which refuses an edit whose new text holds a known
# AI-writing or AI-design tell. Only that binary's exit 10 blocks: this script then prints the
# binary's stderr (the message that names the line and the phrase) and exits 2. Every other
# outcome exits 0 with no output, so the gate fails open: a missing, old, broken or crashing
# binary, a lib.sh that does not load and a missing lock never stop an edit. It never installs
# the binary (session start does) and writes nothing to stdout. The environment variable
# ANTHOLOGY_SLOP_GATE set to exactly 0 turns the gate off: the script exits 0 without running
# the binary.
trap 'exit 0' HUP INT TERM
# ANTHOLOGY_SLOP_GATE=0 (exactly 0) switches the gate off: exit 0 before anything else runs.
if [ "${ANTHOLOGY_SLOP_GATE-}" = 0 ]; then
    exit 0
fi
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
anth_err=$("$ANTH_BIN" slop hook 2>&1 >/dev/null; printf 'x%s' "$?")
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
