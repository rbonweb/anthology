# The `anthology` binary

This page covers the program that `anthology` and `anthology-guardrails` run, how it gets onto your machine, and what to do when it cannot be installed.

## What it is

Both plugins run one static program, `anthology`, written in Rust. It needs no language runtime on your machine.

## Where it comes from

- **Built in the open.** This repo's `anthology-release.yml` workflow builds and tests it on native runners and publishes it on the GitHub release tagged `anthology-bin--v<version>`: one `anthology-<target>.tar.gz` for each of macOS and Linux on arm64 and x86_64, plus `checksums.txt`. Each archive has a build-provenance attestation: `gh attestation verify anthology-<target>.tar.gz --repo rbonweb/anthology`.
- **Pinned.** Each of the two plugins ships the same `anthology.lock`: the version, the release tag and a sha256 for each target's archive. It never means "latest". A new binary arrives only with a plugin update, which installs the new version beside the old one.
- **Installed on first use.** At session start (or at the first tool call that needs it), the plugin's `sh` hook:
  1. downloads the archive for your platform over HTTPS with `curl`, `wget` or `node`;
  2. checks its sha256 against `anthology.lock` before unpacking it;
  3. checks that the binary runs and reports the pinned version;
  4. installs it in `~/.claude/anthology/bin/<version>/` (under `$CLAUDE_CONFIG_DIR` when that is set).

  Each session start re-checks the installed binary against the lock. A version that no lock pins and that has not been used for 30 days is deleted.
- **Offline or air-gapped.** On a machine with access, download the `anthology-<target>.tar.gz` that `anthology.lock` names (the URL is `https://github.com/rbonweb/anthology/releases/download/<tag>/anthology-<target>.tar.gz`), copy it into `~/.claude/anthology/bin/` and start a new session. It is checked against the same pin. A file that does not match is not used, and the message says so.

`/anthology:setup` makes the same install, so you can also run it to retry (see [What setup does](setup.md#what-anthologysetup-does)).

## If it cannot be installed

The guardrails fail closed. Read-only tools still work (`Read`, `Grep`, `Glob`, `WebFetch`, `WebSearch`, `Skill`, `Task`, `Agent`, the to-do and plan tools and the like), so `/anthology:setup` can still load. Shell commands, file writes and edits, MCP tools and any tool not on that list are blocked until the binary is installed. A blocked call tries the install again at most once a minute, so the session heals once the network is back. Until the binary is installed, the `anthology` plugin's routing guide, memory and ATL are off.

**A slow network.** The guardrails download the archive in the background, with a budget of 10 minutes, starting at session start. A tool call waits up to 20 seconds for it and is then blocked with how many bytes have arrived so far; the download goes on, and a later call installs it. A download that stops (the session ends, the machine sleeps, the 10 minutes run out) keeps what it got, and the next attempt continues from there when `curl` is the downloader.

**The installer is always allowed.** While the binary is missing, the guard lets exactly one shell command through: this plugin's own installer, which every `(setup)` block names with the real folder, for example `/bin/sh '/home/user/.claude/plugins/cache/anthology/anthology-guardrails/<version>/hooks/bootstrap.sh' guard --plugin guardrails --quiet --budget 300`. It may also end in `--quiet`, `--budget N` or nothing after `guardrails`. It waits for the download up to the budget, installs the binary and prints `status=installed`. Anything else on that line (another path, a symlink to it, `;`, `&&`, `$(…)`, a redirection, one more space) is blocked like every other command.

What you may see, and what to do (the text in parentheses names the cause):

| The message says | What to do |
|---|---|
| At session start: `anthology-guardrails: could not install the guard binary (…)` or `anthology: could not install the anthology binary (…)` | Start a new session while online, or run the installer command the `anthology-guardrails` message names. The `anthology` message says that the guard, which runs the same binary, blocks shell commands, file edits and MCP tools too, and has no command of its own, because the guard lets through only the guardrails' installer |
| `Blocked by anthology-guardrails (setup): the guard binary is not installed (…)` | Run the installer command the message names. Starting a new session while online, or waiting a minute and retrying, also works |
| `… is still downloading in the background (N bytes so far)` | A slow network: retry in a few seconds, or run the installer command, which waits for the download |
| `… download timed out (slow network)` | Retry: the next try continues the download. Or run the installer command |
| `… could not be downloaded (no network: …)` | Connect, then retry, or run the installer command |
| `… does not match anthology.lock (sha256 mismatch)` | The download was not what the lock pins (a proxy that changes downloads, or a broken release). Nothing was installed. Retry on another network, or report it |
| `… the last install attempt was less than a minute ago` | Retry in a minute, or run the installer command |
| `… did not work (…) and was set aside` | Nothing: it is reinstalled on a later call (at most once a minute) |
| `… failed to run 3 times in 24 hours` | A new session tries once more, at most once an hour. Or disable `anthology-guardrails` until a fixed version ships |
| `… does not run on this machine` | A wrong architecture, or a home folder mounted `noexec`. Disable `anthology-guardrails`, fix the cause, start a new session |
| `… no downloader (curl, wget or node) was found` | Install one, or download the archive by hand: the message gives the URL and the folder |
| `… cannot be installed because <folder> is not writable` | Fix that folder's permissions, then start a new session |
| `… there is no guard binary for this platform` | Anthology supports macOS and Linux on x86_64 and arm64 only. Disable `anthology-guardrails` to use Claude Code here |
| `Blocked by anthology-guardrails (internal): …` | The guard itself failed or ran out of time on this input, and blocked it to be safe. Rephrase the command, or report it |

**The way out that always works:** run `/plugin` and disable `anthology-guardrails`. `/plugin` never goes through the guard, so it works even when every tool call is blocked, and every `(setup)` and `(internal)` message names it. Disabling it turns all the guardrails off, so turn it back on once the cause is fixed.

## Removing it

`/anthology:setup remove` keeps the binaries in `~/.claude/anthology/bin/` while `anthology-guardrails` is still installed, because its guard needs them. After you uninstall the plugins, delete that folder yourself (about 6 MB per version).
