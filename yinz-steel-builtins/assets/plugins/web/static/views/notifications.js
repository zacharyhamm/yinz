import { html, useState, useEffect } from './ui.js';
import { pushSupport } from '../push.js';

export function Notifications({ push }) {
  const [enabled, setEnabled] = useState(false);
  const [busy, setBusy] = useState(true);
  const [message, setMessage] = useState('');
  const support = pushSupport();
  useEffect(() => {
    let live = true;
    push.sync().then((value) => { if (live) setEnabled(value); })
      .catch((error) => { if (live) setMessage(error.message); })
      .finally(() => { if (live) setBusy(false); });
    return () => { live = false; };
  }, []);
  async function toggle() {
    setBusy(true);
    setMessage('');
    try {
      if (enabled) await push.disable();
      else await push.enable();
      setEnabled(!enabled);
    } catch (error) { setMessage(error.message); }
    finally { setBusy(false); }
  }
  return html`<section aria-label="Session notifications">
    <p>${support || 'Receive notifications from all local Yinz sessions, including when this app is closed.'}</p>
    ${!support ? html`<button type="button" class="button" disabled=${busy} onClick=${toggle}>${busy ? 'Checking notifications…' : enabled ? 'Disable notifications' : 'Enable notifications'}</button>` : null}
    <p role="status">${message || (enabled ? 'Notifications enabled for this device.' : '')}</p>
  </section>`;
}
