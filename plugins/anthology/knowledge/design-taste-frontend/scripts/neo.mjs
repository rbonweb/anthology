// The BrowserOS Neo bridge: a localhost hub that serves the page, plus a harness page the agent opens in its own
// BrowserOS Neo tab. The checkers talk to the hub over 127.0.0.1 HTTP; the hub hands their jobs to the harness, which
// runs them in same-origin iframes sized to each width. No browser is launched and no debugging port is opened.
//   node neo.mjs serve [--root <dir>] [--port 5300]   run from the project folder, in the background
//   node neo.mjs status                               JSON; exit 0 when ready, 3 when not
//   node neo.mjs stop
// The client module (status, connect, NeoTab) is what the ported checkers import.
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const INFO_DIR = '.taste-skill', INFO_FILE = 'neo.json';
const FIRST_PORT = 5300, IDLE_MS = 2 * 3600 * 1000, POLL_MS = 25000, SEEN_MS = 30000;
const MAX_BODY = 256 * 1024 * 1024;
const HARNESS_OPS = new Set(['open', 'resize', 'goto', 'eval', 'logs', 'close', 'ping']);
const NOOP = new Set(['Page.enable', 'Runtime.enable', 'Network.enable', 'Log.enable', 'Page.bringToFront', 'Emulation.setScrollbarsHidden',
  'Emulation.setFocusEmulationEnabled', 'Emulation.setTouchEmulationEnabled', 'Network.setCacheDisabled', 'Page.handleJavaScriptDialog']);
const MIME = { '.html': 'text/html; charset=utf-8', '.htm': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8', '.js': 'text/javascript; charset=utf-8', '.mjs': 'text/javascript; charset=utf-8', '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.jpg': 'image/jpeg', '.jpeg': 'image/jpeg', '.webp': 'image/webp', '.avif': 'image/avif', '.gif': 'image/gif', '.ico': 'image/x-icon', '.woff': 'font/woff', '.woff2': 'font/woff2', '.ttf': 'font/ttf', '.otf': 'font/otf', '.mp4': 'video/mp4', '.webm': 'video/webm', '.glb': 'model/gltf-binary', '.gltf': 'model/gltf+json', '.wasm': 'application/wasm', '.txt': 'text/plain; charset=utf-8', '.glsl': 'text/plain; charset=utf-8', '.frag': 'text/plain; charset=utf-8', '.vert': 'text/plain; charset=utf-8' };
const GZ = new Set(['.html', '.htm', '.css', '.js', '.mjs', '.json', '.svg', '.txt']);
const KIND = { '.html': 'Document', '.htm': 'Document', '.css': 'Stylesheet', '.js': 'Script', '.mjs': 'Script', '.png': 'Image', '.jpg': 'Image', '.jpeg': 'Image', '.webp': 'Image', '.avif': 'Image', '.gif': 'Image', '.svg': 'Image', '.ico': 'Image', '.woff': 'Font', '.woff2': 'Font', '.ttf': 'Font', '.otf': 'Font', '.mp4': 'Media', '.webm': 'Media', '.json': 'Fetch' };
const SAVE_EXT = new Set(['.jpg', '.jpeg', '.png', '.webp']);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const hex = () => crypto.randomBytes(16).toString('hex');
const same = (a, b) => typeof a === 'string' && typeof b === 'string' && a.length === b.length && crypto.timingSafeEqual(Buffer.from(a), Buffer.from(b));
const within = (root, file) => file === root || file.startsWith(root + path.sep);

export class NeoError extends Error {
  constructor(message) { super(message); this.name = 'NeoError'; this.neo = true; }
}

// ---------------------------------------------------------------- the client: what the checkers import
function request(origin, method, pathname, { token, body, ms = 60000 } = {}) {
  return new Promise((resolve, reject) => {
    const u = new URL(pathname, origin);
    const data = body === undefined ? null : Buffer.from(typeof body === 'string' ? body : JSON.stringify(body));
    const headers = { 'X-Neo-Token': token || '' };
    if (data) { headers['Content-Type'] = 'application/json'; headers['Content-Length'] = data.length; }
    const req = http.request({ host: u.hostname, port: u.port, path: u.pathname + u.search, method, headers, timeout: ms + 10000, agent: false }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, body: Buffer.concat(chunks) }));
      res.on('error', reject);
    });
    req.on('timeout', () => req.destroy(new Error('the bridge did not answer')));
    req.on('error', reject);
    req.end(data);
  });
}

