#!/usr/bin/env node
// Phone-flow browser checks for the builtin yinz web app (test-only; never shipped).
//
//   node run.mjs <base-url> [filter]   password from YINZ_WEB_PASSWORD or one stdin line;
//                                      the optional filter runs scenarios whose name contains it
//   PLAYWRIGHT_BROWSERS_PATH=<dir>     required: the provisioned Playwright Firefox
//
// LIVE (real server): /login and / pages, /static/** assets and module graph,
// POST /api/login, GET /api/session, POST /api/logout and /api/logout-all.
// INTERCEPTED (page.route fixtures, labelled `intercepted` below): the agent-data routes
// /api/agents, /api/history, /api/hit, /api/event, /api/tools, /api/usage, /api/prompts and
// /api/agent/*. Each intercepted request first replays its Authorization header against the
// live GET /api/session, so revoked or missing bearers still get the real native 401.
//
// The password, tokens, Authorization headers, login bodies and draft text are never
// printed; failure screenshots (target/web-browser-results/) mask those fields.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, '../../../../..');
const NPM_CI = 'npm ci --prefix yinz-steel-builtins/src/unit/web/browser --ignore-scripts --no-audit --no-fund';
const INSTALL = 'env PLAYWRIGHT_BROWSERS_PATH=<worktree>/target/web-browser node yinz-steel-builtins/src/unit/web/browser/node_modules/playwright/cli.js install firefox';

function setupFailure(message) {
  console.log(`FAIL setup: ${message}`);
  process.exit(2);
}

// ---------------------------------------------------------------------------------------
// Inputs

const baseArg = process.argv[2] || process.env.YINZ_WEB_BASE_URL || '';
let ORIGIN;
try {
  const u = new URL(baseArg);
  if (u.protocol !== 'http:' && u.protocol !== 'https:') throw new Error('scheme');
  ORIGIN = u.origin;
} catch {
  setupFailure('pass the server base URL, e.g. `node run.mjs http://127.0.0.1:PORT` (or YINZ_WEB_BASE_URL).');
}

async function readPassword() {
  if (typeof process.env.YINZ_WEB_PASSWORD === 'string') {
    const p = process.env.YINZ_WEB_PASSWORD;
    delete process.env.YINZ_WEB_PASSWORD; // never inherited by the browser process
    return p;
  }
  if (process.stdin.isTTY) return '';
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return Buffer.concat(chunks).toString('utf8').replace(/\r?\n$/, '');
}
const PASSWORD = await readPassword();
if (!PASSWORD) setupFailure('provide the web password through YINZ_WEB_PASSWORD or one line on stdin.');

// Playwright's debug channels can echo typed input; never enable them here.
delete process.env.DEBUG;
delete process.env.PWDEBUG;

const BROWSERS = process.env.PLAYWRIGHT_BROWSERS_PATH;
if (!BROWSERS || !path.isAbsolute(BROWSERS)) {
  setupFailure(`PLAYWRIGHT_BROWSERS_PATH must name the absolute provisioned browser directory. Provision it with:\n  ${NPM_CI}\n  ${INSTALL}`);
}
let firefox;
try {
  ({ firefox } = await import('playwright'));
} catch (e) {
  setupFailure(`the test-only playwright package is not installed (${e.code || e.message}). Run from the repository root:\n  ${NPM_CI}`);
}
const executable = firefox.executablePath();
if (!executable || !fs.existsSync(executable)) {
  setupFailure(`Playwright Firefox is missing under ${BROWSERS} (expected ${executable}). Run from the repository root:\n  ${INSTALL}`);
}

function redact(text) {
  let t = String(text ?? '');
  if (PASSWORD) t = t.split(PASSWORD).join('[password]');
  return t.replace(/yzw1_[A-Za-z0-9_-]+/g, 'yzw1_[token]').replace(/Bearer\s+\S+/gi, 'Bearer [token]');
}

// ---------------------------------------------------------------------------------------
// Fixtures for the intercepted agent-data routes (deterministic, mutable per context)

const INTERCEPTED = new Set(['/api/agents', '/api/history', '/api/hit', '/api/event', '/api/tools', '/api/usage', '/api/prompts']);
const intercepted = (pathname) => INTERCEPTED.has(pathname) || pathname.startsWith('/api/agent/');
const AGENT_ID = 'live:dev:7';
const REF7 = { id: AGENT_ID, kind: 'codex', session_id: 'conv-1' };
const REF7B = { id: AGENT_ID, kind: 'codex', session_id: 'conv-2' };
const XSS = (n) => `<img src=x onerror="window.__yinzXss=${n}">`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const detailHash = (run = 'run-7', tab = 'timeline') => `#/a/${encodeURIComponent(AGENT_ID)}?run=${run}&tab=${tab}`;
const FILLER = 'The agent kept working through the change and reported progress in a few longer sentences so phone rows wrap.';

class Fixture {
  constructor() {
    this.latest = 45;
    this.gone = false;
    this.retarget = false;
    this.prompt = 'submitted';
    this.revokeOnPrompt = false;
    this.screenTick = 0;
    this.rows = null; // optional successful list payload, including [] for the empty state
    this.fullReply = null;
    this.fail = new Map(); // path -> {mode:'abort'|'hold'|'reply', times, status, headers, body}
    this.held = [];
    this.log = [];
    this.problems = [];
  }
  count(pathname, method) {
    return this.log.filter((e) => e.path === pathname && (!method || e.method === method)).length;
  }
  bodies(pathname) {
    return this.log.filter((e) => e.path === pathname && e.body).map((e) => e.body);
  }
  releaseHeld() {
    for (const route of this.held.splice(0)) route.abort('failed').catch(() => {});
  }
}

function row(fx, id, state, extra = {}) {
  const retargeted = fx.retarget && id === AGENT_ID;
  const r = {
    id, ref: retargeted ? { ...REF7B } : { id, kind: 'codex', session_id: 'conv-1' },
    session: 'browser-check', current_session: true, pane_id: id.split(':').pop(), provider: 'codex', state,
    glyph: { waiting: '?', busy: '●', idle: '○' }[state] || '?',
    status: { waiting: 'needs input', busy: 'working', idle: 'idle' }[state] || state,
    host: null, title: `Agent ${id.split(':').pop()}`, activity: 'editing', project: 'yinz',
    branch: 'main', last_reply: { text: 'Done with step 1', truncated: false }, last_hook_at_ms: 1, focused: false,
    selectable: true, suppressed: false, directory: '/w', run: retargeted ? 'run-7b' : `run-${id.split(':').pop()}`,
    wait_needs_a_person: state === 'waiting', source: 'hook', can_prompt: true, prompt_unavailable: null,
    usage_unavailable: null, ...extra,
  };
  // Mirror the default Scheme hook's structure, paired with real /api/agents Scheme
  // assertions in web.test.scm; this fixture is not evidence of Scheme execution.
  const source = r.last_reply?.text || '';
  const chars = Array.from(source);
  const preview = chars.slice(0, 160).join('') + (chars.length > 160 || r.last_reply?.truncated ? '…' : '');
  const badges = (r.badges || []).map((b) => `<span class="badge" data-tone="${esc(b.tone || '')}">${esc(b.text)}</span>`).join('');
  r.html = `<div class="agent-row" data-state="${esc(r.state)}"><div class="agent-row-heading"><span class="agent-glyph" aria-hidden="true">${esc(r.glyph)}</span><strong class="agent-title">${esc(r.title.trim() ? r.title : r.provider)}</strong><span class="agent-status">${esc(r.status)}</span></div><div class="agent-meta">${[r.provider, r.project, r.branch].filter((s) => s && s.trim()).map(esc).join(' · ')}</div>${badges ? `<div class="badges">${badges}</div>` : ''}${source.trim() ? `<p class="agent-reply">${esc(preview)}</p>` : ''}</div>`;
  if (r.wrapper) r.html = `<div class="custom-row">${r.html}</div>`;
  return r;
}

function agents(fx) {
  if (fx.rows) return {
    session: 'browser-check', now_ms: Date.now(),
    counts: Object.fromEntries(['waiting', 'busy', 'idle'].map((s) => [s, fx.rows.filter((r) => r.state === s).length])),
    agents: fx.rows,
  };
  return {
    session: 'browser-check', now_ms: Date.now(), counts: { waiting: 1, busy: 1, idle: 1 },
    agents: [
      row(fx, AGENT_ID, 'waiting', { title: `Fix flaky test ${XSS(1)}` }),
      row(fx, 'live:dev:8', 'busy', { focused: true, title: 'Refactor parser' }),
      row(fx, 'live:peer:9', 'idle', {
        current_session: false, session: 'peer', host: 'box', title: 'Peer agent', can_prompt: false,
        prompt_unavailable: `No conversation ID ${XSS(2)}`,
      }),
    ],
  };
}

function time(rev) {
  return `12:${String(rev % 60).padStart(2, '0')}`;
}

function ev(run, rev, kind, text, extra = {}) {
  return {
    revision: rev, revisions: [rev], range: null, kind, created_at_ms: rev * 60000, time: time(rev), markdown: text,
    truncated: false, tool_calls: 0, request: { op: 'event', run, revision: rev }, html: `<p>${esc(text)}</p>`, ...extra,
  };
}

function toolsEvent(run, rev) {
  const range = { start: { created_at_ms: (rev - 1) * 60000, revision: rev - 1 }, end: { created_at_ms: (rev + 1) * 60000, revision: rev + 1 }, worker: null };
  return {
    revision: rev, revisions: [], range, kind: 'tools', created_at_ms: rev * 60000, time: time(rev), markdown: '',
    truncated: false, tool_calls: 3, request: { op: 'tools', run, range, before: null },
  };
}

function runEvent(run, rev) {
  if (rev === 30) return toolsEvent(run, rev);
  const kind = rev % 5 === 0 ? 'prompt' : 'message';
  return ev(run, rev, kind, `Message ${rev}. ${kind === 'prompt' ? 'Please continue.' : FILLER}`, rev === 44 ? { truncated: true } : {});
}

