// node --test: timeline cache/navigation parity with the native history adapters.
// Cases are ported from:
// - yinz-plugin-host/src/unit/web_agents.rs (history navigation, retarget, empty
//   activity replacements, archive adapter ties/nulls/late delivery/tools)
// - yinz-plugin-host/src/unit/palette_agents.rs (delivery merge never leaves
//   overlapping groups) and default-plugins/agent-proto/src/unit/conversation.rs
//   (delivered_activity_neighbors_ignore_completion_alternatives)
import test from 'node:test';
import assert from 'node:assert/strict';
import { staticUrl } from './fakes.mjs';

const {
  compareCursor, rangeOverlaps, rangeContains, mergeEvents, displayItems, sortEvents,
  TimelineCache, TimelineStore, ToolsReader,
} = await import(staticUrl('timeline.js'));
const { requestUrl } = await import(staticUrl('api.js'));

const c = (revision, created_at_ms = null) => ({ created_at_ms, revision });
const prose = (revision, created_at_ms = null, kind = 'message') => ({
  revision, revisions: [revision], range: null, kind, created_at_ms, time: '', markdown: `m${revision}`,
  truncated: false, tool_calls: 0, request: { op: 'event', run: 'run', revision },
});
const raw = (revision, kind = 'tool_call', created_at_ms = null, extra = {}) => ({ ...prose(revision, created_at_ms, kind), ...extra });
const group = (revision, range, tool_calls, created_at_ms = null) => ({
  revision, revisions: [], range, kind: 'tools', created_at_ms, time: '', markdown: '', truncated: false, tool_calls,
  request: { op: 'tools', run: 'run', range, before: null },
});
const page = (fields) => ({
  run: 'run', retargeted: false, agent_gone: false, live: null, header: {}, events: [], earlier: false, next_from: null,
  previous_run: null, origin_run: null, resumable: false, status: '', navigation: {}, activity_ranges: [], ...fields,
});

test('reopens a timeline with its older pages and delivery cursor while live state awaits confirmation', () => {
  const cache = new TimelineCache();
  const view = { run: 'run', agent: 'live:local:7' };
  const received = (after_revision) => ({ op: 'history', ...view, after_revision });
  const earlier = { op: 'history', ...view, before: c(20) };
  const openRange = { start: c(20), end: null };
  const first = cache.open(view, 'UTC');
  assert.equal(first.loaded, false);
  assert.deepEqual(first.refreshRequest(), { op: 'history', ...view });
  cache.apply(first, 'UTC', first.ticket('latest'), page({
    live: { id: view.agent },
    events: [prose(20), group(21, openRange, 2)],
    navigation: { earlier, received: received(24) },
  }));
  cache.apply(first, 'UTC', first.ticket('earlier'), page({ events: [prose(1)] }));

  const reopened = cache.open(view, 'UTC');
  assert.equal(reopened.loaded, true);
  assert.equal(reopened.confirmed, false);
  assert.equal(reopened.actionsEnabled, false);
  assert.deepEqual(reopened.items().map((item) => item.key), ['e1', 'e20', 't21']);
  assert.equal(reopened.earlierRequest(), null);
  assert.deepEqual(reopened.refreshRequest(), received(24));

  // The chronological tail may be newer than the supported delivery batch.
  reopened.apply(reopened.ticket('latest'), page({
    live: { id: view.agent },
    events: [prose(90)],
    navigation: { earlier, received: received(91) },
  }));
  assert.deepEqual(reopened.refreshRequest(), received(24));
  const delayedDelivery = reopened.ticket('received');
  const closedRange = { start: c(20), end: c(24) };
  reopened.apply(reopened.ticket('received'), page({
    live: { id: view.agent },
    events: [prose(20), group(22, closedRange, 3), prose(24)],
    activity_ranges: [closedRange],
    navigation: { earlier, received: received(25) },
  }));
  const delayed = reopened.apply(delayedDelivery, page({
    live: { id: view.agent }, navigation: { received: received(6) },
  }));
  assert.equal(delayed.rebuild, undefined);
  assert.equal(reopened.confirmed, true);
  assert.equal(reopened.actionsEnabled, true);
  assert.deepEqual(reopened.items().map((item) => item.key), ['e1', 'e20', 't22', 'e24', 'e90']);
  assert.equal(reopened.earlierRequest(), null);
  assert.deepEqual(reopened.refreshRequest(), received(25));
  assert.deepEqual(first.items().map((item) => item.key), ['e1', 'e20', 't21']);
});

