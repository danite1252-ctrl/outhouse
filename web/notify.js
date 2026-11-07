// Outhouse: notifications. Web Push (phones and desktops, also when Outhouse
// is closed), per-outhouse notification levels, muted channels, "idle"
// reporting so pushes go out when you're away, and opening the right channel
// from a notification. Loaded after app.js and uses its helpers.

const notify = {
  levels: new Map(),   // guild ID ("0" = DMs) → "all" | "mentions" | "nothing"
  muted: new Set(),    // channel IDs
  mutedInfo: [],       // [{channel_id, guild_id, name}]
  devices: [],
  publicKey: null,
  loaded: false,
};

const LEVEL_LABELS = { all: "All messages", mentions: "Only @mentions and replies", nothing: "Nothing" };
const notifyLevel = (guildId) => notify.levels.get(String(guildId)) || (String(guildId) === "0" ? "all" : "mentions");
const notifyMuted = (channelId) => notify.muted.has(channelId);

async function loadNotifySettings() {
  const s = await api("GET", "/api/users/@me/notifications").catch(() => null);
  if (!s) return null;
  notify.levels = new Map(s.levels.map((l) => [l.guild_id, l.level]));
  notify.muted = new Set(s.muted_channels);
  notify.mutedInfo = s.muted ?? [];
  notify.devices = s.devices;
  notify.publicKey = s.vapid_public_key;
  notify.loaded = true;
  return s;
}

async function setNotifyLevel(guildId, level) {
  const ok = await tryApi("PUT", `/api/users/@me/notifications/guilds/${guildId}`, { level }).then(() => true, () => false);
  if (ok) notify.levels.set(String(guildId), level);
  return ok;
}

async function setChannelMuted(channelId, muted) {
  const ok = await tryApi("PUT", `/api/users/@me/notifications/channels/${channelId}`, { muted }).then(() => true, () => false);
  if (!ok) return false;
  if (muted) notify.muted.add(channelId); else notify.muted.delete(channelId);
  renderChannels();
  return true;
}

// Right-click menu items (app.js adds them to the channel and outhouse menus).
function notifyMenuItems(kind, obj) {
  if (kind === "channel") {
    const muted = notifyMuted(obj.id);
    return [{ label: muted ? "Unmute channel" : "Mute channel", onClick: async () => {
      if (await setChannelMuted(obj.id, !muted)) toast(muted ? `#${obj.name} unmuted.` : `#${obj.name} muted: no notifications from it.`);
    } }];
  }
  if (kind === "guild") {
    const current = notifyLevel(obj.id);
    return [{
      label: "Notifications",
      submenu: Object.entries(LEVEL_LABELS).map(([level, label]) => ({
        label, checked: current === level, keepOpen: false,
        onClick: async () => { if (await setNotifyLevel(obj.id, level)) toast(`${obj.name}: ${label.toLowerCase()}.`); },
      })),
    }];
  }
  return [];
}

// Muted channels look quieter in the channel list.
const notifyBaseRenderChannels = renderChannels;
renderChannels = function () {
  notifyBaseRenderChannels();
  for (const node of document.querySelectorAll("#text-channels .channel[data-id]")) {
    const muted = notifyMuted(node.dataset.id);
    node.classList.toggle("muted-channel", muted);
    if (muted && !node.querySelector(".mute-icon")) node.append(el("span", { class: "mute-icon", title: "Muted" }, "🔕"));
  }
};

// ---------- desktop / Android app notifications follow the same settings ----------

const notifyBaseDesktop = desktopNotifyMessage;
desktopNotifyMessage = function (m, ch) {
  if (!desktop || !ch || document.hasFocus() || m.author_id === state.me?.id) return;
  if (notifyMuted(ch.id)) return;
  const level = notifyLevel(ch.guild_id);
  if (level === "nothing") return;
  if (level === "all" && !mentionsMe(m)) {
    const text = plainMentions(m.content).trim() || (m.attachments?.length ? "Sent an attachment" : "");
    desktopInvoke("notify", { title: `${m.author_display_name} in #${ch.name}`, body: text.slice(0, 200) });
    return;
  }
  notifyBaseDesktop(m, ch);
};

// For dms.js: should a DM pop up a desktop notification?
function notifyAllowsDm(m) {
  return notifyLevel("0") !== "nothing" && !notifyMuted(m.channel_id);
}

// ---------- idle: tell the server when you're not looking ----------
// Push notifications only go out while every Outhouse tab/app you have open
// is idle (hidden for a minute, or no input for 10 minutes); phones count as
// idle as soon as Outhouse goes into the background.

