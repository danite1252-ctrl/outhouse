// Outhouse web client: plain JavaScript, no build step.
"use strict";

const AVATAR_COLOURS = ["#ff6b35", "#ffb347", "#fbbf24", "#f2917a", "#4ade80", "#a1a1aa"];
const TEXT = 0;
const VOICE = 2;
const QUICK_REACTIONS = ["👍", "😂", "❤️", "🔥", "😮", "😢"];
const EMOJI = [
  "😀", "😂", "🤣", "😊", "😍", "😎", "🤔", "😮", "😢", "😭", "😡", "🥳",
  "👍", "👎", "👏", "🙌", "🙏", "💪", "👀", "🤝", "✅", "❌", "⚠️", "💯",
  "❤️", "🧡", "💛", "💚", "💙", "💜", "🔥", "✨", "🎉", "🏆", "🎯", "💀",
  "⚔️", "🛡️", "🏹", "🪄", "🐉", "💎", "🍺", "☕", "🍕", "🚽", "🧻", "📞",
];
const TIMEOUTS = [["5 minutes", 5], ["1 hour", 60], ["1 day", 1440], ["1 week", 10080]];

const state = {
  token: localStorage.getItem("outhouse_token"),
  me: null,
  guilds: [],
  guildId: null,
  channels: [],
  channelId: null,
  messages: [],
  members: [],
  roles: [],
  permList: [],
  P: {},
  myPerms: { guild: 0n, channels: {}, top: 0 },
  ws: null,
  heartbeat: null,
  reconnectDelay: 1000,
  replyTo: null,
  editingId: null,
  editDraft: "",
  pendingFiles: [],
  typing: new Map(), // channelId -> Map(userId -> {name, until})
  lastTypingSent: 0,
  unreadGuilds: new Set(),
  voiceStates: new Map(), // userId -> voice state, for the open outhouse
  sounds: [],
  voice: {
    room: null,
    channelId: null,
    guildId: null,
    connecting: false,
    muted: false,
    deafened: false,
    canSpeak: true,
    pttDown: false,
    speaking: new Set(),
    serverMuted: false,   // muted by a moderator
    serverDeafened: false,
    canStream: false,     // allowed to share their screen here
    sharing: false,       // sharing their screen right now
  },
};

// Voice preferences live in this browser only.
const prefs = {
  get(key, fallback) {
    try { const v = localStorage.getItem(`outhouse_${key}`); return v === null ? fallback : JSON.parse(v); } catch { return fallback; }
  },
  set(key, value) {
    try { localStorage.setItem(`outhouse_${key}`, JSON.stringify(value)); } catch { /* storage unavailable */ }
  },
};

const $ = (id) => document.getElementById(id);

// ---------- small helpers ----------

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k === "style") node.style.cssText = v;
    else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
    else if (v === true) node.setAttribute(k, "");
    else if (v !== false && v != null) node.setAttribute(k, v);
  }
  for (const c of children.flat()) {
    if (c == null || c === false || c === "") continue;
    node.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return node;
}

// append() and friends turn null into the text "null" (and false into
// "false"): leave out empty bits, like el() does, so `cond ? node : null`
// is always safe.
for (const proto of [Element.prototype, DocumentFragment.prototype]) {
  for (const name of ["append", "prepend", "replaceChildren", "before", "after"]) {
    const orig = proto[name];
    if (typeof orig !== "function") continue;
    proto[name] = function (...nodes) {
      return orig.apply(this, nodes.filter((n) => n != null && n !== false));
    };
  }
}

const big = (s) => BigInt(s || 0);
const idGt = (a, b) => big(a) > big(b);

function colourFor(id) {
  let h = 0;
  for (const ch of String(id)) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
  return AVATAR_COLOURS[h % AVATAR_COLOURS.length];
}

function hexColour(n) {
  return n ? "#" + Number(n).toString(16).padStart(6, "0") : null;
}

function initials(name) {
  return name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => [...w][0].toUpperCase()).join("");
}

// A picture if they've uploaded one, otherwise their first letter on a colour.
function avatarFor(id, name, size = 40, url) {
  const style = `width:${size}px;height:${size}px;flex-basis:${size}px`;
  const letter = () => {
    const node = el("div", { class: "avatar", style }, [...(name || "?").trim()][0]?.toUpperCase() || "?");
    node.style.background = colourFor(id);
    return node;
  };
  const src = url ?? memberById(id)?.avatar_url ?? (state.me && id === state.me.id ? state.me.avatar_url : null);
  if (!src) return letter();
  const img = el("img", { class: "avatar img", src, alt: "", style });
  img.addEventListener("error", () => img.replaceWith(letter()));
  return img;
}

function toast(text) {
  const t = $("toast");
  t.textContent = text;
  t.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => (t.hidden = true), 3500);
}

function fmtSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

async function copy(text, what = "Copied") {
  try {
    await navigator.clipboard.writeText(text);
    toast(`${what} to clipboard.`);
  } catch {
    toast("Couldn't copy.");
  }
}

async function api(method, path, body) {
  const isForm = body instanceof FormData;
  const res = await fetch(path, {
    method,
    headers: {
      ...(body && !isForm ? { "Content-Type": "application/json" } : {}),
      ...(state.token ? { Authorization: `Bearer ${state.token}` } : {}),
    },
    body: body ? (isForm ? body : JSON.stringify(body)) : undefined,
  });
  if (res.status === 401 && state.token) {
    signOut(false);
    throw new Error("Your session expired. Sign in again.");
  }
  const data = res.status === 204 ? null : await res.json().catch(() => null);
  if (!res.ok) throw new Error((data && data.error) || `Request failed (${res.status})`);
  return data;
}

const tryApi = (...args) => api(...args).catch((err) => { toast(err.message); throw err; });

function timeLabel(iso) {
  const d = new Date(iso);
  const today = new Date();
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return d.toDateString() === today.toDateString() ? `Today at ${time}` : `${d.toLocaleDateString()} ${time}`;
}

// ---------- permissions ----------

function can(key, channelId) {
  const bit = state.P[key];
  if (bit == null) return false;
  const bits = channelId ? big(state.myPerms.channels[channelId]) : state.myPerms.guild;
  return (bits & bit) === bit;
}

const currentGuild = () => state.guilds.find((g) => g.id === state.guildId);
const isOwner = () => currentGuild()?.owner_id === state.me?.id;
const roleById = (id) => state.roles.find((r) => r.id === id);
const memberById = (id) => state.members.find((m) => m.id === id);

function memberTop(m) {
  if (!m) return 0;
  if (m.id === currentGuild()?.owner_id) return Infinity;
  return Math.max(0, ...m.role_ids.map((id) => roleById(id)?.position ?? 0));
}

function myTop() {
  return isOwner() ? Infinity : state.myPerms.top;
}

function canModerate(member, key) {
  return can(key) && member.id !== state.me.id && member.id !== currentGuild()?.owner_id && myTop() > memberTop(member);
}

function memberColour(userId) {
  const m = memberById(userId);
  if (!m) return null;
  const coloured = m.role_ids.map(roleById).filter((r) => r && r.color).sort((a, b) => b.position - a.position);
  return coloured.length ? hexColour(coloured[0].color) : null;
}

function isTimedOut(m) {
  return m.timeout_until && new Date(m.timeout_until) > new Date();
}

// ---------- context menus and popovers ----------

let openMenus = [];

function closeMenus(fromDepth = 0) {
  while (openMenus.length > fromDepth) openMenus.pop().remove();
}

function placeFixed(node, x, y) {
  document.body.append(node);
  const r = node.getBoundingClientRect();
  const left = Math.max(8, Math.min(x, window.innerWidth - r.width - 8));
  const top = Math.max(8, Math.min(y, window.innerHeight - r.height - 8));
  node.style.left = `${left}px`;
  node.style.top = `${top}px`;
}

// items: {label, onClick, danger, disabled, checked, swatch, submenu} | "sep" | {quick, onPick}
function openMenu(x, y, items, depth = 0) {
  closeMenus(depth);
  const menu = el("div", { class: "menu", role: "menu" });
  const visible = items.filter(Boolean);
  // Drop leading, trailing and doubled separators.
  const cleaned = visible.filter((it, i) => it !== "sep" || (i > 0 && i < visible.length - 1 && visible[i - 1] !== "sep"));
  for (const item of cleaned) {
    if (item === "sep") {
      menu.append(el("div", { class: "sep" }));
    } else if (item.custom) {
      menu.append(item.custom);
    } else if (item.quick) {
      menu.append(el("div", { class: "quick" },
        item.quick.map((e) => el("button", { title: e, onclick: () => { closeMenus(); item.onPick(e); } }, e))));
    } else {
      const btn = el("button", { role: "menuitem", class: item.danger ? "danger" : "", disabled: !!item.disabled },
        el("span", {},
          item.swatch ? el("span", { class: "swatch", style: `background:${item.swatch}` }) : null,
          item.label),
        item.submenu ? "›" : item.checked != null ? el("span", { class: `check${item.checked ? " on" : ""}` }, item.checked ? "✓" : "") : null,
      );
      if (item.submenu) {
        const openSub = () => {
          menu.querySelectorAll("button.open").forEach((b) => b.classList.remove("open"));
          btn.classList.add("open");
          const r = btn.getBoundingClientRect();
          const sub = openMenu(r.right + 4, r.top - 6, item.submenu, depth + 1);
          if (r.right + sub.offsetWidth + 8 > window.innerWidth) sub.style.left = `${r.left - sub.offsetWidth - 4}px`;
        };
        btn.addEventListener("mouseenter", openSub);
        btn.addEventListener("click", openSub);
      } else {
        btn.addEventListener("mouseenter", () => closeMenus(depth + 1));
        btn.addEventListener("click", () => {
          if (item.keepOpen) { item.onClick(); return; }
          closeMenus();
          item.onClick();
        });
      }
      menu.append(btn);
    }
  }
  placeFixed(menu, x, y);
  openMenus.push(menu);
  return menu;
}

let popover = null;
function closePopover() {
  if (popover) popover.remove();
  popover = null;
}

function openPopover(anchor, content, align = "above") {
  closePopover();
  closeMenus();
  popover = el("div", { class: "popover" }, content);
  const r = anchor.getBoundingClientRect();
  document.body.append(popover);
  const pr = popover.getBoundingClientRect();
  const x = Math.min(r.right - pr.width, window.innerWidth - pr.width - 8);
  const y = align === "above" ? r.top - pr.height - 8 : r.bottom + 8;
  placeFixed(popover, Math.max(8, x), Math.max(8, y));
  return popover;
}

document.addEventListener("mousedown", (e) => {
  if (openMenus.length && !openMenus.some((m) => m.contains(e.target))) closeMenus();
  if (popover && !popover.contains(e.target) && !e.target.closest("#gif-btn, #emoji-btn, .msg-actions, .menu, .vp-btn, .vp-status")) closePopover();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    closeMenus();
    closePopover();
    document.querySelector(".lightbox")?.remove();
  }
});
window.addEventListener("resize", () => {
  closeMenus();
  // The soundboard's bottom sheet on phones survives the on-screen keyboard.
  if (!(popover?.classList.contains("sb-pop") && window.innerWidth <= 760)) closePopover();
});
window.addEventListener("blur", () => closeMenus());

// Replace the browser's right-click menu inside the app (text boxes keep theirs).
document.addEventListener("contextmenu", (e) => {
  if ($("app").hidden) return;
  if (e.target.closest("input, textarea")) return;
  e.preventDefault();
  const target = e.target.closest("[data-menu]");
  if (!target) return;
  const handler = MENUS[target.dataset.menu];
  if (handler) handler(e, target);
});

// `forText`: custom emoji go in as :name: (the server swaps in the picture).
function emojiPicker(anchor, onPick, { forText = false } = {}) {
  const grid = el("div", { class: "emoji-grid" },
    EMOJI.map((e) => el("button", { title: e, onclick: () => { closePopover(); onPick(e); } }, e)));
  openPopover(anchor, typeof customEmojiTabs === "function" ? customEmojiTabs(grid, onPick, forText) : grid);
}

// Simple dialog returning the entered text (or null).
function ask(title, { placeholder = "", value = "", confirm = "OK", readOnly = false, body = null, allowEmpty = false } = {}) {
  return new Promise((resolve) => {
    const input = el("input", { placeholder, value, readonly: readOnly });
    const dialog = el("dialog", {},
      el("form", { method: "dialog" },
        el("h2", {}, title),
        body,
        input,
        el("div", { class: "row" },
          el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, readOnly ? "Close" : "Cancel"),
          readOnly ? null : el("button", { type: "submit", class: "primary", value: "ok" }, confirm),
        ),
      ),
    );
    dialog.addEventListener("close", () => {
      const ok = dialog.returnValue === "ok";
      const v = input.value.trim();
      dialog.remove();
      resolve(!ok ? null : allowEmpty ? v : v || null);
    });
    document.body.append(dialog);
    dialog.showModal();
    input.focus();
    if (readOnly) input.select();
  });
}

function confirmBox(title, message, confirm = "Confirm") {
  return new Promise((resolve) => {
    const dialog = el("dialog", {},
      el("form", { method: "dialog" },
        el("h2", {}, title),
        el("p", { class: "muted", style: "margin:0" }, message),
        el("div", { class: "row" },
          el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
          el("button", { type: "submit", class: "primary", value: "ok" }, confirm),
        ),
      ),
    );
    dialog.addEventListener("close", () => { dialog.remove(); resolve(dialog.returnValue === "ok"); });
    document.body.append(dialog);
    dialog.showModal();
  });
}

// ---------- auth ----------

let registering = false;

function showAuth() {
  $("app").hidden = true;
  $("auth").hidden = false;
}

function setAuthMode(isRegister) {
  registering = isRegister;
  $("display-name-field").hidden = !isRegister;
  $("auth-submit").textContent = isRegister ? "Create account" : "Sign in";
  $("auth-toggle").textContent = isRegister ? "Already have an account? Sign in" : "New here? Create an account";
  $("auth-subtitle").textContent = isRegister ? "Grab a spot out the back." : "Welcome back. Pull up a seat.";
  $("auth-error").textContent = "";
}

$("auth-toggle").addEventListener("click", () => setAuthMode(!registering));

$("auth-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(e.target);
  const body = { username: form.get("username"), password: form.get("password") };
  if (registering) body.display_name = form.get("display_name") || null;
  $("auth-error").textContent = "";
  try {
    const res = await api("POST", registering ? "/api/auth/register" : "/api/auth/login", body);
    state.token = res.token;
    localStorage.setItem("outhouse_token", res.token);
    e.target.reset();
    start();
  } catch (err) {
    $("auth-error").textContent = err.message;
  }
});

async function signOut(callServer = true) {
  await leaveVoice(callServer).catch(() => {});
  if (callServer) await api("POST", "/api/auth/logout").catch(() => {});
  localStorage.removeItem("outhouse_token");
  state.token = null;
  if (state.ws) {
    state.ws.onclose = null;
    state.ws.close();
  }
  clearInterval(state.heartbeat);
  showAuth();
}

$("logout").addEventListener("click", () => signOut());
$("user-settings").addEventListener("click", () => openUserSettings());

// ---------- gateway ----------

function connect() {
  const proto = location.protocol === "https:" ? "wss" : "ws";
  const ws = new WebSocket(`${proto}://${location.host}/api/gateway`);
  state.ws = ws;
  ws.onopen = () => ws.send(JSON.stringify({ op: "IDENTIFY", d: { token: state.token } }));
  ws.onmessage = (e) => handleEvent(JSON.parse(e.data));
  ws.onclose = () => {
    $("conn-status").classList.remove("up");
    clearInterval(state.heartbeat);
    if (!state.token) return;
    setTimeout(connect, state.reconnectDelay);
    state.reconnectDelay = Math.min(state.reconnectDelay * 2, 30000);
  };
}

const findMessage = (id) => state.messages.find((m) => m.id === id);

