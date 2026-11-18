// Agents list (#/): session-scoped tab groups with labelled state indicators.
import { describeError, freshnessText } from '../api.js';
import { html, useState, useEffect, useLayoutEffect, useRef, usePoller, useNow, ServerHtml, RowBadges, stateLabel, QuickKeys, Banner, Freshness, responseText, cx } from './ui.js';
import { Sheet, ConfirmSheet } from './sheets.js';
import { detailHash } from '../app-route.js';
import { Notifications } from './notifications.js';

function nonblank(value) {
  return typeof value === 'string' && value.trim() !== '';
}

function groupAgents(agents) {
  const groups = new Map();
  for (const row of agents) {
    const session = nonblank(row.session) ? row.session : null;
    const id = row.tab_group?.id;
    const hasGroup = session !== null && Number.isInteger(id) && id >= 0 && id <= 0xffffffff;
    const key = JSON.stringify([session, hasGroup ? id : null]);
    if (!groups.has(key)) {
      const name = hasGroup
        ? (nonblank(row.tab_group.name) ? row.tab_group.name : 'Unnamed tab group')
        : 'Tab group unavailable';
      groups.set(key, { key, name, session, id: hasGroup ? id : null, rows: [] });
    }
    groups.get(key).rows.push(row);
  }
  const labels = new Map();
  for (const group of groups.values()) {
    const label = JSON.stringify([group.session, group.name.trim().replace(/\s+/g, ' ')]);
    const sameLabel = labels.get(label);
    if (sameLabel) {
      sameLabel.showId = sameLabel.id !== null;
      group.showId = group.id !== null;
    } else {
      labels.set(label, group);
    }
  }
  return [...groups.values()];
}

function cardKeys(config) {
  const preferred = ['1', '2', 'esc'].filter((k) => config.quick_keys.includes(k));
  return preferred.length ? preferred : config.quick_keys.slice(0, 3);
}

function quickKeyIdentity(ref) {
  const identity = [ref?.id, ref?.kind, ref?.session_id];
  return identity.every((part) => nonblank(part) && !/\p{Cc}/u.test(part)) ? JSON.stringify(identity) : null;
}

const CARD_SELECTOR = ':scope > .agent-section > .agent-cards > .agent-card';
const CARD_CONTROLS = {
  main: ':scope > a.agent-open-area',
  open: ':scope > .card-actions > a.agent-open',
  key: ':scope > .card-actions > .quick-keys > button.quick-key',
};

function revealOffset(control, header) {
  const rect = control.getBoundingClientRect();
  const viewport = window.visualViewport;
  const viewportTop = viewport?.offsetTop ?? 0;
  const top = Math.max(viewportTop, header.getBoundingClientRect().bottom);
  const bottom = viewportTop + (viewport?.height ?? window.innerHeight);
  if (!rect.height || top >= bottom) return null;
  if (rect.top < top - 1) return rect.top - top;
  if (rect.height > bottom - top) return rect.top >= bottom ? rect.top - top : 0;
  return rect.bottom > bottom + 1 ? rect.bottom - bottom : 0;
}

function captureCardFocus(main, header) {
  const control = document.activeElement;
  const card = control?.closest('.agent-card');
  if (!card || card.parentElement?.parentElement?.parentElement !== main) return null;
  const kind = Object.keys(CARD_CONTROLS).find((kind) =>
    [...card.querySelectorAll(CARD_CONTROLS[kind])].includes(control));
  if (!kind) return null;
  const identity = kind === 'key' ? card.getAttribute('data-quick-key-ref') : null;
  if (kind === 'key' && !identity) return null;
  return {
    control, kind, identity, id: card.dataset.agentId, key: control.dataset.key,
    wasVisible: revealOffset(control, header) === 0,
  };
}

function restoreCardFocus(main, header, focus) {
  if (!focus || (document.activeElement !== document.body && document.activeElement !== focus.control)) return;
  const eligible = (card) => card?.dataset.agentId === focus.id &&
    (focus.kind !== 'key' || card.getAttribute('data-quick-key-ref') === focus.identity);
  let control = focus.control;
  if (!main.contains(control) || !eligible(control.closest('.agent-card'))) {
    const matches = [...main.querySelectorAll(CARD_SELECTOR)].filter(eligible)
      .flatMap((card) => [...card.querySelectorAll(CARD_CONTROLS[focus.kind])])
      .filter((candidate) => candidate.dataset.key === focus.key);
    if (matches.length !== 1) return;
    [control] = matches;
  }
  if (control.matches(':disabled')) return;
  if (document.activeElement === document.body) control.focus({ preventScroll: true });
  if (focus.wasVisible && document.activeElement === control) {
    const offset = revealOffset(control, header);
    if (offset) window.scrollBy({ top: offset, behavior: 'instant' });
  }
}

function AgentCard({ row, env, onKeys, busy }) {
  const waiting = row.state === 'waiting';
  // Replacing the conversation also replaces its quick keys, even in the same group.
  const identity = waiting ? quickKeyIdentity(row.ref) : null;
  const href = row.run ? detailHash({ id: row.id, run: row.run, tab: waiting ? 'screen' : 'timeline' }) : detailHash({ id: row.id, run: '', tab: 'screen' });
  return html`<li class=${cx('agent-card', `card-${row.state || 'unknown'}`, row.wait_needs_a_person && 'card-needs-person')} data-agent-id=${row.id} data-quick-key-ref=${identity}>
    <a class="card-main agent-open-area" href=${href} aria-label=${`Open ${row.title || row.id}, ${stateLabel(row.state)}`}>
      <${ServerHtml} html=${row.html} class="card-html" />
      <${RowBadges} row=${row} />
    </a>
    ${waiting ? html`<div class="card-actions">
      <${QuickKeys} key=${identity} keys=${cardKeys(env.config)} disabled=${busy || !row.ref} label=${`Quick keys for ${row.title || 'agent'}`}
        onKey=${(k) => onKeys(row, k)} />
      <a class="button agent-open" href=${href}>Open</a>
    </div>` : null}
  </li>`;
}

