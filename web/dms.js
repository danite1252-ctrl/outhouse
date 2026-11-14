// Outhouse web client: direct messages and group DMs.
// Loaded after app.js, features.js and chat-extras.js (uses their globals).
//
// The Home button (top of the left rail) switches the sidebar to your DMs.
// A DM behaves like a channel with no outhouse: the normal message list,
// composer, uploads, reactions and edits all work on it unchanged.
"use strict";

state.home = false;
state.dms = [];
state.dmsLoaded = null; // promise of the first load

const DM = 1;
const GROUP_DM = 3;
const MAX_GROUP = 10;
const isDmKind = (kind) => kind === DM || kind === GROUP_DM;

// What everyone can do in a DM (nobody moderates them).
function dmBits() {
  let bits = 0n;
  for (const key of ["view_channel", "send_messages", "read_history", "embed_links", "attach_files", "add_reactions"]) bits |= state.P[key] ?? 0n;
  return bits.toString();
}

const dmOthers = (dm) => dm.recipients.filter((u) => u.id !== state.me?.id);

function dmName(dm) {
  if (dm.name) return dm.name;
  const others = dmOthers(dm);
  if (!others.length) return "Just you";
  return others.map((u) => u.display_name).join(", ");
}

// A picture for the DM: the other person, or a little stack for a group.
function dmIcon(dm, size = 32) {
  const others = dmOthers(dm);
  if (dm.kind === DM && others[0]) {
    const u = others[0];
    return el("span", { class: "avatar-wrap dm-avatar" }, avatarFor(u.id, u.display_name, size, u.avatar_url || undefined),
      u.online ? el("span", { class: "status-dot" }) : null);
  }
  const node = el("span", { class: "group-icon", style: `width:${size}px;height:${size}px;flex-basis:${size}px` }, "👥");
  node.style.background = colourFor(dm.id);
  return node;
}

function asMember(u) {
  return { id: u.id, username: u.username, display_name: u.display_name, role_ids: [], joined_at: null, timeout_until: null, online: u.online, avatar_url: u.avatar_url, bot: u.bot };
}

function dmAsChannel(dm) {
  const old = state.channels.find((c) => c.id === dm.id);
  return { id: dm.id, guild_id: "0", name: dmName(dm), kind: dm.kind, position: 0, last_message_id: dm.last_message_id, last_read_id: old?.last_read_id ?? dm.last_read_id };
}

function sortDms() {
  const key = (d) => big(d.last_message_id || d.id);
  state.dms.sort((a, b) => (key(b) > key(a) ? 1 : key(b) < key(a) ? -1 : 0));
}

// While Home is open, the DMs stand in for the outhouse's channels.
function syncDmChannels() {
  if (!state.home) return;
  state.channels = state.dms.map(dmAsChannel);
  const bits = dmBits();
  state.myPerms = { guild: 0n, channels: Object.fromEntries(state.dms.map((d) => [d.id, bits])), top: 0 };
}

const currentDm = () => (state.home ? state.dms.find((d) => d.id === state.channelId) : null);

function dmUnread(dm) {
  const u = state.unread.get(dm.id);
  const last = u?.last_message_id ?? dm.last_message_id;
  const read = u?.last_read_id ?? dm.last_read_id;
  return !!last && (!read || idGt(last, read));
}

function loadDms() {
  const p = api("GET", "/api/dms").then((list) => {
    state.dms = list;
    sortDms();
    syncDmChannels();
    renderDmList();
    renderHomeBadge();
    return list;
  }).catch((err) => { toast(err.message); return state.dms; });
  state.dmsLoaded = p;
  return p;
}

// Called by app.js when the gateway (re)connects.
function onReady() {
  loadDms();
  if (typeof loadEmojis === "function") loadEmojis();
}

// ---------- the sidebar ----------

