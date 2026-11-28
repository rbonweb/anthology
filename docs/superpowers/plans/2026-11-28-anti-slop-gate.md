# Anti-slop Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use anthology-superpowers:subagent-driven-development (recommended) or anthology-superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Long runs also follow `anthology:large-builds`.

**Goal:** Refuse any Write, Edit or MultiEdit whose new text holds a near-certain AI-writing or AI-design tell, report what got through by other routes to the lead, and give the writing and UI agents a rewrite pass and a fixed design rubric.

**Architecture:** A new library crate `anthology-slop` (masking, rules, edit replay, messages) is linked into the `anthology` binary as `anthology slop hook|check|rules`. A PreToolUse hook script calls `anthology slop hook`, and the after-work mod calls `anthology slop check --since`. Agents get two vendored knowledge folders: `humanizer` and `avoid-ai-design`.

**Tech Stack:** Rust 2024 (workspace `regex =1.13.2`, `anthology_pycompat::json`, `anthology_core::git`), POSIX sh, the TypeScript mods, and `anthology-dev vendor`.

**Spec:** There is no separate spec. This plan's Decisions section records the design. It argues from the owner's brief of 2026-11-28 and from the research note `.claude/agent-memory/anthology-researcher/reference-anti-slop-prior-art.md`. Read both before starting.

## Global Constraints

- No new crates.io dependency. `anthology-slop` uses only `regex` (workspace). No `serde_json` (`crates/repo-tests/tests/no_serde_json.rs`); JSON goes through `anthology_pycompat::json`.
- `#![forbid(unsafe_code)]` at the root of `anthology-slop`. Only `anthology-cli` reads `std::env`.
- Every hook in `plugins/anthology/hooks/hooks.json` uses the exec form `/bin/sh ${CLAUDE_PLUGIN_ROOT}/...` with `"timeout": 60`. Every PreToolUse hook has `"onFailure": "block"`. The hook events stay exactly `SessionStart` and `PreToolUse` (`crates/repo-tests/tests/cutover_hooks.rs`).
- The gate fails open: only binary exit 10 blocks (script exit 2). Every other outcome allows.
- Exit codes. `anthology slop hook`: 0 allow, 10 block. `anthology slop check`: 0 clean, 1 at least one unwaived finding, 2 usage or git error. `anthology slop rules`: 0.
- Largest document checked: 1 MiB (`MAX_DOC = 1 << 20`). A larger one is allowed unchecked.
- Rules are English only. The v1 rule list is exactly the 16 IDs in "The v1 rules". No other rule ships.
- Every rule gives zero findings on the corpus test (`crates/repo-tests/tests/slop_corpus.rs`) before it merges.
- The routing guide (`plugins/anthology/templates/routing-guide.md`, 3923 of 3950 characters) is not changed. Session-start output is not changed.
- Files, commits and docs are written in normal prose. Commits carry only the git config author. No personal paths, emails or host names in any file.
- Builds and tests run in Docker through `scripts/rust-test.sh` (`CARGO_BUILD_JOBS=3`). Test tiers follow `.superpowers/sdd/2026-11-20-rust-port/wave4-brief.md`: per-crate tests, fmt and clippy while coding; CI is the PR gate.
- `README.md` stays at or under 120 lines.
- Licences: the rule lists and patterns are ported from tbhb/vale-ai-tells (MIT), conorbronsdon/avoid-ai-writing (MIT), funboy322/avoid-ai-design (MIT) and leonxlnx/taste-skill copy-lint (MIT). humanizer is MIT. impeccable (Apache-2.0) is neither bundled nor copied. The crate's `NOTICE` carries the MIT copyright lines.

## Review Focus

1. **An Edit inside an existing code fence or frontmatter.** `new_string` holds no fence marker, but the text it lands in is code. Expected: no finding. Test: Task 7 `hook_allows_an_edit_inside_a_code_fence`; Task 2 `fenced_code_is_masked_until_its_own_close`.
2. **A Write that rewrites a human file which already has a tell.** Only the changed lines count, and a line kept unchanged is not new even when it appears twice. Test: Task 6 `write_over_file_marks_only_changed_lines` and `duplicate_unchanged_lines_are_not_new`.
3. **A writer in a Claude Code worktree under `<repo>/.claude/worktrees/<id>/`.** It must be gated; only a repo's own `.claude/` folder is exempt. Test: Task 6 `worktree_under_dot_claude_is_not_exempt`; Task 7 `hook_gates_a_worktree_under_dot_claude`.
4. **An old, missing or crashing binary.** An old binary answers `slop` with a usage error and exit 2. Expected: the edit is allowed and only exit 10 blocks. Test: Task 8 `binary_exit_2_allows`, `missing_binary_allows` and `binary_crash_allows`.
5. **Real text forms.** These must be caught: curly apostrophes (`It’s`), phrases wrapped over two lines, and identifiers. These must pass: technical uses such as `elevate privileges`, `seamless steel pipe`, `It's not installed by default; it's added by setup.`, backticked names and URL slugs. Test: Task 4 `curly_apostrophes_match`, `phrases_match_across_a_line_wrap` and `literal_uses_are_not_flagged`; Task 1 `should_pass_entries_raise_nothing`.

---

## Decisions

**D1. Where the checker lives.**

| | A. `anthology-slop` crate in the binary (chosen) | B. A separate tool (Vale binary + YAML styles, or a Node detector) |
|---|---|---|
| Install | Nothing new: the binary is already pinned, verified and on every machine | A second download per OS, a second lock and a second verifier, or Node on the host (no host installs) |
| Platforms | The same targets as `anthology` (macOS, Linux musl) | Vale ships per-OS Go binaries; Node needs a runtime |
| Context | The rules see the whole post-edit file, so fences and frontmatter are known | Vale sees files; it does not replay an Edit |
| Cost | We port and test the rules ourselves | Rules come ready-made but are tuned for whole documents and editorial review |

B loses on install and on new-text-only checking. Cost if wrong: about 1,500 lines of Rust to keep.

