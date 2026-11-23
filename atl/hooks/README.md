# LLM Agent Hooks

## Scope

**Deployed hook artifacts** — the files and agent-specific configuration installed on user machines by `atl init`. External scripts and plugins are thin delegates to `atl rewrite`; native processors such as Codex call the same Rust rewrite registry directly. Zero filtering logic is duplicated in this directory.

Contains per-agent hook scripts and configuration files for the supported agents (Claude Code, Copilot, Cursor, Cline, Windsurf, Codex, OpenCode, Hermes, Pi, Oh My Pi, Mistral Vibe, Trae, Google Antigravity). `atl init` installs them.

Does **not** own: hook installation/uninstallation (that's `src/hooks/init/`), the rewrite pattern registry (that's `discover/registry`), or integrity verification (that's `src/hooks/integrity.rs`).

## Awareness files

`atl-awareness.md`, `atl-awareness-high.md`, `atl-awareness-full.md` are the agent-neutral
instruction files embedded by `atl init` (`ATL.md`, `GEMINI.md`, rules files, `AGENTS.md` blocks).
`awareness.level` in `config.toml` picks one for hook-based agents; agents without a hook always get
`atl-awareness-full.md`, the only one that tells the agent to prefix external programs with `atl`.

## Purpose

LLM agent integrations that intercept CLI commands and route them through ATL for token optimization. Each hook transparently rewrites raw commands (e.g., `git status`) to their ATL equivalents (e.g., `atl git status`), cutting the bash output that reaches the LLM context without requiring the agent or user to change their workflow.

## How It Works

```
Agent runs command (e.g., "cargo test --nocapture")
  -> Hook intercepts (PreToolUse / plugin event)
  -> Reads JSON input, extracts command string
  -> Uses the shared ATL rewrite registry
  -> Registry matches pattern, returns "atl cargo test --nocapture"
  -> Hook sends response in agent-specific JSON format
  -> Agent executes "atl cargo test --nocapture" instead
  -> Filtered output reaches LLM
```

All rewrite logic lives in the `atl` binary. External hook scripts call `atl rewrite`; native Rust hook processors call the registry directly. This keeps one source of truth for all rewrite patterns.

## Directory Structure

Each agent subdirectory has its own README with hook-specific details:

- **[`claude/`](claude/README.md)** — Shell hook, `PreToolUse` JSON format, `settings.json` patching, test script
- **[`copilot/`](copilot/README.md)** — Rust binary hook, dual format (VS Code Chat vs Copilot CLI), deny-with-suggestion fallback
- **[`cursor/`](cursor/README.md)** — Shell hook, Cursor JSON format, empty `{}` response requirement
- **[`cline/`](cline/README.md)** — Rules file (prompt-level), `.clinerules` project-local installation
- **[`windsurf/`](windsurf/README.md)** — Rules file (prompt-level), `.windsurfrules` workspace-scoped
- **[`codex/`](codex/README.md)** — Rust binary hook (`atl hook codex`), `PreToolUse.updatedInput`, `.codex/hooks.json` / `$CODEX_HOME/hooks.json`, plus `AGENTS.md` awareness
- **[`opencode/`](opencode/README.md)** — TypeScript plugin, OpenCode 2.0 and 1.x >= 1.18.29, in-place mutation
- **[`pi/`](pi/README.md)** — TypeScript extension, `tool_call` event, local `isBashToolCallEvent` guard, in-place mutation, `~/.pi/agent/extensions/`; **shared with Oh My Pi (OMP)** — OMP installs the same file at `.omp/extensions/` via its `legacy-pi-compat` layer
- **[`hermes/`](hermes/README.md)** — Python plugin, `pre_tool_call` hook, in-place terminal command mutation
- **[`vibe/`](vibe/README.md)** — Rust binary hook (`atl hook vibe`), `pre_tool` entry in `~/.vibe/hooks.toml`, `hook_specific_output.tool_input` rewrite plus `system_message` for UI visibility
- **[`antigravity/`](antigravity/README.md)** — Rust binary hook (`atl hook antigravity`), `PreToolUse` plugin entry in `.agents/plugins/atl/hooks.json` or `~/.gemini/config/plugins/atl/hooks.json`, transparent rewrite via `overwrite.CommandLine`

## Supported Agents

| Agent | Mechanism | Hook Type | Can Modify Command? |
|-------|-----------|-----------|---------------------|
| Claude Code | Shell hook (`PreToolUse`) | Transparent rewrite | Yes (`updatedInput`) |
| VS Code Copilot Chat | Rust binary (`atl hook copilot`) | Transparent rewrite | Yes (`updatedInput`) |
| GitHub Copilot CLI | Rust binary (`atl hook copilot`) | Deny-with-suggestion | No (agent retries) |
| Cursor | Rust binary | Transparent rewrite | Yes (`updated_input`) |
| Trae | Rust binary (`atl hook trae`) | Transparent rewrite | Yes (`updatedInput`) |
| Google Antigravity | Rust binary (`atl hook antigravity`) | Transparent rewrite | Yes (`overwrite.CommandLine`) |
| Gemini CLI | Rust binary (`atl hook gemini`) | Transparent rewrite | Yes (`hookSpecificOutput`) |
| Cline / Roo Code | Custom instructions (rules file) | Prompt-level guidance | N/A |
| Windsurf | Custom instructions (rules file) | Prompt-level guidance | N/A |
| Codex CLI | Rust binary (`atl hook codex`) | Transparent rewrite | Yes (`updatedInput`) |
| OpenCode | TypeScript plugin (`execute.before` in 2.0, `tool.execute.before` in 1.x >= 1.18.29) | In-place mutation | Yes |
| Pi | TypeScript extension (`tool_call` event) | In-place mutation | Yes |
| Oh My Pi (OMP) | TypeScript extension (`tool_call` event, shared with Pi) | In-place mutation | Yes |
| Hermes | Python plugin (`pre_tool_call`) | In-place mutation | Yes |
| Mistral Vibe | Rust binary (`atl hook vibe`) | Transparent rewrite | Yes (`hook_specific_output.tool_input`) |

## JSON Formats by Agent

### Claude Code (Shell Hook)

**Input** (stdin):

```json
{
  "tool_name": "Bash",
  "tool_input": { "command": "git status" }
}
```

**Output** (stdout, when rewritten):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "permissionDecisionReason": "ATL auto-rewrite",
    "updatedInput": { "command": "atl git status" }
  }
}
```

### Cursor (Shell Hook)

**Input**: Same as Claude Code.

**Output** (stdout, when rewritten):

```json
{
  "permission": "allow",
  "updated_input": { "command": "atl git status" }
}
```

Returns `{}` when no rewrite (Cursor requires JSON for all paths).

### Trae (Rust Binary)

Install with `atl init --agent trae` for `.trae/hooks.json`, or `atl init -g --agent trae` for `~/.trae/hooks.json`; global installation also updates `~/.trae-cn/hooks.json` when that directory already exists.

**Input**: Trae `PreToolUse` payloads for `RunCommand`, such as:

```json
{
  "tool_name": "RunCommand",
  "tool_input": { "command": "git status", "description": "Check status" }
}
```

**Output** (only when rewritten):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "updatedInput": { "command": "atl git status", "description": "Check status" }
  }
}
```

