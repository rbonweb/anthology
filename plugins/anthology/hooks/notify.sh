#!/bin/sh
# Notification hook: a desktop alert with sound when Claude Code needs your attention. The hook
# entry in hooks.json passes what happened as the one argument: `approval` (a permission prompt)
# or `idle` (waiting for your input). The hook JSON on stdin is ignored, so no text from it ever
# reaches a notifier. The alert is best effort: this script always exits 0 and never blocks a
# session. Set ANTHOLOGY_NOTIFY=0 to turn it off.
[ "${ANTHOLOGY_NOTIFY:-}" = "0" ] && exit 0

case ${1:-} in
    approval) msg="Claude needs your approval" ;;
    idle) msg="Claude is waiting for you" ;;
    *) msg="Claude needs your attention" ;;
esac

# Cap each external call at 5 s where `timeout` exists (hooks.json caps the whole hook at 5 s).
notify_run() {
    if command -v timeout >/dev/null 2>&1; then
        timeout 5 "$@" >/dev/null 2>&1
    else
        "$@" >/dev/null 2>&1
    fi
}

notified=0
case $(uname -s 2>/dev/null) in
    Darwin)
        if command -v osascript >/dev/null 2>&1; then
            notify_run osascript -e 'on run argv' \
                -e 'display notification (item 1 of argv) with title "Claude Code" sound name "Glass"' \
                -e 'end run' "$msg" && notified=1
        fi
        ;;
    Linux)
        if command -v notify-send >/dev/null 2>&1; then
            notify_run notify-send "Claude Code" "$msg" && notified=1
        fi
        if command -v canberra-gtk-play >/dev/null 2>&1; then
            notify_run canberra-gtk-play -i message && notified=1
        elif command -v paplay >/dev/null 2>&1; then
            for sound in /usr/share/sounds/freedesktop/stereo/message.oga \
                /usr/share/sounds/freedesktop/stereo/complete.oga; do
                if [ -r "$sound" ]; then
                    notify_run paplay "$sound" && notified=1
                    break
                fi
            done
        fi
        ;;
esac

# Nothing worked: ring the terminal bell, if there is a terminal to ring.
[ "$notified" = 1 ] || { printf '\a' >/dev/tty; } 2>/dev/null
exit 0
