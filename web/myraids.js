// Outhouse: "My raids". A calendar button at the top of the rail (its badge
// counts raids in the next 24 hours you haven't answered) opens every raid
// you can see, in all your outhouses, as a month or a week, with a dot per
// raid in your colours:
//   yellow  sign-ups are open and you haven't answered
//   green   you're in: signed up, running late, tentative or on the bench
//   red     you said you can't make it
//   grey    it's over, or sign-ups are closed (not open yet, or past the deadline)
// Pick a day to see its raids, sign up (the usual sign-up popover, with your
// characters) or say you can't make it, or jump to the raid's card in chat.
// Loaded after raids.js, raid-admin.js and folders.js; uses their helpers
// (raidWindow, closeX, openSignup, setSignup, openRaidCard, eventWhen, dayKey,
// keyDate, startOfGrid, weekdayOf, WEEKDAYS, STATUS_LABEL, signupsLocked, …).

const MR_HOUR = 3600 * 1000;
const MR_DAY = 24 * MR_HOUR;
const MR_STATE = {
  open: { label: "Not answered yet", short: "Sign-ups open" },
  yes: { label: "You're in", short: "You're in" },
  no: { label: "Can't make it", short: "Absent" },
  past: { label: "Over", short: "Over" },
  closed: { label: "Sign-ups closed", short: "Closed" },
};
const MR_ICON = '<svg viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3" y="5" width="18" height="16" rx="2.5"/><path d="M16 3v4M8 3v4M3 10h18"/><circle cx="8.5" cy="15" r="1.3" fill="currentColor" stroke="none"/><circle cx="12.5" cy="15" r="1.3" fill="currentColor" stroke="none"/></svg>';

// Your own sign-up on a raid, if any.
const mrMine = (ev) => ev.signups.find((s) => s.user_id === state.me?.id) ?? null;

// Which colour a raid is for you (see the top of this file).
function raidDotState(ev, now = Date.now()) {
  const end = new Date(ev.starts_at).getTime() + ev.duration_minutes * 60000;
  if (end <= now) return "past";
  const mine = mrMine(ev);
  if (mine) return mine.status === "no" ? "no" : "yes";
  if (signupsNotOpen(ev) || signupsLocked(ev)) return "closed";
  return "open";
}

// What the day list says about you.
function raidStatusText(ev) {
  const st = raidDotState(ev);
  const mine = mrMine(ev);
  if (mine) return STATUS_LABEL[mine.status] ?? MR_STATE[st].label;
  if (st === "closed" && signupsNotOpen(ev)) return opensText(ev);
  return MR_STATE[st].label;
}

function raidDot(ev, extra = "") {
  const st = raidDotState(ev);
  return el("span", { class: `mr-dot st-${st}${extra ? ` ${extra}` : ""}`, title: `${ev.title} · ${timeOf(ev.starts_at)} · ${raidStatusText(ev)}` });
}

const mrGuild = (id) => state.guilds.find((g) => g.id === id);
function mrGuildIcon(id, size = 20) {
  const g = mrGuild(id);
  const node = el("span", { class: "mr-gicon", style: `width:${size}px;height:${size}px;font-size:${Math.round(size * 0.45)}px`, "aria-hidden": "true" });
  if (g?.icon_url) node.append(el("img", { src: g.icon_url, alt: "" }));
  else node.textContent = initials(g?.name ?? "?").slice(0, 2);
  return node;
}

const mrChannels = new Map(); // channelId -> name (from the server's answers and the open outhouse)
const mrChannelName = (id) => mrChannels.get(id) ?? state.channels.find((c) => c.id === id)?.name ?? "raids";

async function fetchMyRaids(from, to, guildId = null) {
  const q = `from=${encodeURIComponent(from.toISOString())}&to=${encodeURIComponent(to.toISOString())}${guildId ? `&guild=${guildId}` : ""}`;
  const res = await api("GET", `/api/users/@me/raids?${q}`);
  for (const c of res.channels ?? []) mrChannels.set(c.id, c.name);
  return res.events ?? [];
}

// ---------- Going to a message (raid cards here, mentions in mentions.js) ----------