function findInfo(cwd, parents = 2) {
  let dir = path.resolve(cwd);
  for (let i = 0; i <= parents; i++, dir = path.dirname(dir)) {
    try { return JSON.parse(fs.readFileSync(path.join(dir, INFO_DIR, INFO_FILE), 'utf8')); } catch { /* none here */ }
  }
  return null;
}
const alive = (pid) => { try { process.kill(pid, 0); return true; } catch { return false; } };
const NOT_RUNNING = 'the BrowserOS Neo bridge is not running: run `node <skill>/scripts/neo.mjs serve` from the project folder in the background';

export async function status({ cwd = process.cwd() } = {}) {
  const info = findInfo(cwd);
  if (!info || !alive(info.pid)) return { ok: false, why: NOT_RUNNING, info };
  let st;
  try {
    const r = await request(info.origin, 'GET', '/__neo/api/status', { token: info.token, ms: 5000 });
    if (r.status !== 200) throw new Error('refused');
    st = JSON.parse(r.body.toString());
  } catch { return { ok: false, why: NOT_RUNNING, info }; }
  let why = null;
  if (!st.harness) why = `open ${info.harness} in a BrowserOS Neo tab and leave it open`;
  else if (!st.visible || st.raf < 20) why = 'the harness tab is hidden: bring its BrowserOS Neo window to the front (mcp__browseros-neo__run page.cdp Page.bringToFront)';
  return { ok: !why, why, info, hub: new Hub(info), st };
}

export async function connect({ cwd = process.cwd(), wait = 20000 } = {}) {
  const t0 = Date.now();
  for (;;) {
    const s = await status({ cwd });
    if (s.ok) return s.hub;
    if (Date.now() - t0 >= wait) throw new NeoError(s.why);
    await sleep(500);
  }
}

class Hub {
  constructor(info) {
    this.origin = info.origin; this.root = info.root; this.token = info.token; this.tabs = new Set();
  }
  async rpc(op, params = {}, ms = 60000) {
    let r;
    try { r = await request(this.origin, 'POST', '/__neo/api/rpc', { token: this.token, body: { ...params, op, ms }, ms }); }
    catch (e) { throw new NeoError(NOT_RUNNING + ` (${e.message})`); }
    if (r.status === 504) throw new NeoError(`BrowserOS Neo did not answer within ${ms} ms: is the harness tab open and in front?`);
    if (r.status !== 200) throw new NeoError(`the bridge answered ${r.status}: ${r.body.toString().slice(0, 200)}`);
    return JSON.parse(r.body.toString());
  }
  async call(op, params, ms) {
    const r = await this.rpc(op, params, ms);
    if (!r.ok) throw new Error(String(r.error));
    return r.value && r.value.__undef ? undefined : r.value;
  }
  urlFor(file) {
    let abs = path.resolve(file);
    try { abs = fs.realpathSync(abs); } catch { /* not there yet */ }
    const rel = path.relative(this.root, abs);
    if (rel === '..' || rel.startsWith('..' + path.sep) || path.isAbsolute(rel)) {
      throw new NeoError(`the page is outside the bridge's root ${this.root}: restart it with neo.mjs serve --root <dir>`);
    }
    return this.origin + '/' + rel.split(path.sep).map(encodeURIComponent).join('/');
  }
  on() { return () => {}; }
  send() { return Promise.resolve({}); }
  async close() { await Promise.all([...this.tabs].map((t) => t.close())); }
}

