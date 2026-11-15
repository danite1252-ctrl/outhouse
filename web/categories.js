// Outhouse: channel categories and moving channels around (as in Discord).
// app.js draws the list (channelTree / renderChannels); this file adds the
// menus (create, rename, collapse, delete, Move up / Move down / Move to
// category), drag and drop on a computer (Manage Channels), "Sync
// permissions with the category" and live updates. Every move is saved with
// one PATCH /api/guilds/{id}/channels/positions listing every channel you can
// see in its new order; the server renumbers them and tells everyone to
// re-fetch their list. Loaded after channel-settings.js and role-preview.js.

const catById = (id) => state.channels.find((c) => c.id === id && c.kind === CATEGORY);
const catLabel = (c) => (c ? c.name.toUpperCase() : "No category");
const chLabel = (c) => (c.kind === VOICE ? `🔈 ${c.name}` : c.kind === CATEGORY ? c.name.toUpperCase() : `#${c.name}`);
const canMoveChannels = () => can("manage_channels") && !state.preview && !state.home;

// ---------- creating ----------

function createMenuItems(parentId = null) {
  if (!can("manage_channels") || state.preview) return [];
  return [
    { label: "Create text channel", onClick: () => createChannel(TEXT, parentId) },
    { label: "Create voice channel", onClick: () => createChannel(VOICE, parentId) },
    parentId ? null : { label: "Create category", onClick: () => createCategory() },
  ];
}

async function createCategory() {
  const name = await ask("New category", { placeholder: "Raid nights", confirm: "Create",
    body: el("p", { class: "muted cat-ask-note" }, "Categories group channels under a header. Drag channels into it, or use Move to category.") });
  if (!name) return;
  const c = await tryApi("POST", `/api/guilds/${state.guildId}/channels`, { name, kind: CATEGORY }).catch(() => null);
  if (!c) return;
  if (!state.channels.some((x) => x.id === c.id)) state.channels.push(c);
  renderChannels();
  toast(`${catLabel(c)} made. Drag channels into it.`);
  refreshGuild();
}

async function renameCategory(cat) {
  const name = await ask("Rename category", { value: cat.name, confirm: "Save" });
  if (!name || name === cat.name) return;
  const updated = await tryApi("PATCH", `/api/channels/${cat.id}`, { name }).catch(() => null);
  if (updated) { Object.assign(cat, { name: updated.name }); renderChannels(); }
}

// Resolves true when it's gone.
async function deleteCategory(cat) {
  const inside = state.channels.filter((c) => c.parent_id === cat.id).length;
  const ok = await confirmBox(`Delete ${catLabel(cat)}?`, inside
    ? `Its channels aren't deleted: ${inside === 1 ? "the channel moves" : `all ${inside} move`} to the top of the list, with no category.`
    : "There's nothing in it.", "Delete category");
  if (!ok) return false;
  const done = await tryApi("DELETE", `/api/channels/${cat.id}`).then(() => true).catch(() => false);
  if (done) {
    for (const c of state.channels) if (c.parent_id === cat.id) c.parent_id = null;
    state.channels = state.channels.filter((c) => c.id !== cat.id);
    renderChannels();
    toast(`${catLabel(cat)} deleted. Its channels are still here.`);
  }
  return done;
}

// ---------- the layout, and saving it ----------

// The list as arrays of IDs: {text, voice, cats: [{id, text, voice}]}.
function channelLayout() {
  const t = channelTree();
  const ids = (list) => list.map((c) => c.id);
  return { text: ids(t.text), voice: ids(t.voice), cats: t.categories.map((x) => ({ id: x.cat.id, text: ids(x.text), voice: ids(x.voice) })) };
}

function layoutGroup(layout, parentId, kind) {
  const holder = parentId ? layout.cats.find((c) => c.id === parentId) : layout;
  if (!holder) return null;
  return kind === VOICE ? holder.voice : holder.text;
}

