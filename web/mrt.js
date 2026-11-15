// Outhouse: raid attendance. Importing Method Raid Tools' (MRT) attendance
// export (a raid card's ⋯ menu, or the Loot & attendance window), the one
// 📋 Attendance section on raid cards (who came, no-shows, who came without
// signing up, who left early or joined late; hand marks override the
// import, and raid leaders edit them there: Edit attendance), the no-show hint
// raid leaders see next to sign-ups, and attendance per character on the
// full profile page. The By character tab's attendance columns are in
// lootlog.js. Loaded after lootlog.js; uses its helpers (lootKey, lootDay,
// isLootManager), loot.js (plural) and raids.js/profiles.js (raidWindow,
// closeX, …).
"use strict";

const MRT_HELP = "In game, open Method Raid Tools → Attendance, export the raid (or a few nights), copy the grid and paste it here. Bosses already imported are skipped.";

// Sign-up statuses that make missing the raid a no-show ("can't make it"
// and the bench never count).
const NO_SHOW_STATUSES = new Set(["yes", "late", "tentative"]);
const MISSED_TAG = { tentative: "tentative", no: "said absent", bench: "bench" };

const mrtTime = (local) => String(local).slice(11, 16);

// A sign-up from a raid card (ev.signups) or a preview, as one shape.
function mrtSignup(s) {
  const charName = s.character?.full_name || s.character?.name || s.character_name || "";
  return {
    user_id: s.user_id, status: s.status, display_name: s.display_name,
    name: charName || s.display_name, key: charName ? lootKey(charName) : (s.character_key || ""),
    class: s.character?.class || s.class || "",
  };
}

// Marks raid leaders set by hand (Edit attendance). Present, late and
// benched count as attended; excused is neither attended nor a no-show;
// absent is a no-show if they signed up (yes, late or tentative).
const ATT_MARKS = ["present", "late", "benched", "excused", "absent"];
const ATT_MARK_LABEL = { present: "Present", late: "Late", benched: "Benched", excused: "Excused", absent: "Absent" };
const ATT_MARK_ICON = { present: "✓", late: "⏱", benched: "🪑", excused: "🛈", absent: "✗" };
const ATTENDED_MARKS = new Set(["present", "late", "benched"]);

// Who came and who didn't, against the sign-ups. `pulls`: [{boss_name, local}],
// `players`: [{name, key, user_id, class, display_name, present: [bool per pull]}],
// `manual`: hand marks [{user_id, display_name, status}], which override
// what the import says about that member.
function mrtCompare(pulls, players, signups, manual = []) {
  const ups = signups.map(mrtSignup);
  const marks = new Map(manual.map((m) => [m.user_id, m]));
  const n = pulls.length;
  const signupOf = (p) => (p.user_id ? ups.find((s) => s.user_id === p.user_id) : null) || ups.find((s) => s.key && s.key === p.key) || null;
  const attended = [], unsigned = [], partial = [];
  const came = new Set();
  const add = (p, s) => {
    if (s) came.add(s.user_id);
    if (p.user_id) came.add(p.user_id);
    if (s && s.status !== "no") attended.push({ p, s });
    else unsigned.push({ p, s });
  };
  for (const raw of players) {
    const m = raw.user_id ? marks.get(raw.user_id) : null;
    if (m ? !ATTENDED_MARKS.has(m.status) : !raw.present.some(Boolean)) continue;
    // Marked by hand: once, however many characters MRT saw them on.
    if (m && came.has(raw.user_id)) continue;
    const p = m ? { ...raw, mark: m } : raw;
    const s = signupOf(p);
    add(p, s);
    if (!m && n > 1 && !p.present.every(Boolean)) {
      const first = p.present.indexOf(true), last = p.present.lastIndexOf(true);
      const bits = [];
      if (first > 0) bits.push("joined late");
      if (last < n - 1) bits.push("left early");
      const gaps = p.present.map((v, i) => (!v && i > first && i < last ? pulls[i].boss_name : null)).filter(Boolean);
      if (gaps.length) bits.push(`missed ${gaps.join(", ")}`);
      partial.push({ p, s, note: bits.join(", ") });
    }
  }
  // Marked present (late, benched) but not in the import.
  for (const m of manual) {
    if (!ATTENDED_MARKS.has(m.status) || came.has(m.user_id)) continue;
    const s = ups.find((u) => u.user_id === m.user_id) || null;
    add({ name: s?.name || m.display_name, key: s?.key || "", user_id: m.user_id, class: s?.class || "", display_name: m.display_name, present: pulls.map(() => false), mark: m }, s);
  }
  const missed = ups.filter((s) => !came.has(s.user_id)).map((s) => {
    const mark = marks.get(s.user_id)?.status ?? null;
    return { s, mark, counted: NO_SHOW_STATUSES.has(s.status) && mark !== "excused" };
  });
  missed.sort((a, b) => Number(b.counted) - Number(a.counted) || a.s.name.localeCompare(b.s.name));
  return { attended, unsigned, partial, missed, noShows: missed.filter((m) => m.counted).length, there: attended.length + unsigned.length };
}

// "✓ Naj'entus 20:44 / ✗ Supremus 21:10" for a name's tooltip.
const presenceTitle = (pulls, p) => pulls.map((b, i) => `${p.present[i] ? "✓" : "✗"} ${b.boss_name} ${mrtTime(b.local)}`).join("\n");
const markedBy = (m) => `Marked ${ATT_MARK_LABEL[m.status]?.toLowerCase() ?? m.status} by hand${m.marked_by_name ? ` by ${m.marked_by_name}` : ""}`;