function history(fx, q) {
  const requested = q.get('run');
  const agent = q.get('agent');
  const live = agent && !fx.gone ? row(fx, agent, 'waiting', { title: `Fix flaky test ${XSS(1)}` }) : null;
  const retargeted = !!agent && fx.retarget && requested === 'run-7';
  const run = retargeted ? 'run-7b' : requested;
  const before = q.get('before') ? JSON.parse(q.get('before')) : null;
  const after = q.has('after_revision') ? Number(q.get('after_revision')) : null;
  let events;
  let earlier = false;
  let latest = fx.latest;
  if (run === 'run-7') {
    if (before) events = Array.from({ length: 19 }, (_, i) => runEvent(run, i + 1));
    else if (after !== null) events = Array.from({ length: Math.max(0, fx.latest - after + 1) }, (_, i) => runEvent(run, after + i));
    else {
      events = Array.from({ length: fx.latest - 19 }, (_, i) => runEvent(run, 20 + i));
      earlier = true;
    }
  } else if (run === 'run-7b') {
    latest = 3;
    events = after !== null && !retargeted ? [] : [1, 2, 3].map((r) => ev(run, r, 'message', `Retargeted run message ${r}`));
  } else {
    latest = 5;
    events = after !== null ? [] : [1, 2, 3, 4, 5].map((r) => ev(run, r, r === 1 ? 'prompt' : 'message', `Stored ${run} message ${r}`));
  }
  const bound = (r) => (agent ? { ...r, agent } : r);
  const navigation = {
    latest: bound({ op: 'history', run, worker: null, before: null, after_revision: null }),
    received: bound({ op: 'history', run, worker: null, before: null, after_revision: latest + 1 }),
  };
  if (run === 'run-7') navigation.previous = { op: 'history', run: 'run-6', from: null, query: '' };
  // Native delivery replies repeat the newest page's earlier row (conversation_refresh).
  if (earlier || (run === 'run-7' && after !== null)) navigation.earlier = bound({ op: 'history', run, worker: null, before: { created_at_ms: 20 * 60000, revision: 20 }, after_revision: null });
  for (const e of events) {
    if (e.kind === 'tools') continue;
    if (fx.fullReply && e.revision === 44) e.markdown = fx.fullReply;
    e.html = `<p>${esc(e.markdown)}</p>`;
  }
  return {
    run, retargeted, agent_gone: !!agent && fx.gone, live,
    header: { name: 'Fix flaky test', status: 'waiting', location: 'dev', started: '12:00', task: 'fix', project: 'yinz', branch: 'main', session_id: 'conv-1', details: `Run: ${run}` },
    events, earlier, next_from: null, previous_run: run === 'run-7' ? 'run-6' : null, origin_run: null, resumable: false,
    activity_ranges: [], navigation, status: '',
    header_html: `<div class="hdr"><strong>${esc(`Fix flaky test ${XSS(3)}`)}</strong> · ${esc(run)}</div>`,
    details_html: `<pre class="details">${esc(`Run: ${run}\nProvider: codex`)}</pre>`,
  };
}

/** 80 lines like a real pane read: older output, one wide line, then the question at the bottom. */
const SCREEN_LAST = '  2. No';
function screenOutput(tick) {
  const older = Array.from({ length: 76 }, (_, i) => (i === 0 ? `output line 0 (read ${tick})`
    : i === 40 ? `output line 40 ${'wide-'.repeat(30)}end` : `output line ${i}`));
  return [...older, '$ cargo test', '? Allow edit', '  1. Yes', SCREEN_LAST].join('\n');
}

function exactKeys(value, keys) {
  return !!value && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).length === keys.length && keys.every((k) => Object.hasOwn(value, k));
}
const validRef = (a) => exactKeys(a, ['id', 'kind', 'session_id']) && ['id', 'kind', 'session_id'].every((k) => typeof a[k] === 'string' && a[k].trim());

function agentRoute(fx, entry) {
  const { path: p, body } = entry;
  const invalid = (why) => {
    fx.problems.push(`frontend sent an invalid ${p} body (${why})`);
    return [400, { error: { code: 'invalid', message: 'Invalid request body' } }];
  };
  if (entry.method !== 'POST') return [405, { error: { code: 'method_not_allowed' } }];
  if (p === '/api/agent/prompt') {
    if (!exactKeys(body, ['agent', 'submission_id', 'text']) || !validRef(body.agent)) return invalid('shape');
    if (!UUID.test(body.submission_id) || typeof body.text !== 'string' || !body.text.trim()) return invalid('fields');
    if (fx.prompt === 'rejected') {
      return [200, { response: { outcome: 'rejected', submission_id: body.submission_id, message: `Refused ${XSS(5)}`, prompt_delivery: 'not_sent' } }];
    }
    return [200, { response: { outcome: fx.prompt, submission_id: body.submission_id, acknowledged: true, prompt_delivery: 'started' } }];
  }
  if (p === '/api/agent/keys') {
    if (!exactKeys(body, ['agent', 'keys']) || !validRef(body.agent) || !Array.isArray(body.keys) || !body.keys.length || body.keys.length > 8) return invalid('shape');
    return [200, { response: { outcome: 'ok', acknowledged: true, message: `keys delivered ${XSS(4)}` } }];
  }
  if (p === '/api/agent/focus') {
    if (!exactKeys(body, ['agent']) || !validRef(body.agent)) return invalid('shape');
    return [200, { ok: true }];
  }
  if (p === '/api/agent/screen') {
    if (!body || !validRef(body.agent) || Object.keys(body).some((k) => !['agent', 'lines'].includes(k))) return invalid('shape');
    fx.screenTick += 1;
    const output = screenOutput(fx.screenTick);
    return [200, { response: { outcome: 'ok', output }, html: `<pre class="screen">${esc(output)}</pre>` }];
  }
  return [404, { error: { code: 'not_found' } }];
}

function fixtureReply(fx, entry) {
  const q = entry.query;
  switch (entry.path) {
    case '/api/agents': return [200, agents(fx)];
    case '/api/history': return [200, history(fx, q)];
    case '/api/event':
      return [200, { header: 'Agent · 12:44', markdown: fx.fullReply || 'full', tool: '', html: `<p>${esc(fx.fullReply || 'The complete message text for revision 44.')}</p>` }];
    case '/api/tools':
      if (q.has('before')) return [200, { header: 'Tools (older)', markdown: '', tool: '', html: '<pre>older tool record</pre>', page: { events: [], navigation: {} } }];
      return [200, {
        header: 'Tools', markdown: '', tool: '', html: '<pre>tool call: cargo test</pre>',
        page: { events: [], navigation: { earlier: { op: 'tools', run: q.get('run'), range: JSON.parse(q.get('range')), before: { created_at_ms: 1, revision: 2 } } } },
      }];
    case '/api/usage':
      return [200, { usage: { provider: 'codex', status: 'ok', observed_at_ms: 1, plan: null, windows: [{ label: '5h', used_percent: 42, resets_at_ms: null }, { label: 'week', used_percent: 91, resets_at_ms: null }], message: null } }];
    case '/api/prompts':
      return [200, { prompts: [{ text: 'Run the tests', description: 'tests' }, { text: 'Commit the change', description: '' }] }];
    default:
      return agentRoute(fx, entry);
  }
}

// Live native calls made on behalf of an intercepted request (never the page's own body).
async function liveFetch(context, pathname, method, headers, data) {
  return context.request.fetch(`${ORIGIN}${pathname}`, { method, headers, data, maxRedirects: 0, timeout: 15000 });
}

async function handle(context, route, fx) {
  const req = route.request();
  const u = new URL(req.url());
  let body = null;
  try {
    body = req.postData() ? JSON.parse(req.postData()) : null;
  } catch {
    fx.problems.push(`frontend sent malformed JSON to ${u.pathname}`);
  }
  const entry = { method: req.method(), path: u.pathname, query: u.searchParams, body };
  fx.log.push(entry);
  try {
    const authorization = (await req.allHeaders()).authorization;
    const auth = authorization ? { authorization } : {};
    if (fx.revokeOnPrompt && entry.path === '/api/agent/prompt') {
      // Revoke this bearer for real (live native logout) while the prompt is "in flight".
      const out = await liveFetch(context, '/api/logout', 'POST', { ...auth, 'content-type': 'application/json', origin: ORIGIN }, '{}');
      if (out.status() !== 204) fx.problems.push(`live revocation returned ${out.status()}`);
    }
    const session = await liveFetch(context, '/api/session', 'GET', { ...auth, accept: 'application/json' });
    if (session.status() !== 200) {
      if (session.status() !== 401) fx.problems.push(`live session check returned ${session.status()}`);
      await route.fulfill({ response: session });
      return;
    }
    const fail = fx.fail.get(entry.path);
    if (fail && fail.times > 0) {
      fail.times -= 1;
      if (fail.mode === 'abort') return await route.abort('failed');
      if (fail.mode === 'hold') {
        fx.held.push(route);
        return undefined;
      }
      return await route.fulfill({
        status: fail.status, headers: { 'content-type': 'application/json', 'cache-control': 'no-store', 'x-yinz-check': 'intercepted', ...(fail.headers || {}) },
        body: JSON.stringify(fail.body),
      });
    }
    const [status, data] = fixtureReply(fx, entry);
    return await route.fulfill({
      status, headers: { 'content-type': 'application/json', 'cache-control': 'no-store', 'x-yinz-check': 'intercepted' }, body: JSON.stringify(data),
    });
  } catch (e) {
    // Routes of closed pages/contexts throw here; anything else is a check defect.
    if (!/closed|Target page|has been closed|Route is already handled/i.test(String(e && e.message))) fx.problems.push(`fixture error on ${entry.path}: ${redact(e.message)}`);
    return undefined;
  }
}

// ---------------------------------------------------------------------------------------
// Browser contexts and page guards

const INIT = `(() => {
  const report = (kind, detail) => { try { window.__yinzCheckReport(kind, String(detail)); } catch {} };
  document.addEventListener('securitypolicyviolation', (e) => report('csp-violation', e.violatedDirective + ' ' + e.blockedURI), true);
  window.addEventListener('error', (e) => {
    const t = e.target;
    if (t && t !== window && t.tagName) report('resource-error', t.tagName + ' ' + (t.src || t.href || ''));
  }, true);
  window.addEventListener('unhandledrejection', (e) => report('unhandledrejection', e.reason && e.reason.message || e.reason));
  const original = window.fetch.bind(window);
  window.__yinzFetches = [];
  window.__yinzOpen = 0;
  window.fetch = function (input, init) {
    let path = '';
    try { path = new URL(typeof input === 'string' ? input : input.url, location.href).pathname; } catch {}
    window.__yinzFetches.push({ m: (init && init.method) || 'GET', p: path });
    window.__yinzOpen += 1;
    let open = true;
    const close = () => { if (open) { open = false; window.__yinzOpen -= 1; } };
    return original(input, init).then((res) => {
      const text = res.text.bind(res);
      res.text = () => text().finally(close);
      if (res.status === 204) close();
      return res;
    }, (e) => { close(); throw e; });
  };
})();`;

class Guard {
  constructor() {
    this.problems = [];
  }
  add(text) {
    this.problems.push(redact(text).slice(0, 400));
  }
}

let browser;
const contexts = new Set();

