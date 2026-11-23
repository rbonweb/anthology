#!/usr/bin/env bash
# atl-hook-version: 4
# ATL Claude Code hook — rewrites commands to use atl for token savings.
# Requires: atl >= 0.23.0, jq
#
# This is a thin delegating hook: all rewrite logic lives in `atl rewrite`,
# which is the single source of truth (src/discover/registry.rs).
# To add or change rewrite rules, edit the Rust registry — not this file.
#
# Exit code protocol for `atl rewrite`:
#   0 + stdout  Rewrite found, an allow rule matched → auto-allow
#   1           No ATL equivalent → pass through unchanged
#   2           Deny rule matched → pass through (Claude Code native deny handles it)
#   3 + stdout  Ask rule matched, or no rule matched → rewrite but let Claude Code prompt the user

if ! command -v jq &>/dev/null; then
  echo "[atl] WARNING: jq is not installed. Hook cannot rewrite commands. Install jq: https://jqlang.github.io/jq/download/" >&2
  exit 0
fi

if ! command -v atl &>/dev/null; then
  echo "[atl] WARNING: atl is not installed or not in PATH. Hook cannot rewrite commands. Run /anthology:setup to install it." >&2
  exit 0
fi

# Version guard: atl rewrite was added in 0.23.0.
# Older binaries: warn once and exit cleanly (no silent failure).
# Cache the version check to avoid spawning multiple processes on every hook call.
CACHE_DIR=${XDG_CACHE_HOME:-$HOME/.cache}
CACHE_FILE="$CACHE_DIR/atl-hook-version-ok"
if [ ! -f "$CACHE_FILE" ]; then
  ATL_VERSION_RAW=$(atl --version 2>/dev/null)
  ATL_VERSION=${ATL_VERSION_RAW#atl }
  ATL_VERSION=${ATL_VERSION%% *}
  if [ -n "$ATL_VERSION" ]; then
    IFS=. read -r MAJOR MINOR PATCH <<<"$ATL_VERSION"
    # Require >= 0.23.0
    if [ "$MAJOR" -eq 0 ] && [ "$MINOR" -lt 23 ]; then
      echo "[atl] WARNING: atl $ATL_VERSION is too old (need >= 0.23.0). Upgrade: cargo install atl" >&2
      exit 0
    fi
  fi
  mkdir -p "$CACHE_DIR" 2>/dev/null
  touch "$CACHE_FILE" 2>/dev/null
fi

INPUT=$(cat)
CMD=$(jq -r '.tool_input.command // empty' <<<"$INPUT")

if [ -z "$CMD" ]; then
  exit 0
fi

# Delegate all rewrite + permission logic to the Rust binary.
# Scrub ATL_REWRITE_HOST: that channel relaxes ATL's approval gate, and this
# hook turns exit 0 into an explicit allow, so an inherited value (a shell rc,
# .envrc, or CI env) must not reach it. Only the delegate that sets it may
# rely on it.
REWRITTEN=$(env -u ATL_REWRITE_HOST atl rewrite "$CMD" 2>/dev/null)
EXIT_CODE=$?

case $EXIT_CODE in
  0)
    # Rewrite found and an allow rule matched — safe to auto-allow.
    # If the output is identical, the command was already using ATL.
    [ "$CMD" = "$REWRITTEN" ] && exit 0
    ;;
  1)
    # No ATL equivalent — pass through unchanged.
    exit 0
    ;;
  2)
    # Deny rule matched — let Claude Code's native deny rule handle it.
    exit 0
    ;;
  3)
    # An ask rule matched, or no rule did — rewrite the command but do NOT
    # auto-allow, so Claude Code prompts the user for confirmation.
    ;;
  *)
    exit 0
    ;;
esac

if [ "$EXIT_CODE" -eq 3 ]; then
  # Ask: rewrite the command, omit permissionDecision so Claude Code prompts.
  jq -c --arg cmd "$REWRITTEN" \
    '.tool_input.command = $cmd | {
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "updatedInput": .tool_input
      }
    }' <<<"$INPUT"
else
  # Allow: rewrite the command and auto-allow.
  jq -c --arg cmd "$REWRITTEN" \
    '.tool_input.command = $cmd | {
      "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "allow",
        "permissionDecisionReason": "ATL auto-rewrite",
        "updatedInput": .tool_input
      }
    }' <<<"$INPUT"
fi
