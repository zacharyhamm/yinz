// Prompt draft/submission state machine shared by the composer and retry UI.
//
// Safety rules (docs/PLUGIN_PROTOCOL.md "Native agent views"):
// - The editable draft, the complete target {id,kind,session_id}, the frozen
//   attempted text and its generated submission id are persisted to
//   sessionStorage and read back BEFORE any prompt fetch. If that fails, Send is
//   blocked and the draft stays in memory for copying.
// - A pending submission is immutable; edits change only the separate draft.
// - Only native outcome "submitted" for the matching target and id clears the
//   attempt (and the draft, if unedited since). HTTP 2xx, `acknowledged` and
//   `prompt_delivery: "started"` are never success.
// - Uncertain outcomes keep the exact identity, text and id with an explicit
//   "Retry same submission" (same id) or "Discard". Nothing is ever resent
//   automatically, including after login, reload or re-entry.
// - Target changes never redirect an attempt: the original record is disabled
//   and only an explicit user decision starts a draft for the replacement.

import { KEYS, describeError, utf8Length } from './api.js';

export const UNKNOWN_TEXT = 'Outcome unknown — check the timeline';
export const MAX_PROMPT_BYTES = 32768;
const RECORD_VERSION = 1;
const TARGET_OUTCOMES = new Set(['target_changed', 'target_closed', 'target_not_found', 'not_agent', 'target_exited']);
// Rejected by native policy before dispatch: definitely not delivered.
const DEFINITE_HTTP = new Set([400, 403, 404, 408, 413, 415, 429]);

function identity(v) {
  return typeof v === 'string' && v.trim() !== '' && !/[\u0000-\u001f\u007f-\u009f]/.test(v) && utf8Length(v) <= 512;
}

export function validTarget(t) {
  return !!t && identity(t.id) && identity(t.kind) && identity(t.session_id);
}

export function sameTarget(a, b) {
  return !!a && !!b && a.id === b.id && a.kind === b.kind && a.session_id === b.session_id;
}

export function targetOf(t) {
  return { id: t.id, kind: t.kind, session_id: t.session_id };
}

export function targetKey(t) {
  return KEYS.submissionPrefix + [t.id, t.kind, t.session_id].map(encodeURIComponent).join('|');
}

function agentPrefix(id) {
  return `${KEYS.submissionPrefix}${encodeURIComponent(id)}|`;
}