function attachPage(page, guard) {
  page.on('pageerror', (e) => guard.add(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') guard.add(`console error: ${m.text()}`);
  });
  page.on('response', (r) => {
    const u = new URL(r.url());
    if (u.origin !== ORIGIN || intercepted(u.pathname)) return;
    if ((u.pathname.startsWith('/static/') || u.pathname === '/' || u.pathname === '/login') && r.status() >= 400) {
      guard.add(`live ${u.pathname} returned ${r.status()}`);
    }
  });
  page.on('requestfailed', (r) => {
    const u = new URL(r.url());
    const failure = (r.failure() && r.failure().errorText) || '';
    if (u.origin === ORIGIN && u.pathname.startsWith('/static/') && !/NS_BINDING_ABORTED/.test(failure)) guard.add(`module/asset load failed: ${u.pathname} ${failure}`);
  });
}

async function openContext(options = {}) {
  const context = await browser.newContext({
    viewport: options.viewport || { width: 390, height: 844 },
    colorScheme: options.colorScheme || 'light',
    reducedMotion: 'reduce',
    serviceWorkers: 'block',
    locale: 'en-US',
    timezoneId: 'UTC',
    storageState: options.storageState,
  });
  contexts.add(context);
  const fx = new Fixture();
  const guard = new Guard();
  await context.exposeBinding('__yinzCheckReport', (_source, kind, detail) => guard.add(`${kind}: ${detail}`));
  await context.addInitScript(INIT);
  await context.route((u) => u.origin === ORIGIN && intercepted(u.pathname), (route) => handle(context, route, fx));
  context.on('request', (req) => {
    const url = req.url();
    if (url.startsWith('data:') || url.startsWith('blob:') || url === 'about:blank') return;
    let origin = '';
    try {
      origin = new URL(url).origin;
    } catch {
      // unparsable: report below
    }
    if (origin !== ORIGIN) guard.add(`request to a non-same-origin URL: ${url.slice(0, 200)}`);
  });
  context.on('page', (page) => attachPage(page, guard));
  if (options.clock) await context.clock.install();
  const page = await context.newPage();
  return { context, page, fx, guard };
}

// ---------------------------------------------------------------------------------------
// Helpers (bounded waits only)

const pause = (ms) => new Promise((r) => setTimeout(r, ms));

async function textUntil(page, sel, test, what, timeout = 10000) {
  let last = null;
  try {
    return await until(what, async () => {
      last = await textOf(page, sel);
      return last !== null && (typeof test === 'string' ? last.includes(test) : test.test(last));
    }, timeout);
  } catch (e) {
    throw new Error(`${e.message}; ${sel} text was ${last === null ? 'absent' : JSON.stringify(redact(last).slice(0, 160))}`);
  }
}

async function until(what, fn, timeout = 10000) {
  const end = Date.now() + timeout;
  let last = null;
  for (;;) {
    try {
      const v = await fn();
      if (v) return v;
    } catch (e) {
      last = e;
    }
    if (Date.now() > end) throw new Error(`timed out after ${timeout} ms waiting for ${what}${last ? ` (${redact(last.message).slice(0, 200)})` : ''}`);
    await pause(40); // poll interval of a bounded wait
  }
}

function check(condition, message) {
  if (!condition) throw new Error(message);
}

const pathOf = (page) => new URL(page.url()).pathname;
const hashOf = (page) => new URL(page.url()).hash;
const exists = (page, sel) => page.evaluate((s) => !!document.querySelector(s), sel);
const textOf = (page, sel) => page.evaluate((s) => { const e = document.querySelector(s); return e ? e.textContent : null; }, sel);
const valueOf = (page, sel) => page.evaluate((s) => { const e = document.querySelector(s); return e ? e.value : null; }, sel);
const activeId = (page) => page.evaluate(() => (document.activeElement ? document.activeElement.id : ''));
const domClick = (page, sel) => page.evaluate((s) => { const e = document.querySelector(s); if (!e) throw new Error(`missing ${s}`); e.click(); return true; }, sel);
const fetchCount = (page, prefix, method) => page.evaluate(([p, m]) => window.__yinzFetches.filter((f) => f.p.startsWith(p) && (!m || f.m === m)).length, [prefix, method || null]);
const idle = (page) => until('in-page requests to settle', () => page.evaluate(() => window.__yinzOpen === 0));

async function tokenStores(page) {
  return page.evaluate(() => ({
    local: /^yzw1_/.test(localStorage.getItem('yinz.web.v1.token') || ''),
    session: /^yzw1_/.test(sessionStorage.getItem('yinz.web.v1.token') || ''),
  }));
}

async function waitLogin(page) {
  await until('the live /login page', () => pathOf(page) === '/login');
  await until('login.js to start', () => page.evaluate(() => document.title.startsWith('Sign in')));
}

/** Non-keyboard sign-in helper (keyboard-only sign-in is its own scenario). */
async function signIn(page, { remember = false } = {}) {
  await waitLogin(page);
  await page.focus('#password');
  await page.keyboard.insertText(PASSWORD);
  await page.evaluate((r) => { document.getElementById('remember').checked = r; }, remember);
  await page.click('#login-submit');
  await until('return to the app after sign-in', () => pathOf(page) === '/');
}

async function openApp(page, hash, { remember = false } = {}) {
  await page.goto(`${ORIGIN}/${hash}`);
  if (pathOf(page) === '/login' || !(await tokenStores(page)).session) {
    await until('a page settled at / or /login', () => ['/', '/login'].includes(pathOf(page)));
    if (pathOf(page) === '/login') await signIn(page, { remember });
  }
}

async function openDetail(page, hash = detailHash()) {
  await openApp(page, hash);
  await until('timeline events', () => exists(page, '#timeline .event'));
}

async function openList(page) {
  await openApp(page, '#/');
  await until('agent cards', () => exists(page, 'li.agent-card'));
}

async function noInjection(page) {
  const r = await page.evaluate(() => ({ xss: window.__yinzXss, imgs: document.querySelectorAll('img[src="x"]').length }));
  check(r.xss === undefined && r.imgs === 0, `injected markup became DOM (xss=${r.xss}, img=${r.imgs})`);
}

async function noHorizontalOverflow(page, where) {
  const r = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, cw: document.documentElement.clientWidth, bw: document.body.scrollWidth }));
  check(r.sw <= r.cw && r.bw <= r.cw, `${where}: horizontal overflow (scrollWidth ${r.sw}/${r.bw} > clientWidth ${r.cw})`);
}

async function touchTargets(page, where) {
  const bad = await page.evaluate(() => {
    const out = [];
    const name = (el) => el.id ? `#${el.id}` : `${el.tagName.toLowerCase()}.${[...el.classList].join('.')}${el.dataset.key ? `[${el.dataset.key}]` : ''}`;
    for (const el of document.querySelectorAll('button, a.button, [role=tab], input[type=checkbox]')) {
      const cs = getComputedStyle(el);
      let r = el.getBoundingClientRect();
      if (cs.display === 'none' || cs.visibility === 'hidden' || r.width <= 1 || r.height <= 1) continue;
      if (el.matches('input[type=checkbox]')) {
        const label = el.closest('label') || (el.id && document.querySelector(`label[for="${el.id}"]`));
        if (label) r = label.getBoundingClientRect();
      }
      if (r.height < 44 || r.width < 44) out.push(`${name(el)} ${Math.round(r.width)}x${Math.round(r.height)}`);
    }
    for (const card of document.querySelectorAll('li.agent-card')) {
      const h = card.getBoundingClientRect().height;
      if (h < 56) out.push(`agent card ${card.dataset.agentId} height ${Math.round(h)}`);
    }
    return out;
  });
  check(!bad.length, `${where}: touch targets too small: ${bad.join(', ')}`);
}

/**
 * Controls clipped horizontally by a scrolling or clipping ancestor, or pushed past the
 * viewport. Vertical scroll regions clip legitimately, so only the horizontal extent is
 * checked. A composer dock wider than its box would scroll its controls sideways.
 */
async function noClippedControls(page, where) {
  const bad = await page.evaluate(() => {
    const out = [];
    const name = (el) => el.id ? `#${el.id}` : `${el.tagName.toLowerCase()}.${[...el.classList].join('.')}${el.dataset.key ? `[${el.dataset.key}]` : ''}`;
    for (const el of document.querySelectorAll('button, a.button, [role=tab], input[type=checkbox], textarea')) {
      const cs = getComputedStyle(el);
      const r = el.getBoundingClientRect();
      if (cs.display === 'none' || cs.visibility === 'hidden' || r.width <= 1 || r.height <= 1) continue;
      if (r.left < -1 || r.right > innerWidth + 1) out.push(`${name(el)} ${Math.round(r.left)}..${Math.round(r.right)} outside the ${innerWidth}px viewport`);
      for (let a = el.parentElement; a && a !== document.documentElement; a = a.parentElement) {
        const s = getComputedStyle(a);
        if (s.overflowX === 'visible' && s.overflowY === 'visible') continue;
        const box = a.getBoundingClientRect();
        const left = box.left + a.clientLeft;
        const right = left + a.clientWidth;
        if (r.left < left - 1 || r.right > right + 1) {
          out.push(`${name(el)} ${Math.round(r.left)}..${Math.round(r.right)} clipped by ${name(a)} ${Math.round(left)}..${Math.round(right)}`);
          break;
        }
      }
    }
    for (const dock of document.querySelectorAll('.composer-dock')) {
      if (dock.scrollWidth > dock.clientWidth) out.push(`composer dock scrollWidth ${dock.scrollWidth} > clientWidth ${dock.clientWidth}`);
    }
    return out;
  });
  check(!bad.length, `${where}: clipped controls: ${bad.join(', ')}`);
}

/** Wait until a sheet exists and holds focus, so its Escape/Tab handler is attached. */
async function sheetReady(page, id, what) {
  await until(what, () => page.evaluate((s) => { const n = document.getElementById(s); return !!n && n.contains(document.activeElement); }, id));
}

async function tabUntil(page, id, { max = 40, shift = false } = {}) {
  for (let i = 0; i < max; i += 1) {
    if ((await activeId(page)) === id) return;
    await page.keyboard.press(shift ? 'Shift+Tab' : 'Tab');
  }
  check((await activeId(page)) === id, `keyboard focus never reached #${id}`);
}

async function focusInside(page, sheet) {
  return page.evaluate((s) => { const n = document.getElementById(s); return !!n && n.contains(document.activeElement); }, sheet);
}

async function assertTrap(page, sheet, presses = 7) {
  for (const key of ['Tab', 'Shift+Tab']) {
    for (let i = 0; i < presses; i += 1) {
      await page.keyboard.press(key);
      check(await focusInside(page, sheet), `${key} moved focus out of #${sheet}`);
    }
  }
}

async function scrollState(page) {
  return page.evaluate(() => {
    const el = document.getElementById('timeline');
    return { atBottom: el.scrollHeight - el.scrollTop - el.clientHeight < 32, top: el.scrollTop, overflow: el.scrollHeight > el.clientHeight + 100 };
  });
}

/** Stop natural time a little ahead of the page clock (pauseAt cannot move backwards). */
async function pauseClock(context, page) {
  await context.clock.pauseAt(await page.evaluate(() => Date.now() + 1000));
}

