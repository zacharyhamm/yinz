#!/usr/bin/env node
// Phone-flow browser checks for the builtin yinz web app (test-only; never shipped).
//
//   node run.mjs <base-url> [filter]   password from YINZ_WEB_PASSWORD or one stdin line;
//                                      filter (or YINZ_WEB_BROWSER_FILTER) selects scenario names
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
const cardSelector = (id) => `.agent-card[data-agent-id="${id}"]`;
const peerSelector = (digit) => `peer-detail:v1:${digit.repeat(64)}`;
const REF7 = { id: AGENT_ID, kind: 'codex', session_id: 'conv-1' };
const REF7B = { id: AGENT_ID, kind: 'codex', session_id: 'conv-2' };
const XSS = (n) => `<img src=x onerror="window.__yinzXss=${n}">`;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const detailHash = (run = 'run-7', tab = 'timeline', id = AGENT_ID) =>
  `#/a/${encodeURIComponent(id)}?${run ? `run=${run}&` : ''}tab=${tab}`;
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
    this.header = 'short'; // short | long | absent | wrapped | custom
    this.live = {};
    this.usage = null;
    this.fail = new Map(); // path -> {mode:'abort'|'hold'|'reply', times, matches?, status, headers, body}
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
  const defaultStatus = { waiting: 'needs input', busy: 'working', idle: 'idle' }[state];
  const r = {
    id, ref: retargeted ? { ...REF7B } : { id, kind: 'codex', session_id: 'conv-1' },
    session: 'browser-check', current_session: true, pane_id: id.split(':').pop(), provider: 'codex', state,
    glyph: { waiting: '?', busy: '●', idle: '○' }[state] || '?',
    status: defaultStatus || state,
    tab_group: { id: 0, name: 'Default' },
    host: null, title: `Agent ${id.split(':').pop()}`, activity: 'editing', project: 'yinz',
    branch: 'main', last_reply: { text: 'Done with step 1', truncated: false }, last_hook_at_ms: 1, focused: false,
    selectable: true, suppressed: false, directory: '/w', run: retargeted ? 'run-7b' : `run-${id.split(':').pop()}`,
    wait_needs_a_person: state === 'waiting', source: 'hook', can_prompt: true, prompt_unavailable: null,
    usage_unavailable: null, ...extra,
  };
  if (r.detail_id && r.ref) r.ref = { ...r.ref, id: r.detail_id };
  // Mirror the default Scheme hook's structure, paired with real /api/agents Scheme
  // assertions in web.test.scm; this fixture is not evidence of Scheme execution.
  const source = r.last_reply?.text || '';
  const chars = Array.from(source);
  const preview = chars.slice(0, 160).join('') + (chars.length > 160 || r.last_reply?.truncated ? '…' : '');
  const badges = (r.badges || []).map((b) => `<span class="badge" data-tone="${esc(b.tone || '')}">${esc(b.text)}</span>`).join('');
  const stateMarker = defaultStatus && r.status === defaultStatus ? ` data-redundant-state="${esc(state)}"` : '';
  r.html = `<div class="agent-row" data-state="${esc(r.state)}"><div class="agent-row-heading"><span class="agent-glyph" aria-hidden="true">${esc(r.glyph)}</span><strong class="agent-title">${esc(r.title.trim() ? r.title : r.provider)}</strong><span class="agent-status"${stateMarker}>${esc(r.status)}</span></div><div class="agent-meta">${[r.provider, r.project, r.branch].filter((s) => s && s.trim()).map(esc).join(' · ')}</div>${badges ? `<div class="badges">${badges}</div>` : ''}${source.trim() ? `<p class="agent-reply">${esc(preview)}</p>` : ''}</div>`;
  if (r.wrapper) r.html = `<div class="custom-row">${r.html}</div>`;
  return r;
}

