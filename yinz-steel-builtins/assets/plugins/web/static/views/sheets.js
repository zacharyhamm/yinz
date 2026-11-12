// Modal bottom sheets: role=dialog, aria-modal, labelled; focus trap; Escape and
// backdrop dismiss; focus returns to the opener; background scroll is locked
// with a body class (never inline styles) and released on close.
import { html, useEffect, useRef, cx } from './ui.js';

const FOCUSABLE = 'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';
let openCount = 0;
// iOS Safari does not focus buttons on tap, so remember the last activated control
// as the opener to restore focus to.
let lastActivated = null;
if (typeof document !== 'undefined') {
  document.addEventListener('click', (e) => {
    const target = e.target && e.target.closest ? e.target.closest(FOCUSABLE) : null;
    if (target) lastActivated = target;
  }, true);
}

function lockScroll() {
  openCount += 1;
  document.body.classList.add('scroll-locked');
}

function releaseScroll() {
  openCount = Math.max(0, openCount - 1);
  if (openCount === 0) document.body.classList.remove('scroll-locked');
}

export function Sheet({ id, title, onClose, children, footer, wide = false }) {
  const panel = useRef(null);
  const titleId = `${id}-title`;
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const active = document.activeElement;
    const opener = active && active !== document.body ? active : lastActivated;
    lockScroll();
    const node = panel.current;
    const first = node && (node.querySelector('[data-autofocus]') || node.querySelector(FOCUSABLE));
    if (first) first.focus();
    else if (node) node.focus();
    const onKey = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        closeRef.current();
        return;
      }
      if (e.key !== 'Tab' || !node) return;
      const items = [...node.querySelectorAll(FOCUSABLE)].filter((el) => el.offsetParent !== null || el === document.activeElement);
      if (!items.length) {
        e.preventDefault();
        node.focus();
        return;
      }
      const firstItem = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && (document.activeElement === firstItem || !node.contains(document.activeElement))) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && (document.activeElement === last || !node.contains(document.activeElement))) {
        e.preventDefault();
        firstItem.focus();
      }
    };
    document.addEventListener('keydown', onKey, true);
    return () => {
      document.removeEventListener('keydown', onKey, true);
      releaseScroll();
      if (opener && typeof opener.focus === 'function' && document.contains(opener)) opener.focus({ preventScroll: true });
    };
  }, []);
  return html`<div class="sheet-layer">
    <div class="sheet-backdrop" data-testid="sheet-backdrop" onClick=${() => closeRef.current()}></div>
    <div class=${cx('sheet', wide && 'sheet-wide')} id=${id} role="dialog" aria-modal="true" aria-labelledby=${titleId} tabindex="-1" ref=${panel}>
      <div class="sheet-head">
        <h2 class="sheet-title" id=${titleId}>${title}</h2>
        <button type="button" class="sheet-close icon-button" aria-label="Close" onClick=${() => closeRef.current()}>✕</button>
      </div>
      <div class="sheet-body">${children}</div>
      ${footer ? html`<div class="sheet-foot">${footer}</div>` : null}
    </div>
  </div>`;
}

export function ConfirmSheet({ id, title, message, confirmLabel, confirmId, onConfirm, onClose, danger = false }) {
  return html`<${Sheet} id=${id} title=${title} onClose=${onClose}
    footer=${html`<button type="button" class="button" data-autofocus onClick=${onClose}>Cancel</button>
      <button type="button" id=${confirmId} class=${cx('button', danger ? 'button-danger' : 'button-primary')}
        onClick=${() => { onClose(); onConfirm(); }}>${confirmLabel}</button>`}>
    <p>${message}</p>
  <//>`;
}
