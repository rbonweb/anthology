#!/bin/sh
# subagent-stop.sh: the anthology SubagentStop hook. A subagent that ends its turn while its own
# background shell or Monitor is still running floods the lead with "waiting on its own background
# work" notices, so `anthology hook subagent-stop` gives it one more turn, with a reason, when the
# hook input lists such work as running. This script runs the installed binary and prints its
# output (the block line, or nothing).
#
# It fails open and always exits 0 with at most one JSON line: it is a nudge, not a gate. It never
# downloads or installs anything (a stop hook must be quick): a missing library, a missing or
# older binary (one that does not know `hook subagent-stop` exits non-zero), or any failure
# allows the stop and prints nothing.
case $0 in
    */*) anth_hooks=${0%/*} ;;
    *) anth_hooks=. ;;
esac
if [ ! -f "$anth_hooks/lib.sh" ] || ! (command . "$anth_hooks/lib.sh") >/dev/null 2>&1; then
    exit 0
fi
# shellcheck source=lib.sh
command . "$anth_hooks/lib.sh"
if ! anth_resolve "$anth_hooks/.." || ! anth_installed; then
    exit 0
fi
anth_out=$("$ANTH_BIN" hook subagent-stop 2>/dev/null; printf 'x%s' "$?")
anth_rc=${anth_out##*x}
anth_out=${anth_out%x*}
while :; do
    case $anth_out in
        *"$ANTH_NL") anth_out=${anth_out%"$ANTH_NL"} ;;
        *) break ;;
    esac
done
if [ "$anth_rc" = 0 ] && [ -n "$anth_out" ]; then
    printf '%s\n' "$anth_out"
fi
exit 0
