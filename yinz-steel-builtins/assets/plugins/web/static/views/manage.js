// Management sheets keep the submitted agent reference and destination incarnation pinned.
import { html, useEffect, useRef, useState } from './ui.js';
import { describeError } from '../api.js';
import { Sheet } from './sheets.js';

function validDestinations(data) {
  const id = (value) => Number.isSafeInteger(value) && value >= 0;
  return id(data?.current_group) && id(data.current_tab) && typeof data.allow_new_group === 'boolean'
    && Array.isArray(data.groups) && data.groups.every((group) =>
      id(group?.id) && typeof group.name === 'string' && Array.isArray(group.tabs)
      && group.tabs.every((tab) => id(tab?.id) && typeof tab.name === 'string'
        && typeof tab.incarnation === 'string' && tab.incarnation.length > 0));
}

function validLaunchChoices(data) {
  const strings = (values) => Array.isArray(values) && values.every((value) => typeof value === 'string');
  return typeof data?.server_instance === 'string' && data.server_instance.trim().length > 0
    && strings(data.providers) && strings(data.projects) && Array.isArray(data.worktrees)
    && data.worktrees.every((tree) => typeof tree?.path === 'string' && tree.path.length > 0
      && (tree.branch === null || typeof tree.branch === 'string'));
}

function useChoices(key, load, validate) {
  const [result, setResult] = useState(null);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    load(controller.signal).then((reply) => {
      if (controller.signal.aborted) return;
      setResult({ key, revision, reply: reply.ok && !validate(reply.data) ? { ok: false, kind: 'malformed' } : reply });
    });
    return () => controller.abort();
  }, [key, revision]);
  const reply = result?.key === key && result.revision === revision ? result.reply : null;
  return [reply, () => setRevision((n) => n + 1)];
}

function LoadStatus({ reply, retry }) {
  if (!reply) return html`<p role="status">Loading choices…</p>`;
  if (reply.ok) return null;
  return html`<p role="alert">${describeError(reply)} <button type="button" class="link-button" onClick=${retry}>Retry</button></p>`;
}

function ActionForm({ id, title, onClose, submit, label, disabled, danger, children }) {
  const lock = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  async function onSubmit(event) {
    event.preventDefault();
    if (disabled || lock.current) return;
    lock.current = true;
    setBusy(true);
    setError('');
    try {
      const reply = await submit();
      if (!reply.ok) setError(describeError(reply));
    } finally {
      lock.current = false;
      setBusy(false);
    }
  }
  const dismiss = () => { if (!lock.current) onClose(); };
  return html`<${Sheet} id=${id} title=${title} onClose=${dismiss}>
    <form class="agent-form" onSubmit=${onSubmit}>
      <fieldset disabled=${busy}>${children}</fieldset>
      ${error ? html`<p role="alert">${error}</p>` : null}
      <div class="form-actions"><button type="button" class="button" onClick=${dismiss} disabled=${busy}>Cancel</button>
        <button type="submit" class=${danger ? 'button button-danger' : 'button button-primary'} disabled=${disabled || busy}>${label}</button></div>
    </form>
  <//>`;
}

const pendingLaunchKey = 'yinz.web.v1.pending-launch';

function savedLaunch(storage) {
  try {
    const body = JSON.parse(storage.get(pendingLaunchKey).value);
    if (typeof body?.submission_id === 'string' && typeof body.kind === 'string'
        && typeof body.project === 'string' && (body.checkout?.type === 'new'
          ? typeof body.checkout.branch === 'string' : typeof body.checkout?.path === 'string')) return body;
  } catch { /* No readable launch in this tab. */ }
  return null;
}

export function StartSheet({ env, onClose, onLaunched }) {
  const [pending, setPending] = useState(() => savedLaunch(env.session));
  const [provider, setProvider] = useState('');
  const [project, setProject] = useState('');
  const [checkout, setCheckout] = useState('');
  const [branch, setBranch] = useState('');
  const [choices, retry] = useChoices(project, (signal) => env.api.get(
    '/api/agent/launch-choices' + (project ? `?project=${encodeURIComponent(project)}` : ''), { signal }), validLaunchChoices);
  const initial = useRef(null);
  if (choices?.ok) initial.current = choices.data;
  const projects = initial.current?.projects || [];
  const providers = initial.current?.providers || [];
  const trees = choices?.data?.worktrees || [];
  const create = checkout === 'new';
  const ready = choices?.ok && providers.includes(provider) && projects.includes(project)
    && (create ? !!branch.trim() : trees.some((w) => w.path === checkout));
  async function launch() {
    let body = pending;
    if (!body) {
      const nonce = Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) => byte.toString(16).padStart(2, '0')).join('');
      body = { submission_id: choices.data.server_instance + '.' + nonce, kind: provider, project,
        checkout: create ? { type: 'new', branch: branch.trim() } : { type: 'existing', path: checkout } };
      if (!env.session.set(pendingLaunchKey, JSON.stringify(body)).ok) {
        return { message: 'Allow session storage so this launch can be recovered.' };
      }
      setPending(body);
    }
    const r = await env.api.post('/api/agent/launch', body);
    if (r.ok && (typeof r.data?.pane_id !== 'string' || typeof r.data.session !== 'string' || r.data.kind !== body.kind)) return { kind: 'malformed' };
    if (r.ok || ['invalid', 'launch_failed', 'launch_conflict'].includes(r.code)) {
      env.session.remove(pendingLaunchKey);
      setPending(null);
    }
    if (r.ok) onLaunched(r.data);
    return r;
  }
  if (pending) return html`<${ActionForm} id="sheet-start" title="Start agent" label="Check launch" submit=${launch} onClose=${onClose}>
    <p>${pending.kind} in ${pending.project} · ${pending.checkout.branch ?? pending.checkout.path}</p>
  <//>`;
  return html`<${ActionForm} id="sheet-start" title="Start agent" label="Start agent" disabled=${!ready} submit=${launch} onClose=${onClose}>
    <label>Provider<select required value=${provider} onChange=${(e) => setProvider(e.target.value)}>
      <option value="">Choose a provider</option>${providers.map((p) => html`<option key=${p} value=${p}>${p}</option>`)}
    </select></label>
    <label>Project<select required value=${project} onChange=${(e) => { setProject(e.target.value); setCheckout(''); }}>
      <option value="">Choose a project</option>${projects.map((p) => html`<option key=${p} value=${p}>${p}</option>`)}
    </select></label>
    ${choices?.ok && !projects.length ? html`<p>Register a project in Yinz to start an agent.</p>` : null}
    ${project && choices?.ok ? html`<label>Worktree<select required value=${checkout} onChange=${(e) => setCheckout(e.target.value)}>
      <option value="">Choose a worktree</option>
      ${trees.map((w) => html`<option key=${w.path} value=${w.path}>${w.branch ?? 'Detached HEAD'} · ${w.path}</option>`)}
      <option value="new">Create new worktree</option></select></label>` : null}
    ${create ? html`<label>Branch name<input required value=${branch} onInput=${(e) => setBranch(e.target.value)} /></label>` : null}
    <${LoadStatus} reply=${choices} retry=${retry} />
  <//>`;
}

