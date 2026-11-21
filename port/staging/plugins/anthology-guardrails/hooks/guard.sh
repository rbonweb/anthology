#!/bin/sh
# guard.sh: the anthology-guardrails PreToolUse shim (design §9.2). It fails closed.
#
# Exactly two paths allow (exit 0): the binary's allow token (exit 0 and stderr exactly
# `anthology-guard:allow:v1` plus one newline) and the read-only allowlist when the binary
# cannot run. Every other path, including anything unexpected, ends in a block (exit 2). The
# pieces live in lib.sh; the decisions stay here, where the test counts them.
anth_nl='
'
anth_loaded=
anth_input=
anth_call=
ANTH_MSG=
# A hook timeout or an interrupt still blocks, and takes the stdin buffer with it.
trap '[ -z "$anth_input" ] || rm -f "$anth_input"; exit 2' HUP INT TERM
case $0 in
    */*) anth_hooks=${0%/*} ;;
    *) anth_hooks=. ;;
esac
# A lib.sh that is missing or does not parse must not end this shell early: try it in a
# subshell first.
if [ -f "$anth_hooks/lib.sh" ] && (command . "$anth_hooks/lib.sh") >/dev/null 2>&1; then
    # shellcheck source=lib.sh
    command . "$anth_hooks/lib.sh" && anth_loaded=1
fi
if [ "$anth_loaded" = 1 ] && anth_guard_state "$anth_hooks/.."; then
    if [ "$ANTH_STATE" = hot ]; then
        anth_call=1
    elif anth_guard_buffer; then
        anth_input=$ANTH_BUF
        if anth_readonly "$anth_input"; then
            rm -f "$anth_input"
            exit 0
        fi
        anth_guard_offline
        [ "$ANTH_RUN" != 1 ] || anth_call=1
    else
        ANTH_MSG=''
    fi
    if [ "$anth_call" = 1 ]; then
        anth_guard_run "$anth_input"
        if [ "$ANTH_RC" = 0 ] && [ "$ANTH_ERR" = "anthology-guard:allow:v1$anth_nl" ]; then
            [ -z "$anth_input" ] || rm -f "$anth_input"
            exit 0
        fi
        anth_guard_settle
    fi
    [ -z "$anth_input" ] || rm -f "$anth_input"
    case ${ANTH_MSG:-} in
        'Blocked by anthology-guardrails ('*)
            printf '%s\n' "$ANTH_MSG" >&2
            exit 2
            ;;
    esac
fi
[ -z "$anth_input" ] || rm -f "$anth_input"
printf '%s\n' 'Blocked by anthology-guardrails (setup): the guard binary is not installed (the guard shim could not finish), so shell commands, file edits and MCP tools are blocked; run /anthology:setup (or start a new session while online) to install it; if this keeps happening, disable anthology-guardrails with /plugin' >&2
exit 2
