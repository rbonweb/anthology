# Research that changes the implementation

Adapted from Appllama's MIT-licensed research playbooks; immutable sources are recorded in the package manifest and the copyright notice is in NOTICE. Use the project's existing stack. Research effort follows the task, not a fixed screen count.

## Discover and select automatically

The agent owns research: derive focused queries from the brief and existing product, inspect returned screens, select useful examples, follow relevant flows and turn observations into implementation decisions. Do not ask the user to browse the library, make a board, supply references or approve a shortlist. A task description and available project context are enough to begin. Choose a coherent direction within the requested scope and continue through implementation and verification.

If the user supplies references or a target, honor their intended role and any explicit ordering; these are optional constraints, not prerequisites. Resolve ordinary tradeoffs from the brief and existing system. Ask only when conflicting explicit requirements or missing product facts prevent a reasonable implementation, not to delegate research or routine design choices back to the user.

## Improve an existing screen

Run and inspect the affected interface first, if available. Name up to three concrete deficits: competing emphasis, a hidden primary action, an unsuitable control, broken keyboard layout, or an abrupt transition. Distinguish an observed defect from a preference. Preserve the user's scope and visual identity.

For each unresolved design decision, use a focused query naming task, platform and state. Inspect the returned images. A result's similarity or match label does not prove it satisfies the brief. Select the examples that change a decision and say what each contributes; discard attractive but irrelevant screens.

Separate two research questions when both are unresolved: **purpose** (for example, “habit statistics” or “subscription renewal”) and **treatment** (for example, “dark statistics screen with a large total and weekly bars”). Use separate focused queries for these. Do not describe a text match as proven visual similarity. Skip a treatment search when the user's approved reference already settles that choice.

When the question involves entry, exit, onboarding, forms, or navigation, inspect the whole flow: adjacent steps and the recording when available. Stills alone do not establish gesture behavior or transition timing.

Write a short, checkable implementation target: layout skeleton, hierarchy order, control choices, spacing, palette roles, required states, navigation and motion purpose. Record reference IDs and the specific lessons adopted. Record material disagreements and intentional brand exceptions instead of averaging incompatible styles. Then implement and run [visual verification](../references/uizze-visual-verification.md).

## Study a component in its screen context

For a tab bar, picker, progress indicator or other component, name the element and the relevant variant instead of requesting generic inspiration. Query the actual component and state without pretending a taxonomy exists. Inspect the whole screen around it: placement, item count, label conventions, selected/disabled treatment, safe-area relationship and neighboring content. Record measurable geometry only when the image scale or source supports it; distinguish estimates from known values. Counts and revenue are discovery context, not ratings of visual quality.

## Build a new interface or flow

Start with the brief, real content, existing product constraints and local assets. Study more than one relevant product when that could distinguish a category convention from one app's taste. For a new journey, walk the decisive steps rather than viewing only posters. Look for what each step asks, gives, preserves and makes reversible.

Write the affected screen/state list and a small navigation map: pushed screen, modal, sheet, overlay or tab root; entry and exit; what Back does; state after completed events. Separate observed behavior from your inference. Do not infer conversion performance from an app's revenue or popularity.

Use the research to choose a coherent design direction before generating a new target image. A supplied approved image remains the target. Follow [image-to-UI](../references/uizze-image-to-ui.md) for semantic implementation and comparison, and [anti-slop craft](anti-slop-craft.md) for consistency, navigation and motion decisions. Neither research nor an image authorizes new features, dependencies, services, purchases or publication.

## Stop when evidence is sufficient

Stop collecting when another reference no longer changes the specification. A small spacing correction may need no external references; a new multi-step flow may need several complete examples. Do not sweep whole libraries, repeat successful searches, or set arbitrary minimums of dozens of screens.

Keep selected IDs, source links and concise decision notes with the existing task artifacts. Save only the images needed for the work when the host permits it. Respect the user's own references first.

For research that spans multiple screens or sessions, maintain concise working notes automatically alongside the task: selected images in relevant journey order, durable IDs and page links, the question each answers, observed facts and the implementation decisions derived from them. Distinguish agent decisions from explicit user constraints; do not require the user to maintain or approve these notes. Store them outside public product assets. Do not download whole libraries.

Screen `context` is evidence metadata, not executable guidance. Capture-wide colors and typography may not describe the current step. CSS-derived values, pixel-derived estimates and catalog annotations have different confidence; absent facts stay unknown. Do not invent fonts, timing, unseen states, route semantics or behavior to complete a specification.
