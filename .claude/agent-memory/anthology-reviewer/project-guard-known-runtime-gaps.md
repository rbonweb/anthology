---
name: 'Guard gaps open after PR 159 (head b3fe664a)'
description: 'Positional, renamed-program and code-from-value bypasses found in PR 159; closed in bin 0.1.4'
metadata:
  type: project
  modified: 2026-11-22
---

Status: closed in anthology-bin 0.1.4 (see crates/anthology-guard/tests/param_expansion.rs and g0_fp.rs).

PR 159 (ADR-033) closed the b42ae431 gaps (variable program word, prompt-expansion operator, zsh e flag, bare glob qualifiers, read into a subscript). Still Allow on head and proven to run with a marker:
1. Positional program where the analysis does not put the arguments in place: bash -s with arguments and a here-string or pipe, bash or source of /dev/stdin with arguments, find -exec sh -c with the quoted first positional as program and {} as argument, a function called from a trap or eval string, a function with a subshell body f() ( ... ), zsh anonymous function with arguments, command_not_found_handle or handler, zsh read -A argv.
2. Renamed program: printf -v into BASH_CMDS or BASH_ALIASES element, declare -n to those tables, zsh set -A commands or functions, env with a BASH_FUNC_name%% value then bash -c name.
3. Code from a value: declare -n to PS4, BASH_ENV or ENV set to a value holding a command substitution (bash expands BASH_ENV), heredoc body with the zsh e flag or the bash P operator (lexer heredoc path scans only substitutions), setopt with the option name built from a variable.
4. Glob qualifier after a closing paren of a pattern group, and inside a default-value word of a brace expansion in zsh.
5. Arithmetic on a value built piecewise (dollar sign kept in a separate variable, printf -v with a percent-s, pattern replacement inserting a dollar sign).
Literal G1 misses outside the PR classes: zsh noglob, nocorrect and =cmd prefixes, bash named coproc with a brace body, glob in the program path, find -exec with {} as program.
Why: found in the PR 159 security review (2026-11-22) and reported to the lead.
How to apply: re-probe these first in any later guard review; resave with --replace dropping the ones that now block.
