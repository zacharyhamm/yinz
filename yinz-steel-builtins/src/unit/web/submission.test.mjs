// node --test: prompt submission safety (persist-before-fetch, same-id retry,
// no automatic resubmission, clearing only on native "submitted").
import test from 'node:test';
import assert from 'node:assert/strict';
import { FakeClock, FakeFetch, LogStorage, flush, response, staticUrl } from './fakes.mjs';

const { createApi, createTokenStore, safeStorage, MemoryStorage, KEYS } = await import(staticUrl('api.js'));
const { createSubmissions, classifyPromptResult, targetKey, UNKNOWN_TEXT } = await import(staticUrl('submission.js'));

const A = { id: 'live:local:7', kind: 'codex', session_id: 'conv-1' };
const A2 = { id: 'live:local:7', kind: 'codex', session_id: 'conv-2' };

function setup({ fail = null, storage: existing = null } = {}) {
  const log = [];
  const clock = new FakeClock();
  const session = existing || new LogStorage(log, { fail });
  session.log = log;
  const tokens = createTokenStore({ local: safeStorage(() => new MemoryStorage()), session: safeStorage(() => new MemoryStorage()) });
  tokens.set('yzw1_t', false);
  const fetch = new FakeFetch(log);
  const api = createApi({ fetch: fetch.fetch, tokens, now: clock.now, setTimeout: clock.setTimeout, clearTimeout: clock.clearTimeout });
  let n = 0;
  const subs = createSubmissions({ storage: safeStorage(() => session), api, uuid: () => `uuid-${++n}`, now: clock.now });
  return { log, clock, session, fetch, api, subs };
}

const ok = (outcome, submission_id, extra = {}) => response(200, { response: { outcome, submission_id, acknowledged: true, ...extra } });

test('persists the safety record before the prompt fetch, with the exact body', async () => {
  const { log, session, fetch, subs } = setup();
  subs.setDraft(A, 'hello\nworld');
  const held = fetch.hold();
  const sending = subs.send(A);
  await flush();
  const key = targetKey(A);
  const lastWrite = log.map((e, i) => [e, i]).filter(([e]) => e[0] === 'set' && e[1] === key).pop()[1];
  const fetchAt = log.findIndex((e) => e[0] === 'fetch');
  assert.ok(lastWrite < fetchAt, 'storage write precedes fetch');
  const stored = JSON.parse(session.map.get(key));
  assert.deepEqual(stored.target, A);
  assert.deepEqual(stored.pending && [stored.pending.submission_id, stored.pending.text, stored.pending.state], ['uuid-1', 'hello\nworld', 'sending']);
  assert.equal(fetch.calls.length, 1);
  assert.equal(fetch.calls[0].url, '/api/agent/prompt');
  assert.deepEqual(fetch.calls[0].body, { agent: A, submission_id: 'uuid-1', text: 'hello\nworld' });
  assert.equal(subs.view(A).state, 'sending');
  // A second Send while pending is refused without a request.
  assert.equal((await subs.send(A)).ok, false);
  held.resolve(ok('submitted', 'uuid-1'));
  await sending;
  assert.equal(fetch.calls.length, 1);
  assert.equal(subs.view(A).state, 'idle');
  assert.equal(subs.get(A).draft, '');
  assert.equal(session.map.has(key), false);
});

test('persistence failure blocks Send and keeps the draft in memory', async () => {
  const { fetch, subs } = setup({ fail: 'quota' });
  const d = subs.setDraft(A, 'keep me');
  assert.equal(d.ok, false);
  assert.equal(subs.persistenceProblem(A), 'quota');
  const r = await subs.send(A);
  assert.equal(r.ok, false);
  assert.equal(r.persistence, true);
  assert.equal(fetch.calls.length, 0);
  assert.equal(subs.get(A).draft, 'keep me');
  assert.equal(subs.get(A).pending, null);
  // Read-back mismatch (silent storage) also blocks.
  const silent = setup({ storage: new LogStorage([], { readback: false }) });
  silent.subs.setDraft(A, 'x');
  assert.equal((await silent.subs.send(A)).ok, false);
  assert.equal(silent.fetch.calls.length, 0);
});