export class NeoTab {
  static async open(hub, { width, height } = {}, init = []) {
    const t = new NeoTab();
    t.hub = hub; t.id = 't' + hex().slice(0, 12); t.vw = width; t.vh = height; t.failed = [];
    for (const source of init) await hub.call('init', { tab: t.id, source });
    await hub.call('open', { tab: t.id, w: width, h: height });
    hub.tabs.add(t);
    return t;
  }
  async send(method, params = {}) {
    if (method === 'Page.addScriptToEvaluateOnNewDocument') return this.hub.call('init', { tab: this.id, source: params.source });
    if (method === 'Emulation.setDeviceMetricsOverride') {
      await this.hub.call('resize', { tab: this.id, w: params.width, h: params.height });
      this.vw = params.width; this.vh = params.height;
      return {};
    }
    if (NOOP.has(method)) return {};
    if (method === 'Runtime.evaluate') return { result: { value: await this.eval(params.expression) } };
    throw new NeoError(`${method} is not available on BrowserOS Neo`);
  }
  async goto(url, ms = 30000) {
    this.failed = [];
    return this.hub.call('goto', { tab: this.id, url, ms }, ms + 5000);
  }
  async eval(expr, ms = 60000) {
    const r = await this.hub.rpc('eval', { tab: this.id, expr }, ms);
    if (!r.ok) throw new Error('page: ' + String(r.error).slice(0, 500));
    return r.value && r.value.__undef ? undefined : r.value;
  }
  async shot() { return null; }
  logs() { return this.hub.call('logs', { tab: this.id }, 10000); }
  async requests() {
    const r = await request(this.hub.origin, 'GET', '/__neo/api/requests?tab=' + encodeURIComponent(this.id), { token: this.hub.token, ms: 10000 });
    return r.status === 200 ? JSON.parse(r.body.toString()) : [];
  }
  async syncFailed() {
    this.failed = (await this.requests()).filter((r) => r.status >= 400).map((r) => ({ url: r.url, type: r.type, why: String(r.status) }));
  }
  async close() {
    this.hub.tabs.delete(this);
    try { await this.hub.call('close', { tab: this.id }, 5000); } catch { /* the harness may be gone */ }
  }
}

// ---------------------------------------------------------------- pages the hub injects and serves
const LOGGER = `(function () {
  var L = window.__neoLogs = [];
  function put(s) { if (L.length < 50) L.push(String(s).slice(0, 2000)); }
  function first(e) { return String((e && (e.stack || e.message)) || e).split('\\n')[0]; }
  window.addEventListener('error', function (e) { put('exception: ' + first(e.error || e.message)); });
  window.addEventListener('unhandledrejection', function (e) { put('exception: Uncaught (in promise) ' + first(e.reason)); });
  [['error', 'error'], ['warn', 'warning']].forEach(function (k) {
    var orig = console[k[0]];
    console[k[0]] = function () {
      try { put(k[1] + ': ' + [].map.call(arguments, function (a) { return typeof a === 'string' ? a : (a && a.message) || JSON.stringify(a); }).join(' ')); } catch (e) {}
      return orig.apply(console, arguments);
    };
  });
})();`;
const UA_SHIM = `(function () {
  try {
    var ua = navigator.userAgent;
    Object.defineProperty(Navigator.prototype, 'userAgent', { get: function () { return ua.replace('Chrome/', 'HeadlessChrome/'); }, configurable: true });
  } catch (e) {}
})();`;

