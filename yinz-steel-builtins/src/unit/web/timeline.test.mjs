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
  TimelineStore, ToolsReader,
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