function handleEvent({ t, d }) {
  switch (t) {
    case "READY": {
      state.reconnectDelay = 1000;
      $("conn-status").classList.add("up");
      clearInterval(state.heartbeat);
      state.heartbeat = setInterval(() => state.ws.send(JSON.stringify({ op: "HEARTBEAT" })), 30000);
      state.me = d.user;
      state.guilds = d.guilds;
      api("GET", "/api/users/@me/site-owner").then((r) => { state.siteOwner = !!r?.site_owner; }).catch(() => { state.siteOwner = false; });
      renderMe();
      renderGuilds();
      loadUnread();
      onReady();
      const saved = localStorage.getItem("outhouse_guild");
      const pick = state.guilds.find((g) => g.id === (state.guildId || saved)) || state.guilds[0];
      if (state.home || (saved === "@home" && !state.guildId)) openHome(state.channelId);
      else if (pick) selectGuild(pick.id, state.channelId);
      else renderEmpty();
      break;
    }
    case "GUILD_CREATE":
      if (!state.guilds.some((g) => g.id === d.id)) state.guilds.push(d);
      renderGuilds();
      loadUnread();
      break;
    case "GUILD_DELETE": {
      const g = state.guilds.find((x) => x.id === d.guild_id);
      state.guilds = state.guilds.filter((x) => x.id !== d.guild_id);
      if (g && state.guildId === d.guild_id) {
        toast(`You're no longer in ${g.name}.`);
        state.guildId = null;
        if (state.guilds[0]) selectGuild(state.guilds[0].id);
        else openHome();
      }
      renderGuilds();
      loadUnread();
      break;
    }
    case "GUILD_REFRESH":
      if (d.guild_id === state.guildId) refreshGuild();
      loadUnread();
      break;
    case "CHANNEL_CREATE":
      if (d.guild_id === state.guildId && !state.channels.some((c) => c.id === d.id)) {
        state.channels.push(d);
        refreshGuild();
      }
      loadUnread();
      break;
    case "CHANNEL_DELETE":
      if (d.guild_id === state.guildId) {
        state.channels = state.channels.filter((c) => c.id !== d.id);
        if (state.channelId === d.id) {
          const first = state.channels.find((c) => c.kind === TEXT);
          if (first) selectChannel(first.id);
        }
        renderChannels();
      }
      loadUnread();
      break;
    case "MESSAGE_CREATE": {
      clearTyping(d.channel_id, d.author_id);
      const ch = state.channels.find((c) => c.id === d.channel_id);
      desktopNotifyMessage(d, ch);
      if (ch) ch.last_message_id = d.id;
      if (d.author_id === state.me.id && ch) ch.last_read_id = d.id;
      noteUnreadMessage(d);
      // While looking at older messages (from search or pins) new ones wait for "Jump to present".
      if (d.channel_id === state.channelId && !state.detached) {
        if (!findMessage(d.id)) {
          state.messages.push(d);
          renderMessages();
          if (document.hasFocus()) ackChannel();
        }
      }
      renderChannels();
      break;
    }
    case "MESSAGE_UPDATE": {
      const i = state.messages.findIndex((m) => m.id === d.id);
      if (i >= 0) {
        state.messages[i] = d;
        renderMessages();
      }
      onMessageChanged(d.channel_id);
      break;
    }
    case "MESSAGE_DELETE":
      if (d.channel_id === state.channelId) {
        state.messages = state.messages.filter((m) => m.id !== d.id);
        renderMessages();
      }
      onMessageChanged(d.channel_id);
      break;
    case "CHANNEL_ACK":
      onChannelAck(d);
      break;
    case "REACTION_ADD":
    case "REACTION_REMOVE": {
      const m = d.channel_id === state.channelId && findMessage(d.message_id);
      if (!m) break;
      let r = m.reactions.find((x) => x.emoji === d.emoji);
      if (t === "REACTION_ADD") {
        if (!r) m.reactions.push((r = { emoji: d.emoji, user_ids: [] }));
        if (!r.user_ids.includes(d.user_id)) r.user_ids.push(d.user_id);
      } else if (r) {
        r.user_ids = r.user_ids.filter((u) => u !== d.user_id);
        if (!r.user_ids.length) m.reactions = m.reactions.filter((x) => x !== r);
      }
      renderMessages();
      break;
    }
    case "TYPING_START":
      if (d.user_id !== state.me.id) {
        if (!state.typing.has(d.channel_id)) state.typing.set(d.channel_id, new Map());
        state.typing.get(d.channel_id).set(d.user_id, { name: d.display_name, until: Date.now() + 9000 });
        renderTyping();
      }
      break;
    case "MEMBER_JOIN":
      if (d.guild_id === state.guildId && !memberById(d.member.id)) {
        state.members.push(d.member);
        renderMembers();
        toast(`${d.member.display_name} wandered in.`);
      }
      break;
    case "MEMBER_LEAVE":
      if (d.guild_id === state.guildId) {
        state.members = state.members.filter((m) => m.id !== d.user_id);
        renderMembers();
      }
      break;
    case "PRESENCE_UPDATE": {
      const m = memberById(d.user_id);
      if (m && m.online !== d.online) {
        m.online = d.online;
        renderMembers();
      }
      break;
    }
    case "VOICE_STATE_UPDATE":
      if (d.user_id === state.me.id && typeof voiceSelfUpdate === "function") voiceSelfUpdate(d);
      if (d.guild_id === state.guildId) {
        const before = state.voiceStates.get(d.user_id);
        if (d.channel_id) state.voiceStates.set(d.user_id, d);
        else state.voiceStates.delete(d.user_id);
        if (state.voice.channelId && d.user_id !== state.me.id) {
          const joined = d.channel_id === state.voice.channelId && before?.channel_id !== state.voice.channelId;
          const left = before?.channel_id === state.voice.channelId && d.channel_id !== state.voice.channelId;
          if (joined) chime(660, 880);
          if (left) chime(880, 520);
        }
        renderChannels();
      }
      break;
    case "SOUNDBOARD_PLAY":
      if (d.channel_id === state.voice.channelId && !isLocallyMuted(d.user_id)) playSound(d.url, d.volume);
      break;
    case "VOICE_MOVE":
      if (typeof onVoiceMove === "function") onVoiceMove(d);
      break;
    case "VOICE_KICKED":
      if (typeof onVoiceKicked === "function") onVoiceKicked(d);
      break;
    case "ENTRANCE_SOUND":
      if (typeof onEntranceSound === "function") onEntranceSound(d);
      break;
    case "SOUNDS_CHANGED":
      if (d.guild_id === state.guildId) loadSounds();
      if (typeof onSoundsChanged === "function") onSoundsChanged(d);
      break;
    case "COMMANDS_CHANGED":
      if (d.guild_id === state.guildId) loadCommands();
      break;
    case "INVALID_SESSION":
      signOut(false);
      break;
  }
}

let ackTimer = null;
function ackChannel() {
  clearTimeout(ackTimer);
  ackTimer = setTimeout(() => {
    const ch = state.channels.find((c) => c.id === state.channelId);
    const last = state.messages[state.messages.length - 1];
    if (!ch || !last || (ch.last_read_id && !idGt(last.id, ch.last_read_id))) return;
    ch.last_read_id = last.id;
    noteChannelRead(ch.id, last.id);
    api("POST", `/api/channels/${ch.id}/ack`, { message_id: last.id }).catch(() => {});
    renderChannels();
  }, 400);
}
window.addEventListener("focus", ackChannel);

// ---------- rendering: rail, channels, members ----------

function renderMe() {
  $("me-name").textContent = state.me.display_name;
  const a = $("me-avatar");
  const url = state.me.avatar_url;
  a.textContent = url ? "" : [...state.me.display_name][0].toUpperCase();
  a.style.background = url ? `center / cover no-repeat url("${url}")` : colourFor(state.me.id);
  a.style.cursor = "pointer";
  a.onclick = (e) => openProfile(state.me.id, e.currentTarget);
}

function renderGuilds() {
  $("guild-list").replaceChildren(
    ...state.guilds.map((g) =>
      el("button", {
        class: `guild${g.id === state.guildId ? " active" : ""}`,
        title: g.name,
        "aria-label": g.name,
        "data-menu": "guild",
        "data-id": g.id,
        onclick: () => selectGuild(g.id),
      }, initials(g.name), guildBadge(g.id)),
    ),
  );
}

function renderEmpty() {
  $("guild-name").textContent = "No outhouse yet";
  for (const id of ["text-channels", "voice-channels", "member-list"]) $(id).replaceChildren();
  $("invite-btn").hidden = true;
  $("new-channel").hidden = true;
  $("composer-input").disabled = true;
  $("channel-name").textContent = "—";
  $("messages").replaceChildren(
    el("div", { class: "empty" },
      el("p", {}, "You're not in any outhouses yet."),
      el("p", {}, "Build one with ", el("strong", {}, "+"), " or join a friend's with an invite code."),
    ),
  );
}

function isUnread(c) {
  return c.kind === TEXT && c.last_message_id && (!c.last_read_id || idGt(c.last_message_id, c.last_read_id));
}

function renderChannels() {
  const text = state.channels.filter((c) => c.kind === TEXT);
  const voice = state.channels.filter((c) => c.kind === VOICE);
  $("text-channels").replaceChildren(
    ...text.map((c) =>
      el("button", {
        class: `channel${c.id === state.channelId ? " active" : ""}${c.id !== state.channelId && isUnread(c) ? " unread" : ""}`,
        "data-menu": "channel",
        "data-id": c.id,
        onclick: () => selectChannel(c.id),
      }, el("span", { class: "hash" }, "#"), el("span", { class: "cname" }, c.name), channelBadge(c.id)),
    ),
  );
  $("voice-channels").replaceChildren(
    ...voice.flatMap((c) => {
      const here = [...state.voiceStates.values()].filter((v) => v.channel_id === c.id);
      const mine = state.voice.channelId === c.id;
      return [
        el("button", {
          class: `channel voice${mine ? " active" : ""}`,
          "data-menu": "channel",
          "data-id": c.id,
          title: can("connect", c.id) ? `Join ${c.name}` : "You can't join this channel",
          onclick: () => joinVoice(c.id),
        }, el("span", { class: "hash" }, "🔈"), c.name),
        ...here.map((v) => {
          const m = memberById(v.user_id);
          const speaking = mine && state.voice.speaking.has(v.user_id);
          const live = mine && v.user_id === state.me.id && state.voice.live;
          return el("div", { class: `voice-user${speaking ? " speaking" : ""}${live ? " live" : ""}`, "data-menu": "member", "data-id": v.user_id },
            avatarFor(v.user_id, m?.display_name, 22),
            el("span", { class: "vname" }, m?.display_name ?? "Someone"),
            typeof voiceUserIcons === "function" ? voiceUserIcons(v, mine)
              : v.deafened ? el("span", { class: "vicon", title: "Deafened" }, "🎧") : v.muted ? el("span", { class: "vicon", title: "Muted" }, "🔇") : null);
        }),
      ];
    }),
  );
  renderVoicePanel();
  $("new-channel").hidden = !can("manage_channels");
  $("invite-btn").hidden = !can("create_invite");
}

function renderMembers() {
  const owner = currentGuild()?.owner_id;
  const hoisted = state.roles.filter((r) => r.hoist && r.id !== state.guildId).sort((a, b) => b.position - a.position);
  const groups = new Map(hoisted.map((r) => [r.id, []]));
  const rest = [];
  const offline = [];
  for (const m of state.members) {
    if (!m.online) { offline.push(m); continue; }
    const top = hoisted.find((r) => m.role_ids.includes(r.id));
    (top ? groups.get(top.id) : rest).push(m);
  }
  const row = (m) =>
    el("div", { class: `member${isTimedOut(m) ? " timed-out" : ""}${m.online ? "" : " offline"}`, "data-menu": "member", "data-id": m.id },
      el("div", { class: "avatar-wrap", onclick: (e) => openProfile(m.id, e.currentTarget) }, avatarFor(m.id, m.display_name, 32), m.online ? el("span", { class: "status-dot" }) : null),
      el("div", { class: "info" },
        el("div", { style: memberColour(m.id) ? `color:${memberColour(m.id)}` : "" },
          m.display_name, m.id === owner ? el("span", { class: "crown", title: "Owner" }, "👑") : null,
          m.bot ? el("span", { class: "bot-tag" }, "BOT") : null),
        isTimedOut(m) ? el("div", { class: "sub" }, "Timed out") : null,
      ),
    );
  const nodes = [];
  for (const r of hoisted) {
    const list = groups.get(r.id);
    if (!list.length) continue;
    nodes.push(el("div", { class: "member-group" }, `${r.name.toUpperCase()} — ${list.length}`), ...list.map(row));
  }
  if (rest.length) nodes.push(el("div", { class: "member-group" }, `ONLINE — ${rest.length}`), ...rest.map(row));
  if (offline.length) nodes.push(el("div", { class: "member-group" }, `GONE HOME — ${offline.length}`), ...offline.map(row));
  $("member-list").replaceChildren(...nodes);
}

// ---------- rendering: messages ----------

// Mentions are stored as <@userId> and <@&roleId> so renames don't break them.
function mentionLabel(kind, id) {
  if (kind === "role") return `@${roleById(id)?.name ?? "deleted-role"}`;
  const m = memberById(id);
  return `@${m ? m.display_name : "unknown-user"}`;
}

function mentionPill(kind, id) {
  const pill = el("span", { class: "mention" }, mentionLabel(kind, id));
  if (kind === "role") {
    const c = hexColour(roleById(id)?.color);
    if (c) { pill.style.color = c; pill.style.background = `${c}26`; }
  }
  return pill;
}

// Plain-text version (for reply previews and the edit box).
function plainMentions(text) {
  return text.replace(/<@(&?)(\d+)>/g, (_, amp, id) => mentionLabel(amp ? "role" : "user", id))
    .replace(/<:([a-z0-9_]{2,32}):\d+>/g, ":$1:");
}

// A custom emoji from an outhouse (falls back to :name: if it's been deleted).
function customEmoji(name, id) {
  const img = el("img", { class: "custom-emoji", src: `/emojis/${id}`, alt: `:${name}:`, title: `:${name}:`, loading: "lazy", draggable: "false" });
  img.addEventListener("error", () => img.replaceWith(`:${name}:`));
  return img;
}

// A reaction's emoji: plain text, or a picture for custom ones.
function emojiLabel(emoji) {
  const m = /^<:([a-z0-9_]{2,32}):(\d+)>$/.exec(emoji);
  return m ? customEmoji(m[1], m[2]) : emoji;
}