test('only "submitted" with the matching id clears; 2xx/acknowledged/started never do', async () => {
  for (const [label, reply] of [
    ['ok outcome', ok('ok', 'uuid-1')],
    ['completed', ok('completed', 'uuid-1')],
    ['started failure', ok('failed', 'uuid-1', { prompt_delivery: 'started' })],
    ['missing delivery', ok('failed', 'uuid-1')],
    ['timed out', ok('timed_out', 'uuid-1', { prompt_delivery: 'started' })],
    ['other id', ok('submitted', 'uuid-other')],
    ['malformed', response(200, { nope: true })],
    ['not json', response(200, 'garbage')],
    ['504', response(504, { error: { code: 'timeout' } })],
    ['502', response(502, { error: { code: 'plugin_failed' } })],
    ['network', new TypeError('down')],
  ]) {
    const { fetch, subs } = setup();
    subs.setDraft(A, 'text');
    fetch.push(reply);
    await subs.send(A);
    const v = subs.view(A);
    assert.equal(v.state, 'unknown', label);
    assert.ok(v.message.startsWith(UNKNOWN_TEXT), label);
    assert.equal(v.submission_id, 'uuid-1', label);
    assert.equal(v.text, 'text', label);
    assert.equal(fetch.calls.length, 1, `${label}: no automatic retry`);
  }
});

test('abort/timeout of a prompt is uncertain, and retry reuses the same id and text', async () => {
  const { fetch, subs, clock } = setup();
  subs.setDraft(A, 'once');
  fetch.hold();
  const p = subs.send(A);
  await flush();
  await clock.advance(45000);
  await p;
  assert.equal(subs.view(A).state, 'unknown');
  // Editing creates a separate draft; the pending attempt is immutable.
  subs.setDraft(A, 'once, edited');
  assert.equal(subs.get(A).pending.text, 'once');
  fetch.push(ok('submitted', 'uuid-1'));
  await subs.retry(A);
  assert.equal(fetch.calls.length, 2);
  assert.deepEqual(fetch.calls[1].body, fetch.calls[0].body);
  assert.deepEqual(fetch.calls[1].body, { agent: A, submission_id: 'uuid-1', text: 'once' });
  // Submitted clears the attempt but not text edited since.
  assert.equal(subs.view(A).state, 'idle');
  assert.equal(subs.get(A).draft, 'once, edited');
});

test('definite failures keep the draft and allow an explicit new attempt with a new id', async () => {
  for (const reply of [
    ok('rejected', 'uuid-1', { message: 'Agent is not waiting for input' }),
    ok('failed', 'uuid-1', { prompt_delivery: 'not_sent', message: 'busy' }),
    response(400, { error: { code: 'invalid', message: 'text must be nonblank' } }),
    response(429, { error: { code: 'rate_limited' } }, { 'Retry-After': '3' }),
  ]) {
    const { fetch, subs } = setup();
    subs.setDraft(A, 'again');
    fetch.push(reply);
    await subs.send(A);
    assert.equal(subs.view(A).state, 'failed');
    assert.equal(subs.get(A).draft, 'again');
    fetch.push(ok('submitted', 'uuid-2'));
    await subs.send(A);
    assert.equal(fetch.calls[1].body.submission_id, 'uuid-2');
    assert.equal(subs.view(A).state, 'idle');
  }
});

test('rehydration after reload never auto-submits; sending becomes unknown', async () => {
  const first = setup();
  first.subs.setDraft(A, 'survive');
  first.fetch.hold();
  first.subs.send(A); // page "dies" mid-flight
  await flush();
  const stored = first.session;
  const second = setup({ storage: stored });
  const v = second.subs.view(A);
  assert.equal(v.state, 'unknown');
  assert.equal(v.submission_id, 'uuid-1');
  assert.equal(v.text, 'survive');
  await flush();
  assert.equal(second.fetch.calls.length, 0, 'no automatic resubmission');
  assert.equal(JSON.parse(stored.map.get(targetKey(A))).pending.state, 'unknown');
  second.fetch.push(ok('submitted', 'uuid-1'));
  await second.subs.retry(A);
  assert.equal(second.fetch.calls[0].body.submission_id, 'uuid-1');
});

test('corrupt or foreign records are dropped, not sent', async () => {
  const s = new LogStorage([]);
  s.map.set(targetKey(A), '{not json');
  const { subs, fetch } = setup({ storage: s });
  assert.equal(subs.view(A).state, 'idle');
  assert.equal(s.map.has(targetKey(A)), false);
  s.map.set(targetKey(A), JSON.stringify({ v: 1, target: A2, draft: 'x', pending: { submission_id: 'z', text: 'x', state: 'unknown' } }));
  const again = setup({ storage: s });
  assert.equal(again.subs.view(A).state, 'idle');
  assert.equal(fetch.calls.length + again.fetch.calls.length, 0);
});

