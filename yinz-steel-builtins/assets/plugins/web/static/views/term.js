import { useNativeCharacterWidths } from './character-width.mjs';
import { Terminal } from '../vendor/xterm.mjs';
import { describeError } from '../api.js';
import { html, useLayoutEffect, useRef, useState, usePoller, Banner, QuickKeys, responseText } from './ui.js';

// xterm updates these sheets when its font metrics or palette change. Constructed
// stylesheets let the document retain its external-only content security policy.
function registerTerminalStyles() {
  if (customElements.get('yinz-xterm-style')) return;
  class TerminalStyles extends HTMLElement {
    sheet = new CSSStyleSheet();
    connectedCallback() {
      document.adoptedStyleSheets.push(this.sheet);
    }
    disconnectedCallback() {
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter((sheet) => sheet !== this.sheet);
    }
    set textContent(css) {
      this.sheet.replaceSync(css);
    }
  }
  customElements.define('yinz-xterm-style', TerminalStyles);
}

const maxInputBytes = 16 * 1024;

const terminalKeys = new Map([
  ['Enter', 'Enter'], ['Escape', 'Esc'], ['Tab', 'Tab'], ['Backspace', 'Backspace'],
  ['ArrowLeft', 'Left'], ['ArrowRight', 'Right'], ['ArrowUp', 'Up'], ['ArrowDown', 'Down'],
  ...['Home', 'End', 'PageUp', 'PageDown', 'Delete', 'Insert'].map((name) => [name, name]),
]);

function negotiatedKey(event) {
  if (event.type !== 'keydown' || event.isComposing || event.keyCode === 229 || event.getModifierState('AltGraph')) return;
  const { key, ctrlKey, altKey, shiftKey, metaKey } = event;
  // Let the browser dispatch clipboard events and xterm handle composed text.
  if (((metaKey || (ctrlKey && shiftKey)) && ['c', 'v', 'x'].includes(key.toLowerCase()))
      || (shiftKey && key === 'Insert')) return;
  const modifiers = [];
  if (ctrlKey) modifiers.push('Ctrl');
  if (altKey) modifiers.push('Alt');
  if (shiftKey) modifiers.push('Shift');
  if (metaKey) modifiers.push('Super');
  if (!modifiers.length && key !== 'Escape') return;
  let bare = terminalKeys.get(key);
  if (!bare && /^F(?:[1-9]|1[0-9]|2[0-4])$/.test(key)) bare = { F: Number(key.slice(1)) };
  if (!bare && (ctrlKey || altKey || metaKey) && [...key].length === 1) bare = { Char: key };
  if (bare) return { bare_key: bare, key_modifiers: modifiers };
}

