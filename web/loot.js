// Outhouse: raid composition on raid cards (roles against slots, classes,
// armour, raid buffs, boss debuffs, utility and who can dispel what), soft
// reserves (pick items, see who else wants them, the raid leaders' table,
// lock, export), loot won, and the outhouse's loot log (also on profile
// cards). Loaded after raid-admin.js; uses the raid planner's helpers
// (renderEventCard hooks, raidWindow, canManageEvent, eventWhen, …).

const QUALITY_COLOURS = ["#9d9d9d", "#ffffff", "#1eff00", "#0070dd", "#a335ee", "#ff8000", "#e6cc80", "#00ccff"];
const qualityColour = (q) => QUALITY_COLOURS[q] ?? "#ffffff";
const RESERVE_LOCKS = [["deadline", "When sign-ups close (or at the start)"], ["start", "When the raid starts"], ["manual", "Only when I lock them"]];
const ARMOUR_LABEL = { cloth: "Cloth", leather: "Leather", mail: "Mail", plate: "Plate" };
const BUFF_KINDS = [["buff", "Raid buffs"], ["debuff", "Boss debuffs"], ["utility", "Utility"], ["dispel", "Dispels"], ["note", "Not tracked"]];
const plural = (n, one, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

// ---------- Composition ----------

const comp = { cache: new Map(), pending: new Map(), open: new Set(), extra: new Set() };

function compSig(ev, extra) {
  return JSON.stringify([ev.game, ev.max_tanks, ev.max_healers, ev.max_dps, extra,
    ev.signups.map((s) => [s.user_id, s.status, s.role, s.class, s.spec, s.character?.full_name ?? "", s.character?.game ?? ""])]);
}

// The composition for a raid (from the server, which has the buff table),
// cached until its sign-ups change. Returns what it has right now (maybe
// slightly old) and fills in [data-comp-key] nodes when a fresh one arrives.
function loadComp(ev, extra) {
  const key = `${ev.id}|${extra ? 1 : 0}`;
  const sig = compSig(ev, extra);
  const hit = comp.cache.get(key);
  if (hit && hit.sig === sig) return hit.data;
  if (comp.pending.get(key) !== sig) {
    comp.pending.set(key, sig);
    api("GET", `/api/events/${ev.id}/composition${extra ? "?extra=1" : ""}`)
      .then((data) => {
        comp.cache.set(key, { sig, data });
        for (const node of document.querySelectorAll(`[data-comp-key="${key}"]`)) node.compFill?.();
      })
      .catch(() => {})
      .finally(() => { if (comp.pending.get(key) === sig) comp.pending.delete(key); });
  }
  return hit?.data ?? null;
}

// Raid leaders: a compact "what's missing" line at the top of the card.
function raidNeedsLine(ev) {
  if (eventWhen(ev).over || !canManageEvent(ev)) return null;
  const key = `${ev.id}|0`;
  const node = el("div", { class: "ev-needs", "data-comp-key": key, hidden: true });
  node.compFill = () => {
    const c = comp.cache.get(key)?.data;
    node.hidden = !c;
    if (!c) return;
    node.classList.toggle("ok", !c.missing.length);
    node.title = "Counts the roster only (not tentative or bench). Open Composition for the details.";
    node.replaceChildren(...(c.missing.length
      ? [el("span", { class: "ev-needs-icon" }, "⚠️"), el("span", {}, el("strong", {}, "Need: "), c.missing.join(" · "))]
      : [el("span", { class: "ev-needs-icon" }, "✓"), el("span", {}, "Every role filled and the key raid buffs covered")]));
  };
  loadComp(ev, false);
  node.compFill();
  return node;
}

function buffTitle(b) {
  if (b.kind === "note") return `${b.name}: ${b.note}`;
  if (b.covered) return `${b.name}: ${b.providers.join(", ")}${b.note ? ` (${b.note})` : ""}`;
  return `${b.name}: nobody yet. Needs a ${b.sources.join(" or ")}${b.note ? ` (${b.note})` : ""}`;
}

function buffChip(b) {
  const state_ = b.kind === "note" ? "note" : b.covered ? "yes" : "no";
  const title = buffTitle(b);
  return el("button", {
    type: "button", class: `comp-buff ${state_}`, title, "data-buff": b.key,
    // Phones can't hover: a tap says who brings it.
    onclick: () => toast(title),
  }, el("span", { class: "comp-mark", "aria-hidden": "true" }, state_ === "note" ? "ℹ" : state_ === "yes" ? "✓" : "✗"),
  el("span", { class: "comp-buff-name" }, b.name),
  b.covered && b.providers.length > 1 ? el("span", { class: "comp-n" }, `×${b.providers.length}`) : null,
  el("span", { class: "sr-only" }, state_ === "yes" ? " covered" : state_ === "no" ? " missing" : ""));
}

function renderComp(ev, c, extra, rerender) {
  const roles = el("div", { class: "comp-roles" }, c.roles.map((r) => el("span", {
    class: `comp-role ${r.missing ? "short" : r.count > r.max ? "over" : "ok"}`,
    title: r.missing ? `${ROLE_INFO[r.role].label}: ${r.count} of ${r.max}, ${r.missing} to go` : `${ROLE_INFO[r.role].label}: ${r.count} of ${r.max}`,
  }, `${ROLE_INFO[r.role].icon} ${ROLE_INFO[r.role].label} ${r.count}/${r.max}`, r.missing ? el("strong", { class: "comp-need" }, ` · need ${r.missing}`) : null)));
  const classes = c.classes.length ? [
    el("div", { class: "comp-bar", role: "img", "aria-label": c.classes.map((k) => `${k.class || "No class"} ${k.count}`).join(", ") },
      c.classes.map((k) => el("span", { style: `flex:${k.count};background:${classColour(k.class) ?? "#6b6b6b"}`, title: `${k.class || "No class"}: ${k.people.join(", ")}` }))),
    el("div", { class: "comp-classes" }, c.classes.map((k) => el("span", {
      class: "comp-class", style: `--cc:${classColour(k.class) ?? "#9a9a9a"}`, title: k.people.join(", "),
    }, el("span", { class: "comp-dot" }), `${k.class || "No class"} `, el("strong", {}, k.count)))),
  ] : [el("p", { class: "muted small", style: "margin:0" }, extra ? "Nobody signed up yet." : "Nobody on the roster yet.")];
  const armour = el("div", { class: "comp-armour" }, el("span", { class: "comp-label" }, "Armour"),
    c.armour.map((a) => el("span", { class: `comp-arm${a.count ? "" : " zero"}` }, `${ARMOUR_LABEL[a.kind]} `, el("strong", {}, a.count))));
  const groups = BUFF_KINDS.map(([kind, label]) => {
    const list = c.buffs.filter((b) => b.kind === kind);
    if (!list.length) return null;
    const have = list.filter((b) => b.covered).length;
    return el("div", { class: `comp-group k-${kind}` },
      el("div", { class: "comp-group-head" }, label, kind === "note" ? null : el("span", { class: "muted" }, ` ${have}/${list.length}`)),
      el("div", { class: "comp-buffs" }, list.map(buffChip)));
  });
  const toggle = el("label", { class: "comp-extra" },
    el("input", { type: "checkbox", checked: extra, onchange: (e) => {
      if (e.target.checked) comp.extra.add(ev.id); else comp.extra.delete(ev.id);
      rerender();
    } }), "Count tentative & bench");
  return el("div", { class: "comp-inner" },
    el("div", { class: "comp-top" }, roles, toggle),
    ...classes, armour,
    // What's missing lives here (the card only shows a "gaps" chip).
    c.missing.length ? el("div", { class: "comp-missing" }, el("strong", {}, "Need: "), c.missing.join(" · ")) : null,
    el("div", { class: "comp-groups" }, groups),
    el("p", { class: "muted small comp-foot" }, c.era === "tbc" ? "The Burning Crusade buffs. Hover or tap one to see who brings it." : "Classic-era buffs (WoW: Forever and Classic Era). Hover or tap one to see who brings it."));
}

function raidCompSection(ev) {
  const extra = comp.extra.has(ev.id);
  const key = `${ev.id}|${extra ? 1 : 0}`;
  const details = el("details", { class: "ev-comp", "data-comp-key": key });
  if (comp.open.has(ev.id)) details.open = true;
  const sum = el("span", { class: "ev-comp-sum" });
  const body = el("div", { class: "ev-comp-body" });
  details.append(el("summary", {}, el("span", { class: "ev-comp-title" }, "🧩 Composition"), sum), body);
  const rerender = () => { comp.open.add(ev.id); details.replaceWith(raidCompSection(ev)); };
  details.compFill = () => {
    const c = comp.cache.get(key)?.data;
    if (c) {
      const checked = c.buffs.filter((b) => b.kind !== "note");
      const have = checked.filter((b) => b.covered).length;
      sum.replaceChildren(`${plural(c.counted, "person", "people")}${extra ? " incl. tentative & bench" : ""} · ${have}/${checked.length} buffs`,
        c.missing.length ? el("span", { class: "ev-comp-gaps" }, ` · ${plural(c.missing.length, "gap")}`) : null);
    }
    if (!details.open) return;
    body.replaceChildren(c ? renderComp(ev, c, extra, rerender) : el("p", { class: "muted small" }, "Working it out…"));
  };
  details.addEventListener("toggle", () => {
    if (details.open) { comp.open.add(ev.id); loadComp(ev, extra); } else comp.open.delete(ev.id);
    details.compFill();
  });
  if (details.open || canManageEvent(ev) || comp.cache.has(key)) loadComp(ev, extra);
  details.compFill();
  return details;
}

// ---------- Items: icons, names, tooltips (linked to Wowhead) ----------

// Wowhead's address for a branch ("classic", "tbc", …; "" = retail).
function wowheadBranchUrl(branch) {
  const base = (state.wowInfo?.wowhead_base || "https://www.wowhead.com").replace(/\/+$/, "");
  if (branch == null) return (state.wowInfo?.wowhead_url || `${base}/classic`).replace(/\/+$/, "");
  return branch ? `${base}/${branch}` : base;
}
// The branch a raid's items link to.
const branchForGame = (game) => state.wowInfo?.wowhead_branches?.[game || "forever"] ?? null;
const evBranch = (ev) => (loot.dialog && loot.ev?.id === ev?.id && loot.table?.item_branch != null ? loot.table.item_branch : branchForGame(ev?.game));

const itemInfo = new Map();
function itemData(id, branch) {
  const key = `${id}|${branch ?? ""}`;
  if (!itemInfo.has(key)) itemInfo.set(key, api("GET", `/api/wow/items/${id}${branch ? `?branch=${encodeURIComponent(branch)}` : ""}`).catch(() => null));
  return itemInfo.get(key);
}

// The item's icon (our copy of Wowhead's), or a plain square if there's none.
function itemIcon(id, size = "") {
  const img = el("img", { class: `item-icon${size ? ` ${size}` : ""}`, src: `/api/wow/item-icons/${id}`, alt: "", loading: "lazy", decoding: "async", draggable: "false" });
  img.addEventListener("error", () => { img.removeAttribute("src"); img.classList.add("none"); }, { once: true });
  return img;
}

let itemTip = null;
function hideItemTip() { itemTip?.remove(); itemTip = null; }
// Scrolling moves the tooltip with its item, and hides it once the item's gone.
function followItemTip() {
  const anchor = itemTip?.anchor;
  if (!anchor) return;
  const r = anchor.getBoundingClientRect();
  if (!anchor.isConnected || r.bottom < 0 || r.top > window.innerHeight || !r.width) return hideItemTip();
  placeTip(itemTip, anchor);
}

function placeTip(tip, anchor) {
  const r = anchor.getBoundingClientRect();
  const t = tip.getBoundingClientRect();
  let y = r.bottom + 6;
  if (y + t.height > window.innerHeight - 8) y = Math.max(8, r.top - t.height - 6);
  tip.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - t.width - 8))}px`;
  tip.style.top = `${y}px`;
}

const TIP_Q = new Set(["q", "q0", "q1", "q2", "q3", "q4", "q5", "q6", "q7"]);
function tipLines(info) {
  if (info?.lines?.length) {
    return info.lines.map((l) => el("div", { class: `tip-line${TIP_Q.has(l.q) ? ` ${l.q}` : ""}` },
      el("span", {}, l.left), l.right ? el("span", { class: "tip-right" }, l.right) : null));
  }
  return (info?.tooltip || "").split("\n").filter(Boolean).map((l) => el("div", { class: "tip-line" }, el("span", {}, l)));
}

function showItemTip(anchor, id, name, quality, branch, { touch = false } = {}) {
  hideItemTip();
  const tip = el("div", { class: `item-tip${touch ? " touch" : ""}`, role: "tooltip" },
    el("div", { class: "item-tip-head" }, itemIcon(id, "big"), el("strong", { class: `q${quality}` }, name)),
    el("div", { class: "item-tip-body muted small" }, "Loading…"));
  tip.dataset.item = id;
  tip.anchor = anchor;
  itemTip = tip;
  ((typeof raidHost === "function" && raidHost()) || document.body).append(tip);
  placeTip(tip, anchor);
  itemData(id, branch).then((info) => {
    if (itemTip !== tip) return;
    const lines = tipLines(info);
    tip.replaceChildren(
      el("div", { class: "item-tip-head" }, itemIcon(id, "big"),
        el("strong", { class: `q${info?.quality ?? quality}` }, info?.name || name)),
      lines.length ? el("div", { class: "item-tip-body" }, lines) : el("div", { class: "item-tip-body muted small" }, `Item ${id}`),
      touch ? el("a", { class: "item-tip-open", href: anchor.href, target: "_blank", rel: "noopener noreferrer" }, "Open on Wowhead ↗") : null);
    placeTip(tip, anchor);
  });
}

// An item: icon and name in its quality colour, linking to Wowhead (the
// raid's branch) in a new tab, with a tooltip on hover (or the first tap).
function itemName(id, name, quality, branch = null) {
  const a = el("a", { class: `item-name q${quality}`, href: `${wowheadBranchUrl(branch)}/item=${id}`, target: "_blank", rel: "noopener noreferrer", "data-item-link": id },
    itemIcon(id), el("span", { class: "item-label" }, name));
  let touched = false;
  a.addEventListener("pointerdown", (e) => { touched = e.pointerType === "touch"; });
  a.addEventListener("mouseenter", () => { if (!touched) showItemTip(a, id, name, quality, branch); });
  a.addEventListener("focus", () => { if (!touched) showItemTip(a, id, name, quality, branch); });
  a.addEventListener("mouseleave", () => { if (!touched) hideItemTip(); });
  a.addEventListener("blur", () => { if (!touched) hideItemTip(); });
  a.addEventListener("click", (e) => {
    // Phones: the first tap shows the tooltip (with a link to Wowhead).
    if (!touched) return;
    if (itemTip?.dataset.item === String(id) && itemTip.classList.contains("touch")) return;
    e.preventDefault();
    showItemTip(a, id, name, quality, branch, { touch: true });
  });
  return a;
}

// ---------- Soft reserves on the card ----------

const reserversOf = (ev, itemId) => ev.reserves.filter((r) => r.item_id === itemId);
const signupOf = (ev, userId) => ev.signups.find((s) => s.user_id === userId);
const reserverName = (ev, userId) => {
  const s = signupOf(ev, userId);
  return s ? signupLabel(s) : (memberById(userId)?.display_name ?? "Someone");
};
const myLiveSignup = (ev) => ev.signups.find((s) => s.user_id === state.me.id && s.status !== "no");

function srBadge(ev, itemId, { mineToo = true } = {}) {
  const who = reserversOf(ev, itemId);
  if (!who.length) return null;
  const others = who.filter((r) => r.user_id !== state.me.id).length;
  const contested = who.length > 1;
  return el("span", {
    class: `sr-count${contested ? " contested" : ""}`,
    title: `Reserved by ${who.map((r) => reserverName(ev, r.user_id)).join(", ")}`,
  }, mineToo ? `${who.length} SR` : `${others} other${others === 1 ? "" : "s"}`);
}

function lockText(ev) {
  if (ev.reserves_locked) return ev.reserve_lock_override === true ? "🔒 Locked by the raid leader" : "🔒 Locked";
  if (ev.reserve_lock_override === false) return "Unlocked by the raid leader";
  if (ev.reserve_lock === "manual") return "Open until the raid leader locks them";
  const at = ev.reserve_lock === "start" || !ev.signups_close_at ? new Date(ev.starts_at) : new Date(ev.signups_close_at);
  return `Locks ${relativeTime(at)}`;
}

function raidLootRow(ev) {
  const when = eventWhen(ev);
  const manager = canManageEvent(ev);
  const sr = ev.soft_reserves > 0;
  if (!sr && !(manager && when.started)) return null;
  const mine = ev.reserves.filter((r) => r.user_id === state.me.id);
  const signed = !!myLiveSignup(ev);
  return el("div", { class: `ev-loot${ev.reserves_locked ? " locked" : ""}` },
    sr ? el("div", { class: "ev-loot-head" },
      el("strong", {}, "🎲 Soft reserves"),
      el("span", { class: "muted small" }, [`${ev.soft_reserves} each`, ev.loot_instance_name, plural(ev.reserves.length, "reserve")].filter(Boolean).join(" · ")),
      el("span", { class: `ev-loot-lock${ev.reserves_locked ? " on" : ""}` }, lockText(ev))) : null,
    sr && mine.length ? el("div", { class: "ev-loot-mine" }, el("span", { class: "muted small" }, "Yours:"),
      mine.map((r) => el("span", { class: "sr-chip" }, itemName(r.item_id, r.item_name, r.quality, branchForGame(ev.game)), srBadge(ev, r.item_id)))) : null,
    el("div", { class: "ev-actions" },
      sr ? el("button", { class: `ev-btn sr${signed && !mine.length && !ev.reserves_locked ? " want" : ""}`, onclick: () => openLoot(ev, signed ? "reserve" : "all") },
        signed ? (mine.length ? (ev.reserves_locked ? "My reserves" : "Change reserves") : "🎲 Reserve loot") : "See reserves") : null,
      sr && (manager || ev.reserves.length) ? el("button", { class: "ev-btn ghost", onclick: () => openLoot(ev, "all") }, manager ? "Reserves table" : "Everyone's reserves") : null,
      manager && when.started ? el("button", { class: "ev-btn loot", onclick: () => openLoot(ev, "won") }, "💰 Loot won") : null));
}

// ---------- The soft reserves window ----------
// Everything about a raid's loot in one window: picking reserves (boss by
// boss, with filters), everyone's reserves, loot won and, for raid leaders,
// the loot list itself (Wowhead pages, refresh, import by link).

const CATEGORIES = [["all", "All"], ["weapon", "Weapons"], ["armour", "Armour"], ["trinket", "Trinkets"], ["ring", "Rings"], ["neck", "Necks"], ["back", "Backs"], ["other", "Other"]];
const loot = { dialog: null, root: null, ev: null, tab: "reserve", view: "item", table: null, awards: [], search: null, found: null, finding: false,
  cat: "all", suits: false, junk: false, trash: true, open: new Map(), polls: 0, pollTimer: null };

// Opens the window for a raid (by ID; the raid card's "Soft reserves" link).
async function openSoftReserves(eventId, tab = "reserve") {
  const id = String(eventId);
  const known = (loot.ev?.id === id && loot.ev) || null;
  const ev = known ?? await api("GET", `/api/events/${id}`).catch(() => null);
  if (!ev) return toast("Couldn't open that raid.");
  return openLoot(ev, tab);
}
window.openSoftReserves = openSoftReserves;

async function openLoot(ev, tab = "reserve") {
  loot.dialog?.close();
  const dialog = raidWindow("loot-dialog", () => { if (loot.dialog === dialog) { loot.dialog = null; hideItemTip(); clearTimeout(loot.pollTimer); } });
  const sameRaid = loot.ev?.id === ev.id;
  Object.assign(loot, { dialog, ev, tab, table: null, awards: [], found: null, finding: false, polls: 0 });
  if (!sameRaid) Object.assign(loot, { cat: "all", suits: false, junk: false, trash: true, open: new Map() });
  loot.root = el("div", { class: "loot-root" });
  loot.search = el("input", { type: "search", class: "loot-search", placeholder: "Search, or paste a Wowhead link or item ID", maxlength: 400, "aria-label": "Find an item" });
  loot.search.addEventListener("input", () => { loot.found = null; drawLootList(); });
  loot.search.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); lookUpItem(); } });
  dialog.append(loot.root);
  drawLoot();
  await refreshLoot();
}

async function refreshLoot() {
  const ev = loot.ev;
  const [table, awards] = await Promise.all([
    api("GET", `/api/events/${ev.id}/loot-table`).catch(() => null),
    api("GET", `/api/events/${ev.id}/loot`).catch(() => []),
  ]);
  if (!loot.dialog || loot.ev.id !== ev.id) return;
  setTable(table ?? { instance: "", instance_name: "", items: [], bosses: [], sources: [] });
  loot.awards = awards;
  drawLoot();
}

// A new loot list; while Wowhead is still being read, ask again shortly.
function setTable(table) {
  loot.table = table;
  clearTimeout(loot.pollTimer);
  if (table.refreshing && loot.polls < 20) {
    loot.polls++;
    const ev = loot.ev;
    loot.pollTimer = setTimeout(async () => {
      const t = await api("GET", `/api/events/${ev.id}/loot-table`).catch(() => null);
      if (!t || !loot.dialog || loot.ev?.id !== ev.id) return;
      setTable(t);
      drawLoot();
    }, 3000);
  } else if (!table.refreshing) loot.polls = 0;
}

// A raid changed (reply to something we did, or live from the gateway).
function lootEventChanged(ev) {
  if (!loot.dialog || loot.ev?.id !== ev.id) return;
  loot.ev = ev;
  drawLoot();
  clearTimeout(lootEventChanged.timer);
  lootEventChanged.timer = setTimeout(refreshLoot, 150);
}

function applyLootReply(reply) {
  if (!reply) return;
  applyEventReply(reply);
  if (reply.event) lootEventChanged(reply.event);
}

async function reserveItem(itemId) {
  const reply = await tryApi("PUT", `/api/events/${loot.ev.id}/reserves`, { item_id: itemId }).catch(() => null);
  applyLootReply(reply);
}

async function unreserveItem(itemId, userId = state.me.id) {
  const q = userId !== state.me.id ? `?user_id=${userId}` : "";
  const reply = await tryApi("DELETE", `/api/events/${loot.ev.id}/reserves/${itemId}${q}`).catch(() => null);
  applyLootReply(reply);
}

// A pasted Wowhead zone, NPC or object (chest) link: its loot can be imported.
function wowheadListLink(text) {
  const m = /wowhead\.com\/(?:[a-z0-9_-]+\/)?(zone|npc|object)=(\d+)/i.exec(text || "");
  return m ? { kind: m[1].toLowerCase(), id: Number(m[2]) } : null;
}
const KIND_LABEL = { zone: "zone", npc: "boss", object: "chest" };

async function lookUpItem() {
  const q = loot.search.value.trim();
  if (!q || loot.finding) return;
  if (wowheadListLink(q)) {
    if (canManageEvent(loot.ev)) importSource(q);
    return;
  }
  loot.finding = true;
  drawLootList();
  const found = await api("GET", `/api/wow/items?q=${encodeURIComponent(q)}`).catch(() => null);
  loot.finding = false;
  if (!loot.dialog || loot.search.value.trim() !== q) return drawLootList();
  loot.found = found ?? { none: q };
  drawLootList();
}

function drawLoot() {
  if (!loot.dialog) return;
  const ev = loot.ev;
  const sr = ev.soft_reserves > 0;
  const manager = canManageEvent(ev);
  const tabs = [sr ? ["reserve", "Reserve"] : null, sr ? ["all", `Everyone's (${ev.reserves.length})`] : null, ["won", `Loot won (${loot.awards.length})`],
    manager ? ["list", "Loot list"] : null].filter(Boolean);
  if (!tabs.some(([k]) => k === loot.tab)) loot.tab = tabs[0][0];
  const focused = document.activeElement === loot.search;
  const sel = focused ? [loot.search.selectionStart, loot.search.selectionEnd] : null;
  const scroller = loot.root.querySelector(".loot-body");
  const scroll = scroller?.dataset.tab === loot.tab ? scroller.scrollTop : 0;
  const sub = [loot.table?.instance_name || ev.loot_instance_name || (loot.table ? "No raid picked" : ""), sr ? `${ev.soft_reserves} reserve${ev.soft_reserves === 1 ? "" : "s"} each` : "Soft reserves are off", sr ? lockText(ev) : null].filter(Boolean);
  const head = el("div", { class: "loot-head" },
    el("div", { class: "loot-title" }, el("h2", {}, sr ? "🎲 Soft reserves" : "💰 Loot"),
      el("div", { class: "muted small" }, el("strong", {}, ev.title), ` · ${eventWhen(ev).text}`),
      el("div", { class: "loot-sub small" }, sub.join(" · "))),
    closeX(loot.dialog));
  const tabBar = el("div", { class: "loot-tabs", role: "tablist" }, tabs.map(([k, label]) => el("button", {
    type: "button", role: "tab", class: `loot-tab${loot.tab === k ? " on" : ""}`, "aria-selected": loot.tab === k ? "true" : "false", "data-tab": k,
    onclick: () => { loot.tab = k; drawLoot(); },
  }, label)));
  const body = !loot.table ? el("p", { class: "muted" }, "Loading…")
    : loot.tab === "reserve" ? drawReserveTab(ev, manager)
      : loot.tab === "all" ? drawAllTab(ev, manager)
        : loot.tab === "list" ? drawListTab(ev)
          : drawWonTab(ev, manager);
  const bodyBox = el("div", { class: "loot-body", "data-tab": loot.tab }, body);
  loot.root.replaceChildren(head, tabs.length > 1 ? tabBar : null, bodyBox);
  bodyBox.scrollTop = scroll;
  if (focused && loot.search.isConnected) { loot.search.focus(); loot.search.setSelectionRange(...sel); }
}

