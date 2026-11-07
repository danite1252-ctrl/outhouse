// Outhouse: raid calendar, attendance (taking it, the outhouse table, and
// on profile cards) and live raid updates. Loaded after features.js and
// uses its raid helpers (renderEventCard, openRaidDialog, eventWhen, …).

const ATT_LABEL = { present: "Present", late: "Late", absent: "Absent", benched: "Benched" };
const ATT_ICON = { present: "✓", late: "⏱", absent: "✗", benched: "🪑" };
const MONTHS_FMT = { month: "long", year: "numeric" };
const myTimeZone = () => Intl.DateTimeFormat().resolvedOptions().timeZone || "your time zone";

// ---------- Popovers, menus and toasts inside raid windows ----------
// Raid windows are modal dialogs, which sit above everything else on the
// page; anything popping up from them has to live inside them to be seen.
const raidHost = () => [...document.querySelectorAll("dialog.raid-host[open]")].pop() ?? null;

const raidsBasePlaceFixed = placeFixed;
placeFixed = function (node, x, y) {
  const host = raidHost();
  if (!host) return raidsBasePlaceFixed(node, x, y);
  host.append(node);
  const r = node.getBoundingClientRect();
  node.style.left = `${Math.max(8, Math.min(x, window.innerWidth - r.width - 8))}px`;
  node.style.top = `${Math.max(8, Math.min(y, window.innerHeight - r.height - 8))}px`;
};

const raidsBaseToast = toast;
toast = function (text) {
  const t = $("toast");
  const host = raidHost();
  if (host && t.parentNode !== host) host.append(t);
  else if (!host && t.parentNode !== document.body) document.body.append(t);
  raidsBaseToast(text);
};

function raidWindow(className, onClose) {
  const dialog = el("dialog", { class: `raid-host ${className}` });
  dialog.addEventListener("close", () => {
    const t = $("toast");
    if (dialog.contains(t)) document.body.append(t);
    closePopover();
    closeMenus();
    dialog.remove();
    onClose?.();
  });
  document.body.append(dialog);
  dialog.showModal();
  return dialog;
}

const closeX = (dialog) => el("button", { type: "button", class: "icon-btn raid-close", title: "Close", "aria-label": "Close", onclick: () => dialog.close() }, "✕");

// ---------- One raid in a window (for raids whose card isn't in chat yet) ----------

const raidCard = { dialog: null, ev: null, body: null };

function openRaidCard(ev) {
  raidCard.ev = ev;
  const dialog = raidWindow("raid-card-dialog", () => { if (raidCard.dialog === dialog) raidCard.dialog = null; });
  raidCard.dialog = dialog;
  raidCard.body = el("div", { class: "raid-card-body" });
  dialog.append(el("div", { class: "raid-card-head" },
    el("span", { class: "muted small" }, signupsNotOpen(ev) ? `📬 ${opensText(ev)} (the sign-up post goes up in chat then).` : ""), closeX(dialog)), raidCard.body);
  drawRaidCard();
}

function drawRaidCard() {
  if (!raidCard.dialog) return;
  raidCard.body.replaceChildren(renderEventCard({ event: raidCard.ev }));
}

// ---------- Calendar ----------

const cal = { dialog: null, root: null, month: null, selected: null, events: [], from: null, to: null, open: null, loading: false };

const dayKey = (d) => localDateValue(d);
const keyDate = (key) => { const [y, m, d] = key.split("-").map(Number); return new Date(y, m - 1, d); };
const startOfGrid = (month) => { const d = new Date(month); d.setDate(1 - weekdayOf(d)); d.setHours(0, 0, 0, 0); return d; };
const isPhoneWidth = () => matchMedia("(max-width: 760px)").matches;
const timeOf = (iso) => new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

function openCalendar(day = new Date()) {
  if (!state.guildId) return;
  cal.dialog?.close();
  cal.month = new Date(day.getFullYear(), day.getMonth(), 1);
  cal.selected = dayKey(day);
  cal.open = null;
  cal.events = [];
  const dialog = raidWindow("calendar-dialog", () => { if (cal.dialog === dialog) cal.dialog = null; });
  cal.dialog = dialog;
  cal.root = el("div", { class: "cal" });
  dialog.append(cal.root);
  loadCalendar();
}

