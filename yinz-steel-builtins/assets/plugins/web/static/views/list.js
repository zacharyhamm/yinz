// Agents pages: one session-scoped tab group at a time.
import { describeError, freshnessText } from '../api.js';
import { html, useState, useEffect, useLayoutEffect, useRef, usePoller, useNow, ServerHtml, RowBadges, stateLabel, QuickKeys, Banner, responseText, cx } from './ui.js';
import { Sheet, ConfirmSheet } from './sheets.js';
import { cardDetail, controlTarget, detailHash, groupHash, isScopedPeer, validGroupId } from '../app-route.js';
import { StartSheet, LaunchWait } from './manage.js';
import { SiriSetup } from './siri.js';
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
    const hasGroup = session !== null && validGroupId(id);
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

function GroupLabel({ group, marker }) {
  const { name, session, id, showId, rows } = group;
  return html`<span class="section-name">${name}</span>
    <span class="section-count">${` · ${rows.length} ${rows.length === 1 ? 'agent' : 'agents'}`}</span>
    <span class="section-session">${session === null ? 'Owning session unknown'
      : html`${marker ? `${marker} · ` : ''}Session <span class=${marker ? 'section-session-spelling' : null}>${session}</span>${showId ? ` · Group ${id}` : ''}`}</span>`;
}

function cardKeys(config) {
  const preferred = ['1', '2', 'esc'].filter((k) => config.quick_keys.includes(k));
  return preferred.length ? preferred : config.quick_keys.slice(0, 3);
}

const CARD_SELECTOR = ':scope > .agent-section > .agent-cards > .agent-card';
const CARD_CONTROLS = {
  main: ':scope > a.agent-open-area',
  next: ':scope > .card-waiting-next',
  open: ':scope > .card-actions > a.agent-open',
  key: ':scope > .card-actions > .quick-keys > button.quick-key',
};

// The lowest edge of the pinned chrome: the Agents bar, or the group heading once it sticks under it.
function obscuredTop(header) {
  const bottom = header.getBoundingClientRect().bottom;
  const heading = header.parentElement?.querySelector(':scope > .list-main > .agent-section > .section-title');
  const rect = heading?.getBoundingClientRect();
  return rect && rect.height && rect.top <= bottom + 1 ? Math.max(bottom, rect.bottom) : bottom;
}

function revealOffset(control, header) {
  const rect = control.getBoundingClientRect();
  const viewport = window.visualViewport;
  const viewportTop = viewport?.offsetTop ?? 0;
  const top = Math.max(viewportTop, obscuredTop(header));
  const bottom = viewportTop + (viewport?.height ?? window.innerHeight);
  if (!rect.height || top >= bottom) return null;
  if (rect.top < top - 1) return rect.top - top;
  if (rect.height > bottom - top) return rect.top >= bottom ? rect.top - top : 0;
  return rect.bottom > bottom + 1 ? rect.bottom - bottom : 0;
}

function focusAndReveal(control, header) {
  control.focus({ preventScroll: true });
  const offset = revealOffset(control, header);
  if (offset) window.scrollBy({ top: offset, behavior: 'instant' });
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
    if (cards.length === 1) {
      const matches = [...cards[0].querySelectorAll(CARD_CONTROLS[focus.kind])]
        .filter((candidate) => candidate.dataset.key === focus.key && !candidate.matches(':disabled'));
      if (matches.length === 1) [control] = matches;
    }
  } else if (focus.href && main.contains(focus.control) &&
    !focus.control.closest('.agent-card')?.dataset.agentRef &&
    focus.control.getAttribute('href') === focus.href) {
    control = focus.control;
  }
  if (!control) {
    if (document.activeElement === focus.control) focus.control.blur();
    return;
  }
  if (document.activeElement !== control) control.focus({ preventScroll: true });
  if (focus.wasVisible && document.activeElement === control) {
    const offset = revealOffset(control, header);
    if (offset) window.scrollBy({ top: offset, behavior: 'instant' });
  }
}

