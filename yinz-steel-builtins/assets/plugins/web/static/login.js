// Entry point for `/login`: native password login through fetch (never a native
// form submission). The password is cleared after every attempt and is never
// logged, stored or placed in a URL.
import {
  readBootstrapConfig, safeStorage, createTokenStore, createApi, describeError, takeReturn, takeNotice, setNotice,
} from './api.js';

const config = readBootstrapConfig(document);
const local = safeStorage(() => window.localStorage);
const session = safeStorage(() => window.sessionStorage);
const tokens = createTokenStore({ local, session });
const api = createApi({ fetch: (url, init) => window.fetch(url, init), tokens, config });

const form = document.getElementById('login-form');
const password = document.getElementById('password');
const label = document.getElementById('label');
const remember = document.getElementById('remember');
const submit = document.getElementById('login-submit');
const status = document.getElementById('login-status');

let countdown = null;
let busy = false;

function show(text, tone = 'info') {
  if (!status) return;
  status.textContent = text;
  status.classList.toggle('status-error', tone === 'error');
  status.classList.toggle('status-ok', tone === 'ok');
}

function setBusy(value) {
  busy = value;
  if (submit) submit.disabled = value || countdown !== null;
  if (form) form.setAttribute('aria-busy', value ? 'true' : 'false');
}

function startCountdown(ms, prefix) {
  if (countdown !== null) window.clearInterval(countdown);
  const until = Date.now() + ms;
  const tick = () => {
    const left = Math.ceil((until - Date.now()) / 1000);
    if (left <= 0) {
      window.clearInterval(countdown);
      countdown = null;
      if (submit) submit.disabled = busy;
      show('You can try again now.');
      return;
    }
    show(`${prefix} Try again in ${left} s.`, 'error');
  };
  countdown = window.setInterval(tick, 1000);
  if (submit) submit.disabled = true;
  tick();
}

const LOGIN_TEXT = {
  invalid_credentials: 'Wrong password.',
  invalid_request: 'The sign-in request was rejected. Check the password and device label.',
  no_password: 'No web password is set. Run `yinz set-password` on the host.',
  session_store_unavailable: 'The host session store is unavailable. Run `yinz set-password --revoke-sessions` on the host.',
  unauthenticated: 'Sign in again.',
};

function failureText(result) {
  if (result.kind !== 'http') return describeError(result);
  if (LOGIN_TEXT[result.code]) return LOGIN_TEXT[result.code];
  const base = describeError({ ...result, retryAfterMs: null });
  if (result.status === 403) return `${base} Open this page from an allowed address.`;
  return base;
}

async function signIn(event) {
  event.preventDefault();
  if (busy || countdown !== null || !password) return;
  const body = {
    password: password.value,
    label: label && label.value.trim() ? label.value.trim().slice(0, 128) : '',
    remember: !!(remember && remember.checked),
  };
  // Clear the secret from the field and drop local references to it.
  password.value = '';
  setBusy(true);
  show('Signing in…');
  const result = await api.post('/api/login', body, { auth: false });
  body.password = '';
  setBusy(false);
  if (!result.ok) {
    if (result.retryAfterMs != null && (result.status === 429 || result.status === 503)) {
      startCountdown(result.retryAfterMs, failureText(result));
    } else show(failureText(result), 'error');
    password.focus();
    return;
  }
  const data = result.data || {};
  const stored = tokens.set(data.token, body.remember);
  if (!stored.ok) {
    // The token cannot reach the app page; revoke it rather than leave it dangling.
    await api.post('/api/logout', {}, { handleUnauthenticated: false });
    tokens.clear();
    show(stored.warning, 'error');
    return;
  }
  if (stored.warning) setNotice(session, stored.warning);
  show('Signed in.', 'ok');
  const hash = takeReturn(session);
  window.location.replace(`/${hash}`);
}

if (form) {
  form.addEventListener('submit', signIn);
  const notice = takeNotice(session);
  if (notice) show(notice);
  if (!local.available && !session.available) {
    show('This browser is blocking site storage, so sign-in cannot be kept. Allow site data for this page.', 'error');
  }
  if (label && !label.value) label.placeholder = 'This device';
  document.title = `Sign in · ${config.title}`;
}