function lootBanner() {
  const t = loot.table;
  if (!t) return null;
  if (t.refreshing) return el("p", { class: "loot-banner busy" }, el("span", { class: "loot-spin", "aria-hidden": "true" }), "Reading the full loot list from Wowhead… it'll appear here in a moment.");
  if (t.offline) return el("p", { class: "loot-banner warn" }, "Wowhead couldn't be reached, so this is a short built-in list for now. You can still reserve anything by its Wowhead link or name.");
  return null;
}

function drawReserveTab(ev, manager) {
  const me = myLiveSignup(ev);
  const mine = ev.reserves.filter((r) => r.user_id === state.me.id);
  const notOpen = signupsNotOpen(ev) && !manager;
  const canEdit = !!me && !notOpen && (!ev.reserves_locked || manager) && !eventWhen(ev).over;
  const status = !me ? "Sign up for the raid first (from its card), then come back to pick your reserves."
    : notOpen ? "Sign-ups aren't open yet, so neither are reserves."
      : ev.reserves_locked && !manager ? "🔒 Reserves are locked. Ask the raid leader if you need a change."
        : ev.reserves_locked ? "🔒 Reserves are locked for everyone else. As raid leader you can still change them."
          : `Pick ${ev.soft_reserves === 1 ? "one item" : `up to ${ev.soft_reserves} items`}. Everyone can see what's reserved, so you'll know who you're up against.`;
  loot.canEdit = canEdit;
  loot.atLimit = mine.length >= ev.soft_reserves;
  loot.myClass = me?.class || "";
  loot.listBox = el("div", { class: "loot-list" });
  loot.filterBox = el("div", { class: "loot-filters" });
  drawLootList();
  const branch = evBranch(ev);
  return el("div", { class: "loot-reserve" },
    el("p", { class: `loot-status${!me || (ev.reserves_locked && !manager) ? " warn" : ""}` }, status),
    el("div", { class: "loot-mine" },
      el("div", { class: "loot-mine-head" }, `Your reserves (${mine.length}/${ev.soft_reserves})`),
      mine.length ? el("div", { class: "loot-mine-list" }, mine.map((r) => el("span", { class: "sr-chip big" },
        itemName(r.item_id, r.item_name, r.quality, branch), srBadge(ev, r.item_id),
        canEdit ? el("button", { type: "button", class: "sr-x", title: `Take back ${r.item_name}`, "aria-label": `Take back ${r.item_name}`, onclick: () => unreserveItem(r.item_id) }, "✕") : null)))
        : el("span", { class: "muted small" }, "Nothing yet.")),
    lootBanner(),
    el("div", { class: "loot-finder" }, loot.search, loot.filterBox),
    loot.listBox);
}