test('cached history is scoped to the run, live binding and timezone across a retarget', () => {
  const cache = new TimelineCache();
  const view = { run: 'run', agent: 'live:local:7' };
  const first = cache.open(view, 'UTC');
  cache.apply(first, 'UTC', first.ticket('latest'), page({
    events: [prose(7)], navigation: { received: { op: 'history', ...view, after_revision: 8 } },
  }));
  assert.equal(cache.open({ run: view.run }, 'UTC').loaded, false);
  assert.equal(cache.open({ ...view, agent: 'live:local:8' }, 'UTC').loaded, false);
  assert.equal(cache.open(view, 'America/Chicago').loaded, false);

  const replacement = { ...view, run: 'replacement' };
  assert.equal(cache.open(replacement, 'UTC').loaded, false);
  const reopened = cache.open(view, 'UTC');
  cache.apply(reopened, 'UTC', reopened.ticket('received'), page({
    run: replacement.run, retargeted: true, events: [prose(2)],
    navigation: { received: { op: 'history', ...replacement, after_revision: 3 } },
  }));
  assert.deepEqual(cache.open(view, 'UTC').events().map((event) => event.revision), [7]);
  assert.deepEqual(cache.open(replacement, 'UTC').events().map((event) => event.revision), [2]);

  cache.clear();
  assert.equal(cache.open(view, 'UTC').loaded, false);
  assert.equal(cache.open(replacement, 'UTC').loaded, false);
});

test('retarget merges into the destination snapshot and continues its delivery stream', () => {
  const cache = new TimelineCache();
  const source = { run: 'run', agent: 'live:local:7' };
  const target = { ...source, run: 'replacement' };
  const request = (view, cursor) => ({ op: 'history', ...view, ...cursor });
  const earlier = request(target, { before: c(10) });
  const received = request(target, { after_revision: 26 });

  const first = cache.open(target, 'UTC');
  cache.apply(first, 'UTC', first.ticket('latest'), page({
    run: target.run, events: [prose(25)],
    navigation: { earlier: request(target, { before: c(25) }), received },
  }));
  cache.apply(first, 'UTC', first.ticket('earlier'), page({
    run: target.run, events: [prose(10)], navigation: { earlier },
  }));

  const store = cache.open(source, 'UTC');
  cache.apply(store, 'UTC', store.ticket('latest'), page({
    events: [prose(900)], navigation: { received: request(source, { after_revision: 901 }) },
  }));
  const stale = store.ticket('earlier');
  const result = cache.apply(store, 'UTC', store.ticket('received'), page({
    run: target.run, retargeted: true, events: [prose(40)],
    navigation: { earlier: request(target, { before: c(40) }), received: request(target, { after_revision: 41 }) },
  }));
  assert.equal(result.retargeted, true);
  assert.deepEqual(store.events().map((event) => event.revision), [10, 25, 40]);
  assert.deepEqual(store.earlierRequest(), earlier);
  assert.deepEqual(store.refreshRequest(), received);
  assert.deepEqual(cache.apply(store, 'UTC', stale, page({ events: [prose(890)] })), { applied: false, reason: 'fenced' });

  const reopened = cache.open(target, 'UTC');
  assert.deepEqual(reopened.events(), store.events());
  assert.deepEqual(reopened.earlierRequest(), earlier);
  assert.deepEqual(reopened.refreshRequest(), received);
  const next = request(target, { after_revision: 28 });
  cache.apply(reopened, 'UTC', reopened.ticket('received'), page({
    run: target.run, events: [prose(27)], navigation: { received: next },
  }));
  assert.deepEqual(reopened.events().map((event) => event.revision), [10, 25, 27, 40]);
  assert.deepEqual(reopened.refreshRequest(), next);
  assert.deepEqual(cache.open(source, 'UTC').events().map((event) => event.revision), [900]);
});

test('evicts explicit unavailable history and starts fresh if the archive returns', () => {
  const cache = new TimelineCache();
  const view = { run: 'run' };
  const request = { op: 'history', ...view };
  const first = cache.open(view, 'UTC');
  cache.apply(first, 'UTC', first.ticket('latest'), page({
    events: [prose(20)], navigation: { earlier: { ...request, before: c(20) }, received: { ...request, after_revision: 21 } },
  }));
  const store = cache.open(view, 'UTC');
  const stale = store.ticket('earlier');
  const missing = cache.apply(store, 'UTC', store.ticket('received'), page({ status: 'No recorded history' }));
  assert.equal(missing.applied, true);
  assert.equal(store.confirmed, true);
  assert.equal(store.meta.status, 'No recorded history');
  assert.deepEqual(store.events(), []);
  assert.equal(store.earlierRequest(), null);
  assert.deepEqual(store.refreshRequest(), request);
  assert.equal(cache.apply(store, 'UTC', stale, page({ events: [prose(19)] })).reason, 'fenced');

  const reopened = cache.open(view, 'UTC');
  assert.equal(reopened.loaded, false);
  assert.deepEqual(reopened.events(), []);
  assert.deepEqual(reopened.refreshRequest(), request);

  const earlier = { ...request, before: c(30) };
  const received = { ...request, after_revision: 31 };
  cache.apply(store, 'UTC', store.ticket('latest'), page({ events: [prose(30)], navigation: { earlier, received } }));
  assert.deepEqual(store.events().map((event) => event.revision), [30]);
  assert.deepEqual(store.earlierRequest(), earlier);
  assert.deepEqual(store.refreshRequest(), received);
  assert.deepEqual(cache.open(view, 'UTC').events(), store.events());
});