function mrtName(p, pulls, { warn = true } = {}) {
  const colour = classColour(p.class);
  const title = [p.display_name && p.display_name !== p.name ? p.display_name : null, p.mark ? markedBy(p.mark) : null,
    pulls?.length && !p.mark ? presenceTitle(pulls, p) : null].filter(Boolean).join("\n");
  if (!p.user_id && warn) {
    return el("span", { class: "mrt-name unmatched", title: `Nobody in this outhouse has a character by that name${title ? `\n${title}` : ""}` },
      el("span", { class: "loot-warn", "aria-label": "Not matched" }, "⚠"), p.name);
  }
  return el("span", { class: `mrt-name${p.mark ? " manual" : ""}`, style: colour ? `color:${colour}` : "", title },
    p.name, p.mark ? el("span", { class: `mrt-tag manual ${p.mark.status}` }, `✎ ${ATT_MARK_LABEL[p.mark.status]?.toLowerCase() ?? p.mark.status}`) : null);
}

function signupChip(s, tag, counted) {
  const colour = classColour(s.class);
  return el("span", { class: `mrt-name${counted ? " noshow" : ""}`, style: colour ? `color:${colour}` : "", title: s.display_name !== s.name ? s.display_name : "" },
    s.name, tag ? el("span", { class: `mrt-tag${counted ? "" : " excused"}` }, tag) : null);
}

const missedTag = ({ s, mark, counted }) => {
  if (mark === "excused") return "✎ excused";
  if (mark === "absent") return counted ? "✎ absent · no-show" : "✎ absent";
  return counted ? (s.status === "tentative" ? "tentative · no-show" : "no-show") : MISSED_TAG[s.status] ?? s.status;
};

// The four lists, for the import preview and the raid card.
function mrtCompareView(pulls, players, signups, manual = []) {
  const c = mrtCompare(pulls, players, signups, manual);
  const group = (cls, title, hint, items) => el("div", { class: `mrt-group ${cls}`, "data-group": cls },
    el("div", { class: "mrt-group-head", title: hint }, title, el("span", { class: "mrt-n" }, ` ${items.length}`)),
    items.length ? el("div", { class: "mrt-names" }, items) : el("div", { class: "muted small" }, "Nobody"));
  return {
    c,
    node: el("div", { class: "mrt-compare" },
      group("attended", "✓ Signed up & attended", "Signed up and there for at least one boss (or marked present, late or benched)", c.attended.map(({ p }) => mrtName(p, pulls))),
      group("missed", "✗ Signed up, didn't attend", "No-shows count: signed up (or late, or tentative) and not there for any boss, or marked absent. “Can't make it”, the bench and anyone marked excused don't count.",
        c.missed.map((m) => signupChip(m.s, missedTag(m), m.counted))),
      group("unsigned", "＋ Attended without signing up", "There for at least one boss (or marked present), but not signed up to the raid", c.unsigned.map(({ p, s }) => el("span", { class: "mrt-chip" }, mrtName(p, pulls),
        s?.status === "no" ? el("span", { class: "mrt-tag excused" }, "said absent") : null))),
      pulls.length > 1 ? group("partial", "◐ Left early / joined late", "There for some bosses but not all", c.partial.map(({ p, note }) => el("span", { class: "mrt-chip" }, mrtName(p, pulls), el("span", { class: "mrt-tag excused" }, note)))) : null),
  };
}

const compareLine = (c) => [`${c.there} attended`, plural(c.noShows, "no-show"), c.unsigned.length ? `${c.unsigned.length} unsigned` : null].filter(Boolean).join(" · ");

// ======================================================================
// Importing MRT's export
// ======================================================================