const CLASS_BIT = { Warrior: 1, Paladin: 2, Hunter: 4, Rogue: 8, Priest: 16, Shaman: 64, Mage: 128, Warlock: 256, Druid: 1024 };
const suitsClass = (it, cls) => !it.classes || !CLASS_BIT[cls] || (it.classes & CLASS_BIT[cls]) !== 0;

function lootItemRow(ev, it, manager, branch) {
  const mine = ev.reserves.some((r) => r.user_id === state.me.id && r.item_id === it.item_id);
  const btn = mine ? el("button", { type: "button", class: "ev-btn on sr-pick", disabled: !loot.canEdit, onclick: () => unreserveItem(it.item_id), title: loot.canEdit ? "Take it back" : "" }, "Reserved ✓")
    : el("button", { type: "button", class: "ev-btn sr-pick", disabled: !loot.canEdit || loot.atLimit, title: loot.atLimit && loot.canEdit ? "Take back one of yours first" : "", onclick: () => reserveItem(it.item_id) }, "Reserve");
  const won = loot.awards.filter((a) => a.item_id === it.item_id);
  const count = reserversOf(ev, it.item_id).length;
  const meta = [it.slot, it.chance != null ? `${it.chance < 1 ? it.chance.toFixed(1) : Math.round(it.chance)}% drop` : null, it.boss && it.showBoss ? it.boss : null,
    won.length ? `won by ${won.map((a) => a.character_name || a.display_name).join(", ")}` : null].filter(Boolean);
  return el("div", { class: `loot-row${mine ? " mine" : ""}`, "data-item": it.item_id, "data-cat": it.category || "" },
    el("div", { class: "loot-row-main" }, itemName(it.item_id, it.name, it.quality, branch),
      meta.length ? el("span", { class: "loot-row-meta muted small" }, meta.join(" · ")) : null),
    count ? srBadge(ev, it.item_id) : el("span", { class: "sr-count none", title: "Nobody has reserved it yet" }, "0 SR"),
    manager && it.custom ? el("button", { type: "button", class: "icon-btn small", title: "Remove from this raid's list", "aria-label": `Remove ${it.name} from the list`, onclick: () => removeListItem(it) }, "🗑") : null,
    loot.tab === "reserve" ? btn : null);
}

