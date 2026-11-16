// Permission is requested directly from a tap, before any asynchronous work.
export function pushSupport(win = window, nav = navigator) {
  if (!win.isSecureContext) return 'Open Yinz over HTTPS to enable notifications.';
  const ios = /iPad|iPhone|iPod/.test(nav.userAgent) || (nav.platform === 'MacIntel' && nav.maxTouchPoints > 1);
  if (ios && !nav.standalone && !win.matchMedia('(display-mode: standalone)').matches) {
    return 'To receive notifications on iPhone or iPad, use Share → Add to Home Screen, then open Yinz from its icon.';
  }
  if (!nav.serviceWorker || !win.PushManager || !win.Notification) return 'This browser does not support push notifications. On iPhone or iPad, use iOS 16.4 or later.';
  return '';
}
export function publicKeyBytes(value) {
  const text = atob(value.replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(text, (char) => char.charCodeAt(0));
}
export function registerWorker(nav = navigator) {
  return nav.serviceWorker.register('/static/sw.js', { scope: '/', updateViaCache: 'none' });
}
export function createPush({ api, nav = navigator, win = window, registration }) {
  async function save(subscription) {
    const result = await api.post('/api/push', subscription.toJSON());
    if (!result.ok) throw new Error('Could not save notifications on the server. Try again.');
  }
  return {
    async sync() {
      if (pushSupport(win, nav)) return false;
      const worker = await registration;
      const subscription = await worker.pushManager.getSubscription();
      if (!subscription || win.Notification.permission !== 'granted') return false;
      await save(subscription);
      return true;
    },
    async enable() {
      const unsupported = pushSupport(win, nav);
      if (unsupported) throw new Error(unsupported);
      const permission = await win.Notification.requestPermission();
      if (permission !== 'granted') throw new Error('Notifications are blocked. Allow notifications for Yinz in your device settings.');
      await registration;
      const worker = await nav.serviceWorker.ready;
      const result = await api.get('/api/push');
      if (!result.ok || !result.data.public_key) throw new Error('Could not prepare notifications on the server. Try again.');
      const key = publicKeyBytes(result.data.public_key);
      let subscription = await worker.pushManager.getSubscription();
      if (subscription && subscription.options.applicationServerKey
          && Array.from(new Uint8Array(subscription.options.applicationServerKey)).join() !== Array.from(key).join()) {
        await subscription.unsubscribe();
        subscription = null;
      }
      subscription ||= await worker.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: key });
      await save(subscription);
    },
    async disable() {
      const worker = await registration;
      const subscription = await worker.pushManager.getSubscription();
      if (!subscription) return;
      const result = await api.post('/api/push', { endpoint: subscription.endpoint });
      if (!result.ok) throw new Error('Could not disable notifications on the server. Try again.');
      await subscription.unsubscribe();
    },
  };
}