const IDLE_NO_INPUT_MS = 10 * 60 * 1000;
const IDLE_HIDDEN_MS = 60 * 1000;
const coarsePointer = window.matchMedia?.("(pointer: coarse)").matches;
const idle = { lastInput: Date.now(), hiddenSince: document.hidden ? Date.now() : null, sent: false, timer: null };

function isIdleNow() {
  const now = Date.now();
  if (document.hidden && idle.hiddenSince != null && (coarsePointer || now - idle.hiddenSince >= IDLE_HIDDEN_MS)) return true;
  return now - idle.lastInput >= IDLE_NO_INPUT_MS;
}

function syncIdle(force = false) {
  const now = isIdleNow();
  if ((now === idle.sent && !force) || state.ws?.readyState !== 1) return;
  try {
    state.ws.send(JSON.stringify({ op: "IDLE", d: { idle: now } }));
    idle.sent = now;
  } catch { /* reconnecting */ }
}

function noteInput() {
  idle.lastInput = Date.now();
  if (idle.sent && !document.hidden) syncIdle();
}
for (const ev of ["pointerdown", "keydown", "wheel", "touchstart"]) window.addEventListener(ev, noteInput, { passive: true, capture: true });
let lastMove = 0;
window.addEventListener("mousemove", () => { const t = Date.now(); if (t - lastMove > 5000) { lastMove = t; noteInput(); } }, { passive: true });
window.addEventListener("focus", noteInput);
document.addEventListener("visibilitychange", () => {
  idle.hiddenSince = document.hidden ? Date.now() : null;
  if (!document.hidden) idle.lastInput = Date.now();
  syncIdle();
  clearTimeout(idle.timer);
  if (document.hidden && !coarsePointer) idle.timer = setTimeout(syncIdle, IDLE_HIDDEN_MS + 500);
});
window.addEventListener("pagehide", () => { idle.hiddenSince = idle.hiddenSince ?? Date.now(); idle.lastInput = 0; syncIdle(); });
setInterval(syncIdle, 30_000);

// ---------- live updates ----------

const notifyBaseHandle = handleEvent;
handleEvent = function (ev) {
  notifyBaseHandle(ev);
  if (ev.t === "READY") {
    idle.sent = false; // a new connection starts out active
    syncIdle();
    loadNotifySettings().then(() => { renderChannels(); refreshPushSubscription(); });
  }
};

// ---------- opening a channel from a notification ----------

// A notification opened Outhouse at /?guild=…&channel=…: start there.
(() => {
  const q = new URLSearchParams(location.search);
  const guild = q.get("guild"), channel = q.get("channel");
  if (!guild || !/^\d+$/.test(guild) || (channel && !/^\d+$/.test(channel))) return;
  history.replaceState(null, "", location.pathname);
  try {
    if (guild === "0") {
      localStorage.setItem("outhouse_guild", "@home");
      if (channel) state.channelId = channel;
    } else {
      localStorage.setItem("outhouse_guild", guild);
      if (channel) localStorage.setItem(`outhouse_channel_${guild}`, channel);
    }
  } catch { /* storage unavailable */ }
})();

function openTarget({ guild_id, channel_id }) {
  if (!guild_id || !state.me) return;
  document.getElementById("app")?.classList.remove("show-nav", "show-members");
  if (guild_id === "0") return openHome(channel_id || undefined);
  if (!state.guilds.some((g) => g.id === guild_id)) return;
  if (state.guildId === guild_id && channel_id && state.channels.some((c) => c.id === channel_id)) return selectChannel(channel_id);
  selectGuild(guild_id, channel_id || undefined);
}

if ("serviceWorker" in navigator) {
  navigator.serviceWorker.addEventListener("message", (e) => {
    if (e.data?.type === "outhouse-open") openTarget(e.data);
    if (e.data?.type === "outhouse-resubscribe") refreshPushSubscription(true);
  });
}

// ---------- Web Push on this device ----------

const isIos = /iPad|iPhone|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
const isStandalone = window.matchMedia?.("(display-mode: standalone)").matches || navigator.standalone === true;

function pushSupport() {
  if (desktop) return { ok: false, why: window.__OUTHOUSE_PLATFORM__ === "android" ? "android-app" : "desktop-app" };
  if (!window.isSecureContext) return { ok: false, why: "insecure" };
  if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) {
    return { ok: false, why: isIos && !isStandalone ? "ios-browser" : "unsupported" };
  }
  if (!notify.publicKey) return { ok: false, why: "server" };
  return { ok: true };
}