const HARNESS = `<!doctype html>
<html><head><meta charset="utf-8"><title>neo harness</title>
<style>html,body{margin:0;background:#fff;font:14px system-ui,sans-serif}#note{position:fixed;right:0;bottom:0;z-index:2;padding:6px 10px;background:#111;color:#fff}#frames{position:relative}#frames iframe{position:absolute;left:0;top:0;border:0;background:#fff}</style></head>
<body><div id="frames"></div><div id="note">BrowserOS Neo harness: leave this tab open</div>
<script>
const T = '%TOKEN%';
const frames = new Map();
let seq = 0, raf = 0, frameCount = 0;
const post = (p, body) => fetch('/__neo/' + p, { method: 'POST', body });
(function tick() { frameCount++; requestAnimationFrame(tick); })();
setInterval(() => { raf = frameCount; frameCount = 0; }, 1000);
const frameOf = (job) => { const f = frames.get(job.tab); if (!f) throw new Error('no frame for tab ' + job.tab); return f; };
const ops = {
  async open(job) {
    const f = document.createElement('iframe');
    f.style.width = job.w + 'px'; f.style.height = job.h + 'px'; f.src = 'about:blank';
    document.getElementById('frames').appendChild(f);
    frames.set(job.tab, f);
  },
  async resize(job) { const f = frameOf(job); f.style.width = job.w + 'px'; f.style.height = job.h + 'px'; },
  async goto(job) {
    const f = frameOf(job);
    const u = new URL(job.url);
    u.searchParams.set('__neo', job.tab); u.searchParams.set('__neo_n', String(++seq));
    const status = await new Promise((resolve) => {
      const timer = setTimeout(() => { f.onload = null; resolve('timeout'); }, job.ms || 30000);
      f.onload = () => { clearTimeout(timer); f.onload = null; resolve('load'); };
      f.src = u.href;
    });
    return { url: job.url, status };
  },
  async eval(job) {
    const w = frameOf(job).contentWindow;
    let timer;
    const limit = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('timeout after ' + job.ms + ' ms')), job.ms || 60000); });
    try {
      const v = await Promise.race([(0, w.eval)(job.expr), limit]);
      return v === undefined ? { __undef: true } : JSON.parse(JSON.stringify(v));
    } finally { clearTimeout(timer); }
  },
  async logs(job) { const w = frameOf(job).contentWindow; return w.__neoLogs ? w.__neoLogs.splice(0) : []; },
  async close(job) { const f = frames.get(job.tab); if (f) { f.remove(); frames.delete(job.tab); } },
  async ping() { return { visible: document.visibilityState === 'visible', raf }; },
};
async function run(job) {
  let out;
  try { out = { ok: true, value: await ops[job.op](job) }; } catch (e) { out = { ok: false, error: String((e && e.stack) || e) }; }
  let body;
  try { body = JSON.stringify(out); } catch (e) { body = JSON.stringify({ ok: false, error: 'the result is not JSON: ' + e }); }
  await post('done?t=' + T + '&id=' + encodeURIComponent(job.id), body);
}
(async () => {
  await post('hello?t=' + T, '{}');
  document.title = 'neo harness: connected';
  const beat = () => fetch('/__neo/beat?t=' + T + '&v=' + document.visibilityState + '&r=' + raf).catch(() => {});
  beat(); setInterval(beat, 2000);
  for (;;) {
    try {
      const r = await fetch('/__neo/next?t=' + T + '&v=' + document.visibilityState + '&r=' + raf);
      if (r.status === 200) run(await r.json());
    } catch (e) { document.title = 'neo harness: waiting'; await new Promise((res) => setTimeout(res, 1000)); }
  }
})();
</script></body></html>
`;

