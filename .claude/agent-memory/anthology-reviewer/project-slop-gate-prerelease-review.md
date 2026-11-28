---
name: 'Slop gate pre-release review (build 494ccb71)'
description: 'Probed false blocks and fail-open in crates/anthology-slop on build; Edit context re-check, unmasked list fences, proper nouns, quadratic line_of'
metadata:
  type: project
  modified: 2026-11-28
---

Reviewed origin/build at 494ccb71 before the slop gate release; Task 7 (crates/anthology-cli/src/cmd/slop.rs) was still a stub with no PR.
Findings, each reproduced with a probe crate in the rust image:
- edit::apply marks the whole new_string of an Edit as new, so an Edit whose context repeats an old human tell blocks; the same change as a MultiEdit is allowed (line diff).
- The Markdown mask only knows fences indented 0 to 3 spaces: list-nested or MkDocs 4-space fences, indented code, raw pre blocks, TOML +++ frontmatter, multi-line MDX export or JSX tags and quotes that wrap a line are read as prose.
- Proper nouns block: Delve (Go debugger), Apache Tapestry, Unleash. Technical idioms block: "to the next level of the tree", "is not only X. It is also", "This is not the default, it is".
- Density rules count only the new text, so a 2-line hours list with spaced en dashes, or 3 platform lists in one sentence, blocks.
- JSX: text after a closing brace up to a generic (export function elevate<T>) and HTML inside a string literal count as text, against D8.
- ui.purple-gradient fires on any violet stop: viridis legends, hue sliders, Discord blurple. Stories and test files are not exempt (pravatar, lorem ipsum).
- line_of is O(n) per finding and per waiver: about 170k hits or 18k waivers in 1 MiB takes over 15 s; an unclosed linear-gradient( repeated also scans to the end each time.
- The block message gives only HTML-comment waiver syntax, which does not waive in CSS or JSX.
Why: these are the checks to repeat when Task 7 lands or the rules are narrowed. The probe method: a /tmp crate depending on path /w/crates/anthology-slop, fed into scripts/rust-test.sh run as a tar on stdin, child processes with a 15 s kill for stress inputs.
