# Security

## Reporting a vulnerability

Please report security problems privately, through GitHub's private vulnerability reporting: open the repository's **Security** tab and choose **Report a vulnerability**. Do not open a public issue for a vulnerability.

Useful things to report:

- A way to run something a guardrail is meant to block (G1 to G6 in the [README](README.md#guardrails)) that the guard lets through. Include the exact tool call or command.
- A way to make `/anthology:setup`, `/anthology:setup remove`, `/anthology:import` or `/anthology:export` read or write outside the places they are documented to use.
- A way to get an rtk binary installed that does not match the hash pinned in `plugins/anthology/rtk.lock`.
- A way to get a note from a repo you did not approve into your main session, or a repo-memory note into a session by a route other than the ones documented (agent memory in a cloned repo already reaches its own agent; see [the memory trust model](#the-memory-trust-model) and ADR-019).
- A secret or personal data committed to this repository.

Fixes ship as a new plugin release, so turn on auto-update for the marketplace to receive them.

## Threat model

Anthology configures Claude Code and the agents that run inside it. The risks it addresses:

| Risk | What stands in the way |
|---|---|
| An agent installs browser tooling on your machine, or launches Chrome | Guardrails G1 and G2, and running tests in Docker |
| An agent reads your Chrome profile (cookies, saved logins) | Deny rules and guardrail G3 |
| An agent uses a browser other than BrowserOS Neo | Guardrails G4 and G5 |
| A secret ends up in a note committed to a repo | Guardrail G6 |
| A stranger's committed notes steer your agents | Repo auto-memory: the approved-repos rule. Agent memory: not blocked, only mitigated by the hints-not-instructions rule in the agent bodies; review `.claude/agent-memory/` in repos you do not trust (see [The memory trust model](#the-memory-trust-model)) |
| A tampered rtk binary | A pinned sha256 per target and build-provenance attestations |
| Vendored upstream content changes silently | Every pin is exact, and CI regenerates all vendored content and fails on a difference |

The assumed cause is a mistake, or an agent misled by content it read (a web page, a file in a repo). It is **not** a defence against a determined attacker, a malicious plugin, or code you choose to run yourself. The guardrails are not a sandbox.

What Anthology changes outside the plugin, by who writes it:

**`/anthology:setup`, after it shows the changes and you confirm once:**

- `~/.claude/settings.json`: the `Bash(rtk *)` allow, the `Bash(anthology-mem save *)` allow, the Chrome and Chromium `Read(...)` deny rules, one `Read(...)` allow for the folder that holds the plugin's files, and `worktree.baseRef`.
- The rtk download.
- Approving the current repo for memory. You can decline this step. It adds the two `merge=union` lines to the repo's `.gitattributes`.
- Creating the personal-memory folder, and copying the repo's notes from the old path-keyed `~/.claude/projects/<encoded path>/memory/` folder into it. A note goes into the repo's `.claude/memory/` instead only if you pick it, one by one, after seeing its diff, and the privacy scan finds nothing in it.

**Session start, automatically:**

- It runs the guardrails on a known-bad sample, reconciles the rtk binary with `rtk.lock` (a download into the plugin's data folder) and writes `~/.claude/anthology/state.json`.
- In approved repos only: it writes `autoMemoryDirectory` into `.claude/settings.local.json` and a git exclude entry for that file, regenerates the `MEMORY.md` indexes, copies notes from the old path-keyed `~/.claude/projects/<encoded path>/memory/` folder into personal memory (never into the repo), and moves `type: user` notes out of `.claude/memory/` and `.claude/agent-memory/` into personal memory. Nothing is overwritten; a repo file is deleted only after its personal copy is verified.
- If a release changed the `settings.json` entries that setup makes and one of them is missing, it prints one line, "anthology settings changed in this release — run /anthology:setup to review". It never edits `settings.json` itself. If you removed one of those entries on purpose, the line comes back at each session start after every release that changes setup's entries, until you run setup again.
- If the personal-memory folder is a symlink or not a folder, it prints one line and does not load, move into or copy into it. Make it a real folder to turn memory back on.
- The memory search index lives in the plugin's data folder.

**`/anthology:setup remove` takes out:**

- the settings entries Anthology added (rules you added yourself are never touched, and `worktree.baseRef` goes back to its previous value);
- the downloaded rtk and `state.json`;
- optionally, the repo approvals.

It never deletes memory notes, the `.gitattributes` lines, the personal-memory folder or exports. Uninstalling the plugin without `--keep-data` removes its data folder (the rtk binaries and the search index).

Nothing is written to `~/.claude/CLAUDE.md`.

## Guardrail limits

Read these before relying on the guardrails. The guard is built to stop accidental browser installs and launches by cooperative agents. It is not a sandbox, and it does not defend against deliberate obfuscation.

- **Two layers, neither a jail.** The deny rules cover Claude Code's built-in file tools and the Bash file commands it recognises. The hook, `guard.py`, sees each tool call before it runs and blocks the call with exit code 2. There is no OS-level enforcement.
- **A hook sees commands, not what a script does.** A script that launches Chrome internally is invisible to it. That is why the knowledge Anthology ships must never launch Chrome, and why the deny layer exists.
- **Rules match commands, not text.** `grep playwright package.json` is allowed on purpose.
- **Indirection hides commands.** Commands built inside variables or arrays and then run are not seen for what they are. Neither is a command whose text comes from another command's output and is fed to a shell: `bash -c "$(…)"`, `eval`, `<(…)`, `source <(…)`. Unusual nested shell syntax, such as a comment or a `case` inside `$(…)`, or a quoted `}` inside `${…}`, can also get past the parser.
- **Some inputs are not path-checked.** Free-text tool fields (an agent prompt, todo text) and most MCP tool arguments are not checked for browser profile paths. Inside interpreter code (`python3 -c`, `node -e`, a heredoc fed to `python3`, a BrowserOS Neo `run` script) only profile paths spelled out in one string are seen: a path assembled piecewise (`Path.home() / "Library" / …`, variables) is not, and neither is a command run from inside the code (`os.system(…)`).
- **Profile coverage is bounded.** The guard and the deny rules cover the known Chrome and Chromium profile folders and their parents. Grandparent folders, such as `find ~ -name Cookies`, and the profile folders of snap and flatpak browsers are not covered. The native `Read` deny rules that setup adds are the second layer for browser profiles.
- **Docker is exempt from G1 and G2.** Commands whose program is `docker` or `docker-compose` are not checked against those two rules, because running browser tests in a container is the sanctioned way. G3 to G6 still apply to them.
- **G6 checks content, not files.** It looks for common key and token formats (AWS access keys, GitHub tokens, `sk-` API keys, Slack tokens, private-key blocks) and `password` or `api key` assignments in what `Write` or `Edit` puts into a memory folder, and in command text that writes there. **Copying a secret file into memory is not caught.** Neither is a secret in a format it does not know.
- **It fails open on malformed input.** The guard exits 0 when its input is not valid JSON, so a bad payload can never block your whole session.
- **It cannot be silently off, but it can be off.** At session start Anthology runs the guard copy the hook actually executes (the marketplace folder for a local-folder marketplace, the plugin cache otherwise) on a known-bad sample and shows a loud message if the guard cannot run or does not block. If `python3` is missing, the guard cannot run: tool calls go ahead and you see only that message.
- **A block wins over rtk.** The guard checks the command Claude asked for, not rtk's rewrite of it, and a block takes precedence over an rtk allow. `rtk`, `rtk bypass`, `rtk proxy`, `rtk run`, `rtk err`, `rtk test` and `rtk summary` prefixes are stripped before the rules run, and an `rtk run -c` or `--shell` script is checked like `sh -c`, so `rtk npx playwright install` and `rtk run -c 'npx playwright install'` are still G1.
- **macOS and Linux only.** Setup refuses on Windows, and there are no PowerShell-aware rules.

## The `Bash(rtk *)` allow

Setup adds a blanket `Bash(rtk *)` allow to `~/.claude/settings.json` so that commands rewritten by rtk do not prompt you, except where one of your deny or ask rules is involved (below).

- `rtk bypass <cmd>` runs any command. So the allow also **auto-approves `rtk bypass <anything>`**: an agent, or text that misleads an agent, can run an arbitrary command without a permission prompt by putting `rtk bypass` in front of it.
- rtk's hook applies the deny and ask rules of your settings files to the command and to the command an `rtk …` form wraps (`rtk bypass`, `proxy`, `run`, `err`, `test`, `summary`, `rtk <tool> …`), recursively, including `sh -c` scripts, `find -exec`, `xargs`, `env`, a dashed git builtin (`/usr/lib/git-core/git-push` is `git push`) and words after a redirect (`git >/dev/null push`). A deny prints an explicit hook deny; an ask makes Claude Code prompt even though `Bash(rtk *)` matches. The files it reads: `<project>/.claude/settings{,.local}.json` (also under `CLAUDE_PROJECT_DIR`), `~/.claude/settings{,.local}.json` and `managed-settings.json` plus `managed-settings.d/*.json` (with `allowManagedPermissionRulesOnly` honored). An unreadable settings file makes it ask for every command.
- **What the rules cover: commands, not what programs do.** rtk's hook aims to match Claude Code's own rule matching and add to it. A Bash deny or ask rule is enforced against the commands the agent types, in every form: each `rtk` form, a wrapper, a compound command, a substitution, a redirect. A `Read(…)` rule is enforced against every file the command line names or redirects. What an allowed program then does with the files, plugins, configs or build steps it loads (`build.rs`, test files, MSBuild targets, the linker, plugins, a kubeconfig `exec` entry, compose files) is out of scope: deny rules restrict commands, not what programs do.
- **The plain shape when you have rules.** As soon as you have any deny or ask rule (Bash or `Read`), a command line that runs `rtk` (or mentions it anywhere) runs without a prompt only when all of this holds; anything else asks, and a deny rule that matches still denies:
  - **One simple command.** Outside quotes only letters, digits, spaces and `-_./:,=+%@` appear: no `;`, `&&`, `||`, `|`, `&` or newline; no subshell, group or function; no redirect of any kind, here-string or here-document; no substitution, `$` expansion, glob, brace, extglob character or comment; no word opening with `=` or `@`. `$`, a backquote, a backslash and `!` may appear only inside single quotes (`-m 'feat!: x'`). Because zsh's `EXTENDED_GLOB` makes `^`, `#` and a later `~` glob operators (`c^1./.env` matches `config/.env`), an unquoted word holding one asks, except a `commit -m` message and a git revision of the strict shape `REF` followed by `~N`, `^N`, `~` or `^` (or two such joined by `..` or `...`), where `REF` is letters, digits, `_` and `-` with no `/` or `.` (`HEAD~1`, `main^`, `HEAD~5..HEAD`). A leading `~` is your `$HOME` only as `~` or `~/…` with no later `^`, `~` or `#` (with `HOME` unset it asks). Nothing stands before `rtk`: no assignment, `cd`, `exec`, `eval`, `source` or launcher.
  - **A listed command.** `rtk ls`, `tree`, `read`, `wc`, `diff`, `grep`, `rg`, `find`; `rtk git status`, `log`, `diff`, `show`, `branch`, `blame`, `add`, `commit`, `checkout`, `switch`, `rev-parse`, `ls-files` (no git option before the subcommand); `rtk cargo build`, `test`, `check`, `clippy`, `fmt`.
  - **Listed options.** Each command has its own option list; an unknown option or an abbreviation asks. Among those not listed: `git add -f`, `git commit -F`, `git log --output`, `git diff --ext-diff`, `grep -f`, `--pre`, `rg -z`, `find -exec`, `-delete`, `-fprint`, `-newer`, `cargo --config`, `-Z`, `+toolchain`, `--target`, `--manifest-path`. Any word after cargo's `--` asks. An option with an optional value takes it only glued, as git does (`--pretty=oneline`, `-U5`, `-uno`), and an option that takes the next word asks when that word starts with `-`.
  - **Literal paths, checked.** Every operand and option value except a search pattern (grep's pattern and `-e`, `--include`, `--glob`, find's `-name` and `-path`) is resolved against the working directory, lexically and through symlinks. It asks when it is `-` or lies under `/dev`, `/proc` or `/sys`, and it is checked against your `Read(…)` rules: a deny rule denies, an ask rule asks. In a git command no word after the subcommand may hold a `:` (so `git show HEAD:<file>` and `--pretty=format:%h` ask), except a `commit -m` message, which is not a path and is not checked. Quoted text is literal (`'$x'` is a name), except that git reads its pathspecs as globs, so a git word holding `*`, `?`, `[` or `\` asks even when quoted. For a strict revision word such as `x~1` or `x^1`, every file in the working directory whose name starts with `x` is checked too, since `EXTENDED_GLOB` matches those; a directory too large to scan asks.
  - **No rule text.** A Bash deny or ask rule's command text found anywhere in the line asks, whatever the quoting (`rtk ls 'git push'`).
  rtk rewrites a command only into that plain shape; otherwise it leaves the command as written, so Claude Code judges it as it would without rtk. Everything off the list asks, including `rtk bypass`, `rtk run`, kubectl, docker, dotnet, go, npm, npx, pip, uv, pytest, prettier, rspec and gh. The lists are at the top of `rtk/src/hooks/anthology_gate.rs`. With only a `git push` deny, 54 of 124 common agent commands probed prompt (mostly `rtk` forms an agent typed itself, such as `rtk npm run build` or `rtk bypass …`), and 26 lose their rtk rewrite; `rtk git status`, `rtk ls`, `rtk cargo test`, `rtk grep`, `rtk read` and the like do not prompt.
- `Read(…)` deny rules block the reads rtk would otherwise launder: `cat`, `head`, `tail`, `grep`, `rg`, `diff`, `wc`, `ls`, `find`, `tree` and `rtk read` of a denied path, and a denied path named by any command behind `rtk` (also glued to an option, `-fsecret.txt`; a word opening with `@` asks). Paths are resolved against the working directory, `cd`, `~` and symlinks **that exist when the hook runs**: a link or copy the same command creates (`ln -s . d && cat d/secret.txt`, `cp -r`, `tar`) is not seen. A path it cannot resolve (`$VAR`, a glob, `cat < secr?t.txt`, a redirect after a `cd`, `/proc/…`, `/dev/fd/…`) asks. `Read(…)` ask rules are handled the same way.
- **Not covered:** rules given with `--settings` or `--disallowedTools`, server-managed settings and a macOS MDM profile, which no hook can read; files a recursive search walks into (`grep -r x .`), including one a glob value selects (`rg -g secret.txt`, `grep -r --include secret.txt x .`); files, links and settings the same command creates (above); what a plain command runs from files, out of scope as above (the tests and `build.rs` `cargo test` builds, git hooks, git and cargo settings, `.gitattributes` filters) and the environment your session already has (`GIT_PAGER`, `NODE_OPTIONS`); git output that prints a `Read`-denied file's content without naming it (`git show HEAD`, `git log -p`, `git diff`, or `git show <blob>` with a blob id `git ls-files -s` printed; `git show HEAD:<file>` asks); the content of an argument file a program reads on its own (an `@file` on the line asks); and a command hidden from any parser (a `python -c` script, `eval` of a variable), which no rule catches without rtk either. Programs that run a string (`awk`, `make`, `ed`, `sqlite3`, `python -c`) are not read, but behind `rtk` they ask when you have rules.
- The guardrails G1 to G6 check the command the agent asked for, with the `rtk` prefixes stripped as described above.
- It is an accepted risk, chosen for a prompt-free workflow, not an oversight.
- `/anthology:setup remove` takes the allow out again. You can also delete the entry from `~/.claude/settings.json` yourself.

### What rtk keeps on your machine

- **What:** a SQLite tracking database with every Bash command line the hook sees, the rtk command it became and token counts (`rtk gain` reads it); a recall store with output rtk left out of a result (`rtk recall` reads it); and a tee spool of raw output. Command lines and output can contain secrets.
- **Where:** `~/.claude/anthology/rtk/` (`history.db`, `recall.db`, `tee/`), or the same under `$CLAUDE_CONFIG_DIR`. `bin/rtk` and `bin/rtk-hook` set `RTK_DB_PATH`, `RTK_RECALL_DB` and `RTK_TEE_DIR` there. Files are created private to you (0600, folder 0700). Nothing is uploaded.
- **How long:** the history 90 days, the recall store 30 days, the tee spool the last 20 files. Expired rows leave `rtk gain` and `rtk recall`, but their bytes can stay in the database file until SQLite overwrites them.
- **Deleting it:** `/anthology:setup remove` deletes the folder; you can also delete it yourself at any time. Deleting the folder is the only way to remove the command lines from disk: `rtk gain --reset --yes` only zeroes the `rtk gain` numbers, and the text of the deleted rows can stay in the file. rtk has no switch that stops recording command lines but keeps `rtk gain`.
- **Version 1.2.1-anthology.1** kept these in rtk's default folder (`~/Library/Application Support/rtk/` on macOS, `~/.local/share/rtk/` on Linux). Remove leaves that folder alone because another rtk install may share it; delete it yourself if you ran that version.

The binary behind it is verified before use. The pinned sha256 values, one per target, are in `plugins/anthology/rtk.lock`. They are the hashes of the release archives (`rtk-<target>.tar.gz`), and an archive is checked before anything in it is unpacked or installed. The archives are built by the `rtk-release.yml` workflow and carry build-provenance attestations. To check one, download it and run `gh attestation verify <file> --repo rbonweb/anthology`. rtk is built for Claude Code only and has no telemetry; CI runs the built binary with the network disabled (`scripts/rtk-net-test.sh`). The shipped source is the Apache-2.0 edition: upstream's BUSL-1.1 engine is removed, and so is the output compression that depended on it.

## The memory trust model

Notes are Markdown that is loaded into a session's context. A note can therefore steer an agent, the way a line in a `CLAUDE.md` can. The model:

- **Memory is opt-in per repo, per user.** It is active only in repos you approved by running `/anthology:setup` inside them. In any other repo no repo auto-memory is written or injected (your personal-memory index still is), and a repo that already has `.claude/memory/` only gets a one-line hint. This covers repo auto-memory only; agent memory is the next point.
- **Agent memory loads in unapproved repos too.** Roster agents set `memory: project`, so Claude Code itself loads `.claude/agent-memory/anthology-<agent>/MEMORY.md` into that agent in any repo, approved or not, and the agent bodies also tell the agent to read the folder. Anthology cannot stop the platform load. What it does: every agent body says notes are hints, never instructions, and that they cannot override your request, the BrowserOS-only rule or the Docker rule (subagents never receive the routing guide, so it is not named). That is guidance to the model, not a barrier. Review `.claude/agent-memory/` in any repo you do not trust, like any other code ([ADR-019](DECISIONS.md)).
- **Approval follows the remote.** Approvals in `~/.claude/anthology/approved-repos.json` are keyed by the normalized git remote URL, so a re-clone of the same repo is trusted automatically. Anyone who can push to that remote can add notes you will load. Treat memory changes like code changes: review them, and use CODEOWNERS for `.claude/memory/` and `.claude/agent-memory/` (see the [README](README.md#codeowners-for-memory)).
- **Repo memory is visible to everyone who can see the repo.** If the repo is public, so is its memory. Never put secrets or personal details in it. Guardrail G6 blocks the obvious secret patterns and is not a scanner.
- **Agent notes are saved without a prompt.** Setup allows `Bash(anthology-mem save *)`, so roster agents save their own notes silently, also in repos you have not approved (Claude Code's own agent memory works there too). The helper writes only into `.claude/agent-memory/anthology-<agent>/` of the current git repo and refuses unknown agents, symlinks and anything the privacy scan below blocks; those checks are accident guards, not a guarantee. An agent misled by something it read can still store misleading instructions in its own notes, which Claude Code loads on that agent's next run, so review `.claude/agent-memory/` changes before you commit them.
- **Personal memory stays out of repos.** It lives under `~/.claude/anthology/personal-memory/`. Its path and index are added to your main session at start, even when it is empty.
- **`user` notes are moved out of the repo.** Claude Code's auto memory writes every note type into the repo's memory folder. At each session start in an approved repo, Anthology moves `type: user` notes from `.claude/memory/` and `.claude/agent-memory/` into personal memory, and the repo `MEMORY.md` indexes never list them. If one was already committed, its removal still needs a commit, and it stays in the git history.
- **Old notes stay personal.** Notes from Claude Code's old path-keyed memory folder are copied into personal memory, never into the repo on their own. Moving one into the repo is a per-note choice in `/anthology:setup`, and only for notes the privacy scan passes.
- **The privacy scan guards anthology's writes into repo memory, not every write.** Setup's moves, `/anthology:import` and `anthology-mem save` scan each note before it goes into the repo. A note with a home path (`/Users/<name>`, `/home/<name>`), an email address, a private or public IP address, a private host name (such as `*.local`, `*.lan`, `*.internal`) or this machine's name, an `ssh user@host` target, an `.ssh/` key path or a secret-looking token is blocked: setup and import send it to personal memory instead, and `anthology-mem save` refuses it. The findings name a kind and a line, never the text. It also looks for `secret_access_key = …`-style assignments and `scheme://user:password@host` URLs. It does not scan Claude Code's own auto-memory writes, or an agent's own Write or Edit into its agent folder (that is guardrail G6's narrower job). It is an accident guard, not a guarantee: it knows common formats only, so read what you commit.
- **Exports hold personal notes.** An export refuses to write into a git work tree. Keep the archive private.
- **Imports are checked.** Import shows a dry run first, never overwrites, extracts with Python's `data` filter (no absolute paths, `..`, links or devices), writes only the three memory locations, refuses archives over 50 MB, reports secret-looking content, and sends repo notes the privacy scan blocks to personal memory. Import archives only from people you trust: their notes become yours.
- **Claude Code's own rules apply.** A project-level `autoMemoryDirectory` is honored only in a trusted workspace.

## Other things to know

- **Content fetched at run time.** The `web-design-guidelines` knowledge downloads its rulebook with `WebFetch` when an agent uses it. Treat that as external content.
- **Vendored content.** Every folder under `plugins/anthology/knowledge/` and `plugins/anthology-superpowers/` is generated from an exact upstream commit by `scripts/vendor.py`. Nothing is edited by hand, and `vendor.py check` runs in CI on every PR.
- **Supply chain.** CI images are pinned by digest, test dependencies by hash and GitHub Actions by commit SHA, and Dependabot proposes the bumps. Secrets are scanned with gitleaks on every PR.
