// node --test: transport, credentials, polling and static-tree policy.
import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { FakeClock, FakeFetch, LogStorage, STATIC_DIR, flush, response, staticUrl } from './fakes.mjs';

const api = await import(staticUrl('api.js'));
const {
  KEYS, createApi, createPoller, createTokenStore, safeStorage, requestUrl, parseRetryAfter,
  describeError, normalizeConfig, saveReturn, takeReturn, logoutNotice, freshnessText,
} = api;

function setup({ token = 'yzw1_token', remember = false, fail = null } = {}) {
  const log = [];
  const clock = new FakeClock();
  const local = new LogStorage(log, { fail });
  const session = new LogStorage(log, { fail });
  const tokens = createTokenStore({ local: safeStorage(() => local), session: safeStorage(() => session) });
  if (token) tokens.set(token, remember);
  const fetch = new FakeFetch(log);
  const events = [];
  const client = createApi({
    fetch: fetch.fetch,
    tokens,
    now: clock.now,
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    onUnauthenticated: (e) => events.push(e),
  });
  return { log, clock, local, session, tokens, fetch, client, events };
}

test('bearer only in the Authorization header of same-origin relative requests', async () => {
  const { fetch, client } = setup();
  fetch.push(response(200, { ok: 1 }));
  const r = await client.get('/api/agents');
  assert.equal(r.ok, true);
  const call = fetch.calls[0];
  assert.equal(call.url, '/api/agents');
  assert.equal(call.init.headers.Authorization, 'Bearer yzw1_token');
  assert.equal(call.init.credentials, 'same-origin');
  assert.ok(!call.url.includes('yzw1'));
  await assert.rejects(() => client.get('https://evil.example/api'), /same-origin/);
  await assert.rejects(() => client.get('//evil.example/api'), /same-origin/);
  assert.equal(fetch.calls.length, 1);
});

test('token storage: remember uses localStorage and removes the other store', () => {
  const { local, session, tokens } = setup({ token: null });
  session.map.set(KEYS.token, 'yzw1_old');
  const r = tokens.set('yzw1_new', true);
  assert.deepEqual([r.ok, r.remembered, r.persisted], [true, true, true]);
  assert.equal(local.map.get(KEYS.token), 'yzw1_new');
  assert.equal(session.map.has(KEYS.token), false);
  tokens.set('yzw1_tab', false);
  assert.equal(session.map.get(KEYS.token), 'yzw1_tab');
  assert.equal(local.map.has(KEYS.token), false);
  // Corrupt values are dropped rather than sent.
  session.map.set(KEYS.token, 'bad token\n');
  assert.equal(tokens.get(), 'yzw1_tab'); // page-memory copy only
  assert.equal(session.map.has(KEYS.token), false);
});

test('token storage failures never claim remember succeeded', () => {
  const log = [];
  const local = new LogStorage(log, { fail: 'quota' });
  const session = new LogStorage(log);
  const tokens = createTokenStore({ local: safeStorage(() => local), session: safeStorage(() => session) });
  const r = tokens.set('yzw1_x', true);
  assert.equal(r.ok, true);
  assert.equal(r.remembered, false);
  assert.match(r.warning, /this tab only/);
  const denied = createTokenStore({
    local: safeStorage(() => { throw new Error('SecurityError'); }),
    session: safeStorage(() => { throw new Error('SecurityError'); }),
  });
  const d = denied.set('yzw1_x', false);
  assert.equal(d.ok, false);
  assert.equal(d.persisted, false);
  assert.match(d.warning, /blocking site storage/);
  assert.equal(denied.get(), 'yzw1_x');
  assert.deepEqual(denied.clear(), { ok: false });
});

test('401 clears the bearer, saves the route and fires once per auth generation', async () => {
  const { fetch, client, tokens, session, events } = setup();
  fetch.push(
    response(401, { error: { code: 'unauthenticated' } }, { 'WWW-Authenticate': 'Bearer' }),
    response(401, { error: { code: 'unauthenticated' } }),
  );
  const gen = client.authGeneration;
  const [a, b] = await Promise.all([client.get('/api/agents'), client.get('/api/history?run=r')]);
  assert.equal(a.status, 401);
  assert.equal(b.status, 401);
  assert.equal(events.length, 1);
  assert.equal(tokens.get(), null);
  assert.ok(client.stale(a));
  assert.equal(client.authGeneration, gen + 1);
  assert.equal(saveReturn(safeStorage(() => session), '#/a/x?run=r&tab=screen'), true);
  assert.equal(takeReturn(safeStorage(() => session)), '#/a/x?run=r&tab=screen');
  assert.equal(takeReturn(safeStorage(() => session)), '');
  // Unsafe hashes are not stored.
  saveReturn(safeStorage(() => session), 'javascript:alert(1)');
  assert.equal(takeReturn(safeStorage(() => session)), '');
});

