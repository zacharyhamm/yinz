// Agents list (#/): server-ordered sections with labelled state indicators.
import { describeError, freshnessText } from '../api.js';
import { html, useState, useEffect, usePoller, useNow, ServerHtml, RowBadges, QuickKeys, Banner, Freshness, responseText, cx } from './ui.js';
import { Sheet, ConfirmSheet } from './sheets.js';
import { detailHash } from '../app-route.js';

const SECTIONS = [
  ['waiting', 'Needs you'],
  ['busy', 'Working'],
  ['idle', 'Idle'],
];

function cardKeys(config) {
  const preferred = ['1', '2', 'esc'].filter((k) => config.quick_keys.includes(k));
  return preferred.length ? preferred : config.quick_keys.slice(0, 3);
}

function AgentCard({ row, env, onKeys, busy, showState }) {
  const waiting = row.state === 'waiting';
  const href = row.run ? detailHash({ id: row.id, run: row.run, tab: waiting ? 'screen' : 'timeline' }) : detailHash({ id: row.id, run: '', tab: 'screen' });
  return html`<li class=${cx('agent-card', `card-${row.state || 'unknown'}`, row.wait_needs_a_person && 'card-needs-person')} data-agent-id=${row.id}>
    <a class="card-main agent-open-area" href=${href} aria-label=${`Open ${row.title || row.id}`}>
      <${ServerHtml} html=${row.html} class="card-html" />
      <${RowBadges} row=${row} showState=${showState} />
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

  const groups = SECTIONS.map(([key, label]) => [key, label, agents.filter((a) => a.state === key)]);
  const other = agents.filter((a) => !SECTIONS.some(([k]) => k === a.state));
  if (other.length) groups.push(['other', 'Other', other]);

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
      ${groups.filter(([, , rows]) => rows.length).map(([key, label, rows]) => html`<section class="agent-section" id=${`section-${key}`} data-state=${key} aria-labelledby=${`section-${key}-title`}>
        <h2 class="section-title" id=${`section-${key}-title`}>${label} <span class="section-count">${rows.length}</span></h2>
        <ul class="agent-cards">
          ${rows.map((row) => html`<${AgentCard} key=${row.id} row=${row} env=${env} onKeys=${sendKey} busy=${!!busyKeys[row.id]} showState=${key === 'other'} />`)}
        </ul>
      </section>`)}
    </main>
    ${menu ? html`<${Sheet} id="sheet-menu" title="Menu" onClose=${() => setMenu(false)}>
      <div class="menu-list">
        <button type="button" class="button menu-item" id="logout" onClick=${() => { setMenu(false); env.logout(false); }}>Log out</button>
        <button type="button" class="button menu-item" id="logout-all" onClick=${() => { setMenu(false); setConfirmAll(true); }}>Log out everywhere</button>
      </div>
    <//>` : null}
    ${confirmAll ? html`<${ConfirmSheet} id="sheet-logout-all" title="Log out everywhere?" danger=${true}
      message="Every signed-in device, including this one, will need the password again."
      confirmLabel="Log out everywhere" confirmId="logout-all-confirm" onClose=${() => setConfirmAll(false)} onConfirm=${() => env.logout(true)} />` : null}
  </div>`;
}