async function loadCalendar() {
  const from = startOfGrid(cal.month);
  const to = new Date(from); to.setDate(to.getDate() + 42);
  cal.from = from; cal.to = to;
  cal.loading = true;
  drawCalendar();
  const guild = state.guildId;
  const events = await tryApi("GET", `/api/guilds/${guild}/events?from=${encodeURIComponent(from.toISOString())}&to=${encodeURIComponent(to.toISOString())}`).catch(() => []);
  if (cal.from !== from || guild !== state.guildId) return; // moved on meanwhile
  cal.events = events;
  cal.loading = false;
  drawCalendar();
}

function moveMonth(delta) {
  cal.month = new Date(cal.month.getFullYear(), cal.month.getMonth() + delta, 1);
  const today = new Date();
  cal.selected = dayKey(cal.month.getMonth() === today.getMonth() && cal.month.getFullYear() === today.getFullYear() ? today : cal.month);
  cal.open = null;
  loadCalendar();
}

function eventsByDay() {
  const map = new Map();
  for (const ev of [...cal.events].sort((a, b) => new Date(a.starts_at) - new Date(b.starts_at))) {
    const key = dayKey(new Date(ev.starts_at));
    if (!map.has(key)) map.set(key, []);
    map.get(key).push(ev);
  }
  return map;
}

// The plan dialog asks which channel to post in, so any channel you can post raids in will do.
const canPlanHere = () => raidChannels().length > 0;

function planOn(key) {
  const date = keyDate(key);
  const today = new Date(); today.setHours(0, 0, 0, 0);
  if (date < today) return toast("That day has been and gone. Pick today or later.");
  openRaidDialog(null, { date });
}

function myMark(ev) {
  const mine = ev.signups.find((s) => s.user_id === state.me.id);
  return mine && mine.status !== "no" ? mine : null;
}

function drawCalendar() {
  if (!cal.dialog) return;
  const scroller = cal.root.querySelector(".cal-scroll");
  const keep = scroller?.scrollTop ?? 0;
  const byDay = eventsByDay();
  const todayKey = dayKey(new Date());
  const head = el("div", { class: "cal-head" },
    el("div", { class: "cal-nav" },
      el("button", { type: "button", class: "icon-btn", title: "Previous month", "aria-label": "Previous month", onclick: () => moveMonth(-1) }, "‹"),
      el("h2", {}, cal.month.toLocaleDateString([], MONTHS_FMT)),
      el("button", { type: "button", class: "icon-btn", title: "Next month", "aria-label": "Next month", onclick: () => moveMonth(1) }, "›"),
      el("button", { type: "button", class: "cal-today", onclick: () => { cal.month = new Date(new Date().getFullYear(), new Date().getMonth(), 1); cal.selected = todayKey; loadCalendar(); } }, "Today")),
    el("div", { class: "cal-actions" },
      el("button", { type: "button", class: "cal-today", onclick: () => openAttendanceTable() }, "📊 Attendance"),
      canPlanHere() ? el("button", { type: "button", class: "primary cal-plan", title: "Plan a raid", onclick: () => planOn(cal.selected) }, "＋ Plan", el("span", { class: "plan-long" }, " a raid")) : null,
      closeX(cal.dialog)));
  const pendingNote = !isPhoneWidth() && cal.events.some(signupsNotOpen) ? " Blue raids: sign-ups aren't open yet." : "";
  const tz = el("div", { class: "cal-tz muted small" }, `Times are in your time zone (${myTimeZone()}).`, pendingNote, cal.loading ? " Loading…" : "");
  const body = isPhoneWidth() ? drawAgenda(byDay, todayKey) : drawMonth(byDay, todayKey);
  cal.root.replaceChildren(head, tz, body);
  const s = cal.root.querySelector(".cal-scroll");
  if (s) s.scrollTop = keep;
}

