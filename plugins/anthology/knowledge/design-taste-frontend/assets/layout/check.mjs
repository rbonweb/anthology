#!/usr/bin/env node
/*
  taste-skill layout check - runs assets/layout/layout-check.js on a page through the BrowserOS Neo bridge at each width
  (a harness frame of that width on the bridge's page, which node <skill>/scripts/neo.mjs serve serves), and prints every finding.

    node <skill>/assets/layout/check.mjs --url <file> [--sizes 1440x900,1024x768,820x1180,768x1024,390x844] [--root <dir>] [--json]

  --root is the folder the bridge serves (a built site's folder, so its root-relative links (/style.css) resolve); it must be
  the folder the bridge was started with. --shots is not available on BrowserOS Neo: take the eyes step's shots (references/gate.md).
  Then, once per size, it lengthens the hero's h1 to its type's longest headline (heroes.json
  h1.maxChars, with words from the lead) and checks again: the hero must still be one screen
  (hero-maxchars) and the h1 within its lines at 1440 and 390 (hero-lines). On a page in a language that
  builds long compounds (lang de, nl, da, sv, nb, nn, no, fi) the added words start with one
  (Wochenendöffnungszeiten), which must break inside the column. --no-stress skips it.
  Exit code 1 when there is a finding.
*/
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { connect, NeoTab } from '../../scripts/neo.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const opt = (k, d) => { const i = args.indexOf('--' + k); return i < 0 ? d : (args[i + 1] && !args[i + 1].startsWith('--') ? args[i + 1] : true); };
const target = opt('url'), shots = opt('shots'), rootArg = opt('root');
const sizes = String(opt('sizes', '1440x900,1024x768,820x1180,768x1024,390x844')).split(',').map(s => s.split('x').map(Number));
if (!target || target === true || rootArg === true) { console.error('usage: check.mjs --url <file> [--sizes 1440x900,1024x768,820x1180,768x1024,390x844] [--root dir] [--json]'); process.exit(2); }
const sleep = ms => new Promise(r => setTimeout(r, ms));
const src = fs.readFileSync(path.join(here, 'layout-check.js'), 'utf8');
// a page file is served by the BrowserOS Neo bridge (its root is the folder neo.mjs serve was started with), never opened from file://
const hub = await connect().catch(e => { console.error('check: ' + e.message); process.exit(2); });
let url = target;
if (/^https?:\/\//i.test(target)) {
  let same = false;
  try { same = new URL(target).origin === hub.origin; } catch { same = false; }
  if (!same) { console.error('check: BrowserOS Neo checks a local file served by neo.mjs (build the page, then --root <build folder>)'); process.exit(2); }
} else {
  let file = path.resolve(/^file:/i.test(target) ? fileURLToPath(target) : target);
  if (fs.existsSync(file) && fs.statSync(file).isDirectory()) file = path.join(file, 'index.html');
  if (!fs.existsSync(file)) { console.error(`check: no page at ${file}`); process.exit(2); }
  // the page's ../ references must resolve inside the bridge's root
  const ups = Math.max(0, ...[...fs.readFileSync(file, 'utf8').matchAll(/(?:(?:src|href)\s*=\s*|\bfrom\s*|\bimport\s*\(?\s*|url\(\s*)["']?((?:\.\.\/)+)/g)].map(m => m[1].length / 3));
  let root = path.dirname(file); for (let i = 0; i < ups; i++) root = path.dirname(root);
  if (rootArg) {
    root = path.resolve(rootArg); const rel = path.relative(root, file);
    if (rel.split(path.sep)[0] === '..' || path.isAbsolute(rel)) { console.error(`check: the page is not inside --root ${root}: ${file}`); process.exit(2); }
    if (fs.realpathSync(root) !== hub.root) { console.error(`check: the BrowserOS Neo bridge serves ${hub.root}; restart it with: neo.mjs serve --root ${root}`); process.exit(2); }
  }
  try { url = hub.urlFor(file); } catch (e) { console.error('check: ' + e.message); process.exit(2); }
}
const { heroes } = JSON.parse(fs.readFileSync(path.join(here, 'heroes.json'), 'utf8'));
// the stress: the page's own h1 markup (from its source, before layout.js split it) with words from the
// lead added to its last plain text until it holds maxChars characters; the letters moment is left off
const STRESS = String.raw`(async (maxChars) => {
  const h1 = document.querySelector('main > section h1'); if (!h1) return null;
  const doc = new DOMParser().parseFromString(await (await fetch(location.href)).text(), 'text/html');
  const src = doc.querySelector('main > section h1'); if (!src) return null;
  const words = (document.querySelector('main > section .hero__lead')?.textContent || 'what the company makes and sells every day').replace(/[^\p{L}\s]/gu, ' ').split(/\s+/).filter(w => w.length > 2).map(w => w.toLowerCase());
  const len = () => src.textContent.replace(/\s+/g, ' ').trim().length;
  const tw = doc.createTreeWalker(src, NodeFilter.SHOW_TEXT); let last = null;
  for (let n = tw.nextNode(); n; n = tw.nextNode()) if (n.textContent.trim() && !n.parentElement.closest('.ly-word, .ly-pic, .ly-mark')) last = n;
  if (!last || !words.length) return null;
  // one long compound first on a page whose language builds them (a German heading holds words of 20 letters and
  // more): it must break with a hyphen inside its column at 390 (system.css hyphens: auto, in the page's lang).
  // An English page never holds one, and hyphens: auto in English does not break it, so it is left out there.
  if (/^(de|nl|da|sv|nb|nn|no|fi)\b/i.test(document.documentElement.lang || '')) words.unshift('Wochenendöffnungszeiten');
  for (let i = 0; len() < maxChars && i < 80; i++) {
    const w = words[i % words.length], room = maxChars - len() - 1;
    if (room < 2) break;
    last.textContent = last.textContent.replace(/\s*$/, '') + ' ' + w.slice(0, room) + (/\s$/.test(last.textContent) ? ' ' : '');
  }
  h1.removeAttribute('data-ly-letters'); h1.classList.remove('ly-locked', 'ly-letters');
  h1.innerHTML = src.innerHTML;
  await new Promise(r => requestAnimationFrame(() => setTimeout(r, 300)));
  return { chars: len(), text: h1.textContent.replace(/\s+/g, ' ').trim() };
})`;

const [w0, h0] = sizes[0];
const guard = setTimeout(() => { console.error('check: timed out'); hub.close().finally(() => process.exit(2)); }, 240000);
if (shots) console.error('check: --shots is not available on BrowserOS Neo; take the eyes step\'s shots (references/gate.md)');

let failed = false;
const report = [];
try {
  const tab = await NeoTab.open(hub, { width: w0, height: h0 });
  const ev = x => tab.eval(x, 60000);
  const errors = [];
  for (const [W, H] of sizes) {
    await tab.send('Emulation.setDeviceMetricsOverride', { width: W, height: H });
    await tab.goto(url, 20000);
    await ev(`new Promise(r => { const t = performance.now(); (function w() { if (document.documentElement.dataset.ready || performance.now() - t > 5000) r(); else setTimeout(w, 50); })(); })`);
    await ev('document.fonts ? document.fonts.ready.then(() => 1) : 1');
    await sleep(900);
    const res = await ev(src + '\n;window.lyCheck()');
    res.size = `${W}x${H}`;
    res.overflowX = await ev('document.documentElement.scrollWidth > document.documentElement.clientWidth');
    if (res.overflowX) res.findings.push({ rule: 'overflow-x', where: 'page', detail: 'the page scrolls sideways' });
    report.push(res);
    if (!opt('no-stress')) {
      const id = await ev(`(document.querySelector('main > section')?.className.match(/hero--([a-z]+)/) || [])[1] || ''`);
      const h = heroes.find(x => x.id === id);
      if (h) {
        const st = await ev(`${STRESS}(${h.h1.maxChars})`);
        if (st) {
          const again = await ev(src + '\n;window.lyCheck()');
          const lines = again.hero?.h1?.lines ?? 0, allowed = W >= 1400 ? h.h1.lines[0] : W <= 420 ? h.h1.lines[1] : null;
          res.stress = { hero: id, maxChars: h.h1.maxChars, chars: st.chars, lines, height: again.hero?.height };
          for (const f of again.findings.filter(f => f.rule === 'hero-fold')) res.findings.push({ rule: 'hero-maxchars', where: f.where, detail: `with a ${st.chars}-character h1 (heroes.json maxChars): ${f.detail}` });
          if (allowed && lines > allowed) res.findings.push({ rule: 'hero-lines', where: 'section.hero h1', detail: `the check fills the h1 to ${st.chars} characters (this hero's maxChars in heroes.json), not the page's own: it takes ${lines} lines at ${W}, ${allowed} at most` });
          if (await ev('document.documentElement.scrollWidth > document.documentElement.clientWidth')) res.findings.push({ rule: 'hero-maxchars', where: 'page', detail: `with a ${st.chars}-character h1 the page scrolls sideways` });
        }
      }
    }
    errors.push(...(await tab.logs()).filter(l => /^(error|exception)/.test(l)).map(l => l.split('\n')[0]));
    if (errors.length) { res.errors = [...errors]; errors.length = 0; }
  }
  await tab.close();
} catch (e) {
  console.error('check:', e.message);
  failed = true;
} finally {
  clearTimeout(guard);
  await hub.close();
}

if (opt('json')) console.log(JSON.stringify(report, null, 1));
else for (const r of report) {
  console.log(`\n${r.size}  lines x ${r.L} and ${r.R}, nav ${r.nav}px, one screen ${r.screen}px`);
  if (r.hero) { const h = r.hero, b = v => v ? `${v.w}x${v.h} at ${v.x},${v.y}` : '-';
    console.log(`  hero ${h.height} tall; h1 ${h.h1 ? `${h.h1.size}px/${h.h1.leading}, ${h.h1.lines} lines, ${b(h.h1)}` : '-'}; lead ${h.lead ? `${h.lead.size}/${h.lead.lineHeight}, ${h.lead.lines} lines, ${h.lead.w} wide` : '-'}; buttons ${h.buttons ? `${h.buttons.n} x ${h.buttons.h}` : '-'}; visual ${b(h.visual)}`); }
  if (r.stress) console.log(`  stress: ${r.stress.hero} h1 at ${r.stress.chars} of ${r.stress.maxChars} characters takes ${r.stress.lines} lines, hero ${r.stress.height} tall`);
  for (const s of r.sections) console.log(`  section ${s.where.slice(0, 60).padEnd(60)} y ${String(s.top).padStart(5)}  h ${String(s.height).padStart(5)}  content ${s.content}`);
  if (!r.findings.length) console.log('  no findings');
  for (const f of r.findings) console.log(`  ${f.rule.padEnd(16)} ${f.where.slice(0, 70)}: ${f.detail}`);
  if (r.errors) for (const e of r.errors) console.log(`  page error: ${e}`);
}
const n = report.reduce((a, r) => a + r.findings.length, 0);
process.exit(failed ? 2 : n ? 1 : 0);
