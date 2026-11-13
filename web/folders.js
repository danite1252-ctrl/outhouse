// Outhouse: folders on the left rail. Group outhouses under headers, collapse
// them, reorder everything (drag on desktop, menus everywhere), all saved on
// the server so it's the same on every device. Also draws outhouse pictures
// on the rail. Loaded after app.js, chat-extras.js and safety.js; uses their
// globals (el, api, tryApi, state, openMenu, MENUS, ask, confirmBox, toast,
// guildBadge, guildMentions, guildUnread, initials, selectGuild, settingsPage).

const FOLDER_COLOURS = [
  ["ember", "Ember", "#ff6b35"],
  ["glow", "Glow", "#ffb347"],
  ["red", "Red", "#f87171"],
  ["pink", "Pink", "#f472b6"],
  ["purple", "Purple", "#a78bfa"],
  ["blue", "Blue", "#60a5fa"],
  ["teal", "Teal", "#2dd4bf"],
  ["green", "Green", "#4ade80"],
  ["grey", "Grey", "#a1a1aa"],
];
const FOLDER_NAME_MAX = 32;
const MAX_FOLDERS = 50;
const folderHex = (c) => (FOLDER_COLOURS.find(([k]) => k === c) || FOLDER_COLOURS[0])[2];

state.rail = { items: null, version: 0, saved: 0, saving: null };

// ---------- the model ----------

// The layout tidied against the outhouses you're in right now: gone ones
// dropped, new ones at the end (outside folders), as the server does.
function railItems() {
  const known = new Set(state.guilds.map((g) => g.id));
  const seen = new Set();
  const keep = (id) => known.has(id) && !seen.has(id) && seen.add(id);
  const items = [];
  for (const it of state.rail.items || []) {
    if (it.type === "guild") { if (keep(it.id)) items.push({ type: "guild", id: it.id }); }
    else if (it.type === "folder") items.push({ ...it, guild_ids: it.guild_ids.filter(keep) });
  }
  for (const g of state.guilds) if (!seen.has(g.id)) items.push({ type: "guild", id: g.id });
  return items;
}

const railFolders = () => railItems().filter((it) => it.type === "folder");
const folderById = (items, id) => items.find((it) => it.type === "folder" && it.id === id);
const folderOf = (items, guildId) => items.find((it) => it.type === "folder" && it.guild_ids.includes(guildId));
const guildName = (id) => state.guilds.find((g) => g.id === id)?.name ?? "Outhouse";

// Takes an outhouse out of wherever it is.
function detachGuild(items, guildId) {
  const i = items.findIndex((it) => it.type === "guild" && it.id === guildId);
  if (i >= 0) items.splice(i, 1);
  for (const f of items) if (f.type === "folder") f.guild_ids = f.guild_ids.filter((g) => g !== guildId);
}

// Changes the layout (a copy) and saves it. `change` edits `items` in place.
function updateRail(change) {
  const items = railItems().map((it) => (it.type === "folder" ? { ...it, guild_ids: [...it.guild_ids] } : it));
  if (change(items) === false) return;
  state.rail.items = items;
  state.rail.version += 1;
  renderGuilds();
  if (typeof refreshFoldersPage === "function") refreshFoldersPage();
  saveRail();
}

// One save at a time, always sending the newest layout.
function saveRail() {
  if (state.rail.saving) { state.rail.again = true; return; }
  const version = state.rail.version;
  const body = { items: state.rail.items.map((it) => (it.type === "folder"
    ? { type: "folder", id: it.id && !String(it.id).startsWith("new-") ? it.id : undefined, name: it.name, color: it.color || null, collapsed: !!it.collapsed, guild_ids: it.guild_ids }
    : { type: "guild", id: it.id })) };
  state.rail.saving = tryApi("PUT", "/api/users/@me/guild-layout", body)
    .then((saved) => {
      // The server picked IDs for new folders: take its copy unless there's newer.
      if (saved && state.rail.version === version) {
        state.rail.items = saved.items;
        state.rail.saved = version;
        renderGuilds();
        if (typeof refreshFoldersPage === "function") refreshFoldersPage();
      }
    })
    .catch(() => {})
    .finally(() => {
      state.rail.saving = null;
      if (state.rail.again) { state.rail.again = false; saveRail(); }
    });
}