export function ListView({ env }) {
  const now = useNow(1000);
  const [menu, setMenu] = useState(false);
  const [confirmAll, setConfirmAll] = useState(false);
  const [busyKeys, setBusyKeys] = useState({});
  const [state, refresh] = usePoller({
    key: 'agents',
    intervalMs: env.config.poll.list_ms,
    env,
    run: async ({ signal }) => {
      const r = await env.api.get('/api/agents', { signal });
      return r.ok && (!r.data || !Array.isArray(r.data.agents)) ? { ok: false, kind: 'malformed' } : r;
    },
  });
  const data = state && state.data;
  const agents = data ? data.agents.filter((a) => a && typeof a.id === 'string') : [];
  const waiting = data && data.counts && Number.isInteger(data.counts.waiting) ? data.counts.waiting : agents.filter((a) => a.state === 'waiting').length;
  const groups = groupAgents(agents);
  const header = useRef(null);
  const main = useRef(null);
  const previousData = useRef(null);
  const focus = data && data !== previousData.current && main.current
    ? captureCardFocus(main.current, header.current)
    : null;

  // Capture before reconciliation, while the old card and its position still exist.
  useLayoutEffect(() => {
    previousData.current = data;
    restoreCardFocus(main.current, header.current, focus);
  }, [data]);

  useEffect(() => {
    document.title = waiting > 0 ? `(${waiting}) ${env.config.title}` : env.config.title;
  }, [waiting]);

  async function sendKey(row, key) {
    if (!row.ref) return;
    setBusyKeys((b) => ({ ...b, [row.id]: true }));
    const agent = { id: row.ref.id, kind: row.ref.kind, session_id: row.ref.session_id };
    const r = await env.api.post('/api/agent/keys', { agent, keys: [key] });
    setBusyKeys((b) => ({ ...b, [row.id]: false }));
    if (r.ok) env.toast(`${row.title || 'Agent'}: ${responseText(r.data && r.data.response)}`);
    else if (r.status === 409) env.toast('The agent changed or disappeared. The key was not sent; the list was refreshed.');
    else if (r.status !== 401) env.toast(`Key not confirmed: ${describeError(r)} Check the screen before trying again.`);
    refresh();
  }

  return html`<div class="page list-page">
    <header class="app-header" id="list-header" ref=${header}>
      <div class="header-row">
        <h1 class="header-title">Agents${waiting > 0 ? html` · <span class="need-count">${waiting} need you</span>` : null}</h1>
        <button type="button" class="icon-button" id="menu-button" aria-label="Menu" aria-haspopup="dialog" onClick=${() => setMenu(true)}>⋯</button>
      </div>
      <div class="header-sub">
        <${Freshness} id="list-freshness" state=${state} now=${now} />
        ${data && typeof data.session === 'string' ? html`<span class="serving-session" id="list-session">Session ${data.session}</span>` : null}
        <button type="button" class="link-button" id="list-refresh" onClick=${() => refresh()}>Refresh</button>
      </div>
    </header>
    ${env.notice ? html`<${Banner} id="banner-notice" tone="info" actions=${html`<button type="button" class="link-button" onClick=${env.clearNotice}>Dismiss</button>`}>${env.notice}<//>` : null}
    <main class=${cx('list-main', state && ['offline', 'retrying', 'error'].includes(state.status) && 'is-stale')} id="main" ref=${main}>
      ${state && state.error && ['offline', 'retrying', 'error'].includes(state.status) ? html`<${Banner} id="banner-offline" tone="warn">${freshnessText(state, now)}<//>` : null}
      ${!data && (!state || state.loading) ? html`<p class="empty">Loading agents…</p>` : null}
      ${data && !agents.length ? html`<p class="empty" id="list-empty">No agents are running.</p>` : null}
      ${groups.map(({ key, name, session, id, showId, rows }, index) => html`<section key=${key} class="agent-section" aria-labelledby=${`agent-group-${index}`}>
        <h2 class="section-title" id=${`agent-group-${index}`}>
          <span class="section-name">${name}</span>
          <span class="section-count">${` · ${rows.length} ${rows.length === 1 ? 'agent' : 'agents'}`}</span>
          <span class="section-session">${session === null ? 'Session unavailable' : `Session ${session}${showId ? ` · Group ${id}` : ''}`}</span>
        </h2>
        <ul class="agent-cards">
          ${rows.map((row) => html`<${AgentCard} key=${row.id} row=${row} env=${env} onKeys=${sendKey} busy=${!!busyKeys[row.id]} />`)}
        </ul>
      </section>`)}
    </main>
    ${menu ? html`<${Sheet} id="sheet-menu" title="Menu" onClose=${() => setMenu(false)}>
      <div class="menu-list">
        <${Notifications} push=${env.push} />
        <button type="button" class="button menu-item" id="logout" onClick=${() => { setMenu(false); env.logout(false); }}>Log out</button>
        <button type="button" class="button menu-item" id="logout-all" onClick=${() => { setMenu(false); setConfirmAll(true); }}>Log out everywhere</button>
      </div>
    <//>` : null}
    ${confirmAll ? html`<${ConfirmSheet} id="sheet-logout-all" title="Log out everywhere?" danger=${true}
      message="Every signed-in device, including this one, will need the password again."
      confirmLabel="Log out everywhere" confirmId="logout-all-confirm" onClose=${() => setConfirmAll(false)} onConfirm=${() => env.logout(true)} />` : null}
  </div>`;
}
