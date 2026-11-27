---
name: 'Guard G3 scans tool-input code keys; brace cap does not apply there'
description: 'Why an MCP tool with JavaScript in code hit G0, and which G3 code scans still cap'
metadata:
  type: project
  modified: 2026-11-27
---

check() always runs g3_input over a tool input code keys (code, script, expression, func, fn), for any tool. A word with a slash and a hint word (config, chrom, google, a bracket) is brace-expanded in full; over 4096 results it set Paths::capped and check returned TooComplex (G0), the only G0 a command-less input can reach. code_hits_profile now takes cap_blocks: false for tool-input code (Python-compatible expansion only), true for entry.code of a Bash command. Why: the BrowserOS run tool takes JavaScript in code, and a compact object literal gave a false G0. How to apply: reproduce with 13 small object literals in one unspaced word plus //config; tests in crates/anthology-guard/tests/mcp_not_shell.rs; G5 (banned browser servers), G3 path keys and G6 file tools were intended MCP rules and stay.