function agents(fx) {
  if (fx.rows) return {
    session: 'browser-check', now_ms: Date.now(),
    counts: fx.counts ?? Object.fromEntries(['waiting', 'busy', 'idle'].map((s) => [s, fx.rows.filter((r) => r.state === s).length])),
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

function headerFields(fx, run) {
  const long = fx.header === 'long';
  return {
    name: long ? `修正 🧭 ${'longIdentity'.repeat(12)}` : `Fix flaky test ${XSS(3)}`,
    status: 'waiting', location: long ? `/workspace/${'location/'.repeat(12)}` : 'dev', started: '12:00',
    task: long ? `Review the complete mobile conversation and preserve all metadata. ${FILLER}` : 'fix',
    project: long ? 'Mobile conversation project with a long descriptive name' : 'yinz',
    branch: long ? `feature/${'unbroken-branch'.repeat(12)}` : 'main', session_id: 'conv-1', details: `Run: ${run}`,
  };
}

// Mirror %web-app-header in yinz-steel/src/web_app.scm; its real Scheme assertions
// and web_builtin_production_http cover the production hook, not this imitation.
function headerHtml(fx, header) {
  if (fx.header === 'absent') return '';
  const markup = `<header class="agent-header"><h2>${esc(header.name)}</h2><p class="agent-header-status">${esc(header.status)}</p>${header.task ? `<p class="agent-header-task">${esc(header.task)}</p>` : ''}<p class="agent-header-meta">${['project', 'branch', 'location', 'started'].map((k) => header[k]).filter((s) => s && s.trim()).map(esc).join(' · ')}</p></header>`;
  if (fx.header === 'wrapped') return `<section class="custom-header">${markup}<a href="#custom-information" id="custom-information">Custom information link</a></section>`;
  if (fx.header === 'custom') return `<section class="custom-header"><h2>Custom hook output</h2><p>${esc(FILLER.repeat(12))}</p><button type="button" class="button" id="custom-information">Custom information action</button></section>`;
  return markup;
}

function history(fx, q) {
  const requested = q.get('run');
  const agent = q.get('agent');
  const scoped = agent?.startsWith('peer-detail:');
  const matches = scoped ? agents(fx).agents.filter((row) => row.detail_id === agent) : [];
  let live = null;
  if (agent && !fx.gone) {
    if (scoped && matches.length === 1) {
      const { tab_group, tab_group_session, ...selected } = matches[0];
      live = selected;
    } else if (!scoped) {
      live = row(fx, agent, 'waiting', { title: fx.header === 'long' ? headerFields(fx, requested).name : `Fix flaky test ${XSS(1)}`, ...fx.live });
    }
  }
  const run = scoped ? live?.run || requested : agent && fx.retarget && requested === 'run-7' ? 'run-7b' : requested;
  const retargeted = run !== requested;
  const before = !retargeted && q.get('before') ? JSON.parse(q.get('before')) : null;
  const after = !retargeted && q.has('after_revision') ? Number(q.get('after_revision')) : null;
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
    if (fx.code && e.revision === fx.latest) e.html += `<pre><code>${esc('const complete = '+ 'long_code_token'.repeat(30) + ';\nreturn complete;')}</code></pre>`;
  }
  return {
    run, retargeted, agent_gone: !!agent && !live, live,
    header: headerFields(fx, run),
    events, earlier, next_from: null, previous_run: run === 'run-7' ? 'run-6' : null, origin_run: null, resumable: false,
    activity_ranges: [], navigation, status: '',
    header_html: headerHtml(fx, headerFields(fx, run)),
    details_html: `<pre class="details">${esc(`Run: ${run}\nProvider: codex`)}</pre>`,
  };
}

/**
 * 80 lines like a real pane read: older output, one wide line, a 124-column
 * box-drawn agent panel and permission prompt, then the question at the bottom.
 */
const SCREEN_LAST = '  2. No';
const WIDE_PATH = '/home/dev/projects/yinz/yinz-steel-builtins/assets/plugins/web/static/views/detail-screen-panel.js';
const WIDE_OPTION = "2. Yes, and don't ask again this session for edits under /home/dev/projects/yinz/yinz-steel-builtins (shift+tab)";
const SCREEN_COLUMNS = 124;
const boxed = (lines) => [`╭${'─'.repeat(SCREEN_COLUMNS - 2)}╮`,
  ...lines.map((l) => `│ ${l.padEnd(SCREEN_COLUMNS - 4)} │`), `╰${'─'.repeat(SCREEN_COLUMNS - 2)}╯`];
const WIDE_BLOCK = [
  ...boxed(['✻ Welcome to the agent', `cwd: ${WIDE_PATH}`]),
  ...boxed([`Edit file ${WIDE_PATH}?`, '❯ 1. Yes', `  ${WIDE_OPTION}`, '  3. No, and tell the agent what to do differently (esc)']),
];
const WIDE_START = 50;
function screenOutput(tick) {
  const older = Array.from({ length: 76 }, (_, i) => (i === 0 ? `output line 0 (read ${tick})`
    : i === 40 ? `output line 40 ${'wide-'.repeat(30)}end`
      : i >= WIDE_START && i < WIDE_START + WIDE_BLOCK.length ? WIDE_BLOCK[i - WIDE_START] : `output line ${i}`));
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
  if (p === '/api/agent/launch-choices' && entry.method === 'GET') {
    return [200, { server_instance: 'browser-instance', providers: ['codex'], projects: ['Review'], worktrees: entry.query.has('project')
      ? [{ path: '/review', branch: null }] : [] }];
  }
  if (entry.method !== 'POST') return [405, { error: { code: 'method_not_allowed' } }];
  if (p === '/api/agent/launch') {
    if (!/^browser-instance\.[0-9a-f]{32}$/.test(body?.submission_id)) return invalid('submission id');
    if (body?.kind !== 'codex' || body.project !== 'Review' || body.checkout?.path !== '/review') return invalid('checkout');
    return [200, { session: 'browser-check', pane_id: '7', kind: 'codex' }];
  }
  if (p === '/api/agent/destinations') {
    if (!validRef(body?.agent)) return invalid('agent');
    return [200, { current_group: 0, current_tab: 1, allow_new_group: true,
      groups: [{ id: 0, name: 'Default', tabs: [{ id: 1, name: 'Shell', incarnation: 'old' }] },
        { id: 4, name: 'Review', tabs: [{ id: 5, name: 'Shell', incarnation: 'other' }] }] }];
  }
  if (p === '/api/agent/move') {
    const dest = body?.destination;
    if (body.agent?.session_id !== REF7.session_id || dest?.group !== 4 || dest.tab !== 5 || dest.incarnation !== 'other') return invalid('destination');
    fx.live.location = { group: { id: 4, name: 'Review' }, tab: { id: 5, name: 'Shell' } };
    return [200, { ok: true }];
  }
  if (p === '/api/agent/close') {
    if (!validRef(body?.agent)) return invalid('agent');
    fx.rows = [];
    fx.gone = true;
    return [200, { ok: true }];
  }
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
  if (p === '/api/agent/term') {
    if (!validRef(body.agent) || Object.keys(body).some((k) => !['agent', 'after'].includes(k))) return invalid('shape');
    const rows = fx.termRows || 24;
    const cursor = fx.termMouse === 'legacy' ? '1-100' : fx.termMouse ? '1-80' : '1-40';
    let data = '';
    if (!body.after) {
      const output = fx.termRows ? `\x1b[${rows};1H${SCREEN_LAST}` : screenOutput(1).replaceAll('\n', '\r\n');
      data = '\x1b[?2004h\x1b[?1h\x1b[32m' + output + '\x1b[0m\x1b[6n';
    } else if (body.after !== cursor) {
      if (fx.termMouse === 'legacy') data = '\x1b[?1006l\x1b[?2004l\r\nLegacy mouse';
      else if (fx.termMouse) data = '\x1b[?1049h\x1b[HMouse mode\x1b[?1006h\x1b[?1002h';
      else data = '\x1b[?1049l\x1b[?1002l\x1b[?2004h\r\nLegacy mode';
    }
    return [200, { response: { outcome: 'ok', terminal: { snapshot: !body.after, cols: 120, rows,
      supports_kitty_keyboard_protocol: fx.termMouse === true, cursor, data: Buffer.from(data).toString('base64') } } }];
  }
  if (p === '/api/agent/term/input') {
    const mouse = exactKeys(body.data, ['mouse']) && typeof body.data.mouse === 'string';
    if (!validRef(body.agent) || (typeof body.data !== 'string' && !mouse && !exactKeys(body.data, ['bare_key', 'key_modifiers']))) return invalid('shape');
    return [200, { response: { outcome: 'ok' } }];
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
      if (fx.usage) return [200, { usage: fx.usage }];
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
    if (fail && fail.times > 0 && (!fail.matches || fail.matches(entry))) {
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
const publicRequestStarts = [];
const publicRequestWindowMs = 1500;
const publicRequestWindowLimit = 64;

function isPublicPageOrAsset(pathname) {
  return pathname === '/' || pathname === '/login' || pathname.startsWith('/static/');
}

async function continuePublicRequest(route) {
  // Fresh fixture contexts load the same module graph without a shared cache.
  // Pace that synthetic traffic across contexts below the listener's shared
  // public budget (50 requests/s, burst 100). All responses still come from Yinz.
  for (;;) {
    const now = performance.now();
    while (publicRequestStarts.length && now - publicRequestStarts[0] >= publicRequestWindowMs) {
      publicRequestStarts.shift();
    }
    if (publicRequestStarts.length < publicRequestWindowLimit) {
      publicRequestStarts.push(now);
      return route.continue();
    }
    await pause(Math.ceil(publicRequestStarts[0] + publicRequestWindowMs - now));
  }
}

function attachPage(page, guard) {
  page.on('pageerror', (e) => guard.add(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error') guard.add(`console error: ${m.text()}`);
  });
  page.on('response', (r) => {
    const u = new URL(r.url());
    if (u.origin !== ORIGIN || intercepted(u.pathname)) return;
    if (isPublicPageOrAsset(u.pathname) && r.status() >= 400) {
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
  await context.route((u) => u.origin === ORIGIN && isPublicPageOrAsset(u.pathname), continuePublicRequest);
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
  await page.goto(`${ORIGIN}/${hash}`, { waitUntil: 'commit' });
  // An unauthenticated app document schedules its redirect after it loads.
  await until('the sign-in page or authenticated app', async () =>
    pathOf(page) === '/login' || await exists(page, '#app .app'));
  if (pathOf(page) === '/login') await signIn(page, { remember });
}

async function openDetail(page, hash = detailHash()) {
  await openApp(page, hash);
  await until('timeline events', () => exists(page, '#timeline .event'));
}

async function openList(page) {
  await openApp(page, '#/');
  await until('agent cards', () => exists(page, 'li.agent-card'));
}

const groupRoute = (session, id) => `#/g/${encodeURIComponent(JSON.stringify([session, id]))}`;

async function switchGroup(page, session, id) {
  const hash = groupRoute(session, id);
  if (hashOf(page) === hash) return;
  await page.click('#switch-group');
  await page.locator(`#sheet-groups a[href="${hash}"]`).click();
  await until('selected group page', async () =>
    hashOf(page) === hash && await activeId(page) === 'main' && await exists(page, '.agent-section'));
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
    const bottomGap = el.scrollHeight - el.scrollTop - el.clientHeight;
    return { atBottom: bottomGap < 32, bottomGap, top: el.scrollTop, overflow: el.scrollHeight > el.clientHeight + 100 };
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

async function terminalFits(page, content = '2. No') {
  await textUntil(page, '.xterm-rows', content, 'terminal populated before measuring the grid');
  await until('native terminal grid fits the panel width', () => page.evaluate(() => {
    const grid = document.querySelector('.xterm-screen')?.getBoundingClientRect();
    const stage = document.querySelector('.term-stage')?.getBoundingClientRect();
    return grid?.width > 0 && Math.abs(grid.left - stage.left) < 1 && Math.abs(grid.right - stage.right) < 1;
  }));
}

async function waitPolls(page, fx, n = 2) {
  const start = fx.count('/api/history');
  await until(`${n} more tail polls`, () => fx.count('/api/history') >= start + n, 15000);
}

// ---------------------------------------------------------------------------------------
// Scenarios

const results = [];
const RESULTS_DIR = path.join(REPO, 'target', 'web-browser-results');

const FILTER = process.argv[3] || process.env.YINZ_WEB_BROWSER_FILTER || '';

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
  check(await page.locator('.agent-section').count() === 1, 'the list rendered multiple groups');
  await noHorizontalOverflow(page, `list ${width}px`);
  await touchTargets(page, `list ${width}px`);
  await noClippedControls(page, `list ${width}px`);
  await switchGroup(page, 'peer', 0);
  check((await textOf(page, cardSelector('live:peer:9'))).includes('session peer'), 'peer session chip missing');
  check(!(await exists(page, cardSelector(AGENT_ID))), 'the serving session appeared on the peer page');
  await switchGroup(page, 'browser-check', 0);
  // Agent titles arrive as server-escaped row html; the title also reaches toasts as text.
  check((await textOf(page, cardSelector(AGENT_ID))).includes('<img src=x'), 'escaped title text not shown');
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
  for (const tab of ['timeline', 'term', 'details']) {
    await domClick(page, `#tab-${tab}`);
    await until(`${tab} panel`, async () => (await page.evaluate((t) => document.getElementById(`tab-${t}`).getAttribute('aria-selected'), tab)) === 'true' && exists(page, `#panel-${tab}`));
    if (tab === 'term') await terminalFits(page);
    if (tab === 'details') await until('details content', () => exists(page, '#details'));
    await noHorizontalOverflow(page, `detail ${tab} ${width}px`);
    await touchTargets(page, `detail ${tab} ${width}px`);
    await noClippedControls(page, `detail ${tab} ${width}px`);
  }
  check((await textOf(page, '#detail-header-html')).includes('<img src=x'), 'escaped header text not shown');
  await noInjection(page);
}

async function iphoneScreenKeys(open, customKeys) {
  const { page, fx } = await open({ viewport: { width: 393, height: 852 } });
  // A continuation must arrive even when the next ordinary poll is a minute away.
  await page.addInitScript((keys) => {
    document.addEventListener('readystatechange', () => {
      const node = document.getElementById('yinz-config');
      if (document.readyState === 'interactive' && node) {
        const config = JSON.parse(node.textContent);
        node.textContent = JSON.stringify({ ...config, ...(keys && { quick_keys: keys }),
          poll: { ...config.poll, screen_ms: 60000 } });
      }
    });
  }, customKeys);
  await openDetail(page);
  const readPath = '/api/agent/term';
  fx.fail.set(readPath, { mode: 'hold', times: customKeys ? 4 : 3 });
  await page.click('#tab-term');
  await until('empty terminal mounted with snapshot in flight', async () =>
    fx.held.length === 1 && await exists(page, '.xterm-rows'));
  check(!(await textOf(page, '.xterm-rows')).trim(), 'terminal already had output before the snapshot');
  const [, reply] = agentRoute(fx, { method: 'POST', path: readPath, body: fx.bodies(readPath)[0] });
  const frame = reply.response.terminal;
  const bytes = Buffer.from(frame.data, 'base64');
  const split = bytes.lastIndexOf(SCREEN_LAST);
  check(split > 0, 'fixture is missing its final line');
  const fulfillPart = (part) => fx.held.shift().fulfill({ status: 200, json: { response: {
    outcome: 'ok', terminal: { ...frame, ...part },
  } } });
  await fulfillPart({ snapshot_continues: true, cursor: 'snapshot:1', data: bytes.subarray(0, split).toString('base64') });
  await until('continuation in flight', () => fx.held.length === 1);
  check(fx.bodies(readPath)[1].after === 'snapshot:1', 'continuation requested with wrong cursor');
  await textUntil(page, '.xterm-rows', '1. Yes', 'first snapshot part rendered');
  check(await page.locator('.screen-keys .quick-key').first().isDisabled(), 'input enabled before final part');
  await fulfillPart({ snapshot: false, data: bytes.subarray(split).toString('base64') });
  await textUntil(page, '.xterm-rows', '2. No', 'terminal populated before measuring and photographing keys');
  check((await textOf(page, '.xterm-rows')).includes('1. Yes'), 'final part cleared the first part');
  const quickKey = page.locator('.screen-keys .quick-key').first();
  await until('live cursor poll in flight', () => fx.held.length === 1);
  check(fx.bodies(readPath).at(-1).after === frame.cursor, 'wrong live cursor');
  check(await quickKey.isDisabled(), 'input enabled at snapshot capture offset');
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.type('ignored');
  if (customKeys) {
    await fulfillPart({ snapshot: true, cursor: 'replacement', data: frame.data });
    await until('replacement snapshot catch-up', () => fx.held.length === 1);
    check(fx.bodies(readPath).at(-1).after === 'replacement', 'wrong replacement cursor');
    check(await quickKey.isDisabled(), 'replacement snapshot released input');
  }
  const delta = customKeys ? '' : '\r\nDialog updated';
  await fulfillPart({ snapshot: false, data: Buffer.from(delta).toString('base64') });
  await until('input enabled after live delta', () => quickKey.isEnabled());
  if (delta) await textUntil(page, '.xterm-rows', 'Dialog updated', 'updated dialog rendered');
  const { header } = await detailGeometry(page);
  check(header <= 96, `393px collapsed header is ${header}px tall`);
  const keys = await page.locator('.screen-keys .quick-key').evaluateAll((buttons) => buttons.map((button) => {
    const box = button.getBoundingClientRect();
    const range = document.createRange();
    range.selectNodeContents(button);
    const label = range.getBoundingClientRect();
    return { key: button.dataset.key, top: box.top, labelFits: label.left >= box.left && label.right <= box.right };
  }));
  const expected = customKeys || ['1', '2', '3', 'enter', 'esc', 'up', 'down'];
  check(JSON.stringify(keys.map((k) => k.key)) === JSON.stringify(expected), 'wrong Screen quick keys');
  check(keys.every((k) => k.top === keys[0].top && k.labelFits), `393px key row: ${JSON.stringify(keys)}`);
  await touchTargets(page, '393px Screen');
  await noHorizontalOverflow(page, '393px Screen');
  await noClippedControls(page, '393px Screen');
  fs.mkdirSync(RESULTS_DIR, { recursive: true });
  await page.screenshot({ path: path.join(RESULTS_DIR, `screen-393-${keys.length}-keys.png`), mask: [page.locator('#composer-input')] });
  await page.click('.screen-keys [data-key="enter"]');
  await until('Screen Enter delivered', () => fx.count('/api/agent/keys') === 1);
  check(fx.count('/api/agent/term/input') === 0, 'keyboard input leaked during catch-up');
  check(JSON.stringify(fx.bodies('/api/agent/keys')[0]) === JSON.stringify({ agent: REF7, keys: ['enter'] }),
    'Screen Enter sent the wrong key or target');
}

async function detailGeometry(page) {
  return page.evaluate(() => {
    const height = (s) => document.querySelector(s)?.getBoundingClientRect().height || 0;
    return { width: innerWidth, height: innerHeight, dark: matchMedia('(prefers-color-scheme: dark)').matches,
      header: height('#detail-header'), timeline: height('#timeline'), composer: height('.composer-dock'),
      safeTop: parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--safe-top')),
      safeBottom: parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--safe-bottom')) };
  });
}

async function headerCharacterization(open, width, scheme) {
  const height = width === 320 ? 640 : 844;
  const { page, fx } = await open({ viewport: { width, height }, colorScheme: scheme });
  await openDetail(page);
  await page.emulateMedia({ colorScheme: scheme });
  for (const variant of ['short', 'long']) {
    fx.header = variant;
    await fixturePoll(page, fx);
    await until('default header and idle composer', () => exists(page, '#detail-header-html .agent-header-meta'));
    await until('usage windows', () => exists(page, '#quota .quota-window'));
    await page.emulateMedia({ colorScheme: scheme });
    await nextFrame(page);
    const metrics = await detailGeometry(page);
    check(metrics.width === width && metrics.height === height && metrics.dark === (scheme === 'dark'), 'incorrect characterization viewport/theme');
    console.log(`CHARACTERIZATION ${variant} ${scheme} ${JSON.stringify(metrics)}`);
    fs.mkdirSync(RESULTS_DIR, { recursive: true });
    fs.writeFileSync(path.join(RESULTS_DIR, `baseline-${width}-${scheme}-${variant}.json`), JSON.stringify(metrics, null, 2));
  }
}

async function setDocumentHidden(page, hidden) {
  return page.evaluate((h) => {
    Object.defineProperty(document, 'hidden', { configurable: true, get: () => h });
    Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => (h ? 'hidden' : 'visible') });
    document.dispatchEvent(new Event('visibilitychange'));
    return window.__yinzFetches.filter((f) => f.p.startsWith('/api/')).length;
  }, hidden);
}

// Reuse the app's foreground refresh path, then wait for completed fixture data.
async function refreshFixture(page, fx, pathname) {
  await idle(page);
  const before = fx.count(pathname);
  await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
  await until(`completed ${pathname} fixture refresh`, async () => fx.count(pathname) > before && await page.evaluate(() => window.__yinzOpen === 0));
}

async function fixturePoll(page, fx, pathname = '/api/history') {
  await refreshFixture(page, fx, pathname);
  await nextFrame(page);
}

async function disclosure(page, open, activation = 'click') {
  if (activation === 'click') await page.click('#detail-information-toggle');
  else {
    await page.focus('#detail-information-toggle');
    await page.keyboard.press(activation);
  }
  await until(`agent information ${open ? 'shown' : 'hidden'}`, () => page.evaluate((expected) => {
    const button = document.getElementById('detail-information-toggle');
    const region = document.getElementById(button.getAttribute('aria-controls'));
    return button.getAttribute('aria-expanded') === String(expected) && region.hidden === !expected
      && button.getAttribute('aria-label') === `${expected ? 'Hide' : 'Show'} agent information`;
  }, open));
  await nextFrame(page);
  check(await activeId(page) === 'detail-information-toggle', 'disclosure activation lost focus');
}

async function timelineAnchor(page) {
  return page.evaluate(() => {
    const el = document.getElementById('timeline');
    const box = el.getBoundingClientRect();
    const item = [...el.querySelectorAll('[data-key]')].find((n) => n.getBoundingClientRect().bottom > box.top);
    return { key: item.dataset.key, offset: item.getBoundingClientRect().top - box.top };
  });
}

async function sameAnchor(page, anchor, where) {
  await until(`${where}: timeline anchor restored`, () => page.evaluate(({ key, offset }) => {
    const el = document.getElementById('timeline');
    const item = [...el.querySelectorAll('[data-key]')].find((n) => n.dataset.key === key);
    return item && Math.abs(item.getBoundingClientRect().top - el.getBoundingClientRect().top - offset) <= 2;
  }, anchor));
}

/** Resolve text/background through transparent layers and ancestor opacity. */
async function detailContrast(page, where) {
  const bad = await page.evaluate(() => {
    const rgba = (s) => {
      const n = s.match(/[\d.]+/g).map(Number);
      return [n[0], n[1], n[2], n[3] ?? 1];
    };
    const over = (a, b) => {
      const alpha = a[3] + b[3] * (1 - a[3]);
      return alpha ? [0, 1, 2].map((i) => (a[i] * a[3] + b[i] * b[3] * (1 - a[3])) / alpha).concat(alpha) : [0, 0, 0, 0];
    };
    const paint = (el, ink) => {
      for (let n = el; n; n = n.parentElement) {
        const cs = getComputedStyle(n);
        ink = over(ink, rgba(cs.backgroundColor));
        ink[3] *= Number(cs.opacity);
      }
      return over(ink, [255, 255, 255, 1]);
    };
    const luminance = (rgb) => rgb.slice(0, 3).map((v) => v / 255).map((v) => v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4)
      .reduce((sum, v, i) => sum + v * [0.2126, 0.7152, 0.0722][i], 0);
    const contrast = (a, b) => (Math.max(luminance(a), luminance(b)) + 0.05) / (Math.min(luminance(a), luminance(b)) + 0.05);
    const failures = [];
    const root = document.querySelector('.detail-page');
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    for (let t = walker.nextNode(); t; t = walker.nextNode()) {
      const el = t.parentElement;
      if (!t.textContent.trim() || el.closest('.visually-hidden, .term-host, :disabled') || !el.checkVisibility()) continue;
      const cs = getComputedStyle(el);
      const ratio = contrast(paint(el, rgba(cs.color)), paint(el, [0, 0, 0, 0]));
      if (ratio < 4.5) failures.push(`${el.id || el.className || el.tagName} text ${ratio.toFixed(2)} (${cs.color})`);
    }
    const input = document.getElementById('composer-input');
    if (input && !input.disabled) {
      const cs = getComputedStyle(input);
      if (contrast(paint(input, rgba(cs.color)), paint(input, [0, 0, 0, 0])) < 4.5) failures.push('composer input text');
    }
    const focused = document.activeElement;
    if (root.contains(focused) && focused.matches(':focus-visible')) {
      const cs = getComputedStyle(focused);
      const ratio = contrast(rgba(cs.outlineColor), paint(focused.parentElement, [0, 0, 0, 0]));
      if (parseFloat(cs.outlineWidth) < 3 || ratio < 3) failures.push(`focus outline ${ratio.toFixed(2)}`);
    }
    return failures;
  });
  check(!bad.length, `${where}: contrast failures: ${bad.join(', ')}`);
}

async function compactDetail(open, width, scheme) {
  const height = width === 320 ? 640 : width === 390 ? 844 : 768;
  const { page, fx } = await open({ viewport: { width, height }, colorScheme: scheme });
  await openDetail(page);
  await page.emulateMedia({ colorScheme: scheme });
  for (const variant of ['short', 'long']) {
    fx.header = variant;
    await fixturePoll(page, fx);
    await until('healthy quota and composer', () => exists(page, '#quota .quota-window'));
    const g = await detailGeometry(page);
    check(g.width === width && g.height === height && g.dark === (scheme === 'dark'), 'wrong detail viewport/theme');
    check(g.safeTop === 0 && g.safeBottom === 0, 'budget fixture requires zero emulated safe-area insets');
    check(g.header <= 96 && g.composer <= 64, `detail chrome too tall: ${JSON.stringify(g)}`);
    if (width !== 1024) check(g.timeline >= (width === 320 ? 450 : 650), `timeline budget failed: ${JSON.stringify(g)}`);
    fs.mkdirSync(RESULTS_DIR, { recursive: true });
    fs.writeFileSync(path.join(RESULTS_DIR, `final-${width}-${scheme}-${variant}.json`), JSON.stringify(g, null, 2));
    const baselineFile = path.join(RESULTS_DIR, `baseline-${width}-${scheme}-${variant}.json`);
    console.log(`GEOMETRY ${variant} ${scheme}: final ${JSON.stringify(g)}; baseline ${fs.existsSync(baselineFile) ? fs.readFileSync(baselineFile, 'utf8').replace(/\s+/g, ' ') : 'not recorded in this checkout'}`);
    check(await page.evaluate(() => document.getElementById('detail-information').hidden), 'fresh view was not collapsed');
    await page.focus('#detail-information-toggle');
    await page.keyboard.press('Tab');
    check(await activeId(page) === 'overflow-button', 'collapsed information entered the tab order');
    await disclosure(page, true, 'Enter');
    check(await page.evaluate(() => document.activeElement.matches(':focus-visible') && parseFloat(getComputedStyle(document.activeElement).outlineWidth) >= 3), 'keyboard disclosure focus is invisible');
    const info = await page.evaluate(() => {
      const el = document.getElementById('detail-information');
      return { height: el.clientHeight, text: el.textContent, labelled: el.getAttribute('aria-label'), tab: el.tabIndex };
    });
    for (const k of ['name', 'task', 'project', 'branch', 'location']) check(info.text.includes(headerFields(fx, 'run-7')[k]), `expanded information lost ${k}`);
    check(info.labelled === 'Agent information' && info.tab === 0 && info.height <= Math.min(height * 0.3, 240), 'information region not labelled, keyboard scrollable or bounded');
    await page.focus('#detail-information');
    await page.keyboard.press('End');
    await until('information scrolled to its end', () => page.evaluate(() => {
      const el = document.getElementById('detail-information');
      return el.scrollTop + el.clientHeight >= el.scrollHeight - 2;
    }));
    await page.focus('#detail-information-toggle');
    await page.keyboard.press('Tab');
    await page.keyboard.press('Shift+Tab');
    // Focus the persistent disclosure before a confirmed header refresh.
    await page.focus('#detail-information-toggle');
    await fixturePoll(page, fx);
    check(await activeId(page) === 'detail-information-toggle', 'poll replaced the disclosure/focus');
    for (const tab of ['details', 'term', 'timeline']) {
      await page.click(`#tab-${tab}`);
      await until(`${tab} ready`, () => exists(page, tab === 'term' ? '.xterm-rows' : `#panel-${tab}`));
      check(await page.getAttribute('#detail-information-toggle', 'aria-expanded') === 'true', 'tab change reset disclosure');
    }
    await page.focus('#detail-information-toggle');
    await detailContrast(page, `${width} ${scheme} expanded ${variant}`);
    await disclosure(page, false, 'Space');
    await until('timeline remains pinned after collapse', async () => (await scrollState(page)).atBottom);
    await noHorizontalOverflow(page, `detail ${width} ${scheme} ${variant}`);
    await noClippedControls(page, `detail ${width} ${scheme} ${variant}`);
    await touchTargets(page, `detail ${width} ${scheme} ${variant}`);
    check(await page.evaluate(() => getComputedStyle(document.getElementById('composer-input')).fontSize === '16px'), 'composer input font changed');
    if (width === 1024) check(await page.evaluate(() => getComputedStyle(document.querySelector('.event')).maxWidth === '80%'), 'desktop message width changed');
    fs.mkdirSync(RESULTS_DIR, { recursive: true });
    await page.screenshot({ path: path.join(RESULTS_DIR, `compact-detail-${width}-${scheme}-${variant}.png`), mask: [page.locator('#composer-input')] });
    await detailContrast(page, `${width} ${scheme} collapsed ${variant}`);
  }
  await noInjection(page);
  // Reflow keeps a reader's item/offset rather than interpreting geometry as arrivals.
  await page.evaluate(() => { const el = document.getElementById('timeline'); el.scrollTop = 500; });
  await nextFrame(page);
  let anchor = await timelineAnchor(page);
  await disclosure(page, true);
  await sameAnchor(page, anchor, 'expand');
  await disclosure(page, false);
  await sameAnchor(page, anchor, 'collapse');
  check(!(await exists(page, '#timeline-new')), 'geometry alone announced new messages');
  await page.setViewportSize({ width: width - 20, height });
  await nextFrame(page);
  await sameAnchor(page, anchor, 'width reflow');
  await page.setViewportSize({ width, height });
  await nextFrame(page);
  await sameAnchor(page, anchor, 'width restored');
  await page.fill('#composer-input', 'draft line one\nline two\nline three\nline four');
  await nextFrame(page);
  await sameAnchor(page, anchor, 'multiline draft');
  await page.evaluate(() => document.getElementById('composer-input').setSelectionRange(3, 9));
  await disclosure(page, true);
  check(await page.evaluate(() => { const el = document.getElementById('composer-input'); return el.selectionStart === 3 && el.selectionEnd === 9; }), 'disclosure changed draft selection');
  fx.code = true;
  fx.latest += 1;
  // Trigger data delivery and geometry in the same browser task; neither can
  // overwrite the pre-change reading anchor.
  await page.evaluate(() => { document.dispatchEvent(new Event('visibilitychange')); document.getElementById('detail-information-toggle').click(); });
  await until('simultaneous appended event', () => exists(page, `#timeline [data-key="e${fx.latest}"] code`));
  await nextFrame(page);
  await sameAnchor(page, anchor, 'simultaneous append/collapse');
  await disclosure(page, true);
  await sameAnchor(page, anchor, 'append while disclosed');
  check((await textOf(page, '#timeline code')).includes('long_code_token'.repeat(30)), 'event code was clipped or lost');
  await detailContrast(page, 'event code');
  await page.locator('#timeline-earlier').scrollIntoViewIfNeeded();
  await nextFrame(page);
  anchor = await timelineAnchor(page);
  await page.click('#timeline-earlier');
  await until('earlier messages', () => exists(page, '[data-key="e1"]'));
  await sameAnchor(page, anchor, 'prepend while disclosed');
  await page.locator('.event-read').scrollIntoViewIfNeeded();
  await nextFrame(page);
  anchor = await timelineAnchor(page);
  await page.click('.event-read');
  await sheetReady(page, 'sheet-reader', 'expanded reader focus');
  await detailContrast(page, 'message reader');
  await page.keyboard.press('Escape');
  await sameAnchor(page, anchor, 'reader close');
  await page.locator('.tools-group').scrollIntoViewIfNeeded();
  await nextFrame(page);
  anchor = await timelineAnchor(page);
  await page.click('.tools-group');
  await textUntil(page, '#sheet-reader', 'cargo test', 'tool reader content');
  await detailContrast(page, 'code reader');
  await page.keyboard.press('Escape');
  await sameAnchor(page, anchor, 'tools close');
  if (width !== 1024) {
    await page.setViewportSize({ width, height: 480 });
    await nextFrame(page);
    await sameAnchor(page, anchor, 'reduced height');
    check((await detailGeometry(page)).timeline > 0, 'reduced-height timeline disappeared');
    for (const sel of ['#composer-input', '#composer-send', '#composer-prompts']) {
      await page.locator(sel).scrollIntoViewIfNeeded();
      check(await page.locator(sel).evaluate((el) => { const r = el.getBoundingClientRect(); return r.top >= 0 && r.bottom <= innerHeight; }), `${sel} unreachable at reduced height`);
    }
    await noClippedControls(page, 'reduced height');
    await page.setViewportSize({ width, height });
    await nextFrame(page);
    await sameAnchor(page, anchor, 'restore height');
  }
  check(await valueOf(page, '#composer-input') === 'draft line one\nline two\nline three\nline four', 'resize/disclosure changed draft');
  check(fx.count('/api/agent/prompt') === 0, 'layout changes submitted a prompt');
  await page.click('#tab-term');
  await terminalFits(page);
  await disclosure(page, false);
  await terminalFits(page);
}

async function terminalViewport(open) {
  const { page, fx } = await open({ viewport: { width: 390, height: 640 } });
  fx.termRows = 160;
  await openApp(page, detailHash('run-7', 'term'));
  const scroll = page.locator('.screen-scroll');
  const promptVisible = async (what) => {
    await terminalFits(page);
    await until(what, () => scroll.evaluate((el) => {
      const row = el.querySelector('.xterm-rows > :last-child');
      const rect = row.getBoundingClientRect();
      const viewport = el.getBoundingClientRect();
      return row.textContent.includes('2. No') && rect.height > 0 &&
        rect.top >= Math.max(0, viewport.top) - 1 && rect.bottom <= Math.min(innerHeight, viewport.bottom) + 1 &&
        el.scrollTop > 20 && el.scrollHeight - el.clientHeight - el.scrollTop <= 2;
    }));
  };
  await promptVisible('final row visible on attach');
  await page.setViewportSize({ width: 640, height: 390 });
  await promptVisible('final row visible after rotation');
  await page.setViewportSize({ width: 640, height: 360 });
  await promptVisible('final row visible after height-only resize');

  await scroll.evaluate((el) => { el.scrollTop = 120; });
  await nextFrame(page);
  const readingRow = () => scroll.evaluate((el) => {
    const row = el.querySelector('.xterm-rows > :first-child').getBoundingClientRect();
    return (el.getBoundingClientRect().top - row.top) / row.height;
  });
  const previous = await readingRow();
  await page.setViewportSize({ width: 390, height: 600 });
  await terminalFits(page);
  await until('same row at the top after resizing while reading', async () =>
    Math.abs(await readingRow() - previous) < 0.5);
  await scroll.evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await promptVisible('reader returned to the final row');
  await page.setViewportSize({ width: 390, height: 510 });
  await promptVisible('final row follows a subsequent resize');
}

async function terminalInteraction(open, width, scheme) {
  const { page, fx } = await open({ viewport: { width, height: 640 }, colorScheme: scheme });
  await openApp(page, detailHash('run-7', 'screen'));
  await textUntil(page, '.xterm-rows', '2. No', 'legacy route opens a populated terminal');
  await until('screen reader can read ? Allow edit', () =>
    page.getByRole('listitem').filter({ hasText: '? Allow edit' }).isVisible());
  await terminalFits(page);
  await page.setViewportSize({ width: 640, height: width });
  await terminalFits(page);
  await noHorizontalOverflow(page, 'rotated terminal');
  await page.locator('.xterm-screen').click();
  await page.keyboard.type('abc');
  await page.keyboard.press('ArrowUp');
  await page.keyboard.press('Control+c');
  const textarea = page.locator('.xterm-helper-textarea');
  await textarea.evaluate((el) => {
    // Mobile dictation can insert text without a preceding key event.
    el.setRangeText('voix 🎙');
    el.dispatchEvent(new InputEvent('input', { inputType: 'insertText', data: 'voix 🎙', composed: true }));
    el.dispatchEvent(new CompositionEvent('compositionstart'));
    el.value += '字';
    el.dispatchEvent(new CompositionEvent('compositionend', { data: '字' }));
    el.dispatchEvent(new InputEvent('input', { inputType: 'insertText', data: '字', composed: true }));
  });
  const pasteText = (text) => textarea.evaluate((el, data) => {
    const paste = new ClipboardEvent('paste', { bubbles: true });
    paste.clipboardData.setData('text/plain', data);
    el.dispatchEvent(paste);
  }, text);
  const inputPath = '/api/agent/term/input';
  const keyboardBytes = (body) => typeof body.data === 'string';
  const received = () => Buffer.concat(fx.bodies(inputPath).filter(keyboardBytes).map((body) => Buffer.from(body.data, 'base64')));
  const typed = 'abc\x1bOAvoix 🎙字';
  await until('input-only text and IME commit arrive once', () => received().toString() === typed);
  const typedBodies = fx.bodies(inputPath);
  const control = typedBodies.findIndex((body) => !keyboardBytes(body));
  check(JSON.stringify(typedBodies[control]?.data) === JSON.stringify({ bare_key: { Char: 'c' }, key_modifiers: ['Ctrl'] })
    && typedBodies.filter((body) => !keyboardBytes(body)).length === 1
    && Buffer.concat(typedBodies.slice(0, control).map((body) => Buffer.from(body.data, 'base64'))).toString() === 'abc\x1bOA',
    'Ctrl+C lost its physical identity or arrived out of order');
  await pasteText('貼り付け');
  const expected = typed + '\x1b[200~貼り付け\x1b[201~';
  try {
    await until('ordered keyboard and bracketed paste bytes', () => received().toString() === expected);
  } catch (error) {
    throw new Error(`${error.message}; fixture received ${received().toString('hex')}, focused ${await page.evaluate(() => document.activeElement.className)}`);
  }
  const readPath = '/api/agent/term';
  const quickKey = page.locator('.screen-keys .quick-key').first();
  const inputText = (body) => keyboardBytes(body) ? Buffer.from(body.data, 'base64').toString() : null;
  // Twelve framing bytes plus the UTF-8 payload fill one guarded write.
  const atLimit = 'p'.repeat(16370) + 'é';
  const framed = '\x1b[200~' + atLimit + '\x1b[201~';
  await pasteText(atLimit + 'x');
  await textUntil(page, '.screen-feedback', '16 KiB', 'oversized paste rejected locally');
  check(await quickKey.isEnabled(), 'size warning disabled the terminal');
  const beforePaste = fx.count(inputPath);
  fx.fail.set(inputPath, { mode: 'hold', times: 1 });
  await pasteText(atLimit);
  await until('complete paste in flight', () => fx.held.length === 1);
  check(inputText(fx.bodies(inputPath)[beforePaste]) === framed, 'paste boundaries or size changed');
  // Hold queued typing across a failed read and multipart replacement.
  await page.keyboard.type('q');
  fx.fail.set(readPath, { mode: 'abort', times: Infinity });
  await until('failed read pauses typing', () => quickKey.isDisabled());
  await fx.held.shift().fulfill({ status: 200, json: { response: { outcome: 'ok' } } });
  await page.keyboard.type('ignored');
  fx.fail.set(readPath, { mode: 'hold', times: 3 });
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  const part = async (frame) => {
    await until('held replay read', () => fx.held.length === 1);
    return fx.held.shift().fulfill({ status: 200, json: { response: { outcome: 'ok',
      terminal: { cols: 120, rows: 24, ...frame },
    } } });
  };
  await part({ snapshot: true, snapshot_continues: true, cursor: 'part:1', data: Buffer.from('\x1b[?2004h').toString('base64') });
  await part({ snapshot: false, cursor: 'capture' });
  await until('live read outstanding', () => fx.held.length === 1);
  check(await quickKey.isDisabled(), 'queued typing released at capture offset');
  check(fx.count(inputPath) === beforePaste + 1, 'queued typing reached stale dialog');
  await part({ snapshot: false, cursor: '1-40', data: Buffer.from('Current dialog').toString('base64') });
  await until('live poll releases queued typing once', () => received().toString() === expected + framed + 'q');
  await page.setViewportSize({ width, height: 640 });
  await terminalFits(page, 'Current dialog');
  fx.termMouse = true;
  await textUntil(page, '.xterm-rows', 'Mouse mode', 'alternate-screen mouse application');
  await until('screen reader can read Mouse mode', () =>
    page.getByRole('listitem').filter({ hasText: 'Mouse mode' }).isVisible());
  const grid = page.locator('.xterm-screen');
  await grid.scrollIntoViewIfNeeded();
  const box = await grid.boundingBox();
  const point = (column) => box.x + box.width * (column - 0.5) / 120;
  const y = box.y + box.height * 2.5 / 24;
  const beforeMouse = fx.count(inputPath);
  await page.mouse.move(point(99), y);
  await page.mouse.down();
  await page.mouse.move(point(100), y);
  await page.mouse.up();
  const mouseBytes = (start) => Buffer.concat(fx.bodies(inputPath).slice(start).map(({ data }) => Buffer.from(data.mouse, 'base64')));
  await until('SGR press, drag and release at scaled coordinates', () =>
    mouseBytes(beforeMouse).toString() === '\x1b[<0;99;3M\x1b[<32;100;3M\x1b[<0;100;3m');
  fx.fail.set(inputPath, { mode: 'abort', times: 1, matches: (entry) => inputText(entry.body) === framed });
  await pasteText(atLimit);
  await textUntil(page, '.screen-feedback', 'Input stopped', 'uncertain terminal input stops delivery');
  const reads = fx.count(readPath);
  await until('polling continues after the failed write', () => fx.count(readPath) >= reads + 2);
  check(await quickKey.isDisabled(), 'successful poll enabled input after a failed write');
  const writes = fx.count(inputPath);
  await page.keyboard.type('discarded');
  await page.getByRole('button', { name: 'Reconnect' }).click();
  await until('reconnected to the same pane', () => page.locator('.screen-keys .quick-key').first().isEnabled());
  await page.locator('.xterm-helper-textarea').focus();
  await page.keyboard.type('z');
  await until('fresh input after reconnect', () => fx.count('/api/agent/term/input') === writes + 1);
  check(received().toString().endsWith(framed + 'z'), `unexpected bytes after reconnect: ${received().subarray(-32).toString('hex')}`);

  const beforeKeys = fx.count(inputPath);
  await page.keyboard.type('a');
  await page.keyboard.press('Shift+Enter');
  await page.keyboard.press('Control+Tab');
  await page.keyboard.press('Enter');
  await page.keyboard.type('b');
  const ordered = [
    'YQ==', { bare_key: 'Enter', key_modifiers: ['Shift'] },
    { bare_key: 'Tab', key_modifiers: ['Ctrl'] }, 'DQ==', 'Yg==',
  ];
  await until('Kitty modified keys preserve identity and queue order', () => {
    const delivered = fx.bodies(inputPath).slice(beforeKeys).map((body) => body.data);
    return JSON.stringify(delivered) === JSON.stringify(ordered);
  });

  fx.termMouse = 'legacy';
  await textUntil(page, '.xterm-rows', 'Legacy mouse', 'legacy encoding with unbracketed paste');
  fx.fail.set(inputPath, { mode: 'hold', times: 1 });
  await page.keyboard.type('h');
  await until('keyboard write in flight before mouse and paste', () => fx.held.length === 1);
  const beforeMixed = fx.count(inputPath);
  await page.mouse.click(point(1), y);
  const legacyPress = '\x1b[M !#';
  const legacyClick = legacyPress + '\x1b[M#!#';
  await pasteText(legacyPress);
  await page.mouse.click(point(100), y); // column 100 encodes as a single non-UTF-8 byte
  await page.keyboard.press('Enter');
  await page.keyboard.type('k');
  await fx.held.shift().fulfill({ status: 200, json: { response: { outcome: 'ok' } } });
  const encode = (s) => Buffer.from(s, 'latin1').toString('base64');
  const mixed = [{ mouse: encode(legacyClick) }, encode(legacyPress),
    { mouse: encode('\x1b[M \x84#\x1b[M#\x84#') }, 'DQ==', 'aw=='];
  await until('legacy mouse, identical keyboard paste, mouse, Enter and typing retain order', () =>
    JSON.stringify(fx.bodies(inputPath).slice(beforeMixed).map((body) => body.data)) === JSON.stringify(mixed));

  fx.termMouse = false;
  await textUntil(page, '.xterm-rows', 'Legacy mode', 'pane disabled Kitty keyboard mode');
  const beforeShiftEnter = fx.count(inputPath);
  await page.keyboard.press('Shift+Enter');
  await until('Shift+Enter has the same physical identity after a legacy frame', () =>
    JSON.stringify(fx.bodies(inputPath)[beforeShiftEnter]?.data) === JSON.stringify(ordered[1]));

  fx.fail.set(inputPath, { mode: 'hold', times: 1 });
  await pasteText(atLimit);
  await until('paste request before navigation', () => fx.held.length === 1);
  check(inputText(fx.bodies(inputPath).at(-1)) === framed, 'navigation can discard a paste suffix');
  await page.click('#tab-details');
  await fx.held.shift().abort();
}

async function detailSafety(open, width, scheme) {
  const { page, fx } = await open({ viewport: { width, height: 480 }, colorScheme: scheme });
  fx.header = 'long';
  await openDetail(page);
  await page.emulateMedia({ colorScheme: scheme });
  await disclosure(page, true);
  await page.fill('#composer-input', 'Saved draft\nsecond line\nthird line\nfourth line');
  fx.fail.set('/api/agent/prompt', { mode: 'abort', times: 1 });
  await page.click('#composer-send');
  await textUntil(page, '#submission-unknown', 'Outcome unknown', 'saved submission feedback');
  // An in-flight poll can consume a one-shot failure before fixturePoll refreshes.
  // Keep the outage active through inspection, then explicitly recover below.
  fx.fail.set('/api/history', { mode: 'abort', times: Infinity });
  await fixturePoll(page, fx);
  await textUntil(page, '#banner-offline', 'Offline', 'disclosed offline feedback');
  await fixturePoll(page, fx);
  await textUntil(page, '#banner-offline', 'Offline', 'disclosed offline feedback survives another poll');
  await textUntil(page, '#submission-unknown', 'Outcome unknown', 'saved submission survives offline polls');
  const reachable = async (selectors) => {
    check((await detailGeometry(page)).timeline > 0, 'safety feedback removed the timeline');
    for (const sel of selectors) {
      await page.locator(sel).scrollIntoViewIfNeeded();
      check(await page.locator(sel).evaluate((el) => {
        const r = el.getBoundingClientRect();
        let top = 0, bottom = innerHeight;
        for (let p = el.parentElement; p; p = p.parentElement) {
          if (['auto', 'scroll', 'hidden'].includes(getComputedStyle(p).overflowY)) {
            const b = p.getBoundingClientRect(); top = Math.max(top, b.top); bottom = Math.min(bottom, b.bottom);
          }
        }
        return r.top >= top - 1 && r.bottom <= bottom + 1;
      }), `${sel} clipped by the reduced-height layout`);
    }
  };
  await reachable(['#banner-offline .link-button', '#submission-retry', '#submission-discard', '#composer-input', '#composer-send', '#composer-prompts']);
  await detailContrast(page, `unknown/offline ${width} ${scheme}`);
  await touchTargets(page, 'safety feedback');
  await noClippedControls(page, 'safety feedback');
  await fixturePoll(page, fx);
  await textUntil(page, '#banner-offline', 'Offline', 'offline feedback remains after safety inspection');
  await textUntil(page, '#submission-unknown', 'Outcome unknown', 'saved submission remains after safety inspection');
  check(fx.count('/api/agent/prompt') === 1, 'disclosure/safety layout replayed a prompt');
  fx.fail.delete('/api/history');
  await fixturePoll(page, fx);
  await until('disclosed offline recovery', async () => !(await exists(page, '#banner-offline')));
  await textUntil(page, '#submission-unknown', 'Outcome unknown', 'saved submission survives history recovery');
  check(fx.count('/api/agent/prompt') === 1, 'history recovery replayed a prompt');
  await page.locator('#submission-retry').scrollIntoViewIfNeeded();
  await page.click('#submission-retry');
  await until('same-id retry resolved', async () => !(await exists(page, '#submission-unknown')));
  const [first, retry] = fx.bodies('/api/agent/prompt');
  check(first.submission_id === retry.submission_id && first.text === retry.text, 'safety retry changed identity');
  fx.prompt = 'rejected';
  await page.fill('#composer-input', 'Rejected prompt');
  await page.click('#composer-send');
  await textUntil(page, '#submission-failed', 'Refused', 'definite error feedback');
  await reachable(['#submission-failed .link-button']);
  await detailContrast(page, `failed ${scheme}`);
  await page.click('#submission-failed .link-button');
  await page.click('#composer-prompts');
  await textUntil(page, '#sheet-prompts', 'Run the tests', 'past prompts sheet');
  await detailContrast(page, `past prompts ${scheme}`);
  await page.keyboard.press('Escape');
  await page.click('#overflow-button');
  await sheetReady(page, 'sheet-overflow', 'actions sheet focus');
  await detailContrast(page, `actions ${scheme}`);
  await page.click('#action-interrupt');
  await sheetReady(page, 'sheet-interrupt-confirm', 'interrupt confirmation');
  await detailContrast(page, `danger confirmation ${scheme}`);
  await page.keyboard.press('Escape');
  // Exercise the actual persistence warning with a denied browser storage write.
  await page.evaluate(() => {
    window.__yinzOriginalSetItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function (key, value) {
      if (key.includes('submission')) throw new DOMException('Fixture quota exceeded', 'QuotaExceededError');
      return window.__yinzOriginalSetItem.call(this, key, value);
    };
  });
  await page.fill('#composer-input', 'Persistence warning draft');
  await textUntil(page, '#submission-persistence', 'not saving drafts', 'persistence feedback');
  await detailContrast(page, `persistence ${scheme}`);
  await reachable(['#composer-input', '#composer-prompts']);
  await page.evaluate(() => { Storage.prototype.setItem = window.__yinzOriginalSetItem; });
  await page.fill('#composer-input', 'Recovered persistent draft');
  fx.retarget = true;
  await fixturePoll(page, fx);
  await textUntil(page, '#banner-retargeted', 'new run', 'retarget feedback');
  await textUntil(page, '.stale-submissions', /new run|earlier conversation|identity changed/, 'stale draft feedback');
  await reachable(['.submission-adopt', '.submission-discard-stale', '#composer-input']);
  await detailContrast(page, `stale submissions ${scheme}`);
}

async function detailFallbacks(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 480 } });
  await openDetail(page);
  await disclosure(page, true);
  for (const variant of ['wrapped', 'custom', 'absent']) {
    fx.header = variant;
    await fixturePoll(page, fx);
    check(await page.getAttribute('#detail-information-toggle', 'aria-expanded') === 'true', 'header variant reset disclosure');
    if (variant === 'absent') check((await textOf(page, '#detail-information h2')).includes('Fix flaky test'), 'missing header lost full title fallback');
    else {
      await page.focus('#detail-information');
      await page.keyboard.press('End');
      await page.locator('#custom-information').scrollIntoViewIfNeeded();
      check(await page.locator('#custom-information').isVisible(), 'custom hook control unreachable');
      await disclosure(page, false);
      await page.focus('#detail-information-toggle');
      await page.keyboard.press('Tab');
      check(await activeId(page) === 'overflow-button', 'collapsed custom link/action focusable');
      await disclosure(page, true);
    }
    await noHorizontalOverflow(page, variant);
  }
  for (const state of ['busy', 'idle', 'waiting']) {
    fx.live = { state, wait_needs_a_person: state === 'waiting' };
    await fixturePoll(page, fx);
    await until('live labelled state', () => exists(page, `.detail-summary .state-${state}`));
    await detailContrast(page, `state ${state}`);
  }
  fx.live = { title: '', state: 'paused', host: 'remote host', current_session: false, session: 'peer', focused: true };
  await fixturePoll(page, fx);
  check((await textOf(page, '.detail-summary .state-text')) === 'paused', 'unknown state lost label');
  check((await textOf(page, '.detail-summary .header-title')) === headerFields(fx, 'run-7').name, 'header name fallback lost');
  for (const sel of ['.badge-host', '.badge-peer', '.badge-focused']) check(await page.locator(`#detail-information ${sel}`).isVisible(), `secondary badge missing: ${sel}`);
  for (const status of ['unavailable', 'unsupported', 'expired', 'error', 'ok']) {
    fx.usage = { status, windows: [], message: 'Fixture usage explanation' };
    await fixturePoll(page, fx, '/api/usage');
    check((await textOf(page, '#quota')).includes(status === 'ok' ? 'No usage windows' : status === 'unsupported' ? 'not supported' : status === 'expired' ? 'expired' : status === 'error' ? 'could not be read' : 'unavailable'), `usage ${status} feedback missing`);
  }
  // Usage transport errors are also discoverable inside the disclosure.
  fx.fail.set('/api/usage', { mode: 'reply', times: 1, status: 503, body: { error: { message: 'Usage transport error' } } });
  await page.reload();
  await until('reloaded summary', () => exists(page, '#detail-information-toggle'));
  check(await page.getAttribute('#detail-information-toggle', 'aria-expanded') === 'false', 'reload retained disclosure');
  await disclosure(page, true);
  await textUntil(page, '#quota', 'Usage transport error', 'usage HTTP error');
  fx.gone = true;
  await fixturePoll(page, fx);
  await textUntil(page, '#banner-agent-gone', 'gone', 'gone agent banner');
  check(!(await exists(page, '#overflow-button')), 'gone agent retained actions');
  await page.click('#tab-details');
  await page.click('#details-previous');
  await textUntil(page, '#banner-stored', 'stored', 'stored run');
  check(await page.getAttribute('#detail-information-toggle', 'aria-expanded') === 'false', 'new stored view retained disclosure');
  await disclosure(page, true);
  check(!(await exists(page, '#composer-input')), 'stored run offered composer');
  await page.evaluate(() => { location.hash = '#/'; });
  await until('list after stored view', () => exists(page, '.list-main'));
  fx.gone = false;
  fx.rows = [row(fx, AGENT_ID, 'waiting', { run: '', title: 'No recorded run' })];
  await page.evaluate((h) => { location.hash = h; }, `#/a/${encodeURIComponent(AGENT_ID)}`);
  await until('waiting no-run screen', () => exists(page, '.xterm-rows'));
  check(await page.getAttribute('#detail-information-toggle', 'aria-expanded') === 'false', 'no-run view not collapsed');
  await disclosure(page, true);
  check(await textOf(page, '#detail-information h2') === 'No recorded run', 'no-run full title missing');
  check(await exists(page, '#composer-input'), 'no-run live composer missing');
}

async function cachedAgentList(open) {
  const { page, fx } = await open({ viewport: { width: 375, height: 480 } });
  const sparse = { ...row(fx, 'live:dev:8', 'idle'), html: '' };
  fx.rows = [row(fx, AGENT_ID, 'waiting'), sparse];
  await openList(page);
  const link = page.locator(`${cardSelector(sparse.id)} .agent-open-area`);
  const height = await link.evaluate((el) => el.getBoundingClientRect().height);
  check(height >= 56, `empty agent-row hook left a ${height}px Open link`);

  const message = FILLER.repeat(3).slice(0, 300);
  fx.fail.set('/api/agents', { mode: 'reply', times: Infinity, status: 503,
    body: { error: { code: 'busy', message } } });
  await textUntil(page, '#list-freshness', message, 'long retry message after list poll');
  const status = await page.locator('#list-header .header-status').boundingBox();
  for (const id of ['#list-refresh', '#list-next-waiting']) {
    const action = await page.locator(id).boundingBox();
    check(action && action.y >= status.y + status.height, `${id} crowds the stale status`);
  }
  const href = await link.getAttribute('href');
  await link.click();
  await until('cached card opens', () => hashOf(page) === href);
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
  const reference = cardSelector('live:dev:8');
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
      header: document.getElementById('list-header').getBoundingClientRect().height,
      height: card.getBoundingClientRect().height, replyHeight: reply.getBoundingClientRect().height,
      lineHeight: parseFloat(rs.lineHeight), titleSize: ts.fontSize, replySize: rs.fontSize,
      titleColour: ts.color, replyColour: rs.color, titleContrast: contrast(ts.color, cs.backgroundColor),
      replyContrast: contrast(rs.color, cs.backgroundColor), text: reply.textContent,
      clamp: rs.webkitLineClamp,
    };
  }, reference);
  if (width >= 390) check(metrics.header <= 96, `Agents header too tall: ${metrics.header}px at ${width}px`);
  check(metrics.titleColour !== metrics.replyColour, 'task and response colours are identical');
  check(metrics.titleContrast >= 4.5 && metrics.replyContrast >= 4.5, `low text contrast: ${JSON.stringify(metrics)}`);
  check(metrics.titleSize === '16px' && metrics.replySize === '14px', 'unexpected title/reply typography');
  check(metrics.clamp === '3' && metrics.replyHeight <= metrics.lineHeight * 3 + 1, `reply exceeds three lines: ${JSON.stringify(metrics)}`);
  check(metrics.text === `${longReply.slice(0, 160)}…`, 'long preview lost its character cap or ellipsis cue');
  check(metrics.height < (width === 320 ? 240 : 220), `reference card too tall: ${metrics.height}px at ${width}px`);
  check(await textOf(page, `${cardSelector(AGENT_ID)} .agent-reply`) === 'Ready for your review.', 'short reply changed');
  check(JSON.stringify(await page.locator('.list-main .state-text:visible').allTextContents()) === JSON.stringify(['Needs you', 'Working']), 'card state badges missing or reordered');
  check(await page.getByRole('link', { name: 'Open Review the change, Needs you', exact: true }).count() === 1, 'card link label omits the agent state');
  check(await page.locator('.list-main .agent-status:visible').count() === 0, 'duplicate default status visible through a wrapper');
  check(await page.evaluate(() => getComputedStyle(document.querySelector('.card-needs-person')).borderLeftColor !== getComputedStyle(document.querySelector('.card-busy')).borderLeftColor), 'needs-person emphasis lost');
  await noHorizontalOverflow(page, `compact list ${width}px ${scheme}`);
  await noClippedControls(page, `compact list ${width}px ${scheme}`);
  await touchTargets(page, `compact list ${width}px ${scheme}`);
  fs.mkdirSync(RESULTS_DIR, { recursive: true });
  await page.screenshot({ path: path.join(RESULTS_DIR, `compact-list-${width}-${scheme}.png`), fullPage: true });
  console.log(`compact list ${width}px ${scheme}: card ${metrics.height}px; reply ${metrics.replyHeight}px; title/reply contrast ${metrics.titleContrast.toFixed(2)}/${metrics.replyContrast.toFixed(2)}`);

  await switchGroup(page, 'peer', 0);
  check(await textOf(page, `${cardSelector('live:peer:9')} .agent-reply`) === 'Upstream preview…', 'upstream truncation cue missing');
  check(await textOf(page, '.list-main .state-text') === 'Idle', 'peer card state badge missing');
  for (const sel of ['.custom-row .badge', '.badge-focused', '.badge-peer', '.badge-host', '.agent-glyph']) check(await exists(page, sel), `missing context ${sel}`);
  check((await textOf(page, '.custom-row .badges')).includes('<custom context>'), 'custom badge was not preserved as text');
  await switchGroup(page, 'browser-check', 0);

  // Keyboard focus must remain visible and Enter must activate the card itself.
  await page.focus('#list-refresh');
  await page.keyboard.press('Tab');
  check(await activeId(page) === 'list-next-waiting', 'waiting shortcut missing from tab order');
  await page.keyboard.press('Tab');
  check(await activeId(page) === 'start-agent', 'Start agent missing from tab order');
  await page.keyboard.press('Tab');
  check(await page.evaluate(() => document.activeElement.matches('.agent-open-area') && parseFloat(getComputedStyle(document.activeElement).outlineWidth) >= 2), 'card keyboard focus indication missing');
  await page.keyboard.press('Enter');
  await until('card opens waiting screen', async () => hashOf(page) === detailHash('run-7', 'term') && exists(page, '.xterm-rows'));
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
  await until('return to list', () => exists(page, cardSelector(AGENT_ID)));

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
  await until('edge-case rows', () => exists(page, cardSelector('live:dev:14')));
  const edge = await page.evaluate(() => {
    const card = document.querySelector('.agent-card[data-agent-id="live:dev:10"]');
    const title = card.querySelector('.agent-title');
    const meta = card.querySelector('.agent-meta');
    return {
      title: title.textContent, titleWrapped: title.getBoundingClientRect().height > parseFloat(getComputedStyle(title).lineHeight) * 2,
      titleUnclipped: title.scrollHeight <= title.clientHeight + 1,
      metaHidden: getComputedStyle(meta).display === 'none', reply: card.querySelector('.agent-reply').textContent,
      replies: document.querySelectorAll('.list-main .agent-reply').length,
      longMeta: document.querySelector('.card-waiting .agent-meta').textContent,
    };
  });
  check(edge.title === '界🙂'.repeat(100) && edge.titleWrapped && edge.titleUnclipped, 'long Unicode title lost or clipped');
  check(edge.metaHidden && edge.replies === 1, 'empty metadata/reply occupied a row');
  check(edge.reply === `${'界🙂'.repeat(80)}…`, 'Unicode reply preview damaged');
  check(edge.longMeta.includes('project'.repeat(30)) && edge.longMeta.includes('branch'.repeat(30)), 'long metadata lost');
  check(await page.locator(`${cardSelector('live:dev:14')} .agent-status`).isVisible() && (await textOf(page, `${cardSelector('live:dev:14')} .state-badge`)).includes('paused'), 'unknown state label missing');
  await noHorizontalOverflow(page, `long content ${width}px ${scheme}`);
  await noClippedControls(page, `long content ${width}px ${scheme}`);
  await touchTargets(page, `long content ${width}px ${scheme}`);
  await noInjection(page);

  // A successful zero-agent response is distinct from network errors and empty replies.
  fx.rows = [];
  await domClick(page, '#list-refresh');
  await until('successful empty list', () => exists(page, '#list-empty'));
  check(await textOf(page, '#list-empty') === 'No agents in this group.', 'empty group explanation missing');
  check(!(await exists(page, '.agent-section')) && !(await exists(page, '#banner-offline')), 'empty list retained cards or an error');
  fx.rows = null;
  await domClick(page, '#list-refresh');
  await until('populated list recovers', () => exists(page, cardSelector(AGENT_ID)));
}

