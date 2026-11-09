// Outhouse: profiles, raid planner, bots & slash commands, and phone support.
// Loaded after app.js and uses its helpers (el, api, state, prefs, …).

// ======================================================================
// Profiles
// ======================================================================

// Built-in profile backgrounds. Keys must match PROFILE_THEMES on the server.
const PROFILE_THEMES = [
  { key: "ember", name: "Ember", css: "linear-gradient(135deg, #3a1206 0%, #ff6a2b 60%, #ffb36b 100%)" },
  { key: "dusk", name: "Dusk", css: "linear-gradient(160deg, #1b1035 0%, #5b2a6e 50%, #ff7a59 100%)" },
  { key: "moonwell", name: "Moonwell", css: "radial-gradient(circle at 70% 30%, #b9f1ff 0%, #3a6ea5 35%, #10162e 75%)" },
  { key: "felfire", name: "Felfire", css: "radial-gradient(circle at 30% 85%, #c6ff4a 0%, #2f8a1e 35%, #0b1a08 80%)" },
  { key: "frostfall", name: "Frostfall", css: "linear-gradient(180deg, #e8f7ff 0%, #6fb7ff 45%, #1b2b4a 100%)" },
  { key: "verdant", name: "Verdant", css: "linear-gradient(135deg, #0f2a1d 0%, #2e7d4f 50%, #c9e265 100%)" },
  { key: "stormwind", name: "Stormwind", css: "linear-gradient(135deg, #0e2a5c 0%, #2f5fb3 55%, #e8c35a 100%)" },
  { key: "abyss", name: "Abyss", css: "radial-gradient(circle at 50% 120%, #2a8fbf 0%, #0a2a43 40%, #04070f 80%)" },
  { key: "goldleaf", name: "Gold leaf", css: "linear-gradient(135deg, #3b2a05 0%, #c79a2b 50%, #fff1b8 100%)" },
  { key: "outhouse", name: "Starry night", css: "radial-gradient(circle at 82% 28%, #fff6d5 0 5%, transparent 6%), radial-gradient(1px 1px at 20% 30%, #fff 50%, transparent 51%), radial-gradient(1px 1px at 40% 15%, #fff 50%, transparent 51%), radial-gradient(1px 1px at 60% 40%, #fff 50%, transparent 51%), linear-gradient(180deg, #0b1026 0%, #1d2a52 60%, #ff6a2b 100%)" },
  // Animated (gently; switched off for people who ask for less motion).
  { key: "emberdrift", name: "Ember drift", anim: true, css: "radial-gradient(circle at 50% 130%, #ff8a3d 0%, #9a2a08 38%, #2a0b04 70%, #120503 100%)" },
  { key: "starfield", name: "Starfield", anim: true, css: "radial-gradient(ellipse at 50% -10%, #2b3a78 0%, #0d1433 50%, #04050d 100%)" },
  { key: "aurora", name: "Aurora", anim: true, css: "linear-gradient(180deg, #030b14 0%, #08283a 55%, #0b2a1e 100%)" },
];
const themeCss = (key) => PROFILE_THEMES.find((t) => t.key === key)?.css ?? null;
const themeAnimated = (key) => !!PROFILE_THEMES.find((t) => t.key === key)?.anim;

function profileBanner(p) {
  if (p.banner_url) return `center / cover no-repeat url("${p.banner_url}")`;
  return themeCss(p.profile_theme) ?? (p.accent_color ? hexColour(p.accent_color) : colourFor(p.id));
}

function profileCard(p, { preview = false } = {}) {
  const member = memberById(p.id);
  const roles = (member?.role_ids ?? []).map(roleById).filter(Boolean).sort((a, b) => b.position - a.position);
  const theme = themeCss(p.profile_theme);
  const shownName = member?.display_name ?? p.display_name;
  const since = (iso) => new Date(iso).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" });
  const anim = theme && themeAnimated(p.profile_theme) ? ` anim anim-${p.profile_theme}` : "";
  const card = el("div", { class: `profile-card${theme ? " themed" : ""}${anim}`, style: theme ? `--theme:${theme}` : (p.accent_color ? `--accent-ring:${hexColour(p.accent_color)}` : "") },
    anim ? el("div", { class: "pc-fx", "aria-hidden": "true" }) : null,
    el("div", { class: `pc-banner${p.banner_url ? " img" : ""}`, style: `background:${profileBanner(p)}` }),
    el("div", { class: "pc-avatar" }, avatarFor(p.id, shownName, 84, p.avatar_url || undefined)),
    el("div", { class: "pc-body" },
      el("div", { class: "pc-name" }, shownName, p.bot ? el("span", { class: "bot-tag" }, "BOT") : null),
      el("div", { class: "pc-user" }, `@${p.username}`, shownName !== p.display_name ? ` · ${p.display_name}` : ""),
      p.about_me ? el("div", { class: "pc-section" }, el("h4", {}, "ABOUT ME"), el("p", {}, p.about_me)) : null,
      roles.length ? el("div", { class: "pc-section" }, el("h4", {}, "ROLES"),
        el("div", { class: "pc-roles" }, roles.map((r) => el("span", { class: "pc-role" },
          el("span", { class: "dot", style: `background:${hexColour(r.color) || "#a1a1aa"}` }), r.name)))) : null,
      el("div", { class: "pc-section" }, el("h4", {}, "MEMBER SINCE"),
        el("p", {}, `Outhouse: ${since(p.created_at)}`, member?.joined_at ? ` · ${currentGuild()?.name ?? "Here"}: ${since(member.joined_at)}` : "")),
      preview ? null : el("div", { class: "pc-actions" },
        p.id === state.me.id
          ? el("button", { class: "ghost", onclick: () => { closePopover(); openUserSettings("look"); } }, "Edit profile")
          : [
            p.bot ? null : el("button", { class: "primary pc-message", onclick: () => { closePopover(); openDmWith(p.id); } }, "Message"),
            state.guildId ? el("button", { class: "ghost", onclick: () => { closePopover(); input.value += `@${shownName} `; input.focus(); autosize(); } }, "Mention") : null,
          ]),
    ),
  );
  return card;
}

async function openProfile(userId, anchor) {
  if (!userId || !anchor) return;
  try {
    const p = await api("GET", `/api/profiles/${userId}`);
    const pop = openPopover(anchor, profileCard(p), "below");
    pop.classList.add("profile-pop");
    // Keep it beside the thing you clicked and on screen.
    const r = anchor.getBoundingClientRect();
    const pr = pop.getBoundingClientRect();
    let x = r.right + 10;
    if (x + pr.width > window.innerWidth - 8) x = r.left - pr.width - 10;
    placeFixed(pop, Math.max(8, x), Math.min(r.top, window.innerHeight - pr.height - 8));
  } catch (err) {
    toast(err.message);
  }
}

async function drawProfileLook(main) {
  const p = await api("GET", `/api/profiles/${state.me.id}`);
  const draft = { about_me: p.about_me, profile_theme: p.profile_theme, accent_color: p.accent_color };
  const preview = el("div", { class: "pl-preview" });
  const paint = () => preview.replaceChildren(el("h4", { class: "muted small" }, "PREVIEW"), profileCard({ ...p, ...draft, avatar_url: state.me.avatar_url ?? p.avatar_url }, { preview: true }));

  const pickFile = (accept, onFile) => {
    const f = el("input", { type: "file", accept, hidden: true });
    f.addEventListener("change", () => f.files[0] && onFile(f.files[0]));
    document.body.append(f);
    f.click();
    setTimeout(() => f.remove(), 60_000);
  };
  const upload = (kind) => pickFile("image/png,image/jpeg,image/gif,image/webp", async (file) => {
    const form = new FormData();
    form.append("file", file, file.name);
    toast("Uploading…");
    const user = await tryApi("POST", `/api/users/@me/${kind}`, form).catch(() => null);
    if (!user) return;
    state.me = user;
    renderMe();
    main.replaceChildren();
    drawProfileLook(main);
  });
  const remove = async (kind) => {
    const user = await tryApi("DELETE", `/api/users/@me/${kind}`).catch(() => null);
    if (!user) return;
    state.me = user;
    renderMe();
    main.replaceChildren();
    drawProfileLook(main);
  };

  const about = el("textarea", { maxlength: 190, rows: 3, placeholder: "Main: Prot Pally. Alts: too many." }, draft.about_me);
  const count = el("span", { class: "muted small" }, `${draft.about_me.length}/190`);
  about.addEventListener("input", () => { draft.about_me = about.value; count.textContent = `${about.value.length}/190`; paint(); });

  const colour = el("input", { type: "color", value: hexColour(draft.accent_color) || "#ff6a2b" });
  colour.addEventListener("input", () => { draft.accent_color = parseInt(colour.value.slice(1), 16); paint(); });

  const themes = el("div", { class: "theme-grid" });
  const drawThemes = () => themes.replaceChildren(
    el("button", { type: "button", class: `theme-swatch none${draft.profile_theme ? "" : " on"}`, title: "No background", onclick: () => { draft.profile_theme = ""; drawThemes(); paint(); } }, "None"),
    ...PROFILE_THEMES.map((t) => el("button", {
      type: "button", class: `theme-swatch${draft.profile_theme === t.key ? " on" : ""}${t.anim ? ` anim anim-${t.key}` : ""}`, title: t.name, style: `background:${t.css}`,
      onclick: () => { draft.profile_theme = t.key; drawThemes(); paint(); },
    }, el("span", {}, t.name))),
  );
  drawThemes();

  const save = el("button", { class: "primary", onclick: async () => {
    const ok = await tryApi("PATCH", "/api/users/@me", draft).then(() => true, () => false);
    if (ok) toast("Profile saved.");
  } }, "Save profile");

  paint();
  main.append(
    el("h2", {}, "Profile & picture"),
    el("div", { class: "pl-layout" },
      el("div", { class: "pl-form" },
        el("div", { class: "field" }, "Profile picture",
          el("div", { class: "inline-row" },
            avatarFor(state.me.id, state.me.display_name, 56, state.me.avatar_url || undefined),
            el("button", { class: "ghost", onclick: () => upload("avatar") }, "Upload picture"),
            state.me.avatar_url ? el("button", { class: "ghost danger", onclick: () => remove("avatar") }, "Remove") : null),
          el("span", { class: "muted small", style: "font-weight:400" }, "PNG, JPEG, GIF (animated is fine) or WebP, up to 8 MB.")),
        el("div", { class: "field" }, "Banner",
          el("div", { class: "inline-row" },
            el("button", { class: "ghost", onclick: () => upload("banner") }, p.banner_url ? "Change banner" : "Upload banner"),
            p.banner_url ? el("button", { class: "ghost danger", onclick: () => remove("banner") }, "Remove") : null),
          el("span", { class: "muted small", style: "font-weight:400" }, "Shown across the top of your profile card. A banner replaces the background below.")),
        el("label", { class: "field" }, el("span", {}, "About me ", count), about),
        el("div", { class: "field" }, "Profile background", themes),
        el("label", { class: "field" }, "Accent colour (used when there's no background)",
          el("div", { class: "inline-row" }, colour,
            el("button", { type: "button", class: "ghost", onclick: () => { draft.accent_color = 0; paint(); } }, "Default"))),
        el("div", {}, save),
      ),
      preview,
    ),
  );
}

