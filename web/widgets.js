// Outhouse: sidebar widgets, stacked above the member list (on phones, at
// the top of the members drawer): a clock with your time and the realm's
// (plus the weekly reset), a mini raid calendar for this outhouse, your
// latest mentions, and a compact soundboard while you're in voice. Each
// can be collapsed, hidden or moved; User settings → Sidebar widgets lists
// them all. Saved on the server (api/widgets.rs), the same on every device.
// Loaded after myraids.js and mentions.js (raidDot, openMyRaids,
// mentionCard, openMentionsInbox, …) and soundboard.js.

const WIDGET_INFO = {
  clock: { title: "Clock", about: "Your time, and the realm's time and weekly reset for the outhouse you're in." },
  calendar: { title: "Raids", name: "Raid calendar", about: "This week (or month) of raids in the outhouse you're in, coloured by your sign-ups. Tap a day to open My raids." },
  mentions: { title: "Mentions", about: "The latest three messages that pinged you, anywhere." },
  soundboard: { title: "Soundboard", about: "Your favourite and recent sounds. Only shows while you're in a voice channel." },
};
const WIDGET_DEFAULTS = [
  { id: "clock", on: true, collapsed: false },
  { id: "calendar", on: true, collapsed: false, mode: "week" },
  { id: "mentions", on: true, collapsed: false },
  { id: "soundboard", on: false, collapsed: false },
];
// The zones WoW realms run on, offered first (any IANA zone can be typed).
// `same`: other zone names on the same clock (shown with the preset's label).
const REALM_ZONES = [
  { tz: "America/Chicago", label: "US Central", same: ["America/North_Dakota/Center"] },
  { tz: "America/Denver", label: "US Mountain" },
  { tz: "America/Los_Angeles", label: "US Pacific" },
  { tz: "America/New_York", label: "US Eastern" },
  { tz: "Australia/Sydney", label: "Oceanic (Sydney)" },
  { tz: "America/Sao_Paulo", label: "Brazil" },
  { tz: "Europe/Paris", label: "EU Central" },
  { tz: "Europe/London", label: "EU UK" },
  { tz: "Asia/Seoul", label: "Korea" },
  { tz: "Asia/Taipei", label: "Taiwan" },
];
// Realm time can differ per game (Forever, TBC and Era realms aren't all on one clock).
const REALM_GAMES = [["forever", "WoW: Forever", "Forever"], ["tbc", "TBC", "TBC"], ["era", "Classic Era", "Era"]];
const REGION_LABEL = { us: "US", eu: "EU", kr: "Korea", tw: "Taiwan" };
// Weekly reset, by region: US (and Oceanic realms, which are in the US
// region) on Tuesdays at 15:00 UTC, EU on Wednesdays at 04:00 UTC.
const WEEKLY_RESET = { us: { day: 2, hour: 15 }, eu: { day: 3, hour: 4 } };

state.widgets = { list: WIDGET_DEFAULTS.map((w) => ({ ...w })), version: 0, saved: 0, saving: null, again: false, loaded: false };

// ---------- the model ----------

function widgetList() {
  const list = state.widgets.list.filter((w) => WIDGET_INFO[w.id]);
  for (const d of WIDGET_DEFAULTS) if (!list.some((w) => w.id === d.id)) list.push({ ...d });
  return list;
}

function updateWidgets(change) {
  const list = widgetList().map((w) => ({ ...w }));
  if (change(list) === false) return;
  state.widgets.list = list;
  state.widgets.version += 1;
  renderWidgets(true);
  if (typeof refreshWidgetsPage === "function") refreshWidgetsPage();
  saveWidgets();
}

function saveWidgets() {
  if (state.widgets.saving) { state.widgets.again = true; return; }
  const version = state.widgets.version;
  const body = { widgets: state.widgets.list.map((w) => ({ id: w.id, on: !!w.on, collapsed: !!w.collapsed, ...(w.id === "calendar" ? { mode: w.mode || "week" } : {}) })) };
  state.widgets.saving = tryApi("PUT", "/api/users/@me/widgets", body)
    .then((saved) => {
      if (saved && state.widgets.version === version) {
        state.widgets.list = saved.widgets;
        state.widgets.saved = version;
      }
    })
    .catch(() => {})
    .finally(() => {
      state.widgets.saving = null;
      if (state.widgets.again) { state.widgets.again = false; saveWidgets(); }
    });
}

async function loadWidgets() {
  if (state.widgets.saving || state.widgets.version !== state.widgets.saved) return; // ours is newer
  const res = await api("GET", "/api/users/@me/widgets").catch(() => null);
  if (!res || state.widgets.saving || state.widgets.version !== state.widgets.saved) return;
  state.widgets.list = res.widgets;
  state.widgets.loaded = true;
  renderWidgets(true);
  if (typeof refreshWidgetsPage === "function") refreshWidgetsPage();
}