async function nextFrame(page) {
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true)))));
}

async function revokeLive(page) {
  // Real native revocation of this tab's bearer; the token never leaves the page.
  const status = await page.evaluate(async () => {
    const t = sessionStorage.getItem('yinz.web.v1.token') || localStorage.getItem('yinz.web.v1.token');
    const r = await fetch('/api/logout', { method: 'POST', headers: { Authorization: `Bearer ${t}`, 'Content-Type': 'application/json' }, body: '{}' });
    return r.status;
  });
  check(status === 204, `live logout returned ${status}`);
}

/** Screen output region, its newest line, the quick keys and the composer dock. */
function screenGeometry(page) {
  return page.evaluate((last) => {
    const region = document.getElementById('screen-scroll');
    const pre = document.querySelector('#screen pre.screen');
    const keys = document.querySelector('.screen-keys');
    const dock = document.querySelector('.composer-dock');
    if (!region || !pre || !keys) return null;
    const text = pre.firstChild;
    const range = document.createRange();
    const at = text.data.lastIndexOf(last);
    range.setStart(text, at);
    range.setEnd(text, at + last.length);
    const box = (r) => ({ top: r.top, bottom: r.bottom });
    return {
      scrollTop: region.scrollTop,
      atBottom: region.scrollTop + region.clientHeight >= region.scrollHeight - 2,
      overflow: region.scrollHeight > region.clientHeight + 100,
      region: box(region.getBoundingClientRect()),
      last: box(range.getBoundingClientRect()),
      keys: box(keys.getBoundingClientRect()),
      dockTop: dock ? dock.getBoundingClientRect().top : innerHeight,
      height: innerHeight,
    };
  }, SCREEN_LAST);
}

async function assertScreenPinned(page, where) {
  const g = await until(`${where}: screen pinned to its newest line`, async () => {
    const v = await screenGeometry(page);
    return v && v.atBottom ? v : null;
  });
  check(g.overflow, `${where}: screen fixture does not overflow its region`);
  check(g.last.top >= g.region.top - 1 && g.last.bottom <= g.region.bottom + 1,
    `${where}: the question is outside the output region (${Math.round(g.last.top)}..${Math.round(g.last.bottom)} vs ${Math.round(g.region.top)}..${Math.round(g.region.bottom)})`);
  check(g.keys.top >= 0 && g.keys.bottom <= g.height && g.keys.bottom <= g.dockTop + 1,
    `${where}: quick keys are not visible above the composer (${Math.round(g.keys.top)}..${Math.round(g.keys.bottom)}, dock ${Math.round(g.dockTop)})`);
}

async function waitPolls(page, fx, n = 2) {
  const start = fx.count('/api/history');
  await until(`${n} more tail polls`, () => fx.count('/api/history') >= start + n, 15000);
}

// ---------------------------------------------------------------------------------------
// Scenarios

const results = [];
const RESULTS_DIR = path.join(REPO, 'target', 'web-browser-results');

const FILTER = process.argv[3] || '';

async function scenario(name, fn) {
  if (FILTER && !name.includes(FILTER)) return;
  const opened = [];
  const open = async (options) => {
    const c = await openContext(options);
    opened.push(c);
    return c;
  };
  let error = null;
  try {
    await fn(open);
  } catch (e) {
    error = redact(e && e.message ? e.message : e);
  }
  const problems = opened.flatMap((c) => [...c.guard.problems, ...c.fx.problems.map(redact)]);
  if (!error && problems.length) error = problems.join('; ');
  else if (error && problems.length) error += ` [also: ${problems.join('; ')}]`;
  if (error && fs.existsSync(path.join(REPO, 'target'))) {
    const c = opened[opened.length - 1];
    try {
      fs.mkdirSync(RESULTS_DIR, { recursive: true });
      const p = c && c.context.pages().at(-1);
      if (p) {
        await p.screenshot({
          path: path.join(RESULTS_DIR, `${name.replace(/\W+/g, '_')}.png`), timeout: 5000,
          mask: ['#password', '#composer-input', '.submission-text', '.prompt-text'].map((s) => p.locator(s)),
        });
      }
    } catch {
      // screenshots are optional
    }
  }
  for (const c of opened) {
    c.fx.releaseHeld();
    await c.context.close().catch(() => {});
    contexts.delete(c.context);
  }
  results.push({ name, ok: !error, error });
  console.log(`${error ? 'FAIL' : 'PASS'} ${name}${error ? `: ${error}` : ''}`);
}

async function keyboardLogin(open) {
  const { context, page } = await open();
  await page.goto(`${ORIGIN}/${detailHash()}`);
  await waitLogin(page);
  await noHorizontalOverflow(page, 'login 390px');
  await tabUntil(page, 'password', { max: 4 });
  await page.keyboard.type(PASSWORD);
  await page.keyboard.press('Tab');
  check((await activeId(page)) === 'label', 'Tab from password did not reach the device label');
  await page.keyboard.type('Browser check');
  await page.keyboard.press('Tab');
  check((await activeId(page)) === 'remember', 'Tab from label did not reach Remember');
  await page.keyboard.press('Tab');
  check((await activeId(page)) === 'login-submit', 'Tab from Remember did not reach Sign in');
  await page.keyboard.press('Enter');
  await until('the saved detail route after sign-in', () => pathOf(page) === '/' && hashOf(page) === detailHash());
  await until('timeline events', () => exists(page, '#timeline .event'));
  check(!page.url().includes(PASSWORD), 'password appeared in a URL');
  const stores = await tokenStores(page);
  check(stores.session && !stores.local, `session-only sign-in stored token in ${JSON.stringify(stores)}`);
  // Repeated entry: a reload with the stored token goes straight to the app.
  const visited = [];
  page.on('framenavigated', (f) => { if (f === page.mainFrame()) visited.push(new URL(f.url()).pathname); });
  await page.goto(`${ORIGIN}/#/`);
  await page.reload();
  await until('agent list after reload', () => exists(page, 'li.agent-card'));
  check(!visited.includes('/login'), 'reload with a stored token visited /login');
  // A new tab does not share a session-only token.
  const tab = await context.newPage();
  await tab.goto(`${ORIGIN}/`);
  await waitLogin(tab);
}

async function rememberedLogin(open) {
  const { context, page } = await open();
  await page.goto(`${ORIGIN}/`);
  await waitLogin(page);
  await page.focus('#password');
  await page.keyboard.insertText('not the password');
  await page.click('#login-submit');
  await until('wrong-password status', async () => (await textOf(page, '#login-status')).includes('Wrong password'));
  check(await page.evaluate(() => document.getElementById('login-status').classList.contains('status-error')), 'wrong password not marked as an error');
  check((await valueOf(page, '#password')) === '', 'password field not cleared after a failed attempt');
  await page.focus('#remember');
  await page.keyboard.press('Space');
  check(await page.evaluate(() => document.getElementById('remember').checked), 'Space did not check Remember');
  await page.focus('#password');
  await page.keyboard.insertText(PASSWORD);
  await page.keyboard.press('Enter');
  await until('agent list after remembered sign-in', async () => pathOf(page) === '/' && exists(page, 'li.agent-card'));
  const stores = await tokenStores(page);
  check(stores.local && !stores.session, `remembered sign-in stored token in ${JSON.stringify(stores)}`);
  for (const target of [await context.newPage(), (await open({ storageState: await context.storageState() })).page]) {
    const visited = [];
    target.on('framenavigated', (f) => { if (f === target.mainFrame()) visited.push(new URL(f.url()).pathname); });
    await target.goto(`${ORIGIN}/`);
    await until('agent list for a remembered device', () => exists(target, 'li.agent-card'));
    check(!visited.includes('/login'), 'remembered device was sent to /login');
  }
}

async function phoneLayout(open, width) {
  const { page, fx } = await open({ viewport: { width, height: width === 320 ? 640 : 844 } });
  await page.goto(`${ORIGIN}/`);
  await waitLogin(page);
  await noHorizontalOverflow(page, `login ${width}px`);
  await touchTargets(page, `login ${width}px`);
  await noClippedControls(page, `login ${width}px`);
  await signIn(page);
  await until('agent cards', () => exists(page, 'li.agent-card'));
  await until('three sections', async () => (await exists(page, '#section-waiting')) && (await exists(page, '#section-busy')) && (await exists(page, '#section-idle')));
  await noHorizontalOverflow(page, `list ${width}px`);
  await touchTargets(page, `list ${width}px`);
  await noClippedControls(page, `list ${width}px`);
  check((await textOf(page, 'li.agent-card[data-agent-id="live:peer:9"]')).includes('session peer'), 'peer session chip missing');
  // Agent titles arrive as server-escaped row html; the title also reaches toasts as text.
  check((await textOf(page, '#section-waiting')).includes('<img src=x'), 'escaped title text not shown');
  await domClick(page, 'li.agent-card[data-agent-id="live:dev:7"] .quick-key[data-key="1"]');
  await until('quick key request', () => fx.count('/api/agent/keys') === 1);
  const keys = fx.bodies('/api/agent/keys')[0];
  check(JSON.stringify(keys) === JSON.stringify({ agent: REF7, keys: ['1'] }), 'quick key body mismatch');
  await until('quick key toast', async () => (await textOf(page, '#toast')).includes('keys delivered <img'));
  await noInjection(page);
  await domClick(page, '#menu-button');
  // The sheet takes focus in the same commit that creates it, so it can already hold
  // focus the first time it is seen.
  const opened = await until('menu sheet', () => page.evaluate(() => {
    const n = document.getElementById('sheet-menu');
    return n ? { inside: n.contains(document.activeElement) } : null;
  }));
  check(opened.inside, 'focus was outside the menu sheet when it appeared');
  await noHorizontalOverflow(page, `menu sheet ${width}px`);
  await touchTargets(page, `menu sheet ${width}px`);
  await noClippedControls(page, `menu sheet ${width}px`);
  await sheetReady(page, 'sheet-menu', 'focus inside the menu sheet');
  await page.keyboard.press('Escape');
  await until('menu closed', async () => !(await exists(page, '#sheet-menu')));
  await page.evaluate((h) => { location.hash = h; }, detailHash());
  await until('timeline events', () => exists(page, '#timeline .event'));
  await until('composer', () => exists(page, '#composer-input'));
  for (const tab of ['timeline', 'screen', 'details']) {
    await domClick(page, `#tab-${tab}`);
    await until(`${tab} panel`, async () => (await page.evaluate((t) => document.getElementById(`tab-${t}`).getAttribute('aria-selected'), tab)) === 'true' && exists(page, `#panel-${tab}`));
    if (tab === 'screen') {
      await until('screen content', () => exists(page, '#screen pre.screen'));
      await assertScreenPinned(page, `screen ${width}px`);
      // A remount starts at the newest line again.
      await domClick(page, '#tab-timeline');
      await until('timeline panel', () => exists(page, '#panel-timeline'));
      await domClick(page, '#tab-screen');
      await until('screen content again', () => exists(page, '#screen pre.screen'));
      await assertScreenPinned(page, `screen ${width}px after a tab switch`);
      // Scrolled up, new reads leave the position alone.
      await page.evaluate(() => { document.getElementById('screen-scroll').scrollTop = 0; });
      await nextFrame(page);
      const reads = fx.count('/api/agent/screen');
      await until('another screen read', () => fx.count('/api/agent/screen') > reads, 10000);
      await nextFrame(page);
      const g = await screenGeometry(page);
      check(g.scrollTop < 2, `screen ${width}px: a read moved a scrolled-up screen to ${Math.round(g.scrollTop)}`);
    }
    if (tab === 'details') await until('details content', () => exists(page, '#details'));
    await noHorizontalOverflow(page, `detail ${tab} ${width}px`);
    await touchTargets(page, `detail ${tab} ${width}px`);
    await noClippedControls(page, `detail ${tab} ${width}px`);
  }
  check((await textOf(page, '#detail-header-html')).includes('<img src=x'), 'escaped header text not shown');
  await noInjection(page);
}