function layoutRemove(layout, id) {
  for (const holder of [layout, ...layout.cats]) {
    for (const list of [holder.text, holder.voice]) {
      const i = list.indexOf(id);
      if (i >= 0) { list.splice(i, 1); return; }
    }
  }
  const i = layout.cats.findIndex((c) => c.id === id);
  if (i >= 0) layout.cats.splice(i, 1);
}

// Saves a layout. Positions are the same numbers the channels already have,
// handed out in the new order (so channels you can't see keep their place
// among them). `moved`: a channel that changed category, to offer a sync.
async function saveLayout(layout, moved = null) {
  document.querySelector(".cat-sync-offer")?.remove(); // about an earlier move
  const order = [];
  const push = (id, parent) => order.push({ id, parent });
  for (const id of layout.text) push(id, null);
  for (const id of layout.voice) push(id, null);
  for (const cat of layout.cats) {
    push(cat.id, undefined);
    for (const id of cat.text) push(id, cat.id);
    for (const id of cat.voice) push(id, cat.id);
  }
  const byId = new Map(state.channels.map((c) => [c.id, c]));
  const numbers = order.map((o) => byId.get(o.id)?.position ?? 0).sort((a, b) => a - b);
  for (let i = 1; i < numbers.length; i++) if (numbers[i] <= numbers[i - 1]) numbers[i] = numbers[i - 1] + 1;
  const body = order.map((o, i) => {
    const c = byId.get(o.id);
    if (c) { c.position = numbers[i]; if (o.parent !== undefined) c.parent_id = o.parent; }
    return o.parent === undefined ? { id: o.id, position: numbers[i] } : { id: o.id, position: numbers[i], parent_id: o.parent };
  });
  renderChannels();
  const list = await tryApi("PATCH", `/api/guilds/${state.guildId}/channels/positions`, body).catch(() => null);
  if (!list) { reloadChannelList(); return; }
  mergeChannelList(list);
  if (moved) offerSync(moved);
}

// The server's list (positions, categories, collapsed, synced) into
// state.channels, keeping newer read markers we already have.
function mergeChannelList(list) {
  const old = new Map(state.channels.map((c) => [c.id, c]));
  state.channels = list.map((c) => {
    const mine = old.get(c.id);
    if (!mine) return c;
    const lastRead = mine.last_read_id && (!c.last_read_id || idGt(mine.last_read_id, c.last_read_id)) ? mine.last_read_id : c.last_read_id;
    const lastMsg = mine.last_message_id && (!c.last_message_id || idGt(mine.last_message_id, c.last_message_id)) ? mine.last_message_id : c.last_message_id;
    return Object.assign(mine, c, { last_read_id: lastRead, last_message_id: lastMsg });
  });
  renderChannels();
  if (state.channelId && !state.channels.some((c) => c.id === state.channelId)) {
    const first = state.channels.find((c) => c.kind === TEXT);
    if (first) selectChannel(first.id);
  }
}

let reloadTimer = null;
function reloadChannelList() {
  clearTimeout(reloadTimer);
  reloadTimer = setTimeout(async () => {
    const id = state.guildId;
    if (!id || state.home) return;
    // Previewing a role: the preview keeps its own copy of the list.
    if (state.preview) return refreshGuild();
    const list = await api("GET", `/api/guilds/${id}/channels`).catch(() => null);
    if (list && state.guildId === id && !state.home && !state.preview) mergeChannelList(list);
  }, 120);
}

// ---------- moving with menus (phones, keyboards, everyone) ----------

function moveChannel(c, delta) {
  const layout = channelLayout();
  const list = layoutGroup(layout, catById(c.parent_id) ? c.parent_id : null, c.kind);
  const i = list?.indexOf(c.id) ?? -1;
  const j = i + delta;
  if (i < 0 || j < 0 || j >= list.length) return;
  [list[i], list[j]] = [list[j], list[i]];
  saveLayout(layout);
}

function moveCategory(cat, delta) {
  const layout = channelLayout();
  const i = layout.cats.findIndex((c) => c.id === cat.id);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= layout.cats.length) return;
  [layout.cats[i], layout.cats[j]] = [layout.cats[j], layout.cats[i]];
  saveLayout(layout);
}

