#!/usr/bin/env node
/*
  product-render/make-posters.mjs - checks a product spec and writes its posters and stills beside it.

    node assets/objects/product-render/make-posters.mjs <spec.json> [more specs] 
         [--variants three-quarter,side,detail-saddle,three-quarter+loaded] [--grounds light,dark] [--px 1100] [--legacy]
    node assets/objects/product-render/make-posters.mjs --check <spec.json>   (no browser: the spec, its views, scenes and shaders)

  First the spec is built in Node, with every view and scene it will make: unknown part types, props
  or materials, a part a view or a scene names that is not there, too many parts, and words in the
  shader a laptop's GPU rejects (glslProblems) stop it with exit 1. Then make-posters.html is opened
  in the BrowserOS Neo harness (the bridge: node <skill>/scripts/neo.mjs serve from the project folder, its harness open in a
  BrowserOS Neo tab; it renders on the browser's GPU), and what it prints is written beside the spec.

  The views (spec.make.variants, or --variants; "view+scene" for a scene): for each variant and ground
  (spec.make.grounds, default light) one render, from which it writes
    <poster>-<variant>.fallback-render[-dark].png         the view's poster (the live render's fallback)
    stills/<poster>-<variant>-<colour>[-dark].webp        a still per colourway (spec.colors), transparent,
    stills/<poster>-<variant>-<colour>[-dark]-half.webp   cropped to the product and its shadow, and at half size
    <poster>.stills.json                                  what was made and where the product sits in each still
  The colourways are the one render with its accent part lit again in each colour (as the poster is),
  so a colour costs no render. --legacy also writes the turntable's four posters (the old places:
  <poster>.fallback-render.png, -render-dark.png, .fallback.png, -dark.png). --variants takes a part of
  the list (one call each keeps a run short); the manifest is merged with the earlier run's.
  spec.supports (a list of view names) is the views the product draws well: make.variants and the page's views stay inside it, and a
  view dropped from it loses its entry in the manifest on the next run (delete its files with it).
  A spec without its posters shows nothing until WebGL draws: run this after every change to a spec.
  A big product renders slowly: up to a few minutes a variant for 60-90 parts. Each variant is
  written as soon as it is made, so a run cut short by --timeout (minutes, default 30) keeps the ones done.
  --quiet leaves out the file lines.
*/
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { build } from './build.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');              // assets/objects: ../_print-core resolves
const args = process.argv.slice(2);
const VALUED = ['--variants', '--grounds', '--px', '--timeout'];
const opt = (k, d) => { const i = args.indexOf('--' + k); return i < 0 ? d : (args[i + 1] && !args[i + 1].startsWith('--') ? args[i + 1] : true); };
const specs = args.filter((a, i) => !a.startsWith('--') && !(i > 0 && VALUED.includes(args[i - 1])));
if (!specs.length) { console.error('usage: make-posters.mjs <spec.json> [...] [--variants a,b+scene] [--grounds light,dark] [--px 1100] [--legacy] [--check]'); process.exit(1); }
const sleep = ms => new Promise(r => setTimeout(r, ms));
const variantsOf = spec => String(typeof opt('variants') === 'string' ? opt('variants') : (spec.make?.variants || []).join(',')).split(',').map(s => s.trim()).filter(Boolean);

// 1. every spec builds, with every view and scene it will make, and its shaders hold no word a GPU rejects
let bad = 0;
for (const s of specs) {
  try {
    const spec = JSON.parse(fs.readFileSync(s, 'utf8')), base = pathToFileURL(path.resolve(s)).href;
    const obj = build(spec, { base });
    const p = obj.problems();
    if (p.length) { bad++; console.log(`${s}: the shader uses words a GPU rejects\n  ${p.join('\n  ')}`); }
    else console.log(`${s}: builds (${obj.spec.parts.length} parts)`);
    for (const v of variantsOf(spec)) {
      const [view, scene] = v.split('+');
      const o = build(spec, { base, view, scene: scene || null });
      const q = o.problems();
      if (q.length) { bad++; console.log(`  ${v}: the shader uses words a GPU rejects\n    ${q.join('\n    ')}`); }
      else console.log(`  ${v}: builds (${o.spec.parts.length} parts; framed ${o.fit.ex.map(x => (2 * x).toFixed(3)).join(' x ')} units)`);
    }
  } catch (e) { bad++; console.log(`${s}: ${e.message}`); }
  const rel = path.relative(root, path.resolve(s));
  if (rel.startsWith('..')) { bad++; console.log(`${s}: keep the spec under ${root} (the poster page loads it from there)`); }
}
if (bad || opt('check')) process.exit(bad ? 1 : 0);