test('keeps cached history when the live agent is gone', () => {
  const cache = new TimelineCache();
  const view = { run: 'run', agent: 'live:local:7' };
  const first = cache.open(view, 'UTC');
  const received = { op: 'history', ...view, after_revision: 21 };
  cache.apply(first, 'UTC', first.ticket('latest'), page({ events: [prose(20)], navigation: { received } }));

  const store = cache.open(view, 'UTC');
  cache.apply(store, 'UTC', store.ticket('received'), page({ agent_gone: true, navigation: { received } }));
  assert.equal(store.meta.agent_gone, true);
  assert.deepEqual(store.events(), [prose(20)]);
  const reopened = cache.open(view, 'UTC');
  assert.deepEqual(reopened.events(), store.events());
  assert.deepEqual(reopened.refreshRequest(), received);
});

test('small timelines with delivery cursors share the cache budget', () => {
  const cache = new TimelineCache();
  const runs = Array.from({ length: 24 }, (_, i) => `run-${i}`);
  for (const run of runs) {
    const store = cache.open({ run }, 'UTC');
    cache.apply(store, 'UTC', store.ticket('latest'), page({
      run, navigation: { received: { op: 'history', run, after_revision: 1 } },
    }));
  }
  const unavailable = cache.open({ run: 'missing' }, 'UTC');
  cache.apply(unavailable, 'UTC', unavailable.ticket('latest'), page({ run: 'missing', status: 'No recorded history' }));
  assert.equal(cache.open({ run: 'missing' }, 'UTC').loaded, false);
  for (const run of runs) {
    assert.equal(cache.open({ run }, 'UTC').loaded, true, run);
  }
});

test('Range::overlaps parity: exclusive prose boundaries, open ends, workers independent', () => {
  const r = (s, e, worker = null) => ({ start: s, end: e, worker });
  assert.equal(rangeOverlaps(r(c(1, 1), c(5, 5)), r(c(5, 5), c(9, 9))), false, 'adjacent');
  assert.equal(rangeOverlaps(r(c(1, 1), c(6, 6)), r(c(5, 5), c(9, 9))), true);
  assert.equal(rangeOverlaps(r(null, null), r(c(5, 5), null)), true);
  assert.equal(rangeOverlaps(r(null, c(5, 5)), r(c(5, 5), null)), false);
  assert.equal(rangeOverlaps(r(null, null, 'a'), r(null, null, 'b')), false, 'different workers');
  assert.equal(rangeOverlaps(r(null, null, 'a'), r(null, null, null)), false);
  // Null timestamps sort before dated entries; revision breaks ties.
  assert.ok(compareCursor(c(9, null), c(1, 0)) < 0);
  assert.ok(compareCursor(c(1, 5), c(2, 5)) < 0);
  assert.ok(compareCursor(c(3, 4), c(2, 5)) < 0);
  assert.equal(rangeContains(r(c(1, 1), c(5, 5)), c(5, 5)), false);
  assert.equal(rangeContains(r(c(1, 1), c(5, 5)), c(3, 3)), true);
  assert.equal(rangeContains(r(c(1, 1), c(5, 5), 'w'), c(3, 3), null), false);
});

