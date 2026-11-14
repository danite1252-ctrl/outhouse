// Outhouse: the Mentions inbox. An @ button in the chat header (with your
// unread count) opens the messages that pinged you in the last 30 days,
// across every outhouse and DM: @you, one of your roles, @everyone/@here,
// or a reply to you. Grouped by day, newest first, with Jump, Mark as read
// and Mark all read, and filters (all or unread; one outhouse or DMs).
// The server keeps the list (api/mentions.rs) and only ever returns what
// you can still see. Loaded after chat-extras.js and myraids.js (uses
// showSidePanel, closeSidePanel, goToMessage, mrGuildIcon, …).

const MN_PAGE = 25;
const MN_ICON = '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 13.5V18a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-4.5"/><path d="M3 13.5 5.6 5.4A2 2 0 0 1 7.5 4h9a2 2 0 0 1 1.9 1.4l2.6 8.1"/><path d="M3 13.5h5l1.5 2.5h5l1.5-2.5h5"/></svg>';
const MN_KIND = { user: "mentioned you", role: "mentioned your role", everyone: "pinged everyone", reply: "replied to you" };

state.mentionInbox = { unread: 0, latest: [], latestAt: 0 };
const mnPanel = { filter: "all", guild: "", items: [], hasMore: false, loading: false, seq: 0 };

// ---------- the count ----------

let mnCountTimer = null;
function loadMentionCount(delay = 250) {
  clearTimeout(mnCountTimer);
  mnCountTimer = setTimeout(async () => {
    if (!state.me) return;
    const res = await api("GET", "/api/users/@me/mentions/count").catch(() => null);
    if (!res) return;
    state.mentionInbox.unread = res.unread;
    paintMentionsButton();
  }, delay);
}

function paintMentionsButton() {
  const btn = $("mentions-btn");
  if (!btn) return;
  const n = state.mentionInbox.unread;
  btn.querySelector(".mention-badge")?.remove();
  btn.title = n ? `Mentions: ${n} unread` : "Mentions";
  btn.setAttribute("aria-label", btn.title);
  if (n) btn.append(el("span", { class: "mention-badge mn-count", "aria-hidden": "true" }, n > 99 ? "99+" : String(n)));
  if (typeof refreshMentionsWidget === "function") refreshMentionsWidget();
}

// The newest few, for the sidebar widget.
let mnLatestTimer = null;
function loadLatestMentions(delay = 250) {
  clearTimeout(mnLatestTimer);
  mnLatestTimer = setTimeout(async () => {
    if (!state.me) return;
    const res = await api("GET", "/api/users/@me/mentions?limit=3").catch(() => null);
    if (!res) return;
    state.mentionInbox.latest = res.mentions;
    state.mentionInbox.unread = res.unread;
    state.mentionInbox.latestAt = Date.now();
    paintMentionsButton();
  }, delay);
}

// ---------- one mention ----------

// Message text for a snippet: mentions as names where we know them.
function mentionSnippet(m) {
  if (m.filtered) return "Filtered by automod";
  let text = (m.content || "").replace(/<@(&?)(\d+)>/g, (_, amp, id) => {
    if (!amp) {
      if (id === state.me?.id) return `@${state.me.display_name}`;
      const who = memberById(id);
      return who ? `@${who.display_name}` : "@someone";
    }
    const role = roleById(id);
    return role ? `@${role.name}` : "@role";
  }).replace(/<:([a-z0-9_]{2,32}):\d+>/g, ":$1:").replace(/<t:(-?\d+)(?::[tTdDfFR])?>/g, (_, t) => new Date(Number(t) * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }))
    .replace(/\*\*|__/g, "").replace(/\s+/g, " ").trim();
  if (!text) {
    if (m.sticker) text = `🏷 ${m.sticker.name}`;
    else if (m.attachments?.some((a) => a.voice)) text = "🎤 Voice message";
    else if (m.attachments?.length) text = `📎 ${m.attachments.length} file${m.attachments.length === 1 ? "" : "s"}`;
    else if (m.event) text = `📅 ${m.event.title}`;
    else text = "New message";
  }
  return text.length > 220 ? `${text.slice(0, 219)}…` : text;
}

function mentionPlace(it) {
  if (it.guild_id === "0") return it.channel_name ? `@${it.channel_name}` : "Direct message";
  const thread = it.thread_name ? ` › ${it.thread_name}` : "";
  return `#${it.channel_name}${thread}`;
}

