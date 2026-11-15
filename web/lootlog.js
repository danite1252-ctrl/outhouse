// Outhouse: loot history. Importing Gargul's awarded-loot export (a raid
// card's ⋯ menu, or the outhouse's Loot page), the Loot section on raid
// cards, the outhouse's Loot page (filters, sorting, per character, CSV,
// recent imports with undo) and each character's loot on the full profile
// page. Loaded after loot.js and profiles.js; uses their helpers (itemName,
// itemData, raidWindow, closeX, classColour, profileSection, …).
"use strict";

const lootDay = (iso, opts = { weekday: "short", day: "numeric", month: "short" }) => {
  const [y, m, d] = String(iso).slice(0, 10).split("-").map(Number);
  return new Date(y, m - 1, d).toLocaleDateString([], opts);
};
// What character names are matched on (as the server does): no realm, accents or case.
const lootKey = (name) => String(name || "").split("-")[0].normalize("NFD").replace(/\p{M}/gu, "").normalize("NFC").trim().replace(/\s+/g, " ").toLowerCase();
const tbcItem = (id) => id >= 24000 && id < 40000;

// An item that may only be known by its ID: shows "Item 32240" (linked to
// Wowhead) and fills in the real name and colour once the tooltip arrives.
function lootItemName(id, name, quality, branch) {
  const known = name && name !== `Item ${id}`;
  const br = branch || (tbcItem(id) ? branchForGame("tbc") : null);
  const a = itemName(id, known ? name : `Item ${id}`, quality ?? 4, br);
  if (!known) {
    a.classList.add("unknown");
    itemData(id, br).then((info) => {
      if (!info?.name || !a.isConnected) return;
      a.querySelector(".item-label").textContent = info.name;
      a.className = `item-name q${info.quality}`;
    });
  }
  return a;
}

// Who won it: their character in its class colour (opens their profile), a
// name nobody here has (with a warning), or "Disenchanted".
function lootWho(a, { member = false } = {}) {
  if (a.disenchanted) return el("span", { class: "loot-de-who" }, "✨ Disenchanted");
  const name = a.character_name || a.display_name || "Someone";
  const colour = classColour(a.character_class);
  if (!a.user_id) {
    return el("span", { class: "loot-who unmatched", title: "Nobody in this outhouse has a character by that name" },
      el("span", { class: "loot-warn", "aria-label": "Not matched" }, "⚠"), name);
  }
  return el("button", { type: "button", class: "loot-who", style: colour ? `color:${colour}` : "", title: a.display_name ? `${a.display_name}'s profile` : "Profile",
    onclick: (e) => openProfile(a.user_id, e.currentTarget) },
  name, member && a.display_name && a.display_name !== name ? el("span", { class: "muted small loot-member" }, ` ${a.display_name}`) : null);
}

const osBadge = () => el("span", { class: "loot-os", title: "Off spec" }, "OS");
const srWonBadge = () => el("span", { class: "sr-count won", title: "They had soft-reserved it" }, "SR");

function isLootManager(guildId = state.guildId) {
  return can("manage_events") || (typeof hasRaidManagerRole === "function" && hasRaidManagerRole(guildId));
}

async function removeLootEntry(a) {
  const who = a.disenchanted ? "Disenchanted" : (a.character_name || a.display_name);
  if (!(await confirmBox("Remove from the loot history?", `${a.item_name} → ${who}${a.raid_title ? ` (${a.raid_title})` : ""}. This is noted in the audit log.`, "Remove"))) return false;
  return tryApi("DELETE", `/api/loot/${a.id}`).then(() => true, () => false);
}

// ======================================================================
// Importing Gargul's export
// ======================================================================

const GARGUL_HELP = "In game, open Gargul's awarded loot export (/gl export → Awarded loot), copy the CSV and paste it here. Rows already imported are skipped.";