async function loadRail() {
  if (state.rail.saving || state.rail.version !== state.rail.saved) return; // ours is newer
  const layout = await api("GET", "/api/users/@me/guild-layout").catch(() => null);
  if (!layout || state.rail.saving || state.rail.version !== state.rail.saved) return;
  state.rail.items = layout.items;
  renderGuilds();
  if (typeof refreshFoldersPage === "function") refreshFoldersPage();
}

// ---------- actions ----------

async function askFolderName(title, value = "") {
  const name = await ask(title, { placeholder: "Raids", value, confirm: value ? "Save" : "Create" });
  if (name == null) return null;
  const clean = name.trim();
  if (!clean) return null;
  if ([...clean].length > FOLDER_NAME_MAX) { toast(`Folder names are up to ${FOLDER_NAME_MAX} characters.`); return null; }
  return clean;
}

// A new folder (with an outhouse in it, or empty from settings).
async function newFolder(guildId = null) {
  if (railFolders().length >= MAX_FOLDERS) return toast(`You can have up to ${MAX_FOLDERS} folders.`);
  const name = await askFolderName("New folder");
  if (!name) return;
  const folder = { type: "folder", id: `new-${Math.random().toString(36).slice(2, 10)}`, name, color: null, collapsed: false, guild_ids: [] };
  updateRail((items) => {
    if (guildId) {
      // Where the outhouse was (or just after its old folder).
      const host = folderOf(items, guildId);
      let at = host ? items.indexOf(host) + 1 : items.findIndex((it) => it.type === "guild" && it.id === guildId);
      if (at < 0) at = items.length;
      detachGuild(items, guildId);
      folder.guild_ids.push(guildId);
      items.splice(Math.min(at, items.length), 0, folder);
    } else {
      items.push(folder);
    }
  });
}

function addToFolder(guildId, folderId) {
  updateRail((items) => {
    const f = folderById(items, folderId);
    if (!f) return false;
    detachGuild(items, guildId);
    f.guild_ids.push(guildId);
  });
}

function removeFromFolder(guildId) {
  updateRail((items) => {
    const f = folderOf(items, guildId);
    if (!f) return false;
    f.guild_ids = f.guild_ids.filter((g) => g !== guildId);
    items.splice(items.indexOf(f) + 1, 0, { type: "guild", id: guildId });
  });
}

// Up or down one place, within its folder or along the top level.
function moveGuild(guildId, delta) {
  updateRail((items) => {
    const f = folderOf(items, guildId);
    const list = f ? f.guild_ids : items;
    const i = f ? list.indexOf(guildId) : items.findIndex((it) => it.type === "guild" && it.id === guildId);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= list.length) return false;
    [list[i], list[j]] = [list[j], list[i]];
  });
}

function canMoveGuild(guildId, delta) {
  const items = railItems();
  const f = folderOf(items, guildId);
  const list = f ? f.guild_ids : items;
  const i = f ? list.indexOf(guildId) : items.findIndex((it) => it.type === "guild" && it.id === guildId);
  return i >= 0 && i + delta >= 0 && i + delta < list.length;
}

function moveFolder(folderId, delta) {
  updateRail((items) => {
    const i = items.findIndex((it) => it.type === "folder" && it.id === folderId);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= items.length) return false;
    [items[i], items[j]] = [items[j], items[i]];
  });
}

function canMoveFolder(folderId, delta) {
  const items = railItems();
  const i = items.findIndex((it) => it.type === "folder" && it.id === folderId);
  return i >= 0 && i + delta >= 0 && i + delta < items.length;
}

function setFolder(folderId, patch) {
  updateRail((items) => {
    const f = folderById(items, folderId);
    if (!f) return false;
    Object.assign(f, patch);
  });
}

function collapseAllFolders(collapsed = true) {
  updateRail((items) => { for (const f of items) if (f.type === "folder") f.collapsed = collapsed; });
}

async function renameFolder(folderId) {
  const f = folderById(railItems(), folderId);
  if (!f) return;
  const name = await askFolderName("Rename folder", f.name);
  if (name) setFolder(folderId, { name });
}