function chip(label, on, onclick, extra = {}) {
  return el("button", { type: "button", class: `loot-chip${on ? " on" : ""}`, "aria-pressed": on ? "true" : "false", onclick, ...extra }, label);
}

function drawFilters(items) {
  const box = loot.filterBox;
  if (!box) return;
  const cats = CATEGORIES.filter(([k]) => k === "all" || items.some((it) => it.category === k));
  const cls = loot.myClass;
  box.replaceChildren(
    el("div", { class: "loot-chips", role: "group", "aria-label": "Item type" }, cats.map(([k, label]) =>
      chip(label, loot.cat === k, () => { loot.cat = k; drawLootList(); }, { "data-cat": k }))),
    el("div", { class: "loot-chips toggles" },
      cls && CLASS_BIT[cls] ? chip(`Suits my ${cls}`, loot.suits, () => { loot.suits = !loot.suits; drawLootList(); }, { "data-filter": "suits", style: classColour(cls) ? `--cc:${classColour(cls)}` : "" }) : null,
      items.some((it) => it.trash) ? chip("Trash drops", loot.trash, () => { loot.trash = !loot.trash; drawLootList(); }, { "data-filter": "trash" }) : null,
      items.some((it) => it.quality < 2) ? chip("Grey & white items", loot.junk, () => { loot.junk = !loot.junk; drawLootList(); }, { "data-filter": "junk" }) : null,
      el("span", { class: "loot-chip-gap" }),
      chip("Expand all", false, () => { for (const k of loot.sections ?? []) loot.open.set(k, true); drawLootList(); }, { class: "loot-chip plain" }),
      chip("Collapse all", false, () => { for (const k of loot.sections ?? []) loot.open.set(k, false); drawLootList(); }, { class: "loot-chip plain" })));
}

