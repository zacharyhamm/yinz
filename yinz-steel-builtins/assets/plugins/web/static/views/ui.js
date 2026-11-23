// Shared view primitives. This module owns the ONLY raw-HTML property use in the
// app: ServerHtml renders server-produced, escaped `*html` fields. Every other
// value (titles, errors, agent text, prompts) is rendered as text.
import { h, render } from '../vendor/preact.module.js';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from '../vendor/hooks.module.js';
import htm from '../vendor/htm.module.js';
import { createPoller, freshnessText } from '../api.js';

export const html = htm.bind(h);
export { h, render, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState };

export function ServerHtml({ html: markup, class: cls = '', tag = 'div', id }) {
  return h(tag, { class: `server-html ${cls}`.trim(), id, dangerouslySetInnerHTML: { __html: typeof markup === 'string' ? markup : '' } });
}

export function cx(...parts) {
  return parts.filter(Boolean).join(' ');
}

/** Re-render periodically for ages and countdowns. */
export function useNow(intervalMs = 1000) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  // Renders caused by new data must not show a countdown or age up to one tick stale.
  return Math.max(now, Date.now());
}

/**
 * One poller per resource identity (`key`). Changing the key or unmounting stops
 * the old poller (aborting its request and fencing late completions).
 */
export function usePoller({ key, run, intervalMs, enabled = true, env }) {
  const [state, setState] = useState(null);
  const runRef = useRef(run);
  runRef.current = run;
  const pollerRef = useRef(null);
  useEffect(() => {
    if (!enabled) {
      setState(null);
      return undefined;
    }
    const poller = createPoller({
      run: (ctx) => runRef.current(ctx),
      intervalMs,
      isHidden: env.visibility.isHidden,
      onUpdate: setState,
    });
    pollerRef.current = poller;
    const unsubscribe = env.visibility.subscribe(() => poller.visibilityChanged());
    poller.start();
    return () => {
      unsubscribe();
      poller.stop();
      if (pollerRef.current === poller) pollerRef.current = null;
    };
  }, [key, enabled, intervalMs]);
  const refresh = useCallback(() => (pollerRef.current ? pollerRef.current.refresh() : Promise.resolve(null)), []);
  return [state, refresh];
}

export function Banner({ id, tone = 'info', children, actions, focusRef }) {
  return html`<div id=${id} class=${cx('banner', `banner-${tone}`)} role=${tone === 'error' || tone === 'warn' ? 'alert' : 'status'}
    ref=${focusRef} tabindex=${focusRef ? -1 : undefined}>
    <div class="banner-text">${children}</div>
    ${actions ? html`<div class="banner-actions">${actions}</div>` : null}
  </div>`;
}

export function Freshness({ state, now, id }) {
  if (!state) return null;
  const stale = state.status === 'offline' || state.status === 'retrying' || state.status === 'error';
  return html`<span id=${id} class=${cx('freshness', stale && 'freshness-stale')}>${freshnessText(state, now)}</span>`;
}

const STATE_LABELS = { waiting: 'Needs you', busy: 'Working', idle: 'Idle' };
const STATE_GLYPHS = { waiting: '!', busy: '●', idle: '○' };

export function stateLabel(state) {
  if (typeof state !== 'string' || !state.trim()) return 'Unknown';
  return Object.hasOwn(STATE_LABELS, state) ? STATE_LABELS[state] : state;
}

export function StateBadge({ row }) {
  const state = row && typeof row.state === 'string' ? row.state : 'unknown';
  const known = Object.hasOwn(STATE_LABELS, state);
  return html`<span class=${cx('badge', 'state-badge', `state-${known ? state : 'unknown'}`, row && row.wait_needs_a_person && 'needs-person')}>
    <span class="state-glyph" aria-hidden="true">${known ? STATE_GLYPHS[state] : '?'}</span>
    <span class="state-text">${stateLabel(state)}</span>
  </span>`;
}

/** Labelled indicators; colour is never the only signal. Hook-supplied
 * `row.badges` are rendered server-side by the `agent-row` hook into row.html. */