async function groupedAgentList(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const name = 'Release Review 7';
  const longName = `Peer ${XSS(3)} ${'release'.repeat(40)}`;
  const first = row(fx, AGENT_ID, 'waiting', { tab_group: { id: 0, name } });
  const busy = row(fx, 'live:dev:8', 'busy', {
    tab_group: { id: 0, name }, status: 'Checking the build', wrapper: true,
  });
  const second = row(fx, 'live:dev:9', 'idle', { tab_group: { id: 1, name } });
  const secondBusy = row(fx, 'live:dev:15', 'busy', { tab_group: { id: 1, name } });
  const peer = row(fx, 'live:peer:7', 'idle', {
    session: 'peer', current_session: false, tab_group: { id: 0, name },
  });
  const peerLong = row(fx, 'live:peer:8', 'idle', {
    session: 'peer', current_session: false, tab_group: { id: 0xffffffff, name: longName },
  });
  const missing = row(fx, 'live:dev:10', 'idle', { tab_group: null });
  const invalid = row(fx, 'live:dev:11', 'idle', { tab_group: { id: 0x100000000, name: 'Invalid' } });
  const fallbackName = row(fx, 'live:dev:16', 'idle', { tab_group: { id: 3, name: 'Tab group unavailable' } });
  const peerMissing = row(fx, 'live:peer:10', 'idle', {
    session: 'peer', tab_group_session: ' ', current_session: false, tab_group: undefined,
  });
  const ownerless = row(fx, 'live:unknown:12', 'idle', { session: undefined, tab_group: { id: 0, name } });
  const alsoOwnerless = row(fx, 'live:unknown:13', 'idle', { session: ' ', tab_group: { id: 0, name } });
  const unavailable = row(fx, 'live:unavailable:12', 'idle', { session: 'unavailable', current_session: false, tab_group: null });
  const alsoUnavailable = row(fx, 'live:unavailable:13', 'idle', { session: 'unavailable', current_session: false, tab_group: null });
  const unnamed = row(fx, 'live:dev:14', 'idle', { tab_group: { id: 2, name: ' ' } });
  second.html = second.html.replace(' data-redundant-state="idle"', '').replace('>idle</span>', '>Awaiting a deploy</span>');
  fx.rows = [peer, first, second, busy, secondBusy, peerLong, missing, fallbackName, peerMissing,
    ownerless, invalid, alsoOwnerless, unnamed, unavailable, alsoUnavailable];
  fx.counts = { waiting: 4, busy: 2, idle: 10 };
  await openList(page);
  const home = groupRoute('browser-check', 0);
  check(hashOf(page) === home, 'Agents home did not choose the serving session default');
  const sections = () => page.locator('.agent-section').evaluateAll((nodes) => nodes.map((node) => {
    const heading = document.getElementById(node.getAttribute('aria-labelledby'));
    if (!heading || heading !== node.querySelector('h2')) throw new Error('invalid group heading association');
    return [
      heading.querySelector('.section-name').textContent,
      heading.querySelector('.section-session').textContent,
      [...node.querySelectorAll('.agent-card')].map((card) => card.dataset.agentId),
    ];
  }));
  const pages = [
    [['peer', 0], [name, 'Session peer', [peer.id]]],
    [['browser-check', 0], [name, 'Session browser-check · Group 0', [first.id, busy.id]]],
    [['browser-check', 1], [name, 'Session browser-check · Group 1', [second.id, secondBusy.id]]],
    [['peer', 0xffffffff], [longName, 'Session peer', [peerLong.id]]],
    [['browser-check', null], ['Tab group unavailable', 'Session browser-check', [missing.id, invalid.id]]],
    [['browser-check', 3], ['Tab group unavailable', 'Session browser-check · Group 3', [fallbackName.id]]],
    [['peer', null], ['Tab group unavailable', 'Session peer', [peerMissing.id]]],
    [[null, null], ['Tab group unavailable', 'Owning session unknown', [ownerless.id, alsoOwnerless.id]]],
    [['browser-check', 2], ['Unnamed tab group', 'Session browser-check', [unnamed.id]]],
    [['unavailable', null], ['Tab group unavailable', 'Session unavailable', [unavailable.id, alsoUnavailable.id]]],
  ];
  await page.focus('#switch-group');
  await page.keyboard.press('Enter');
  await sheetReady(page, 'sheet-groups', 'group picker holds focus');
  const links = page.locator('#sheet-groups .group-link');
  const hashes = await links.evaluateAll((nodes) => nodes.map((link) => link.hash));
  check(JSON.stringify(hashes) === JSON.stringify(pages.map(([key]) => groupRoute(...key))), 'picker order or group identities are incorrect');
  check(await page.locator('#sheet-groups [aria-current="page"]').getAttribute('href') === home, 'picker omitted current page');
  await noHorizontalOverflow(page, 'group picker at 320px');
  await touchTargets(page, 'group picker at 320px');
  await page.keyboard.press('Escape');
  await until('picker closes and restores focus', async () => await activeId(page) === 'switch-group');
  check(await page.locator(`${cardSelector(busy.id)} .agent-status`).isVisible(), 'hook-supplied status hidden');
  for (const [key, expected] of pages) {
    await switchGroup(page, ...key);
    check(JSON.stringify(await sections()) === JSON.stringify([expected]), 'incorrect group page membership');
    const [title, owner, rows] = expected;
    const label = `${title} · ${rows.length} ${rows.length === 1 ? 'agent' : 'agents'} ${owner}`;
    check(await page.getByRole('heading', { name: label, exact: true }).count() === 1, `wrong accessible heading: ${label}`);
    check(await page.getByRole('region', { name: label, exact: true }).count() === 1, `wrong accessible region: ${label}`);
    await noHorizontalOverflow(page, 'tab group labels at 320px');
  }
  await switchGroup(page, 'browser-check', 0);
  await switchGroup(page, 'browser-check', 1);
  const secondHash = groupRoute('browser-check', 1);
  await page.evaluate(() => history.back());
  await until('Back restores the default group', async () => hashOf(page) === home && await exists(page, cardSelector(first.id)));
  await page.evaluate(() => history.forward());
  await until('Forward restores the second group', async () => hashOf(page) === secondHash && await exists(page, cardSelector(second.id)));
  await page.reload();
  await until('reload restores the second group', async () => hashOf(page) === secondHash && await exists(page, cardSelector(second.id)));
  check(await page.locator('.section-title').first().evaluate((el) => getComputedStyle(el).textTransform) === 'none', 'group name case changed');
  check(await textOf(page, '.need-count') === '4 need you' && (await page.title()).startsWith('(4) '), 'global attention was recomputed from visible rows');
  check(await page.locator(`${cardSelector(second.id)} .agent-status`).isVisible(), 'custom markup status hidden');
  await noInjection(page);

  const renamedGroup = { id: 0, name: 'Review Complete' };
  const local = [first, second, busy, secondBusy];
  fx.rows = [peer, ...local.map((r) => ({ ...r, tab_group: renamedGroup })), peerLong];
  await domClick(page, '#list-refresh');
  await textUntil(page, '#list-empty', 'No agents in this group.', 'selected group disappears');
  check(hashOf(page) === secondHash, 'an empty group redirected');
  const restored = row(fx, 'live:dev:19', 'idle', { tab_group: second.tab_group });
  fx.rows = [...fx.rows, restored];
  await until('selected group recovers', () => exists(page, cardSelector(restored.id)));
  check(hashOf(page) === secondHash, 'group recovery changed the URL');
  const moved = [renamedGroup.name, 'Session browser-check', local.map((r) => r.id)];
  await switchGroup(page, 'browser-check', 0);
  check(JSON.stringify(await sections()) === JSON.stringify([moved]), 'rename or group move did not update the default page');
  check(hashOf(page) === home, 'rename changed the default group URL');
}