// ---------------------------------------------------------------- the hub
function startHub({ root, port0 }) {
  const token = hex(), saveToken = hex();
  const jobs = [], pending = new Map(), inits = new Map(), logs = new Map();
  const seen = { at: 0, visible: false, raf: 0 };
  let port = 0, origins = [], lastApi = Date.now(), lastTab = null, jobSeq = 0, poll = null;

  const log = (tab, entry) => { if (!logs.has(tab)) logs.set(tab, []); logs.get(tab).push(entry); };
  const send = (res, code, body, type = 'application/json', extra = {}) => {
    const data = typeof body === 'string' || Buffer.isBuffer(body) ? body : JSON.stringify(body);
    res.writeHead(code, { 'Content-Type': type, 'Content-Length': Buffer.byteLength(data), 'Cache-Control': 'no-store', ...extra });
    res.end(data);
  };
  const readBody = (req, limit) => new Promise((resolve, reject) => {
    const chunks = []; let n = 0;
    req.on('data', (c) => { n += c.length; if (n > limit) { reject(new Error('too large')); req.destroy(); } else chunks.push(c); });
    req.on('end', () => resolve(Buffer.concat(chunks)));
    req.on('error', reject);
  });
  const fail = (id, error) => { const p = pending.get(id); if (p) { pending.delete(id); clearTimeout(p.timer); p.resolve({ ok: false, error }); } };

  function release(job) {
    if (poll) { const p = poll; poll = null; clearTimeout(p.timer); p.reply(job); }
  }

  function queue(job, ms) {
    return new Promise((resolve) => {
      job.id = 'j' + (++jobSeq);
      const timer = setTimeout(() => {
        pending.delete(job.id);
        const i = jobs.indexOf(job); if (i >= 0) jobs.splice(i, 1);
        resolve({ timeout: true });
      }, ms + 5000);
      pending.set(job.id, { resolve, timer });
      jobs.push(job);
      if (poll) release(jobs.shift());
    });
  }

  async function handleRpc(req, res) {
    let b;
    try { b = JSON.parse((await readBody(req, 32 * 1024 * 1024)).toString()); } catch { return send(res, 400, { ok: false, error: 'bad json' }); }
    const tab = String(b.tab || ''), ms = Number(b.ms) > 0 ? Number(b.ms) : 60000;
    if (b.op === 'init') {
      if (!inits.has(tab)) inits.set(tab, []);
      inits.get(tab).push(String(b.source || ''));
      return send(res, 200, { ok: true, value: { identifier: String(inits.get(tab).length) } });
    }
    if (!HARNESS_OPS.has(b.op)) return send(res, 400, { ok: false, error: 'unknown op ' + b.op });
    if (b.op === 'goto') { logs.set(tab, []); lastTab = tab; }
    const job = { ...b, tab, ms };
    const out = await queue(job, ms);
    if (b.op === 'close') { inits.delete(tab); logs.delete(tab); if (lastTab === tab) lastTab = null; }
    if (out.timeout) return send(res, 504, { ok: false, error: `no answer within ${ms} ms` });
    return send(res, 200, out);
  }

  async function handleSave(req, res, u) {
    const file = u.searchParams.get('file') || '';
    const parts = file.split('/');
    const bad = !file || file.includes('\0') || file.includes('\\') || file.startsWith('/') || parts.some((s) => s === '' || s === '.' || s === '..');
    if (bad || parts.length < 2) return send(res, 400, { error: 'bad file name' });
    if (!SAVE_EXT.has(path.extname(file).toLowerCase()) || !(parts[0] === 'shots' || parts[0] === INFO_DIR)) {
      return send(res, 403, { error: 'only images under shots/ or ' + INFO_DIR + '/ can be saved' });
    }
    let text;
    try { text = (await readBody(req, MAX_BODY)).toString('latin1'); } catch { return send(res, 413, { error: 'too large' }); }
    const comma = text.startsWith('data:') ? text.indexOf(',') : -1;
    const data = Buffer.from(comma >= 0 ? text.slice(comma + 1) : text, 'base64');
    if (!data.length) return send(res, 400, { error: 'empty image' });
    const target = path.join(root, ...parts);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    if (!within(root, fs.realpathSync(path.dirname(target)))) return send(res, 403, { error: 'outside the root' });
    fs.writeFileSync(target, data);
    return send(res, 200, { file, bytes: data.length });
  }

  function handleNeo(req, res, u) {
    const sub = u.pathname.slice('/__neo'.length) || '/';
    if (sub === '/' && req.method === 'GET') return send(res, 200, HARNESS.replace('%TOKEN%', token), 'text/html; charset=utf-8');
    if (sub.startsWith('/api/')) {
      const given = req.headers['x-neo-token'];
      if (!same(given, sub === '/api/save' ? saveToken : token)) return send(res, 401, { error: 'token' });
      lastApi = Date.now();
      const guard = (p) => p.catch(() => { if (!res.headersSent) send(res, 500, { error: 'failed' }); });
      if (sub === '/api/rpc' && req.method === 'POST') return guard(handleRpc(req, res));
      if (sub === '/api/save' && req.method === 'POST') return guard(handleSave(req, res, u));
      if (sub === '/api/status' && req.method === 'GET') {
        return send(res, 200, { root, port, harness: Date.now() - seen.at < SEEN_MS, visible: seen.visible, raf: seen.raf });
      }
      if (sub === '/api/requests' && req.method === 'GET') return send(res, 200, logs.get(u.searchParams.get('tab')) || []);
      return send(res, 404, { error: 'no such call' });
    }
    if (!['/next', '/done', '/hello', '/beat'].includes(sub)) return send(res, 404, { error: 'not found' });
    if (!same(u.searchParams.get('t'), token)) return send(res, 401, { error: 'token' });
    if ((sub === '/next' || sub === '/beat') && req.method === 'GET') {
      seen.at = Date.now(); seen.visible = u.searchParams.get('v') === 'visible'; seen.raf = Number(u.searchParams.get('r')) || 0;
      if (sub === '/beat') return send(res, 204, '');
      if (jobs.length) return send(res, 200, jobs.shift());
      release(null);
      const wait = Math.min(POLL_MS, (Number(u.searchParams.get('wait')) || POLL_MS / 1000) * 1000);
      const reply = (job) => (job ? send(res, 200, job) : send(res, 204, ''));
      poll = { reply, timer: setTimeout(() => { poll = null; reply(null); }, wait) };
      res.on('close', () => { if (poll && poll.reply === reply) { clearTimeout(poll.timer); poll = null; } });
      return undefined;
    }
    if (sub === '/done' && req.method === 'POST') {
      return readBody(req, MAX_BODY).then((body) => {
        const p = pending.get(u.searchParams.get('id'));
        if (p) { pending.delete(u.searchParams.get('id')); clearTimeout(p.timer); try { p.resolve(JSON.parse(body.toString())); } catch { p.resolve({ ok: false, error: 'the harness sent bad json' }); } }
        send(res, 204, '');
      }, () => send(res, 413, { error: 'too large' }));
    }
    if (sub === '/hello' && req.method === 'POST') {
      for (const id of [...pending.keys()]) fail(id, 'harness reloaded');
      jobs.length = 0; release(null); seen.at = Date.now();
      return send(res, 204, '');
    }
    return send(res, 405, { error: 'method' });
  }

  const INJECT = (tab) => [LOGGER, UA_SHIM, ...(inits.get(tab) || [])].map((s) => `<script data-neo-init>${s}</script>`).join('');
  function inject(html, tab) {
    const block = INJECT(tab);
    for (const re of [/<head(?:\s[^>]*)?>/i, /<html(?:\s[^>]*)?>/i, /^\s*<!doctype[^>]*>/i]) {
      const m = html.match(re);
      if (m) return html.slice(0, m.index + m[0].length) + block + html.slice(m.index + m[0].length);
    }
    return block + html;
  }

  function handleStatic(req, res, u) {
    if (req.method !== 'GET' && req.method !== 'HEAD') return send(res, 405, 'method not allowed', 'text/plain');
    let pathname;
    try { pathname = decodeURIComponent(u.pathname); } catch { return send(res, 400, 'bad path', 'text/plain'); }
    if (pathname.includes('\0')) return send(res, 400, 'bad path', 'text/plain');
    let file = path.resolve(root, '.' + pathname);
    if (!within(root, file)) return send(res, 403, 'forbidden', 'text/plain');
    const secret = path.relative(root, file).split(path.sep)[0] === INFO_DIR && !SAVE_EXT.has(path.extname(file).toLowerCase());
    let st;
    try {
      st = fs.statSync(file);
      if (st.isDirectory()) { file = path.join(file, 'index.html'); st = fs.statSync(file); }
      if (secret || !within(root, fs.realpathSync(file))) throw new Error('hidden');
    } catch { st = null; }
    const own = u.searchParams.get('__neo');
    let tab = own || null;
    if (!tab) { try { tab = new URL(req.headers.referer || '').searchParams.get('__neo'); } catch { tab = null; } }
    tab = tab || lastTab;
    const ext = path.extname(file).toLowerCase();
    const shown = new URL(u); shown.searchParams.delete('__neo'); shown.searchParams.delete('__neo_n');
    const entry = { url: shown.href, type: KIND[ext] || 'Other', status: st ? 200 : 404, bytes: 0, decoded: st ? st.size : 0, mime: MIME[ext] || 'application/octet-stream' };
    if (!st) { entry.failed = true; if (tab) log(tab, entry); return send(res, 404, 'not found', 'text/plain'); }
    let body = fs.readFileSync(file);
    if (own && (ext === '.html' || ext === '.htm')) body = Buffer.from(inject(body.toString('utf8'), own));
    const headers = { 'Content-Type': entry.mime, 'Cache-Control': 'no-store' };
    if (GZ.has(ext) && /\bgzip\b/.test(req.headers['accept-encoding'] || '')) { body = zlib.gzipSync(body, { level: 6 }); headers['Content-Encoding'] = 'gzip'; }
    headers['Content-Length'] = body.length;
    entry.bytes = body.length;
    if (tab) log(tab, entry);
    res.writeHead(200, headers);
    res.end(req.method === 'HEAD' ? undefined : body);
    return undefined;
  }

  const server = http.createServer((req, res) => {
    try {
      const host = req.headers.host;
      if (host !== `127.0.0.1:${port}` && host !== `localhost:${port}`) return send(res, 421, 'misdirected request', 'text/plain');
      if (req.headers.origin && !origins.includes(req.headers.origin)) return send(res, 403, 'foreign origin', 'text/plain');
      const u = new URL(req.url, `http://127.0.0.1:${port}`);
      if (u.pathname === '/__neo' || u.pathname.startsWith('/__neo/')) return handleNeo(req, res, u);
      return handleStatic(req, res, u);
    } catch (e) {
      if (!res.headersSent) send(res, 500, 'error', 'text/plain');
      return undefined;
    }
  });

  return new Promise((resolve, reject) => {
    let p = port0;
    const attempt = () => {
      if (p > port0 + 9) return reject(new Error(`no free port in ${port0}-${port0 + 9}`));
      server.once('error', () => { p++; attempt(); });
      server.listen(p, '127.0.0.1', () => {
        server.removeAllListeners('error');
        port = p;
        origins = [`http://127.0.0.1:${port}`, `http://localhost:${port}`];
        resolve({ server, port, token, saveToken, idle: () => Date.now() - lastApi });
      });
    };
    attempt();
  });
}