function renderText(text) {
  const out = [];
  const usernames = new Set(state.members.map((m) => m.username));
  const pattern = /(https?:\/\/[^\s<]+)|`([^`\n]+)`|<@(&?)(\d+)>|<:([a-z0-9_]{2,32}):(\d+)>|(@[\w.]+)/g;
  let last = 0;
  for (const m of text.matchAll(pattern)) {
    if (m.index > last) out.push(text.slice(last, m.index));
    if (m[1]) out.push(el("a", { href: m[1], target: "_blank", rel: "noopener noreferrer" }, m[1]));
    else if (m[5]) out.push(customEmoji(m[5], m[6]));
    else if (m[2]) out.push(el("code", {}, m[2]));
    else if (m[4]) out.push(mentionPill(m[3] ? "role" : "user", m[4]));
    else {
      const name = m[7].slice(1).toLowerCase();
      out.push(name === "everyone" || name === "here" || usernames.has(name) ? el("span", { class: "mention" }, m[7]) : m[7]);
    }
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

function mentionsMe(m) {
  const c = m.content.toLowerCase();
  const myRoles = memberById(state.me.id)?.role_ids ?? [];
  return c.includes(`<@${state.me.id}>`) || myRoles.some((r) => c.includes(`<@&${r}>`)) ||
    c.includes(`@${state.me.username}`) || c.includes("@everyone") || c.includes("@here") ||
    (m.reply_to && m.reply_to.author_id === state.me.id && m.author_id !== state.me.id);
}

function openLightbox(src) {
  document.body.append(el("div", { class: "lightbox", onclick: (e) => e.currentTarget.remove() }, el("img", { src, alt: "" })));
}

function renderAttachments(m) {
  if (!m.attachments.length) return null;
  return el("div", { class: "attachments" }, m.attachments.map((a) => {
    if (a.content_type.startsWith("image/")) {
      return el("img", { src: a.url, alt: a.filename, loading: "lazy", onclick: () => openLightbox(a.url) });
    }
    if (a.content_type.startsWith("video/")) return el("video", { src: a.url, controls: true, preload: "metadata" });
    if (a.content_type.startsWith("audio/")) return el("audio", { src: a.url, controls: true, preload: "metadata" });
    return el("div", { class: "file-card" }, "📄",
      el("div", {}, el("a", { href: a.url, target: "_blank", rel: "noopener" }, a.filename), el("div", { class: "size" }, fmtSize(a.size))));
  }));
}

function renderRich(e) {
  const colour = e.color ? hexColour(e.color) : "var(--ember)";
  const linkOrText = (text, url, cls) => url
    ? el("a", { class: cls, href: url, target: "_blank", rel: "noopener noreferrer" }, text)
    : el("span", { class: cls }, text);
  return el("div", { class: "embed rich", style: `border-left-color:${colour}` },
    el("div", { class: "rich-main" },
      e.author_name ? el("div", { class: "rich-author" },
        e.author_icon_url ? el("img", { src: e.author_icon_url, alt: "" }) : null,
        linkOrText(e.author_name, e.author_url, "")) : null,
      e.title ? linkOrText(e.title, e.url, "title") : null,
      e.description ? el("div", { class: "desc" }, renderText(e.description)) : null,
      e.fields?.length ? el("div", { class: "rich-fields" }, e.fields.map((f) =>
        el("div", { class: `rich-field${f.inline ? " inline" : ""}` }, el("div", { class: "fname" }, f.name), el("div", { class: "fvalue" }, renderText(f.value))))) : null,
      e.image_url ? el("img", { class: "rich-image", src: e.image_url, alt: "", loading: "lazy", onclick: () => openLightbox(e.image_url) }) : null,
      e.footer || e.timestamp ? el("div", { class: "rich-footer" },
        [e.footer, e.timestamp ? new Date(e.timestamp).toLocaleString() : null].filter(Boolean).join(" • ")) : null,
    ),
    e.thumbnail_url ? el("img", { class: "rich-thumb", src: e.thumbnail_url, alt: "", loading: "lazy" }) : null,
  );
}

function renderEmbeds(m) {
  if (!m.embeds.length) return null;
  return m.embeds.map((e) => {
    if (e.kind === "rich") return renderRich(e);
    if (e.kind === "image") {
      return el("div", { class: "embed-image" }, el("img", { src: e.image_url, alt: "", loading: "lazy", onclick: () => openLightbox(e.image_url) }));
    }
    return el("div", { class: "embed" },
      e.site_name ? el("div", { class: "site" }, e.site_name) : null,
      e.title ? el("a", { class: "title", href: e.url, target: "_blank", rel: "noopener noreferrer" }, e.title) : null,
      e.description ? el("div", { class: "desc" }, e.description) : null,
      e.image_url ? el("img", { src: e.image_url, alt: "", loading: "lazy", onclick: () => openLightbox(e.image_url) }) : null,
    );
  });
}

function renderReactions(m) {
  if (!m.reactions.length) return null;
  return el("div", { class: "reactions" },
    m.reactions.map((r) => {
      const mine = r.user_ids.includes(state.me.id);
      const names = r.user_ids.map((id) => memberById(id)?.display_name || "someone").join(", ");
      return el("button", { class: `reaction${mine ? " me" : ""}`, title: names, onclick: () => toggleReaction(m, r.emoji) },
        emojiLabel(r.emoji), el("span", { class: "count" }, r.user_ids.length));
    }),
    can("add_reactions", state.channelId)
      ? el("button", { class: "reaction", title: "Add reaction", onclick: (e) => emojiPicker(e.currentTarget, (emoji) => toggleReaction(m, emoji)) }, "＋")
      : null,
  );
}

function messageActions(m) {
  const mine = m.author_id === state.me.id;
  const bar = el("div", { class: "msg-actions" },
    can("add_reactions", state.channelId)
      ? el("button", { title: "Add reaction", onclick: (e) => emojiPicker(e.currentTarget, (emoji) => toggleReaction(m, emoji)) }, "😀")
      : null,
    can("send_messages", state.channelId) ? el("button", { title: "Reply", onclick: () => startReply(m) }, "↩") : null,
    can("send_messages", state.channelId) && !m.thread_id ? el("button", { class: "thread-btn", title: m.thread ? "Open thread" : "Start a thread", onclick: () => startThread(m) }, "🧵") : null,
    mine ? el("button", { title: "Edit", onclick: () => startEdit(m.id) }, "✎") : null,
    can("manage_messages", state.channelId)
      ? el("button", { class: `pin${m.pinned_at ? " on" : ""}`, title: m.pinned_at ? "Unpin" : "Pin", onclick: () => togglePin(m) }, "📌")
      : null,
    mine || can("manage_messages", state.channelId)
      ? el("button", { class: "danger", title: "Delete", onclick: (e) => deleteMessage(m, e.shiftKey) }, "🗑")
      : null,
    el("button", { title: "More", onclick: (e) => { const r = e.currentTarget.getBoundingClientRect(); MENUS.message({ clientX: r.left, clientY: r.bottom + 4 }, e.currentTarget.closest(".msg")); } }, "⋯"),
  );
  return bar;
}

// Members' names follow nickname changes live; webhook posts keep the name they were sent with.
const authorName = (m) => (m.author_bot ? null : memberById(m.author_id)?.display_name) ?? m.author_display_name;

function renderMessages() {
  const box = $("messages");
  const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 150;
  const nodes = [];
  let prev = null;
  for (const m of state.messages) {
    const grouped = prev && !m.reply_to && !m.interaction && !m.event && !prev.event && !m.poll && !prev.poll && !m.lfg && !prev.lfg && !prev.thread && prev.author_id === m.author_id && prev.author_display_name === m.author_display_name &&
      new Date(m.created_at) - new Date(prev.created_at) < 5 * 60 * 1000;
    const colour = memberColour(m.author_id);
    const imageOnly = m.embeds.length === 1 && m.embeds[0].kind === "image" && m.content.trim() === m.embeds[0].url;
    const jumbo = /^(\s*<:[a-z0-9_]{2,32}:\d+>\s*){1,10}$/.test(m.content);
    const editing = state.editingId === m.id;

    const body = el("div", { class: "msg-body" },
      grouped ? null : el("div", { class: "msg-meta" },
        el("span", { class: "msg-author", style: colour ? `color:${colour}` : "", onclick: (e) => openProfile(m.author_id, e.currentTarget) }, authorName(m)),
        m.author_bot ? el("span", { class: "bot-tag" }, "BOT") : null,
        el("time", { class: "msg-time", datetime: m.created_at, title: new Date(m.created_at).toLocaleString() }, timeLabel(m.created_at))),
      editing ? editBox(m) : (m.content && !imageOnly
        ? el("p", { class: `msg-text${jumbo ? " jumbo" : ""}` }, renderText(m.content), m.edited_at ? el("span", { class: "edited", title: new Date(m.edited_at).toLocaleString() }, "(edited)") : null)
        : null),
      m.event ? renderEventCard(m) : null,
      m.poll ? renderPollCard(m) : null,
      renderAttachments(m),
      renderEmbeds(m),
      renderReactions(m),
      m.thread ? threadLink(m.thread) : null,
    );

    const article = el("article", {
      class: `msg${grouped ? "" : " first"}${state.replyTo?.id === m.id ? " highlight" : ""}${mentionsMe(m) ? " mentions-me" : ""}${state.flashId === m.id ? " flash" : ""}${m.pinned_at ? " pinned" : ""}`,
      "data-menu": "message",
      "data-id": m.id,
      id: `msg-${m.id}`,
    }, grouped ? el("div", { class: "gutter" }) : messageAvatar(m), body, editing ? null : messageActions(m));

    if (m.interaction) {
      nodes.push(el("div", { class: "reply-ref interaction-ref" },
        "↱ ", el("strong", {}, memberById(m.interaction.user_id)?.display_name ?? m.interaction.display_name),
        " used ", el("code", {}, `/${m.interaction.command}`)));
    }
    if (m.reply_to) {
      nodes.push(el("div", { class: "reply-ref", onclick: () => jumpTo(m.reply_to.id) },
        "↱ ", el("strong", {}, m.reply_to.author_display_name), " ", plainMentions(m.reply_to.content) || "(attachment)"));
    }
    nodes.push(article);
    prev = m;
  }
  if (!nodes.length) {
    const ch = state.channels.find((c) => c.id === state.channelId);
    const dm = ch && ch.kind !== TEXT && ch.kind !== VOICE;
    nodes.push(el("div", { class: "empty" }, el("p", {}, dm ? `This is the start of your chat with ${ch.name}.` : `This is the start of #${ch ? ch.name : "channel"}.`), el("p", {}, "Say g'day.")));
  }
  if (nodes.length && state.historyStart === state.channelId && state.messages.length) {
    nodes.unshift(el("div", { class: "history-start" }, "You've reached the start. Nothing older down the long drop."));
  }
  box.replaceChildren(...nodes);
  if (nearBottom || renderMessages.forceBottom) box.scrollTop = box.scrollHeight;
  renderMessages.forceBottom = false;
  const editing = box.querySelector(".edit-box textarea");
  if (editing && document.activeElement !== editing) {
    editing.focus();
    editing.setSelectionRange(editing.value.length, editing.value.length);
  }
}

function messageAvatar(m) {
  // Real people and bots follow their current picture; webhook posts keep the one they were sent with.
  const live = memberById(m.author_id)?.avatar_url;
  const node = avatarFor(m.author_id, authorName(m), 40, (m.author_bot && !memberById(m.author_id) ? m.author_avatar_url : live ?? m.author_avatar_url) || undefined);
  node.style.cursor = "pointer";
  node.addEventListener("click", (e) => openProfile(m.author_id, e.currentTarget));
  return node;
}

function jumpTo(id) {
  const node = $(`msg-${id}`);
  if (!node) return jumpToMessage(state.channelId, id);
  node.scrollIntoView({ behavior: "smooth", block: "center" });
  node.classList.add("highlight");
  setTimeout(() => node.classList.remove("highlight"), 1500);
}

function renderTyping() {
  const now = Date.now();
  const map = state.typing.get(state.channelId);
  const names = [];
  if (map) for (const [uid, t] of map) (t.until > now ? names.push(t.name) : map.delete(uid));
  const text = !names.length ? "" :
    names.length === 1 ? `${names[0]} is scribbling on the wall…` :
    names.length <= 3 ? `${names.join(", ")} are scribbling on the wall…` : "Several people are scribbling…";
  $("typing").textContent = text;
}
setInterval(renderTyping, 1000);

function clearTyping(channelId, userId) {
  state.typing.get(channelId)?.delete(userId);
  renderTyping();
}

// ---------- mentions while typing ----------

const mentionInput = $("composer-input");

// Readable text like "@Raiders" in the box maps to a stored token.
function mentionCandidates() {
  const list = [];
  if (can("mention_everyone", state.channelId)) {
    list.push({ text: "@everyone", token: "@everyone", label: "@everyone", sub: "Everyone in the outhouse" });
    list.push({ text: "@here", token: "@here", label: "@here", sub: "Everyone online" });
  }
  for (const r of state.roles) {
    if (r.id === state.guildId) continue;
    list.push({ text: `@${r.name}`, token: `<@&${r.id}>`, label: `@${r.name}`, sub: "Role", colour: hexColour(r.color) });
  }
  for (const m of state.members) {
    list.push({ text: `@${m.display_name}`, token: `<@${m.id}>`, label: m.display_name, sub: `@${m.username}`, userId: m.id });
  }
  return list;
}

// Swap readable mentions for tokens (longest names first so "@Raid Leaders"
// isn't caught by "@Raid").
function encodeMentions(text) {
  const cands = mentionCandidates().filter((c) => c.token !== c.text).sort((a, b) => b.text.length - a.text.length);
  let out = text;
  for (const c of cands) {
    const escaped = c.text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    out = out.replace(new RegExp(`${escaped}(?![\\w])`, "g"), c.token);
  }
  return out;
}

let mentionState = null; // { start, items, index, box }

function closeMentions() {
  mentionState?.box.remove();
  mentionState = null;
}

function updateMentions() {
  const caret = mentionInput.selectionStart ?? mentionInput.value.length;
  const before = mentionInput.value.slice(0, caret);
  const match = before.match(/(^|\s)@([^\s@]{0,32})$/);
  if (!match || !state.guildId) return closeMentions();
  const query = match[2].toLowerCase();
  const items = mentionCandidates()
    .filter((c) => c.label.toLowerCase().replace(/^@/, "").includes(query) || c.sub.toLowerCase().includes(query))
    .slice(0, 8);
  if (!items.length) return closeMentions();
  const start = caret - match[2].length - 1;
  const index = mentionState && mentionState.start === start ? Math.min(mentionState.index, items.length - 1) : 0;
  mentionState?.box.remove();
  const box = el("div", { class: "mention-list", role: "listbox" },
    el("div", { class: "mention-title" }, "MENTION"),
    items.map((c, i) => el("button", {
      type: "button",
      class: i === index ? "active" : "",
      onmousedown: (e) => { e.preventDefault(); pickMention(c); },
    },
      c.userId ? avatarFor(c.userId, c.label, 24) : el("span", { class: "mention-icon", style: c.colour ? `color:${c.colour}` : "" }, "@"),
      el("span", { class: "label", style: c.colour ? `color:${c.colour}` : "" }, c.label),
      el("span", { class: "sub" }, c.sub))),
  );
  $("composer-wrap").append(box);
  mentionState = { start, items, index, box };
}

function pickMention(c) {
  const caret = mentionInput.selectionStart ?? mentionInput.value.length;
  const text = `${c.text} `;
  mentionInput.value = mentionInput.value.slice(0, mentionState.start) + text + mentionInput.value.slice(caret);
  const pos = mentionState.start + text.length;
  mentionInput.setSelectionRange(pos, pos);
  closeMentions();
  mentionInput.focus();
  autosize();
}

mentionInput.addEventListener("keydown", (e) => {
  if (!mentionState) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    e.stopImmediatePropagation();
    const n = mentionState.items.length;
    mentionState.index = (mentionState.index + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
    [...mentionState.box.querySelectorAll("button")].forEach((b, i) => b.classList.toggle("active", i === mentionState.index));
  } else if (e.key === "Enter" || e.key === "Tab") {
    e.preventDefault();
    e.stopImmediatePropagation();
    pickMention(mentionState.items[mentionState.index]);
  } else if (e.key === "Escape") {
    e.stopImmediatePropagation();
    closeMentions();
  }
});
mentionInput.addEventListener("input", updateMentions);
mentionInput.addEventListener("click", updateMentions);
mentionInput.addEventListener("blur", () => setTimeout(closeMentions, 150));

// ---------- message actions ----------

async function toggleReaction(m, emoji) {
  const mine = m.reactions.find((r) => r.emoji === emoji)?.user_ids.includes(state.me.id);
  const path = `/api/channels/${m.channel_id}/messages/${m.id}/reactions/${encodeURIComponent(emoji)}`;
  await tryApi(mine ? "DELETE" : "PUT", path).catch(() => {});
}

function startReply(m) {
  state.replyTo = { id: m.id, name: m.author_display_name };
  $("reply-name").textContent = m.author_display_name;
  $("reply-bar").hidden = false;
  $("composer-input").focus();
  renderMessages();
}

function cancelReply() {
  state.replyTo = null;
  $("reply-bar").hidden = true;
  renderMessages();
}
$("reply-cancel").addEventListener("click", cancelReply);

function startEdit(id) {
  state.editingId = id;
  state.editDraft = plainMentions(findMessage(id)?.content ?? "");
  renderMessages();
}

function editBox(m) {
  const ta = el("textarea", {}, state.editDraft);
  ta.addEventListener("input", () => (state.editDraft = ta.value));
  const finish = async (save) => {
    const content = encodeMentions(ta.value.trim());
    state.editingId = null;
    renderMessages();
    if (save && content !== m.content) {
      if (!content && !m.attachments.length) return deleteMessage(m);
      await tryApi("PATCH", `/api/channels/${m.channel_id}/messages/${m.id}`, { content }).catch(() => {});
    }
    $("composer-input").focus();
  };
  ta.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); finish(true); }
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); finish(false); }
  });
  return el("div", { class: "edit-box" }, ta, el("div", { class: "edit-hint" }, "Enter to save · Esc to cancel"));
}

async function deleteMessage(m, skipConfirm = false) {
  if (!skipConfirm && !(await confirmBox("Delete message?", "This can't be undone. (Tip: hold Shift to skip this.)", "Delete"))) return;
  await tryApi("DELETE", `/api/channels/${m.channel_id}/messages/${m.id}`).catch(() => {});
}

// ---------- composer: text, uploads, GIFs ----------

const input = $("composer-input");

function autosize() {
  input.style.height = "auto";
  input.style.height = `${Math.min(input.scrollHeight, 200)}px`;
}

input.addEventListener("input", () => {
  autosize();
  if (input.value && Date.now() - state.lastTypingSent > 8000 && state.channelId) {
    state.lastTypingSent = Date.now();
    api("POST", `/api/channels/${state.channelId}/typing`).catch(() => {});
  }
});

input.addEventListener("keydown", (e) => {
  if (mentionState) return; // the mention list is handling keys
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    $("composer").requestSubmit();
  } else if (e.key === "ArrowUp" && !input.value) {
    const mine = [...state.messages].reverse().find((m) => m.author_id === state.me.id);
    if (mine) { e.preventDefault(); startEdit(mine.id); }
  } else if (e.key === "Escape" && state.replyTo) {
    cancelReply();
  }
});

input.addEventListener("paste", (e) => {
  const files = [...(e.clipboardData?.files || [])];
  if (files.length) { e.preventDefault(); addFiles(files); }
});

function addFiles(files) {
  if (!state.channelId) return;
  if (!can("attach_files", state.channelId)) return toast("You can't upload files here.");
  for (const f of files) {
    if (f.size > 25 * 1024 * 1024) { toast(`${f.name} is over 25 MB.`); continue; }
    if (state.pendingFiles.length >= 10) { toast("Up to 10 files at a time."); break; }
    state.pendingFiles.push(f);
  }
  renderPendingFiles();
  input.focus();
}

function renderPendingFiles() {
  const box = $("pending-files");
  box.hidden = !state.pendingFiles.length;
  box.replaceChildren(...state.pendingFiles.map((f, i) =>
    el("span", { class: "chip" }, f.type.startsWith("image/") ? "🖼" : "📄", ` ${f.name} `, el("span", { class: "muted" }, fmtSize(f.size)),
      el("button", { type: "button", "aria-label": `Remove ${f.name}`, onclick: () => { state.pendingFiles.splice(i, 1); renderPendingFiles(); } }, "✕"))));
}

$("attach-btn").addEventListener("click", () => $("file-input").click());
$("file-input").addEventListener("change", (e) => { addFiles([...e.target.files]); e.target.value = ""; });

let dragDepth = 0;
window.addEventListener("dragenter", (e) => {
  if ($("app").hidden || !state.channelId || !e.dataTransfer?.types.includes("Files")) return;
  dragDepth++;
  $("drop-overlay").hidden = false;
});
window.addEventListener("dragleave", () => { if (--dragDepth <= 0) { dragDepth = 0; $("drop-overlay").hidden = true; } });
window.addEventListener("dragover", (e) => e.preventDefault());
window.addEventListener("drop", (e) => {
  e.preventDefault();
  dragDepth = 0;
  $("drop-overlay").hidden = true;
  if (e.dataTransfer?.files.length) addFiles([...e.dataTransfer.files]);
});

async function sendMessage(content) {
  content = encodeMentions(content);
  const channelId = state.channelId;
  const replyId = state.replyTo?.id;
  const files = state.pendingFiles;
  let msg;
  if (files.length) {
    const form = new FormData();
    form.append("content", content);
    if (replyId) form.append("reply_to_id", replyId);
    for (const f of files) form.append("files", f, f.name);
    toast(`Uploading ${files.length} file${files.length > 1 ? "s" : ""}…`);
    msg = await api("POST", `/api/channels/${channelId}/messages/upload`, form);
  } else {
    msg = await api("POST", `/api/channels/${channelId}/messages`, { content, reply_to_id: replyId || null });
  }
  state.pendingFiles = [];
  renderPendingFiles();
  if (state.replyTo) cancelReply();
  if (channelId === state.channelId && !findMessage(msg.id)) {
    state.messages.push(msg);
    renderMessages.forceBottom = true;
    renderMessages();
  }
}