async function groupHistoryScroll(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const session = 'browser-check';
  const groups = ['First', 'Second', 'Short'].map((name, id) => ({ id, name }));
  const cards = groups.map((tab_group, index) =>
    Array.from({ length: index === 2 ? 1 : 16 }, (_, i) =>
      row(fx, `live:dev:${30 + index * 20 + i}`, 'idle', { tab_group })));
  fx.rows = cards.flat();
  await openList(page);

  function expectOffset(id, top, description) {
    return until(description, async () =>
      hashOf(page) === groupRoute(session, id) &&
      await exists(page, cardSelector(cards[id][0].id)) &&
      await page.evaluate((top) => Math.abs(window.scrollY - top) <= 1, top));
  }
  async function scrollGroup(id, top) {
    await page.evaluate((top) => window.scrollTo({ top, behavior: 'instant' }), top);
    await expectOffset(id, top, 'the fixture scrolls to the requested offset');
  }
  async function traverse(delta, id, top, description) {
    await page.evaluate((delta) => history.go(delta), delta);
    await expectOffset(id, top, description);
  }

  await expectOffset(0, 0, 'Agents opens at the top of the first group');
  await scrollGroup(0, 500);
  await switchGroup(page, session, 1);
  await expectOffset(1, 0, 'a new selection opens at the top of another long group');
  await scrollGroup(1, 1000);
  await switchGroup(page, session, 2);
  await expectOffset(2, 0, 'a new selection opens at the top of a short group');
  await traverse(-1, 1, 1000, 'Back restores the second group from a short page');
  await traverse(-1, 0, 500, 'Back restores the first group');
  await traverse(1, 1, 1000, 'Forward restores the second group');
  await traverse(1, 2, 0, 'Forward restores the short group');

  await switchGroup(page, session, 1);
  await expectOffset(1, 0, 'a new entry for a visited group starts at the top');
  await scrollGroup(1, 650);
  await traverse(-2, 1, 1000, 'a history jump to the same group restores the older entry');
  await traverse(2, 1, 650, 'a history jump forward restores the newer entry');
}

