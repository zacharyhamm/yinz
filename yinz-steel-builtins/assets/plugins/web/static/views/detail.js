// Agent detail (#/a/<id>?run=…&tab=…): Timeline | Screen | Details, composer and sheets.
import {
  describeError, freshnessText, SCREEN_TEXT_SIZES, normalizeScreenView, readScreenView, writeScreenView,
} from '../api.js';
import { TimelineStore, ToolsReader } from '../timeline.js';
import { sameTarget, targetOf, validTarget } from '../submission.js';
import { controlTarget, detailHash, detailListRow, isPeerDetail, isScopedPeer } from '../app-route.js';
import {
  html, useState, useEffect, useLayoutEffect, useMemo, useRef, useCallback, usePoller, useNow,
  ServerHtml, StateBadge, RowBadges, QuickKeys, QuotaBar, Banner, responseText, keyName, cx,
} from './ui.js';
import { Sheet, ConfirmSheet } from './sheets.js';
import { Composer, StaleSubmissions } from './composer.js';
import { KeysSheet } from './keys.js';

const KIND_LABELS = { prompt: 'You', message: 'Agent', result: 'Result', plan: 'Plan', question: 'Question', step: 'Step', summary: 'Summary', tool_call: 'Tool call', tool_result: 'Tool result', tools: 'Tools' };
const PEER_HANDOFF_MS = 10000;

function kindLabel(kind) {
  return KIND_LABELS[kind] || (typeof kind === 'string' ? kind : 'Event');
}

// A resize can clamp scrollTop and dispatch scroll before ResizeObserver runs.
// Keep the last reading position until correction, rather than treating that
// browser-generated scroll as a user leaving the bottom. The explicit disclosure
// captures before reflow; other geometry changes use the same scroll memory.
function useScrollLayout(ref, capture, restore, content, layoutKey, beforeResize, present = true) {
  const size = useRef(null);
  const measure = useCallback(() => {
    const el = ref.current;
    if (!el || !size.current || el.clientWidth !== size.current.width || el.clientHeight !== size.current.height) return;
    capture(el);
  }, [capture]);
  const correct = useCallback(() => {
    const el = ref.current;
    if (!el || !el.clientWidth || !el.clientHeight) return;
    restore(el);
    size.current = { width: el.clientWidth, height: el.clientHeight };
    capture(el);
  }, [capture, restore]);
  useLayoutEffect(correct, [content, layoutKey]);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    beforeResize.current = measure;
    const observer = new ResizeObserver(correct);
    observer.observe(el);
    return () => {
      observer.disconnect();
      if (beforeResize.current === measure) beforeResize.current = null;
    };
  }, [correct, measure, present]);
  return measure;
}

function TimelineItem({ item, onTools, onEvent }) {
  if (item.kind === 'tools') {
    const n = Number.isInteger(item.tool_calls) ? item.tool_calls : 0;
    return html`<div class="timeline-item tools-item" data-key=${item.key}>
      <button type="button" class="tools-group" onClick=${() => onTools(item)} disabled=${!item.request}
        aria-label=${`Show ${n} tool calls${item.time ? ` from ${item.time}` : ''}`}>
        <span class="tools-glyph" aria-hidden="true">⚙</span> tools ×${n}
        ${item.time ? html`<span class="event-time">${item.time}</span>` : null}
      </button>
    </div>`;
  }
  return html`<article class=${cx('timeline-item', 'event', `kind-${/^[a-z_]{1,32}$/.test(item.kind || '') ? item.kind : 'other'}`)} data-key=${item.key}>
    <div class="event-meta"><span class="event-kind">${kindLabel(item.kind)}</span>${item.time ? html` <span class="event-time">${item.time}</span>` : null}</div>
    <${ServerHtml} html=${item.html} class="event-body" />
    ${item.truncated ? html`<button type="button" class="link-button event-read" onClick=${() => onEvent(item)}>Read the full message</button>` : null}
  </article>`;
}

/**
 * Newest at the bottom. Stays pinned only when already at the bottom; otherwise
 * the first visible item is kept in place (prepends, group splits, reader close)
 * and a "New messages" control appears.
 */
