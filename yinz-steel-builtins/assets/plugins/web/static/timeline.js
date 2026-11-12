// Timeline cache and navigation over the native history representation.
//
// Rust counterparts (keep in parity):
// - yinz_types::agent::history::{Cursor, Range} and Range::overlaps
//   (yinz-types/src/agent/history.rs): null timestamps sort before dated entries,
//   revision breaks ties; activity ranges exclude their prose boundaries and
//   different worker scopes never overlap.
// - agent_proto::conversation::activity_ranges / activity
//   (default-plugins/agent-proto/src/conversation.rs): native pages return grouped
//   ranged activity events (`revisions: []`, `range` set, `tool_calls` counted)
//   plus replacement intervals in `activity_ranges`.
// - HistoryCache::merge (yinz-screen/src/screen/palette_history.rs) and the web
//   palette adapters `adapt`/`navigation` (yinz-plugin-host/src/web_agents.rs):
//   remove cached activity overlapping any replacement interval, then merge by
//   revision and order by timestamp/revision. Navigation requests are followed as
//   returned, reapplying tz; the client never synthesizes newest+1.
//
// One intentional difference from HistoryCache: the `earlier` cursor is taken only
// from older pages and from the first page of a run, never from a later refresh or
// delivery. Native delivery replies repeat the newest page's `earlier` row
// (palette_history::conversation_refresh), which describes a boundary the cache
// already reaches; adopting it after history is exhausted would offer
// "Load earlier" again and walk back over pages already loaded.

export const TOOL_KINDS = new Set(['tool_call', 'tool_result']);
export const TOOLS_PAGE_REVISIONS = 30;

function at(c) {
  return c && c.created_at_ms !== undefined ? c.created_at_ms : null;
}

/** Cursor order: null timestamps first, then timestamp, then revision. */
export function compareCursor(a, b) {
  const x = at(a);
  const y = at(b);
  if (x === null && y !== null) return -1;
  if (x !== null && y === null) return 1;
  if (x !== null && y !== null && x !== y) return x < y ? -1 : 1;
  return a.revision === b.revision ? 0 : a.revision < b.revision ? -1 : 1;
}

export function eventCursor(e) {
  return { created_at_ms: e.created_at_ms ?? null, revision: e.revision };
}

function worker(w) {
  return w === undefined ? null : w;
}

/** Port of Range::overlaps: exclusive boundaries, workers independent. */
export function rangeOverlaps(a, b) {
  if (worker(a.worker) !== worker(b.worker)) return false;
  if (a.end && b.start && compareCursor(a.end, b.start) <= 0) return false;
  if (b.end && a.start && compareCursor(b.end, a.start) <= 0) return false;
  return true;
}

/** A position strictly between the range's prose boundaries, in the same worker scope. */
export function rangeContains(range, cursor, scopeWorker = null) {
  if (worker(range.worker) !== worker(scopeWorker)) return false;
  if (range.start && compareCursor(cursor, range.start) <= 0) return false;
  if (range.end && compareCursor(cursor, range.end) >= 0) return false;
  return true;
}

export function isGrouped(e) {
  return !!e && e.range !== null && e.range !== undefined && typeof e.range === 'object';
}

export function isRawTool(e) {
  return !isGrouped(e) && TOOL_KINDS.has(e.kind);
}

export function eventKey(e) {
  return isGrouped(e) ? `t${e.revision}` : `e${e.revision}`;
}

export function sortEvents(events) {
  return [...events].sort((a, b) => compareCursor(eventCursor(a), eventCursor(b)) || (isGrouped(a) === isGrouped(b) ? 0 : isGrouped(a) ? -1 : 1));
}

function validEvent(e) {
  return !!e && typeof e === 'object' && Number.isSafeInteger(e.revision) && e.revision > 0
    && (e.created_at_ms === null || e.created_at_ms === undefined || Number.isFinite(e.created_at_ms));
}

