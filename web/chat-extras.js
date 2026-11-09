// Outhouse web client: pinned messages, message search, unread markers and
// mention counts. Loaded after app.js and features.js (uses their globals).
"use strict";

// ======================================================================
// Unread markers and mention counts
// ======================================================================

state.unread = new Map(); // channelId -> { guild_id, last_message_id, last_read_id, mentions, direct }
// Threads with replies you haven't read: threadId -> { channel_id, guild_id, last_message_id, last_read_id, mentions }
state.threadUnread = new Map();
state.myRoles = new Map(); // guildId -> [roleId] (for spotting role mentions in other outhouses)
state.detached = false; // looking at older messages (from search or pins)
state.flashId = null; // message to highlight after jumping to it

let unreadTimer = null;
// Re-fetches unread state from the server (debounced: several events can arrive at once).
function loadUnread() {
  clearTimeout(unreadTimer);
  unreadTimer = setTimeout(async () => {
    const res = await api("GET", "/api/users/@me/unread").catch(() => null);
    if (!res) return;
    state.unread = new Map(res.channels.map((c) => [c.channel_id, {
      guild_id: c.guild_id,
      last_message_id: c.last_message_id,
      last_read_id: c.last_read_id,
      mentions: c.mention_count,
      direct: c.direct_count ?? 0,
    }]));
    state.threadUnread = new Map((res.threads ?? []).map((t) => [t.thread_id, {
      channel_id: t.channel_id,
      guild_id: t.guild_id,
      last_message_id: t.last_message_id,
      last_read_id: t.last_read_id,
      mentions: t.mention_count,
    }]));
    state.myRoles = new Map(res.roles.map((r) => [r.guild_id, r.role_ids]));
    // Keep the open outhouse's channel list in step (another device may have read things).
    for (const ch of state.channels) {
      const u = state.unread.get(ch.id);
      if (u?.last_read_id && (!ch.last_read_id || idGt(u.last_read_id, ch.last_read_id))) ch.last_read_id = u.last_read_id;
    }
    refreshUnreadUi();
  }, 150);
}

function mentionsMeIn(m, guildId) {
  if (m.author_id === state.me.id) return false;
  if (m.reply_to?.author_id === state.me.id) return true;
  if (m.content.includes(`<@${state.me.id}>`) || /(^|\s)@(everyone|here)\b/.test(m.content)) return true;
  const roles = state.myRoles.get(guildId) ?? (guildId === state.guildId ? memberById(state.me.id)?.role_ids : null) ?? [];
  return roles.some((id) => m.content.includes(`<@&${id}>`));
}

// A new message arrived (any outhouse).
function noteUnreadMessage(m) {
  const u = state.unread.get(m.channel_id);
  if (!u) { loadUnread(); return; } // a channel we haven't heard of yet
  u.last_message_id = m.id;
  const looking = m.channel_id === state.channelId && document.hasFocus() && !state.detached;
  if (m.author_id === state.me.id) {
    u.last_read_id = m.id;
    u.mentions = 0;
    u.direct = 0;
  } else if (mentionsMeIn(m, u.guild_id) && !looking) {
    u.mentions += 1;
    // In a DM: does it @mention you (what still counts while it's muted)?
    if (u.guild_id === "0" && pingsMe(m, u.guild_id)) u.direct = (u.direct ?? 0) + 1;
  }
  refreshUnreadUi();
}

// An @mention of you, your roles, @everyone/@here, or a reply to you
// (for thread replies and muted DMs, where mentionsMeIn doesn't apply).
function pingsMe(m, guildId) {
  if (m.author_id === state.me.id) return false;
  if (m.reply_to?.author_id === state.me.id) return true;
  if (m.content.includes(`<@${state.me.id}>`) || /(^|\s)@(everyone|here)\b/.test(m.content)) return true;
  const roles = state.myRoles.get(guildId) ?? (guildId === state.guildId ? memberById(state.me.id)?.role_ids : null) ?? [];
  return roles.some((id) => m.content.includes(`<@&${id}>`));
}