async function compactAgentList(open, width, scheme) {
  const { page, fx } = await open({ viewport: { width, height: 844 }, colorScheme: scheme });
  const longReply = FILLER.repeat(6).slice(0, 512);
  fx.fullReply = `${longReply} The reader retains the rest of this response. ${FILLER.repeat(3)}`;
  fx.rows = [
    row(fx, AGENT_ID, 'waiting', { title: 'Review the change', last_reply: { text: 'Ready for your review.', truncated: false } }),
    row(fx, 'live:dev:8', 'busy', { title: 'Refactor parser', last_reply: { text: longReply, truncated: false } }),
    row(fx, 'live:peer:9', 'idle', { title: 'Peer agent', wrapper: true, focused: true, current_session: false,
      session: 'peer', host: 'box', badges: [{ text: '<custom context>', tone: 'info' }], last_reply: { text: 'Upstream preview', truncated: true } }),
  ];
  await openList(page);
  // COOP navigation can drop Firefox media emulation; verify the actual content viewport
  // and theme after login, before measuring any typography or colours.
  await page.emulateMedia({ colorScheme: scheme });
  check(await page.evaluate(({ width, scheme }) => innerWidth === width && matchMedia('(prefers-color-scheme: dark)').matches === (scheme === 'dark'), { width, scheme }), 'wrong viewport or colour scheme');
  const reference = '#section-busy .agent-card';
  const metrics = await page.evaluate((sel) => {
    const card = document.querySelector(sel);
    const title = card.querySelector('.agent-title');
    const reply = card.querySelector('.agent-reply');
    const ts = getComputedStyle(title), rs = getComputedStyle(reply), cs = getComputedStyle(card);
    const luminance = (colour) => {
      const rgb = colour.match(/[\d.]+/g).slice(0, 3).map((v) => Number(v) / 255).map((v) => v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
      return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
    };
    const contrast = (a, b) => (Math.max(luminance(a), luminance(b)) + 0.05) / (Math.min(luminance(a), luminance(b)) + 0.05);
    return {
      height: card.getBoundingClientRect().height, replyHeight: reply.getBoundingClientRect().height,
      lineHeight: parseFloat(rs.lineHeight), titleSize: ts.fontSize, replySize: rs.fontSize,
      titleColour: ts.color, replyColour: rs.color, titleContrast: contrast(ts.color, cs.backgroundColor),
      replyContrast: contrast(rs.color, cs.backgroundColor), text: reply.textContent,
      clamp: rs.webkitLineClamp,
    };
  }, reference);
  check(metrics.titleColour !== metrics.replyColour, 'task and response colours are identical');
  check(metrics.titleContrast >= 4.5 && metrics.replyContrast >= 4.5, `low text contrast: ${JSON.stringify(metrics)}`);
  check(metrics.titleSize === '16px' && metrics.replySize === '14px', 'unexpected title/reply typography');
  check(metrics.clamp === '3' && metrics.replyHeight <= metrics.lineHeight * 3 + 1, `reply exceeds three lines: ${JSON.stringify(metrics)}`);
  check(metrics.text === `${longReply.slice(0, 160)}…`, 'long preview lost its character cap or ellipsis cue');
  check(metrics.height < (width === 320 ? 240 : 220), `reference card too tall: ${metrics.height}px at ${width}px`);
  check(await textOf(page, '#section-waiting .agent-reply') === 'Ready for your review.', 'short reply changed');
  check(await textOf(page, '#section-idle .agent-reply') === 'Upstream preview…', 'upstream truncation cue missing');
  check(!(await exists(page, '.list-main .state-badge')), 'duplicate known-state badge');
  check(!(await exists(page, '#section-busy .badges')), 'empty badge row retained');
  check(await page.evaluate(() => [...document.querySelectorAll('.list-main .agent-status')].every((el) => getComputedStyle(el).display === 'none')), 'duplicate heading state visible through a wrapper');
  for (const sel of ['.custom-row .badge', '.badge-focused', '.badge-peer', '.badge-host', '.agent-glyph']) check(await exists(page, sel), `missing context ${sel}`);
  check((await textOf(page, '.custom-row .badges')).includes('<custom context>'), 'custom badge was not preserved as text');
  check(await page.evaluate(() => getComputedStyle(document.querySelector('.card-needs-person')).borderLeftColor !== getComputedStyle(document.querySelector('.card-busy')).borderLeftColor), 'needs-person emphasis lost');
  await noHorizontalOverflow(page, `compact list ${width}px ${scheme}`);
  await noClippedControls(page, `compact list ${width}px ${scheme}`);
  await touchTargets(page, `compact list ${width}px ${scheme}`);
  fs.mkdirSync(RESULTS_DIR, { recursive: true });
  await page.screenshot({ path: path.join(RESULTS_DIR, `compact-list-${width}-${scheme}.png`), fullPage: true });
  console.log(`compact list ${width}px ${scheme}: card ${metrics.height}px; reply ${metrics.replyHeight}px; title/reply contrast ${metrics.titleContrast.toFixed(2)}/${metrics.replyContrast.toFixed(2)}`);

  // Keyboard focus must remain visible and Enter must activate the card itself.
  await page.focus('#list-refresh');
  await page.keyboard.press('Tab');
  check(await page.evaluate(() => document.activeElement.matches('.agent-open-area') && parseFloat(getComputedStyle(document.activeElement).outlineWidth) >= 2), 'card keyboard focus indication missing');
  await page.keyboard.press('Enter');
  await until('card opens waiting screen', async () => hashOf(page) === detailHash('run-7', 'screen') && exists(page, '#screen pre.screen'));
  check((await textOf(page, '#detail-header .state-badge')).includes('Needs you'), 'detail state badge missing');
  await domClick(page, '#tab-timeline');
  await until('full response in timeline', async () => (await textOf(page, '#timeline')).includes(fx.fullReply));
  check(await page.evaluate(() => [...document.querySelectorAll('#timeline .event-body')].every((el) => getComputedStyle(el).webkitLineClamp !== '3')), 'list clamp leaked to timeline');
  await domClick(page, '.event-read');
  await until('full response in reader', async () => (await textOf(page, '#sheet-reader')).includes(fx.fullReply));
  check(await page.evaluate(() => [...document.querySelectorAll('#sheet-reader p')].some((el) => getComputedStyle(el).webkitLineClamp !== '3' && el.getBoundingClientRect().height > parseFloat(getComputedStyle(el).lineHeight) * 3)), 'reader response was visually capped');
  await sheetReady(page, 'sheet-reader', 'focus inside full response reader');
  await page.keyboard.press('Escape');
  await until('reader closes', async () => !(await exists(page, '#sheet-reader')));
  await domClick(page, '#detail-back');
  await until('return to list', () => exists(page, '#section-waiting'));

  // Edge cases retain useful text, including long unbroken metadata, without empty rows.
  fx.rows = [
    row(fx, AGENT_ID, 'waiting', { title: 'Long task 界🙂 '.repeat(18), project: 'project'.repeat(30), branch: 'branch'.repeat(30), last_reply: null }),
    row(fx, 'live:dev:10', 'busy', { title: '界🙂'.repeat(100), provider: '', project: '', branch: '', last_reply: { text: '界🙂'.repeat(256), truncated: false } }),
    row(fx, 'live:dev:11', 'idle', { last_reply: { text: '', truncated: false } }),
    row(fx, 'live:dev:12', 'idle', { last_reply: undefined }),
    row(fx, 'live:dev:13', 'idle', { last_reply: { text: ' \n\t', truncated: true } }),
    row(fx, 'live:dev:14', 'paused', { title: 'Unknown state', status: 'paused', last_reply: null }),
  ];
  await domClick(page, '#list-refresh');
  await until('edge-case rows', () => exists(page, '#section-other'));
  const edge = await page.evaluate(() => {
    const card = document.querySelector('.agent-card[data-agent-id="live:dev:10"]');
    const title = card.querySelector('.agent-title');
    const meta = card.querySelector('.agent-meta');
    return {
      title: title.textContent, titleWrapped: title.getBoundingClientRect().height > parseFloat(getComputedStyle(title).lineHeight) * 2,
      titleUnclipped: title.scrollHeight <= title.clientHeight + 1,
      metaHidden: getComputedStyle(meta).display === 'none', reply: card.querySelector('.agent-reply').textContent,
      replies: document.querySelectorAll('.list-main .agent-reply').length,
      unknownVisible: getComputedStyle(document.querySelector('#section-other .agent-status')).display !== 'none',
      longMeta: document.querySelector('#section-waiting .agent-meta').textContent,
    };
  });
  check(edge.title === '界🙂'.repeat(100) && edge.titleWrapped && edge.titleUnclipped, 'long Unicode title lost or clipped');
  check(edge.metaHidden && edge.replies === 1, 'empty metadata/reply occupied a row');
  check(edge.reply === `${'界🙂'.repeat(80)}…`, 'Unicode reply preview damaged');
  check(edge.longMeta.includes('project'.repeat(30)) && edge.longMeta.includes('branch'.repeat(30)), 'long metadata lost');
  check(edge.unknownVisible && (await textOf(page, '#section-other .state-badge')).includes('paused'), 'unknown state label missing');
  await noHorizontalOverflow(page, `long content ${width}px ${scheme}`);
  await noClippedControls(page, `long content ${width}px ${scheme}`);
  await touchTargets(page, `long content ${width}px ${scheme}`);
  await noInjection(page);

  // A successful zero-agent response is distinct from network errors and empty replies.
  fx.rows = [];
  await domClick(page, '#list-refresh');
  await until('successful empty list', () => exists(page, '#list-empty'));
  check(await textOf(page, '#list-empty') === 'No agents are running.', 'empty-list explanation missing');
  check(!(await exists(page, '.agent-section')) && !(await exists(page, '#banner-offline')), 'empty list retained cards or an error');
  fx.rows = null;
  await domClick(page, '#list-refresh');
  await until('populated list recovers', () => exists(page, '#section-waiting'));
}

async function darkMode(open) {
  const bg = {};
  // Playwright Firefox drops the context colorScheme when these COOP pages swap browsing
  // context groups, so the media emulation is reapplied (and verified) after each navigation.
  const emulate = async (page, scheme) => {
    await page.emulateMedia({ colorScheme: scheme });
    const dark = await page.evaluate(() => matchMedia('(prefers-color-scheme: dark)').matches);
    check(dark === (scheme === 'dark'), `could not emulate prefers-color-scheme: ${scheme}`);
  };
  for (const scheme of ['light', 'dark']) {
    const { page } = await open({ colorScheme: scheme });
    await page.goto(`${ORIGIN}/login`);
    await waitLogin(page);
    await emulate(page, scheme);
    const login = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    await signIn(page);
    await until('agent cards', () => exists(page, 'li.agent-card'));
    await emulate(page, scheme);
    const card = await page.evaluate(() => getComputedStyle(document.querySelector('li.agent-card')).backgroundColor);
    bg[scheme] = { login, card };
  }
  check(bg.light.login !== bg.dark.login, `dark mode did not change the login background (${bg.light.login})`);
  check(bg.light.card !== bg.dark.card, `dark mode did not change the card background (${bg.light.card})`);
}

async function safeAreaManifest(open) {
  const { page } = await open();
  await page.goto(`${ORIGIN}/login`);
  await waitLogin(page);
  const r = await page.evaluate(async () => {
    const css = await (await fetch('/static/app.css')).text();
    const link = document.querySelector('link[rel="manifest"]');
    const manifest = link ? await (await fetch(link.getAttribute('href'))).json() : null;
    return {
      css: ['top', 'bottom', 'left', 'right'].every((s) => css.includes(`env(safe-area-inset-${s}`)),
      parsed: [...document.styleSheets].some((s) => s.href && s.href.endsWith('/static/app.css') && s.cssRules.length > 20),
      href: link && link.getAttribute('href'),
      viewport: (document.querySelector('meta[name="viewport"]') || {}).content || '',
      manifest: manifest && manifest.display === 'standalone' && Array.isArray(manifest.icons) && manifest.icons.length > 0,
    };
  });
  check(r.css, 'app.css lacks env(safe-area-inset-*) for every edge');
  check(r.parsed, 'app.css did not load as a stylesheet');
  check(r.href === '/static/manifest.webmanifest', `manifest link ${r.href}`);
  check(r.viewport.includes('viewport-fit=cover'), 'viewport meta lacks viewport-fit=cover');
  check(r.manifest, 'manifest is not a standalone app manifest with icons');
}

async function sheetsKeyboard(open) {
  const { page, fx } = await open();
  await openList(page);
  await page.evaluate(() => document.activeElement && document.activeElement.blur());
  await tabUntil(page, 'menu-button', { max: 6 });
  await page.keyboard.press('Enter');
  await until('menu sheet with focus inside', async () => (await exists(page, '#sheet-menu')) && focusInside(page, 'sheet-menu'));
  check(await page.evaluate(() => document.body.classList.contains('scroll-locked')), 'background scroll not locked');
  await assertTrap(page, 'sheet-menu');
  await page.keyboard.press('Escape');
  await until('menu closed', async () => !(await exists(page, '#sheet-menu')));
  check((await activeId(page)) === 'menu-button', 'focus not restored to the menu button');
  check(!(await page.evaluate(() => document.body.classList.contains('scroll-locked'))), 'scroll lock not released');

  await page.evaluate((h) => { location.hash = h; }, detailHash());
  await until('overflow button', () => exists(page, '#overflow-button'));
  await page.evaluate(() => document.activeElement && document.activeElement.blur());
  await tabUntil(page, 'overflow-button', { max: 10 });
  await page.keyboard.press('Enter');
  await until('actions sheet', async () => (await exists(page, '#sheet-overflow')) && focusInside(page, 'sheet-overflow'));
  await assertTrap(page, 'sheet-overflow');
  await tabUntil(page, 'action-keys', { max: 6 });
  await page.keyboard.press('Enter');
  await until('keys sheet', async () => (await exists(page, '#sheet-keys')) && focusInside(page, 'sheet-keys'));
  for (const key of ['1', 'enter']) {
    for (let i = 0; i < 30; i += 1) {
      if (await page.evaluate((k) => document.activeElement && document.activeElement.dataset.key === k, key)) break;
      await page.keyboard.press('Tab');
    }
    await page.keyboard.press('Enter');
  }
  await until('composed key sequence', async () => (await textOf(page, '#keys-sequence')) === '1 Enter');
  await tabUntil(page, 'keys-send', { max: 30 });
  await page.keyboard.press('Enter');
  await until('keys result', async () => ((await textOf(page, '#keys-result')) || '').includes('keys delivered <img'));
  check(JSON.stringify(fx.bodies('/api/agent/keys').at(-1)) === JSON.stringify({ agent: REF7, keys: ['1', 'enter'] }), 'send keys body mismatch');
  await noInjection(page);
  // The sheet held focus when it opened (above), so its Escape handler is attached. Send
  // is disabled while sending and again once the sequence clears, so focus may have
  // dropped to the body by now; the document-level handler still sees the key.
  check(await exists(page, '#sheet-keys'), 'keys sheet closed before Escape');
  await page.keyboard.press('Escape');
  await until('keys sheet closed', async () => !(await exists(page, '#sheet-keys')));
  check((await activeId(page)) === 'overflow-button', 'focus not restored to the actions button');
  // Interrupt needs a confirmation and sends exactly one Esc.
  await page.keyboard.press('Enter');
  await until('actions sheet', async () => (await exists(page, '#sheet-overflow')) && focusInside(page, 'sheet-overflow'));
  await tabUntil(page, 'action-interrupt', { max: 6 });
  await page.keyboard.press('Enter');
  await until('interrupt confirmation', async () => (await exists(page, '#sheet-interrupt-confirm')) && focusInside(page, 'sheet-interrupt-confirm'));
  const before = fx.count('/api/agent/keys');
  await tabUntil(page, 'action-interrupt-confirm', { max: 4 });
  await page.keyboard.press('Enter');
  await until('interrupt request', () => fx.count('/api/agent/keys') === before + 1);
  check(JSON.stringify(fx.bodies('/api/agent/keys').at(-1).keys) === '["esc"]', 'interrupt did not send a single esc');
  await until('interrupt toast', async () => (await textOf(page, '#toast')).includes('Interrupt'));
}

async function composerKeyboard(open) {
  const { page, fx } = await open();
  await openDetail(page);
  await until('enabled composer', () => page.evaluate(() => { const e = document.getElementById('composer-input'); return e && !e.disabled; }));
  await page.evaluate(() => document.activeElement && document.activeElement.blur());
  await tabUntil(page, 'composer-input', { max: 120 });
  const draft = 'Run the focused tests';
  await page.keyboard.type(draft);
  await page.keyboard.press('Tab');
  check((await activeId(page)) === 'composer-send', 'Tab from the composer did not reach Send');
  await page.keyboard.press('Enter');
  await until('prompt request', () => fx.count('/api/agent/prompt') === 1);
  const body = fx.bodies('/api/agent/prompt')[0];
  check(body.text === draft && JSON.stringify(body.agent) === JSON.stringify(REF7) && UUID.test(body.submission_id), 'prompt body mismatch');
  await until('draft cleared after submitted', async () => (await valueOf(page, '#composer-input')) === '');
  // Past prompts sheet by keyboard, with focus restored to its opener.
  await page.focus('#composer-input');
  await page.keyboard.press('Shift+Tab');
  check((await activeId(page)) === 'composer-prompts', 'Shift+Tab from the composer did not reach Past prompts');
  await page.keyboard.press('Enter');
  await until('prompts filter focused', async () => (await activeId(page)) === 'prompts-filter');
  await until('prompt list', () => exists(page, '#sheet-prompts .prompt-item'));
  await page.keyboard.type('commit');
  await until('filtered prompts', () => page.evaluate(() => document.querySelectorAll('#sheet-prompts .prompt-item').length === 1));
  await assertTrap(page, 'sheet-prompts', 4);
  await page.focus('#sheet-prompts .prompt-item');
  await page.keyboard.press('Enter');
  await until('picked prompt in draft', async () => (await valueOf(page, '#composer-input')) === 'Commit the change');
  check(!(await exists(page, '#sheet-prompts')), 'prompts sheet still open');
  check((await activeId(page)) === 'composer-prompts', 'focus not restored to Past prompts');
  check(fx.count('/api/agent/prompt') === 1, 'picking a prompt sent it');
}

async function readerPrepend(open) {
  const { page, fx } = await open();
  await openDetail(page);
  await domClick(page, '.tools-group');
  await until('tools reader', async () => ((await textOf(page, '#sheet-reader')) || '').includes('tool call: cargo test'));
  await domClick(page, '#reader-more');
  await until('older tool records', async () => ((await textOf(page, '#sheet-reader')) || '').includes('older tool record'));
  check(!(await exists(page, '#reader-more')), 'reader offers more after the last page');
  check(fx.log.some((e) => e.path === '/api/tools' && e.query.has('before') && e.query.has('range')), 'older tools read lacked range+before');
  await sheetReady(page, 'sheet-reader', 'focus inside the tools reader');
  await page.keyboard.press('Escape');
  await until('reader closed', async () => !(await exists(page, '#sheet-reader')));
  check(await page.evaluate(() => document.activeElement && document.activeElement.classList.contains('tools-group')), 'focus not restored to the tools group');
  await domClick(page, '.event-read');
  await until('event reader', async () => ((await textOf(page, '#sheet-reader')) || '').includes('complete message text'));
  await sheetReady(page, 'sheet-reader', 'focus inside the event reader');
  await page.keyboard.press('Escape');
  await until('reader closed', async () => !(await exists(page, '#sheet-reader')));

  const s = await scrollState(page);
  check(s.overflow, 'timeline does not scroll; fixture too short for anchoring checks');
  await page.evaluate(() => { document.getElementById('timeline').scrollTop = 0; });
  await nextFrame(page);
  const anchor = await page.evaluate(() => {
    const n = document.querySelector('#timeline [data-key]');
    return { key: n.getAttribute('data-key'), top: n.getBoundingClientRect().top };
  });
  check(anchor.key === 'e20', `unexpected first item ${anchor.key}`);
  await domClick(page, '#timeline-earlier');
  await until('prepended older page', () => exists(page, '#timeline [data-key="e1"]'));
  await nextFrame(page);
  const after = await page.evaluate((k) => ({
    top: document.querySelector(`#timeline [data-key="${k}"]`).getBoundingClientRect().top,
    scrollTop: document.getElementById('timeline').scrollTop,
  }), anchor.key);
  check(Math.abs(after.top - anchor.top) <= 2, `prepend moved the anchor from ${Math.round(anchor.top)} to ${Math.round(after.top)}`);
  check(after.scrollTop > 0, 'prepend did not keep the reading position');
  check(!(await exists(page, '#timeline-earlier')), 'Load earlier still offered after the first event');
  // Delivery replies repeat the newest page's earlier cursor; exhausted history stays exhausted.
  const olderReads = () => fx.log.filter((e) => e.path === '/api/history' && e.query.has('before')).length;
  const reads = olderReads();
  await waitPolls(page, fx);
  check(!(await exists(page, '#timeline-earlier')), 'Load earlier returned after delivery polls');
  check(olderReads() === reads, 'an exhausted timeline read older pages again');
}

async function newMessages(open) {
  const { page, fx } = await open();
  await openDetail(page);
  await until('timeline pinned at the bottom', async () => (await scrollState(page)).atBottom);
  fx.latest = 46;
  await until('delivered message 46', () => exists(page, '#timeline [data-key="e46"]'));
  await nextFrame(page);
  check((await scrollState(page)).atBottom, 'pinned timeline did not stay at the bottom');
  check(!(await exists(page, '#timeline-new')), 'New messages shown while pinned');
  check(fx.log.some((e) => e.path === '/api/history' && e.query.get('after_revision') === '46'), 'delivery did not follow navigation.received');
  await page.evaluate(() => { const el = document.getElementById('timeline'); el.scrollTop = Math.max(0, el.scrollTop - 600); });
  await nextFrame(page);
  check(!(await scrollState(page)).atBottom, 'could not scroll up');
  const anchor = await page.evaluate(() => {
    const el = document.getElementById('timeline');
    const box = el.getBoundingClientRect();
    const n = [...el.querySelectorAll('[data-key]')].find((x) => x.getBoundingClientRect().bottom > box.top);
    return { key: n.getAttribute('data-key'), top: n.getBoundingClientRect().top };
  });
  await page.evaluate(() => {
    const el = document.getElementById('timeline');
    const native = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop');
    window.__yinzScrollWrites = 0;
    Object.defineProperty(el, 'scrollTop', {
      configurable: true,
      get() { return native.get.call(this); },
      set(v) { window.__yinzScrollWrites += 1; native.set.call(this, v); },
    });
  });
  // Two unchanged tail polls span more than two one-second age re-renders.
  await waitPolls(page, fx);
  const writes = await page.evaluate(() => {
    delete document.getElementById('timeline').scrollTop;
    return window.__yinzScrollWrites;
  });
  check(writes === 0, `the timeline wrote scrollTop ${writes} times while nothing changed`);
  fx.latest = 47;
  await until('New messages control', () => exists(page, '#timeline-new'));
  check(await exists(page, '#timeline [data-key="e47"]'), 'message 47 not rendered');
  const top = await page.evaluate((k) => document.querySelector(`#timeline [data-key="${k}"]`).getBoundingClientRect().top, anchor.key);
  check(Math.abs(top - anchor.top) <= 2, `new message moved the reading position (${Math.round(anchor.top)} -> ${Math.round(top)})`);
  await domClick(page, '#timeline-new');
  await until('jump to newest', async () => (await scrollState(page)).atBottom && !(await exists(page, '#timeline-new')));
}

async function hiddenForeground(open) {
  const { context, page } = await open({ clock: true });
  await openList(page);
  const hide = (hidden) => page.evaluate((h) => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => h });
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (h ? 'hidden' : 'visible') });
    document.dispatchEvent(new Event('visibilitychange'));
    return window.__yinzFetches.filter((f) => f.p.startsWith('/api/')).length;
  }, hidden);
  await pauseClock(context, page);
  await idle(page);
  const c0 = await fetchCount(page, '/api/agents');
  await context.clock.runFor(3100);
  await until('one list poll after 3 s', async () => (await fetchCount(page, '/api/agents')) === c0 + 1);
  await idle(page);
  const hiddenAt = await hide(true);
  for (let i = 0; i < 6; i += 1) {
    await context.clock.runFor(10000);
    await idle(page);
  }
  check((await fetchCount(page, '/api/')) === hiddenAt, 'requests were made while hidden');
  await until('paused freshness', async () => ((await textOf(page, '#list-freshness')) || '').startsWith('Paused'));
  const visibleAt = await hide(false);
  await until('immediate refresh on foreground', async () => (await fetchCount(page, '/api/')) === visibleAt + 1, 5000);
  await idle(page);

  // Detail: tail, usage and screen polling also stop while hidden.
  await context.clock.resume();
  await page.evaluate((h) => { location.hash = h; }, detailHash());
  await until('timeline events', () => exists(page, '#timeline .event'));
  await pauseClock(context, page);
  await idle(page);
  const detailHidden = await hide(true);
  for (let i = 0; i < 3; i += 1) {
    await context.clock.runFor(10000);
    await idle(page);
  }
  check((await fetchCount(page, '/api/')) === detailHidden, 'detail polled while hidden');
  const back = await hide(false);
  await until('detail refresh on foreground', async () => (await fetchCount(page, '/api/history')) > 0 && (await fetchCount(page, '/api/')) > back, 5000);
}