function b64ToBytes(s) {
  const pad = "=".repeat((4 - (s.length % 4)) % 4);
  const raw = atob((s + pad).replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

async function swRegistration() {
  const existing = await navigator.serviceWorker.getRegistration("/");
  if (existing) return existing;
  await navigator.serviceWorker.register("/sw.js");
  return navigator.serviceWorker.ready;
}

async function currentSubscription() {
  if (!pushSupport().ok) return null;
  const reg = await navigator.serviceWorker.getRegistration("/");
  return reg ? reg.pushManager.getSubscription() : null;
}

// Keeps the server's copy of this browser's subscription up to date (or
// re-subscribes after the server's key changed, e.g. a restored backup).
async function refreshPushSubscription(force = false) {
  try {
    const sub = await currentSubscription();
    if (!sub) return;
    const key = sub.options?.applicationServerKey;
    const sameKey = !key || btoa(String.fromCharCode(...new Uint8Array(key))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "") === notify.publicKey;
    if (!sameKey) {
      await sub.unsubscribe();
      if (Notification.permission === "granted") await enablePush(true);
      return;
    }
    if (force || !notify.devices.some((d) => d.endpoint === sub.endpoint)) {
      await api("POST", "/api/push/subscriptions", sub.toJSON());
      await loadNotifySettings();
    }
  } catch { /* not fatal */ }
}

async function enablePush(quiet = false) {
  const support = pushSupport();
  if (!support.ok) { if (!quiet) toast("Push notifications don't work here."); return false; }
  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    if (!quiet) toast(permission === "denied" ? "Notifications are blocked for this site in your browser settings." : "No worries, push stays off.");
    return false;
  }
  try {
    const reg = await swRegistration();
    let sub = await reg.pushManager.getSubscription();
    if (!sub) sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: b64ToBytes(notify.publicKey) });
    await api("POST", "/api/push/subscriptions", sub.toJSON());
    await loadNotifySettings();
    if (!quiet) toast("Push notifications are on for this device.");
    return true;
  } catch (err) {
    if (!quiet) toast(`Couldn't turn on push: ${err.message}`);
    return false;
  }
}

async function disablePush() {
  const sub = await currentSubscription().catch(() => null);
  if (sub) {
    await api("DELETE", "/api/push/subscriptions", { endpoint: sub.endpoint }).catch(() => {});
    await sub.unsubscribe().catch(() => {});
  }
  await loadNotifySettings();
  toast("Push notifications are off for this device.");
}

function deviceLabel(ua) {
  if (!ua) return "A browser";
  const os = /Android/.test(ua) ? "Android" : /iPhone|iPad/.test(ua) ? "iPhone/iPad" : /Windows/.test(ua) ? "Windows" : /Mac OS/.test(ua) ? "Mac" : /Linux/.test(ua) ? "Linux" : "";
  const browser = /Edg\//.test(ua) ? "Edge" : /Firefox\//.test(ua) ? "Firefox" : /SamsungBrowser/.test(ua) ? "Samsung Internet" : /Chrome\//.test(ua) ? "Chrome" : /Safari\//.test(ua) ? "Safari" : "Browser";
  return os ? `${browser} on ${os}` : browser;
}

// ---------- User settings → Notifications ----------

