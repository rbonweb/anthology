---
name: 'check-repo agent, deps and scanner rules: where the calls live'
description: 'Design calls in scan rules agent.rs, deps.rs, scanner.rs: pinned MCP LOW, MEDIUM floor, lock URL classes, shared helpers'
metadata:
  type: project
  modified: 2026-11-28
---

The agent, deps and scanner rules in crates/anthology-scan/src/rules share small helpers that live in agent.rs as pub(super): name_of, parent_of, line_at, clip. A pinned runner MCP server (npx pkg@1.2.3) is a direct LOW finding, not a Surface, because the orchestrator max(floor, severity) cannot lower a level. Other MCP commands are Surfaces with floor MEDIUM so an allowlisted shape like tsc is not INFO. Settings and skill commands run from the folder above .claude, so a Surface dir is that folder. Lockfile URLs: http is HIGH; git+ is LOW; a foreign https host is MEDIUM when it keeps the registry layout (/name/-/file) and LOW (bare tarball) when it does not; an official host without the layout is a name mismatch. Name checks decode percent escapes up to three times and resolve .. first. JSON walks must be iterative because the pycompat parser has no depth limit. deps.rs counts hits per rule in a Tally and converts the first offset to a line once, so a lockfile with thousands of hits stays linear. Rust fmt is checked by lint on the build host; rustfmt --edition 2024 on the host formats the same way as toolchain 1.100.
