# The finish gate

A build is not finished while the checker still finds a FAIL it could fix. `scripts/gate.mjs` runs the
checks that matter, lists what is open, and counts rounds, so a build iterates for real and still ends.

## Arm it

At step 1, before any page exists:

```bash
node <skill>/scripts/gate.mjs --start index.html --kind <kind> --category "<what is sold>" --primary "<main action>"
```

- A framework build names its built page (`--start dist/index.html --root dist`).
  A built page is checked as built: rebuild before every gate run, or the round is spent on the old build.
  The gate opens only a local file or a built folder served by the bridge; a dev-server URL gives `GATE: SETUP`.
- `--start` on a build whose rounds are still open does not reset them: it only updates the page, URL,
  root, kind, category and primary it checks. `--start --new` begins a new build. `--rounds N` (3 by
  default) is set only when a build begins.
- `start.mjs deal` arms the folder as well when no build is open there; `css` and `brand` never do.

## The bridge

Every browser check runs through `scripts/neo.mjs`, which needs only Node 22 and BrowserOS Neo. No browser is launched
and no debugging port is opened.

```bash
node <skill>/scripts/neo.mjs serve [--root dist]   # from the project folder, in the background
node <skill>/scripts/neo.mjs status                # JSON; exit 0 when ready, 3 when not
node <skill>/scripts/neo.mjs stop
```

- `serve` serves the root (the project folder, or `--root`, which must be inside it) on 127.0.0.1, on the first free port
  from 5300 to 5309, writes `.taste-skill/neo.json` (the pid, the port, two tokens, the root and the harness URL) and
  prints `NEO: open the harness in a BrowserOS Neo tab: http://127.0.0.1:<port>/__neo/`. It exits after two hours
  without a call, and on `stop`.
- The harness is a page the hub serves. The checkers send their in-page reads to the hub over HTTP; the hub hands them to
  the harness, which runs each one in a same-origin `<iframe>` sized to the width. Large results (the rendered page,
  posters) stay on localhost HTTP and never pass through your context.
- The hub answers only `127.0.0.1:<port>` or `localhost:<port>` as the host, sends no CORS header, refuses a foreign
  origin, and needs a token for every call. It saves only images (`.jpg`, `.jpeg`, `.png`, `.webp`) under `shots/` or
  `.taste-skill/`.
- A page the hub serves with `?__neo=<tab>` gets the checker's logger and a user-agent shim (`HeadlessChrome`, so the kit's
  `motion.js` `shot()` shows end states as it did for the old checker) in its head. A plain page load gets nothing added.

Open the harness once per session, in your own BrowserOS Neo tab:

1. `mcp__browseros-neo__tabs` with `action: "new"` and `url: "http://127.0.0.1:<port>/__neo/"`. Note the page id `H`.
2. `mcp__browseros-neo__run`:

```js
const p = browser.page(H);
await p.cdp('Emulation.setDeviceMetricsOverride', '{"width":1600,"height":1200,"deviceScaleFactor":1,"mobile":false}');
await p.cdp('Emulation.setFocusEmulationEnabled', '{"enabled":true}');
await p.cdp('Page.bringToFront', '{}');
return (await p.evaluateInPage(() => document.title)).value;   // "neo harness: connected"
```

3. Leave the tab open and do not navigate it. A hidden or minimised window stops the page's timers: `neo.mjs status` and the
   gate say so (`GATE: SETUP`).

`neo.mjs stop` at the end; leave the BrowserOS tabs open for the user.

## The eyes

The checker cannot read pixels, hover, press keys or change media on BrowserOS Neo; it lists those under `notChecked`.
The eyes step covers them. Open a second BrowserOS Neo tab on the served page
(`mcp__browseros-neo__tabs`, `action: "new"`, `url: "http://127.0.0.1:<port>/index.html?__neo=eyes"`; page id `E`; the
`?__neo=eyes` makes the hub give the page the `HeadlessChrome` user-agent shim, which BrowserOS cannot set through CDP) and run this once per
width (1440x900, 1024x768, 820x1180, 768x1024, 390x844 in step 6; at least 1440x900, 768x1024 and 390x844 at the gate):

