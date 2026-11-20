#!/usr/bin/env node
/*
  scenes/_demo/shoot.mjs - the big scenes' own tools (taste-skill, references/scenes.md). Node 22+, no packages.

    node assets/scenes/_demo/shoot.mjs index [--out <file|dir>] [--in-skill]
         scan assets/scenes/<id>/meta.json into index.json. It writes to --out, else to the project's own
         assets/scenes/index.json (under the folder it runs from), else next to this script when this copy is not
         the installed skill; it never rewrites the skill's own index.json unless --in-skill says so
    node assets/scenes/_demo/shoot.mjs probe --only <id> --p 0.5 --expr "<js>"   (one scene at one progress: prints the expression)
    node assets/scenes/_demo/shoot.mjs make --html <url path> --save <file.png>
         (opens the page, waits for window.__out, a data: URL, and saves it: the demo's object render)

  make and probe run through the BrowserOS Neo bridge (node <skill>/scripts/neo.mjs serve, started from the project folder
  that holds this copy's assets/scenes, with its harness open in a BrowserOS Neo tab). To look at a scene's demo, use the
  eyes step (references/gate.md) at 1440x900 and 390x844.
*/
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DIR = path.resolve(HERE, '..');                   // assets/scenes
const ROOT = path.resolve(DIR, '..', '..');             // the taste-skill folder
const INDEX = path.join(DIR, 'index.json');
const IN_SKILL = fs.existsSync(path.join(ROOT, 'SKILL.md'));   // this assets/scenes is the installed skill's own
const FITS = ['hero', 'statement', 'product', 'between', 'closing'];
const COSTS = ['low', 'mid', 'high'];

const argv = process.argv.slice(2);
const cmd = argv[0];
const opt = (k, d) => { const i = argv.indexOf('--' + k); return i < 0 ? d : (argv[i + 1] && !argv[i + 1].startsWith('--') ? argv[i + 1] : true); };
const list = v => (v && v !== true ? String(v).split(',').map(s => s.trim()).filter(Boolean) : null);
const die = m => { console.error(m); process.exit(1); };
const sleep = ms => new Promise(r => setTimeout(r, ms));
// the bridge module: beside this script's skill folder, else the one the running bridge names in .taste-skill/neo.json
async function neoModule() {
  const own = path.join(ROOT, 'scripts', 'neo.mjs');
  if (fs.existsSync(own)) return import(pathToFileURL(own).href);
  for (let d = process.cwd(), i = 0; i < 3; i++, d = path.dirname(d)) {
    try { return await import(pathToFileURL(JSON.parse(fs.readFileSync(path.join(d, '.taste-skill', 'neo.json'), 'utf8')).script).href); } catch { /* not here */ }
  }
  throw new Error('start the BrowserOS Neo bridge first: node <skill>/scripts/neo.mjs serve');
}