async function drawNotificationsPage(main) {
  await loadNotifySettings();
  const support = pushSupport();
  const sub = support.ok ? await currentSubscription().catch(() => null) : null;
  const here = sub && notify.devices.some((d) => d.endpoint === sub.endpoint);
  const redraw = () => { main.replaceChildren(); drawNotificationsPage(main); };

  const why = {
    "android-app": ["The Android app can't get push notifications while it's closed (Android's built-in web view doesn't support them), so it only notifies you while it's open or recently in the background.",
      `For notifications on your phone at any time, open ${location.host} in Chrome, sign in, tap ⋮ → Install app (or Add to Home screen), then turn on push in Settings → Notifications there.`],
    "desktop-app": ["The Windows app pops up notifications for mentions, replies and DMs while it's running, even when it's hidden in the tray. Nothing to switch on here.",
      "For your phone, open Outhouse in Chrome on the phone and turn on push there."],
    insecure: ["Push notifications need the site to be on HTTPS."],
    "ios-browser": ["On iPhone and iPad, push only works once Outhouse is on your Home Screen: tap Share → Add to Home Screen, open it from there, then turn push on here (iOS 16.4 or newer)."],
    unsupported: ["This browser can't do push notifications. Chrome, Edge, Firefox and Samsung Internet can (on Android, installing Outhouse from Chrome's menu works best)."],
    server: ["Push notifications aren't switched on for this server."],
  };
  const blocked = support.ok && Notification.permission === "denied";
  const pushBox = el("div", { class: "notify-box" },
    el("h3", {}, "Push notifications on this device"),
    support.ok
      ? el("p", { class: "muted small" }, "Get DMs, @mentions, replies and raid reminders on this device when you're not using Outhouse, even with it closed.")
      : null,
    ...(support.ok ? [] : why[support.why].map((t) => el("p", { class: "muted small" }, t))),
    support.ok ? el("div", { class: "push-state" },
      el("span", { class: `push-dot${here ? " on" : ""}` }),
      el("span", {}, blocked ? "Blocked in your browser's site settings" : here ? "On for this device" : "Off for this device")) : null,
    support.ok ? el("div", { class: "inline-row wrap" },
      here
        ? el("button", { class: "ghost", onclick: async () => { await disablePush(); redraw(); } }, "Turn off on this device")
        : el("button", { class: "primary", disabled: blocked, onclick: async () => { if (await enablePush()) redraw(); } }, "Turn on push notifications"),
      notify.devices.length ? el("button", { class: "ghost", onclick: async () => {
        const res = await tryApi("POST", "/api/push/test").catch(() => null);
        if (res) toast(res.sent ? `Test sent to ${res.sent} device${res.sent === 1 ? "" : "s"}. 🚽` : "Couldn't reach any of your devices.");
        if (res?.removed) redraw();
      } }, "Send a test") : null) : null,
  );

  const devices = el("div", { class: "notify-box" },
    el("h3", {}, "Devices with push on"),
    notify.devices.length ? null : el("p", { class: "muted small" }, "None yet."),
    ...notify.devices.map((d) => el("div", { class: "device-row" },
      el("span", { class: "device-icon" }, /Android|iPhone|iPad/.test(d.user_agent) ? "📱" : "💻"),
      el("div", { class: "int-info" },
        el("div", { class: "name" }, deviceLabel(d.user_agent), sub && d.endpoint === sub.endpoint ? el("span", { class: "this-device" }, " (this one)") : null),
        el("div", { class: "muted small" }, `Added ${new Date(d.created_at).toLocaleDateString()}${d.last_ok_at ? ` · last notified ${relTime(d.last_ok_at)}` : ""}`)),
      el("button", { class: "ghost small-btn", onclick: async () => {
        await tryApi("DELETE", "/api/push/subscriptions", { id: d.id }).catch(() => {});
        redraw();
      } }, "Remove"))),
  );

  const levelSelect = (guildId, options) => {
    const sel = el("select", { class: "select", "aria-label": "Notification level" },
      options.map((lv) => el("option", { value: lv, selected: notifyLevel(guildId) === lv }, LEVEL_LABELS[lv])));
    sel.addEventListener("change", async () => {
      const before = notifyLevel(guildId);
      if (!(await setNotifyLevel(guildId, sel.value))) sel.value = before;
    });
    return sel;
  };
  const levels = el("div", { class: "notify-box" },
    el("h3", {}, "What to notify you about"),
    el("p", { class: "muted small" }, "Applies to push notifications and the desktop app. Right-click an outhouse or channel to change it from there too."),
    el("div", { class: "level-row" }, el("span", { class: "level-name" }, "💬 Direct messages"), levelSelect("0", ["all", "nothing"])),
    ...state.guilds.map((g) => el("div", { class: "level-row" },
      el("span", { class: "level-name" }, g.name), levelSelect(g.id, ["all", "mentions", "nothing"]))),
  );

  const mutedRows = notify.mutedInfo.length ? notify.mutedInfo : [...notify.muted].map((id) => ({ channel_id: id, name: state.channels.find((c) => c.id === id)?.name ?? "a channel", guild_id: state.guildId }));
  const muted = el("div", { class: "notify-box" },
    el("h3", {}, "Muted channels"),
    mutedRows.length ? null : el("p", { class: "muted small" }, "None. Right-click a channel → Mute channel to silence it."),
    ...mutedRows.map((m) => el("div", { class: "device-row" },
      el("span", { class: "device-icon" }, "🔕"),
      el("div", { class: "int-info" },
        el("div", { class: "name" }, m.guild_id === "0" ? m.name || "A direct message" : `#${m.name}`),
        el("div", { class: "muted small" }, m.guild_id === "0" ? "Direct messages" : state.guilds.find((g) => g.id === m.guild_id)?.name ?? "")),
      el("button", { class: "ghost small-btn", onclick: async () => { if (await setChannelMuted(m.channel_id, false)) redraw(); } }, "Unmute"))),
  );

  main.append(el("h2", {}, "Notifications"), pushBox, devices, levels, muted);
}