// `ev`: a raid card's import (columns go to that raid). Without: the
// Loot & attendance window's (columns go to raids by date and time).
function openMrtImport(ev = null, onDone = null) {
  const guildId = ev?.guild_id ?? state.guildId;
  const dialog = raidWindow("mrt-dialog");
  const base = ev ? `/api/events/${ev.id}/attendance/mrt` : `/api/guilds/${guildId}/attendance/mrt`;
  const text = el("textarea", { class: "gi-text", rows: 7, spellcheck: "false", placeholder: "\t15/11/2026 20:44:00 - High Warlord Naj'entus4\nAvizandum\tx\nShambulàncè\tx\n…", "aria-label": "MRT attendance export" });
  const file = el("input", { type: "file", accept: ".tsv,.txt,.csv,text/plain", hidden: true });
  const body = el("div", { class: "gi-body mrt-body" });
  const foot = el("div", { class: "gi-foot" });
  let preview = null;
  let cols = [];
  file.addEventListener("change", async () => {
    const f = file.files?.[0];
    if (!f) return;
    if (f.size > 500_000) return toast("That file's too big (500 KB at most).");
    text.value = await f.text();
    run();
  });
  const run = async () => {
    if (!text.value.trim()) return toast("Paste MRT's attendance export first.");
    go.disabled = true;
    go.textContent = "Reading…";
    const p = await tryApi("POST", `${base}/preview`, { text: text.value }).catch(() => null);
    go.disabled = false;
    go.textContent = "Preview";
    if (!p || !dialog.isConnected) return;
    preview = p;
    cols = p.columns.map((c) => ({ ...c, pick: !c.duplicate && c.in_window && !!c.event_id, raid: c.event_id ?? "" }));
    draw();
  };
  const go = el("button", { type: "button", class: "primary", "data-act": "preview", onclick: run }, "Preview");
  const paste = el("div", { class: "gi-paste" },
    el("p", { class: "muted small gi-help" }, MRT_HELP),
    text,
    el("div", { class: "gi-paste-actions" },
      el("button", { type: "button", class: "ghost", onclick: () => file.click() }, "Choose a file…"), file, el("span", { class: "gi-gap" }), go));

  const raidById = (id) => preview.raids.find((r) => r.id === id);
  const raidLabel = (r) => `${lootDay(r.local_date)} ${r.local_time} · ${r.title}`;
  // A player as seen in one raid (their sign-up there, else their character).
  const asIn = (p, raidId, present) => {
    const m = (raidId && p.matches[raidId]) || p.member;
    return { name: p.name, key: p.key, user_id: m?.user_id ?? null, class: m?.class ?? "", display_name: m?.display_name ?? "", present };
  };
  const draw = () => {
    if (!preview) return;
    const picked = cols.filter((c) => c.pick && c.raid);
    const raidIds = [...new Set(picked.map((c) => c.raid))];
    const summary = [
      plural(cols.length, "boss", "bosses"),
      plural(preview.players.length, "character"),
      ...raidIds.map((id) => `${picked.filter((c) => c.raid === id).length} → ${raidById(id)?.title ?? "a raid"}`),
      preview.duplicates ? `${preview.duplicates} already imported` : null,
    ].filter(Boolean).join(" · ");
    const notes = [...preview.warnings.map((w) => `Line ${w.line}: ${w.message}`), ...preview.ignored.map((l) => `Left out (above the grid): “${l}”`)];
    const warnings = notes.length ? el("details", { class: "gi-errors", open: preview.warnings.length > 0 },
      el("summary", {}, `⚠ ${plural(notes.length, "note")}`), el("ul", {}, notes.slice(0, 30).map((n) => el("li", { class: "small" }, n)))) : null;

    // Bosses: tick, when, boss (matched), difficulty, raid.
    const pickable = cols.filter((c) => !c.duplicate);
    const all = el("input", { type: "checkbox", "aria-label": "Tick every boss", checked: pickable.length > 0 && pickable.every((c) => c.pick) });
    all.addEventListener("change", () => { for (const c of pickable) c.pick = all.checked && (!!c.raid || !!ev); draw(); });
    const bosses = el("div", { class: "mrt-cols", role: "table", "aria-label": "Bosses to import" },
      el("div", { class: "mrt-col mrt-headrow", role: "row" }, el("span", {}, all), el("span", {}, "When"), el("span", {}, "Boss"), el("span", {}, "There"), el("span", {}, ev ? "" : "Raid")),
      cols.map((c) => {
        const box = el("input", { type: "checkbox", checked: c.pick, disabled: c.duplicate, "aria-label": `Import ${c.boss_name} at ${c.time}` });
        box.addEventListener("change", () => { c.pick = box.checked; draw(); });
        let raidCell;
        if (!ev) {
          const near = preview.raids.filter((x) => Math.abs((new Date(x.local_date) - new Date(c.date)) / 86400000) <= 1);
          const others = preview.raids.filter((x) => !near.includes(x));
          const sel = el("select", { class: "select mrt-raid", "aria-label": `Raid for ${c.boss_name} at ${c.time}`, disabled: c.duplicate },
            near.map((x) => el("option", { value: x.id, selected: c.raid === x.id }, raidLabel(x))),
            others.length ? el("optgroup", { label: "Other days" }, others.map((x) => el("option", { value: x.id, selected: c.raid === x.id }, raidLabel(x)))) : null,
            el("option", { value: "", selected: !c.raid }, "Don't import"));
          sel.addEventListener("change", () => { c.raid = sel.value; c.pick = !!sel.value; c.reason = ""; draw(); });
          raidCell = el("span", { class: "mrt-c-raid" }, sel, c.reason && !c.raid ? el("span", { class: "gi-reason small" }, c.reason) : null);
        } else {
          raidCell = el("span", { class: "mrt-c-raid" }, !c.in_window && !c.duplicate ? el("span", { class: "gi-tag out", title: "Not on this raid's night (or the day either side)" }, "not this raid") : null);
        }
        return el("div", { class: `mrt-col${c.duplicate ? " dup" : ""}${!c.in_window ? " out" : ""}${c.pick ? " picked" : ""}`, role: "row", "data-col": c.index },
          el("span", {}, box),
          el("span", { class: "mrt-c-when" }, lootDay(c.date, { weekday: "short", day: "numeric", month: "short" }), el("span", { class: "muted small" }, ` ${c.time}`)),
          el("span", { class: "mrt-c-boss" },
            el("strong", {}, c.boss_name),
            c.encounter_name ? el("span", { class: "mrt-enc", title: `${c.encounter_name}${c.instance_name ? ` (${c.instance_name})` : ""}` }, "✓", c.instance_name ? ` ${c.instance_name}` : "") : el("span", { class: "muted small", title: "Not on the encounter lists: kept by its name" }, " not a listed boss"),
            c.difficulty != null ? el("span", { class: "mrt-diff", title: `MRT's difficulty ID: ${c.difficulty}` }, c.difficulty_label || `difficulty ${c.difficulty}`) : null,
            c.duplicate ? el("span", { class: "gi-tag dup" }, "already imported") : null),
          el("span", { class: "mrt-c-n" }, String(c.present)),
          raidCell);
      }));

    // The grid: characters × bosses.
    const head = el("tr", {}, el("th", { class: "mrt-g-name" }, "Character"), cols.map((c) => el("th", { class: `mrt-g-boss${c.pick ? "" : " off"}`, title: `${c.boss_name} · ${c.date} ${c.time}` }, el("span", {}, c.boss_name), el("span", { class: "muted" }, c.time))));
    const rows = preview.players.map((p) => {
      const raidId = cols.find((c, i) => c.pick && c.raid && p.present[i])?.raid ?? (ev?.id ?? cols.find((c) => c.raid)?.raid);
      const who = asIn(p, raidId, p.present);
      return el("tr", { "data-key": p.key },
        el("td", { class: "mrt-g-name" }, mrtName(who, null), who.display_name && who.display_name !== p.name ? el("span", { class: "muted small" }, ` ${who.display_name}`) : null),
        p.present.map((v, i) => el("td", { class: `mrt-cell${v ? " here" : v === false ? " gone" : " none"}${cols[i].pick ? "" : " off"}`, title: v ? "There" : v === false ? "Not there" : "Not in that grid" }, v ? "✓" : v === false ? "·" : "")));
    });
    const grid = el("div", { class: "mrt-grid-wrap" }, el("table", { class: "mrt-grid" }, el("thead", {}, head), el("tbody", {}, rows)));

    // Against each raid's sign-ups (the ticked bosses).
    const compares = raidIds.map((id) => {
      const r = raidById(id);
      const idx = cols.map((c, i) => (c.pick && c.raid === id ? i : -1)).filter((i) => i >= 0);
      const pulls = idx.map((i) => ({ boss_name: cols[i].boss_name, local: cols[i].local }));
      const players = preview.players.filter((p) => idx.some((i) => p.present[i] != null)).map((p) => asIn(p, id, idx.map((i) => !!p.present[i])));
      const { c, node } = mrtCompareView(pulls, players, r?.signups ?? []);
      return el("section", { class: "mrt-raid-compare", "data-event": id },
        el("div", { class: "mrt-raid-head" }, el("strong", {}, r?.title ?? "Raid"), el("span", { class: "muted small" }, ` ${r ? lootDay(r.local_date) : ""} · ${compareLine(c)}`)),
        node);
    });
    const n = picked.length;
    const doImport = el("button", { type: "button", class: "primary", "data-act": "import", disabled: !n, onclick: async () => {
      doImport.disabled = true;
      const picks = picked.map((c) => ({ index: c.index, event_id: ev ? ev.id : c.raid }));
      const res = await tryApi("POST", `${base}/import`, { text: text.value, picks }).catch(() => null);
      doImport.disabled = false;
      if (!res || !dialog.isConnected) return;
      showDone(res);
    } }, n ? `Import ${plural(n, "boss", "bosses")}` : "Import");
    body.replaceChildren(
      el("div", { class: "gi-summary" }, el("strong", {}, summary)),
      warnings,
      el("div", { class: "mrt-scroll" },
        bosses,
        compares.length ? el("div", { class: "mrt-compares" }, el("h3", { class: "mrt-h" }, "Against the sign-ups"), compares)
          : el("p", { class: "muted small" }, "Pick a raid for the bosses to compare with its sign-ups."),
        el("h3", { class: "mrt-h" }, "Who was there"),
        grid));
    foot.replaceChildren(
      el("button", { type: "button", class: "ghost", onclick: () => { preview = null; body.replaceChildren(); foot.replaceChildren(); paste.hidden = false; text.focus(); } }, "← Paste again"),
      el("span", { class: "gi-gap" }),
      doImport);
    paste.hidden = true;
  };
  const showDone = (res) => {
    const bits = [`Imported ${plural(res.bosses, "boss", "bosses")}`];
    if (res.bosses) bits.push(`${res.players} there`);
    if (res.duplicates) bits.push(`${res.duplicates} already in`);
    body.replaceChildren(el("div", { class: "gi-done" },
      el("p", { class: "gi-done-head" }, res.bosses ? "✓ " : "", bits.join(" · "), "."),
      res.import_id ? el("p", { class: "muted small" }, "Imported by mistake? You can undo the whole import for 24 hours (here, or under Loot & attendance → Imports).") : el("p", { class: "muted small" }, "Nothing new: those bosses were already in.")));
    foot.replaceChildren(
      res.import_id ? el("button", { type: "button", class: "ghost danger-btn", "data-act": "undo", onclick: async () => {
        if (!(await confirmBox("Undo this import?", `The ${plural(res.bosses, "boss", "bosses")} it added come out of the raid's attendance again.`, "Undo import"))) return;
        const ok = await tryApi("DELETE", `/api/attendance/imports/${res.import_id}`).then(() => true, () => false);
        if (ok) { toast("Import undone."); onDone?.(); dialog.close(); }
      } }, "Undo import") : null,
      el("span", { class: "gi-gap" }),
      el("button", { type: "button", class: "primary", onclick: () => dialog.close() }, "Done"));
    toast(`${bits.join(" · ")}.`);
    onDone?.();
  };
  dialog.append(
    el("div", { class: "att-head" },
      el("div", {}, el("h2", {}, "Import attendance (MRT)"),
        el("div", { class: "muted small" }, ev ? `${ev.title} · ${eventWhen(ev).text}` : `${currentGuild()?.name ?? ""} · bosses go to raids by date and time`)),
      el("div", { class: "att-head-tools" }, closeX(dialog))),
    paste, body, foot);
  setTimeout(() => text.focus(), 50);
  return dialog;
}
window.openMrtImport = openMrtImport;