// `ev`: a raid card's import (rows filtered to its night). Without: the Loot page's.
function openGargulImport(ev = null, onDone = null) {
  const guildId = ev?.guild_id ?? state.guildId;
  const dialog = raidWindow("gargul-dialog");
  const base = ev ? `/api/events/${ev.id}/loot` : `/api/guilds/${guildId}/loot`;
  const text = el("textarea", { class: "gi-text", rows: 7, spellcheck: "false", placeholder: "dateTime,character,itemID,offspec,id\n2026-11-08,Shamièè,32240,1,12811834116478231330\n…", "aria-label": "Gargul export" });
  const file = el("input", { type: "file", accept: ".csv,.txt,text/csv,text/plain", hidden: true });
  const body = el("div", { class: "gi-body" });
  const foot = el("div", { class: "gi-foot" });
  let preview = null;
  let rows = [];
  file.addEventListener("change", async () => {
    const f = file.files?.[0];
    if (!f) return;
    if (f.size > 1_000_000) return toast("That file's too big (1 MB at most).");
    text.value = await f.text();
    run();
  });
  const run = async () => {
    if (!text.value.trim()) return toast("Paste Gargul's export first.");
    go.disabled = true;
    go.textContent = "Reading…";
    const p = await tryApi("POST", `${base}/preview`, { text: text.value }).catch(() => null);
    go.disabled = false;
    go.textContent = "Preview";
    if (!p || !dialog.isConnected) return;
    preview = p;
    rows = p.rows.map((r) => ({ ...r, pick: !r.duplicate && r.in_window, raid: r.event_id ?? "" }));
    draw();
  };
  const go = el("button", { type: "button", class: "primary", "data-act": "preview", onclick: run }, "Preview");
  const paste = el("div", { class: "gi-paste" },
    el("p", { class: "muted small gi-help" }, GARGUL_HELP),
    text,
    el("div", { class: "gi-paste-actions" },
      el("button", { type: "button", class: "ghost", onclick: () => file.click() }, "Choose a .csv file…"), file, el("span", { class: "gi-gap" }), go));

  const raidLabel = (r) => `${lootDay(r.local_date)} · ${r.title}`;
  const draw = () => {
    if (!preview) return;
    const raids = preview.raids;
    const picked = rows.filter((r) => r.pick);
    const counts = new Map();
    for (const r of picked) counts.set(r.raid, (counts.get(r.raid) ?? 0) + 1);
    const summary = [
      `${rows.length} row${rows.length === 1 ? "" : "s"}`,
      ...[...counts].map(([id, n]) => `${n} → ${id ? (raids.find((x) => x.id === id)?.title ?? "a raid") : "no raid"}`),
      preview.duplicates ? `${preview.duplicates} already imported` : null,
      preview.errors.length ? `${preview.errors.length} couldn't be read` : null,
    ].filter(Boolean).join(" · ");
    const errors = preview.errors.length ? el("details", { class: "gi-errors" },
      el("summary", {}, `⚠ ${preview.errors.length} line${preview.errors.length === 1 ? "" : "s"} left out`),
      el("ul", {}, preview.errors.slice(0, 50).map((e) => el("li", { class: "small" }, `Line ${e.line}: ${e.message}`)))) : null;
    const rule = { lock: "Won soft reserves are marked ✓ Won and locked for later raids of the same instance.", remove: "Won soft reserves are marked won and taken off the raid.", keep: "Won soft reserves are marked ✓ Won." }[preview.sr_won] ?? "";
    const pickable = rows.filter((r) => !r.duplicate);
    const all = el("input", { type: "checkbox", "aria-label": "Tick every row", checked: pickable.length > 0 && pickable.every((r) => r.pick) });
    all.addEventListener("change", () => { for (const r of pickable) r.pick = all.checked; draw(); });
    const list = el("div", { class: "gi-rows", role: "table", "aria-label": "Rows to import" },
      el("div", { class: "gi-row gi-headrow", role: "row" },
        el("span", { class: "gi-c-pick" }, all), el("span", { class: "gi-c-date" }, "Date"), el("span", { class: "gi-c-who" }, "Character"),
        el("span", { class: "gi-c-item" }, "Item"), el("span", { class: "gi-c-raid" }, ev ? "" : "Raid")),
      rows.map((r) => {
        const box = el("input", { type: "checkbox", checked: r.pick, disabled: r.duplicate, "aria-label": `Import line ${r.line}` });
        box.addEventListener("change", () => { r.pick = box.checked; draw(); });
        const m = r.matched;
        const who = r.disenchanted ? el("span", { class: "loot-de-who" }, "✨ Disenchanted")
          : m ? el("span", { class: "loot-who matched", style: classColour(m.class) ? `color:${classColour(m.class)}` : "", title: `${m.display_name} (${m.via === "signup" ? "signed up to the raid" : "a character on their profile"})` },
            r.character, el("span", { class: "muted small loot-member" }, ` ${m.display_name}`))
            : el("span", { class: "loot-who unmatched", title: "Nobody in this outhouse has a character by that name. It's kept under this name." },
              el("span", { class: "loot-warn", "aria-label": "Not matched" }, "⚠"), r.character);
        let raidCell = null;
        if (!ev) {
          const near = raids.filter((x) => Math.abs((new Date(x.local_date) - new Date(r.date)) / 86400000) <= 1);
          const others = raids.filter((x) => !near.includes(x));
          const sel = el("select", { class: "select gi-raid", "aria-label": `Raid for line ${r.line}`, disabled: r.duplicate },
            near.map((x) => el("option", { value: x.id, selected: r.raid === x.id }, raidLabel(x))),
            others.length ? el("optgroup", { label: "Other days" }, others.map((x) => el("option", { value: x.id, selected: r.raid === x.id }, raidLabel(x)))) : null,
            el("option", { value: "", selected: !r.raid }, "Keep without a raid"));
          sel.addEventListener("change", () => { r.raid = sel.value; r.reason = ""; draw(); });
          raidCell = el("span", { class: "gi-c-raid" }, sel, r.reason && !r.raid ? el("span", { class: "gi-reason small" }, r.reason) : null);
        } else {
          raidCell = el("span", { class: "gi-c-raid" }, r.duplicate ? null : !r.in_window ? el("span", { class: "gi-tag out", title: "Not on this raid's night (or the day either side)" }, "not this raid") : null);
        }
        return el("div", {
          class: `gi-row${r.duplicate ? " dup" : ""}${!r.in_window ? " out" : ""}${r.disenchanted ? " de" : ""}${r.pick ? " picked" : ""}`,
          role: "row", "data-line": r.line, "data-gargul": r.gargul_id, "data-date": r.date,
        },
        el("span", { class: "gi-c-pick" }, box),
        el("span", { class: "gi-c-date" }, lootDay(r.date, { day: "numeric", month: "short" }), r.time ? el("span", { class: "muted small" }, ` ${r.time}`) : null),
        el("span", { class: "gi-c-who" }, who),
        el("span", { class: "gi-c-item" }, lootItemName(r.item_id, r.item_name, r.quality, ev ? branchForGame(ev.game) : null),
          r.offspec ? osBadge() : null,
          r.reserved ? el("span", { class: "sr-count won", title: "They soft-reserved it in this raid: it'll be marked won" }, "SR") : null,
          r.duplicate ? el("span", { class: "gi-tag dup" }, "already imported") : null),
        raidCell);
      }));
    const n = picked.length;
    const doImport = el("button", { type: "button", class: "primary", "data-act": "import", disabled: !n, onclick: async () => {
      doImport.disabled = true;
      const picks = picked.map((r) => ({ gargul_id: r.gargul_id, event_id: ev ? ev.id : (r.raid || null) }));
      const res = await tryApi("POST", `${base}/import`, { text: text.value, picks }).catch(() => null);
      doImport.disabled = false;
      if (!res || !dialog.isConnected) return;
      showDone(res);
    } }, n ? `Import ${n} item${n === 1 ? "" : "s"}` : "Import");
    body.replaceChildren(
      el("div", { class: "gi-summary" }, el("strong", {}, summary), rule ? el("span", { class: "muted small" }, ` ${rule}`) : null),
      errors, list);
    foot.replaceChildren(
      el("button", { type: "button", class: "ghost", onclick: () => { preview = null; body.replaceChildren(); foot.replaceChildren(); paste.hidden = false; text.focus(); } }, "← Paste again"),
      el("span", { class: "gi-gap" }),
      doImport);
    paste.hidden = true;
  };
  const showDone = (res) => {
    const bits = [`Imported ${res.imported} item${res.imported === 1 ? "" : "s"}`];
    if (res.duplicates) bits.push(`${res.duplicates} already in`);
    if (res.without_raid) bits.push(`${res.without_raid} without a raid`);
    if (res.won_reserves) bits.push(`${res.won_reserves} soft reserve${res.won_reserves === 1 ? "" : "s"} won`);
    body.replaceChildren(el("div", { class: "gi-done" },
      el("p", { class: "gi-done-head" }, res.imported ? "✓ " : "", bits.join(" · "), "."),
      res.import_id ? el("p", { class: "muted small" }, "Imported by mistake? You can undo the whole import for 24 hours (here, or under Loot → Imports).") : el("p", { class: "muted small" }, "Nothing new: everything was already in.")));
    foot.replaceChildren(
      res.import_id ? el("button", { type: "button", class: "ghost danger-btn", "data-act": "undo", onclick: async () => {
        if (!(await confirmBox("Undo this import?", `The ${res.imported} item${res.imported === 1 ? "" : "s"} it added come out of the loot history again.`, "Undo import"))) return;
        const ok = await tryApi("DELETE", `/api/loot/imports/${res.import_id}`).then(() => true, () => false);
        if (ok) { toast("Import undone."); onDone?.(); dialog.close(); }
      } }, "Undo import") : null,
      el("span", { class: "gi-gap" }),
      el("button", { type: "button", class: "primary", onclick: () => dialog.close() }, "Done"));
    toast(`${bits.join(" · ")}.`);
    onDone?.();
  };
  dialog.append(
    el("div", { class: "att-head" },
      el("div", {}, el("h2", {}, "Import loot from Gargul"),
        el("div", { class: "muted small" }, ev ? `${ev.title} · ${eventWhen(ev).text}` : `${currentGuild()?.name ?? ""} · rows go to raids by date and sign-ups`)),
      el("div", { class: "att-head-tools" }, closeX(dialog))),
    paste, body, foot);
  setTimeout(() => text.focus(), 50);
  return dialog;
}
window.openGargulImport = openGargulImport;