async function deleteFolder(folderId) {
  const f = folderById(railItems(), folderId);
  if (!f) return;
  const n = f.guild_ids.length;
  const ok = await confirmBox(`Delete “${f.name}”?`,
    n ? `The ${n === 1 ? "outhouse" : `${n} outhouses`} in it go back on the rail on their own. You stay in ${n === 1 ? "it" : "them"}.` : "It's empty.",
    "Delete folder");
  if (!ok) return;
  updateRail((items) => {
    const i = items.findIndex((it) => it.type === "folder" && it.id === folderId);
    if (i < 0) return false;
    const [gone] = items.splice(i, 1);
    items.splice(i, 0, ...gone.guild_ids.map((id) => ({ type: "guild", id })));
  });
}

// ---------- drawing the rail ----------

function folderIcon(colour, size = 18) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  for (const [k, v] of Object.entries({ width: size, height: size, viewBox: "0 0 24 24", "aria-hidden": "true" })) svg.setAttribute(k, v);
  const path = document.createElementNS(ns, "path");
  path.setAttribute("d", "M3 6.5A1.5 1.5 0 0 1 4.5 5h4.6l2 2h8.4A1.5 1.5 0 0 1 21 8.5v9A1.5 1.5 0 0 1 19.5 19h-15A1.5 1.5 0 0 1 3 17.5z");
  path.setAttribute("fill", colour);
  svg.append(path);
  return svg;
}

function guildFace(g) {
  return g.icon_url
    ? el("img", { class: "guild-icon", src: g.icon_url, alt: "", draggable: "false" })
    : initials(g.name);
}

const dragOk = () => !(typeof isTouch === "function" && isTouch());

function railGuildButton(g, extra = "") {
  return el("button", {
    class: `guild${g.id === state.guildId ? " active" : ""}${g.icon_url ? " has-icon" : ""}${extra}`,
    title: g.name,
    "aria-label": g.name,
    "data-menu": "guild",
    "data-id": g.id,
    draggable: dragOk() ? "true" : false,
    onclick: () => selectGuild(g.id),
  }, guildFace(g), guildBadge(g.id));
}

// The red count (or the dot) for everything in a collapsed folder.
function folderBadge(f) {
  let n = 0;
  for (const id of f.guild_ids) n += guildMentions(id);
  if (n) return el("span", { class: "mention-badge rail-badge", title: `${n} unread mention${n === 1 ? "" : "s"} in ${f.name}` }, n > 99 ? "99+" : String(n));
  return f.guild_ids.some((id) => id !== state.guildId && guildUnread(id)) ? el("span", { class: "badge" }) : null;
}

function railFolder(f) {
  const colour = folderHex(f.color);
  const guilds = f.guild_ids.map((id) => state.guilds.find((g) => g.id === id)).filter(Boolean);
  const hasActive = guilds.some((g) => g.id === state.guildId);
  const wrap = el("div", {
    class: `rail-folder ${f.collapsed ? "closed" : "open"}${hasActive ? " has-active" : ""}`,
    "data-folder": f.id,
    style: `--fc:${colour}`,
  });
  const toggle = () => setFolder(f.id, { collapsed: !f.collapsed });
  if (f.collapsed) {
    const mini = el("span", { class: `folder-mini n${Math.min(guilds.length, 4)}` },
      guilds.slice(0, 4).map((g) => el("span", { class: "mini" }, g.icon_url ? el("img", { src: g.icon_url, alt: "" }) : initials(g.name).slice(0, 1))));
    wrap.append(el("button", {
      class: "folder-tile",
      title: `${f.name} (${guilds.length})`,
      "aria-label": `${f.name}, folder with ${guilds.length} outhouse${guilds.length === 1 ? "" : "s"}, collapsed`,
      "aria-expanded": "false",
      "data-menu": "folder",
      "data-folder": f.id,
      draggable: dragOk() ? "true" : false,
      onclick: toggle,
    }, guilds.length ? mini : folderIcon(colour, 22), folderBadge(f)));
    // So you're not lost: the open outhouse peeks out under its folder.
    const active = guilds.find((g) => g.id === state.guildId);
    if (active) wrap.append(railGuildButton(active, " peek"));
  } else {
    wrap.append(
      el("button", {
        class: "folder-head",
        title: f.name,
        "aria-label": `${f.name}, folder, expanded`,
        "aria-expanded": "true",
        "data-menu": "folder",
        "data-folder": f.id,
        draggable: dragOk() ? "true" : false,
        onclick: toggle,
      }, folderIcon(colour), el("span", { class: "folder-name" }, f.name)),
      el("div", { class: "folder-guilds" }, guilds.map((g) => railGuildButton(g))),
    );
  }
  return wrap;
}