ATL deliberately does not set `permissionDecision`; Trae retains its own command-approval policy.

### Codex CLI (Rust Binary)

**Input** (stdin):

```json
{
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "permission_mode": "default",
  "tool_input": { "command": "git status" }
}
```

**Output** (stdout, when rewritten):

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "permissionDecisionReason": "ATL auto-rewrite",
    "updatedInput": { "command": "atl git status" }
  }
}
```

The `allow` value is required by Codex to accept `updatedInput`; Codex still runs its native approval and sandbox checks after applying the replacement. Those checks classify the rewritten command, and Codex does not currently unwrap the `atl` binary for its safe/dangerous-command heuristics. Missing or unknown permission modes and other no-rewrite cases produce no stdout.

### Copilot CLI (Rust Binary)

**Input** (stdin, camelCase, `toolArgs` is JSON-stringified):

```json
{
  "toolName": "bash",
  "toolArgs": "{\"command\": \"git status\"}"
}
```

**Output** (no `updatedInput` support -- uses deny-with-suggestion):

```json
{
  "permissionDecision": "deny",
  "permissionDecisionReason": "Token savings: use `atl git status` instead"
}
```

### VS Code Copilot Chat (Rust Binary)

**Input** (stdin, snake_case):

```json
{
  "tool_name": "Bash",
  "tool_input": { "command": "git status" }
}
```

**Output**: Same as Claude Code format (with `updatedInput`).

### Gemini CLI (Rust Binary)

**Input** (stdin):

```json
{
  "tool_name": "run_shell_command",
  "tool_input": { "command": "git status" }
}
```

**Output** (when rewritten):

```json
{
  "decision": "allow",
  "hookSpecificOutput": {
    "tool_input": { "command": "atl git status" }
  }
}
```

**No rewrite**: `{"decision": "allow"}`

### Mistral Vibe (Rust Binary)

**Input** (stdin):

```json
{
  "tool_name": "bash",
  "tool_input": { "command": "git status" },
  "hook_event_name": "pre_tool",
  "session_id": "..."
}
```

**Output** (when rewritten):

```json
{
  "hook_specific_output": {
    "tool_input": { "command": "atl git status" }
  },
  "system_message": "atl: rewrote to `atl git status`"
}
```

**No rewrite**: exit 0 with empty stdout (Vibe's contract for "no opinion" from a `pre_tool` hook).

### Google Antigravity (Rust Binary)

**Input** (stdin):

```json
{
  "toolCall": {
    "name": "run_command",
    "args": { "CommandLine": "git status" }
  }
}
```

**Output** (when rewritten):

```json
{
  "decision": "allow",
  "reason": "Rewritten by atl for token optimization.",
  "overwrite": {
    "CommandLine": "atl git status"
  }
}
```

**No rewrite**: `{"decision": "allow"}`

### OpenCode (TypeScript Plugin)

Delegates to `atl hook opencode` via `node:child_process.execFile` (supports [OpenCode 2.0 and 1.x >= 1.18.29](opencode/README.md)). The Rust side owns the rewrite rules and the permission verdict; the plugin only carries it. The hook must be awaited — `args.command = …` after the call returns has nothing left to rewrite:

```typescript
const tool = String(input?.tool ?? "").toLowerCase()
if (tool === "bash" || tool === "shell") {
  // 2.x also has `event.agent`, which atl needs so an agent-scoped ask or deny
  // is judged against the agent that runs the command. 1.x sends no agent.
  const argv = ["hook", "opencode"]
  if (agent) argv.push("--agent", agent)
  argv.push(command)
  const rewritten = await new Promise((resolve) => {
    execFile(atlBin, argv, { encoding: "utf8", timeout: 3000, windowsHide: true }, (err, stdout) => {
      if (err) return resolve(null) // pass through unchanged
      try {
        resolve(JSON.parse(String(stdout ?? "").trim() || "{}").command ?? null)
      } catch {
        resolve(null)
      }
    })
  })
  // {} means the rewrite would change OpenCode's own verdict — run as typed.
  if (rewritten && rewritten !== command) {
    args.command = rewritten
  }
}
```

### Hermes (Python Plugin)

Mutates `args["command"]` in place and returns no directive, so on Hermes v2026.8.16 (0.20.2) and later another plugin's `modify` directive that sets `command` replaces the rewrite. The adapter uses a 5-second default rewrite timeout, configurable through `ATL_HERMES_TIMEOUT` (see [timeout limits](hermes/README.md#how-it-works)). Simplified success path:

```python
result = subprocess.run(
    ["atl", "rewrite", command], shell=False, stdin=subprocess.DEVNULL,
    capture_output=True, text=True, timeout=_rewrite_timeout(),
)
rewritten = result.stdout.strip()
if result.returncode in {0, 3} and rewritten and rewritten != command:
    args["command"] = rewritten