async function groupDialogFocus(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const first = row(fx, AGENT_ID, 'idle', { tab_group: { id: 0, name: 'First' } });
  const second = row(fx, 'live:dev:8', 'idle', { tab_group: { id: 1, name: 'Second' } });
  const third = row(fx, 'live:dev:9', 'idle', { tab_group: { id: 2, name: 'Third' } });
  fx.rows = [first, third, second];
  await openList(page);

  const picker = '#sheet-groups';
  const links = page.locator(`${picker} .group-link`);
  const route = (id) => groupRoute('browser-check', id);
  const groupLink = (id) => page.locator(`${picker} a[href="${route(id)}"]`);
  const isFocused = (control) => control.evaluateAll(([el]) => el === document.activeElement);
  const waitForGroup = (agent) => until(`group ${agent.tab_group.name} is rendered`, async () =>
    hashOf(page) === route(agent.tab_group.id) && await exists(page, cardSelector(agent.id)));

  await page.click('#switch-group');
  check(await links.last().getAttribute('href') === route(1), 'the second group must start last in the picker');
  await groupLink(1).focus();
  fx.rows = [second, first, third];
  await until('the poll moves the focused group to first', async () =>
    await links.first().getAttribute('href') === route(1));
  check(await isFocused(groupLink(1)), 'the poll dropped focus from the reordered group link');
  await page.keyboard.press('Enter');
  await waitForGroup(second);
  check(!(await exists(page, picker)) && await activeId(page) === 'main', 'selecting a group did not close the picker and focus the page');

  await page.focus('#switch-group');
  await page.keyboard.press('Enter');
  await sheetReady(page, 'sheet-groups', 'the picker opens from the keyboard');
  for (const id of [1, 0, 2]) {
    await page.keyboard.press('Tab');
    check(await isFocused(groupLink(id)), 'Tab did not reach the next group link');
  }
  const focusedGeometry = () => groupLink(2).evaluate((link) => {
    const body = link.closest('.sheet-body');
    const rect = link.getBoundingClientRect();
    const top = body.getBoundingClientRect().top + body.clientTop;
    return {
      focused: link === document.activeElement,
      visible: rect.bottom > top && rect.top < top + body.clientHeight,
      sheetTop: body.scrollTop,
      pageTop: window.scrollY,
    };
  });
  const beforeInsertion = await focusedGeometry();
  check(beforeInsertion.focused && beforeInsertion.visible, 'the focused group should start visible');
  const inserted = Array.from({ length: 20 }, (_, i) =>
    row(fx, `live:dev:${30 + i}`, 'idle', { tab_group: { id: 10 + i, name: `Added ${i}` } }));
  fx.rows = [...inserted, second, first, third];
  await until('the poll inserts groups above the focused link', async () => await links.count() === inserted.length + 3);
  const afterInsertion = await focusedGeometry();
  check(afterInsertion.focused && afterInsertion.visible && afterInsertion.sheetTop > 0,
    'the poll clipped the focused group link');
  check(afterInsertion.pageTop === beforeInsertion.pageTop, 'revealing the group link scrolled the page');

  await page.locator(`${picker} .sheet-body`).evaluate((body) => { body.scrollTop = 0; });
  const scrolledAway = await focusedGeometry();
  check(scrolledAway.focused && !scrolledAway.visible && scrolledAway.sheetTop === 0,
    'the picker fixture did not scroll the focused link out of view');
  fx.rows = [...inserted, second, first, { ...third, tab_group: { ...third.tab_group, name: 'Third renamed' } }];
  await textUntil(page, `${picker} a[href="${route(2)}"] .section-name`, 'Third renamed', 'poll renames the hidden group');
  const afterRename = await focusedGeometry();
  check(afterRename.focused && !afterRename.visible && afterRename.sheetTop === 0,
    'the poll undid manual scrolling in the picker');

  fx.rows = [second, first];
  await until('the poll removes the focused group', async () => await links.count() === 2);
  check(await isFocused(page.locator(`${picker} .sheet-close`)), 'the removed group did not return focus to Close');
  await page.keyboard.press('Enter');
  await until('Close dismisses the picker', async () => !(await exists(page, picker)));

  for (const [opener, sheet, selector] of [
    ['#switch-group', picker, `.group-link[href="${route(1)}"]`],
    ['#menu-button', '#sheet-menu', '#logout-all'],
    ['#menu-button', '#sheet-logout-all', '[data-autofocus]'],
  ]) {
    await page.click(opener);
    if (sheet === '#sheet-logout-all') await page.click('#logout-all');
    const control = page.locator(`${sheet} ${selector}`);
    await control.focus();
    await page.evaluate(() => history.back());
    await waitForGroup(first);
    check(await isFocused(control), `Back moved focus out of ${sheet} or off its active control`);
    await page.evaluate(() => history.forward());
    await waitForGroup(second);
    check(await isFocused(control), `Forward moved focus out of ${sheet} or off its active control`);
    await page.keyboard.press('Escape');
    await until(`Escape dismisses ${sheet}`, async () => !(await exists(page, sheet)));
  }

  for (const dismiss of ['Escape', 'Close']) {
    fx.rows = [second, first];
    await until('the other group brings the picker opener back', () => exists(page, '#switch-group'));
    await page.focus('#switch-group');
    await page.keyboard.press('Enter');
    await sheetReady(page, 'sheet-groups', 'the picker opens with both groups');
    fx.rows = [second];
    await until('the poll removes the picker opener', async () =>
      !(await exists(page, '#switch-group')) && await links.count() === 1);
    if (dismiss === 'Escape') await page.keyboard.press('Escape');
    else await page.locator(`${picker} .sheet-close`).click();
    await until(`${dismiss} dismisses the picker and focuses Agents main`, async () =>
      !(await exists(page, picker)) && await activeId(page) === 'main');
  }
}

async function peerGroupOwnership(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const source = row(fx, 'live:old-name:7', 'waiting', {
    title: 'Original peer', session: 'old-name', tab_group_session: 'old-name', current_session: false,
    tab_group: { id: 0, name: 'Source default' }, detail_id: peerSelector('a'),
  });
  const replacement = row(fx, source.id, 'waiting', {
    title: 'Replacement peer', session: 'old-name', tab_group_session: 'old-name', current_session: false,
    tab_group: { id: 0, name: 'Replacement default' }, detail_id: peerSelector('b'),
    ref: { ...source.ref, session_id: 'replacement-conversation' },
  });
  const missing = row(fx, 'live:old-name:8', 'idle', {
    title: 'Original ungrouped', session: 'old-name', tab_group_session: 'old-name', current_session: false,
    tab_group: null,
  });
  const replacementMissing = row(fx, missing.id, 'idle', {
    title: 'Replacement ungrouped', session: 'old-name', tab_group_session: 'old-name', current_session: false,
    tab_group: null, ref: { ...missing.ref, session_id: 'another-conversation' },
  });
  fx.rows = [source, missing];
  await openList(page);

  const region = (group, session) => page.getByRole('region', {
    name: `${group} · 1 agent Session ${session}`, exact: true,
  });
  const oldKey = region('Source default', 'old-name').getByRole('button', { name: 'Key 1', exact: true });
  await oldKey.focus();
  const oldNode = await oldKey.elementHandle();
  const renamed = { ...source, tab_group_session: 'new-name' };
  const renamedMissing = { ...missing, tab_group_session: 'new-name' };
  fx.rows = [renamed, replacement, renamedMissing, replacementMissing];

  const sections = () => page.locator('.agent-section').evaluateAll((nodes) => nodes.map((section) => [
    section.querySelector('.section-name').textContent,
    section.querySelector('.section-session').textContent,
    [...section.querySelectorAll('.agent-card')].map((card) => [
      card.dataset.agentId, card.querySelector('.agent-title').textContent, card.querySelector('.badge-peer').textContent,
    ]),
  ]));
  const expected = [
    ['Source default', 'Session new-name', [[source.id, source.title, 'session new-name']]],
    ['Replacement default', 'Session old-name', [[replacement.id, replacement.title, 'session old-name']]],
    ['Tab group unavailable', 'Session new-name', [[missing.id, missing.title, 'session new-name']]],
    ['Tab group unavailable', 'Session old-name', [[replacementMissing.id, replacementMissing.title, 'session old-name']]],
  ];
  await until('the reused name shows its own page', async () => JSON.stringify(await sections()) === JSON.stringify([expected[1]]));
  check(hashOf(page) === groupRoute('old-name', 0), 'peer rename changed the URL of the selected page');
  check(await oldNode.evaluate((el) => !el.isConnected), 'the replacement reused the original quick key');
  check(await page.evaluate(() => document.activeElement === document.body), 'the replacement inherited focus');
  for (const [owner, id, index] of [['new-name', null, 2], ['old-name', null, 3], ['new-name', 0, 0]]) {
    await switchGroup(page, owner, id);
    check(JSON.stringify(await sections()) === JSON.stringify([expected[index]]), 'mixed peer rows across pages');
  }
  const sourceRegion = region('Source default', 'new-name');
  const currentKey = sourceRegion.getByRole('button', { name: 'Key 1', exact: true });
  await currentKey.focus();

  const sourceSection = await sourceRegion.elementHandle();
  // Enter posts the old guarded identity even though the visible owner is the new name.
  await page.keyboard.press('Enter');
  await until('renamed peer quick key posts', () => fx.count('/api/agent/keys', 'POST') === 1);
  await until('renamed peer quick key completes', () => currentKey.isEnabled());
  check(JSON.stringify(fx.bodies('/api/agent/keys')) === JSON.stringify([{ agent: source.ref, keys: ['1'] }]),
    'Enter changed the guarded ref or sent more than one request');
  for (const status of [409, 200]) {
    const changed = { outcome: 'target_changed', message: 'Agent disappeared or changed identity' };
    fx.fail.set('/api/agent/keys', { mode: 'reply', times: 1, status,
      body: status === 200 ? { response: changed } : { error: { ...changed, code: 'target_changed' } } });
    await currentKey.click();
    await textUntil(page, '#toast', 'Refresh Agents and reopen the card', `HTTP ${status} retry guidance`);
    await until('key is re-enabled', () => currentKey.isEnabled());
  }
  fx.fail.set('/api/agent/keys', { mode: 'reply', times: 1, status: 200,
    body: { response: { outcome: 'failed', message: 'Agent request outcome unknown; inspect agent before retrying' } } });
  await currentKey.click();
  await textUntil(page, '#toast', 'inspect agent before retrying', 'uncertain key outcome');

  const publishedId = 'live:new-name:7';
  const published = { ...renamed, id: publishedId, session: 'new-name' };
  const publishedMissingId = 'live:new-name:8';
  const publishedMissing = {
    ...renamedMissing, id: publishedMissingId, session: 'new-name', ref: { ...renamedMissing.ref, id: publishedMissingId },
  };
  fx.rows = [published, replacement, publishedMissing, replacementMissing];
  await until('poll publishes the renamed checkpoint', () => exists(page, cardSelector(publishedId)));
  check(await sourceSection.evaluate((el, id) => el.isConnected && el.querySelector('.agent-card')?.dataset.agentId === id, publishedId),
    'republishing the checkpoint replaced the section for the same cache owner');
  await currentKey.click();
  await until('published peer key keeps its ref', () => fx.bodies('/api/agent/keys').length === 5);
  check(fx.bodies('/api/agent/keys').at(-1).agent.id === source.detail_id, 'republication changed the guarded ref');
}

function peerDetailRows(fx) {
  const renamed = row(fx, 'live:old-peer:7', 'waiting', {
    title: 'Original peer', session: 'old-peer', tab_group_session: 'new-peer', current_session: false,
    detail_id: peerSelector('a'), run: '', tab_group: { id: 0, name: 'Source' },
  });
  const replacement = row(fx, renamed.id, 'waiting', {
    title: 'Replacement peer', session: 'old-peer', tab_group_session: 'old-peer', current_session: false,
    detail_id: peerSelector('c'), run: '', tab_group: { id: 0, name: 'Replacement' },
  });
  const published = { ...renamed, id: 'live:new-peer:7', session: 'new-peer' };
  return { renamed, replacement, published };
}

