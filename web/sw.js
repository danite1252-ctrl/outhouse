// Outhouse service worker. It's here so phones can install Outhouse as an
// app; it deliberately caches nothing, so you always get the latest version.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));