function mentionAge(iso) {
  const d = new Date(iso);
  const today = new Date();
  if (d.toDateString() === today.toDateString()) return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  return d.toLocaleDateString([], { day: "numeric", month: "short" });
}

async function markMentionsRead(body) {
  const res = await tryApi("POST", "/api/users/@me/mentions/read", body).catch(() => null);
  if (!res) return null;
  state.mentionInbox.unread = res.unread;
  paintMentionsButton();
  return res;
}

async function openMention(it) {
  if (it.unread) {
    it.unread = false;
    markMentionsRead({ message_ids: [it.message.id] });
    if (mnPanel.items.includes(it)) drawMentionsInbox();
  }
  if (isPhone()) closeSidePanel();
  await goToMessage(it.guild_id, it.channel_id, it.message.id, it.thread_id);
}

function mentionCard(it, { compact = false } = {}) {
  const m = it.message;
  const g = it.guild_id === "0" ? null : mrGuild(it.guild_id);
  const node = el("article", {
    class: `mn-card${it.unread ? " unread" : ""}${compact ? " compact" : ""}`, role: "button", tabindex: "0", "data-id": m.id,
    "aria-label": `${m.author_display_name} ${MN_KIND[it.kind] ?? "mentioned you"} in ${g ? `${g.name}, ` : ""}${mentionPlace(it)}${it.unread ? ", unread" : ""}`,
    onclick: (e) => { if (!e.target.closest("button")) openMention(it); },
  },
  el("div", { class: "mn-meta" },
    avatarFor(m.author_id, m.author_display_name, compact ? 20 : 26, m.author_avatar_url || undefined),
    el("strong", { class: "mn-author" }, m.author_display_name),
    el("span", { class: "mn-kind muted" }, MN_KIND[it.kind] ?? ""),
    el("time", { class: "mn-time muted", datetime: m.created_at, title: new Date(m.created_at).toLocaleString() }, mentionAge(m.created_at))),
  el("div", { class: "mn-text" }, mentionSnippet(m)),
  el("div", { class: "mn-where muted small" },
    g ? mrGuildIcon(g.id, 16) : el("span", { class: "mn-dm-ic", "aria-hidden": "true" }, "@"),
    el("span", { class: "mn-place" }, `${g ? `${g.name} · ` : ""}${mentionPlace(it)}`),
    compact ? null : el("span", { class: "mn-actions" },
      it.unread ? el("button", { type: "button", class: "ghost mn-read", title: "Mark as read", onclick: async () => {
        it.unread = false;
        drawMentionsInbox();
        await markMentionsRead({ message_ids: [m.id] });
      } }, "Mark read") : null,
      el("button", { type: "button", class: "ghost mn-jump", onclick: () => openMention(it) }, "Jump"))));
  node.addEventListener("keydown", (e) => { if (e.key === "Enter" && e.target === node) openMention(it); });
  return node;
}

// ---------- the inbox ----------

function mnDayHead(iso) {
  const d = new Date(iso);
  const today = new Date();
  const yesterday = new Date(); yesterday.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === yesterday.toDateString()) return "Yesterday";
  return d.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" });
}

function openMentionsInbox(opts = {}) {
  if (sidePanel.kind === "mentions" && !opts.filter && opts.guild == null) return closeSidePanel();
  if (opts.filter) mnPanel.filter = opts.filter;
  if (opts.guild != null) mnPanel.guild = opts.guild;
  const filters = el("div", { class: "mn-filters" });
  showSidePanel("mentions", "Mentions", filters);
  loadMentionsInbox();
}

function drawMentionFilters() {
  const bar = sidePanel.node?.querySelector(".mn-filters");
  if (!bar) return;
  const which = el("div", { class: "mn-seg", role: "group", "aria-label": "Show" },
    [["all", "All"], ["unread", "Unread"]].map(([v, label]) => el("button", {
      type: "button", class: `mn-seg-btn${mnPanel.filter === v ? " on" : ""}`, "aria-pressed": mnPanel.filter === v ? "true" : "false",
      onclick: () => { mnPanel.filter = v; loadMentionsInbox(); },
    }, label)));
  const where = el("select", { class: "select mn-where-pick", "aria-label": "Where", onchange: () => { mnPanel.guild = where.value; loadMentionsInbox(); } },
    el("option", { value: "" }, "Everywhere"),
    el("option", { value: "0", selected: mnPanel.guild === "0" }, "Direct messages"),
    state.guilds.map((g) => el("option", { value: g.id, selected: mnPanel.guild === g.id }, g.name)));
  const all = el("button", {
    type: "button", class: "ghost mn-all", disabled: !state.mentionInbox.unread,
    onclick: async () => {
      const res = await markMentionsRead(mnPanel.guild ? { all: true, guild_id: mnPanel.guild } : { all: true });
      if (res) { toast(res.marked ? `Marked ${res.marked} as read.` : "Nothing new."); loadMentionsInbox(); }
    },
  }, "Mark all read");
  bar.replaceChildren(which, where, all);
}

