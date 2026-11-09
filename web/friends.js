// Outhouse web client: friends, friend requests, blocking and privacy.
// Loaded after dms.js (uses its globals: openHome, selectDm, renderDmList,
// renderHomeBadge, openDmWith, dmIcon, …).
//
// Home → Friends lists your friends (Online / All), requests (Pending) and
// people you've blocked, with an "Add friend" box. Messages from people you
// block are folded away in shared channels ("Blocked message — Show").
"use strict";

state.relationships = new Map(); // userId -> { kind, user, since }
state.friendsView = false;
state.friendsTab = "online";
state.revealedBlocked = new Set(); // message IDs you chose to show anyway

const relOf = (userId) => state.relationships.get(userId)?.kind ?? "none";
const isBlocked = (userId) => relOf(userId) === "blocked";
const incomingCount = () => [...state.relationships.values()].filter((r) => r.kind === "incoming").length;

async function loadRelationships() {
  const list = await api("GET", "/api/users/@me/relationships").catch(() => null);
  if (!list) return;
  state.relationships = new Map(list.map((r) => [r.user.id, r]));
  afterRelationshipChange();
}

function afterRelationshipChange() {
  renderHomeBadge();
  if (state.home) renderDmList();
  // (The Add friend box keeps what you typed and its answer.)
  if (state.friendsView && state.friendsTab !== "add") drawFriendsPage();
  if (state.channelId && !state.friendsView) renderMessages();
}

// ---------- actions ----------

async function sendFriendRequest(userId) {
  const r = await tryApi("PUT", `/api/users/@me/relationships/${userId}`).catch(() => null);
  if (!r) return null;
  state.relationships.set(r.user.id, r);
  toast(r.kind === "friend" ? `You and ${r.user.display_name} are now friends.` : `Friend request sent to ${r.user.display_name}.`);
  afterRelationshipChange();
  return r;
}

async function removeRelationship(userId, verb) {
  const ok = await tryApi("DELETE", `/api/users/@me/relationships/${userId}`).then(() => true, () => false);
  if (!ok) return;
  const name = state.relationships.get(userId)?.user.display_name;
  state.relationships.delete(userId);
  if (verb && name) toast(`${verb} ${name}.`.replace("  ", " "));
  afterRelationshipChange();
}

async function blockUser(userId, name) {
  if (!(await confirmBox(`Block ${name}?`, "They won't be able to DM you or send you friend requests, and they won't see when you're online. Their messages in shared channels get folded away. They aren't told.", "Block"))) return;
  const r = await tryApi("PUT", `/api/users/@me/blocks/${userId}`).catch(() => null);
  if (!r) return;
  state.relationships.set(userId, r);
  toast(`Blocked ${name}.`);
  afterRelationshipChange();
}

async function unblockUser(userId, name) {
  const ok = await tryApi("DELETE", `/api/users/@me/blocks/${userId}`).then(() => true, () => false);
  if (!ok) return;
  state.relationships.delete(userId);
  toast(`Unblocked ${name}.`);
  afterRelationshipChange();
}

// Menu items for someone (member list, profile card, DM list).
function friendMenuItems(userId, name, bot = false) {
  if (!userId || userId === state.me?.id || bot) return [];
  const kind = relOf(userId);
  return [
    kind === "none" ? { label: "Add friend", onClick: () => sendFriendRequest(userId) } : null,
    kind === "incoming" ? { label: "Accept friend request", onClick: () => sendFriendRequest(userId) } : null,
    kind === "incoming" ? { label: "Ignore friend request", onClick: () => removeRelationship(userId, "Ignored the request from") } : null,
    kind === "outgoing" ? { label: "Cancel friend request", onClick: () => removeRelationship(userId) } : null,
    kind === "friend" ? { label: "Remove friend", danger: true, onClick: async () => {
      if (await confirmBox(`Remove ${name} as a friend?`, "You can always send another request later.", "Remove friend")) removeRelationship(userId, "Removed");
    } } : null,
    kind === "blocked"
      ? { label: "Unblock", onClick: () => unblockUser(userId, name) }
      : { label: "Block", danger: true, onClick: () => blockUser(userId, name) },
  ];
}

// ---------- Home → Friends ----------

const FRIEND_TABS = [["online", "Online"], ["all", "All"], ["pending", "Pending"], ["blocked", "Blocked"]];