```

## Command Rewrite Registry

The registry handles command patterns across these categories:

| Category | Examples |
|----------|----------|
| Test Runners | vitest, pytest, cargo test, go test, playwright |
| Build Tools | cargo build, npm, pnpm, dotnet, make |
| VCS | git status/log/diff/show |
| Language Servers | tsc, mypy |
| Linters | eslint, ruff, golangci-lint, biome |
| Package Managers | pip, cargo install, pnpm list |
| File Operations | ls, find, grep, cat, head, tail |
| Infrastructure | docker, kubectl, aws, terraform |


### Compound Command Handling

The registry handles `&&`, `||`, `;`, `|`, `|&`, and `&` operators:

- **Pipe** (`|`): Producers and intermediate stages stay raw; only a pipeline-safe final stage is rewritten
- **Stderr pipe** (`|&`): The complete pipeline stays raw
- **And/Or/Semicolon** (`&&`, `||`, `;`): Both sides rewritten independently
- **Pipeline-safe rules**: Initially limited to argument-safe `grep`, `rg`, and `wc` invocations; search pattern-file forms defer

Example: `cargo fmt --all && cargo test` becomes `atl cargo fmt --all && atl cargo test`

### Override Controls

- **`ATL_DISABLED=1`**: Per-command override (`ATL_DISABLED=1 git status` runs raw)
- **`exclude_commands`**: In `~/.config/atl/config.toml`, list commands to never rewrite. Matches against the full command after stripping env prefixes. Subcommand patterns work (`"git push"` excludes `git push origin main`). Patterns starting with `^` are treated as regex.
- **Already-ATL**: `atl git status` passes through unchanged (no `atl atl git`)

## Exit Code Contract

Hooks must **never block command execution**. All error paths (missing binary, bad JSON, rewrite failure) must exit 0 so the agent's command runs unmodified. A hook that exits non-zero prevents the user's command from executing.

When there is no rewrite to apply, the hook must produce no output (or `{}` for Cursor, which requires JSON on all paths).

## Graceful Degradation

Hooks are **non-blocking** -- they never prevent a command from executing:

- jq not installed: warning to stderr, exit 0 (command runs raw)
- atl binary not found: warning to stderr, exit 0
- atl version too old (< 0.23.0): warning to stderr, exit 0
- Invalid JSON input: pass through unchanged
- `atl rewrite` crashes: hook exits 0 (subprocess error ignored)
- Filter logic error: fallback to raw command output

## Adding a New Agent Integration

New integrations must follow the [Exit Code Contract](#exit-code-contract) and [Graceful Degradation](#graceful-degradation) above, as well as the project's [Design Philosophy](../CONTRIBUTING.md#design-philosophy).

### Integration Tiers

| Tier | Mechanism | Maintenance | Examples |
|------|-----------|-------------|----------|
| **Full hook** | Shell script or Rust binary, intercepts commands via agent's hook API | High — must track agent API changes | Claude Code, Cursor, Copilot, Gemini, Codex, Antigravity |
| **Plugin** | TypeScript/JS/Python plugin in agent's plugin system | Medium — agent manages loading | OpenCode, Hermes, Pi, Oh My Pi |
| **Rules file** | Prompt-level instructions the agent reads | Low — no code to break | Cline, Windsurf |

### Eligibility

ATL supports AI coding assistants that developers actually use day-to-day. To add a new agent:

- Agent has a **documented, stable hook/plugin API** (not experimental/alpha)
- Agent is **actively maintained** (commit activity in last 3 months)
- Integration follows the **exit code contract** (exit 0 on all error paths)
- Hook output matches the **agent's expected JSON format** exactly

### Maintenance

If an agent's API changes and the hook breaks, the integration should be updated promptly. If the agent becomes unmaintained or the hook can't be fixed, the integration may be deprecated with a release note.
