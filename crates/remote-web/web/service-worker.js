const OFFLINE_CACHE = "miv-remote-offline-v1";
const OFFLINE_URL = "/offline.html";
const OFFLINE_CACHE_PREFIX = "miv-remote-offline-";

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(OFFLINE_CACHE)
      .then((cache) => cache.add(new Request(OFFLINE_URL, { cache: "reload" })))
      .then(() => self.skipWaiting())
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) =>
        Promise.all(
          names
            .filter(
              (name) => name.startsWith(OFFLINE_CACHE_PREFIX) && name !== OFFLINE_CACHE
            )
            .map((name) => caches.delete(name))
        )
      )
      .then(() => self.clients.claim())
  );
});

async function offlineNavigation() {
  const cached = await caches.match(OFFLINE_URL, {
    cacheName: OFFLINE_CACHE,
    ignoreSearch: true,
  });
  return (
    cached ??
    new Response("mIV に接続できません。PC 側のリモート接続を確認してください。", {
      status: 503,
      headers: { "Content-Type": "text/plain; charset=utf-8" },
    })
  );
}

self.addEventListener("fetch", (event) => {
  const request = event.request;
  // Only top-level page navigation has a fallback. Scripts, styles, images,
  // authentication, and every API response always use the network directly.
  if (request.method !== "GET" || request.mode !== "navigate") return;

  event.respondWith(
    fetch(request)
      .then((response) => {
        if (response.status < 500) return response;
        return offlineNavigation();
      })
      .catch(() => offlineNavigation())
  );
});