test('logout: empty 204 without JSON parsing; failure still clears local credentials', async () => {
  const ok = setup();
  ok.fetch.push(response(204));
  const r = await ok.client.logout();
  assert.equal(r.confirmed, true);
  assert.deepEqual(ok.fetch.calls[0].body, {});
  assert.equal(ok.fetch.calls[0].url, '/api/logout');
  assert.equal(ok.fetch.calls[0].init.headers['Content-Type'], 'application/json');
  assert.equal(ok.tokens.get(), null);
  assert.equal(logoutNotice(r), 'Signed out.');

  const offline = setup();
  offline.fetch.push(new TypeError('network down'));
  const all = await offline.client.logout({ all: true });
  assert.equal(offline.fetch.calls[0].url, '/api/logout-all');
  assert.equal(all.confirmed, false);
  assert.equal(offline.tokens.get(), null);
  assert.equal(offline.events.length, 0);
  assert.match(logoutNotice({ ...all, all: true }), /revocation is unconfirmed/);
});

test('deadlines: GET 10 s, POST 45 s; abort is a timeout, not cancellation', async () => {
  const { fetch, client, clock } = setup();
  fetch.hold();
  const get = client.get('/api/agents');
  await flush();
  assert.deepEqual(clock.pending(), [10000]);
  await clock.advance(10000);
  assert.equal((await get).kind, 'timeout');
  fetch.hold();
  const post = client.post('/api/agent/keys', { agent: {}, keys: ['esc'] });
  await flush();
  assert.deepEqual(clock.pending(), [45000]);
  await clock.advance(45000);
  assert.equal((await post).kind, 'timeout');
  // Caller cancellation is distinguished from a deadline.
  fetch.hold();
  const c = new AbortController();
  const aborted = client.get('/api/agents', { signal: c.signal });
  c.abort();
  assert.equal((await aborted).kind, 'aborted');
});

test('error text: optional message with code fallbacks and Retry-After', () => {
  assert.equal(describeError({ ok: false, kind: 'http', status: 503, code: 'no_password' }), 'No web password is set. Run `yinz set-password` on the host.');
  assert.match(describeError({ ok: false, kind: 'http', status: 429, code: 'rate_limited', retryAfterMs: 61000 }), /Try again in 61 s/);
  assert.equal(describeError({ ok: false, kind: 'http', status: 404, code: 'not_found', message: 'Agent not found' }), 'Agent not found');
  assert.equal(describeError({ ok: false, kind: 'http', status: 503, code: 'updating' }), 'Yinz is updating.');
  assert.equal(describeError({ ok: false, kind: 'http', status: 413, code: null }), 'The request is too large.');
  assert.equal(describeError({ ok: false, kind: 'http', status: 418, code: null }), 'Request failed (HTTP 418).');
  assert.equal(parseRetryAfter('5', 0), 5000);
  assert.equal(parseRetryAfter(new Date(10_000).toUTCString(), 4000), 6000);
  assert.equal(parseRetryAfter('soon', 0), null);
});

test('requestUrl converts native navigation requests and reapplies tz', () => {
  assert.equal(
    requestUrl({ op: 'history', run: 'R', agent: 'live:s:7', worker: null, before: null, after_revision: 42 }, 'UTC'),
    '/api/history?run=R&agent=live%3As%3A7&after_revision=42&tz=UTC',
  );
  const earlier = requestUrl({ op: 'history', run: 'R', worker: 'w', before: { created_at_ms: null, revision: 2 }, after_revision: null }, null);
  const u = new URL(earlier, 'http://x');
  assert.deepEqual(JSON.parse(u.searchParams.get('before')), { created_at_ms: null, revision: 2 });
  assert.equal(u.searchParams.has('after_revision'), false);
  assert.equal(requestUrl({ op: 'history', run: 'R', from: null, query: '' }), '/api/history?run=R');
  assert.equal(requestUrl({ op: 'tools', run: 'R', revisions: [5, 10] }, 'UTC'), '/api/tools?run=R&revisions=5%2C10&tz=UTC');
  const tools = new URL(requestUrl({ op: 'tools', run: 'R', range: { start: null, end: { created_at_ms: 9, revision: 3 }, worker: null }, before: null }), 'http://x');
  assert.deepEqual(JSON.parse(tools.searchParams.get('range')), { start: null, end: { created_at_ms: 9, revision: 3 }, worker: null });
  assert.throws(() => requestUrl({ op: 'resume', run: 'R' }));
});