function drawLootList() {
  const box = loot.listBox;
  if (!box || !loot.table) return;
  const ev = loot.ev;
  const manager = canManageEvent(ev);
  const branch = evBranch(ev);
  const raw = loot.search.value.trim();
  const q = raw.toLowerCase();
  const all = loot.table.items;
  drawFilters(all);
  const reservedIds = new Set(ev.reserves.map((r) => r.item_id));
  const filtering = !!q || loot.cat !== "all" || loot.suits;
  const items = all.filter((it) => {
    if (q && !(it.name.toLowerCase().includes(q) || String(it.item_id) === q || (it.boss || "").toLowerCase().includes(q) || (it.slot || "").toLowerCase().includes(q))) return false;
    if (reservedIds.has(it.item_id) && loot.cat === "all" && !loot.suits) return true;
    if (!loot.junk && it.quality < 2 && !reservedIds.has(it.item_id)) return false;
    if (!loot.trash && it.trash) return false;
    if (loot.cat !== "all" && (it.category || "other") !== loot.cat) return false;
    if (loot.suits && !suitsClass(it, loot.myClass)) return false;
    return true;
  });
  // Sections: bosses in raid order, then the rest.
  const order = [...(loot.table.bosses || [])];
  const groups = new Map(order.map((b) => [b, []]));
  const sectionOf = (it) => (it.trash ? "Trash & other drops" : it.boss || (it.custom ? "Added by your raid leaders" : "Other items"));
  for (const it of items) {
    const g = sectionOf(it);
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(it);
  }
  const sections = [...groups].filter(([, list]) => list.length);
  loot.sections = sections.map(([k]) => k);
  const small = items.length <= 24 || sections.length <= 1;
  const rows = sections.map(([boss, list]) => {
    const reserves = list.reduce((n, it) => n + reserversOf(ev, it.item_id).length, 0);
    const mineHere = list.some((it) => ev.reserves.some((r) => r.user_id === state.me.id && r.item_id === it.item_id));
    const auto = filtering || small || reserves > 0;
    const open = loot.open.has(boss) && !filtering ? loot.open.get(boss) : auto;
    const d = el("details", { class: `loot-boss${mineHere ? " has-mine" : ""}`, "data-boss": boss });
    d.open = open;
    d.addEventListener("toggle", () => { if (!filtering) loot.open.set(boss, d.open); });
    d.append(el("summary", {},
      el("span", { class: "loot-boss-name" }, boss),
      el("span", { class: "loot-boss-meta muted small" }, `${plural(list.length, "item")}${reserves ? ` · ${reserves} SR` : ""}`)),
    el("div", { class: "loot-boss-items" }, list.map((it) => lootItemRow(ev, it, manager, branch))));
    return d;
  });
  const f = loot.found;
  const link = wowheadListLink(raw);
  const importBox = link ? el("div", { class: "loot-group found" },
    el("div", { class: "loot-group-head" }, `Wowhead ${KIND_LABEL[link.kind]} page`),
    manager && loot.table.instance
      ? el("button", { type: "button", class: "primary small-btn loot-import-btn", onclick: () => importSource(raw) }, `＋ Import this ${KIND_LABEL[link.kind]}'s loot into the ${loot.table.instance_name} list`)
      : el("p", { class: "muted small", style: "margin:0" }, manager ? "Pick which raid this is (Edit raid → Loot list) first." : "That's a whole loot list. Raid leaders can import it; paste an item link to reserve one item.")) : null;
  const lookup = q && !link && !loot.finding && !f ? el("button", { type: "button", class: "loot-lookup", onclick: lookUpItem },
    `🔎 Look up “${raw}” on Wowhead`, el("span", { class: "muted small" }, " (Enter)")) : null;
  const finding = loot.finding ? el("p", { class: "muted small" }, "Looking it up…") : null;
  const found = f && !f.none ? el("div", { class: "loot-group found" },
    el("div", { class: "loot-group-head" }, "Found"),
    lootItemRow(ev, { item_id: f.item_id, name: f.name, quality: f.quality, boss: "" }, manager, branch),
    manager && loot.table.instance && !loot.table.items.some((i) => i.item_id === f.item_id)
      ? el("button", { type: "button", class: "ghost small-btn", onclick: () => addListItem(String(f.item_id)) }, `＋ Add to the ${loot.table.instance_name} list for everyone`) : null)
    : f?.none ? el("p", { class: "muted small" }, `Nothing found for “${f.none}”. Try its Wowhead link or item ID.`) : null;
  const empty = !all.length && !q
    ? el("p", { class: "muted small" }, loot.table.instance
      ? "This raid's list is empty. Search for an item by name, or paste a Wowhead link."
      : `No raid picked for this one${manager ? " (Edit raid → Loot list)" : ""}. Search for any item by name, or paste a Wowhead link.`) : null;
  const hidden = all.length && !items.length && !q ? el("p", { class: "muted small" }, "Nothing matches those filters.") : null;
  box.replaceChildren(importBox, found, finding, lookup, ...rows, empty, hidden,
    q && !link && !items.length && !f && !loot.finding ? el("p", { class: "muted small" }, "Not on this raid's list.") : null);
}

async function addListItem(query) {
  const table = await tryApi("POST", `/api/events/${loot.ev.id}/loot-items`, { query }).catch(() => null);
  if (!table) return;
  setTable(table);
  loot.found = null;
  toast("Added to the list.");
  drawLoot();
}

async function removeListItem(it) {
  if (!(await confirmBox("Remove from the list?", `${it.name} comes off the ${loot.table.instance_name} list for every raid in this outhouse. Reserves already made stay.`, "Remove"))) return;
  const table = await tryApi("DELETE", `/api/events/${loot.ev.id}/loot-items/${it.item_id}`).catch(() => null);
  if (!table) return;
  setTable(table);
  drawLoot();
}

async function importSource(url) {
  if (loot.importing) return;
  loot.importing = true;
  toast("Reading that page from Wowhead…");
  const table = await tryApi("POST", `/api/events/${loot.ev.id}/loot-sources`, { url }).catch(() => null);
  loot.importing = false;
  if (!table || !loot.dialog) return;
  setTable(table);
  loot.search.value = "";
  const src = table.sources[table.sources.length - 1];
  toast(src ? `Imported ${src.name} (${plural(src.items, "item")}).` : "Imported.");
  drawLoot();
}

async function removeSource(src) {
  if (!(await confirmBox("Remove this page?", `${src.name}'s loot comes off the ${loot.table.instance_name} list for every raid in this outhouse. Reserves already made stay.`, "Remove"))) return;
  const params = new URLSearchParams({ branch: src.branch, kind: src.kind, id: String(src.id) });
  const table = await tryApi("DELETE", `/api/events/${loot.ev.id}/loot-sources?${params}`).catch(() => null);
  if (!table) return;
  setTable(table);
  drawLoot();
}

async function refreshFromWowhead(btn) {
  btn.disabled = true;
  btn.textContent = "Reading Wowhead…";
  const table = await tryApi("POST", `/api/events/${loot.ev.id}/loot-table/refresh`).catch(() => null);
  if (!loot.dialog) return;
  if (table) { setTable(table); toast(table.refreshing ? "Still reading Wowhead; the list fills in shortly." : "Loot list refreshed from Wowhead."); }
  drawLoot();
}

const SOURCE_STATUS = { ok: "", pending: "Reading…", missing: "Not on Wowhead", error: "Wowhead didn't answer (showing the saved list)" };