function friendRow(r) {
  const u = r.user;
  const sub = {
    friend: u.online ? "Online" : "Offline",
    incoming: "Wants to be friends",
    outgoing: "Request sent",
    blocked: "Blocked",
  }[r.kind];
  const iconBtn = (label, text, onClick, cls = "") => el("button", { type: "button", class: `fr-btn ${cls}`, title: label, "aria-label": label, onclick: (e) => { e.stopPropagation(); onClick(e); } }, text);
  const actions = {
    friend: [
      iconBtn("Message", "💬", () => openDmWith(u.id)),
      iconBtn("More", "⋯", (e) => { const b = e.currentTarget.getBoundingClientRect(); openMenu(b.left, b.bottom + 4, friendMenuItems(u.id, u.display_name)); }),
    ],
    incoming: [
      iconBtn("Accept", "✓", () => sendFriendRequest(u.id), "ok"),
      iconBtn("Ignore", "✕", () => removeRelationship(u.id, "Ignored the request from"), "no"),
    ],
    outgoing: [iconBtn("Cancel request", "✕", () => removeRelationship(u.id), "no")],
    blocked: [el("button", { type: "button", class: "ghost small-btn", onclick: () => unblockUser(u.id, u.display_name) }, "Unblock")],
  }[r.kind];
  return el("div", { class: `friend-row fr-${r.kind}`, "data-user": u.id, onclick: (e) => openProfile(u.id, e.currentTarget.querySelector(".avatar-wrap") || e.currentTarget) },
    el("span", { class: "avatar-wrap" }, avatarFor(u.id, u.display_name, 36, u.avatar_url || undefined),
      r.kind === "friend" && u.online ? el("span", { class: "status-dot" }) : null),
    el("div", { class: "fr-text" },
      el("div", { class: "fr-name" }, u.display_name, el("span", { class: "fr-user" }, ` @${u.username}`)),
      el("div", { class: `fr-sub${sub === "Online" ? " online" : ""}` }, sub)),
    el("div", { class: "fr-actions" }, actions));
}

function addFriendBox() {
  const field = el("input", { id: "add-friend-input", placeholder: "Their username, like bob", maxlength: 32, autocomplete: "off", "aria-label": "Username to add" });
  const status = el("p", { class: "af-status", role: "status" });
  const send = el("button", { type: "submit", class: "primary", disabled: true }, "Send request");
  field.addEventListener("input", () => { send.disabled = !field.value.trim(); status.textContent = ""; status.className = "af-status"; });
  const form = el("form", { class: "add-friend", onsubmit: async (e) => {
    e.preventDefault();
    const username = field.value.trim();
    if (!username) return;
    send.disabled = true;
    try {
      const r = await api("POST", "/api/users/@me/relationships", { username });
      state.relationships.set(r.user.id, r);
      status.className = "af-status ok";
      status.textContent = r.kind === "friend" ? `You and ${r.user.display_name} are now friends.` : `Sent! Your request is waiting for ${r.user.display_name}.`;
      field.value = "";
      afterRelationshipChange();
    } catch (err) {
      status.className = "af-status err";
      status.textContent = err.message;
    } finally {
      send.disabled = !field.value.trim();
    }
  } }, el("div", { class: "af-row" }, field, send));
  return el("section", { class: "af-box" },
    el("h3", {}, "Add friend"),
    el("p", { class: "muted small" }, "Add someone by their Outhouse username (not their display name)."),
    form, status);
}

