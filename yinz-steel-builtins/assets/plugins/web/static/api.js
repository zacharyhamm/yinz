// Transport, credentials and polling for the builtin web app.
//
// Everything here is DOM-free: fetch, storage, clocks, timers and visibility are
// injected so `node --test` can drive the exact request sequence. Credentials are
// sent only as an Authorization header on same-origin relative paths and are never
// written to URLs, markup, logs or the console.

export const KEYS = Object.freeze({
  token: 'yinz.web.v1.token',
  returnHash: 'yinz.web.v1.return',
  notice: 'yinz.web.v1.notice',
  submissionPrefix: 'yinz.web.v1.submission:',
});

export const QUICK_KEYS = Object.freeze([
  'esc', 'enter', 'tab', 'up', 'down', 'left', 'right', 'backspace', 'y', 'n',
  '1', '2', '3', '4', '5', '6', '7', '8', '9',
]);

export const DEFAULT_CONFIG = Object.freeze({
  title: 'yinz',
  poll: Object.freeze({ list_ms: 3000, tail_ms: 2000, usage_ms: 60000, screen_ms: 2000 }),
  quick_keys: Object.freeze(['1', '2', '3', 'enter', 'esc', 'up', 'down']),
  timeouts: Object.freeze({ get_ms: 10000, post_ms: 45000 }),
});

const MIN_BACKOFF_MS = 2000;
const MAX_BACKOFF_MS = 30000;
const MAX_RETRY_AFTER_MS = 10 * 60 * 1000;

function boundedInt(value, fallback, min, max) {
  return Number.isInteger(value) && value >= min && value <= max ? value : fallback;
}

/** Accept only the public bootstrap whitelist; anything malformed falls back to defaults. */
export function normalizeConfig(raw) {
  const c = raw && typeof raw === 'object' ? raw : {};
  const poll = c.poll && typeof c.poll === 'object' ? c.poll : {};
  const timeouts = c.timeouts && typeof c.timeouts === 'object' ? c.timeouts : {};
  const d = DEFAULT_CONFIG;
  const keys = Array.isArray(c.quick_keys)
    ? c.quick_keys.filter((k) => QUICK_KEYS.includes(k)).slice(0, 8)
    : [];
  return {
    title: typeof c.title === 'string' && [...c.title].length >= 1 && [...c.title].length <= 64 ? c.title : d.title,
    poll: {
      list_ms: boundedInt(poll.list_ms, d.poll.list_ms, 500, 600000),
      tail_ms: boundedInt(poll.tail_ms, d.poll.tail_ms, 500, 600000),
      usage_ms: boundedInt(poll.usage_ms, d.poll.usage_ms, 1000, 3600000),
      screen_ms: boundedInt(poll.screen_ms, d.poll.screen_ms, 500, 600000),
    },
    quick_keys: keys.length ? keys : [...d.quick_keys],
    timeouts: {
      get_ms: boundedInt(timeouts.get_ms, d.timeouts.get_ms, 1000, 120000),
      post_ms: boundedInt(timeouts.post_ms, d.timeouts.post_ms, 1000, 300000),
    },
  };
}

export function readBootstrapConfig(doc) {
  try {
    const node = doc.getElementById('yinz-config');
    return normalizeConfig(node ? JSON.parse(node.textContent || '{}') : {});
  } catch {
    return normalizeConfig({});
  }
}

function quotaError(e) {
  return !!e && (e.name === 'QuotaExceededError' || e.name === 'NS_ERROR_DOM_QUOTA_REACHED' || e.code === 22);
}

/**
 * Wrap a Storage getter. Accessing `window.localStorage` itself can throw (denied
 * site data), and every operation can throw (quota, private mode). Writes are
 * validated by read-back so callers never claim persistence that did not happen.
 */