function poller(client, clock, fetch, opts = {}) {
  let hidden = false;
  const updates = [];
  const p = createPoller({
    run: ({ signal }) => client.get('/api/agents', { signal }),
    intervalMs: 3000,
    now: clock.now,
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    isHidden: () => hidden,
    onUpdate: (s) => updates.push(s),
    ...opts,
  });
  return { p, updates, setHidden: (h) => { hidden = h; } };
}

test('polling is single-flight: manual refresh joins the active request', async () => {
  const { fetch, client, clock } = setup();
  const held = fetch.hold();
  const { p } = poller(client, clock, fetch);
  const first = p.start();
  const manual = p.refresh();
  const again = p.refresh();
  await flush();
  assert.equal(fetch.calls.length, 1);
  held.resolve(response(200, { agents: [] }));
  await Promise.all([first, manual, again]);
  assert.equal(fetch.calls.length, 1);
  assert.deepEqual(clock.pending(), [3000]);
  fetch.push(response(200, { agents: [1] }));
  await clock.advance(3000);
  assert.equal(fetch.calls.length, 2);
  assert.deepEqual(p.state.data, { agents: [1] });
  p.stop();
  assert.deepEqual(clock.pending(), []);
});

test('hidden documents pause polling; foreground refreshes immediately', async () => {
  const { fetch, client, clock } = setup();
  const { p, setHidden } = poller(client, clock, fetch);
  fetch.push(response(200, { n: 1 }));
  await p.start();
  setHidden(true);
  p.visibilityChanged();
  assert.deepEqual(clock.pending(), []);
  await clock.advance(60000);
  assert.equal(fetch.calls.length, 1);
  setHidden(false);
  fetch.push(response(200, { n: 2 }));
  p.visibilityChanged();
  await flush();
  assert.equal(fetch.calls.length, 2);
  assert.deepEqual(p.state.data, { n: 2 });
  p.stop();
});

test('backoff 2 s doubling to a 30 s bound, Retry-After honored, no storms', async () => {
  const { fetch, client, clock } = setup();
  const { p } = poller(client, clock, fetch);
  const delays = [];
  for (let i = 0; i < 6; i += 1) fetch.push(new TypeError('offline'));
  await p.start();
  for (let i = 0; i < 5; i += 1) {
    delays.push(clock.pending()[0]);
    await clock.advance(clock.pending()[0]);
  }
  delays.push(clock.pending()[0]);
  assert.deepEqual(delays, [2000, 4000, 8000, 16000, 30000, 30000]);
  assert.equal(p.state.status, 'offline');
  assert.match(freshnessText({ ...p.state, updatedAt: clock.now() - 120000 }, clock.now()), /^Offline · updated 2m ago$/);
  // Manual refresh during backoff (no Retry-After) issues one request, not a storm.
  fetch.push(response(503, { error: { code: 'busy' } }, { 'Retry-After': '7' }));
  await p.refresh();
  assert.equal(fetch.calls.length, 7);
  assert.deepEqual(clock.pending(), [7000]);
  await p.refresh();
  await p.refresh();
  assert.equal(fetch.calls.length, 7, 'refresh waits out Retry-After');
  assert.match(freshnessText(p.state, clock.now()), /Retrying in 7s/);
  fetch.push(response(200, { ok: true }));
  await clock.advance(7000);
  assert.equal(fetch.calls.length, 8);
  assert.equal(p.state.status, 'ok');
  assert.deepEqual(clock.pending(), [3000]);
  p.stop();
});

test('stopped or superseded pollers fence late completions and cancel work', async () => {
  const { fetch, client, clock } = setup();
  const held = fetch.hold();
  const { p, updates } = poller(client, clock, fetch);
  p.start();
  await flush();
  const signal = fetch.calls[0].init.signal;
  p.stop(); // navigation away
  assert.equal(signal.aborted, true);
  held.resolve(response(200, { late: true }));
  await flush();
  assert.equal(p.state.data, undefined);
  assert.ok(updates.every((u) => u.data === undefined));
  assert.deepEqual(clock.pending(), []);
});

test('auth generation fences results produced before re-login', async () => {
  const { fetch, client } = setup();
  const held = fetch.hold();
  const pending = client.get('/api/agents');
  await flush();
  client.bumpAuthGeneration();
  held.resolve(response(200, { agents: ['old'] }));
  const r = await pending;
  assert.equal(r.ok, true);
  assert.equal(client.stale(r), true);
});