// ======================================================================
// The Loot section on raid cards
// ======================================================================

const raidLoot = { cache: new Map(), open: new Set(), pending: new Map(), deOpen: new Set() };

function loadRaidLoot(ev) {
  const sig = String(ev.loot_count ?? 0);
  const hit = raidLoot.cache.get(ev.id);
  if (hit?.sig === sig) return hit.awards;
  if (raidLoot.pending.get(ev.id) !== sig) {
    raidLoot.pending.set(ev.id, sig);
    api("GET", `/api/events/${ev.id}/loot`).then((awards) => {
      raidLoot.cache.set(ev.id, { sig, awards });
      for (const node of document.querySelectorAll(`details.ev-lootlog[data-event="${ev.id}"]`)) node.lootFill?.();
    }).catch(() => {}).finally(() => { if (raidLoot.pending.get(ev.id) === sig) raidLoot.pending.delete(ev.id); });
  }
  return hit?.awards ?? null;
}

function raidLootEntry(ev, a, manager) {
  return el("div", { class: `rl-row${a.disenchanted ? " de" : ""}`, "data-award": a.id },
    lootItemName(a.item_id, a.item_name, a.quality, a.branch || branchForGame(ev.game)),
    el("span", { class: "rl-badges" }, a.offspec ? osBadge() : null, a.reserved ? srWonBadge() : null),
    el("span", { class: "rl-who" }, lootWho(a)),
    manager ? el("button", { type: "button", class: "sr-x", title: "Remove from the loot history", "aria-label": `Remove ${a.item_name}`, onclick: async () => {
      if (await removeLootEntry(a)) { raidLoot.cache.delete(ev.id); toast("Removed."); }
    } }, "✕") : null);
}

