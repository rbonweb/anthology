# The slop gate

This page covers the slop gate, which refuses an edit when its new text holds a known AI-writing or AI-design tell. It lists what the gate reads, the 16 rules, how to quote a specimen or waive a hit, the files it skips, how to run it yourself and how to switch it off.

The rules look for known tells, one phrase or code pattern at a time. They do not judge whether a text is good, and they prove nothing about who wrote it: people write `in today's fast-paced world` too. A passage can pass every rule and still read badly.

## What it checks

A `PreToolUse` hook in the `anthology` plugin runs `anthology slop hook` before each `Write`, `Edit` and `MultiEdit` call, in the main session and in every agent. When the new text holds a hit, the call is refused and nothing is written. The agent gets a message that names each line, the phrase and what to do instead:

```text
anthology slop gate: the Write of docs/a.md was not applied. Its new text has these AI tells:
  line 3: text.ai-vocab "delve": say the concrete thing the word stands for: the step, the number, the part
Rewrite each passage in plain, specific words, then retry; a synonym swap is not a fix. Put a quoted specimen in a code span or between <!-- slop-ignore-start --> and <!-- slop-ignore-end -->. Add <!-- slop-ok: <rule> <reason> --> only for wording the user asked for word for word; the lead is told about each one.
```

The message lists at most 20 lines and then says `and N more`.

Four kinds of file are checked, matched by extension in any letter case:

| Kind | Extensions | What counts as text |
|---|---|---|
| Markdown | `.md`, `.markdown`, `.mdx` | Prose. Code, comments, tags, quotes and links are masked (see [Quotes, specimens and waivers](#quotes-specimens-and-waivers)). |
| HTML | `.html`, `.htm` | Text outside tags, except inside `<script>`, `<style>`, `<pre>` and `<code>`. Also the values of `alt`, `title`, `placeholder`, `aria-label` and `label`, and the `content` of the `description` meta tag and the `og:title` and `og:description` tags. |
| JSX | `.tsx`, `.jsx` | Text between tags and the same five attribute values. A string literal anywhere else is never text, because class names, keys, URLs and log lines live there. |
| CSS | `.css`, `.scss` | None. The design rules read the source. |

Text rules read the text. Design rules read the source: style blocks, class values and component names. A phrase matches in either apostrophe style (`'` and `’`), in any letter case, and across a line wrap.

A file is checked only up to 1 MiB. A bigger one is allowed unchecked.

## New text only

The gate checks what a call adds, not the file it lands in. It builds the file as it would be after the call and keeps only the hits that touch new text. A hit counts when any part of it does.

- **`Edit`.** What `new_string` changes in `old_string`. The gate sees the whole file, so an edit inside an existing code fence or frontmatter lands in code and is not flagged, even though `new_string` holds no fence marker.
- **`MultiEdit`.** The edits are applied in order, and what differs at the end is new.
- **`Write` to a new file.** The whole content.
- **`Write` over an existing file.** What differs from the old file. Old and new lines are matched one for one: a line you kept is not new, even when it appears twice, but a copy you added is. Blank lines are skipped. A file that already holds a tell can be rewritten as long as the rewrite adds none.

How much of a changed line is new depends on the file. In Markdown it is the words that changed. A line often holds a whole paragraph that somebody else wrote, so fixing one typo in it does not bring an old `seamless` on the same line back. A rewritten line, one that shares fewer than half of its words with the line it replaced, is new as a whole. Whitespace is not a change. In HTML, JSX and CSS the whole changed line is new, because a design rule reads the code around the change.

The two density rules count only the new text too: the dashes and the words in it.

The gate allows a call when it cannot work out the result: an `Edit` whose `old_string` is not found or matches more than once, a file that is not UTF-8, or a result over 1 MiB. Claude Code reports a failed `Edit` itself.

## The rules

A **block** rule refuses on one hit. A **density** rule counts hits across the new text and refuses only when there are 3 or more and they come faster than the rate in the table.

| Rule | Files | Kind | What it flags |
|---|---|---|---|
| `text.ai-vocab` | Markdown, HTML, JSX | block | Stock AI vocabulary: `delve`, `tapestry`, `seamless`, `elevate`, `unleash`, `supercharge`, `game-changer`, `groundbreaking`, `unparalleled`, `synergy`, `holistic`, `meticulous`, `plethora`, `multifaceted`, `ever-evolving`, and their other forms. Literal uses pass: `elevate privileges`, `seamless steel pipe`, `seamless texture`, `groundbreaking ceremony`. |
| `text.stock-phrase` | Markdown, HTML, JSX | block | Stock openers and cliches: `in today's fast-paced world`, `let's dive in`, `without further ado`, `gone are the days`, `look no further`, `a testament to`, `unlock the full potential`, `harness the power of`, `it's important to note that`. |
| `text.chat-residue` | Markdown, HTML, JSX | block | Chat-assistant wording left in a file: `I hope this helps`, `let me know if you have any questions`, `I'd be happy to help`, `great question`, `you're absolutely right`, `as an AI, I`, `here's the revised version`, and a sentence that opens with `Certainly!`, `Absolutely!` or `Sure!`. |
| `text.not-x-but-y` | Markdown, HTML, JSX | block | A contrast that says what a thing is not before it says what it is: `It's not a tool, it's a platform.`, `This isn't just a cache; it's a memory layer.`, `It's not about speed. It's about trust.` |
| `text.hype-cta` | Markdown, HTML, JSX | block | Hype calls to action: `ready to transform`, `to the next level`, `join thousands of`, `say goodbye to`, `at your fingertips`, `in just a few clicks`, `we've got you covered`, `don't miss out`. |
| `text.em-dash-density` | Markdown, HTML | density | 3 or more em dashes (`—`, or an en dash with a space on each side) at a rate above 1 per 80 words. Dashes in structure do not count (see below the table). |
| `text.triad-density` | Markdown, HTML | density | 3 or more lists of three, such as `plans, specs and roadmaps`, at a rate above 1 per 150 words. Each item needs 4 letters or more. A list stays on one line, and a phrase that is not a list does not count (see below the table). |
| `ui.purple-gradient` | CSS, HTML, JSX | block | A gradient with a violet or indigo stop (`C1`): `linear-gradient(135deg, #6366f1, #a855f7)`, or the Tailwind classes `bg-gradient-to-r from-indigo-500 to-purple-600`. A `var(--x)` is resolved first. A blue gradient passes. |
| `ui.gradient-text` | CSS, HTML, JSX | block | Text filled with a gradient (`C6`): `background-clip: text` together with a transparent text fill, or `bg-clip-text text-transparent`. |
| `ui.shadcn-default` | CSS, HTML | block | The untouched shadcn/ui base theme (`K1`): a `--primary` set to a stock value such as `oklch(0.205 0 0)` or `222.2 47.4% 11.2%`. |
| `ui.glow-orb` | CSS, HTML, JSX | block | A decorative blurred orb (`C11`): `border-radius: 50%` with a `filter: blur()` of 24px or more, or the classes `rounded-full`, `blur-3xl` and `absolute` together. |
| `ui.effect-library` | JSX | block | A stock effect-library component (`K10`): `<ShimmerButton>`, `<BackgroundBeams>`, `<Spotlight>`, `<Meteors>`, or a path under `components/magicui/` or `components/aceternity/`. |
| `ui.placeholder-media` | CSS, HTML, JSX | block | An avatar or image service meant for placeholders (`IM4`): `i.pravatar.cc`, `api.dicebear.com`, `ui-avatars.com`, `picsum.photos`, `placehold.co`. |
| `ui.placeholder-text` | HTML, JSX | block | Placeholder copy in the text: `Lorem ipsum`, `[Company Name]`, `[insert tagline]`. |
| `ui.five-star-row` | HTML, JSX | block | Five stars in a row (`IM5`): `★★★★★`, five `<Star />` icons, or an array of five mapped to stars. |
| `ui.generator-signature` | HTML, JSX | block | A site generator left in (`F1`): `lovable-tagger`, `gpt-engineer`, `v0.dev`, `bolt.new`, or `<meta name="generator" content="v0">`. |

The two density rules skip text that is structure or a plain verb phrase, since people write it too:

- **Dashes that do not count.** A dash in an ATX heading (`## Step 1 — Install`) or a table row. A dash after the label of a list item: a bold, linked or underscore term, with an optional `: value` and an optional task-list box (`- **Name** — what it does`). A dash after a leading date or version (`- 2026-11-20 — Fixed the lock check.`, `## [1.2.3] — 2026-11-28`). A dash inside `[...]`.
- **Lists of three that do not count.** A list never crosses a line break, a list item or a sentence end. It is dropped when the word after it starts an object (`the`, `a`, `an`, `your`, `their`, `its`, `this`, `our`, `his`, `her`, `my`, `it`, `them`), because then the last two words are a verb phrase, as in `stop and return the question`. Items that are linked or bold labels do not count either.

The codes in parentheses are entries in the avoid-ai-design catalogue, which the `ui-reviewer` agent also scores against. `anthology slop rules` prints the same list with each rule's kind, its files and a one-line summary.

Every rule has to give no finding on this repo's own docs. CI runs all of them over `README.md`, the other top-level documents and every page under `docs/` (`crates/repo-tests/tests/slop_corpus.rs`), so a rule that flags a human page is narrowed, never waived.

## Quotes, specimens and waivers

A page about tells has to show them. In order of preference:

1. **Quote the specimen.** In Markdown these are never checked: frontmatter, fenced code, inline code, HTML comments and tags, blockquote lines, URLs and link targets, reference link definitions, MDX `import` and `export` lines, and double-quoted spans (straight or curly quotes, on one line, up to 300 characters).
2. **Ignore a region.** Put a start marker and an end marker on lines of their own, each indented by at most 3 spaces. Everything between them is skipped by every rule. A start marker may carry a reason after the keyword, and a start with no end runs to the end of the file.
3. **Waive one hit.** Put a `slop-ok` comment on the same line as the finding. It names a rule and gives a reason of 8 characters or more.

```text
Ignore, Markdown and HTML:  <!-- slop-ignore-start -->   ...   <!-- slop-ignore-end -->
Ignore, CSS and JS:         /* slop-ignore-start */      ...   /* slop-ignore-end */
Ignore, JSX:                {/* slop-ignore-start */}    ...   {/* slop-ignore-end */}

Waive, Markdown and HTML:   <!-- slop-ok: text.ai-vocab the product is called Seamless Sync -->
Waive, JSX:                 {/* slop-ok: ui.purple-gradient brand colour from the style guide */}
Waive, CSS and JS:          /* slop-ok: ui.purple-gradient brand colour from the style guide */
                            // slop-ok: ui.purple-gradient brand colour from the style guide
```

A waiver with an unknown rule ID or a shorter reason waives nothing, and the hit stands. A density hit has no single line, so a valid waiver for that rule on any line of the new text waives it.

Use a waiver only for wording the user asked for word for word, such as a product that is really called `Seamless Sync`. The after-work check lists every waiver and region an agent adds, and the `ui-reviewer` lists them in its report, so the lead can ask why.

## Files it skips

- Any file whose extension is not in the table above.
- Anything under the Claude config folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
- A file with one of these folder names in its path below the repository root, in any letter case: `.git`, `.claude`, `.superpowers`, `node_modules`, `vendor`, `third_party`, `dist`, `build`, `.next`, `coverage`, `fixtures`, `__fixtures__`, `__snapshots__`, `__tests__`, `__mocks__`, `testdata` and `stories`. The root is the nearest folder above the file that holds `.git`, as a folder or a file. With no such folder, the whole path is searched for those names.
- A git submodule inside one of those folders, such as `vendor/lib`. It has its own `.git` file, so the search goes on above it, up to the repository that holds it.
- A story or test file: a name with `.stories.`, `.test.` or `.spec.` in it, such as `Button.stories.tsx` or `guide.test.md`.
- A legal file: a name that starts with `LICENSE`, `LICENCE`, `COPYING`, `NOTICE`, `DISCLAIMER` or `CODE_OF_CONDUCT`, in any letter case, with or without an extension (`LICENSE`, `license-mit.md`, `NOTICE.md`). Their fixed wording, such as `express or implied`, would trip the list-of-three rule.

An agent's worktree sits under `<repo>/.claude/worktrees/<id>/` and holds its own `.git` file, so it is its own root and is checked. Only the repo's own `.claude/` folder (the memory notes, for one) is skipped.

## The after-work check

The gate sees only the `Write`, `Edit` and `MultiEdit` tools. When a writing agent reports, the after-work mod (see [After-work check](mods.md#after-work-check)) also runs `anthology slop check --since <the main session's HEAD>` in the agent's worktree. That finds text that reached a file another way, such as a scaffolder or `sed` run through the shell, and it lists the waivers the agent added. It adds up to two gaps to the block under the report:

```text
the agent's changes hold 2 AI tells (anthology slop check): docs/a.md:3: text.ai-vocab: "delve"; docs/a.md:9: text.chat-residue: "I hope this helps"; send it back to rewrite them
the agent added 1 slop waiver or ignore region: docs/b.md:7: slop-ok text.ai-vocab: the product is called Seamless Sync; confirm the user asked for that wording
```

A gap names the first three lines. The lead can send the agent back to rewrite the passages, or resume it. The check adds nothing when the binary is not installed (the mod never downloads it), when the run times out or fails, or when the agent works in the main session's own folder.

## Running it yourself

`anthology slop check` runs the same rules from a shell. In a Claude Code session `anthology` is on the Bash path. Elsewhere, use the installed binary (see [Where it comes from](binary.md#where-it-comes-from)).

```text
anthology slop check docs README.md        check whole files, and folders walked for the kinds above
anthology slop check --since origin/main   check only the lines added since a ref
anthology slop check --root ../app --since main
anthology slop rules                       list the rules
```

- **Paths.** Each file is checked whole. A folder is walked, and the exempt folders (see [Files it skips](#files-it-skips)) are left out, and so is a folder below it that holds a `.git` folder, because that is a clone of another repo (a vendor cache, say). A clone named on the command line is checked. Paths in the output are relative to `--root`, else to the current folder.
- **`--since REF`.** Checks the lines added between the merge base of `REF` and `HEAD` and your working tree, plus every untracked file in full. Deleted files are skipped.
- **`--root DIR`.** Runs the git commands of `--since` in `DIR`.
- **Output.** One line per unwaived finding, then one line per waiver or ignore region in the new text:

  ```text
  docs/a.md:3: text.ai-vocab: "delve"
  docs/b.md:7: slop-ok text.ai-vocab: the product is called Seamless Sync
  docs/b.md:12: slop-ignore region
  ```

- **Exit codes.** `0` when nothing is unwaived, `1` when something is, `2` for a usage or git error (the message on stderr starts with `anthology slop:`). With neither a path nor `--since`, the command exits `2`.

`anthology slop rules` prints one tab-separated line per rule: the ID, `block` or `density`, the kinds, and the summary.

The first release with the gate is binary `0.2.10`. To check pull requests in CI, download that release, verify it and compare against the base branch:

```yaml
slop:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v7
      with:
        fetch-depth: 0
    - name: Install anthology
      env:
        GH_TOKEN: ${{ github.token }}
      run: |
        mkdir "$RUNNER_TEMP/anthology"
        cd "$RUNNER_TEMP/anthology"
        gh release download anthology-bin--v0.2.10 --repo rbonweb/anthology \
          --pattern anthology-x86_64-unknown-linux-musl.tar.gz
        gh attestation verify anthology-x86_64-unknown-linux-musl.tar.gz --repo rbonweb/anthology
        tar -xzf anthology-x86_64-unknown-linux-musl.tar.gz anthology
        echo "$RUNNER_TEMP/anthology" >> "$GITHUB_PATH"
    - name: Check the new text
      run: anthology slop check --since "origin/${{ github.base_ref }}"
```

`fetch-depth: 0` gives git the history it needs for the merge base.

## Switching it off

Set `ANTHOLOGY_SLOP_GATE=0` in the environment, for example in the `env` block of `~/.claude/settings.json`, and restart the session. It turns off the hook and the slop part of the after-work check. The other switches are in the [switch table](mods.md#switches).

To let one passage through, quote it or waive it instead (see [Quotes, specimens and waivers](#quotes-specimens-and-waivers)).

## Limits

- **Text written through the shell** (a redirect, `sed -i`, a scaffolder) is never seen by the hook. Only the after-work check reaches it, after the agent reports.
- **English only.** The rules match English wording.
- **Quoted spans are not checked.** Text inside double quotes, a code span or a blockquote passes, so a tell can hide there.
- **Known tells only.** A new habit passes until someone adds its rule, and a clean page can still be dull.
- **The hook fails open.** When the binary is missing, too old to know `slop`, or crashes, the edit goes through. Only the binary's own refusal blocks it. The hook never downloads the binary: the session start does.
- **A slow check is skipped.** The hook gives its rule work 5 seconds. A run that would pass the 60-second hook timeout, which blocks the edit, stops at the 5-second deadline and allows the edit instead. The binary prints `anthology slop: the check was skipped, it ran past the 5 s limit` on stderr, and that edit is not checked at all.
