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

export function Banner({ id, tone = 'info', children, actions }) {
  return html`<div id=${id} class=${cx('banner', `banner-${tone}`)} role=${tone === 'error' || tone === 'warn' ? 'alert' : 'status'}>
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
  return STATE_LABELS[state] || (typeof state === 'string' && state ? state : 'Unknown');
}

export function StateBadge({ row }) {
  const state = row && typeof row.state === 'string' ? row.state : 'unknown';
  return html`<span class=${cx('badge', 'state-badge', `state-${STATE_LABELS[state] ? state : 'unknown'}`, row && row.wait_needs_a_person && 'needs-person')}>
    <span class="state-glyph" aria-hidden="true">${STATE_GLYPHS[state] || '?'}</span>
    <span class="state-text">${stateLabel(state)}</span>
  </span>`;
}

/** Labelled indicators; colour is never the only signal. Hook-supplied
 * `row.badges` are rendered server-side by the `agent-row` hook into row.html. */
export function RowBadges({ row, showState = true }) {
  if (!row || (!showState && !row.focused && row.current_session !== false && !row.host)) return null;
  return html`<span class="badges">
    ${showState ? html`<${StateBadge} row=${row} />` : null}
    ${row.focused ? html`<span class="badge badge-focused" title="Focused on the desktop"><span aria-hidden="true">⌖</span> focused</span>` : null}
    ${row.current_session === false ? html`<span class="badge badge-peer">session ${String(row.session || '')}</span>` : null}
    ${row.host ? html`<span class="badge badge-host">@${String(row.host)}</span>` : null}
  </span>`;
}

function quotaClass(percent) {
  const p = Math.max(0, Math.min(100, Number(percent) || 0));
  return `q-${Math.round(p / 10)}`;
}

const USAGE_STATUS = {
  unavailable: 'Usage unavailable',
  unsupported: 'Usage not supported for this provider',
  expired: 'Usage credentials expired',
  error: 'Usage could not be read',
};

/** Quota bars from ProviderUsage; explicit messages instead of fake zeros. */
export function QuotaBar({ usage, error }) {
  if (error) return html`<div class="quota quota-message" id="quota">${error}</div>`;
  if (!usage) return null;
  if (usage.status !== 'ok') {
    const text = USAGE_STATUS[usage.status] || 'Usage unavailable';
    return html`<div class="quota quota-message" id="quota">${text}${usage.message ? `: ${usage.message}` : ''}</div>`;
  }
  const windows = Array.isArray(usage.windows) ? usage.windows.filter((w) => w && Number.isFinite(w.used_percent)) : [];
  if (!windows.length) return html`<div class="quota quota-message" id="quota">No usage windows reported</div>`;
  return html`<div class="quota" id="quota">
    ${windows.slice(0, 3).map((w) => html`<div class="quota-window">
      <span class="quota-label">${String(w.label || '')} ${Math.round(w.used_percent)}%</span>
      <span class="quota-track" role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow=${Math.round(w.used_percent)} aria-label=${`${w.label} usage`}>
        <span class=${cx('quota-fill', quotaClass(w.used_percent), w.used_percent >= 90 && 'quota-high')}></span>
      </span>
    </div>`)}
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
