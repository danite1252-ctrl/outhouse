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
    // Raid leaders already have this line at the top of the card.
    c.missing.length && (extra || !canManageEvent(ev) || eventWhen(ev).over)
      ? el("div", { class: "comp-missing" }, el("strong", {}, "Missing: "), c.missing.join(" · ")) : null,
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

// ---------- Items: names, tooltips, lookups ----------

const itemInfo = new Map();
function itemData(id) {
  if (!itemInfo.has(id)) itemInfo.set(id, api("GET", `/api/wow/items/${id}`).catch(() => null));
  return itemInfo.get(id);
}

let itemTip = null;
function hideItemTip() { itemTip?.remove(); itemTip = null; }

function placeTip(tip, anchor) {
  const r = anchor.getBoundingClientRect();
  const t = tip.getBoundingClientRect();
  let y = r.bottom + 6;
  if (y + t.height > window.innerHeight - 8) y = Math.max(8, r.top - t.height - 6);
  tip.style.left = `${Math.max(8, Math.min(r.left, window.innerWidth - t.width - 8))}px`;
  tip.style.top = `${y}px`;
}

function showItemTip(anchor, id, name, quality) {
  hideItemTip();
  const tip = el("div", { class: "item-tip", role: "tooltip" },
    el("div", { class: "item-tip-head" }, el("strong", { style: `color:${qualityColour(quality)}` }, name)),
    el("div", { class: "item-tip-body muted small" }, "Loading…"));
  itemTip = tip;
  ((typeof raidHost === "function" && raidHost()) || document.body).append(tip);
  placeTip(tip, anchor);
  itemData(id).then((info) => {
    if (itemTip !== tip) return;
    const lines = (info?.tooltip || "").split("\n").filter(Boolean);
    tip.replaceChildren(
      el("div", { class: "item-tip-head" },
        info?.icon_url ? hideBroken(el("img", { src: info.icon_url, alt: "", width: 36, height: 36 })) : null,
        el("strong", { style: `color:${qualityColour(info?.quality ?? quality)}` }, info?.name || name)),
      lines.length ? el("div", { class: "item-tip-body" }, lines.map((l) => el("div", {}, l))) : el("div", { class: "item-tip-body muted small" }, `Item ${id}`));
    placeTip(tip, anchor);
  });
}

function wowheadItemUrl(id) {
  const base = (state.wowInfo?.wowhead_url || "https://www.wowhead.com/classic").replace(/\/+$/, "");
  return `${base}/item=${id}`;
}

function itemName(id, name, quality) {
  const a = el("a", { class: "item-name", href: wowheadItemUrl(id), target: "_blank", rel: "noopener noreferrer", style: `color:${qualityColour(quality)}` }, name);
  a.addEventListener("mouseenter", () => showItemTip(a, id, name, quality));
  a.addEventListener("focus", () => showItemTip(a, id, name, quality));
  a.addEventListener("mouseleave", hideItemTip);
  a.addEventListener("blur", hideItemTip);
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
      mine.map((r) => el("span", { class: "sr-chip" }, itemName(r.item_id, r.item_name, r.quality), srBadge(ev, r.item_id)))) : null,
    el("div", { class: "ev-actions" },
      sr ? el("button", { class: `ev-btn sr${signed && !mine.length && !ev.reserves_locked ? " want" : ""}`, onclick: () => openLoot(ev, signed ? "reserve" : "all") },
        signed ? (mine.length ? (ev.reserves_locked ? "My reserves" : "Change reserves") : "🎲 Reserve loot") : "See reserves") : null,
      sr && (manager || ev.reserves.length) ? el("button", { class: "ev-btn ghost", onclick: () => openLoot(ev, "all") }, manager ? "Reserves table" : "Everyone's reserves") : null,
      manager && when.started ? el("button", { class: "ev-btn loot", onclick: () => openLoot(ev, "won") }, "💰 Loot won") : null));
}

// ---------- The loot window ----------

const loot = { dialog: null, root: null, ev: null, tab: "reserve", view: "item", table: null, awards: [], search: null, found: null, finding: false };