// Into a category (or none), at the end of its text or voice channels.
function moveToCategory(c, parentId) {
  const layout = channelLayout();
  layoutRemove(layout, c.id);
  const list = layoutGroup(layout, parentId, c.kind);
  if (!list) return;
  list.push(c.id);
  saveLayout(layout, parentId ? c.id : null);
}

// "Move up / Move down / Move to category ›" in a channel's menu.
function channelMoveItems(c) {
  if (!canMoveChannels() || !can("manage_channels", c.id) || (c.kind !== TEXT && c.kind !== VOICE)) return [];
  const layout = channelLayout();
  const parent = catById(c.parent_id) ? c.parent_id : null;
  const list = layoutGroup(layout, parent, c.kind) || [];
  const i = list.indexOf(c.id);
  const cats = channelTree().categories.map((t) => t.cat);
  return [
    "sep",
    { label: "Move up", disabled: i <= 0, onClick: () => moveChannel(c, -1) },
    { label: "Move down", disabled: i < 0 || i >= list.length - 1, onClick: () => moveChannel(c, 1) },
    cats.length ? {
      label: "Move to category",
      submenu: [
        { label: "No category", checked: !parent, onClick: () => { if (parent) moveToCategory(c, null); } },
        "sep",
        ...cats.map((cat) => ({ label: catLabel(cat), checked: parent === cat.id, onClick: () => { if (parent !== cat.id) moveToCategory(c, cat.id); } })),
      ],
    } : null,
    "sep",
  ];
}

MENUS.category = function (e, node) {
  const cat = catById(node.dataset.id);
  if (!cat) return;
  const manage = can("manage_channels", cat.id) && !state.preview;
  const cats = channelLayout().cats;
  const i = cats.findIndex((c) => c.id === cat.id);
  const all = channelTree().categories.map((t) => t.cat);
  openMenu(e.clientX, e.clientY, [
    { label: cat.collapsed ? "Expand category" : "Collapse category", onClick: () => toggleCategory(cat) },
    all.some((c) => !c.collapsed) ? { label: "Collapse all categories", onClick: () => all.forEach((c) => { if (!c.collapsed) toggleCategory(c, true); }) } : null,
    all.some((c) => c.collapsed) ? { label: "Expand all categories", onClick: () => all.forEach((c) => { if (c.collapsed) toggleCategory(c, false); }) } : null,
    "sep",
    ...(manage ? createMenuItems(cat.id) : []),
    "sep",
    manage ? { label: "Rename category", onClick: () => renameCategory(cat) } : null,
    manage && canMoveChannels() ? { label: "Move up", disabled: i <= 0, onClick: () => moveCategory(cat, -1) } : null,
    manage && canMoveChannels() ? { label: "Move down", disabled: i < 0 || i >= cats.length - 1, onClick: () => moveCategory(cat, 1) } : null,
    can("manage_roles") && !state.preview ? { label: "Edit permissions", onClick: () => openEditChannel(cat, "permissions") } : null,
    manage ? { label: "Edit category", onClick: () => openEditChannel(cat) } : null,
    manage ? { label: "Delete category", danger: true, onClick: () => deleteCategory(cat) } : null,
    "sep",
    { label: "Copy category ID", onClick: () => copy(cat.id, "ID copied") },
  ]);
};

// Right-click on empty space in the channel list.
MENUS["channel-list"] = function (e) {
  if (state.home || !state.guildId) return;
  const items = createMenuItems();
  if (items.length) openMenu(e.clientX, e.clientY, items);
};

// ---------- "Sync permissions with the category" ----------

async function syncChannel(c, quiet = false) {
  const res = await tryApi("POST", `/api/channels/${c.id}/sync-permissions`).catch(() => null);
  if (!res) return false;
  const local = state.channels.find((x) => x.id === c.id);
  if (local) local.permissions_synced = true;
  if (!quiet) toast(`${chLabel(c)} now has ${catLabel(catById(c.parent_id))}'s permissions.`);
  paintSyncBars();
  return true;
}