$("composer").addEventListener("submit", async (e) => {
  e.preventDefault();
  const content = input.value.trim();
  if ((!content && !state.pendingFiles.length) || !state.channelId) return;
  const command = !state.pendingFiles.length && matchCommand(content);
  input.value = "";
  autosize();
  try {
    if (command) await runCommand(command, content);
    else await sendMessage(content);
  } catch (err) {
    input.value = content;
    autosize();
    toast(err.message);
  }
});

$("emoji-btn").addEventListener("click", (e) => {
  if (popover) return closePopover();
  emojiPicker(e.currentTarget, (emoji) => {
    const pos = input.selectionStart ?? input.value.length;
    input.value = input.value.slice(0, pos) + emoji + input.value.slice(pos);
    input.focus();
    input.setSelectionRange(pos + emoji.length, pos + emoji.length);
  }, { forText: true });
});

$("gif-btn").addEventListener("click", (e) => {
  if (popover) return closePopover();
  if (!state.channelId) return;
  const grid = el("div", { class: "gif-grid" }, el("p", { class: "muted" }, "Loading…"));
  const search = el("input", { placeholder: "Search GIFs", "aria-label": "Search GIFs" });
  let timer = null;
  const load = async (q) => {
    try {
      const gifs = await api("GET", `/api/gifs${q ? `?q=${encodeURIComponent(q)}` : ""}`);
      grid.replaceChildren(...(gifs.length ? gifs.map((g) =>
        el("img", { src: g.preview_url, alt: g.title, title: g.title, loading: "lazy", onclick: () => {
          closePopover();
          sendMessage(g.url).catch((err) => toast(err.message));
        } })) : [el("p", { class: "muted" }, "No GIFs found.")]));
    } catch (err) {
      grid.replaceChildren(el("p", { class: "muted" }, err.message));
    }
  };
  search.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(() => load(search.value.trim()), 350); });
  openPopover(e.currentTarget, el("div", { class: "gif-panel" }, search, grid, el("div", { class: "gif-credit" }, "Powered by GIPHY")));
  search.focus();
  load("");
});

// ---------- voice ----------

const LK = () => window.LivekitClient;

// A short two-note blip when someone joins or leaves your channel.
let chimeCtx = null;
function chime(from, to) {
  if (state.voice.deafened || !prefs.get("voice_chimes", true)) return;
  try {
    chimeCtx = chimeCtx || new AudioContext();
    const osc = chimeCtx.createOscillator();
    const gain = chimeCtx.createGain();
    const t = chimeCtx.currentTime;
    osc.frequency.setValueAtTime(from, t);
    osc.frequency.linearRampToValueAtTime(to, t + 0.12);
    gain.gain.setValueAtTime(0.0001, t);
    gain.gain.exponentialRampToValueAtTime(0.08, t + 0.02);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.25);
    osc.connect(gain).connect(chimeCtx.destination);
    osc.start(t);
    osc.stop(t + 0.3);
  } catch { /* audio not available */ }
}

const userVolume = (userId) => prefs.get(`vol_${userId}`, 1);
// "Mute for me": only you stop hearing them (and their soundboard clips).
const isLocallyMuted = (userId) => prefs.get(`lmute_${userId}`, false);
const isDeafened = () => state.voice.deafened || state.voice.serverDeafened;

function applyVolume(participant) {
  if (!participant || participant.isLocal) return;
  const off = isDeafened() || isLocallyMuted(participant.identity);
  const volume = off ? 0 : userVolume(participant.identity) * outputVolume();
  try {
    participant.setVolume(volume);
    // Screen share sound has its own volume in LiveKit.
    const { Track } = LK();
    participant.setVolume(volume, Track.Source.ScreenShareAudio);
  } catch { /* participant gone */ }
}

function applyAllVolumes() {
  state.voice.room?.remoteParticipants.forEach(applyVolume);
}

function volumeSlider(m) {
  const value = el("span", { class: "vol-value" }, `${Math.round(userVolume(m.id) * 100)}%`);
  const slider = el("input", { type: "range", min: 0, max: 200, step: 5, value: Math.round(userVolume(m.id) * 100), "aria-label": `Volume for ${m.display_name}` });
  slider.addEventListener("input", () => {
    prefs.set(`vol_${m.id}`, slider.value / 100);
    value.textContent = `${slider.value}%`;
    applyVolume(state.voice.room?.remoteParticipants.get(m.id));
  });
  slider.addEventListener("dblclick", () => { slider.value = 100; slider.dispatchEvent(new Event("input")); });
  return el("div", { class: "menu-slider", title: "Double-click to reset to 100%" }, el("div", { class: "row" }, el("span", {}, "User volume"), value), slider);
}

// ---- Microphone pipeline ----
// mic → gain (input volume) → one track that we publish. Changing the mic,
// noise suppression or the volume only rewires the input side, so the
// published track never changes and push to talk stays instant.
const micPipe = { ctx: null, room: null, stream: null, src: null, gain: null, gate: null, gateOpen: false, dest: null, pub: null, ready: null };
const inputVolume = () => prefs.get("input_volume", 1);
// Voice activity sensitivity, in dB (Discord-style: -80 picks up everything, 0 nothing).
const vadDb = () => prefs.get("vad_db", -50);
const dbToRms = (db) => 10 ** (db / 20);
const levelPct = (rms) => Math.max(0, Math.min(100, ((20 * Math.log10(rms || 1e-6)) + 80) / 80 * 100));
const syncVadMarker = () => document.documentElement.style.setProperty("--vad-pos", `${levelPct(dbToRms(vadDb()))}%`);
syncVadMarker();

// The noise gate runs on the audio thread, so it reacts instantly even while
// Outhouse is a background tab (timers there slow down to once a second).
const GATE_CODE = `
class OuthouseGate extends AudioWorkletProcessor {
  constructor() {
    super();
    this.threshold = 0.003; this.enabled = true; this.g = 0; this.quiet = 1e9; this.frames = 0;
    this.port.onmessage = (e) => Object.assign(this, e.data);
  }
  process(inputs, outputs) {
    const inp = inputs[0], out = outputs[0];
    if (!inp || !inp.length) return true;
    const first = inp[0];
    let sum = 0;
    for (let i = 0; i < first.length; i++) sum += first[i] * first[i];
    const rms = Math.sqrt(sum / first.length);
    this.quiet = rms > this.threshold ? 0 : this.quiet + first.length;
    const open = !this.enabled || this.quiet < 0.35 * sampleRate; // stay open through short pauses
    const target = open ? 1 : 0;
    const attack = 1 / (0.005 * sampleRate), release = 1 / (0.06 * sampleRate);
    let g = this.g;
    for (let c = 0; c < out.length; c++) {
      const src = inp[c] || first, dst = out[c];
      g = this.g;
      for (let n = 0; n < dst.length; n++) {
        g = target > g ? Math.min(target, g + attack) : Math.max(target, g - release);
        dst[n] = src[n] * g;
      }
    }
    this.g = g;
    this.frames += first.length;
    if (this.frames >= sampleRate * 0.05) { this.frames = 0; this.port.postMessage({ open }); }
    return true;
  }
}
registerProcessor("outhouse-gate", OuthouseGate);`;
const gateModules = new WeakMap();
async function makeGate(ctx, onState) {
  if (!gateModules.has(ctx)) {
    const url = URL.createObjectURL(new Blob([GATE_CODE], { type: "application/javascript" }));
    gateModules.set(ctx, ctx.audioWorklet.addModule(url));
  }
  await gateModules.get(ctx);
  const node = new AudioWorkletNode(ctx, "outhouse-gate");
  if (onState) node.port.onmessage = (e) => onState(e.data.open);
  configureGate(node);
  return node;
}
// Push to talk doesn't use the gate: holding the button sends everything.
function configureGate(node, forceOpen = false) {
  node?.port.postMessage({ threshold: dbToRms(vadDb()), enabled: !forceOpen && !usingPtt() });
}
const outputVolume = () => prefs.get("output_volume", 1);

// (Re)opens the microphone with the current settings and plugs it in.
async function refreshMicInput() {
  if (!micPipe.gain) return;
  const stream = await navigator.mediaDevices.getUserMedia({ audio: micOptions() });
  micPipe.src?.disconnect();
  micPipe.stream?.getTracks().forEach((t) => t.stop());
  micPipe.stream = stream;
  micPipe.src = micPipe.ctx.createMediaStreamSource(stream);
  micPipe.src.connect(micPipe.gain);
}

async function buildMic(room) {
  micPipe.ctx = micPipe.ctx || new AudioContext();
  await micPipe.ctx.resume().catch(() => {});
  micPipe.gain = micPipe.ctx.createGain();
  micPipe.gain.gain.value = inputVolume();
  micPipe.dest = micPipe.ctx.createMediaStreamDestination();
  micPipe.gate = await makeGate(micPipe.ctx, (open) => { micPipe.gateOpen = open; });
  micPipe.gain.connect(micPipe.gate).connect(micPipe.dest);
  micPipe.room = room;
  await refreshMicInput();
  const { Track } = LK();
  micPipe.pub = await room.localParticipant.publishTrack(micPipe.dest.stream.getAudioTracks()[0], {
    source: Track.Source.Microphone, name: "microphone",
  });
}

function teardownMic() {
  micPipe.src?.disconnect();
  micPipe.gain?.disconnect();
  micPipe.gate?.disconnect();
  micPipe.stream?.getTracks().forEach((t) => t.stop());
  Object.assign(micPipe, { room: null, stream: null, src: null, gain: null, gate: null, gateOpen: false, dest: null, pub: null, ready: null });
}

const wantMic = () => {
  const v = state.voice;
  return !v.muted && !v.deafened && !v.serverMuted && !v.serverDeafened && (!usingPtt() || v.pttDown);
};

async function setMic(on) {
  const room = state.voice.room;
  if (!room || !state.voice.canSpeak) return;
  try {
    // LiveKit drops the mic track if a moderator's server mute took away the
    // right to publish it; start a fresh one when it's needed again.
    if (micPipe.room === room && micPipe.pub && !room.localParticipant.trackPublications.has(micPipe.pub.trackSid)) teardownMic();
    if (on && micPipe.room !== room) {
      micPipe.ready = micPipe.ready || buildMic(room).catch((err) => { teardownMic(); throw err; });
      await micPipe.ready;
    }
    const track = micPipe.pub?.track;
    if (!track || state.voice.room !== room) return;
    // Check again: push to talk may have been released while the mic was starting.
    if (wantMic()) await track.unmute(); else await track.mute();
  } catch (err) {
    toast(`Microphone problem: ${err.message}`);
  }
}

function micOptions() {
  const deviceId = prefs.get("input_device", "");
  return {
    echoCancellation: true,
    noiseSuppression: prefs.get("noise_suppression", true),
    autoGainControl: true,
    ...(deviceId ? { deviceId } : {}),
  };
}

const usingPtt = () => prefs.get("voice_mode", "vad") === "ptt";

async function syncMic() {
  await setMic(wantMic());
}

async function pushVoiceState() {
  if (!state.voice.room) return;
  api("POST", "/api/voice/state", { muted: state.voice.muted, deafened: state.voice.deafened, streaming: !!state.voice.sharing }).catch(() => {});
}

// `moved`: a moderator moved you, maybe to an outhouse you're not looking at
// ({ guildId, name }); the server has already checked you can join.
async function joinVoice(channelId, moved = null) {
  if (state.voice.channelId === channelId || state.voice.connecting) return;
  if (!LK()) return toast("The voice library didn't load. Check your internet connection and refresh.");
  if (!moved && !can("connect", channelId)) return toast("You can't join this voice channel.");
  await leaveVoice(true);
  state.voice.connecting = true;
  renderVoicePanel();
  let res;
  try {
    res = await api("POST", `/api/channels/${channelId}/voice/join`);
  } catch (err) {
    state.voice.connecting = false;
    renderVoicePanel();
    return toast(err.message);
  }
  const { Room, RoomEvent } = LK();
  const output = prefs.get("output_device", "");
  const room = new Room({
    adaptiveStream: true,
    dynacast: true,
    audioCaptureDefaults: micOptions(),
    // Plays voices through Web Audio, so volumes can go above 100%.
    webAudioMix: true,
    // DTX stops sending during silence, which can clip the start of words.
    publishDefaults: { dtx: false, red: true },
    ...(output ? { audioOutput: { deviceId: output } } : {}),
  });
  room
    .on(RoomEvent.TrackSubscribed, (track, _pub, participant) => {
      if (track.kind === "video") { if (typeof renderScreens === "function") renderScreens(); return; }
      if (track.kind !== "audio") return;
      const audio = track.attach();
      audio.dataset.identity = participant.identity;
      $("voice-audio").append(audio);
      applyVolume(participant);
    })
    .on(RoomEvent.TrackUnsubscribed, (track) => {
      track.detach().forEach((e) => e.remove());
      // The publication still holds the track at this point; redraw just after.
      if (track.kind === "video" && typeof renderScreens === "function") setTimeout(renderScreens, 0);
    })
    .on(RoomEvent.TrackUnpublished, () => { if (typeof renderScreens === "function") setTimeout(renderScreens, 0); })
    .on(RoomEvent.ParticipantDisconnected, () => { if (typeof renderScreens === "function") setTimeout(renderScreens, 0); })
    .on(RoomEvent.LocalTrackPublished, () => { if (typeof onLocalTracksChanged === "function") onLocalTracksChanged(); })
    .on(RoomEvent.LocalTrackUnpublished, () => { if (typeof onLocalTracksChanged === "function") onLocalTracksChanged(); })
    .on(RoomEvent.ParticipantConnected, applyVolume)
    .on(RoomEvent.MediaDevicesError, (err) => toast(`Microphone problem: ${err.message}`))
    .on(RoomEvent.Disconnected, () => {
      if (state.voice.room === room) {
        toast("Disconnected from voice.");
        leaveVoice(true);
      }
    });

  try {
    await room.connect(res.url, res.token);
  } catch (err) {
    state.voice.connecting = false;
    renderVoicePanel();
    api("POST", "/api/voice/leave").catch(() => {});
    return toast(`Couldn't reach the voice server (${err.message}). Is LiveKit running? docker compose up -d livekit`);
  }
  const ch = state.channels.find((c) => c.id === channelId);
  // Browsers only allow microphones on https:// or localhost.
  const micAllowed = window.isSecureContext && !!navigator.mediaDevices?.getUserMedia;
  Object.assign(state.voice, {
    room, channelId, guildId: moved?.guildId ?? state.guildId, connecting: false, canSpeak: res.can_speak && micAllowed,
    listenOnly: res.can_speak && !micAllowed,
    channelName: moved?.name ?? ch?.name ?? "Voice", speaking: new Set(),
    serverMuted: !!res.server_muted, serverDeafened: !!res.server_deafened, canStream: !!res.can_stream, sharing: false,
  });
  if (!state.voice.canSpeak) state.voice.muted = true;
  if (state.voice.listenOnly) {
    toast("Listening only: browsers block the microphone on http:// addresses. Use localhost on the Outhouse PC, or HTTPS once it's hosted.");
  }
  try { await room.startAudio(); } catch { /* needs a click; the join click usually counts */ }
  await syncMic();
  if (state.voice.muted || state.voice.deafened) pushVoiceState();
  applyAllVolumes();
  startSpeakingMeter();
  syncDesktopPtt();
  chime(520, 780);
  renderChannels();
}

async function leaveVoice(callServer = true) {
  const room = state.voice.room;
  if (!room && !state.voice.channelId) return;
  state.voice.room = null;
  state.voice.channelId = null;
  state.voice.speaking = new Set();
  state.voice.sharing = false;
  if (typeof clearScreens === "function") clearScreens();
  stopSpeakingMeter();
  teardownMic();
  if (desktop) desktopInvoke("set_ptt", { code: null });
  if (room) {
    try { await room.disconnect(); } catch { /* already gone */ }
  }
  $("voice-audio").replaceChildren();
  if (callServer) api("POST", "/api/voice/leave").catch(() => {});
  chime(780, 440);
  renderChannels();
}

async function toggleMute() {
  if (state.voice.listenOnly) return toast("Your browser blocks the microphone on this http:// address, so you can listen but not talk.");
  if (!state.voice.canSpeak) return toast("You don't have permission to speak here.");
  if (state.voice.serverMuted || state.voice.serverDeafened) return toast("A moderator has muted you in this outhouse.");
  state.voice.muted = !state.voice.muted;
  if (!state.voice.muted && state.voice.deafened) state.voice.deafened = false;
  await syncMic();
  applyAllVolumes();
  pushVoiceState();
  renderVoicePanel();
}

async function toggleDeafen() {
  if (state.voice.serverDeafened) return toast("A moderator has deafened you in this outhouse.");
  state.voice.deafened = !state.voice.deafened;
  await syncMic();
  applyAllVolumes();
  pushVoiceState();
  renderVoicePanel();
}