function Timeline({ items, memory, earlier, loadingEarlier, onEarlier, onTools, onEvent, loaded, waitingForAgent, layoutKey, beforeResize }) {
  const ref = useRef(null);
  const [fresh, setFresh] = useState(false);
  const lastKey = useRef(memory.lastKey);
  const capture = useCallback((el) => {
    memory.atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 32;
    if (memory.atBottom) setFresh(false);
    const top = el.scrollTop;
    memory.anchor = null;
    for (const node of el.querySelectorAll('[data-key]')) {
      if (node.offsetTop + node.offsetHeight > top) {
        memory.anchor = { key: node.getAttribute('data-key'), offset: node.offsetTop - top };
        break;
      }
    }
  }, [memory]);
  const restore = useCallback((el) => {
    // Write only when the position must change: assigning scrollTop during a touch
    // drag or fling can stop momentum scrolling.
    if (memory.atBottom) {
      if (el.scrollHeight - el.scrollTop - el.clientHeight > 1) el.scrollTop = el.scrollHeight;
    } else if (memory.anchor) {
      const node = [...el.querySelectorAll('[data-key]')].find((n) => n.getAttribute('data-key') === memory.anchor.key);
      const target = node ? Math.max(0, Math.min(el.scrollHeight - el.clientHeight, node.offsetTop - memory.anchor.offset)) : null;
      if (target !== null && Math.abs(el.scrollTop - target) > 1) el.scrollTop = target;
    }
  }, [memory]);
  useLayoutEffect(() => {
    const last = items.length ? items[items.length - 1].key : null;
    if (!memory.atBottom && lastKey.current !== null && last !== lastKey.current) setFresh(true);
    lastKey.current = last;
    memory.lastKey = last;
  }, [items]);
  const measure = useScrollLayout(ref, capture, restore, items, `${layoutKey}|${waitingForAgent}`, beforeResize);
  const toBottom = () => {
    const el = ref.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    memory.atBottom = true;
    setFresh(false);
  };
  return html`<div class="timeline-wrap">
    <div class="timeline-scroll" ref=${ref} onScroll=${measure} id="timeline" aria-live="off">
      ${earlier ? html`<div class="timeline-earlier"><button type="button" class="button" id="timeline-earlier" disabled=${loadingEarlier}
        onClick=${onEarlier}>${loadingEarlier ? 'Loading…' : 'Load earlier'}</button></div>` : null}
      ${loaded && !items.length ? html`<p class="empty">No messages in this run yet.</p>` : null}
      ${!loaded ? html`<p class="empty">Loading timeline…</p>` : null}
      ${items.map((item) => html`<${TimelineItem} key=${item.key} item=${item} onTools=${onTools} onEvent=${onEvent} />`)}
      <div class="timeline-response" role="status" aria-live="polite">${waitingForAgent ? html`
        <span class="timeline-spinner" aria-hidden="true"></span>Waiting for agent…
      ` : null}</div>
    </div>
    ${fresh ? html`<button type="button" class="new-messages" id="timeline-new" onClick=${toBottom}>↓ New messages</button>` : null}
  </div>`;
}

function ReaderBody({ reader, error, loading }) {
  if (error) return html`<p class="error-text" role="alert">${error}</p>`;
  if (loading && !reader) return html`<p class="empty">Loading…</p>`;
  if (!reader) return null;
  return html`<div class="reader">
    ${reader.header ? html`<p class="reader-header">${String(reader.header)}</p>` : null}
    <${ServerHtml} html=${reader.html} class="reader-body" />
  </div>`;
}

function EventReaderSheet({ env, item, onClose }) {
  const [state, setState] = useState({ loading: true, reader: null, error: null });
  useEffect(() => {
    const controller = new AbortController();
    env.api.read(item.request, env.tz, { signal: controller.signal }).then((r) => {
      if (controller.signal.aborted) return;
      setState(r.ok ? { loading: false, reader: r.data, error: null } : { loading: false, reader: null, error: describeError(r) });
    });
    return () => controller.abort();
  }, []);
  return html`<${Sheet} id="sheet-reader" title=${`${kindLabel(item.kind)}${item.time ? ` · ${item.time}` : ''}`} onClose=${onClose} wide=${true}>
    <${ReaderBody} ...${state} />
  <//>`;
}