// selectChannel's load, so a jump can wait for the channel to be up first.
const mrBaseSelectChannel = selectChannel;
selectChannel = function (id, opts) {
  const p = mrBaseSelectChannel(id, opts);
  state.channelLoading = p;
  return p;
};

// Opens an outhouse (guild "0": your DMs), the channel, then the message
// (or the thread).
async function goToMessage(guildId, channelId, messageId, threadId = null) {
  document.getElementById("app")?.classList.remove("show-nav", "show-members");
  if (guildId === "0") {
    if (!state.home || state.channelId !== channelId) await openHome(channelId);
  } else {
    if (!mrGuild(guildId)) return toast("You're not in that outhouse any more.");
    if (state.home || state.guildId !== guildId) await selectGuild(guildId, channelId);
    if (state.guildId !== guildId) return;
    if (state.channelId !== channelId) {
      if (!state.channels.some((c) => c.id === channelId)) return toast("You can't see that channel any more.");
      await selectChannel(channelId);
    }
  }
  await state.channelLoading?.catch?.(() => {});
  if (state.channelId !== channelId) return;
  if (threadId && typeof openThread === "function") return openThread(String(threadId));
  if (messageId) return jumpToMessage(channelId, messageId);
}

// ---------- The rail button and its badge ----------

const myRaids = {
  dialog: null, root: null,
  view: prefs.get("myraids_view", "month") === "week" ? "week" : "month",
  anchor: null, selected: null, guild: null,
  events: [], from: null, to: null, loading: false,
  soon: [], // the next 24 hours, for the badge
};

function mrSoonCount() {
  const now = Date.now();
  return myRaids.soon.filter((ev) => {
    const t = new Date(ev.starts_at).getTime();
    return t >= now && t < now + MR_DAY && raidDotState(ev, now) === "open";
  }).length;
}

function paintRailButton() {
  const btn = $("myraids-btn");
  if (!btn) return;
  const n = mrSoonCount();
  btn.querySelector(".mr-badge")?.remove();
  btn.title = n ? `My raids: ${n} in the next 24 hours you haven't answered` : "My raids";
  btn.setAttribute("aria-label", btn.title);
  if (n) btn.append(el("span", { class: "mr-badge", "aria-hidden": "true" }, n > 9 ? "9+" : String(n)));
  $("nav-toggle")?.classList.toggle("has-raids", n > 0);
}

let mrSoonTimer = null;
function loadSoonRaids(delay = 0) {
  clearTimeout(mrSoonTimer);
  mrSoonTimer = setTimeout(async () => {
    if (!state.me) return;
    const now = new Date();
    const events = await fetchMyRaids(now, new Date(now.getTime() + MR_DAY + MR_HOUR)).catch(() => null);
    if (!events) return;
    myRaids.soon = events;
    paintRailButton();
  }, delay);
}
setInterval(() => { if (state.me) loadSoonRaids(); }, 5 * 60 * 1000);
// A raid's start sliding into (or out of) the next 24 hours.
setInterval(paintRailButton, 60 * 1000);

(function railButton() {
  const scroll = $("rail-scroll");
  if (!scroll) return;
  const btn = el("button", { class: "rail-btn myraids-btn", id: "myraids-btn", type: "button", title: "My raids", "aria-label": "My raids", onclick: () => openMyRaids() });
  btn.innerHTML = MR_ICON;
  scroll.prepend(btn);
})();

// ---------- The My raids window ----------

const mrWeekStart = (d) => { const s = new Date(d); s.setHours(0, 0, 0, 0); s.setDate(s.getDate() - weekdayOf(s)); return s; };
const mrMonthStart = (d) => new Date(d.getFullYear(), d.getMonth(), 1);