**D2. Hook event.** PostToolUse exit 2 (the research's choice) shows Claude an error, but the text is already on disk, and an agent that moves on ships it. PreToolUse exit 2 refuses the call, so nothing lands until the passage is rewritten. The repo tests also pin the events to SessionStart and PreToolUse. Chosen: PreToolUse with matcher `Write|Edit|MultiEdit`. Cost if wrong: an agent pays one extra rewrite round per refusal.

**D3. Design checking.**

| | Port the code-certain rules to Rust (chosen) | Bundle impeccable `detect` |
|---|---|---|
| Size | About 400 lines in the binary | One engine binary per OS in the plugin (megabytes each) |
| Platforms | Every `anthology` target | Only the targets upstream builds |
| Browser | None | Its render checks drive Chrome over CDP, which the browser rule forbids |
| False positives | Only 9 rules that read source code, each with negative tests | 62 rules, several judging taste |
| Licence | MIT (avoid-ai-design detector) | Apache-2.0 |

The remaining P0/P1 tells become the ui-reviewer's fixed rubric (Task 9).

**D4. Stop hook.** No Stop or SubagentStop hook:
- The cutover test pins the events.
- PreToolUse already refuses every Write, Edit and MultiEdit.
- With `stop_hook_active`, a Stop hook can force at most one continuation and then has to let the agent stop. It is never a hard barrier.

What it would add is files written by Bash (scaffolders, `sed`). The after-work mod covers that: when a writer reports, it runs `anthology slop check --since <lead HEAD>` in the writer's worktree and appends gaps for the lead, who can resume the agent (Task 10).

**D5. ui-taste.** Not refreshed. It is Uizze's adaptation of impeccable skill-v4.6.0, pinned by checksum, and only Uizze can move it. The gate and the new rubric do not depend on it.

**D6. Who is gated.** Every session and agent where the plugin is on, for these files:
- `.md`, `.markdown` and `.mdx` (Markdown);
- `.html` and `.htm` (Html);
- `.tsx` and `.jsx` (Jsx);
- `.css` and `.scss` (Css).

A path is exempt when it is under the Claude config folder (`CLAUDE_CONFIG_DIR`, else `$HOME/.claude`). It is also exempt when its path relative to the nearest folder holding `.git` has one of these components: `.git`, `.claude`, `.superpowers`, `node_modules`, `vendor`, `third_party`, `dist`, `build`, `.next`, `coverage`, `fixtures`, `__fixtures__`, `__snapshots__`.

The switch is `ANTHOLOGY_SLOP_GATE=0` in the environment. It turns off both the hook and the after-work part.

**D7. Fail mode.** The script allows on everything but binary exit 10. Only a hook killed at its 60 s timeout blocks, through `onFailure`, and the script does nothing slow.

**D8. JSX and TSX.** Only JSX text between tags and the string values of `alt`, `title`, `placeholder`, `aria-label` and `label` count as text. General string literals are never text (class names, keys, URLs and logs live there).

## The v1 rules

`B` = block (one hit refuses). `D` = density (only the new text is counted). The kinds are `md` = Markdown, `html`, `jsx`, `css`. Catalogue IDs are avoid-ai-design's.

```text
id                      sev kinds          summary (string in RULES)                                  hint (string in RULES)
text.ai-vocab           B   md,html,jsx    stock AI vocabulary                                        say the concrete thing the word stands for: the step, the number, the part
text.stock-phrase       B   md,html,jsx    stock opener or cliche                                     cut the lead-in and start with the point
text.chat-residue       B   md,html,jsx    chat-assistant wording in a file                           delete it: chat wording does not belong in a file
text.not-x-but-y        B   md,html,jsx    denied-alternative contrast                                 state the positive claim once, without the denied alternative
text.hype-cta           B   md,html,jsx    hype call to action                                        say what happens when the reader acts, with one fact
text.em-dash-density    D   md,html        em dashes: 3 or more, over 1 per 120 words                 use commas, colons, parentheses or full stops for most of these dashes
text.triad-density      D   md,html        lists of three: 3 or more, over 1 per 150 words            keep the lists the content needs; vary or cut the rest
ui.purple-gradient      B   css,html,jsx   purple or indigo gradient (C1)                              pick a gradient from the brand palette, or use a flat colour
ui.gradient-text        B   css,html,jsx   gradient-filled text (C6)                                   use a solid text colour; put emphasis in size or weight
ui.shadcn-default       B   css,html       untouched shadcn/ui base theme (K1)                         set the theme tokens to the product's own palette
ui.glow-orb             B   css,html,jsx   decorative blurred orb (C11)                                remove the orb, or replace it with an image or shape that carries meaning
ui.effect-library       B   jsx            stock effect-library component (K10)                        build the effect the design needs, or drop it
ui.placeholder-media    B   css,html,jsx   placeholder avatar or image service (IM4)                   use the product's real media or a local asset
ui.placeholder-text     B   html,jsx       placeholder copy                                            write the real copy for this screen
ui.five-star-row        B   html,jsx       five-star row (IM5)                                         show a real rating with its source, or no stars
ui.generator-signature  B   html,jsx       generator signature left in (F1)                            remove the generator tag or package
```

Every pattern is case-insensitive (`(?i)`). `['’]` stands for either apostrophe, and `\s+` sits between words so a soft line wrap still matches. The exact patterns are in Tasks 4 and 5.

### Waivers and specimens

~~~text
Line waiver (same line as the finding; reason of 8 or more characters; known rule id):
  Markdown/HTML:  <!-- slop-ok: text.ai-vocab the product is called Seamless Sync -->
  JSX:            {/* slop-ok: ui.purple-gradient brand colour from the style guide */}
  CSS/JS:         /* slop-ok: ui.purple-gradient brand colour from the style guide */   or   // slop-ok: ...
A density finding is waived by a valid waiver for that rule on any line of the new text.

Ignore region (whole-line markers, at most 3 spaces of indent; an unclosed start runs to the end):
  <!-- slop-ignore-start --> ... <!-- slop-ignore-end -->
  /* slop-ignore-start */ ... /* slop-ignore-end */      {/* slop-ignore-start */} ... {/* slop-ignore-end */}
A start marker may carry a reason after the keyword. A region masks everything for every rule.

Always masked in Markdown (never checked): frontmatter, fenced code, inline code, HTML comments, HTML tags,
blockquote lines, URLs and link targets, reference link definitions, MDX import/export lines,
double-quoted spans (straight or curly, one line, at most 300 characters).
~~~

The block message tells agents that waivers are only for wording the user asked for word for word. The after-work check lists every waiver and region an agent adds (Task 10), and the ui-reviewer lists them too (Task 9).

### Prompt edits in this plan (for the later copywriting plan to merge with)

Only these lines change in agent prompts, all in Task 9:
- `docs-writer.md`, `marketer.md` and `product-planner.md`: one Startup line (humanizer) and one Do line (humanizer pass and waiver rule).
- `ui-reviewer.md`: one Startup line (avoid-ai-design) and one Do line (rubric).
- `ui-designer.md`: one Startup line (avoid-ai-design, before the website-clone line) and one Do line.

## Waves

| Wave | Tasks | Starts when |
|---|---|---|
| A | 1 | now; freezes every interface below |
| B | 2, 3, 4, 5, 6, 8, 9, 10, 11 | Task 1 is merged into `build` |
| C | 7 (needs 2, 4, 6), 12 (needs 2 to 6) | their inputs are merged |
| D | 13 | everything is merged and CI is green |

The file sets in each wave are disjoint. Each task adds its own changelog fragment file. Task 1 bumps both plugin versions, so later branches never touch `plugin.json`.

---

### Task 1: The frozen interface, the engine and the wiring

**Files:**
- Create: `crates/anthology-slop/Cargo.toml`, `crates/anthology-slop/NOTICE`, `crates/anthology-slop/src/lib.rs`, `src/mask/mod.rs`, `src/mask/common.rs`, `src/mask/markdown.rs`, `src/mask/html.rs`, `src/mask/jsx.rs`, `src/mask/css.rs`, `src/rules/mod.rs`, `src/rules/text.rs`, `src/rules/density.rs`, `src/rules/ui.rs`, `src/rules/color.rs`, `src/edit.rs`, `src/paths.rs`, `src/report.rs`
- Create: `crates/anthology-slop/tests/corpus.rs`, `crates/anthology-slop/tests/fixtures/should-pass.txt`, `crates/repo-tests/tests/slop_corpus.rs`
- Create: `crates/anthology-cli/src/cmd/slop.rs` (stub), `changelog.d/anthology-guardrails/60-binary-0-2-10.md`
- Modify: `Cargo.toml` (workspace version `0.2.9` to `0.2.10`; add `anthology-slop = { path = "crates/anthology-slop" }` to `[workspace.dependencies]`), `Cargo.lock`, `fuzz/Cargo.lock`, `crates/anthology-cli/Cargo.toml` (`anthology-slop.workspace = true`), `crates/repo-tests/Cargo.toml` (`anthology-slop.workspace = true`), `crates/anthology-cli/src/cmd/mod.rs` (`pub mod slop;`, `Some("slop") => slop::run(rest, stdin)`, USAGE line `  slop hook|check|rules`), `crates/anthology-cli/tests/commands.rs` (rows `["slop","hook"]`, `["slop","check"]`, `["slop","rules"]`), `ci/test-weights.tsv` (row `anthology-slop	3`), `plugins/anthology/.claude-plugin/plugin.json` (`0.6.0`), `plugins/anthology-guardrails/.claude-plugin/plugin.json` (`0.2.19`)

**Interfaces (frozen; later tasks use exactly these names):**

```rust
// lib.rs
pub mod edit; pub mod paths; pub mod report;
mod mask; mod rules;
pub use mask::{View, Waiver, MASK};
pub use rules::RULES;
pub const MAX_DOC: usize = 1 << 20;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub enum Kind { Markdown, Html, Jsx, Css }
impl Kind { pub fn name(self) -> &'static str } // "markdown" | "html" | "jsx" | "css"
pub fn kind_of(path: &std::path::Path) -> Option<Kind>; // md markdown mdx | html htm | tsx jsx | css scss; extension case-insensitive
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Severity { Block, Density }
impl Severity { pub fn name(self) -> &'static str } // "block" | "density"
pub struct Rule { pub id: &'static str, pub severity: Severity, pub kinds: &'static [Kind],
                  pub summary: &'static str, pub hint: &'static str, pub(crate) run: rules::Run }
pub fn rule(id: &str) -> Option<&'static Rule>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)] pub struct Span { pub start: usize, pub end: usize }
impl Span { pub fn intersects(self, other: Span) -> bool } // start < other.end && other.start < end
#[derive(Clone, Debug, PartialEq, Eq)] pub struct Hit { pub rule: &'static str, pub span: Span, pub parts: Vec<Span>, pub excerpt: String }
#[derive(Clone, Debug, PartialEq, Eq)] pub struct Finding { pub rule: &'static str, pub line: usize, pub excerpt: String, pub waiver: Option<String> }
pub fn check(kind: Kind, doc: &str, new: &[Span]) -> Vec<Finding>;      // = check_with(kind, doc, new, RULES)
pub fn check_all(kind: Kind, doc: &str) -> Vec<Finding>;                // new = [0, doc.len())
pub fn waivers_in(kind: Kind, doc: &str, new: &[Span]) -> Vec<Waiver>;  // waiver comments and region starts on lines touching `new`
pub fn line_of(doc: &str, at: usize) -> usize;                          // 1-based
pub fn excerpt(doc: &str, span: Span) -> String;                        // whitespace collapsed, MASK bytes dropped, over 60 chars: first 57 + "..."
pub(crate) fn check_with(kind: Kind, doc: &str, new: &[Span], rules: &[Rule]) -> Vec<Finding>;

// mask/mod.rs
pub const MASK: u8 = 0x01;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View { pub text: String, pub code: String, pub style: Vec<Span>, pub classes: Vec<Span>,
                  pub dash_skip: Vec<Span>, pub waivers: Vec<Waiver> }
// text: doc with every non-text byte set to MASK (newlines kept); code: doc with comments and ignore
// regions set to MASK; style: CSS regions of `code`; classes: class/className values in `code`;
// dash_skip: ranges whose dashes the density rule skips. Every string has doc.len() bytes.
impl View { pub fn plain(doc: &str) -> View } // text = code = doc, style = [0, len), nothing else
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Waiver { pub line: usize, pub rule: Option<&'static str>, pub reason: String, pub valid: bool } // rule None = region start
pub(crate) fn view(kind: Kind, doc: &str) -> View; // dispatches to markdown::view, html::view, jsx::view, css::view (Task 1 stubs: View::plain)

// mask/common.rs (implemented in Task 1)
pub(crate) fn blank(buf: &mut [u8], span: Span);            // every byte but b'\n' becomes MASK
pub(crate) fn regions(doc: &str) -> (Vec<Span>, Vec<Waiver>); // ignore regions and their start-marker waivers
pub(crate) fn comment_waivers(doc: &str, comment: Span) -> Vec<Waiver>;
pub(crate) fn mask_quotes(buf: &mut [u8]);
pub(crate) fn mask_urls(buf: &mut [u8]);
pub(crate) fn into_string(buf: Vec<u8>) -> String;          // String::from_utf8; masking replaces whole chars so it cannot fail

// rules/mod.rs
pub(crate) enum Run { Text(fn(&View) -> Vec<Hit>), Density(fn(&View, &[Span]) -> Vec<Hit>) }
pub static RULES: &[Rule]; // the 16 rules of "The v1 rules", in that order, with those summary and hint strings
// rules/text.rs (Task 4): ai_vocab, stock_phrase, chat_residue, not_x_but_y, hype_cta : fn(&View) -> Vec<Hit>
// rules/density.rs (Task 4): em_dash, triad : fn(&View, &[Span]) -> Vec<Hit>
// rules/ui.rs (Task 5): purple_gradient, gradient_text, shadcn_default, glow_orb, effect_library,
//                       placeholder_media, placeholder_text, five_star_row, generator_signature : fn(&View) -> Vec<Hit>
// rules/color.rs (Task 5): pub(crate) struct Rgba { r: f64, g: f64, b: f64, a: f64 }, parse_color, hsl, is_violet, colors_in
// Task 1 stubs: every rule fn returns Vec::new().

// edit.rs (Task 6; stubs return None / empty)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolEdit { Write { content: String }, Edit { old: String, new: String, replace_all: bool },
                    MultiEdit { edits: Vec<(String, String, bool)> } }
impl ToolEdit { pub fn tool(&self) -> &'static str } // "Write" | "Edit" | "MultiEdit"
pub fn apply(before: Option<&str>, edit: &ToolEdit) -> Option<(String, Vec<Span>)>;
pub fn added_lines(before: &str, after: &str) -> Vec<Span>;
pub fn line_spans(doc: &str, ranges: &[(usize, usize)]) -> Vec<Span>; // 1-based inclusive line ranges
// paths.rs (Task 6)
pub const EXEMPT_DIRS: [&str; 13] = [".git", ".claude", ".superpowers", "node_modules", "vendor", "third_party",
    "dist", "build", ".next", "coverage", "fixtures", "__fixtures__", "__snapshots__"];
pub fn repo_root(path: &std::path::Path) -> Option<std::path::PathBuf>;
pub fn exempt(path: &std::path::Path, root: Option<&std::path::Path>, config_dir: Option<&std::path::Path>) -> bool;
pub fn shown(path: &std::path::Path, root: Option<&std::path::Path>) -> String;
// report.rs (Task 6)
pub const MAX_SHOWN: usize = 20;
pub fn hook_message(tool: &str, shown: &str, findings: &[Finding]) -> String;
pub fn check_line(shown: &str, f: &Finding) -> String;
pub fn waiver_line(shown: &str, w: &Waiver) -> String;
pub fn rules_table() -> String;

// crates/anthology-cli/src/cmd/slop.rs (Task 1 stub: Output::not_ported_sub("slop", &["hook", "check", "rules"], args))
pub fn run(args: &[std::ffi::OsString], stdin: &mut dyn std::io::Read) -> super::Output;
pub const HOOK_BLOCK: i32 = 10;
```

**The CLI contract (frozen, used by Tasks 7, 8, 10):**

~~~text
anthology slop hook                      stdin: Claude Code PreToolUse JSON. 0 = allow (no output), 10 = block (message on stderr).
anthology slop check [--root DIR] [--since REF] [PATH...]
    No --since: each PATH (file, or folder walked, exempt folders skipped) is checked whole.
    --since REF: lines added between `git merge-base REF HEAD` and the working tree, plus untracked files whole.
    stdout, per file: one check_line per unwaived finding, then one waiver_line per waiver in the new text.
      check_line:  {shown}:{line}: {rule}: "{excerpt}"
      waiver_line: {shown}:{line}: slop-ok {rule}: {reason}      or      {shown}:{line}: slop-ignore region
    exit 0 none unwaived, 1 some, 2 usage or git error (stderr: one line starting "anthology slop: ").
anthology slop rules                     stdout: rules_table(), one "{id}\t{severity}\t{kinds,comma-joined}\t{summary}" line per rule.
~~~

**The engine (`check_with`) algorithm:**
1. `view = mask::view(kind, doc)`.
2. For each rule whose `kinds` holds `kind`, run its fn: `Run::Text(f)` gets `f(&view)`, `Run::Density(f)` gets `f(&view, new)`.
3. Keep a Block hit when any of its `parts` (or `[span]` when `parts` is empty) intersects a `new` span. Keep every Density hit.
4. `line = line_of(doc, hit.span.start)`.
5. Waiver for a Block hit: a `valid` waiver with `rule == Some(id)` and the same line. Waiver for a Density hit: a valid waiver for the rule on any line touched by `new`. `Finding.waiver` = that waiver's reason.
6. Sort by `(line, rule)`. Drop repeats of the same `(line, rule)`.

**Waiver parsing:**
- Inside a comment, the waiver regex is `slop-ok:\s*([a-z]+\.[a-z0-9-]+)(?:\s+(.*?))?\s*(?:-->|\*/\s*\}?)?\s*$`.
- A waiver is `valid` when `rule(id)` is `Some` and the trimmed reason has 8 or more characters.
- The region marker regex is `(?m)^[ \t]{0,3}(?:<!--|\{?/\*|//)\s*slop-ignore-(start|end)\b[^\n]*$`.

**Quote masking.** A `"` or `“` opens a span only when the byte before it is not an ASCII letter or digit. The span closes at the next `"` or `”` on the same line, within 300 characters, with no backtick between. The content is masked and the quote marks are kept.

**URL masking:** `https?://[^\s)>\]"']+`.

- [ ] **Step 1: Write the failing tests**

In `src/lib.rs` `#[cfg(test)]`:

```rust
#[test] fn rules_are_the_sixteen_v1_rules() {
    let ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
    assert_eq!(ids, ["text.ai-vocab","text.stock-phrase","text.chat-residue","text.not-x-but-y","text.hype-cta",
        "text.em-dash-density","text.triad-density","ui.purple-gradient","ui.gradient-text","ui.shadcn-default",
        "ui.glow-orb","ui.effect-library","ui.placeholder-media","ui.placeholder-text","ui.five-star-row","ui.generator-signature"]);
    assert_eq!(RULES.iter().filter(|r| r.severity == Severity::Density).count(), 2);
    assert!(RULES.iter().all(|r| !r.kinds.is_empty() && !r.hint.is_empty()));
}
#[test] fn kind_of_maps_extensions() { /* a.md, a.MDX, a.markdown -> Markdown; a.htm -> Html; a.tsx, a.jsx -> Jsx; a.scss -> Css; a.ts, a.rs, a.txt -> None */ }
#[test] fn check_with_keeps_block_hits_that_touch_new_text() { /* test rule hits [0,4) and [10,14); new=[9,12) -> only the second */ }
#[test] fn a_hit_counts_when_any_part_touches_new_text() { /* span [0,4), parts [[0,4),[20,24)], new [21,22) -> kept */ }
#[test] fn density_hits_are_always_kept() {}
#[test] fn a_valid_line_waiver_waives_and_keeps_the_reason() { /* doc line "x <!-- slop-ok: text.ai-vocab named by the user -->" -> waiver Some("named by the user") */ }
#[test] fn a_waiver_without_a_reason_or_with_an_unknown_rule_does_not_waive() {}
#[test] fn findings_are_sorted_and_deduplicated() {}
#[test] fn line_of_counts_from_one() { assert_eq!(line_of("a\nb\nc", 0), 1); assert_eq!(line_of("a\nb\nc", 4), 3); }
#[test] fn excerpt_collapses_whitespace_and_caps_at_sixty() {}
```

The test rule is a local `Rule` with `run: Run::Text(fixed)`, passed to `check_with`.

In `src/mask/common.rs` `#[cfg(test)]`: `blank_keeps_newlines_and_length`, `regions_from_whole_line_markers_in_every_comment_style`, `an_unclosed_region_runs_to_the_end`, `quotes_are_masked_up_to_300_chars_on_one_line`, `a_quote_after_a_digit_does_not_open` (`15" screen`), `a_quote_span_never_crosses_a_backtick`, `urls_are_masked`, `comment_waivers_are_parsed`.

In `crates/anthology-slop/tests/corpus.rs`:

```rust
#[test] fn should_pass_entries_raise_nothing() {
    // fixtures/should-pass.txt: entries separated by a line that is exactly "%%"; each checked with check_all(Kind::Markdown, entry)
    // assert every entry gives no finding with waiver None; on failure list "entry N: rule line excerpt"
}
```

Fill `tests/fixtures/should-pass.txt` with exactly these entries:

~~~~text
To elevate privileges, the installer calls sudo once.
%%
The mill rolls seamless steel pipe in three diameters.
%%
Use a seamless texture so the floor tiles repeat without a visible edge.
%%
The museum restored a medieval tapestry from the abbey.
%%
The groundbreaking ceremony starts at nine.
%%
It's not installed by default; it's added by setup.
%%
Tests, builds and end-to-end suites run in containers that mount only the worktree.
%%
The `seamless` crate and the `delve` command are names, not prose.
%%
The rule flags "I hope this helps" when it appears outside quotes.
%%
> Let me know if you have any questions.
%%
See https://example.com/seamless-sync/elevate for the upstream notes.
%%
```text
I hope this helps. It's not a bug, it's a feature.
```
%%
<!-- slop-ignore-start -->
Unlock the full potential of your workflow.
<!-- slop-ignore-end -->
%%
| Rule | Example |
|---|---|
| a | — |
| b | — |
| c | — |
%%
Feel free to open an issue when the guard blocks a command you need.
%%
In summary, the guard blocks installs, launches and profile access.
%%
Of course the hook still runs when the binary is missing; it allows the edit.
%%
Certainly the binary is faster than the shell version.
%%
The deploy runs build, test, and release jobs; each job has its own log.
%%
The header gradient goes from #3b82f6 to #0ea5e9.
~~~~

In `crates/repo-tests/tests/slop_corpus.rs`:

```rust
#[test] fn the_rules_flag_nothing_in_the_human_docs() {
    // files: README.md, CONTRIBUTING.md, SECURITY.md, DECISIONS.md, CHANGELOG.md and every docs/**/*.md (repo::path)
    // for each: anthology_slop::check_all(Kind::Markdown, &text); collect findings with waiver None
    // assert empty; message lists "{rel}:{line}: {rule} \"{excerpt}\"" for each
}
#[test] fn the_corpus_holds_the_front_docs_and_the_pages() { /* at least 12 files, and README.md and docs/agents.md among them */ }
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `scripts/rust-test.sh test -p anthology-slop`
Expected: FAIL. The engine tests fail on the stubbed `check_with` (`left: [] right: [...]`). The corpus tests pass, because the rules are still stubs.

- [ ] **Step 3: Implement**

Write the types, `RULES` (summary and hint strings exactly as in the rules table), `check_with`, `line_of`, `excerpt`, `rule`, `kind_of`, `View::plain` and `mask/common.rs`. Every other fn is a stub with its frozen signature.

`NOTICE` carries:
- one line naming each upstream, with its copyright: `Copyright (c) 2025-2026 Tony Burns` (tbhb/vale-ai-tells), `Copyright (c) 2026 Conor Bronsdon` (conorbronsdon/avoid-ai-writing), `Copyright (c) 2026 ungspirit` (funboy322/avoid-ai-design), `Copyright (c) 2026 Leonxlnx` (leonxlnx/taste-skill);
- the MIT permission text once, copied from `plugins/anthology/knowledge/design-taste-frontend/LICENSE`.

Lock refresh:
- `scripts/rust-test.sh run cargo update --workspace`
- `scripts/rust-test.sh run cargo update --workspace --manifest-path fuzz/Cargo.toml`

Only version lines and the new crate entry may change in the locks.

The fragment `changelog.d/anthology-guardrails/60-binary-0-2-10.md` holds one line: `- The pinned anthology binary moves to 0.2.10, which adds the slop gate.`

- [ ] **Step 4: Run the tests and see them pass**

Run: `scripts/rust-test.sh test -p anthology-slop` and `scripts/rust-test.sh test -p anthology-cli --test commands` and `scripts/rust-test.sh test -p repo-tests --test slop_corpus --test ci_scope`
Expected: PASS. `anthology slop hook` exits 70 with `anthology: slop hook is not ported yet`.

Also run: `scripts/rust-test.sh run cargo clippy -p anthology-slop -p anthology-cli --all-targets -- -D warnings` and `scripts/rust-test.sh run cargo fmt --check`. Expected: clean.

- [ ] **Step 5: Commit**

`git commit -m "feat(slop): the anthology-slop crate interface, engine and corpus tests" -m "Shared-registration change: cmd/mod.rs, workspace, weights, plugin versions."`

---

### Task 2: Markdown masking

**Files:** Modify: `crates/anthology-slop/src/mask/markdown.rs` (only this file; tests inside it)

**Interfaces:**
- Consumes: `View`, `Waiver`, `MASK` and `mask::common::*` (Task 1).
- Produces: `pub(crate) fn view(doc: &str) -> View` for Markdown. `style` and `classes` are empty and `code` = `text`.

**What it masks in `text`:**
1. Ignore regions.
2. Frontmatter: the doc starts with `---\n` and runs to the next line that is `---` or `...`.
3. Fenced code. The open line has at most 3 spaces of indent and 3 or more backticks or tildes. The block closes on a line with the same character, at least as many times. An unclosed fence runs to the end.
4. HTML comments (multi-line). Their waivers go to `waivers`.
5. Inline code: a backtick run of n closes at the next run of n on the same line.
6. HTML tags matching `<[A-Za-z/!][^>\n]*>`.
7. Blockquote lines: `^[ ]{0,3}>`, but not when the `>` is followed by `=` or a digit.
8. URLs and link targets `\]\([^)\n]*\)`. The link text is kept.
9. Reference link definitions `^[ ]{0,3}\[[^\]]+\]:\s*\S+`.
10. Lines starting with `import ` or `export `.
11. Double-quoted spans (`mask_quotes`).

`dash_skip` holds every table row (first non-space character `|`) and every lead-term separator matching `^\s*[-*+]\s+\*\*[^*\n]+\*\*\s*[—–]` (the dash only).

- [ ] **Step 1: Write the failing tests:**
  - `frontmatter_is_masked`
  - `fenced_code_is_masked_until_its_own_close` (a ``` fence inside a ~~~ fence stays code)
  - `an_unclosed_fence_masks_to_the_end`
  - `inline_code_is_masked`
  - `html_comments_are_masked_and_their_waivers_kept`
  - `blockquote_lines_are_masked_but_comparisons_are_not` (`>= 5` and `>5` stay text)
  - `urls_and_link_targets_are_masked_link_text_is_not`
  - `quoted_spans_are_masked`
  - `ignore_regions_are_masked`
  - `table_rows_and_lead_term_dashes_are_dash_skip`
  - `mdx_import_and_export_lines_are_masked`
  - `html_tags_are_masked`
  - `offsets_and_newlines_are_preserved`: for every case, `text.len() == doc.len()` and the `\n` positions are equal.

  Each test asserts the masked bytes are `MASK` and the prose bytes are unchanged.
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p anthology-slop mask::markdown`. Expected: FAIL (the stub returns the doc unmasked).
- [ ] **Step 3: Implement `view`** as one pass over the lines for the block constructs (frontmatter, fences, regions, blockquotes, tables), then regexes for the inline constructs on the lines left.
- [ ] **Step 4: Run** the same command, then `scripts/rust-test.sh test -p anthology-slop --test corpus` and `scripts/rust-test.sh test -p repo-tests --test slop_corpus`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(slop): mask code, quotes and markup in Markdown`