renderGuilds = function () {
  const byId = new Map(state.guilds.map((g) => [g.id, g]));
  $("guild-list").replaceChildren(
    ...railItems().map((it) => (it.type === "folder" ? railFolder(it) : byId.has(it.id) ? railGuildButton(byId.get(it.id)) : null)),
  );
};

// ---------- menus ----------

function folderMenuItemsForGuild(g) {
  const items = railItems();
  const host = folderOf(items, g.id);
  const folders = items.filter((it) => it.type === "folder" && it !== host);
  return [
    {
      label: "Add to folder",
      submenu: [
        ...folders.map((f) => ({ label: f.name, swatch: folderHex(f.color), onClick: () => addToFolder(g.id, f.id) })),
        folders.length ? "sep" : null,
        { label: "New folder…", onClick: () => newFolder(g.id) },
      ],
    },
    host ? { label: `Remove from ${host.name}`, onClick: () => removeFromFolder(g.id) } : null,
    { label: "Move up", disabled: !canMoveGuild(g.id, -1), onClick: () => moveGuild(g.id, -1) },
    { label: "Move down", disabled: !canMoveGuild(g.id, 1), onClick: () => moveGuild(g.id, 1) },
  ];
}

MENUS.guild = withExtraMenuItems(MENUS.guild, (e, node) => {
  const g = state.guilds.find((x) => x.id === node.dataset.id);
  // Only from the rail itself (the outhouse name menu has no folders).
  if (!g || !node.closest?.("#guild-list")) return [];
  return folderMenuItemsForGuild(g);
}, (label) => label === "Copy outhouse ID");

MENUS.folder = (e, node) => {
  const f = folderById(railItems(), node.dataset.folder);
  if (!f) return;
  openMenu(e.clientX, e.clientY, [
    { label: f.collapsed ? "Expand" : "Collapse", onClick: () => setFolder(f.id, { collapsed: !f.collapsed }) },
    { label: "Collapse all folders", onClick: () => collapseAllFolders(true) },
    { label: "Expand all folders", onClick: () => collapseAllFolders(false) },
    "sep",
    { label: "Rename…", onClick: () => renameFolder(f.id) },
    {
      label: "Colour",
      submenu: FOLDER_COLOURS.map(([key, label, hex]) => ({ label, swatch: hex, checked: (f.color || "ember") === key, onClick: () => setFolder(f.id, { color: key === "ember" ? null : key }) })),
    },
    { label: "Move up", disabled: !canMoveFolder(f.id, -1), onClick: () => moveFolder(f.id, -1) },
    { label: "Move down", disabled: !canMoveFolder(f.id, 1), onClick: () => moveFolder(f.id, 1) },
    "sep",
    { label: "Delete folder", danger: true, onClick: () => deleteFolder(f.id) },
  ]);
};

// ---------- drag and drop (mouse) ----------