// Push-to-talk (works while Outhouse is the focused window; the desktop app
// will make it work while you're in game).
function isTyping(e) {
  return e.target.closest?.("input, textarea, select, [contenteditable]");
}
// A quiet click when push to talk opens/closes your mic. Only you hear it.
function pttBlip(down) {
  const v = state.voice;
  if (!prefs.get("ptt_sounds", true) || v.muted || v.deafened || v.serverMuted || v.serverDeafened || !v.canSpeak) return;
  try {
    chimeCtx = chimeCtx || new AudioContext();
    const osc = chimeCtx.createOscillator();
    const gain = chimeCtx.createGain();
    const t = chimeCtx.currentTime;
    osc.type = "sine";
    osc.frequency.setValueAtTime(down ? 620 : 520, t);
    osc.frequency.linearRampToValueAtTime(down ? 820 : 400, t + 0.05);
    gain.gain.setValueAtTime(0.0001, t);
    gain.gain.exponentialRampToValueAtTime(0.035 * Math.min(outputVolume(), 1.5), t + 0.008);
    gain.gain.exponentialRampToValueAtTime(0.0001, t + 0.07);
    osc.connect(gain).connect(chimeCtx.destination);
    osc.start(t);
    osc.stop(t + 0.08);
  } catch { /* audio not available */ }
}

function setPtt(down) {
  if (state.voice.pttDown === down) return;
  state.voice.pttDown = down;
  pttBlip(down);
  syncMic();
  renderVoicePanel();
}

document.addEventListener("keydown", (e) => {
  if (!state.voice.room || !usingPtt() || e.repeat || isTyping(e)) return;
  if (e.code === prefs.get("ptt_key", "Backquote")) {
    e.preventDefault();
    setPtt(true);
  }
});
document.addEventListener("keyup", (e) => {
  if (!state.voice.room || !usingPtt()) return;
  if (e.code === prefs.get("ptt_key", "Backquote")) setPtt(false);
});
// Mouse buttons can be the push-to-talk key too: middle, back (4) and forward (5).
const MOUSE_CODES = { 1: "MouseMiddle", 3: "Mouse4", 4: "Mouse5" };
document.addEventListener("mousedown", (e) => {
  if (!state.voice.room || !usingPtt()) return;
  if (MOUSE_CODES[e.button] && MOUSE_CODES[e.button] === prefs.get("ptt_key", "Backquote")) {
    e.preventDefault();
    setPtt(true);
  }
});
document.addEventListener("mouseup", (e) => {
  const code = MOUSE_CODES[e.button];
  if (!code || code !== prefs.get("ptt_key", "Backquote")) return;
  e.preventDefault(); // stop the back/forward buttons from leaving the page
  if (state.voice.room && usingPtt()) setPtt(false);
});
// Middle-click would otherwise start auto-scroll or open links.
document.addEventListener("auxclick", (e) => {
  if (MOUSE_CODES[e.button] === prefs.get("ptt_key", "Backquote")) e.preventDefault();
});
window.addEventListener("blur", () => {
  if (state.voice.pttDown) { state.voice.pttDown = false; syncMic(); renderVoicePanel(); }
});

function renderVoicePanel() {
  const panel = $("voice-panel");
  const v = state.voice;
  if (!v.room && !v.connecting) { panel.hidden = true; return; }
  panel.hidden = false;
  const status = v.connecting ? "Connecting…" : v.listenOnly ? "Listening only" : v.serverMuted || v.serverDeafened ? "Muted by a moderator" : usingPtt() ? (v.pttDown ? "Talking" : `Push to talk: ${keyLabel(prefs.get("ptt_key", "Backquote"))}`) : "Voice connected";
  const forcedMute = v.serverMuted || v.serverDeafened;
  const shareBtn = typeof screenShareButton === "function" ? screenShareButton() : null;
  // (replaceChildren would show a null as the text "null")
  panel.replaceChildren(...[
    el("div", { class: "vp-top" },
      el("div", { class: "vp-info" },
        v.connecting
          ? el("div", { class: "vp-status connecting" }, status)
          : el("button", { class: `vp-status${forcedMute ? " forced" : ""}`, title: "Connection details", onclick: (e) => openVoiceStats(e.currentTarget) }, status),
        el("div", { class: "vp-channel" }, v.connecting ? "" : `${v.channelName}${v.guildId !== state.guildId ? ` · ${state.guilds.find((g) => g.id === v.guildId)?.name ?? ""}` : ""}`)),
      el("button", { class: "icon-btn danger", title: "Leave voice", "aria-label": "Leave voice", onclick: () => leaveVoice(true) }, "📞")),
    v.connecting ? null : el("div", { class: `vp-buttons${shareBtn ? " five" : ""}` },
      el("button", { class: `vp-btn${v.muted || forcedMute ? " off" : ""}${forcedMute ? " forced" : ""}`, title: forcedMute ? "Muted by a moderator" : v.muted ? "Unmute" : "Mute", onclick: toggleMute }, v.muted || forcedMute ? "🔇" : "🎙"),
      el("button", { class: `vp-btn${isDeafened() ? " off" : ""}${v.serverDeafened ? " forced" : ""}`, title: v.serverDeafened ? "Deafened by a moderator" : v.deafened ? "Undeafen" : "Deafen", onclick: toggleDeafen }, isDeafened() ? "🔕" : "🎧"),
      shareBtn,
      soundboardButton(),
      el("button", { class: "vp-btn", title: "Voice settings", onclick: (e) => openVoiceSettings(e.currentTarget) }, "⚙")),
    v.connecting ? null : touchPttButton(),
  ].filter(Boolean));
}

// ---- Speaking indicator ----
// Measured in this browser from the actual audio, so the ring lights up as
// soon as someone talks (the server's version is smoothed and arrives late).
const SPEAK_THRESHOLD = 0.012; // loudness (RMS) that counts as talking
const SPEAK_HOLD_MS = 250;     // keep the ring on through short gaps between words
const meter = { ctx: null, timer: null, taps: new Map() };

function voiceTracks(room) {
  const { Track } = LK();
  const out = new Map();
  const mic = room.localParticipant.getTrackPublication(Track.Source.Microphone);
  if (mic?.track && !mic.isMuted && !state.voice.muted) out.set(room.localParticipant.identity, mic.track.mediaStreamTrack);
  for (const p of room.remoteParticipants.values()) {
    for (const pub of p.audioTrackPublications.values()) {
      if (pub.track && !pub.isMuted && pub.source === Track.Source.Microphone) out.set(p.identity, pub.track.mediaStreamTrack);
    }
  }
  return out;
}

function startSpeakingMeter() {
  stopSpeakingMeter();
  try {
    meter.ctx = meter.ctx || new AudioContext();
    meter.ctx.resume().catch(() => {});
  } catch { return; }
  meter.timer = setInterval(meterTick, 50);
}

function stopSpeakingMeter() {
  clearInterval(meter.timer);
  meter.timer = null;
  for (const tap of meter.taps.values()) tap.source.disconnect();
  meter.taps.clear();
}

function meterTick() {
  const room = state.voice.room;
  if (!room) return;
  const tracks = voiceTracks(room);
  for (const [id, tap] of meter.taps) {
    if (tracks.get(id) !== tap.mst) { tap.source.disconnect(); meter.taps.delete(id); }
  }
  const now = performance.now();
  const speaking = new Set();
  for (const [id, mst] of tracks) {
    let tap = meter.taps.get(id);
    if (!tap) {
      if (!mst || mst.readyState !== "live") continue;
      const source = meter.ctx.createMediaStreamSource(new MediaStream([mst]));
      const analyser = meter.ctx.createAnalyser();
      analyser.fftSize = 512;
      source.connect(analyser);
      tap = { mst, source, analyser, buf: new Float32Array(analyser.fftSize), loudAt: 0 };
      meter.taps.set(id, tap);
    }
    tap.analyser.getFloatTimeDomainData(tap.buf);
    let sum = 0;
    for (const x of tap.buf) sum += x * x;
    if (Math.sqrt(sum / tap.buf.length) > SPEAK_THRESHOLD) tap.loudAt = now;
    if (now - tap.loudAt < SPEAK_HOLD_MS) speaking.add(id);
  }
  // A faint ring on yourself whenever your mic is actually sending.
  const mic = micPipe.pub;
  const live = !!mic && !mic.isMuted && (usingPtt() || micPipe.gateOpen);
  if (live !== state.voice.live) {
    state.voice.live = live;
    document.querySelector(`#voice-channels .voice-user[data-id="${state.me.id}"]`)?.classList.toggle("live", live);
  }
  const prev = state.voice.speaking;
  if (speaking.size === prev.size && [...speaking].every((id) => prev.has(id))) return;
  state.voice.speaking = speaking;
  // Flip the class in place; redrawing the whole sidebar 20 times a second would be wasteful.
  document.querySelectorAll("#voice-channels .voice-user").forEach((row) => {
    row.classList.toggle("speaking", speaking.has(row.dataset.id));
  });
}

// ---- Connection details (click "Voice connected") ----
async function voiceStats() {
  const room = state.voice.room;
  if (!room) return null;
  const { Track } = LK();
  const reports = [];
  for (const p of room.remoteParticipants.values()) {
    for (const pub of p.audioTrackPublications.values()) {
      if (pub.track?.receiver) reports.push(await pub.track.receiver.getStats());
    }
  }
  const mic = room.localParticipant.getTrackPublication(Track.Source.Microphone);
  if (mic?.track?.sender) reports.push(await mic.track.sender.getStats());

  const out = { protocol: null, rtt: null, lost: 0, received: 0, concealed: 0, samples: 0, jitter: 0, sendLoss: null };
  for (const report of reports) {
    const byId = new Map();
    report.forEach((s) => byId.set(s.id, s));
    let pair = null;
    report.forEach((s) => {
      if (s.type === "transport" && s.selectedCandidatePairId) pair = byId.get(s.selectedCandidatePairId);
    });
    if (!pair) report.forEach((s) => { if (s.type === "candidate-pair" && s.nominated && s.state === "succeeded") pair = s; });
    if (pair) {
      if (pair.currentRoundTripTime != null) out.rtt = Math.max(out.rtt ?? 0, pair.currentRoundTripTime);
      const local = byId.get(pair.localCandidateId);
      if (local) out.protocol = local.relayProtocol || local.protocol;
    }
    report.forEach((s) => {
      if (s.type === "inbound-rtp" && s.kind === "audio") {
        out.lost += s.packetsLost ?? 0;
        out.received += s.packetsReceived ?? 0;
        out.concealed += s.concealedSamples ?? 0;
        out.samples += s.totalSamplesReceived ?? 0;
        out.jitter = Math.max(out.jitter, s.jitter ?? 0);
      }
      if (s.type === "remote-inbound-rtp" && s.kind === "audio" && s.fractionLost != null) {
        out.sendLoss = Math.max(out.sendLoss ?? 0, s.fractionLost);
      }
    });
  }
  return out;
}

function openVoiceStats(anchor) {
  const body = el("div", { class: "vstats" }, "Measuring…");
  const pop = openPopover(anchor, body, "above");
  const pct = (a, b) => (b > 0 ? `${((a / b) * 100).toFixed(1)}%` : "–");
  const row = (label, value, bad) => el("div", { class: `vstats-row${bad ? " bad" : ""}` }, el("span", {}, label), el("span", {}, value));
  const draw = async () => {
    if (!pop.isConnected) return clearInterval(timer);
    const s = await voiceStats();
    if (!s) return;
    const lossIn = s.received + s.lost > 0 ? s.lost / (s.received + s.lost) : 0;
    const repaired = s.samples > 0 ? s.concealed / s.samples : 0;
    const tcp = s.protocol && s.protocol !== "udp";
    body.replaceChildren(
      el("div", { class: "vstats-title" }, "Voice connection"),
      row("Route", s.protocol ? s.protocol.toUpperCase() : "Not connected yet", tcp || !s.protocol),
      row("Ping", s.rtt != null ? `${Math.round(s.rtt * 1000)} ms` : "–", s.rtt > 0.15),
      row("Packets lost (incoming)", pct(s.lost, s.received + s.lost), lossIn > 0.02),
      row("Packets lost (your mic)", s.sendLoss != null ? `${(s.sendLoss * 100).toFixed(1)}%` : "–", s.sendLoss > 0.02),
      row("Audio patched over", pct(s.concealed, s.samples), repaired > 0.03),
      row("Jitter", `${Math.round(s.jitter * 1000)} ms`, s.jitter > 0.03),
      tcp ? el("p", { class: "vstats-hint" }, "Voice is going over TCP because UDP port 7882 isn't getting through. TCP stutters whenever a packet is late. Check the firewall rule for UDP 7882.") : null,
      !tcp && (lossIn > 0.02 || repaired > 0.03) ? el("p", { class: "vstats-hint" }, "Audio is being lost on the way. On Windows, Docker's network forwarding is a common cause: try .\\start.ps1 -NativeVoice.") : null,
    );
  };
  const timer = setInterval(draw, 1000);
  draw();
}

function keyLabel(code) {
  if (code === "MouseMiddle") return "Middle mouse";
  if (code === "Mouse4") return "Mouse 4 (back)";
  if (code === "Mouse5") return "Mouse 5 (forward)";
  return code.replace(/^Key/, "").replace(/^Digit/, "").replace("Backquote", "`").replace("Space", "Space");
}

// The voice controls, shared by the ⚙ popover and User settings → Voice & audio.
async function voiceSettingsControls() {
  const L = LK();
  let inputs = [], outputs = [];
  if (navigator.mediaDevices) try {
    inputs = await L.Room.getLocalDevices("audioinput");
    outputs = await L.Room.getLocalDevices("audiooutput");
  } catch { /* permission not granted yet */ }
  const devSelect = (list, key, kind) => {
    const sel = el("select", { class: "select" },
      el("option", { value: "" }, "System default"),
      list.map((d) => el("option", { value: d.deviceId, selected: d.deviceId === prefs.get(key, "") }, d.label || "Unnamed device")));
    sel.addEventListener("change", async () => {
      prefs.set(key, sel.value);
      try {
        if (kind === "audioinput") await refreshMicInput();
        else if (state.voice.room) await state.voice.room.switchActiveDevice(kind, sel.value || "default");
      } catch (err) { toast(err.message); }
      if (kind === "audioinput" && micTest.stream) restartMicTest?.();
    });
    return sel;
  };
  const mode = el("select", { class: "select" },
    el("option", { value: "vad", selected: !usingPtt() }, "Voice activity"),
    el("option", { value: "ptt", selected: usingPtt() }, "Push to talk"));
  const keyBtn = el("button", { type: "button", class: "ghost" }, `Key: ${keyLabel(prefs.get("ptt_key", "Backquote"))}`);
  keyBtn.addEventListener("click", () => {
    keyBtn.textContent = "Press a key or mouse button… (Esc to cancel)";
    const finish = (code) => {
      if (code) prefs.set("ptt_key", code);
      keyBtn.textContent = `Key: ${keyLabel(prefs.get("ptt_key", "Backquote"))}`;
      document.removeEventListener("keydown", grabKey, true);
      document.removeEventListener("mousedown", grabMouse, true);
      renderVoicePanel();
      syncDesktopPtt();
    };
    const grabKey = (e) => {
      e.preventDefault();
      e.stopPropagation();
      finish(e.code === "Escape" ? null : e.code);
    };
    // Left and right click stay free for clicking around.
    const grabMouse = (e) => {
      const code = MOUSE_CODES[e.button];
      if (!code) return;
      e.preventDefault();
      e.stopPropagation();
      finish(code);
    };
    document.addEventListener("keydown", grabKey, true);
    document.addEventListener("mousedown", grabMouse, true);
  });
  const pttRow = el("label", { class: "field", hidden: !usingPtt() }, "Push-to-talk key", keyBtn);
  const dbValue = el("span", { class: "vol-value" }, `${vadDb()} dB`);
  const dbSlider = el("input", { type: "range", min: -80, max: 0, step: 1, value: vadDb(), "aria-label": "Voice activity sensitivity" });
  dbSlider.addEventListener("input", () => {
    prefs.set("vad_db", Number(dbSlider.value));
    dbValue.textContent = `${dbSlider.value} dB`;
    syncVadMarker();
    configureGate(micPipe.gate);
    configureGate(micTest.gate, !micTest.gated);
  });
  const vadRow = el("div", { class: "field", hidden: usingPtt() },
    el("span", {}, "Sensitivity"),
    el("div", { class: "vol-row db-row" }, el("span", { class: "muted small" }, "Picks up more"), dbSlider, dbValue),
    el("span", { class: "muted small", style: "font-weight:400" }, "Your mic only sends when you're louder than this. Drag right if background noise gets through, left if your words get cut off."));
  mode.addEventListener("change", () => {
    prefs.set("voice_mode", mode.value);
    pttRow.hidden = mode.value !== "ptt";
    vadRow.hidden = mode.value === "ptt";
    syncDesktopPtt();
    configureGate(micPipe.gate);
    configureGate(micTest.gate, !micTest.gated);
    state.voice.pttDown = false;
    syncMic();
    renderVoicePanel();
  });
  const check = (key, label, fallback, after) => {
    const box = el("input", { type: "checkbox", checked: prefs.get(key, fallback) });
    box.addEventListener("change", () => { prefs.set(key, box.checked); after?.(); });
    return el("label", { class: "check-row" }, box, label);
  };
  const volume = (label, key, onInput) => {
    const value = el("span", { class: "vol-value" }, `${Math.round(prefs.get(key, 1) * 100)}%`);
    const slider = el("input", { type: "range", min: 0, max: 200, step: 5, value: Math.round(prefs.get(key, 1) * 100), "aria-label": label });
    slider.addEventListener("input", () => {
      prefs.set(key, slider.value / 100);
      value.textContent = `${slider.value}%`;
      onInput();
    });
    slider.addEventListener("dblclick", () => { slider.value = 100; slider.dispatchEvent(new Event("input")); });
    return el("div", { class: "vol-row", title: "Double-click to reset to 100%" }, el("span", {}, label), slider, value);
  };
  return {
    devices: [
      el("label", { class: "field" }, "Microphone", devSelect(inputs, "input_device", "audioinput")),
      volume("Input volume", "input_volume", () => {
        if (micPipe.gain) micPipe.gain.gain.value = inputVolume();
        if (micTest.inGain) micTest.inGain.gain.value = inputVolume();
      }),
      el("label", { class: "field" }, "Speakers / headset", devSelect(outputs, "output_device", "audiooutput")),
      volume("Output volume", "output_volume", () => {
        applyAllVolumes();
        if (micTest.outGain) micTest.outGain.gain.value = outputVolume();
      }),
    ],
    mode: [el("label", { class: "field" }, "Input mode", mode), pttRow, vadRow],
    checks: [
      check("noise_suppression", "Noise suppression", true, () => { refreshMicInput().catch((err) => toast(err.message)); restartMicTest?.(); }),
      check("voice_chimes", "Join and leave sounds", true),
      check("ptt_sounds", "Push-to-talk click (only you hear it)", true),
    ],
  };
}

