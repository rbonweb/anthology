# ATL (Anthology Token Layer)

ATL is a command-line proxy that cuts the command output an AI coding agent reads. It runs a
command, compresses what the command printed (failing tests with their assertions instead of
every passing test, a three-line `git status` instead of forty), and passes the exit code
through. It never prints more than the command did, and when a filter fails the agent gets the
raw output.

ATL is part of [Anthology](https://github.com/rbonweb/anthology), a Claude Code plugin set. It is
a fork of rtk by RTK AI Labs (https://github.com/rtk-ai/rtk), licensed under the Apache License
2.0. See [Fork and credit](#fork-and-credit).

## Install

Anthology installs ATL for you. Its session-start hook downloads the pinned `atl` binary from the
Anthology releases, checks its checksum, and places it under the plugin's data folder; the
`anthology setup` skill then allows `Bash(atl *)` and wires the hook. You do not install ATL
by hand.

To build it from source, from this folder (its own Cargo workspace, which needs `../crates/anthology-shell`):

```bash
cargo build --release
target/release/atl --version
```

## Use

```bash
atl git status                  # a compact status
atl cargo test                  # failures only, with their assertions
atl read src/main.rs            # a file, with the noise cut
atl bypass git log --oneline    # any command, unfiltered, still counted
atl gain                        # what ATL saved, today and overall
atl discover                    # commands that ran without ATL, and what they would have saved
atl --help                      # every command
```

Token counts are estimated as `bytes / 4`, so the percentages are reliable and the absolute
numbers are approximate. A 90% cut in command output is not a 90% cut in cost: command output is
one part of the input tokens.

Project-specific filters go in `.atl/filters.toml`. ATL reads that file only after you run
`atl trust` in the project, and the trust is tied to the file's content.

Folders worth knowing: [`src/cmds`](src/cmds/README.md) holds one filter per command,
[`src/hooks`](src/hooks/README.md) holds the hook and the Anthology gate, and
[`docs`](docs/guide/index.md) holds the user guide inherited from upstream.

## What changed from upstream

- Renamed to ATL: the binary, the crates, the environment variables (`ATL_*`), the project
  folder (`.atl/`) and every message.
- Trimmed to the Apache-2.0 edition: the BUSL-1.1 engine, proxy and plugin crates are gone, and
  so is everything that only they used.
- The Anthology gate: ATL's hook decides from the shared shell parser in
  [`../crates/anthology-shell`](../crates/anthology-shell) which commands it may rewrite and
  allow, and it fails closed.
- The fork's own changes (auto-allow hook, minimal `read`, Node and Python filters, `cd` chains,
  lint scripts, OpenCode `run`, the exact awareness text) each have a named test in
  [`tests/`](tests) (`fork_*`).

## Fork and credit

ATL is a fork of rtk by RTK AI Labs, https://github.com/rtk-ai/rtk, Copyright 2026 RTK AI Labs,
under the Apache License 2.0. The fork follows upstream releases: each upstream tag is merged
with `scripts/sync-atl.sh`, which renames the upstream tree to ATL before it merges, so
upstream's own history and tests stay the base of this code. The work that makes up ATL, the
renaming, the trimming and the gate, is by the Anthology contributors.

- [LICENSE](LICENSE) is the Apache License 2.0, as upstream ships it.
- [NOTICE](NOTICE) says that this is a modified version of the upstream project (a fork of rtk) and who changed it. It ships in
  every ATL release archive next to the binary and LICENSE.
- The authors and the copyright of the upstream code stay as they are in `Cargo.toml` and in
  every source file.
- The Apache License does not give anyone the right to use the upstream project's name, so this
  fork does not use it as its own name. That is why it is called ATL.

Upstream's own bug reports and feature requests belong at the upstream project. Problems with
ATL belong at https://github.com/rbonweb/anthology/issues.

## Disclaimer

See [DISCLAIMER.md](DISCLAIMER.md).