async function loadMentionsInbox(more = false) {
  if (sidePanel.kind !== "mentions") return;
  const seq = ++mnPanel.seq;
  mnPanel.loading = true;
  if (!more) { mnPanel.items = []; mnPanel.hasMore = false; }
  drawMentionsInbox();
  const q = new URLSearchParams({ limit: String(MN_PAGE) });
  if (mnPanel.filter === "unread") q.set("filter", "unread");
  if (mnPanel.guild) q.set("guild", mnPanel.guild);
  if (more && mnPanel.items.length) q.set("before", mnPanel.items[mnPanel.items.length - 1].message.id);
  const res = await api("GET", `/api/users/@me/mentions?${q}`).catch((err) => { toast(err.message); return null; });
  if (seq !== mnPanel.seq || sidePanel.kind !== "mentions") return;
  mnPanel.loading = false;
  if (res) {
    mnPanel.items = more ? [...mnPanel.items, ...res.mentions] : res.mentions;
    mnPanel.hasMore = res.has_more;
    state.mentionInbox.unread = res.unread;
    paintMentionsButton();
  }
  drawMentionsInbox();
}

function drawMentionsInbox() {
  const body = sidePanel.kind === "mentions" ? sidePanel.body : null;
  if (!body) return;
  drawMentionFilters();
  const keep = body.scrollTop;
  const nodes = [];
  let day = null;
  for (const it of mnPanel.items) {
    const label = mnDayHead(it.message.created_at);
    if (label !== day) { day = label; nodes.push(el("h4", { class: "mn-day" }, label)); }
    nodes.push(mentionCard(it));
  }
  if (!mnPanel.items.length) {
    nodes.push(mnPanel.loading ? el("p", { class: "sp-note" }, "Loading…") : el("div", { class: "sp-empty" },
      el("div", { class: "sp-empty-icon" }, "📭"),
      el("p", {}, mnPanel.filter === "unread" ? "You're all caught up." : "No mentions in the last 30 days."),
      el("p", { class: "muted" }, "When someone @mentions you, one of your roles or everyone, or replies to you, it lands here.")));
  }
  if (mnPanel.hasMore) nodes.push(el("button", { type: "button", class: "ghost sp-more", disabled: mnPanel.loading, onclick: () => loadMentionsInbox(true) }, mnPanel.loading ? "Loading…" : "Load more"));
  body.replaceChildren(...nodes);
  body.scrollTop = keep;
}

// ---------- the header button ----------

(function mentionsButton() {
  const tools = document.querySelector(".chat-tools");
  if (!tools) return;
  const btn = el("button", { class: "tool-btn mn-btn", id: "mentions-btn", type: "button", "data-panel": "mentions", title: "Mentions", "aria-label": "Mentions", onclick: () => openMentionsInbox() });
  btn.innerHTML = MN_ICON;
  btn.append(el("span", { class: "tool-label" }, "Mentions"));
  tools.prepend(btn);
})();

// ---------- live updates ----------

let mnInboxTimer = null;
function mentionsChanged() {
  loadLatestMentions();
  if (sidePanel.kind === "mentions") {
    clearTimeout(mnInboxTimer);
    mnInboxTimer = setTimeout(() => loadMentionsInbox(), 400);
  }
}

const mnBaseHandle = handleEvent;
handleEvent = function (ev) {
  mnBaseHandle(ev);
  switch (ev.t) {
    case "READY":
      loadLatestMentions(400);
      break;
    case "MENTIONS_CHANGED":
      mentionsChanged();
      break;
    // Reading a channel or thread reads its mentions too.
    case "CHANNEL_ACK":
    case "THREAD_ACK":
      if (state.mentionInbox.unread || sidePanel.kind === "mentions") mentionsChanged();
      break;
    case "MESSAGE_DELETE":
      if (state.mentionInbox.latest.some((it) => it.message.id === ev.d.id) || mnPanel.items.some((it) => it.message.id === ev.d.id)) mentionsChanged();
      break;
  }
};
