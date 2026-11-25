#!/bin/sh
# no-attribution.sh: the PreToolUse (Bash) hook that enforces the attribution rule: commits, PRs
# and code comments carry only the git config author. It denies a `git commit` (amend included) or
# a `gh pr create|edit` whose command holds an assistant co-author trailer or a "Generated with"
# line. Everything else is allowed: no output, exit 0. This hook is the backstop: the fix at the
# source is Claude Code's `attribution` setting (`/anthology:setup` writes it into an approved
# repo's .claude/settings.json), and the deny reason says so.
#
# It catches the usual habit and is not a guarantee. Only the command text is checked, in the
# `Name: value` spelling, so `git commit --trailer`, `-F file` and `gh pr create --body-file` are
# not checked, and neither is a word built up by the shell. It also denies a command that removes
# the trailers from old commits if that command spells the pattern out; spell it so it does not
# hold the assistant's name.
#
# The hook input is only searched with grep, never parsed, never evaluated and never run. The
# search runs on the value of each `"command"` key only, so a description or any other field that
# names the assistant cannot cause a deny. In the input JSON a quote inside a string is always
# `\"`, so the key pattern only matches a real key and its whole string value, and a newline inside
# the command is the two characters `\n`: a trailer is matched up to the next `\n`, `\r` or the end
# of the string, so the name has to sit on the trailer's own line.
#
# PATH without its relative and empty entries first, as hooks/lib.sh's anth_safe_path makes it:
# the hook runs in the folder of the repository the user opened, which must not supply the cat,
# tr or grep below.
case :${PATH-}: in
  *::*|*:[!/]*)
    PATH=$(
      anth_in=${PATH-}
      PATH=/usr/bin:/bin
      set -f
      IFS=:
      # shellcheck disable=SC2086 # split on IFS (`:`) on purpose; globbing is off
      for anth_dir in $anth_in; do
        # `(/*)`: bash 3.2 misreads a bare `/*)` inside `$( )`.
        case $anth_dir in
          (/*) printf '%s:' "$anth_dir" ;;
        esac
      done
    )
    PATH=${PATH%:} ;;
esac
[ -n "${PATH-}" ] || PATH=/usr/bin:/bin:/usr/sbin:/sbin
export PATH
input=$(cat)
cmd=$(printf '%s' "$input" | LC_ALL=C tr -d '\n\r' |
  LC_ALL=C grep -Eo '"command"[[:space:]]*:[[:space:]]*"([^"\\]|\\.)*"')
[ -n "$cmd" ] || exit 0

# A commit, or a pull request created or edited. `git` may be followed by options (`git -C dir`).
command_re='(^|[^[:alnum:]_-])(git[[:space:]]+([^;&|]*[[:space:]])?commit|gh[[:space:]]+pr[[:space:]]+(create|edit))([^[:alnum:]_-]|$)'
# The attribution text: a co-author trailer that names the assistant or its maker, or the
# "Generated with" line.
text_re='co-authored-by:([^\\"]|\\[^nr])*(claude|anthropic)|generated with[[:space:]]+\[?claude'

if printf '%s' "$cmd" | LC_ALL=C grep -Eiq "$command_re" &&
   printf '%s' "$cmd" | LC_ALL=C grep -Eiq "$text_re"; then
  printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Blocked by anthology: commits, PRs and code comments carry only the git config author. Remove the Co-Authored-By trailer or the Generated with line that names an assistant, do not mention Claude or AI, then run the command again. To stop Claude Code adding it at the source, set \"attribution\": {\"commit\": \"\", \"pr\": \"\"} in the repo'\''s .claude/settings.json (/anthology:setup does this for an approved repo) or in ~/.claude/settings.json."}}'
fi
exit 0