function ToolsReaderSheet({ env, item, onClose }) {
  const reader = useMemo(() => new ToolsReader(item.request), []);
  const [, setVersion] = useState(0);
  const [state, setState] = useState({ loading: false, error: null });
  const controller = useRef(null);
  const load = useCallback(async (request) => {
    if (!request || controller.current) return;
    controller.current = new AbortController();
    setState({ loading: true, error: null });
    const ticket = reader.ticket();
    const r = await env.api.read(request, env.tz, { signal: controller.current.signal });
    const aborted = controller.current.signal.aborted;
    controller.current = null;
    if (aborted) return;
    if (r.ok) reader.apply(ticket, r.data);
    setState({ loading: false, error: r.ok ? null : describeError(r) });
    setVersion((v) => v + 1);
  }, [reader]);
  useEffect(() => {
    load(reader.firstRequest());
    return () => {
      reader.gen += 1;
      if (controller.current) controller.current.abort();
    };
  }, []);
  const next = reader.nextRequest();
  return html`<${Sheet} id="sheet-reader" title=${`Tools ×${item.tool_calls || 0}`} onClose=${onClose} wide=${true}>
    ${next ? html`<button type="button" class="button" id="reader-more" disabled=${state.loading} onClick=${() => load(next)}>
      ${state.loading ? 'Loading…' : 'Load older records'}</button>` : null}
    ${reader.pages.map((page, i) => html`<${ReaderBody} key=${i} reader=${page} />`)}
    ${state.error ? html`<p class="error-text" role="alert">${state.error}</p>` : null}
    ${state.loading && !reader.pages.length ? html`<p class="empty">Loading…</p>` : null}
    ${reader.pages.length ? html`<p class="hint">Bounded previews of retained tool records, 30 per page.</p>` : null}
  <//>`;
}

function ScreenPanel({ env, target: ref, enabled, onTargetGone, layoutKey, beforeResize, view, onView }) {
  const now = useNow(1000);
  const [busy, setBusy] = useState(false);
  const scroller = useRef(null);
  const position = useRef({ pinned: true, top: 0 });
  const [state, refresh] = usePoller({
    key: ref ? `screen:${ref.id}|${ref.kind}|${ref.session_id}` : 'screen:none',
    enabled: !!ref && enabled,
    intervalMs: env.config.poll.screen_ms,
    env,
    run: async ({ signal }) => {
      const r = await env.api.post('/api/agent/screen', { agent: ref, lines: 80 }, { signal });
      if (r.status === 409 || r.data?.response?.outcome === 'target_changed') onTargetGone();
      return r;
    },
  });
  async function key(k) {
    if (!ref || !enabled) return;
    setBusy(true);
    const r = await env.api.post('/api/agent/keys', { agent: ref, keys: [k] });
    setBusy(false);
    if (r.status === 409 || r.data?.response?.outcome === 'target_changed') {
      env.toast('The agent changed or disappeared. The key was not sent.');
      onTargetGone();
    } else if (r.ok) env.toast(`${keyName(k)}: ${responseText(r.data && r.data.response)}`);
    else if (r.status !== 401) env.toast(`Key not confirmed: ${describeError(r)} Check the screen before trying again.`);
    refresh();
  }
  const data = state && state.data;
  const markup = data ? data.html : null;
  // The question a waiting agent asks is at the bottom of its screen, so the output
  // starts pinned to its newest line and stays there unless the user scrolls up.
  const capture = useCallback((el) => {
    position.current.pinned = el.scrollHeight - el.scrollTop - el.clientHeight < 32;
    position.current.top = el.scrollTop;
  }, []);
  const restore = useCallback((el) => {
    const target = position.current.pinned ? el.scrollHeight - el.clientHeight
      : Math.max(0, Math.min(el.scrollHeight - el.clientHeight, position.current.top));
    if (Math.abs(el.scrollTop - target) > 1) el.scrollTop = target;
  }, []);
  const onScroll = useScrollLayout(scroller, capture, restore, markup, `${layoutKey}|${view.wrap}|${view.size}`, beforeResize, !!ref);
  if (!ref) return html`<p class="empty">No live agent to read.</p>`;
  // Capture the reading position before the reflow; the composite layout key
  // restores it (pinned, or a clamped offset) once the new classes apply.
  const changeView = (next) => {
    onScroll();
    onView(next);
  };
  const sizeIndex = SCREEN_TEXT_SIZES.indexOf(view.size);
  const smallest = sizeIndex <= 0;
  const largest = sizeIndex >= SCREEN_TEXT_SIZES.length - 1;
  // aria-disabled, never native disabled: a focused stepper keeps keyboard focus at its bound.
  const step = (delta) => {
    const next = SCREEN_TEXT_SIZES[sizeIndex + delta];
    if (next === undefined) return;
    changeView({ ...view, size: next });
  };
  const response = data && data.response;
  const failed = response && response.outcome && response.outcome !== 'ok' && response.outcome !== 'completed';
  return html`<div class="screen-panel">
    <div class="screen-feedback" role="region" aria-label="Screen status and feedback" tabindex="0">
      <div class="screen-status">
        ${state ? html`<span class=${cx('freshness', state.status !== 'ok' && 'freshness-stale')}>${freshnessText(state, now)}</span>` : null}
        <button type="button" class="link-button" id="screen-refresh" onClick=${() => refresh()}>Refresh</button>
        <span class="screen-view" role="group" aria-label="Screen view">
          <button type="button" class="screen-view-button" id="screen-wrap" aria-pressed=${String(view.wrap)}
            onClick=${() => changeView({ ...view, wrap: !view.wrap })}>Wrap</button>
          <button type="button" class="screen-view-button" id="screen-text-smaller" aria-label="Smaller screen text"
            aria-disabled=${smallest ? 'true' : undefined} onClick=${() => step(-1)}>A−</button>
          <button type="button" class="screen-view-button" id="screen-text-larger" aria-label="Larger screen text"
            aria-disabled=${largest ? 'true' : undefined} onClick=${() => step(1)}>A+</button>
          <span class="visually-hidden" id="screen-text-size" role="status" aria-live="polite">${`Text ${view.size} px`}</span>
        </span>
      </div>
      ${failed ? html`<${Banner} id="banner-screen" tone="warn">${responseText(response)}<//>` : null}
      ${!data && state && state.error ? html`<${Banner} id="banner-screen" tone="warn">${describeError(state.error)}<//>` : null}
    </div>
    <div class="screen-scroll" id="screen-scroll" ref=${scroller} onScroll=${onScroll}>
      ${data ? html`<${ServerHtml} html=${markup} class="screen-html" id="screen" />` : html`<p class="empty">Reading the screen…</p>`}
    </div>
    <div class="screen-keys" role="region" aria-label="Screen quick keys" tabindex="0">
      <${QuickKeys} keys=${env.config.quick_keys} disabled=${busy || !enabled} onKey=${key} />
    </div>
  </div>`;
}

function OverflowSheet({ env, live, target, onClose, onInterrupt, onKeys, onTargetGone }) {
  const peer = live.current_session === false;
  async function focus() {
    onClose();
    const r = await env.api.post('/api/agent/focus', { agent: target });
    if (r.code === 'target_changed') {
      env.toast('The agent changed or disappeared.');
      onTargetGone();
    } else if (r.ok && r.data?.ok === true) {
      env.toast(peer ? `Desktop switched to session ${live.session}.` : 'Focused on the desktop.');
    } else if (r.status !== 401) {
      env.toast(describeError(r.ok ? { kind: 'malformed' } : r));
    }
  }
  return html`<${Sheet} id="sheet-overflow" title="Actions" onClose=${onClose}>
    <div class="menu-list">
      <button type="button" class="button menu-item" id="action-focus" disabled=${live.selectable === false} onClick=${focus}>
        <span>Focus on desktop</span>
        ${peer ? html`<span class="menu-sub">switches session to ${String(live.session || '')}</span>` : null}
      </button>
      <button type="button" class="button menu-item" id="action-interrupt" onClick=${() => { onClose(); onInterrupt(); }}>
        <span>Interrupt</span><span class="menu-sub">sends Esc after you confirm</span>
      </button>
      <button type="button" class="button menu-item" id="action-keys" onClick=${() => { onClose(); onKeys(); }}>Send keys…</button>
    </div>
  <//>`;
}

