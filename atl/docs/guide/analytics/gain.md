---
title: Token Savings Analytics
description: Measure and analyze the bash output reduction ATL achieves with atl gain
sidebar:
  order: 1
---

# Token Savings Analytics

`atl gain` shows how much bash output ATL has removed across all your commands, with daily, weekly, and monthly breakdowns.

What `atl gain` measures is the reduction in **bash output bytes**, converted to estimated tokens. Bash output is one contributor to input tokens, alongside your prompt, the system prompt and conversation history, and input tokens are in turn only part of the bill, which also counts output tokens. See [How ATL Savings Work](../resources/savings-explained.md) for the full picture.

## Quick reference

```bash
# Default summary
atl gain

# Temporal breakdowns
atl gain --daily          # all days since tracking started
atl gain --weekly         # aggregated by week
atl gain --monthly        # aggregated by month
atl gain --all            # all breakdowns at once

# Classic flags
atl gain --graph          # ASCII graph, last 30 days
atl gain --history        # last 10 commands
atl gain --quota          # monthly quota savings estimate (default tier: 20x)
atl gain --quota -t pro   # use pro tier token budget for estimate
atl gain --recalls        # recall efficiency per filter (calibration)

# Live dashboard
atl gain                  # auto-launches the dashboard when the ATL Proxy runs
atl gain --watch          # force the dashboard
atl gain --text           # force the static view

# Attribution (where the savings came from)
atl gain --by-method      # per compression method
atl gain --by-rule        # per pipeline rule
atl gain --by-layer       # per token layer (read, tool_result, history, ...)
atl gain --by-pipeline    # per pipeline
atl gain --pipeline NAME  # one pipeline, sliced by where its savings land
atl gain --shadow         # what shadow rules would have saved

# Export
atl gain --all --format json > savings.json
atl gain --all --format csv  > savings.csv

# Reset
atl gain --reset          # zero the savings and recall stats (asks first)
```

## Daily breakdown

```bash
atl gain --daily
```

**Example output** (illustrative values — yours depend entirely on which commands you run):

```
📅 Daily Breakdown (3 days)
════════════════════════════════════════════════════════════════
Date            Cmds      Input     Output      Saved   Save%
────────────────────────────────────────────────────────────────
2026-01-01       100     500.0K     100.0K     400.0K   80.0%
2026-01-02       100     500.0K     100.0K     400.0K   80.0%
2026-01-03       100     500.0K     100.0K     400.0K   80.0%
────────────────────────────────────────────────────────────────
TOTAL            300       1.5M     300.0K       1.2M   80.0%
```

- **Cmds**: ATL commands executed
- **Input**: Estimated tokens from raw command output (`bytes / 4`)
- **Output**: Estimated tokens after filtering (`bytes / 4`)
- **Saved**: Input - Output, in estimated tokens
- **Save%**: Saved / Input × 100 — a **bash output byte ratio**, not a share of your bill

## Weekly and monthly breakdowns

```bash
atl gain --weekly
atl gain --monthly
```

Same columns as daily, aggregated by Sunday-Saturday week or calendar month.

## Export formats

| Format | Flag | Use case |
|--------|------|----------|
| `text` | default | Terminal display |
| `json` | `--format json` | Programmatic analysis, dashboards |
| `csv` | `--format csv` | Excel, Python/R, Google Sheets |

**JSON structure:**
```json
{
  "summary": {
    "total_commands": 300,
    "total_input": 1500000,
    "total_output": 300000,
    "total_saved": 1200000,
    "avg_savings_pct": 80.0
  },
  "daily": [...],
  "weekly": [...],
  "monthly": [...]
}
```

## The RTK Pro line