// Raid leaders: where the list comes from, refresh, import another page.
function drawListTab(ev) {
  const t = loot.table;
  const url = el("input", { type: "url", placeholder: "Paste a Wowhead zone, boss or chest link", maxlength: 400, "aria-label": "Wowhead link to import" });
  const go = el("button", { type: "submit", class: "primary" }, "Import");
  const form = el("form", { class: "loot-import" }, url, go);
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!wowheadListLink(url.value)) return toast("Paste a Wowhead link to a zone, NPC (boss) or object (chest), e.g. wowhead.com/classic/zone=2717.");
    go.disabled = true;
    await importSource(url.value.trim());
    go.disabled = false;
  });
  const day = (iso) => new Date(iso).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
  const sources = t.sources?.length ? el("div", { class: "loot-sources" }, t.sources.map((s) => el("div", { class: `loot-source ${s.status}`, "data-source": `${s.kind}-${s.id}` },
    el("div", { class: "loot-source-main" },
      el("a", { href: s.url, target: "_blank", rel: "noopener noreferrer", class: "loot-source-name" }, s.name || `${s.kind} ${s.id}`),
      el("span", { class: "muted small" }, [s.builtin ? "This raid's zone" : `Imported ${KIND_LABEL[s.kind] || s.kind}`, `Wowhead ${s.branch || "retail"}`, plural(s.items, "item"),
        s.fetched_at ? `read ${day(s.fetched_at)}` : null, SOURCE_STATUS[s.status] || null].filter(Boolean).join(" · "))),
    s.builtin ? null : el("button", { type: "button", class: "sr-x", title: "Remove this page from the list", "aria-label": `Remove ${s.name}`, onclick: () => removeSource(s) }, "✕"))))
    : el("p", { class: "muted small" }, t.instance ? "This raid isn't one we know on Wowhead yet. Import its zone or bosses below." : "Pick which raid this is (Edit raid → Loot list) to get its loot from Wowhead.");
  const custom = t.items.filter((i) => i.custom);
  return el("div", { class: "loot-listtab" },
    lootBanner(),
    el("div", { class: "loot-list-head" },
      el("div", {}, el("strong", {}, t.instance_name || "No raid picked"), el("div", { class: "muted small" }, `${plural(t.items.length, "item")} · ${plural((t.bosses || []).length, "boss", "bosses")} · read from Wowhead and refreshed weekly`)),
      t.instance ? el("button", { type: "button", class: "ev-btn", onclick: (e) => refreshFromWowhead(e.currentTarget) }, "↻ Refresh from Wowhead") : null),
    sources,
    t.instance ? el("div", { class: "loot-import-box" },
      el("div", { class: "loot-mine-head" }, "Import more loot"),
      el("p", { class: "muted small", style: "margin:0" }, "New WoW: Forever raid, a world boss or a chest? Paste its Wowhead link and its drops join this list for every raid in the outhouse."),
      form) : null,
    custom.length ? el("div", { class: "loot-import-box" }, el("div", { class: "loot-mine-head" }, `Added by hand (${custom.length})`),
      el("div", { class: "loot-mine-list" }, custom.map((it) => el("span", { class: "sr-chip" }, itemName(it.item_id, it.name, it.quality, evBranch(ev)),
        el("button", { type: "button", class: "sr-x", title: "Remove from this raid's list", "aria-label": `Remove ${it.name} from the list`, onclick: () => removeListItem(it) }, "✕"))))) : null);
}

function personChip(ev, userId, { onRemove = null } = {}) {
  const s = signupOf(ev, userId);
  const colour = s ? classColour(s.class) : null;
  return el("span", { class: "sr-person", style: colour ? `--cc:${colour}` : "" },
    el("span", { class: "sr-person-name" }, reserverName(ev, userId)),
    onRemove ? el("button", { type: "button", class: "sr-x", title: "Remove this reserve", "aria-label": `Remove ${reserverName(ev, userId)}'s reserve`, onclick: onRemove }, "✕") : null);
}

function wonBy(itemId) {
  const won = loot.awards.filter((a) => a.item_id === itemId);
  return won.length ? el("span", { class: "sr-won" }, `💰 ${won.map((a) => a.character_name || a.display_name).join(", ")}`) : null;
}

function awardMenu(ev, it, anchor) {
  const r = anchor.getBoundingClientRect();
  const who = reserversOf(ev, it.item_id);
  openMenu(r.left, r.bottom + 4, [
    ...who.map((w) => ({ label: `${reserverName(ev, w.user_id)} (reserved)`, onClick: () => giveLoot(w.user_id, it.item_id) })),
    who.length ? "sep" : null,
    { label: "Someone else…", onClick: () => { loot.tab = "won"; loot.prefill = { item: String(it.item_id), name: it.item_name ?? it.name }; drawLoot(); } },
  ]);
}

async function giveLoot(userId, itemId, query = null, note = "") {
  const body = { user_id: userId, note };
  if (itemId) body.item_id = itemId; else body.query = query;
  const a = await tryApi("POST", `/api/events/${loot.ev.id}/loot`, body).catch(() => null);
  if (!a) return false;
  loot.awards = [...loot.awards, a];
  toast(`${a.item_name} → ${a.character_name || a.display_name}. In the loot log.`);
  drawLoot();
  return true;
}

function drawAllTab(ev, manager) {
  const started = eventWhen(ev).started;
  const views = el("div", { class: "seg" }, [["item", "By item"], ["player", "By player"]].map(([k, label]) => el("button", {
    type: "button", class: `seg-btn${loot.view === k ? " on" : ""}`, onclick: () => { loot.view = k; drawLoot(); },
  }, label)));
  const tools = manager ? el("div", { class: "loot-tools" },
    el("button", { type: "button", class: `ev-btn${ev.reserves_locked ? " on" : ""}`, onclick: () => setLock(!ev.reserves_locked) }, ev.reserves_locked ? "🔓 Unlock" : "🔒 Lock reserves"),
    ev.reserve_lock_override != null ? el("button", { type: "button", class: "ev-btn ghost", title: "Lock and unlock on the raid's own schedule again", onclick: () => setLock(null) }, "Auto lock") : null,
    el("button", { type: "button", class: "ev-btn", onclick: () => exportReserves("csv") }, "⬇ CSV"),
    el("button", { type: "button", class: "ev-btn", onclick: () => exportReserves("text") }, "📋 Copy list")) : null;
  let content;
  if (!ev.reserves.length) {
    content = el("p", { class: "muted" }, "Nobody has reserved anything yet.");
  } else if (loot.view === "item") {
    const byItem = new Map();
    for (const r of ev.reserves) {
      if (!byItem.has(r.item_id)) byItem.set(r.item_id, { item_id: r.item_id, item_name: r.item_name, quality: r.quality, who: [] });
      byItem.get(r.item_id).who.push(r);
    }
    const rows = [...byItem.values()].sort((a, b) => b.who.length - a.who.length || a.item_name.localeCompare(b.item_name));
    content = el("div", { class: "sr-table" }, rows.map((it) => el("div", { class: `sr-trow${it.who.length > 1 ? " contested" : ""}`, "data-item": it.item_id },
      el("div", { class: "sr-tcell item" }, itemName(it.item_id, it.item_name, it.quality, evBranch(ev)), srBadge(ev, it.item_id)),
      el("div", { class: "sr-tcell who" }, it.who.map((w) => personChip(ev, w.user_id, { onRemove: manager ? () => removeReserve(ev, w) : null })), wonBy(it.item_id)),
      manager && started ? el("button", { type: "button", class: "ev-btn small-btn", onclick: (e) => awardMenu(ev, it, e.currentTarget) }, "Won by…") : null)));
  } else {
    const people = new Map();
    for (const r of ev.reserves) {
      if (!people.has(r.user_id)) people.set(r.user_id, []);
      people.get(r.user_id).push(r);
    }
    const rows = [...people].sort((a, b) => reserverName(ev, a[0]).localeCompare(reserverName(ev, b[0])));
    content = el("div", { class: "sr-table" }, rows.map(([userId, list]) => el("div", { class: "sr-trow", "data-user": userId },
      el("div", { class: "sr-tcell player" }, personChip(ev, userId)),
      el("div", { class: "sr-tcell items" }, list.map((r) => el("span", { class: "sr-chip" }, itemName(r.item_id, r.item_name, r.quality, evBranch(ev)), srBadge(ev, r.item_id),
        manager ? el("button", { type: "button", class: "sr-x", title: "Remove this reserve", "aria-label": `Remove ${r.item_name}`, onclick: () => removeReserve(ev, r) }, "✕") : null))))));
  }
  const missing = ev.signups.filter((s) => s.status !== "no" && !ev.reserves.some((r) => r.user_id === s.user_id));
  return el("div", { class: "loot-all" },
    el("div", { class: "loot-all-bar" }, views, tools),
    content,
    missing.length ? el("p", { class: "muted small loot-noreserve" }, el("strong", {}, `No reserve yet (${missing.length}): `), missing.map(signupLabel).join(", ")) : null);
}

async function removeReserve(ev, r) {
  if (!(await confirmBox("Remove reserve?", `${reserverName(ev, r.user_id)}'s reserve on ${r.item_name} will be removed.`, "Remove"))) return;
  await unreserveItem(r.item_id, r.user_id);
}

async function setLock(locked) {
  const reply = await tryApi("PUT", `/api/events/${loot.ev.id}/reserves/lock`, { locked }).catch(() => null);
  if (!reply) return;
  applyLootReply(reply);
  toast(locked === true ? "Reserves locked." : locked === false ? "Reserves unlocked." : "Reserves lock on schedule again.");
}

