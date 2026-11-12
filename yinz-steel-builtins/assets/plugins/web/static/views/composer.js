// Bottom composer and submission retry UI, driven by submission.js.
import { describeError } from '../api.js';
import { recordView, targetOf, validTarget, UNKNOWN_TEXT } from '../submission.js';
import { html, useState, useEffect, useMemo, cx } from './ui.js';
import { Sheet } from './sheets.js';

function useSubmissions(env) {
  const [, setVersion] = useState(0);
  useEffect(() => env.submissions.subscribe(() => setVersion((v) => v + 1)), []);
}

function rowsFor(text) {
  const lines = (text.match(/\n/g) || []).length + 1;
  const wrapped = Math.ceil(text.length / 40);
  return Math.max(1, Math.min(8, Math.max(lines, wrapped)));
}

function preview(text) {
  const t = String(text || '').replace(/\s+/g, ' ').trim();
  return t.length > 140 ? `${t.slice(0, 140)}…` : t;
}

function PromptsSheet({ env, onPick, onClose }) {
  const [state, setState] = useState({ loading: true, prompts: [], error: null });
  const [filter, setFilter] = useState('');
  useEffect(() => {
    const controller = new AbortController();
    env.api.get('/api/prompts', { signal: controller.signal }).then((r) => {
      if (controller.signal.aborted) return;
      const prompts = r.ok && r.data && Array.isArray(r.data.prompts) ? r.data.prompts.filter((p) => p && typeof p.text === 'string') : [];
      setState({ loading: false, prompts, error: r.ok ? null : describeError(r) });
    });
    return () => controller.abort();
  }, []);
  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return q ? state.prompts.filter((p) => p.text.toLowerCase().includes(q) || String(p.description || '').toLowerCase().includes(q)) : state.prompts;
  }, [filter, state.prompts]);
  return html`<${Sheet} id="sheet-prompts" title="Past prompts" onClose=${onClose} wide=${true}>
    <label class="field">
      <span class="field-label">Filter</span>
      <input type="search" id="prompts-filter" class="input" value=${filter} autocomplete="off" data-autofocus
        onInput=${(e) => setFilter(e.currentTarget.value)} />
    </label>
    ${state.loading ? html`<p class="empty">Loading…</p>` : null}
    ${state.error ? html`<p class="error-text" role="alert">${state.error}</p>` : null}
    ${!state.loading && !state.error && !shown.length ? html`<p class="empty">No matching prompts.</p>` : null}
    <ul class="prompt-list">
      ${shown.slice(0, 200).map((p, i) => html`<li key=${i}>
        <button type="button" class="prompt-item" onClick=${() => onPick(p.text)}>
          <span class="prompt-text">${preview(p.text)}</span>
          ${p.description ? html`<span class="prompt-desc">${String(p.description)}</span>` : null}
        </button>
      </li>`)}
    </ul>
  <//>`;
}

/** Records written for an earlier identity of this pane: explicit adopt or discard. */
export function StaleSubmissions({ env, agentId, current, canPrompt, dock = false }) {
  useSubmissions(env);
  const records = env.submissions.staleRecords(agentId, current);
  if (!records.length) return null;
  return html`<div class=${cx('stale-submissions', dock && 'composer-dock')}>
    ${records.map((rec) => {
      const text = rec.pending ? rec.pending.text : rec.draft;
      const adopted = rec.adopted && current && rec.adopted.session_id === current.session_id && rec.adopted.kind === current.kind;
      return html`<div class="submission-panel submission-incompatible" role="alert" key=${`${rec.target.kind}|${rec.target.session_id}`}>
        <p class="submission-message">${rec.incompatible ? rec.incompatible.reason
          : rec.pending ? `${UNKNOWN_TEXT}. The agent is unavailable, so it cannot be retried now.`
            : 'Draft saved for this agent while it is unavailable.'}
          ${rec.incompatible && rec.pending && rec.pending.state === 'unknown' ? ` ${UNKNOWN_TEXT}.` : ''}</p>
        <p class="submission-text">${preview(text)}</p>
        <div class="submission-actions">
          ${current && canPrompt && !adopted ? html`<button type="button" class="button submission-adopt"
            onClick=${() => env.submissions.adopt(rec.target, current)}>Use as a new draft for the current conversation</button>` : null}
          ${adopted ? html`<span class="hint">Copied into the current draft.</span>` : null}
          <button type="button" class="button submission-discard-stale" onClick=${() => env.submissions.discard(rec.target)}>Discard</button>
        </div>
      </div>`;
    })}
  </div>`;
}