After a week of data, `atl gain` ends with one dim line about [RTK Pro](https://pro.rtk-ai.app), at most once a day and only on a terminal: a piped `atl gain`, and so an agent reading it, never sees it. `pro_hint = false` under `[display]` in `config.toml`, or `ATL_PRO_HINT=0`, turns it off.

## How token estimation works

`atl gain` estimates tokens as `bytes / 4`; ATL ships no tokenizer. The same estimator is applied to raw and filtered output, so the percentage is reliable; the absolute token counts are approximate and will not match your provider's billing.

```
Input Tokens  = estimate_tokens(raw_command_output)
Output Tokens = estimate_tokens(atl_filtered_output)
Saved Tokens  = Input - Output
Savings %     = (Saved / Input) × 100
```

## Database

Savings data is stored locally in SQLite:

- **Location**: `~/.local/share/atl/history.db` on Linux, `~/Library/Application Support/atl/history.db` on macOS (override with `ATL_DB_PATH`) — paths below use the Linux location
- **Retention**: 90 days (automatic cleanup)
- **Scope**: Global across all projects and Claude sessions

```bash
# Inspect raw data
sqlite3 ~/.local/share/atl/history.db \
  "SELECT timestamp, atl_cmd, saved_tokens FROM commands
   ORDER BY timestamp DESC LIMIT 10"

# Backup
cp ~/.local/share/atl/history.db ~/backups/atl-history-$(date +%Y%m%d).db

# Reset
rm ~/.local/share/atl/history.db    # recreated on next command
```

Prefer `--reset` over deleting the file: it zeroes both the token savings and the
[recall stats](#recall-efficiency), which live separately, and leaves the database intact.

```bash
atl gain --reset          # asks for confirmation
atl gain --reset --yes    # skip the prompt (scripts)
```

## Analysis workflows

```bash
# Weekly progress: generate a CSV report every Monday
atl gain --weekly --format csv > reports/week-$(date +%Y-%W).csv

# Monthly budget review
atl gain --monthly --format json | jq '.monthly[] |
  {month, saved_tokens, quota_pct: (.saved_tokens / 6000000 * 100)}'

# Cron: daily JSON snapshot for a dashboard
0 0 * * * atl gain --all --format json > /var/www/dashboard/atl-stats.json
```

**Python/pandas:**
```python
import pandas as pd
import subprocess

result = subprocess.run(['atl', 'gain', '--all', '--format', 'csv'],
                       capture_output=True, text=True)
lines = result.stdout.split('\n')
daily_start = lines.index('# Daily Data') + 2
daily_end = lines.index('', daily_start)
daily_df = pd.read_csv(pd.StringIO('\n'.join(lines[daily_start:daily_end])))
daily_df['date'] = pd.to_datetime(daily_df['date'])
daily_df.plot(x='date', y='savings_pct', kind='line')
```

**GitHub Actions (weekly stats):**
```yaml
on:
  schedule:
    - cron: '0 0 * * 1'
jobs:
  stats:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - run: cargo install --git https://github.com/rtk-ai/rtk --branch master atl
      - run: atl gain --weekly --format json > stats/week-$(date +%Y-%W).json
      - run: git add stats/ && git commit -m "Weekly atl stats" && git push
```

## Quota estimate

`--quota` expresses the estimated tokens saved as a fraction of a monthly subscription budget. Like every other figure in `atl gain`, it is derived from the `bytes / 4` estimate of bash output, so treat it as an order of magnitude rather than a billing forecast.

```bash
atl gain --quota          # uses 20x tier by default
atl gain --quota -t pro   # Claude Pro plan budget
atl gain --quota -t 5x    # 5× usage plan budget
atl gain --quota -t 20x   # 20× usage plan budget
```

The tiers (`pro`, `5x`, `20x`) correspond to Anthropic Claude API subscription levels, each with a different monthly token allocation. ATL uses those allocations as a denominator to express your savings as a percentage of your budget.

:::tip[Find missed savings]
`atl gain` shows what ATL saved. To find commands that ran *without* ATL and calculate what you lost, see [atl discover](./discover.md).
:::

## Live dashboard

`--watch` replaces the static report with a full-screen live dashboard covering **both** savings layers at once: the agent hooks trimming command output, and the [ATL Proxy](../proxy/overview.md) compressing billed input tokens.

```bash
atl gain --watch
```

### Auto-launch

A bare `atl gain` launches the dashboard **automatically** once the ATL Proxy is running. All four conditions must hold:

1. You passed no flag asking for a specific static view — no `--daily`, `--weekly`, `--monthly`, `--all`, `--history`, `--graph`, `--quota`, and `--format` left at `text`.
2. You did not pass `--text`.
3. Standard output is an interactive terminal — piping or redirecting opts out, so scripts and cron jobs are unaffected.
4. The ATL Proxy is running and reachable.

Without the proxy running, `atl gain` behaves exactly as it always has.

`--text` is the escape hatch — it forces the static view with the proxy up:

```bash
atl gain --text
```

`--watch` and `--text` are mutually exclusive. Asking for `--watch` without a terminal falls back to the text view with a warning rather than failing.

## Attribution

The default report tells you *how much* was saved. The attribution flags tell you *where it came from*, which helps you tune a pipeline. Each covers both the hook and proxy sites; proxy figures appear only if you run the [ATL Proxy](../proxy/overview.md).

```bash
atl gain --by-method      # which compression method did the work
atl gain --by-rule        # which pipeline rule fired
atl gain --by-layer       # which token layer (read, tool_result, history, ...)
atl gain --by-pipeline    # which pipeline
```

These combine — passing several renders several sections in one report.

At the proxy, what `proxy.request` rules remove is counted under the `rules` method and credited to the rule that rewrote each tool result. Rules chained with `continue = true` run as one chain and share one entry (`first + second`). The default report lists the largest of them under `via rules`, and `--by-rule` lists them all:

```
Trimmed in flight: 1.7K removed across the 1 req it touched (per-mechanism ratios below)
  via rules:       1.7K
    web-pages:     1.7K
```

### One pipeline in detail

`--pipeline <NAME>` slices a single pipeline by where its savings actually land: which tool outputs the hooks trimmed, and which methods compressed billed input.

```bash
atl gain --pipeline my-pipeline
```

### Shadow rules

Shadow rules are recorded but never emitted — they let you measure a rule's effect before trusting it with real output. `--shadow` reports what they *would* have saved:

```bash
atl gain --shadow
```

A shadow rule showing strong savings is a candidate to promote; one showing nothing is a rule you can delete.

:::note[Full edition only]
Rule, pipeline and shadow attribution describe the compression engine, which the Apache-2.0 edition does not include. See [Editions and Licensing](../resources/editions.md).
:::

## Recall efficiency

`--recalls` measures how often your AI assistant goes back for output a filter elided — the signal that a filter's cap is too aggressive for your workflow. Every time a filter stores elided output (an *elision*) and every time the assistant retrieves it (a *recall*), ATL counts it per filter:

```bash
atl gain --recalls
```

Illustrative output:

```
Recall efficiency  (current mode: sqlite)

SQLITE (exact — reads go through atl recall)
FILTER                   ELISIONS RECALLED   RATE
vitest                         50       20    40%
docker-images                 100        2     2%

TEE (approximate — bash-observed reads only)
FILTER                   ELISIONS RECALLED   RATE
cargo_test                     40        4   ≥10%
```

How to read it:

- **RATE** is the share of elided outputs the assistant went back for. Re-reading the same entry counts once — the rate measures *entries consulted*, not read commands.
- A **high rate** means that filter regularly hides output the assistant needs: every recall is an extra API round-trip you paid for. Consider raising that filter's cap.
- A **low rate** means the filter's cap is well calibrated — the elided output was noise.

The two sections are never merged because the data quality differs:

- **SQLITE (exact)**: reads go through `atl recall <hash>`, the only access path, so the count is exact.
- **TEE (approximate)**: in legacy [tee mode](../getting-started/configuration.md#recall-system), reads are shell commands (`tail`, `cat`, `grep`, …) observed by the rewrite hook before execution. Editor or assistant file-tool reads are invisible, denied commands are not counted, and the per-file dedup window is finite — so the `≥` rate is approximate, not exact. A high rate is still a reliable signal that the cap is too aggressive.

Stats survive entry eviction and retention cleanup: calibration data is kept even after the underlying outputs are purged.

## Troubleshooting

**No data showing:**
```bash
ls -lh ~/.local/share/atl/history.db
sqlite3 ~/.local/share/atl/history.db "SELECT COUNT(*) FROM commands"
git status    # run any tracked command to generate data
```

**Incorrect statistics:** Token estimation is a heuristic. For precise counts, use `tiktoken`:
```bash
pip install tiktoken
git status > output.txt
python -c "
import tiktoken
enc = tiktoken.get_encoding('cl100k_base')
print(len(enc.encode(open('output.txt').read())), 'actual tokens')
"
```