const peerRegion = (page, name, owner) => page.getByRole('region', {
  name: `${name} · 1 agent Session ${owner}`, exact: true,
});

async function peerFocusAcrossRepublication(open, control) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const { renamed, replacement, published } = peerDetailRows(fx);
  const key = control.includes('quick-key');
  fx.rows = [renamed];
  await openList(page);
  const original = peerRegion(page, 'Source', 'new-peer').locator(control);
  await original.focus();
  fx.rows = [replacement, published];
  await until('poll republishes original and restores the old name', () =>
    exists(page, cardSelector(published.id)));
  check(await original.evaluate((el) => document.activeElement === el), 'focus left the republished peer');
  await page.keyboard.press('Enter');
  if (key) {
    await until('focused key posts once', () => fx.count('/api/agent/keys', 'POST') === 1);
    check(JSON.stringify(fx.bodies('/api/agent/keys')[0]) === JSON.stringify({ agent: renamed.ref, keys: ['1'] }),
      'the replacement inherited the focused key');
    await until('key re-enabled', () => original.isEnabled());
  } else {
    await textUntil(page, '#detail-header .header-title', renamed.title, 'Enter opens the original peer');
    check(decodeURIComponent(await hashOf(page)).startsWith(`#/a/${renamed.detail_id}?`), 'opened the wrong selector');
    await until('Screen uses the scoped ref', () => fx.bodies('/api/agent/term').some((body) => body.agent.id === renamed.detail_id));
    await domClick(page, '#detail-back');
    await until('return to the peer cards', () => original.count());
  }
  await original.focus();
  fx.rows = [replacement];
  await until('original peer disappears', async () => !(await original.count()));
  check(await page.evaluate(() => document.activeElement === document.body), 'replacement inherited focus');
  const beforeEnter = await hashOf(page);
  await page.keyboard.press('Enter');
  check(await hashOf(page) === beforeEnter && fx.count('/api/agent/keys', 'POST') === Number(key),
    'Enter activated a control after the original peer disappeared');
}

async function scopedPeerDetails(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const { renamed, replacement, published } = peerDetailRows(fx);
  fx.rows = [renamed];
  await openList(page);
  const openCard = (name, owner) => peerRegion(page, name, owner).getByRole('link', { name: 'Open', exact: true }).click();
  await openCard('Source', 'new-peer');
  await textUntil(page, '#detail-header .header-title', renamed.title, 'runless source title');
  await page.locator('#composer-input').fill('Original attempted prompt');
  fx.fail.set('/api/agent/prompt', { mode: 'abort', times: 1 });
  await page.click('#composer-send');
  await until('lost prompt outcome is visible', () => exists(page, '#submission-unknown'));
  await page.locator('#composer-input').fill('Original edited draft');

  const recorded = { ...published, run: 'run-after-publication' };
  fx.rows = [replacement, recorded];
  await until('runless peer gains a recorded run after republication', async () =>
    decodeURIComponent(await hashOf(page)).includes(`${renamed.detail_id}?run=run-after-publication`));
  await until('same peer keeps its edited draft and pending attempt', async () =>
    (await valueOf(page, '#composer-input')) === 'Original edited draft' && await exists(page, '#submission-retry'));
  await domClick(page, '#detail-back');
  await switchGroup(page, 'old-peer', 0);
  await openCard('Replacement', 'old-peer');
  await textUntil(page, '#detail-header .header-title', replacement.title, 'replacement title');
  check(await valueOf(page, '#composer-input') === '', 'replacement inherited the original draft');
  check(!(await exists(page, '#submission-retry, .stale-submissions')), 'replacement inherited the original attempt');
  await page.locator('#composer-input').fill('Replacement draft');
  await domClick(page, '#detail-back');
  await switchGroup(page, 'new-peer', 0);
  await openCard('Source', 'new-peer');
  await until('original keeps its draft across navigation', async () =>
    (await valueOf(page, '#composer-input')) === 'Original edited draft' && await exists(page, '#submission-retry'));
  await page.click('#tab-timeline');
  await textUntil(page, '#timeline', 'Stored run-after-publication message', 'original history');
  fx.rows = [replacement, { ...recorded, run: 'next-run' }];
  await until('same peer changes run', async () => (await hashOf(page)).includes('run=next-run'));
  await textUntil(page, '#timeline', 'Stored next-run message', 'rollover history');
  check(fx.log.some((e) => e.path === '/api/usage' && e.query.get('agent') === renamed.detail_id), 'Usage lost the scoped selector');

  await page.click('#overflow-button');
  await sheetReady(page, 'sheet-overflow', 'actions sheet holds focus');
  await page.click('#action-keys');
  await until('keys sheet open', () => exists(page, '#sheet-keys'));
  fx.rows = [replacement];
  await textUntil(page, '#banner-agent-gone', 'history stays readable', 'scoped peer disappears');
  check(!(await exists(page, '#sheet-keys, #composer-input, #submission-retry')), 'missing peer retained live controls');
  await page.reload();
  await until('reloaded route exposes only recovery text', () => exists(page, '.stale-submissions textarea'));
  check(JSON.stringify(await page.locator('.stale-submissions textarea').evaluateAll((nodes) =>
    nodes.map((node) => [node.value, node.readOnly]))) === JSON.stringify([
    ['Original attempted prompt', true], ['Original edited draft', true],
  ]), 'recovery did not isolate both saved texts');
  check(!(await exists(page, '#submission-retry, .submission-adopt')), 'unverified peer offered retry or adoption');
  fx.rows = [replacement, { ...recorded, run: 'next-run' }];
  await until('same peer returns and permits explicit retry', () => exists(page, '#submission-retry'));
  check(fx.count('/api/agent/prompt', 'POST') === 1, 'navigation replayed a prompt');
  await page.click('#submission-retry');
  await until('explicit retry posts', () => fx.count('/api/agent/prompt', 'POST') === 2);
  const [first, retry] = fx.bodies('/api/agent/prompt');
  check(JSON.stringify(first) === JSON.stringify(retry) && first.agent.id === renamed.detail_id,
    'the retry changed the prompt, submission ID or target');
  await until('submitted attempt clears', async () => !(await exists(page, '#submission-retry')));
  check(await valueOf(page, '#composer-input') === 'Original edited draft', 'retry cleared the separately edited draft');
}

async function openScopedPeer(open, { run = 'peer-retained', state = 'busy' } = {}) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const { published, replacement } = peerDetailRows(fx);
  const peer = { ...published, run, state };
  const hash = `#/a/${encodeURIComponent(peer.detail_id)}?${new URLSearchParams({ run, tab: 'timeline' })}`;
  fx.rows = [peer, replacement];
  await openApp(page, hash);
  await until('scoped composer is enabled', () => page.locator('#composer-input').isEnabled());
  return { page, fx, peer, replacement, hash };
}

async function saveUnknownPeerPrompt(page, fx) {
  await page.locator('#composer-input').fill('Original attempted prompt');
  fx.fail.set('/api/agent/prompt', { mode: 'abort', times: 1 });
  await page.click('#composer-send');
  await until('lost prompt outcome is visible', () => exists(page, '#submission-unknown'));
  await page.locator('#composer-input').fill('Independent edited draft');
}

async function assertPeerRecovery(page, texts) {
  const values = () => page.locator('.stale-submissions textarea').evaluateAll((nodes) =>
    nodes.map((node) => [node.value, node.readOnly]));
  const expected = JSON.stringify(texts.map((text) => [text, true]));
  await until('read-only recovery contains the saved texts', async () => JSON.stringify(await values()) === expected);
  check(!(await exists(page, '#composer-input, #overflow-button, #submission-retry, .submission-adopt, .screen-keys')),
    'an unverified peer offered live controls');
  check(await page.locator('#tab-term').isDisabled(), 'an unverified peer allowed Screen');
}

async function manageAgent(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const malformed = (path, body) => fx.fail.set(path, { mode: 'reply', status: 200, times: 1, body });
  const retryMalformed = async (sheet, submit) => {
    await until('malformed inventory reported in the open sheet', async () =>
      (await sheet.getByRole('alert').allTextContents()).some((text) => text.includes('response this page cannot read')));
    check(await submit.isDisabled(), 'malformed inventory enabled submission');
    await sheet.getByRole('button', { name: 'Retry', exact: true }).click();
  };
  fx.rows = [];
  await openApp(page, '#/');
  await textUntil(page, '#list-empty', 'No agents', 'empty list');
  malformed('/api/agent/launch-choices', {});
  await page.click('#start-agent');
  const start = page.locator('#sheet-start');
  const submitStart = start.getByRole('button', { name: 'Start agent', exact: true });
  await retryMalformed(start, submitStart);
  await start.getByLabel('Provider').selectOption('codex');
  malformed('/api/agent/launch-choices', { server_instance: 'browser-instance', providers: ['bad provider'],
    projects: ['Review'], worktrees: [{ path: '/review', branch: 7 }] });
  await start.getByLabel('Project').selectOption('Review');
  await retryMalformed(start, submitStart);
  check(await start.getByLabel('Provider').inputValue() === 'codex', 'invalid choices replaced the selected provider');
  await start.getByLabel('Worktree').selectOption({ label: 'Detached HEAD · /review' });
  fx.fail.set('/api/agent/launch', { mode: 'abort', times: 1 });
  await submitStart.click();
  await textUntil(page, '#sheet-start [role=alert]', 'Cannot reach', 'uncertain launch');
  const key = 'yinz.web.v1.pending-launch';
  const saved = await page.evaluate((k) => JSON.parse(sessionStorage.getItem(k)), key);
  await page.reload();
  await page.click('#start-agent');
  await start.getByRole('button', { name: 'Check launch' }).click();
  await until('pending launch is cleared', () => page.evaluate((k) => sessionStorage.getItem(k) === null, key));
  const attempts = fx.log.filter((r) => r.path === '/api/agent/launch');
  check(attempts.length === 2 && attempts.every((r) => r.body.submission_id === saved.submission_id),
    'recovering a launch must preserve its submission ID');
  await textUntil(page, '.banner', 'Waiting for codex', 'launch awaiting identity');
  fx.live = { title: 'Phone agent', location: { group: { id: 0, name: 'Default' }, tab: { id: 1, name: 'Shell' } } };
  fx.rows = [row(fx, AGENT_ID, 'idle', fx.live)];
  await page.click('#list-refresh');
  await until('launched agent opens', () => exists(page, '#overflow-button'));
  const opened = hashOf(page);
  await page.click('#overflow-button');
  malformed('/api/agent/destinations', {});
  await page.click('#action-move');
  const move = page.locator('#sheet-move');
  const submitMove = move.getByRole('button', { name: 'Move agent', exact: true });
  await retryMalformed(move, submitMove);
  await move.getByLabel('Group', { exact: true }).selectOption('4');
  await move.getByLabel('Tab', { exact: true }).selectOption('other');
  malformed('/api/agent/destinations', { current_group: 0, current_tab: 1, allow_new_group: true,
    groups: [{ id: 4, name: 'Review', tabs: [{ id: 5, name: 'Shell', incarnation: 7 }] }] });
  await move.getByRole('button', { name: 'Refresh destinations' }).click();
  await retryMalformed(move, submitMove);
  fx.fail.set('/api/agent/move', { mode: 'reply', times: 1, status: 409, body: { error: { code: 'unavailable', message: 'Destination unavailable' } } });
  await submitMove.click();
  await textUntil(page, '#sheet-move [role="alert"]', 'Destination unavailable', 'move failure shown in picker');
  check(await move.getByLabel('Tab', { exact: true }).inputValue() === 'other', 'move lost the selected tab');
  await submitMove.click();
  await until('move form closed', async () => !(await exists(page, '#sheet-move')));
  check(hashOf(page) === opened, 'move left the conversation');
  await page.click('#detail-information-toggle');
  await textUntil(page, '#detail-location', 'Review (#4) · Shell (#5)', 'location refreshed after move');
  for (const confirm of [false, true]) {
    await page.click('#overflow-button');
    await page.click('#action-close');
    await textUntil(page, '#sheet-close-agent', 'Phone agent in browser-check · Review (#4)', 'close identifies agent and location');
    await page.locator('#sheet-close-agent').getByRole('button', { name: confirm ? 'Close agent' : 'Cancel', exact: true }).click();
    if (!confirm) check(fx.count('/api/agent/close') === 0 && hashOf(page) === opened, 'cancel closed the agent');
  }
  await textUntil(page, '#list-empty', 'No agents', 'returned to refreshed list');
  check(fx.count('/api/agent/close') === 1, 'sent multiple close requests');
}

async function peerDesktopFocus(open) {
  const { page, fx, peer } = await openScopedPeer(open);
  const focus = async (status, body) => {
    fx.fail.set('/api/agent/focus', { mode: 'reply', times: 1, status, body });
    await page.click('#overflow-button');
    await sheetReady(page, 'sheet-overflow', 'Focus actions are available');
    check(!(await exists(page, '#action-move, #action-close')), 'peer offered local management actions');
    await page.click('#action-focus');
  };
  for (const message of ['Focus is temporarily unavailable while the peer publishes its renamed session.', 'No terminal attached']) {
    await focus(409, { error: { code: 'unavailable', message } });
    await textUntil(page, '#toast', message, 'native Focus reason');
    check(await page.locator('#composer-input').isEnabled(), 'Focus-only failure disabled the composer');
    check(await page.locator('#tab-term').isEnabled(), 'Focus-only failure disabled Screen');
  }
  await page.click('#tab-term');
  await textUntil(page, '.xterm-rows', '2. No', 'screen remains readable');
  await page.locator('.screen-keys .quick-key').first().click();
  await until('key goes to the same peer', () => fx.bodies('/api/agent/keys').some((body) => body.agent.id === peer.detail_id));
  await focus(200, { ok: false });
  await textUntil(page, '#toast', 'cannot read', 'unconfirmed Focus response');
  await focus(200, { ok: true });
  await textUntil(page, '#toast', `Desktop switched to session ${peer.session}`, 'verified switch');
  await focus(409, { error: { code: 'target_changed', message: 'Agent disappeared or changed identity' } });
  await textUntil(page, '#toast', 'The agent changed or disappeared.', 'changed peer message');
  await until('changed peer loses guarded controls', async () =>
    await page.locator('#tab-term').isDisabled() && !(await exists(page, '#composer-input, #overflow-button, .screen-keys')));
}

async function scopedPeerRetry(open) {
  const { page, fx, peer, replacement } = await openScopedPeer(open);
  await saveUnknownPeerPrompt(page, fx);
  fx.fail.set('/api/agent/prompt', { mode: 'reply', times: 1, status: 409,
    body: { error: { code: 'target_changed', message: 'Agent disappeared or changed identity' } } });
  // The retry snapshot misses before the next History snapshot observes the loss.
  await page.click('#submission-retry');
  await until('retry receives the typed cache miss', () => fx.count('/api/agent/prompt', 'POST') === 2);
  await assertPeerRecovery(page, ['Original attempted prompt', 'Independent edited draft']);
  fx.rows = [replacement];
  await page.reload();
  await assertPeerRecovery(page, ['Original attempted prompt', 'Independent edited draft']);
  fx.rows = [peer, replacement];
  await until('same binding offers same-ID retry after verification', () => exists(page, '#submission-retry'));
  check(await valueOf(page, '#composer-input') === 'Independent edited draft', 'reverification lost the edited draft');
  check(fx.count('/api/agent/prompt', 'POST') === 2, 'reverification retried automatically');
  await page.click('#submission-retry');
  await until('one more explicit retry', () => fx.count('/api/agent/prompt', 'POST') === 3);
  const bodies = fx.bodies('/api/agent/prompt');
  check(bodies.every((body) => JSON.stringify(body) === JSON.stringify(bodies[0])) &&
    bodies[0].agent.id === peer.detail_id, 'the original POST body changed');
  await until('successful retry clears its pending attempt', async () => !(await exists(page, '#submission-retry')));
  check(await valueOf(page, '#composer-input') === 'Independent edited draft', 'successful retry erased the edit');
}

async function scopedPeerInitialLookupFailure(open) {
  for (const run of ['peer-retained', '']) {
    const { page, fx, peer, replacement, hash } = await openScopedPeer(open, { run });
    await saveUnknownPeerPrompt(page, fx);
    const path = run ? '/api/history' : '/api/agents';
    fx.fail.set(path, { mode: 'reply', times: Infinity, status: 503,
      body: { error: { code: 'unavailable', message: 'First lookup failed' } } });
    await page.reload();
    await until('failure offers lookup Retry', () => exists(page, '#banner-offline button'));
    await assertPeerRecovery(page, ['Original attempted prompt', 'Independent edited draft']);
    if (run) {
      await openApp(page, `${hash}&bind=0`);
      await until('stored run remains unbound on error', () => exists(page, '#banner-stored'));
      check(!(await exists(page, '.stale-submissions, #composer-input')), 'stored run revealed scoped text');
      await openApp(page, hash);
      await assertPeerRecovery(page, ['Original attempted prompt', 'Independent edited draft']);
    }
    fx.rows = [replacement];
    fx.fail.delete(path);
    await page.locator('#banner-offline button').click();
    await until('lookup confirms peer is unavailable', () => exists(page, '#banner-agent-gone'));
    await assertPeerRecovery(page, ['Original attempted prompt', 'Independent edited draft']);
    fx.rows = [peer];
    await until('only the original peer restores retry', () => exists(page, '#submission-retry'));
    check(await valueOf(page, '#composer-input') === 'Independent edited draft', 'the saved edit changed');
    check(fx.count('/api/agent/prompt', 'POST') === 1, 'recovery sent a prompt');
  }
}

