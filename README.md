# Anthology

Anthology is a plugin marketplace for Claude Code. It gives every project a roster of agents with fixed models and their own knowledge, guardrails against browser tooling and secrets, condensed command output, and memory that lives in the repo.

It is opinionated and preconfigured: install it and follow its rules. There are no modes, levels or toggles to set. macOS and Linux only.

## What you get

Three plugins. Installing `anthology` brings the other two with it.

- **`anthology`**
  - An agent roster: Opus researches, plans and reviews, Sonnet builds, Haiku scouts. Each agent reads its own knowledge, and a routing guide tells the main session when to delegate and to whom. See [Agents](docs/agents.md).
  - Memory as Markdown notes committed with your project. It survives re-clones and team merges, with search, export and import. See [Memory](docs/memory.md).
  - ATL (Anthology Token Layer), which condenses the output of common dev commands to save tokens. It is a fork of rtk by RTK AI Labs. See [ATL](docs/atl.md).
  - Claude Code mods: strict delegation, an after-work check on agent reports and an `/anthology-status` pane. See [Mods](docs/mods.md).
  - Commits that carry only your name: no `Co-Authored-By` trailer and no "Generated with" line. See [Commit attribution](docs/guardrails.md#commits-carry-only-your-name).
  - The commands `/anthology:setup`, `/anthology:export`, `/anthology:import` and `/anthology-status`.
- **`anthology-guardrails`** blocks what agents must not do: install or launch a browser on your machine, touch your Chrome profile, use any browser other than BrowserOS Neo, or write secrets into memory. See [Guardrails](docs/guardrails.md).
- **`anthology-superpowers`** is the superpowers skills for the main session, renamed so they do not clash with the official copy.

Two things to read before you install:

- The guard stops accidental installs and launches by cooperative agents. It is not a sandbox. [Known limits](docs/guardrails.md#known-limits) lists what it misses.
- Setup allows `Bash(atl *)`, which also auto-approves `atl bypass <cmd>`, and that runs any command. [The `Bash(atl *)` risk](docs/atl.md#the-bashatl--risk) says what still protects you, and `/anthology:setup remove` takes the allow out again.

## Install

Run steps 1 and 2 in a shell, with the `claude plugin` commands. They work everywhere, including the VS Code extension, which does not support the `/plugin` slash commands. In the Claude Code terminal app you can type the `/plugin` form instead.

1. Add the marketplace:

   ```text
   claude plugin marketplace add rbonweb/anthology
   ```

   In the terminal app: `/plugin marketplace add rbonweb/anthology`.

2. Install the plugin. This also installs `anthology-guardrails` and `anthology-superpowers`:

   ```text
   claude plugin install anthology@anthology
   ```

   In the terminal app: `/plugin install anthology@anthology`.

3. Turn on auto-update, in the terminal app: run `/plugin`, open the **Marketplaces** tab, select `anthology` and enable auto-update. Auto-update is off by default for every marketplace that is not Anthropic's own, so without this step you only get new versions when you update by hand (see [Updating and removing](#updating-and-removing)). The VS Code extension has no such screen, so update by hand there.
4. Run `/anthology:setup` in each repo you want memory in. The first run also does the one-time machine setup. It checks your machine, shows every change and applies them only after you confirm once. Restart the session when it finishes. See [Setup](docs/setup.md).

The first session after you install downloads the plugins' `anthology` program. Until it is installed, the guardrails block shell commands, file edits and MCP tools. See [The `anthology` binary](docs/binary.md) for what to do if the download fails.

## Requirements

- macOS or Linux, on arm64 or x86_64. Setup refuses on Windows.
- Claude Code `2.1.330` or newer.
- For the one-time download of the `anthology` binary: network access to GitHub, one of `curl`, `wget` or `node`, one of `sha256sum`, `shasum` or `openssl`, and `tar`. macOS and most Linux systems have these. `node` is the last resort, and behind a proxy it works only on node 24 or newer. Offline, copy the archive in by hand (see [The `anthology` binary](docs/binary.md#where-it-comes-from)).
- Node 22 or newer.
- Docker, for tests, builds and end-to-end suites.
- BrowserOS Neo, for all browser work.
- git.

`/anthology:setup` checks that the `anthology` binary is installed, that `node`, Docker (installed and running) and BrowserOS Neo are present, and tells you what is missing. It does not check versions, git or the Claude Code version.

## Updating and removing

**Updating.** With auto-update on (install step 3), Claude Code updates the plugins after a session starts, and the new version loads after `/reload-plugins` or at your next launch. To update by hand, run `claude plugin marketplace update anthology`, then `claude plugin update anthology@anthology` (and the same for `anthology-guardrails` and `anthology-superpowers`), and restart Claude Code. See [Updating](docs/setup.md#updating).

**Removing.** Run `/anthology:setup remove`. It takes out the settings entries Anthology added and the downloaded ATL and its history, and never deletes your memory notes. Then uninstall the plugin:

```text
claude plugin uninstall anthology@anthology --prune -y
```

See [Removing](docs/setup.md#removing) for what it keeps and what to delete by hand.

## Documentation

- [Setup, updating and removing](docs/setup.md): what `/anthology:setup` changes, what happens at session start, replacing the official superpowers and design-skills.
- [Agents, commands and working rules](docs/agents.md): model roles, the roster, the commands and host mode.
- [Claude Code mods](docs/mods.md): strict delegation, the after-work check, the status pane, and the switches that turn them off.
- [Guardrails](docs/guardrails.md): what the guard blocks, its known limits, commit attribution and the browser rule.
- [ATL](docs/atl.md): condensed command output, local history and the `Bash(atl *)` risk.
- [Memory](docs/memory.md): approved repos, re-clones, team merges, CODEOWNERS, `anthology-mem`, export and import.
- [The `anthology` binary](docs/binary.md): where it comes from, how it is verified and what to do when it cannot be installed.

Want notifications? Anthology sends none; use [Coucou](https://github.com/Louis-CFM/coucou).

## Project

[CONTRIBUTING.md](CONTRIBUTING.md) · [SECURITY.md](SECURITY.md) · [CHANGELOG.md](CHANGELOG.md) · [DECISIONS.md](DECISIONS.md) · [LICENSE](LICENSE) (Apache-2.0). Each plugin ships its license text, and each vendored folder ships its upstream license (and NOTICE, where upstream has one).
