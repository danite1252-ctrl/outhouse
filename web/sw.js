// Outhouse service worker. It's here so phones can install Outhouse as an
// app and get push notifications; it deliberately caches nothing, so you
// always get the latest version.
self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => e.waitUntil(self.clients.claim()));

// Time tags (<t:UNIX> / <t:UNIX:STYLE>) in a notification, written out in
// this device's own time zone, like they show in chat.
function spellTimeTags(text) {
  if (!text || !text.includes("<t:")) return text;
  return text.replace(/<t:(-?\d{1,13})(?::([tTdDfFR]))?>/g, (whole, unix, style = "f") => {
    const d = new Date(Number(unix) * 1000);
    if (Number.isNaN(d.getTime())) return whole;
    const time = { hour: "numeric", minute: "2-digit" };
    try {
      switch (style) {
        case "t": return d.toLocaleTimeString([], time);
        case "T": return d.toLocaleTimeString([], { ...time, second: "2-digit" });
        case "d": return d.toLocaleDateString([], { day: "numeric", month: "numeric", year: "numeric" });
        case "D": return d.toLocaleDateString([], { day: "numeric", month: "long", year: "numeric" });
        case "F": return d.toLocaleString([], { weekday: "long", day: "numeric", month: "long", year: "numeric", ...time });
        case "R": {
          const secs = Math.round((d.getTime() - Date.now()) / 1000);
          const units = [["year", 31536000], ["month", 2592000], ["week", 604800], ["day", 86400], ["hour", 3600], ["minute", 60]];
          const rtf = new Intl.RelativeTimeFormat([], { numeric: "auto" });
          for (const [unit, size] of units) if (Math.abs(secs) >= size) return rtf.format(Math.round(secs / size), unit);
          return rtf.format(secs, "second");
        }
        default: return d.toLocaleString([], { day: "numeric", month: "long", year: "numeric", ...time });
      }
    } catch {
      return d.toISOString();
    }
  });
}

// A push from the Outhouse server (DMs, mentions, replies, raid notices).
self.addEventListener("push", (e) => {
  let data = {};
  try { data = e.data ? e.data.json() : {}; } catch { data = { body: e.data ? e.data.text() : "" }; }
  const title = spellTimeTags(data.title || "Outhouse");
  e.waitUntil(self.registration.showNotification(title, {
    body: spellTimeTags(data.body || ""),
    icon: "/icons/icon-192.png",
    badge: "/icons/icon-192.png",
    tag: data.tag || undefined,
    renotify: !!data.tag,
    timestamp: Date.now(),
    data: { url: data.url || "/", guild_id: data.guild_id, channel_id: data.channel_id, thread_id: data.thread_id || null, report_id: data.report_id || null },
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
      open.postMessage({ type: "outhouse-open", guild_id: target.guild_id, channel_id: target.channel_id, thread_id: target.thread_id || null, report_id: target.report_id || null });
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