// openMyRaids({day, guild, view})
function openMyRaids(opts = {}) {
  document.getElementById("app")?.classList.remove("show-nav", "show-members");
  const day = opts.day ? new Date(opts.day) : new Date();
  myRaids.dialog?.close();
  myRaids.selected = dayKey(day);
  myRaids.guild = opts.guild ?? null;
  if (opts.view) myRaids.view = opts.view;
  myRaids.anchor = myRaids.view === "week" ? mrWeekStart(day) : mrMonthStart(day);
  myRaids.events = [];
  const dialog = raidWindow("myraids-dialog", () => { if (myRaids.dialog === dialog) myRaids.dialog = null; });
  myRaids.dialog = dialog;
  myRaids.root = el("div", { class: "mr" });
  dialog.append(myRaids.root);
  loadMyRaids();
  return dialog;
}

function mrRange() {
  if (myRaids.view === "week") {
    const from = new Date(myRaids.anchor);
    const to = new Date(from); to.setDate(to.getDate() + 7);
    return [from, to];
  }
  const from = startOfGrid(myRaids.anchor);
  const to = new Date(from); to.setDate(to.getDate() + 42);
  return [from, to];
}

async function loadMyRaids() {
  const [from, to] = mrRange();
  myRaids.from = from; myRaids.to = to;
  myRaids.loading = true;
  drawMyRaids();
  const events = await fetchMyRaids(from, to).catch((err) => { toast(err.message); return null; });
  if (myRaids.from !== from || !myRaids.dialog) return; // moved on meanwhile
  myRaids.events = events ?? [];
  myRaids.loading = false;
  drawMyRaids();
}

function mrMove(delta) {
  const a = new Date(myRaids.anchor);
  if (myRaids.view === "week") a.setDate(a.getDate() + 7 * delta);
  else a.setMonth(a.getMonth() + delta);
  myRaids.anchor = myRaids.view === "week" ? a : mrMonthStart(a);
  const today = new Date();
  const [from, to] = mrRange();
  const inView = myRaids.view === "week" ? today >= from && today < to : today.getMonth() === myRaids.anchor.getMonth() && today.getFullYear() === myRaids.anchor.getFullYear();
  myRaids.selected = dayKey(inView ? today : myRaids.anchor);
  loadMyRaids();
}

function mrSetView(view) {
  if (myRaids.view === view) return;
  myRaids.view = view;
  prefs.set("myraids_view", view);
  const sel = keyDate(myRaids.selected);
  myRaids.anchor = view === "week" ? mrWeekStart(sel) : mrMonthStart(sel);
  loadMyRaids();
}

function mrToday() {
  const today = new Date();
  myRaids.selected = dayKey(today);
  myRaids.anchor = myRaids.view === "week" ? mrWeekStart(today) : mrMonthStart(today);
  loadMyRaids();
}

// The raids shown (the outhouse filter), by day.
function mrByDay() {
  const map = new Map();
  const list = myRaids.events.filter((ev) => !myRaids.guild || ev.guild_id === myRaids.guild)
    .sort((a, b) => new Date(a.starts_at) - new Date(b.starts_at));
  for (const ev of list) {
    const key = dayKey(new Date(ev.starts_at));
    if (!map.has(key)) map.set(key, []);
    map.get(key).push(ev);
  }
  return map;
}

function mrTitle() {
  if (myRaids.view === "month") return myRaids.anchor.toLocaleDateString([], { month: "long", year: "numeric" });
  const end = new Date(myRaids.anchor); end.setDate(end.getDate() + 6);
  const fmt = new Intl.DateTimeFormat([], { day: "numeric", month: "short", year: "numeric" });
  if (typeof fmt.formatRange === "function") return fmt.formatRange(myRaids.anchor, end);
  return `${myRaids.anchor.toLocaleDateString([], { day: "numeric", month: "short" })} – ${fmt.format(end)}`;
}

