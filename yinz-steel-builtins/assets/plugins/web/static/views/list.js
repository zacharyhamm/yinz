// Agents list (#/): session-scoped tab groups with labelled state indicators.
import { describeError, freshnessText } from '../api.js';
import { html, useState, useEffect, useLayoutEffect, usePoller, useNow, ServerHtml, RowBadges, stateLabel, QuickKeys, Banner, Freshness, responseText, cx } from './ui.js';
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
      groups.set(key, { key, name, session, rows: [] });
    }
    groups.get(key).rows.push(row);
  }
  return [...groups.values()];
}

function cardKeys(config) {
  const preferred = ['1', '2', 'esc'].filter((k) => config.quick_keys.includes(k));
  return preferred.length ? preferred : config.quick_keys.slice(0, 3);
}

function AgentCard({ row, env, onKeys, busy }) {
  const waiting = row.state === 'waiting';
  const href = row.run ? detailHash({ id: row.id, run: row.run, tab: waiting ? 'screen' : 'timeline' }) : detailHash({ id: row.id, run: '', tab: 'screen' });
  return html`<li class=${cx('agent-card', `card-${row.state || 'unknown'}`, row.wait_needs_a_person && 'card-needs-person')} data-agent-id=${row.id}>
    <a class="card-main agent-open-area" href=${href} aria-label=${`Open ${row.title || row.id}, ${stateLabel(row.state)}`}>
      <${ServerHtml} html=${row.html} class="card-html" />
      <${RowBadges} row=${row} />
    </a>
    ${waiting ? html`<div class="card-actions">
      <${QuickKeys} keys=${cardKeys(env.config)} disabled=${busy || !row.ref} label=${`Quick keys for ${row.title || 'agent'}`}
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
  const focusedControl = document.activeElement?.closest('.agent-card a, .agent-card button');

  // Reordering a keyed section can move its existing DOM nodes and blur a control.
  useLayoutEffect(() => {
    if (focusedControl?.isConnected && document.activeElement === document.body) {
      focusedControl.focus({ preventScroll: true });
    }
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
    <header class="app-header" id="list-header">
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
    <main class=${cx('list-main', state && ['offline', 'retrying', 'error'].includes(state.status) && 'is-stale')} id="main">
      ${state && state.error && ['offline', 'retrying', 'error'].includes(state.status) ? html`<${Banner} id="banner-offline" tone="warn">${freshnessText(state, now)}<//>` : null}
      ${!data && (!state || state.loading) ? html`<p class="empty">Loading agents…</p>` : null}
      ${data && !agents.length ? html`<p class="empty" id="list-empty">No agents are running.</p>` : null}
      ${groups.map(({ key, name, session, rows }, index) => html`<section key=${key} class="agent-section" aria-labelledby=${`agent-group-${index}`}>
        <h2 class="section-title" id=${`agent-group-${index}`}>
          <span class="section-name">${name}</span>
          <span class="section-count">${rows.length}</span>
          <span class="section-session">${session === null ? 'Session unavailable' : `Session ${session}`}</span>
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