function drawFriendsPage() {
  const box = $("messages");
  if (!state.friendsView || !box) return;
  const all = [...state.relationships.values()];
  const pending = all.filter((r) => r.kind === "incoming" || r.kind === "outgoing")
    .sort((a, b) => (a.kind === b.kind ? 0 : a.kind === "incoming" ? -1 : 1));
  const lists = {
    online: all.filter((r) => r.kind === "friend" && r.user.online),
    all: all.filter((r) => r.kind === "friend"),
    pending,
    blocked: all.filter((r) => r.kind === "blocked"),
  };
  const tab = state.friendsTab;
  const shown = lists[tab] ?? [];
  const n = incomingCount();
  const tabs = el("div", { class: "friend-tabs", role: "tablist" },
    FRIEND_TABS.map(([key, label]) => el("button", {
      type: "button", role: "tab", class: `ftab${tab === key ? " on" : ""}`, "aria-selected": tab === key ? "true" : "false", "data-tab": key,
      onclick: () => { state.friendsTab = key; drawFriendsPage(); },
    }, label, key === "pending" && n ? el("span", { class: "mention-badge" }, n) : null)),
    el("button", { type: "button", class: `ftab add${tab === "add" ? " on" : ""}`, "data-tab": "add", onclick: () => { state.friendsTab = "add"; drawFriendsPage(); } }, "Add friend"));
  const empty = {
    online: "No friends online right now. Pull up a seat and wait.",
    all: "No friends yet. Add someone by their username.",
    pending: "No friend requests waiting.",
    blocked: "You haven't blocked anyone.",
  }[tab];
  const heading = tab === "add" ? null : el("div", { class: "fr-count" }, `${{ online: "ONLINE", all: "ALL FRIENDS", pending: "PENDING", blocked: "BLOCKED" }[tab]} — ${shown.length}`);
  const filter = tab === "add" || shown.length < 8 ? null : el("input", { type: "search", class: "fr-filter", placeholder: "Search", "aria-label": "Search friends" });
  const listBox = el("div", { class: "friend-list" });
  const drawList = () => {
    const q = filter?.value.trim().toLowerCase() ?? "";
    const rows = shown.filter((r) => !q || r.user.display_name.toLowerCase().includes(q) || r.user.username.includes(q));
    listBox.replaceChildren(...(rows.length ? rows.map(friendRow) : [el("p", { class: "fr-empty muted" }, q ? "Nobody by that name." : empty)]));
  };
  filter?.addEventListener("input", drawList);
  drawList();
  box.replaceChildren(el("div", { class: "friends-page" },
    tabs,
    tab === "add" ? addFriendBox() : [filter, heading, listBox]));
  if (tab === "add" && !isTouch()) $("add-friend-input")?.focus({ preventScroll: true });
}

function showFriends() {
  state.friendsView = true;
  $("app").classList.add("friends-mode");
  state.channelId = null;
  state.messages = [];
  state.members = [];
  renderMembers();
  $("composer-wrap").hidden = true;
  setChatIcon("👋");
  $("channel-name").textContent = "Friends";
  $("channel-topic").textContent = "";
  updateDmTools();
  renderDmList();
  drawFriendsPage();
}

function leaveFriendsView() {
  if (!state.friendsView) return;
  state.friendsView = false;
  $("app").classList.remove("friends-mode");
  $("composer-wrap").hidden = false;
}

async function openFriends(tab) {
  if (tab) state.friendsTab = tab;
  if (state.home) return showFriends();
  state.friendsOpening = true;
  try { await openHome(); } finally { state.friendsOpening = false; }
  if (state.home) showFriends();
}

const frBaseSelectDm = selectDm;
selectDm = async function (id) {
  if (state.friendsOpening) return; // openFriends shows the Friends page instead
  leaveFriendsView();
  return frBaseSelectDm(id);
};

const frBaseSelectChannel = selectChannel;
selectChannel = async function (id, ...rest) {
  leaveFriendsView();
  return frBaseSelectChannel(id, ...rest);
};

const frBaseSelectGuild = selectGuild;
selectGuild = function (id, preferChannel) {
  leaveFriendsView();
  return frBaseSelectGuild(id, preferChannel);
};

// The DM list starts with Friends (and a count of requests waiting).
const frBaseRenderDmList = renderDmList;
renderDmList = function () {
  frBaseRenderDmList();
  const box = $("dm-list");
  if (!box || box.hidden) return;
  const n = incomingCount();
  box.prepend(el("button", {
    class: `channel dm-row friends-row${state.friendsView ? " active" : ""}`,
    id: "friends-row",
    title: "Friends",
    onclick: () => openFriends(),
  }, el("span", { class: "group-icon friends-icon", "aria-hidden": "true" }, "👋"),
  el("span", { class: "dm-text" }, el("span", { class: "cname" }, "Friends")),
  n ? el("span", { class: "mention-badge", title: `${n} friend request${n === 1 ? "" : "s"}` }, n) : null));
};

// Friend requests add to the red count on the Home button.
const frBaseRenderHomeBadge = renderHomeBadge;
renderHomeBadge = function () {
  frBaseRenderHomeBadge();
  const home = $("home-btn");
  const n = incomingCount();
  if (!home || !n) return;
  let dms = 0;
  for (const [id, u] of state.unread) if (u.guild_id === "0") dms += ownCount(id, u);
  home.querySelector(".mention-badge")?.remove();
  home.append(el("span", { class: "mention-badge rail-badge", "data-requests": n, title: `${n} friend request${n === 1 ? "" : "s"}${dms ? `, ${dms} unread` : ""}` }, countLabel(dms + n)));
};

