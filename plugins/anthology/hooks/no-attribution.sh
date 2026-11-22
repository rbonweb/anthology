#!/bin/sh
# no-attribution.sh: the PreToolUse (Bash) hook that enforces the attribution rule: commits, PRs
# and code comments carry only the git config author. It denies a `git commit` (amend included) or
# a `gh pr create|edit` whose text holds an assistant co-author trailer or a "Generated with" line.
# Everything else is allowed: no output, exit 0.
#
# The hook input is only searched with grep, never parsed, never evaluated and never run. In the
# input JSON a newline inside the command is the two characters `\n`, so a trailer is matched up to
# the next `\n` or `\r`: the name has to sit on the trailer's own line.
input=$(cat)

# A commit, or a pull request created or edited. `git` may be followed by options (`git -C dir`).
command_re='(^|[^[:alnum:]_-])(git[[:space:]]+([^;&|]*[[:space:]])?commit|gh[[:space:]]+pr[[:space:]]+(create|edit))([^[:alnum:]_-]|$)'
# The attribution text: a co-author trailer that names the assistant or its maker, or the
# "Generated with" line.
text_re='co-authored-by:([^\\]|\\[^nr])*(claude|anthropic)|generated with[[:space:]]+\[?claude'

if printf '%s' "$input" | LC_ALL=C grep -Eiq "$command_re" &&
   printf '%s' "$input" | LC_ALL=C grep -Eiq "$text_re"; then
  printf '%s\n' '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"Blocked by anthology: commits, PRs and code comments carry only the git config author. Remove the Co-Authored-By trailer or the Generated with line that names an assistant, do not mention Claude or AI, then run the command again."}}'
fi
exit 0