function drawMyRaids() {
  if (!myRaids.dialog) return;
  const keep = myRaids.root.querySelector(".mr-scroll")?.scrollTop ?? 0;
  const byDay = mrByDay();
  const todayKey = dayKey(new Date());
  const step = myRaids.view === "week" ? "week" : "month";
  const head = el("div", { class: "mr-head" },
    el("div", { class: "mr-titlebar" },
      el("span", { class: "mr-logo", "aria-hidden": "true" }),
      el("h2", {}, "My raids"),
      el("div", { class: "mr-views", role: "group", "aria-label": "Show" },
        ["month", "week"].map((v) => el("button", { type: "button", class: `mr-view${myRaids.view === v ? " on" : ""}`, "aria-pressed": myRaids.view === v ? "true" : "false", onclick: () => mrSetView(v) }, v === "month" ? "Month" : "Week"))),
      closeX(myRaids.dialog)),
    el("div", { class: "mr-nav" },
      el("button", { type: "button", class: "icon-btn", title: `Previous ${step}`, "aria-label": `Previous ${step}`, onclick: () => mrMove(-1) }, "‹"),
      el("h3", { class: "mr-period" }, mrTitle()),
      el("button", { type: "button", class: "icon-btn", title: `Next ${step}`, "aria-label": `Next ${step}`, onclick: () => mrMove(1) }, "›"),
      el("button", { type: "button", class: "cal-today", onclick: mrToday }, "Today")));
  head.querySelector(".mr-logo").innerHTML = MR_ICON;
  const guildIds = [...new Set([...state.guilds.map((g) => g.id)])];
  const chips = el("div", { class: "mr-chips", role: "group", "aria-label": "Outhouses" },
    el("button", { type: "button", class: `mr-chip${myRaids.guild ? "" : " on"}`, "aria-pressed": myRaids.guild ? "false" : "true", onclick: () => { myRaids.guild = null; drawMyRaids(); } }, "All outhouses"),
    guildIds.map((id) => {
      const n = myRaids.events.filter((ev) => ev.guild_id === id).length;
      return el("button", { type: "button", class: `mr-chip${myRaids.guild === id ? " on" : ""}`, "aria-pressed": myRaids.guild === id ? "true" : "false", title: mrGuild(id)?.name, onclick: () => { myRaids.guild = myRaids.guild === id ? null : id; drawMyRaids(); } },
        mrGuildIcon(id, 18), el("span", { class: "mr-chip-name" }, mrGuild(id)?.name ?? "Outhouse"), n ? el("span", { class: "mr-chip-n" }, n) : null);
    }));
  const legend = el("div", { class: "mr-legend muted small" },
    ["open", "yes", "no", "past"].map((k) => el("span", { class: "mr-key" }, el("span", { class: `mr-dot st-${k}` }), k === "past" ? "Over or closed" : MR_STATE[k].label)),
    el("span", { class: "mr-tz" }, `Your time (${myTimeZone()})${myRaids.loading ? " · Loading…" : ""}`));
  const grid = myRaids.view === "week" ? mrWeekGrid(byDay, todayKey) : mrMonthGrid(byDay, todayKey);
  const day = mrDayPanel(byDay.get(myRaids.selected) ?? []);
  myRaids.root.replaceChildren(head, chips, legend, el("div", { class: `mr-body mr-scroll mr-is-${myRaids.view}` }, grid, day));
  const s = myRaids.root.querySelector(".mr-scroll");
  if (s) s.scrollTop = keep;
}

function mrPick(key) {
  myRaids.selected = key;
  drawMyRaids();
  // Phones: the day's raids are under the calendar.
  if (isPhoneWidth()) myRaids.root.querySelector(".mr-day")?.scrollIntoView({ block: "nearest", behavior: "smooth" });
}

function mrDayLabel(d, evs) {
  const states = evs.map((ev) => MR_STATE[raidDotState(ev)].short.toLowerCase());
  return `${d.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" })}${evs.length ? `, ${evs.length} raid${evs.length === 1 ? "" : "s"}: ${states.join(", ")}` : ""}`;
}

function mrMonthGrid(byDay, todayKey) {
  const first = startOfGrid(myRaids.anchor);
  const cells = [];
  for (let i = 0; i < 42; i++) {
    const d = new Date(first); d.setDate(first.getDate() + i);
    const key = dayKey(d);
    const evs = byDay.get(key) ?? [];
    cells.push(el("button", {
      type: "button",
      class: `mr-cell${d.getMonth() !== myRaids.anchor.getMonth() ? " other" : ""}${key === todayKey ? " today" : ""}${key === myRaids.selected ? " selected" : ""}${evs.length ? " busy" : ""}`,
      "data-day": key, "aria-label": mrDayLabel(d, evs), "aria-pressed": key === myRaids.selected ? "true" : "false",
      onclick: () => mrPick(key),
    },
    el("span", { class: "mr-num" }, d.getDate()),
    evs.length ? el("span", { class: "mr-dots" }, evs.slice(0, 4).map((ev) => raidDot(ev)), evs.length > 4 ? el("span", { class: "mr-more" }, `+${evs.length - 4}`) : null) : null));
  }
  return el("div", { class: "mr-grid" }, WEEKDAYS.map((n) => el("div", { class: "mr-wd" }, n)), cells);
}