function raidLootSection(ev) {
  if (!ev.loot_count) return null;
  const manager = canManageEvent(ev);
  const details = el("details", { class: "ev-lootlog", "data-event": ev.id });
  if (raidLoot.open.has(ev.id)) details.open = true;
  const sum = el("span", { class: "ev-comp-sum" }, `${ev.loot_count} item${ev.loot_count === 1 ? "" : "s"}`);
  const body = el("div", { class: "ev-lootlog-body" });
  details.append(el("summary", {}, el("span", { class: "ev-comp-title" }, "💰 Loot"), sum), body);
  details.lootFill = () => {
    const awards = raidLoot.cache.get(ev.id)?.awards ?? null;
    if (awards) {
      const de = awards.filter((a) => a.disenchanted);
      const won = awards.filter((a) => !a.disenchanted);
      sum.textContent = [`${won.length} won`, de.length ? `${de.length} disenchanted` : null, won.some((a) => a.reserved) ? `${won.filter((a) => a.reserved).length} SR` : null].filter(Boolean).join(" · ");
    }
    if (!details.open) return;
    if (!awards) return body.replaceChildren(el("p", { class: "muted small" }, "Loading…"));
    const won = awards.filter((a) => !a.disenchanted);
    const de = awards.filter((a) => a.disenchanted);
    // By boss when the loot lists know where things drop.
    const byBoss = won.some((a) => a.boss);
    let list;
    if (byBoss) {
      const groups = new Map();
      for (const a of won) {
        const k = a.boss || "Other";
        if (!groups.has(k)) groups.set(k, []);
        groups.get(k).push(a);
      }
      if (groups.has("Other")) { const o = groups.get("Other"); groups.delete("Other"); groups.set("Other", o); }
      list = [...groups].map(([boss, items]) => el("div", { class: "rl-boss", "data-boss": boss },
        el("div", { class: "rl-boss-head" }, boss, el("span", { class: "muted small" }, ` · ${items.length}`)),
        items.map((a) => raidLootEntry(ev, a, manager))));
    } else {
      list = won.map((a) => raidLootEntry(ev, a, manager));
    }
    const deBox = de.length ? el("details", { class: "rl-de" }, el("summary", { class: "muted small" }, `✨ Disenchanted (${de.length})`),
      de.map((a) => raidLootEntry(ev, a, manager))) : null;
    if (deBox) {
      deBox.open = raidLoot.deOpen.has(ev.id);
      deBox.addEventListener("toggle", () => { if (deBox.open) raidLoot.deOpen.add(ev.id); else raidLoot.deOpen.delete(ev.id); });
    }
    body.replaceChildren(
      won.length ? el("div", { class: "rl-list" }, list) : el("p", { class: "muted small" }, "Everything was disenchanted."),
      deBox,
      el("div", { class: "rl-foot" },
        el("button", { type: "button", class: "link small", onclick: () => openLootLog({ event: ev.id }) }, "📜 In the Loot page"),
        manager ? el("button", { type: "button", class: "link small", onclick: () => openGargulImport(ev) }, "Import more from Gargul…") : null));
  };
  details.addEventListener("toggle", () => {
    if (details.open) { raidLoot.open.add(ev.id); loadRaidLoot(ev); } else raidLoot.open.delete(ev.id);
    details.lootFill();
  });
  loadRaidLoot(ev);
  details.lootFill();
  return details;
}

// ======================================================================
// The outhouse's Loot page
// ======================================================================

const LOOT_SORTS = [["date", "When"], ["item", "Item"], ["character", "Who"], ["raid", "Raid"]];