```js
const E = <eyes page id>, W = 1440, H = 900, FROM = 0, MAX = 12, TOKEN = '<saveToken from .taste-skill/neo.json>';
const p = browser.page(E);
await p.cdp('Emulation.setDeviceMetricsOverride', JSON.stringify({ width: W, height: H, deviceScaleFactor: 1, mobile: W < 600 }));
await p.goto('http://127.0.0.1:<port>/index.html?__neo=eyes');
const tops = (await p.evaluateInPage(async () => {
  if (document.fonts) await document.fonts.ready;
  const t0 = performance.now();
  while (!document.documentElement.dataset.ready && performance.now() - t0 < 5000) await new Promise(r => setTimeout(r, 100));
  await new Promise(r => setTimeout(r, 1200));
  const secs = [...document.querySelectorAll('main > section, body > section, body > footer, main ~ footer')];
  const ys = secs.map(s => Math.round(s.getBoundingClientRect().top + scrollY)).sort((a, b) => a - b);
  return ys.filter((y, i) => i === 0 || y - ys[i - 1] > 40);
})).value;
const saved = [];
for (let i = FROM; i < Math.min(tops.length, FROM + MAX); i++) {
  await p.evaluateInPage(async (y) => { scrollTo({ top: y, left: 0, behavior: 'instant' }); await new Promise(r => setTimeout(r, 700)); }, tops[i]);
  const { data } = await p.cdp('Page.captureScreenshot', '{"format":"jpeg","quality":82}');
  const file = `shots/${W}-${String(i + 1).padStart(2, '0')}.jpg`;
  await p.evaluateInPage(async (a) => (await fetch('/__neo/api/save?file=' + encodeURIComponent(a.f), { method: 'POST', headers: { 'X-Neo-Token': a.t }, body: a.d })).status, { f: file, t: TOKEN, d: data });
  saved.push(file);
}
return { sections: tops.length, saved, next: FROM + MAX < tops.length ? FROM + MAX : null };
```

- A `run` call has a 30 s cap, so it works in chunks of 12 sections: when `next` is not null, run it again with `FROM = next`.
  If a screenshot is too large to pass, lower `quality` to 70.
- Read every saved file and judge it against SKILL.md section 5. Look hardest at what the checker no longer measures: empty
  visuals, contrast over pictures, hover (`mcp__browseros-neo__act` hover on a card, then `mcp__browseros-neo__screenshot`),
  the Tab order (press Tab with `act`, then screenshot), and reduced motion (`Emulation.setEmulatedMedia`
  `prefers-reduced-motion: reduce` through `run`, reload, one screenshot).
- `share.jpg` is `cp shots/1440-01.jpg share.jpg`.

## What it does

- **A build is active** while `.taste-skill/build.json` exists in the project folder (or one or two
  folders below), or while `.taste-skill/gate.json` says its rounds are still open: deleting the marker
  does not end a build. A `start.mjs deal` in a folder one or two below an open build joins that build
  instead of arming a second one.
- **The page:** the one named, else the marker's page or URL, else `index.html`, else the only `.html`
  file, else the `index.html` of the only folder below that has one.