function named(item) {
  return `${item.name} (#${item.id})`;
}

export function agentLocation(live) {
  return [live.session, live.location?.group && named(live.location.group), live.location?.tab && named(live.location.tab), live.pane_id].filter(Boolean).join(' · ');
}

export function MoveSheet({ env, agent, live, onClose, onMoved }) {
  const [inventory, retry] = useChoices(agent.id, (signal) =>
    env.api.post('/api/agent/destinations', { agent }, { signal }), validDestinations);
  const [groupId, setGroup] = useState('');
  const [tabKey, setTab] = useState('');
  const [groupName, setGroupName] = useState('');
  const [tabName, setTabName] = useState('');
  const data = inventory?.data;
  const group = data?.groups.find((g) => String(g.id) === groupId);
  const tab = group?.tabs.find((t) => t.incarnation === tabKey);
  const newGroup = groupId === 'new' && data?.allow_new_group;
  const newTab = tabKey === 'new' && !!group;
  const ready = !!data && (newGroup || newTab || (tab && tab.id !== data.current_tab));
  async function move() {
    const destination = newGroup ? { type: 'new_group', name: groupName.trim(), tab_name: tabName.trim() }
      : newTab ? { type: 'new_tab', group: group.id, name: tabName.trim() }
      : { type: 'tab', group: group.id, tab: tab.id, incarnation: tab.incarnation };
    const r = await env.api.post('/api/agent/move', { agent, destination });
    if (r.ok && r.data?.ok !== true) return { kind: 'malformed' };
    if (r.ok) onMoved();
    return r;
  }
  return html`<${ActionForm} id="sheet-move" title="Move agent" label="Move agent" onClose=${onClose} disabled=${!ready} submit=${move}>
    <p>Current location · ${agentLocation(live)}</p>
    <${LoadStatus} reply=${inventory} retry=${retry} />
    ${data ? html`<label><span id="move-group-label">Group</span><select aria-labelledby="move-group-label" required value=${groupId} onChange=${(e) => { setGroup(e.target.value); setTab(''); }}>
      <option value="">Choose a group</option>${data.groups.map((g) => html`<option key=${g.id} value=${String(g.id)}>${named(g)}</option>`)}
      ${data.allow_new_group ? html`<option value="new">New group</option>` : null}
    </select></label>` : null}
    ${group ? html`<label><span id="move-tab-label">Tab</span><select aria-labelledby="move-tab-label" required value=${tabKey} onChange=${(e) => setTab(e.target.value)}>
      <option value="">Choose a tab</option>${group.tabs.map((t) => html`<option key=${t.incarnation} value=${t.incarnation}
        disabled=${t.id === data.current_tab}>${named(t)}${t.id === data.current_tab ? ' · current' : ''}</option>`)}
      <option value="new">New tab</option></select></label>` : null}
    ${newGroup ? html`<label>Group name (optional)<input value=${groupName} onInput=${(e) => setGroupName(e.target.value)} /></label>` : null}
    ${newGroup || newTab ? html`<label>Tab name (optional)<input value=${tabName} onInput=${(e) => setTabName(e.target.value)} /></label>` : null}
    ${data ? html`<button type="button" class="link-button" onClick=${retry}>Refresh destinations</button>` : null}
  <//>`;
}

export function CloseSheet({ env, agent, live, onClose, onClosed }) {
  async function close() {
    const r = await env.api.post('/api/agent/close', { agent });
    if (r.ok && r.data?.ok !== true) return { kind: 'malformed' };
    if (r.ok) onClosed();
    return r;
  }
  return html`<${ActionForm} id="sheet-close-agent" title="Close agent?" label="Close agent" danger onClose=${onClose} submit=${close}>
    <p>Close ${live.title || agent.kind} in ${agentLocation(live)}? Its archived conversation will remain available.</p>
  <//>`;
}
