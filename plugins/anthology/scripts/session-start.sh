#!/bin/sh
# SessionStart hook wrapper. A hook whose interpreter is missing fails silently, so this one is
# plain sh: without a working python3 (3.9 or newer) it prints one JSON systemMessage itself
# and exits 0; otherwise it hands over to session_start.py.
if ! command -v python3 >/dev/null 2>&1 ||
  ! python3 -c 'import sys; sys.exit(sys.version_info < (3, 9))' >/dev/null 2>&1; then
  printf '%s\n' '{"systemMessage":"anthology: a working python3 (3.9 or newer) was not found on PATH, so the guardrails self-test and the routing guide are off. Install Python 3.9 or newer and restart the session."}'
  exit 0
fi
case $0 in
  */*) dir=${0%/*} ;;
  *) dir=. ;;
esac
exec python3 "$dir/session_start.py"