// This client read a channel up to a message.
function noteChannelRead(channelId, lastId) {
  const u = state.unread.get(channelId);
  if (!u) return;
  if (!u.last_read_id || idGt(lastId, u.last_read_id)) u.last_read_id = lastId;
  if (!u.last_message_id || !idGt(u.last_message_id, u.last_read_id)) { u.mentions = 0; u.direct = 0; }
  refreshUnreadUi();
}

// One of your clients (maybe another device) marked a channel read.
function onChannelAck(d) {
  const ch = state.channels.find((c) => c.id === d.channel_id);
  if (ch && (!ch.last_read_id || idGt(d.last_read_id, ch.last_read_id))) ch.last_read_id = d.last_read_id;
  const u = state.unread.get(d.channel_id);
  if (!u) return;
  if (!u.last_read_id || idGt(d.last_read_id, u.last_read_id)) u.last_read_id = d.last_read_id;
  if (u.last_message_id && idGt(u.last_message_id, u.last_read_id) && u.mentions) loadUnread(); // only partly read: recount
  else { u.mentions = 0; u.direct = 0; }
  refreshUnreadUi();
}

// What a channel's own unread count adds up to: a muted DM only counts
// messages that @mention you (they still badge, but never notify).
function ownCount(channelId, u) {
  if (!u) return 0;
  if (u.guild_id === "0" && typeof notifyMuted === "function" && notifyMuted(channelId)) return u.direct ?? 0;
  return u.mentions;
}

// Unread thread replies that mention you, in one channel.
function threadMentionsIn(channelId) {
  let n = 0;
  for (const t of state.threadUnread.values()) if (t.channel_id === channelId) n += t.mentions;
  return n;
}

// A channel's badge: its own mentions plus those in its threads.
const channelMentions = (channelId) => ownCount(channelId, state.unread.get(channelId)) + threadMentionsIn(channelId);

function guildMentions(guildId) {
  let n = 0;
  for (const [id, u] of state.unread) if (u.guild_id === guildId) n += ownCount(id, u);
  for (const t of state.threadUnread.values()) if (t.guild_id === guildId) n += t.mentions;
  return n;
}

function guildUnread(guildId) {
  if (guildId === state.guildId) return state.channels.some((c) => isUnread(c));
  for (const u of state.unread.values()) {
    if (u.guild_id === guildId && u.last_message_id && (!u.last_read_id || idGt(u.last_message_id, u.last_read_id))) return true;
  }
  return false;
}

const countLabel = (n) => (n > 99 ? "99+" : String(n));
const mentionsTitle = (n) => `${n} unread mention${n === 1 ? "" : "s"}`;

// The red count on an outhouse in the left rail (or the plain unread dot).
function guildBadge(guildId) {
  const n = guildMentions(guildId);
  if (n) return el("span", { class: "mention-badge rail-badge", title: mentionsTitle(n) }, countLabel(n));
  return guildId !== state.guildId && guildUnread(guildId) ? el("span", { class: "badge" }) : null;
}

// The red count next to a channel name.
function channelBadge(channelId) {
  const n = channelMentions(channelId);
  return n ? el("span", { class: "mention-badge", title: mentionsTitle(n) }, countLabel(n)) : null;
}

function updateTitle() {
  let total = 0;
  for (const [id, u] of state.unread) total += ownCount(id, u);
  for (const t of state.threadUnread.values()) total += t.mentions;
  const title = total ? `(${countLabel(total)}) Outhouse` : "Outhouse";
  if (document.title !== title) document.title = title;
  // On phones the outhouse list is tucked away, so the ☰ button gets a red dot.
  $("nav-toggle")?.classList.toggle("has-mentions", total > 0);
}

function refreshUnreadUi() {
  if (!state.me) return;
  renderGuilds();
  if (state.guildId) renderChannels();
  updateTitle();
}

// ======================================================================
// Side panel (pins and search results)
// ======================================================================

const sidePanel = { kind: null, node: null, body: null, guildId: null, query: "" };