async function openLootLog(filters = {}) {
  if (!state.guildId) return;
  const guild = state.guildId;
  const manager = isLootManager(guild);
  const dialog = raidWindow("loot-log-dialog");
  const f = { tab: filters.tab ?? "items", sort: "date", dir: "desc" };
  let entries = [];
  let more = false;
  let total = 0;
  const field = (label, node) => el("label", { class: "ll-field" }, el("span", { class: "ll-label" }, label), node);
  const player = el("select", { class: "select", "aria-label": "Player" },
    el("option", { value: "" }, "Everyone"),
    [...state.members].filter((m) => !m.bot).sort((a, b) => a.display_name.localeCompare(b.display_name))
      .map((m) => el("option", { value: m.id, selected: m.id === filters.user }, m.display_name)));
  const character = el("input", { type: "search", placeholder: "Any character", "aria-label": "Character", maxlength: 60, value: filters.character ?? "" });
  const raid = el("select", { class: "select", "aria-label": "Raid" }, el("option", { value: "" }, "Any raid"), el("option", { value: "none" }, "No raid"));
  const instance = el("select", { class: "select", "aria-label": "Instance" }, el("option", { value: "" }, "Any instance"));
  const from = el("input", { type: "date", "aria-label": "From" });
  const to = el("input", { type: "date", "aria-label": "To" });
  const spec = el("select", { class: "select", "aria-label": "Spec" },
    el("option", { value: "" }, "Main & off spec"), el("option", { value: "ms" }, "Main spec"), el("option", { value: "os" }, "Off spec"));
  const deBox = el("input", { type: "checkbox", "aria-label": "Include disenchanted" });
  const search = el("input", { type: "search", placeholder: "Search items, raids or people", "aria-label": "Search the loot history", maxlength: 100 });
  const body = el("div", { class: "loot-log-list" }, el("p", { class: "muted" }, "Loading…"));
  const countLine = el("span", { class: "muted small ll-count" });
  const moreBtn = el("button", { type: "button", class: "ghost", hidden: true, onclick: () => load(true) }, "Load more");
  if (filters.event) raid.append(el("option", { value: filters.event, selected: true }, "This raid"));

  const params = () => {
    const q = new URLSearchParams();
    if (player.value) q.set("user", player.value);
    if (character.value.trim()) q.set("character", character.value.trim());
    if (raid.value) q.set("event", raid.value);
    if (instance.value) q.set("instance", instance.value);
    if (from.value) q.set("from", from.value);
    if (to.value) q.set("to", to.value);
    if (spec.value) q.set("spec", spec.value);
    if (deBox.checked) q.set("de", "1");
    if (search.value.trim()) q.set("q", search.value.trim());
    return q;
  };
  const sortHead = (key, label) => {
    const on = f.sort === key;
    return el("th", { "aria-sort": on ? (f.dir === "asc" ? "ascending" : "descending") : "none" },
      el("button", { type: "button", class: `ll-sort${on ? " on" : ""}`, "data-sort": key, onclick: () => {
        if (f.sort === key) f.dir = f.dir === "asc" ? "desc" : "asc";
        else { f.sort = key; f.dir = key === "date" ? "desc" : "asc"; }
        load();
      } }, label, on ? el("span", { class: "ll-arrow", "aria-hidden": "true" }, f.dir === "asc" ? " ▲" : " ▼") : null));
  };
  const drawItems = () => {
    countLine.textContent = `${total} item${total === 1 ? "" : "s"}`;
    if (!entries.length) {
      body.replaceChildren(el("div", { class: "att-empty" }, el("p", {}, total === 0 && !params().toString() ? "No loot yet." : "Nothing matches those filters."),
        el("p", { class: "muted small" }, manager ? "Import Gargul's awarded loot export, or log loot from a raid's card (💰 Loot won) once it starts." : "Raid leaders import loot from Gargul or log it from a raid's card.")));
    } else {
      body.replaceChildren(el("table", { class: "loot-log-table" },
        el("thead", {}, el("tr", {}, LOOT_SORTS.map(([k, label]) => sortHead(k, label)), manager ? el("th", {}, "") : null)),
        el("tbody", {}, entries.map((a) => el("tr", { "data-award": a.id, class: a.disenchanted ? "de" : "" },
          el("td", { class: "muted small ll-when" }, lootDay(a.awarded_on, { day: "numeric", month: "short", year: "numeric" })),
          el("td", { class: "ll-item" }, lootItemName(a.item_id, a.item_name, a.quality, a.branch || null),
            a.offspec ? osBadge() : null, a.reserved ? srWonBadge() : null),
          el("td", { class: "ll-who" }, lootWho(a, { member: true })),
          el("td", { class: "ll-raid" }, a.raid_title ? el("div", {}, a.raid_title) : el("div", { class: "muted" }, "No raid"),
            el("div", { class: "muted small" }, [a.instance_name, a.raid_starts_at ? new Date(a.raid_starts_at).toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" }) : null].filter(Boolean).join(" · "))),
          manager ? el("td", {}, el("button", { type: "button", class: "sr-x", title: "Remove from the loot history", "aria-label": `Remove ${a.item_name}`, onclick: async () => {
            if (await removeLootEntry(a)) { entries = entries.filter((x) => x.id !== a.id); total--; drawItems(); }
          } }, "✕")) : null)))));
    }
    moreBtn.hidden = !more;
  };
  // By character: loot won and raid attendance (MRT imports) side by side.
  // Attendance % = raids attended out of raids attended or signed up to and
  // missed, over the picked range (loot uses the same dates).
  const range = el("select", { class: "select ll-range", "aria-label": "Range" },
    [["4w", "Last 4 weeks"], ["8w", "Last 8 weeks"], ["phase", "This phase"], ["all", "All time"], ["custom", "Custom (From / To)"]]
      .map(([v, label]) => el("option", { value: v, selected: v === prefs.get("loot_range", "all") }, label)));
  const rangeLine = el("span", { class: "muted small ll-range-label" });
  const P_COLS = [["character", "Character"], ["attendance", "Attendance"], ["noshows", "No-shows"], ["last_raid", "Last raid"],
    ["ms", "Main spec"], ["os", "Off spec"], ["sr", "SR"], ["total", "Items"], ["per_raid", "Per raid"], ["last_item", "Last item"]];
  const P_HINT = {
    attendance: "Raids they were there for (at least one boss, or marked present, late or benched) out of raids they attended or signed up to and missed. Only raids with attendance (imported from MRT or taken by hand) count; excused never counts.",
    noshows: "Signed up (or late, or tentative) and not there for any boss. “Can't make it” and the bench don't count.",
    per_raid: "Items won per raid attended",
  };
  const pSort = { key: "attendance", dir: "desc" };
  let people = [];
  const pValue = (c, key) => ({
    character: (c.character_name || c.display_name || "").toLowerCase(),
    attendance: c.att ? c.att.percent * 1000 + c.att.attended : -1,
    noshows: c.att?.no_shows ?? -1, last_raid: c.att?.last_attended ?? "",
    ms: c.ms, os: c.os, sr: c.reserved, total: c.total, per_raid: c.per_raid ?? -1, last_item: c.last_on ?? "",
  })[key];
  const drawPeople = () => {
    moreBtn.hidden = true;
    const list = [...people].sort((a, b) => {
      const x = pValue(a, pSort.key), y = pValue(b, pSort.key);
      const cmp = typeof x === "string" ? x.localeCompare(y) : x - y;
      return (pSort.dir === "asc" ? cmp : -cmp) || b.total - a.total || (a.character_name || "").localeCompare(b.character_name || "");
    });
    countLine.textContent = plural(list.length, "character");
    if (!list.length) return body.replaceChildren(el("p", { class: "muted att-empty" }, "Nobody's won anything or been to a raid with attendance in that range."));
    const head = (key, label) => {
      const on = pSort.key === key;
      return el("th", { "data-col": key, title: P_HINT[key] ?? "", "aria-sort": on ? (pSort.dir === "asc" ? "ascending" : "descending") : "none" },
        el("button", { type: "button", class: `ll-sort${on ? " on" : ""}`, "data-psort": key, onclick: () => {
          if (pSort.key === key) pSort.dir = pSort.dir === "asc" ? "desc" : "asc";
          else { pSort.key = key; pSort.dir = key === "character" ? "asc" : "desc"; }
          drawPeople();
        } }, label, on ? el("span", { class: "ll-arrow", "aria-hidden": "true" }, pSort.dir === "asc" ? " ▲" : " ▼") : null));
    };
    const day = (d) => (d ? lootDay(d, { day: "numeric", month: "short", year: "numeric" }) : "—");
    body.replaceChildren(el("table", { class: "loot-log-table ll-people" },
      el("thead", {}, el("tr", {}, P_COLS.map(([k, label]) => head(k, label)))),
      el("tbody", {}, list.map((c) => el("tr", { "data-character": c.character_key, "data-user": c.user_id ?? "" },
        el("td", { "data-col": "character" }, el("button", { type: "button", class: "loot-who", style: classColour(c.character_class) ? `color:${classColour(c.character_class)}` : "",
          title: "Show their items", onclick: () => { character.value = c.character_name; setTab("items"); } },
        c.user_id ? null : el("span", { class: "loot-warn", title: "Nobody in this outhouse has a character by that name" }, "⚠"), c.character_name || c.display_name || "?"),
        c.display_name && c.display_name !== c.character_name ? el("span", { class: "muted small" }, ` ${c.display_name}`) : null),
        el("td", { class: "ll-att", "data-col": "attendance" }, c.att ? [
          el("span", { class: `ll-pct${c.att.percent < 75 ? " low" : ""}` }, `${c.att.percent}%`),
          el("span", { class: "muted small" }, ` ${c.att.attended} of ${c.att.eligible}`),
          el("span", { class: "ll-bar-track", "aria-hidden": "true" }, el("span", { class: "ll-bar-fill", style: `width:${c.att.percent}%` }))] : el("span", { class: "muted" }, "—")),
        el("td", { class: `ll-num${c.att?.no_shows ? " ll-noshow" : ""}`, "data-col": "noshows" }, c.att ? String(c.att.no_shows) : "—"),
        el("td", { class: "muted small ll-num", "data-col": "last_raid" }, day(c.att?.last_attended)),
        el("td", { class: "ll-num", "data-col": "ms" }, c.ms), el("td", { class: "ll-num", "data-col": "os" }, c.os), el("td", { class: "ll-num", "data-col": "sr" }, c.reserved),
        el("td", { class: "ll-num", "data-col": "total" }, el("strong", {}, c.total)),
        el("td", { class: "ll-num", "data-col": "per_raid" }, c.per_raid == null ? "—" : c.per_raid.toFixed(1)),
        el("td", { class: "muted small ll-num", "data-col": "last_item" }, day(c.last_on)))))));
  };
  const loadPeople = async (n) => {
    const r = range.value;
    const aq = new URLSearchParams({ range: r });
    if (r === "custom") { if (from.value) aq.set("from", from.value); if (to.value) aq.set("to", to.value); }
    const att = await tryApi("GET", `/api/guilds/${guild}/attendance/mrt/characters?${aq}`).catch(() => null);
    if (n !== seq || !dialog.isConnected) return;
    const q = params();
    q.delete("from"); q.delete("to");
    if (r !== "all" && att?.from) q.set("from", att.from);
    if (r !== "all" && att?.to) q.set("to", att.to);
    if (r === "custom") { if (from.value) q.set("from", from.value); if (to.value) q.set("to", to.value); }
    const loot = await tryApi("GET", `/api/guilds/${guild}/loot/summary?${q}`).catch(() => null);
    if (n !== seq || !dialog.isConnected || !loot) return;
    rangeLine.textContent = att ? `${att.label} · ${plural(att.raids, "raid")} with attendance` : "";
    // Attendance only knows players and characters: the loot filters narrow it by hand.
    const charQ = lootKey(character.value);
    const keep = (a) => (!player.value || a.user_id === player.value) && (!charQ || a.character_key.includes(charQ));
    const rows = new Map(loot.map((c) => [`${c.character_key}|${c.user_id ?? ""}`, { ...c }]));
    for (const a of (att?.characters ?? []).filter(keep)) {
      const k = `${a.character_key}|${a.user_id ?? ""}`;
      const row = rows.get(k) ?? { character_key: a.character_key, character_name: a.character_name, user_id: a.user_id, display_name: a.display_name,
        character_class: a.character_class, ms: 0, os: 0, reserved: 0, total: 0, last_on: null };
      if (!row.character_class) row.character_class = a.character_class;
      row.att = a;
      rows.set(k, row);
    }
    people = [...rows.values()].map((c) => ({ ...c, per_raid: c.att?.attended ? c.total / c.att.attended : null }));
    drawPeople();
  };
  range.addEventListener("change", () => { prefs.set("loot_range", range.value); load(); });
  const drawImports = (list, attList) => {
    moreBtn.hidden = true;
    countLine.textContent = "";
    if (!list.length && !attList?.length) return body.replaceChildren(el("p", { class: "muted att-empty" }, "No imports yet."));
    const when = (i) => new Date(i.created_at).toLocaleString([], { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" });
    body.replaceChildren(
      el("h3", { class: "ll-imports-head" }, "💰 Loot (Gargul)"),
      list.length ? el("div", { class: "ll-imports" }, list.map((i) => el("div", { class: `ll-import${i.undone_at ? " undone" : ""}`, "data-import": i.id },
        el("div", {}, el("strong", {}, `${i.row_count} item${i.row_count === 1 ? "" : "s"}`), ` by ${i.imported_by_name ?? "someone"}`,
          el("div", { class: "muted small" }, when(i),
            i.undone_at ? " · undone" : i.remaining !== i.row_count ? ` · ${i.remaining} still in` : "")),
        i.can_undo ? el("button", { type: "button", class: "ghost danger-btn", onclick: async () => {
          if (!(await confirmBox("Undo this import?", `The items it added come out of the loot history again${i.remaining ? ` (${i.remaining})` : ""}.`, "Undo import"))) return;
          if (await tryApi("DELETE", `/api/loot/imports/${i.id}`).then(() => true, () => false)) { toast("Import undone."); load(); }
        } }, "Undo") : null))) : el("p", { class: "muted small ll-imports-none" }, "None yet."),
      el("h3", { class: "ll-imports-head" }, "📋 Attendance (MRT)"),
      attList?.length ? el("div", { class: "ll-imports" }, attList.map((i) => el("div", { class: `ll-import${i.undone_at ? " undone" : ""}`, "data-att-import": i.id },
        el("div", {}, el("strong", {}, plural(i.boss_count, "boss", "bosses")), ` · ${i.player_count} there · by ${i.imported_by_name ?? "someone"}`,
          el("div", { class: "muted small" }, [when(i), i.raids.join(", "), i.undone_at ? "taken out" : i.remaining !== i.boss_count ? `${i.remaining} still in` : null].filter(Boolean).join(" · "))),
        i.can_undo || i.can_delete ? el("button", { type: "button", class: "ghost danger-btn", "data-act": i.can_undo ? "undo" : "delete", onclick: async () => {
          const what = i.can_undo ? "Undo this import?" : "Delete this import?";
          if (!(await confirmBox(what, `The ${plural(i.remaining, "boss", "bosses")} it added come out of the raids' attendance again. This is noted in the audit log.`, i.can_undo ? "Undo import" : "Delete import"))) return;
          if (await tryApi("DELETE", `/api/attendance/imports/${i.id}`).then(() => true, () => false)) { toast(i.can_undo ? "Import undone." : "Import deleted."); load(); }
        } }, i.can_undo ? "Undo" : "Delete") : null))) : el("p", { class: "muted small ll-imports-none" }, "None yet."),
      el("p", { class: "muted small" }, "Whoever imported can undo an import for 24 hours; raid managers can delete attendance imports any time. Single items can be removed from the Items list."));
  };
  let seq = 0;
  const load = async (append = false) => {
    const n = ++seq;
    rangeBox.hidden = f.tab !== "people";
    if (f.tab === "imports") {
      const [list, attList] = await Promise.all([
        tryApi("GET", `/api/guilds/${guild}/loot/imports`).catch(() => null),
        tryApi("GET", `/api/guilds/${guild}/attendance/mrt/imports`).catch(() => null)]);
      if (n === seq && list && dialog.isConnected) drawImports(list, attList);
      return;
    }
    const q = params();
    if (f.tab === "people") return loadPeople(n);
    q.set("limit", "50");
    q.set("sort", f.sort);
    q.set("dir", f.dir);
    if (append) q.set("offset", String(entries.length));
    const data = await tryApi("GET", `/api/guilds/${guild}/loot?${q}`).catch(() => null);
    if (n !== seq || !dialog.isConnected) return;
    if (!data) { body.replaceChildren(el("p", { class: "muted" }, "Couldn't load the loot history.")); return; }
    entries = append ? [...entries, ...data.entries] : data.entries;
    more = data.has_more;
    total = data.total;
    drawItems();
  };
  const tabs = el("div", { class: "seg ll-tabs", role: "tablist" });
  const setTab = (t) => {
    f.tab = t;
    tabs.replaceChildren(...[["items", "Items"], ["people", "By character"], manager ? ["imports", "Imports"] : null].filter(Boolean).map(([k, label]) => el("button", {
      type: "button", role: "tab", class: `seg-btn${f.tab === k ? " on" : ""}`, "aria-selected": f.tab === k ? "true" : "false", "data-tab": k, onclick: () => setTab(k),
    }, label)));
    filterBox.hidden = t === "imports";
    load();
  };
  let timer = null;
  for (const input of [search, character]) input.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(() => load(), 250); });
  for (const input of [player, raid, instance, spec, deBox]) input.addEventListener("change", () => load());
  // Picking dates on the By character tab makes the range a custom one.
  for (const input of [from, to]) input.addEventListener("change", () => {
    if (f.tab === "people" && range.value !== "custom") { range.value = "custom"; prefs.set("loot_range", "custom"); }
    load();
  });
  const exportCsv = async () => {
    const data = await tryApi("GET", `/api/guilds/${guild}/loot/export?${params()}`).catch(() => null);
    if (!data) return;
    const blob = new Blob([data.csv], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const name = (currentGuild()?.name ?? "outhouse").replace(/[^\w\- ]+/g, "").trim().replace(/\s+/g, "-") || "outhouse";
    const a = el("a", { href: url, download: `${name}-loot.csv` });
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 5000);
    toast(`${data.rows} item${data.rows === 1 ? "" : "s"} saved as CSV.`);
  };
  const rangeBox = el("div", { class: "ll-rangebox", hidden: true }, el("label", { class: "ll-range-wrap" }, el("span", { class: "ll-label" }, "Range"), range), rangeLine);
  const extra = (node) => { node.classList.add("ll-extra"); return node; };
  const filterBox = el("div", { class: "loot-log-filters" },
    field("Player", player), field("Character", character), field("Raid", raid), extra(field("Instance", instance)),
    extra(field("From", from)), extra(field("To", to)), extra(field("Spec", spec)),
    extra(el("label", { class: "ll-check" }, deBox, "Disenchanted")),
    el("div", { class: "ll-search" }, search),
    // Phones: the rest of the filters on request.
    el("button", { type: "button", class: "link small ll-more", onclick: (e) => {
      const on = filterBox.classList.toggle("all");
      e.currentTarget.textContent = on ? "Fewer filters" : "More filters";
    } }, "More filters"));
  dialog.append(
    el("div", { class: "att-head" },
      el("div", {}, el("h2", {}, "📜 Loot & attendance"), el("div", { class: "muted small" }, currentGuild()?.name ?? "")),
      el("div", { class: "att-head-tools" },
        manager ? el("button", { type: "button", class: "ghost small-btn", "data-act": "gargul", onclick: () => openGargulImport(null, () => load()) }, "Import", el("span", { class: "ll-long" }, " Gargul export"), "…") : null,
        manager && typeof openMrtImport === "function" ? el("button", { type: "button", class: "ghost small-btn", "data-act": "mrt", title: "Import attendance from Method Raid Tools", onclick: () => openMrtImport(null, () => load()) },
          el("span", { class: "ll-long" }, "Import attendance (MRT)"), el("span", { class: "ll-short" }, "MRT"), "…") : null,
        el("button", { type: "button", class: "ghost small-btn", "data-act": "csv", title: "The items shown (all of them, not just this page)", onclick: exportCsv }, "⬇ CSV"),
        closeX(dialog))),
    el("div", { class: "ll-bar" }, tabs, countLine, rangeBox),
    filterBox,
    el("div", { class: "att-table-wrap" }, body), moreBtn);
  // Raids from the last few months, and the instances.
  const since = new Date(Date.now() - 180 * 86400000), until = new Date(Date.now() + 86400000);
  api("GET", `/api/guilds/${guild}/events?from=${encodeURIComponent(since.toISOString())}&to=${encodeURIComponent(until.toISOString())}`).then((list) => {
    const past = list.filter((e) => new Date(e.starts_at) < until).sort((a, b) => new Date(b.starts_at) - new Date(a.starts_at));
    raid.append(...past.filter((e) => e.id !== filters.event).map((e) => el("option", { value: e.id }, `${new Date(e.starts_at).toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" })} · ${e.title}`)));
    if (filters.event) {
      const mine = past.find((e) => e.id === filters.event);
      const opt = raid.querySelector(`option[value="${filters.event}"]`);
      if (mine && opt) opt.textContent = `${new Date(mine.starts_at).toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" })} · ${mine.title}`;
    }
  }).catch(() => {});
  api("GET", `/api/guilds/${guild}/loot-instances`).then((list) => {
    instance.append(...list.map((i) => el("option", { value: i.key }, i.name)));
  }).catch(() => {});
  setTab(f.tab);
  return dialog;
}
window.openLootLog = openLootLog;

// ======================================================================
// Full profile page: each character's loot
// ======================================================================

function charLootBlock(c, guildName) {
  const LIMIT = 6;
  let all = false;
  const list = el("div", { class: "fpl-items" });
  const drawList = () => {
    const items = all ? c.items : c.items.slice(0, LIMIT);
    list.replaceChildren(...items.map((a) => el("div", { class: "fpl-row", "data-award": a.id },
      el("span", { class: "muted small fpl-date" }, lootDay(a.awarded_on, { day: "numeric", month: "short" })),
      lootItemName(a.item_id, a.item_name, a.quality, a.branch || null),
      a.offspec ? osBadge() : null, a.reserved ? srWonBadge() : null,
      el("span", { class: "muted small fpl-raid" }, a.raid_title || "No raid"))),
    c.items.length > LIMIT ? el("button", { type: "button", class: "link small", onclick: () => { all = !all; drawList(); } }, all ? "Show fewer" : `Show all ${c.items.length}`) : null);
  };
  drawList();
  const colour = classColour(c.character_class);
  return el("div", { class: "fpl-char", "data-character": c.character_key, style: colour ? `--cc:${colour}` : "" },
    el("div", { class: "fpl-head" },
      el("strong", { class: "fpl-name" }, c.character_name || "No character"),
      el("span", { class: "fpl-count" }, `${c.this_phase} item${c.this_phase === 1 ? "" : "s"} ${c.phase_label === "last 2 months" ? "in the last 2 months" : `this phase (${c.phase_label})`}`),
      el("span", { class: "muted small" }, [`${c.total} in all`, c.offspec ? `${c.offspec} off spec` : null, guildName].filter(Boolean).join(" · "))),
    list);
}

async function addProfileLoot(page, p) {
  if (!["full", "limited"].includes(p.level)) return;
  const data = await api("GET", `/api/users/${p.id}/loot`).catch(() => null);
  if (!data?.length || !page.isConnected) return;
  const several = data.length > 1;
  const blocks = data.flatMap((g) => g.characters.map((c) => ({ c, g })));
  // Their profile characters get a count on their card.
  const keyOf = (ch) => [lootKey(ch.name), lootKey(`${ch.name} ${ch.last_name || ""}`)];
  const cards = [...page.querySelectorAll(".fp-chars .fp-char")];
  (p.characters ?? []).forEach((ch, i) => {
    const keys = keyOf(ch);
    const mine = blocks.filter(({ c }) => keys.includes(c.character_key));
    const n = mine.reduce((s, { c }) => s + c.this_phase, 0);
    if (!mine.length || !cards[i]) return;
    cards[i].querySelector(".fp-char-text")?.append(el("button", { type: "button", class: "link small fp-char-loot", onclick: () => page.querySelector(`.fpl-char[data-character="${CSS.escape(mine[0].c.character_key)}"]`)?.scrollIntoView({ behavior: "smooth", block: "center" }) },
      `💰 ${n} item${n === 1 ? "" : "s"} this phase`));
  });
  const section = el("div", { class: "fp-loot", id: "fp-loot" },
    el("h4", {}, "LOOT HISTORY"),
    blocks.map(({ c, g }) => charLootBlock(c, several ? g.guild_name : null)));
  const wow = page.querySelector("#fp-wow");
  if (wow) wow.append(section);
  else {
    const s = profileSection("loot", "Loot history", section);
    section.querySelector("h4")?.remove();
    const notice = page.querySelector(".fp-notice");
    if (notice) notice.before(s); else page.append(s);
  }
}

if (typeof drawProfileOverview === "function") {
  const llBaseOverview = drawProfileOverview;
  drawProfileOverview = function (page, p, close) {
    const out = llBaseOverview(page, p, close);
    addProfileLoot(page, p);
    return out;
  };
}