function mrWeekGrid(byDay, todayKey) {
  const cols = [];
  for (let i = 0; i < 7; i++) {
    const d = new Date(myRaids.anchor); d.setDate(d.getDate() + i);
    const key = dayKey(d);
    const evs = byDay.get(key) ?? [];
    cols.push(el("div", { class: `mr-wcol${key === todayKey ? " today" : ""}${key === myRaids.selected ? " selected" : ""}`, "data-day": key },
      el("button", { type: "button", class: "mr-whead", "aria-label": mrDayLabel(d, evs), "aria-pressed": key === myRaids.selected ? "true" : "false", onclick: () => mrPick(key) },
        el("span", { class: "mr-wday" }, WEEKDAYS[weekdayOf(d)]), el("span", { class: "mr-wnum" }, d.getDate())),
      el("div", { class: "mr-wlist" },
        evs.length ? evs.map((ev) => el("button", {
          type: "button", class: `mr-wraid st-${raidDotState(ev)}`, title: `${ev.title} · ${raidStatusText(ev)}`,
          onclick: () => mrPick(key),
        }, raidDot(ev), el("span", { class: "mr-wtime" }, timeOf(ev.starts_at)), el("span", { class: "mr-wtitle" }, ev.title), mrGuildIcon(ev.guild_id, 16)))
          : el("span", { class: "mr-wnone" }, "—"))));
  }
  return el("div", { class: "mr-week" }, cols);
}

