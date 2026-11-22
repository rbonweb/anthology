# ATL

Prefix external programs with `atl`: `atl git status`, `atl cargo test`,
`atl npm run build`, `atl ls src/`; keep prefixes inside chains such as `atl git add . &&
atl git commit -m "msg"`. Programs without an ATL filter run as-is, so prefixing from `PATH`
is safe. Do not prefix shell builtins or keywords: `test`, `[`, `read`, `cd`, `echo`, `export`,
`eval`, `source`, `printf`, or shadowed `env`; run the native command directly when needed.

# Command output

Command output here is condensed to save tokens, keeping every signal and dropping costly noise. Treat it as the complete result: run commands normally, and batch related commands into one call to avoid extra turns. Truncated results state their recovery path in their own output.

Exact text stays exact: file contents (`cat`, `head`, `sed -n`), `grep -o` and `grep -c` results, `jq -r` values, and `curl -o` / `-w` results come back unchanged, and in-place edits such as `sed -i` are never rewritten. Do not add `ATL_DISABLED=1` or `atl bypass` to get exact text. Re-run a command as `atl bypass <cmd>` only when its result is unusable: empty when output was clearly expected, contradicting its exit code, or garbled.

## About ATL

ATL (Anthology Token Layer) is a CLI proxy that filters command output to save tokens; behavior and exit code are unchanged.

- `atl gain` / `atl gain --history` — token savings, overall and per command.
- `atl bypass <cmd>` — run a command unfiltered, still tracked.
- `ATL_DISABLED=1 <cmd>` — skip ATL for one command.
- `atl discover` — find past commands ATL could have condensed.