(function dmSidebar() {
  const home = document.querySelector(".rail .home");
  if (home) {
    home.id = "home-btn";
    home.title = "Direct messages";
    home.setAttribute("aria-label", "Direct messages");
    home.addEventListener("click", () => openHome());
  }
  const list = el("div", { id: "dm-list", class: "dm-list channels", hidden: true });
  document.querySelector(".sidebar .channels")?.before(list);
  // Group options (rename, add people, leave) in the chat header.
  const tools = document.querySelector(".chat-tools");
  tools?.prepend(el("button", { class: "tool-btn", id: "dm-tools", hidden: true, title: "Group options", "aria-label": "Group options", onclick: (e) => {
    const dm = currentDm();
    if (!dm) return;
    const r = e.currentTarget.getBoundingClientRect();
    MENUS.dm({ clientX: r.left, clientY: r.bottom + 6 }, { dataset: { id: dm.id } });
  } }, "⋯"));
})();

function renderHomeBadge() {
  const home = $("home-btn");
  if (!home) return;
  home.classList.toggle("active", state.home);
  home.querySelector(".mention-badge")?.remove();
  let n = 0;
  for (const [id, u] of state.unread) if (u.guild_id === "0") n += ownCount(id, u);
  if (n) home.append(el("span", { class: "mention-badge rail-badge", title: `${n} unread direct message${n === 1 ? "" : "s"}` }, countLabel(n)));
}

function renderDmList() {
  const box = $("dm-list");
  if (!box) return;
  box.hidden = !state.home;
  if (!state.home) return;
  const row = (dm) => {
    const n = channelMentions(dm.id);
    const others = dmOthers(dm);
    const active = dm.id === state.channelId;
    const sub = dm.kind === GROUP_DM ? `${dm.recipients.length} members` : others[0]?.online ? "Online" : null;
    // Muted conversations sit greyed out and don't light up for new messages.
    const muted = typeof notifyMuted === "function" && notifyMuted(dm.id);
    return el("button", {
      class: `channel dm-row${active ? " active" : ""}${!active && !muted && dmUnread(dm) ? " unread" : ""}${muted ? " muted-dm" : ""}`,
      "data-menu": "dm",
      "data-id": dm.id,
      title: dmName(dm),
      onclick: () => selectDm(dm.id),
    }, dmIcon(dm, 32),
    el("span", { class: "dm-text" }, el("span", { class: "cname" }, dmName(dm)), sub ? el("span", { class: `dm-sub${sub === "Online" ? " online" : ""}` }, sub) : null),
    muted ? el("span", { class: "mute-icon", title: typeof muteLabel === "function" ? muteLabel(dm.id) : "Muted" }, "🔕") : null,
    n ? el("span", { class: "mention-badge", title: muted ? `${n} unread mention${n === 1 ? "" : "s"}` : `${n} unread` }, countLabel(n)) : null);
  };
  box.replaceChildren(
    el("div", { class: "section-label" }, "DIRECT MESSAGES",
      el("button", { class: "icon-btn small", id: "new-dm", title: "New message or group", "aria-label": "New message or group", onclick: openNewDm }, "+")),
    ...(state.dms.length ? state.dms.map(row) : [el("p", { class: "dm-empty" }, "No DMs yet. Press + to start one, or click someone's name and pick Message.")]),
  );
}

function setChatIcon(text) {
  const hash = document.querySelector(".chat-head .hash");
  if (hash) hash.textContent = text;
}

function updateDmTools() {
  const btn = $("dm-tools");
  if (btn) btn.hidden = !(currentDm()?.kind === GROUP_DM);
}

// ---------- switching between outhouses and Home ----------

