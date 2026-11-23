---
title: Embedding atl in your own agent harness
description: Call atl rewrite from your own bash tool instead of installing a shipped hook — the exit-code contract, where the call goes, fail-open rules, and per-session accounting
sidebar:
  order: 5
---

# Embedding atl in your own agent harness

This page is for people who build an agent harness and want the rewrite step inside their own bash tool, rather than installing one of the hooks listed in [Supported Agents](supported-agents.md). The Pi and Hermes plugins and the legacy shell hooks are thin delegates around one binary call, `atl rewrite`; the Rust-binary hooks that `atl init` installs today (`atl hook claude`, `atl hook opencode` and friends) run the same registry in-process. For a host of your own, `atl rewrite` is the whole integration surface. Everything below was verified against atl 1.2.0.

## What `atl rewrite` returns

`atl rewrite -- "<command>"` prints the rewritten command to stdout (no trailing newline) and reports its decision in the exit code. Pass `--` before the command: without it, a command string that reads as an option of `atl rewrite` itself (`--help`, `-h`) prints atl's help on stdout with exit 0. The contract is defined in [`src/hooks/rewrite_cmd.rs`](https://github.com/rtk-ai/rtk/blob/master/src/hooks/rewrite_cmd.rs):

| Exit | Stdout | Meaning | What your host should do |
|------|--------|---------|--------------------------|
| 0 | rewritten command | Rewrite, and Claude Code allow rules cover every command in it | Run the rewritten command |
| 1 | empty | No ATL equivalent, or a Claude Code deny rule matched inside a shell wrapper's script (`bash -c '…'`) | Run the original command |
| 2 | empty | A Claude Code deny rule matched | Your call. The rule comes from Claude Code's settings, so the usual choice is to run the original command and let your own policy decide |
| 3 | rewritten command | Rewrite, with an "ask" rule matched, allow rules covering only part of the command, a shell wrapper (`bash -c '…'`), a settings file that cannot be parsed, or no rule matched at all | Run the rewritten command |

The exit code carries a permission verdict read from Claude Code's settings files (`settings.json` and `settings.local.json` in the nearest `.claude/` directory above the working directory, and in `~/.claude`, or in `$CLAUDE_CONFIG_DIR` when it is set to a non-empty path; for a project under your home with no `.claude/` of its own, the nearest one is usually `~/.claude` itself). Outside Claude Code those files usually do not exist, so no rule matches, the verdict is `Default`, and **exit 3 is the ordinary answer for every rewritable command**:

```bash
$ atl rewrite -- "git status"; echo " -> exit $?"
atl git status -> exit 3
$ atl rewrite -- "htop"; echo " -> exit $?"
 -> exit 1
```

A host that treats 3 as anything other than 0 has ATL silently switched off. Treat 0 and 3 identically: when stdout is non-empty, that is the command to run. On a machine that also has Claude Code installed, its allow and deny rules show up as 0 and 2, and an `ATL_REWRITE_HOST=openclaw` inherited from the environment turns the ordinary 3 into 0; treating 0 and 3 alike keeps your host indifferent to both.

Three more details of the call:

- **Read stdout only.** Stderr can carry advisory lines: a once-a-day "No hook installed" notice on machines where the Claude Code config directory (`~/.claude`, or `$CLAUDE_CONFIG_DIR`) exists without an ATL hook, or an `ATL_DISABLED=1 detected` message.
- **Compare output with input.** A command that already starts with `atl` comes back unchanged (still exit 3), and there is nothing to substitute.
- **Check `ATL_DISABLED` yourself before spawning `atl rewrite`**, as the Pi extension does. Releases up to 1.2.0 honor it only as an in-command prefix (`atl rewrite -- "ATL_DISABLED=1 git status"` exits 1); [rtk-ai/rtk#3917](https://github.com/rtk-ai/rtk/pull/3917) makes `atl rewrite` honor the exported variable as well.

`atl rewrite` does not run the command and does not open the savings database. Compound commands are handled (`cargo fmt --all && cargo test` becomes `atl cargo fmt --all && atl cargo test`); commands containing a file redirect (`> out.txt`) or command substitution pass through with exit 1; a file-descriptor duplication such as `2>&1` does not block the rewrite.

## Where the call goes

Run the rewrite **after your own approval gate and before you spawn the shell**:

```text
agent asks for "git status"
  -> your policy check or permission prompt sees "git status"
  -> atl rewrite -- "git status"     prints "atl git status", exit 3
  -> the shell runs "atl git status"
  -> the model reads the filtered output
```

A person approving a command then sees the command the agent asked for, not the rewritten one, and allow or deny patterns written for `git ...` keep matching. If your transcript shows the executed command, show the rewritten form there so the substitution stays visible.

## Rewrites that are more than a prefix

The exit-code contract says nothing about the shape of the rewritten command. Most rewrites only insert `atl` in front of a command segment (`git log -n 3` becomes `atl git log -n 3`, `FOO=1 git status` becomes `FOO=1 atl git status`, `cd x && git status` becomes `cd x && atl git status`), but some rules rename the command or reorder its arguments. On atl 1.2.0:

| Command | `atl rewrite` output |
|---------|----------------------|
| `cat foo.txt` | `atl read foo.txt` |
| `head -n 5 foo.txt` | `atl read foo.txt --head-lines 5` |
| `python -m pytest -x` | `atl pytest -x` |
| `npm run lint` | `atl lint` |
| `yadm status` | `atl git status` |

If your transcript shows the rewritten command, nothing more is needed. If your audit log or permission prompt records only the command the agent asked for and what runs must match it token for token, accept only rewrites that are pure insertions and run the original command otherwise:

- split both strings on whitespace and shell operators (`&&`, `||`, `|`, `;`, `&`), keeping each quoted span, quotes included, as one token;
- walk the rewritten tokens against the original left to right, preferring a literal match, so an argument that is itself `atl` (`git log --grep atl`) counts as data, not as an inserted prefix;
- every unmatched rewritten token must be exactly `atl`;
- the walk must consume the original exactly, with no token dropped, added or requoted.

The cost is that commands like the ones above run unfiltered. [ismoss/dsh-atl](https://github.com/ismoss/dsh-atl) is an external example of this check (`isPrefixInsertion` in `lib/index.js`, negative cases in `test/invariant.test.mjs`).

## Fail open

The command must always run, with or without atl. Run the original command unchanged when:

- `atl` is not on `PATH`
- `atl --version` reports a version older than 0.25.0, the release that added `atl rewrite` (the Pi extension and the legacy shell hooks check for 0.23.0; on 0.23.0 and 0.24.0 the call exits 2 with nothing on stdout, one more reason to run the original on exit 2 rather than block)
- `atl rewrite` has not answered within about 2 seconds (the Pi extension uses a 2 s timeout, the Hermes plugin 5 s by default; a rewrite normally answers in under 20 ms)
- it prints nothing to stdout
- it exits with any code other than 0 or 3, or crashes
- spawning it fails with `E2BIG` because the command is very long (Linux refuses a single argument of 32 pages or more, 128 KiB with 4 KiB pages; `ARG_MAX` is 1 MiB on macOS)

Probe `atl --version` once per host process and remember the answer; do not re-probe on every command. The reference implementation is the Pi extension, [`hooks/pi/atl.ts`](https://github.com/rtk-ai/rtk/blob/master/hooks/pi/atl.ts): about 140 lines covering the version probe, the timeout, and the 0/3 handling.

## A minimal sketch

```python
import functools
import os
import shutil
import subprocess

TIMEOUT_S = 2


@functools.cache
def atl_ready() -> bool:
    """Probe once per process and remember the result."""
    if shutil.which("atl") is None:
        return False
    try:
        out = subprocess.run(["atl", "--version"], capture_output=True, text=True,
                             timeout=TIMEOUT_S).stdout          # "atl 1.2.0"
        major, minor, _ = (int(x) for x in out.split()[1].split("."))
        return (major, minor) >= (0, 25)
    except (OSError, subprocess.TimeoutExpired, ValueError, IndexError):
        return False


def rewrite(command: str) -> str:
    """Return the command to spawn. Every failure returns the input unchanged."""
    if os.environ.get("ATL_DISABLED") == "1" or not atl_ready():
        return command
    try:
        r = subprocess.run(["atl", "rewrite", "--", command], capture_output=True,
                           text=True, timeout=TIMEOUT_S)
    # missing binary, E2BIG, too slow, a NUL in the command
    except (OSError, subprocess.TimeoutExpired, ValueError):
        return command
    if r.returncode in (0, 3) and r.stdout.strip():
        return r.stdout.strip()
    return command                   # 1: no filter or wrapped deny, 2: deny, other
```

Call `rewrite()` on the approved command string and hand the result to the same shell you would have used anyway. `atl` has to be on the `PATH` of that shell too.

## Accounting per invocation

Every filtered command records its raw and filtered sizes in a SQLite history that `atl gain` reads. The default file is shared by every project and session run under the same account, so a host that runs several sessions at once cannot attribute savings from it. Give each command its own database instead: point `ATL_DB_PATH` at a fresh temporary path, read the totals once when the command finishes, then delete the file.

```bash
export ATL_DB_PATH=/tmp/session-42/cmd-7.db   # a path that does not exist yet
atl git status                                # the filtered command creates the file
atl gain --format json                        # totals for that file only
rm "$ATL_DB_PATH"
```

`atl gain --format json` against a path that does not exist yet creates it and returns zeros. Without `--all` the document holds only a `summary` object, with these fields (this one is after a single `atl git status` in a clean checkout):

```json
{
  "summary": {
    "total_commands": 1,
    "total_input": 14,
    "total_output": 9,
    "total_saved": 5,
    "avg_savings_pct": 35.714285714285715,
    "total_time_ms": 6,
    "avg_time_ms": 6,
    "oversized_excluded": 0
  }
}
```

`total_input`, `total_output`, and `total_saved` are estimated tokens (`bytes / 4`); `total_time_ms` and `avg_time_ms` are execution time in milliseconds.

The totals leave out any command whose raw output was above 7,500 estimated tokens, that is 30,000 bytes. The cap stands in for Claude Code's default Bash output limit of 30,000 characters, past which the agent would not have read the output without atl either; it counts bytes, so non-ASCII output reaches it sooner. Such a command only increments `oversized_excluded`. With one database per command, a large output therefore reads `"total_commands": 0` and `"oversized_excluded": 1`, with every total at zero: check `oversized_excluded` before recording a zero.

`ATL_DB_PATH` takes precedence over `tracking.database_path` in `config.toml`, and an empty pre-created file works as well as a missing one. The variable has no effect on `atl rewrite`, which never opens the savings database.

## Sandbox environment

Set `ATL_TELEMETRY_DISABLED=1` in the environment of every atl process. Telemetry is opt-in (it requires consent given through `atl init` or `atl telemetry enable`), so a fresh sandbox sends nothing anyway; the variable states the intent and holds if a shared config ever turns it on. See [Telemetry & Privacy](../resources/telemetry.md).

## Escape hatch and exclusions

- `atl bypass <cmd>` runs a command with raw, unfiltered output (`atl proxy`, its name before 1.0.0, still works). It is still recorded in the history, at zero savings. Use it when the agent needs the complete output of a command ATL would otherwise condense.
- Commands that must never be rewritten go in `[hooks] exclude_commands` in `config.toml`; `atl rewrite` answers exit 1 for a matching command. See [Excluding commands from auto-rewrite](configuration.md#excluding-commands-from-auto-rewrite) for the matching rules.

```toml
[hooks]
exclude_commands = ["git push", "psql"]
```

## What the numbers mean

The figures from `atl gain` measure bash output bytes, converted to tokens by estimate. They are not a bill, and they do not translate into cost at the same rate. Read [How ATL Savings Work](../resources/savings-explained.md) before putting them on a dashboard.

## Verified against

atl 1.2.0 on Linux (first written against 0.48.0 on macOS). Exit codes come from `src/hooks/rewrite_cmd.rs` and `hooks/pi/atl.ts`, the `atl gain --format json` field names from `ExportSummary` in `src/analytics/gain.rs` (mirroring `GainSummary` in `src/core/tracking.rs`), and every command above was run against that binary, except the 0.23.0 and 0.24.0 behaviour, which comes from those release binaries.