test('preserves navigation and empty activity replacements (web_agents_history_preserves_navigation_and_empty_activity_replacements)', () => {
  const scope = { start: c(1), end: c(9), worker: 'worker' };
  const store = new TimelineStore({ run: 'run', agent: 'missing', worker: 'worker' });
  const first = page({
    agent_gone: true,
    events: [group(3, scope, 2)],
    activity_ranges: [scope, { start: c(9), end: null, worker: 'worker' }],
    navigation: {
      earlier: { op: 'history', run: 'run', worker: 'worker', before: { created_at_ms: null, revision: 2 }, after_revision: null, agent: 'missing' },
      received: { op: 'history', run: 'run', worker: 'worker', before: null, after_revision: 10, agent: 'missing' },
    },
  });
  assert.equal(store.apply(store.ticket('latest'), first).applied, true);
  const [item] = store.items();
  assert.deepEqual(item.range, scope);
  assert.deepEqual(item.revisions, []);
  assert.equal(item.tool_calls, 2);
  assert.deepEqual(item.request, { op: 'tools', run: 'run', range: scope, before: null });
  assert.equal(store.meta.agent_gone, true);
  assert.equal(store.actionsEnabled, false);
  // Inclusive delivery watermark followed exactly as returned; never newest+1.
  const url = new URL(requestUrl(store.refreshRequest(), 'UTC'), 'http://x');
  assert.equal(url.searchParams.get('after_revision'), '10');
  assert.equal(url.searchParams.get('agent'), 'missing');
  assert.equal(new URL(requestUrl(store.earlierRequest()), 'http://x').searchParams.get('before'), '{"created_at_ms":null,"revision":2}');
  // An empty replacement interval removes the overlapping cached group although
  // no new event occupies it.
  store.apply(store.ticket('received'), page({
    events: [], agent_gone: true, activity_ranges: [{ start: c(2), end: c(4), worker: 'worker' }],
    navigation: { received: { op: 'history', run: 'run', worker: 'worker', before: null, after_revision: 10, agent: 'missing' } },
  }));
  assert.deepEqual(store.items(), []);
  // A replacement for another worker scope leaves this scope alone.
  const other = mergeEvents(new Map([['t3', group(3, scope, 2)]]), [], [{ start: null, end: null, worker: 'other' }], 'worker');
  assert.equal(other.size, 1);
});

test('delivered activity neighbors (conversation.rs) split an open group without overlap', () => {
  // Newest page: prose 1, open-ended group after it.
  const store = new TimelineStore({ run: 'run', worker: 'worker' });
  const open = { start: c(1, 1), end: null, worker: 'worker' };
  store.apply(store.ticket('latest'), page({
    events: [prose(1, 1), group(2, open, 2, 2)],
    activity_ranges: [{ start: null, end: c(1, 1), worker: 'worker' }, open],
    navigation: { received: { op: 'history', run: 'run', worker: 'worker', before: null, after_revision: 5 } },
  }));
  assert.equal(store.items().length, 2);
  // Delivery of prose 5 replaces [1,∞) with [1,5) and [5,∞).
  const ranges = [
    { start: null, end: c(1, 1), worker: 'worker' },
    { start: c(1, 1), end: c(5, 5), worker: 'worker' },
    { start: c(5, 5), end: null, worker: 'worker' },
  ];
  store.apply(store.ticket('received'), page({
    events: [prose(1, 1), group(2, ranges[1], 2, 2), prose(5, 5)],
    activity_ranges: ranges,
    navigation: { received: { op: 'history', run: 'run', worker: 'worker', before: null, after_revision: 6 } },
  }));
  const items = store.items();
  assert.deepEqual(items.map((i) => i.key), ['e1', 't2', 'e5']);
  assert.equal(items[1].tool_calls, 2);
  const groups = items.filter((i) => i.range);
  for (let i = 0; i < groups.length; i += 1) {
    for (let j = 0; j < i; j += 1) assert.equal(rangeOverlaps(groups[i].range, groups[j].range), false);
  }
  assert.equal(store.refreshRequest().after_revision, 6);
});

test('palette delivery loop: repeated deliveries never leave overlapping groups (palette_agents.rs)', () => {
  // Prose at odd revisions, tool calls at even ones, delivered in chunks with
  // replacement intervals around each delivered prose message (activity_ranges).
  const store = new TimelineStore({ run: 'run' });
  const range = (a, b) => ({ start: a === null ? null : c(a, a), end: b === null ? null : c(b, b), worker: null });
  store.apply(store.ticket('latest'), page({
    events: [prose(1, 1), group(2, range(1, null), 1, 2)],
    activity_ranges: [range(null, 1), range(1, null)],
    navigation: { received: { op: 'history', run: 'run', worker: null, before: null, after_revision: 3 } },
  }));
  let next = 3;
  while (next <= 41) {
    const delivered = [];
    for (let r = next; r < next + 10; r += 2) delivered.push(r);
    const events = [];
    const ranges = [];
    for (const r of delivered) {
      events.push(prose(r, r));
      const before = range(r - 2, r);
      const after = range(r, null);
      ranges.push(before, after);
      events.push(group(r - 1, before, 1, r - 1));
    }
    // Open group after the newest delivered prose.
    const last = delivered[delivered.length - 1];
    events.push(group(last + 1, range(last, null), 1, last + 1));
    store.apply(store.ticket('received'), page({
      events, activity_ranges: ranges,
      navigation: { received: { op: 'history', run: 'run', worker: null, before: null, after_revision: last + 2 } },
    }));
    const groups = store.items().filter((i) => i.range);
    for (let i = 0; i < groups.length; i += 1) {
      for (let j = 0; j < i; j += 1) {
        assert.equal(rangeOverlaps(groups[i].range, groups[j].range), false, `delivery from ${next}`);
      }
    }
    next = last + 2;
  }
  const kinds = store.items().map((i) => i.kind);
  assert.equal(kinds.filter((k) => k === 'message').length, 1 + 20);
  assert.equal(kinds.filter((k) => k === 'tools').length, 21);
});