/* ------------------------------------------------------------------ index */
function validate(meta, folder, base = DIR) {
  const errs = [];
  for (const k of ['id', 'name', 'look', 'reduced', 'trigger']) if (typeof meta[k] !== 'string' || !meta[k].trim()) errs.push(`meta.${k} must be a non-empty string`);
  if (meta.id && meta.id !== folder) errs.push(`meta.id "${meta.id}" must match the folder "${folder}"`);
  if (!Array.isArray(meta.fits) || !meta.fits.length || meta.fits.some(f => !FITS.includes(f))) errs.push(`meta.fits must list some of ${FITS.join(', ')}`);
  if (!COSTS.includes(meta.cost)) errs.push(`meta.cost must be one of ${COSTS.join(', ')}`);
  if (typeof meta.webgl !== 'number' || meta.webgl < 0 || meta.webgl > 1) errs.push('meta.webgl must be 0 or 1');
  if (typeof meta.hold !== 'number') errs.push('meta.hold must be a number (screens of scroll)');
  if (meta.maxPerPage !== 1) errs.push('meta.maxPerPage must be 1');
  if (!meta.states || !['start', 'middle', 'end'].every(s => meta.states[s])) errs.push('meta.states needs start, middle and end');
  for (const f of ['scene.js', 'scene.css', 'demo.html']) if (!fs.existsSync(path.join(base, folder, f))) errs.push(`${f} is missing`);
  return errs;
}
// where index writes: --out (a .json file, or a folder for its index.json), else the project's own assets/scenes
// under the folder it runs from, else next to this script when this copy is not the installed skill; null when the
// only place left is the skill's own folder (--in-skill writes there on purpose)
// a path compared by its real location, so a symlinked install (install.sh) is the same file
const realOf = (p) => { try { return fs.realpathSync(p).toLowerCase(); } catch { return path.resolve(p).toLowerCase(); } };
function indexTarget() {
  const o = opt('out');
  if (o && o !== true) { const p = path.resolve(String(o)); return /\.json$/i.test(p) ? p : path.join(p, 'index.json'); }
  if (!IN_SKILL || opt('in-skill') === true) return INDEX;
  const local = path.resolve('assets', 'scenes');
  if (fs.existsSync(local) && realOf(local) !== realOf(DIR)) return path.join(local, 'index.json');
  return null;
}
const hasScenes = d => { try { return fs.readdirSync(d, { withFileTypes: true }).some(e => e.isDirectory() && !e.name.startsWith('_') && fs.existsSync(path.join(d, e.name, 'meta.json'))); } catch { return false; } };
const fwd = p => p.replace(/\\/g, '/');
function buildIndex({ quiet = false, write = true } = {}) {
  const target = write ? indexTarget() : null;
  // the scenes read are the ones beside the index it writes (the project's own copy), else this script's own
  const BASE = target && hasScenes(path.dirname(target)) ? path.dirname(target) : DIR;
  const dirs = fs.readdirSync(BASE, { withFileTypes: true }).filter(d => d.isDirectory() && !d.name.startsWith('_')).map(d => d.name).sort();
  const scenes = [], problems = [];
  for (const id of dirs) {
    const mp = path.join(BASE, id, 'meta.json');
    if (!fs.existsSync(mp)) continue;
    let meta;
    try { meta = JSON.parse(fs.readFileSync(mp, 'utf8')); } catch (e) { problems.push(`${id}: meta.json: ${e.message}`); continue; }
    const errs = validate(meta, id, BASE);
    if (errs.length) { problems.push(...errs.map(e => `${id}: ${e}`)); continue; }
    scenes.push({ ...meta, weight: meta.weight ?? 1, needs: meta.needs || {}, types: meta.types || ['software', 'tech', 'gaming'] });
  }
  const fits = {};
  scenes.forEach(s => s.fits.forEach(f => { fits[f] = (fits[f] || 0) + 1; }));
  const index = {
    version: 1, generated: new Date().toISOString(), count: scenes.length, fits,
    rule: 'Tech pages only (software, tech, gaming): at most one big scene a page, dealt at random among those whose fits, types and needs the brief meets (references/scenes.md); phones and reduced motion get the still or the light version; the rest of the page stays calm.',
    load: '<link rel="stylesheet" href="assets/scenes/scenes.css"> and <link rel="stylesheet" href="assets/scenes/<id>/scene.css"> in <head>; <section class="sc" data-big-scene="<id>" data-big-scene-opts=\'{…}\'><div class="sc__stage">…</div></section>; <script type="module" src="assets/scenes/scenes.js"></script> once at the end of <body>.',
    scenes,
  };
  if (write && !target) {
    console.error(`index: not written. ${fwd(INDEX)} belongs to the installed skill, and ${fwd(process.cwd())} ${realOf(path.resolve('assets', 'scenes')) === realOf(DIR) ? 'is the installed skill itself' : 'has no assets/scenes of its own'}: run it from the project folder (it writes <project>/assets/scenes/index.json), pass --out <file|dir>, or --in-skill to rewrite the skill's own index on purpose`);
    process.exitCode = 2;
    return index;
  }
  if (write) { fs.mkdirSync(path.dirname(target), { recursive: true }); fs.writeFileSync(target, JSON.stringify(index, null, 1) + '\n'); }
  if (!quiet && write) console.log(`wrote ${fwd(target)} (from the scenes in ${fwd(BASE)})`);
  if (!quiet) { console.log(`index: ${scenes.length} scenes (${Object.entries(fits).map(([k, n]) => `${k} ${n}`).join(', ')})`); if (problems.length) console.log('problems:\n  ' + problems.join('\n  ')); }
  return index;
}
// read only: a missing or broken index.json is built in memory, never written from here
function readIndex() { try { return JSON.parse(fs.readFileSync(INDEX, 'utf8')); } catch { return buildIndex({ quiet: true, write: false }); } }