export function safeStorage(getStorage) {
  let store = null;
  let failure = null;
  try {
    store = typeof getStorage === 'function' ? getStorage() : getStorage;
    if (!store) failure = 'unavailable';
  } catch {
    failure = 'denied';
  }
  return {
    get available() {
      return !!store;
    },
    get(key) {
      if (!store) return { ok: false, value: null, error: failure };
      try {
        return { ok: true, value: store.getItem(key) };
      } catch {
        return { ok: false, value: null, error: 'denied' };
      }
    },
    set(key, value) {
      if (!store) return { ok: false, error: failure };
      try {
        store.setItem(key, value);
        return store.getItem(key) === value ? { ok: true } : { ok: false, error: 'readback' };
      } catch (e) {
        return { ok: false, error: quotaError(e) ? 'quota' : 'denied' };
      }
    },
    remove(key) {
      if (!store) return { ok: false, error: failure };
      try {
        store.removeItem(key);
        return { ok: true };
      } catch {
        return { ok: false, error: 'denied' };
      }
    },
    keys() {
      if (!store) return [];
      try {
        const out = [];
        for (let i = 0; i < store.length; i += 1) {
          const k = store.key(i);
          if (k != null) out.push(k);
        }
        return out;
      } catch {
        return [];
      }
    },
  };
}

/** In-memory Storage double, also used as a last-resort fallback. */
export class MemoryStorage {
  constructor() {
    this.map = new Map();
  }
  get length() {
    return this.map.size;
  }
  key(i) {
    return [...this.map.keys()][i] ?? null;
  }
  getItem(k) {
    return this.map.has(k) ? this.map.get(k) : null;
  }
  setItem(k, v) {
    this.map.set(k, String(v));
  }
  removeItem(k) {
    this.map.delete(k);
  }
  clear() {
    this.map.clear();
  }
}

const TOKEN_SHAPE = /^[\x21-\x7e]{1,512}$/;

/**
 * Bearer token storage: localStorage when remembered, sessionStorage otherwise.
 * Logging in removes the token from the other store. A token kept only in page
 * memory cannot survive navigation, so callers must surface `persisted:false`.
 */
export function createTokenStore({ local, session }) {
  let memory = null;
  return {
    get() {
      for (const store of [session, local]) {
        const r = store.get(KEYS.token);
        if (r.ok && r.value != null) {
          if (TOKEN_SHAPE.test(r.value)) return r.value;
          store.remove(KEYS.token); // corrupt value: never send it
        }
      }
      return memory;
    },
    set(token, remember) {
      if (typeof token !== 'string' || !TOKEN_SHAPE.test(token)) {
        return { ok: false, persisted: false, remembered: false, warning: 'The host returned an unusable sign-in token.' };
      }
      memory = token;
      const primary = remember ? local : session;
      const other = remember ? session : local;
      other.remove(KEYS.token);
      if (primary.set(KEYS.token, token).ok) return { ok: true, persisted: true, remembered: !!remember };
      if (remember && session.set(KEYS.token, token).ok) {
        return {
          ok: true,
          persisted: true,
          remembered: false,
          warning: 'This browser would not remember the device; you are signed in for this tab only.',
        };
      }
      return {
        ok: false,
        persisted: false,
        remembered: false,
        warning: 'This browser is blocking site storage, so the sign-in cannot be kept. Allow site data and try again.',
      };
    },
    clear() {
      memory = null;
      const a = session.remove(KEYS.token);
      const b = local.remove(KEYS.token);
      return { ok: a.ok && b.ok };
    },
  };
}

/** Seconds or HTTP-date Retry-After, as milliseconds from `nowMs`; null when absent. */
export function parseRetryAfter(value, nowMs) {
  if (value == null || value === '') return null;
  const text = String(value).trim();
  if (/^\d{1,9}$/.test(text)) return Math.min(Number(text) * 1000, MAX_RETRY_AFTER_MS);
  const at = Date.parse(text);
  if (Number.isNaN(at)) return null;
  return Math.min(Math.max(0, at - nowMs), MAX_RETRY_AFTER_MS);
}

