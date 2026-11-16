import test from 'node:test';
import assert from 'node:assert/strict';
import { staticUrl } from './fakes.mjs';
const { pushSupport, createPush } = await import(staticUrl('push.js'));

function fixture() {
  const calls = [];
  const subscription = { endpoint: 'https://web.push.apple.com/test', toJSON() { return { endpoint: this.endpoint, keys: {} }; }, async unsubscribe() { calls.push('unsubscribe'); } };
  let current = null;
  const worker = { pushManager: { async getSubscription() { return current; }, async subscribe(options) { calls.push(['subscribe', options]); current = subscription; return current; } } };
  const win = { isSecureContext: true, PushManager: {}, matchMedia: () => ({ matches: true }), Notification: { permission: 'granted', requestPermission() { calls.push('permission'); return Promise.resolve('granted'); } } };
  const nav = { userAgent: 'iPhone', standalone: true, serviceWorker: { ready: Promise.resolve(worker) } };
  const api = { async get() { calls.push('get'); return { ok: true, data: { public_key: 'BAUG' } }; }, async post(path, body) { calls.push(['save', path, body]); return { ok: true }; } };
  return { calls, subscription, win, nav, api, registration: Promise.resolve(worker) };
}
test('iOS requires HTTPS and Home Screen installation', () => {
  const f = fixture();
  assert.equal(pushSupport(f.win, f.nav), '');
  f.nav.standalone = false;
  f.win.matchMedia = () => ({ matches: false });
  assert.match(pushSupport(f.win, f.nav), /Add to Home Screen/);
  f.win.isSecureContext = false;
  assert.match(pushSupport(f.win, f.nav), /HTTPS/);
});
test('enable requests permission synchronously from the tap and saves encrypted push subscription', async () => {
  const f = fixture();
  const push = createPush(f);
  const enabling = push.enable();
  assert.deepEqual(f.calls, ['permission']);
  await enabling;
  assert.equal(f.calls[2][1].userVisibleOnly, true);
  assert.deepEqual([...f.calls[2][1].applicationServerKey], [4, 5, 6]);
  assert.equal(await push.sync(), true);
  await push.disable();
  assert.deepEqual(f.calls.at(-2), ['save', '/api/push', { endpoint: f.subscription.endpoint }]);
  assert.equal(f.calls.at(-1), 'unsubscribe');
});
test('denied permission and failed server registration are shown as failures', async () => {
  const f = fixture();
  f.win.Notification.requestPermission = async () => 'denied';
  await assert.rejects(createPush(f).enable(), /blocked/);
  assert.deepEqual(f.calls, []);
  f.win.Notification.requestPermission = async () => 'granted';
  f.api.post = async () => ({ ok: false });
  await assert.rejects(createPush(f).enable(), /Could not save/);
});

test('service worker displays every push and focuses an existing app on notification tap', async () => {
  const { readFile } = await import('node:fs/promises');
  const { runInNewContext } = await import('node:vm');
  const handlers = {};
  const displayed = [];
  const opened = [];
  const client = { url: 'https://yinz.test/#/', async navigate(url) { opened.push(url); }, async focus() { opened.push('focus'); } };
  const self = {
    addEventListener: (type, handler) => { handlers[type] = handler; },
    registration: { async showNotification(title, options) { displayed.push({ title, options }); } },
    location: { origin: 'https://yinz.test' },
    clients: { async matchAll() { return [client]; }, async openWindow(url) { opened.push(url); } },
  };
  runInNewContext(await readFile(new URL(staticUrl('sw.js')), 'utf8'), { self, URL, Response, fetch });
  let waiting;
  handlers.push({ data: { json: () => ({ title: 'session · title', body: 'body', tag: 'unique' }) }, waitUntil: (promise) => { waiting = promise; } });
  await waiting;
  assert.equal(displayed[0].title, 'session · title');
  assert.equal(displayed[0].options.body, 'body');
  handlers.push({ data: { json() { throw new Error('invalid'); } }, waitUntil: (promise) => { waiting = promise; } });
  await waiting;
  assert.equal(displayed[1].title, 'Yinz');
  handlers.notificationclick({ notification: { close() { opened.push('close'); } }, waitUntil: (promise) => { waiting = promise; } });
  await waiting;
  assert.deepEqual(opened, ['close', '/', 'focus']);
});