const setWidget = (id, patch) => updateWidgets((list) => { const w = list.find((x) => x.id === id); if (!w) return false; Object.assign(w, patch); });
function moveWidget(id, delta) {
  updateWidgets((list) => {
    const i = list.findIndex((w) => w.id === id);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= list.length) return false;
    [list[i], list[j]] = [list[j], list[i]];
  });
}
const canMoveWidget = (id, delta) => { const list = widgetList(); const i = list.findIndex((w) => w.id === id); return i >= 0 && i + delta >= 0 && i + delta < list.length; };

// ---------- the stack ----------

const widgetStack = el("div", { class: "widget-stack", id: "widget-stack", "aria-label": "Widgets", role: "region" });
document.querySelector("aside.members")?.prepend(widgetStack);
const inVoice = () => !!state.voice?.room;

// What the widgets depend on, so they're only redrawn when it changes.
function widgetContext() {
  const rs = state.raidSettings?.guild_id === state.guildId ? state.raidSettings : null;
  return [state.home ? "home" : state.guildId, inVoice(), JSON.stringify(realmClocks().map((c) => [c.game, c.time_zone, c.region, c.derived])), state.me?.id].join("|");
}
let widgetCtx = null;

function renderWidgets(force = false) {
  const ctx = widgetContext();
  if (!force && ctx === widgetCtx) return;
  widgetCtx = ctx;
  if (!state.me) return;
  const shown = widgetList().filter((w) => w.on && (w.id !== "soundboard" || inVoice()));
  widgetStack.hidden = !shown.length;
  widgetStack.replaceChildren(...shown.map(widgetSection));
}

function widgetSection(w) {
  const info = WIDGET_INFO[w.id];
  const toggle = () => setWidget(w.id, { collapsed: !w.collapsed });
  const extra = w.id === "calendar" && !w.collapsed ? el("div", { class: "w-seg", role: "group", "aria-label": "Calendar view" },
    ["week", "month"].map((m) => el("button", {
      type: "button", class: `w-seg-btn${(w.mode || "week") === m ? " on" : ""}`, "aria-pressed": (w.mode || "week") === m ? "true" : "false",
      onclick: () => setWidget("calendar", { mode: m }),
    }, m === "week" ? "Week" : "Month"))) : null;
  const menu = el("button", {
    type: "button", class: "icon-btn small w-menu", title: `${info.title} options`, "aria-label": `${info.title} options`, "aria-haspopup": "menu",
    onclick: (e) => {
      const r = e.currentTarget.getBoundingClientRect();
      openMenu(r.left - 140, r.bottom + 4, [
        { label: w.collapsed ? "Expand" : "Collapse", onClick: toggle },
        { label: "Move up", disabled: !canMoveWidget(w.id, -1), onClick: () => moveWidget(w.id, -1) },
        { label: "Move down", disabled: !canMoveWidget(w.id, 1), onClick: () => moveWidget(w.id, 1) },
        { label: "Hide this widget", onClick: () => { setWidget(w.id, { on: false }); toast(`${info.title} hidden. Bring it back in User settings → Sidebar widgets.`); } },
        "sep",
        { label: "Sidebar widgets…", onClick: () => openUserSettings("widgets") },
      ]);
    },
  }, "⋯");
  const head = el("header", { class: "w-head" },
    el("button", { type: "button", class: "w-toggle", "aria-expanded": w.collapsed ? "false" : "true", onclick: toggle },
      el("span", { class: "w-chev", "aria-hidden": "true" }, "▾"), el("span", { class: "w-title" }, info.title.toUpperCase())),
    extra, menu);
  const body = el("div", { class: "w-body" });
  if (!w.collapsed) fillWidget(w, body);
  return el("section", { class: `widget w-${w.id}${w.collapsed ? " collapsed" : ""}`, "data-widget": w.id }, head, w.collapsed ? null : body);
}

function fillWidget(w, body) {
  if (w.id === "clock") return drawClock(body);
  if (w.id === "calendar") return drawMiniCalendar(body, w.mode || "week");
  if (w.id === "mentions") return drawMentionsWidget(body);
  if (w.id === "soundboard") return drawSoundboardWidget(body);
}

// ---------- Clock ----------

function nextWeeklyReset(region, now = new Date()) {
  const spec = WEEKLY_RESET[region];
  if (!spec) return null;
  const d = new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate(), spec.hour));
  d.setUTCDate(d.getUTCDate() + ((spec.day - d.getUTCDay() + 7) % 7));
  if (d <= now) d.setUTCDate(d.getUTCDate() + 7);
  return d;
}

