// CrossAgents Mobile PWA Service Worker:
// 1. Caches the static app shell for instant cold-start on iOS Home Screen.
// 2. Receives free Web Push (VAPID) notifications from the CrossAgents Hub and shows a
//    privacy-safe native iOS lock-screen alert.

const SHELL_CACHE = "crossagents-mobile-shell-v10";
// Content-hashed immutable assets (/assets/*.js, *.css, *.wasm) live in a dedicated cache so
// UI/JS-only updates never evict the cached WASM binary and force a slow re-download.
const ASSET_CACHE = "crossagents-mobile-assets-v1";
const SHELL_URLS = ["./", "./index.html", "./manifest.webmanifest", "./icon-192.png", "./icon-512.png"];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(SHELL_CACHE)
      .then((cache) =>
        Promise.all(
          SHELL_URLS.map((u) =>
            fetch(new Request(u, { cache: "no-cache" }))
              .then((res) => (res.ok ? cache.put(u, res) : undefined))
              .catch(() => {}),
          ),
        ),
      )
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((k) => k !== SHELL_CACHE && k !== ASSET_CACHE)
            .map((k) => caches.delete(k)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin) return;

  // Navigation requests: bypass HTTP disk cache (GitHub Pages sends max-age=600) so new
  // deployments appear immediately on reopen, falling back to SW cache only when offline.
  if (req.mode === "navigate") {
    event.respondWith(
      fetch(new Request(req, { cache: "no-cache" }))
        .then((res) => {
          if (res.ok) {
            const clone = res.clone();
            caches.open(SHELL_CACHE).then((cache) => cache.put("./index.html", clone)).catch(() => {});
          }
          return res;
        })
        .catch(() => caches.match("./index.html")),
    );
    return;
  }

  // Hashed static assets & WASM: cache-first in persistent ASSET_CACHE for instant cold-start.
  if (url.pathname.includes("/assets/") || url.pathname.endsWith(".wasm") || url.pathname.endsWith(".png")) {
    event.respondWith(
      caches.open(ASSET_CACHE).then((cache) =>
        cache.match(req).then((cached) => {
          if (cached) return cached;
          return fetch(req).then((res) => {
            if (res.ok) {
              const clone = res.clone();
              cache.put(req, clone).catch(() => {});
            }
            return res;
          });
        }),
      ),
    );
  }
});

// Every incoming Web Push MUST display a visible notification (iOS revokes subscriptions on silent push).
self.addEventListener("push", (event) => {
  let payload = {
    title: "CrossAgents",
    body: "有一项待你处理的提问或审批",
    tag: "crossagents-notice",
    question_id: "",
    agent_id: "",
  };
  if (event.data) {
    try {
      const parsed = event.data.json();
      payload = { ...payload, ...parsed };
    } catch {
      const text = event.data.text();
      if (text) payload.body = text;
    }
  }

  event.waitUntil(
    self.registration.showNotification(payload.title || "CrossAgents", {
      body: payload.body || "有一项待你处理的提问或审批",
      icon: "./icon-192.png",
      badge: "./icon-192.png",
      tag: payload.tag || "crossagents-notice",
      renotify: true,
      data: {
        question_id: payload.question_id || "",
        agent_id: payload.agent_id || "",
      },
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const data = event.notification.data || {};
  const targetHash = data.question_id ? `#inbox?q=${encodeURIComponent(data.question_id)}` : "#inbox";

  event.waitUntil(
    self.clients.matchAll({ type: "window", includeUncontrolled: true }).then((clients) => {
      for (const client of clients) {
        if ("focus" in client) {
          client.postMessage({
            type: "OPEN_QUESTION",
            question_id: data.question_id || "",
            agent_id: data.agent_id || "",
          });
          return client.focus();
        }
      }
      if (self.clients.openWindow) {
        return self.clients.openWindow(`./${targetHash}`);
      }
      return undefined;
    }),
  );
});