// ======================================================================
// Raid planner
// ======================================================================

// WoW: Forever's nine classes and their talent trees. Feral can tank (Bear) or deal damage (Cat).
const WOW_CLASSES = [
  { name: "Druid", colour: "#FF7C0A", specs: [["Balance", "dps"], ["Feral (Bear)", "tank"], ["Feral (Cat)", "dps"], ["Restoration", "healer"]] },
  { name: "Hunter", colour: "#AAD372", specs: [["Beast Mastery", "dps"], ["Marksmanship", "dps"], ["Survival", "dps"]] },
  { name: "Mage", colour: "#3FC7EB", specs: [["Arcane", "dps"], ["Fire", "dps"], ["Frost", "dps"]] },
  { name: "Paladin", colour: "#F48CBA", specs: [["Holy", "healer"], ["Protection", "tank"], ["Retribution", "dps"]] },
  { name: "Priest", colour: "#FFFFFF", specs: [["Discipline", "healer"], ["Holy", "healer"], ["Shadow", "dps"]] },
  { name: "Rogue", colour: "#FFF468", specs: [["Assassination", "dps"], ["Combat", "dps"], ["Subtlety", "dps"]] },
  { name: "Shaman", colour: "#0070DD", specs: [["Elemental", "dps"], ["Enhancement", "dps"], ["Restoration", "healer"]] },
  { name: "Warlock", colour: "#8788EE", specs: [["Affliction", "dps"], ["Demonology", "dps"], ["Destruction", "dps"]] },
  { name: "Warrior", colour: "#C69B6D", specs: [["Arms", "dps"], ["Fury", "dps"], ["Protection", "tank"]] },
];
const classColour = (name) => WOW_CLASSES.find((c) => c.name === name)?.colour ?? null;
const ROLE_INFO = { tank: { icon: "🛡️", label: "Tanks" }, healer: { icon: "✚", label: "Healers" }, dps: { icon: "⚔️", label: "DPS" } };
const STATUS_LABEL = { yes: "Signed up", late: "Running late", tentative: "Tentative", bench: "Bench", no: "Can't make it" };

// Raid sizes per game: WoW: Forever 10/20/40, TBC 10/25, Classic Era
// 20/40 plus 10 (UBRS). "Any game" offers them all.
const RAID_SIZES = [
  { label: "40-player raid", tanks: 5, healers: 10, dps: 25, games: ["forever", "era"] },
  { label: "25-player raid", tanks: 3, healers: 7, dps: 15, games: ["tbc"] },
  { label: "20-player raid", tanks: 3, healers: 5, dps: 12, games: ["forever", "era"] },
  { label: "10-player raid", tanks: 2, healers: 3, dps: 5, games: ["forever", "tbc", "era"] },
];
const raidSizesFor = (game) => RAID_SIZES.filter((s) => !game || s.games.includes(game));

function canManageEvent(ev) {
  return ev.creator_id === state.me.id || can("manage_events", ev.channel_id);
}

const WEEKDAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
// 0 = Monday … 6 = Sunday, for a date in your time zone.
const weekdayOf = (date) => (date.getDay() + 6) % 7;

function repeatLabel(r) {
  if (!r) return "";
  const names = r.weekdays.map((d) => WEEKDAYS[d]);
  const days = names.length > 1 ? `${names.slice(0, -1).join(", ")} & ${names[names.length - 1]}` : names[0];
  return `${r.every_weeks === 1 ? "Weekly" : `Every ${r.every_weeks} weeks`} on ${days}`;
}

const signupsLocked = (ev) => !!ev.signups_close_at && Date.now() >= new Date(ev.signups_close_at).getTime();
const benchOf = (ev) => ev.signups.filter((s) => s.status === "bench");

function relativeTime(date) {
  const mins = Math.round((date - Date.now()) / 60000);
  if (Math.abs(mins) < 1) return "now";
  const fmt = new Intl.RelativeTimeFormat([], { numeric: "auto" });
  if (Math.abs(mins) < 60) return fmt.format(mins, "minute");
  const hours = Math.round(mins / 60);
  if (Math.abs(hours) < 36) return fmt.format(hours, "hour");
  return fmt.format(Math.round(hours / 24), "day");
}

function eventWhen(ev) {
  const start = new Date(ev.starts_at);
  const end = new Date(start.getTime() + ev.duration_minutes * 60000);
  const day = start.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" });
  const t = (d) => d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  const now = Date.now();
  const status = now > end ? "finished" : now >= start ? "on now" : relativeTime(start);
  return { text: `${day} · ${t(start)} – ${t(end)}`, status, live: now >= start && now <= end, over: now > end, started: now >= start };
}

// Their name in the outhouse.
const signupPerson = (s) => memberById(s.user_id)?.display_name ?? s.display_name;
// What the roster calls them: the character they picked, else their name.
const signupLabel = (s) => s.character?.full_name || signupPerson(s);

// "First Last" characters: the last name can hide when the roster is narrow.
function nameParts(s) {
  const c = s.character;
  if (c?.name && c.last_name && c.full_name === `${c.name} ${c.last_name}`) return [c.name, el("span", { class: "nm-last" }, ` ${c.last_name}`)];
  return signupLabel(s);
}

function signupName(s, ev) {
  const colour = classColour(s.class);
  const person = signupPerson(s);
  const manage = canManageEvent(ev);
  const node = el("span", {
    class: `su-name${s.user_id === state.me.id ? " me" : ""}${s.character ? " has-char" : ""}`,
    style: colour ? `color:${colour}` : "",
    title: [s.character ? `${person} in the outhouse` : person, s.class && `${s.spec} ${s.class}`.trim(),
      s.status !== "yes" ? STATUS_LABEL[s.status] : null, s.note ? `“${s.note}”` : null].filter(Boolean).join(" · "),
  }, nameParts(s));
  if (manage) {
    node.classList.add("manageable");
    node.tabIndex = 0;
    node.setAttribute("role", "button");
    node.setAttribute("aria-haspopup", "menu");
    const open = (target) => {
      const r = target.getBoundingClientRect();
      const set = (status) => setSignup(ev, { status, role: s.role, class: s.class, spec: s.spec, note: s.note, user_id: s.user_id, keep_character: true });
      const onRoster = s.status === "yes" || s.status === "late";
      openMenu(r.left, r.bottom + 4, [
        onRoster ? null : { label: "Move to roster", onClick: () => set("yes") },
        s.status !== "bench" ? { label: "Move to bench", onClick: () => set("bench") } : null,
        s.status !== "tentative" ? { label: "Mark tentative", onClick: () => set("tentative") } : null,
        typeof openAddToRaid === "function" ? { label: "Change character or role…", onClick: () => openAddToRaid(ev, s) } : null,
        "sep",
        { label: "Remove from raid", danger: true, onClick: () => tryApi("DELETE", `/api/events/${ev.id}/signups/${s.user_id}`).then(applyEventReply, () => {}) },
      ]);
    };
    node.addEventListener("click", (e) => open(e.currentTarget));
    node.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(e.currentTarget); }
    });
  }
  return node;
}

function cancelRaid(ev, scope) {
  const future = scope === "future";
  const title = ev.series_id ? (future ? "Cancel this and future raids?" : "Cancel this raid?") : "Delete raid?";
  const text = future
    ? `"${ev.title}" on ${eventWhen(ev).text} and every later raid in the series will be cancelled, with their sign-ups.`
    : `"${ev.title}" and all its sign-ups will be removed.`;
  confirmBox(title, text, ev.series_id ? "Cancel raids" : "Delete").then((ok) => {
    if (ok) tryApi("DELETE", `/api/events/${ev.id}?scope=${future ? "future" : "one"}`).catch(() => {});
  });
}

// ---------- The raid card ----------
// Calm and dense, in the spirit of the Discord raid bots: title, one meta
// row, one line of role counts, then the roster grouped by class.

