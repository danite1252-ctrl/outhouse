// Outhouse service worker. It's here so phones can install Outhouse as an
// app and get push notifications; it deliberately caches nothing, so you
// always get the latest version.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

// A push from the Outhouse server (DMs, mentions, replies, raid notices).
self.addEventListener("push", (e) => {
  let data = {};
  try { data = e.data ? e.data.json() : {}; } catch { data = { body: e.data ? e.data.text() : "" }; }
  const title = data.title || "Outhouse";
  e.waitUntil(self.registration.showNotification(title, {
    body: data.body || "",
    icon: "/icons/icon-192.png",
    badge: "/icons/icon-192.png",
    tag: data.tag || undefined,
    renotify: !!data.tag,
    timestamp: Date.now(),
    data: { url: data.url || "/", guild_id: data.guild_id, channel_id: data.channel_id },
  }));
});

// Tapping a notification opens (or focuses) Outhouse on that channel.
self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const target = e.notification.data || {};
  const url = new URL(target.url || "/", self.location.origin).href;
  e.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    const open = windows.find((w) => new URL(w.url).origin === self.location.origin);
    if (open) {
      open.postMessage({ type: "outhouse-open", guild_id: target.guild_id, channel_id: target.channel_id });
      return open.focus();
    }
    return self.clients.openWindow(url);
  })());
});

// The browser replaced the subscription (keys rotated): tell the server.
self.addEventListener("pushsubscriptionchange", (e) => {
  e.waitUntil((async () => {
    const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
    for (const w of windows) w.postMessage({ type: "outhouse-resubscribe" });
  })());
});