function untilText(ms) {
  const mins = Math.max(0, Math.round(ms / 60000));
  const d = Math.floor(mins / 1440), h = Math.floor((mins % 1440) / 60), m = mins % 60;
  if (d) return `${d}d ${h}h`;
  if (h) return `${h}h ${m}m`;
  return `${m}m`;
}

const zonePreset = (tz) => REALM_ZONES.find((z) => z.tz === tz || z.same?.includes(tz));
const zoneLabel = (tz) => zonePreset(tz)?.label ?? tz.replace(/_/g, " ");
const realmGameShort = (g) => REALM_GAMES.find(([id]) => id === g)?.[2] ?? "";

// "UTC+10" now, and whether the zone moves its clocks during the year.
function zoneOffset(tz, at = new Date()) {
  try {
    const part = new Intl.DateTimeFormat("en-US", { timeZone: tz, timeZoneName: "shortOffset" }).formatToParts(at).find((p) => p.type === "timeZoneName")?.value ?? "";
    const m = part.match(/GMT([+-])(\d+)(?::(\d+))?/);
    return m ? (Number(m[1] === "-" ? -1 : 1) * (Number(m[2]) * 60 + Number(m[3] || 0))) : 0;
  } catch { return null; }
}
function offsetText(mins) {
  if (mins == null) return "";
  const sign = mins < 0 ? "−" : "+";
  const a = Math.abs(mins);
  return `UTC${sign}${Math.floor(a / 60)}${a % 60 ? `:${String(a % 60).padStart(2, "0")}` : ""}`;
}
function zoneHasDst(tz) {
  const y = new Date().getUTCFullYear();
  return zoneOffset(tz, new Date(Date.UTC(y, 0, 15))) !== zoneOffset(tz, new Date(Date.UTC(y, 6, 15)));
}

// The outhouse's realm clocks (one per game with its own realm, or one for
// all games), the game of the next raid first.
function realmClocks() {
  if (state.home || !state.guildId) return [];
  const rs = state.raidSettings?.guild_id === state.guildId ? state.raidSettings : null;
  if (!rs) return [];
  let clocks = rs.realm_clocks?.length ? rs.realm_clocks.map((c) => ({ ...c }))
    : rs.realm_effective_time_zone ? [{ game: null, time_zone: rs.realm_effective_time_zone, region: rs.realm_effective_region || "", derived: !!rs.realm_time_zone_derived }] : [];
  const next = nextRaidGame();
  if (next) clocks = [...clocks.filter((c) => c.game === next), ...clocks.filter((c) => c.game !== next)];
  return clocks;
}

// The game of the outhouse's next raid (from the calendar widget or the chat).
function nextRaidGame() {
  const now = Date.now();
  const evs = [...(wcal.guild === state.guildId ? wcal.events : []), ...(state.messages ?? []).map((m) => m.event).filter(Boolean)]
    .filter((ev) => ev.game && ev.guild_id === state.guildId && new Date(ev.starts_at).getTime() > now)
    .sort((a, b) => new Date(a.starts_at) - new Date(b.starts_at));
  return evs[0]?.game ?? null;
}

// The realm clock for a raid in the current outhouse (null: none set).
function realmClockFor(ev) {
  const rs = state.raidSettings?.guild_id === ev.guild_id ? state.raidSettings : realmSettingsCache.get(ev.guild_id);
  if (!rs) { if (ev.guild_id !== state.guildId) realmSettingsLoad(ev.guild_id); return null; }
  const clocks = rs.realm_clocks ?? [];
  return clocks.find((c) => c.game && c.game === ev.game) ?? clocks.find((c) => !c.game)
    ?? (clocks.length && clocks.every((c) => c.time_zone === clocks[0].time_zone) ? clocks[0] : null);
}

// "Realm 7:30 pm" for a raid card when the realm's clock reads differently
// from the reader's (null otherwise).
function realmTimeFor(ev) {
  const c = realmClockFor(ev);
  if (!c) return null;
  const start = new Date(ev.starts_at);
  const realm = safeTime(start, { hour: "numeric", minute: "2-digit", timeZone: c.time_zone });
  if (realm === safeTime(start, { hour: "numeric", minute: "2-digit" })) return null;
  const game = c.game ? `${realmGameShort(c.game)} realm` : "Realm";
  return { text: `Realm ${realm}`, title: `${game} time: ${realm} ${safeDay(start, { weekday: "short", timeZone: c.time_zone })} (${zoneLabel(c.time_zone)})` };
}

// Other outhouses' realm settings, for My raids (fetched once each).
const realmSettingsCache = new Map();
const realmSettingsLoading = new Set();
function realmSettingsLoad(guildId) {
  if (!guildId || realmSettingsCache.has(guildId) || realmSettingsLoading.has(guildId)) return;
  realmSettingsLoading.add(guildId);
  api("GET", `/api/guilds/${guildId}/raid-settings`).then((s) => {
    realmSettingsCache.set(guildId, s || {});
    if (s?.realm_clocks?.length && typeof drawMyRaids === "function" && document.querySelector(".myraids-dialog")) drawMyRaids();
  }).catch(() => realmSettingsCache.set(guildId, {})).finally(() => realmSettingsLoading.delete(guildId));
}