async function scopedPeerDiscards(open) {
  for (const what of ['attempted prompt', 'edited draft', 'shared saved text']) {
    const { page, fx, peer, replacement } = await openScopedPeer(open);
    await saveUnknownPeerPrompt(page, fx);
    const attempt = 'Original attempted prompt';
    const edit = 'Independent edited draft';
    const shared = what === 'shared saved text';
    if (shared) await page.locator('#composer-input').fill(attempt);
    fx.rows = [replacement];
    await assertPeerRecovery(page, shared ? [attempt] : [attempt, edit]);
    const before = fx.log.filter(({ path, method }) => method === 'POST' && path.startsWith('/api/agent/')).length;
    await page.getByRole('button', { name: `Discard ${what}`, exact: true }).click();
    await page.reload();
    await until('peer remains unavailable on reload', () => exists(page, '#banner-agent-gone'));
    const keepingAttempt = what === 'edited draft';
    if (shared) {
      check(!(await exists(page, '.stale-submissions')), 'shared discard left saved texts');
    } else {
      await assertPeerRecovery(page, [keepingAttempt ? attempt : edit]);
    }
    fx.rows = [peer, replacement];
    await until('peer returns after discard', () => page.locator('#composer-input').isEnabled());
    check(await valueOf(page, '#composer-input') === (what === 'attempted prompt' ? edit : ''), 'discard changed the surviving edit');
    check(await exists(page, '#submission-retry') === keepingAttempt, 'discard changed the wrong pending attempt');
    check(fx.log.filter(({ path, method }) => method === 'POST' && path.startsWith('/api/agent/')).length === before,
      'discard or reload posted a live request');
  }
}

async function olderPeerHistory(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const reason = 'Live controls are unavailable until the peer publishes its identity. Refresh Agents and reopen the card.';
  const old = row(fx, 'live:older-peer:11', 'waiting', { current_session: false, session: 'older-peer',
    run: 'older-run', title: 'Older peer with history', can_prompt: false });
  const runless = row(fx, 'live:older-peer:12', 'waiting', { ...old, id: 'live:older-peer:12',
    run: '', title: 'Older peer without history', prompt_unavailable: reason });
  const omitted = row(fx, 'live:older-peer:13', 'waiting', { ...runless, id: 'live:older-peer:13',
    title: 'Older peer without hook guidance', prompt_unavailable: null });
  const { published } = peerDetailRows(fx);
  fx.rows = [old, runless, omitted, published];
  await openList(page);
  for (const [unavailable, explanation] of [[runless, reason], [omitted, 'Refresh Agents to try again']]) {
    const card = page.locator(cardSelector(unavailable.id));
    check((await card.textContent()).includes(explanation), 'runless card omitted availability guidance');
    check(await card.getByRole('link').count() === 0, 'runless older peer offered a live link');
    for (const key of await card.locator('.quick-key').all()) check(await key.isDisabled(), 'older peer key was enabled');
  }
  for (const link of ['.agent-open-area', '.agent-open']) {
    const start = fx.log.length;
    await page.locator(cardSelector(old.id)).locator(link).click();
    await textUntil(page, '#timeline', 'Stored older-run message', 'older waiting card opens stored Timeline');
    // A reopened timeline can render cached text before its history request arrives.
    await until('stored history request for ' + link, () => fx.log.slice(start).some(({ path }) => path === '/api/history'));
    check(await page.getAttribute('#tab-timeline', 'aria-selected') === 'true', 'stored link selected Screen');
    check(await page.locator('#tab-term').isDisabled() && !(await exists(page, '#composer-input')), 'stored route enabled controls');
    await domClick(page, '#detail-back');
    await until('return to older cards', () => exists(page, cardSelector(old.id)));
  }
  const reads = fx.log.filter(({ path }) => path === '/api/history');
  check(reads.every(({ query }) => query.get('run') === old.run && !query.has('agent')), 'stored fallback did not request only the stored run');
  check(!fx.log.some(({ path }) => path.startsWith('/api/agent/')), 'older peer issued a live request');
  await switchGroup(page, 'new-peer', 0);
  await page.locator(cardSelector(published.id)).locator('.agent-open').click();
  await until('valid waiting peer uses Screen', () => fx.bodies('/api/agent/term').some((body) => body.agent.id === published.detail_id));
  check(await page.getAttribute('#tab-term', 'aria-selected') === 'true', 'valid waiting route lost Screen');
}

async function nextWaitingAgent(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const firstGroup = { id: 0, name: 'First' };
  const first = row(fx, AGENT_ID, 'waiting', { tab_group: firstGroup });
  const second = row(fx, 'live:dev:8', 'waiting', { tab_group: { id: 1, name: 'Second' } });
  const idle = Array.from({ length: 15 }, (_, i) => row(fx, `live:dev:${20 + i}`, 'idle', { tab_group: firstGroup }));
  fx.rows = [first, second, ...idle];
  fx.counts = { waiting: 5, busy: 0, idle: idle.length };
  await openList(page);
  const shortcut = page.locator('#list-next-waiting');
  const card = (row) => page.locator(cardSelector(row.id));
  const focused = (row) => until(`jump focuses and reveals ${row.id}`, async () =>
    await exists(page, cardSelector(row.id)) && card(row).locator('.agent-open-area').evaluate((el) => {
      const rect = el.getBoundingClientRect();
      return document.activeElement === el && rect.top >= document.querySelector('#list-header').getBoundingClientRect().bottom - 1 &&
        rect.bottom <= innerHeight + 1;
    }));
  const next = async (from, to) => {
    await page.keyboard.press('Tab');
    check(await card(from).locator('.card-waiting-next').evaluate((el) => document.activeElement === el),
      'ordinary Tab did not reach the adjacent repeat control');
    await page.keyboard.press('Enter');
    await focused(to);
  };
  check(await textOf(page, '.need-count') === '5 need you', 'shortcut replaced the native count');
  await shortcut.focus();
  await page.keyboard.press('Enter');
  await focused(first);
  await next(first, second);
  check(hashOf(page) === groupRoute('browser-check', 1) && !(await exists(page, cardSelector(first.id))), 'cross-group jump did not switch pages');
  await next(second, first);
  await noHorizontalOverflow(page, 'adjacent waiting control at phone width');
  fx.rows = [second, first, ...idle];
  await page.click('#switch-group');
  await until('poll reverses waiting groups in the picker', async () =>
    (await page.locator('#sheet-groups .group-link').evaluateAll((links) => links[0]?.hash)) === groupRoute('browser-check', 1));
  check(hashOf(page) === groupRoute('browser-check', 0), 'group reordering switched the current page');
  await page.keyboard.press('Escape');
  await card(first).locator('.agent-open-area').focus();
  await next(first, second);
  await page.keyboard.press('Enter');
  await until('ordinary Enter opens the focused card', async () => decodeURIComponent(await hashOf(page)).includes(second.id));
  await domClick(page, '#detail-back');
  await until('return to the same Agents page', async () =>
    hashOf(page) === groupRoute('browser-check', 1) && await exists(page, cardSelector(second.id)));
  fx.rows = [first, ...idle];
  await until('removal empties the group page', () => exists(page, '#list-empty'));
  await shortcut.focus();
  await page.keyboard.press('Enter');
  await focused(first);
  check(!(await exists(page, '.card-waiting-next')), 'a single waiting agent retained the adjacent shortcut');
  fx.rows = idle;
  await until('shortcut hides when no rendered waiting card remains', async () => !(await shortcut.count()));
  check(await textOf(page, '.need-count') === '5 need you', 'hook-hidden waiting cards changed the native count');
  check(fx.count('/api/agent/keys', 'POST') === 0, 'shortcut sent a key');
}

async function whitespaceSessionHeadings(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const first = row(fx, 'live:build team:7', 'idle', { session: 'build team', current_session: false });
  const second = row(fx, 'live:build  team:7', 'idle', { session: 'build  team', current_session: false });
  fx.rows = [first, second];
  await openList(page);

  const assertLabels = async (expected) => {
    const links = page.locator('#sheet-groups .group-link');
    const visible = await links.allInnerTexts();
    check(new Set(visible).size === 2, 'the two headings look identical');
    for (const [index, [marker, session]] of expected.entries()) {
      const accessible = `Default · 1 agent ${marker} · Session build team`;
      check(await page.getByRole('link', { name: accessible, exact: true }).count() === 1, `wrong accessible group link: ${accessible}`);
      check(visible[index].includes(`${marker} · Session ${session}`), `visible heading lost its marker or literal spacing: ${JSON.stringify(visible[index])}`);
      check(await links.nth(index).locator('.section-session-spelling').textContent() === session, 'the literal session name changed');
    }
    await noHorizontalOverflow(page, 'whitespace-colliding session headings at 320px');
  };
  const expected = [['2 of 2', first.session], ['1 of 2', second.session]];
  await page.click('#switch-group');
  await assertLabels(expected);
  fx.rows = [second, first];
  await until('poll reverses the picker order', async () =>
    await page.locator('#sheet-groups .group-link').first().getAttribute('href') === groupRoute(second.session, 0));
  await assertLabels(expected.slice().reverse());
  await page.getByRole('link', { name: 'Default · 1 agent 1 of 2 · Session build team', exact: true }).click();
  await until('picker opens the distinct owning session', async () =>
    hashOf(page) === groupRoute(second.session, 0) && await exists(page, cardSelector(second.id)));
}

async function agentListPollFocus(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const initial = { id: 0, name: 'Initial group' };
  const destination = { id: 1, name: 'Destination' };
  const other = row(fx, 'live:dev:9', 'idle', { tab_group: { id: 2, name: 'Other group' } });
  const agent = row(fx, AGENT_ID, 'waiting', { tab_group: initial });
  const neighbor = row(fx, 'live:dev:8', 'idle', { tab_group: destination });
  const tall = Array.from({ length: 14 }, (_, i) => row(fx, `live:dev:${20 + i}`, 'idle', { tab_group: destination }));
  fx.rows = [agent, neighbor, ...tall, other];
  await openList(page);

  const quickKey = page.locator(cardSelector(agent.id)).getByRole('button', { name: 'Key 1', exact: true });
  const geometry = () => quickKey.evaluate((el) => {
    const { top, bottom } = el.getBoundingClientRect();
    const headerBottom = document.getElementById('list-header').getBoundingClientRect().bottom;
    return {
      active: document.activeElement === el, visible: top >= headerBottom - 1 && bottom <= innerHeight + 1,
      documentTop: top + scrollY, viewport: innerHeight, scroll: scrollY,
    };
  });
  const assertFocusedInView = async (label) => {
    const g = await geometry();
    check(g.active && g.visible, `${label}: ${JSON.stringify(g)}`);
    return g;
  };
  await quickKey.focus();
  await assertFocusedInView('initial quick key');
  const oldKey = await quickKey.elementHandle();
  const moved = { ...agent, tab_group: destination };
  fx.rows = [moved, neighbor, ...tall, other];
  await until('automatic poll moves the focused agent off the page', () => exists(page, '#list-empty'));
  check(hashOf(page) === groupRoute('browser-check', 0), 'poll navigated to the destination page');
  check(await oldKey.evaluate((el) => !el.isConnected), 'the group move did not replace the control');
  check(await page.evaluate(() => document.activeElement === document.body), 'focus moved to another control');
  await switchGroup(page, 'browser-check', destination.id);
  await quickKey.focus();
  const beforeReorder = await assertFocusedInView('destination quick key');
  const movedKey = await quickKey.elementHandle();

  fx.rows = [other, ...tall, moved, neighbor];
  await until('automatic poll reorders the cards on the selected page', async () =>
    await page.locator('.agent-card').first().getAttribute('data-agent-id') === tall[0].id);
  const afterReorder = await assertFocusedInView('quick key after card reorder');
  check(await movedKey.evaluate((el) => document.activeElement === el), 'card reorder replaced the focused node');
  check(afterReorder.documentTop - beforeReorder.documentTop > beforeReorder.viewport, 'card reorder did not displace the quick key far enough');
  await page.keyboard.press('Enter');
  await until('focused quick key posts', () => fx.count('/api/agent/keys', 'POST') === 1);
  await until('quick key completes', () => quickKey.isEnabled());
  check(JSON.stringify(fx.bodies('/api/agent/keys')) === JSON.stringify([{ agent: REF7, keys: ['1'] }]),
    'Enter did not send exactly one key for the original conversation');

  await quickKey.focus();
  await page.evaluate(() => window.scrollTo(0, 0));
  const scrolledAway = await geometry();
  check(scrolledAway.active && !scrolledAway.visible && scrolledAway.scroll === 0, 'manual scroll did not move the key out of view');
  const renamed = { ...destination, name: 'Destination renamed' };
  fx.rows = [other, ...[...tall, moved, neighbor].map((agent) => ({ ...agent, tab_group: renamed }))];
  await until('poll applies rename while scrolled away', () => quickKey.evaluate((el, name) =>
    el.closest('.agent-section').querySelector('.section-name').textContent === name, renamed.name));
  const afterManualScroll = await geometry();
  check(afterManualScroll.active && !afterManualScroll.visible && afterManualScroll.scroll === 0, 'poll undid manual scrolling');
}

async function agentListPollFocusGuards(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const original = row(fx, AGENT_ID, 'waiting');
  const quickKey = page.locator(cardSelector(original.id)).getByRole('button', { name: 'Key 1', exact: true });
  const hasBodyFocus = () => page.evaluate(() => document.activeElement === document.body);
  fx.rows = [original];
  await openList(page);

  await quickKey.focus();
  fx.rows = [row(fx, original.id, 'busy')];
  await until('poll removes quick key from working agent', async () => await quickKey.count() === 0);
  check(await hasBodyFocus(), 'focus moved to a different control when the quick key disappeared');

  fx.rows = [original];
  await until('poll restores waiting agent', () => quickKey.isEnabled());
  await quickKey.focus();
  fx.rows = [{ ...original, ref: null }];
  await until('poll renders disabled quick key', () => quickKey.isDisabled());
  check(await hasBodyFocus(), 'focus moved to a disabled replacement');

  fx.rows = [original];
  await until('poll enables quick key', () => quickKey.isEnabled());
  await quickKey.focus();
  const priorKey = await quickKey.elementHandle();
  fx.rows = [{ ...original, ref: REF7B, run: 'run-7b' }];
  await until('poll publishes replacement conversation in the same card', () =>
    page.locator(cardSelector(original.id)).getByRole('link', { name: 'Open', exact: true })
      .evaluate((el) => el.hash.includes('run-7b')));
  check(await priorKey.evaluate((el) => !el.isConnected) && await hasBodyFocus(), 'replacement conversation inherited quick-key focus');
  await page.keyboard.press('Enter');
  check(fx.count('/api/agent/keys', 'POST') === 0, 'Enter sent a key to a retargeted or unavailable agent');
}

