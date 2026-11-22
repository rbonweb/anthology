---
name: 'Test Claude Code hooks without an API key'
description: 'Recipe: fake Anthropic server plus claude -p in a container; pitfalls of module static checks and the plugin-authoring skill'
metadata:
  type: reference
  modified: 2026-11-22
---

To exercise Claude Code hooks and mods with no real model: in a node:22-bookworm-slim container install the claude-code npm package per version under /opt/cc/VERSION, run a small node http server that answers POST /v1/messages with scripted SSE (message_start, content_block_start, input_json_delta for a tool_use, content_block_stop, message_delta, message_stop). Point the base-URL env var at it and give any placeholder value for the API-key env var. Run as the non-root node user (bypassPermissions refuses root) with the config-dir env var under /tmp, and call claude -p with --plugin-dir and --debug-file. Let the mock emit a tool_use only when the conversation has one message, or the loop never ends (a later user turn reuses the same trigger text). Drive hundreds of tool calls in one process by counting tool_result blocks.
Module static checks that bit: a helper that receives the engine object must be a top-level function declaration in the module file (a const arrow inside register is refused); claude plugin validate reads modules on 2.1.311 too. The d.ts type-checks with tsc 5.6, strict, moduleResolution Bundler, empty types list, the d.ts in include.
Pitfalls: the host shell may hold an auth token and base URL in its environment: do not forward them into a container, use the mock. Loading the plugin-authoring skill creates an empty folder under the host dev-mods directory named by the session id at once (no manifest, so mods stay off): remove it with rmdir on the literal path. The npm package ships no claude-code.d.ts; the types come from that skill (copy them into the container). apt-get install rustc in the container pulls gcc and took about 10 minutes on a slow mirror; the guard blocks long multi-heredoc docker exec commands (G0), so write one file per call.
