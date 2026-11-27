# ATL (Anthology Token Layer)

This page covers ATL, the tool that condenses command output, where it comes from, what it stores on your machine, how to upgrade from the legacy rtk name, and the risk that comes with the `Bash(atl *)` allow that setup adds.

## What ATL does

ATL condenses the output of common dev commands, so the main session and every agent spend fewer tokens on `git status`, test runs and the like. A `PreToolUse` hook on Bash runs `atl hook claude`, which rewrites commands (`git status` becomes `atl git status`).

- **Always on.** There are no options. Output is condensed, and the routing guide tells Claude to treat it as complete. Exact text stays exact for `cat`, `grep -o` and `grep -c`, `jq -r` and `curl -o` and `curl -w`. `atl bypass <cmd>` runs a command unfiltered, and the guide says to use it only when a result is unusable.
- **File reads stay as typed.** The hook does not rewrite `cat`, `head` or `tail` into `atl read`: on real use that saved about 3% of what was read, and it changes the text Claude reads. `/anthology:setup` shows, from ATL's [local history](#local-history), how many commands went through ATL and how many tokens it saved (`atl_savings`).
- **Fails safe.** If the binary is missing, the hook exits 0 and commands run unchanged. The `atl` command on your Bash path says "run /anthology:setup" if setup has not run.
- **Claude Code only.** The ATL source lives in this repo under `atl/`. It carries no integration for any other AI tool, and it has no telemetry. A CI test runs the built binary with the network switched off.

## Where it comes from

