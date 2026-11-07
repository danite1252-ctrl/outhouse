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
];
const themeCss = (key) => PROFILE_THEMES.find((t) => t.key === key)?.css ?? null;

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
  const card = el("div", { class: `profile-card${theme ? " themed" : ""}`, style: theme ? `--theme:${theme}` : (p.accent_color ? `--accent-ring:${hexColour(p.accent_color)}` : "") },
    el("div", { class: "pc-banner", style: `background:${profileBanner(p)}` }),
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
          : el("button", { class: "ghost", onclick: () => { closePopover(); input.value += `@${shownName} `; input.focus(); autosize(); } }, "Mention")),
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
      type: "button", class: `theme-swatch${draft.profile_theme === t.key ? " on" : ""}`, title: t.name, style: `background:${t.css}`,
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

const RAID_SIZES = [
  { label: "40-player raid", tanks: 5, healers: 10, dps: 25 },
  { label: "20-player raid", tanks: 3, healers: 5, dps: 12 },
  { label: "10-player raid", tanks: 2, healers: 3, dps: 5 },
];

function canManageEvent(ev) {
  return ev.creator_id === state.me.id || can("manage_events", ev.channel_id);
}

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
  return { text: `${day} · ${t(start)} – ${t(end)}`, status, live: now >= start && now <= end, over: now > end };
}

function signupName(s, ev) {
  const colour = classColour(s.class);
  const node = el("span", {
    class: `su-name${s.user_id === state.me.id ? " me" : ""}`,
    style: colour ? `color:${colour}` : "",
    title: [s.class && `${s.spec} ${s.class}`.trim(), s.note].filter(Boolean).join(" — ") || STATUS_LABEL[s.status],
  }, memberById(s.user_id)?.display_name ?? s.display_name, s.status === "late" ? " ⏱" : "", s.note ? " 💬" : "");
  if (canManageEvent(ev)) {
    node.classList.add("manageable");
    node.addEventListener("click", (e) => {
      const r = e.currentTarget.getBoundingClientRect();
      const set = (status) => tryApi("PUT", `/api/events/${ev.id}/signup`, { ...s, status, user_id: s.user_id }).catch(() => {});
      openMenu(r.left, r.bottom + 4, [
        s.status !== "yes" ? { label: "Confirm", onClick: () => set("yes") } : null,
        s.status !== "bench" ? { label: "Move to bench", onClick: () => set("bench") } : null,
        s.status !== "tentative" ? { label: "Mark tentative", onClick: () => set("tentative") } : null,
        "sep",
        { label: "Remove from raid", danger: true, onClick: () => tryApi("DELETE", `/api/events/${ev.id}/signups/${s.user_id}`).catch(() => {}) },
      ]);
    });
  }
  return node;
}

function renderEventCard(m) {
  const ev = m.event;
  const when = eventWhen(ev);
  const mine = ev.signups.find((s) => s.user_id === state.me.id);
  const active = ev.signups.filter((s) => s.status === "yes" || s.status === "late");
  const column = (role, max) => {
    const people = active.filter((s) => s.role === role);
    return el("div", { class: `ev-col ${role}` },
      el("div", { class: "ev-col-head" }, `${ROLE_INFO[role].icon} ${ROLE_INFO[role].label}`,
        el("span", { class: `ev-count${people.length >= max && max > 0 ? " full" : ""}` }, `${people.length}/${max}`)),
      people.length ? people.map((s, i) => el("div", { class: `ev-person${i >= max ? " over" : ""}` }, signupName(s, ev),
        s.spec ? el("span", { class: "ev-spec" }, s.spec) : null)) : el("div", { class: "ev-empty" }, "—"));
  };
  const list = (status, label) => {
    const people = ev.signups.filter((s) => s.status === status);
    return people.length ? el("div", { class: "ev-line" }, el("strong", {}, `${label} (${people.length}): `), ...people.flatMap((s, i) => [i ? ", " : "", signupName(s, ev)])) : null;
  };
  const quick = (status, label) => el("button", {
    class: `ev-btn${mine?.status === status ? " on" : ""}`,
    onclick: (e) => status === "no" ? setSignup(ev, { status: "no" }) : openSignup(ev, e.currentTarget, status),
  }, label);
  const total = active.length;
  const slots = ev.max_tanks + ev.max_healers + ev.max_dps;

  return el("div", { class: `event-card${when.live ? " live" : ""}${when.over ? " over" : ""}` },
    el("div", { class: "ev-head" },
      el("div", {},
        el("div", { class: "ev-kicker" }, when.live ? "● HAPPENING NOW" : when.over ? "FINISHED" : "RAID SIGN-UP"),
        el("div", { class: "ev-title" }, ev.title),
        el("div", { class: "ev-when" }, `🗓 ${when.text}`, el("span", { class: "ev-rel" }, ` (${when.status})`))),
      el("div", { class: "ev-total", title: "Signed up / slots" }, el("strong", {}, total), `/${slots}`),
      canManageEvent(ev) ? el("button", { class: "icon-btn small", title: "Raid options", onclick: (e) => {
        const r = e.currentTarget.getBoundingClientRect();
        openMenu(r.left, r.bottom + 4, [
          { label: "Edit raid", onClick: () => openRaidDialog(ev) },
          { label: "Copy roster", onClick: () => copy(rosterText(ev), "Roster copied") },
          "sep",
          { label: "Delete raid", danger: true, onClick: async () => { if (await confirmBox("Delete raid?", `"${ev.title}" and all its sign-ups will be removed.`, "Delete")) tryApi("DELETE", `/api/events/${ev.id}`).catch(() => {}); } },
        ]);
      } }, "⋯") : null),
    ev.description ? el("p", { class: "ev-desc" }, renderText(ev.description)) : null,
    el("div", { class: "ev-cols" }, column("tank", ev.max_tanks), column("healer", ev.max_healers), column("dps", ev.max_dps)),
    list("tentative", "Tentative"), list("bench", "Bench"), list("no", "Can't make it"),
    when.over ? null : el("div", { class: "ev-actions" },
      quick("yes", mine && mine.status !== "no" ? "Change sign-up" : "Sign up"),
      quick("late", "Late"),
      quick("tentative", "Tentative"),
      quick("no", "Can't make it"),
      mine ? el("button", { class: "ev-btn ghost", onclick: () => tryApi("DELETE", `/api/events/${ev.id}/signups/${state.me.id}`).catch(() => {}) }, "Withdraw") : null),
  );
}