/**
 * Port of HistoryCache::merge. Before merging a page, drop cached grouped activity
 * whose range overlaps any replacement interval (even intervals with no new event),
 * plus raw tool events positioned inside one (revisions[] compatibility). Ordinary
 * events then dedupe by revision. Grouped ranged activity keeps its range, worker,
 * tool_calls and request; it is never re-collapsed or dropped for empty revisions[].
 */
export function mergeEvents(cache, events, activityRanges, scopeWorker = null) {
  const ranges = Array.isArray(activityRanges) ? activityRanges.filter((r) => r && typeof r === 'object') : [];
  const next = new Map();
  for (const [key, old] of cache) {
    if (isGrouped(old) && ranges.some((r) => rangeOverlaps(old.range, r))) continue;
    if (isRawTool(old) && ranges.some((r) => rangeContains(r, eventCursor(old), old.worker ?? scopeWorker))) continue;
    next.set(key, old);
  }
  for (const e of Array.isArray(events) ? events : []) {
    if (validEvent(e)) next.set(eventKey(e), e);
  }
  return next;
}

/**
 * revisions[] compatibility (formerly yinz_bus::command_palette::collapse_tools):
 * consecutive raw tool_call/tool_result events — across page boundaries, since this
 * runs over the merged cache — become one `{kind:"tools"}` item with summed
 * tool_calls, split at 30 revisions. Prose or ranged activity breaks a group, and
 * different workers never merge. Native ranged groups pass through unchanged.
 */
export function displayItems(sorted, { run } = {}) {
  const items = [];
  let group = null;
  const flush = () => {
    if (group) items.push(group);
    group = null;
  };
  for (const e of sorted) {
    if (isRawTool(e)) {
      const w = worker(e.worker);
      if (!group || group.worker !== w || group.revisions.length >= TOOLS_PAGE_REVISIONS) {
        flush();
        group = {
          key: `c${e.revision}`,
          kind: 'tools',
          compat: true,
          worker: w,
          revision: e.revision,
          revisions: [],
          tool_calls: 0,
          created_at_ms: e.created_at_ms ?? null,
          time: e.time,
          events: [],
          request: null,
        };
      }
      group.revisions.push(e.revision);
      group.events.push(e);
      group.tool_calls += Number.isInteger(e.tool_calls) && e.tool_calls > 0 ? e.tool_calls : e.kind === 'tool_call' ? 1 : 0;
      group.request = { op: 'tools', run, revisions: [...group.revisions] };
      continue;
    }
    flush();
    items.push({ ...e, key: eventKey(e), compat: false });
  }
  flush();
  return items;
}

/**
 * Per-view navigation state. `agent` is the optional live binding: current-run
 * reads carry it (the native layer keeps it on earlier/continue/latest/received),
 * stored-run browsing (previous/origin, run-only) never does. Every read takes a
 * ticket; tickets from before a retarget or reset are fenced.
 */
export class TimelineStore {
  constructor({ run, agent = null, worker: scope = null, query = '' }) {
    this.run = run;
    this.agent = agent || null;
    this.worker = scope;
    this.query = query;
    this.gen = 0;
    this.cache = new Map();
    this.earlier = null;
    this.received = null;
    this.navigation = {};
    this.loaded = false;
    this.meta = {
      live: null,
      header: null,
      header_html: '',
      details_html: '',
      agent_gone: false,
      previous_run: null,
      origin_run: null,
      status: '',
      retargetedAt: null,
    };
  }

  initialRequest() {
    const r = { op: 'history', run: this.run };
    if (this.agent) r.agent = this.agent;
    if (this.worker) r.worker = this.worker;
    if (this.query) r.query = this.query;
    return r;
  }

  /** The returned delivery request (inclusive after_revision), or the initial read. */
  refreshRequest() {
    return this.received || this.initialRequest();
  }

  earlierRequest() {
    return this.earlier;
  }

  latestRequest() {
    return this.navigation.latest || this.initialRequest();
  }