async function exportReserves(kind) {
  const ev = loot.ev;
  const out = await tryApi("GET", `/api/events/${ev.id}/reserves/export`).catch(() => null);
  if (!out) return;
  if (kind === "text") return copy(out.text, "Reserves copied: Item — Player, Player");
  const blob = new Blob([out.csv], { type: "text/csv;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const safe = ev.title.replace(/[^\w\- ]+/g, "").trim().replace(/\s+/g, "-") || "raid";
  const a = el("a", { href: url, download: `${safe}-soft-reserves.csv` });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 5000);
  toast("Soft reserves saved as CSV.");
}

function drawWonTab(ev, manager) {
  const started = eventWhen(ev).started;
  let form = null;
  if (manager && started) {
    const pre = loot.prefill;
    loot.prefill = null;
    const item = el("input", { placeholder: "Item: name, Wowhead link or ID", maxlength: 200, value: pre?.name ?? "", "aria-label": "Item" });
    if (pre) item.dataset.itemId = pre.item;
    item.addEventListener("input", () => { delete item.dataset.itemId; });
    const people = ev.signups.filter((s) => s.status !== "no");
    const others = state.members.filter((m) => !m.bot && !people.some((s) => s.user_id === m.id));
    const who = el("select", { class: "select", "aria-label": "Who won it" },
      el("option", { value: "" }, "Who won it?"),
      people.length ? el("optgroup", { label: "Signed up" }, people.map((s) => el("option", { value: s.user_id }, signupLabel(s)))) : null,
      others.length ? el("optgroup", { label: "Others in the outhouse" }, others.map((m) => el("option", { value: m.id }, m.display_name))) : null);
    const note = el("input", { placeholder: "Note (optional): off-spec, rolled 98…", maxlength: 100, "aria-label": "Note" });
    const go = el("button", { type: "submit", class: "primary" }, "Log it");
    form = el("form", { class: "loot-give" },
      el("div", { class: "loot-give-row" }, item, who), el("div", { class: "loot-give-row" }, note, go));
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      if (!who.value) return toast("Pick who won it.");
      if (!item.value.trim()) return toast("Which item?");
      go.disabled = true;
      const ok = await giveLoot(who.value, item.dataset.itemId ? Number(item.dataset.itemId) : null, item.value.trim(), note.value.trim());
      go.disabled = false;
      if (ok) { item.value = ""; note.value = ""; delete item.dataset.itemId; }
    });
    if (pre) setTimeout(() => who.focus(), 0);
  }
  const list = loot.awards.length ? el("div", { class: "won-list" }, [...loot.awards].reverse().map((a) => el("div", { class: "won-row", "data-award": a.id },
    itemName(a.item_id, a.item_name, a.quality, a.branch || null),
    el("span", { class: "won-arrow muted" }, "→"),
    el("span", { class: "won-who", style: classColour(signupOf(ev, a.user_id)?.class) ? `color:${classColour(signupOf(ev, a.user_id).class)}` : "" }, a.character_name || a.display_name),
    a.reserved ? el("span", { class: "sr-count", title: "They had reserved it" }, "SR") : null,
    a.note ? el("span", { class: "muted small won-note" }, a.note) : null,
    el("span", { class: "muted small won-time" }, new Date(a.awarded_at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })),
    manager ? el("button", { type: "button", class: "sr-x", title: "Take out of the loot log", "aria-label": `Remove ${a.item_name} from the loot log`, onclick: () => removeAward(a) }, "✕") : null)))
    : el("p", { class: "muted" }, started ? "Nothing handed out yet." : "Once the raid starts, its leaders log who won what here.");
  return el("div", { class: "loot-won" }, form, list,
    el("button", { type: "button", class: "link", onclick: () => openLootLog() }, "📜 The outhouse's loot log"));
}

async function removeAward(a) {
  if (!(await confirmBox("Remove from the loot log?", `${a.item_name} → ${a.character_name || a.display_name}`, "Remove"))) return;
  const ok = await tryApi("DELETE", `/api/loot/${a.id}`).then(() => true, () => false);
  if (!ok) return;
  loot.awards = loot.awards.filter((x) => x.id !== a.id);
  drawLoot();
}

// ---------- Loot log (the whole outhouse) ----------