test('401 stops polling instead of retrying', async () => {
  const { fetch, client, clock } = setup();
  const { p } = poller(client, clock, fetch);
  fetch.push(response(401, { error: { code: 'unauthenticated' } }));
  await p.start();
  assert.equal(p.state.status, 'unauthenticated');
  assert.deepEqual(clock.pending(), []);
});

test('bootstrap config accepts only the whitelist', () => {
  const c = normalizeConfig({ title: 'x'.repeat(65), poll: { list_ms: 5 }, quick_keys: ['esc', 'rm -rf', '1'], secret: 'no' });
  assert.equal(c.title, 'yinz');
  assert.equal(c.poll.list_ms, 3000);
  assert.deepEqual(c.quick_keys, ['esc', '1']);
  assert.equal('secret' in c, false);
  assert.deepEqual(normalizeConfig(null).timeouts, { get_ms: 10000, post_ms: 45000 });
});

// ---- static tree policy -------------------------------------------------------

const SERVED = /\.(js|mjs|css|svg|png|ico|webmanifest|json|woff2|txt)$/;

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

function specifiers(source) {
  const out = [];
  const re = /(?:^|[;\s}])(?:import|export)\s*(?:[\w*{}\s,$]+?\s*from\s*)?["']([^"']+)["']|import\s*\(\s*(["'`]?)([^"'`)]*)\2\s*\)/g;
  let m;
  while ((m = re.exec(source))) out.push(m[1] ?? `dynamic:${m[2]}${m[3]}`);
  // Minified vendor files: `from"x"` without whitespace.
  for (const f of source.matchAll(/from\s*"([^"]+)"/g)) out.push(f[1]);
  return [...new Set(out)];
}

test('static import graph resolves to local files with relative specifiers only', () => {
  const entries = ['app.js', 'login.js'];
  const seen = new Set();
  const queue = entries.map((e) => path.join(STATIC_DIR, e));
  while (queue.length) {
    const file = queue.pop();
    if (seen.has(file)) continue;
    seen.add(file);
    assert.ok(fs.existsSync(file), `missing module ${path.relative(STATIC_DIR, file)}`);
    const source = fs.readFileSync(file, 'utf8');
    for (const spec of specifiers(source)) {
      assert.ok(!spec.startsWith('dynamic:'), `dynamic import in ${file}: ${spec}`);
      assert.match(spec, /^\.\.?\//, `non-relative specifier ${spec} in ${path.relative(STATIC_DIR, file)}`);
      const target = path.resolve(path.dirname(file), spec);
      assert.ok(target.startsWith(STATIC_DIR + path.sep), `escapes static tree: ${spec}`);
      assert.ok(SERVED.test(target), `unserved extension: ${spec}`);
      queue.push(target);
    }
  }
  for (const m of ['api.js', 'timeline.js', 'submission.js', 'views/ui.js', 'views/list.js', 'views/detail.js', 'views/sheets.js',
    'vendor/preact.module.js', 'vendor/hooks.module.js', 'vendor/htm.module.js']) {
    assert.ok(seen.has(path.join(STATIC_DIR, m)), `${m} not reachable from entries`);
  }
});

test('static tree stays within served limits and CSP policy', () => {
  const files = walk(STATIC_DIR);
  const served = files.filter((f) => SERVED.test(f));
  let total = 0;
  for (const f of served) {
    const size = fs.statSync(f).size;
    total += size;
    assert.ok(size <= 2 * 1024 * 1024, `${f} too large`);
    assert.ok(path.relative(STATIC_DIR, f).split(path.sep).length <= 6, `${f} too deep`);
  }
  assert.ok(served.length <= 256);
  assert.ok(total <= 8 * 1024 * 1024);
  for (const required of ['app.js', 'login.js', 'app.css', 'manifest.webmanifest', 'icon.svg', 'icon-192.png', 'icon-512.png',
    'vendor/LICENSE-preact.txt', 'vendor/LICENSE-htm.txt']) {
    assert.ok(served.includes(path.join(STATIC_DIR, required)), `missing ${required}`);
  }
  const app = served.filter((f) => /\.m?js$/.test(f) && !f.includes(`${path.sep}vendor${path.sep}`));
  for (const f of [...app, ...served.filter((f) => f.includes(`${path.sep}vendor${path.sep}`) && f.endsWith('.js'))]) {
    const s = fs.readFileSync(f, 'utf8');
    assert.ok(!/\beval\s*\(|new\s+Function\s*\(/.test(s), `eval/new Function in ${f}`);
    assert.ok(!/import\s*\(/.test(s), `dynamic import in ${f}`);
  }
  for (const f of app) {
    const s = fs.readFileSync(f, 'utf8');
    const rel = path.relative(STATIC_DIR, f);
    const dangerous = (s.match(/dangerouslySetInnerHTML/g) || []).length;
    assert.equal(dangerous, rel === path.join('views', 'ui.js') ? 1 : 0, `dangerouslySetInnerHTML in ${rel}`);
    assert.ok(!/\.innerHTML|outerHTML|insertAdjacentHTML|document\.write/.test(s), `HTML sink in ${rel}`);
    assert.ok(!/\bstyle\s*=|\bstyle:\s|setAttribute\(\s*['"]style|\.style\./.test(s), `inline style in ${rel}`);
    assert.ok(!/\son[a-z]+\s*=\s*["']/.test(s), `on* attribute markup in ${rel}`);
    assert.ok(!/console\.(log|info|debug|warn|error)/.test(s), `console logging in ${rel}`);
  }
  const manifest = JSON.parse(fs.readFileSync(path.join(STATIC_DIR, 'manifest.webmanifest'), 'utf8'));
  assert.equal(manifest.start_url, '/');
  assert.equal(manifest.scope, '/');
  assert.equal(manifest.display, 'standalone');
  const png = (name) => {
    const b = fs.readFileSync(path.join(STATIC_DIR, name));
    assert.equal(b.subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
    return [b.readUInt32BE(16), b.readUInt32BE(20)];
  };
  for (const icon of manifest.icons) {
    const name = icon.src.replace('/static/', '');
    assert.ok(fs.existsSync(path.join(STATIC_DIR, name)), icon.src);
    if (icon.type === 'image/png') {
      const [w, h] = png(name);
      assert.equal(icon.sizes, `${w}x${h}`);
    } else {
      assert.equal(icon.type, 'image/svg+xml');
      assert.equal(icon.sizes, 'any');
    }
  }
  assert.ok(!files.some((f) => /service-?worker|sw\.js$/.test(f)), 'no service worker');
});

test('view modules import without a DOM; DOM entry points parse as ES modules', async () => {
  const { execFileSync } = await import('node:child_process');
  for (const m of ['views/ui.js', 'views/sheets.js', 'views/list.js', 'views/detail.js', 'views/composer.js', 'views/keys.js', 'app-route.js']) {
    const mod = await import(staticUrl(m));
    assert.ok(Object.keys(mod).length > 0, m);
  }
  for (const entry of ['app.js', 'login.js']) {
    execFileSync(process.execPath, ['--input-type=module', '--check'], { input: fs.readFileSync(path.join(STATIC_DIR, entry)) });
  }
});

test('hash routes round-trip and never accept malformed ids', async () => {
  const { parseHash, detailHash } = await import(staticUrl('app-route.js'));
  const hash = detailHash({ id: 'live:dev:7', run: 'run 1', tab: 'screen' });
  assert.equal(hash, '#/a/live%3Adev%3A7?run=run+1&tab=screen');
  assert.deepEqual(parseHash(hash), { view: 'detail', id: 'live:dev:7', run: 'run 1', tab: 'screen', bind: true });
  assert.equal(parseHash(detailHash({ id: 'x', run: 'r', bind: false })).bind, false);
  assert.deepEqual(parseHash('#/'), { view: 'list' });
  assert.deepEqual(parseHash('#/a/%E0%A4%A'), { view: 'list' });
  assert.equal(parseHash('#/a/x?tab=evil').tab, null);
});

test('history entry markers: first load, new entries, traversals and foreign state', async () => {
  const { entryState } = await import(staticUrl('app-route.js'));
  const first = entryState(null, null);
  assert.deepEqual(first, { state: { yinzIndex: 0, yinzBack: false }, lastIndex: 0, changed: true });
  const pushed = entryState(null, first.lastIndex);
  assert.deepEqual(pushed, { state: { yinzIndex: 1, yinzBack: true }, lastIndex: 1, changed: true });
  // Traversal back to a known entry keeps its marker and moves the last index.
  const back = entryState(first.state, pushed.lastIndex);
  assert.deepEqual(back, { state: first.state, lastIndex: 0, changed: false });
  // A reload of a deep-linked entry stays without an app entry behind it.
  assert.equal(entryState({ yinzIndex: 4, yinzBack: false }, null).state.yinzBack, false);
  // Foreign keys survive marking; non-object states are replaced.
  assert.deepEqual(entryState({ other: 1 }, 2).state, { other: 1, yinzIndex: 3, yinzBack: true });
  assert.deepEqual(entryState('x', null).state, { yinzIndex: 0, yinzBack: false });
});