function safeTime(date, opts) {
  try { return date.toLocaleTimeString([], opts); } catch { return date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" }); }
}
function safeDay(date, opts) {
  try { return date.toLocaleDateString([], opts); } catch { return date.toLocaleDateString([], { weekday: "short" }); }
}

function drawClock(body) {
  const now = new Date();
  const mine = el("div", { class: "w-clock-row" },
    el("span", { class: "w-clock-label" }, "Your time"),
    el("span", { class: "w-clock-time", "data-clock": "me" }, safeTime(now, { hour: "numeric", minute: "2-digit" })),
    el("span", { class: "w-clock-sub muted", "data-clock": "me-day" }, `${safeDay(now, { weekday: "short", day: "numeric", month: "short" })}`));
  let clocks = realmClocks();
  // Every game on one clock: one line, as before.
  const oneClock = clocks.length > 1 && clocks.every((c) => c.time_zone === clocks[0].time_zone);
  const games = clocks.filter((c) => c.game).map((c) => realmGameShort(c.game));
  if (oneClock) clocks = [{ ...clocks[0], game: null, games }];
  const rows = [mine];
  const realmRow = (c) => {
    const label = clocks.length < 2 ? "Realm time" : c.game ? `${realmGameShort(c.game)} realm` : "Other games";
    const how = c.derived ? `Worked out from the ${REGION_LABEL[c.region] ?? ""} region. Change it in Outhouse settings → Raids.` : c.time_zone;
    return el("div", { class: "w-clock-row realm", "data-game": c.game || "", title: `${label}: ${how}` },
      el("span", { class: "w-clock-label" }, label),
      el("span", { class: "w-clock-time", "data-tz": c.time_zone }, safeTime(now, { hour: "numeric", minute: "2-digit", timeZone: c.time_zone })),
      el("span", { class: "w-clock-sub muted", "data-tz-day": c.time_zone, "data-tz-tail": `${zoneLabel(c.time_zone)}${c.games?.length > 1 ? ` · ${c.games.join(", ")}` : ""}${c.derived ? " (from region)" : ""}` },
        `${safeDay(now, { weekday: "short", timeZone: c.time_zone })} · ${zoneLabel(c.time_zone)}${c.games?.length > 1 ? ` · ${c.games.join(", ")}` : ""}${c.derived ? " (from region)" : ""}`));
  };
  if (clocks.length) {
    const lines = clocks.map(realmRow);
    rows.push(...lines.slice(0, 2));
    if (lines.length > 2) {
      rows.push(el("details", { class: "w-clock-more" }, el("summary", {}, `${lines.length - 2} more`), ...lines.slice(2)));
    }
    const regions = [...new Set(clocks.map((c) => c.region).filter((r) => WEEKLY_RESET[r]))];
    for (const region of regions) {
      const reset = nextWeeklyReset(region, now);
      rows.push(el("div", { class: "w-reset", title: `About ${reset.toLocaleString([], { weekday: "long", hour: "numeric", minute: "2-digit" })} your time (${REGION_LABEL[region]} region; approximate)` },
        el("span", { class: "w-reset-ic", "aria-hidden": "true" }, "↻"),
        el("span", { class: "w-reset-label" }, regions.length > 1 ? `Weekly reset ${REGION_LABEL[region]}` : "Weekly reset"),
        el("strong", { class: "w-reset-in" }, "≈ ", el("span", { "data-reset": region }, untilText(reset - now)))));
    }
  } else if (!state.home && state.guildId && typeof can === "function" && can("manage_guild")) {
    rows.push(el("button", { type: "button", class: "link w-clock-set", onclick: () => openSettings("raids") }, "Set the realm's time zone…"));
  }
  body.replaceChildren(...rows);
}

// Ticks the clocks without redrawing anything else.
setInterval(() => {
  const now = new Date();
  if (!widgetStack.querySelector("[data-clock]")) return;
  const put = (n, v) => { if (n.textContent !== v) n.textContent = v; };
  widgetStack.querySelectorAll('[data-clock="me"]').forEach((n) => put(n, safeTime(now, { hour: "numeric", minute: "2-digit" })));
  widgetStack.querySelectorAll('[data-clock="me-day"]').forEach((n) => put(n, safeDay(now, { weekday: "short", day: "numeric", month: "short" })));
  widgetStack.querySelectorAll("[data-tz]").forEach((n) => put(n, safeTime(now, { hour: "numeric", minute: "2-digit", timeZone: n.dataset.tz })));
  widgetStack.querySelectorAll("[data-tz-day]").forEach((n) => put(n, `${safeDay(now, { weekday: "short", timeZone: n.dataset.tzDay })} · ${n.dataset.tzTail}`));
  widgetStack.querySelectorAll("[data-reset]").forEach((n) => { const r = nextWeeklyReset(n.dataset.reset, now); if (r) put(n, untilText(r - now)); });
}, 5000);

// ---------- Mini raid calendar ----------

const wcal = { key: null, guild: null, from: null, to: null, events: [], loading: false };

function wcalRange(mode) {
  const today = new Date();
  if (mode === "month") {
    const from = startOfGrid(new Date(today.getFullYear(), today.getMonth(), 1));
    const to = new Date(from); to.setDate(to.getDate() + 42);
    return [from, to];
  }
  const from = mrWeekStart(today);
  const to = new Date(from); to.setDate(to.getDate() + 7);
  return [from, to];
}

async function wcalLoad(mode) {
  const guild = state.home ? null : state.guildId;
  const [from, to] = wcalRange(mode);
  const key = `${guild}|${from.toISOString()}|${mode}`;
  if (wcal.key === key && !wcal.stale) return;
  Object.assign(wcal, { key, guild, from, to, loading: true, stale: false });
  const events = await fetchMyRaids(from, to, guild).catch(() => null);
  if (wcal.key !== key) return;
  wcal.events = events ?? [];
  wcal.loading = false;
  redrawWidget("calendar");
}

function redrawWidget(id) {
  const section = widgetStack.querySelector(`.widget[data-widget="${id}"]`);
  const body = section?.querySelector(".w-body");
  const w = widgetList().find((x) => x.id === id);
  if (body && w && !w.collapsed) fillWidget(w, body);
}

function drawMiniCalendar(body, mode) {
  const [from] = wcalRange(mode);
  const guild = state.home ? null : state.guildId;
  if (wcal.key !== `${guild}|${from.toISOString()}|${mode}` || wcal.stale) wcalLoad(mode);
  const byDay = new Map();
  for (const ev of [...wcal.events].sort((a, b) => new Date(a.starts_at) - new Date(b.starts_at))) {
    const key = dayKey(new Date(ev.starts_at));
    if (!byDay.has(key)) byDay.set(key, []);
    byDay.get(key).push(ev);
  }
  const todayKey = dayKey(new Date());
  const thisMonth = new Date().getMonth();
  const cells = [];
  const n = mode === "month" ? 42 : 7;
  for (let i = 0; i < n; i++) {
    const d = new Date(from); d.setDate(from.getDate() + i);
    const key = dayKey(d);
    const evs = byDay.get(key) ?? [];
    cells.push(el("button", {
      type: "button",
      class: `w-day${key === todayKey ? " today" : ""}${mode === "month" && d.getMonth() !== thisMonth ? " other" : ""}${evs.length ? " busy" : ""}`,
      "aria-label": mrDayLabel(d, evs),
      title: evs.map((ev) => `${timeOf(ev.starts_at)} ${ev.title} · ${raidStatusText(ev)}`).join("\n"),
      onclick: () => { document.getElementById("app")?.classList.remove("show-members"); openMyRaids({ day: d, guild, view: mode }); },
    },
    mode === "week" ? el("span", { class: "w-wd" }, WEEKDAYS[i][0]) : null,
    el("span", { class: "w-num" }, d.getDate()),
    el("span", { class: "w-dots" }, evs.slice(0, 3).map((ev) => raidDot(ev)))));
  }
  const next = wcal.events.filter((ev) => new Date(ev.starts_at) > new Date()).sort((a, b) => new Date(a.starts_at) - new Date(b.starts_at))[0];
  body.replaceChildren(
    el("div", { class: `w-cal w-cal-${mode}` }, mode === "month" ? WEEKDAYS.map((w) => el("span", { class: "w-wd-head" }, w[0])) : null, cells),
    next ? el("button", { type: "button", class: "w-next", onclick: () => { document.getElementById("app")?.classList.remove("show-members"); openMyRaids({ day: new Date(next.starts_at), guild, view: mode }); } },
      raidDot(next), el("span", { class: "w-next-title" }, next.title),
      el("span", { class: "muted small" }, new Date(next.starts_at).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" })))
      : el("div", { class: "muted small w-none" }, wcal.loading ? "Loading…" : mode === "week" ? "No more raids this week." : "No more raids this month."));
}

// Called by myraids.js when a raid changes or goes.
let wcalTimer = null;
function widgetsNoteEvent(ev, keep) {
  if (!wcal.from) return;
  keep(wcal.events, wcal.from, wcal.to, wcal.guild);
  clearTimeout(wcalTimer);
  wcalTimer = setTimeout(() => redrawWidget("calendar"), 80);
}
function widgetsForgetEvent(id) {
  wcal.events = wcal.events.filter((x) => x.id !== id);
  redrawWidget("calendar");
}

// ---------- Mentions ----------

function drawMentionsWidget(body) {
  const m = state.mentionInbox;
  const items = m.latest ?? [];
  body.replaceChildren(
    items.length ? el("div", { class: "w-mentions" }, items.map((it) => mentionCard(it, { compact: true })))
      : el("div", { class: "muted small w-none" }, m.latestAt ? "No mentions in the last 30 days." : "Loading…"),
    el("button", { type: "button", class: "link w-see-all", onclick: () => { document.getElementById("app")?.classList.remove("show-members"); openMentionsInbox({ filter: "all", guild: "" }); } },
      m.unread ? `See all · ${m.unread} unread` : "See all"));
}

function refreshMentionsWidget() {
  redrawWidget("mentions");
}

// ---------- Soundboard (while you're in voice) ----------

async function drawSoundboardWidget(body) {
  if (!inVoice()) { body.replaceChildren(); return; }
  if (sb.guildId !== sbGuildId() || (!sb.guild.length && !sb.all.length)) {
    body.replaceChildren(el("div", { class: "muted small w-none" }, "Loading sounds…"));
    await sbLoad().catch(() => {});
    if (!body.isConnected) return;
  }
  const byId = sbIndex();
  const picks = [];
  const add = (s) => { if (s && !picks.includes(s) && picks.length < 8) picks.push(s); };
  [...byId.values()].filter((s) => s.favourite).forEach(add);
  sbRecentIds().map((id) => byId.get(id)).forEach(add);
  sb.guild.filter((s) => !s.hidden).forEach(add);
  body.replaceChildren(
    picks.length ? el("div", { class: "w-sounds" }, picks.map((snd) => el("button", {
      type: "button", class: "w-sound", title: snd.name, onclick: (e) => sbPlay(snd, e.currentTarget),
    }, el("span", { class: "w-sound-emoji" }, snd.emoji || "🔊"), el("span", { class: "w-sound-name" }, snd.name))))
      : el("div", { class: "muted small w-none" }, "No sounds yet."),
    el("button", { type: "button", class: "link w-see-all", onclick: (e) => openSoundboard(e.currentTarget) }, "All sounds…"));
}

// ---------- User settings → Sidebar widgets ----------

let widgetsPageNode = null;
function refreshWidgetsPage() {
  if (widgetsPageNode?.isConnected) drawWidgetsPage(widgetsPageNode);
  else widgetsPageNode = null;
}

function drawWidgetsPage(main) {
  widgetsPageNode = main;
  const list = widgetList();
  const btn = (label, text, onClick, disabled) => el("button", { type: "button", class: "ghost wg-btn", title: label, "aria-label": label, disabled, onclick: onClick }, text);
  main.replaceChildren(
    el("h2", {}, "Sidebar widgets"),
    el("p", { class: "muted" }, "Little helpers above the member list on the right (on phones, at the top of the members drawer). Only you see them, and they're the same on all your devices. Collapse one from its header, or use its ⋯ menu."),
    el("div", { class: "wg-list" }, list.map((w, i) => {
      const info = WIDGET_INFO[w.id];
      const box = el("input", { type: "checkbox", checked: !!w.on, "aria-label": `Show ${info.title}`, onchange: (e) => setWidget(w.id, { on: e.target.checked }) });
      return el("div", { class: `wg-row${w.on ? "" : " off"}`, "data-widget": w.id },
        el("label", { class: "wg-main" }, box,
          el("span", { class: "wg-text" }, el("strong", {}, info.name ?? info.title), el("span", { class: "muted small" }, info.about))),
        w.id === "calendar" ? el("select", { class: "select wg-mode", "aria-label": "Calendar shows", onchange: (e) => setWidget("calendar", { mode: e.target.value }) },
          el("option", { value: "week", selected: (w.mode || "week") === "week" }, "Week"),
          el("option", { value: "month", selected: w.mode === "month" }, "Month")) : null,
        el("span", { class: "wg-move" },
          btn(`Move ${info.title} up`, "↑", () => moveWidget(w.id, -1), i === 0),
          btn(`Move ${info.title} down`, "↓", () => moveWidget(w.id, 1), i === list.length - 1)));
    })),
    el("p", { class: "muted small" }, "The realm time comes from Outhouse settings → Raids (people who can manage the outhouse set it)."),
  );
}

// ---------- Outhouse settings → Raids: realm time ----------

const validZone = (tz) => { try { new Intl.DateTimeFormat([], { timeZone: tz }); return true; } catch { return false; } };
const REGION_ZONE = { us: "America/Los_Angeles", eu: "Europe/Paris", kr: "Asia/Seoul", tw: "Asia/Taipei" };

// The "Realm time" part of the Raids page: {node, values(), saved(settings)}.
// Tabs: All games (the default) and each game's own realm on top of it.
function realmTimeFields(settings) {
  let serverRegion = settings.realm_region ? null : settings.realm_effective_region || null;
  // What's being edited: "" (all games) or a game id → { tz, region }.
  const vals = { "": { tz: settings.realm_time_zone || "", region: settings.realm_region || "" } };
  for (const [g] of REALM_GAMES) {
    const own = settings.realm_games?.[g];
    vals[g] = { tz: own?.time_zone || "", region: own?.region || "" };
  }
  let tab = "";
  const allZones = typeof Intl.supportedValuesOf === "function" ? Intl.supportedValuesOf("timeZone") : REALM_ZONES.map((z) => z.tz);
  for (const z of REALM_ZONES) for (const t of [z.tz, ...(z.same ?? [])]) if (!allZones.includes(t)) allZones.push(t);
  const list = el("datalist", { id: "realm-zone-list" }, allZones.map((z) => el("option", { value: z })));
  const tabs = el("div", { class: "realm-tabs", role: "tablist", "aria-label": "Realm time for" });
  const zone = el("input", { type: "text", class: "realm-zone", list: "realm-zone-list", placeholder: "Automatic", autocomplete: "off", spellcheck: "false", "aria-label": "Realm time zone" });
  const region = el("select", { class: "select realm-region", "aria-label": "Realm region" });
  const presets = el("div", { class: "realm-presets", role: "group", "aria-label": "Common realm time zones" });
  const preview = el("p", { class: "muted small realm-preview" });
  const tabNote = el("p", { class: "muted small realm-tab-note", style: "margin:0" });
  // The zone and region the tab ends up with (a game falls back to all games).
  const effectiveOf = (key) => {
    const v = vals[key];
    const base = key ? effectiveOf("") : null;
    const reg = v.region || (key ? base.region : serverRegion) || "";
    const tz = v.tz || (v.region ? REGION_ZONE[v.region] : key ? base.tz : (reg ? REGION_ZONE[reg] : null));
    return { tz: tz && validZone(tz) ? tz : null, region: reg, derived: !v.tz };
  };
  const paintTabs = () => tabs.replaceChildren(...[["", "All games"], ...REALM_GAMES].map(([key, label]) => {
    const set = key ? !!(vals[key].tz || vals[key].region) : !!(vals[""].tz || vals[""].region);
    return el("button", {
      type: "button", role: "tab", class: `realm-tab${tab === key ? " on" : ""}${set ? " set" : ""}`, "data-game": key, "aria-selected": tab === key ? "true" : "false",
      onclick: () => { tab = key; load(); },
    }, label, set && key ? el("span", { class: "realm-tab-dot", "aria-label": "(own realm time)" }) : null);
  }));
  const chip = (z) => {
    const off = zoneOffset(z.tz);
    const dst = zoneHasDst(z.tz);
    const now = new Date();
    return el("button", {
      type: "button", class: `realm-chip${zone.value.trim() === z.tz || z.same?.includes(zone.value.trim()) ? " on" : ""}`, "data-tz": z.tz,
      title: `${z.tz}${z.same ? ` (same clock as ${z.same.join(", ")})` : ""}${dst ? " · moves for daylight saving" : " · no daylight saving"}`,
      onclick: () => { zone.value = z.tz; vals[tab].tz = z.tz; paint(); },
    },
    el("span", { class: "realm-chip-name" }, z.label),
    el("span", { class: "realm-chip-now" }, safeTime(now, { hour: "numeric", minute: "2-digit", timeZone: z.tz })),
    el("span", { class: "realm-chip-meta" }, `${z.tz.replace(/_/g, " ")} · ${offsetText(off)}${dst ? " · DST" : ""}`),
    z.same ? el("span", { class: "realm-chip-meta" }, `same clock as ${z.same.map((t) => t.replace(/_/g, " ")).join(", ")}`) : null);
  };
  const paint = () => {
    const tz = zone.value.trim();
    vals[tab].tz = tz;
    vals[tab].region = region.value;
    paintTabs();
    presets.replaceChildren(
      el("button", { type: "button", class: `realm-chip auto${tz ? "" : " on"}`, onclick: () => { zone.value = ""; paint(); } },
        el("span", { class: "realm-chip-name" }, tab ? "Same as all games" : "Automatic"),
        el("span", { class: "realm-chip-meta" }, tab ? "uses the All games tab" : "from the region")),
      ...REALM_ZONES.map(chip));
    zone.classList.toggle("bad", !!tz && !validZone(tz));
    if (tz && !validZone(tz)) { preview.textContent = "That isn't a time zone we know. Pick one from the list (like America/Chicago)."; return; }
    const eff = effectiveOf(tab);
    const name = tab ? `${REALM_GAMES.find(([g]) => g === tab)[1]} realm` : "Realm";
    if (!eff.tz) { preview.textContent = `${name} time is hidden: pick a time zone or a region.`; return; }
    const now = new Date();
    const reset = nextWeeklyReset(eff.region, now);
    const how = tz ? "" : vals[tab].region ? `, from the ${REGION_LABEL[eff.region]} region` : tab ? ", same as all games" : `, from the ${REGION_LABEL[eff.region]} region`;
    preview.textContent = `${name} time now: ${safeTime(now, { hour: "numeric", minute: "2-digit", timeZone: eff.tz })} ${safeDay(now, { weekday: "short", timeZone: eff.tz })} (${zoneLabel(eff.tz)}, ${offsetText(zoneOffset(eff.tz))}${zoneHasDst(eff.tz) ? ", moves for daylight saving" : ""}${how})`
      + (reset ? ` · weekly reset in about ${untilText(reset - now)}` : " · no weekly reset shown for this region");
  };
  const load = () => {
    const v = vals[tab];
    zone.value = v.tz;
    zone.placeholder = tab ? "Same as all games" : "Automatic";
    region.replaceChildren(
      el("option", { value: "" }, tab ? "Same as all games" : "Automatic"),
      ...[["us", "US & Oceanic"], ["eu", "Europe"], ["kr", "Korea"], ["tw", "Taiwan"]].map(([r, t]) => el("option", { value: r, selected: v.region === r }, t)));
    tabNote.textContent = tab
      ? `Only if ${REALM_GAMES.find(([g]) => g === tab)[1]} realms run on a different clock. Left on “Same as all games”, it uses the All games tab.`
      : "Used by every game that doesn't have its own realm time.";
    paint();
  };
  zone.addEventListener("input", paint);
  region.addEventListener("change", paint);
  load();
  const node = el("div", { class: "realm-settings" },
    el("h3", { class: "rs-head" }, "Realm time"),
    el("p", { class: "muted", style: "margin:0" }, "The clock widget shows everyone the realm's time next to their own, and when the weekly reset is. Forever, TBC and Era realms can each have their own. Time zones move for daylight saving by themselves."),
    list,
    tabs,
    tabNote,
    el("label", { class: "field" }, "Time zone", zone),
    el("p", { class: "muted small realm-hint", style: "margin:0" }, "Not sure? In game, compare /time (server time) with these."),
    presets,
    el("label", { class: "field realm-region-field" }, "Region (for the weekly reset)", region),
    el("p", { class: "muted small", style: "margin:0" }, `Automatic uses the region's usual time zone (US: Pacific, EU: Paris)${serverRegion ? `; this server's Armory region is ${REGION_LABEL[serverRegion] ?? serverRegion.toUpperCase()}` : ""}. Reset times are approximate: US Tuesday 15:00 UTC, EU Wednesday 04:00 UTC. Oceanic realms are in the US region.`),
    preview);
  return {
    node,
    values: () => ({
      realm_time_zone: vals[""].tz.trim(), realm_region: vals[""].region,
      realm_games: Object.fromEntries(REALM_GAMES.map(([g]) => [g, { time_zone: vals[g].tz.trim(), region: vals[g].region }]).filter(([, v]) => v.time_zone || v.region)),
    }),
    saved: (s) => { serverRegion = s.realm_region ? serverRegion : s.realm_effective_region || serverRegion; widgetCtx = null; renderWidgets(); },
  };
}

// ---------- keeping them current ----------

const wBaseRenderMembers = renderMembers;
renderMembers = function () {
  wBaseRenderMembers();
  renderWidgets();
};

// Voice joins and leaves (the soundboard widget), and anything else that
// changes the context without redrawing the member list.
setInterval(() => renderWidgets(), 1500);

const wBaseHandle = handleEvent;
handleEvent = function (ev) {
  wBaseHandle(ev);
  switch (ev.t) {
    case "READY":
      loadWidgets();
      break;
    case "WIDGETS_CHANGED":
      loadWidgets();
      break;
    case "SOUNDS_CHANGED":
      if (inVoice()) sbLoad().then(() => redrawWidget("soundboard")).catch(() => {});
      break;
    case "GUILD_REFRESH":
      wcal.stale = true;
      redrawWidget("calendar");
      break;
  }
};

// A new day (or week): the calendar moves on.
setInterval(() => { if (wcal.key && !wcal.key.includes(wcalRange(widgetList().find((w) => w.id === "calendar")?.mode || "week")[0].toISOString())) redrawWidget("calendar"); }, 60 * 1000);
