// The todai web app's service worker: makes the app installable and lets it
// open without a connection (it then says it's offline and retries). Only the
// app's own files are cached, network first so a new server version shows up
// at once; /api is never touched, so board data is never stored here.
// Notifications are shown by the page through this worker (Android needs
// that), and a tap on one opens the board it is about.

const CACHE = "todai-shell-v1";
const SHELL = ["/", "/app.js", "/markdown.js", "/app.css", "/icon.svg", "/manifest.webmanifest"];

self.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (e) => {
  e.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (e) => {
  const url = new URL(e.request.url);
  if (e.request.method !== "GET" || url.origin !== location.origin || !SHELL.includes(url.pathname)) return;
  e.respondWith(
    fetch(e.request)
      .then((res) => {
        if (res.ok) {
          const copy = res.clone();
          caches.open(CACHE).then((c) => c.put(e.request, copy));
        }
        return res;
      })
      .catch(() => caches.match(e.request)),
  );
});

self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const target = e.notification.data?.board ? `/#/b/${e.notification.data.board}` : "/#/";
  e.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((list) => {
      for (const c of list) {
        if ("focus" in c) {
          c.postMessage({ open: target });
          return c.focus();
        }
      }
      return self.clients.openWindow(target);
    }),
  );
});