// No DMs yet: Home shows Friends rather than an empty page.
const frBaseOpenHome = openHome;
openHome = async function (preferId) {
  if (preferId === "friends") return openFriends();
  await frBaseOpenHome(preferId);
  if (state.home && !state.channelId && !state.friendsView && !state.dms.length) showFriends();
};

// ---------- profile cards, member menus ----------

const frBaseProfileCard = profileCard;
profileCard = function (p, opts = {}) {
  const card = frBaseProfileCard(p, opts);
  if (opts.preview || p.bot || p.id === state.me?.id) return card;
  const actions = card.querySelector(".pc-actions");
  if (!actions) return card;
  const kind = relOf(p.id);
  const label = { none: "Add friend", incoming: "Accept request", outgoing: "Request sent", friend: "Friends ✓", blocked: "Blocked" }[kind];
  const btn = el("button", {
    class: `ghost pc-friend fr-${kind}`,
    disabled: kind === "outgoing" || kind === "blocked",
    title: kind === "friend" ? "You're friends" : label,
    onclick: () => { if (kind === "none" || kind === "incoming") { closePopover(); sendFriendRequest(p.id); } },
  }, label);
  const more = el("button", { class: "ghost pc-more", title: "More", "aria-label": "More", onclick: (e) => {
    const b = e.currentTarget.getBoundingClientRect();
    const items = friendMenuItems(p.id, p.display_name).filter((it) => it && it.label !== "Add friend" && it.label !== "Accept friend request");
    closePopover();
    openMenu(b.left, b.bottom + 4, items);
  } }, "⋯");
  actions.append(btn, more);
  if (kind === "blocked") actions.querySelector(".pc-message")?.setAttribute("disabled", "");
  return card;
};

const frBaseMemberMenu = MENUS.member;
MENUS.member = function (e, node) {
  const m = memberById(node.dataset.id);
  if (!m || m.id === state.me?.id || m.bot) return frBaseMemberMenu(e, node);
  const realOpen = openMenu;
  openMenu = (x, y, items, depth) => {
    openMenu = realOpen;
    const at = items.findIndex((it) => it?.label === "Copy user ID");
    const extra = ["sep", ...friendMenuItems(m.id, m.display_name, m.bot)];
    if (at >= 0) items.splice(at - 1, 0, ...extra);
    else items.push(...extra);
    return realOpen(x, y, items, depth);
  };
  try { frBaseMemberMenu(e, node); } finally { openMenu = realOpen; }
};

// ---------- blocked people's messages ----------

const frBaseRenderMessages = renderMessages;
renderMessages = function (...args) {
  frBaseRenderMessages(...args);
  if (state.friendsView) return drawFriendsPage();
  const blocked = new Set([...state.relationships.values()].filter((r) => r.kind === "blocked").map((r) => r.user.id));
  if (!blocked.size) return;
  const box = $("messages");
  let run = null;
  for (const node of [...box.children]) {
    const id = node.dataset?.id;
    const m = id && node.matches("article.msg") ? findMessage(id) : null;
    if (m && blocked.has(m.author_id) && !state.revealedBlocked.has(m.id)) {
      node.classList.add("blocked-hidden");
      // The reply line above it goes too.
      const before = node.previousElementSibling;
      if (before?.classList.contains("reply-ref") && !before.classList.contains("blocked-hidden")) before.classList.add("blocked-hidden");
      if (!run) {
        run = { ids: [], bar: el("div", { class: "blocked-bar" }) };
        const anchor = before?.classList.contains("reply-ref") ? before : node;
        anchor.before(run.bar);
      }
      run.ids.push(m.id);
      const ids = run.ids;
      const bar = run.bar;
      bar.replaceChildren(
        el("span", { class: "bb-icon", "aria-hidden": "true" }, "🚫"),
        el("span", {}, ids.length === 1 ? "Blocked message" : `${ids.length} blocked messages`),
        el("button", { type: "button", class: "link bb-show", onclick: () => { ids.forEach((x) => state.revealedBlocked.add(x)); renderMessages(); } }, "Show"));
    } else if (m || node.matches?.("article.msg")) {
      run = null;
    }
  }
};

// ---------- privacy settings ----------