/** Native prompt text rules: nonblank, <= 32 KiB UTF-8, no controls except \n and \t. */
export function promptProblem(text) {
  if (typeof text !== 'string' || text.trim() === '') return 'Write a prompt first.';
  if (utf8Length(text) > MAX_PROMPT_BYTES) return 'The prompt is longer than 32 KiB.';
  if (/[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/.test(text)) return 'The prompt contains control characters.';
  return null;
}

function emptyRecord(target) {
  return { v: RECORD_VERSION, target: targetOf(target), draft: '', pending: null, lastFailure: null, incompatible: null, adopted: null };
}

function normalize(raw, target) {
  if (!raw || typeof raw !== 'object' || raw.v !== RECORD_VERSION || !sameTarget(raw.target, target)) return null;
  const rec = emptyRecord(target);
  rec.draft = typeof raw.draft === 'string' ? raw.draft : '';
  const p = raw.pending;
  if (p && typeof p === 'object' && typeof p.submission_id === 'string' && typeof p.text === 'string') {
    rec.pending = {
      submission_id: p.submission_id,
      text: p.text,
      // Rehydrated attempts are never resent: a reload while sending cannot know what happened.
      state: 'unknown',
      attempts: Number.isInteger(p.attempts) ? p.attempts : 1,
      created_ms: Number.isFinite(p.created_ms) ? p.created_ms : null,
      message: typeof p.message === 'string' ? p.message : null,
    };
    if (p.state === 'sending') rec.pending.message = 'The page reloaded before the outcome arrived.';
  }
  if (raw.lastFailure && typeof raw.lastFailure === 'object') {
    rec.lastFailure = { message: String(raw.lastFailure.message || ''), submission_id: raw.lastFailure.submission_id ?? null };
  }
  if (raw.incompatible && typeof raw.incompatible === 'object') {
    rec.incompatible = {
      reason: String(raw.incompatible.reason || ''),
      replacement: validTarget(raw.incompatible.replacement) ? targetOf(raw.incompatible.replacement) : null,
    };
  }
  if (validTarget(raw.adopted)) rec.adopted = targetOf(raw.adopted);
  return rec;
}

/** Map a prompt request result to a state transition for submission `id`. */
export function classifyPromptResult(result, id) {
  if (!result || !result.ok) {
    if (result && result.kind === 'http') {
      if (result.status === 409) {
        return { state: 'incompatible', message: 'The agent changed or disappeared. This prompt was not sent to any replacement.' };
      }
      if (DEFINITE_HTTP.has(result.status)) return { state: 'failed', message: `Not sent: ${describeError(result)}` };
      if (result.status === 401) return { state: 'unknown', message: `${UNKNOWN_TEXT}. Your sign-in ended during sending.` };
    }
    return { state: 'unknown', message: `${UNKNOWN_TEXT}. ${describeError(result)}` };
  }
  const response = result.data && typeof result.data === 'object' ? result.data.response : null;
  if (!response || typeof response !== 'object' || typeof response.outcome !== 'string') {
    return { state: 'unknown', message: `${UNKNOWN_TEXT}. The reply could not be read.` };
  }
  const native = typeof response.message === 'string' && response.message.trim() ? response.message.trim().slice(0, 300) : null;
  if (response.submission_id !== id) {
    return { state: 'unknown', message: `${UNKNOWN_TEXT}. The reply did not match this submission.` };
  }
  const { outcome } = response;
  if (outcome === 'submitted') return { state: 'submitted', message: null, outcome };
  if (outcome === 'rejected') return { state: 'failed', message: native || 'The agent rejected the prompt.', outcome };
  const notSent = response.prompt_delivery === 'not_sent';
  const started = response.prompt_delivery === 'started';
  if (TARGET_OUTCOMES.has(outcome) && !started) {
    return { state: 'incompatible', message: native || 'The agent changed or closed. This prompt was not sent to any replacement.', outcome };
  }
  if (notSent) return { state: 'failed', message: `Not sent: ${native || outcome.replace(/_/g, ' ')}`, outcome };
  return { state: 'unknown', message: `${UNKNOWN_TEXT}.${native ? ` ${native}` : ''}`, outcome };
}

/** UI view of a record. */
export function recordView(rec) {
  if (!rec) return { state: 'idle' };
  if (rec.incompatible) return { state: 'incompatible', message: rec.incompatible.reason, replacement: rec.incompatible.replacement };
  if (rec.pending) return { state: rec.pending.state, message: rec.pending.message, submission_id: rec.pending.submission_id, text: rec.pending.text };
  if (rec.lastFailure) return { state: 'failed', message: rec.lastFailure.message };
  return { state: 'idle' };
}

export function createSubmissions({ storage, api, uuid = () => globalThis.crypto.randomUUID(), now = () => Date.now() }) {
  const memory = new Map();
  const listeners = new Set();
  const persistErrors = new Map();
  const emit = (target) => {
    for (const l of listeners) l(target);
  };

  function write(rec) {
    const key = targetKey(rec.target);
    memory.set(key, rec);
    const empty = !rec.draft && !rec.pending && !rec.incompatible && !rec.lastFailure;
    const result = empty ? storage.remove(key) : storage.set(key, JSON.stringify(rec));
    if (result.ok) persistErrors.delete(key);
    else persistErrors.set(key, result.error || 'denied');
    return result.ok;
  }

  function load(target) {
    const key = targetKey(target);
    if (memory.has(key)) return memory.get(key);
    const stored = storage.get(key);
    let rec = null;
    if (stored.ok && stored.value != null) {
      try {
        rec = normalize(JSON.parse(stored.value), target);
      } catch {
        rec = null;
      }
      if (!rec) storage.remove(key); // corrupt or foreign record
    }
    rec = rec || emptyRecord(target);
    memory.set(key, rec);
    if (stored.ok && stored.value != null && rec.pending) write(rec); // persist sending -> unknown
    if (!stored.ok) persistErrors.set(key, stored.error || 'denied');
    return rec;
  }

  function update(target, fn) {
    const next = fn({ ...load(target) });
    write(next);
    emit(target);
    return next;
  }

  async function dispatch(target, attempt) {
    const { submission_id: id, text } = attempt;
    const result = await api.post('/api/agent/prompt', { agent: targetOf(target), submission_id: id, text });
    const current = load(target);
    // Discarded, retried or replaced meanwhile: a late reply cannot touch another attempt.
    if (!current.pending || current.pending.submission_id !== id) return { record: current, result, stale: true };
    const c = classifyPromptResult(result, id);
    const record = update(target, (rec) => {
      if (c.state === 'submitted') {
        rec.pending = null;
        rec.lastFailure = null;
        if (rec.draft === text) rec.draft = '';
        if (!rec.draft.trim()) {
          rec.incompatible = null;
          rec.adopted = null;
        }
        return rec;
      }
      if (c.state === 'failed') {
        rec.pending = null;
        rec.lastFailure = { message: c.message, submission_id: id };
        return rec;
      }
      rec.pending = { ...rec.pending, state: 'unknown', message: c.message };
      if (c.state === 'incompatible') rec.incompatible = { reason: c.message, replacement: rec.incompatible?.replacement ?? null };
      return rec;
    });
    return { record, result, classification: c };
  }

  return {
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    get(target) {
      return load(target);
    },
    view(target) {
      return recordView(load(target));
    },
    persistenceProblem(target) {
      return persistErrors.get(targetKey(target)) || null;
    },
    setDraft(target, text) {
      const rec = update(target, (r) => ({ ...r, draft: String(text) }));
      return { ok: !persistErrors.has(targetKey(target)), record: rec };
    },
    /** Why Send is unavailable, or null. */
    sendProblem(target, { canPrompt = true, reason = null } = {}) {
      if (!validTarget(target)) return 'This agent has no complete identity.';
      if (!canPrompt) return reason || 'Prompting is unavailable for this agent.';
      const rec = load(target);
      if (rec.incompatible) return 'This draft belongs to a previous agent identity.';
      if (rec.pending) return rec.pending.state === 'sending' ? 'Sending…' : 'Resolve the previous submission first.';
      return promptProblem(rec.draft);
    },
    async send(target, opts = {}) {
      const problem = this.sendProblem(target, opts);
      if (problem) return { ok: false, reason: problem };
      const before = load(target);
      const attempt = { submission_id: uuid(), text: before.draft, state: 'sending', attempts: 1, created_ms: now(), message: null };
      const rec = { ...before, pending: attempt, lastFailure: null };
      if (!write(rec)) {
        // Never send without a durable safety record; keep the draft visible.
        memory.set(targetKey(target), before);
        emit(target);
        return { ok: false, reason: 'This browser could not save the prompt safety record, so sending is blocked. Copy your draft.', persistence: true };
      }
      emit(target);
      const out = await dispatch(target, attempt);
      return { ok: true, ...out };
    },
    /** Explicit retry of an uncertain attempt: same id, same frozen text, same target. */
    async retry(target) {
      const rec = load(target);
      if (!rec.pending || rec.pending.state !== 'unknown' || rec.incompatible) return { ok: false, reason: 'Nothing to retry.' };
      const attempt = { ...rec.pending, state: 'sending', attempts: rec.pending.attempts + 1, message: null };
      write({ ...rec, pending: attempt }); // the id is already durable; a failed state write is harmless
      emit(target);
      const out = await dispatch(target, attempt);
      return { ok: true, ...out };
    },
    /**
     * Drop the attempt. The editable draft stays for the current identity; a record
     * for a replaced identity cannot be sent anywhere, so it is removed entirely.
     */
    discard(target) {
      return update(target, (r) => ({ ...r, draft: r.incompatible ? '' : r.draft, pending: null, lastFailure: null, incompatible: null, adopted: null }));
    },
    /**
     * Explicit user decision after a retarget left this identity's record disabled:
     * keep the text as a fresh draft (a later Send generates a new id).
     */
    reopen(target) {
      return update(target, (r) => ({
        ...r,
        draft: r.draft.trim() ? r.draft : r.pending ? r.pending.text : r.draft,
        pending: null,
        incompatible: null,
        adopted: null,
      }));
    },
    dismissFailure(target) {
      return update(target, (r) => ({ ...r, lastFailure: null }));
    },
    /** Disable a record whose target was replaced (retarget, 409, same-pane replacement). */
    markIncompatible(target, { replacement = null, reason }) {
      const rec = load(target);
      if (!rec.pending && !rec.draft.trim()) return rec;
      if (rec.incompatible) return rec;
      return update(target, (r) => ({
        ...r,
        incompatible: { reason: reason || 'The agent identity changed.', replacement: replacement ? targetOf(replacement) : null },
      }));
    },
    /** Records for this pane id whose identity differs from `current`. */
    staleRecords(agentId, current) {
      const prefix = agentPrefix(agentId);
      const keys = new Set([...storage.keys(), ...memory.keys()].filter((k) => k.startsWith(prefix)));
      const out = [];
      for (const key of keys) {
        const parts = key.slice(KEYS.submissionPrefix.length).split('|');
        if (parts.length !== 3) continue;
        let target;
        try {
          target = { id: decodeURIComponent(parts[0]), kind: decodeURIComponent(parts[1]), session_id: decodeURIComponent(parts[2]) };
        } catch {
          continue;
        }
        if (!validTarget(target) || (current && sameTarget(target, current))) continue;
        const rec = load(target);
        if (rec.pending || rec.draft.trim() || rec.incompatible) out.push(rec);
      }
      return out;
    },
    /**
     * Explicit user decision: start a draft for the replacement from the old text.
     * The original record is kept (marked adopted) until the user discards it.
     */
    adopt(oldTarget, newTarget) {
      const old = load(oldTarget);
      const text = old.pending ? old.pending.text : old.draft;
      const fresh = load(newTarget);
      if (!fresh.draft.trim()) this.setDraft(newTarget, text);
      update(oldTarget, (r) => ({ ...r, adopted: targetOf(newTarget) }));
      return load(newTarget);
    },
  };
}