async function openHome(preferId) {
  state.home = true;
  state.guildId = null;
  localStorage.setItem("outhouse_guild", "@home");
  $("app").classList.add("home-mode");
  $("guild-name").textContent = "Direct messages";
  state.roles = [];
  state.members = [];
  state.commands = typeof POLL_COMMAND !== "undefined" ? [POLL_COMMAND] : [];
  syncDmChannels();
  renderGuilds();
  renderChannels();
  renderMembers();
  renderDmList();
  renderHomeBadge();
  if (!state.dmsLoaded) loadDms();
  await state.dmsLoaded;
  if (!state.home) return;
  syncDmChannels();
  const saved = localStorage.getItem("outhouse_dm");
  const pick = state.dms.find((d) => d.id === preferId) || state.dms.find((d) => d.id === saved) || state.dms[0];
  if (pick) return selectDm(pick.id);
  state.channelId = null;
  state.messages = [];
  setChatIcon("@");
  $("channel-name").textContent = "Direct messages";
  updateComposer();
  updateDmTools();
  renderDmList();
  $("messages").replaceChildren(el("div", { class: "empty" },
    el("p", {}, "No DMs yet."),
    el("p", {}, "Have a yarn with anyone you share an outhouse with."),
    el("button", { class: "primary", style: "padding:0 18px;margin-top:8px", onclick: openNewDm }, "Start a conversation")));
}

function leaveHome() {
  state.home = false;
  $("app").classList.remove("home-mode");
  setChatIcon("#");
  renderDmList();
  renderHomeBadge();
  updateDmTools();
}

const baseSelectGuild = selectGuild;
selectGuild = function (id, preferChannel) {
  if (state.home) leaveHome();
  return baseSelectGuild(id, preferChannel);
};

async function selectDm(id) {
  if (!state.home) return openHome(id);
  const dm = state.dms.find((d) => d.id === id);
  if (!dm) return;
  localStorage.setItem("outhouse_dm", id);
  syncDmChannels();
  state.members = dm.recipients.map(asMember);
  renderMembers();
  setChatIcon(dm.kind === DM ? "@" : "👥");
  const opening = selectChannel(id);
  updateDmTools(); // selectChannel has set state.channelId by now
  renderDmList();
  await opening;
  updateDmTools();
  renderDmList();
}

// Open (or start) a DM with someone, e.g. from their profile card.
async function openDmWith(userId) {
  const dm = await tryApi("POST", "/api/dms", { user_ids: [userId] }).catch(() => null);
  if (!dm) return;
  upsertDm(dm);
  if (state.home) selectDm(dm.id);
  else openHome(dm.id);
}

function upsertDm(dm) {
  const i = state.dms.findIndex((d) => d.id === dm.id);
  if (i >= 0) state.dms[i] = dm;
  else state.dms.push(dm);
  sortDms();
  if (!state.home) return renderHomeBadge();
  syncDmChannels();
  if (dm.id === state.channelId) {
    state.members = dm.recipients.map(asMember);
    renderMembers();
    $("channel-name").textContent = dmName(dm);
    updateComposer();
    updateDmTools();
  }
  renderDmList();
}

function removeDm(id) {
  state.dms = state.dms.filter((d) => d.id !== id);
  if (!state.home) return;
  syncDmChannels();
  if (state.channelId === id) {
    state.channelId = null;
    openHome();
  } else {
    renderDmList();
  }
}

// ---------- picking people ----------

