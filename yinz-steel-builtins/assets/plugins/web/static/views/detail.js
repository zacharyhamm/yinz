// Agent detail (#/a/<id>?run=…&tab=…): Timeline | Screen | Details, composer and sheets.
import { describeError, freshnessText } from '../api.js';
import { TimelineStore, ToolsReader } from '../timeline.js';
import { sameTarget, targetOf, validTarget } from '../submission.js';
import { detailHash } from '../app-route.js';
import {
  html, useState, useEffect, useLayoutEffect, useMemo, useRef, useCallback, usePoller, useNow,
  ServerHtml, StateBadge, RowBadges, QuickKeys, QuotaBar, Banner, responseText, keyName, cx,
} from './ui.js';
import { Sheet, ConfirmSheet } from './sheets.js';
import { Composer, StaleSubmissions } from './composer.js';
import { KeysSheet } from './keys.js';

const KIND_LABELS = { prompt: 'You', message: 'Agent', result: 'Result', plan: 'Plan', question: 'Question', step: 'Step', summary: 'Summary', tool_call: 'Tool call', tool_result: 'Tool result', tools: 'Tools' };

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
function Timeline({ items, memory, earlier, loadingEarlier, onEarlier, onTools, onEvent, loaded, layoutKey, beforeResize }) {
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
  const measure = useScrollLayout(ref, capture, restore, items, layoutKey, beforeResize);
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

function ScreenPanel({ env, live, enabled, onTargetGone, layoutKey, beforeResize }) {
  const ref = live && live.ref && validTarget(live.ref) ? targetOf(live.ref) : null;
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
      if (!r.ok && r.status === 409) onTargetGone();
      return r;
    },
  });
  async function key(k) {
    if (!ref) return;
    setBusy(true);
    const r = await env.api.post('/api/agent/keys', { agent: ref, keys: [k] });
    setBusy(false);
    if (r.ok) env.toast(`${keyName(k)}: ${responseText(r.data && r.data.response)}`);
    else if (r.status === 409) {
      env.toast('The agent changed or disappeared. The key was not sent.');
      onTargetGone();
    } else if (r.status !== 401) env.toast(`Key not confirmed: ${describeError(r)} Check the screen before trying again.`);
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
  const onScroll = useScrollLayout(scroller, capture, restore, markup, layoutKey, beforeResize, !!ref);
  if (!ref) return html`<p class="empty">No live agent to read.</p>`;
  const response = data && data.response;
  const failed = response && response.outcome && response.outcome !== 'ok' && response.outcome !== 'completed';
  return html`<div class="screen-panel">
    <div class="screen-feedback" role="region" aria-label="Screen status and feedback" tabindex="0">
      <div class="screen-status">
        ${state ? html`<span class=${cx('freshness', state.status !== 'ok' && 'freshness-stale')}>${freshnessText(state, now)}</span>` : null}
        <button type="button" class="link-button" id="screen-refresh" onClick=${() => refresh()}>Refresh</button>
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

function OverflowSheet({ env, live, onClose, onInterrupt, onKeys, onTargetGone }) {
  const peer = live.current_session === false;
  async function focus() {
    onClose();
    const r = await env.api.post('/api/agent/focus', { agent: targetOf(live.ref) });
    if (r.ok) env.toast(peer ? `Desktop switched to session ${live.session}.` : 'Focused on the desktop.');
    else if (r.status === 409 && r.code === 'target_changed') {
      env.toast('The agent changed or disappeared.');
      onTargetGone();
    } else if (r.status !== 401) env.toast(describeError(r));
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
  const identity = route.bind ? `a:${route.id}:${route.run ? 'run' : 'none'}` : `r:${route.run}`;
  const store = useMemo(() => (route.run ? new TimelineStore({ run: route.run, agent: route.bind ? route.id : null }) : null), [identity]);
  const [version, setVersion] = useState(0);
  const bump = () => setVersion((v) => v + 1);
  const [tab, setTabState] = useState(route.tab);
  const [sheet, setSheet] = useState(null);
  const [informationOpen, setInformationOpen] = useState(false);
  const beforeResize = useRef(null);
  const [retargetNotice, setRetargetNotice] = useState(false);
  const [loadingEarlier, setLoadingEarlier] = useState(false);
  const earlierController = useRef(null);
  const memory = useRef({ atBottom: true, anchor: null, lastKey: null }).current;
  const lastRef = useRef(null);
  const now = useNow(1000);

  const [tail, refreshTail] = usePoller({
    key: identity,
    enabled: !!store,
    intervalMs: env.config.poll.tail_ms,
    env,
    run: async ({ signal }) => {
      const ticket = store.ticket(store.loaded ? 'received' : 'latest');
      const r = await env.api.read(store.refreshRequest(), env.tz, { signal });
      if (r.ok && !env.api.stale(r)) {
        const previous = store.meta.live && store.meta.live.ref;
        const applied = store.apply(ticket, r.data);
        if (!applied.applied && applied.reason === 'malformed') return { ok: false, kind: 'malformed' };
        if (applied.retargeted) onRetarget(previous);
        bump();
      }
      return r;
    },
  });

  // Agent without a recorded run yet: find its live row in the list.
  const [list] = usePoller({
    key: `list-for:${route.id}`,
    enabled: !route.run && route.bind,
    intervalMs: env.config.poll.list_ms,
    env,
    run: ({ signal }) => env.api.get('/api/agents', { signal }),
  });
  const listRow = !route.run && list && list.data && Array.isArray(list.data.agents) ? list.data.agents.find((a) => a && a.id === route.id) || null : null;
  useEffect(() => {
    if (listRow && listRow.run) env.replaceHash(detailHash({ id: route.id, run: listRow.run, tab: tab || undefined }));
  }, [listRow && listRow.run]);

  const meta = store ? store.meta : { live: listRow, agent_gone: false };
  const live = route.bind ? meta.live : null;
  const actions = route.bind && !!live && !meta.agent_gone;
  const currentRef = live && live.ref && validTarget(live.ref) ? targetOf(live.ref) : null;

  function onRetarget(previousRef) {
    setRetargetNotice(true);
    memory.atBottom = true;
    memory.anchor = null;
    if (previousRef && validTarget(previousRef)) {
      env.submissions.markIncompatible(targetOf(previousRef), { reason: 'The agent moved to a new run. This prompt was not sent to it.' });
    }
    if (earlierController.current) earlierController.current.abort();
    env.replaceHash(detailHash({ id: route.id, run: store.run, tab: tab || undefined, bind: route.bind }));
  }

  // Same-pane replacement (different kind or conversation): disable old records.
  useEffect(() => {
    if (!currentRef) return;
    const previous = lastRef.current;
    if (previous && !sameTarget(previous, currentRef)) {
      env.submissions.markIncompatible(previous, { replacement: currentRef, reason: 'The agent identity changed. This prompt was not sent to the replacement.' });
    }
    for (const rec of env.submissions.staleRecords(route.id, currentRef)) {
      if (!rec.incompatible) env.submissions.markIncompatible(rec.target, { replacement: currentRef, reason: 'This draft was written for an earlier conversation of this agent.' });
    }
    lastRef.current = currentRef;
  }, [currentRef && `${currentRef.kind}|${currentRef.session_id}`]);

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
    if (!currentRef) return;
    const r = await env.api.post('/api/agent/keys', { agent: currentRef, keys: ['esc'] });
    if (r.ok) env.toast(`Interrupt: ${responseText(r.data && r.data.response)}`);
    else if (r.status === 409) {
      env.toast('The agent changed or disappeared. Esc was not sent.');
      refreshTail();
    } else if (r.status !== 401) env.toast(`Interrupt not confirmed: ${describeError(r)} Check the screen before trying again.`);
  }

  // A new array only after a store change: the one-second age re-render must not rerun
  // the timeline's anchoring effect.
  const items = useMemo(() => (store ? store.items() : []), [store, version]);
  const stale = tail && (tail.status === 'offline' || tail.status === 'retrying' || tail.status === 'error');
  const title = live && live.title ? String(live.title) : store && store.meta.header && store.meta.header.name ? String(store.meta.header.name) : route.id;
  useEffect(() => {
    document.title = `${title} · ${env.config.title}`;
  }, [title]);

  // Return through history when the entry behind this one is the app's; otherwise
  // replace this entry with the list rather than adding one.
  const back = () => {
    if (env.canGoBack()) window.history.back();
    else env.replaceHash('#/');
  };

  return html`<div class=${cx('page', 'detail-page', activeTab === 'screen' && 'detail-screen')}>
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
      ${route.bind && meta.agent_gone ? html`<${Banner} id="banner-agent-gone" tone="warn">This agent is gone. Its history stays readable and actions are disabled. Checking for its return…<//>` : null}
      ${retargetNotice ? html`<${Banner} id="banner-retargeted" tone="info"
        actions=${html`<button type="button" class="link-button" onClick=${() => setRetargetNotice(false)}>Dismiss</button>`}>
        The agent moved to a new run; the timeline was reset.<//>` : null}
      ${stale ? html`<${Banner} id="banner-offline" tone="warn"
        actions=${html`<button type="button" class="link-button" onClick=${() => refreshTail()}>Retry</button>`}>${freshnessText(tail, now)}<//>` : null}
      ${!route.run && route.bind && !listRow ? html`<${Banner} id="banner-no-run" tone="info">${list && list.data ? 'This agent is not running.' : 'Looking for this agent…'}<//>` : null}
    </div>
    <main class=${cx('detail-body', stale && 'is-stale')} id="main">
      <div id=${`panel-${activeTab}`} role="tabpanel" aria-labelledby=${`tab-${activeTab}`} class="panel">
        ${activeTab === 'timeline' ? html`<${Timeline} items=${items} memory=${memory} loaded=${!store || store.loaded} layoutKey=${informationOpen} beforeResize=${beforeResize}
          earlier=${store && !!store.earlierRequest()} loadingEarlier=${loadingEarlier} onEarlier=${loadEarlier}
          onTools=${(item) => setSheet({ type: 'tools', item })} onEvent=${(item) => setSheet({ type: 'event', item })} />` : null}
        ${activeTab === 'screen' ? html`<${ScreenPanel} env=${env} live=${live} enabled=${actions} layoutKey=${informationOpen} beforeResize=${beforeResize} onTargetGone=${() => refreshTail()} />` : null}
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
    ${route.bind && live ? html`<footer class="composer-dock">
      <${StaleSubmissions} env=${env} agentId=${route.id} current=${currentRef} canPrompt=${actions && !!live.can_prompt} />
      <${Composer} env=${env} live=${live} enabled=${actions} onTargetChanged=${() => refreshTail()} />
    </footer>` : null}
    ${route.bind && !live ? html`<${StaleSubmissions} env=${env} agentId=${route.id} current=${null} canPrompt=${false} dock=${true} />` : null}
    ${sheet && sheet.type === 'overflow' && live && currentRef ? html`<${OverflowSheet} env=${env} live=${live} onClose=${() => setSheet(null)}
      onInterrupt=${() => setSheet({ type: 'interrupt' })} onKeys=${() => setSheet({ type: 'keys' })} onTargetGone=${() => refreshTail()} />` : null}
    ${sheet && sheet.type === 'interrupt' ? html`<${ConfirmSheet} id="sheet-interrupt-confirm" title="Interrupt the agent?" danger=${true}
      message="This sends a single Esc key to the agent's terminal. It is not retried automatically."
      confirmLabel="Send Esc" confirmId="action-interrupt-confirm" onClose=${() => setSheet(null)} onConfirm=${interrupt} />` : null}
    ${sheet && sheet.type === 'keys' && currentRef ? html`<${KeysSheet} env=${env} target=${currentRef} onClose=${() => setSheet(null)} onTargetGone=${() => refreshTail()} />` : null}
    ${sheet && sheet.type === 'tools' ? html`<${ToolsReaderSheet} env=${env} item=${sheet.item} onClose=${() => setSheet(null)} />` : null}
    ${sheet && sheet.type === 'event' ? html`<${EventReaderSheet} env=${env} item=${sheet.item} onClose=${() => setSheet(null)} />` : null}
  </div>`;
}