// Return the scroll adjustment for a link outside the sheet body's scrollport.
function hiddenGroupLinkOffset(link, body) {
  const rect = link.getBoundingClientRect();
  if (!rect.height || !body.clientHeight) return null;
  const top = body.getBoundingClientRect().top + body.clientTop;
  const bottom = top + body.clientHeight;
  if (rect.bottom <= top) return rect.top - top;
  if (rect.top >= bottom) return Math.min(rect.top - top, rect.bottom - bottom);
  return 0;
}

function AgentCard({ row, session, href, identity, env, onKeys, onNext, busy }) {
  const waiting = row.state === 'waiting';
  const content = html`<${ServerHtml} html=${row.html} class="card-html" />
    <${RowBadges} row=${row} peerSession=${session ?? 'unavailable'} />
    ${!href ? html`<span class="hint">${nonblank(row.prompt_unavailable) ? row.prompt_unavailable
      : 'Live controls are unavailable. Refresh Agents to try again.'}</span>` : null}`;
  return html`<li class=${cx('agent-card', `card-${row.state || 'unknown'}`, row.wait_needs_a_person && 'card-needs-person')} data-agent-id=${row.id} data-agent-ref=${identity}>
    ${href ? html`<a class="card-main agent-open-area" href=${href} aria-label=${`Open ${row.title || row.id}, ${stateLabel(row.state)}`}>${content}</a>`
      : html`<div class="card-main">${content}</div>`}
    ${onNext ? html`<button type="button" class="link-button card-waiting-next"
      onClick=${onNext}>Next waiting agent</button>` : null}
    ${waiting ? html`<div class="card-actions">
      <${QuickKeys} keys=${cardKeys(env.config)} disabled=${busy || !identity} label=${`Quick keys for ${row.title || 'agent'}`}
        onKey=${(k) => onKeys(row, k)} />
      ${href ? html`<a class="button agent-open" href=${href}>Open</a>` : null}
    </div>` : null}
  </li>`;
}