/* ------------------------------------------------------------------ PNG (8-bit RGB/RGBA) */
function decodePNG(buf) {
  let pos = 8, w = 0, h = 0, type = 0; const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos), name = buf.toString('ascii', pos + 4, pos + 8), data = buf.subarray(pos + 8, pos + 8 + len);
    if (name === 'IHDR') { w = data.readUInt32BE(0); h = data.readUInt32BE(4); type = data[9]; if (data[8] !== 8 || data[12]) throw new Error('png: unsupported'); }
    else if (name === 'IDAT') idat.push(data);
    pos += 12 + len;
  }
  const bpp = type === 6 ? 4 : type === 2 ? 3 : 0;
  if (!bpp) throw new Error('png: colour type ' + type);
  const raw = zlib.inflateSync(Buffer.concat(idat)), stride = w * bpp, px = Buffer.alloc(w * h * 4);
  let prev = Buffer.alloc(stride);
  for (let y = 0; y < h; y++) {
    const f = raw[y * (stride + 1)], line = Buffer.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let i = 0; i < stride; i++) {
      const a = i >= bpp ? line[i - bpp] : 0, b = prev[i], c = i >= bpp ? prev[i - bpp] : 0;
      let v = line[i];
      if (f === 1) v += a; else if (f === 2) v += b; else if (f === 3) v += (a + b) >> 1;
      else if (f === 4) { const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c); v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c; }
      line[i] = v & 255;
    }
    for (let x = 0; x < w; x++) { const o = (y * w + x) * 4; px[o] = line[x * bpp]; px[o + 1] = line[x * bpp + 1]; px[o + 2] = line[x * bpp + 2]; px[o + 3] = bpp === 4 ? line[x * bpp + 3] : 255; }
    prev = line;
  }
  return { w, h, px };
}
function encodePNG(w, h, px) {
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) px.copy(raw, y * (w * 4 + 1) + 1, y * w * 4, (y + 1) * w * 4);
  const chunk = (name, data) => { const len = Buffer.alloc(4); len.writeUInt32BE(data.length); const body = Buffer.concat([Buffer.from(name, 'ascii'), data]); const crc = Buffer.alloc(4); crc.writeUInt32BE(zlib.crc32(body) >>> 0); return Buffer.concat([len, body, crc]); };
  const ihdr = Buffer.alloc(13); ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 6;
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', ihdr), chunk('IDAT', zlib.deflateSync(raw, { level: 6 })), chunk('IEND', Buffer.alloc(0))]);
}
// box-filter downscale by an integer factor
function shrink(img, f) {
  const w = Math.floor(img.w / f), h = Math.floor(img.h / f), px = Buffer.alloc(w * h * 4);
  for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) {
    let r = 0, g = 0, b = 0;
    for (let j = 0; j < f; j++) for (let i = 0; i < f; i++) { const o = ((y * f + j) * img.w + x * f + i) * 4; r += img.px[o]; g += img.px[o + 1]; b += img.px[o + 2]; }
    const o = (y * w + x) * 4, n = f * f; px[o] = r / n; px[o + 1] = g / n; px[o + 2] = b / n; px[o + 3] = 255;
  }
  return { w, h, px };
}
function sheet(rows, gap = 12, bg = [40, 40, 40]) {
  const W = Math.max(...rows.map(r => r.reduce((s, i) => s + i.w + gap, gap)));
  const H = rows.reduce((s, r) => s + Math.max(...r.map(i => i.h)) + gap, gap);
  const px = Buffer.alloc(W * H * 4);
  for (let i = 0; i < W * H; i++) { px[i * 4] = bg[0]; px[i * 4 + 1] = bg[1]; px[i * 4 + 2] = bg[2]; px[i * 4 + 3] = 255; }
  let y = gap;
  for (const r of rows) {
    let x = gap;
    for (const im of r) { for (let j = 0; j < im.h; j++) im.px.copy(px, ((y + j) * W + x) * 4, j * im.w * 4, (j + 1) * im.w * 4); x += im.w + gap; }
    y += Math.max(...r.map(i => i.h)) + gap;
  }
  return encodePNG(W, H, px);
}