const CODE_TEXT = {
  invalid_credentials: 'Wrong password.',
  invalid_request: 'The request was rejected as malformed.',
  unauthenticated: 'Your sign-in expired or was revoked. Sign in again.',
  no_password: 'No web password is set. Run `yinz set-password` on the host.',
  session_store_unavailable:
    'The host session store is unavailable. Run `yinz set-password --revoke-sessions` on the host to recover.',
  rate_limited: 'Too many requests.',
  starting: 'Yinz web is starting.',
  busy: 'Yinz is busy.',
  updating: 'Yinz is updating.',
  paused: 'Yinz is paused for an update.',
  not_serving: 'This session is not serving the web app.',
  unavailable: 'Yinz is temporarily unavailable.',
  timeout: 'Yinz did not answer in time.',
  plugin_failed: 'The web app plugin failed.',
  invalid_response: 'The web app plugin sent an invalid response.',
  invalid: 'The request was invalid.',
  bad_json: 'The request body was not valid JSON.',
  not_found: 'Not found.',
  target_changed: 'The agent disappeared or changed identity.',
  failed: 'The host could not complete the request.',
  method_not_allowed: 'That request method is not allowed.',
  internal: 'The web app hit an internal error.',
};
const STATUS_TEXT = {
  400: 'The request was rejected.',
  403: 'The host refused this request (origin, host or identity policy).',
  404: 'Not found.',
  408: 'The request took too long to send.',
  413: 'The request is too large.',
  415: 'The request content type was refused.',
  429: 'Too many requests.',
  502: 'The web app failed to answer.',
  503: 'Yinz is temporarily unavailable.',
  504: 'Yinz did not answer in time.',
};
// Codes whose locally written text is more actionable than any server message.
const LOCAL_FIRST = new Set(['no_password', 'session_store_unavailable', 'rate_limited', 'invalid_credentials']);

function cleanMessage(message) {
  if (typeof message !== 'string') return null;
  const t = message.replace(/[\u0000-\u001f\u007f]/g, ' ').trim();
  return t ? t.slice(0, 300) : null;
}

/** Human text for a failed request result (never includes credentials). */
export function describeError(result) {
  if (!result || result.ok) return '';
  if (result.kind === 'network') return 'Cannot reach yinz.';
  if (result.kind === 'timeout') return 'The request timed out.';
  if (result.kind === 'aborted') return 'The request was cancelled.';
  if (result.kind === 'malformed') return 'Yinz sent a response this page cannot read.';
  const code = result.code;
  const server = cleanMessage(result.message);
  let text = (code && LOCAL_FIRST.has(code) ? CODE_TEXT[code] : null)
    || server
    || (code && CODE_TEXT[code])
    || STATUS_TEXT[result.status]
    || `Request failed (HTTP ${result.status}).`;
  if (result.retryAfterMs != null && (code === 'rate_limited' || result.status === 429 || result.status === 503)) {
    text += ` Try again in ${Math.max(1, Math.ceil(result.retryAfterMs / 1000))} s.`;
  }
  return text;
}

/** Reads (including idempotent screen reads) back off on these; actions never retry. */
export function isRetryable(result) {
  if (!result || result.ok) return false;
  if (result.kind === 'network' || result.kind === 'timeout' || result.kind === 'malformed') return true;
  return result.kind === 'http' && [429, 502, 503, 504].includes(result.status);
}

export function backoffDelay(failures, retryAfterMs) {
  if (retryAfterMs != null) return Math.min(Math.max(retryAfterMs, 1000), MAX_RETRY_AFTER_MS);
  return Math.min(MAX_BACKOFF_MS, MIN_BACKOFF_MS * 2 ** Math.max(0, failures - 1));
}

function utf8Length(s) {
  let n = 0;
  for (const ch of s) {
    const c = ch.codePointAt(0);
    n += c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4;
  }
  return n;
}

/** Client timezone for history/event/hit/tools reads, or null when unusable. */
export function clientTimezone(intl = globalThis.Intl) {
  try {
    const tz = intl.DateTimeFormat().resolvedOptions().timeZone;
    if (typeof tz === 'string' && tz && utf8Length(tz) <= 64 && !/[\u0000-\u001f\u007f]/.test(tz)) return tz;
  } catch {
    // fall through
  }
  return null;
}

const READ_OPS = new Set(['history', 'hit', 'event', 'tools']);

/**
 * Convert a native navigation request object to a GET URL (contract `requestUrl`):
 * path /api/<op>; drop op, null/undefined and empty query; numbers as decimals;
 * arrays comma-joined; objects JSON text; reapply tz for read operations.
 */
