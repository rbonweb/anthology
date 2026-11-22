# Claude Code Hooks

> Part of [`hooks/`](../README.md)

## Specifics

- Shell-based `PreToolUse` hook -- requires `jq` for JSON parsing
- Returns `updatedInput` JSON for transparent command rewrite (agent doesn't know ATL is involved)
- Exits silently (exit 0) on any failure: jq missing, atl missing, atl too old (< 0.23.0), no match
- Version guard checks `atl --version` against minimum 0.23.0
- `../atl-awareness.md` (or `-high` / `-full`, per `awareness.level` in config.toml) is the instructions file embedded into CLAUDE.md by `atl init`