// ======================================================================
// The Attendance section on raid cards: imported bosses and hand marks,
// and (raid leaders, once the raid has started) Edit attendance
// ======================================================================

// cache: ev.id → {sig, data}; open: sections folded out; edit: ev.id → the
// edit in progress (kept while the card redraws).
const raidAtt = { cache: new Map(), open: new Set(), pending: new Map(), edit: new Map() };

const attSig = (ev) => `${ev.attendance_bosses ?? 0}:${ev.attendance_manual ?? 0}:${ev.attendance_edited_at ?? ""}`;
const hasAttendance = (ev) => !!(ev.attendance_bosses || ev.attendance_manual);

function loadRaidAttendance(ev, { force = false } = {}) {
  const sig = attSig(ev);
  const hit = raidAtt.cache.get(ev.id);
  if (hit?.sig === sig && !force) return hit.data;
  if (raidAtt.pending.get(ev.id) !== sig || force) {
    raidAtt.pending.set(ev.id, sig);
    api("GET", `/api/events/${ev.id}/attendance/mrt`).then((data) => {
      raidAtt.cache.set(ev.id, { sig, data });
      refreshAttendance(ev.id);
    }).catch(() => {}).finally(() => { if (raidAtt.pending.get(ev.id) === sig) raidAtt.pending.delete(ev.id); });
  }
  return hit?.data ?? null;
}