async function openVoiceSettings(anchor) {
  const c = await voiceSettingsControls();
  openPopover(anchor, el("div", { class: "voice-settings" },
    el("h3", {}, "Voice settings"),
    ...c.devices, ...c.mode, ...c.checks,
    el("p", { class: "muted small", style: "margin:0" }, globalPtt ? "Push to talk works in other apps and games too." : "In the browser, push to talk only works while this tab is focused. The Outhouse desktop app makes it work in game."),
    el("button", { type: "button", class: "ghost", onclick: () => { closePopover(); openUserSettings("voice"); } }, "All settings…"),
  ));
}

// ---------- desktop app ----------
// When Outhouse runs inside the Windows app, these hooks add push to talk that
// works in game, notifications and opening links in your browser. In a normal
// browser `desktop` is null and none of this does anything.
const desktop = !!window.__OUTHOUSE_DESKTOP__;
// Push to talk that works in other apps: the Windows app only.
const globalPtt = desktop && window.__OUTHOUSE_PLATFORM__ !== "android";
// The app answers on this address (it never leaves your PC).
const BRIDGE = "http://outhouse.localhost";
const desktopStatus = { text: "", ok: true };
async function desktopCall(path, params = {}) {
  if (!desktop) return null;
  try {
    const res = await fetch(`${BRIDGE}/${path}?${new URLSearchParams(params)}`);
    const text = await res.text();
    if (!res.ok) throw new Error(text || `error ${res.status}`);
    return text;
  } catch (err) {
    desktopStatus.ok = false;
    desktopStatus.text = `Can't reach the desktop app: ${err.message}`;
    document.querySelectorAll(".desktop-status").forEach((n) => { n.textContent = desktopStatus.text; n.classList.add("bad"); });
    return null;
  }
}
// Kept for the calls below.
const desktopInvoke = (cmd, args) => {
  if (cmd === "set_ptt") return desktopCall("ptt", { code: args.code ?? "" });
  if (cmd === "notify") return desktopCall("notify", args);
  if (cmd === "open_external") return desktopCall("open", args);
  return Promise.resolve(null);
};

// Tell the app which key to watch: only while in voice with push to talk on.
async function syncDesktopPtt() {
  if (!globalPtt) return;
  const watching = state.voice.room && state.voice.canSpeak && usingPtt();
  const key = prefs.get("ptt_key", "Backquote");
  const ok = await desktopCall("ptt", { code: watching ? key : "" });
  if (ok === null) return;
  desktopStatus.ok = true;
  desktopStatus.text = watching
    ? `Desktop app: listening for ${keyLabel(key)} in other apps and games.`
    : usingPtt() ? "Desktop app: connected. It listens for your key while you're in voice." : "Desktop app: connected.";
  document.querySelectorAll(".desktop-status").forEach((n) => { n.textContent = desktopStatus.text; n.classList.remove("bad"); });
}

// Called by the app when your key goes down/up while another window is focused.
window.outhouseDesktopPtt = (down) => {
  if (!state.voice.room || !usingPtt()) return;
  setPtt(down);
};

function mentionsMe(m) {
  if (m.author_id === state.me.id) return false;
  if (m.reply_to?.author_id === state.me.id) return true;
  if (m.content.includes(`<@${state.me.id}>`) || /(^|\s)@(everyone|here)\b/.test(m.content)) return true;
  const myRoles = memberById(state.me.id)?.role_ids ?? [];
  return myRoles.some((id) => m.content.includes(`<@&${id}>`));
}

function desktopNotifyMessage(m, ch) {
  if (!desktop || document.hasFocus() || !mentionsMe(m)) return;
  const where = ch ? ` in #${ch.name}` : "";
  const text = plainMentions(m.content).trim() || (m.attachments?.length ? "Sent an attachment" : "");
  desktopInvoke("notify", { title: `${m.author_display_name}${where}`, body: text.slice(0, 200) });
}

if (desktop) {
  // Links to other sites open in your normal browser, not inside the app.
  document.addEventListener("click", (e) => {
    const a = e.target.closest?.("a[href]");
    if (!a || a.target !== "_blank") return;
    e.preventDefault();
    desktopInvoke("open_external", { url: a.href });
  }, true);
  window.open = (url) => { desktopInvoke("open_external", { url: new URL(url, location.href).href }); return null; };
}

// ---------- user settings ----------

async function openUserSettings(tab = "account") {
  stopMicTest();
  const { dialog, nav, main } = settingsShell("User settings");
  dialog.addEventListener("close", stopMicTest);
  const pages = [
    ["account", "My account"],
    ["look", "Profile & picture"],
    ["profile", "Outhouse nicknames"],
    ["voice", "Voice & audio"],
    typeof drawNotificationsPage === "function" ? ["notifications", "Notifications"] : null,
  ].filter(Boolean);
  const draw = (active) => {
    stopMicTest();
    nav.replaceChildren(
      el("h3", {}, "USER SETTINGS"),
      ...pages.map(([id, label]) => el("button", { class: active === id ? "active" : "", onclick: () => draw(id) }, label)),
      el("div", { style: "flex:1" }),
      el("button", { onclick: () => { dialog.close(); signOut(); } }, "Sign out"),
    );
    const page = settingsPage(main);
    if (active === "look") return drawProfileLook(page);
    if (active === "profile") return drawProfiles(page);
    if (active === "voice") return drawVoicePage(page);
    if (active === "notifications") return drawNotificationsPage(page);
    return drawAccount(page);
  };
  draw(tab);
}

function drawAccount(main) {
  const name = el("input", { type: "text", maxlength: 32, value: state.me.display_name });
  const saveName = el("button", { class: "primary", onclick: async () => {
    const user = await tryApi("PATCH", "/api/users/@me", { display_name: name.value }).catch(() => null);
    if (!user) return;
    state.me = user;
    renderMe();
    toast("Name saved.");
  } }, "Save");

  const current = el("input", { type: "password", autocomplete: "current-password" });
  const next = el("input", { type: "password", autocomplete: "new-password", minlength: 8 });
  const again = el("input", { type: "password", autocomplete: "new-password" });
  const savePw = el("button", { class: "ghost", onclick: async () => {
    if (next.value.length < 8) return toast("New passwords need at least 8 characters.");
    if (next.value !== again.value) return toast("The two new passwords don't match.");
    const ok = await tryApi("PATCH", "/api/users/@me", { current_password: current.value, new_password: next.value }).then(() => true, () => false);
    if (!ok) return;
    current.value = next.value = again.value = "";
    toast("Password changed. Any other devices signed in as you have been signed out.");
  } }, "Change password");

  main.append(
    el("h2", {}, "My account"),
    el("label", { class: "field" }, "Display name",
      el("div", { class: "inline-row" }, name, saveName)),
    el("p", { class: "muted small", style: "margin:-6px 0 0" }, "What everyone sees, unless you set a nickname in an outhouse."),
    el("label", { class: "field" }, "Username", el("input", { type: "text", value: state.me.username, disabled: true })),
    el("p", { class: "muted small", style: "margin:-6px 0 0" }, "Used to sign in. It can't be changed."),
    el("h3", { style: "margin:12px 0 0" }, "Password"),
    el("label", { class: "field" }, "Current password", current),
    el("label", { class: "field" }, "New password", next),
    el("label", { class: "field" }, "New password again", again),
    el("div", {}, savePw),
  );
}

async function drawProfiles(main) {
  main.append(el("h2", {}, "Outhouse nicknames"),
    el("p", { class: "muted", style: "margin:0" }, "Use a different name in each outhouse. Leave it blank to use your display name."));
  if (!state.guilds.length) return main.append(el("p", { class: "muted" }, "You're not in any outhouses yet."));
  for (const g of state.guilds) {
    const members = await api("GET", `/api/guilds/${g.id}/members`).catch(() => []);
    const mine = members.find((m) => m.id === state.me.id);
    const nick = mine && mine.display_name !== state.me.display_name ? mine.display_name : "";
    const input = el("input", { type: "text", maxlength: 32, value: nick, placeholder: state.me.display_name });
    const save = el("button", { class: "primary", onclick: async () => {
      const member = await tryApi("PUT", `/api/guilds/${g.id}/nickname`, { nickname: input.value }).catch(() => null);
      if (!member) return;
      toast(input.value.trim() ? `You're now "${member.display_name}" in ${g.name}.` : `Nickname cleared in ${g.name}.`);
      if (g.id === state.guildId) refreshGuild();
    } }, "Save");
    main.append(el("label", { class: "field" }, g.name, el("div", { class: "inline-row" }, input, save)));
  }
}

// ---- mic test (Voice & audio page) ----
const micTest = { stream: null, ctx: null, raf: 0, audio: null, inGain: null, outGain: null, gate: null, gated: true, open: false };
let restartMicTest = null; // set while the Voice & audio page is open
function stopMicTest() {
  cancelAnimationFrame(micTest.raf);
  if (micTest.audio) { micTest.audio.pause(); micTest.audio.srcObject = null; micTest.audio = null; }
  micTest.stream?.getTracks().forEach((t) => t.stop());
  micTest.ctx?.close().catch(() => {});
  micTest.stream = micTest.ctx = micTest.inGain = micTest.outGain = micTest.gate = null;
}

async function drawVoicePage(main) {
  const c = await voiceSettingsControls();
  if (globalPtt) setTimeout(syncDesktopPtt, 0);
  const bar = el("div", { class: "mic-bar" });
  const meter = el("div", { class: "mic-meter" }, bar, el("div", { class: "vad-line", title: "Sensitivity" }));
  const testBtn = el("button", { class: "ghost" }, "Test my mic");
  const playback = el("input", { type: "checkbox", checked: prefs.get("mic_test_playback", true) });
  playback.addEventListener("change", () => {
    prefs.set("mic_test_playback", playback.checked);
    restartMicTest();
  });
  restartMicTest = () => { if (micTest.stream && testBtn.isConnected) { testBtn.click(); testBtn.click(); } };
  testBtn.addEventListener("click", async () => {
    if (micTest.stream) { stopMicTest(); testBtn.textContent = "Test my mic"; bar.style.width = "0"; return; }
    if (!navigator.mediaDevices?.getUserMedia) return toast("This browser only allows the microphone on https:// or localhost.");
    try {
      // Echo cancellation would try to cancel the playback itself, so it's off for the test.
      const audio = playback.checked ? { ...micOptions(), echoCancellation: false } : micOptions();
      micTest.stream = await navigator.mediaDevices.getUserMedia({ audio });
    } catch (err) { return toast(`Microphone problem: ${err.message}`); }
    micTest.ctx = new AudioContext();
    const analyser = micTest.ctx.createAnalyser();
    analyser.fftSize = 512;
    const source = micTest.ctx.createMediaStreamSource(micTest.stream);
    micTest.inGain = micTest.ctx.createGain();
    micTest.inGain.gain.value = inputVolume();
    source.connect(micTest.inGain).connect(analyser);
    if (playback.checked) {
      // Play it back through the chosen speakers/headset, a moment late so it's easy to tell apart.
      const delay = micTest.ctx.createDelay(1);
      delay.delayTime.value = 0.25;
      const out = micTest.ctx.createMediaStreamDestination();
      micTest.outGain = micTest.ctx.createGain();
      micTest.outGain.gain.value = outputVolume();
      // Through the same gate as a call, so you hear exactly what others would.
      micTest.gate = await makeGate(micTest.ctx, (open) => { micTest.open = open; });
      micTest.inGain.connect(micTest.gate).connect(delay).connect(micTest.outGain).connect(out);
      micTest.audio = new Audio();
      micTest.audio.srcObject = out.stream;
      const sink = prefs.get("output_device", "");
      if (sink && micTest.audio.setSinkId) await micTest.audio.setSinkId(sink).catch(() => {});
      micTest.audio.play().catch(() => {});
    }
    const buf = new Float32Array(analyser.fftSize);
    testBtn.textContent = "Stop test";
    const tick = () => {
      if (!micTest.stream || !bar.isConnected) return stopMicTest();
      analyser.getFloatTimeDomainData(buf);
      let sum = 0;
      for (const x of buf) sum += x * x;
      const rms = Math.sqrt(sum / buf.length);
      bar.style.width = `${levelPct(rms)}%`;
      // Green while your mic would be sending.
      bar.classList.toggle("talking", usingPtt() ? rms > SPEAK_THRESHOLD : rms > dbToRms(vadDb()));
      micTest.raf = requestAnimationFrame(tick);
    };
    tick();
  });

  // (append would show a null as the text "null")
  main.append(...[
    el("h2", {}, "Voice & audio"),
    ...c.devices,
    el("div", { class: "field-like" }, el("div", { class: "inline-row" }, testBtn, meter)),
    el("label", { class: "check-row" }, playback, "Play back what my mic hears (use headphones, or speakers will echo)"),
    el("p", { class: "muted small", style: "margin:-6px 0 0" }, "Talk and watch the bar. In voice activity mode the line is your sensitivity: the bar turns green when you're above it and your mic would be sending. With playback on you hear exactly what others would, a moment later, through the speakers/headset chosen above."),
    ...c.mode,
    globalPtt ? el("p", { class: `small desktop-status${desktopStatus.ok ? "" : " bad"}`, style: "margin:-6px 0 0" }, desktopStatus.text || "Desktop app: checking…") : null,
    el("p", { class: "muted small", style: "margin:-6px 0 0" }, `Push to talk works with a keyboard key or the middle / side mouse buttons. ${globalPtt ? "In the desktop app it works while you're in game too." : "In the browser it only works while this tab is focused; the Outhouse desktop app makes it work in game."}`),
    ...c.checks,
    typeof entranceSoundSection === "function" ? await entranceSoundSection() : null,
  ].filter(Boolean));
}

// ---------- soundboard ----------

async function loadSounds() {
  const id = state.guildId;
  const sounds = await api("GET", `/api/guilds/${id}/sounds`).catch(() => []);
  if (state.guildId === id) state.sounds = sounds;
}

function playSound(url, volume = 1, preview = false) {
  if (!preview && (isDeafened() || prefs.get("soundboard_muted", false))) return;
  const audio = new Audio(url);
  audio.volume = Math.max(0, Math.min(1, volume * prefs.get("soundboard_volume", 0.7) * outputVolume()));
  const out = prefs.get("output_device", "");
  if (out && audio.setSinkId) audio.setSinkId(out).catch(() => {});
  audio.play().catch(() => {});
}

// The soundboard popover itself is in soundboard.js.

// ---------- navigation ----------

async function refreshGuild() {
  const id = state.guildId;
  if (!id) return;
  const [channels, members, roles, perms, voice] = await Promise.all([
    api("GET", `/api/guilds/${id}/channels`),
    api("GET", `/api/guilds/${id}/members`),
    api("GET", `/api/guilds/${id}/roles`),
    api("GET", `/api/guilds/${id}/permissions`),
    api("GET", `/api/guilds/${id}/voice`).catch(() => []),
  ]);
  if (state.guildId !== id) return;
  state.voiceStates = new Map(voice.map((v) => [v.user_id, v]));
  loadSounds();
  state.channels = channels;
  state.members = members;
  state.roles = roles;
  state.myPerms = { guild: big(perms.guild), channels: perms.channels, top: perms.top_position };
  renderChannels();
  renderMembers();
  if (state.channelId && !channels.some((c) => c.id === state.channelId)) {
    const first = channels.find((c) => c.kind === TEXT);
    if (first) selectChannel(first.id);
    else state.channelId = null;
  } else {
    updateComposer();
    renderMessages();
  }
}

async function selectGuild(id, preferChannel) {
  state.guildId = id;
  state.unreadGuilds.delete(id);
  localStorage.setItem("outhouse_guild", id);
  const guild = currentGuild();
  $("guild-name").textContent = guild.name;
  renderGuilds();
  state.channelId = null;
  state.messages = [];
  await refreshGuild();
  if (state.guildId !== id) return;
  loadCommands();
  const saved = localStorage.getItem(`outhouse_channel_${id}`);
  const pick = state.channels.find((c) => c.id === (preferChannel || saved) && c.kind === TEXT) || state.channels.find((c) => c.kind === TEXT);
  if (pick) selectChannel(pick.id);
  else $("messages").replaceChildren(el("div", { class: "empty" }, el("p", {}, "No channels you can see yet.")));
}

