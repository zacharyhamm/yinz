import { describeError } from '../api.js';
import { html, useState } from './ui.js';
import { Sheet } from './sheets.js';

export function pairingLink(origin, code) {
  const setup = JSON.stringify({ origin, code });
  return 'shortcuts://run-shortcut?name=' + encodeURIComponent('Talk to Yinz')
    + '&input=text&text=' + encodeURIComponent(setup);
}

export function SiriSetup({ api, onClose }) {
  const [link, setLink] = useState('');
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);
  async function configure(revoke) {
    setBusy(true);
    setLink('');
    setMessage('');
    try {
      const r = await api.post(revoke ? '/api/siri/revoke' : '/api/siri/code', {});
      if (!r.ok) { setMessage(describeError(r)); return; }
      if (revoke) { setMessage('Siri access revoked for this sign-in.'); return; }
      setLink(pairingLink(location.origin, r.data.code));
    } finally { setBusy(false); }
  }
  return html`<${Sheet} id="sheet-siri" title="Add to Siri" onClose=${onClose}>
    <p>Add Talk to Yinz in Shortcuts, then return here to pair this phone.</p>
    <a class="button button-primary" href="/static/talk-to-yinz.shortcut">Install Talk to Yinz</a>
    <button class="button" disabled=${busy} onClick=${() => configure(false)}>${link ? 'Renew pairing link' : 'Create pairing link'}</button>
    ${link ? html`<p><a class="button" href=${link}>Pair this phone</a> This link expires in five minutes.</p>` : null}
    <p>Say “Siri, Talk to Yinz,” then “Find the waiting agents.”</p>
    <button class="link-button" disabled=${busy} onClick=${() => configure(true)}>Revoke Siri access</button>
    <p role="status">${message}</p>
  <//>`;
}
