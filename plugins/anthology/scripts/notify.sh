#!/bin/sh
# Notification hook: a desktop alert with sound when Claude Code needs your attention (a
# permission prompt, or waiting for input). It never blocks or fails the session: every step is
# best effort and the script always exits 0. Set ANTHOLOGY_NOTIFY=0 to turn it off.
#
# The hook JSON arrives on stdin. The message is read with python3 (no eval, no shell parsing)
# and handed to the notifier as an argument, never pasted into AppleScript or a shell command.
[ "${ANTHOLOGY_NOTIFY:-}" = "0" ] && exit 0

msg=$(python3 -I -c '
import json, sys
try:
    text = json.load(sys.stdin).get("message")
except Exception:
    text = None
if not isinstance(text, str):
    text = ""
text = "".join(c for c in text if c.isprintable() or c.isspace())
sys.stdout.buffer.write(" ".join(text.split())[:200].encode("utf-8"))
' 2>/dev/null)
[ -n "$msg" ] || msg="Needs your attention"
# A leading "-" could be read as an option by the notifier.
case $msg in -*) msg=" $msg" ;; esac

# Cap each external call at 5 s where `timeout` exists (hooks.json caps the whole hook at 5 s).
run() {
  if command -v timeout >/dev/null 2>&1; then
    timeout 5 "$@" >/dev/null 2>&1
  else
    "$@" >/dev/null 2>&1
  fi
}

done=0
case $(uname -s 2>/dev/null) in
  Darwin)
    if command -v osascript >/dev/null 2>&1; then
      run osascript -e 'on run argv' \
        -e 'display notification (item 1 of argv) with title "Claude Code" sound name "Glass"' \
        -e 'end run' "$msg" && done=1
    fi
    ;;
  Linux)
    if command -v notify-send >/dev/null 2>&1; then
      run notify-send "Claude Code" "$msg" && done=1
    fi
    if command -v canberra-gtk-play >/dev/null 2>&1; then
      run canberra-gtk-play -i message && done=1
    elif command -v paplay >/dev/null 2>&1; then
      for sound in /usr/share/sounds/freedesktop/stereo/message.oga \
        /usr/share/sounds/freedesktop/stereo/complete.oga; do
        if [ -r "$sound" ]; then
          run paplay "$sound" && done=1
          break
        fi
      done
    fi
    ;;
esac

# Nothing else worked: ring the terminal bell, if there is a terminal to ring.
[ "$done" = 1 ] || { printf '\a' >/dev/tty; } 2>/dev/null
exit 0