// ---------------------------------------------------------------- the command line
function arg(name) { const i = process.argv.indexOf(name); return i > 0 ? process.argv[i + 1] : undefined; }

async function serve() {
  const cwd = fs.realpathSync(process.cwd());
  const existing = findInfo(cwd, 0);
  if (existing && existing.pid !== process.pid && alive(existing.pid)) {
    console.error(`neo: a bridge is already running (pid ${existing.pid}) for ${existing.root}; stop it first: node <skill>/scripts/neo.mjs stop\nNEO: open the harness in a BrowserOS Neo tab: ${existing.harness}`);
    process.exit(2);
  }
  let root;
  try { root = fs.realpathSync(path.resolve(cwd, arg('--root') || '.')); } catch { console.error('neo: --root does not exist'); process.exit(2); }
  if (!within(cwd, root)) { console.error(`neo: --root must be inside the current folder (${cwd})`); process.exit(2); }
  const hub = await startHub({ root, port0: Number(arg('--port')) || FIRST_PORT }).catch((e) => { console.error('neo: ' + e.message); process.exit(2); });
  const origin = `http://127.0.0.1:${hub.port}`, harness = origin + '/__neo/';
  const dir = path.join(cwd, INFO_DIR), file = path.join(dir, INFO_FILE);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, '.gitignore'), '*\n');
  fs.writeFileSync(file, JSON.stringify({ pid: process.pid, port: hub.port, origin, root, token: hub.token, saveToken: hub.saveToken, harness, script: fileURLToPath(import.meta.url), started: new Date().toISOString() }, null, 2) + '\n', { mode: 0o600 });
  const done = (code) => { try { if (findInfo(cwd)?.pid === process.pid) fs.rmSync(file, { force: true }); } catch { /* gone already */ } hub.server.close(); process.exit(code); };
  for (const s of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(s, () => done(0));
  setInterval(() => { if (hub.idle() > IDLE_MS) done(0); }, 60000).unref();
  console.log(`NEO: serving ${root} at ${origin}/`);
  console.log(`NEO: open the harness in a BrowserOS Neo tab: ${harness}`);
}

