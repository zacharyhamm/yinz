// Entry point for `/`: bootstrap config, credentials, session check and routing.
import {
  KEYS, readBootstrapConfig, safeStorage, createTokenStore, createApi, clientTimezone, describeError,
  saveReturn, setNotice, takeNotice, logoutNotice, createPoller,
} from './api.js';
import { createSubmissions } from './submission.js';
import { parseHash } from './app-route.js';
import { html, render, useState, useEffect, cx } from './views/ui.js';
import { ListView } from './views/list.js';
import { DetailView } from './views/detail.js';

const config = readBootstrapConfig(document);
const local = safeStorage(() => window.localStorage);
const session = safeStorage(() => window.sessionStorage);
const tokens = createTokenStore({ local, session });
let leaving = false;

/** Keep the route (drafts are already persisted) and go to /login. */
function toLogin(notice) {
  if (leaving) return;
  leaving = true;
  saveReturn(session, window.location.hash);
  if (notice) setNotice(session, notice);
  // Let in-flight continuations persist their state before navigating.
  window.setTimeout(() => window.location.assign('/login'), 0);
}

const api = createApi({
  fetch: (url, init) => window.fetch(url, init),
  tokens,
  config,
  onUnauthenticated: () => toLogin('Your sign-in expired or was revoked. Sign in again.'),
});

const visibilityListeners = new Set();
document.addEventListener('visibilitychange', () => {
  for (const listener of visibilityListeners) listener();
});
const visibility = {
  isHidden: () => document.hidden,
  subscribe(listener) {
    visibilityListeners.add(listener);
    return () => visibilityListeners.delete(listener);
  },
};

// Another tab logged out: leave without touching pending submission records.
window.addEventListener('storage', (event) => {
  if (event.key !== KEYS.token && event.key !== null) return;
  if (!tokens.get()) {
    api.bumpAuthGeneration();
    toLogin('You were signed out in another tab.');
  }
});

const submissions = createSubmissions({ storage: session, api });
let navigations = 0;
window.addEventListener('hashchange', () => {
  navigations += 1;
});

function App({ initialNotice }) {
  const [route, setRoute] = useState(parseHash(window.location.hash));
  const [toast, setToast] = useState(null);
  const [notice, setNoticeText] = useState(initialNotice);
  useEffect(() => {
    const onHash = () => setRoute(parseHash(window.location.hash));
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);
  useEffect(() => {
    if (!toast) return undefined;
    const t = window.setTimeout(() => setToast(null), 6000);
    return () => window.clearTimeout(t);
  }, [toast]);
  const env = {
    api,
    config,
    tz: clientTimezone(),
    submissions,
    visibility,
    notice,
    clearNotice: () => setNoticeText(''),
    toast: (text) => setToast({ text: String(text), at: Date.now() }),
    pushHash: (hash) => {
      window.location.hash = hash;
    },
    replaceHash: (hash) => {
      if (window.location.hash === hash) return;
      window.history.replaceState(window.history.state, '', hash);
      setRoute(parseHash(hash));
    },
    canGoBack: () => navigations > 0,
    async logout(all) {
      const r = await api.logout({ all });
      toLogin(logoutNotice({ ...r, all }));
    },
  };
  const key = route.view === 'detail' ? `${route.bind ? 'a' : 'r'}:${route.id}:${route.bind ? (route.run ? 'run' : 'none') : route.run}` : 'list';
  return html`<div class="app">
    ${route.view === 'detail' ? html`<${DetailView} key=${key} env=${env} route=${route} />` : html`<${ListView} env=${env} />`}
    <div class=${cx('toast', toast && 'toast-visible')} id="toast" role="status" aria-live="polite">${toast ? toast.text : ''}</div>
  </div>`;
}

function Connecting({ onReady }) {
  const [state, setState] = useState(null);
  useEffect(() => {
    const poller = createPoller({
      run: ({ signal }) => api.session({ signal }),
      intervalMs: 3600000,
      isHidden: visibility.isHidden,
      onUpdate: (s) => {
        setState(s);
        if (s.status === 'ok') {
          poller.stop();
          onReady();
        }
      },
    });
    const unsubscribe = visibility.subscribe(() => poller.visibilityChanged());
    poller.start();
    return () => {
      unsubscribe();
      poller.stop();
    };
  }, []);
  const failed = state && state.error && state.status !== 'unauthenticated';
  return html`<main class="connecting" id="connecting">
    <h1 class="header-title">${config.title}</h1>
    <p role="status" aria-live="polite">${failed ? `${describeError(state.error)} Retrying…` : 'Connecting…'}</p>
  </main>`;
}

function Root() {
  const [ready, setReady] = useState(false);
  const [notice] = useState(() => {
    const text = takeNotice(session);
    if (!local.available && !session.available) return 'This browser is blocking site storage. Drafts are kept on this page only and prompts cannot be sent safely.';
    return text;
  });
  return ready ? html`<${App} initialNotice=${notice} />` : html`<${Connecting} onReady=${() => setReady(true)} />`;
}

const mount = document.getElementById('app');
if (!tokens.get()) toLogin('');
else if (mount) render(html`<${Root} />`, mount);