const refreshAttendance = (id) => { for (const node of document.querySelectorAll(`details.ev-attendance[data-event="${id}"]`)) node.attFill?.(); };

const pullsOf = (data) => data.pulls.map((p) => ({ ...p, boss_name: p.encounter_name || p.boss_name, local: p.pulled_local }));
const playersOf = (data) => data.players.map((p) => ({ name: p.character_name, key: p.character_key, user_id: p.user_id, class: p.character_class, display_name: p.display_name, present: p.present }));

// Raid leaders: open the raid's Attendance section in edit mode (the card's
// "Take attendance" link and ⋯ menu). `card`: the raid card it came from.
function openAttendanceEdit(ev, card = null) {
  raidAtt.open.add(ev.id);
  if (!raidAtt.edit.has(ev.id)) raidAtt.edit.set(ev.id, null);
  loadRaidAttendance(ev);
  const nodes = [...document.querySelectorAll(`details.ev-attendance[data-event="${ev.id}"]`)];
  for (const node of nodes) { node.open = true; node.attFill?.(); }
  const mine = card?.querySelector("details.ev-attendance") ?? nodes[0];
  mine?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  mine?.querySelector("summary")?.focus({ preventScroll: true });
}
window.openAttendanceEdit = openAttendanceEdit;

// Starts an edit from the raid's attendance (once it has loaded).
function editState(ev, data) {
  let ed = raidAtt.edit.get(ev.id);
  if (ed === null && data) {
    const original = new Map(data.manual.map((m) => [m.user_id, m.status]));
    ed = { original, marks: new Map(original), added: new Map(), query: "", saving: false };
    raidAtt.edit.set(ev.id, ed);
  }
  return ed ?? null;
}

// What a member's raid counts as, with this mark (or none) — the same rule
// as the server's (migration 0047).
function attOutcome(mark, there, signup) {
  if (mark) {
    if (ATTENDED_MARKS.has(mark)) return "attended";
    if (mark === "excused") return "excused";
  } else if (there) return "attended";
  return NO_SHOW_STATUSES.has(signup?.status) ? "no_show" : "none";
}
const OUTCOME_TEXT = { attended: "Counts as attended", excused: "Excused: not counted", no_show: "Counts as a no-show", none: "Not counted" };