function rosterText(ev) {
  const when = eventWhen(ev);
  const lines = [`${ev.title} — ${when.text}`];
  for (const role of ["tank", "healer", "dps"]) {
    const people = ev.signups.filter((s) => (s.status === "yes" || s.status === "late") && s.role === role);
    lines.push(`${ROLE_INFO[role].label}: ${people.map((s) => `${memberById(s.user_id)?.display_name ?? s.display_name}${s.spec ? ` (${s.spec} ${s.class})` : ""}`).join(", ") || "—"}`);
  }
  for (const [status, label] of [["tentative", "Tentative"], ["bench", "Bench"]]) {
    const people = ev.signups.filter((s) => s.status === status);
    if (people.length) lines.push(`${label}: ${people.map((s) => memberById(s.user_id)?.display_name ?? s.display_name).join(", ")}`);
  }
  return lines.join("\n");
}

async function setSignup(ev, body) {
  const msg = await tryApi("PUT", `/api/events/${ev.id}/signup`, body).catch(() => null);
  if (msg) applyMessageUpdate(msg);
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
  };
  go.addEventListener("click", async () => {
    prefs.set("raid_class", cls); prefs.set("raid_spec", spec); prefs.set("raid_role", role);
    closePopover();
    await setSignup(ev, { status, role, class: cls, spec, note: note.value.trim() });
  });
  paint();
  const pop = openPopover(anchor, el("div", { class: "signup-pop" },
    el("h3", {}, ev.title),
    el("div", { class: "muted small" }, "Class"), classes,
    el("div", { class: "muted small" }, "Spec"), specs,
    note, go), "above");
  // Line it up with the button rather than off to the side.
  const r = anchor.getBoundingClientRect();
  const above = r.top - pop.offsetHeight - 8;
  placeFixed(pop, r.left, above >= 8 ? above : r.bottom + 8);
}