test('409 and target outcomes disable the original attempt; adopting needs a user decision', async () => {
  const { fetch, subs } = setup();
  subs.setDraft(A, 'for conv-1');
  fetch.push(response(409, { error: { code: 'target_changed', message: 'Agent disappeared or changed identity' } }));
  await subs.send(A);
  assert.equal(subs.view(A).state, 'incompatible');
  assert.equal(subs.sendProblem(A), 'This draft belongs to a previous agent identity.');
  assert.equal((await subs.send(A)).ok, false);
  assert.equal((await subs.retry(A)).ok, false);
  assert.equal(fetch.calls.length, 1);
  // The stale record is discoverable for the same pane id, not for the current identity.
  assert.deepEqual(subs.staleRecords(A.id, A2).map((r) => r.target), [A]);
  assert.deepEqual(subs.staleRecords(A.id, A), []);
  const fresh = subs.adopt(A, A2);
  assert.equal(fresh.draft, 'for conv-1');
  assert.equal(subs.get(A).pending.submission_id, 'uuid-1', 'original record kept');
  assert.equal(subs.get(A).pending.text, 'for conv-1');
  fetch.push(ok('submitted', 'uuid-2'));
  await subs.send(A2);
  assert.deepEqual(fetch.calls[1].body, { agent: A2, submission_id: 'uuid-2', text: 'for conv-1' });
  subs.discard(A);
  assert.deepEqual(subs.staleRecords(A.id, A2), []);

  const outcome = setup();
  outcome.subs.setDraft(A, 'x');
  outcome.fetch.push(ok('target_changed', 'uuid-1', { prompt_delivery: 'not_sent' }));
  await outcome.subs.send(A);
  assert.equal(outcome.subs.view(A).state, 'incompatible');
});

test('retarget while a prompt is in flight: replacement never receives it', async () => {
  const { fetch, subs } = setup();
  subs.setDraft(A, 'racing');
  const held = fetch.hold();
  const p = subs.send(A);
  await flush();
  subs.markIncompatible(A, { replacement: A2, reason: 'The agent started a new conversation.' });
  held.resolve(ok('timed_out', 'uuid-1', { prompt_delivery: 'started' }));
  await p;
  const v = subs.view(A);
  assert.equal(v.state, 'incompatible');
  assert.equal(subs.get(A).pending.submission_id, 'uuid-1');
  assert.equal(subs.get(A2).draft, '');
  assert.equal(fetch.calls.length, 1);
  assert.equal(subs.sendProblem(A2), 'Write a prompt first.');
});

test('reopening a disabled record keeps the text and needs a new explicit Send', async () => {
  const { fetch, subs } = setup();
  subs.setDraft(A, 'after retarget');
  fetch.push(ok('timed_out', 'uuid-1'));
  await subs.send(A);
  subs.markIncompatible(A, { reason: 'The agent moved to a new run.' });
  assert.equal(subs.view(A).state, 'incompatible');
  subs.setDraft(A, '');
  subs.reopen(A);
  assert.equal(subs.view(A).state, 'idle');
  assert.equal(subs.get(A).draft, 'after retarget');
  assert.equal(fetch.calls.length, 1, 'reopen never sends');
  fetch.push(ok('submitted', 'uuid-2'));
  await subs.send(A);
  assert.equal(fetch.calls[1].body.submission_id, 'uuid-2');
});

test('late replies for a discarded attempt are ignored', async () => {
  const { fetch, subs } = setup();
  subs.setDraft(A, 'late');
  const held = fetch.hold();
  const p = subs.send(A);
  await flush();
  subs.discard(A);
  held.resolve(ok('submitted', 'uuid-1'));
  const r = await p;
  assert.equal(r.stale, true);
  assert.equal(subs.get(A).draft, 'late');
  assert.equal(subs.view(A).state, 'idle');
});

test('auth loss during sending is uncertain and keeps identity', async () => {
  const { fetch, subs } = setup();
  subs.setDraft(A, 'auth');
  fetch.push(response(401, { error: { code: 'unauthenticated' } }));
  await subs.send(A);
  const v = subs.view(A);
  assert.equal(v.state, 'unknown');
  assert.equal(v.submission_id, 'uuid-1');
});

test('prompt validation mirrors native rules without a request', async () => {
  const { fetch, subs } = setup();
  subs.setDraft(A, '   ');
  assert.equal((await subs.send(A)).ok, false);
  subs.setDraft(A, 'bell\u0007');
  assert.equal((await subs.send(A)).ok, false);
  subs.setDraft(A, 'x'.repeat(32769));
  assert.equal((await subs.send(A)).ok, false);
  subs.setDraft(A, 'fine');
  assert.equal((await subs.send(A, { canPrompt: false, reason: 'Agent has no conversation ID' })).reason, 'Agent has no conversation ID');
  assert.equal(fetch.calls.length, 0);
  assert.equal(classifyPromptResult({ ok: true, data: { response: { outcome: 'submitted', submission_id: 'a' } } }, 'a').state, 'submitted');
  assert.equal(KEYS.submissionPrefix, 'yinz.web.v1.submission:');
});