export function requestUrl(req, tz) {
  if (!req || typeof req !== 'object' || !READ_OPS.has(req.op)) throw new Error('unsupported navigation request');
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(req)) {
    if (key === 'op' || key === 'tz' || value === null || value === undefined) continue;
    if (key === 'query' && value === '') continue;
    if (typeof value === 'number') params.append(key, String(Math.trunc(value)));
    else if (typeof value === 'string') params.append(key, value);
    else if (Array.isArray(value)) params.append(key, value.map((v) => String(v)).join(','));
    else if (typeof value === 'object') params.append(key, JSON.stringify(value));
    else if (typeof value === 'boolean') params.append(key, String(value));
  }
  if (tz) params.append('tz', tz);
  const query = params.toString();
  return `/api/${req.op}${query ? `?${query}` : ''}`;
}

function sameOriginPath(path) {
  return typeof path === 'string' && path.startsWith('/') && !path.startsWith('//') && !path.includes('\\');
}

/**
 * Create the API client. `onUnauthenticated` runs once per auth generation when an
 * authenticated request returns 401; the token is cleared first. Results never
 * throw: `{ok:true,status,data}` or `{ok:false,kind,status,code,message,retryAfterMs}`.
 */
export function createApi({
  fetch,
  tokens,
  config = DEFAULT_CONFIG,
  now = () => Date.now(),
  setTimeout: setTimer = globalThis.setTimeout,
  clearTimeout: clearTimer = globalThis.clearTimeout,
  AbortController: Controller = globalThis.AbortController,
  onUnauthenticated = () => {},
}) {
  let authGeneration = 0;

  function unauthenticated(gen, code) {
    if (gen !== authGeneration) return;
    authGeneration += 1;
    tokens.clear();
    onUnauthenticated({ code });
  }

  async function request(method, path, options = {}) {
    if (!sameOriginPath(path)) throw new Error('same-origin relative path required');
    const { body, signal, auth = true, handleUnauthenticated = true } = options;
    const timeoutMs = options.timeoutMs ?? (method === 'GET' || method === 'HEAD' ? config.timeouts.get_ms : config.timeouts.post_ms);
    const gen = authGeneration;
    const token = auth ? tokens.get() : null;
    const controller = new Controller();
    let timedOut = false;
    const timer = setTimer(() => {
      timedOut = true;
      controller.abort();
    }, timeoutMs);
    const forward = () => controller.abort();
    if (signal) {
      if (signal.aborted) controller.abort();
      else signal.addEventListener('abort', forward, { once: true });
    }
    const done = () => {
      clearTimer(timer);
      if (signal) signal.removeEventListener('abort', forward);
    };
    const failure = (kind, extra = {}) => ({ ok: false, kind, status: 0, code: null, message: null, retryAfterMs: null, authGen: gen, ...extra });
    const headers = { Accept: 'application/json' };
    if (token) headers.Authorization = `Bearer ${token}`;
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    let response;
    let text;
    try {
      response = await fetch(path, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: controller.signal,
        credentials: 'same-origin',
        cache: 'no-store',
        redirect: 'error',
        referrerPolicy: 'no-referrer',
      });
      text = response.status === 204 ? '' : await response.text();
    } catch {
      done();
      return failure(timedOut ? 'timeout' : signal && signal.aborted ? 'aborted' : 'network');
    }
    done();
    const status = response.status;
    if (status === 204) return { ok: true, status, data: null, authGen: gen };
    let data = null;
    let malformed = false;
    if (text) {
      try {
        data = JSON.parse(text);
      } catch {
        malformed = true;
      }
    }
    if (status >= 200 && status < 300) {
      if (malformed || data === null || typeof data !== 'object') return failure('malformed', { status });
      return { ok: true, status, data, authGen: gen };
    }
    const error = data && typeof data === 'object' && data.error && typeof data.error === 'object' ? data.error : {};
    const result = {
      ok: false,
      kind: 'http',
      status,
      code: typeof error.code === 'string' ? error.code : null,
      message: typeof error.message === 'string' ? error.message : null,
      retryAfterMs: parseRetryAfter(response.headers && response.headers.get ? response.headers.get('Retry-After') : null, now()),
      authGen: gen,
    };
    if (status === 401 && auth && handleUnauthenticated) unauthenticated(gen, result.code);
    return result;
  }

  return {
    get authGeneration() {
      return authGeneration;
    },
    /** True when a result was produced under an older auth generation. */
    stale(result) {
      return !result || result.authGen !== authGeneration;
    },
    request,
    get(path, options) {
      return request('GET', path, options);
    },
    read(req, tz, options) {
      return request('GET', requestUrl(req, tz), options);
    },
    post(path, body, options = {}) {
      return request('POST', path, { ...options, body });
    },
    /**
     * Native session check before the app is used. A 401 runs the normal
     * unauthenticated handler; other failures are returned for retry UI.
     */
    session(options) {
      return request('GET', '/api/session', options);
    },
    /**
     * Explicit logout clears local credentials even when the network fails.
     * `confirmed` is true only for an empty 204 (or 401: the token is already dead).
     */
    async logout({ all = false } = {}) {
      const hadToken = !!tokens.get();
      let result = { ok: true, status: 204, data: null };
      if (hadToken) {
        result = await request('POST', all ? '/api/logout-all' : '/api/logout', { body: {}, handleUnauthenticated: false });
      }
      authGeneration += 1;
      tokens.clear();
      const confirmed = !hadToken ? !all : result.status === 204 || result.status === 401;
      return { confirmed, result };
    },
    /** Called after a fresh login to fence results from before it. */
    bumpAuthGeneration() {
      authGeneration += 1;
    },
  };
}