- **A fork of rtk.** ATL is a fork of rtk by RTK AI Labs ([github.com/rtk-ai/rtk](https://github.com/rtk-ai/rtk)), renamed and trimmed by Anthology. It was called rtk in earlier Anthology releases (see [Upgrading from the legacy rtk name](#upgrading-from-the-legacy-rtk-name)). `atl/LICENSE` is the upstream Apache-2.0 license, unchanged, and `atl/NOTICE` records the origin and the changes. The license and the NOTICE ship in every ATL archive.
- **Apache-2.0 edition.** Upstream (rtk-ai/rtk) also contains a BUSL-1.1 compression engine. Anthology removes it from the source it ships, so what you install is the Apache-2.0 edition. The engine-only features (compressing output after a tool has run, user-defined pipelines and `atl tasks`) are not part of it.
- **Verified binaries.** Setup downloads the binary for your platform from this repo's public GitHub releases (tag `atl--v<version>`) over HTTPS. `plugins/anthology/atl.lock` pins the version and a sha256 for each of the four targets (macOS and Linux, arm64 and x86_64), and the binary is installed only when the hash of the downloaded archive matches. Each release asset has a build-provenance attestation you can check with `gh attestation verify`. At every session start Anthology reconciles the installed binary with the lock.

## Local history

ATL keeps a SQLite history on your machine, never uploaded:

- every Bash command line the hook sees and the atl command it became, with token counts (for `atl gain`), for 90 days;
- for `atl recall`, the output it left out of a result, for 30 days.

Command lines can contain secrets (`TOKEN=…`, `Authorization:` headers). Anthology keeps the history in `~/.claude/anthology/atl/` (`history.db`, `recall.db`, `tee/`; files are private to you), and `/anthology:setup remove` deletes that folder.

To clear it sooner, delete the folder: that is the only way to remove the command lines from disk. `atl gain --reset --yes` only zeroes the `atl gain` numbers; the text of deleted rows can stay in the database file until it is overwritten. ATL has no switch that stops the history but keeps `atl gain`.

The first release, `rtk--v1.2.1-anthology.1`, wrote to upstream rtk's default folder instead (`~/Library/Application Support/rtk/` on macOS, `~/.local/share/rtk/` on Linux). Neither setup nor remove touches that folder, because another rtk install may use it, so delete it yourself if you ran that version.

## Upgrading from the legacy rtk name

Earlier releases installed this tool under its legacy name, rtk. There is no `rtk` alias any more. After the plugin updates:

- Session start installs ATL beside the legacy files (`<plugin data>/atl/`) and leaves the legacy rtk files alone.
- Until you run `/anthology:setup`, commands prompt as usual and settings still list the legacy `Bash(rtk *)` allow. Session start prints "anthology settings changed in this release — run /anthology:setup to review". A session that started before the update, with the legacy rtk hook still loaded, finds nothing to run and fails closed.
- `/anthology:setup` shows the legacy rtk leftovers first, then deletes them after you confirm: the `rtk_path` entry in `~/.claude/anthology/state.json`, the downloaded folder `<plugin data>/rtk/` and the old history `~/.claude/anthology/rtk/` (the command lines it saw, its recall store and tee spool). ATL starts with an empty history; nothing is migrated. The legacy allow rule that setup added is replaced by `Bash(atl *)`.
- A legacy `Bash(rtk *)` allow rule you wrote yourself is never touched.
- Upstream rtk's own folders (`~/.local/share/rtk`, `~/Library/Application Support/rtk` and a project's `.rtk/`) belong to any legacy or other rtk install and are never touched. Project filters live in `.atl/filters.toml` now and need `atl trust` again.

## The `Bash(atl *)` risk

Setup adds a blanket `Bash(atl *)` allow so rewritten commands do not prompt. That rule approves every line that starts with `atl`, and `atl bypass <cmd>` (and `atl proxy`, its old name), `atl run`, `atl err`, `atl test`, `atl summary` and `atl <program>` run any command. Since ATL 1.2.1-anthology.9 the hook follows one principle: **ATL never widens permissions.** A line gets no decision from the hook, so that `Bash(atl *)` approves it, only when Claude Code would have allowed the command anyway. This holds with or without any deny, ask or `Read(…)` rule.

- **A rewrite is made only for a command Claude Code would allow as typed.** `git status` becomes `atl git status` when `anthology perm classify` shows it read-only (every path it reads under the project: `CLAUDE_PROJECT_DIR`, else the git top level of the working directory, else the directory itself) or one of your own allow rules covers it, as Claude Code matches them: `Bash(git status)` is exactly that command, `Bash(git status:*)` and `Bash(git status *)` also take arguments, and a `*` is a wildcard. Anything else (`git push --force`, `ssh h rm -rf x`, `docker run -v /:/h …`, `find . -delete`, `npx pkg`, `cargo build` you did not allow) is left as you typed it, and Claude Code judges that. The blanket `Bash(atl *)` never counts as your allow. As in Claude Code, a `*` or `:*` rule never covers `watch`, `setsid`, `ionice`, `flock` or a `find` with `-exec`, `-execdir`, `-ok`, `-okdir`, `-delete`, `-fprint`, `-fprint0`, `-fprintf`, `-fls` or `-files0-from`. Allow rules count only from the project directory's own `.claude/settings{,.local}.json` (`CLAUDE_PROJECT_DIR`, else the git root of the working directory), your user settings and the managed settings, never from a parent folder's `.claude` (its deny and ask rules are still read). Without the plugin's `anthology` binary only your own allow rules remain.
- **A typed `atl …` line is judged as the command it runs.** `atl bypass`, `proxy`, `run`, `err`, `test`, `summary` and `atl <tool>` (`atl rm -rf x`, `atl sh -c …`, `atl find -delete`, `atl git push --force`) get no decision only when that command is read-only or one of your allow rules covers it; otherwise the hook asks, in every permission mode and in every shape (`atl`, a path to atl, `env atl …`, `FOO=1 atl …`, and inside `&&`, `||`, `;`, a pipe, a group, `sh -c '…'`, `xargs` or `find -exec`). `atl bypass git log -5` runs with no prompt; `atl bypass rm -rf x` asks. Only the plain spelling is proven read-only: the bare word `atl` (or the legacy `rtk`) at the start of a command, an unquoted command after it. ATL's own writing subcommands (`atl init -g`, `config --create`, `trust -y`, `learn --write-rules`, `gain --reset --yes`) ask unless you allowed them; `atl gain`, `atl gain --history` and `atl --version` do not.
- **A subcommand that is not plain text asks, always.** `atl {bypass,rm} -rf x`, `atl ${X:-bypass} …`, `atl $(echo bypass) …`, `X=bypass; atl $X …` and a glob or backquote in that position cannot be read, so the hook asks.
- **Auto mode.** Nothing is rewritten, so an ask is about the line as typed.

What still holds:

- **Your deny and ask rules.** ATL's hook reads the `permissions` of your settings files (user, project, local and managed `managed-settings.json`) and applies them to the command and to the command an `atl …` form wraps (`atl bypass`, `run`, `err`, `test`, `summary`, `atl git …`, and so on).
  - A matching deny rule blocks the call. A launcher the hook does not model, such as `caffeinate git push`, is left to Claude Code's own matching.
  - On a line that mentions `atl`, a matching ask rule makes Claude Code prompt, even under `Bash(atl *)`, in every mode except bypass mode, where the ask is dropped because you chose not to be asked. Auto mode keeps it, so `Bash(atl *)` cannot skip the classifier.
  - On other lines ATL's ask is dropped, because a hook ask would prompt in bypass and auto mode too. Only Claude Code's own, shallower matching applies there: it does not see `git -C . push`, `bash -c '…'` or `find -exec`. Use a deny rule for anything that must never run.
  - A `Read(…)` deny rule blocks `cat`, `head`, `tail`, `grep`, `rg` and `atl read` of a denied path.
  - **No needless ask on a read.** A line that only contains `atl` in a path (`grep -n x atl/src/main.rs`) is not a command that runs atl, so when `anthology perm classify` shows it read-only, no deny, ask or `Read(…)` rule concerns it and every path it reads is under the project, the hook hands back the plain rewrite with no decision of its own, so your `Bash(atl *)` rule decides (or leaves the line to Claude Code), instead of asking. The project is `CLAUDE_PROJECT_DIR`, else the git top level of the working directory, else the directory itself. A deny still wins, and an ask rule still asks. Without the plugin's `anthology` binary the ask stays.
  - **Auto mode.** ATL rewrites nothing, because Claude Code refuses a call a hook changed after auto mode reviewed it. Other modes rewrite as before.
- **Commands, not what programs do.** The hook aims to match Claude Code's own rule matching and add to it. A deny rule, and an ask rule on a line that mentions `atl`, is enforced against the commands the agent types (every `atl` form, wrapper, compound command, substitution and redirect), and a `Read(…)` rule against every file the line names or redirects. What an allowed program then does with files, plugins, configs or build steps it loads (`build.rs`, test files, MSBuild, the linker, plugins, a kubeconfig `exec`, compose files) is out of scope: deny rules restrict commands, not what programs do.
- **The plain shape once you have a rule.** With any deny or ask rule, a line that runs `atl` goes through without a prompt only when it is one plain command from a short list: `atl ls`, `tree`, `read`, `wc`, `diff`, `grep`, `rg`, `find`, the everyday `atl git` reads plus `add` (without `-f`), `commit`, `checkout` and `switch`, and `atl cargo build`, `test`, `check`, `clippy`, `fmt` (nothing after `--`), each with listed options only and literal paths checked against your `Read(…)` rules. These make it ask:
  - a pipe, `&&`, `;`, a redirect, a substitution, a variable, a glob, `cd` in front, a word starting with `@`, a `:` in a git word other than a commit message, and `atl bypass`;
  - every program that loads configuration or runs code a flag names (kubectl, docker, dotnet, go, npm, npx, pip, uv, pytest, prettier, rspec, gh…);
  - a rule's command text anywhere in the line, however it is quoted.

  ATL rewrites only into that shape and otherwise leaves your command as written. With only a `git push` deny, 54 of 124 common agent commands probed prompt (mostly `atl` forms typed directly) and 26 keep their original text instead of an `atl` rewrite; `atl git status`, `atl ls`, `atl cargo test`, `atl grep` and `atl read` do not prompt. Without any rule nothing changes.
- **Not covered.**
  - Rules given only with `--settings` or `--disallowedTools`, server-managed settings and a macOS MDM profile never reach a hook, so they are not applied to the wrapped command.
  - Files a recursive search walks into (`grep -r`, also when `rg -g` or `--include` picks them).
  - Files, links or settings the same command creates (`ln -s . d && cat d/secret.txt`).
  - What a plain command runs from files (the tests and build scripts `cargo test` builds, git hooks and config), which is out of scope as above.
  - Git output that prints a `Read`-denied file's content without naming it (`git show HEAD`, `git log -p`, `git diff`).
  - Code that hides a command from a parser (a `python -c` script, `eval` of a variable) is outside every rule, as it is without atl. Behind `atl` it makes the hook ask when you have rules.
  - Commands that match no rule run without a prompt.
- **The guardrails** check the command the agent asked for (see [SECURITY.md](../SECURITY.md#guardrail-limits) and [Known limits](guardrails.md#known-limits)).

`/anthology:setup remove` takes the allow out again. If you are not comfortable with it, do not run setup, or run `remove` afterwards.