/* ------------------------------------------------------------------ BrowserOS Neo */
async function chrome(_profile, W, H) {
  const { connect, NeoTab } = await neoModule().catch(e => die(e.message));
  const hub = await connect().catch(e => die(e.message));
  if (realOf(hub.root) !== realOf(ROOT)) { await hub.close(); die(`the BrowserOS Neo bridge serves ${hub.root}, not ${ROOT}: restart it from this folder with: node <skill>/scripts/neo.mjs serve`); }
  const tab = await NeoTab.open(hub, { width: W, height: H });
  const logs = [];
  return {
    origin: hub.origin,
    S: (m, p) => tab.send(m, p),
    ev: async (x) => { const v = await tab.eval(x, 300000); logs.push(...await tab.logs()); return v; },
    nav: (u) => tab.goto(u, 20000),
    logs,
    close: async () => { await tab.close(); await hub.close(); },
    kill: () => {},
  };
}

/* ------------------------------------------------------------------ make (an object render) */
async function cmdMake() {
  const html = opt('html'), save = opt('save');
  if (!html || html === true || !save || save === true) die('make: --html <path> --save <file.png>');
  const c = await chrome(null, 1200, 1200);
  try {
    await c.nav(`${c.origin}${html.startsWith('/') ? html : '/' + html}`);
    const data = await c.ev(`new Promise(r => { const t = performance.now(); (function w() { if (window.__out || window.__err) r(window.__out || null); else if (performance.now() - t > 240000) r(null); else setTimeout(w, 100); })(); })`);
    if (!data) {
      const state = await c.ev('JSON.stringify({ ready: document.readyState, title: document.title, err: window.__err || null, url: location.href })');
      console.error('make: the page set no window.__out ' + state + (c.logs.length ? '\n' + c.logs.join('\n') : ''));
      process.exitCode = 1;
    } else {
      fs.writeFileSync(path.resolve(save), Buffer.from(data.split(',')[1], 'base64'));
      console.log(`saved ${save}` + (c.logs.length ? '\n' + c.logs.join('\n') : ''));
    }
  } finally { await c.close(); }
}

/* ------------------------------------------------------------------ probe (one scene, one progress, one expression) */
async function cmdProbe() {
  const only = opt('only'), expr = opt('expr');
  if (!only || only === true || !expr || expr === true) die('probe: --only <id> --p 0.5 --expr "<js>" [--w 1440]');
  const w = +(opt('w', 1440)), H = w < 600 ? 844 : 900, p = +(opt('p', 0.5));
  const c = await chrome(null, w, H);
  try {
    await c.S('Emulation.setDeviceMetricsOverride', { width: w, height: H, deviceScaleFactor: 1, mobile: w < 600 });
    await c.nav(`${c.origin}/assets/scenes/${only}/demo.html`);
    await c.ev(`new Promise(r => { const t = performance.now(); (function w() { if (document.documentElement.dataset.scenesReady || performance.now() - t > 8000) r(); else setTimeout(w, 50); })(); })`);
    await sleep(400);
    await c.ev(`(() => { const r = document.querySelector('[data-big-scene]'); const b = r.getBoundingClientRect(); scrollTo(0, Math.round(b.top + scrollY + ${p} * Math.max(0, b.height - innerHeight))); })()`);
    await sleep(700);
    console.log(JSON.stringify(await c.ev(`(() => { try { return (${expr}); } catch (e) { return 'error: ' + e.message; } })()`), null, 1));
    if (c.logs.length) console.log(c.logs.join('\n'));
  } finally { await c.close(); }
}

if (cmd === 'index') buildIndex();
else if (cmd === 'shoot') die('shoot is not available on BrowserOS Neo; look at the scene demo with the eyes step (references/scenes.md)');
else if (cmd === 'make') cmdMake();
else if (cmd === 'probe') cmdProbe();
else die('usage: shoot.mjs index [--out <file|dir>] [--in-skill] | probe --only <id> --p 0.5 --expr "<js>" | make --html <path> --save <png>');