export function ListView({ env, groupKey, navigation, entry }) {
  const now = useNow(1000);
  const [siri, setSiri] = useState(false);
  const [menu, setMenu] = useState(false);
  const [picker, setPicker] = useState(false);
  const [confirmAll, setConfirmAll] = useState(false);
  const [busyKeys, setBusyKeys] = useState({});
  const [launcher, setLauncher] = useState(null);
  const launch = launcher?.entry === entry ? launcher?.launch : null;
  useLayoutEffect(() => setLauncher(null), [entry]);
  const [waitingForLaunch, setWaitingForLaunch] = useState(false);
  const [state, refresh] = usePoller({
    key: 'agents',
    intervalMs: waitingForLaunch ? Math.min(1000, env.config.poll.list_ms) : env.config.poll.list_ms,
    env,
    run: async ({ signal }) => {
      const r = await env.api.get('/api/agents', { signal });
      return r.ok && (!r.data || !Array.isArray(r.data.agents)) ? { ok: false, kind: 'malformed' } : r;
    },
  });
  const data = state && state.data;
  const hasData = !!data;
  const agents = data ? data.agents.filter((a) => a && typeof a.id === 'string') : [];
  const waiting = data && data.counts && Number.isInteger(data.counts.waiting) ? data.counts.waiting : agents.filter((a) => a.state === 'waiting').length;
  const groups = groupAgents(agents);
  const markers = sessionMarkers(groups);
  const selected = groupKey === undefined
    ? groups.find((group) => group.session === data?.session && group.id === 0) ?? groups[0]
    : groups.find((group) => group.key === groupKey);
  const pageKey = groupKey ?? selected?.key;
  const cards = new Map();
  const bindings = new Map();
  for (const row of agents) {
    const route = cardDetail(row, data.session);
    const href = route && detailHash(route);
    const target = controlTarget(row, data.session);
    const binding = target ? JSON.stringify(target) : null;
    if (binding) bindings.set(binding, (bindings.get(binding) || 0) + 1);
    cards.set(row, { href, target, binding, scoped: route?.bind && isScopedPeer(route.id) ? route.id : null });
  }
  const waitingAgents = groups.flatMap((group) => group.rows
    .filter((row) => row.state === 'waiting' && cards.get(row).href)
    .map((row) => {
      const { binding, href } = cards.get(row);
      return { row, key: group.key, identity: bindings.get(binding) === 1 ? binding : href };
    }));
  const pageWaiting = waitingAgents.filter((agent) => agent.key === pageKey);
  const header = useRef(null);
  const main = useRef(null);
  const launchBanner = useRef(null);
  // The group heading sticks below the Agents bar, whose height follows the safe area and wrapping.
  // A constructed stylesheet carries the measurements, so the page keeps no inline styles
  // (CSP style-src 'self'). Border boxes include the bar's safe-area padding. Browsers
  // without constructed stylesheets (Safari before 16.4) keep app.css's one-row estimate.
  useLayoutEffect(() => {
    if (!('adoptedStyleSheets' in document)) return undefined;
    const heading = main.current.querySelector(':scope > .agent-section > .section-title');
    const sheet = new CSSStyleSheet();
    document.adoptedStyleSheets.push(sheet);
    const measure = () => {
      const bar = header.current.getBoundingClientRect().height;
      const sticky = bar + (heading?.getBoundingClientRect().height ?? 0);
      sheet.replaceSync(`.list-page { --list-header-height: ${bar}px; --list-sticky-height: ${sticky}px; }`);
    };
    const observer = new ResizeObserver(measure);
    observer.observe(header.current, { box: 'border-box' });
    if (heading) observer.observe(heading, { box: 'border-box' });
    measure();
    return () => {
      observer.disconnect();
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter((candidate) => candidate !== sheet);
    };
  }, [pageKey, !!selected]);
  useLayoutEffect(() => {
    if (launch) focusAndReveal(launchBanner.current, header.current);
  }, [launch]);
  const previousJump = useRef(null);
  const pendingJump = useRef(null);
  const previousPage = useRef(pageKey);
  const handledNavigation = useRef(null);
  const previousData = useRef(null);
  // Capture focus before reconciliation, while the old cards and picker links still exist.
  const dataChanged = data && data !== previousData.current;
  const cardFocus = dataChanged && pageKey === previousPage.current && main.current
    ? captureCardFocus(main.current, header.current)
    : null;
  const pickerSheet = dataChanged && picker ? document.getElementById('sheet-groups') : null;
  const pickerBody = pickerSheet?.querySelector('.sheet-body');
  const focusedGroup = pickerBody?.querySelector('.group-link:focus');
  const pickerHref = focusedGroup?.getAttribute('href');
  const pickerLinkWasVisible = focusedGroup && hiddenGroupLinkOffset(focusedGroup, pickerBody) === 0;

  useLayoutEffect(() => {
    if (groupKey === undefined && pageKey !== undefined) env.replaceHash(groupHash(pageKey));
  }, [groupKey, pageKey]);

  useLayoutEffect(() => {
    previousData.current = data;
    restoreCardFocus(main.current, header.current, cardFocus);
    const active = document.activeElement;
    if (!pickerHref || !pickerBody.isConnected || (active !== document.body && active !== focusedGroup)) return;
    const link = [...pickerBody.querySelectorAll('.group-link')]
      .find((candidate) => candidate.getAttribute('href') === pickerHref);
    if (!link) {
      pickerSheet.querySelector('.sheet-close')?.focus({ preventScroll: true });
      return;
    }
    if (active !== link) link.focus({ preventScroll: true });
    if (pickerLinkWasVisible && document.activeElement === link) {
      const offset = hiddenGroupLinkOffset(link, pickerBody);
      if (offset) pickerBody.scrollBy({ top: offset, behavior: 'instant' });
    }
  }, [data, pageKey]);

  useLayoutEffect(() => {
    const previous = previousPage.current;
    previousPage.current = pageKey;
    if (groupKey === undefined || !navigation || handledNavigation.current === navigation) return;
    const { pushed, scrollTop } = navigation;
    // Back from Detail may remount the list and need a fresh response before it can scroll.
    if (scrollTop !== undefined && !hasData) return;
    handledNavigation.current = navigation;
    const jump = pendingJump.current;
    pendingJump.current = null;
    if (pushed || scrollTop !== undefined) window.scrollTo({ top: pushed ? 0 : scrollTop, behavior: 'instant' });
    if (pageKey === previous || picker || menu || confirmAll) return;
    if (pushed && jump?.key === pageKey) {
      const matching = pageWaiting.filter((agent) => agent.identity === jump.identity);
      const exactRow = matching.find((agent) => agent.row === jump.row);
      const candidate = exactRow ?? (matching.length === 1 ? matching[0] : null);
      if (candidate && revealWaiting(candidate)) return;
    }
    main.current.focus({ preventScroll: true });
  }, [pageKey, groupKey, navigation, hasData]);

  useEffect(() => {
    document.title = waiting > 0 ? `(${waiting}) ${env.config.title}` : env.config.title;
  }, [waiting]);

  async function sendKey(row, key) {
    const { target, binding: identity } = cards.get(row);
    if (!identity || bindings.get(identity) !== 1) return;
    setBusyKeys((b) => ({ ...b, [identity]: true }));
    const r = await env.api.post('/api/agent/keys', { agent: target, keys: [key] });
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

  function revealWaiting(agent) {
    const links = main.current.querySelectorAll(`${CARD_SELECTOR}.card-waiting > a.agent-open-area`);
    const link = links[pageWaiting.indexOf(agent)];
    if (!link) return false;
    focusAndReveal(link, header.current);
    return true;
  }

  function nextWaiting(fromRow) {
    const matches = fromRow ? (agent) => agent.row === fromRow
      : (agent) => agent.identity === previousJump.current;
    const previous = waitingAgents.findIndex(matches);
    const ambiguous = !fromRow && previous !== waitingAgents.findLastIndex(matches);
    const index = ambiguous ? 0 : (previous + 1) % waitingAgents.length;
    const agent = waitingAgents[index];
    if (!agent) return;
    previousJump.current = agent.identity;
    if (agent.key === pageKey) revealWaiting(agent);
    else {
      pendingJump.current = agent;
      env.pushHash(groupHash(agent.key));
    }
  }

  function renderGroup(group) {
    const { key, session, rows } = group;
    const occurrences = new Map();
    return html`<section key=${key} class="agent-section" aria-labelledby="agent-group-title">
      <h2 class="section-title" id="agent-group-title">
        <${GroupLabel} group=${group} marker=${markers.get(session)} />
      </h2>
      <ul class="agent-cards">
        ${rows.map((row) => {
          const { href, binding, scoped } = cards.get(row);
          const identity = bindings.get(binding) === 1 ? binding : null;
          const cardKey = JSON.stringify(identity
            ? [scoped || row.id, identity] : [row.id, scoped, href]);
          const occurrence = occurrences.get(cardKey) || 0;
          occurrences.set(cardKey, occurrence + 1);
          return html`<${AgentCard} key=${JSON.stringify([cardKey, occurrence])} row=${row} session=${session} href=${href}
            identity=${identity} env=${env} onKeys=${sendKey} busy=${!!busyKeys[identity]}
            onNext=${href && row.state === 'waiting' && waitingAgents.length > 1 ? () => nextWaiting(row) : null} />`;
        })}
      </ul>
    </section>`;
  }

  return html`<div class="page list-page">
    <header class="app-header" id="list-header" ref=${header}>
      <div class="header-row">
        <h1 class="header-title">Agents</h1>
        ${waitingAgents.length ? html`<button type="button" class="link-button need-count" id="list-next-waiting"
          title="Next waiting agent" aria-label=${`${waiting} need you. Next waiting agent`} onClick=${() => nextWaiting()}>${waiting} need you</button>`
          : waiting > 0 ? html`<span class="need-count">${waiting} need you</span>` : null}
        ${!launch ? html`<button type="button" class="icon-button" id="start-agent" aria-label="Start agent"
          title="Start agent" aria-haspopup="dialog" onClick=${() => setLauncher({ entry })}>+</button>` : null}
        ${groups.length > (selected ? 1 : 0) ? html`<button type="button" class="icon-button" id="switch-group"
          aria-label="Switch group" title="Switch group" aria-haspopup="dialog" onClick=${() => setPicker(true)}>
          <svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round">
            <rect x="3" y="8" width="14" height="12" rx="2"/><path d="M7 4h12a2 2 0 0 1 2 2v10"/>
          </svg>
        </button>` : null}
        <button type="button" class="icon-button" id="list-refresh" aria-label="Refresh" title="Refresh" onClick=${() => refresh()}>
          <svg aria-hidden="true" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 4v6h-6M20 10a8 8 0 1 0 0 5"/>
          </svg>
        </button>
        <button type="button" class="icon-button" id="menu-button" aria-label="Menu" aria-haspopup="dialog" onClick=${() => setMenu(true)}>⋯</button>
      </div>
    </header>
    ${env.notice ? html`<${Banner} id="banner-notice" tone="info" actions=${html`<button type="button" class="link-button" onClick=${env.clearNotice}>Dismiss</button>`}>${env.notice}<//>` : null}
    <main class=${cx('list-main', state && ['offline', 'retrying', 'error'].includes(state.status) && 'is-stale')} id="main" tabindex="-1" ref=${main}>
      ${state && state.error && ['offline', 'retrying', 'error'].includes(state.status) ? html`<${Banner} id="banner-offline" tone="warn">${freshnessText(state, now)}<//>` : null}
      ${!data && (!state || state.loading) ? html`<p class="empty">Loading agents…</p>` : null}
      ${data && !selected ? html`<p class="empty" id="list-empty">${groupKey === undefined ? 'No agents are running.' : 'No agents in this group.'}</p>` : null}
      ${launch ? html`<${LaunchWait} env=${env} launched=${launch} data=${data} bannerRef=${launchBanner}
        setWaiting=${setWaitingForLaunch} onDone=${() => setLauncher(null)} />` : null}
      ${selected ? renderGroup(selected) : null}
    </main>
    ${launcher && launcher.entry === entry && !launch ? html`<${StartSheet} env=${env} onClose=${() => setLauncher(null)}
      onLaunched=${(launch) => { setLauncher({ entry, launch }); refresh(); }} />` : null}
    ${picker ? html`<${Sheet} id="sheet-groups" title="Switch group" onClose=${() => setPicker(false)} fallbackFocusRef=${main}>
      <nav class="group-links" aria-label="Tab groups">
        ${groups.map((group) => html`<a key=${group.key} class="button group-link" href=${groupHash(group.key)}
          aria-current=${group.key === pageKey ? 'page' : null} onClick=${() => setPicker(false)}>
          <${GroupLabel} group=${group} marker=${markers.get(group.session)} />
        </a>`)}
      </nav>
    <//>` : null}
    ${menu ? html`<${Sheet} id="sheet-menu" title="Menu" onClose=${() => setMenu(false)}>
      <div class="menu-list">
        <button type="button" class="button menu-item" onClick=${() => { setMenu(false); setSiri(true); }}>Add to Siri</button>
        <${Notifications} push=${env.push} />
        <button type="button" class="button menu-item" id="logout" onClick=${() => { setMenu(false); env.logout(false); }}>Log out</button>
        <button type="button" class="button menu-item" id="logout-all" onClick=${() => { setMenu(false); setConfirmAll(true); }}>Log out everywhere</button>
      </div>
    <//>` : null}
    ${siri ? html`<${SiriSetup} api=${env.api} onClose=${() => setSiri(false)} />` : null}
    ${confirmAll ? html`<${ConfirmSheet} id="sheet-logout-all" title="Log out everywhere?" danger=${true}
      message="Every signed-in device, including this one, will need the password again."
      confirmLabel="Log out everywhere" confirmId="logout-all-confirm" onClose=${() => setConfirmAll(false)} onConfirm=${() => env.logout(true)} />` : null}
  </div>`;
}
