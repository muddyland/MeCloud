/// <reference types="@sveltejs/kit" />
import { build, files, version } from '$service-worker';

const CACHE = `jmap-mail-${version}`;
const PRECACHE = [...new Set([...build, ...files])];

self.addEventListener('install', e => {
  e.waitUntil(
    caches.open(CACHE)
      .then(c => c.addAll(PRECACHE))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', e => {
  e.waitUntil(
    caches.keys()
      .then(keys => Promise.all(keys.filter(k => k !== CACHE).map(k => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

self.addEventListener('notificationclick', e => {
  e.notification.close();
  e.waitUntil(
    clients.matchAll({ type: 'window', includeUncontrolled: true }).then(list => {
      const existing = list.find(c => c.url.startsWith(self.location.origin));
      if (existing) return existing.focus();
      // These notifications are about new mail, so open Mail rather than the
      // dashboard that now lives at the root.
      return clients.openWindow('/mail');
    })
  );
});

self.addEventListener('fetch', e => {
  const url = new URL(e.request.url);

  // Never intercept API, auth, or SSE requests — always go to network
  if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/auth/')) return;

  // Navigation: network-first, fall back to cached shell
  if (e.request.mode === 'navigate') {
    e.respondWith(
      fetch(e.request).catch(() =>
        caches.match('/index.html') ?? caches.match('/')
      )
    );
    return;
  }

  // Static assets: cache-first
  e.respondWith(
    caches.match(e.request).then(cached => {
      if (cached) return cached;
      return fetch(e.request).then(res => {
        if (res.ok && res.type === 'basic') {
          const clone = res.clone();
          caches.open(CACHE).then(c => c.put(e.request, clone));
        }
        return res;
      });
    })
  );
});