---

### Task 3: HTML, JSX and CSS masking

**Files:** Modify: `crates/anthology-slop/src/mask/html.rs`, `src/mask/jsx.rs`, `src/mask/css.rs` (tests inside each)

**Interfaces:**
- Consumes: Task 1 `View` and `common`.
- Produces: `pub(crate) fn view(doc: &str) -> View` in each file.

**Css:**
- `code` = the doc with `/\*[\s\S]*?\*/` comments masked (waivers kept).
- `style` = `[0, len)`.
- `text` all masked.

**Html:**
- `code` masks comments `<!--[\s\S]*?-->` (waivers kept) and ignore regions.
- `style` holds the content of every `<style>` element and the value of every `style="..."` attribute.
- `classes` holds every `class="..."` or `class='...'` value.
- `text` keeps:
  - text outside tags, minus `<script>`, `<style>`, `<pre>` and `<code>` elements;
  - the values of `alt`, `title`, `placeholder`, `aria-label` and `label`;
  - the `content` of `<meta name="description">` and `<meta property="og:title|og:description">`.
- Quotes and URLs are masked in `text`.

**Jsx:**
- `code` masks:
  - `/* */` comments;
  - `//` comments to the end of line, only where `//` is at line start or after whitespace or one of `;{}(),`, so `'https://x'` is not a comment;
  - ignore regions.
