---
title: Troubleshooting
description: Common ATL issues and how to fix them
sidebar:
  order: 3
---

# Troubleshooting

## `atl gain` says "not a atl command"

**Symptom:**
```bash
$ atl gain
atl: 'gain' is not a atl command. See 'atl --help'.
```

**Cause:** Another program named `atl` comes first on your `PATH`.

**Fix:** find it with `command -v atl`. Inside Anthology, the plugin's own `atl` must be the one that runs: run `anthology doctor`, which warns when it is not.

## AI assistant not using ATL

**Symptom:** Claude Code (or another agent) runs `cargo test` instead of `atl cargo test`.

**Checklist:**

1. Verify ATL is installed:
   ```bash
   atl --version
   atl gain
   ```

2. Initialize the hook:
   ```bash
   atl init --global    # Claude Code
   atl init --global --agent cursor    # Cursor
   atl init --global --opencode  # OpenCode
   ```

3. Restart your AI assistant.

4. Verify hook status:
   ```bash
   atl init --show
   ```

5. Check `settings.json` has the hook registered (Claude Code):
   ```bash
   cat ~/.claude/settings.json | grep atl
   ```

## ATL not found after `cargo install`

**Symptom:**
```bash
$ atl --version
zsh: command not found: atl
```

**Cause:** `~/.cargo/bin` is not in your PATH.

**Fix:**

For bash (`~/.bashrc`) or zsh (`~/.zshrc`):
```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

For fish (`~/.config/fish/config.fish`):
```fish
set -gx PATH $HOME/.cargo/bin $PATH
```

Then reload:
```bash
source ~/.zshrc    # or ~/.bashrc
atl --version
```

## ATL on Windows

### Double-clicking atl.exe does nothing

**Symptom:** You double-click `atl.exe`, a terminal flashes and closes instantly.

**Cause:** ATL is a command-line tool. With no arguments, it prints usage and exits. The console window opens and closes before you can read anything.

**Fix:** Open a terminal first, then run ATL from there:
- Press `Win+R`, type `cmd`, press Enter
- Or open PowerShell or Windows Terminal
- Then run: `atl --version`

### Hook not working (no auto-rewrite)

**Symptom:** On native Windows, commands are not auto-rewritten. An older `atl init -g` printed "Falling back to --claude-md mode".

**Cause:** Before v0.37.2, `atl init -g` registered no hook on native Windows: it fell back to injecting the full ATL instructions into `~/.claude/CLAUDE.md`. A setup made then still has no hook.

**Fix:** Upgrade to v0.37.2 or later, where `atl init -g` registers the auto-rewrite hook on Windows as a native binary command (`atl hook claude`). No Unix shell, bash, or jq is required. Re-run `atl init -g`: it replaces the CLAUDE.md block with an `@ATL.md` reference and, once you confirm, adds `atl hook claude` to `settings.json`. A legacy `~/.claude/hooks/atl-rewrite.sh` hook, if one exists, is deleted along with its `.atl-hook.sha256` and its `settings.json` entry.

Answer `y` when it asks to patch `settings.json`. Outside a terminal it cannot ask and defaults to `N`, so use `--auto-patch` there:

```powershell
atl init -g                # answer y at the settings.json prompt
atl init -g --auto-patch   # or: patch settings.json without asking
```

The default `N` leaves no hook registered: any legacy `atl-rewrite.sh` entry is removed and `atl hook claude` is not added. The `ATL hook registered (global).` banner prints either way; the line that confirms the patch is `settings.json: hook added` (or `settings.json: hook already present` when an earlier run added it). Restart Claude Code, then confirm:

```powershell
atl init --show
```

It should report `[ok] Hook: atl hook claude (native binary command)`.

[WSL](https://learn.microsoft.com/en-us/windows/wsl/install) also works and behaves like Linux if you prefer it.

### Node.js tools not found

**Symptom:**
```
atl vitest --run
Error: program not found
```

**Cause:** On Windows, Node.js tools are installed as `.CMD`/`.BAT` wrappers. Older ATL versions couldn't find them.

**Fix:** Update to ATL v0.23.1+ (every Anthology release is newer). Update the Anthology plugin, then check:
```bash
atl --version    # should be 0.23.1+
```

## Compilation error during installation

```bash
rustup update stable
rustup default stable
cargo clean
cargo build --release
cargo install --path . --force
```

Minimum required Rust version: 1.91 (edition 2024 needs Cargo 1.85 or newer).

## OpenCode not using ATL

```bash
atl init --global --opencode
# restart OpenCode
atl init --show    # should show "OpenCode: plugin installed"
```

The plugin needs OpenCode 1.18.29 or newer. With an older version, `atl init`
installs nothing and prints the version it found, when it can read
`opencode --version`. Upgrade OpenCode rather than looking for a rewrite
setting.

If the plugin loads but nothing changes, check that `atl` is on the `PATH`
OpenCode itself sees. The plugin only resolves it from `PATH`, because
OpenCode's shell tool does not source `.profile`, `.bashrc` or `.bash_profile`
and a binary found elsewhere cannot be spawned as a bare `atl`. Set
`ATL_DISABLED=1` to confirm which way round the behaviour goes.

## Does ATL break Claude's prompt cache?

No. ATL filters command output once, at execution time. The filtered result is written into the
conversation history and never changes afterwards, and prompt caching matches on a stable prefix
— ATL does not rewrite anything the cache has already seen.

Smaller tool results also make caching cheaper: cache writes bill at 1.25x and cache reads at
0.1x the input rate, so fewer tokens in means less to write once and less to re-read every turn.

To see your own cache write and read volumes next to ATL's savings:

```bash
atl cc-economics
```

`atl gain` reports token savings only; the cache breakdown lives in `atl cc-economics`.

## Network calls fail behind a proxy

ATL's own HTTP calls (`atl pipeline add <url>`, `atl pipeline search`, task workers with a remote upstream, the telemetry ping) follow `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and `NO_PROXY`. Filtered commands and the hook make no network call.

If one of these calls reports `Connection refused` while the target is reachable, check the proxy variables in that shell:

```bash
env | grep -i _proxy
```

- A stale or unreachable proxy makes the call fail. The error names the target, not the proxy.
- SOCKS proxies are not supported. `socks5h://` fails the call; `socks5://` and `socks4://` are ignored and the call connects directly.
- `localhost` and loopback addresses always connect directly.
- To bypass the proxy for one host, add it to `NO_PROXY` as the exact host name or as a `.suffix`, with no spaces after commas:

```bash
NO_PROXY=internal.example.com,.corp.example.com atl pipeline add https://internal.example.com/p.toml
```

## Run the diagnostic script

From the ATL repository root:

```bash
bash scripts/check-installation.sh
```

Checks:
- ATL installed and in PATH
- Correct version (Token Layer, not Type Kit)
- Available features
- Claude Code integration
- Hook status

## Still stuck?

Open an issue: https://github.com/rtk-ai/rtk/issues