function showSidePanel(kind, title, ...extra) {
  closeSidePanel();
  const body = el("div", { class: "sp-body" });
  const node = el("aside", { class: `side-panel ${kind}-panel`, "aria-label": title },
    el("header", { class: "sp-head" },
      el("span", { class: "sp-title" }, title),
      el("button", { class: "icon-btn", title: "Close", "aria-label": "Close", onclick: closeSidePanel }, "✕")),
    ...extra,
    body);
  // Sits beside the chat (in place of the member list); on phones it covers the chat below the header.
  document.querySelector("main.chat").after(node);
  $("app").classList.add("panel-open");
  Object.assign(sidePanel, { kind, node, body, guildId: state.guildId });
  document.querySelectorAll(".chat-tools [data-panel]").forEach((b) => b.classList.toggle("on", b.dataset.panel === kind));
  return body;
}

function closeSidePanel() {
  sidePanel.node?.remove();
  $("app").classList.remove("panel-open");
  Object.assign(sidePanel, { kind: null, node: null, body: null });
  document.querySelectorAll(".chat-tools [data-panel]").forEach((b) => b.classList.remove("on"));
}

const isPhone = () => matchMedia("(max-width: 760px)").matches;
const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

// Message text with the searched-for words marked.
function highlighted(text, terms) {
  let plain = plainMentions(text);
  if (plain.length > 400) plain = plain.slice(0, 400) + "…";
  if (!terms?.length) return plain;
  const re = new RegExp(`(?<![\\p{L}\\p{N}])(${terms.map(escapeRe).join("|")})`, "giu");
  return plain.split(re).map((part, i) => (i % 2 ? el("mark", {}, part) : part));
}

// One message in the pins or search list; clicking it jumps there.
function messageCard(m, { terms = null, showChannel = false, unpin = false } = {}) {
  const ch = state.channels.find((c) => c.id === m.channel_id);
  const image = m.attachments.find((a) => a.content_type.startsWith("image/")) ?? null;
  const imageEmbed = !image ? m.embeds.find((e) => e.kind === "image") : null;
  const imageOnly = imageEmbed && m.content.trim() === imageEmbed.url;
  const others = m.attachments.length - (image ? 1 : 0);
  const node = el("div", { class: "sp-card", role: "button", tabindex: "0", "data-id": m.id, onclick: () => jumpToMessage(m.channel_id, m.id) },
    el("div", { class: "sp-meta" },
      avatarFor(m.author_id, authorName(m), 22, m.author_bot && !memberById(m.author_id) ? m.author_avatar_url || undefined : undefined),
      el("strong", { style: memberColour(m.author_id) ? `color:${memberColour(m.author_id)}` : "" }, authorName(m)),
      showChannel ? el("span", { class: "sp-chan" }, `#${ch?.name ?? "channel"}`) : null,
      el("time", { class: "sp-time", datetime: m.created_at, title: new Date(m.created_at).toLocaleString() }, timeLabel(m.created_at)),
      unpin ? el("button", { class: "sp-x", title: "Unpin", "aria-label": "Unpin", onclick: (e) => { e.stopPropagation(); togglePin(m); } }, "✕") : null),
    m.content && !imageOnly ? el("div", { class: "sp-text" }, terms ? highlighted(m.content, terms) : renderText(m.content)) : null,
    m.event ? el("div", { class: "sp-text muted" }, `📅 ${m.event.title}`) : null,
    image || imageEmbed ? el("img", { class: "sp-thumb", src: image ? image.url : imageEmbed.image_url, alt: "", loading: "lazy" }) : null,
    others > 0 ? el("div", { class: "sp-files" }, `📎 ${others} file${others === 1 ? "" : "s"}`) : null);
  node.addEventListener("keydown", (e) => { if (e.key === "Enter") node.click(); });
  return node;
}

// ======================================================================
// Pinned messages
// ======================================================================

async function togglePin(m) {
  const unpin = !!m.pinned_at;
  const msg = await tryApi(unpin ? "DELETE" : "PUT", `/api/channels/${m.channel_id}/pins/${m.id}`).catch(() => null);
  if (!msg) return;
  applyMessageUpdate(msg);
  toast(unpin ? "Unpinned." : "Pinned. It's under 📌 Pins.");
  if (sidePanel.kind === "pins") drawPins();
}