// 2. the posters and stills, in the BrowserOS Neo harness
// the bridge module: in the skill folder beside this one, else the one the running bridge names in .taste-skill/neo.json
async function neoModule() {
  const own = path.join(here, '..', '..', '..', 'scripts', 'neo.mjs');
  if (fs.existsSync(own)) return import(pathToFileURL(own).href);
  for (let d = process.cwd(), i = 0; i < 3; i++, d = path.dirname(d)) {
    try { return await import(pathToFileURL(JSON.parse(fs.readFileSync(path.join(d, '.taste-skill', 'neo.json'), 'utf8')).script).href); } catch { /* not here */ }
  }
  throw new Error('start the BrowserOS Neo bridge first: node <skill>/scripts/neo.mjs serve');
}
const { connect, NeoTab } = await neoModule().catch(e => { console.error('make-posters: ' + e.message); process.exit(1); });
const hub = await connect().catch(e => { console.error('make-posters: ' + e.message); process.exit(1); });
let pageUrl;
try { pageUrl = hub.urlFor(path.join(root, 'product-render', 'make-posters.html')); }
catch { console.error('make-posters: run make-posters from the project folder whose bridge serves it'); process.exit(1); }
let tab = null;
const guard = setTimeout(() => { console.error('make-posters: timed out'); hub.close().finally(() => process.exit(2)); }, (+opt('timeout', 30) + 0.5) * 60 * 1000);
let code = 0;
try {
  tab = await NeoTab.open(hub, { width: 900, height: 900 });
  const ev = x => tab.eval(x, 120000).catch(e => { if (e.neo) throw e; return undefined; });
  for (const s of specs) {
    const rel = path.relative(root, path.resolve(s)).split(path.sep).join('/');
    const spec = JSON.parse(fs.readFileSync(s, 'utf8'));
    // one page load per variant, its files written as soon as it is made (a run cut short keeps what it made)
    const items = variantsOf(spec), batches = items.length ? items.map(v => [v]) : [[]];
    const t0 = Date.now();
    for (const [bi, batch] of batches.entries()) {
      const qs = new URLSearchParams({ spec: '../' + rel });
      if (opt('render-only')) qs.set('only', 'render');
      if (opt('legacy') && bi === 0) qs.set('legacy', '1');
      qs.set('variants', batch.join(','));
      for (const k of ['grounds', 'px']) if (typeof opt(k) === 'string') qs.set(k, opt(k));
      await tab.goto(`${pageUrl}?${qs}`, 60000);
      let state = null;
      while (Date.now() - t0 < +opt('timeout', 30) * 60 * 1000) { await sleep(500); state = await ev('document.documentElement.dataset.ready || ""'); if (state) break; }
      if (state !== '1') { code = 1; console.log(`${s} ${batch.join(',') || 'legacy'}: nothing made (${state ? await ev('window.__posterError') : 'timed out'})`); break; }
      const dir = path.dirname(path.resolve(s));
      for (const name of (await ev('Object.keys(window.__files)')) || []) {
        const data = await ev(`window.__files[${JSON.stringify(name)}]`);
        const file = path.join(dir, name);
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.writeFileSync(file, Buffer.from(data.split(',')[1], 'base64'));
        if (!opt('quiet')) console.log(`  wrote ${name} (${Math.round(fs.statSync(file).size / 1024)} kB)`);
      }
      // the manifest: what was made, merged with the earlier runs' (each variant keeps the size it was made at: a
      // close-up at --px 800 and the hero at 1100 live in one manifest; the page reads each still's own w, h and box)
      const made = await ev('window.__manifest');
      if (made && Object.keys(made.variants).length) {
        const mf = path.join(dir, `${spec.poster || spec.id}.stills.json`);
        let prev = null; try { prev = JSON.parse(fs.readFileSync(mf, 'utf8')); } catch { /* none */ }
        // an earlier run's variants stay, unless the spec no longer supports their view (spec.supports: a view dropped as it read wrong)
        const keep = Object.fromEntries(Object.entries(prev && prev.variants ? prev.variants : {}).filter(([, v]) => !spec.supports || spec.supports.includes(v.view) || /^detail[:-]/.test(v.view)));
        for (const v of Object.values(made.variants)) v.px = made.px;
        const merged = { ...made, variants: { ...keep, ...made.variants } };
        fs.writeFileSync(mf, JSON.stringify(merged, null, 1) + '\n');
        console.log(`  wrote ${path.relative(dir, mf)} (${Object.keys(merged.variants).length} variants)`);
      }
      for (const r of (await ev('window.__report')) || []) console.log(`  ${r.key} (${r.ground}): ${r.frame.join('x')} frame, product measured/predicted ${r.measuredVsPredicted.join(' / ')}, off centre ${r.centrePx.join(', ')} px, ${Math.round(r.ms / 1000)} s`);
    }
    console.log(`${s}: done in ${Math.round((Date.now() - t0) / 1000)} s`);
  }
} catch (e) { code = 1; console.error(e.message); }
clearTimeout(guard);
if (tab) await tab.close();
await hub.close();
process.exit(code);
