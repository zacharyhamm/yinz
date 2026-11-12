// Node-only doubles for the web frontend tests: manual clock/timers, scripted
// fetch, logging storage and abortable deferreds. No DOM required.
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));
export const STATIC_DIR = path.resolve(here, '../../../assets/plugins/web/static');
export const staticUrl = (rel) => new URL(`file://${path.join(STATIC_DIR, rel)}`).href;

export class FakeClock {
  constructor(start = 1_000_000) {
    this.t = start;
    this.timers = new Map();
    this.nextId = 1;
    this.now = () => this.t;
    this.setTimeout = (fn, ms) => {
      const id = this.nextId++;
      this.timers.set(id, { fn, at: this.t + Math.max(0, ms) });
      return id;
    };
    this.clearTimeout = (id) => {
      this.timers.delete(id);
    };
  }
  pending() {
    return [...this.timers.values()].map((t) => t.at - this.t).sort((a, b) => a - b);
  }
  /** Advance time, firing due timers in order; flush microtasks between them. */
  async advance(ms) {
    const end = this.t + ms;
    for (;;) {
      let due = null;
      for (const [id, t] of this.timers) if (t.at <= end && (!due || t.at < due[1].at)) due = [id, t];
      if (!due) break;
      this.timers.delete(due[0]);
      this.t = due[1].at;
      due[1].fn();
      await flush();
    }
    this.t = end;
    await flush();
  }
}

export async function flush(rounds = 20) {
  for (let i = 0; i < rounds; i += 1) await Promise.resolve();
}

export function response(status, body, headers = {}) {
  const text = body === undefined ? '' : typeof body === 'string' ? body : JSON.stringify(body);
  const h = Object.fromEntries(Object.entries(headers).map(([k, v]) => [k.toLowerCase(), String(v)]));
  return {
    status,
    ok: status >= 200 && status < 300,
    headers: { get: (k) => h[k.toLowerCase()] ?? null },
    text: async () => text,
  };
}

/**
 * Scripted fetch. Each call records {url, init, body} and is answered by the next
 * handler (a response, an Error to reject with, or a function). `hold()` returns a
 * deferred that the test resolves later; aborts reject it like a browser.
 */
export class FakeFetch {
  constructor(log = []) {
    this.calls = [];
    this.queue = [];
    this.log = log;
    this.fetch = (url, init = {}) => this.handle(url, init);
  }
  push(...handlers) {
    this.queue.push(...handlers);
    return this;
  }
  hold() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
      resolve = res;
      reject = rej;
    });
    const d = { promise, resolve, reject, held: true };
    this.queue.push(d);
    return d;
  }
  handle(url, init) {
    const call = { url, init, body: init.body === undefined ? undefined : JSON.parse(init.body) };
    this.calls.push(call);
    this.log.push(['fetch', url]);
    const next = this.queue.shift();
    if (!next) return Promise.reject(new TypeError('no scripted response'));
    if (next.held) {
      if (init.signal) {
        if (init.signal.aborted) next.reject(Object.assign(new Error('aborted'), { name: 'AbortError' }));
        init.signal.addEventListener('abort', () => next.reject(Object.assign(new Error('aborted'), { name: 'AbortError' })));
      }
      return next.promise;
    }
    if (next instanceof Error) return Promise.reject(next);
    if (typeof next === 'function') return Promise.resolve(next(call));
    return Promise.resolve(next);
  }
}

/** Storage double logging writes; `fail` makes operations throw like a browser. */
export class LogStorage {
  constructor(log = [], { fail = null, readback = true } = {}) {
    this.map = new Map();
    this.log = log;
    this.fail = fail; // null | 'quota' | 'denied' | Set of keys | function(key)
    this.readback = readback;
  }
  get length() {
    return this.map.size;
  }
  key(i) {
    return [...this.map.keys()][i] ?? null;
  }
  failing(key) {
    if (!this.fail) return false;
    if (typeof this.fail === 'function') return this.fail(key);
    if (this.fail instanceof Set) return this.fail.has(key);
    return true;
  }
  getItem(k) {
    if (this.fail === 'denied') throw Object.assign(new Error('denied'), { name: 'SecurityError' });
    return this.map.has(k) ? this.map.get(k) : null;
  }
  setItem(k, v) {
    this.log.push(['set', k]);
    if (this.failing(k)) {
      throw Object.assign(new Error('quota'), { name: this.fail === 'denied' ? 'SecurityError' : 'QuotaExceededError' });
    }
    if (this.readback) this.map.set(k, String(v));
  }
  removeItem(k) {
    this.log.push(['remove', k]);
    if (this.fail === 'denied') throw Object.assign(new Error('denied'), { name: 'SecurityError' });
    this.map.delete(k);
  }
}