test('chronological order with null timestamps first, revision ties, dedupe and late delivery', () => {
  const store = new TimelineStore({ run: 'archive' });
  store.apply(store.ticket('latest'), page({
    run: 'archive',
    events: [prose(4, 2000), prose(3, 1000), prose(2, null), prose(1, null), prose(6, 3000), prose(7, 3000)],
    navigation: {
      earlier: { op: 'history', run: 'archive', worker: null, before: { created_at_ms: null, revision: 1 }, after_revision: null },
      received: { op: 'history', run: 'archive', worker: null, before: null, after_revision: 8 },
    },
  }));
  assert.deepEqual(store.events().map((e) => e.revision), [1, 2, 3, 4, 6, 7]);
  // Inclusive watermark: the delivery page may repeat revision 8's neighbours; dedupe by revision.
  const late = prose(85, 2500);
  store.apply(store.ticket('received'), page({
    run: 'archive',
    events: [prose(7, 3000), late],
    navigation: { received: { op: 'history', run: 'archive', worker: null, before: null, after_revision: 86 } },
  }));
  assert.deepEqual(store.events().map((e) => e.revision), [1, 2, 3, 4, 85, 6, 7]);
  // An older watermark arriving late never regresses the delivery cursor.
  store.apply(store.ticket('received'), page({
    run: 'archive', events: [],
    navigation: { received: { op: 'history', run: 'archive', worker: null, before: null, after_revision: 8 } },
  }));
  assert.equal(store.refreshRequest().after_revision, 86);
  // earlier is kept from the newest page until an older page replaces it.
  assert.equal(store.earlierRequest().before.revision, 1);
  store.apply(store.ticket('earlier'), page({ run: 'archive', events: [], navigation: {} }));
  assert.equal(store.earlierRequest(), null);
});

test('revisions[] compatibility: tool groups across pages, prose breaks, 30 split, workers', () => {
  const store = new TimelineStore({ run: 'run' });
  // Newest page starts with tool events; the older page ends with more of them.
  store.apply(store.ticket('latest'), page({
    events: [raw(10, 'tool_call', 10), raw(11, 'tool_result', 11), prose(12, 12)],
    navigation: { earlier: { op: 'history', run: 'run', worker: null, before: { created_at_ms: 10, revision: 10 }, after_revision: null } },
  }));
  store.apply(store.ticket('earlier'), page({ events: [prose(5, 5), raw(8, 'tool_call', 8), raw(9, 'tool_result', 9)] }));
  let items = store.items();
  assert.deepEqual(items.map((i) => i.kind), ['message', 'tools', 'message']);
  assert.deepEqual(items[1].revisions, [8, 9, 10, 11]);
  assert.equal(items[1].tool_calls, 2);
  assert.deepEqual(items[1].request, { op: 'tools', run: 'run', revisions: [8, 9, 10, 11] });
  // Split at 30 revisions.
  const many = Array.from({ length: 65 }, (_, i) => raw(100 + i, i % 2 ? 'tool_result' : 'tool_call', 100 + i));
  items = displayItems(sortEvents(many), { run: 'run' });
  assert.deepEqual(items.map((i) => i.revisions.length), [30, 30, 5]);
  assert.equal(items.reduce((n, i) => n + i.tool_calls, 0), 33);
  // Different workers do not merge; ranged groups are passed through, not re-collapsed.
  const mixed = [raw(1, 'tool_call', 1, { worker: 'a' }), raw(2, 'tool_call', 2, { worker: 'b' }),
    group(3, { start: c(2, 2), end: null, worker: null }, 4, 3), raw(4, 'tool_call', 4)];
  items = displayItems(sortEvents(mixed), { run: 'run' });
  assert.deepEqual(items.map((i) => [i.kind, i.compat, i.tool_calls]), [['tools', true, 1], ['tools', true, 1], ['tools', false, 4], ['tools', true, 1]]);
  // A replacement interval removes cached raw tool events inside it.
  const merged = mergeEvents(new Map([['e8', raw(8, 'tool_call', 8)], ['e5', prose(5, 5)]]), [], [{ start: c(5, 5), end: null, worker: null }], null);
  assert.deepEqual([...merged.keys()], ['e5']);
});