- `style` = `[0, len)` of `code`.
- `classes` holds the values of `className="..."`, `className='...'` and `className={...}` (no nested braces; the whole brace content).
- `text` keeps JSX text segments:
  - a segment is `>([^<>{}]+)[<{]` or `\}([^<>{}]+)<`;
  - it counts only when it holds a letter and none of `=`, `;`, `&&`, `||`, `=>`, `(` or `)`.
- `text` also keeps the string values of `alt`, `title`, `placeholder`, `aria-label` and `label`.
- Waivers come from comments, including `{/* slop-ok: ... */}`.

- [ ] **Step 1: Write the failing tests.**
  - `html.rs`:
    - `text_outside_tags_is_text`
    - `script_style_pre_code_and_comments_are_not_text`
    - `listed_attributes_and_meta_descriptions_are_text`
    - `style_blocks_and_style_attributes_are_style_spans`
    - `class_values_are_class_spans`
    - `comment_waivers_are_kept`
  - `jsx.rs`:
    - `jsx_text_between_tags_is_text`
    - `code_between_angle_brackets_is_not_text` (`if (a > b && c < d) {}` gives no text)
    - `listed_string_attributes_are_text`
    - `class_name_values_in_all_three_forms_are_class_spans`
    - `comments_are_masked_but_urls_in_strings_are_not`
    - `jsx_comment_waivers_are_kept`
  - `css.rs`:
    - `comments_are_masked_and_waivers_kept`
    - `the_whole_file_is_style`
    - `text_is_all_masked`
  - Every test also asserts `text.len() == code.len() == doc.len()`.
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p anthology-slop mask::`. Expected: FAIL.
- [ ] **Step 3: Implement** the three `view` fns with `regex` scans. Element content is matched with `(?is)<(script|style|pre|code)\b[^>]*>.*?</\1>`, written without a backreference: one regex per tag.
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(slop): text, style and class views for HTML, JSX and CSS`

---

### Task 4: Text rules and density rules

**Files:** Modify: `crates/anthology-slop/src/rules/text.rs`, `src/rules/density.rs` (tests inside each)

**Interfaces:**
- Consumes: `View`, `Hit`, `Span` and `excerpt`.
- Produces: the 7 fns named in Task 1.
- All text rules match on `view.text`. Hit `span` = the match. `excerpt` = `crate::excerpt(&view.code, span)`; `code` keeps prose bytes, and for Markdown `code == text`.

**Patterns.** Each block below is one regex. The lines are alternatives joined with `|`, inside `(?i)\b(?:...)\b`, with `\s+` between words:

```text
ai_vocab (word list; then the literal exceptions below):
delv(?:e|es|ed|ing) | tapestr(?:y|ies) | seamless(?:ly)? | elevat(?:e|es|ing) | unleash(?:es|ed|ing)? |
supercharg(?:e|es|ing) | turbocharg(?:e|es|ing) | game-chang(?:er|ers|ing) | groundbreaking | unparalleled |
synerg(?:y|ies|istic) | holistic(?:ally)? | meticulous(?:ly)? | plethora | multifaceted | ever-evolving
literal exceptions, tested on the 40 bytes after the match (and the 30 before, where stated):
  seamless, seamlessly: after ^\s+(?:pipes?|tubes?|tubing|steel|knit|knitted|knitting|garments?|textures?|tiles?|tiling|tileable|patterns?|loops?|gutters?|flooring|stockings|socks|underwear)\b
  elevate, elevates, elevating: after ^\s+(?:the\s+|your\s+|their\s+|its\s+|a\s+)?(?:privileges?|permissions?|rights|access|process|shell|user|account|token|session|prompt)\b
  tapestry, tapestries: after ^\s+(?:weaving|weavers?|looms?|museum|kits?|wool|yarn|workshops?)\b  or before \b(?:woven|medieval|antique|hand-woven|wool|silk)\s+$
  groundbreaking: after ^\s+(?:ceremony|ceremonies)\b

stock_phrase:
in\s+today['’]s\s+(?:fast-paced|digital\s+age|digital\s+world|rapidly\s+evolving|ever-changing) |
in\s+(?:an|the)\s+ever-evolving | embark(?:s|ed|ing)?\s+on\s+(?:a|an|the|this|our|your)\s+(?:journey|quest|mission|adventure) |
let['’]s\s+dive\s+(?:in|into|deeper) | without\s+further\s+ado | gone\s+are\s+the\s+days | imagine\s+a\s+world\s+where |
look\s+no\s+further | in\s+the\s+realm\s+of | (?:a|stands\s+as\s+a)\s+testament\s+to | navigat(?:e|ing)\s+the\s+complexities |
unlock(?:s|ing)?\s+the\s+(?:full\s+)?(?:power|potential) | harness(?:es|ing)?\s+the\s+power\s+of |
it['’]s\s+(?:important|worth)\s+(?:to\s+note|noting)\s+that

chat_residue (the last alternative uses (?m) and no leading \b):
(?:i\s+)?hope\s+(?:this|that)\s+helps | hopefully\s+(?:this|that)\s+helps | i\s+hope\s+(?:this|that)\s+(?:was|is)\s+helpful |
i\s+hope\s+you\s+(?:find|found)\s+this\s+(?:helpful|useful) |
let\s+me\s+know\s+if\s+(?:you\s+have\s+any|you\s+need\s+any|you\s+need\s+anything|there['’]s\s+anything) |
i['’]d\s+be\s+(?:happy|glad)\s+to\s+help | i['’]m\s+(?:happy|glad|here)\s+to\s+help | great\s+question |
you['’]re\s+absolutely\s+right | as\s+an\s+ai(?:\s+language\s+model|\s+assistant)?,?\s+i |
here['’]s\s+(?:a|the)\s+(?:revised|rewritten|updated|polished|improved)\s+(?:version|draft) |
(?:^|[.!?]\s+)(?:certainly|absolutely|sure)!

not_x_but_y (five separate regexes):
1 \b(?:it|this|that)(?:['’]s|\s+is|\s+was)\s+not\s+(?:just\s+|merely\s+|simply\s+|only\s+|really\s+)?(?:about|a|an|the|your|our)\s+[^;,—–:.!?\n]{1,80}[;,—–:]\s*(?:it|this|that)(?:['’]s|\s+is)\b
2 \b(?:it|this|that)\s+(?:isn['’]t|wasn['’]t)\s+(?:just\s+|merely\s+|simply\s+|only\s+|really\s+)?(?:about|a|an|the|your|our)\s+[^;,—–:.!?\n]{1,80}[;,—–:]\s*(?:it|this|that)(?:['’]s|\s+is)\b
3 \b(?:isn['’]t|is\s+not|aren['’]t|are\s+not)\s+(?:just|merely|simply|only)\s+[^.!?\n]{1,60}[.;,—–:]\s+(?:it|they)(?:['’]s|['’]re|\s+is|\s+are)\b
4 \b(?:it|this)(?:['’]s\s+not|\s+is\s+not|\s+isn['’]t)\s+(?:just\s+|really\s+)?about\s+[^.!?\n]{1,80}\.\s+it(?:['’]s|\s+is)\s+about\b
5 \b(?:it['’]s|this\s+is)\s+(?:more\s+than|less\s+about)\s+[^;,—–:.!?\n]{1,60}(?:[;,—–:]\s*it['’]s|\s+and\s+more\s+about)\b

hype_cta:
ready\s+to\s+(?:transform|supercharge|elevate|level\s+up|take\s+your) | to\s+the\s+next\s+level |
join\s+(?:thousands|millions|hundreds)\s+of | transform\s+(?:the\s+way\s+you|your\s+[a-z]+\s+(?:experience|workflow|business)) |
revolutioni[sz]e\s+(?:the\s+way|your|how) | the\s+future\s+of\s+[a-z]+\s+is\s+(?:here|now) | say\s+goodbye\s+to |
at\s+your\s+fingertips | in\s+just\s+a\s+few\s+clicks | we['’]ve\s+got\s+you\s+covered | the\s+possibilities\s+are\s+endless |
(?:start|begin)\s+your\s+journey | don['’]t\s+miss\s+out
```