/** Text for logout outcomes, used by the list menu and the login notice. */
export function logoutNotice({ confirmed, all }) {
  if (confirmed) return all ? 'Signed out on every device.' : 'Signed out.';
  return 'Signed out on this device; server revocation is unconfirmed.';
}

/**
 * Before leaving for /login: keep the route (never credentials) so it can be
 * restored after sign-in. Returns false when storage refused it.
 */
export function saveReturn(session, hash) {
  if (typeof hash !== 'string' || !/^#\/[^\s]{0,2046}$/.test(hash)) return session.remove(KEYS.returnHash).ok;
  return session.set(KEYS.returnHash, hash).ok;
}

export function takeReturn(session) {
  const r = session.get(KEYS.returnHash);
  session.remove(KEYS.returnHash);
  const hash = r.ok ? r.value : null;
  return typeof hash === 'string' && /^#\/[^\s]{0,2046}$/.test(hash) ? hash : '';
}

export function setNotice(session, text) {
  return session.set(KEYS.notice, String(text).slice(0, 300)).ok;
}

export function takeNotice(session) {
  const r = session.get(KEYS.notice);
  session.remove(KEYS.notice);
  return r.ok && typeof r.value === 'string' ? r.value.slice(0, 300) : '';
}

/**
 * One poller per resource identity with at most one active request. A manual
 * refresh joins the in-flight request instead of starting another. Completions
 * from a stopped or superseded run are fenced. Hidden documents pause timers and
 * a foreground transition refreshes immediately (honoring a pending Retry-After).
 * Retryable failures back off 2 s -> 30 s, or follow Retry-After.
 */
export function createPoller({
  run,
  intervalMs,
  now = () => Date.now(),
  setTimeout: setTimer = globalThis.setTimeout,
  clearTimeout: clearTimer = globalThis.clearTimeout,
  AbortController: Controller = globalThis.AbortController,
  isHidden = () => false,
  onUpdate = () => {},
  errorDelayMs = MAX_BACKOFF_MS,
}) {
  let timer = null;
  let inflight = null;
  let seq = 0;
  let stopped = false;
  let started = false;
  const state = {
    data: undefined,
    updatedAt: null,
    error: null,
    status: 'idle',
    loading: false,
    failures: 0,
    nextAt: null,
    retryAfterAt: null,
    paused: false,
  };
  const emit = () => onUpdate({ ...state });

  function clear() {
    if (timer !== null) clearTimer(timer);
    timer = null;
  }

  function schedule(delay) {
    clear();
    if (stopped) return;
    if (isHidden()) {
      state.paused = true;
      state.nextAt = null;
      return;
    }
    state.paused = false;
    state.nextAt = now() + delay;
    timer = setTimer(() => {
      timer = null;
      start();
    }, delay);
  }

  function finish(id, result) {
    if (id !== seq || stopped) return state;
    inflight = null;
    state.loading = false;
    if (result && result.ok) {
      state.failures = 0;
      state.data = result.data;
      state.updatedAt = now();
      state.error = null;
      state.status = 'ok';
      state.retryAfterAt = null;
      schedule(intervalMs);
    } else if (result && result.kind === 'aborted') {
      schedule(intervalMs);
    } else if (result && result.kind === 'http' && result.status === 401) {
      state.status = 'unauthenticated';
      state.error = result;
      clear();
      state.nextAt = null;
    } else {
      state.failures += 1;
      state.error = result || { ok: false, kind: 'malformed' };
      if (isRetryable(result)) {
        const delay = backoffDelay(state.failures, result.retryAfterMs);
        state.status = result.kind === 'http' ? 'retrying' : 'offline';
        state.retryAfterAt = result.retryAfterMs != null ? now() + delay : null;
        schedule(delay);
      } else {
        state.status = 'error';
        state.retryAfterAt = null;
        schedule(Math.max(intervalMs, errorDelayMs));
      }
    }
    emit();
    return state;
  }

  function start() {
    if (stopped) return Promise.resolve(state);
    if (inflight) return inflight.promise;
    clear();
    started = true;
    seq += 1;
    const id = seq;
    const controller = new Controller();
    state.loading = true;
    state.nextAt = null;
    emit();
    const promise = Promise.resolve()
      .then(() => run({ signal: controller.signal }))
      .then(
        (result) => finish(id, result),
        () => finish(id, { ok: false, kind: 'network' }),
      );
    inflight = { id, controller, promise };
    return promise;
  }

  return {
    get state() {
      return { ...state };
    },
    start() {
      return start();
    },
    /** Manual refresh: shares the in-flight request, waits out Retry-After. */
    refresh() {
      if (stopped) return Promise.resolve(state);
      if (inflight) return inflight.promise;
      if (state.retryAfterAt !== null && now() < state.retryAfterAt) return Promise.resolve(state);
      return start();
    },
    visibilityChanged() {
      if (stopped || !started) return;
      if (isHidden()) {
        clear();
        state.paused = true;
        state.nextAt = null;
        emit();
        return;
      }
      state.paused = false;
      if (inflight) return;
      if (state.retryAfterAt !== null && now() < state.retryAfterAt) schedule(state.retryAfterAt - now());
      else start();
    },
    stop() {
      stopped = true;
      seq += 1;
      clear();
      if (inflight) inflight.controller.abort();
      inflight = null;
      state.loading = false;
    },
    get stopped() {
      return stopped;
    },
  };
}

/** "Offline · updated 2m ago" style freshness text. */
export function ageText(updatedAt, nowMs) {
  if (updatedAt == null) return 'never updated';
  const s = Math.max(0, Math.round((nowMs - updatedAt) / 1000));
  if (s < 5) return 'just now';
  if (s < 60) return `${s}s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  return `${Math.round(m / 60)}h ago`;
}

export function freshnessText(state, nowMs) {
  if (!state) return '';
  const age = `updated ${ageText(state.updatedAt, nowMs)}`;
  if (state.status === 'offline') return `Offline · ${age}`;
  if (state.status === 'retrying') {
    const wait = state.retryAfterAt ? Math.max(0, Math.ceil((state.retryAfterAt - nowMs) / 1000)) : null;
    const reason = describeError(state.error).replace(/ Try again in \d+ s\.$/, '');
    return `${reason}${wait !== null ? ` Retrying in ${wait}s` : ' Retrying'} · ${age}`;
  }
  if (state.status === 'error') return `${describeError(state.error)} · ${age}`;
  if (state.paused) return `Paused · ${age}`;
  return age;
}

export { utf8Length };
