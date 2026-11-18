// Agents list (#/): session-scoped tab groups with labelled state indicators.
import { describeError, freshnessText } from '../api.js';
import { html, useState, useEffect, useLayoutEffect, useRef, usePoller, useNow, ServerHtml, RowBadges, stateLabel, QuickKeys, Banner, Freshness, responseText, cx } from './ui.js';
import { Sheet, ConfirmSheet } from './sheets.js';
import { cardDetail, detailHash } from '../app-route.js';
import { targetOf, validTarget } from '../submission.js';
import { Notifications } from './notifications.js';

function nonblank(value) {
  return typeof value === 'string' && value.trim() !== '';
}

function normalizeLabel(name) {
  return name.trim().replace(/\s+/g, ' ');
}

function groupAgents(agents) {
  const groups = new Map();
  for (const row of agents) {
    const session = nonblank(row.tab_group_session) ? row.tab_group_session
      : nonblank(row.session) ? row.session : null;
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
    const label = JSON.stringify([group.session, normalizeLabel(group.name)]);
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

function sessionMarkers(groups) {
  const ownersByLabel = new Map();
  for (const { session } of groups) {
    if (session === null) continue;
    const label = normalizeLabel(session);
    if (!ownersByLabel.has(label)) ownersByLabel.set(label, new Set());
    ownersByLabel.get(label).add(session);
  }
  const markers = new Map();
  for (const owners of ownersByLabel.values()) {
    if (owners.size < 2) continue;
    [...owners].sort().forEach((owner, index) => markers.set(owner, `${index + 1} of ${owners.size}`));
  }
  return markers;
}

function cardKeys(config) {
  const preferred = ['1', '2', 'esc'].filter((k) => config.quick_keys.includes(k));
  return preferred.length ? preferred : config.quick_keys.slice(0, 3);
}

function refIdentity(ref) {
  return validTarget(ref) ? JSON.stringify(targetOf(ref)) : null;
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
  return {
    control, kind, identity: card.dataset.agentRef, href: control.getAttribute('href'), key: control.dataset.key,
    wasVisible: revealOffset(control, header) === 0,
  };
}

function restoreCardFocus(main, header, focus) {
  if (!focus || (document.activeElement !== document.body && document.activeElement !== focus.control)) return;
  let control;
  if (focus.identity) {
    const cards = [...main.querySelectorAll(CARD_SELECTOR)].filter((card) => card.dataset.agentRef === focus.identity);
    if (cards.length !== 1) return;
    const matches = [...cards[0].querySelectorAll(CARD_CONTROLS[focus.kind])]
      .filter((candidate) => candidate.dataset.key === focus.key && !candidate.matches(':disabled'));
    if (matches.length !== 1) return;
    [control] = matches;
  } else if (focus.kind !== 'key' && main.contains(focus.control) &&
    !focus.control.closest('.agent-card')?.dataset.agentRef &&
    focus.control.getAttribute('href') === focus.href) {
    control = focus.control;
  }
  if (!control) return;
  if (document.activeElement === document.body) control.focus({ preventScroll: true });
  if (focus.wasVisible && document.activeElement === control) {
    const offset = revealOffset(control, header);
    if (offset) window.scrollBy({ top: offset, behavior: 'instant' });
  }
}

function AgentCard({ row, session, href, identity, env, onKeys, busy }) {
  const waiting = row.state === 'waiting';
  const content = html`<${ServerHtml} html=${row.html} class="card-html" />
    <${RowBadges} row=${row} peerSession=${session ?? 'unavailable'} />`;
  return html`<li class=${cx('agent-card', `card-${row.state || 'unknown'}`, row.wait_needs_a_person && 'card-needs-person')} data-agent-id=${row.id} data-agent-ref=${identity}>
    ${href ? html`<a class="card-main agent-open-area" href=${href} aria-label=${`Open ${row.title || row.id}, ${stateLabel(row.state)}`}>${content}</a>`
      : html`<div class="card-main">${content}</div>`}
    ${waiting ? html`<div class="card-actions">
      <${QuickKeys} keys=${cardKeys(env.config)} disabled=${busy || !identity} label=${`Quick keys for ${row.title || 'agent'}`}
        onKey=${(k) => onKeys(row, k)} />
      ${href ? html`<a class="button agent-open" href=${href}>Open</a>` : null}
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
  const markers = sessionMarkers(groups);
  const header = useRef(null);
  const main = useRef(null);
  const previousJump = useRef(null);
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
    const identity = refIdentity(row.ref);
    if (!identity) return;
    setBusyKeys((b) => ({ ...b, [identity]: true }));
    const agent = targetOf(row.ref);
    const r = await env.api.post('/api/agent/keys', { agent, keys: [key] });
    setBusyKeys((b) => {
      const next = { ...b };
      delete next[identity];
      return next;
    });
    const response = r.data?.response;
    if (r.status === 409 || response?.outcome === 'target_changed') {
      env.toast('The agent changed or disappeared. The key was not sent. Refresh Agents and reopen the card before trying again.');
    } else if (r.ok) env.toast(`${row.title || 'Agent'}: ${responseText(response)}`);
    else if (r.status !== 401) env.toast(`Key not confirmed: ${describeError(r)} Check the screen before trying again.`);
    refresh();
  }

  function nextWaiting() {
    const links = [...main.current.querySelectorAll(`${CARD_SELECTOR}.card-waiting > a.agent-open-area`)];
    const identities = links.map((link) => {
      const card = link.closest('.agent-card');
      return card.dataset.agentRef || link.getAttribute('href');
    });
    const previous = identities.indexOf(previousJump.current);
    const index = previous !== identities.lastIndexOf(previousJump.current) ? 0 : (previous + 1) % links.length;
    const link = links[index];
    if (!link) return;
    previousJump.current = identities[index];
    link.focus({ preventScroll: true });
    const offset = revealOffset(link, header.current);
    if (offset) window.scrollBy({ top: offset, behavior: 'instant' });
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
        ${agents.some((row) => row.state === 'waiting' && cardDetail(row, data.session)) ? html`<button type="button" class="button next-waiting" id="list-next-waiting"
          onClick=${nextWaiting}>Next waiting agent</button>` : null}
      </div>
    </header>
    ${env.notice ? html`<${Banner} id="banner-notice" tone="info" actions=${html`<button type="button" class="link-button" onClick=${env.clearNotice}>Dismiss</button>`}>${env.notice}<//>` : null}
    <main class=${cx('list-main', state && ['offline', 'retrying', 'error'].includes(state.status) && 'is-stale')} id="main" ref=${main}>
      ${state && state.error && ['offline', 'retrying', 'error'].includes(state.status) ? html`<${Banner} id="banner-offline" tone="warn">${freshnessText(state, now)}<//>` : null}
      ${!data && (!state || state.loading) ? html`<p class="empty">Loading agents…</p>` : null}
      ${data && !agents.length ? html`<p class="empty" id="list-empty">No agents are running.</p>` : null}
      ${groups.map(({ key, name, session, id, showId, rows }, index) => {
        const marker = markers.get(session);
        const sessionLabel = session === null ? 'Owning session unknown'
          : html`${marker ? `${marker} · ` : ''}Session <span class=${marker ? 'section-session-spelling' : null}>${session}</span>${showId ? ` · Group ${id}` : ''}`;
        const occurrences = new Map();
        return html`<section key=${key} class="agent-section" aria-labelledby=${`agent-group-${index}`}>
          <h2 class="section-title" id=${`agent-group-${index}`}>
            <span class="section-name">${name}</span>
            <span class="section-count">${` · ${rows.length} ${rows.length === 1 ? 'agent' : 'agents'}`}</span>
            <span class="section-session">${sessionLabel}</span>
          </h2>
          <ul class="agent-cards">
            ${rows.map((row) => {
              const route = cardDetail(row, data.session);
              const href = route && detailHash(route);
              const identity = refIdentity(row.ref);
              const cardKey = JSON.stringify([row.id, row.detail_id, identity || href]);
              const occurrence = occurrences.get(cardKey) || 0;
              occurrences.set(cardKey, occurrence + 1);
              return html`<${AgentCard} key=${JSON.stringify([cardKey, occurrence])} row=${row} session=${session} href=${href}
                identity=${identity} env=${env} onKeys=${sendKey} busy=${!!busyKeys[identity]} />`;
            })}
          </ul>
        </section>`;
      })}
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