function openPins() {
  if (sidePanel.kind === "pins") return closeSidePanel();
  if (!state.channelId) return;
  showSidePanel("pins", "📌 Pinned messages");
  drawPins();
}

async function drawPins() {
  const channelId = state.channelId;
  const body = sidePanel.body;
  if (!body || !channelId) return;
  if (!body.childElementCount) body.append(el("p", { class: "sp-note" }, "Loading…"));
  const pins = await api("GET", `/api/channels/${channelId}/pins`).catch((err) => { toast(err.message); return null; });
  if (sidePanel.kind !== "pins" || sidePanel.body !== body || state.channelId !== channelId || !pins) return;
  const ch = state.channels.find((c) => c.id === channelId);
  const canUnpin = can("manage_messages", channelId);
  body.replaceChildren(
    pins.length
      ? el("p", { class: "sp-note" }, `${pins.length} pinned in #${ch?.name ?? "channel"}${pins.length >= 50 ? " (that's the limit of 50)" : ""}`)
      : el("div", { class: "sp-empty" },
        el("div", { class: "sp-empty-icon" }, "📌"),
        el("p", {}, `Nothing pinned in #${ch?.name ?? "this channel"} yet.`),
        el("p", { class: "muted" }, canUnpin ? "Pin the good stuff from a message's ⋯ or right-click menu." : "People who can manage messages can pin the good stuff here.")),
    ...pins.map((m) => messageCard(m, { unpin: canUnpin })),
  );
}

// A message changed or went away: keep an open pins list current.
let pinsRedraw = null;
function onMessageChanged(channelId) {
  if (sidePanel.kind !== "pins" || channelId !== state.channelId) return;
  clearTimeout(pinsRedraw);
  pinsRedraw = setTimeout(drawPins, 200);
}

// ======================================================================
// Search
// ======================================================================

const SEARCH_PAGE = 25;
const searchHint = () => el("p", { class: "sp-note" },
  "Filters: ", el("code", {}, "from:@name"), " ", el("code", {}, "in:#channel"), " ", el("code", {}, "has:image"),
  " ", el("code", {}, "has:file"), " ", el("code", {}, "has:link"));

function openSearchPanel(focus = true) {
  if (sidePanel.kind === "search") {
    if (focus) (isPhone() ? sidePanel.node.querySelector(".sp-search input") : $("search-input"))?.focus();
    return sidePanel.body;
  }
  const input = el("input", { type: "search", placeholder: "Search this outhouse", "aria-label": "Search messages", value: sidePanel.query || "" });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); runSearch(input.value); }
  });
  const body = showSidePanel("search", "Search", el("div", { class: "sp-search" }, input));
  body.append(searchHint());
  if (focus && isPhone()) setTimeout(() => input.focus(), 50);
  return body;
}

async function runSearch(raw, offset = 0) {
  const q = raw.trim();
  if (!q || !state.guildId) return;
  const guildId = state.guildId;
  sidePanel.query = q;
  const desk = $("search-input");
  if (desk && desk.value !== q) desk.value = q;
  const body = openSearchPanel(false);
  const panelInput = sidePanel.node.querySelector(".sp-search input");
  if (panelInput && panelInput.value !== q) panelInput.value = q;
  if (!offset) body.replaceChildren(el("p", { class: "sp-note" }, "Searching…"));
  const res = await api("GET", `/api/guilds/${guildId}/search?q=${encodeURIComponent(q)}&limit=${SEARCH_PAGE}&offset=${offset}`)
    .catch((err) => { toast(err.message); return null; });
  if (sidePanel.kind !== "search" || sidePanel.body !== body || sidePanel.query !== q || state.guildId !== guildId) return;
  if (!res) { body.replaceChildren(searchHint()); return; }
  body.querySelector(".sp-more")?.remove();
  const cards = res.messages.map((m) => messageCard(m, { terms: res.terms, showChannel: true }));
  if (!offset) {
    body.replaceChildren(
      el("p", { class: "sp-note sp-count" }, res.total ? `${res.total} result${res.total === 1 ? "" : "s"}` : "No messages found."),
      ...(res.total ? [] : [el("div", { class: "sp-empty" }, el("div", { class: "sp-empty-icon" }, "🔍"), el("p", { class: "muted" }, "Nothing down the long drop. Try fewer words or a different filter.")), searchHint()]),
      ...cards);
  } else {
    body.append(...cards);
  }
  const shown = offset + res.messages.length;
  if (shown < res.total && res.messages.length) {
    body.append(el("button", { class: "ghost sp-more", onclick: () => runSearch(q, shown) }, "Load more"));
  }
}