export function TermPanel({ env, target, onTargetGone }) {
  const host = useRef(null);
  const stage = useRef(null);
  const session = useRef(null);
  const [ready, setReady] = useState(false);
  const [warning, setWarning] = useState('');
  const [sizeWarning, setSizeWarning] = useState('');
  const [state, refresh] = usePoller({
    key: 'term', intervalMs: env.config.poll.screen_ms, env,
    run: async ({ signal }) => {
      const active = session.current;
      for (;;) {
        const after = active.cursor;
        const r = await env.api.post('/api/agent/term', { agent: target, ...(after && { after }) }, { signal });
        if (signal.aborted || session.current !== active) return r;
        const response = r.data?.response;
        const frame = response?.terminal;
        if (!r.ok || response?.outcome !== 'ok' || !frame) {
          active.accepting = false;
          setReady(false);
          if (r.status === 409 || response?.outcome === 'target_changed') {
            active.uncertain = true;
            active.queue.length = 0;
            onTargetGone();
          }
          return r;
        }
        if (frame.snapshot) {
          active.accepting = false;
          setReady(false);
          active.term.reset();
          active.term.resize(frame.cols, frame.rows);
        }
        if (frame.data) {
          const bytes = Uint8Array.from(atob(frame.data), (c) => c.charCodeAt(0));
          await new Promise((done) => active.term.write(bytes, done));
        }
        if (signal.aborted || session.current !== active) return r;
        active.cursor = frame.cursor;
        if (frame.snapshot_continues) {
          active.replay = 'parts';
          continue;
        }
        // The last part ends at capture time. Follow its live cursor even
        // when a retry or replacement snapshot intervenes.
        if (active.replay === 'parts' || (active.replay === 'live' && frame.snapshot)) {
          active.replay = 'live';
          continue;
        }
        active.replay = null;
        active.accepting = !active.uncertain;
        setReady(active.accepting);
        active.scheduleWrite();
        return r;
      }
    },
  });

  useLayoutEffect(() => {
    registerTerminalStyles();
    const term = new Terminal({ scrollback: 2000, fontSize: 14, allowProposedApi: true, screenReaderMode: true });
    useNativeCharacterWidths(term);
    const controller = new AbortController();
    const active = { term, cursor: null, replay: null, accepting: false, uncertain: false, queue: [], sending: false };
    session.current = active;
    term.open(host.current);

    const layout = new CSSStyleSheet();
    document.adoptedStyleSheets.push(layout);
    const grid = host.current.querySelector('.xterm-screen');
    const scroll = stage.current.parentElement;
    let pinned = true;
    let readingOffset = 0;
    let scale = 1;
    let viewportHeight, viewportWidth, contentHeight, paddingTop;
    const rememberScroll = () => {
      // Reflow can clamp scrollTop and fire scroll before the resize observer.
      if (scroll.clientHeight !== viewportHeight || scroll.clientWidth !== viewportWidth
          || scroll.scrollHeight !== contentHeight) return;
      pinned = contentHeight - viewportHeight - scroll.scrollTop <= 2;
      // Keep the top of the reading area in the grid's unscaled coordinates.
      readingOffset = (scroll.scrollTop - paddingTop) / scale;
    };
    scroll.addEventListener('scroll', rememberScroll, { signal: controller.signal });
    const fit = () => {
      const width = grid.offsetWidth;
      if (!width || !stage.current) return;
      scale = stage.current.clientWidth / width;
      if (!scale) return;
      layout.replaceSync(`.term-stage { height: ${Math.ceil(grid.offsetHeight * scale)}px }
        .term-host { width: ${width}px; transform: scale(${scale}) }`);
      viewportHeight = scroll.clientHeight;
      viewportWidth = scroll.clientWidth;
      contentHeight = scroll.scrollHeight;
      paddingTop = parseFloat(getComputedStyle(scroll).paddingTop);
      const top = pinned ? contentHeight - viewportHeight : readingOffset * scale + paddingTop;
      if (Math.abs(scroll.scrollTop - top) > 1) scroll.scrollTop = top;
    };
    const observer = new ResizeObserver(fit);
    observer.observe(stage.current);
    observer.observe(grid);
    observer.observe(scroll);
    fit();

    let timer;
    function scheduleWrite() {
      if (!active.accepting || active.sending || timer || !active.queue.length) return;
      // Coalesce repeats and mouse motion within the listener's per-token budget.
      timer = setTimeout(writeNext, 80);
    }
    active.scheduleWrite = scheduleWrite;
    async function writeNext() {
      timer = null;
      if (!active.accepting || !active.queue.length) return;
      active.sending = true;
      const item = active.queue.shift();
      const key = item.key !== undefined;
      let body;
      if (key) body = { keys: [item.key] };
      else if (item.terminalKey) body = { data: item.terminalKey };
      else {
        const encoded = btoa(item.bytes);
        body = { data: item.mouse ? { mouse: encoded } : encoded };
      }
      try {
        const r = await env.api.post(key ? '/api/agent/keys' : '/api/agent/term/input',
          { agent: target, ...body }, { signal: controller.signal });
        if (controller.signal.aborted) return;
        if (!r.ok || r.data?.response?.outcome !== 'ok') {
          active.uncertain = true;
          active.accepting = false;
          active.queue.length = 0;
          setReady(false);
          setWarning('Input stopped. The terminal may have received partial input. ' + (r.ok ? responseText(r.data?.response) : describeError(r)));
          if (r.status === 409 || r.data?.response?.outcome === 'target_changed') onTargetGone();
        }
      } finally {
        active.sending = false;
        if (!controller.signal.aborted) scheduleWrite();
      }
    }
    active.quickKey = (key) => {
      if (!active.accepting) return;
      active.queue.push({ key });
      scheduleWrite();
    };
    function send(bytes, mouse) {
      if (!active.accepting) return;
      // xterm emits the whole paste, including both paste markers, in one event.
      if (bytes.length > maxInputBytes) {
        setSizeWarning('Input exceeds the 16 KiB limit. Send a smaller paste.');
        return;
      }
      if (!bytes.length) return;
      setSizeWarning('');
      const chunk = String.fromCharCode(...bytes);
      const last = active.queue.at(-1);
      // The host records a standalone Enter as an agent submission boundary.
      if (last?.mouse === mouse && last.bytes !== '\r' && chunk !== '\r' && last.bytes.length + chunk.length <= maxInputBytes) last.bytes += chunk;
      else active.queue.push({ bytes: chunk, mouse });
      scheduleWrite();
    }
    term.attachCustomKeyEventHandler((event) => {
      const terminalKey = negotiatedKey(event);
      if (!terminalKey) return true;
      event.preventDefault();
      if (active.accepting) {
        active.queue.push({ terminalKey });
        scheduleWrite();
      }
      return false;
    });
    term.onData((text) => send(new TextEncoder().encode(text), false));
    term.onBinary((text) => send(Uint8Array.from(text, (c) => c.charCodeAt(0)), true));
    return () => {
      active.accepting = false;
      active.queue.length = 0;
      session.current = null;
      clearTimeout(timer);
      controller.abort();
      observer.disconnect();
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter((sheet) => sheet !== layout);
      term.dispose();
    };
  }, []);

  const response = state?.data?.response;
  const error = warning || (state?.error ? describeError(state.error) : response && response.outcome !== 'ok' && responseText(response)) || sizeWarning;
  function reconnect() {
    const active = session.current;
    active.uncertain = false;
    setWarning('');
    refresh();
  }
  return html`<div class="screen-panel">
    <div class="screen-feedback" role="region" aria-label="Terminal status" tabindex="0">
      <div class="screen-status">
        ${!ready && !error ? 'Connecting…' : null}
        <button class="link-button" type="button" onClick=${reconnect}>${warning ? 'Reconnect' : 'Refresh'}</button>
      </div>
      ${error ? html`<${Banner} tone="warn">${error}<//>` : null}
    </div>
    <div class="screen-scroll"><div class="term-stage" ref=${stage}><div class="term-host" ref=${host}></div></div></div>
    <div class="screen-keys" role="region" aria-label="Terminal quick keys" tabindex="0">
      <${QuickKeys} keys=${env.config.quick_keys} disabled=${!ready} onKey=${(key) => session.current?.quickKey(key)} />
    </div>
  </div>`;
}