function chooseTab(route, live) {
  if (route.tab) return route.tab;
  return live && live.state === 'waiting' ? 'screen' : 'timeline';
}

export function DetailView({ env, route }) {
  const scoped = isPeerDetail(route.id);
  const recoverScoped = route.bind && isScopedPeer(route.id);
  const identity = route.bind ? `a:${route.id}:${route.run ? 'run' : 'none'}` : `r:${route.run}`;
  const store = useMemo(() => (route.run ? new TimelineStore({ run: route.run, agent: route.bind ? route.id : null }) : null), [identity]);
  const [version, setVersion] = useState(0);
  const bump = () => setVersion((v) => v + 1);
  const [tab, setTabState] = useState(route.tab);
  const [sheet, setSheet] = useState(null);
  const [peerRejected, setPeerRejected] = useState(false);
  const [informationOpen, setInformationOpen] = useState(false);
  const beforeResize = useRef(null);
  // Per device: kept across polls, tab switches and reloads. When storage fails,
  // the in-memory choice still applies to this view.
  const [screenView, setScreenView] = useState(() => readScreenView(env.local));
  const changeScreenView = useCallback((next) => {
    const view = normalizeScreenView(next);
    setScreenView(view);
    writeScreenView(env.local, view);
  }, []);
  const [retargetNotice, setRetargetNotice] = useState(false);
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  const earlierController = useRef(null);
  const memory = useRef({ atBottom: true, anchor: null, lastKey: null }).current;
  const lastRef = useRef(null);
  const liveRow = useRef(null);
  const [sending, setSending] = useState(null);
  const [awaitingStatus, setAwaitingStatus] = useState(null);
  const now = useNow(1000);

  const clearAwaitingStatus = useCallback((confirmation) => {
    setAwaitingStatus((current) => current === confirmation ? null : current);
  }, []);

  function observePeerActivity(row) {
    const snapshot = awaitingStatus?.peerActivity;
    if (!snapshot || !sameTarget(controlTarget(row), awaitingStatus.scope.target)) return;
    const hookAt = row.last_hook_at_ms;
    const stateChanged = typeof row.state === 'string' && row.state !== snapshot.state;
    const newerHook = Number.isFinite(hookAt) && hookAt > (snapshot.lastHookAtMs ?? -Infinity);
    if (stateChanged || newerHook) {
      clearAwaitingStatus(awaitingStatus);
    }
  }

  function finishStatusPoll(signal, result, row) {
    // Each request captures the confirmation it can acknowledge when it starts.
    if (!awaitingStatus || signal.aborted) return;
    if (!awaitingStatus.peerActivity || !result.ok) clearAwaitingStatus(awaitingStatus);
    else observePeerActivity(row);
  }

  const [tail, refreshTail] = usePoller({
    key: identity,
    enabled: !!store,
    intervalMs: env.config.poll.tail_ms,
    env,
    run: async ({ signal }) => {
      const ticket = store.ticket(store.loaded ? 'received' : 'latest');
      let r = await env.api.read(store.refreshRequest(), env.tz, { signal });
      let row;
      if (!signal.aborted && r.ok && !env.api.stale(r)) {
        const previous = store.meta.live && store.meta.live.ref;
        const applied = store.apply(ticket, r.data);
        if (applied.applied) {
          row = store.meta.live;
          if (applied.retargeted) onRetarget(previous);
          bump();
        } else if (applied.reason === 'malformed') {
          r = { ok: false, kind: 'malformed' };
        }
      }
      finishStatusPoll(signal, r, row);
      return r;
    },
  });

  // Agent without a recorded run yet: find its live row in the list.
  const [list, refreshList] = usePoller({
    key: `list-for:${route.id}`,
    enabled: !route.run && route.bind,
    intervalMs: env.config.poll.list_ms,
    env,
    run: async ({ signal }) => {
      const r = await env.api.get('/api/agents', { signal });
      const row = r.ok && !env.api.stale(r) && Array.isArray(r.data?.agents)
        ? detailListRow(r.data.agents, route.id, r.data.session) : null;
      finishStatusPoll(signal, r, row);
      return r;
    },
  });
  const listRows = !route.run && list?.data?.agents;
  const listRow = Array.isArray(listRows) ? detailListRow(listRows, route.id, list.data.session) : null;
  useEffect(() => {
    if (listRow && listRow.run) env.replaceHash(detailHash({ id: route.id, run: listRow.run, tab: tab || undefined }));
  }, [listRow && listRow.run]);

  const meta = store ? store.meta : { live: listRow, agent_gone: Array.isArray(listRows) && !listRow };
  const live = route.bind ? meta.live : null;
  liveRow.current = live;
  const candidate = controlTarget(live);
  const currentRef = candidate?.id === route.id ? candidate : null;
  const actions = route.bind && !!currentRef && !meta.agent_gone && !peerRejected;
  const unavailable = meta.agent_gone || peerRejected || ((store?.loaded || list?.data) && !currentRef);
  const lookup = store ? tail : list;
  const stale = lookup && ['offline', 'retrying', 'error'].includes(lookup.status);
  const scopedRecovery = recoverScoped && (unavailable || (!currentRef && stale));
  const refreshTarget = store ? refreshTail : refreshList;

  function onTargetGone() {
    if (scoped) setPeerRejected(true);
    refreshTarget();
  }

  useEffect(() => {
    if (!actions) {
      setSheet((value) => ['overflow', 'interrupt', 'keys'].includes(value?.type) ? null : value);
    }
  }, [actions]);
  const scope = useMemo(() => actions && currentRef ? { target: currentRef } : null,
    [actions, store?.run, currentRef?.id, currentRef?.kind, currentRef?.session_id]);

  useLayoutEffect(() => {
    setAwaitingStatus(null);
    setSending(scope && env.submissions.view(scope.target).state === 'sending' ? scope : null);
    if (!scope) return undefined;
    return env.submissions.subscribe((target, classification) => {
      if (!sameTarget(target, scope.target)) return;
      setSending(env.submissions.view(target).state === 'sending' ? scope : null);
      if (classification?.state === 'submitted') {
        const row = liveRow.current;
        if (!sameTarget(controlTarget(row), target)) return;
        const peerActivity = row.current_session === false ? {
          state: row.state,
          lastHookAtMs: row.last_hook_at_ms,
          expiresAt: Date.now() + PEER_HANDOFF_MS,
        } : null;
        setAwaitingStatus({ scope, peerActivity });
      }
    });
  }, [env.submissions, scope]);

  useLayoutEffect(() => {
    if (!awaitingStatus?.peerActivity || awaitingStatus.scope !== scope) return undefined;
    const timer = setTimeout(() => clearAwaitingStatus(awaitingStatus), awaitingStatus.peerActivity.expiresAt - Date.now());
    return () => clearTimeout(timer);
  }, [awaitingStatus, scope, clearAwaitingStatus]);

  useEffect(() => {
    if (awaitingStatus && awaitingStatus.scope === scope && !env.visibility.isHidden()) refreshTarget();
  }, [awaitingStatus, scope, refreshTarget, env.visibility]);

  useEffect(() => {
    if (recoverScoped && actions) env.submissions.restoreVerifiedRetry(currentRef);
  }, [recoverScoped, actions, currentRef?.id, currentRef?.kind, currentRef?.session_id]);

  function onRetarget(previousRef) {
    setRetargetNotice(true);
    memory.atBottom = true;
    memory.anchor = null;
    if (!scoped && previousRef && validTarget(previousRef)) {
      env.submissions.markIncompatible(targetOf(previousRef), { reason: 'The agent moved to a new run. This prompt was not sent to it.' });
    }
    if (earlierController.current) earlierController.current.abort();
    env.replaceHash(detailHash({ id: route.id, run: store.run, tab: tab || undefined, bind: route.bind }));
  }

  // Same-pane replacement (different kind or conversation): disable old records.
  useEffect(() => {
    if (!currentRef) return;
    const previous = lastRef.current;
    if (!scoped) {
      if (previous && !sameTarget(previous, currentRef)) {
        env.submissions.markIncompatible(previous, { replacement: currentRef, reason: 'The agent identity changed. This prompt was not sent to the replacement.' });
      }
      for (const rec of env.submissions.staleRecords(route.id, currentRef)) {
        if (!rec.incompatible) env.submissions.markIncompatible(rec.target, { replacement: currentRef, reason: 'This draft was written for an earlier conversation of this agent.' });
      }
    }
    lastRef.current = currentRef;
  }, [currentRef?.id, currentRef?.kind, currentRef?.session_id]);

  // Default tab once, when the agent state is first known (waiting -> Screen).
  const known = store ? store.loaded : !!listRow;
  useEffect(() => {
    if (!tab && known) setTabState(chooseTab(route, live));
  }, [known]);
  const activeTab = tab || 'timeline';
  function setTab(next) {
    setTabState(next);
    env.replaceHash(detailHash({ id: route.id, run: store ? store.run : route.run, tab: next, bind: route.bind }));
  }

  const [usage] = usePoller({
    key: `usage:${route.id}`,
    enabled: !!live && route.bind,
    intervalMs: env.config.poll.usage_ms,
    env,
    run: ({ signal }) => env.api.get(`/api/usage?${new URLSearchParams({ agent: route.id })}`, { signal }),
  });

  useEffect(() => () => {
    if (earlierController.current) earlierController.current.abort();
  }, [identity]);

  async function loadEarlier() {
    const request = store && store.earlierRequest();
    if (!request || earlierController.current) return;
    const controller = new AbortController();
    earlierController.current = controller;
    setLoadingEarlier(true);
    const ticket = store.ticket('earlier');
    const r = await env.api.read(request, env.tz, { signal: controller.signal });
    if (earlierController.current === controller) earlierController.current = null;
    setLoadingEarlier(false);
    if (controller.signal.aborted) return;
    if (r.ok && !env.api.stale(r)) store.apply(ticket, r.data);
    else if (!r.ok && r.status !== 401) env.toast(`Could not load earlier messages: ${describeError(r)}`);
    bump();
  }

  async function followLatest() {
    if (!store) return;
    const ticket = store.ticket('latest');
    const r = await env.api.read(store.latestRequest(), env.tz);
    if (r.ok && !env.api.stale(r)) {
      const previous = store.meta.live && store.meta.live.ref;
      const applied = store.apply(ticket, r.data);
      if (applied.applied) observePeerActivity(store.meta.live);
      if (applied.retargeted) onRetarget(previous);
      memory.atBottom = true;
      setTabState('timeline');
      bump();
    } else if (!r.ok && r.status !== 401) env.toast(describeError(r));
  }

  function openLineage(which) {
    const req = store && store.lineageRequest(which);
    if (!req) return;
    env.pushHash(detailHash({ id: route.id, run: req.run, tab: 'timeline', bind: false }));
  }

  async function interrupt() {
    if (!actions) return;
    const r = await env.api.post('/api/agent/keys', { agent: currentRef, keys: ['esc'] });
    if (r.status === 409 || r.data?.response?.outcome === 'target_changed') {
      env.toast('The agent changed or disappeared. Esc was not sent.');
      onTargetGone();
    } else if (r.ok) env.toast(`Interrupt: ${responseText(r.data && r.data.response)}`);
    else if (r.status !== 401) env.toast(`Interrupt not confirmed: ${describeError(r)} Check the screen before trying again.`);
  }

  // A new array only after a store change: the one-second age re-render must not rerun
  // the timeline's anchoring effect.
  const items = useMemo(() => (store ? store.items() : []), [store, version]);
  const waitingForAgent = actions && ((scope !== null && sending === scope)
    || (!stale && (live.state === 'busy' || awaitingStatus?.scope === scope)));
  const title = live && live.title ? String(live.title) : store && store.meta.header && store.meta.header.name ? String(store.meta.header.name)
    : scoped ? 'Peer agent' : route.id;
  useEffect(() => {
    document.title = `${title} · ${env.config.title}`;
  }, [title]);

  // Return through history when the entry behind this one is the app's; otherwise
  // replace this entry with the list rather than adding one.
  const back = () => {
    if (env.canGoBack()) window.history.back();
    else env.replaceHash('#/');
  };

  const screenTab = activeTab === 'screen';
  return html`<div class=${cx('page', 'detail-page', screenTab && 'detail-screen', screenTab && screenView.wrap && 'screen-wrap',
    screenTab && `screen-text-${screenView.size}`)}>
    <header class="app-header detail-header" id="detail-header">
      <div class="header-row detail-summary">
        <button type="button" class="icon-button" id="detail-back" aria-label="Back" onClick=${back}>‹</button>
        <div class="header-main">
          <h1 class="header-title" title=${title}>${title}</h1>
          ${live ? html`<${StateBadge} row=${live} />` : null}
        </div>
        <button type="button" class="icon-button" id="detail-information-toggle"
          aria-label=${`${informationOpen ? 'Hide' : 'Show'} agent information`} aria-expanded=${String(informationOpen)} aria-controls="detail-information"
          onClick=${() => { if (beforeResize.current) beforeResize.current(); setInformationOpen((open) => !open); }}>
          <span aria-hidden="true">ⓘ</span>
        </button>
        ${actions && currentRef ? html`<button type="button" class="icon-button" id="overflow-button" aria-label="More actions" aria-haspopup="dialog"
          onClick=${() => setSheet({ type: 'overflow' })}>⋯</button>` : null}
      </div>
      <section id="detail-information" class="detail-information" aria-label="Agent information" tabindex="0" hidden=${!informationOpen}>
        <h2 class="information-title">${title}</h2>
        ${store && store.meta.header_html ? html`<${ServerHtml} html=${store.meta.header_html} class="header-html" id="detail-header-html" />`
          : null}
        ${live ? html`<${RowBadges} row=${live} showState=${false} />` : null}
        ${live ? html`<${QuotaBar} usage=${usage && usage.data && usage.data.usage} error=${usage && !usage.data && usage.error ? describeError(usage.error) : null} />` : null}
      </section>
      <div class="segmented" role="tablist" aria-label="Agent views">
        ${['timeline', 'screen', 'details'].map((t) => html`<button type="button" role="tab" id=${`tab-${t}`} aria-controls=${`panel-${t}`}
          aria-selected=${activeTab === t ? 'true' : 'false'} class=${cx('segment', activeTab === t && 'segment-active')}
          disabled=${t === 'screen' && !actions} onClick=${() => setTab(t)}>${t[0].toUpperCase()}${t.slice(1)}</button>`)}
      </div>
    </header>
    <div class="banners">
      ${!route.bind ? html`<${Banner} id="banner-stored" tone="info">Viewing a stored run, not the live agent.<//>` : null}
      ${route.bind && meta.agent_gone ? html`<${Banner} id="banner-agent-gone" tone="warn">${store
        ? 'This agent is gone. Its history stays readable and actions are disabled. Checking for its return…'
        : 'This agent is unavailable. Return to Agents and reopen its card.'}<//>` : null}
      ${peerRejected && !meta.agent_gone ? html`<${Banner} tone="warn">
        The peer could not be verified. Refresh Agents and reopen its card before trying again.<//>` : null}
      ${retargetNotice ? html`<${Banner} id="banner-retargeted" tone="info"
        actions=${html`<button type="button" class="link-button" onClick=${() => setRetargetNotice(false)}>Dismiss</button>`}>
        The agent moved to a new run; the timeline was reset.<//>` : null}
      ${stale ? html`<${Banner} id="banner-offline" tone="warn"
        actions=${html`<button type="button" class="link-button" onClick=${refreshTarget}>Retry</button>`}>${freshnessText(lookup, now)}<//>` : null}
      ${!route.run && route.bind && !listRow ? html`<${Banner} id="banner-no-run" tone="info">${list && list.data ? 'This agent is not running.' : 'Looking for this agent…'}<//>` : null}
    </div>
    <main class=${cx('detail-body', stale && 'is-stale')} id="main">
      <div id=${`panel-${activeTab}`} role="tabpanel" aria-labelledby=${`tab-${activeTab}`} class="panel">
        ${activeTab === 'timeline' ? html`<${Timeline} items=${items} memory=${memory} loaded=${!store || store.loaded} waitingForAgent=${waitingForAgent} layoutKey=${informationOpen} beforeResize=${beforeResize}
          earlier=${store && !!store.earlierRequest()} loadingEarlier=${loadingEarlier} onEarlier=${loadEarlier}
          onTools=${(item) => setSheet({ type: 'tools', item })} onEvent=${(item) => setSheet({ type: 'event', item })} />` : null}
        ${activeTab === 'screen' ? html`<${ScreenPanel} env=${env} target=${actions ? currentRef : null} enabled=${actions} layoutKey=${informationOpen} beforeResize=${beforeResize}
          view=${screenView} onView=${changeScreenView} onTargetGone=${onTargetGone} />` : null}
        ${activeTab === 'details' ? html`<div class="details-panel">
          ${store && store.meta.details_html ? html`<${ServerHtml} html=${store.meta.details_html} class="details-html" id="details" />` : html`<p class="empty">No details yet.</p>`}
          <div class="details-nav">
            ${store && store.lineageRequest('previous') ? html`<button type="button" class="button" id="details-previous" onClick=${() => openLineage('previous')}>Previous run</button>` : null}
            ${store && store.lineageRequest('origin') ? html`<button type="button" class="button" id="details-origin" onClick=${() => openLineage('origin')}>Original run</button>` : null}
            ${store ? html`<button type="button" class="button" id="details-latest" onClick=${followLatest}>Latest</button>` : null}
          </div>
        </div>` : null}
      </div>
    </main>
    ${route.bind && live && !(scoped && unavailable) ? html`<footer class="composer-dock">
      ${!scoped ? html`<${StaleSubmissions} env=${env} agentId=${route.id} current=${currentRef} canPrompt=${actions && !!live.can_prompt} />` : null}
      <${Composer} env=${env} live=${live} target=${currentRef} enabled=${actions}
        onTargetChanged=${(state) => state === 'incompatible' ? onTargetGone() : refreshTarget()} />
    </footer>` : null}
    ${((route.bind && !scoped && !live) || scopedRecovery) ? html`<${StaleSubmissions} env=${env}
      agentId=${route.id} scoped=${recoverScoped} dock=${true} />` : null}
    ${actions && sheet?.type === 'overflow' ? html`<${OverflowSheet} env=${env} live=${live} target=${currentRef} onClose=${() => setSheet(null)}
      onInterrupt=${() => setSheet({ type: 'interrupt' })} onKeys=${() => setSheet({ type: 'keys' })} onTargetGone=${onTargetGone} />` : null}
    ${actions && sheet?.type === 'interrupt' ? html`<${ConfirmSheet} id="sheet-interrupt-confirm" title="Interrupt the agent?" danger=${true}
      message="This sends a single Esc key to the agent's terminal. It is not retried automatically."
      confirmLabel="Send Esc" confirmId="action-interrupt-confirm" onClose=${() => setSheet(null)} onConfirm=${interrupt} />` : null}
    ${actions && sheet?.type === 'keys' ? html`<${KeysSheet} env=${env} target=${currentRef} onClose=${() => setSheet(null)} onTargetGone=${onTargetGone} />` : null}
    ${sheet && sheet.type === 'tools' ? html`<${ToolsReaderSheet} env=${env} item=${sheet.item} onClose=${() => setSheet(null)} />` : null}
    ${sheet && sheet.type === 'event' ? html`<${EventReaderSheet} env=${env} item=${sheet.item} onClose=${() => setSheet(null)} />` : null}
  </div>`;
}
