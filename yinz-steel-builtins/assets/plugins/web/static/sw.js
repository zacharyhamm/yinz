// Never cache authenticated pages, credentials, agent output, or API responses.
self.addEventListener('install', (event) => event.waitUntil(self.skipWaiting()));
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));
self.addEventListener('fetch', (event) => {
  if (event.request.mode !== 'navigate') return;
  event.respondWith(fetch(event.request).catch(() => new Response(
    '<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Yinz is offline</title><h1>Yinz is offline</h1><p>Reconnect to your network and open Yinz again.</p><a href="/">Try again</a></html>',
    { status: 503, headers: { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' } },
  )));
});
self.addEventListener('push', (event) => {
  let data = {};
  try { data = event.data ? event.data.json() : {}; } catch { /* Still display every push. */ }
  event.waitUntil(self.registration.showNotification(
    typeof data.title === 'string' && data.title ? data.title : 'Yinz',
    {
      body: typeof data.body === 'string' ? data.body : 'A session has a new notification.',
      icon: '/static/icon-192.png',
      tag: typeof data.tag === 'string' ? data.tag : undefined,
      data: { url: '/' },
    },
  ));
});
self.addEventListener('notificationclick', (event) => {
  event.notification.close();
  event.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: 'window', includeUncontrolled: true });
    const existing = windows.find((client) => new URL(client.url).origin === self.location.origin);
    if (existing) {
      await existing.navigate('/');
      await existing.focus();
    } else {
      await self.clients.openWindow('/');
    }
  })());
});