  /** Lineage links: stored-run browsing, intentionally without an agent binding. */
  lineageRequest(which) {
    const r = this.navigation[which];
    return r && r.op === 'history' ? r : null;
  }

  ticket(kind) {
    return { gen: this.gen, kind, run: this.run };
  }

  /** Abandon all in-flight reads and cached content (navigation or auth change). */
  reset({ run = this.run, agent = this.agent } = {}) {
    this.gen += 1;
    this.run = run;
    this.agent = agent || null;
    this.cache = new Map();
    this.earlier = null;
    this.received = null;
    this.navigation = {};
    this.loaded = false;
  }

  apply(ticket, page) {
    if (!ticket || ticket.gen !== this.gen) return { applied: false, reason: 'fenced' };
    if (!page || typeof page !== 'object' || typeof page.run !== 'string' || !Array.isArray(page.events)) {
      return { applied: false, reason: 'malformed' };
    }
    const older = ticket.kind === 'earlier';
    let retargeted = false;
    let switched = false;
    if (page.retargeted === true || page.run !== this.run) {
      // An older page can never belong to a replacement run.
      if (older) return { applied: false, reason: 'fenced' };
      const previousRun = this.run;
      this.gen += 1;
      this.run = page.run;
      this.cache = new Map();
      this.earlier = null;
      this.received = null;
      this.navigation = {};
      retargeted = previousRun !== null;
      switched = true;
      this.meta.retargetedAt = previousRun;
    }
    const navigation = page.navigation && typeof page.navigation === 'object' ? page.navigation : {};
    // See the header: later non-older pages carry the newest page's stale boundary.
    if (older || switched || !this.loaded) this.earlier = navigation.earlier || null;
    if (!older) {
      const recv = navigation.received;
      if (recv && (!this.received || retargeted || !(recv.after_revision < this.received.after_revision))) {
        this.received = recv;
      }
      this.navigation = navigation;
      this.meta = {
        ...this.meta,
        live: page.live || null,
        header: page.header || null,
        header_html: typeof page.header_html === 'string' ? page.header_html : '',
        details_html: typeof page.details_html === 'string' ? page.details_html : '',
        agent_gone: page.agent_gone === true,
        previous_run: page.previous_run ?? null,
        origin_run: page.origin_run ?? null,
        status: typeof page.status === 'string' ? page.status : '',
      };
    }
    this.cache = mergeEvents(this.cache, page.events, page.activity_ranges, this.worker);
    this.loaded = true;
    return { applied: true, retargeted };
  }

  events() {
    return sortEvents(this.cache.values());
  }

  items() {
    return displayItems(this.events(), { run: this.run });
  }

  /** Actions are disabled while the bound agent is gone; history stays readable. */
  get actionsEnabled() {
    return !!this.agent && !this.meta.agent_gone && !!this.meta.live;
  }
}

/**
 * Range tools reader: the separate `page` extension advances bounded 30-record
 * pages through `page.navigation.earlier` (older records).
 */
export class ToolsReader {
  constructor(request) {
    this.first = request;
    this.pages = []; // oldest first
    this.gen = 0;
    this.next = null;
    this.done = false;
  }

  firstRequest() {
    return this.first;
  }

  nextRequest() {
    return this.done ? null : this.next;
  }

  ticket() {
    return { gen: this.gen };
  }

  apply(ticket, reader) {
    if (!ticket || ticket.gen !== this.gen || !reader || typeof reader !== 'object') return false;
    this.pages.unshift(reader);
    const page = reader.page && typeof reader.page === 'object' ? reader.page : null;
    const earlier = page && page.navigation && page.navigation.earlier;
    this.next = earlier && earlier.op === 'tools' ? earlier : null;
    this.done = !this.next;
    return true;
  }

  records() {
    return this.pages.flatMap((r) => (r.page && Array.isArray(r.page.events) ? r.page.events : []));
  }
}