// ======================================================================
// Jumping to a message (search results, pins, replies further back)
// ======================================================================

function setDetached(on) {
  state.detached = on;
  let bar = $("detached-bar");
  if (!bar && on) {
    bar = el("div", { id: "detached-bar", class: "detached-bar" },
      el("span", {}, "You're looking at older messages."),
      el("button", { class: "ghost", onclick: () => { if (state.channelId) selectChannel(state.channelId, { toPresent: true }); } }, "Jump to present"));
    $("composer-wrap").prepend(bar);
  }
  if (bar) bar.hidden = !on;
}

let flashTimer = null;
function flashMessage(id) {
  const node = $(`msg-${id}`);
  if (!node) return;
  state.flashId = id;
  node.classList.add("flash");
  node.scrollIntoView({ block: "center" });
  clearTimeout(flashTimer);
  flashTimer = setTimeout(() => {
    state.flashId = null;
    $(`msg-${id}`)?.classList.remove("flash");
  }, 2600);
}

async function jumpToMessage(channelId, messageId) {
  if (isPhone()) closeSidePanel(); // the panel covers the chat on phones
  if (state.channelId !== channelId) {
    if (!state.channels.some((c) => c.id === channelId)) return toast("That channel's gone.");
    await selectChannel(channelId);
    if (state.channelId !== channelId) return;
  }
  if (!$(`msg-${messageId}`)) {
    const msgs = await api("GET", `/api/channels/${channelId}/messages?around=${messageId}&limit=60`).catch((err) => { toast(err.message); return null; });
    if (!msgs || state.channelId !== channelId) return;
    if (!msgs.some((m) => m.id === messageId)) return toast("That message has been flushed (deleted).");
    const ch = state.channels.find((c) => c.id === channelId);
    const newest = msgs[msgs.length - 1];
    state.messages = msgs;
    setDetached(!!(ch?.last_message_id && idGt(ch.last_message_id, newest.id)));
    renderMessages();
  }
  flashMessage(messageId);
}

// Called by selectChannel whenever a channel opens.
function onChannelChanged() {
  setDetached(false);
  if (sidePanel.kind === "search" && sidePanel.guildId !== state.guildId) closeSidePanel();
  if (sidePanel.kind === "pins") drawPins();
}

// ======================================================================
// Header buttons
// ======================================================================

(function chatTools() {
  const head = document.querySelector(".chat-head");
  if (!head) return;
  const input = el("input", { id: "search-input", type: "search", placeholder: "Search", "aria-label": "Search messages", autocomplete: "off" });
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); runSearch(input.value); }
    if (e.key === "Escape") { input.value = ""; input.blur(); if (sidePanel.kind === "search") closeSidePanel(); }
  });
  const tools = el("div", { class: "chat-tools" },
    el("button", { class: "tool-btn", id: "pins-btn", "data-panel": "pins", title: "Pinned messages", "aria-label": "Pinned messages", onclick: openPins },
      "📌", el("span", { class: "tool-label" }, "Pins")),
    el("label", { class: "search-box" }, el("span", { class: "search-icon", "aria-hidden": "true" }, "🔍"), input),
    el("button", { class: "icon-btn mobile-only", id: "search-toggle", "data-panel": "search", title: "Search", "aria-label": "Search", onclick: () => (sidePanel.kind === "search" ? closeSidePanel() : openSearchPanel(true)) }, "🔍"));
  head.insertBefore(tools, $("conn-status"));
})();

// Coming back to the window: re-check counts in case the connection dropped.
window.addEventListener("focus", () => { if (state.me && state.unread.size === 0) loadUnread(); });
