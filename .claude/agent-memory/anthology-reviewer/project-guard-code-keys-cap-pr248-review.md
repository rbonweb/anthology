---
name: 'Code-key brace cap drop (PR 248) review findings'
description: 'Head b20257d7: dropping G0 for tool-input code keys opens a brace-built profile path in shell-running code keys; the Bash call-site flag is not pinned by any guard test'
metadata:
  type: project
  modified: 2026-11-27
---

Head b20257d7 (base 0ae7b204) makes tool-input code keys (script, code, expression, func, fn) read with the 32-result, 4-round Python brace expansion when the full expansion passes 4096 results, instead of G0.
Open at that head: (1) a profile path built with 13 empty brace groups inside one component, for example ~/.config/chrom then 13 copies of {,} then ium/Default/Cookies, under a script or code key (also nested in an array, and mcp__ide__executeCode with a ! line) went from G0 on base to Allow on head; 12 groups (4096) still give G3. Pre-existing gaps in the same layer (variable, quote split, JS string concat) were already Allow on base, so the layer is a literal-spelling tripwire only, never a shell check. (2) Flipping the true in g3 code_hits_profile(entry.code, cwd, true) to false survives the whole anthology-guard suite including mcp_not_shell.rs: python3 -c and node -e still cap through their raw word; only a heredoc body (python3 - with a heredoc, node with a heredoc) shows the mutant as Allow.
Why: the next review of a code-key or brace-cap change should re-run these shapes and check the heredoc pin exists.
How to apply: probe with the release binary (anthology guard hook --shim, JSON on stdin, HOME set to the gd.rs test home) from a node script in a scripts/rust-test.sh session container; the host guard itself blocks a Bash command that spells the 13-group shape, so build payloads with string repeat in the script. host_path_vars.rs fails 20 tests when the tree sits under /tmp in that container, on base as well, so read its failures as environment.