function attendanceEditor(ev, data, ed) {
  const pulls = pullsOf(data);
  const players = playersOf(data);
  const nameOf = (id, fallback) => memberById(id)?.display_name ?? fallback ?? "Someone";
  const people = new Map();
  for (const s of ev.signups) {
    if (s.status !== "no" || ed.marks.has(s.user_id) || ed.original.has(s.user_id)) {
      people.set(s.user_id, { id: s.user_id, name: signupLabel(s), person: signupPerson(s), signup: s, cls: s.class });
    }
  }
  for (const p of players) {
    if (p.user_id && !people.has(p.user_id)) people.set(p.user_id, { id: p.user_id, name: p.name, person: nameOf(p.user_id, p.display_name), signup: null, cls: p.class });
  }
  for (const m of data.manual) if (!people.has(m.user_id)) people.set(m.user_id, { id: m.user_id, name: nameOf(m.user_id, m.display_name), signup: null });
  for (const [id, name] of ed.added) if (!people.has(id)) people.set(id, { id, name, signup: null });
  const bosses = (id) => players.filter((p) => p.user_id === id).reduce((n, p) => Math.max(n, p.present.filter(Boolean).length), 0);
  const rank = (s) => !s ? 5 : s.status === "yes" || s.status === "late" ? 0 : s.status === "bench" ? 1 : s.status === "tentative" ? 2 : 4;
  const roleRank = { tank: 0, healer: 1, dps: 2 };
  const sorted = [...people.values()].sort((a, b) => rank(a.signup) - rank(b.signup)
    || (roleRank[a.signup?.role] ?? 3) - (roleRank[b.signup?.role] ?? 3) || a.name.localeCompare(b.name));
  const redraw = () => refreshAttendance(ev.id);

  const rows = sorted.map((p) => {
    const mark = ed.marks.get(p.id) ?? "";
    const s = p.signup;
    const seen = pulls.length ? bosses(p.id) : 0;
    const outcome = attOutcome(mark, seen > 0, s);
    const colour = classColour(s?.class ?? p.cls);
    const bits = [
      s ? STATUS_LABEL[s.status] : "Didn't sign up",
      pulls.length ? (seen ? `MRT: ${seen}/${pulls.length} ${pulls.length === 1 ? "boss" : "bosses"}` : "MRT: not there") : null,
    ].filter(Boolean);
    return el("div", { class: `att-row${mark ? ` marked ${mark}` : ""} o-${outcome}`, "data-user": p.id },
      el("div", { class: "att-who" },
        el("span", { class: "att-name", style: colour ? `color:${colour}` : "", title: p.person && p.person !== p.name ? p.person : "" }, p.name),
        el("span", { class: "muted small" }, bits.join(" · ")),
        el("span", { class: `att-outcome small o-${outcome}` }, `${OUTCOME_TEXT[outcome]}${mark ? "" : pulls.length && seen ? " (from MRT)" : ""}`)),
      el("div", { class: "att-choices", role: "group", "aria-label": `${p.name}'s attendance` },
        ATT_MARKS.map((k) => el("button", {
          type: "button", class: `att-choice ${k}${mark === k ? " on" : ""}`, "aria-pressed": mark === k ? "true" : "false",
          title: mark === k ? `${ATT_MARK_LABEL[k]} (click again to clear${pulls.length ? " and go by MRT" : ""})` : ATT_MARK_LABEL[k],
          onclick: () => { if (ed.marks.get(p.id) === k) ed.marks.delete(p.id); else ed.marks.set(p.id, k); redraw(); },
        }, el("span", { class: "att-ic", "aria-hidden": "true" }, ATT_MARK_ICON[k]), el("span", { class: "att-lbl" }, ATT_MARK_LABEL[k])))));
  });

  const changes = () => [...new Set([...ed.marks.keys(), ...ed.original.keys()])]
    .filter((id) => ed.marks.get(id) !== ed.original.get(id))
    .map((id) => ({ user_id: id, status: ed.marks.get(id) ?? "" }));
  const counts = ATT_MARKS.map((k) => [k, [...ed.marks.values()].filter((v) => v === k).length]).filter(([, n]) => n)
    .map(([k, n]) => `${n} ${ATT_MARK_LABEL[k].toLowerCase()}`);
  const outcomes = sorted.map((p) => attOutcome(ed.marks.get(p.id), pulls.length && bosses(p.id) > 0, p.signup));
  const summary = el("div", { class: "att-summary muted small" },
    [`Listed here: ${outcomes.filter((o) => o === "attended").length} attended`, plural(outcomes.filter((o) => o === "no_show").length, "no-show"),
      counts.length ? `marked by hand: ${counts.join(", ")}` : "nobody marked by hand"].join(" · "));

  const allPresent = el("button", { type: "button", class: "ghost small-btn att-all", onclick: () => {
    for (const p of people.values()) {
      if (ed.marks.has(p.id)) continue;
      const st = p.signup?.status;
      if (st === "yes" || st === "late") ed.marks.set(p.id, st === "late" ? "late" : "present");
      else if (st === "bench") ed.marks.set(p.id, "benched");
    }
    redraw();
  } }, "✓ Mark everyone signed up as present");

  // Add someone who isn't listed (didn't sign up, MRT didn't see them).
  const search = el("input", { type: "search", class: "att-search", placeholder: "Add someone else…", "aria-label": "Add someone who isn't listed", value: ed.query, autocomplete: "off" });
  const found = el("div", { class: "att-found" });
  const drawFound = () => {
    const q = ed.query.trim().toLowerCase();
    const hits = q ? state.members.filter((m) => !m.bot && !people.has(m.id) && m.display_name.toLowerCase().includes(q))
      .sort((a, b) => a.display_name.localeCompare(b.display_name)).slice(0, 8) : [];
    found.replaceChildren(...hits.map((m) => el("button", { type: "button", class: "att-add", "data-user": m.id, onclick: () => {
      ed.added.set(m.id, m.display_name);
      ed.marks.set(m.id, "present");
      ed.query = "";
      redraw();
    } }, avatarFor(m.id, m.display_name, 18), m.display_name, el("span", { class: "muted small" }, " ＋ present"))),
    q && !hits.length ? el("span", { class: "muted small" }, "Nobody else by that name.") : null);
  };
  search.addEventListener("input", () => { ed.query = search.value; drawFound(); });
  drawFound();

  const n = changes().length;
  const save = el("button", { type: "button", class: "primary", "data-act": "save-attendance", disabled: ed.saving || !n, onclick: async () => {
    const records = changes();
    if (!records.length) return;
    ed.saving = true;
    redraw();
    const reply = await tryApi("PUT", `/api/events/${ev.id}/attendance`, { records }).catch(() => null);
    ed.saving = false;
    if (!reply) return redraw();
    raidAtt.edit.delete(ev.id);
    const hit = raidAtt.cache.get(ev.id);
    if (hit) hit.data = { ...hit.data, manual: reply };
    // The no-show hints on other raids count this raid too.
    if (typeof reliability !== "undefined") reliability.cache.clear();
    toast("Attendance saved.");
    redraw();
    loadRaidAttendance(ev, { force: true });
  } }, ed.saving ? "Saving…" : n ? `Save attendance (${n})` : "Save attendance");
  const cancel = el("button", { type: "button", class: "ghost", onclick: () => { raidAtt.edit.delete(ev.id); redraw(); } }, n ? "Discard changes" : "Done");

  return el("div", { class: "att-editor" },
    el("p", { class: "muted small att-help" }, pulls.length
      ? "A mark overrides MRT for that person on this raid (say they joined after the last boss). Leave someone unmarked to go by MRT."
      : "Mark who came. Present, late and benched count as attended; excused doesn't count either way; absent is a no-show if they signed up."),
    el("div", { class: "att-tools" }, allPresent, summary),
    el("div", { class: "att-list" }, rows.length ? rows : el("p", { class: "muted small" }, "Nobody signed up. Add people below.")),
    el("div", { class: "att-addbox" }, search, found),
    el("div", { class: "row att-save" }, cancel, save));
}