// Small original line icons (16×16), coloured by CSS.
const RC_ICONS = {
  leader: '<path d="M2.5 12.5h11M3 12.5 2 5.5l3.5 2.8L8 3.5l2.5 4.8L14 5.5l-1 7"/>',
  date: '<rect x="2" y="3.5" width="12" height="10.5" rx="2"/><path d="M2 7h12M5.5 2v3M10.5 2v3"/>',
  time: '<circle cx="8" cy="8" r="6"/><path d="M8 4.5V8l2.5 1.5"/>',
  soon: '<path d="M4 2h8M4 14h8M5 2c0 3.5 6 3.5 6 6s-6 2.5-6 6M11 2c0 3.5-6 3.5-6 6s6 2.5 6 6"/>',
  people: '<circle cx="6" cy="5.5" r="2.5"/><path d="M1.5 13.5c.5-2.6 2.2-4 4.5-4s4 1.4 4.5 4"/><path d="M10.5 3.2a2.5 2.5 0 0 1 0 4.6M12 9.8c1.3.5 2.2 1.7 2.5 3.7"/>',
  repeat: '<path d="M3 7V6a2.5 2.5 0 0 1 2.5-2.5H12M10 1.5l2 2-2 2M13 9v1a2.5 2.5 0 0 1-2.5 2.5H4M6 14.5l-2-2 2-2"/>',
  lock: '<rect x="3" y="7" width="10" height="7" rx="1.5"/><path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2"/>',
  mail: '<rect x="2" y="3.5" width="12" height="9" rx="1.5"/><path d="m2.5 4.5 5.5 4 5.5-4"/>',
  warn: '<path d="M8 2 1.5 13.5h13z"/><path d="M8 6.5v3.2M8 11.6v.1"/>',
  late: '<circle cx="8" cy="8" r="5.5"/><path d="M8 5v3h2.5"/>',
  maybe: '<path d="M5.8 6a2.3 2.3 0 1 1 3.3 2.1c-.7.4-1.1.9-1.1 1.7M8 12v.1"/>',
  note: '<path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z"/>',
  more: '<path d="m4.5 6.5 3.5 3.5 3.5-3.5"/>',
  tank: '<path class="fill" d="M8 1.6 2.8 3.4v4.1c0 3 2.1 5.3 5.2 6.8 3.1-1.5 5.2-3.8 5.2-6.8V3.4z"/>',
  healer: '<path class="fill" d="M6.3 2h3.4v4.3H14v3.4H9.7V14H6.3V9.7H2V6.3h4.3z"/>',
  melee: '<path d="M13.5 2.5 6.8 9.2M10.3 2.5h3.2v3.2M4.4 6.8l4.8 4.8M5.6 10.4 2.5 13.5"/>',
  ranged: '<circle cx="8" cy="8" r="5.6"/><circle cx="8" cy="8" r="2.2"/><circle class="fill" cx="8" cy="8" r=".9"/>',
};
function rcIcon(name, cls = "") {
  const span = el("span", { class: `rc-ic ic-${name}${cls ? ` ${cls}` : ""}`, "aria-hidden": "true" });
  span.innerHTML = `<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">${RC_ICONS[name]}</svg>`;
  return span;
}

// Damage dealers who fight up close; everyone else on DPS is ranged.
const MELEE_SPECS = { Warrior: null, Rogue: null, Paladin: null, Shaman: ["Enhancement"], Druid: ["Feral (Cat)", "Feral"] };
function dpsKind(s) {
  if (!(s.class in MELEE_SPECS)) return "ranged";
  const specs = MELEE_SPECS[s.class];
  if (!specs) return "melee";
  return specs.includes(s.spec) ? "melee" : s.spec ? "ranged" : (s.class === "Shaman" ? "melee" : "ranged");
}
// tank / healer / melee / ranged
const fightRole = (s) => s.role === "dps" ? dpsKind(s) : s.role || "ranged";
const FIGHT_LABEL = { tank: "Tank", healer: "Healer", melee: "Melee DPS", ranged: "Ranged DPS" };