export function RowBadges({ row, showState = true, peerSession = row?.session }) {
  if (!row || (!showState && !row.focused && row.current_session !== false && !row.host)) return null;
  return html`<span class="badges">
    ${showState ? html`<${StateBadge} row=${row} />` : null}
    ${row.focused ? html`<span class="badge badge-focused" title="Focused on the desktop"><span aria-hidden="true">⌖</span> focused</span>` : null}
    ${row.current_session === false ? html`<span class="badge badge-peer">session ${String(peerSession || '')}</span>` : null}
    ${row.host ? html`<span class="badge badge-host">@${String(row.host)}</span>` : null}
  </span>`;
}

function usageTone(display) {
  if (display.stale || !Number.isFinite(display.remaining)) return 'muted';
  if (display.remaining > 25) return 'ok';
  return display.remaining >= 10 ? 'low' : 'danger';
}

function text(value) {
  return typeof value === 'string' ? value : '';
}

/** The command palette's usage column (`agent_header`), from the server's
 * quota display: remaining weekly share, the 5-hour window, other windows, reset.
 * `stale` marks a retained reply whose refresh failed: the server computed its
 * labels when it answered, so they would otherwise stay fresh indefinitely. */
export function UsageColumn({ display, error, stale = false }) {
  const rows = [];
  let value = null;
  if (display && typeof display === 'object') {
    const other = Array.isArray(display.other) ? display.other.map(text).filter(Boolean) : [];
    const known = Number.isFinite(display.value);
    const quota = known || !!text(display.short) || other.length > 0;
    if (stale && !display.stale) {
      // As quota::Display marks an aged observation: label, muted tone and summary.
      display = { ...display, stale: true, weekly_label: 'Weekly · stale',
        compact: quota ? [text(display.compact), 'stale'].filter(Boolean).join(' · ') : display.compact };
    }
    if (!quota) {
      rows.push('Usage unavailable', text(display.compact) || '—', '5hr · —');
    } else {
      rows.push(text(display.weekly_label) || 'Weekly');
      value = known ? html`<span class="usage-value" data-tone=${usageTone(display)}>${display.value}%</span>` : '—';
      rows.push(value);
      rows.push(text(display.short) || '5hr · —', ...other);
    }
    rows.push(text(display.reset) || 'Reset —');
  } else {
    rows.push('Usage unavailable', error || '—', '5hr · —', 'Reset —');
  }
  const compact = display && typeof display === 'object'
    ? [text(display.compact), text(display.reset)].filter(Boolean) : [error].filter(Boolean);
  return html`<div class="usage" id="usage">
    <ul class="usage-full" aria-label="Usage">${rows.map((row) => html`<li>${row}</li>`)}</ul>
    ${compact.length ? html`<ul class="usage-compact" aria-label="Usage">${compact.map((row) => html`<li>${row}</li>`)}</ul>` : null}
  </div>`;
}

export const KEY_LABELS = { esc: 'Esc', enter: 'Enter', tab: 'Tab', up: '↑', down: '↓', left: '←', right: '→', backspace: '⌫', y: 'y', n: 'n' };
export function keyLabel(k) {
  return KEY_LABELS[k] || k;
}
const KEY_NAMES = { esc: 'Escape', enter: 'Enter', tab: 'Tab', up: 'Up arrow', down: 'Down arrow', left: 'Left arrow', right: 'Right arrow', backspace: 'Backspace' };
export function keyName(k) {
  return KEY_NAMES[k] || `Key ${k}`;
}

export function QuickKeys({ keys, disabled, onKey, label = 'Quick keys' }) {
  return html`<div class="quick-keys" role="group" aria-label=${label}>
    ${keys.map((k) => html`<button type="button" class="quick-key" data-key=${k} disabled=${disabled}
      aria-label=${keyName(k)} onClick=${() => onKey(k)}>${keyLabel(k)}</button>`)}
  </div>`;
}

/** Human summary of a keys/screen AgentResponse. */
export function responseText(response) {
  if (!response || typeof response !== 'object') return 'No reply.';
  const message = typeof response.message === 'string' && response.message.trim() ? response.message.trim() : '';
  const outcome = typeof response.outcome === 'string' ? response.outcome.replace(/_/g, ' ') : 'unknown';
  return message ? `${outcome}: ${message}` : outcome;
}