async function agentListPollFocusOutsideCards(open) {
  const { page, fx } = await open({ viewport: { width: 320, height: 640 } });
  const original = row(fx, AGENT_ID, 'waiting');
  const customRow = { ...original, html: `${original.html}<span class="hook-focus" tabindex="0">Hook content</span>` };
  const nextGroup = { id: 1, name: 'Destination' };
  fx.rows = [customRow];
  await openList(page);

  const refresh = page.getByRole('button', { name: 'Refresh', exact: true });
  await refresh.focus();
  fx.rows = [{ ...customRow, tab_group: nextGroup }];
  await until('poll moves card off the page while Refresh is focused', () => exists(page, '#list-empty'));
  check(await refresh.evaluate((el) => document.activeElement === el), 'poll stole focus from the header');

  await switchGroup(page, 'browser-check', nextGroup.id);
  const hook = page.locator('.hook-focus');
  await hook.focus();
  check(await hook.evaluate((el) => document.activeElement === el), 'hook control did not receive focus');
  fx.rows = [customRow];
  await until('poll moves the card with custom content off the page', () => exists(page, '#list-empty'));
  check(await page.evaluate(() => document.activeElement === document.body), 'poll restored focus to hook content');
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

async function timelineResponseSpinner(open) {
  const { page, fx } = await open();
  const spinner = '#timeline .timeline-spinner';
  const promptPath = '/api/agent/prompt';
  const historyPath = '/api/history';
  const fillHeld = async (entry) => {
    const [status, json] = fixtureReply(fx, entry);
    await fx.held.shift().fulfill({ status, json });
  };
  const send = async () => {
    await page.locator('#composer-input').fill('Run the focused tests');
    await domClick(page, '#composer-send');
  };

  fx.live.state = 'idle';
  await openDetail(page);
  check(!(await exists(page, spinner)), 'idle agent shows a spinner');
  const pinned = await scrollState(page);
  check(pinned.overflow && pinned.bottomGap <= 1, `idle Timeline must start overflowing and pinned (gap ${pinned.bottomGap}px)`);
  fx.live.state = 'busy';
  await fixturePoll(page, fx);
  check(await exists(page, spinner), 'external activity did not show a spinner at the bottom');
  const busyGap = (await scrollState(page)).bottomGap;
  check(busyGap <= 1, `spinner left a ${busyGap}px gap at the bottom of the pinned Timeline`);
  fx.live.state = 'idle';
  await fixturePoll(page, fx);
  check(!(await exists(page, spinner)), 'spinner remained when external activity ended');

  fx.fail.set(promptPath, { mode: 'hold', times: 1 });
  await send();
  await until('held prompt', () => fx.held.length === 1);
  await textUntil(page, '#timeline [role="status"]', 'Waiting for agent…', 'pending prompt status');
  check(await page.locator(spinner).isVisible(), 'missing spinner while sending');
  check(await page.locator(spinner).evaluate((el) => getComputedStyle(el).animationName) === 'none', 'spinner animates with reduced motion');
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  check(await page.locator(spinner).evaluate((el) => getComputedStyle(el).animationName) === 'timeline-spin', 'spinner does not animate');

  fx.fail.set(historyPath, { mode: 'hold', times: 1 });
  await until('held pre-confirmation history request', () => fx.held.length === 2);
  await fillHeld(fx.log.find((e) => e.path === promptPath));
  await until('draft cleared after confirmation', async () => (await valueOf(page, '#composer-input')) === '');
  check(await exists(page, spinner), 'confirmation hid the spinner before the agent state arrived');
  fx.fail.set(historyPath, { mode: 'hold', times: 1 });
  await fillHeld(fx.log.findLast((e) => e.path === historyPath));
  await until('held post-confirmation history request', () => fx.held.length === 1);
  check(await exists(page, spinner), 'pre-confirmation history hid the spinner');

  fx.live.state = 'busy';
  await fillHeld(fx.log.findLast((e) => e.path === historyPath));
  await textUntil(page, '.detail-summary .state-badge', 'Working', 'working agent state');
  check(await exists(page, spinner), 'working agent lost its spinner');
  fx.latest += 1;
  fx.live.state = 'idle';
  await fixturePoll(page, fx);
  check(await exists(page, `#timeline [data-key="e${fx.latest}"]`), 'the reply was not rendered');
  check(!(await exists(page, spinner)), 'spinner remained after the reply and idle state');

  await page.locator('#timeline').evaluate((el) => { el.scrollTop = 0; });
  await nextFrame(page);
  const anchor = await timelineAnchor(page);
  fx.live.state = 'busy';
  await fixturePoll(page, fx);
  check(await exists(page, spinner), 'externally started activity did not show a spinner');
  await sameAnchor(page, anchor, 'spinner appeared');
  check(!(await exists(page, '#timeline-new')), 'spinner was counted as a new message');
  fx.live.state = 'waiting';
  await fixturePoll(page, fx);
  check(!(await exists(page, spinner)), 'agent that needs input still shows a spinner');
  await sameAnchor(page, anchor, 'spinner disappeared');

  fx.prompt = 'rejected';
  fx.fail.set(promptPath, { mode: 'hold', times: 1 });
  await send();
  await until('held rejected prompt', () => fx.held.length === 1);
  check(await exists(page, spinner), 'spinner did not show while the rejected prompt was in flight');
  await fillHeld(fx.log.findLast((e) => e.path === promptPath));
  await until('prompt rejection', () => exists(page, '#submission-failed'));
  check(!(await exists(page, spinner)), 'rejected prompt still shows a spinner');

  fx.live.state = 'busy';
  await fixturePoll(page, fx);
  check(await exists(page, spinner), 'working agent did not show a spinner before opening history');
  await domClick(page, '#tab-details');
  check(!(await exists(page, '.timeline-spinner')), 'spinner leaked into Details');
  await domClick(page, '#details-previous');
  await textUntil(page, '#timeline', 'Stored run-6', 'stored run content');
  check(!(await exists(page, spinner)), 'stored run shows a spinner');
}

async function peerTimelineHandoff(open, recorded) {
  const { context, page, fx } = await open({ clock: true });
  const detailId = peerSelector('a');
  const peerId = 'live:peer:7';
  const run = recorded ? 'run-7' : '';
  const spinner = '#timeline .timeline-spinner';
  const promptPath = '/api/agent/prompt';
  const statusPath = recorded ? '/api/history' : '/api/agents';
  const badge = '.detail-summary .state-badge';
  const setActivity = (state, hookAt) => {
    fx.rows = [row(fx, peerId, state, {
      detail_id: detailId, last_hook_at_ms: hookAt, current_session: false, session: 'peer', run,
    })];
  };
  const poll = () => refreshFixture(page, fx, statusPath);
  const send = async () => {
    const before = fx.count(promptPath);
    await page.locator('#composer-input').fill('Run the focused tests');
    await domClick(page, '#composer-send');
    await until('peer draft cleared after confirmation', async () => fx.count(promptPath) > before && (await valueOf(page, '#composer-input')) === '');
    check(fx.bodies(promptPath).at(-1)?.agent?.id === detailId, 'prompt did not use the scoped peer ref');
    return page.evaluate(() => Date.now());
  };
  const confirm = async () => {
    await idle(page);
    const before = fx.count(statusPath);
    const confirmedAt = await send();
    await context.clock.runFor(150);
    await until('confirmation started a status refresh', () => fx.count(statusPath) > before);
    await idle(page);
    check(await exists(page, spinner), 'post-confirmation cached idle response hid the peer spinner');
    return confirmedAt;
  };
  const advanceTo = async (time) => context.clock.runFor(time - await page.evaluate(() => Date.now()));

  setActivity('idle', 10);
  await openApp(page, detailHash(run, 'timeline', detailId));
  await textUntil(page, badge, 'Idle', 'idle peer state');
  await pauseClock(context, page);
  check(!(await exists(page, spinner)), 'idle peer shows a spinner before submitting');

  await confirm();
  setActivity('busy', 10);
  await poll();
  await textUntil(page, badge, 'Working', 'fresh busy peer state');
  check(await exists(page, spinner), 'fresh busy state failed to take over the peer spinner');
  setActivity('idle', 10);
  await poll();
  await textUntil(page, badge, 'Idle', 'peer finished working');
  check(!(await exists(page, spinner)), 'the peer handoff lingered after a busy state took over');

  await confirm();
  setActivity('idle', 9);
  await poll();
  check(await exists(page, spinner), 'an older idle hook report cleared the handoff');

  fx.fail.set(statusPath, { mode: 'hold', times: 1 });
  await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));
  await until('held peer status request', () => fx.held.length === 1);
  await send();
  await context.clock.runFor(150);
  setActivity('idle', 11);
  const [status, json] = fixtureReply(fx, fx.log.findLast((e) => e.path === statusPath));
  await fx.held.shift().fulfill({ status, json });
  await idle(page);
  check(await exists(page, spinner), 'a status read started before the second confirmation cleared its handoff');
  await poll();
  check(!(await exists(page, spinner)), 'newer idle hook report did not clear the peer handoff');

  const first = await confirm();
  const second = await confirm();
  await advanceTo(first + 10000);
  await idle(page);
  check(await exists(page, spinner), 'the first confirmation timer cleared the second handoff');
  await advanceTo(second + 9999);
  await idle(page);
  check(await exists(page, spinner), 'unchanged peer status cleared the handoff before 10 seconds');
  await context.clock.runFor(1);
  await until('peer handoff expired at 10 seconds', async () => !(await exists(page, spinner)));
}

async function hiddenTimelineConfirmation(open) {
  const { context, page, fx } = await open({ clock: true });
  const detailId = peerSelector('a');
  const promptPath = '/api/agent/prompt';
  const statusPath = '/api/history';
  fx.rows = [row(fx, 'live:peer:7', 'idle', {
    detail_id: detailId, current_session: false, session: 'peer', run: 'run-7',
  })];
  await openDetail(page, detailHash('run-7', 'timeline', detailId));
  await pauseClock(context, page);
  await idle(page);

  fx.fail.set(promptPath, { mode: 'hold', times: 1 });
  await page.locator('#composer-input').fill('Run the focused tests');
  await domClick(page, '#composer-send');
  await until('held prompt before hiding', () => fx.held.length === 1);
  await setDocumentHidden(page, true);
  const before = await fetchCount(page, statusPath);
  const [status, json] = fixtureReply(fx, fx.log.findLast((e) => e.path === promptPath));
  await fx.held.shift().fulfill({ status, json });
  await until('draft cleared while hidden', async () => (await valueOf(page, '#composer-input')) === '');
  await context.clock.runFor(3500);
  check(await fetchCount(page, statusPath) === before, 'hidden confirmation started a status GET');

  await setDocumentHidden(page, false);
  await until('foreground started a status GET', async () => (await fetchCount(page, statusPath)) === before + 1);
  await idle(page);
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

async function cachedTimelineRevisit(open) {
  const { page, fx } = await open();
  const historyPath = '/api/history';
  const timelineEvent = (revision) => `#timeline [data-key="e${revision}"]`;
  const cardLink = (agent) => `${cardSelector(agent)} .agent-open-area`;
  const backToList = async () => {
    await domClick(page, '#detail-back');
    await until('returned to the agent list', () => exists(page, cardLink(AGENT_ID)));
  };

  fx.live.state = 'idle';
  fx.rows = [row(fx, AGENT_ID, 'idle'), row(fx, 'live:dev:8', 'busy')];
  await openList(page);
  await domClick(page, cardLink(AGENT_ID));
  await until('first chat timeline', () => exists(page, timelineEvent(45)));
  await domClick(page, '#timeline-earlier');
  await until('older page in the first chat', () => exists(page, timelineEvent(1)));
  await backToList();
  await domClick(page, cardLink('live:dev:8'));
  await textUntil(page, '#timeline', 'Stored run-8 message 5', 'second chat timeline');
  check(!(await exists(page, timelineEvent(45))), 'second chat rendered the first chat’s messages');
  await backToList();

  fx.latest = 46;
  fx.fail.set(historyPath, {
    mode: 'hold', times: 1,
    matches: ({ query }) => query.get('run') === 'run-7',
  });
  await domClick(page, cardLink(AGENT_ID));
  await until('held history refresh for the reopened chat', () => fx.held.length === 1);
  check(await exists(page, timelineEvent(45)) && await exists(page, timelineEvent(1)),
    'reopened chat did not render both cached pages before its first response');
  check(!(await exists(page, timelineEvent(46))), 'held delivery appeared before its response');
  check(!(await exists(page, '#composer-input')) && !(await exists(page, '#overflow-button')),
    'cached history enabled live controls before verification');
  const refresh = fx.log.findLast(({ path, query }) => path === historyPath && query.get('run') === 'run-7');
  check(refresh.query.get('after_revision') === '46' && !refresh.query.has('before')
    && refresh.query.get('agent') === AGENT_ID, 'reopened chat did not resume its bound delivery cursor');

  const [status, json] = fixtureReply(fx, refresh);
  await fx.held.shift().fulfill({ status, json });
  await until('new message and verified controls', async () =>
    await exists(page, timelineEvent(46)) && await exists(page, '#composer-input'));
  check(await page.locator(timelineEvent(46)).count() === 1, 'delivered message was duplicated');
  check(await exists(page, timelineEvent(1)) && await exists(page, timelineEvent(45)),
    'delivery replaced cached history');
  check(!(await exists(page, '#timeline-earlier')), 'delivery restored an exhausted older-history cursor');

  await backToList();
  const timezone = 'America/Chicago';
  await page.evaluate((timeZone) => {
    const resolvedOptions = Intl.DateTimeFormat.prototype.resolvedOptions;
    Intl.DateTimeFormat.prototype.resolvedOptions = function () {
      return { ...resolvedOptions.call(this), timeZone };
    };
  }, timezone);
  fx.fail.set(historyPath, {
    mode: 'hold', times: 1,
    matches: ({ query }) => query.get('run') === 'run-7',
  });
  await domClick(page, cardLink(AGENT_ID));
  await until('held history request after the timezone change', () => fx.held.length === 1);
  const newVisit = fx.log.findLast(({ path, query }) => path === historyPath && query.get('run') === 'run-7');
  check(newVisit.query.get('tz') === timezone, 'reopened chat did not send the new timezone');
  check(!newVisit.query.has('after_revision') && !newVisit.query.has('before'), 'reopened chat reused the old timezone’s cursors');
  check(!(await exists(page, '#timeline .event')), 'reopened chat rendered the old timezone’s history');

  const [newStatus, newJson] = fixtureReply(fx, newVisit);
  await fx.held.shift().fulfill({ status: newStatus, json: newJson });
  await until('new timezone history and older-page control', async () =>
    await exists(page, timelineEvent(46)) && await exists(page, '#timeline-earlier'));
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
  const hide = (hidden) => setDocumentHidden(page, hidden);
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
  await disclosure(page, true);
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
  await disclosure(page, true);
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
  await disclosure(page, true);
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
  await disclosure(page, true);
  fx.gone = true;
  await until('agent gone banner', () => exists(page, '#banner-agent-gone'));
  check(!(await exists(page, '#overflow-button')) && !(await exists(page, '#composer-input')), 'actions still offered for a gone agent');
  check(await page.evaluate(() => document.getElementById('tab-term').disabled), 'screen tab enabled for a gone agent');
  fx.gone = false;
  fx.retarget = true;
  await until('retarget banner', () => exists(page, '#banner-retargeted'), 15000);
  check(await page.getAttribute('#detail-information-toggle', 'aria-expanded') === 'true', 'retarget reset disclosure');
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
  const home = groupRoute('browser-check', 0);
  await until('back to the list', async () => hashOf(page) === home && await exists(page, 'li.agent-card'));
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
  await until('list after deep-link back', async () => hashOf(page) === home && await exists(page, 'li.agent-card'));
  const replaced = await page.evaluate(() => history.length);
  check(replaced === deep, `deep-link back changed history length (${deep} -> ${replaced})`);
}

async function logoutFlows(open) {
  const other = await open();
  await openList(other.page);
  const { page } = await open();
  await openApp(page, '#/', { remember: true });
  await until('agent cards', () => exists(page, 'li.agent-card'));
  await switchGroup(page, 'peer', 0);
  await domClick(page, '#menu-button');
  await until('menu', () => exists(page, '#logout'));
  await domClick(page, '#logout');
  await waitLogin(page);
  await until('signed out notice', async () => ((await textOf(page, '#login-status')) || '').includes('Signed out.'));
  const cleared = await tokenStores(page);
  check(!cleared.local && !cleared.session, 'logout kept a token');
  await signIn(page, { remember: true });
  await until('sign-in returns to the group page', async () =>
    hashOf(page) === groupRoute('peer', 0) && await exists(page, cardSelector('live:peer:9')));
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
  if (process.env.YINZ_WEB_HEADER_BASELINE === '1') {
    for (const width of [320, 390]) for (const scheme of ['light', 'dark']) {
      await scenario(`header characterization ${width}px ${scheme}`, (open) => headerCharacterization(open, width, scheme));
    }
    return;
  }
  await scenario('keyboard-only login, session-only token, repeated entry', keyboardLogin);
  await scenario('remembered login across tabs and contexts', rememberedLogin);
  await scenario('phone layout 320px: overflow, touch targets, text-only server strings', (open) => phoneLayout(open, 320));
  await scenario('phone layout 390px: overflow, touch targets, text-only server strings', (open) => phoneLayout(open, 390));
  await scenario('iPhone 16 Screen key row with defaults', (open) => iphoneScreenKeys(open));
  await scenario('iPhone 16 Screen key row with eight keys', (open) =>
    iphoneScreenKeys(open, ['esc', 'tab', 'enter', 'backspace', 'up', 'down', 'left', 'right']));
  for (const width of [320, 390]) {
    for (const scheme of ['light', 'dark']) await scenario(`compact agent list ${width}px ${scheme}`, (open) => compactAgentList(open, width, scheme));
  }
  await scenario('compact agent list 1024px desktop', (open) => compactAgentList(open, 1024, 'light'));
  await scenario('short phone viewport with a long retry and sparse cached card', cachedAgentList);
  await scenario('Agents group membership, fallback and poll updates', groupedAgentList);
  await scenario('Agents group scroll through history', groupHistoryScroll);
  await scenario('Agents group dialog focus through polls and history', groupDialogFocus);
  await scenario('Agents peer group ownership during session name reuse', peerGroupOwnership);
  await scenario('Agents peer focus across republication: card area', (open) => peerFocusAcrossRepublication(open, '.agent-open-area'));
  await scenario('Agents peer focus across republication: Open', (open) => peerFocusAcrossRepublication(open, '.agent-open'));
  await scenario('Agents peer focus across republication: quick key', (open) => peerFocusAcrossRepublication(open, '.quick-key[data-key="1"]'));
  await scenario('Agents peer Detail routing and draft isolation', scopedPeerDetails);
  await scenario('Agents start, move and close on a phone', manageAgent);
  await scenario('Agents peer desktop Focus response and controls', peerDesktopFocus);
  await scenario('Agents scoped peer retry through cache loss', scopedPeerRetry);
  await scenario('Agents scoped draft recovery after first lookup failure', scopedPeerInitialLookupFailure);
  await scenario('Agents scoped draft recovery discards each text independently', scopedPeerDiscards);
  await scenario('Agents older peer history and unavailable controls', olderPeerHistory);
  await scenario('Agents Next waiting agent navigation', nextWaitingAgent);
  await scenario('Agents headings for whitespace-colliding session names', whitespaceSessionHeadings);
  await scenario('Agents keyboard focus through automatic group moves and reorders', agentListPollFocus);
  await scenario('Agents keyboard focus guards during automatic updates', agentListPollFocusGuards);
  await scenario('Agents polls respect keyboard focus outside built-in card controls', agentListPollFocusOutsideCards);
  for (const width of [320, 390, 1024]) for (const scheme of ['light', 'dark']) {
    await scenario(`compact detail ${width}px ${scheme}`, (open) => compactDetail(open, width, scheme));
  }
  await scenario('interactive terminal viewport pinning', terminalViewport);
  for (const width of [320, 390]) for (const scheme of ['light', 'dark']) {
    await scenario(`interactive terminal ${width}px ${scheme}`, (open) => terminalInteraction(open, width, scheme));
  }
  await scenario('detail disclosure fallbacks and quota discovery', detailFallbacks);
  for (const width of [320, 390]) for (const scheme of ['light', 'dark']) {
    await scenario(`detail safety ${width}px ${scheme}`, (open) => detailSafety(open, width, scheme));
  }
  await scenario('dark mode changes computed backgrounds', darkMode);
  await scenario('safe-area CSS and manifest', safeAreaManifest);
  await scenario('sheet focus trap, Escape and focus restore; keys and interrupt', sheetsKeyboard);
  await scenario('keyboard composer and past prompts', composerKeyboard);
  await scenario('Timeline response spinner', timelineResponseSpinner);
  await scenario('Timeline response spinner for a peer with recorded history', (open) => peerTimelineHandoff(open, true));
  await scenario('Timeline response spinner for a peer without a recorded run', (open) => peerTimelineHandoff(open, false));
  await scenario('Timeline response spinner confirmation while hidden', hiddenTimelineConfirmation);
  await scenario('readers and prepend keeps the reading anchor', readerPrepend);
  await scenario('cached timeline renders while a reopened chat catches up', cachedTimelineRevisit);
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