test('refresh tracks gone and returned agents (web_agents_history_navigation_refresh_tracks_live_gone_and_returned_agents)', () => {
  const agent = 'live:local:7';
  const recv = (run, after) => ({ op: 'history', run, agent, worker: null, before: null, after_revision: after });
  const store = new TimelineStore({ run: 'run', agent });
  assert.deepEqual(store.refreshRequest(), { op: 'history', run: 'run', agent });
  store.apply(store.ticket('latest'), page({
    live: { id: agent, ref: { id: agent, kind: 'codex', session_id: 's1' } },
    events: [prose(219, 219000), prose(220, 220000)],
    navigation: { received: recv('run', 221), latest: { op: 'history', run: 'run', agent, worker: null, before: null, after_revision: null } },
  }));
  assert.equal(store.actionsEnabled, true);
  // Disappearance: history stays, actions disabled, the refresh keeps the binding.
  store.apply(store.ticket('received'), page({ agent_gone: true, live: null, events: [], navigation: { received: recv('run', 221) } }));
  assert.equal(store.meta.agent_gone, true);
  assert.equal(store.actionsEnabled, false);
  assert.equal(store.events().length, 2);
  assert.equal(store.refreshRequest().agent, agent);
  // Return on a replacement run: retarget replaces the cache and cursors.
  const result = store.apply(store.ticket('received'), page({
    run: 'replacement', retargeted: true, live: { id: agent, ref: { id: agent, kind: 'codex', session_id: 's2' } },
    events: [1, 2, 3, 4].map((r) => prose(r, 2_000_000 + r)),
    navigation: { received: recv('replacement', 5) },
  }));
  assert.equal(result.retargeted, true);
  assert.equal(store.run, 'replacement');
  assert.equal(store.meta.retargetedAt, 'run');
  assert.deepEqual(store.events().map((e) => e.revision), [1, 2, 3, 4]);
  assert.deepEqual(store.refreshRequest(), recv('replacement', 5));
  assert.equal(store.actionsEnabled, true);
});

test('retarget during an older-page fetch fences the stale page and resets cursors', () => {
  const agent = 'live:local:7';
  const store = new TimelineStore({ run: 'run', agent, worker: 'old-worker' });
  store.apply(store.ticket('latest'), page({
    events: [prose(200, 200), prose(201, 201)],
    navigation: {
      earlier: { op: 'history', run: 'run', agent, worker: 'old-worker', before: { created_at_ms: 200, revision: 200 }, after_revision: null },
      received: { op: 'history', run: 'run', agent, worker: 'old-worker', before: null, after_revision: 202 },
    },
  }));
  const older = store.ticket('earlier');
  assert.equal(store.earlierRequest().worker, 'old-worker');
  store.apply(store.ticket('received'), page({
    run: 'replacement', retargeted: true, events: [prose(1, 1)],
    navigation: { received: { op: 'history', run: 'replacement', agent, worker: null, before: null, after_revision: 2 } },
  }));
  assert.deepEqual(store.apply(older, page({ events: [prose(150, 150)] })), { applied: false, reason: 'fenced' });
  assert.deepEqual(store.events().map((e) => e.revision), [1]);
  assert.equal(store.earlierRequest(), null);
  assert.equal(store.refreshRequest().worker, null);
  assert.equal(store.refreshRequest().run, 'replacement');
  // A late latest/received result from before the retarget is also fenced.
  const stale = { gen: store.gen - 1, kind: 'received', run: 'run' };
  assert.equal(store.apply(stale, page({ events: [prose(203, 203)] })).applied, false);
});

test('previous/origin browsing stays unbound; per-event requests never carry agent', () => {
  const agent = 'live:local:7';
  const store = new TimelineStore({ run: 'run', agent });
  store.apply(store.ticket('latest'), page({
    previous_run: 'previous', origin_run: 'original', events: [prose(5, 5)],
    navigation: {
      previous: { op: 'history', run: 'previous', from: null, query: '' },
      origin: { op: 'history', run: 'original', from: null, query: '' },
      latest: { op: 'history', run: 'run', agent, from: null, query: '' },
    },
  }));
  const previous = store.lineageRequest('previous');
  assert.equal(previous.agent, undefined);
  assert.equal(requestUrl(previous, 'UTC'), '/api/history?run=previous&tz=UTC');
  assert.equal(store.latestRequest().agent, agent);
  const stored = new TimelineStore({ run: previous.run });
  assert.deepEqual(stored.initialRequest(), { op: 'history', run: 'previous' });
  stored.apply(stored.ticket('latest'), page({ run: 'previous', events: [prose(1, 1)] }));
  assert.equal(stored.actionsEnabled, false);
  assert.ok(!requestUrl(store.events()[0].request).includes('agent'));
});