**Density rules** count only inside `new` spans, on `view.text`. A word is a whitespace-separated token holding a letter; MASK bytes separate tokens.

`em_dash`:
- Count `—` (U+2014) and ` – ` (a space, U+2013, a space) outside `dash_skip`.
- Fire when `n >= 3 && n * 120 > words`.
- One hit: `span` = the first counted dash. `excerpt` = `format!("{n} em dashes in {words} words (at most 1 per 120)")`.

`triad`:
- Count non-overlapping matches of `(?i)\b[a-z]{4,},\s+[a-z]{4,},?\s+(?:and|or)\s+[a-z]{4,}\b`.
- Fire when `n >= 3 && n * 150 > words`.
- `excerpt` = `format!("{n} lists of three in {words} words (at most 1 per 150)")`.

- [ ] **Step 1: Write the failing tests** on `View::plain(..)`. One block per fn, with positives from the patterns and negatives, plus:
  - `curly_apostrophes_match`: `It’s not a tool, it’s a platform.` gives one `text.not-x-but-y` hit.
  - `phrases_match_across_a_line_wrap`: `I hope this\nhelps.` gives one `text.chat-residue` hit.
  - `literal_uses_are_not_flagged`: each literal exception, plus `It's not installed by default; it's added by setup.` and `Feel free to open an issue.`, gives no hit.
  - `masked_bytes_never_join_a_phrase`: `hope \x01\x01\x01 helps` gives no hit.
  - `em_dash`:
    - `three_dashes_in_twenty_words_fire`;
    - `ten_dashes_in_two_thousand_words_pass`;
    - `dashes_in_dash_skip_spans_do_not_count`;
    - `only_dashes_in_new_text_count`.
  - `triad`:
    - `three_triads_in_a_hundred_words_fire`;
    - `two_triads_never_fire`.
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p anthology-slop rules::`. Expected: FAIL (the stubs return no hits).
- [ ] **Step 3: Implement.** Compile each regex once (`std::sync::LazyLock`).
- [ ] **Step 4: Run** that command, `--test corpus`, and `scripts/rust-test.sh test -p repo-tests --test slop_corpus`. Expected: PASS. A corpus finding means a pattern is too broad. Narrow the pattern and add the corpus sentence as a negative test. Never waive in the docs.
- [ ] **Step 5: Commit** `feat(slop): the text and density rules`

---

### Task 5: UI rules

**Files:** Modify: `crates/anthology-slop/src/rules/ui.rs`, `src/rules/color.rs` (tests inside each)

**Interfaces:**
- Consumes: `View` (`style`, `classes`, `code`, `text`).
- Produces: the 9 UI fns and the `color.rs` helpers:
  - `parse_color(&str) -> Option<Rgba>` reads `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()` (commas or spaces, `/ a` and `a%`), `hsl()` and `hsla()`;
  - `hsl(Rgba) -> (f64, f64, f64)` returns `(h 0..360, s 0..100, l 0..100)`;
  - `is_violet(c)` = h in 228..=300, s >= 40, 15 < l < 90;
  - `colors_in(&str) -> Vec<Rgba>`.
- A "rule body" is a `([^{}]+)\{([^{}]*)\}` match inside a `style` span.

```text
purple_gradient (C1): in style spans, each (?i)(?:linear|radial|conic)-gradient\(([^;{}]*)\) whose argument, with var(--x) resolved
    against `--x: value` declarations anywhere in `code` (depth at most 6), holds a colour with is_violet; span = the call.
  In each class span: \b(?:from|via|to)-(?:indigo|violet|purple|fuchsia)-\d{2,3}\b together with \bbg-(?:gradient|linear|radial)\S*; span = the class value.