function updateComposer() {
  const ch = state.channels.find((c) => c.id === state.channelId);
  const canSend = ch && can("send_messages", ch.id);
  input.disabled = !canSend;
  const dm = ch && ch.kind !== TEXT && ch.kind !== VOICE;
  input.placeholder = !ch ? "Write on the wall…" : !canSend ? "You can't post in this channel." : dm ? `Message ${ch.kind === 1 ? "@" : ""}${ch.name}` : `Write on the wall in #${ch.name}`;
  $("attach-btn").hidden = !(ch && can("attach_files", ch.id));
}

async function selectChannel(id) {
  if (state.channelId !== id) {
    state.pendingFiles = [];
    renderPendingFiles();
    if (state.replyTo) { state.replyTo = null; $("reply-bar").hidden = true; }
    state.editingId = null;
  }
  state.channelId = id;
  localStorage.setItem(`outhouse_channel_${state.guildId}`, id);
  onChannelChanged();
  const ch = state.channels.find((c) => c.id === id);
  $("channel-name").textContent = ch.name;
  updateComposer();
  renderChannels();
  renderTyping();
  const messages = await api("GET", `/api/channels/${id}/messages?limit=75`).catch((err) => { toast(err.message); return []; });
  if (state.channelId !== id) return;
  state.messages = messages;
  state.historyStart = messages.length < 75 ? id : null;
  renderMessages.forceBottom = true;
  renderMessages();
  ackChannel();
  if (!input.disabled) input.focus();
}

// ---------- right-click menus ----------

const MENUS = {
  message(e, node) {
    const m = findMessage(node.dataset.id);
    if (!m) return;
    const mine = m.author_id === state.me.id;
    const selection = window.getSelection()?.toString();
    const link = e.target?.closest?.("a");
    const img = e.target?.closest?.("img");
    openMenu(e.clientX, e.clientY, [
      can("add_reactions", state.channelId) ? { quick: QUICK_REACTIONS, onPick: (emoji) => toggleReaction(m, emoji) } : null,
      can("add_reactions", state.channelId) ? { label: "Add reaction…", onClick: () => emojiPicker(node.querySelector(".msg-body"), (emoji) => toggleReaction(m, emoji)) } : null,
      can("send_messages", state.channelId) ? { label: "Reply", onClick: () => startReply(m) } : null,
      m.thread ? { label: "Open thread", onClick: () => openThread(m.thread) } : can("send_messages", state.channelId) ? { label: "Start a thread", onClick: () => startThread(m) } : null,
      mine ? { label: "Edit message", onClick: () => startEdit(m.id) } : null,
      can("manage_messages", state.channelId) ? { label: m.pinned_at ? "Unpin message" : "Pin message", onClick: () => togglePin(m) } : null,
      "sep",
      selection ? { label: "Copy selection", onClick: () => copy(selection, "Selection copied") } : null,
      m.content ? { label: "Copy text", onClick: () => copy(plainMentions(m.content), "Message copied") } : null,
      link ? { label: "Copy link", onClick: () => copy(link.href, "Link copied") } : null,
      link ? { label: "Open link", onClick: () => window.open(link.href, "_blank", "noopener") } : null,
      img ? { label: "Open image", onClick: () => window.open(img.src, "_blank", "noopener") } : null,
      "sep",
      { label: "Copy message ID", onClick: () => copy(m.id, "ID copied") },
      mine || can("manage_messages", state.channelId) ? "sep" : null,
      mine || can("manage_messages", state.channelId) ? { label: "Delete message", danger: true, onClick: () => deleteMessage(m) } : null,
    ]);
  },

  channel(e, node) {
    const c = state.channels.find((x) => x.id === node.dataset.id);
    if (!c) return;
    openMenu(e.clientX, e.clientY, [
      c.kind === VOICE ? (state.voice.channelId === c.id
        ? { label: "Leave voice", onClick: () => leaveVoice(true) }
        : { label: "Join voice", disabled: !can("connect", c.id), onClick: () => joinVoice(c.id) }) : null,
      c.kind === TEXT ? { label: "Mark as read", disabled: !isUnread(c) && !channelMentions(c.id), onClick: () => { if (c.last_message_id) { c.last_read_id = c.last_message_id; noteChannelRead(c.id, c.last_message_id); api("POST", `/api/channels/${c.id}/ack`, { message_id: c.last_message_id }).catch(() => {}); renderChannels(); } } } : null,
      can("create_invite") ? { label: "Invite people", onClick: invitePeople } : null,
      ...(c.kind === TEXT && typeof notifyMenuItems === "function" ? notifyMenuItems("channel", c) : []),
      "sep",
      can("manage_roles") ? { label: "Edit permissions", onClick: () => openChannelPermissions(c) } : null,
      c.kind === TEXT && can("manage_webhooks", c.id) ? { label: "Webhooks & news feeds", onClick: () => openSettings("integrations") } : null,
      can("manage_channels", c.id) ? { label: "Delete channel", danger: true, onClick: () => deleteChannel(c) } : null,
      "sep",
      { label: "Copy channel ID", onClick: () => copy(c.id, "ID copied") },
    ]);
  },

  guild(e, node) {
    const g = state.guilds.find((x) => x.id === node.dataset.id);
    if (!g) return;
    const here = g.id === state.guildId;
    const owner = g.owner_id === state.me.id;
    openMenu(e.clientX, e.clientY, [
      !here ? { label: "Open", onClick: () => selectGuild(g.id) } : null,
      here && can("create_invite") ? { label: "Invite people", onClick: invitePeople } : null,
      here && can("manage_channels") ? { label: "Create text channel", onClick: () => createChannel(TEXT) } : null,
      here && can("manage_channels") ? { label: "Create voice channel", onClick: () => createChannel(VOICE) } : null,
      here && (can("manage_roles") || can("ban_members") || can("manage_webhooks") || can("manage_guild") || can("view_audit_log")) ? { label: "Outhouse settings", onClick: () => openSettings(can("manage_roles") ? "roles" : can("manage_webhooks") ? "integrations" : can("manage_guild") ? "emoji" : can("ban_members") ? "bans" : "audit") } : null,
      here && can("manage_guild") ? { label: "Custom emoji", onClick: () => openSettings("emoji") } : null,
      ...(typeof notifyMenuItems === "function" ? ["sep", ...notifyMenuItems("guild", g)] : []),
      "sep",
      { label: "Copy outhouse ID", onClick: () => copy(g.id, "ID copied") },
      "sep",
      owner ? { label: "Delete outhouse", danger: true, onClick: () => deleteGuild(g) } : { label: "Leave outhouse", danger: true, onClick: () => leaveGuild(g) },
    ]);
  },

  member(e, node) {
    const m = memberById(node.dataset.id);
    if (!m) return;
    const assignable = state.roles.filter((r) => r.id !== state.guildId && myTop() > r.position).sort((a, b) => b.position - a.position);
    const items = [
      { label: "View profile", onClick: () => openProfile(m.id, node) },
      { label: "Mention", onClick: () => { input.value += `@${m.display_name} `; input.focus(); autosize(); } },
      "sep",
    ];
    if (m.id !== state.me.id && state.voice.room && state.voiceStates.get(m.id)?.channel_id === state.voice.channelId) {
      items.unshift({ custom: volumeSlider(m), label: "volume" },
        { label: "Mute for me", checked: isLocallyMuted(m.id), onClick: () => toggleLocalMute(m) }, "sep");
    }
    if (typeof voiceModerationItems === "function") items.push(...voiceModerationItems(m));
    if (can("manage_roles") && assignable.length) {
      items.push({
        label: "Roles",
        submenu: assignable.map((r) => ({
          label: r.name,
          swatch: hexColour(r.color) || "#a1a1aa",
          checked: m.role_ids.includes(r.id),
          keepOpen: false,
          onClick: () => tryApi(m.role_ids.includes(r.id) ? "DELETE" : "PUT", `/api/guilds/${state.guildId}/members/${m.id}/roles/${r.id}`).catch(() => {}),
        })),
      });
    }
    if (canModerate(m, "timeout_members")) {
      items.push({
        label: isTimedOut(m) ? "Timed out…" : "Time out",
        submenu: [
          ...TIMEOUTS.map(([label, minutes]) => ({ label, onClick: () => timeoutMember(m, minutes, label) })),
          isTimedOut(m) ? "sep" : null,
          isTimedOut(m) ? { label: "Remove timeout", onClick: () => timeoutMember(m, 0) } : null,
        ],
      });
    }
    items.push("sep");
    if (canModerate(m, "kick_members")) items.push({ label: `Kick ${m.display_name}`, danger: true, onClick: () => kickMember(m) });
    if (canModerate(m, "ban_members")) items.push({ label: `Ban ${m.display_name}`, danger: true, onClick: () => banMember(m) });
    items.push("sep", { label: "Copy user ID", onClick: () => copy(m.id, "ID copied") });
    openMenu(e.clientX, e.clientY, items);
  },
};

// ---------- outhouse / channel / member actions ----------

async function invitePeople() {
  if (typeof openInviteDialog === "function") return openInviteDialog();
  const res = await tryApi("POST", `/api/guilds/${state.guildId}/invites`).catch(() => null);
  if (res) await ask("Invite code (lasts 7 days)", { value: res.code, readOnly: true });
}

async function createChannel(kind = TEXT) {
  const voice = kind === VOICE;
  const name = await ask(voice ? "New voice channel" : "New text channel", { placeholder: voice ? "Raid comms" : "raid-signups", confirm: "Create" });
  if (!name) return;
  const c = await tryApi("POST", `/api/guilds/${state.guildId}/channels`, { name, kind }).catch(() => null);
  if (c) {
    if (!state.channels.some((x) => x.id === c.id)) state.channels.push(c);
    await refreshGuild();
    if (!voice) selectChannel(c.id);
  }
}

async function deleteChannel(c) {
  if (await confirmBox(`Delete #${c.name}?`, "All of its messages go with it. This can't be undone.", "Delete channel")) {
    await tryApi("DELETE", `/api/channels/${c.id}`).catch(() => {});
  }
}

async function leaveGuild(g) {
  if (await confirmBox(`Leave ${g.name}?`, "You'll need a new invite to get back in.", "Leave")) {
    await tryApi("POST", `/api/guilds/${g.id}/leave`).catch(() => {});
  }
}

async function deleteGuild(g) {
  const typed = await ask(`Delete ${g.name}?`, {
    placeholder: g.name,
    confirm: "Delete forever",
    body: el("p", { class: "muted", style: "margin:0" }, "Every channel and message will be gone for good. Type the outhouse's name to confirm."),
  });
  if (typed === g.name) await tryApi("DELETE", `/api/guilds/${g.id}`).catch(() => {});
  else if (typed) toast("The name didn't match, so nothing was deleted.");
}

async function timeoutMember(m, minutes, label) {
  await tryApi("POST", `/api/guilds/${state.guildId}/members/${m.id}/timeout`, { minutes }).catch(() => {});
  toast(minutes ? `${m.display_name} is timed out for ${label}.` : `${m.display_name}'s timeout is over.`);
}

async function kickMember(m) {
  if (await confirmBox(`Kick ${m.display_name}?`, "They can come back with a new invite.", "Kick")) {
    await tryApi("DELETE", `/api/guilds/${state.guildId}/members/${m.id}`).catch(() => {});
  }
}

async function banMember(m) {
  const reason = await ask(`Ban ${m.display_name}?`, {
    placeholder: "Reason (optional)",
    confirm: "Ban",
    allowEmpty: true,
    body: el("p", { class: "muted", style: "margin:0" }, "They'll be removed and can't rejoin until unbanned."),
  });
  if (reason === null) return;
  await tryApi("PUT", `/api/guilds/${state.guildId}/bans/${m.id}`, { reason: reason || null }).catch(() => {});
}

// ---------- settings: roles and bans ----------

// Each settings page draws into its own fresh box, so a page that's still
// loading (or that adds to what's there) can never end up under another one.
function settingsPage(main) {
  const page = el("div", { class: "settings-page" });
  main.replaceChildren(page);
  main.scrollTop = 0;
  return page;
}

function settingsShell(title, navItems, renderMain) {
  const nav = el("nav", { class: "settings-nav" });
  const main = el("section", { class: "settings-main" });
  const dialog = el("dialog", { class: "settings" }, nav, main,
    el("button", { class: "icon-btn close-x", "aria-label": "Close", onclick: () => dialog.close() }, "✕"));
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  return { dialog, nav, main, title };
}

async function openSettings(tab = "roles", selectedRoleId = null) {
  const shell = settingsShell("Outhouse settings");
  const { nav, main } = shell;
  let drawing = 0;
  const draw = async (activeTab, roleId) => {
    const mine = ++drawing;
    // Mark the tab straight away; the page follows once the roles are in.
    nav.querySelectorAll("button.active").forEach((b) => b.classList.remove("active"));
    const roles = await api("GET", `/api/guilds/${state.guildId}/roles`);
    if (mine !== drawing) return; // another tab was picked meanwhile
    state.roles = roles;
    nav.replaceChildren(el("h3", {}, currentGuild().name.toUpperCase()));
    if (can("manage_roles")) {
      nav.append(el("h3", {}, "ROLES"));
      for (const r of roles) {
        nav.append(el("button", {
          class: activeTab === "roles" && roleId === r.id ? "active" : "",
          onclick: () => draw("roles", r.id),
        }, el("span", { class: "swatch", style: `width:10px;height:10px;border-radius:50%;background:${hexColour(r.color) || "#a1a1aa"}` }), r.name));
      }
      nav.append(el("button", { onclick: async () => {
        const name = await ask("New role", { placeholder: "Raider", confirm: "Create" });
        if (!name) return;
        const role = await tryApi("POST", `/api/guilds/${state.guildId}/roles`, { name }).catch(() => null);
        if (role) draw("roles", role.id);
      } }, "＋ Create role"));
    }
    if (can("manage_webhooks") || can("manage_guild")) nav.append(el("h3", {}, "APPS"));
    if (can("manage_webhooks")) {
      nav.append(el("button", { class: activeTab === "integrations" ? "active" : "", onclick: () => draw("integrations") }, "Webhooks & news feeds"));
    }
    if (can("manage_guild")) {
      nav.append(el("button", { class: activeTab === "bots" ? "active" : "", onclick: () => draw("bots") }, "Bots"));
      nav.append(el("button", { class: activeTab === "emoji" ? "active" : "", onclick: () => draw("emoji") }, "Emoji"));
    }
    if (can("manage_guild") && typeof drawBadgeSettings === "function") {
      nav.append(el("button", { class: activeTab === "badges" ? "active" : "", onclick: () => draw("badges") }, "Badges"));
    }
    if (can("manage_guild") && typeof drawRaidSettings === "function") {
      nav.append(el("h3", {}, "RAIDS"), el("button", { class: activeTab === "raids" ? "active" : "", onclick: () => draw("raids") }, "Raids"));
    }
    const canAudit = (can("view_audit_log") || can("manage_guild")) && typeof drawAuditLog === "function";
    if (can("ban_members") || canAudit) nav.append(el("h3", {}, "MODERATION"));
    if (can("ban_members")) {
      nav.append(el("button", { class: activeTab === "bans" ? "active" : "", onclick: () => draw("bans") }, "Bans"));
    }
    if (can("manage_guild") && typeof drawInviteSettings === "function") {
      nav.append(el("button", { class: activeTab === "invites" ? "active" : "", onclick: () => draw("invites") }, "Invites"));
    }
    if (canAudit) {
      nav.append(el("button", { class: activeTab === "audit" ? "active" : "", onclick: () => draw("audit") }, "Audit log"));
    }
    if (state.siteOwner && typeof drawServerStatus === "function") {
      nav.append(el("h3", {}, "SERVER"), el("button", { class: activeTab === "status" ? "active" : "", onclick: () => draw("status") }, "Server status"));
    }
    const page = settingsPage(main);
    if (activeTab === "bans") return drawBans(page);
    if (activeTab === "audit") return drawAuditLog(page);
    if (activeTab === "invites") return drawInviteSettings(page);
    if (activeTab === "badges") return drawBadgeSettings(page);
    if (activeTab === "raids") return drawRaidSettings(page);
    if (activeTab === "status") return drawServerStatus(page);
    if (activeTab === "integrations") return drawIntegrations(page);
    if (activeTab === "bots") return drawBots(page);
    if (activeTab === "emoji") return drawEmojiSettings(page);
    const role = roles.find((r) => r.id === roleId) || roles.find((r) => r.id !== state.guildId) || roles[0];
    if (role) drawRole(page, role, (id) => draw("roles", id));
  };
  draw(tab, selectedRoleId);
}

// A ✕ / ✓ switch (or ✕ / inherit / ✓ when `inherit` is true).
function triSwitch(initial, { inherit = true, onChange, disabled = false }) {
  const wrap = el("div", { class: "tri" });
  let current = initial;
  const make = (value, cls, label, title) => {
    const b = el("button", { type: "button", class: cls, title, disabled }, label);
    b.addEventListener("click", () => { current = value; paint(); onChange(value); });
    return [value, b];
  };
  const buttons = [
    make("deny", "deny", "✕", "Off"),
    inherit ? make("inherit", "inherit", "/", "Inherit") : null,
    make("allow", "allow", "✓", "On"),
  ].filter(Boolean);
  const paint = () => buttons.forEach(([v, b]) => b.classList.toggle("on", v === current));
  buttons.forEach(([, b]) => wrap.append(b));
  paint();
  return wrap;
}