async function openLoot(ev, tab = "reserve") {
  loot.dialog?.close();
  const dialog = raidWindow("loot-dialog", () => { if (loot.dialog === dialog) { loot.dialog = null; hideItemTip(); } });
  Object.assign(loot, { dialog, ev, tab, table: null, awards: [], found: null, finding: false });
  loot.root = el("div", { class: "loot-root" });
  loot.search = el("input", { type: "search", class: "loot-search", placeholder: "Search the list, or paste a Wowhead link or item ID", maxlength: 200, "aria-label": "Find an item" });
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
  loot.table = table ?? { instance: "", instance_name: "", items: [] };
  loot.awards = awards;
  drawLoot();
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

async function lookUpItem() {
  const q = loot.search.value.trim();
  if (!q || loot.finding) return;
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
  const tabs = [sr ? ["reserve", "Reserve"] : null, sr ? ["all", `Everyone's (${ev.reserves.length})`] : null, ["won", `Loot won (${loot.awards.length})`]].filter(Boolean);
  if (!tabs.some(([k]) => k === loot.tab)) loot.tab = tabs[0][0];
  const focused = document.activeElement === loot.search;
  const sel = focused ? [loot.search.selectionStart, loot.search.selectionEnd] : null;
  const sub = [loot.table?.instance_name || ev.loot_instance_name || (loot.table ? "No raid picked" : ""), sr ? `${ev.soft_reserves} reserve${ev.soft_reserves === 1 ? "" : "s"} each` : "Soft reserves are off", sr ? lockText(ev) : null].filter(Boolean);
  const head = el("div", { class: "loot-head" },
    el("div", { class: "loot-title" }, el("h2", {}, sr ? "🎲 Soft reserves" : "💰 Loot"),
      el("div", { class: "muted small" }, el("strong", {}, ev.title), ` · ${eventWhen(ev).text}`),
      el("div", { class: "loot-sub small" }, sub.join(" · "))),
    closeX(loot.dialog));
  const tabBar = el("div", { class: "loot-tabs", role: "tablist" }, tabs.map(([k, label]) => el("button", {
    type: "button", role: "tab", class: `loot-tab${loot.tab === k ? " on" : ""}`, "aria-selected": loot.tab === k ? "true" : "false",
    onclick: () => { loot.tab = k; drawLoot(); },
  }, label)));
  const body = !loot.table ? el("p", { class: "muted" }, "Loading…")
    : loot.tab === "reserve" ? drawReserveTab(ev, manager)
      : loot.tab === "all" ? drawAllTab(ev, manager)
        : drawWonTab(ev, manager);
  loot.root.replaceChildren(head, tabs.length > 1 ? tabBar : null, el("div", { class: "loot-body" }, body));
  if (focused && loot.search.isConnected) { loot.search.focus(); loot.search.setSelectionRange(...sel); }
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
  loot.listBox = el("div", { class: "loot-list" });
  drawLootList();
  return el("div", { class: "loot-reserve" },
    el("p", { class: `loot-status${!me || (ev.reserves_locked && !manager) ? " warn" : ""}` }, status),
    el("div", { class: "loot-mine" },
      el("div", { class: "loot-mine-head" }, `Your reserves (${mine.length}/${ev.soft_reserves})`),
      mine.length ? el("div", { class: "loot-mine-list" }, mine.map((r) => el("span", { class: "sr-chip big" },
        itemName(r.item_id, r.item_name, r.quality), srBadge(ev, r.item_id),
        canEdit ? el("button", { type: "button", class: "sr-x", title: `Take back ${r.item_name}`, "aria-label": `Take back ${r.item_name}`, onclick: () => unreserveItem(r.item_id) }, "✕") : null)))
        : el("span", { class: "muted small" }, "Nothing yet.")),
    loot.search, loot.listBox);
}

function lootItemRow(ev, it, manager) {
  const mine = ev.reserves.some((r) => r.user_id === state.me.id && r.item_id === it.item_id);
  const btn = mine ? el("button", { type: "button", class: "ev-btn on sr-pick", disabled: !loot.canEdit, onclick: () => unreserveItem(it.item_id), title: loot.canEdit ? "Take it back" : "" }, "Reserved ✓")
    : el("button", { type: "button", class: "ev-btn sr-pick", disabled: !loot.canEdit || loot.atLimit, title: loot.atLimit && loot.canEdit ? "Take back one of yours first" : "", onclick: () => reserveItem(it.item_id) }, "Reserve");
  const won = loot.awards.filter((a) => a.item_id === it.item_id);
  return el("div", { class: `loot-row${mine ? " mine" : ""}`, "data-item": it.item_id },
    el("div", { class: "loot-row-main" }, itemName(it.item_id, it.name, it.quality),
      el("span", { class: "loot-row-meta muted small" }, [it.boss, won.length ? `won by ${won.map((a) => a.character_name || a.display_name).join(", ")}` : null].filter(Boolean).join(" · "))),
    srBadge(ev, it.item_id),
    manager && it.custom ? el("button", { type: "button", class: "icon-btn small", title: "Remove from this raid's list", "aria-label": `Remove ${it.name} from the list`, onclick: () => removeListItem(it) }, "🗑") : null,
    loot.tab === "reserve" ? btn : null);
}

function drawLootList() {
  const box = loot.listBox;
  if (!box || !loot.table) return;
  const ev = loot.ev;
  const manager = canManageEvent(ev);
  const q = loot.search.value.trim().toLowerCase();
  const items = loot.table.items.filter((it) => !q || it.name.toLowerCase().includes(q) || String(it.item_id) === q);
  const groups = new Map();
  for (const it of items) {
    const g = it.boss || (it.custom ? "Added by your raid leaders" : "Other items");
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g).push(it);
  }
  const rows = [...groups].map(([boss, list]) => el("div", { class: "loot-group" },
    el("div", { class: "loot-group-head" }, boss),
    list.map((it) => lootItemRow(ev, { ...it, boss: "" }, manager))));
  const f = loot.found;
  const lookup = q && !loot.finding && !f ? el("button", { type: "button", class: "loot-lookup", onclick: lookUpItem },
    `🔎 Look up “${loot.search.value.trim()}” on Wowhead`, el("span", { class: "muted small" }, " (Enter)")) : null;
  const finding = loot.finding ? el("p", { class: "muted small" }, "Looking it up…") : null;
  const found = f && !f.none ? el("div", { class: "loot-group found" },
    el("div", { class: "loot-group-head" }, "Found"),
    lootItemRow(ev, { item_id: f.item_id, name: f.name, quality: f.quality, boss: "" }, manager),
    manager && loot.table.instance && !loot.table.items.some((i) => i.item_id === f.item_id)
      ? el("button", { type: "button", class: "ghost small-btn", onclick: () => addListItem(String(f.item_id)) }, `＋ Add to the ${loot.table.instance_name} list for everyone`) : null)
    : f?.none ? el("p", { class: "muted small" }, `Nothing found for “${f.none}”. Try its Wowhead link or item ID.`) : null;
  const empty = !items.length && !q
    ? el("p", { class: "muted small" }, loot.table.instance
      ? "This raid's list is empty. Search for an item by name, or paste a Wowhead link."
      : `No raid picked for this one${manager ? " (Edit raid → Loot list)" : ""}. Search for any item by name, or paste a Wowhead link.`) : null;
  box.replaceChildren(found, finding, lookup, ...rows, empty,
    q && !items.length && !f && !loot.finding ? el("p", { class: "muted small" }, "Not on this raid's list.") : null);
}