async function offlineRetryAfter(open) {
  const { context, page, fx } = await open({ clock: true });
  await openList(page);
  await pauseClock(context, page);
  await idle(page);
  const n = () => fetchCount(page, '/api/agents');
  const step = async (ms, expected, what) => {
    await context.clock.runFor(ms);
    await pause(0);
    await until(what, async () => (await n()) === expected);
    await idle(page);
  };
  let c = await n();
  fx.fail.set('/api/agents', { mode: 'abort', times: Infinity });
  await step(3100, c + 1, 'poll after 3 s');
  await textUntil(page, '#banner-offline', 'Offline', 'offline banner');
  check(await page.evaluate(() => document.querySelector('#main').classList.contains('is-stale')), 'stale list not marked');
  c = await n();
  await step(2100, c + 1, 'first 2 s backoff retry');
  c = await n();
  await step(3800, c, 'no retry before 4 s backoff');
  await step(300, c + 1, 'second backoff retry at 4 s');
  // Busy host with Retry-After: visible countdown, honoured by polls and manual refresh.
  fx.fail.set('/api/agents', { mode: 'reply', times: Infinity, status: 503, headers: { 'retry-after': '7' }, body: { error: { code: 'busy', message: `Host busy ${XSS(6)}` } } });
  c = await n();
  await step(8100, c + 1, 'retry after 8 s backoff');
  await textUntil(page, '#banner-offline', /Retrying in 7s/, 'Retry-After countdown');
  check(((await textOf(page, '#banner-offline')) || '').includes('Host busy <img'), 'server error message not shown as text');
  await noInjection(page);
  c = await n();
  await domClick(page, '#list-refresh');
  await idle(page);
  check((await n()) === c, 'manual refresh ignored Retry-After');
  // The countdown re-renders on a one-second interval whose phase is arbitrary.
  await context.clock.runFor(2000);
  await textUntil(page, '#banner-offline', /Retrying in [56]s/, 'countdown ticks');
  await step(4800, c, 'no request before Retry-After');
  fx.fail.delete('/api/agents');
  await step(300, c + 1, 'request once Retry-After passed');
  await until('banner cleared after recovery', async () => !(await exists(page, '#banner-offline')));
  // Detail tail read rate limited with Retry-After.
  await context.clock.resume();
  await page.evaluate((h) => { location.hash = h; }, detailHash());
  await until('timeline events', () => exists(page, '#timeline .event'));
  await pauseClock(context, page);
  await idle(page);
  const h = () => fetchCount(page, '/api/history');
  fx.fail.set('/api/history', { mode: 'reply', times: Infinity, status: 429, headers: { 'retry-after': '5' }, body: { error: { code: 'rate_limited' } } });
  let hc = await h();
  await context.clock.runFor(2100);
  await until('tail poll', async () => (await h()) === hc + 1);
  await idle(page);
  await textUntil(page, '#banner-offline', /Too many requests\. Retrying in 5s/, 'rate-limit banner');
  hc = await h();
  await context.clock.runFor(4800);
  await idle(page);
  check((await h()) === hc, 'tail polled during Retry-After');
  fx.fail.delete('/api/history');
  await context.clock.runFor(300);
  await until('tail poll after Retry-After', async () => (await h()) === hc + 1);
  await until('detail banner cleared', async () => !(await exists(page, '#banner-offline')));
}

