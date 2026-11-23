---
title: ATL Documentation
description: ATL (Anthology Token Layer) — cut the bash output your agent reads on common dev commands, with zero workflow changes
sidebar:
  order: 1
---

# ATL — Anthology Token Layer

ATL is a CLI proxy that sits between your AI assistant and your development tools. It filters command output before it reaches the LLM, keeping only what matters and discarding boilerplate, progress bars, and noise.

**Result:** fewer bash output bytes reaching the LLM per command, without changing how you work. You run `git status` as usual — ATL's hook intercepts it, filters the output, and the LLM sees a compact 3-line summary instead of 40 lines.

## How it works

```
Your AI assistant runs:  git status
                              ↓
              Hook intercepts (PreToolUse)
                              ↓
              atl git status  (transparent rewrite)
                              ↓
     Raw output: 40 lines     →     Filtered: 3 lines
                              ↓
              LLM sees the compact output
```

Zero config changes to your workflow. The hook handles everything automatically.

## What the savings mean

ATL reduces **bash output bytes** — the output a shell command sends back before your agent reads it. That is not the same as reducing your bill by the same amount.

Bash output is one contributor to input tokens, alongside your prompt, the system prompt and conversation history. Input tokens are in turn only part of the bill, which also counts output tokens. The reduction dilutes at every step.

See [How ATL Savings Work](./resources/savings-explained.md) for the full picture, including why the token counts ATL reports are estimates.

## What ATL optimizes

Dozens of commands across all major ecosystems — Git, Cargo/Rust, JavaScript, Python, Go, Ruby, .NET, Docker/Kubernetes, and more. See [What ATL Optimizes](./resources/what-atl-covers.md) for the full list.

## Get started

1. **[Installation](./getting-started/installation.md)** — Install ATL and verify you have the right package
2. **[Quick Start](./getting-started/quick-start.md)** — Connect to your AI assistant in 5 minutes
3. **[Supported Agents](./getting-started/supported-agents.md)** — Claude Code, Cursor, Copilot, Gemini, and more

## Measure your savings

```bash
atl gain           # total savings across all sessions
atl gain --daily   # day-by-day breakdown
atl gain --weekly  # weekly aggregation
```

See [Token Savings Analytics](./analytics/gain.md) for export formats and analysis workflows.

## Analyze your usage

```bash
atl discover       # find commands that ran without ATL (missed savings)
atl session        # ATL adoption rate per Claude Code session
```

See [Discover and Session](./analytics/discover.md) for details.

## Go further

Two optional layers sit on top of the hooks. Neither changes anything until you opt in.

- **[The engine](./pipelines/overview.md)** — everything ATL does is expressed in one composable `pipeline.toml`: which commands get rewritten, what happens to a tool's output, what the proxy compresses. With no pipeline file you are on `atl:default` and nothing changes. Start with [Writing a Pipeline](./pipelines/writing-pipelines.md); [Conditions and Judges](./pipelines/conditions.md) and [Methods](./pipelines/methods.md) are the vocabulary, [Pipeline Commands](./pipelines/commands.md) is for inspecting, debugging and sharing one.
- **[The ATL Proxy](./proxy/overview.md)** — a local daemon that runs the pipeline on the API traffic itself, one layer above bash output. Ships as its own download. See [What the Proxy Compresses](./proxy/compression.md) and [Proxy Configuration](./proxy/configuration.md).

## Further reading

- [Configuration](./getting-started/configuration.md) — config.toml, global flags, env vars, recall recovery
- [Troubleshooting](./resources/troubleshooting.md) — common issues and fixes
- [Telemetry & Privacy](./resources/telemetry.md) — what ATL collects and how to opt out
- [Agent Coverage](./resources/agent-coverage.md) — what each agent's hook API lets ATL do
- [Editions and Licensing](./resources/editions.md) — the three downloads, what each contains, where the licence boundary falls
- [ARCHITECTURE.md](https://github.com/rtk-ai/rtk/blob/master/ARCHITECTURE.md) — system design for contributors