function raidAttendanceSection(ev) {
  const manager = canManageEvent(ev);
  const started = eventWhen(ev).started;
  if (!hasAttendance(ev) && !(manager && started)) return null;
  const details = el("details", { class: "ev-lootlog ev-attendance", "data-event": ev.id });
  if (raidAtt.open.has(ev.id)) details.open = true;
  const sum = el("span", { class: "ev-comp-sum" }, hasAttendance(ev) ? "" : "Not taken yet");
  const body = el("div", { class: "ev-lootlog-body" });
  details.append(el("summary", {}, el("span", { class: "ev-comp-title" }, "📋 Attendance"), sum), body);
  details.attFill = () => {
    const data = raidAtt.cache.get(ev.id)?.data ?? null;
    const ed = editState(ev, data);
    if (ed) sum.textContent = "Editing…";
    else if (data && (data.pulls.length || data.manual.length)) sum.textContent = compareLine(mrtCompare(pullsOf(data), playersOf(data), ev.signups, data.manual));
    else if (data || !hasAttendance(ev)) sum.textContent = "Not taken yet";
    if (!details.open) return;
    if (!data) return body.replaceChildren(el("p", { class: "muted small" }, "Loading…"));
    if (ed && data.can_edit) return body.replaceChildren(attendanceEditor(ev, data, ed));
    const pulls = pullsOf(data);
    const players = playersOf(data);
    const bossList = pulls.length ? el("div", { class: "mrt-bosses" }, pulls.map((b, i) => {
      const here = players.filter((p) => p.present[i]);
      return el("span", { class: "mrt-boss", title: `${here.length} there:\n${here.map((p) => p.name).join(", ")}` },
        el("strong", {}, b.boss_name), el("span", { class: "muted small" }, ` ${mrtTime(b.local)} · ${here.length}`),
        b.difficulty != null ? el("span", { class: "mrt-diff" }, ` ${({ 3: "10", 4: "25", 9: "40", 148: "20" })[b.difficulty] ?? b.difficulty}`) : null);
    })) : null;
    const lastEdit = [...data.manual].sort((a, b) => String(b.marked_at).localeCompare(String(a.marked_at)))[0];
    const manualNote = data.manual.length ? el("div", { class: "muted small att-manual-note", title: lastEdit?.marked_by_name ? `Last changed by ${lastEdit.marked_by_name}` : "" },
      `✎ ${plural(data.manual.length, "person", "people")} marked by hand${pulls.length ? " (these override MRT)" : ""}`) : null;
    const empty = !pulls.length && !data.manual.length;
    const editBtn = data.can_edit ? el("button", { type: "button", class: `${empty ? "primary" : "ghost"} small-btn att-edit`, "data-act": "edit-attendance", onclick: () => openAttendanceEdit(ev, details.closest(".event-card")) },
      empty ? "Take attendance" : "✎ Edit attendance") : null;
    body.replaceChildren(
      empty ? el("p", { class: "muted small" }, data.can_edit ? "No attendance yet. Mark who came, or import Method Raid Tools' export." : "No attendance yet.") : null,
      bossList,
      empty ? null : mrtCompareView(pulls, players, ev.signups, data.manual).node,
      manualNote,
      el("div", { class: "rl-foot" },
        editBtn,
        el("button", { type: "button", class: "link small", onclick: () => openLootLog({ tab: "people" }) }, "📜 Attendance by character"),
        manager && typeof openMrtImport === "function" ? el("button", { type: "button", class: "link small", onclick: () => openMrtImport(ev) }, pulls.length ? "Import more from MRT…" : "Import from MRT…") : null));
  };
  details.addEventListener("toggle", () => {
    if (details.open) { raidAtt.open.add(ev.id); loadRaidAttendance(ev); } else raidAtt.open.delete(ev.id);
    details.attFill();
  });
  if (hasAttendance(ev) || details.open) loadRaidAttendance(ev);
  details.attFill();
  return details;
}
window.raidAttendanceSection = raidAttendanceSection;