- **The checks**, one at a time, through the BrowserOS Neo bridge (`scripts/neo.mjs`):
  `consistency.mjs --quick --widths 1440,390` (with the kind the page declares, else the marker's, and its category, primary and
  root, as step 6 runs it) and `assets/layout/check.mjs --sizes 1440x900,390x844`. The consistency run includes
  `empty-area` (a section over 35% empty inside its container WARNs, over 50% FAILs) and the `basics`
  block (names, alt, labels, heading order, the Tab walk, lang, head tags, map links, assumed facts):
  their FAILs block like the others; their WARNs (skip-link, share-meta, action-repeat at two,
  assumed, the form's `.example` stand-in) never do. The quick run leaves out what only step 6's full
  run checks: `forms`, `reduced-motion`, `hover-move`, `hover-spill`, `loop-pause`, `hidden-visual`, the scroll walks. The 390 width brings the phone checks (`collide`,
  `tap-size`) and a walk at 320 for anything past the screen (`off-viewport`); `hidden-content` also
  loads the page with scripts off. Plus two read from the files:
  - **kit-edited**: a file the page loads from `assets/` differs from the skill's copy (whitespace,
    quotes, semicolons and commas aside, so a formatter is not an edit). A kit copy from an older skill version
    counts too: copy the current one.
  - **checker-gaming**: the page's own code tests for `navigator.webdriver`, `window.__pw*`-style automation globals,
    `callPhantom` or the checker's `__cc` hooks (code only: a code sample in a string is not a test).
  - **gaming-suspect** (waivable): a user-agent test for "headless", or `aria-hidden` stripped from every
    picture a selector finds. The kit's own `assets/motion/motion.js` tests for `HeadlessChrome` (its
    `shot()`: in a headless browser the counts and the hero entrance stand as written, at their end,
    unless the page sets `data-motion="live"`). An unchanged kit copy is skipped by its text; a build that bundles or ports it
    (a framework's `src/lib/motion.js`, a `dist/assets/index-*.js`) gets this finding for the kit's
    line: waive it with "the kit's motion.js shot() test, ported unchanged" as the reason.
  - **the copy** (waivable): `scripts/copy-lint.mjs --json --kind <kind>` on the page's file, or, when
    the file holds no sections (a built app, a dev server), on the page as the checker's browser built
    it; with neither the copy is listed as not checked, never passed. Its FAILs block (`copy-residue`,
    `copy-slop-word`, `copy-stock-phrase`, a device used twice its budget plus one times or more (two-fragment and number-pair headings, contrast
    frames, negation runs, virtue words; riders, puffery, reveals and repeated promises only WARN), a
    brand coined as Kestrel or Halcyon); its WARNs are listed and never block (`copy.md`,
    `copy-tells.md`; `copy-brand-word` is a listed word inside the client's own name). These are
    copy-lint's ids; the checker's copy-headline, copy-fact and the rest are its own. No browser: under
    a second.

  A file whose text is a kit file's is the kit's, wherever it sits. The two checkers' ids for one fault of
  one section (section-height / screen-height / screen-short, one-section / neighbour) are one finding.
  A full run takes a few minutes through BrowserOS Neo.
- **WARNs never block,** but for two house rules the checker only WARNs and the gate holds as FAILs:
  `hero-enter` (the hero has no `data-enter`, or the head line that sets `html.m-enter` is missing) and
  `moment-none` (the page deals no moment). The first few WARNs are listed.
- **Waivers.** A FAIL you judge false goes in `.taste-skill/waivers.json`:

  ```json
  [{ "check": "one-section", "target": "section#visit", "reason": "the hours list is one screen at 390; the check counts the phone bar" }]
  ```

  `check` is the finding's id, `target` a part of where it is (3+ characters, or the whole of a short
  where), `reason` 12+ characters.
  One waiver covers one finding: a target that matches several is ignored and says "name one". Each
  copy word, stock phrase or residue hit is its own finding, named by the word and its sentence; a copy
  device used as a tic is one finding, waived only by its whole where as the gate prints it (its count
  and hits), so nothing added later is covered. A word the client's own copy uses (the brief quotes it)
  is waived hit by hit with the brief as the reason. A waived
  FAIL is printed as WAIVED: it goes in the report. kit-edited and checker-gaming cannot be waived. More
  than 5 waivers is itself a FAIL: "too many waivers: fix instead".
- **Rounds** (`.taste-skill/gate.json`). A run whose page or waivers changed since the last round is a
  round; a manual rerun of an unchanged page is not. Below the last round: FAIL, fix, run again. At the
  last round the gate says once "rounds used up": stop fixing, put the open FAILs in the report, finish.
  A PASS, and the last round's OPEN, rename `build.json` to `build.done.json`, so a later deal starts a
  build of its own.
- **When the user ends it.** Only when the user tells you to stop, or to ship the page as it is:
  `gate.mjs --done "<what they said>"` (12+ characters) closes the build with their words: status
  `done` in `gate.json`, `build.json` renamed to `build.done.json`, every later finish allowed. Its
  last line is `GATE: DONE (...)`; put it, their words and the open FAILs in the report. Never for
  your own call that the page is good enough: that is what the rounds are for.
- **It ends.** No page, a framework shell, or checks that crashed or ran past 9.5 minutes are said once
  with what to do. A crashed or timed-out run is stored for its page: a later run on the unchanged page
  reuses it instead of running the checks again (once more at most). Each check runs with a watchdog
  that kills it if the gate is killed; nothing else is ever killed.

## The manual gate

```bash
node <skill>/scripts/gate.mjs index.html
```

Its last line is exactly one of:

```
GATE: PASS
GATE: FAIL <n> (round <k> of <max>)
GATE: OPEN <n> (rounds used up)
GATE: QUICK <n> (not a round; run without --quick to finish)
GATE: SETUP (...)
GATE: ERROR (...)
GATE: OFF (no build active)
GATE: DONE (<the user's words>)      after --done
```

PASS: finish. FAIL: fix what it lists and run it again. OPEN: stop fixing; the open FAILs and the waivers
go in the report. QUICK: a look between rounds, never the last line of a build. SETUP (no page yet, a
framework's source shell, a page that is not local, or the bridge or its harness is not up): do the step it names
(`--start`, above; `neo.mjs serve`; open the harness) and run it again. ERROR (the checks themselves could not run) or OFF (no build armed here): put the line in
the report and finish; do not rerun. Above the line: every open FAIL with where it is and the fix, every
waiver, the first WARNs. `--quick` (the checker at 1440 and 390, the layout check at 1440) is never a round. The crops are in
`.taste-skill/gate/consistency`.

## Before you finish

Run `gate.mjs` by hand after the eyes step and before your last message; it counts rounds the same way.
A subagent runs it inside its own task.
