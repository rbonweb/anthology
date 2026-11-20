# Security

## Reporting a vulnerability

Please report security problems privately, through GitHub's private vulnerability reporting: open the repository's **Security** tab and choose **Report a vulnerability**. Do not open a public issue for a vulnerability.

Useful things to report:

- A way to run something a guardrail is meant to block (G1 to G6 in the [README](README.md#guardrails)) that the guard lets through. Include the exact tool call or command.
- A way to make `/anthology:setup`, `/anthology:setup remove`, `/anthology:import` or `/anthology:export` read or write outside the places they are documented to use.
- A way to get an rtk binary installed that does not match the hash pinned in `plugins/anthology/rtk.lock`.
- A way to get a note from a repo you did not approve into your session.
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
| A stranger's committed notes steer your agents | The approved-repos rule (see [The memory trust model](#the-memory-trust-model)) |
| A tampered rtk binary | A pinned sha256 per target and build-provenance attestations |
| Vendored upstream content changes silently | Every pin is exact, and CI regenerates all vendored content and fails on a difference |

The assumed cause is a mistake, or an agent misled by content it read (a web page, a file in a repo). It is **not** a defence against a determined attacker, a malicious plugin, or code you choose to run yourself. The guardrails are not a sandbox.

What Anthology changes outside the plugin, by who writes it:

**`/anthology:setup`, after it shows the changes and you confirm once:**

- `~/.claude/settings.json`: the `Bash(rtk *)` allow, the `Bash(anthology-mem save *)` allow, the Chrome and Chromium `Read(...)` deny rules, one `Read(...)` allow for the folder that holds the plugin's files, and `worktree.baseRef`.
- The rtk download.
- Approving the current repo for memory. You can decline this step. It adds the two `merge=union` lines to the repo's `.gitattributes`.
- Creating the personal-memory folder.

**Session start, automatically:**

- It runs the guardrails on a known-bad sample, reconciles the rtk binary with `rtk.lock` (a download into the plugin's data folder) and writes `~/.claude/anthology/state.json`.
- In approved repos only: it writes `autoMemoryDirectory` into `.claude/settings.local.json` and a git exclude entry for that file, regenerates the `MEMORY.md` indexes, and copies notes from the old path-keyed `~/.claude/projects/<encoded path>/memory/` folder (never overwriting anything).
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
- **Some inputs are not path-checked.** Free-text tool fields (an agent prompt, todo text) and most MCP tool arguments are not checked for browser profile paths. A BrowserOS Neo `run` script can open `file://` URLs.
- **Profile coverage is bounded.** The guard and the deny rules cover the known Chrome and Chromium profile folders and their parents. Grandparent folders, such as `find ~ -name Cookies`, and the profile folders of snap and flatpak browsers are not covered. The native `Read` deny rules that setup adds are the second layer for browser profiles.
- **Docker is exempt from G1 and G2.** Commands whose program is `docker` or `docker-compose` are not checked against those two rules, because running browser tests in a container is the sanctioned way. G3 to G6 still apply to them.
- **G6 checks content, not files.** It looks for common key and token formats (AWS access keys, GitHub tokens, `sk-` API keys, Slack tokens, private-key blocks) and `password` or `api key` assignments in what `Write` or `Edit` puts into a memory folder, and in command text that writes there. **Copying a secret file into memory is not caught.** Neither is a secret in a format it does not know.
- **It fails open on malformed input.** The guard exits 0 when its input is not valid JSON, so a bad payload can never block your whole session.
- **It cannot be silently off, but it can be off.** At session start Anthology runs the guard on a known-bad sample and shows a loud message if the guard cannot run or does not block. If `python3` is missing, the guard cannot run: tool calls go ahead and you see only that message.
- **A block wins over rtk.** The guard checks the command Claude asked for, not rtk's rewrite of it, and a block takes precedence over an rtk allow. `rtk`, `rtk bypass` and `rtk proxy` prefixes are stripped before the rules run, so `rtk npx playwright install` is still G1.
- **macOS and Linux only.** Setup refuses on Windows, and there are no PowerShell-aware rules.

## The `Bash(rtk *)` allow

Setup adds a blanket `Bash(rtk *)` allow to `~/.claude/settings.json` so that commands rewritten by rtk never prompt you.

- `rtk bypass <cmd>` runs any command. So the allow also **auto-approves `rtk bypass <anything>`**: an agent, or text that misleads an agent, can run an arbitrary command without a permission prompt by putting `rtk bypass` in front of it.
- The guardrails and the deny rules still apply to the wrapped command, so G1 to G6 and the Chrome profile rules still block it.
- It is an accepted risk, chosen for a prompt-free workflow, not an oversight.
- `/anthology:setup remove` takes the allow out again. You can also delete the entry from `~/.claude/settings.json` yourself.

The binary behind it is verified before use. The pinned sha256 values are in `plugins/anthology/rtk.lock`, and the binary is installed only when its hash matches. Release assets carry build-provenance attestations. To check one, download it and run `gh attestation verify <file> --repo rbonweb/anthology`. rtk is built for Claude Code only and has no telemetry; a test runs it with the network disabled.

## The memory trust model

Notes are Markdown that is loaded into a session's context. A note can therefore steer an agent, the way a line in a `CLAUDE.md` can. The model:

- **Memory is opt-in per repo, per user.** It is active only in repos you approved by running `/anthology:setup` inside them. In any other repo no repo memory is written or injected (your personal-memory index still is), and a repo that already has `.claude/memory/` only gets a one-line hint. A stranger's committed notes never load on their own.
- **Approval follows the remote.** Approvals in `~/.claude/anthology/approved-repos.json` are keyed by the normalized git remote URL, so a re-clone of the same repo is trusted automatically. Anyone who can push to that remote can add notes you will load. Treat memory changes like code changes: review them, and use CODEOWNERS for `.claude/memory/` and `.claude/agent-memory/` (see the [README](README.md#codeowners-for-memory)).
- **Repo memory is visible to everyone who can see the repo.** If the repo is public, so is its memory. Never put secrets or personal details in it. Guardrail G6 blocks the obvious secret patterns and is not a scanner.
- **Personal memory stays out of repos.** It lives under `~/.claude/anthology/personal-memory/`. Its index is added to your main session at start.
- **Exports hold personal notes.** An export refuses to write into a git work tree. Keep the archive private.
- **Imports are checked.** Import shows a dry run first, never overwrites, extracts with Python's `data` filter (no absolute paths, `..`, links or devices), writes only the three memory locations, refuses archives over 50 MB, and reports secret-looking content. Import archives only from people you trust: their notes become yours.
- **Claude Code's own rules apply.** A project-level `autoMemoryDirectory` is honored only in a trusted workspace.

## Other things to know

- **Content fetched at run time.** The `web-design-guidelines` knowledge downloads its rulebook with `WebFetch` when an agent uses it. Treat that as external content.
- **Vendored content.** Every folder under `plugins/anthology/knowledge/` and `plugins/anthology-superpowers/` is generated from an exact upstream commit by `scripts/vendor.py`. Nothing is edited by hand, and `vendor.py check` runs in CI on every PR.
- **Supply chain.** CI images are pinned by digest, test dependencies by hash and GitHub Actions by commit SHA, and Dependabot proposes the bumps. Secrets are scanned with gitleaks on every PR.