test('range tools reader advances bounded pages through page.navigation', () => {
  const range = { start: c(1, 1), end: null, worker: null };
  const reader = new ToolsReader({ op: 'tools', run: 'run', range, before: null });
  const pageOf = (revs, earlier) => ({
    header: 'Retained tools', markdown: '', tool: '', html: '<p>x</p>',
    page: page({ events: revs.map((r) => raw(r, 'tool_call', r)), navigation: earlier ? { earlier } : {} }),
  });
  const next = { op: 'tools', run: 'run', range, before: { created_at_ms: 31, revision: 31 } };
  const t1 = reader.ticket();
  assert.equal(reader.apply(t1, pageOf(Array.from({ length: 30 }, (_, i) => 31 + i), next)), true);
  assert.deepEqual(reader.nextRequest(), next);
  const url = new URL(requestUrl(reader.nextRequest(), 'UTC'), 'http://x');
  assert.deepEqual(JSON.parse(url.searchParams.get('before')), { created_at_ms: 31, revision: 31 });
  assert.equal(reader.apply(reader.ticket(), pageOf([2, 3], null)), true);
  assert.equal(reader.nextRequest(), null);
  assert.equal(reader.records()[0].revision, 2);
  assert.equal(reader.records().length, 32);
});

test('earlier is adopted only from older pages and the first page of a run', () => {
  const agent = 'live:local:7';
  const before = (revision) => ({ op: 'history', run: 'run', agent, worker: null, before: { created_at_ms: revision, revision }, after_revision: null });
  const recv = (run, after) => ({ op: 'history', run, agent, worker: null, before: null, after_revision: after });
  // Exhaustion survives a native delivery reply that repeats the newest page's earlier row.
  const store = new TimelineStore({ run: 'run', agent });
  store.apply(store.ticket('latest'), page({ events: [prose(20, 20), prose(21, 21)], navigation: { earlier: before(20), received: recv('run', 22) } }));
  assert.deepEqual(store.earlierRequest(), before(20));
  store.apply(store.ticket('earlier'), page({ events: [prose(1, 1), prose(19, 19)], navigation: {} }));
  assert.equal(store.earlierRequest(), null);
  store.apply(store.ticket('received'), page({ events: [prose(22, 22)], navigation: { earlier: before(20), received: recv('run', 23) } }));
  assert.equal(store.earlierRequest(), null, 'exhausted history stays exhausted');
  assert.equal(store.refreshRequest().after_revision, 23);
  // A newer earlier cursor from a delivery before any older page does not replace the original.
  const grown = new TimelineStore({ run: 'run', agent });
  grown.apply(grown.ticket('latest'), page({ events: [prose(20, 20)], navigation: { earlier: before(20), received: recv('run', 21) } }));
  grown.apply(grown.ticket('received'), page({ events: [prose(60, 60)], navigation: { earlier: before(40), received: recv('run', 61) } }));
  assert.deepEqual(grown.earlierRequest(), before(20));
  // A first page without older history stays without it.
  const short = new TimelineStore({ run: 'run', agent });
  short.apply(short.ticket('latest'), page({ events: [prose(1, 1)], navigation: { received: recv('run', 2) } }));
  short.apply(short.ticket('received'), page({ events: [prose(2, 2)], navigation: { earlier: before(1), received: recv('run', 3) } }));
  assert.equal(short.earlierRequest(), null);
  // A retarget page and a reset store adopt their own earlier.
  const replacementEarlier = { op: 'history', run: 'replacement', agent, worker: null, before: { created_at_ms: 5, revision: 5 }, after_revision: null };
  store.apply(store.ticket('received'), page({
    run: 'replacement', retargeted: true, events: [prose(5, 5)],
    navigation: { earlier: replacementEarlier, received: recv('replacement', 6) },
  }));
  assert.deepEqual(store.earlierRequest(), replacementEarlier);
  short.reset();
  short.apply(short.ticket('latest'), page({ events: [prose(9, 9)], navigation: { earlier: before(9), received: recv('run', 10) } }));
  assert.deepEqual(short.earlierRequest(), before(9));
});

function seedSnapshot(cache, run, { agent = null, tz = 'UTC', event = prose(100) } = {}) {
  const view = { run, agent };
  const store = cache.open(view, tz);
  cache.apply(store, tz, store.ticket('latest'), page({
    run, events: [event], navigation: { received: { op: 'history', ...view, after_revision: 101 } },
  }));
  return store;
}