// A list of people you share an outhouse with, with tick boxes.
// Resolves to { ids, name } or null.
async function pickPeople({ title, confirm, max, exclude = [], askName = false }) {
  const contacts = await api("GET", "/api/users/@me/contacts").catch((err) => { toast(err.message); return null; });
  if (!contacts) return null;
  const people = contacts.filter((u) => !exclude.includes(u.id));
  return new Promise((resolve) => {
    const picked = new Set();
    const filter = el("input", { type: "search", placeholder: "Find someone", "aria-label": "Find someone" });
    const list = el("div", { class: "people-list" });
    const count = el("span", { class: "muted small" });
    const name = el("input", { placeholder: "Group name (optional)", maxlength: 100, hidden: true });
    const ok = el("button", { type: "submit", class: "primary", value: "ok", disabled: true }, confirm);
    const sync = () => {
      count.textContent = max > 1 ? `${picked.size} of ${max} picked` : "";
      ok.disabled = !picked.size;
      name.hidden = !askName || picked.size < 2;
      ok.textContent = askName ? (picked.size >= 2 ? "Create group" : "Message") : confirm;
    };
    const draw = () => {
      const q = filter.value.trim().toLowerCase();
      const shown = people.filter((u) => !q || u.display_name.toLowerCase().includes(q) || u.username.includes(q));
      list.replaceChildren(...(shown.length ? shown.map((u) => {
        const box = el("input", { type: "checkbox", checked: picked.has(u.id) });
        box.addEventListener("change", () => {
          if (box.checked && picked.size >= max) { box.checked = false; return toast(`You can pick up to ${max}.`); }
          box.checked ? picked.add(u.id) : picked.delete(u.id);
          sync();
        });
        return el("label", { class: "person-row" }, box, avatarFor(u.id, u.display_name, 28, u.avatar_url || undefined),
          el("span", { class: "person-name" }, u.display_name, el("span", { class: "muted small" }, ` @${u.username}`)));
      }) : [el("p", { class: "muted small", style: "margin:8px 0" }, people.length ? "Nobody by that name." : "Nobody to add. Invite people to one of your outhouses first.")]));
    };
    filter.addEventListener("input", draw);
    const dialog = el("dialog", { class: "people-dialog" },
      el("form", { method: "dialog" },
        el("h2", {}, title),
        el("p", { class: "muted small", style: "margin:0" }, "You can message anyone you share an outhouse with."),
        filter, list, name,
        el("div", { class: "row" }, count, el("span", { style: "flex:1" }),
          el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"), ok)));
    dialog.addEventListener("close", () => {
      dialog.remove();
      resolve(dialog.returnValue === "ok" && picked.size ? { ids: [...picked], name: name.value.trim() } : null);
    });
    document.body.append(dialog);
    draw();
    sync();
    dialog.showModal();
    filter.focus();
  });
}

async function openNewDm() {
  const res = await pickPeople({ title: "New message", confirm: "Message", max: MAX_GROUP - 1, askName: true });
  if (!res) return;
  const dm = await tryApi("POST", "/api/dms", { user_ids: res.ids, name: res.ids.length > 1 ? res.name || null : null }).catch(() => null);
  if (!dm) return;
  upsertDm(dm);
  if (state.home) selectDm(dm.id);
  else openHome(dm.id);
}

async function addPeople(dm) {
  const room = MAX_GROUP - dm.recipients.length;
  if (room <= 0) return toast(`Group DMs can have up to ${MAX_GROUP} people.`);
  const res = await pickPeople({ title: `Add people to ${dmName(dm)}`, confirm: "Add", max: room, exclude: dm.recipients.map((u) => u.id) });
  if (!res) return;
  for (const id of res.ids) {
    const updated = await tryApi("POST", `/api/dms/${dm.id}/recipients`, { user_id: id }).catch(() => null);
    if (!updated) break;
    upsertDm(updated);
  }
}

async function renameGroup(dm) {
  const name = await ask("Rename group", { value: dm.name, placeholder: dmName({ ...dm, name: "" }), confirm: "Save", allowEmpty: true });
  if (name === null) return;
  const updated = await tryApi("PATCH", `/api/dms/${dm.id}`, { name }).catch(() => null);
  if (updated) upsertDm(updated);
}

async function leaveGroup(dm) {
  if (!(await confirmBox(`Leave ${dmName(dm)}?`, "You won't see new messages unless someone adds you back.", "Leave group"))) return;
  const ok = await tryApi("DELETE", `/api/dms/${dm.id}/recipients/@me`).then(() => true).catch(() => false);
  if (ok) removeDm(dm.id);
}

function markDmRead(dm) {
  const last = state.unread.get(dm.id)?.last_message_id ?? dm.last_message_id;
  if (!last) return;
  dm.last_read_id = last;
  const ch = state.channels.find((c) => c.id === dm.id);
  if (ch) ch.last_read_id = last;
  noteChannelRead(dm.id, last);
  api("POST", `/api/channels/${dm.id}/ack`, { message_id: last }).catch(() => {});
  renderDmList();
}

MENUS.dm = (e, node) => {
  const dm = state.dms.find((d) => d.id === node.dataset.id);
  if (!dm) return;
  const other = dmOthers(dm)[0];
  const group = dm.kind === GROUP_DM;
  openMenu(e.clientX, e.clientY, [
    { label: "Mark as read", disabled: !dmUnread(dm), onClick: () => markDmRead(dm) },
    ...(typeof muteMenuItems === "function" ? muteMenuItems(dm.id, dmName(dm), "conversation") : []),
    !group && other ? { label: "View profile", onClick: () => openFullProfile(other.id) } : null,
    group ? { label: "Rename group", onClick: () => renameGroup(dm) } : null,
    group ? { label: "Add people", disabled: dm.recipients.length >= MAX_GROUP, onClick: () => addPeople(dm) } : null,
    "sep",
    { label: group ? "Copy group ID" : "Copy user ID", onClick: () => copy(group ? dm.id : other?.id ?? dm.id, "ID copied") },
    group ? "sep" : null,
    group ? { label: "Leave group", danger: true, onClick: () => leaveGroup(dm) } : null,
  ]);
};

// ---------- live updates ----------

const dmsBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  switch (t) {
    case "DM_UPDATE":
      upsertDm(d);
      if (!state.unread.has(d.id)) loadUnread();
      break;
    case "DM_DELETE":
      removeDm(d.id);
      loadUnread();
      break;
    case "MESSAGE_CREATE": {
      const dm = state.dms.find((x) => x.id === d.channel_id);
      if (dm && !d.thread_id) {
        dm.last_message_id = d.id;
        if (d.author_id === state.me?.id) dm.last_read_id = d.id;
        sortDms();
        if (state.home) syncDmChannels();
        if (desktop && d.author_id !== state.me?.id && !document.hasFocus() && !mentionsMe(d)
            && (typeof notifyAllowsDm !== "function" || notifyAllowsDm(d))) {
          const text = plainTimeTags(plainMentions(d.content)).trim() || (d.attachments?.length ? "Sent an attachment" : d.poll ? `📊 ${d.poll.question}` : "");
          desktopInvoke("notify", { title: dm.kind === DM ? d.author_display_name : `${d.author_display_name} in ${dmName(dm)}`, body: text.slice(0, 200) });
        }
      } else if (!dm && !state.channels.some((c) => c.id === d.channel_id) && !state.unread.has(d.channel_id)) {
        loadDms(); // probably a DM someone just started with us
      }
      break;
    }
    case "CHANNEL_ACK": {
      const dm = state.dms.find((x) => x.id === d.channel_id);
      if (dm && (!dm.last_read_id || idGt(d.last_read_id, dm.last_read_id))) dm.last_read_id = d.last_read_id;
      break;
    }
    case "PRESENCE_UPDATE":
      for (const dm of state.dms) for (const u of dm.recipients) if (u.id === d.user_id) u.online = d.online;
      break;
  }
  dmsBaseHandle(ev);
  if (state.home && ["MESSAGE_CREATE", "CHANNEL_ACK", "PRESENCE_UPDATE"].includes(t)) renderDmList();
};

// Every unread DM counts like a mention (it was said to you).
const baseMentionsMeIn = mentionsMeIn;
mentionsMeIn = function (m, guildId) {
  if (m.thread_id) return false;
  if (guildId === "0") return m.author_id !== state.me.id;
  return baseMentionsMeIn(m, guildId);
};

const baseRefreshUnreadUi = refreshUnreadUi;
refreshUnreadUi = function () {
  baseRefreshUnreadUi();
  renderHomeBadge();
  if (state.home) renderDmList();
};