function drawMonth(byDay, todayKey) {
  const first = startOfGrid(cal.month);
  const cells = [];
  for (let i = 0; i < 42; i++) {
    const d = new Date(first); d.setDate(first.getDate() + i);
    const key = dayKey(d);
    const evs = byDay.get(key) ?? [];
    const other = d.getMonth() !== cal.month.getMonth();
    cells.push(el("button", {
      type: "button",
      class: `cal-day${other ? " other" : ""}${key === todayKey ? " today" : ""}${key === cal.selected ? " selected" : ""}${evs.length ? " busy" : ""}`,
      "data-day": key,
      "aria-label": `${d.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" })}${evs.length ? `, ${evs.length} raid${evs.length === 1 ? "" : "s"}` : ""}`,
      onclick: () => { cal.selected = key; cal.open = null; drawCalendar(); },
      ondblclick: () => canPlanHere() && planOn(key),
    },
      el("span", { class: "cal-num" }, d.getDate()),
      evs.slice(0, 3).map((ev) => el("span", { class: `cal-chip${myMark(ev) ? " mine" : ""}${eventWhen(ev).over ? " over" : ""}${signupsNotOpen(ev) ? " pending" : ""}`, title: `${timeOf(ev.starts_at)} ${ev.title}${signupsNotOpen(ev) ? ` · ${opensText(ev)}` : ""}` },
        el("span", { class: "cal-chip-time" }, timeOf(ev.starts_at)), ` ${ev.title}`)),
      evs.length > 3 ? el("span", { class: "cal-more" }, `+${evs.length - 3} more`) : null));
  }
  const grid = el("div", { class: "cal-grid" },
    WEEKDAYS.map((n) => el("div", { class: "cal-wd" }, n)),
    cells);
  const sel = keyDate(cal.selected);
  const evs = byDay.get(cal.selected) ?? [];
  const panel = el("div", { class: "cal-daypanel cal-scroll" },
    el("h3", {}, sel.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" })),
    evs.length ? evs.map((ev) => renderEventCard({ event: ev })) : el("p", { class: "muted small" }, "No raids this day."),
    canPlanHere() ? el("button", { type: "button", class: "cal-plan-day", onclick: () => planOn(cal.selected) }, "＋ Plan a raid on this day") : null);
  return el("div", { class: "cal-wrap" }, grid, panel);
}

// Phones: a list of the month's raid days instead of a grid.
function drawAgenda(byDay, todayKey) {
  const keys = [...byDay.keys()].filter((k) => keyDate(k).getMonth() === cal.month.getMonth()).sort();
  const days = keys.map((key) => {
    const d = keyDate(key);
    return el("section", { class: `cal-agenda-day${key === todayKey ? " today" : ""}`, "data-day": key },
      el("h3", {}, d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" }), key === todayKey ? el("span", { class: "cal-today-tag" }, "Today") : null),
      byDay.get(key).map((ev) => {
        const mine = myMark(ev);
        const active = ev.signups.filter((s) => s.status === "yes" || s.status === "late").length;
        const open = cal.open === ev.id;
        return el("div", { class: `cal-agenda-item${open ? " open" : ""}` },
          el("button", { type: "button", class: "cal-agenda-row", onclick: () => { cal.open = open ? null : ev.id; drawCalendar(); } },
            el("span", { class: "cal-agenda-time" }, timeOf(ev.starts_at)),
            el("span", { class: "cal-agenda-title" }, ev.title,
              ev.series_id ? el("span", { class: "rl-tag" }, "🔁") : null,
              signupsLocked(ev) ? el("span", { class: "rl-tag" }, "🔒") : null,
              signupsNotOpen(ev) ? el("span", { class: "cal-opens muted small" }, `📬 ${opensText(ev)}`) : null),
            el("span", { class: "cal-agenda-count muted small" }, `${active}/${ev.max_tanks + ev.max_healers + ev.max_dps}`),
            mine ? el("span", { class: "rl-mine" }, `✓ ${STATUS_LABEL[mine.status]}`) : null),
          open ? renderEventCard({ event: ev }) : null);
      }));
  });
  return el("div", { class: "cal-agenda cal-scroll" },
    days.length ? days : el("p", { class: "muted small cal-empty" }, cal.loading ? "Loading…" : `No raids in ${cal.month.toLocaleDateString([], { month: "long" })}.`),
    canPlanHere() ? el("button", { type: "button", class: "cal-plan-day", onclick: () => planOn(cal.selected) }, "＋ Plan a raid") : null);
}

// ---------- Live updates ----------

function onEventChanged(ev) {
  if (cal.dialog && ev.guild_id === state.guildId) {
    const i = cal.events.findIndex((x) => x.id === ev.id);
    const start = new Date(ev.starts_at);
    const inRange = cal.from && start >= cal.from && start < cal.to;
    if (i >= 0 && inRange) cal.events[i] = ev;
    else if (i >= 0) cal.events.splice(i, 1);
    else if (inRange) cal.events.push(ev);
    clearTimeout(onEventChanged.timer);
    onEventChanged.timer = setTimeout(drawCalendar, 50);
  }
  if (raidCard.dialog && raidCard.ev.id === ev.id) { raidCard.ev = ev; drawRaidCard(); }
}

function onEventDeleted(id) {
  if (cal.dialog) {
    cal.events = cal.events.filter((x) => x.id !== id);
    drawCalendar();
  }
  if (raidCard.dialog && raidCard.ev.id === id) raidCard.dialog.close();
}

const raidsBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  switch (t) {
    case "EVENT_UPDATE":
      onEventChanged(d);
      break;
    case "EVENT_DELETE":
      onEventDeleted(d.id);
      break;
    case "RAID_PROMOTED":
      toast(`🎉 You're in! A spot opened up in ${d.title}, so you're off the bench.`);
      if (typeof desktop !== "undefined" && desktop && !document.hasFocus()) {
        desktopInvoke("notify", { title: "You're off the bench", body: `A spot opened up in ${d.title}.` });
      }
      break;
  }
  raidsBaseHandle(ev);
};

// ---------- Taking attendance ----------

async function openAttendanceDialog(ev) {
  const existing = await tryApi("GET", `/api/events/${ev.id}/attendance`).catch(() => null);
  if (!existing) return;
  const marks = new Map(existing.map((r) => [r.user_id, r.status]));
  const original = new Map(marks);
  const people = new Map();
  const rank = (s) => !s ? 5 : s.status === "yes" || s.status === "late" ? 0 : s.status === "bench" ? 1 : s.status === "tentative" ? 2 : 4;
  const roleRank = { tank: 0, healer: 1, dps: 2 };
  for (const s of ev.signups) if (s.status !== "no") people.set(s.user_id, { id: s.user_id, name: memberById(s.user_id)?.display_name ?? s.display_name, signup: s });
  for (const r of existing) if (!people.has(r.user_id)) people.set(r.user_id, { id: r.user_id, name: memberById(r.user_id)?.display_name ?? r.display_name, signup: null });

  const dialog = raidWindow("attendance-dialog");
  const list = el("div", { class: "att-list" });
  const summary = el("div", { class: "att-summary muted small" });
  const add = el("select", { class: "select", "aria-label": "Add someone" });

  const draw = () => {
    const sorted = [...people.values()].sort((a, b) => rank(a.signup) - rank(b.signup)
      || (roleRank[a.signup?.role] ?? 3) - (roleRank[b.signup?.role] ?? 3) || a.name.localeCompare(b.name));
    list.replaceChildren(...(sorted.length ? sorted.map((p) => {
      const s = p.signup;
      const colour = classColour(s?.class);
      const mark = marks.get(p.id) ?? "";
      return el("div", { class: `att-row${mark ? ` marked ${mark}` : ""}`, "data-user": p.id },
        el("div", { class: "att-who" },
          el("span", { class: "att-name", style: colour ? `color:${colour}` : "" }, p.name),
          el("span", { class: "muted small" }, s ? `${ROLE_INFO[s.role]?.icon ?? ""} ${s.spec ? `${s.spec} ${s.class}` : s.class || ""} · ${STATUS_LABEL[s.status]}`.trim() : "Didn't sign up")),
        el("div", { class: "att-choices", role: "group", "aria-label": `${p.name}'s attendance` },
          Object.keys(ATT_LABEL).map((k) => el("button", {
            type: "button", class: `att-choice ${k}${mark === k ? " on" : ""}`, title: ATT_LABEL[k], "aria-pressed": mark === k ? "true" : "false",
            onclick: () => { if (marks.get(p.id) === k) marks.delete(p.id); else marks.set(p.id, k); draw(); },
          }, el("span", { class: "att-ic" }, ATT_ICON[k]), el("span", { class: "att-lbl" }, ATT_LABEL[k])))));
    }) : [el("p", { class: "muted small" }, "Nobody signed up. Add people below.")]));
    const counts = Object.keys(ATT_LABEL).map((k) => `${[...marks.values()].filter((v) => v === k).length} ${ATT_LABEL[k].toLowerCase()}`);
    const unmarked = [...people.keys()].filter((id) => !marks.has(id)).length;
    summary.textContent = `${counts.join(" · ")}${unmarked ? ` · ${unmarked} not marked` : ""}`;
    const others = state.members.filter((m) => !m.bot && !people.has(m.id)).sort((a, b) => a.display_name.localeCompare(b.display_name));
    add.replaceChildren(el("option", { value: "" }, "＋ Add someone who didn't sign up…"), ...others.map((m) => el("option", { value: m.id }, m.display_name)));
    add.hidden = !others.length;
  };
  add.addEventListener("change", () => {
    const m = memberById(add.value);
    if (m) { people.set(m.id, { id: m.id, name: m.display_name, signup: null }); marks.set(m.id, "present"); }
    draw();
  });
  const allPresent = el("button", { type: "button", class: "ghost att-all", onclick: () => {
    for (const p of people.values()) {
      const st = p.signup?.status;
      if (st === "yes" || st === "late") marks.set(p.id, "present");
      else if (st === "bench") marks.set(p.id, "benched");
    }
    draw();
  } }, "✓ Mark all signed-up as present");
  const save = el("button", { type: "button", class: "primary", onclick: async () => {
    const records = [];
    for (const id of new Set([...marks.keys(), ...original.keys()])) {
      if (marks.get(id) !== original.get(id)) records.push({ user_id: id, status: marks.get(id) ?? "" });
    }
    if (!records.length) return dialog.close();
    save.disabled = true;
    const ok = await tryApi("PUT", `/api/events/${ev.id}/attendance`, { records }).then(() => true, () => false);
    save.disabled = false;
    if (ok) { dialog.close(); toast("Attendance saved."); }
  } }, "Save attendance");

  dialog.append(
    el("div", { class: "att-head" },
      el("div", {}, el("h2", {}, "Attendance"), el("div", { class: "muted small" }, `${ev.title} · ${eventWhen(ev).text}`)),
      closeX(dialog)),
    el("div", { class: "att-tools" }, allPresent, summary),
    list,
    add,
    el("div", { class: "row" }, el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"), save));
  draw();
}

// ---------- The outhouse's attendance table ----------

async function openAttendanceTable() {
  if (!state.guildId) return;
  const dialog = raidWindow("attendance-table-dialog");
  let limit = prefs.get("attendance_limit", 10);
  let sort = { key: "percent", dir: -1 };
  let data = null;
  const body = el("div", { class: "att-table-wrap" }, el("p", { class: "muted" }, "Loading…"));
  const pick = el("select", { class: "select", "aria-label": "How many raids" },
    [5, 10, 20, 50].map((n) => el("option", { value: n, selected: n === limit }, `Last ${n} raids`)));
  pick.addEventListener("change", () => { limit = Number(pick.value); prefs.set("attendance_limit", limit); load(); });

  const cols = [
    ["name", "Member"], ["percent", "%"], ["present", "✓"], ["late", "⏱"], ["absent", "✗"], ["benched", "🪑"],
  ];
  const colTitle = { name: "Member", percent: "Attendance (present, late or benched)", present: "Present", late: "Late", absent: "Absent", benched: "Benched" };
  const nameOf = (m) => memberById(m.user_id)?.display_name ?? m.display_name;
  const draw = () => {
    if (!data) return;
    if (!data.raids.length) {
      body.replaceChildren(el("div", { class: "att-empty" },
        el("p", {}, "No attendance taken yet."),
        el("p", { class: "muted small" }, "Once a raid starts, its leader can take attendance from the raid card (📋 Take attendance).")));
      return;
    }
    const rows = [...data.members].sort((a, b) => {
      const v = sort.key === "name" ? nameOf(a).localeCompare(nameOf(b)) : (a[sort.key] - b[sort.key]) || (a.total - b.total);
      return v * sort.dir || nameOf(a).localeCompare(nameOf(b));
    });
    const th = ([key, label]) => el("th", {
      class: `sortable${sort.key === key ? ` sorted ${sort.dir > 0 ? "asc" : "desc"}` : ""}${key === "name" ? " name" : ""}`,
      title: `Sort by ${colTitle[key].toLowerCase()}`, "aria-sort": sort.key === key ? (sort.dir > 0 ? "ascending" : "descending") : "none",
      onclick: () => { sort = sort.key === key ? { key, dir: -sort.dir } : { key, dir: key === "name" ? 1 : -1 }; draw(); },
    }, label, sort.key === key ? (sort.dir > 0 ? " ▲" : " ▼") : "");
    const day = (iso) => new Date(iso).toLocaleDateString([], { day: "numeric", month: "short" });
    body.replaceChildren(el("table", { class: "att-table" },
      el("thead", {}, el("tr", {}, cols.map(th),
        data.raids.map((r) => el("th", { class: "raid-col", title: `${r.title} · ${new Date(r.starts_at).toLocaleString([], { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" })}` }, day(r.starts_at))))),
      el("tbody", {}, rows.map((m) => el("tr", {},
        el("td", { class: "name" }, el("button", { type: "button", class: "att-member", onclick: (e) => openProfile(m.user_id, e.currentTarget) },
          avatarFor(m.user_id, nameOf(m), 22), nameOf(m))),
        el("td", { class: "pct" }, el("div", { class: "pct-bar", style: `--pct:${m.percent}%` }, el("span", {}, `${m.percent}%`))),
        el("td", {}, m.present), el("td", {}, m.late), el("td", {}, m.absent), el("td", {}, m.benched),
        m.history.map((h, i) => el("td", { class: "raid-col" }, h
          ? el("span", { class: `att-dot ${h}`, title: `${ATT_LABEL[h]} · ${data.raids[i].title}` })
          : el("span", { class: "att-dot none", title: `Not marked · ${data.raids[i].title}` }))))))));
  };
  const load = async () => {
    const guild = state.guildId;
    data = await tryApi("GET", `/api/guilds/${guild}/attendance?limit=${limit}`).catch(() => null);
    if (!data) { body.replaceChildren(el("p", { class: "muted" }, "Couldn't load attendance.")); return; }
    draw();
  };
  dialog.append(
    el("div", { class: "att-head" },
      el("div", {}, el("h2", {}, "Raid attendance"), el("div", { class: "muted small" }, currentGuild()?.name ?? "")),
      el("div", { class: "att-head-tools" }, pick, closeX(dialog))),
    el("div", { class: "att-legend muted small" }, Object.keys(ATT_LABEL).map((k) => el("span", {}, el("span", { class: `att-dot ${k}` }), ATT_LABEL[k]))),
    body);
  load();
}

// ---------- Attendance on profile cards ----------

const raidsBaseOpenProfile = openProfile;
openProfile = async function (userId, anchor) {
  await raidsBaseOpenProfile(userId, anchor);
  const guild = state.guildId;
  const pop = popover;
  if (!guild || state.home || !pop?.classList.contains("profile-pop")) return;
  const a = await api("GET", `/api/guilds/${guild}/attendance/${userId}`).catch(() => null);
  if (!a || !a.total || popover !== pop || !pop.isConnected) return;
  const came = a.present + a.late + a.benched;
  const day = (iso) => new Date(iso).toLocaleDateString([], { day: "numeric", month: "short" });
  const section = el("div", { class: "pc-section pc-attendance" },
    el("h4", {}, "RAID ATTENDANCE"),
    el("div", { class: "pca-row" },
      el("strong", { class: "pca-pct" }, `${a.percent}%`),
      el("span", { class: "muted small" }, `${came} of ${a.total} raid${a.total === 1 ? "" : "s"}${a.late ? ` · ${a.late} late` : ""}${a.benched ? ` · ${a.benched} benched` : ""}`)),
    el("div", { class: "pca-dots", "aria-label": "Recent raids, oldest first" },
      [...a.recent].reverse().map((r) => el("span", { class: `att-dot ${r.status}`, title: `${r.title} · ${day(r.starts_at)} · ${ATT_LABEL[r.status]}` }))));
  const body = pop.querySelector(".pc-body");
  body?.insertBefore(section, body.querySelector(".pc-actions"));
  const r = pop.getBoundingClientRect();
  if (r.bottom > window.innerHeight - 8) pop.style.top = `${Math.max(8, window.innerHeight - r.height - 8)}px`;
};