test('rollback on the first restored delivery clears the run and fences pending reads', () => {
  const cache = new TimelineCache();
  const view = { run: 'run' };
  seedSnapshot(cache, 'run');
  seedSnapshot(cache, 'run', { agent: 'live' });
  const store = cache.open(view, 'UTC');
  const pending = store.ticket('earlier');
  const delivery = store.ticket('received');
  // A chronological tail can arrive first. Only delivery validates the saved cursor.
  cache.apply(store, 'UTC', store.ticket('latest'), page({ events: [prose(5)] }));
  const received = { op: 'history', ...view, after_revision: 6 };
  const earlier = { op: 'history', ...view, before: c(4) };
  const replacement = page({ events: [prose(4), prose(5)], navigation: { received, earlier } });
  assert.equal(cache.apply(store, 'UTC', delivery, replacement).rebuild, true);
  assert.equal(store.confirmed, false);
  assert.deepEqual(store.events(), []);
  assert.equal(cache.open({ ...view, agent: 'live' }, 'UTC').loaded, false);
  assert.equal(cache.apply(store, 'UTC', pending, page({ events: [prose(99)] })).reason, 'fenced');
  const initial = { op: 'history', ...view };
  assert.deepEqual(store.refreshRequest(), initial);
  // A failed rebuild leaves the next poll free to retry the initial read.
  assert.equal(cache.apply(store, 'UTC', store.ticket('latest'), null).applied, false);
  assert.deepEqual(store.refreshRequest(), initial);
  cache.apply(store, 'UTC', store.ticket('latest'), replacement);
  assert.deepEqual(store.events(), replacement.events);
  assert.deepEqual(store.earlierRequest(), earlier);
  assert.deepEqual(store.refreshRequest(), received);
});

test('an explicit unavailable reply evicts every binding and timezone for only the missing run', () => {
  const cache = new TimelineCache();
  seedSnapshot(cache, 'other');
  seedSnapshot(cache, 'run', { agent: 'live' });
  seedSnapshot(cache, 'run', { tz: 'America/Chicago' });
  const store = seedSnapshot(cache, 'run');
  cache.apply(store, 'UTC', store.ticket('received'), page({ status: 'No recorded history' }));
  assert.equal(cache.open({ run: 'run', agent: 'live' }, 'UTC').loaded, false);
  assert.equal(cache.open({ run: 'run' }, 'America/Chicago').loaded, false);
  assert.equal(cache.open({ run: 'other' }, 'UTC').loaded, true);
});

test('reopening after completion repair produces the same events as a fresh visit', () => {
  const cache = new TimelineCache();
  const result = prose(100, null, 'result');
  seedSnapshot(cache, 'run', { event: result });
  const reopened = cache.open({ run: 'run' }, 'UTC');
  const message = { ...prose(101), markdown: result.markdown };
  const freshPage = page({
    events: [message], suppressed_result_revisions: [100],
    navigation: { received: { op: 'history', run: 'run', after_revision: 102 } },
  });
  // Delivery and the chronological tail can disagree about the same result.
  cache.apply(reopened, 'UTC', reopened.ticket('received'), { ...freshPage, events: [result, message] });
  const fresh = new TimelineStore({ run: 'run' });
  fresh.apply(fresh.ticket('latest'), freshPage);
  assert.deepEqual(reopened.events(), fresh.events());
  assert.deepEqual(cache.open({ run: 'run' }, 'UTC').events(), [message]);
});

test('aggregate budget evicts whole least-recently-used snapshots and skips oversized views', () => {
  // Two 1,000-character HTML bodies fit together, three exceed this budget.
  const cache = new TimelineCache(6000);
  const seed = (run) => seedSnapshot(cache, run, { event: { ...prose(100), html: 'x'.repeat(1000) } });
  seed('aaa');
  seed('bbb');
  seed('bbb'); // replacing a snapshot charges only its current contents
  assert.equal(cache.open({ run: 'aaa' }, 'UTC').loaded, true);
  seed('ccc');
  const evicted = cache.open({ run: 'bbb' }, 'UTC');
  assert.deepEqual(evicted.events(), []);
  assert.deepEqual(evicted.refreshRequest(), { op: 'history', run: 'bbb' });
  assert.equal(cache.open({ run: 'aaa' }, 'UTC').loaded, true);
  const active = seedSnapshot(cache, 'ccc', { event: { ...prose(100), html: 'x'.repeat(4000) } });
  assert.equal(active.events().length, 1);
  assert.equal(active.refreshRequest().after_revision, 101);
  assert.equal(cache.open({ run: 'ccc' }, 'UTC').loaded, false);
  assert.equal(cache.open({ run: 'aaa' }, 'UTC').loaded, true);
});
