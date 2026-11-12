// "Send keys…" sheet: compose up to eight allowlisted keys for the captured
// complete identity. One explicit request per tap of Send; never replayed.
import { QUICK_KEYS, describeError } from '../api.js';
import { html, useState, keyLabel, keyName, responseText } from './ui.js';
import { Sheet } from './sheets.js';

export function KeysSheet({ env, target, onClose, onTargetGone }) {
  const [keys, setKeys] = useState([]);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState(null);
  async function send() {
    if (!keys.length || busy) return;
    setBusy(true);
    setResult(null);
    const r = await env.api.post('/api/agent/keys', { agent: target, keys });
    setBusy(false);
    if (r.ok) {
      setResult(responseText(r.data && r.data.response));
      setKeys([]);
    } else if (r.status === 409) {
      setResult('The agent changed or disappeared. The keys were not sent.');
      onTargetGone();
    } else {
      setResult(`Not confirmed: ${describeError(r)} Check the screen before sending again.`);
    }
  }
  return html`<${Sheet} id="sheet-keys" title="Send keys" onClose=${onClose}
    footer=${html`<button type="button" class="button" id="keys-clear" disabled=${!keys.length || busy} onClick=${() => setKeys([])}>Clear</button>
      <button type="button" class="button button-primary" id="keys-send" disabled=${!keys.length || busy} onClick=${send}>${busy ? 'Sending…' : 'Send'}</button>`}>
    <p class="keys-sequence" id="keys-sequence" aria-live="polite">${keys.length ? keys.map(keyLabel).join(' ') : 'No keys selected'}</p>
    <div class="key-grid" role="group" aria-label="Keys">
      ${QUICK_KEYS.map((k) => html`<button type="button" class="quick-key" data-key=${k} aria-label=${keyName(k)}
        disabled=${keys.length >= 8 || busy} onClick=${() => setKeys((ks) => (ks.length < 8 ? [...ks, k] : ks))}>${keyLabel(k)}</button>`)}
    </div>
    ${result ? html`<p class="keys-result" id="keys-result" role="status">${result}</p>` : null}
  <//>`;
}