(function railDragAndDrop() {
  const list = $("guild-list");
  let drag = null; // {kind: "guild"|"folder", id}
  let drop = null; // {node, zone}
  const clear = () => {
    list.querySelectorAll(".drop-before, .drop-after, .drop-into").forEach((n) => n.classList.remove("drop-before", "drop-after", "drop-into"));
    list.classList.remove("drop-end");
    drop = null;
  };

  list.addEventListener("dragstart", (e) => {
    const node = e.target.closest?.("[draggable='true']");
    if (!node) return;
    drag = node.dataset.folder && !node.classList.contains("guild")
      ? { kind: "folder", id: node.dataset.folder }
      : { kind: "guild", id: node.dataset.id };
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", drag.id);
    requestAnimationFrame(() => node.classList.add("dragging"));
  });
  list.addEventListener("dragend", () => {
    drag = null;
    clear();
    list.querySelectorAll(".dragging").forEach((n) => n.classList.remove("dragging"));
  });

  // Where would it go? A guild: above or below it. A folder header: above,
  // into it (outhouses only), or below. Empty space at the end: last.
  const where = (e) => {
    const node = e.target.closest?.(".guild, .folder-head, .folder-tile");
    if (!node || !list.contains(node)) return { node: null, zone: "end" };
    const r = node.getBoundingClientRect();
    const y = (e.clientY - r.top) / r.height;
    if (node.classList.contains("guild")) {
      if (node.classList.contains("peek")) return { node: node.closest(".rail-folder").querySelector(".folder-tile"), zone: "into" };
      return { node, zone: y < 0.5 ? "before" : "after" };
    }
    if (drag.kind === "guild") return { node, zone: y < 0.25 ? "before" : (y > 0.75 && node.classList.contains("folder-tile")) ? "after" : "into" };
    return { node, zone: y < 0.5 ? "before" : "after" };
  };

  list.addEventListener("dragover", (e) => {
    if (!drag) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    const w = where(e);
    if (drop && drop.node === w.node && drop.zone === w.zone) return;
    clear();
    drop = w;
    if (!w.node) list.classList.add("drop-end");
    else {
      // A folder dropped beside an outhouse in a folder goes beside that folder.
      const target = drag.kind === "folder" && w.node.closest(".folder-guilds") ? w.node.closest(".rail-folder") : w.node;
      target.classList.add(`drop-${w.zone}`);
    }
  });
  list.addEventListener("dragleave", (e) => { if (!list.contains(e.relatedTarget)) clear(); });

  list.addEventListener("drop", (e) => {
    if (!drag) return;
    e.preventDefault();
    const d = drag;
    const w = drop || where(e);
    clear();
    applyDrop(d, w);
  });

  function applyDrop(d, w) {
    updateRail((items) => {
      let moving;
      if (d.kind === "guild") {
        if (w.node && w.node.dataset.id === d.id) return false;
        detachGuild(items, d.id);
        moving = { type: "guild", id: d.id };
      } else {
        if (w.node && (w.node.dataset.folder === d.id || w.node.closest(".rail-folder")?.dataset.folder === d.id)) return false;
        const i = items.findIndex((it) => it.type === "folder" && it.id === d.id);
        if (i < 0) return false;
        [moving] = items.splice(i, 1);
      }
      if (!w.node) { items.push(moving); return; }
      const inFolder = w.node.closest(".folder-guilds") ? w.node.closest(".rail-folder").dataset.folder : null;
      const asFolder = w.node.dataset.folder && !w.node.classList.contains("guild") ? w.node.dataset.folder : null;
      if (asFolder) {
        const f = folderById(items, asFolder);
        if (!f) { items.push(moving); return; }
        if (w.zone === "into" && moving.type === "guild") { f.guild_ids.push(moving.id); return; }
        items.splice(items.indexOf(f) + (w.zone === "after" ? 1 : 0), 0, moving);
        return;
      }
      const targetId = w.node.dataset.id;
      if (inFolder && moving.type === "guild") {
        const f = folderById(items, inFolder);
        const at = f.guild_ids.indexOf(targetId);
        f.guild_ids.splice(at < 0 ? f.guild_ids.length : at + (w.zone === "after" ? 1 : 0), 0, moving.id);
        return;
      }
      if (inFolder) {
        const f = folderById(items, inFolder);
        items.splice(items.indexOf(f) + (w.zone === "after" ? 1 : 0), 0, moving);
        return;
      }
      const at = items.findIndex((it) => it.type === "guild" && it.id === targetId);
      items.splice(at < 0 ? items.length : at + (w.zone === "after" ? 1 : 0), 0, moving);
    });
  }
})();

// ---------- User settings → Folders ----------

let foldersPageNode = null;
function refreshFoldersPage() {
  if (foldersPageNode?.isConnected) drawFoldersPage(foldersPageNode);
  else foldersPageNode = null;
}