function localInputValue(date) {
  const pad = (n) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function openRaidDialog(existing = null) {
  if (!existing && !state.channelId) return toast("Open a channel first.");
  const ev = existing;
  const defaultStart = new Date();
  defaultStart.setDate(defaultStart.getDate() + 1);
  defaultStart.setHours(20, 0, 0, 0);
  const title = el("input", { maxlength: 100, value: ev?.title ?? "", placeholder: "Raid night: Molten Core", required: true });
  const when = el("input", { type: "datetime-local", value: localInputValue(ev ? new Date(ev.starts_at) : defaultStart), required: true });
  const hours = el("select", { class: "select" }, [1, 1.5, 2, 2.5, 3, 3.5, 4, 5, 6].map((h) =>
    el("option", { value: h * 60, selected: (ev?.duration_minutes ?? 180) === h * 60 }, `${h} hour${h === 1 ? "" : "s"}`)));
  const tanks = el("input", { type: "number", min: 0, max: 40, value: ev?.max_tanks ?? RAID_SIZES[0].tanks });
  const healers = el("input", { type: "number", min: 0, max: 40, value: ev?.max_healers ?? RAID_SIZES[0].healers });
  const dps = el("input", { type: "number", min: 0, max: 40, value: ev?.max_dps ?? RAID_SIZES[0].dps });
  const size = el("select", { class: "select" },
    el("option", { value: "" }, "Custom"),
    RAID_SIZES.map((s, i) => el("option", { value: i, selected: !ev && i === 0 }, s.label)));
  size.addEventListener("change", () => {
    const s = RAID_SIZES[size.value];
    if (s) { tanks.value = s.tanks; healers.value = s.healers; dps.value = s.dps; }
  });
  [tanks, healers, dps].forEach((n) => n.addEventListener("input", () => (size.value = "")));
  const desc = el("textarea", { rows: 3, maxlength: 1000, placeholder: "Which raid, difficulty, flasks/food, where to meet…" }, ev?.description ?? "");

  const dialog = el("dialog", { class: "raid-dialog" },
    el("form", { method: "dialog" },
      el("h2", {}, ev ? "Edit raid" : "Plan a raid"),
      el("label", { class: "field" }, "Name", title),
      el("div", { class: "raid-row" },
        el("label", { class: "field" }, "Starts (your time)", when),
        el("label", { class: "field" }, "Length", hours)),
      el("label", { class: "field" }, "Size", size),
      el("div", { class: "raid-row three" },
        el("label", { class: "field" }, "🛡️ Tanks", tanks),
        el("label", { class: "field" }, "✚ Healers", healers),
        el("label", { class: "field" }, "⚔️ DPS", dps)),
      el("label", { class: "field" }, "Details", desc),
      el("p", { class: "muted small", style: "margin:0" }, "Everyone signed up gets an @mention 15 minutes before it starts."),
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
        el("button", { type: "submit", class: "primary", value: "ok" }, ev ? "Save" : "Post raid")),
    ),
  );
  dialog.addEventListener("close", async () => {
    const ok = dialog.returnValue === "ok";
    dialog.remove();
    if (!ok) return;
    const body = {
      title: title.value.trim(),
      description: desc.value.trim(),
      starts_at: new Date(when.value).toISOString(),
      duration_minutes: Number(hours.value),
      max_tanks: Number(tanks.value), max_healers: Number(healers.value), max_dps: Number(dps.value),
    };
    if (ev) {
      const msg = await tryApi("PATCH", `/api/events/${ev.id}`, body).catch(() => null);
      if (msg) applyMessageUpdate(msg);
    } else {
      const msg = await tryApi("POST", `/api/channels/${state.channelId}/events`, body).catch(() => null);
      if (msg && !findMessage(msg.id)) {
        state.messages.push(msg);
        renderMessages.forceBottom = true;
        renderMessages();
      }
    }
  });
  document.body.append(dialog);
  dialog.showModal();
  if (!ev) title.focus();
}

// The calendar button: upcoming raids in this outhouse, plus "Plan a raid".
async function openRaidList(anchor) {
  const box = el("div", { class: "raid-list" }, el("p", { class: "muted" }, "Loading…"));
  openPopover(anchor, box, "above");
  const events = await api("GET", `/api/guilds/${state.guildId}/events`).catch(() => []);
  const rows = events.map((ev) => {
    const when = eventWhen(ev);
    const mine = ev.signups.find((s) => s.user_id === state.me.id);
    const ch = state.channels.find((c) => c.id === ev.channel_id);
    return el("button", { class: "raid-row-btn", onclick: async () => {
      closePopover();
      if (state.channelId !== ev.channel_id) await selectChannel(ev.channel_id);
      setTimeout(() => jumpTo(ev.message_id), 300);
    } },
      el("div", { class: "rl-title" }, ev.title, mine && mine.status !== "no" ? el("span", { class: "rl-mine" }, `✓ ${STATUS_LABEL[mine.status]}`) : null),
      el("div", { class: "muted small" }, `${when.text} (${when.status})${ch ? ` · #${ch.name}` : ""}`));
  });
  box.replaceChildren(
    el("h3", {}, "Upcoming raids"),
    ...(rows.length ? rows : [el("p", { class: "muted small" }, "Nothing planned yet.")]),
    can("send_messages", state.channelId) ? el("button", { class: "primary", onclick: () => { closePopover(); openRaidDialog(); } }, "＋ Plan a raid") : null,
  );
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

// The Android app can't update itself, so offer the new APK when there is one.
(async function androidUpdateCheck() {
  const current = window.__OUTHOUSE_APP_VERSION__;
  if (window.__OUTHOUSE_PLATFORM__ !== "android" || !current) return;
  const latest = await fetch("/updates/android.json", { cache: "no-store" }).then((r) => (r.ok ? r.json() : null)).catch(() => null);
  if (!latest?.version || latest.version === current || prefs.get("apk_skipped", "") === latest.version) return;
  const a = latest.version.split(".").map(Number), b = current.split(".").map(Number);
  const i = a.findIndex((n, k) => n !== (b[k] ?? 0));
  if (i < 0 || !(a[i] > (b[i] ?? 0))) return; // not newer
  const bar = el("div", { class: "update-bar" },
    el("span", {}, `Outhouse ${latest.version} is out.`),
    el("button", { class: "primary", onclick: () => { desktopInvoke("open_external", { url: new URL(latest.url, location.href).href }); bar.remove(); } }, "Download"),
    el("button", { class: "ghost", onclick: () => { prefs.set("apk_skipped", latest.version); bar.remove(); } }, "Later"));
  document.body.append(bar);
})();

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