async function drawPrivacyPage(main) {
  const p = await api("GET", "/api/users/@me/privacy").catch((err) => { toast(err.message); return null; });
  if (!p) return;
  const save = async (patch) => {
    const r = await tryApi("PUT", "/api/users/@me/privacy", patch).catch(() => null);
    if (r) toast("Saved.");
  };
  const radios = (name, value, options, onPick) => el("div", { class: "radio-list", role: "radiogroup" },
    options.map(([v, label, hint]) => {
      const input = el("input", { type: "radio", name, value: v, checked: v === value });
      input.addEventListener("change", () => input.checked && onPick(v));
      return el("label", { class: "radio-row" }, input, el("span", {}, el("strong", {}, label), hint ? el("span", { class: "muted small" }, hint) : null));
    }));
  const blocked = [...state.relationships.values()].filter((r) => r.kind === "blocked");
  main.append(
    el("h2", {}, "Privacy & friends"),
    el("section", { class: "notify-box" },
      el("h3", {}, "Who can start a DM with you"),
      radios("allow_dms", p.allow_dms, [
        ["everyone", "Anyone in your outhouses", "People you share an outhouse with (and friends)."],
        ["friends", "Friends only", "Conversations you already have stay open."],
      ], (v) => save({ allow_dms: v }))),
    el("section", { class: "notify-box" },
      el("h3", {}, "Who can send you friend requests"),
      radios("allow_friend_requests", p.allow_friend_requests, [
        ["everyone", "Everyone", "Anyone who knows your username."],
        ["shared", "People in your outhouses", null],
        ["friends_of_friends", "Friends of friends", null],
        ["nobody", "Nobody", "You can still send requests yourself."],
      ], (v) => save({ allow_friend_requests: v }))),
    el("section", { class: "notify-box" },
      el("h3", {}, `Blocked (${blocked.length})`),
      blocked.length
        ? el("div", { class: "friend-list compact" }, blocked.map(friendRow))
        : el("p", { class: "muted small" }, "You haven't blocked anyone. Block someone from their profile card or right-click menu.")),
  );
}

// ---------- live updates ----------

const frBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  switch (t) {
    case "READY":
      loadRelationships();
      break;
    case "RELATIONSHIP_UPDATE": {
      const before = relOf(d.user.id);
      if (d.kind === "none") state.relationships.delete(d.user.id);
      else state.relationships.set(d.user.id, { kind: d.kind, user: d.user, since: new Date().toISOString() });
      if (d.kind === "incoming" && before !== "incoming") {
        toast(`👋 ${d.user.display_name} sent you a friend request.`);
        if (desktop && !document.hasFocus()) desktopInvoke("notify", { title: "New friend request", body: `${d.user.display_name} wants to be friends.` });
      }
      if (d.kind === "friend" && before === "outgoing") toast(`${d.user.display_name} accepted your friend request.`);
      afterRelationshipChange();
      break;
    }
    case "PRESENCE_UPDATE": {
      const r = state.relationships.get(d.user_id);
      if (r && r.user.online !== d.online) {
        r.user.online = d.online;
        if (state.friendsView) drawFriendsPage();
      }
      break;
    }
  }
  frBaseHandle(ev);
};

// A friend-request notification opens Home → Friends → Pending.
(() => {
  const q = new URLSearchParams(location.search);
  if (!q.has("friends")) return;
  state.openFriendsOnReady = true;
  q.delete("friends");
  const rest = q.toString();
  history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : ""));
})();
const frBaseOnReady = onReady;
onReady = function (...args) {
  frBaseOnReady(...args);
  if (state.openFriendsOnReady) {
    state.openFriendsOnReady = false;
    setTimeout(() => openFriends("pending"), 0);
  }
};

// One-to-one DMs: friend and block options in their right-click menu.
const frBaseDmMenu = MENUS.dm;
MENUS.dm = function (e, node) {
  const dm = state.dms.find((d) => d.id === node.dataset?.id);
  const other = dm && dm.kind === DM ? dmOthers(dm)[0] : null;
  if (!other || other.bot) return frBaseDmMenu(e, node);
  const realOpen = openMenu;
  openMenu = (x, y, items, depth) => {
    openMenu = realOpen;
    const at = items.findIndex((it) => it === "sep");
    items.splice(at >= 0 ? at : items.length, 0, "sep", ...friendMenuItems(other.id, other.display_name));
    return realOpen(x, y, items, depth);
  };
  try { frBaseDmMenu(e, node); } finally { openMenu = realOpen; }
};