async function draftAuthLoss(open) {
  const { page, fx } = await open();
  await openDetail(page);
  await until('enabled composer', () => page.evaluate(() => { const e = document.getElementById('composer-input'); return e && !e.disabled; }));
  // Network failure while drafting: stale banner, draft kept, no redirect.
  await page.focus('#composer-input');
  const draft = 'Draft kept across failures';
  await page.keyboard.type(draft);
  fx.fail.set('/api/history', { mode: 'abort', times: Infinity });
  await until('detail offline banner', async () => ((await textOf(page, '#banner-offline')) || '').includes('Offline'), 15000);
  check(pathOf(page) === '/' && (await valueOf(page, '#composer-input')) === draft, 'network failure lost the draft or left the page');
  fx.fail.delete('/api/history');
  await until('detail recovered', async () => !(await exists(page, '#banner-offline')), 40000);
  // Real auth loss while drafting.
  await revokeLive(page);
  await until('redirect to /login after revocation', () => pathOf(page) === '/login');
  await until('expired notice', async () => ((await textOf(page, '#login-status')) || '').includes('expired'));
  await signIn(page);
  await until('restored route', () => hashOf(page) === detailHash());
  await until('restored draft', async () => (await valueOf(page, '#composer-input')) === draft);
  await waitPolls(page, fx);
  check(fx.count('/api/agent/prompt') === 0, 'a prompt was sent without Send');
}

async function sendAuthLoss(open) {
  const { page, fx } = await open();
  await openDetail(page);
  await until('enabled composer', () => page.evaluate(() => { const e = document.getElementById('composer-input'); return e && !e.disabled; }));
  await page.focus('#composer-input');
  const draft = 'Prompt sent as auth ends';
  await page.keyboard.type(draft);
  fx.revokeOnPrompt = true;
  await domClick(page, '#composer-send');
  await until('redirect to /login', () => pathOf(page) === '/login');
  fx.revokeOnPrompt = false;
  await until('expired notice', async () => ((await textOf(page, '#login-status')) || '').includes('expired'));
  await signIn(page);
  await until('restored route', () => hashOf(page) === detailHash());
  await until('uncertain submission panel', () => exists(page, '#submission-unknown'));
  check(((await textOf(page, '#submission-unknown')) || '').includes('sign-in ended'), 'uncertain panel lacks the auth reason');
  check((await valueOf(page, '#composer-input')) === draft, 'draft not preserved');
  await waitPolls(page, fx);
  check(fx.count('/api/agent/prompt') === 1, `prompt resent automatically (${fx.count('/api/agent/prompt')} requests)`);
  await domClick(page, '#submission-retry');
  await until('explicit retry', () => fx.count('/api/agent/prompt') === 2);
  const [first, second] = fx.bodies('/api/agent/prompt');
  check(first.submission_id === second.submission_id && first.text === second.text && JSON.stringify(second.agent) === JSON.stringify(REF7), 'retry changed the submission');
  await until('retry resolved', async () => !(await exists(page, '#submission-unknown')) && (await valueOf(page, '#composer-input')) === '');
}