// After moving a channel into a category whose permissions differ: offer
// to copy them (never automatic: a channel keeps its own until you sync).
function offerSync(id) {
  const c = state.channels.find((x) => x.id === id);
  const cat = c && catById(c.parent_id);
  if (!c || !cat || c.permissions_synced || !can("manage_roles")) return;
  document.querySelector(".cat-sync-offer")?.remove();
  const box = el("div", { class: "cat-sync-offer", role: "status" },
    el("span", { class: "cso-text" }, `${chLabel(c)} has its own permissions. Sync them with ${catLabel(cat)}?`),
    el("span", { class: "cso-btns" },
      el("button", { type: "button", class: "ghost", onclick: () => box.remove() }, "Keep them"),
      el("button", { type: "button", class: "primary", onclick: async () => { box.remove(); await syncChannel(c); } }, "Sync")));
  document.body.append(box);
  setTimeout(() => box.remove(), 15000);
}

// The bar on top of the Permissions tab: "Synced with RAIDS" or "Not
// synced … Sync now" for a channel in a category; "Sync all channels" on a
// category.
const syncBars = new Set();
function categorySyncBar(ch) {
  if (ch.kind === CATEGORY) {
    const bar = el("div", { class: "ec-sync cat" });
    bar._paint = () => {
      const inside = state.channels.filter((c) => c.parent_id === ch.id);
      const out = inside.filter((c) => !c.permissions_synced);
      bar.replaceChildren(
        el("span", { class: "ec-sync-text" }, inside.length
          ? out.length ? `${out.length} of ${inside.length} channel${inside.length === 1 ? "" : "s"} in it ${out.length === 1 ? "has" : "have"} different permissions.` : "Every channel in it is synced with these permissions."
          : "No channels in it yet. New ones can copy these permissions."),
        out.length ? el("button", { type: "button", class: "ghost ec-sync-btn", onclick: async (e) => {
          e.currentTarget.disabled = true;
          let n = 0;
          for (const c of out) if (await syncChannel(c, true)) n++;
          toast(`Synced ${n} channel${n === 1 ? "" : "s"} with ${catLabel(ch)}.`);
        } }, "Sync all channels") : null);
    };
    syncBars.add(bar);
    bar._paint();
    return bar;
  }
  const cat = catById(ch.parent_id);
  if (!cat) return null;
  const bar = el("div", { class: "ec-sync" });
  bar._paint = () => {
    const now = state.channels.find((c) => c.id === ch.id) || ch;
    const synced = !!now.permissions_synced;
    bar.classList.toggle("synced", synced);
    bar.replaceChildren(
      el("span", { class: `ec-sync-badge ${synced ? "on" : "off"}` }, synced ? "Synced" : "Not synced"),
      el("span", { class: "ec-sync-text" }, synced ? `Same permissions as ${catLabel(cat)}.` : `Different from ${catLabel(cat)}.`),
      synced ? null : el("button", { type: "button", class: "ghost ec-sync-btn", onclick: async (e) => {
        e.currentTarget.disabled = true;
        if (await syncChannel(now)) document.querySelector("dialog.edit-channel .ec-perm-targets button.active")?.click();
      } }, "Sync now"));
  };
  syncBars.add(bar);
  bar._paint();
  return bar;
}

function paintSyncBars() {
  for (const bar of syncBars) {
    if (!bar.isConnected) { syncBars.delete(bar); continue; }
    bar._paint();
  }
}

// ---------- drag and drop (computer, Manage Channels) ----------

// While dragging, redraws of the list wait (a redraw would drop the drag).
let catDragging = false;
let catRedrawLater = false;