async function addListItem(query) {
  const table = await tryApi("POST", `/api/events/${loot.ev.id}/loot-items`, { query }).catch(() => null);
  if (!table) return;
  loot.table = table;
  loot.found = null;
  toast("Added to the list.");
  drawLoot();
}

async function removeListItem(it) {
  if (!(await confirmBox("Remove from the list?", `${it.name} comes off the ${loot.table.instance_name} list for every raid in this outhouse. Reserves already made stay.`, "Remove"))) return;
  const table = await tryApi("DELETE", `/api/events/${loot.ev.id}/loot-items/${it.item_id}`).catch(() => null);
  if (!table) return;
  loot.table = table;
  drawLoot();
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
      el("div", { class: "sr-tcell item" }, itemName(it.item_id, it.item_name, it.quality), srBadge(ev, it.item_id)),
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
      el("div", { class: "sr-tcell items" }, list.map((r) => el("span", { class: "sr-chip" }, itemName(r.item_id, r.item_name, r.quality), srBadge(ev, r.item_id),
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
    itemName(a.item_id, a.item_name, a.quality),
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
          el("td", {}, itemName(a.item_id, a.item_name, a.quality), a.reserved ? el("span", { class: "sr-count", title: "They had reserved it" }, "SR") : null),
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
      itemName(a.item_id, a.item_name, a.quality),
      el("span", { class: "muted small" }, `${a.instance_name || a.raid_title} · ${day(a.awarded_at)}`)))),
    el("button", { type: "button", class: "link small", onclick: () => { closePopover(); openLootLog({ user: userId }); } }, "Full loot log"));
  const body = pop.querySelector(".pc-body");
  body?.insertBefore(section, body.querySelector(".pc-actions"));
  const r = pop.getBoundingClientRect();
  if (r.bottom > window.innerHeight - 8) pop.style.top = `${Math.max(8, window.innerHeight - r.height - 8)}px`;
};

// Tooltips don't outlive what they point at (or a tap somewhere else).
document.addEventListener("scroll", hideItemTip, true);
document.addEventListener("pointerdown", (e) => { if (!e.target.closest?.(".item-name")) hideItemTip(); }, true);