async function showStatus() {
  const s = await status();
  const out = { ok: s.ok, why: s.why, root: s.info?.root ?? null, port: s.info?.port ?? null, harness: s.st?.harness ?? false, visible: s.st?.visible ?? false };
  console.log(JSON.stringify(out));
  process.exit(s.ok ? 0 : 3);
}

async function stop() {
  const info = findInfo(process.cwd());
  if (!info) { console.log('NEO: no bridge is running'); return; }
  const where = path.resolve(process.cwd());
  let file = null;
  for (let dir = where, i = 0; i < 3 && !file; i++, dir = path.dirname(dir)) { const f = path.join(dir, INFO_DIR, INFO_FILE); if (fs.existsSync(f)) file = f; }
  let ours = false;
  try { ours = (await request(info.origin, 'GET', '/__neo/api/status', { token: info.token, ms: 3000 })).status === 200; } catch { ours = false; }
  if (ours && alive(info.pid)) {
    process.kill(info.pid, 'SIGTERM');
    for (let i = 0; i < 50 && alive(info.pid); i++) await sleep(100);
    if (alive(info.pid)) process.kill(info.pid, 'SIGKILL');
  }
  if (file) fs.rmSync(file, { force: true });
  console.log('NEO: stopped');
}

const here = process.argv[1] && (() => { try { return fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url)); } catch { return false; } })();
if (here) {
  const cmd = process.argv[2];
  if (cmd === 'serve') await serve();
  else if (cmd === 'status') await showStatus();
  else if (cmd === 'stop') await stop();
  else { console.error('usage: node neo.mjs serve [--root <dir>] [--port 5300] | status | stop'); process.exit(2); }
}