(function channelDragAndDrop() {
  const tree = $("channel-tree");
  let drag = null; // {type: "channel"|"category", id, kind}
  let drop = null; // {parent, kind, ref, zone} or {cat, zone} for categories
  let mark = null;
  const clear = () => {
    tree.querySelectorAll(".drop-before, .drop-after, .drop-into, .drop-empty").forEach((n) => n.classList.remove("drop-before", "drop-after", "drop-into", "drop-empty"));
    drop = null;
    mark = null;
  };

  tree.addEventListener("dragstart", (e) => {
    const node = e.target.closest?.("[draggable='true']");
    if (!node || !tree.contains(node) || !channelDragOk()) return;
    if (node.classList.contains("category-head")) drag = { type: "category", id: node.dataset.id };
    else {
      const c = state.channels.find((x) => x.id === node.dataset.id);
      if (!c) return;
      drag = { type: "channel", id: c.id, kind: c.kind };
    }
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", drag.id);
    catDragging = true;
    tree.classList.add("drag-on");
    const shown = node.classList.contains("category-head") ? node.parentElement : node;
    requestAnimationFrame(() => shown.classList.add("dragging"));
  });
  const stopped = () => {
    catDragging = false;
    if (catRedrawLater) { catRedrawLater = false; renderChannels(); }
  };
  tree.addEventListener("dragend", () => {
    drag = null;
    stopped();
    clear();
    tree.classList.remove("drag-on");
    tree.querySelectorAll(".dragging").forEach((n) => n.classList.remove("dragging"));
  });

  const groupNode = (parent, kind) => tree.querySelector(`.ch-group[data-parent="${parent || ""}"][data-kind="${kind}"]`);

  // The start or end of a channel's own group (text or voice) in a category:
  // a text channel can't go among voice channels, so it snaps to the text
  // channels there (Discord's rule), and the other way round.
  const edge = (parent, zone) => {
    const g = groupNode(parent, drag.kind);
    const items = g ? [...g.querySelectorAll(":scope > .ch-item")] : [];
    if (!items.length) return { parent, kind: drag.kind, ref: null, zone, show: g, cls: "drop-empty" };
    const n = zone === "start" ? items[0] : items[items.length - 1];
    return { parent, kind: drag.kind, ref: n.dataset.id, zone: zone === "start" ? "before" : "after", show: n, cls: zone === "start" ? "drop-before" : "drop-after" };
  };

  const whereChannel = (e) => {
    const t = e.target;
    const item = t.closest?.(".ch-item");
    if (item && tree.contains(item)) {
      const g = item.parentElement;
      const parent = g.dataset.parent || null;
      if (Number(g.dataset.kind) !== drag.kind) return edge(parent, drag.kind === TEXT ? "end" : "start");
      const r = (item.querySelector(".channel") || item).getBoundingClientRect();
      const after = e.clientY > r.top + r.height / 2;
      return { parent, kind: drag.kind, ref: item.dataset.id, zone: after ? "after" : "before", show: item, cls: after ? "drop-after" : "drop-before" };
    }
    const head = t.closest?.(".category-head");
    if (head && tree.contains(head)) {
      const target = edge(head.dataset.id, "start");
      return { ...target, show: head, cls: "drop-into" };
    }
    const g = t.closest?.(".ch-group");
    if (g && tree.contains(g)) {
      const parent = g.dataset.parent || null;
      return edge(parent, Number(g.dataset.kind) === drag.kind ? "end" : drag.kind === TEXT ? "end" : "start");
    }
    const cat = t.closest?.(".ch-category");
    if (cat && tree.contains(cat)) return edge(cat.dataset.id, "end");
    // Below everything: the end of the last category (or the top groups).
    const cats = tree.querySelectorAll(".ch-category");
    return edge(cats.length ? cats[cats.length - 1].dataset.id : null, "end");
  };

  const whereCategory = (e) => {
    const cats = [...tree.querySelectorAll(".ch-category")];
    if (!cats.length) return null;
    const cat = e.target.closest?.(".ch-category");
    if (cat && tree.contains(cat)) {
      const r = cat.getBoundingClientRect();
      const head = cat.querySelector(".category-head").getBoundingClientRect();
      const after = cat.classList.contains("collapsed") || r.height < head.height * 2 ? e.clientY > r.top + r.height / 2 : e.clientY > head.bottom;
      return { cat: cat.dataset.id, zone: after ? "after" : "before", show: cat, cls: after ? "drop-after" : "drop-before" };
    }
    const first = cats[0].getBoundingClientRect();
    if (e.clientY < first.top) return { cat: cats[0].dataset.id, zone: "before", show: cats[0], cls: "drop-before" };
    const last = cats[cats.length - 1];
    return { cat: last.dataset.id, zone: "after", show: last, cls: "drop-after" };
  };

  tree.addEventListener("dragover", (e) => {
    if (!drag) return;
    const w = drag.type === "category" ? whereCategory(e) : whereChannel(e);
    if (!w) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    const key = `${w.show?.dataset?.id || w.show?.dataset?.parent}:${w.cls}`;
    if (mark === key) return;
    clear();
    drop = w;
    mark = key;
    w.show?.classList.add(w.cls);
  });
  tree.addEventListener("dragleave", (e) => { if (!tree.contains(e.relatedTarget)) clear(); });

  tree.addEventListener("drop", (e) => {
    if (!drag) return;
    e.preventDefault();
    const d = drag;
    const w = drop || (d.type === "category" ? whereCategory(e) : whereChannel(e));
    clear();
    catDragging = false;
    catRedrawLater = false;
    if (!w) return;
    if (d.type === "category") return dropCategory(d, w);
    dropChannel(d, w);
  });

  function dropChannel(d, w) {
    if (w.ref === d.id) return;
    const c = state.channels.find((x) => x.id === d.id);
    if (!c) return;
    const before = catById(c.parent_id) ? c.parent_id : null;
    const layout = channelLayout();
    layoutRemove(layout, d.id);
    const list = layoutGroup(layout, w.parent, d.kind);
    if (!list) return;
    let at = w.ref ? list.indexOf(w.ref) : w.zone === "before" || w.zone === "start" ? 0 : list.length;
    if (at < 0) at = list.length;
    else if (w.ref && w.zone === "after") at += 1;
    list.splice(at, 0, d.id);
    const old = channelLayout();
    if (JSON.stringify(old) === JSON.stringify(layout)) return;
    saveLayout(layout, w.parent && w.parent !== before ? d.id : null);
  }

  function dropCategory(d, w) {
    if (w.cat === d.id) return;
    const layout = channelLayout();
    const i = layout.cats.findIndex((c) => c.id === d.id);
    if (i < 0) return;
    const [moving] = layout.cats.splice(i, 1);
    let at = layout.cats.findIndex((c) => c.id === w.cat);
    if (at < 0) at = layout.cats.length;
    else if (w.zone === "after") at += 1;
    layout.cats.splice(at, 0, moving);
    if (JSON.stringify(channelLayout()) === JSON.stringify(layout)) return;
    saveLayout(layout);
  }
})();

// ---------- live updates ----------

const catBaseHandle = handleEvent;
handleEvent = function (ev) {
  catBaseHandle(ev);
  const { t, d } = ev;
  if (t === "CHANNELS_REORDERED") {
    if (d.guild_id === state.guildId && !state.home) reloadChannelList();
  } else if (t === "CATEGORY_COLLAPSE") {
    if (d.guild_id !== state.guildId || state.home) return;
    const cat = catById(d.channel_id);
    if (cat && cat.collapsed !== d.collapsed) { cat.collapsed = d.collapsed; renderChannels(); }
  } else if (t === "CHANNEL_DELETE" && d.guild_id === state.guildId && !state.home) {
    // A category went: its channels are uncategorised now.
    let changed = false;
    for (const c of state.channels) if (c.parent_id === d.id) { c.parent_id = null; changed = true; }
    if (changed) renderChannels();
  }
};

// Repaint open "Synced / Not synced" bars when the list changes.
const catBaseRenderChannels = renderChannels;
renderChannels = function (...args) {
  if (catDragging) { catRedrawLater = true; return; }
  const out = catBaseRenderChannels(...args);
  if (syncBars.size) paintSyncBars();
  return out;
};

if (state.channels?.length && state.guildId && !state.home) renderChannels();