function drawFoldersPage(main) {
  foldersPageNode = main;
  const items = railItems();
  const folders = items.filter((it) => it.type === "folder");
  const loose = items.filter((it) => it.type === "guild").map((it) => it.id);
  const iconBtn = (label, text, onClick, disabled = false, cls = "ghost") =>
    el("button", { type: "button", class: `${cls} fo-btn`, title: label, "aria-label": label, disabled, onclick: onClick }, text);
  const folderCard = (f) => {
    const add = el("select", { class: "select fo-add", "aria-label": `Add an outhouse to ${f.name}`, onchange: () => { if (add.value) addToFolder(add.value, f.id); } },
      el("option", { value: "" }, "＋ Add an outhouse…"),
      state.guilds.filter((g) => !f.guild_ids.includes(g.id)).map((g) => el("option", { value: g.id }, guildName(g.id))));
    const colour = el("select", { class: "select fo-colour", "aria-label": "Colour", onchange: () => setFolder(f.id, { color: colour.value === "ember" ? null : colour.value }) },
      FOLDER_COLOURS.map(([key, label]) => el("option", { value: key, selected: (f.color || "ember") === key }, label)));
    return el("div", { class: "fo-card", "data-folder": f.id, style: `--fc:${folderHex(f.color)}` },
      el("div", { class: "fo-head" },
        folderIcon(folderHex(f.color), 20),
        el("strong", { class: "fo-name" }, f.name),
        el("span", { class: "muted small" }, `${f.guild_ids.length} outhouse${f.guild_ids.length === 1 ? "" : "s"}`),
        el("span", { class: "fo-actions" },
          iconBtn("Move folder up", "↑", () => moveFolder(f.id, -1), !canMoveFolder(f.id, -1)),
          iconBtn("Move folder down", "↓", () => moveFolder(f.id, 1), !canMoveFolder(f.id, 1)),
        )),
      el("div", { class: "fo-row" },
        iconBtn("Rename", "Rename", () => renameFolder(f.id)),
        colour,
        el("label", { class: "fo-check" }, el("input", { type: "checkbox", checked: !!f.collapsed, onchange: (e) => setFolder(f.id, { collapsed: e.target.checked }) }), "Collapsed"),
        iconBtn("Delete folder", "Delete", () => deleteFolder(f.id), false, "danger-btn")),
      el("div", { class: "fo-guilds" },
        f.guild_ids.length ? null : el("div", { class: "muted small" }, "Nothing in here yet."),
        f.guild_ids.map((id) => el("div", { class: "fo-guild" },
          el("span", { class: "fo-gname" }, guildName(id)),
          iconBtn(`Move ${guildName(id)} up`, "↑", () => moveGuild(id, -1), !canMoveGuild(id, -1)),
          iconBtn(`Move ${guildName(id)} down`, "↓", () => moveGuild(id, 1), !canMoveGuild(id, 1)),
          iconBtn(`Take ${guildName(id)} out`, "Remove", () => removeFromFolder(id))))),
      add);
  };
  main.replaceChildren(
    el("h2", {}, "Folders"),
    el("p", { class: "muted" }, "Group outhouses on the left rail under headers, and collapse the ones you don't need open. Only you see your folders; they're the same on all your devices. On a computer you can also drag outhouses and folders around the rail; right-click (or long-press) for the same options."),
    el("div", { class: "fo-top" },
      el("button", { class: "primary", type: "button", onclick: () => newFolder() }, "＋ New folder"),
      folders.length ? el("button", { class: "ghost", type: "button", onclick: () => collapseAllFolders(true) }, "Collapse all") : null,
      folders.length ? el("button", { class: "ghost", type: "button", onclick: () => collapseAllFolders(false) }, "Expand all") : null),
    ...(folders.length ? folders.map(folderCard) : [el("p", { class: "muted small" }, "No folders yet.")]),
    loose.length ? el("h3", { class: "fo-sub" }, "Not in a folder") : null,
    loose.length ? el("div", { class: "fo-guilds fo-loose" }, loose.map((id) => el("div", { class: "fo-guild" },
      el("span", { class: "fo-gname" }, guildName(id)),
      iconBtn(`Move ${guildName(id)} up`, "↑", () => moveGuild(id, -1), !canMoveGuild(id, -1)),
      iconBtn(`Move ${guildName(id)} down`, "↓", () => moveGuild(id, 1), !canMoveGuild(id, 1)),
      folders.length ? el("select", { class: "select fo-move", "aria-label": `Put ${guildName(id)} in a folder`, onchange: (e) => { if (e.target.value) addToFolder(id, e.target.value); } },
        el("option", { value: "" }, "Put in folder…"), folders.map((f) => el("option", { value: f.id }, f.name))) : null))) : null,
  );
}

// ---------- live updates ----------

const foldersBaseHandle = handleEvent;
handleEvent = function (ev) {
  foldersBaseHandle(ev);
  if (ev.t === "READY" || ev.t === "GUILD_LAYOUT_CHANGED") loadRail();
  if (ev.t === "GUILD_UPDATE") {
    const g = state.guilds.find((x) => x.id === ev.d.id);
    if (g) {
      Object.assign(g, ev.d);
      if (g.id === state.guildId) $("guild-name").textContent = g.name;
      renderGuilds();
      refreshFoldersPage();
    }
  }
};