export function Composer({ env, live, enabled, onTargetChanged }) {
  useSubmissions(env);
  const [prompts, setPrompts] = useState(false);
  const [problem, setProblem] = useState(null);
  const target = live && live.ref && validTarget(live.ref) ? targetOf(live.ref) : null;
  const canPrompt = !!live && live.can_prompt === true && !!target;
  const reason = !live ? null
    : !enabled ? 'Actions are disabled while the agent is unavailable.'
      : !canPrompt ? (typeof live.prompt_unavailable === 'string' && live.prompt_unavailable) || 'Prompting is unavailable for this agent.' : null;
  const rec = target ? env.submissions.get(target) : null;
  const view = recordView(rec);
  const persistence = target ? env.submissions.persistenceProblem(target) : null;
  const sending = view.state === 'sending';
  const draft = rec ? rec.draft : '';

  async function send() {
    setProblem(null);
    const r = await env.submissions.send(target, { canPrompt: canPrompt && enabled, reason });
    if (!r.ok) setProblem(r.reason);
    else if (r.classification && r.classification.state === 'incompatible') onTargetChanged();
  }
  async function retry() {
    setProblem(null);
    const r = await env.submissions.retry(target);
    if (r.ok && r.classification && r.classification.state === 'incompatible') onTargetChanged();
  }
  function pick(text) {
    setPrompts(false);
    if (!target) return;
    env.submissions.setDraft(target, draft.trim() ? `${draft}\n${text}` : text);
  }
  const blocked = !target || !enabled || !canPrompt;
  const sendProblem = target ? env.submissions.sendProblem(target, { canPrompt: canPrompt && enabled, reason }) : 'No live agent.';

  return html`<div class="composer" id="composer">
    ${persistence ? html`<p class="submission-panel submission-persistence" id="submission-persistence" role="alert">
      This browser is not saving drafts (${persistence}). Your text is kept on this page only, and sending is blocked if the prompt safety record cannot be saved.</p>` : null}
    ${view.state === 'unknown' && !rec.incompatible ? html`<div class="submission-panel submission-unknown" id="submission-unknown" role="alert">
      <p class="submission-message">${view.message || UNKNOWN_TEXT}</p>
      <p class="submission-text">${preview(view.text)}</p>
      <div class="submission-actions">
        <button type="button" class="button button-primary" id="submission-retry" disabled=${!enabled} onClick=${retry}>Retry same submission</button>
        <button type="button" class="button" id="submission-discard" onClick=${() => env.submissions.discard(target)}>Discard</button>
      </div>
    </div>` : null}
    ${view.state === 'incompatible' && target ? html`<div class="submission-panel submission-incompatible" id="submission-incompatible" role="alert">
      <p class="submission-message">${view.message}</p>
      <div class="submission-actions">
        <button type="button" class="button" id="submission-reopen" disabled=${!enabled || !canPrompt} onClick=${() => env.submissions.reopen(target)}>Use the text as a new draft</button>
        <button type="button" class="button" id="submission-discard" onClick=${() => env.submissions.discard(target)}>Discard</button>
      </div>
    </div>` : null}
    ${view.state === 'failed' ? html`<div class="submission-panel submission-failed" id="submission-failed" role="alert">
      <p class="submission-message">${view.message}</p>
      <div class="submission-actions"><button type="button" class="link-button" onClick=${() => env.submissions.dismissFailure(target)}>Dismiss</button></div>
    </div>` : null}
    ${sending ? html`<p class="submission-panel submission-sending" id="submission-sending" role="status">Sending… waiting for the agent to confirm.</p>` : null}
    ${reason ? html`<p class="composer-reason" id="composer-reason">${reason}</p>` : null}
    ${problem && problem !== sendProblem ? html`<p class="composer-reason error-text" role="alert">${problem}</p>` : null}
    <div class="composer-row">
      <button type="button" class="icon-button" id="composer-prompts" aria-label="Past prompts" aria-haspopup="dialog"
        disabled=${blocked} onClick=${() => setPrompts(true)}>☰</button>
      <label class="visually-hidden" for="composer-input">Prompt</label>
      <textarea id="composer-input" class="composer-input" rows=${rowsFor(draft)} value=${draft}
        placeholder=${blocked ? 'Prompting unavailable' : 'Message the agent'} disabled=${blocked}
        autocomplete="off" autocapitalize="sentences" spellcheck="true"
        onInput=${(e) => target && env.submissions.setDraft(target, e.currentTarget.value)}></textarea>
      <button type="button" class=${cx('button', 'button-primary', 'composer-send')} id="composer-send"
        disabled=${blocked || !!sendProblem} title=${sendProblem || 'Send'} onClick=${send}>Send</button>
    </div>
    ${prompts ? html`<${PromptsSheet} env=${env} onPick=${pick} onClose=${() => setPrompts(false)} />` : null}
  </div>`;
}