const rcDescOpen = new Set();
const rcPlural = (n, one, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

function renderEventCard(m) {
  const ev = m.event;
  const inChat = !!m.id;
  const when = eventWhen(ev);
  const manager = canManageEvent(ev);
  const locked = signupsLocked(ev);
  const notOpen = signupsNotOpen(ev);
  const mine = ev.signups.find((s) => s.user_id === state.me.id);
  const isActive = (s) => s.status === "yes" || s.status === "late";
  const active = ev.signups.filter(isActive);
  const bench = benchOf(ev);
  const absent = ev.signups.filter((s) => s.status === "no");
  const slots = ev.max_tanks + ev.max_healers + ev.max_dps;
  const sr = ev.soft_reserves > 0;
  const reserved = new Set((ev.reserves ?? []).map((r) => r.user_id));
  const start = new Date(ev.starts_at);
  const end = new Date(start.getTime() + ev.duration_minutes * 60000);
  const hm = (d) => d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

  // Sign-up order, like the raid bots number them.
  const order = new Map([...ev.signups].sort((a, b) => new Date(a.signed_at) - new Date(b.signed_at)).map((s, i) => [s.user_id, i + 1]));

  // ---- Header ----
  const leader = memberById(ev.creator_id)?.display_name;
  const meta = (icon, text, attrs = {}) => el("span", { class: "rc-meta-item", ...attrs }, rcIcon(icon), el("span", {}, text));
  const metaRow = el("div", { class: "rc-meta" },
    leader ? meta("leader", leader, { title: "Raid leader" }) : null,
    meta("date", start.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" }), { title: start.toLocaleDateString([], { weekday: "long", day: "numeric", month: "long", year: "numeric" }) }),
    meta("time", `${hm(start)} – ${hm(end)}`, { title: `Your time (${Intl.DateTimeFormat().resolvedOptions().timeZone || "local"})` }),
    meta("soon", when.status, { class: `rc-meta-item ev-rel${when.live ? " live" : ""}${when.over ? " over" : ""}` }),
    meta("people", `${active.length}/${slots}${bench.length ? ` (+${bench.length})` : ""}`, {
      class: "rc-meta-item rc-count",
      title: `${active.length} signed up of ${slots}${bench.length ? `, ${bench.length} on the bench` : ""}`,
    }));

  const closes = ev.signups_close_at ? new Date(ev.signups_close_at) : null;
  const chips = [];
  if (notOpen && !when.over) {
    chips.push(el("span", { class: "rc-chip ev-deadline ev-opens", title: manager ? "As raid leader you can already add people" : "" }, rcIcon("mail"), opensText(ev)));
  } else if (closes && !when.over) {
    chips.push(locked
      ? el("span", { class: "rc-chip ev-deadline locked", title: manager ? "As raid leader you can still make changes" : "" }, rcIcon("lock"), "Sign-ups closed")
      : el("span", { class: "rc-chip ev-deadline", title: closes.toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" }) }, rcIcon("lock"), `Closes ${relativeTime(closes)}`));
  }
  if (manager && !when.over) {
    const key = `${ev.id}|0`;
    const gaps = el("button", { type: "button", class: "rc-chip ev-gaps warn", "data-comp-key": key, hidden: true });
    const ok = el("span", { class: "rc-chip ev-comp-ok", "data-comp-key": key, hidden: true, title: "Every role filled and the key raid buffs covered" }, "✓ Comp complete");
    ok.compFill = () => {
      const c = typeof comp !== "undefined" ? comp.cache.get(key)?.data : null;
      ok.hidden = !c || !!c.missing.length;
    };
    gaps.compFill = () => {
      const c = typeof comp !== "undefined" ? comp.cache.get(key)?.data : null;
      gaps.hidden = !c || !c.missing.length;
      if (!c || !c.missing.length) return;
      gaps.title = `Need: ${c.missing.join(" · ")}`;
      gaps.setAttribute("aria-label", `${rcPlural(c.missing.length, "gap")} in the composition. Show composition`);
      gaps.replaceChildren(rcIcon("warn"), rcPlural(c.missing.length, "gap"));
    };
    gaps.addEventListener("click", () => {
      const d = gaps.closest(".event-card")?.querySelector("details.ev-comp");
      if (!d) return;
      d.open = true;
      d.scrollIntoView({ block: "nearest", behavior: "smooth" });
      d.querySelector("summary")?.focus({ preventScroll: true });
    });
    if (typeof loadComp === "function") { loadComp(ev, false); gaps.compFill(); ok.compFill(); }
    chips.push(gaps, ok);
  }

  const descText = (ev.description || "").trim();
  const descLong = descText.includes("\n") || descText.length > 110;
  const descOpen = rcDescOpen.has(ev.id);
  const desc = descText ? el("div", { class: `rc-desc${descLong && !descOpen ? " clamped" : ""}` },
    el("div", { class: "ev-desc" }, renderText(descOpen || !descLong ? descText : descText.split("\n")[0])),
    descLong ? el("button", { type: "button", class: "rc-more", "aria-expanded": descOpen ? "true" : "false", onclick: () => {
      if (rcDescOpen.has(ev.id)) rcDescOpen.delete(ev.id); else rcDescOpen.add(ev.id);
      redrawEvent(ev);
    } }, descOpen ? "less" : "more") : null) : null;

  const series = !!ev.series_id;
  const menuBtn = manager ? el("button", { class: "icon-btn small rc-menu", title: "Raid options", "aria-label": "Raid options", "aria-haspopup": "menu", onclick: (e) => {
    const r = e.currentTarget.getBoundingClientRect();
    openMenu(r.left, r.bottom + 4, [
      { label: series ? "Edit this raid" : "Edit raid", onClick: () => openRaidDialog(ev) },
      series ? { label: "Edit this and future raids", onClick: () => openRaidDialog(ev, { scope: "future" }) } : null,
      typeof openAddToRaid === "function" && !when.over ? { label: "Add someone…", onClick: () => openAddToRaid(ev) } : null,
      { label: "Copy roster", onClick: () => copy(rosterText(ev), "Roster copied") },
      typeof openRaidExport === "function" ? { label: "Export / invite list", onClick: () => openRaidExport(ev) } : null,
      when.started ? { label: "Take attendance", onClick: () => openAttendanceDialog(ev) } : null,
      "sep",
      { label: series ? "Cancel this raid" : "Delete raid", danger: true, onClick: () => cancelRaid(ev, "one") },
      series ? { label: "Cancel this and future raids", danger: true, onClick: () => cancelRaid(ev, "future") } : null,
    ]);
  } }, "⋯") : null;

  const head = el("div", { class: "rc-head" },
    el("div", { class: "rc-titlebar" },
      el("div", { class: "ev-title" }, ev.title),
      ev.game ? el("span", { class: `ev-game g-${ev.game}`, title: GAME_LABEL[ev.game] ?? ev.game }, GAME_SHORT[ev.game] ?? ev.game) : null,
      ev.repeat ? el("span", { class: "ev-repeat", title: `Repeating raid (${repeatLabel(ev.repeat)}): each one has its own sign-ups` }, rcIcon("repeat"), repeatLabel(ev.repeat)) : null),
    menuBtn);

  // ---- Roles ----
  const by = (pred) => active.filter(pred).length;
  const roleChip = (cls, icon, label, n, max) => el("span", {
    class: `rc-role ${cls}${max != null && n < max && !when.over ? " short" : ""}${max != null && n >= max && max > 0 ? " full" : ""}`,
    title: max != null ? `${label}: ${n} of ${max}${n < max ? `, ${max - n} to go` : ""}` : `${label}: ${n}`,
  }, rcIcon(icon), `${label} `, el("strong", {}, max != null ? `${n}/${max}` : n));
  const dps = by((s) => s.role === "dps");
  const roles = el("div", { class: "rc-roles", "aria-label": "Roles" },
    roleChip("tank", "tank", "Tanks", by((s) => s.role === "tank"), ev.max_tanks),
    roleChip("healer", "healer", "Healers", by((s) => s.role === "healer"), ev.max_healers),
    roleChip("ranged", "ranged", "Ranged", by((s) => s.role === "dps" && dpsKind(s) === "ranged")),
    roleChip("melee", "melee", "Melee", by((s) => s.role === "dps" && dpsKind(s) === "melee")),
    el("span", { class: `rc-role dps-total${dps < ev.max_dps && !when.over ? " short" : ""}`, title: `DPS: ${dps} of ${ev.max_dps}` }, "DPS ", el("strong", {}, `${dps}/${ev.max_dps}`)));

  // ---- Roster, grouped by class ----
  const srTag = (s) => {
    if (!sr) return null;
    if (reserved.has(s.user_id)) return el("span", { class: "rc-sr", title: "Has soft-reserved" }, "SR");
    return manager ? el("span", { class: "rc-sr none", title: "No soft reserve yet" }) : null;
  };
  const entry = (s) => {
    const fr = fightRole(s);
    return el("div", {
      class: `rc-entry st-${s.status}${s.user_id === state.me.id ? " mine" : ""}`,
      "data-role": s.role || "none", "data-user": s.user_id, style: `--cc:${classColour(s.class) ?? "#9a9a9a"}`,
    },
    el("span", { class: `rc-glyph r-${fr}`, title: `${s.spec ? `${s.spec} · ` : ""}${FIGHT_LABEL[fr]}` }, rcIcon(fr), el("span", { class: "sr-only" }, FIGHT_LABEL[fr])),
    el("span", { class: "rc-num", title: `Signed up #${order.get(s.user_id)}` }, order.get(s.user_id)),
    signupName(s, ev),
    s.status === "late" ? el("span", { class: "rc-mark late", title: "Running late" }, rcIcon("late"), el("span", { class: "sr-only" }, " (late)")) : null,
    s.status === "tentative" ? el("span", { class: "rc-mark maybe", title: "Tentative" }, rcIcon("maybe"), el("span", { class: "sr-only" }, " (tentative)")) : null,
    s.note ? el("span", { class: "rc-mark note", title: s.note }, rcIcon("note")) : null,
    srTag(s));
  };
  const listed = ev.signups.filter((s) => isActive(s) || s.status === "tentative")
    .sort((a, b) => order.get(a.user_id) - order.get(b.user_id));
  const classNames = [...WOW_CLASSES.map((c) => c.name), ""];
  const groups = classNames.map((name) => {
    const people = listed.filter((s) => (classNames.includes(s.class) ? s.class : "") === name);
    if (!people.length) return null;
    const colour = classColour(name) ?? "#9a9a9a";
    return el("section", { class: "rc-class", style: `--cc:${colour}`, "data-class": name || "other" },
      el("h4", { class: "rc-class-head" }, el("span", { class: "rc-badge", "aria-hidden": "true" }, (name || "?")[0]),
        `${name || "Other"} `, el("span", { class: "rc-class-n" }, `(${people.length})`)),
      people.map(entry));
  }).filter(Boolean);
  const roster = groups.length ? el("div", { class: "rc-roster ev-roster" }, groups)
    : el("div", { class: "rc-empty" }, when.over ? "Nobody signed up." : notOpen ? "Sign-ups aren't open yet." : "No sign-ups yet. Be the first!");

  const line = (cls, label, people, hint = "", tags = true) => people.length ? el("div", { class: `rc-line ${cls}` },
    el("span", { class: "rc-line-label", title: hint }, `${label} (${people.length}):`), " ",
    ...people.flatMap((s, i) => [i ? el("span", { class: "rc-sep" }, ", ") : null, signupName(s, ev), tags && reserved.has(s.user_id) ? srTag(s) : null])) : null;
  const benchHint = bench.some((s) => s.auto_bench) && !when.over ? "In sign-up order: the first in moves up when a spot opens" : "";

  // ---- Composition (closed by default) ----
  const compSection = typeof raidCompSection === "function" ? raidCompSection(ev) : null;

  // ---- Actions and links ----
  const canSign = !when.over && (!locked || manager) && (!notOpen || manager);
  const signedUp = mine && mine.status !== "no";
  const primary = canSign ? el("button", {
    class: `ev-btn rc-primary${signedUp ? "" : " go"}`,
    onclick: (e) => openSignup(ev, e.currentTarget, mine && mine.status !== "no" && mine.status !== "bench" ? mine.status : "yes"),
  }, signedUp ? "Change sign-up" : "Sign up") : null;
  const moreBtn = canSign ? el("button", {
    class: "ev-btn rc-status", "aria-haspopup": "menu", "aria-label": "Other sign-up options: late, tentative, can't make it",
    title: "Late, tentative, can't make it…",
    onclick: (e) => {
      const btn = e.currentTarget;
      const r = btn.getBoundingClientRect();
      openMenu(r.left, r.bottom + 4, [
        { label: "Running late", checked: mine?.status === "late" || null, onClick: () => openSignup(ev, btn, "late") },
        { label: "Tentative", checked: mine?.status === "tentative" || null, onClick: () => openSignup(ev, btn, "tentative") },
        { label: "Can't make it", checked: mine?.status === "no" || null, onClick: () => setSignup(ev, { status: "no" }) },
        mine ? "sep" : null,
        mine ? { label: "Withdraw", danger: true, onClick: () => tryApi("DELETE", `/api/events/${ev.id}/signups/${state.me.id}`).then(applyEventReply, () => {}) } : null,
      ]);
    },
  }, mine && mine.status !== "yes" ? STATUS_LABEL[mine.status] : "Status", rcIcon("more")) : null;
  const you = !canSign && mine ? el("span", { class: "rc-you" }, `You: ${STATUS_LABEL[mine.status]}`) : null;

  const link = (cls, text, onclick, attrs = {}) => el("button", { type: "button", class: `ev-link ${cls}`, onclick, ...attrs }, text);
  const myRes = (ev.reserves ?? []).filter((r) => r.user_id === state.me.id);
  const srLink = sr && typeof openLoot === "function" ? link(`sr${signedUp && !myRes.length && !ev.reserves_locked ? " want" : ""}`,
    [`Soft reserves (${(ev.reserves ?? []).length})`, signedUp && !myRes.length && !ev.reserves_locked ? el("span", { class: "rc-dot", title: "You haven't reserved yet" }) : null],
    // Your own picks first if you haven't any yet; raid leaders otherwise land on the table.
    () => openLoot(ev, signedUp && !ev.reserves_locked && (!myRes.length || !manager) ? "reserve" : "all"),
    { title: [`Soft reserves: ${[ev.loot_instance_name, `${ev.soft_reserves} each`, typeof lockText === "function" ? lockText(ev) : null].filter(Boolean).join(" · ")}`,
      "The SR tag on the roster marks who has reserved."].join("\n") }) : null;
  const links = [
    srLink,
    manager && when.started && typeof openLoot === "function" ? link("loot", "Loot won", () => openLoot(ev, "won")) : null,
    manager && when.started ? link(`attendance${ev.attendance_taken ? " done" : ""}`, ev.attendance_taken ? "Attendance ✓" : "Take attendance", () => openAttendanceDialog(ev)) : null,
    manager && typeof openRaidExport === "function" && ev.signups.some((s) => s.status !== "no")
      ? link("export", "Invite list", () => openRaidExport(ev), { title: "Names to paste into Method Raid Tools or an /invite macro" }) : null,
    inChat && typeof openCalendar === "function" ? link("calendar-link", "Calendar", () => openCalendar(start)) : null,
  ].filter(Boolean);

  const foot = primary || you || links.length ? el("div", { class: "rc-foot" },
    primary || you ? el("div", { class: "rc-actions ev-actions" }, primary, moreBtn, you) : null,
    links.length ? el("nav", { class: "rc-links", "aria-label": "Raid links" }, links) : null) : null;

  return el("div", {
    class: `event-card rc${when.live ? " live" : ""}${when.over ? " over" : ""}${locked ? " locked" : ""}${notOpen ? " not-open" : ""}${sr ? " sr-on" : ""}`,
    "data-event": ev.id, "data-size": slots,
  },
  head,
  desc,
  metaRow,
  chips.length ? el("div", { class: "rc-chips" }, chips) : null,
  roles,
  roster,
  bench.length || absent.length ? el("div", { class: "rc-lines" },
    line("ev-bench", "Bench", bench, benchHint),
    line("ev-absent", "Absence", absent, "", false)) : null,
  compSection,
  foot);
}

// Redraw one raid wherever it shows (chat card, calendar, raid window).
function redrawEvent(ev) {
  const msg = state.messages.find((x) => x.event?.id === ev.id);
  if (msg) renderMessages();
  if (typeof onEventChanged === "function") onEventChanged(ev);
}

function rosterText(ev) {
  const when = eventWhen(ev);
  const lines = [`${ev.title} — ${when.text}`];
  for (const role of ["tank", "healer", "dps"]) {
    const people = ev.signups.filter((s) => (s.status === "yes" || s.status === "late") && s.role === role);
    lines.push(`${ROLE_INFO[role].label}: ${people.map((s) => `${signupLabel(s)}${s.spec ? ` (${s.spec} ${s.class})` : ""}`).join(", ") || "—"}`);
  }
  for (const [status, label] of [["tentative", "Tentative"], ["bench", "Bench"]]) {
    const people = ev.signups.filter((s) => s.status === status);
    if (people.length) lines.push(`${label}: ${people.map(signupLabel).join(", ")}`);
  }
  return lines.join("\n");
}

async function setSignup(ev, body) {
  const reply = await tryApi("PUT", `/api/events/${ev.id}/signup`, body).catch(() => null);
  if (!reply) return;
  applyEventReply(reply);
  const me = reply.event?.signups.find((s) => s.user_id === state.me.id);
  if (!body.user_id && me?.auto_bench) {
    const place = benchOf(reply.event).filter((s) => s.auto_bench && s.role === me.role).findIndex((s) => s.user_id === state.me.id) + 1;
    toast(`${ROLE_INFO[me.role].label} are full, so you're on the bench (#${place}). You'll move up automatically if a spot opens.`);
  }
}

// A raid changed: its card in chat (if it has one) and any open calendar.
function applyEventReply(reply) {
  if (!reply) return;
  if (reply.id) applyMessageUpdate(reply);
  if (reply.event && typeof onEventChanged === "function") onEventChanged(reply.event);
}

function applyMessageUpdate(msg) {
  const i = state.messages.findIndex((x) => x.id === msg.id);
  if (i >= 0) { state.messages[i] = msg; renderMessages(); }
}

// Pick class and spec (remembered for next time), then sign up.
function openSignup(ev, anchor, status) {
  const mine = ev.signups.find((s) => s.user_id === state.me.id);
  let cls = mine?.class || prefs.get("raid_class", "");
  let spec = mine?.spec || prefs.get("raid_spec", "");
  let role = mine?.role || prefs.get("raid_role", "");
  const note = el("input", { placeholder: "Note (optional): bringing alt, need to leave at 10…", maxlength: 100, value: mine?.note ?? "" });
  const classes = el("div", { class: "su-classes" });
  const specs = el("div", { class: "su-specs" });
  const go = el("button", { class: "primary" }, STATUS_LABEL[status] === "Signed up" ? "Sign up" : `Sign up (${STATUS_LABEL[status].toLowerCase()})`);
  const full = el("div", { class: "su-full muted small" });
  const paint = () => {
    classes.replaceChildren(...WOW_CLASSES.map((c) => el("button", {
      type: "button", class: `su-class${cls === c.name ? " on" : ""}`, style: `--cc:${c.colour}`, title: c.name,
      onclick: () => { cls = c.name; if (!c.specs.some(([n]) => n === spec)) { spec = ""; role = ""; } paint(); },
    }, c.name)));
    const c = WOW_CLASSES.find((x) => x.name === cls);
    specs.replaceChildren(...(c ? c.specs.map(([n, r]) => el("button", {
      type: "button", class: `su-spec${spec === n ? " on" : ""}`,
      onclick: () => { spec = n; role = r; paint(); },
    }, `${ROLE_INFO[r].icon} ${n}`)) : [el("span", { class: "muted small" }, "Pick your class first")]));
    go.disabled = !role;
    // Heads-up when the role is already full (you'd go on the bench).
    const max = { tank: ev.max_tanks, healer: ev.max_healers, dps: ev.max_dps }[role];
    const taken = ev.signups.filter((s) => s.role === role && (s.status === "yes" || s.status === "late") && s.user_id !== state.me.id).length;
    const alreadyIn = mine && mine.role === role && (mine.status === "yes" || mine.status === "late");
    full.textContent = role && (status === "yes" || status === "late") && !alreadyIn && taken >= max
      ? `🪑 ${ROLE_INFO[role].label} are full: you'll go on the bench and move up when a spot opens.` : "";
  };
  const box = el("div", { class: "signup-pop" },
    el("h3", {}, ev.title),
    el("div", { class: "muted small" }, "Class"), classes,
    el("div", { class: "muted small" }, "Spec"), specs,
    full, note, go);
  go.addEventListener("click", async () => {
    prefs.set("raid_class", cls); prefs.set("raid_spec", spec); prefs.set("raid_role", role);
    closePopover();
    // wow.js sets data-character-id when you pick one of your characters.
    await setSignup(ev, { status, role, class: cls, spec, note: note.value.trim(), character_id: box.dataset.characterId || null });
  });
  paint();
  const pop = openPopover(anchor, box, "above");
  // Line it up with the button rather than off to the side.
  const r = anchor.getBoundingClientRect();
  const above = r.top - pop.offsetHeight - 8;
  placeFixed(pop, r.left, above >= 8 ? above : r.bottom + 8);
}

function localInputValue(date) {
  const pad = (n) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}
const localDateValue = (date) => localInputValue(date).slice(0, 10);

const DEADLINES = [[0, "Sign-ups stay open"], [60, "1 hour before"], [120, "2 hours before"], [240, "4 hours before"],
  [720, "12 hours before"], [1440, "1 day before"], [2880, "2 days before"]];

// Text channels in this outhouse you can post a raid in.
const raidChannels = () => state.channels.filter((c) => c.kind === TEXT && can("send_messages", c.id));

const GAME_LABEL = { forever: "WoW: Forever", tbc: "TBC", era: "Classic Era" };
const GAME_SHORT = { forever: "Forever", tbc: "TBC", era: "Era" };
const POST_DAY_CHOICES = [1, 2, 3, 4, 5, 6, 7, 10, 14];
const fullDay = (d) => d.toLocaleString([], { weekday: "short", day: "numeric", month: "short", hour: "numeric", minute: "2-digit" });

// When the sign-up post for a raid starting at `start` goes up under `rule`
// (in your time zone; null: straight away).
function postTimeFor(rule, start) {
  if (!rule || rule.mode === "now" || !start || isNaN(start)) return null;
  const d = new Date(start);
  if (rule.mode === "days") { d.setDate(d.getDate() - rule.days); return d; }
  const back = ((weekdayOf(start) - rule.weekday) % 7 + 7) % 7 || 7;
  const [h, m] = (rule.time || "09:00").split(":").map(Number);
  d.setDate(d.getDate() - back);
  d.setHours(h, m, 0, 0);
  return d;
}

// "Remove the chat card after the raid" (the raid itself always stays).
const CARD_CLEANUP = [["off", "Keep it"], ["after", "3 hours after it ends"], ["next", "When the next raid's sign-ups go up"]];
const CARD_CLEANUP_HINT = {
  off: "The sign-up card stays in chat.",
  after: "The card leaves chat 3 hours after the raid ends. The raid stays in the calendar with its sign-ups and attendance.",
  next: "Each raid's card leaves chat when the next raid's sign-ups go up (if those go up before it has ended, 3 hours after it ends). The raid stays in the calendar with its sign-ups and attendance.",
};
// What the dialog starts on: the outhouse's choice (Outhouse settings → Raids).
function cardCleanupDefault(series) {
  const s = state.raidSettings?.guild_id === state.guildId ? state.raidSettings : null;
  return series ? (s?.card_cleanup_series || "next") : (s?.card_cleanup_single || "off");
}

// The card isn't up yet: sign-ups open later.
const signupsNotOpen = (ev) => !ev.message_id && !!ev.signups_open_at && Date.now() < new Date(ev.signups_open_at).getTime();
const opensText = (ev) => `Sign-ups open ${fullDay(new Date(ev.signups_open_at))}`;

// Plan a raid (optionally on a given day), or edit one. For a raid in a
// series, `scope: "future"` edits it and every later one.
function openRaidDialog(existing = null, { scope = "one", date = null } = {}) {
  const channels = raidChannels();
  if (!existing && !channels.length) return toast("There's no text channel here you can post a raid in.");
  const ev = existing;
  const defaultStart = date ? new Date(date) : new Date();
  if (!date) defaultStart.setDate(defaultStart.getDate() + 1);
  defaultStart.setHours(20, 0, 0, 0);
  const title = el("input", { maxlength: 100, value: ev?.title ?? "", placeholder: "Raid night: Molten Core", required: true });
  const when = el("input", { type: "datetime-local", value: localInputValue(ev ? new Date(ev.starts_at) : defaultStart), required: true });
  const hours = el("select", { class: "select" }, [1, 1.5, 2, 2.5, 3, 3.5, 4, 5, 6].map((h) =>
    el("option", { value: h * 60, selected: (ev?.duration_minutes ?? 180) === h * 60 }, `${h} hour${h === 1 ? "" : "s"}`)));
  const tanks = el("input", { type: "number", min: 0, max: 40, value: ev?.max_tanks ?? RAID_SIZES[0].tanks });
  const healers = el("input", { type: "number", min: 0, max: 40, value: ev?.max_healers ?? RAID_SIZES[0].healers });
  const dps = el("input", { type: "number", min: 0, max: 40, value: ev?.max_dps ?? RAID_SIZES[0].dps });
  const size = el("select", { class: "select" });
  const applySize = () => {
    const s = RAID_SIZES[size.value];
    if (s) { tanks.value = s.tanks; healers.value = s.healers; dps.value = s.dps; }
  };
  // The size list follows the Game picker (built further down).
  const fillSizes = (gameId, pickFirst) => {
    const keep = size.value;
    const list = raidSizesFor(gameId);
    size.replaceChildren(el("option", { value: "" }, "Custom"),
      ...list.map((s) => el("option", { value: RAID_SIZES.indexOf(s) }, s.label)));
    if (pickFirst) { size.value = String(RAID_SIZES.indexOf(list[0])); applySize(); }
    else if ([...size.options].some((o) => o.value === keep)) size.value = keep;
    else size.value = "";
  };
  size.addEventListener("change", applySize);
  [tanks, healers, dps].forEach((n) => n.addEventListener("input", () => (size.value = "")));
  const desc = el("textarea", { rows: 3, maxlength: 1000, placeholder: "Which raid, difficulty, flasks/food, where to meet…" }, ev?.description ?? "");
  const current = ev?.signup_deadline_minutes ?? 0;
  const deadline = el("select", { class: "select", "aria-label": "Sign-ups close" },
    (DEADLINES.some(([m]) => m === current) ? DEADLINES : [...DEADLINES, [current, `${Math.round(current / 60)} hours before`]])
      .map(([m, label]) => el("option", { value: m, selected: m === current }, label)));

  // Where it goes (new raids): this channel, else the last one used.
  // The outhouse's sign-up channel (Outhouse settings → Raids) comes first.
  const remembered = prefs.get(`raid_channel_${state.guildId}`, "");
  const preferred = state.raidSettings?.guild_id === state.guildId ? state.raidSettings.default_channel_id : null;
  const startChannel = channels.find((c) => c.id === preferred) ?? channels.find((c) => c.id === state.channelId) ?? channels.find((c) => c.id === remembered) ?? channels[0];
  const channel = el("select", { class: "select", "aria-label": "Post in" },
    channels.map((c) => el("option", { value: c.id, selected: c.id === startChannel?.id }, `# ${c.name}`)));
  const game = el("select", { class: "select", "aria-label": "Game" },
    el("option", { value: "" }, "Any game"),
    Object.entries(GAME_LABEL).map(([id, label]) => el("option", { value: id, selected: (ev ? ev.game : prefs.get("raid_game", "")) === id }, label)));
  // New raids start on the biggest size for the game; editing keeps the
  // raid's own slots (shown as Custom unless they match a size).
  fillSizes(game.value, !ev);
  if (ev) {
    const match = RAID_SIZES.findIndex((s) => s.tanks === ev.max_tanks && s.healers === ev.max_healers && s.dps === ev.max_dps);
    if (match >= 0 && [...size.options].some((o) => o.value === String(match))) size.value = String(match);
  }
  game.addEventListener("change", () => fillSizes(game.value, size.value === "" ? false : !raidSizesFor(game.value).includes(RAID_SIZES[size.value])));

  // Repeating (new raids only).
  const repeat = el("select", { class: "select", "aria-label": "Repeat" },
    el("option", { value: 0 }, "Doesn't repeat"),
    [1, 2, 3, 4].map((n) => el("option", { value: n }, n === 1 ? "Every week" : `Every ${n} weeks`)));
  const days = new Set([weekdayOf(defaultStart)]);
  let daysTouched = false;
  const dayChips = el("div", { class: "day-chips", role: "group", "aria-label": "Repeats on" });
  const paintDays = () => dayChips.replaceChildren(...WEEKDAYS.map((n, i) => el("button", {
    type: "button", class: `day-chip${days.has(i) ? " on" : ""}`, "aria-pressed": days.has(i) ? "true" : "false",
    onclick: () => { daysTouched = true; days.has(i) ? days.delete(i) : days.add(i); paintDays(); postDefaults(); },
  }, n)));
  paintDays();
  const ends = el("select", { class: "select", "aria-label": "Ends" },
    el("option", { value: "count" }, "After a number of raids"),
    el("option", { value: "until" }, "On a date"),
    el("option", { value: "never" }, "Never"));
  const count = el("input", { type: "number", min: 1, max: 104, value: 8, "aria-label": "Number of raids" });
  const later = new Date(defaultStart); later.setMonth(later.getMonth() + 3);
  const until = el("input", { type: "date", value: localDateValue(later), "aria-label": "Last day" });
  const endsValue = el("label", { class: "field" }, "How many", count);
  ends.addEventListener("change", () => {
    endsValue.replaceChildren(...(ends.value === "count" ? ["How many", count] : ends.value === "until" ? ["Last day", until] : [el("span", { class: "muted small", style: "font-weight:400" }, "Until you cancel it")]));
  });
  const repeatBox = el("div", { class: "repeat-box", hidden: true },
    el("div", { class: "field" }, "Repeats on", dayChips),
    el("div", { class: "raid-row even" }, el("label", { class: "field" }, "Ends", ends), endsValue),
    el("p", { class: "muted small", style: "margin:0" }, "Each raid gets its own sign-ups. Raids are made four weeks ahead; each one's sign-up post goes up when you choose below."));

  // "Edit this and future raids": the repeat pattern can change too (from
  // this raid on). The server shows what that would do before saving.
  const editPattern = !!(ev?.series_id && scope === "future" && ev.repeat);
  let patternTouched = false;
  let lastPreview = null;
  const patternPreview = el("div", { class: "pattern-preview", "aria-live": "polite" });
  if (editPattern) {
    repeat.querySelector('option[value="0"]')?.remove();
    if (![1, 2, 3, 4].includes(ev.repeat.every_weeks)) repeat.append(el("option", { value: ev.repeat.every_weeks }, `Every ${ev.repeat.every_weeks} weeks`));
    repeat.value = String(ev.repeat.every_weeks);
    days.clear();
    for (const d of ev.repeat.weekdays) days.add(d);
    daysTouched = true;
    paintDays();
    ends.value = ev.repeat.count ? "count" : ev.repeat.until ? "until" : "never";
    if (ev.repeat.until) until.value = ev.repeat.until;
    if (ev.repeat.count) count.value = ev.repeat.count;
    // A counted series: how many from this raid on.
    ends.addEventListener("change", () => {
      if (ends.value === "count" && endsValue.firstChild?.nodeType === 3) endsValue.firstChild.textContent = "How many from this raid on";
    });
    ends.dispatchEvent(new Event("change"));
    repeatBox.hidden = false;
  }
  const currentPattern = () => ({
    weekdays: [...days].sort((a, b) => a - b),
    every_weeks: Number(repeat.value),
    count: ends.value === "count" ? Number(count.value) : null,
    until: ends.value === "until" ? until.value : null,
    time_zone: ev?.repeat?.time_zone || Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC",
  });
  let previewTimer = null;
  let previewSeq = 0;
  const runPreview = () => {
    if (!editPattern) return;
    clearTimeout(previewTimer);
    previewTimer = setTimeout(async () => {
      const seq = ++previewSeq;
      if (!days.size) { patternPreview.replaceChildren(el("p", { class: "error" }, "Pick at least one day for it to repeat on.")); return; }
      const body = { starts_at: when.value ? new Date(when.value).toISOString() : ev.starts_at };
      if (patternTouched) body.repeat = currentPattern();
      let p;
      try {
        p = await api("POST", `/api/events/${ev.id}/repeat-preview`, body);
      } catch (err) {
        if (seq === previewSeq) { lastPreview = null; patternPreview.replaceChildren(el("p", { class: "error" }, err.message)); }
        return;
      }
      if (seq !== previewSeq || !dialog.isConnected) return;
      lastPreview = p;
      // What's left of a counted series, from this raid on.
      if (!patternTouched && p.remaining_count && ends.value === "count") count.value = p.remaining_count;
      const warn = patternTouched && p.removed_raids
        ? el("p", { class: "pattern-warn" }, `⚠️ ${p.removed_raids} upcoming raid${p.removed_raids === 1 ? "" : "s"} no longer fit${p.removed_raids === 1 ? "s" : ""} and will be cancelled`,
          p.removed_signups ? `, dropping ${p.removed_signups} sign-up${p.removed_signups === 1 ? "" : "s"}. Everyone signed up gets a DM from the Outhouse Helper.` : " (nobody has signed up for them yet).")
        : null;
      const kept = patternTouched && p.kept_raids
        ? el("p", { class: "muted small", style: "margin:0" }, `${p.kept_raids} raid${p.kept_raids === 1 ? "" : "s"} on the same day stay${p.kept_raids === 1 ? "s" : ""}${p.kept_signups ? `, keeping ${p.kept_signups} sign-up${p.kept_signups === 1 ? "" : "s"}` : ""}.`)
        : null;
      patternPreview.replaceChildren(
        el("div", { class: "pp-head" }, `🔁 ${repeatLabel(p.repeat)}`, el("span", { class: "muted small" }, " · next raids:")),
        el("ol", { class: "pp-dates" }, p.next.map((d) => el("li", {}, fullDay(new Date(d))))),
        warn, kept,
        patternTouched && !p.first_of_series ? el("p", { class: "muted small", style: "margin:0" }, "Raids before this one keep the old pattern (and their sign-ups and attendance).") : null);
    }, 250);
  };
  if (editPattern) {
    const touch = () => { patternTouched = true; runPreview(); };
    dayChips.addEventListener("click", touch);
    [repeat, ends, count, until].forEach((n) => { n.addEventListener("change", touch); n.addEventListener("input", touch); });
    when.addEventListener("change", runPreview);
    runPreview();
  }

  // When the sign-up post goes up.
  const rule = ev?.post_rule ?? null;
  let postTouched = false;
  const postMode = el("select", { class: "select", "aria-label": "Post sign-ups" },
    el("option", { value: "now" }, "Right away"),
    el("option", { value: "days" }, "Some days before"),
    el("option", { value: "weekday" }, "On a set day"));
  postMode.value = rule?.mode ?? "now";
  const postDays = el("select", { class: "select", "aria-label": "Days before" },
    POST_DAY_CHOICES.map((n) => el("option", { value: n }, `${n} day${n === 1 ? "" : "s"} before`)));
  if (rule?.days && !POST_DAY_CHOICES.includes(rule.days)) postDays.append(el("option", { value: rule.days }, `${rule.days} days before`));
  postDays.value = rule?.days ?? 7;
  const postDay = el("select", { class: "select", "aria-label": "Post on" },
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"].map((n, i) => el("option", { value: i }, `On ${n}`)));
  postDay.value = rule?.weekday ?? 0;
  const postTime = el("input", { type: "time", value: rule?.time ?? "09:00", "aria-label": "Post at" });
  const postPreview = el("p", { class: "muted small post-preview", style: "margin:0" });
  const postExtra = el("div", { class: "post-extra" });
  const currentRule = () => postMode.value === "days" ? { mode: "days", days: Number(postDays.value) }
    : postMode.value === "weekday" ? { mode: "weekday", weekday: Number(postDay.value), time: postTime.value || "09:00" }
    : { mode: "now" };
  const paintPost = () => {
    postExtra.replaceChildren(...(postMode.value === "days" ? [postDays] : postMode.value === "weekday" ? [postDay, postTime] : []));
    postExtra.hidden = postMode.value === "now";
    const start = when.value ? new Date(when.value) : null;
    const at = postTimeFor(currentRule(), start);
    const series = !ev && repeat.value !== "0";
    postPreview.textContent = !at
      ? (series ? "Each raid's sign-up post goes up as soon as the raid is made (four weeks ahead)." : "The sign-up post goes up in chat straight away.")
      : at <= new Date()
        ? (series ? "For the first raid that time has passed, so its post goes up straight away; later raids follow this." : "That time has already passed, so the sign-up post goes up straight away.")
        : `${series ? "The first raid's" : "The"} sign-up post goes up ${fullDay(at)}. Until then the raid is in the calendar and sign-ups are closed.`;
  };
  // Repeating raids: by default the day after the last raid day of the week, at 9am.
  const postDefaults = () => {
    if (ev || postTouched) return paintPost();
    if (repeat.value === "0") { postMode.value = "now"; return paintPost(); }
    const sorted = [...days].sort((a, b) => a - b);
    postMode.value = "weekday";
    postDay.value = ((sorted.length ? sorted[sorted.length - 1] : weekdayOf(new Date(when.value))) + 1) % 7;
    postTime.value = "09:00";
    paintPost();
  };
  [postMode, postDays, postDay, postTime].forEach((n) => n.addEventListener("input", () => { postTouched = true; paintPost(); }));
  postMode.addEventListener("change", () => { postTouched = true; paintPost(); });
  when.addEventListener("change", () => {
    if (!daysTouched && when.value) { days.clear(); days.add(weekdayOf(new Date(when.value))); paintDays(); }
    postDefaults();
  });
  repeat.addEventListener("change", () => { repeatBox.hidden = repeat.value === "0"; postDefaults(); });
  paintPost();
  // Editing one raid whose post is already up (or was taken down): nothing to choose.
  const postedAlready = ev && (ev.message_id || ev.card_removed) && scope !== "future";
  const postField = postedAlready ? null : el("div", { class: "field post-field" }, "Post sign-ups",
    el("div", { class: "post-row" }, postMode, postExtra), postPreview);

  // Removing the chat card after the raid.
  const isSeries = () => (ev ? !!ev.series_id : repeat.value !== "0");
  let cleanupTouched = false;
  const cleanup = el("select", { class: "select", "aria-label": "Remove the chat card after the raid" });
  const cleanupHint = el("p", { class: "muted small cleanup-hint", style: "margin:0" });
  const paintCleanup = (value) => {
    const allowed = CARD_CLEANUP.filter(([v]) => v !== "next" || isSeries());
    const want = allowed.some(([v]) => v === value) ? value : cardCleanupDefault(isSeries());
    cleanup.replaceChildren(...allowed.map(([v, label]) => el("option", { value: v, selected: v === want }, label)));
    cleanup.value = want;
    cleanupHint.textContent = CARD_CLEANUP_HINT[cleanup.value] ?? "";
  };
  paintCleanup(ev ? (ev.card_cleanup || "off") : cardCleanupDefault(isSeries()));
  cleanup.addEventListener("change", () => { cleanupTouched = true; paintCleanup(cleanup.value); });
  repeat.addEventListener("change", () => paintCleanup(cleanupTouched ? cleanup.value : cardCleanupDefault(isSeries())));
  const cardGone = ev?.card_removed && scope !== "future";
  const cleanupField = cardGone
    ? el("p", { class: "muted small cleanup-gone", style: "margin:0" }, "🧹 This raid's chat card has been removed. The raid, its sign-ups and attendance are still here.")
    : el("div", { class: "field cleanup-field" },
      el("label", { class: "cleanup-label" }, "Remove the chat card after the raid", cleanup), cleanupHint);

  // Loot list and soft reserves (loot.js).
  const lootFields = typeof raidLootFields === "function" ? raidLootFields(ev, title, game) : null;

  const heading = !ev ? "Plan a raid" : !ev.series_id ? "Edit raid" : scope === "future" ? "Edit this and future raids" : "Edit this raid";
  const form = el("form", { method: "dialog" },
    el("h2", {}, heading),
    ev?.series_id ? el("p", { class: "muted small", style: "margin:0" }, scope === "future"
      ? "Changes apply to this raid and every later one in the series. Moving the time moves them all by the same amount. Changing the days or how often it repeats starts the new pattern from this raid; earlier raids stay as they were."
      : "Only this raid changes; the rest of the series stays as it is.") : null,
    el("label", { class: "field" }, "Name", title),
    ev ? null : el("div", { class: "raid-row even" },
      el("label", { class: "field" }, "Post in", channel),
      el("label", { class: "field" }, "Game", game)),
    ev ? el("label", { class: "field" }, "Game", game) : null,
    el("div", { class: "raid-row" },
      el("label", { class: "field" }, "Starts (your time)", when),
      el("label", { class: "field" }, "Length", hours)),
    ev && !editPattern ? null : el("label", { class: "field" }, "Repeat", repeat),
    ev && !editPattern ? null : repeatBox,
    editPattern ? patternPreview : null,
    postField,
    cleanupField,
    el("div", { class: "raid-row even" },
      el("label", { class: "field" }, "Size", size),
      el("label", { class: "field" }, "Sign-ups close", deadline)),
    el("div", { class: "raid-row three" },
      el("label", { class: "field" }, "🛡️ Tanks", tanks),
      el("label", { class: "field" }, "✚ Healers", healers),
      el("label", { class: "field" }, "⚔️ DPS", dps)),
    lootFields?.node,
    el("label", { class: "field" }, "Details", desc),
    el("p", { class: "muted small", style: "margin:0" }, "When a role fills up, extra sign-ups go on the bench and move up automatically. Everyone signed up gets an @mention 15 minutes before it starts."),
    el("div", { class: "row" },
      el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
      el("button", { type: "submit", class: "primary", value: "ok" }, ev ? "Save" : "Post raid")),
  );
  form.addEventListener("submit", (e) => {
    if ((!ev || editPattern) && repeat.value !== "0" && !days.size) { e.preventDefault(); toast("Pick at least one day for it to repeat on."); }
  });
  const dialog = el("dialog", { class: "raid-dialog" }, form);
  dialog.addEventListener("close", async () => {
    const ok = dialog.returnValue === "ok";
    dialog.remove();
    if (!ok) return;
    const tz = Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
    const body = {
      title: title.value.trim(),
      description: desc.value.trim(),
      starts_at: new Date(when.value).toISOString(),
      duration_minutes: Number(hours.value),
      max_tanks: Number(tanks.value), max_healers: Number(healers.value), max_dps: Number(dps.value),
      signup_deadline_minutes: Number(deadline.value),
      game: game.value,
    };
    if (postField && (!ev || postTouched)) body.post_rule = { ...currentRule(), time_zone: tz };
    if (!cardGone && (!ev || cleanupTouched)) body.card_cleanup = cleanup.value;
    if (lootFields) Object.assign(body, lootFields.changes());
    if (ev) {
      body.scope = scope;
      if (editPattern && patternTouched) body.repeat = currentPattern();
      const reply = await tryApi("PATCH", `/api/events/${ev.id}`, body).catch(() => null);
      applyEventReply(reply);
      if (reply && body.repeat) {
        const ev2 = reply.event;
        const gone = lastPreview?.removed_raids ?? 0;
        toast(`Repeating raid changed: ${repeatLabel(ev2?.repeat ?? body.repeat)}.${gone ? ` ${gone} raid${gone === 1 ? "" : "s"} cancelled.` : ""}`);
      }
      return;
    }
    if (repeat.value !== "0") {
      body.repeat = {
        weekdays: [...days].sort((a, b) => a - b),
        every_weeks: Number(repeat.value),
        count: ends.value === "count" ? Number(count.value) : null,
        until: ends.value === "until" ? until.value : null,
        time_zone: tz,
      };
    }
    const channelId = channel.value;
    prefs.set(`raid_channel_${state.guildId}`, channelId);
    prefs.set("raid_game", game.value);
    const reply = await tryApi("POST", `/api/channels/${channelId}/events`, body).catch(() => null);
    if (!reply) return;
    if (reply.id && reply.channel_id === state.channelId && !findMessage(reply.id)) {
      state.messages.push(reply);
      renderMessages.forceBottom = true;
      renderMessages();
    }
    const ev2 = reply.event;
    const where = channelId !== state.channelId ? ` in #${state.channels.find((c) => c.id === channelId)?.name ?? "channel"}` : "";
    if (body.repeat) toast(`Repeating raid planned${where}: ${repeatLabel(ev2.repeat)}.${ev2 && signupsNotOpen(ev2) ? ` ${opensText(ev2)}.` : ""}`);
    else if (ev2 && signupsNotOpen(ev2)) toast(`Raid planned${where}. ${opensText(ev2)}.`);
    else if (!reply.id || where) toast(`Raid posted${where}.`);
    applyEventReply(reply);
  });
  document.body.append(dialog);
  dialog.showModal();
  if (!ev) title.focus();
}

// Open a raid: jump to its card in chat, or show it in a window if its card isn't up yet.
async function openRaid(ev) {
  if (!ev.message_id) return openRaidCard(ev);
  if (state.channelId !== ev.channel_id) await selectChannel(ev.channel_id);
  setTimeout(() => jumpTo(ev.message_id), 300);
}

// The calendar button: upcoming raids in this outhouse, plus "Plan a raid".
async function openRaidList(anchor) {
  const box = el("div", { class: "raid-list" }, el("p", { class: "muted" }, "Loading…"));
  openPopover(anchor, box, "above");
  const events = await api("GET", `/api/guilds/${state.guildId}/events`).catch(() => []);
  const rows = events.slice(0, 12).map((ev) => {
    const when = eventWhen(ev);
    const mine = ev.signups.find((s) => s.user_id === state.me.id);
    const ch = state.channels.find((c) => c.id === ev.channel_id);
    return el("button", { class: "raid-row-btn", onclick: () => { closePopover(); openRaid(ev); } },
      el("div", { class: "rl-title" }, ev.title, ev.series_id ? el("span", { class: "rl-tag", title: repeatLabel(ev.repeat) }, "🔁") : null,
        signupsLocked(ev) ? el("span", { class: "rl-tag", title: "Sign-ups closed" }, "🔒") : null,
        signupsNotOpen(ev) ? el("span", { class: "rl-tag", title: opensText(ev) }, "📬") : null,
        mine && mine.status !== "no" ? el("span", { class: "rl-mine" }, `✓ ${STATUS_LABEL[mine.status]}`) : null),
      el("div", { class: "muted small" }, `${when.text} (${when.status})${ch ? ` · #${ch.name}` : ""}${signupsNotOpen(ev) ? ` · sign-ups open ${fullDay(new Date(ev.signups_open_at))}` : ""}`));
  });
  box.replaceChildren(
    el("div", { class: "rl-head" }, el("h3", {}, "Upcoming raids"),
      el("div", { class: "rl-tools" },
        el("button", { type: "button", class: "rl-tool", onclick: () => { closePopover(); openCalendar(); } }, "📅 Calendar"),
        el("button", { type: "button", class: "rl-tool", onclick: () => { closePopover(); openAttendanceTable(); } }, "📊 Attendance"),
        typeof openLootLog === "function" ? el("button", { type: "button", class: "rl-tool", onclick: () => { closePopover(); openLootLog(); } }, "📜 Loot") : null)),
    rows.length ? el("div", { class: "rl-rows" }, rows) : el("p", { class: "muted small" }, "Nothing planned yet."),
    ...[
      events.length > rows.length ? el("button", { type: "button", class: "link", onclick: () => { closePopover(); openCalendar(); } }, `+${events.length - rows.length} more in the calendar`) : null,
      raidChannels().length ? el("button", { class: "primary", onclick: () => { closePopover(); openRaidDialog(); } }, "＋ Plan a raid") : null,
    ].filter(Boolean),
  );
  // It grew: line it up above the button again.
  const pop = box.closest(".popover");
  if (!pop) return;
  const r = anchor.getBoundingClientRect();
  const pr = pop.getBoundingClientRect();
  placeFixed(pop, Math.min(r.right - pr.width, window.innerWidth - pr.width - 8), r.top - pr.height - 8);
}

$("raid-btn").addEventListener("click", (e) => {
  if (popover) return closePopover();
  if (!state.guildId) return;
  openRaidList(e.currentTarget);
});

// Relative times on raid cards ("in 3 hours") stay fresh.
setInterval(() => {
  if (state.messages.some((m) => m.event) && !document.hidden && !state.editingId) renderMessages();
}, 60_000);

// ======================================================================
// Bots and slash commands
// ======================================================================

state.commands = [];

async function loadCommands() {
  const guildId = state.guildId;
  const list = await api("GET", `/api/guilds/${guildId}/commands`).catch(() => []);
  if (state.guildId === guildId) state.commands = list;
}

// "/roll 2d6" → the roll command, if a bot here offers it.
function matchCommand(text) {
  const m = text.match(/^\/([a-z0-9_-]+)(?:\s|$)/);
  return m ? state.commands.find((c) => c.name === m[1]) ?? null : null;
}

// Options as `name:value` pairs; or, with one option, everything after the command.
function parseOptions(command, text) {
  const rest = text.replace(/^\/[a-z0-9_-]+\s*/, "");
  const options = {};
  const names = command.options.map((o) => o.name);
  const re = /([a-z0-9_-]+):\s*("([^"]*)"|(.*?))(?=\s+[a-z0-9_-]+:|$)/g;
  let found = false;
  for (const m of rest.matchAll(re)) {
    if (!names.includes(m[1])) continue;
    options[m[1]] = (m[3] ?? m[4] ?? "").trim();
    found = true;
  }
  if (!found && rest.trim() && names.length) options[names[0]] = rest.trim();
  return options;
}

async function runCommand(command, text) {
  await api("POST", `/api/channels/${state.channelId}/interactions`, {
    command_id: command.id,
    options: parseOptions(command, text),
  });
}

// The "/" menu above the chat box.
const commandPop = el("div", { class: "command-pop", hidden: true });
$("composer-wrap").prepend(commandPop);
let commandIndex = 0;
let commandMatches = [];

function usage(c) {
  return `/${c.name}${c.options.map((o) => ` ${o.required ? "" : "["}${o.name}:…${o.required ? "" : "]"}`).join("")}`;
}

function updateCommandPop() {
  const v = input.value;
  const typing = v.match(/^\/([a-z0-9_-]*)$/);
  const chosen = matchCommand(v);
  if (typing && state.commands.length) {
    commandMatches = state.commands.filter((c) => c.name.startsWith(typing[1]));
    commandIndex = Math.min(commandIndex, Math.max(0, commandMatches.length - 1));
    commandPop.replaceChildren(
      el("div", { class: "cp-head" }, "COMMANDS"),
      ...commandMatches.map((c, i) => el("button", {
        type: "button", class: `cp-item${i === commandIndex ? " on" : ""}`,
        onmousedown: (e) => { e.preventDefault(); pickCommand(c); },
      }, el("span", { class: "cp-name" }, usage(c)), el("span", { class: "cp-desc" }, c.description), el("span", { class: "cp-bot" }, c.bot_name))));
    commandPop.hidden = !commandMatches.length;
  } else if (chosen && chosen.options.length) {
    commandMatches = [];
    commandPop.replaceChildren(el("div", { class: "cp-hint" },
      el("strong", {}, usage(chosen)), " — ", chosen.description || chosen.bot_name,
      el("div", { class: "muted small" }, chosen.options.map((o) => `${o.name}: ${o.description || ""}${o.required ? " (required)" : ""}`).join(" · "))));
    commandPop.hidden = false;
  } else {
    commandMatches = [];
    commandPop.hidden = true;
  }
}

function pickCommand(c) {
  input.value = `/${c.name} `;
  input.focus();
  autosize();
  updateCommandPop();
}

input.addEventListener("input", updateCommandPop);
input.addEventListener("blur", () => setTimeout(() => { if (document.activeElement !== input) commandPop.hidden = true; }, 150));
input.addEventListener("keydown", (e) => {
  if (commandPop.hidden || !commandMatches.length) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    e.stopImmediatePropagation();
    commandIndex = (commandIndex + (e.key === "ArrowDown" ? 1 : -1) + commandMatches.length) % commandMatches.length;
    updateCommandPop();
  } else if (e.key === "Enter" || e.key === "Tab") {
    e.preventDefault();
    e.stopImmediatePropagation();
    pickCommand(commandMatches[commandIndex]);
  } else if (e.key === "Escape") {
    commandPop.hidden = true;
  }
}, true);
$("composer").addEventListener("submit", () => setTimeout(updateCommandPop, 0));

async function drawBots(main) {
  const bots = await api("GET", `/api/guilds/${state.guildId}/bots`).catch((err) => { toast(err.message); return []; });
  const redraw = () => { main.replaceChildren(); drawBots(main); };
  const showToken = (res, fresh) => {
    const snippet = `$env:OUTHOUSE_URL="${location.origin}"; $env:OUTHOUSE_BOT_TOKEN="${res.token}"; node dice-bot.mjs`;
    ask(fresh ? `${res.bot.display_name} is ready` : "New token", {
      value: res.token,
      readOnly: true,
      body: el("div", { class: "muted small", style: "display:grid;gap:8px" },
        el("p", { style: "margin:0" }, "This token is the bot's password. It's only shown now: copy it somewhere safe. Anyone with it can post as the bot."),
        el("p", { style: "margin:0" }, "To try the example dice bot (needs Node.js 22+), download ",
          el("a", { href: "/bots/dice-bot.mjs", download: "dice-bot.mjs" }, "dice-bot.mjs"), " and run this in the same folder (PowerShell):"),
        el("pre", { class: "code-snippet" }, snippet),
        el("button", { type: "button", class: "ghost", onclick: () => copy(snippet, "Command copied") }, "Copy command")),
    });
  };
  const name = el("input", { placeholder: "Dice, Raid Helper, News…", maxlength: 32 });
  main.append(
    el("h2", {}, "Bots"),
    el("p", { class: "muted", style: "margin:0" }, "Bots are accounts your own code controls: they can post, react and offer slash commands like /roll. Give them roles like anyone else."),
    el("div", { class: "int-form", style: "grid-template-columns: 1fr auto" },
      name,
      el("button", { class: "primary", onclick: async () => {
        if (!name.value.trim()) return name.focus();
        const res = await tryApi("POST", `/api/guilds/${state.guildId}/bots`, { name: name.value.trim() }).catch(() => null);
        if (!res) return;
        redraw();
        showToken(res, true);
      } }, "New bot")),
    ...(bots.length ? bots.map((b) => el("div", { class: "int-item" },
      el("div", { class: "inline-row" },
        avatarFor(b.user.id, b.user.display_name, 36, b.user.avatar_url || undefined),
        el("div", {},
          el("div", {}, el("strong", {}, b.user.display_name), " ", el("span", { class: `bot-state${b.online ? " on" : ""}` }, b.online ? "● running" : "○ not running")),
          el("div", { class: "muted small" }, b.commands.length ? b.commands.map((c) => `/${c}`).join("  ") : "No slash commands yet"))),
      el("div", { class: "inline-row" },
        el("button", { class: "ghost", onclick: async () => {
          if (!(await confirmBox("Reset token?", `The old token stops working straight away; ${b.user.display_name} needs the new one.`, "Reset"))) return;
          const res = await tryApi("POST", `/api/bots/${b.user.id}/token`).catch(() => null);
          if (res) showToken(res, false);
        } }, "Reset token"),
        el("button", { class: "ghost danger", onclick: async () => {
          if (!(await confirmBox("Remove bot?", `${b.user.display_name} leaves every outhouse and its token stops working. Its old messages stay.`, "Remove"))) return;
          await tryApi("DELETE", `/api/bots/${b.user.id}`).catch(() => {});
          redraw();
        } }, "Remove")),
    )) : [el("p", { class: "muted" }, "No bots yet.")]),
  );
}

// ======================================================================
// Phones and tablets
// ======================================================================

const isTouch = () => matchMedia("(pointer: coarse)").matches;

// A big "hold to talk" button in the voice panel on touch screens.
function touchPttButton() {
  if (!isTouch() || !usingPtt() || !state.voice.canSpeak) return null;
  const btn = el("button", { class: `ptt-hold${state.voice.pttDown ? " on" : ""}` }, state.voice.pttDown ? "Talking…" : "Hold to talk");
  btn.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    setPtt(true);
    const release = () => { setPtt(false); window.removeEventListener("pointerup", release); window.removeEventListener("pointercancel", release); };
    window.addEventListener("pointerup", release);
    window.addEventListener("pointercancel", release);
  });
  btn.addEventListener("contextmenu", (e) => e.preventDefault());
  return btn;
}

if (window.__OUTHOUSE_PLATFORM__ === "android") document.documentElement.classList.add("android-app");

// The Android app's "new version" offer is the orange update button (updates.js).

// Slide-out panels for the channel list and member list on narrow screens.
(function mobileLayout() {
  const app = $("app");
  const toggle = (cls) => app.classList.toggle(cls);
  const close = () => app.classList.remove("show-nav", "show-members");
  const header = document.querySelector(".chat-head");
  if (header) {
    header.prepend(el("button", { class: "icon-btn mobile-only", id: "nav-toggle", "aria-label": "Channels", onclick: () => toggle("show-nav") }, "☰"));
    header.append(el("button", { class: "icon-btn mobile-only", id: "members-toggle", "aria-label": "Members", onclick: () => toggle("show-members") }, "👥"));
  }
  app.append(el("div", { class: "mobile-scrim", onclick: close }));
  // Picking a channel closes the drawer.
  document.addEventListener("click", (e) => {
    if (matchMedia("(max-width: 760px)").matches && e.target.closest(".channel:not(.voice), .guild")) setTimeout(close, 0);
  });
})();

// Installable app (Add to Home screen on Android).
if ("serviceWorker" in navigator && location.protocol === "https:") {
  navigator.serviceWorker.register("/sw.js").catch(() => {});
}