async function reloadRecovery(open) {
  const { page, fx } = await open();
  await openDetail(page);
  await until('enabled composer', () => page.evaluate(() => { const e = document.getElementById('composer-input'); return e && !e.disabled; }));
  await page.focus('#composer-input');
  await page.keyboard.type('Network drops this reply');
  fx.fail.set('/api/agent/prompt', { mode: 'abort', times: 1 });
  await domClick(page, '#composer-send');
  await until('uncertain after network failure', async () => ((await textOf(page, '#submission-unknown')) || '').includes('Cannot reach yinz'));
  await page.reload();
  await until('uncertain submission after reload', () => exists(page, '#submission-unknown'));
  await waitPolls(page, fx);
  check(fx.count('/api/agent/prompt') === 1, 'reload resent the prompt');
  await domClick(page, '#submission-retry');
  await until('explicit retry', () => fx.count('/api/agent/prompt') === 2);
  const [a, b] = fx.bodies('/api/agent/prompt');
  check(a.submission_id === b.submission_id && a.text === b.text, 'retry after reload changed the submission');
  await until('retry resolved', async () => !(await exists(page, '#submission-unknown')));
  // Reload while sending: the record survives as uncertain and is never replayed.
  await page.focus('#composer-input');
  await page.keyboard.type('Reload during send');
  fx.fail.set('/api/agent/prompt', { mode: 'hold', times: 1 });
  await domClick(page, '#composer-send');
  await until('sending state', () => exists(page, '#submission-sending'));
  await until('held prompt', () => fx.held.length === 1);
  await page.reload();
  fx.releaseHeld();
  // Firefox may settle the aborted fetch before unloading ("Cannot reach yinz"); otherwise the
  // rehydrated `sending` record says the page reloaded. Either way it is uncertain, not resent.
  await textUntil(page, '#submission-unknown', /^Outcome unknown.*(reloaded before the outcome|Cannot reach yinz)/, 'uncertain after reload during send');
  await waitPolls(page, fx);
  check(fx.count('/api/agent/prompt') === 3, 'reload during send replayed the prompt');
  await domClick(page, '#submission-discard');
  await until('discarded', async () => !(await exists(page, '#submission-unknown')));
  check(fx.count('/api/agent/prompt') === 3, 'discard sent a prompt');
  // A definite native rejection is shown as text.
  fx.prompt = 'rejected';
  await until('send enabled', () => page.evaluate(() => !document.getElementById('composer-send').disabled));
  await domClick(page, '#composer-send');
  await until('failed panel', async () => ((await textOf(page, '#submission-failed')) || '').includes('Refused <img'));
  await noInjection(page);
}

async function goneRetargetStored(open) {
  const { page, fx } = await open();
  await openDetail(page);
  fx.gone = true;
  await until('agent gone banner', () => exists(page, '#banner-agent-gone'));
  check(!(await exists(page, '#overflow-button')) && !(await exists(page, '#composer-input')), 'actions still offered for a gone agent');
  check(await page.evaluate(() => document.getElementById('tab-screen').disabled), 'screen tab enabled for a gone agent');
  fx.gone = false;
  fx.retarget = true;
  await until('retarget banner', () => exists(page, '#banner-retargeted'), 15000);
  await until('route follows the new run', () => hashOf(page).includes('run=run-7b'));
  await textUntil(page, '#timeline', 'Retargeted run message', 'new run content');
  check(!((await textOf(page, '#timeline')) || '').includes('Message 45'), 'old run events survived the retarget');
  fx.retarget = false;
  // Same-agent hash edits keep the mounted view (by design, for retargets); go via the list.
  await page.evaluate(() => { location.hash = '#/'; });
  await until('agent cards', () => exists(page, 'li.agent-card'));
  await page.evaluate((h) => { location.hash = h; }, detailHash('run-7', 'details'));
  await until('details panel', () => exists(page, '#details-previous'));
  await domClick(page, '#details-previous');
  await until('stored-run route', () => hashOf(page).includes('bind=0') && hashOf(page).includes('run=run-6'));
  await until('stored banner', () => exists(page, '#banner-stored'));
  await until('stored run content', async () => ((await textOf(page, '#timeline')) || '').includes('Stored run-6'));
  const reads = fx.log.filter((e) => e.path === '/api/history' && e.query.get('run') === 'run-6');
  check(reads.length && reads.every((e) => !e.query.has('agent')), 'stored-run browsing carried the live agent binding');
  check(!(await exists(page, '#composer-input')), 'composer offered for a stored run');
  await domClick(page, '#detail-back');
  await until('back to the details route', async () => hashOf(page) === detailHash('run-7', 'details') && exists(page, '#details-previous'));
}

async function backThroughHistory(open) {
  const { page } = await open();
  await openList(page);
  await domClick(page, 'li.agent-card[data-agent-id="live:dev:7"] a.agent-open');
  await until('detail opened from the list', () => exists(page, '#detail-back'));
  const opened = await page.evaluate(() => history.length);
  const detail = hashOf(page);
  await domClick(page, '#detail-back');
  await until('back to the list', async () => hashOf(page) === '#/' && exists(page, 'li.agent-card'));
  const after = await page.evaluate(() => history.length);
  check(after === opened, `back added history entries (${opened} -> ${after})`);
  await page.evaluate(() => history.forward());
  await until('forward returns to the detail', async () => hashOf(page) === detail && exists(page, '#detail-back'));
  // A deep-linked detail has nothing of ours behind it: back replaces it with the list.
  await page.goto('about:blank');
  await page.goto(`${ORIGIN}/${detailHash()}`);
  await until('deep-linked detail', () => exists(page, '#detail-back'));
  const deep = await page.evaluate(() => history.length);
  await domClick(page, '#detail-back');
  await until('list after deep-link back', async () => hashOf(page) === '#/' && exists(page, 'li.agent-card'));
  const replaced = await page.evaluate(() => history.length);
  check(replaced === deep, `deep-link back changed history length (${deep} -> ${replaced})`);
}

async function logoutFlows(open) {
  const other = await open();
  await openList(other.page);
  const { page } = await open();
  await openApp(page, '#/', { remember: true });
  await until('agent cards', () => exists(page, 'li.agent-card'));
  await domClick(page, '#menu-button');
  await until('menu', () => exists(page, '#logout'));
  await domClick(page, '#logout');
  await waitLogin(page);
  await until('signed out notice', async () => ((await textOf(page, '#login-status')) || '').includes('Signed out.'));
  const cleared = await tokenStores(page);
  check(!cleared.local && !cleared.session, 'logout kept a token');
  await signIn(page, { remember: true });
  await until('agent cards', () => exists(page, 'li.agent-card'));
  await domClick(page, '#menu-button');
  await until('menu', () => exists(page, '#logout-all'));
  await domClick(page, '#logout-all');
  await until('logout-all confirmation', () => exists(page, '#sheet-logout-all'));
  await domClick(page, '#logout-all-confirm');
  await waitLogin(page);
  await until('everywhere notice', async () => ((await textOf(page, '#login-status')) || '').includes('every device'));
  // The other device's live session was revoked too.
  await other.page.reload();
  await waitLogin(other.page);
}

// ---------------------------------------------------------------------------------------

async function main() {
  browser = await firefox.launch({ headless: true });
  console.log(`yinz web browser checks: Firefox ${browser.version()} via Playwright (PLAYWRIGHT_BROWSERS_PATH set), server ${ORIGIN}`);
  console.log('LIVE: pages, /static assets, /api/login, /api/session, /api/logout(-all). INTERCEPTED fixtures: /api/agents, /api/history, /api/hit, /api/event, /api/tools, /api/usage, /api/prompts, /api/agent/* (bearer re-checked live).');
  await scenario('keyboard-only login, session-only token, repeated entry', keyboardLogin);
  await scenario('remembered login across tabs and contexts', rememberedLogin);
  await scenario('phone layout 320px: overflow, touch targets, text-only server strings', (open) => phoneLayout(open, 320));
  await scenario('phone layout 390px: overflow, touch targets, text-only server strings', (open) => phoneLayout(open, 390));
  for (const width of [320, 390]) {
    for (const scheme of ['light', 'dark']) await scenario(`compact agent list ${width}px ${scheme}`, (open) => compactAgentList(open, width, scheme));
  }
  await scenario('compact agent list 1024px desktop', (open) => compactAgentList(open, 1024, 'light'));
  await scenario('dark mode changes computed backgrounds', darkMode);
  await scenario('safe-area CSS and manifest', safeAreaManifest);
  await scenario('sheet focus trap, Escape and focus restore; keys and interrupt', sheetsKeyboard);
  await scenario('keyboard composer and past prompts', composerKeyboard);
  await scenario('readers and prepend keeps the reading anchor', readerPrepend);
  await scenario('new-message anchoring (pinned vs reading)', newMessages);
  await scenario('hidden document pauses polling; foreground refreshes immediately', hiddenForeground);
  await scenario('offline banner, backoff and Retry-After without request storms', offlineRetryAfter);
  await scenario('network and auth failure while drafting keep the draft, no send', draftAuthLoss);
  await scenario('auth failure while sending: uncertain, no auto-resend, same-id retry', sendAuthLoss);
  await scenario('reload recovery of uncertain submissions and same-id retry', reloadRecovery);
  await scenario('agent gone, retarget and stored-run browsing', goneRetargetStored);
  await scenario('back returns through history', backThroughHistory);
  await scenario('live logout and logout everywhere', logoutFlows);
}

let exitCode = 1;
const shutdown = async () => {
  for (const c of contexts) await c.close().catch(() => {});
  if (browser) await browser.close().catch(() => {});
};
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    shutdown().finally(() => process.exit(130));
  });
}
try {
  await main();
  const failed = results.filter((r) => !r.ok);
  console.log(`${failed.length ? 'FAIL' : 'PASS'}: ${results.length - failed.length}/${results.length} browser scenarios passed`);
  exitCode = failed.length ? 1 : 0;
} catch (e) {
  console.log(`FAIL harness: ${redact(e && e.stack ? e.stack : e)}`);
  exitCode = 1;
} finally {
  await shutdown();
}
process.exit(exitCode);
