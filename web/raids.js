// Outhouse: raid calendar, the outhouse's attendance table and attendance on
// profile cards (taking it is the raid card's 📋 Attendance section, mrt.js)
// and live raid updates. Loaded after features.js and
// uses its raid helpers (renderEventCard, openRaidDialog, eventWhen, …).

const ATT_LABEL = { present: "Present", late: "Late", benched: "Benched", excused: "Excused", absent: "Absent" };
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

// ---------- One raid in a window (for raids whose card isn't in chat yet, or was removed) ----------

const raidCard = { dialog: null, ev: null, body: null };

function openRaidCard(ev) {
  raidCard.ev = ev;
  const dialog = raidWindow("raid-card-dialog", () => { if (raidCard.dialog === dialog) raidCard.dialog = null; });
  raidCard.dialog = dialog;
  raidCard.body = el("div", { class: "raid-card-body" });
  dialog.append(el("div", { class: "raid-card-head" },
    el("span", { class: "muted small" }, signupsNotOpen(ev) ? `📬 ${opensText(ev)} (the sign-up post goes up in chat then).`
      : ev.card_removed ? "🧹 Its chat card was removed after the raid; everything's still here." : ""), closeX(dialog)), raidCard.body);
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

// ---------- The outhouse's attendance table ("Raid attendance") ----------
// One attendance: MRT imports and hand marks, counted like everywhere else
// (no-shows show as Absent, the bench as Benched; % = attended out of
// attended + no-shows).

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
    ["name", "Member"], ["percent", "%"], ["present", "✓"], ["late", "⏱"], ["absent", "✗"], ["benched", "🪑"], ["excused", "🛈"],
  ];
  const colTitle = { name: "Member", percent: "Attendance: raids attended out of raids attended or missed after signing up", present: "Present (MRT or by hand)", late: "Late", absent: "Absent (no-shows)", benched: "Benched", excused: "Excused (not counted)" };
  const nameOf = (m) => memberById(m.user_id)?.display_name ?? m.display_name;
  const draw = () => {
    if (!data) return;
    if (!data.raids.length) {
      body.replaceChildren(el("div", { class: "att-empty" },
        el("p", {}, "No attendance yet."),
        el("p", { class: "muted small" }, "Raid leaders can import Method Raid Tools' attendance export (raid card ⋯ → Import attendance (MRT)…, or the Loot & attendance window), or take it by hand once a raid starts (the raid card's 📋 Attendance → Edit attendance).")));
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
      el("tbody", {}, rows.map((m) => el("tr", { "data-user": m.user_id },
        el("td", { class: "name" }, el("button", { type: "button", class: "att-member", onclick: (e) => openProfile(m.user_id, e.currentTarget) },
          avatarFor(m.user_id, nameOf(m), 22), nameOf(m))),
        el("td", { class: "pct", title: m.total ? `Attended ${m.total - m.absent} of ${m.total} raids that count` : "Nothing counts yet (bench, excused or not signed up)" },
          el("div", { class: "pct-bar", style: `--pct:${m.total ? m.percent : 0}%` }, el("span", {}, m.total ? `${m.percent}%` : "—"))),
        el("td", {}, m.present), el("td", {}, m.late), el("td", {}, m.absent), el("td", {}, m.benched), el("td", {}, m.excused),
        m.history.map((h, i) => el("td", { class: "raid-col" }, h
          ? el("span", { class: `att-dot ${h}`, title: `${ATT_LABEL[h]} · ${data.raids[i].title}` })
          : el("span", { class: "att-dot none", title: `Not there · ${data.raids[i].title}` }))))))));
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
    el("div", { class: "att-legend muted small" }, Object.keys(ATT_LABEL).map((k) => el("span", {}, el("span", { class: `att-dot ${k}` }), ATT_LABEL[k])),
      typeof openLootLog === "function" ? el("button", { type: "button", class: "link small att-by-char", onclick: () => { dialog.close(); openLootLog({ tab: "people" }); } }, "📜 By character") : null),
    body);
  load();
}

// ---------- Attendance on profile cards ----------
// The same numbers as everywhere else (MRT imports and hand marks): the last
// 8 weeks (else all time) in this outhouse, and their last raids as dots.

const ATT_DOT = { present: "present", late: "late", benched: "benched", excused: "excused", absent: "absent" };
const attDot = (r) => (r.manual_status ? ATT_DOT[r.manual_status] : null) ?? ({ attended: "present", no_show: "absent", excused: "excused" })[r.outcome] ?? "none";
const ATT_OUTCOME_LABEL = { attended: "Attended", no_show: "No-show", excused: "Excused", absent: "Absent (didn't sign up)" };

const raidsBaseOpenProfile = openProfile;
openProfile = async function (userId, anchor) {
  await raidsBaseOpenProfile(userId, anchor);
  const guild = state.guildId;
  const pop = popover;
  if (!guild || state.home || !pop?.classList.contains("profile-pop")) return;
  const all = await api("GET", `/api/users/${userId}/attendance`).catch(() => null);
  const a = all?.find((g) => g.guild_id === guild);
  if (!a || !(a.all_time.eligible || a.raids.length) || popover !== pop || !pop.isConnected) return;
  const recent = a.recent.eligible > 0;
  const t = recent ? a.recent : a.all_time;
  const day = (iso) => new Date(iso).toLocaleDateString([], { day: "numeric", month: "short" });
  const raids = (n) => `${n} raid${n === 1 ? "" : "s"}`;
  const section = el("div", { class: "pc-section pc-attendance" },
    el("h4", {}, "RAID ATTENDANCE"),
    el("div", { class: "pca-row" },
      el("strong", { class: "pca-pct" }, t.eligible ? `${t.percent}%` : "—"),
      el("span", { class: "muted small" }, [
        `${t.attended} of ${raids(t.eligible)}${recent ? " in 8 weeks" : ""}`,
        t.no_shows ? `${t.no_shows} no-show${t.no_shows === 1 ? "" : "s"}` : null,
        t.excused ? `${t.excused} excused` : null].filter(Boolean).join(" · "))),
    el("div", { class: "pca-dots", "aria-label": "Recent raids, oldest first" },
      [...a.raids].reverse().map((r) => el("span", { class: `att-dot ${attDot(r)}`,
        title: `${r.title} · ${day(r.starts_at)} · ${r.manual_status ? `${ATT_LABEL[r.manual_status] ?? r.manual_status} (marked by hand)` : ATT_OUTCOME_LABEL[r.outcome] ?? r.outcome}` }))));
  const body = pop.querySelector(".pc-body");
  body?.insertBefore(section, body.querySelector(".pc-actions"));
  const r = pop.getBoundingClientRect();
  if (r.bottom > window.innerHeight - 8) pop.style.top = `${Math.max(8, window.innerHeight - r.height - 8)}px`;
};