// ======================================================================
// The no-show hint on sign-ups (raid leaders only)
// ======================================================================

const reliability = { cache: new Map(), pending: new Set() };
const RELIABILITY_TTL = 5 * 60 * 1000;

function loadReliability(ev) {
  const hit = reliability.cache.get(ev.id);
  if (hit && Date.now() - hit.at < RELIABILITY_TTL) return hit.data;
  if (!reliability.pending.has(ev.id)) {
    reliability.pending.add(ev.id);
    api("GET", `/api/events/${ev.id}/attendance/reliability`).then((data) => {
      reliability.cache.set(ev.id, { at: Date.now(), data });
      for (const node of document.querySelectorAll(`.rc-rely[data-event="${ev.id}"]`)) node.relyFill?.();
    }).catch(() => reliability.cache.set(ev.id, { at: Date.now(), data: null })).finally(() => reliability.pending.delete(ev.id));
  }
  return hit?.data ?? null;
}

// Next to a sign-up on the roster: "2 no-shows in 4 weeks" (hidden until it
// applies; only raid leaders ever ask).
function reliabilityTag(ev, s) {
  if (!canManageEvent(ev) || s.status === "no") return null;
  const tag = el("span", { class: "rc-rely", "data-event": ev.id, "data-user": s.user_id, hidden: true });
  tag.relyFill = () => {
    const data = reliability.cache.get(ev.id)?.data;
    const me = data?.people?.find((p) => p.user_id === s.user_id);
    tag.hidden = !me;
    if (!me) return;
    const weeks = Math.round(data.days / 7);
    tag.replaceChildren(el("span", { class: "rc-rely-long" }, `${me.no_shows} no-shows`), el("span", { class: "rc-rely-short", "aria-hidden": "true" }, `${me.no_shows}✗`));
    tag.setAttribute("aria-label", `${me.no_shows} no-shows in ${weeks} weeks`);
    tag.title = `${me.no_shows} no-shows in the last ${weeks} weeks (signed up to ${me.signed_up} raids with attendance). Only raid leaders see this.`;
  };
  loadReliability(ev);
  tag.relyFill();
  return tag;
}
window.reliabilityTag = reliabilityTag;

// ======================================================================
// Full profile page: attendance per character
// ======================================================================

function charAttendanceBlock(c, guildName) {
  const r = c.recent, a = c.all_time;
  const colour = classColour(c.character_class);
  const pct = (x) => (x.eligible ? `${x.percent}%` : "—");
  return el("div", { class: "fpa-char", "data-character": c.character_key, style: colour ? `--cc:${colour}` : "" },
    el("strong", { class: "fpl-name" }, c.character_name),
    el("span", { class: "fpa-pct", title: "Raids attended out of raids they attended or signed up to and missed (raids with attendance, from MRT or taken by hand)" },
      `${pct(r)} `, el("span", { class: "muted small" }, `attended ${r.attended} of ${r.eligible} in the last 8 weeks`)),
    el("span", { class: "muted small" }, [
      r.last_attended ? `last raid ${lootDay(r.last_attended, { day: "numeric", month: "short" })}` : a.last_attended ? `last raid ${lootDay(a.last_attended, { day: "numeric", month: "short", year: "numeric" })}` : null,
      r.no_shows ? plural(r.no_shows, "no-show") : null,
      `${a.attended} of ${a.eligible} in all`, guildName].filter(Boolean).join(" · ")));
}

async function addProfileAttendance(page, p) {
  if (!["full", "limited"].includes(p.level)) return;
  const data = await api("GET", `/api/users/${p.id}/attendance`).catch(() => null);
  if (!data?.length || !page.isConnected) return;
  const several = data.length > 1;
  const blocks = data.flatMap((g) => g.characters.map((c) => ({ c, g })));
  const cards = [...page.querySelectorAll(".fp-chars .fp-char")];
  (p.characters ?? []).forEach((ch, i) => {
    const keys = [lootKey(ch.name), lootKey(`${ch.name} ${ch.last_name || ""}`)];
    const mine = blocks.find(({ c }) => keys.includes(c.character_key));
    if (!mine || !cards[i]) return;
    const r = mine.c.recent;
    cards[i].querySelector(".fp-char-text")?.append(el("span", { class: "small fp-char-att", title: "Raids attended in the last 8 weeks (from MRT or taken by hand)" },
      r.eligible ? `📋 ${r.percent}% attendance (${r.attended}/${r.eligible})` : `📋 ${mine.c.all_time.attended} raids attended`));
  });
  const section = el("div", { class: "fp-attendance", id: "fp-attendance" },
    el("h4", {}, "RAID ATTENDANCE"),
    blocks.map(({ c, g }) => charAttendanceBlock(c, several ? g.guild_name : null)));
  const wow = page.querySelector("#fp-wow");
  if (wow) {
    const loot = wow.querySelector("#fp-loot");
    if (loot) loot.before(section); else wow.append(section);
  } else {
    const s = profileSection("attendance", "Raid attendance", section);
    section.querySelector("h4")?.remove();
    const notice = page.querySelector(".fp-notice");
    if (notice) notice.before(s); else page.append(s);
  }
}

if (typeof drawProfileOverview === "function") {
  const mrtBaseOverview = drawProfileOverview;
  drawProfileOverview = function (page, p, close) {
    const out = mrtBaseOverview(page, p, close);
    addProfileAttendance(page, p);
    return out;
  };
}