async function openLootLog(filters = {}) {
  if (!state.guildId) return;
  const guild = state.guildId;
  const dialog = raidWindow("loot-log-dialog");
  let entries = [];
  let more = false;
  const player = el("select", { class: "select", "aria-label": "Player" },
    el("option", { value: "" }, "Everyone"),
    [...state.members].filter((m) => !m.bot).sort((a, b) => a.display_name.localeCompare(b.display_name))
      .map((m) => el("option", { value: m.id, selected: m.id === filters.user }, m.display_name)));
  const raid = el("select", { class: "select", "aria-label": "Raid" }, el("option", { value: "" }, "Any raid"));
  const search = el("input", { type: "search", placeholder: "Search items, raids or people", "aria-label": "Search the loot log", maxlength: 100 });
  const body = el("div", { class: "loot-log-list" }, el("p", { class: "muted" }, "Loading…"));
  const moreBtn = el("button", { type: "button", class: "ghost", hidden: true, onclick: () => load(true) }, "Load more");
  const manageAny = can("manage_events") || (typeof hasRaidManagerRole === "function" && hasRaidManagerRole(guild));
  const draw = () => {
    if (!entries.length) {
      body.replaceChildren(el("div", { class: "att-empty" }, el("p", {}, "No loot logged yet."),
        el("p", { class: "muted small" }, "Raid leaders log who won what from a raid's card (💰 Loot won) once it starts.")));
    } else {
      const day = (iso) => new Date(iso).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
      body.replaceChildren(el("table", { class: "loot-log-table" },
        el("thead", {}, el("tr", {}, el("th", {}, "When"), el("th", {}, "Item"), el("th", {}, "Who"), el("th", {}, "Raid"), manageAny ? el("th", {}, "") : null)),
        el("tbody", {}, entries.map((a) => el("tr", { "data-award": a.id },
          el("td", { class: "muted small" }, day(a.awarded_at)),
          el("td", { class: "ll-item" }, itemName(a.item_id, a.item_name, a.quality, a.branch || null), a.reserved ? el("span", { class: "sr-count", title: "They had reserved it" }, "SR") : null),
          el("td", {}, el("button", { type: "button", class: "att-member", onclick: (e) => openProfile(a.user_id, e.currentTarget) },
            avatarFor(a.user_id, a.display_name, 20), a.character_name || a.display_name),
          a.character_name && a.character_name !== a.display_name ? el("span", { class: "muted small" }, ` ${a.display_name}`) : null),
          el("td", {}, el("div", {}, a.raid_title), el("div", { class: "muted small" }, [a.instance_name, new Date(a.raid_starts_at).toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" })].filter(Boolean).join(" · "))),
          manageAny ? el("td", {}, el("button", { type: "button", class: "sr-x", title: "Take out of the loot log", "aria-label": `Remove ${a.item_name}`, onclick: async () => {
            if (!(await confirmBox("Remove from the loot log?", `${a.item_name} → ${a.character_name || a.display_name}`, "Remove"))) return;
            if (await tryApi("DELETE", `/api/loot/${a.id}`).then(() => true, () => false)) { entries = entries.filter((x) => x.id !== a.id); draw(); }
          } }, "✕")) : null)))));
    }
    moreBtn.hidden = !more;
  };
  let seq = 0;
  const load = async (append = false) => {
    const n = ++seq;
    const params = new URLSearchParams({ limit: "50" });
    if (player.value) params.set("user", player.value);
    if (raid.value) params.set("instance", raid.value);
    if (search.value.trim()) params.set("q", search.value.trim());
    if (append && entries.length) params.set("before", entries[entries.length - 1].id);
    const data = await tryApi("GET", `/api/guilds/${guild}/loot?${params}`).catch(() => null);
    if (n !== seq || !dialog.isConnected) return;
    if (!data) { body.replaceChildren(el("p", { class: "muted" }, "Couldn't load the loot log.")); return; }
    entries = append ? [...entries, ...data.entries] : data.entries;
    more = data.has_more;
    draw();
  };
  let timer = null;
  search.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(() => load(), 250); });
  player.addEventListener("change", () => load());
  raid.addEventListener("change", () => load());
  dialog.append(
    el("div", { class: "att-head" },
      el("div", {}, el("h2", {}, "📜 Loot log"), el("div", { class: "muted small" }, currentGuild()?.name ?? "")),
      el("div", { class: "att-head-tools" }, closeX(dialog))),
    el("div", { class: "loot-log-filters" }, player, raid, search),
    el("div", { class: "att-table-wrap" }, body), moreBtn);
  api("GET", `/api/guilds/${guild}/loot-instances`).then((list) => {
    raid.append(...list.map((i) => el("option", { value: i.key }, i.name)));
  }).catch(() => {});
  load();
}

// ---------- Plan / Edit raid: the loot fields ----------

const lootInstanceCache = new Map();
function lootInstances(guildId) {
  if (!lootInstanceCache.has(guildId)) {
    lootInstanceCache.set(guildId, api("GET", `/api/guilds/${guildId}/loot-instances`).catch(() => { lootInstanceCache.delete(guildId); return []; }));
  }
  return lootInstanceCache.get(guildId);
}

// Short names people put in raid titles.
const INSTANCE_ALIASES = { mc: ["mc", "molten core"], ony: ["ony", "onyxia"], bwl: ["bwl", "blackwing"], zg: ["zg", "zul'gurub", "zulgurub"],
  aq20: ["aq20", "ruins of ahn"], aq40: ["aq40", "temple of ahn"], naxx: ["naxx", "naxxramas"], kara: ["kara", "karazhan"], gruul: ["gruul"],
  mag: ["mag", "magtheridon"], ssc: ["ssc", "serpentshrine"], tk: ["tk", "tempest keep", "the eye"], hyjal: ["hyjal"], bt: ["bt", "black temple"],
  za: ["za", "zul'aman", "zulaman"], swp: ["swp", "sunwell"] };
function guessInstance(title) {
  const t = ` ${title.toLowerCase().replace(/[^a-z0-9' ]+/g, " ")} `;
  for (const [key, names] of Object.entries(INSTANCE_ALIASES)) {
    if (names.some((n) => t.includes(` ${n} `) || (n.length > 4 && t.includes(n)))) return key;
  }
  return "";
}

// The "Loot" part of Plan/Edit raid: raid (loot list), soft reserves and
// when they lock. Returns { node, values(), changed() } for features.js.
function raidLootFields(ev, titleInput, gameSelect) {
  const OTHER = "__other";
  const instance = el("select", { class: "select", "aria-label": "Loot list" }, el("option", { value: "" }, "None"));
  const other = el("input", { maxlength: 60, placeholder: "Which raid? e.g. a new WoW: Forever raid", "aria-label": "Raid name", hidden: true });
  const srs = el("select", { class: "select", "aria-label": "Soft reserves" },
    [[0, "Off"], [1, "1 per person"], [2, "2 per person"]].map(([n, label]) => el("option", { value: n, selected: (ev?.soft_reserves ?? 0) === n }, label)));
  const lock = el("select", { class: "select", "aria-label": "Reserves lock" },
    RESERVE_LOCKS.map(([v, label]) => el("option", { value: v, selected: (ev?.reserve_lock || "deadline") === v }, label)));
  const lockField = el("label", { class: "field" }, "Reserves lock", lock);
  const hint = el("p", { class: "muted small loot-hint", style: "margin:0" });
  let touched = !!ev;
  const current = ev?.loot_instance ?? "";
  const paint = () => {
    other.hidden = instance.value !== OTHER;
    lockField.hidden = srs.value === "0";
    hint.textContent = srs.value === "0"
      ? "Pick the raid so its leaders can log who won what. Turn on soft reserves to let people call dibs."
      : "People signed up pick from the raid's list (or any item by Wowhead link). Raid leaders can lock, edit and export them.";
  };
  let list = [];
  const currentValue = () => (instance.value === OTHER ? other.value.trim() : instance.value);
  const setValue = (v) => {
    if (!v) instance.value = "";
    else if (list.some((i) => i.key === v)) instance.value = v;
    else { instance.value = OTHER; other.value = v; }
    paint();
  };
  // Options: the listed raids (the game's era first) and the outhouse's own.
  const fill = () => {
    const game = gameSelect?.value ?? "";
    const groups = [["classic", "Classic & WoW: Forever"], ["tbc", "The Burning Crusade"], ["", "This outhouse's own"]];
    const ordered = game === "tbc" ? [groups[1], groups[0], groups[2]] : groups;
    instance.replaceChildren(el("option", { value: "" }, "None"),
      ...ordered.map(([era, label]) => {
        const items = list.filter((i) => i.era === era);
        return items.length ? el("optgroup", { label }, items.map((i) => el("option", { value: i.key }, i.name))) : null;
      }),
      el("option", { value: OTHER }, "Other…"));
  };
  fill();
  setValue(current);
  lootInstances(state.guildId).then((l) => {
    const v = currentValue();
    list = l;
    fill();
    setValue(v || (!touched ? guessInstance(titleInput.value) : ""));
  });
  instance.addEventListener("change", () => { touched = true; paint(); if (instance.value === OTHER) other.focus(); });
  srs.addEventListener("change", paint);
  titleInput.addEventListener("input", () => {
    if (touched) return;
    const g = guessInstance(titleInput.value);
    if (g && list.length) setValue(g);
  });
  gameSelect?.addEventListener("change", () => { const v = currentValue(); fill(); setValue(v); });
  const values = () => ({
    soft_reserves: Number(srs.value),
    reserve_lock: lock.value,
    loot_instance: instance.value === OTHER ? other.value.trim() : instance.value,
  });
  paint();
  const node = el("div", { class: "loot-fields" },
    el("div", { class: "raid-row even" },
      el("label", { class: "field" }, "🎒 Loot list", instance),
      el("label", { class: "field" }, "🎲 Soft reserves", srs)),
    other, lockField, hint);
  return {
    node,
    values,
    // Only what's different from the raid as it is (everything for new raids).
    changes() {
      const v = values();
      if (!ev) return v.soft_reserves || v.loot_instance ? v : {};
      const out = {};
      if (v.soft_reserves !== (ev.soft_reserves ?? 0)) out.soft_reserves = v.soft_reserves;
      if (v.reserve_lock !== (ev.reserve_lock || "deadline") && v.soft_reserves) out.reserve_lock = v.reserve_lock;
      if (v.loot_instance !== (ev.loot_instance ?? "")) out.loot_instance = v.loot_instance;
      return out;
    },
  };
}

// ---------- Live updates, profile cards ----------

const lootBaseHandle = handleEvent;
handleEvent = function (ev) {
  if (ev.t === "EVENT_UPDATE") lootEventChanged(ev.d);
  if (ev.t === "EVENT_DELETE" && loot.dialog && loot.ev?.id === ev.d.id) loot.dialog.close();
  lootBaseHandle(ev);
};

const lootBaseOpenProfile = openProfile;
openProfile = async function (userId, anchor) {
  await lootBaseOpenProfile(userId, anchor);
  const guild = state.guildId;
  const pop = popover;
  if (!guild || state.home || !pop?.classList.contains("profile-pop")) return;
  const data = await api("GET", `/api/guilds/${guild}/loot?user=${userId}&limit=5`).catch(() => null);
  if (!data?.entries?.length || popover !== pop || !pop.isConnected) return;
  const day = (iso) => new Date(iso).toLocaleDateString([], { day: "numeric", month: "short" });
  const section = el("div", { class: "pc-section pc-loot" },
    el("h4", {}, "RECENT LOOT"),
    el("div", { class: "pcl-list" }, data.entries.map((a) => el("div", { class: "pcl-row" },
      itemName(a.item_id, a.item_name, a.quality, a.branch || null),
      el("span", { class: "muted small" }, `${a.instance_name || a.raid_title} · ${day(a.awarded_at)}`)))),
    el("button", { type: "button", class: "link small", onclick: () => { closePopover(); openLootLog({ user: userId }); } }, "Full loot log"));
  const body = pop.querySelector(".pc-body");
  body?.insertBefore(section, body.querySelector(".pc-actions"));
  const r = pop.getBoundingClientRect();
  if (r.bottom > window.innerHeight - 8) pop.style.top = `${Math.max(8, window.innerHeight - r.height - 8)}px`;
};

// Tooltips don't outlive what they point at (or a tap somewhere else).
document.addEventListener("scroll", followItemTip, true);
document.addEventListener("pointerdown", (e) => { if (!e.target.closest?.(".item-name, .item-tip")) hideItemTip(); }, true);