gradient_text (C6): a rule body holding (?:-webkit-)?background-clip\s*:\s*['"]?text and (?:-webkit-text-fill-color|color)\s*:\s*['"]?transparent,
    or backgroundClip\s*:\s*['"]text['"] and (?:WebkitTextFillColor|color)\s*:\s*['"]transparent['"]; parts = the two declarations.
  A class span holding \bbg-clip-text\b and \btext-transparent\b; span = the class value.
shadcn_default (K1): in style spans, --primary\s*:\s*(?:oklch\(0\.205 0 0\)|222\.2 47\.4% 11\.2%|240 5\.9% 10%|0 0% 9%); span = the declaration.
glow_orb (C11): a rule body holding border-radius\s*:\s*(?:50%|100%|9{3,}px) and filter\s*:\s*blur\(\s*(\d+) with the number >= 24; parts = both declarations.
  A class span holding \brounded-full\b, \bblur-(?:2xl|3xl|\[\d{2,}px\]) and \b(?:absolute|fixed|-z-10)\b.
effect_library (K10), on code: <(?:Spotlight|BackgroundBeams|BackgroundBeamsWithCollision|AnimatedBeam|ShimmerButton|NumberTicker|BorderBeam|
    Meteors|SparklesCore|LampContainer|AuroraBackground|RetroGrid|OrbitingCircles|TextGenerateEffect|TypewriterEffect|HeroHighlight|
    WavyBackground|InfiniteMovingCards|MagicCard|NeonGradientCard|ShineBorder|AnimatedGradientText|RainbowButton|FlipWords|
    HoverBorderGradient|MovingBorder|GlowingEffect)\b   or   components/(?:magicui|aceternity)/
placeholder_media (IM4), on code: (?i)\b(?:api\.dicebear\.com|i\.pravatar\.cc|ui-avatars\.com|robohash\.org|randomuser\.me/api/portraits|
    placehold\.co|via\.placeholder\.com|picsum\.photos|source\.unsplash\.com/random)
placeholder_text, on text: (?i)\blorem\s+ipsum\b|\[(?:your\s+|company\s+|brand\s+|business\s+|product\s+)?(?:company|brand|business|product)\s+name\]|\[insert\b
five_star_row (IM5), on code: ★{5}|(?:<Star(?:Icon)?\b[^>]*/>\s*){5}|(?:\[\.\.\.Array\(5\)\]|Array\.from\(\{\s*length\s*:\s*5\s*\}\)|Array\(5\)\.fill\([^)]*\))[\s\S]{0,200}?(?:<Star|★)
generator_signature (F1), on code: (?i)lovable-tagger|gpt-?engineer|\bv0\.dev\b|\bbolt\.new\b|name=["']generator["'][^>]*content=["'](?:v0|lovable|bolt)
```

- [ ] **Step 1: Write the failing tests** with Views built by hand (`View { style: vec![..], classes: vec![..], ..View::plain(doc) }`):
  - `color.rs`:
    - `parses_every_colour_form`;
    - `indigo_500_is_violet_and_blue_500_is_not` (`#6366f1` hue about 239; `#3b82f6` hue about 217).
  - `ui.rs`, one positive and one negative per rule:
    - `linear-gradient(135deg, #6366f1, #a855f7)` fires; `linear-gradient(#3b82f6, #0ea5e9)` does not;
    - `--brand: #7c3aed;` plus `linear-gradient(var(--brand), white)` fires;
    - `bg-gradient-to-r from-indigo-500 to-purple-600` fires; `from-indigo-500` alone does not;
    - the kebab, camel and Tailwind gradient-text forms fire; `background-clip: text` alone does not;
    - `oklch(0.205 0 0)` fires; `oklch(0.45 0.2 30)` does not;
    - `blur(96px)` with `50%` fires; `blur(8px)` does not;
    - `<ShimmerButton>` fires; `<Marquee>` does not;
    - `i.pravatar.cc` fires; `example.com/a.png` does not;
    - `Lorem ipsum` in `text` fires; the same string only in `code` does not;
    - `★★★★★` fires; `★★★★☆` does not;
    - `lovable-tagger` fires.
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p anthology-slop rules::ui rules::color`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** that command and `--test corpus`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(slop): the code-certain UI rules`

---

### Task 6: Edit replay, exemptions and messages

**Files:** Modify: `crates/anthology-slop/src/edit.rs`, `src/paths.rs`, `src/report.rs` (tests inside each)

**Interfaces:**
- Consumes: `Span`, `Finding`, `Waiver`, `RULES` and `Kind::name`.
- Produces: the frozen fns of Task 1.

`apply`:
- Write: with no `before`, the result is `(content, [0, len))`. With `before`, it is `(content, added_lines(before, content))`.
- Edit:
  - `old` empty and `before` None or empty: `(new, [0, len))`.
  - `old` empty otherwise: `None`.
  - Occurrence count 0, or more than 1 without `replace_all`: `None`.
  - Otherwise replace (all occurrences when `replace_all`) and return one span per inserted non-empty `new`.
- MultiEdit: apply each edit in order as an Edit on the running text; any `None` gives `None`. Spans = `added_lines(before.unwrap_or(""), after)`.

`added_lines`:
- Split `after` into lines, without their line endings. Keep a count of `before`'s lines.
- For each `after` line in order: when its count is above 0, take one from the count. Otherwise its span is new.
- Skip blank lines and merge adjacent spans.

`repo_root` walks up from the file's folder, at most 64 levels, to the first folder holding `.git` (a folder or a file).

`exempt` is true:
- when `path` starts with `config_dir`;
- or when any component of `path.strip_prefix(root)` (or of `path` when there is no root) is in `EXEMPT_DIRS`.

`shown` = the path relative to the root when under it, else the path as given.

Message copy (exact):

~~~text
hook_message:
anthology slop gate: the {tool} of {shown} was not applied. Its new text has these AI tells:
  line {line}: {rule} "{excerpt}": {hint}
  ... (at most MAX_SHOWN lines, then)  and {k} more
Rewrite each passage in plain, specific words, then retry; a synonym swap is not a fix. Put a quoted specimen in a code span or between <!-- slop-ignore-start --> and <!-- slop-ignore-end -->. Add <!-- slop-ok: <rule> <reason> --> only for wording the user asked for word for word; the lead is told about each one.
(lines joined with "\n", ending with "\n"; only findings whose waiver is None are passed in)
check_line:  {shown}:{line}: {rule}: "{excerpt}"
waiver_line: {shown}:{line}: slop-ok {rule}: {reason}        (rule Some)
             {shown}:{line}: slop-ignore region               (rule None)
~~~

- [ ] **Step 1: Write the failing tests.**
  - `edit.rs`:
    - `write_new_file_is_all_new`
    - `write_over_file_marks_only_changed_lines`
    - `duplicate_unchanged_lines_are_not_new`
    - `edit_spans_cover_the_inserted_text`
    - `replace_all_spans_every_insertion`
    - `missing_or_ambiguous_old_is_none`
    - `empty_old_on_a_missing_file_creates_it`
    - `multiedit_applies_in_order_and_diffs_lines`
    - `multiedit_with_a_missing_old_is_none`
    - `line_spans_map_inclusive_ranges`
  - `paths.rs`:
    - `worktree_under_dot_claude_is_not_exempt`: root `/r/.claude/worktrees/w`, path `/r/.claude/worktrees/w/docs/a.md`, result false.
    - `a_repos_own_dot_claude_is_exempt`: root `/r`, path `/r/.claude/memory/x.md`, result true.
    - `node_modules_and_vendor_are_exempt`
    - `the_config_dir_is_exempt`
    - `repo_root_finds_a_git_folder_or_file` (tempdir)
    - `shown_is_relative_to_the_root`
  - `report.rs`:
    - `hook_message_is_the_exact_copy`: compare the whole string for two findings.
    - `hook_message_caps_at_twenty_and_counts_the_rest`
    - `check_and_waiver_line_formats`
    - `rules_table_has_one_tab_separated_line_per_rule`
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p anthology-slop edit:: paths:: report::`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(slop): edit replay, exempt paths and the gate's messages`

---

### Task 7: `anthology slop hook|check|rules`

**Files:**
- Modify: `crates/anthology-cli/src/cmd/slop.rs`
- Create: `crates/anthology-cli/tests/slop.rs`

**Interfaces:**
- Consumes: the whole `anthology_slop` API, `super::hook::read_input` (8 MiB cap), `anthology_core::Env`, `anthology_core::git::{Git, RealGit}` and `anthology_pycompat::json`.
- Produces: the CLI contract of Task 1.

**`hook` steps.** Any failure along the way gives exit 0 with no output.
1. `Env::from_process()`. When `ANTHOLOGY_SLOP_GATE` is `0`, exit 0.
2. Read stdin and parse it as JSON. `tool_name` must be `Write`, `Edit` or `MultiEdit`. Read `tool_input.file_path`; join it to `cwd` when it is relative.
3. `kind_of`, `repo_root`, then `exempt` with `config_dir` = `CLAUDE_CONFIG_DIR`, else `$HOME/.claude`.
4. Read `before`. A missing file gives None. A read error, non-UTF-8 content or more than `MAX_DOC` bytes gives exit 0.
5. Build `ToolEdit` from `content`, from `old_string`/`new_string`/`replace_all` (default false), or from `edits[]`. Run `apply`. None, or an `after` over `MAX_DOC`, gives exit 0.
6. `check`. Keep the findings whose waiver is None. When there are none, exit 0. Otherwise write `hook_message(edit.tool(), &shown, ..)` to stderr and exit `HOOK_BLOCK`.

Wrap the whole hook in `catch_unwind` with a silent panic hook, as `cmd/hook.rs` does. A panic gives exit 0.

**`check`:**
- Parse `--root`, `--since` and paths with `lexopt`.
- With `--since`, run git through `RealGit` in `--root`:
  - `rev-parse --show-toplevel`;
  - `merge-base <REF> HEAD`;
  - `diff --no-color --no-ext-diff --unified=0 -M <base> --`, parsing `+++ b/<path>` lines and `@@ -a,b +c,d @@` hunks (d defaults to 1; d = 0 adds no lines);
  - `ls-files --others --exclude-standard -z`.
- Skip files whose kind is None, exempt files and deleted files.

- [ ] **Step 1: Write the failing tests** in `tests/slop.rs`. Run the binary as `commands.rs` does: `env_clear`, a sandbox `HOME` and `CLAUDE_CONFIG_DIR`, and a temp repo with a `.git` folder.
  - `hook_blocks_a_write_with_a_tell`: the Write of `docs/a.md` holding `We delve into caching.` exits 10, and stderr starts with `anthology slop gate: the Write of docs/a.md was not applied.`.
  - `hook_allows_clean_text`: exit 0, empty stdout and stderr.
  - `hook_allows_an_edit_inside_a_code_fence`: the file has a fence; the Edit replaces a line inside it with `I hope this helps.`; exit 0.
  - `hook_checks_only_the_new_text_of_an_edit`: the file already has a tell elsewhere; a clean Edit exits 0.
  - `hook_ignores_other_tools_and_kinds`: Bash, and a `.rs` file, exit 0.
  - `hook_fails_open_on_bad_input`: input that is not JSON, a missing `file_path`, a non-UTF-8 file, a file over 1 MiB and an `old_string` not found all exit 0.
  - `hook_honours_the_switch`: `ANTHOLOGY_SLOP_GATE=0` exits 0.
  - `hook_skips_exempt_paths`: `.claude/memory/x.md` and `node_modules/p/README.md` exit 0.
  - `hook_gates_a_worktree_under_dot_claude`: a `.git` file in `<tmp>/.claude/worktrees/w`; a tell in `docs/a.md` there exits 10.
  - `check_reports_findings_and_exits_1`
  - `check_clean_exits_0`
  - `check_since_reports_only_added_lines`: commit a file with one tell, then add a line with another; `--since HEAD` reports only the second line.
  - `check_since_includes_untracked_files`
  - `check_lists_waivers`: a `slop-ok` line gives a `slop-ok` waiver line, and the exit code follows the unwaived findings.
  - `check_usage_errors_exit_2`: no paths and no `--since`; an unknown flag.
  - `rules_prints_every_rule`: 16 lines.
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p anthology-cli --test slop`. Expected: FAIL (the stub exits 70).
- [ ] **Step 3: Implement `cmd/slop.rs`.** Keep pure helpers in this file for unit tests: `fn hook_with(env: &Env, raw: &str) -> Output` and `fn check_with(env: &Env, git: &dyn Git, args: &[OsString]) -> Output`.
- [ ] **Step 4: Run** the same command, then `scripts/rust-test.sh run cargo clippy -p anthology-cli --all-targets -- -D warnings`. Expected: PASS and clean.
- [ ] **Step 5: Commit** `feat(cli): anthology slop hook, check and rules`

---

### Task 8: The PreToolUse hook

**Files:**
- Create: `plugins/anthology/hooks/slop-gate.sh` (mode 0755), `crates/repo-tests/tests/shims/slop_gate.rs`, `changelog.d/anthology/60-slop-gate.md`
- Modify:
  - `plugins/anthology/hooks/hooks.json`;
  - `crates/repo-tests/tests/shims.rs` (`mod slop_gate;` beside the other shim modules);
  - `crates/repo-tests/tests/shims/statics.rs` (`anthology_hooks_json_runs_session_start_sh_and_keeps_atl_hook` want);
  - `crates/repo-tests/tests/shims/planted.rs` (`the_other_hooks_run_no_planted_tool`);
  - `crates/anthology-runtime/tests/atl_install.rs` (`test_hooks_json_shape` want);
  - `ci/jobs/shellcheck.sh` (add `"$staging/anthology/hooks/slop-gate.sh"`).

**Interfaces:**
- Consumes: the CLI contract (exit 10 = block).
- Produces: the hook entry, appended as a second object in the `PreToolUse` array:

```json
{
  "matcher": "Write|Edit|MultiEdit",
  "hooks": [
    {
      "type": "command",
      "command": "/bin/sh",
      "args": ["${CLAUDE_PLUGIN_ROOT}/hooks/slop-gate.sh"],
      "timeout": 60,
      "onFailure": "block"
    }
  ]
}
```

**`slop-gate.sh` behaviour:**
1. `#!/bin/sh`, then a header comment: what it does, that it fails open, and that only exit 10 from the binary blocks.
2. `trap 'exit 0' HUP INT TERM`.
3. Find `anth_hooks` from `$0`, as `session-start.sh` does. When `lib.sh` is missing or does not load in a subshell, exit 0.
4. Source `lib.sh`, then `anth_paths "$anth_hooks/.."`. When `anth_resolve "$anth_hooks/.."` and `anth_installed` do not both succeed, exit 0. It never installs.
5. Run `anth_err=$("$ANTH_BIN" slop hook 2>&1 >/dev/null; printf 'x%s' "$?")`. Split off the code after the last `x`, as `session-start.sh` does; it must be digits only (`anth_only`).
6. Code 10: print the captured text to stderr (trailing newlines trimmed, one added) and exit 2. Any other code: exit 0. The script writes nothing to stdout.

- [ ] **Step 1: Write the failing tests:**
  - `slop_gate.rs`. Build a fake binary as `format!("#!/bin/sh\ncase ${{1:-}} in\n--version) printf '%s\\n' '{}'; exit 0 ;;\nslop) {body} ;;\nesac\nexit 0\n", version_line(V))`. Pin it with `Sandbox::new(release_of(V, &bytes))` and install it with `sb.install(&bytes, 0o755)`. Run it with `sb.run(sh, "anthology/hooks/slop-gate.sh", &[], payload)` inside `each_shell`, using the same script-path form the guard shim tests use.
    - `missing_binary_allows`: no install; exit 0 and empty output.
    - `binary_exit_10_blocks_with_its_stderr`: body `cat >/dev/null; printf 'anthology slop gate: x\n' >&2; exit 10`; exit 2, and stderr is exactly `anthology slop gate: x\n`.
    - `binary_exit_2_allows`: an old binary's usage error; exit 0.
    - `binary_crash_allows`: exit 101; exit 0.
    - `stdin_reaches_the_binary`: the body exits 10 only when stdin holds `MARK`.
    - `nothing_goes_to_stdout`.
  - The exact JSON wants in `statics.rs` and `atl_install.rs`.
  - In `planted.rs`, a third case: `slop-gate.sh` with no binary installed runs no planted tool and exits 0.
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p repo-tests --test shims` and `scripts/rust-test.sh test -p anthology-runtime --test atl_install`. Expected: FAIL.
- [ ] **Step 3: Write the script and the `hooks.json` entry.**
- [ ] **Step 4: Run** those two commands, `scripts/rust-test.sh test -p repo-tests --test cutover_hooks`, and `ci/jobs/shellcheck.sh`. Expected: PASS, and `shellcheck: N files clean`.
- [ ] **Step 5: Commit** `feat(anthology): the slop gate PreToolUse hook`. The fragment holds `- Add: the slop gate refuses a Write, Edit or MultiEdit whose new text holds a known AI-writing or AI-design tell, and names the line and phrase.`

The PR body carries the map lines from "Project map lines".

---

### Task 9: Knowledge and agent prompts

**Files:**
- Modify:
  - `vendor/sources.json` (2 entries, after `website-clone`);
  - `crates/repo-tests/src/repo.rs`;
  - `crates/repo-tests/tests/knowledge.rs` (30 to 32);
  - `crates/repo-tests/tests/licenses.rs` (folder and source counts 30 to 32, `checked` 29 to 31);
  - `crates/anthology-dev/tests/vendor/repo_api_test.rs` (32, and the fn name `..._the_32_...`);
  - `crates/anthology-dev/tests/vendor/sources_json_test.rs` (add `"humanizer"`, `"avoid-ai-design"` before `"atl"`);
  - `crates/anthology-dev/tests/smoke.rs` (`UI_KNOWLEDGE` 10 to 11, with `"avoid-ai-design"` before `"website-clone"`; the docs-writer, marketer, product-planner and ui-reviewer rows);
  - `crates/anthology-dev/src/startup_budget.rs` and `crates/repo-tests/tests/roster.rs` (`BUDGET`);
  - `crates/anthology-dev/tests/golden/dev.startup-budget.jsonl.gz` (re-blessed);
  - `plugins/anthology/agents/{docs-writer,marketer,product-planner,ui-reviewer,ui-designer}.md`;
  - `docs/agents.md` (roster table "Knowledge it reads" column).
- Create:
  - `vendor/patches/humanizer.patch`, `vendor/patches/avoid-ai-design.patch`;
  - `vendor/licenses/` files (written by `vendor fetch`);
  - `plugins/anthology/knowledge/humanizer/**`, `plugins/anthology/knowledge/avoid-ai-design/**` (written by `vendor build`);
  - `changelog.d/anthology/61-humanizer-and-design-rubric.md`.

**Interfaces:**
- Consumes: the rule IDs (Task 1), for the rubric's mapping line.
- Produces:
  - `knowledge/humanizer/SKILL.md` (frontmatter `name: humanizer`);
  - `knowledge/avoid-ai-design/SKILL.md` (frontmatter `name: avoid-ai-design`);
  - `knowledge/avoid-ai-design/ai-tells-catalog.md`, `aesthetic-directions.md` and `design-md-template.md` (upstream `references/`).

**Source entries:**

```json
{
  "name": "humanizer",
  "repo": "https://github.com/blader/humanizer",
  "path": "",
  "ref": "main",
  "commit": "a32f24407364c74c812b43c15926d638bee1e072",
  "license": "MIT",
  "license_file": "LICENSE",
  "target": "plugins/anthology/knowledge/humanizer",
  "transform": [{ "patch": "vendor/patches/humanizer.patch" }]
},
{
  "name": "avoid-ai-design",
  "repo": "https://github.com/funboy322/avoid-ai-design",
  "path": "references",
  "ref": "main",
  "commit": "60f5853a706873da3a30b3ddf3cc29f9683c885c",
  "license": "MIT",
  "license_file": "LICENSE",
  "target": "plugins/anthology/knowledge/avoid-ai-design",
  "transform": [{ "patch": "vendor/patches/avoid-ai-design.patch" }]
}
```

**`humanizer.patch`:**
- Delete every shipped file except `SKILL.md` and `LICENSE`.
- In `SKILL.md`'s "How to work" step 2, replace `ask for it or write a simpler sentence` with `write a simpler sentence, or name the missing detail in your report`, because subagents cannot ask.

**`avoid-ai-design.patch`** adds `SKILL.md` with this content. Specimens stay in code spans.

~~~~markdown
---
name: avoid-ai-design
description: Use as the fixed P0/P1 rubric when building or reviewing a UI, to find the defaults that make a page look generated. Adapted from funboy322/avoid-ai-design (MIT); the catalogue in this folder is upstream's references.
---

# AI design tells: the fixed rubric

A tell is a default nobody chose. It never proves who made a page. Judge the rendered page and its source against every ID below, in order, and against no other list.

## How to score

1. The slop gate already refuses the code-certain subset in new code: `ui.purple-gradient` (C1), `ui.gradient-text` (C6), `ui.shadcn-default` (K1), `ui.glow-orb` (C11), `ui.effect-library` (K10), `ui.placeholder-media` (IM4), `ui.five-star-row` (IM5), `ui.generator-signature` (F1). Check them anyway: a scaffolder run through the shell is not gated.
2. For each ID, read its entry in `ai-tells-catalog.md` (in this folder) before you decide.
3. Mark each finding code-certain (seen in the source), rendered (seen on screen) or inferred.
4. A P0 or P1 tell the brief did not ask for fails the review. A tell the brief asked for passes, and is named as such.
5. T1 (Inter as the primary face) counts only with two or more other tells on the same page: a calibration on 202 human sites found Inter alone is not a tell.

## P0: a layperson sees it

C1 purple or indigo gradient · C6 gradient-filled headline text · L1 pill badge over a centred hero · L2 three identical icon-topped feature cards · K1 untouched shadcn/ui base theme · T1 Inter as the primary face (see rule 5)

## P1: a designer sees it

SD1 cream ground with a terracotta accent · SD2 near-black ground with one acid-green or vermilion accent · SD4 tracked all-caps labels on three or more elements · SD5 one accented word in the headline · SD6 decorative 01/02/03 markers · SD9 a palette the vendors name as a default · T2 a face from the tasteful-free-font cluster · T4 Geist left untouched · T7 a typeface declared but never loaded · C2 indigo or violet primary buttons · C9 muted text below WCAG AA · C10 a one-note palette · C11 decorative blurred orbs · L4 round-number stat strip · L6 one centred width for every section · L7 three pricing tiers with a highlighted middle tier · L8 the default four-column footer · L9 the stock section order · L11 a big number as the hero · K2 one radius and soft shadow on every surface · K3 glassmorphism by reflex · K4 coloured side-stripe on rounded cards · K6 icon in a tinted rounded square · K8 default radius token · K9 the default dark SaaS card · K10 stock effect-library components · I2 emoji as interface icons · I3 the worn icon set and the sparkle glyph · CP1 vague aspirational headline · CP3 arrow glyph welded to a CTA · IM4 placeholder avatars or media · IM5 a five-star row on every testimonial · F1 generator signature left in · K7 no visible keyboard focus · M6 motion without prefers-reduced-motion

## Report

One row per finding: ID, where (selector, file and line, or screenshot), code-certain / rendered / inferred, and the fix. Then list every `slop-ok` comment and `slop-ignore` region in the change, with its reason. End with pass or fail under rule 4.
~~~~

**Agent prompt edits (exact):**
- `docs-writer.md`:
  - Startup: after the ux-writing line, add `` - `${CLAUDE_PLUGIN_ROOT}/knowledge/humanizer/SKILL.md` ``.
  - Do: after `- Write files in normal prose, not the caveman style.`, add `- Before you commit prose, run the humanizer pass on every paragraph you wrote or changed (its Embedded mode: keep every fact, return only the final text). The slop gate refuses an edit whose new text holds a known AI tell: rewrite the passage, and add a slop-ok waiver only when the brief asks for that exact wording.`
- `marketer.md` and `product-planner.md`: the same Startup line, last in Startup. The same Do line after their `Edit Markdown docs only, in the main clone.` line, with `Before you commit prose` changed to `Before you hand back a draft`.
- `ui-reviewer.md`:
  - Startup: last, add `` - `${CLAUDE_PLUGIN_ROOT}/knowledge/avoid-ai-design/SKILL.md` ``.
  - Do: after the `Check hierarchy, spacing...` line, add `- Score the change against the avoid-ai-design rubric as a fixed checklist: every P0/P1 tell with its ID and place; one the brief did not ask for fails the review. List every slop-ok waiver and slop-ignore region in the change.`
- `ui-designer.md`:
  - Startup: before the website-clone line, add `` - `${CLAUDE_PLUGIN_ROOT}/knowledge/avoid-ai-design/SKILL.md` ``.
  - Do: after the `Which knowledge leads` line, add `- Before the first screen, check your plan against the avoid-ai-design P0/P1 list; the slop gate refuses an Edit or Write that adds a code-certain tell.`

**`repo.rs`:**
- `expected_knowledge` gains `"humanizer"` and `"avoid-ai-design"`, and its doc comment becomes `the stripped 22, the unchanged 3 and seven more`.
- Each roster entry gets the knowledge ref in the Startup order above.
- `the_roster_data_matches_tests_repo_py`: expected knowledge 32; ui-designer knowledge 11.

- [ ] **Step 1: Update the counts and roster tests first.**
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p repo-tests --test knowledge --test licenses --test roster`. Expected: FAIL (the folders are missing).
- [ ] **Step 3: Vendor.**
  1. Add both entries to `vendor/sources.json`.
  2. Run `scripts/dev.sh vendor fetch humanizer`, then `scripts/dev.sh vendor build humanizer`, and the same for `avoid-ai-design`.
  3. Make the patch edits in the shipped trees. Write the rubric with Write; its tells are in code spans or plain names, so the gate is not involved.
  4. Turn the edits into patches: `scripts/dev.sh vendor diff humanizer > vendor/patches/humanizer.patch`, then the same for `avoid-ai-design`.
  5. Run `scripts/dev.sh vendor build humanizer` and `scripts/dev.sh vendor build avoid-ai-design`, then `scripts/dev.sh vendor check`. Expected: clean.
  6. Make the prompt edits.
  7. Run `scripts/dev.sh startup-budget`. For each agent marked `OVER`, set its entry in both `BUDGET` tables to the printed value. Comment `// +humanizer rewrite pass (owner, 2026-11-28)` or `// +avoid-ai-design rubric (owner, 2026-11-28)`.
  8. Re-bless: `scripts/dev.sh golden bless --surface dev.startup-budget`.
- [ ] **Step 4: Run:** `scripts/rust-test.sh test -p repo-tests` and `scripts/rust-test.sh test -p anthology-dev --test vendor --test smoke`, then `scripts/dev.sh startup-budget`. Expected: PASS, and every agent `ok`. Every agent body stays at or under 60 lines.
- [ ] **Step 5: Commit** `feat(anthology): humanizer rewrite pass and the avoid-ai-design rubric`. The fragment holds `- Add: docs-writer, marketer and product-planner run a humanizer rewrite pass; ui-designer and ui-reviewer use the avoid-ai-design P0/P1 rubric.`

---

### Task 10: The slop part of the after-work check

**Files:**
- Modify: `plugins/anthology/hooks/mods/afterwork.ts`, `plugins/anthology/tests/mods/afterwork.test.ts`, `docs/mods.md`
- Create: `changelog.d/anthology/62-afterwork-slop.md`

**Interfaces:**
- Consumes: `runAnthology` and `onAbsolutePath` (`util.ts`), and the `check` output of the CLI contract.
- Produces: `async function slopGaps($: EngineInterface, agentId: string): Promise<string[]>`, which returns 0 to 2 gap strings.

**`slopGaps` steps.** It returns `[]` on any failure or switch.
1. Return `[]` when `ANTHOLOGY_SLOP_GATE` is `0`.
2. Find the agent folder as `dirtyFiles` does, and return `[]` when it is the session's own folder.
3. Get the lead's HEAD: `git rev-parse HEAD` in `$.session.cwd()`, with the first git from `onAbsolutePath`, timeout `REV_TIMEOUT_MS = 1500`.
4. Run `runAnthology($, ['slop', 'check', '--root', dir, '--since', sha], '', SLOP_TIMEOUT_MS)` with `SLOP_TIMEOUT_MS = 4000`. A result of `undefined`, or an exit code other than 0 or 1, gives `[]`.
5. Finding lines match `/^.+:\d+: [a-z]+\.[a-z0-9-]+: "/`. Waiver lines match `/^.+:\d+: slop-(?:ok [a-z]+\.[a-z0-9-]+:|ignore region)/`.

In `afterworkReport`, run `dirtyFiles` and `slopGaps` together with `Promise.all`, and push the slop gaps after the dirty-files gap.

Gap copy (exact; `first3` = the first three lines joined with `; `):

~~~text
the agent's changes hold {n} AI tell(s) (anthology slop check): {first3}; send it back to rewrite them
the agent added {m} slop waiver(s) or ignore region(s): {first3}; confirm the user asked for that wording
~~~

Use `tell` for 1 and `tells` otherwise, and `waiver ... region` for 1 and `waivers ... regions` otherwise.

- [ ] **Step 1: Write the failing tests** in `afterwork.test.ts`, using the fake `$` patterns of `afterwork.test.ts` and `permissions.test.ts`:
  - `a writer whose changes hold tells gets a slop gap`
  - `added waivers raise their own gap`
  - `a clean check adds nothing`
  - `a missing binary, a timeout or exit 2 adds nothing`
  - `ANTHOLOGY_SLOP_GATE=0 never runs the check`
  - `the session's own folder is not checked`
- [ ] **Step 2: Run:** `ci/jobs/mods.sh`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Update `docs/mods.md` and run `ci/jobs/mods.sh`.** Add the two gaps to the after-work list, and add a switch-table row: `| ANTHOLOGY_SLOP_GATE=0 | Turns off the slop gate (see [The slop gate](slop-gate.md)) and its part of the after-work check |`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(mods): the after-work check runs anthology slop check on a writer's worktree`. The fragment holds `- Add: the after-work check lists AI tells and slop waivers in a writer's changes, including files written through the shell.`

---

### Task 11: The docs page and README

**Files:**
- Create: `docs/slop-gate.md`
- Modify: `README.md`, `crates/repo-tests/tests/docs.rs` (`DOC_PAGES` 7 to 8 with `"docs/slop-gate.md"`, plus the test below)

**Interfaces:**
- Consumes: `anthology_slop::RULES` (ids, summaries) and the CLI contract.

**Page outline,** in this order:
- `# The slop gate`
- `## What it checks`: the file kinds and the three tools.
- `## New text only`: Edit replay, line diff, density over new text.
- `## The rules`: a table of ID | files | block or density | what it flags. Every ID and every specimen goes in backticks.
- `## Quotes, specimens and waivers`: the syntax from "Waivers and specimens", in code blocks.
- `## Files it skips`
- `## The after-work check`
- `## Running it yourself`: `anthology slop check`, `--since`, `--root`, exit codes, and a CI example.
- `## Switching it off`: `ANTHOLOGY_SLOP_GATE=0`.
- `## Limits`:
  - text written through the shell reaches only the after-work check;
  - English only;
  - quoted spans are not checked;
  - the hook fails open when the binary is missing or old.

**README:**
- In "What you get", under the `anthology` bullets, add `- A slop gate: an edit whose new text holds a known AI-writing or AI-design tell is refused, with the line and the phrase. See [The slop gate](docs/slop-gate.md).`
- In "Documentation", add `- [The slop gate](docs/slop-gate.md): the rules, what counts as new text, waivers and the switch.`

- [ ] **Step 1: Write the failing test** in `docs.rs`: `slop_gate_page_names_every_rule`. For each `anthology_slop::RULES` id, assert that `` `{id}` `` appears in `docs/slop-gate.md`.
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p repo-tests --test docs`. Expected: FAIL.
- [ ] **Step 3: Write the page and the README lines.**
- [ ] **Step 4: Run:** `scripts/rust-test.sh test -p repo-tests --test docs --test slop_corpus`. Expected: PASS. The page passes the corpus, and the README is at or under 120 lines.
- [ ] **Step 5: Commit** `docs: the slop gate page`

---

### Task 12: End-to-end fixtures and the ship gate

**Files:**
- Create: `crates/anthology-slop/tests/e2e.rs`, `crates/anthology-slop/tests/fixtures/should-fail.txt`

**Interfaces:**
- Consumes: `check_all`, `kind_of` and `Kind` (Tasks 1 to 6 merged).

**Fixture format:** one case per line, `rule<TAB>kind<TAB>text`, where `\n` in `text` stands for a newline. Exact content:

~~~text
text.ai-vocab	markdown	We delve into the cache layer.
text.ai-vocab	markdown	The sync is seamless across devices.
text.ai-vocab	html	<p>Elevate your morning routine.</p>
text.stock-phrase	markdown	In today’s fast-paced world, teams ship daily.
text.stock-phrase	markdown	Let's dive in and read the code.
text.chat-residue	markdown	I hope this helps!
text.chat-residue	markdown	Let me know if you have any questions about the setup.
text.not-x-but-y	markdown	It's not a tool, it's a platform.
text.not-x-but-y	markdown	This isn’t just a cache; it’s a memory layer.
text.not-x-but-y	markdown	It's not about speed.\nIt's about trust.
text.hype-cta	jsx	export const Hero = () => <h1>Take your team to the next level</h1>
text.hype-cta	html	<a href="/join">Join thousands of happy teams</a>
text.em-dash-density	markdown	The hook runs first — before the guard — and then the mod reads the result — once.
text.triad-density	markdown	Plans, specs and roadmaps go to the planner. Pages, themes and widgets go to the designer. Tests, builds and releases go to the runner.
ui.purple-gradient	css	.hero { background: linear-gradient(135deg, #6366f1, #a855f7); }
ui.purple-gradient	jsx	export const A = () => <div className="bg-gradient-to-r from-indigo-500 to-purple-600" />
ui.gradient-text	css	.title { background-clip: text; -webkit-text-fill-color: transparent; }
ui.gradient-text	jsx	export const T = () => <h1 className="bg-clip-text text-transparent">Hi</h1>
ui.shadcn-default	css	:root { --primary: oklch(0.205 0 0); }
ui.glow-orb	css	.orb { border-radius: 50%; filter: blur(96px); }
ui.effect-library	jsx	export const B = () => <ShimmerButton>Go</ShimmerButton>
ui.placeholder-media	html	<img src="https://i.pravatar.cc/150?img=3" alt="Ana">
ui.placeholder-text	html	<p>Lorem ipsum dolor sit amet.</p>
ui.five-star-row	html	<span>★★★★★</span>
ui.generator-signature	html	<meta name="generator" content="v0">
~~~

- [ ] **Step 1: Write the tests:**
  - `every_should_fail_case_raises_its_rule`: `check_all(kind, text)` holds a finding with that rule. On failure, list every missing case.
  - `every_rule_has_a_should_fail_case`: every `RULES` id appears in the fixture.
  - `waived_cases_do_not_block`: a few cases with a valid `slop-ok` comment appended give findings with `waiver` Some.
- [ ] **Step 2: Run:** `scripts/rust-test.sh test -p anthology-slop --test e2e`. Expected: PASS once Tasks 2 to 6 are merged. A failure names a rule or mask to fix in its owning task's file; open the fix in that task's area.
- [ ] **Step 3: Run the full gate:**
  - `scripts/rust-test.sh test -p anthology-slop`
  - `scripts/rust-test.sh test -p anthology-cli`
  - `scripts/rust-test.sh test -p repo-tests`
  - `CI=true bash ci/jobs/difftest-linux.sh`

  Expected: PASS, and difftest shows 0 unexplained.
- [ ] **Step 4: Commit** `test(slop): end-to-end fixtures for every rule`

---

### Task 13: Release

Outward-facing: the tag and the pushes wait for the owner's go.

- [ ] **Step 1:** On green `build`, tag and push `anthology-bin--v0.2.10`. Run `timeout 170 gh run watch <id>` until `anthology-release.yml` succeeds.
- [ ] **Step 2:** Run `scripts/dev.sh lock write`. It rewrites both `anthology.lock` files, which are byte-identical. Commit `chore(release): pin anthology-bin 0.2.10` and merge it through a PR with green CI.
- [ ] **Step 3:** Run `scripts/release.sh anthology-guardrails 0.2.19 --push`, then `scripts/release.sh anthology 0.6.0 --push`. Expected: both end with `RELEASE OK <tag>`.
- [ ] **Step 4: Smoke, in a fresh session with the released plugins.**
  - A Write of `docs/x.md` holding `` `We delve into caching.` `` (without the backticks) is refused with the gate message.
  - A clean Write succeeds.
  - With `ANTHOLOGY_SLOP_GATE=0`, the refused Write succeeds.

---

## Project map lines (for the PR body of Task 8; the lead applies them)

- Pieces: `crates/anthology-slop` is the slop gate library: masking, rules, Edit replay and messages, with no env reads. The binary runs it as `anthology slop hook|check|rules` (`src/cmd/slop.rs`). The build graph gains `anthology-slop` <- `anthology-cli`, `repo-tests`.
- Flows: a Write, Edit or MultiEdit runs `hooks/slop-gate.sh`, then `anthology slop hook`. Exit 10 blocks (script exit 2). Every other outcome allows, so the hook fails open. Its pins are in `shims/statics.rs`, `atl_install.rs` and `shims/slop_gate.rs`. The after-work mod runs `anthology slop check --since <lead HEAD>` on a writer's worktree.
- Checks: `repo-tests/tests/slop_corpus.rs` keeps every rule silent on `README.md`, the top-level `*.md` files and `docs/**`.
