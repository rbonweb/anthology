# Command output

Command output here is condensed to save tokens, keeping every signal and dropping costly noise. Treat it as the complete result: run commands normally, and batch related commands into one call to avoid extra turns. Truncated results state their recovery path in their own output.

Exact text stays exact: file contents (`cat`, `head`, `sed -n`), `grep -o` and `grep -c` results, `jq -r` values, and `curl -o` / `-w` results come back unchanged, and in-place edits such as `sed -i` are never rewritten. Do not add `ATL_DISABLED=1` or `atl bypass` to get exact text. Re-run a command as `atl bypass <cmd>` only when its result is unusable: empty when output was clearly expected, contradicting its exit code, or garbled.

## About ATL

The condensing is done by ATL (Anthology Token Layer), a CLI proxy. A hook
rewrites each shell command to `atl <cmd>` before it runs; behavior and
exit code are unchanged, only the output is filtered. Commands ATL has no
filter for run as-is.

- `atl gain` / `atl gain --history` — token savings, overall and per command.
- `atl bypass <cmd>` — run a command unfiltered, still tracked.
- `ATL_DISABLED=1 <cmd>` — skip the hook for one command.
- `atl discover` — find past commands ATL could have condensed.