function mrDayPanel(evs) {
  const d = keyDate(myRaids.selected);
  return el("section", { class: "mr-day", "aria-label": "Raids on the day" },
    el("h3", {}, d.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" })),
    evs.length ? evs.map(mrRaidRow) : el("p", { class: "muted small mr-empty" }, myRaids.loading ? "Loading…" : "No raids this day."),
    !evs.length && !myRaids.loading ? mrNextHint(d) : null);
}

// "Next up" on an empty day: the next raid after it (with the filter).
function mrNextHint(day) {
  const after = new Date(day); after.setDate(after.getDate() + 1);
  const next = myRaids.events.filter((ev) => (!myRaids.guild || ev.guild_id === myRaids.guild) && new Date(ev.starts_at) >= after)
    .sort((a, b) => new Date(a.starts_at) - new Date(b.starts_at))[0];
  if (!next) return null;
  const when = new Date(next.starts_at);
  return el("button", { type: "button", class: "mr-next", onclick: () => mrPick(dayKey(when)) },
    raidDot(next), el("span", {}, "Next up: ", el("strong", {}, next.title)),
    el("span", { class: "muted small" }, when.toLocaleString([], { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" })));
}

function mrRaidRow(ev) {
  const st = raidDotState(ev);
  const mine = mrMine(ev);
  const start = new Date(ev.starts_at);
  const end = new Date(start.getTime() + ev.duration_minutes * 60000);
  const hm = (x) => x.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const slots = ev.max_tanks + ev.max_healers + ev.max_dps;
  const active = ev.signups.filter((s) => s.status === "yes" || s.status === "late").length;
  const bench = ev.signups.filter((s) => s.status === "bench").length;
  const over = st === "past";
  const canSign = !over && !signupsNotOpen(ev) && !signupsLocked(ev);
  const signedUp = mine && mine.status !== "no";
  const buttons = [];
  if (canSign) {
    buttons.push(el("button", { type: "button", class: `ev-btn mr-signup${signedUp ? "" : " go"}`, onclick: (e) => openSignup(ev, e.currentTarget, mine && mine.status !== "no" && mine.status !== "bench" ? mine.status : "yes") }, signedUp ? "Change" : "Sign up"));
    buttons.push(mine?.status === "no"
      ? el("span", { class: "mr-absent-on" }, "✗ Absent")
      : el("button", { type: "button", class: "ev-btn mr-absent", onclick: () => setSignup(ev, { status: "no" }) }, "Absent"));
  }
  buttons.push(el("button", { type: "button", class: "ev-link mr-open", onclick: () => mrOpen(ev) }, ev.message_id ? `Open in #${mrChannelName(ev.channel_id)}` : "Open raid"));
  return el("article", { class: `mr-raid st-${st}`, "data-event": ev.id },
    el("div", { class: "mr-raid-where muted small" }, mrGuildIcon(ev.guild_id, 18), el("span", {}, mrGuild(ev.guild_id)?.name ?? "Outhouse"), el("span", { class: "mr-sep" }, "·"), el("span", {}, `#${mrChannelName(ev.channel_id)}`)),
    el("div", { class: "mr-raid-top" },
      raidDot(ev),
      el("strong", { class: "mr-raid-title" }, ev.title),
      el("span", { class: `mr-pill st-${st}` }, raidStatusText(ev))),
    el("div", { class: "mr-raid-meta muted small" },
      el("span", {}, `${hm(start)} – ${hm(end)}`),
      el("span", {}, `${active}/${slots} signed up${bench ? ` (+${bench} bench)` : ""}`),
      !over && !mine && signupsLocked(ev) ? el("span", {}, "🔒 Sign-ups closed") : null),
    el("div", { class: "mr-raid-actions" }, buttons));
}

async function mrOpen(ev) {
  if (!ev.message_id) return openRaidCard(ev);
  myRaids.dialog?.close();
  await goToMessage(ev.guild_id, ev.channel_id, ev.message_id);
}

// ---------- Live updates ----------

let mrRedraw = null;
function mrNoteEvent(ev) {
  const keep = (list, from, to, guild) => {
    const i = list.findIndex((x) => x.id === ev.id);
    const start = new Date(ev.starts_at).getTime();
    const end = start + ev.duration_minutes * 60000;
    const inRange = from && start < to.getTime() && end > from.getTime() && (!guild || ev.guild_id === guild);
    if (i >= 0 && inRange) list[i] = ev;
    else if (i >= 0) list.splice(i, 1);
    else if (inRange) list.push(ev);
  };
  if (myRaids.dialog) {
    keep(myRaids.events, myRaids.from, myRaids.to, null);
    clearTimeout(mrRedraw);
    mrRedraw = setTimeout(drawMyRaids, 50);
  }
  const now = new Date();
  keep(myRaids.soon, now, new Date(now.getTime() + MR_DAY + MR_HOUR), null);
  paintRailButton();
  if (typeof widgetsNoteEvent === "function") widgetsNoteEvent(ev, keep);
}

function mrForgetEvent(id) {
  myRaids.events = myRaids.events.filter((x) => x.id !== id);
  myRaids.soon = myRaids.soon.filter((x) => x.id !== id);
  if (myRaids.dialog) drawMyRaids();
  paintRailButton();
  if (typeof widgetsForgetEvent === "function") widgetsForgetEvent(id);
}

const mrBaseOnEventChanged = onEventChanged;
onEventChanged = function (ev) {
  mrBaseOnEventChanged(ev);
  mrNoteEvent(ev);
};

const mrBaseHandle = handleEvent;
handleEvent = function (ev) {
  mrBaseHandle(ev);
  switch (ev.t) {
    case "READY":
      loadSoonRaids(300);
      break;
    // (EVENT_UPDATE reaches mrNoteEvent through onEventChanged, in raids.js.)
    case "EVENT_DELETE":
      mrForgetEvent(ev.d.id);
      break;
    // You can see different channels now: raids may have come or gone.
    case "GUILD_REFRESH":
    case "GUILD_CREATE":
    case "GUILD_DELETE":
      loadSoonRaids(500);
      if (myRaids.dialog) loadMyRaids();
      break;
  }
};