function drawRole(main, role, redraw) {
  const isEveryone = role.id === state.guildId;
  const editable = isEveryone || myTop() > role.position;
  const everyoneBits = big(state.roles.find((r) => r.id === state.guildId)?.permissions);
  let allow = big(role.permissions);
  let deny = big(role.deny_permissions);
  const name = el("input", { type: "text", value: role.name, disabled: isEveryone || !editable, maxlength: 32 });
  const colour = el("input", { type: "color", value: hexColour(role.color) || "#a1a1aa", disabled: isEveryone || !editable });
  let hoist = role.hoist;
  const hoistBtn = el("button", { type: "button", class: `toggle${hoist ? " on" : ""}`, disabled: isEveryone || !editable, "aria-label": "Show separately", onclick: () => { hoist = !hoist; hoistBtn.classList.toggle("on", hoist); } });

  const rows = state.permList.map((p) => {
    const bit = big(p.bit);
    const initial = (allow & bit) ? "allow" : (deny & bit) ? "deny" : isEveryone ? "deny" : "inherit";
    const inheritedOn = (everyoneBits & bit) === bit;
    const hint = el("span", { class: "inherit-hint" });
    const paintHint = (value) => {
      hint.textContent = !isEveryone && value === "inherit" ? (inheritedOn ? " · on from @everyone" : " · off from @everyone") : "";
    };
    paintHint(initial);
    const sw = triSwitch(initial, {
      inherit: !isEveryone,
      disabled: !editable,
      onChange: (value) => {
        allow &= ~bit;
        deny &= ~bit;
        if (value === "allow") allow |= bit;
        if (value === "deny" && !isEveryone) deny |= bit;
        paintHint(value);
      },
    });
    return el("div", { class: "perm-row" }, el("div", {}, el("div", { class: "name" }, p.name, hint), el("div", { class: "desc" }, p.description)), sw);
  });

  main.replaceChildren(
    el("h2", {}, isEveryone ? "@everyone" : `Edit role: ${role.name}`),
    el("p", { class: "muted", style: "margin:0" }, isEveryone
      ? "The starting point for everyone in the outhouse. ✓ on, ✕ off."
      : "✓ turns a permission on, ✕ turns it off, / keeps whatever @everyone (or a lower role) has. If someone's roles disagree, the higher role wins."),
    !editable ? el("p", { class: "error" }, "This role is ranked the same as your top role or higher, so you can't edit it.") : null,
    isEveryone ? null : el("label", { class: "field" }, "Role name", name),
    isEveryone ? null : el("div", { style: "display:flex;gap:24px;align-items:center" },
      el("label", { class: "field" }, "Colour", colour),
      el("div", { class: "perm-row", style: "border:0;flex:1" }, el("div", {}, el("div", { class: "name" }, "Show separately"), el("div", { class: "desc" }, "Group members with this role in the member list.")), hoistBtn)),
    el("h3", { style: "margin:8px 0 0" }, "Permissions"),
    ...rows,
    editable ? el("div", { class: "settings-actions" },
      isEveryone ? el("span") : el("button", { class: "danger-btn", onclick: async () => {
        if (await confirmBox(`Delete ${role.name}?`, "Members with it go back to their other roles' permissions.", "Delete role")) {
          await tryApi("DELETE", `/api/guilds/${state.guildId}/roles/${role.id}`).catch(() => {});
          redraw(null);
        }
      } }, "Delete role"),
      el("button", { class: "primary", onclick: async () => {
        const body = { permissions: allow.toString(), hoist };
        if (!isEveryone) {
          body.name = name.value;
          body.color = parseInt(colour.value.slice(1), 16);
          body.deny_permissions = deny.toString();
        }
        const saved = await tryApi("PATCH", `/api/guilds/${state.guildId}/roles/${role.id}`, body).catch(() => null);
        if (saved) { toast("Role saved."); redraw(saved.id); }
      } }, "Save changes"),
    ) : null,
  );
}

async function drawBans(main) {
  const bans = await tryApi("GET", `/api/guilds/${state.guildId}/bans`).catch(() => []);
  main.replaceChildren(
    el("h2", {}, "Bans"),
    bans.length ? null : el("p", { class: "muted" }, "Nobody's banned. Lovely."),
    ...bans.map((b) => el("div", { class: "ban-row" },
      el("div", {}, el("div", { style: "font-weight:600" }, b.display_name, " ", el("span", { class: "muted" }, `@${b.username}`)), b.reason ? el("div", { class: "muted" }, b.reason) : null),
      el("button", { class: "ghost", onclick: async () => {
        await tryApi("DELETE", `/api/guilds/${state.guildId}/bans/${b.id}`).catch(() => {});
        drawBans(main);
      } }, "Unban"))),
  );
}

// ---------- settings: webhooks and news feeds ----------

const FEED_PRESETS = [
  ["Wowhead: all news", "https://www.wowhead.com/news/rss/all"],
  ["Wowhead: Retail", "https://www.wowhead.com/news/rss/retail"],
  ["Wowhead: Classic", "https://www.wowhead.com/news/rss/classic-series"],
];

function channelPicker(selected) {
  const textChannels = state.channels.filter((c) => c.kind === TEXT && can("manage_webhooks", c.id));
  return el("select", { class: "select" }, textChannels.map((c) => el("option", { value: c.id, selected: c.id === selected }, `#${c.name}`)));
}

const channelName = (id) => `#${state.channels.find((c) => c.id === id)?.name ?? "deleted-channel"}`;

function relTime(iso) {
  if (!iso) return "not yet";
  const mins = Math.round((Date.now() - new Date(iso)) / 60000);
  return mins < 1 ? "just now" : mins < 60 ? `${mins} min ago` : `${Math.round(mins / 60)} h ago`;
}

async function drawIntegrations(main) {
  const [hooks, feeds] = await Promise.all([
    tryApi("GET", `/api/guilds/${state.guildId}/webhooks`).catch(() => []),
    tryApi("GET", `/api/guilds/${state.guildId}/feeds`).catch(() => []),
  ]);
  const redraw = () => drawIntegrations(main);

  // Webhooks
  const hookName = el("input", { type: "text", placeholder: "Raider.IO alerts", maxlength: 32 });
  const hookChannel = channelPicker(state.channelId);
  const hookRows = hooks.map((h) => {
    const url = `${location.origin}/api/webhooks/${h.id}/${h.token}`;
    return el("div", { class: "int-row" },
      el("div", { class: "int-info" }, el("div", { class: "name" }, h.name), el("div", { class: "muted small" }, `Posts in ${channelName(h.channel_id)}`)),
      el("button", { class: "ghost", onclick: () => copy(url, "Webhook URL copied") }, "Copy URL"),
      el("button", { class: "danger-btn", onclick: async () => {
        if (await confirmBox(`Delete ${h.name}?`, "Anything using its URL will stop working.", "Delete")) {
          await tryApi("DELETE", `/api/webhooks/${h.id}`).catch(() => {});
          redraw();
        }
      } }, "Delete"));
  });

  // Feeds
  const feedUrl = el("input", { type: "text", placeholder: "https://example.com/feed.xml" });
  const preset = el("select", { class: "select", onchange: () => { if (preset.value) feedUrl.value = preset.value; } },
    el("option", { value: "" }, "Pick a preset…"), FEED_PRESETS.map(([label, url]) => el("option", { value: url }, label)));
  const feedChannel = channelPicker(state.channelId);
  const include = el("input", { type: "text", placeholder: "e.g. raid, patch notes" });
  const exclude = el("input", { type: "text", placeholder: "e.g. PTR, sponsored" });
  const addFeedBtn = el("button", { class: "primary" }, "Follow feed");
  addFeedBtn.addEventListener("click", async () => {
    if (!feedUrl.value.trim()) return toast("Enter a feed address or pick a preset.");
    addFeedBtn.disabled = true;
    addFeedBtn.textContent = "Checking feed…";
    const ok = await tryApi("POST", `/api/channels/${feedChannel.value}/feeds`, {
      url: feedUrl.value.trim(), include_keywords: include.value, exclude_keywords: exclude.value,
    }).catch(() => null);
    if (ok) { toast(`Following ${ok.title}. The newest post is on its way.`); redraw(); }
    else { addFeedBtn.disabled = false; addFeedBtn.textContent = "Follow feed"; }
  });
  const feedRows = feeds.map((f) => el("div", { class: "int-row" },
    el("div", { class: "int-info" },
      el("div", { class: "name" }, f.title),
      el("div", { class: "muted small" }, `${channelName(f.channel_id)} · checked ${relTime(f.last_checked_at)}`,
        f.include_keywords ? ` · only: ${f.include_keywords}` : "", f.exclude_keywords ? ` · skip: ${f.exclude_keywords}` : ""),
      f.last_error ? el("div", { class: "error small" }, f.last_error) : null),
    el("button", { class: "ghost", onclick: async () => {
      const res = await tryApi("POST", `/api/feeds/${f.id}/check`).catch(() => null);
      if (res) { toast(res.last_error ? "Check failed." : "Checked. Anything new has been posted."); redraw(); }
    } }, "Check now"),
    el("button", { class: "danger-btn", onclick: async () => {
      if (await confirmBox(`Stop following ${f.title}?`, "Posts already in the channel stay.", "Unfollow")) {
        await tryApi("DELETE", `/api/feeds/${f.id}`).catch(() => {});
        redraw();
      }
    } }, "Unfollow")));

  main.replaceChildren(
    el("h2", {}, "News feeds"),
    el("p", { class: "muted", style: "margin:0" }, "Follow an RSS or Atom feed and new posts appear in a channel (checked every 10 minutes). Filters match words in the title or summary; separate several with commas."),
    ...(feedRows.length ? feedRows : [el("p", { class: "muted" }, "No feeds yet.")]),
    el("div", { class: "int-form" },
      el("label", { class: "field" }, "Preset", preset),
      el("label", { class: "field" }, "Feed address", feedUrl),
      el("label", { class: "field" }, "Post in", feedChannel),
      el("label", { class: "field" }, "Only posts mentioning (optional)", include),
      el("label", { class: "field" }, "Skip posts mentioning (optional)", exclude),
      el("div", {}, addFeedBtn)),
    el("h2", { style: "margin-top:16px" }, "Webhooks"),
    el("p", { class: "muted", style: "margin:0" }, "A webhook URL lets another app post into a channel. It uses Discord's format, so anything with a \"Discord webhook\" option works: paste the Outhouse URL there instead. Keep URLs private: anyone with one can post."),
    ...(hookRows.length ? hookRows : [el("p", { class: "muted" }, "No webhooks yet.")]),
    el("div", { class: "int-form" },
      el("label", { class: "field" }, "Name", hookName),
      el("label", { class: "field" }, "Post in", hookChannel),
      el("div", {}, el("button", { class: "primary", onclick: async () => {
        const name = hookName.value.trim();
        if (!name) return toast("Give the webhook a name.");
        const h = await tryApi("POST", `/api/channels/${hookChannel.value}/webhooks`, { name }).catch(() => null);
        if (h) {
          await copy(`${location.origin}/api/webhooks/${h.id}/${h.token}`, "Webhook created and its URL copied");
          redraw();
        }
      } }, "Create webhook"))),
  );
}

// ---------- settings: channel permissions ----------

async function openChannelPermissions(channel) {
  const { nav, main } = settingsShell(`#${channel.name}`);
  const draw = async (targetId) => {
    const overwrites = await tryApi("GET", `/api/channels/${channel.id}/permissions`).catch(() => null);
    if (!overwrites) return;
    const roles = state.roles.slice().sort((a, b) => b.position - a.position);
    const targets = [];
    const everyone = roles.find((r) => r.id === state.guildId);
    targets.push({ id: everyone.id, kind: "role", label: "@everyone" });
    for (const o of overwrites) {
      if (o.target_id === state.guildId) continue;
      const label = o.kind === "role" ? roleById(o.target_id)?.name : memberById(o.target_id)?.display_name;
      targets.push({ id: o.target_id, kind: o.kind, label: label || "(gone)" });
    }
    const current = targets.find((t) => t.id === targetId) || targets[0];

    nav.replaceChildren(el("h3", {}, `#${channel.name.toUpperCase()}`), el("h3", {}, "ROLES & MEMBERS"),
      ...targets.map((t) => el("button", { class: t.id === current.id ? "active" : "", onclick: () => draw(t.id) }, t.kind === "member" ? "👤 " : "", t.label)));
    const missingRoles = roles.filter((r) => !targets.some((t) => t.id === r.id));
    if (missingRoles.length) {
      nav.append(el("button", { onclick: (e) => {
        const r = e.currentTarget.getBoundingClientRect();
        openMenu(r.left, r.bottom, missingRoles.map((role) => ({ label: role.name, swatch: hexColour(role.color) || "#a1a1aa", onClick: async () => {
          await tryApi("PUT", `/api/channels/${channel.id}/permissions/${role.id}`, { kind: "role", allow: "0", deny: "0" }).catch(() => {});
          draw(role.id);
        } })));
      } }, "＋ Add a role"));
    }
    const missingMembers = state.members.filter((m) => !targets.some((t) => t.id === m.id));
    if (missingMembers.length) {
      nav.append(el("button", { onclick: (e) => {
        const r = e.currentTarget.getBoundingClientRect();
        openMenu(r.left, r.bottom, missingMembers.map((m) => ({ label: m.display_name, onClick: async () => {
          await tryApi("PUT", `/api/channels/${channel.id}/permissions/${m.id}`, { kind: "member", allow: "0", deny: "0" }).catch(() => {});
          draw(m.id);
        } })));
      } }, "＋ Add a member"));
    }

    const existing = overwrites.find((o) => o.target_id === current.id);
    let allow = big(existing?.allow);
    let deny = big(existing?.deny);
    const rows = state.permList.filter((p) => p.channel).map((p) => {
      const bit = big(p.bit);
      const tri = el("div", { class: "tri" });
      const setState = (s) => {
        allow &= ~bit;
        deny &= ~bit;
        if (s === "allow") allow |= bit;
        if (s === "deny") deny |= bit;
        paint();
      };
      const buttons = {
        deny: el("button", { type: "button", class: "deny", title: "Deny", onclick: () => setState("deny") }, "✕"),
        inherit: el("button", { type: "button", class: "inherit", title: "Inherit", onclick: () => setState("inherit") }, "/"),
        allow: el("button", { type: "button", class: "allow", title: "Allow", onclick: () => setState("allow") }, "✓"),
      };
      const paint = () => {
        const s = (allow & bit) ? "allow" : (deny & bit) ? "deny" : "inherit";
        for (const [k, b] of Object.entries(buttons)) b.classList.toggle("on", k === s);
      };
      tri.append(buttons.deny, buttons.inherit, buttons.allow);
      paint();
      return el("div", { class: "perm-row" }, el("div", {}, el("div", { class: "name" }, p.name), el("div", { class: "desc" }, p.description)), tri);
    });

    main.replaceChildren(
      el("h2", {}, `#${channel.name}: ${current.label}`),
      el("p", { class: "muted", style: "margin:0" }, "✕ takes a permission away in this channel, ✓ grants it, / uses the role's normal setting. Tip: to make a private channel, deny View channels for @everyone and allow it for a role."),
      ...rows,
      el("div", { class: "settings-actions" },
        current.id === state.guildId && !existing ? el("span") : el("button", { class: "danger-btn", onclick: async () => {
          await tryApi("DELETE", `/api/channels/${channel.id}/permissions/${current.id}`).catch(() => {});
          draw(null);
        } }, current.id === state.guildId ? "Reset" : "Remove"),
        el("button", { class: "primary", onclick: async () => {
          const ok = await tryApi("PUT", `/api/channels/${channel.id}/permissions/${current.id}`, { kind: current.kind, allow: allow.toString(), deny: deny.toString() }).catch(() => null);
          if (ok) toast("Channel permissions saved.");
        } }, "Save changes"),
      ),
    );
  };
  draw(null);
}

// ---------- buttons in the sidebar ----------

$("new-guild").addEventListener("click", async () => {
  const name = await ask("Build a new outhouse", { placeholder: "The Workshop", confirm: "Build it" });
  if (!name) return;
  const g = await tryApi("POST", "/api/guilds", { name }).catch(() => null);
  if (g) {
    if (!state.guilds.some((x) => x.id === g.id)) state.guilds.push(g);
    selectGuild(g.id);
  }
});

$("join-guild").addEventListener("click", async () => {
  const typed = await ask("Join an outhouse", { placeholder: "Invite link or code", confirm: "Join" });
  const code = typed && (typeof inviteCodeFrom === "function" ? inviteCodeFrom(typed) : typed);
  if (!code) return;
  const g = await tryApi("POST", `/api/invites/${encodeURIComponent(code)}`).catch(() => null);
  if (g) {
    if (!state.guilds.some((x) => x.id === g.id)) state.guilds.push(g);
    selectGuild(g.id);
  }
});

$("invite-btn").addEventListener("click", () => invitePeople());
$("new-channel").addEventListener("click", () => createChannel(TEXT));
$("guild-name").addEventListener("click", (e) => {
  if (!state.guildId) return;
  const r = e.currentTarget.getBoundingClientRect();
  MENUS.guild({ clientX: r.left, clientY: r.bottom + 6 }, { dataset: { id: state.guildId } });
});
$("guild-name").style.cursor = "pointer";
$("guild-name").title = "Outhouse menu";

// ---------- start ----------

async function start() {
  $("auth").hidden = true;
  $("app").hidden = false;
  try {
    state.permList = await api("GET", "/api/permissions");
    state.P = Object.fromEntries(state.permList.map((p) => [p.key, big(p.bit)]));
  } catch (err) {
    toast(err.message);
  }
  connect();
}

setAuthMode(false);
// Wait for features.js and chat-extras.js before anything can call into them.
function boot() {
  if (state.token) start();
  else showAuth();
}
if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot);
else boot();
