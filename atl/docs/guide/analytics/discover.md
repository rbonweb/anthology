---
title: Discover and Session
description: Find missed savings opportunities with atl discover, and track ATL adoption with atl session
sidebar:
  order: 2
---

# Discover and Session

## atl discover — find missed savings

`atl discover` analyzes your Claude Code command history to identify commands that ran without ATL filtering, and estimates how much bash output ATL would have removed from them.

```bash
atl discover                    # analyze current project history
atl discover --all              # all projects
atl discover --all --since 7    # last 7 days, all projects
```

**Example output** (illustrative values):

```
Missed savings analysis (last 7 days)
────────────────────────────────────
Command              Count   Est. lost
cargo test              12     ~48,000 tokens
git log                  8     ~12,000 tokens
pnpm list                3      ~6,000 tokens
────────────────────────────────────
Total missed:           23     ~66,000 tokens

Run `atl init --global` to capture these automatically.
```

The `~N tokens` figures are **estimated bash output bytes divided by 4**, not tokens billed by your provider. ATL ships no real tokenizer, and bash output is only one contributor to input tokens. Read them as an order of magnitude of the output volume ATL could have compressed. See [How ATL Savings Work](../resources/savings-explained.md).

If commands appear in the missed list after installing ATL, it usually means the hook isn't active for that agent. See [Troubleshooting](../resources/troubleshooting.md#ai-assistant-not-using-atl) — "AI assistant not using ATL".

## atl session — adoption tracking

`atl session` shows ATL adoption across recent Claude Code sessions: how many shell commands ran through ATL vs. raw.

```bash
atl session
```

**Example output** (illustrative values):

```
Recent sessions (last 10)
─────────────────────────────────────────────────────
Session                         Total   ATL   Coverage
2026-01-03 10:00  (40 cmds)       40    38      95.0%
2026-01-02 10:00  (40 cmds)       40    40     100.0%
2026-01-01 10:00  (40 cmds)       40    36      90.0%
─────────────────────────────────────────────────────
Average coverage: 95.0%
```

Low coverage on a session usually means ATL was disabled (`ATL_DISABLED=1`) or the hook wasn't active for a specific subagent.
