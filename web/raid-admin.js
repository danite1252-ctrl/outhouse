// Outhouse: running raids. Raid managers chosen by role and the default
// sign-up channel (Outhouse settings → Raids), adding someone to a raid with
// one of their characters, and the invite list export (for Method Raid
// Tools, invite addons and /invite macros). Loaded after wow.js; uses the
// raid planner's helpers (canManageEvent, setSignup, raidWindow, classChip…).

state.raidSettings = null;

async function loadRaidSettings(guildId) {
  const s = await api("GET", `/api/guilds/${guildId}/raid-settings`).catch(() => null);
  if (!s || state.guildId !== guildId) return;
  state.raidSettings = { ...s, guild_id: guildId };
  if (state.messages.some((m) => m.event)) renderMessages();
}

const raBaseRefreshGuild = refreshGuild;
refreshGuild = async function () {
  const id = state.guildId;
  if (state.raidSettings?.guild_id !== id) state.raidSettings = null;
  const settings = id ? loadRaidSettings(id) : null;
  await raBaseRefreshGuild();
  await settings;
};

// Has one of the outhouse's raid manager roles (and isn't timed out).
function hasRaidManagerRole(guildId = state.guildId) {
  const s = state.raidSettings;
  if (!s || s.guild_id !== guildId || !s.manager_role_ids.length) return false;
  const me = memberById(state.me.id);
  if (!me || (me.timeout_until && new Date(me.timeout_until) > new Date())) return false;
  return me.role_ids.some((r) => s.manager_role_ids.includes(r));
}

const raBaseCanManageEvent = canManageEvent;
canManageEvent = function (ev) {
  return raBaseCanManageEvent(ev) || hasRaidManagerRole(ev.guild_id);
};

const raBaseHandle = handleEvent;
handleEvent = function (ev) {
  if (ev.t === "RAID_SETTINGS_UPDATE" && ev.d.guild_id === state.guildId) {
    state.raidSettings = { ...ev.d.settings, guild_id: ev.d.guild_id };
    renderMessages();
    if (typeof drawCalendar === "function" && cal?.dialog) drawCalendar();
  }
  raBaseHandle(ev);
};

// ---------- Outhouse settings → Raids ----------

async function drawRaidSettings(page) {
  page.replaceChildren(el("h2", {}, "Raids"), el("p", { class: "muted" }, "Loading…"));
  const [settings, roles] = await Promise.all([
    api("GET", `/api/guilds/${state.guildId}/raid-settings`).catch(() => null),
    api("GET", `/api/guilds/${state.guildId}/roles`).catch(() => state.roles),
  ]);
  if (!settings || !page.isConnected) return;
  const chosen = new Set(settings.manager_role_ids);
  const others = roles.filter((r) => r.id !== state.guildId).sort((a, b) => b.position - a.position);
  const count = (roleId) => state.members.filter((m) => m.role_ids.includes(roleId)).length;
  const roleRows = others.map((r) => {
    const box = el("input", { type: "checkbox", checked: chosen.has(r.id), "aria-label": r.name });
    box.addEventListener("change", () => { box.checked ? chosen.add(r.id) : chosen.delete(r.id); paintWho(); });
    const n = count(r.id);
    return el("label", { class: "rs-role" }, box,
      el("span", { class: "swatch", style: `background:${hexColour(r.color) || "#a1a1aa"}` }),
      el("span", { class: "rs-role-name" }, r.name),
      el("span", { class: "muted small" }, `${n} ${n === 1 ? "member" : "members"}`));
  });
  const who = el("p", { class: "muted small rs-who" });
  const paintWho = () => {
    const people = state.members.filter((m) => !m.bot && m.role_ids.some((r) => chosen.has(r)));
    who.textContent = people.length
      ? `Raid managers by role right now: ${people.map((m) => m.display_name).sort((a, b) => a.localeCompare(b)).join(", ")}.`
      : "Nobody has a raid manager role yet.";
  };
  paintWho();
  const textChannels = state.channels.filter((c) => c.kind === TEXT);
  const channel = el("select", { class: "select", "aria-label": "Sign-up channel" },
    el("option", { value: "" }, "The channel you're in"),
    textChannels.map((c) => el("option", { value: c.id, selected: c.id === settings.default_channel_id }, `# ${c.name}`)));
  const cleanupPick = (label, value, choices) => el("select", { class: "select", "aria-label": label },
    choices.map(([v, text]) => el("option", { value: v, selected: v === value }, text)));
  const cleanupSeries = cleanupPick("Old cards of repeating raids", settings.card_cleanup_series || "next", CARD_CLEANUP);
  const cleanupSingle = cleanupPick("Old cards of one-off raids", settings.card_cleanup_single || "off", CARD_CLEANUP.filter(([v]) => v !== "next"));
  const save = el("button", { class: "primary", onclick: async () => {
    save.disabled = true;
    const saved = await tryApi("PUT", `/api/guilds/${state.guildId}/raid-settings`, {
      manager_role_ids: [...chosen], default_channel_id: channel.value || null,
      card_cleanup_series: cleanupSeries.value, card_cleanup_single: cleanupSingle.value,
    }).catch(() => null);
    save.disabled = false;
    if (saved) {
      state.raidSettings = { ...saved, guild_id: state.guildId };
      toast("Raid settings saved.");
      renderMessages();
    }
  } }, "Save changes");

  page.replaceChildren(
    el("h2", {}, "Raids"),
    el("h3", { class: "rs-head" }, "Raid managers"),
    el("p", { class: "muted", style: "margin:0" }, "People with these roles run every raid in this outhouse: they can move people between the roster and the bench, add and remove sign-ups, take attendance, edit or cancel raids and export invite lists. It doesn't give them any other powers."),
    others.length
      ? el("div", { class: "rs-roles", role: "group", "aria-label": "Raid manager roles" }, roleRows)
      : el("p", { class: "muted" }, "There are no roles yet. Make one under Roles (say, Raid leader), then tick it here."),
    who,
    el("p", { class: "muted small", style: "margin:0" }, "Whoever planned a raid can always run it, and so can anyone with the Manage events permission."),
    el("h3", { class: "rs-head" }, "Sign-up channel"),
    el("label", { class: "field rs-channel" }, "Plan a raid posts in", channel),
    el("p", { class: "muted small", style: "margin:0" }, "Plan a raid starts with this channel picked. You can still choose another channel for each raid."),
    el("h3", { class: "rs-head" }, "Old sign-up cards"),
    el("p", { class: "muted", style: "margin:0" }, "Raid sign-up cards can tidy themselves out of chat once the raid is over. Only the chat card goes: the raid stays in the calendar with its sign-ups and attendance."),
    el("div", { class: "rs-cleanup" },
      el("label", { class: "field" }, "Repeating raids", cleanupSeries),
      el("label", { class: "field" }, "One-off raids", cleanupSingle)),
    el("p", { class: "muted small", style: "margin:0" }, "This is what Plan a raid starts with. Each raid can still choose for itself under \u201cRemove the chat card after the raid\u201d."),
    el("div", { class: "settings-actions" }, el("span"), save),
  );
}

// ---------- Adding someone (or changing their character) ----------

// A raid leader signs someone up, or changes someone's sign-up, with one of
// their characters.
async function openAddToRaid(ev, existing = null) {
  const dialog = raidWindow("add-raid-dialog");
  const head = el("div", { class: "att-head" },
    el("div", {}, el("h2", {}, existing ? `Change ${signupLabel(existing)}'s sign-up` : "Add someone to the raid"),
      el("div", { class: "muted small" }, `${ev.title} · ${eventWhen(ev).text}`)),
    closeX(dialog));
  const body = el("div", { class: "add-raid-body" });
  dialog.append(head, body);

  let userId = existing?.user_id ?? null;
  let chars = [];
  // "" none, "keep" the copy already on the sign-up, or a character id.
  let picked = existing?.character ? (existing.character.id ?? "keep") : "";
  let cls = existing?.class ?? "";
  let spec = existing?.spec ?? "";
  let role = existing?.role ?? "";
  const signedUp = new Set(ev.signups.map((s) => s.user_id));
  const people = state.members.filter((m) => !m.bot && (!signedUp.has(m.id) || m.id === userId))
    .sort((a, b) => a.display_name.localeCompare(b.display_name));
  const who = el("select", { class: "select", "aria-label": "Who" },
    el("option", { value: "" }, "Pick someone…"), people.map((m) => el("option", { value: m.id, selected: m.id === userId }, m.display_name)));
  const status = el("select", { class: "select", "aria-label": "Where" },
    [["yes", "On the roster"], ["late", "Running late"], ["tentative", "Tentative"], ["bench", "On the bench"]]
      .map(([v, l]) => el("option", { value: v, selected: v === (existing?.status === "no" ? "yes" : existing?.status ?? "yes") }, l)));
  const charRow = el("div", { class: "char-chips" });
  const classes = el("div", { class: "su-classes" });
  const specs = el("div", { class: "su-specs" });
  const save = el("button", { type: "button", class: "primary" }, existing ? "Save" : "Add to raid");

  const paint = () => {
    const chips = chars.map((c) => {
      const b = classChip(c, { small: true, on: picked === c.id, onclick: () => {
        picked = picked === c.id ? "" : c.id;
        if (picked) { cls = c.class; if (!WOW_CLASSES.find((k) => k.name === cls)?.specs.some(([n]) => n === spec)) { spec = ""; role = ""; } }
        paint();
      } });
      return b;
    });
    if (existing?.character && !existing.character.id) {
      chips.unshift(el("button", { type: "button", class: `char-chip small${picked === "keep" ? " on" : ""}`, title: "This character has since been removed from their profile",
        onclick: () => { picked = picked === "keep" ? "" : "keep"; paint(); } },
      el("span", { class: "char-name" }, existing.character.full_name), el("span", { class: "char-meta" }, "removed")));
    }
    charRow.replaceChildren(...(userId
      ? (chips.length ? chips : [el("span", { class: "muted small" }, ev.game ? `No ${GAME_LABEL[ev.game] ?? ev.game} characters on their profile: they'll show by name.` : "No characters on their profile: they'll show by name.")])
      : [el("span", { class: "muted small" }, "Pick someone first")]));
    classes.replaceChildren(...WOW_CLASSES.map((c) => el("button", {
      type: "button", class: `su-class${cls === c.name ? " on" : ""}`, style: `--cc:${c.colour}`, title: c.name,
      onclick: () => {
        cls = c.name;
        if (!c.specs.some(([n]) => n === spec)) { spec = ""; role = ""; }
        const char = chars.find((x) => x.id === picked);
        if (char && char.class !== cls) picked = "";
        paint();
      },
    }, c.name)));
    const k = WOW_CLASSES.find((x) => x.name === cls);
    specs.replaceChildren(...(k ? k.specs.map(([n, r]) => el("button", {
      type: "button", class: `su-spec${spec === n ? " on" : ""}`, onclick: () => { spec = n; role = r; paint(); },
    }, `${ROLE_INFO[r].icon} ${n}`)) : [el("span", { class: "muted small" }, "Pick a class first")]));
    save.disabled = !userId || !role;
  };
  const loadChars = async () => {
    chars = [];
    paint();
    if (!userId) return;
    const forUser = userId;
    const profile = await api("GET", `/api/profiles/${userId}`).catch(() => null);
    if (forUser !== userId) return;
    chars = (profile?.characters ?? []).filter((c) => !ev.game || (c.game || "forever") === ev.game);
    if (!existing && !picked) {
      const main = chars.find((c) => c.is_main) ?? chars[0];
      if (main) { picked = main.id; cls = main.class; spec = ""; role = ""; }
    }
    paint();
  };
  who.addEventListener("change", () => { userId = who.value || null; picked = ""; loadChars(); });
  save.addEventListener("click", async () => {
    save.disabled = true;
    const body = { user_id: userId, status: status.value, role, class: cls, spec, note: existing?.note ?? "" };
    if (picked === "keep") body.keep_character = true;
    else body.character_id = picked || null;
    const reply = await tryApi("PUT", `/api/events/${ev.id}/signup`, body).catch(() => null);
    save.disabled = false;
    if (!reply) return;
    applyEventReply(reply);
    dialog.close();
    toast(existing ? "Sign-up changed." : "Added to the raid.");
  });

  body.append(
    existing ? null : el("label", { class: "field" }, "Who", who),
    el("div", { class: "muted small" }, "Their characters"), charRow,
    el("div", { class: "muted small" }, "Class"), classes,
    el("div", { class: "muted small" }, "Spec"), specs,
    el("label", { class: "field" }, "Where", status),
    el("div", { class: "row" }, el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"), save));
  loadChars();
}

// ---------- Export / invite list ----------

// In-game names for an export entry. WoW: Forever characters go as
// "First Last" (what /invite wants there, e.g. /invite Golgoroth Ironmaw).
function exportName(e) {
  const c = e.character;
  if (!c) return { name: e.display_name, flagged: true };
  return { name: c.last_name ? `${c.name} ${c.last_name}` : c.name, flagged: false };
}

// "/invite A\n/invite B" split into macros of under 255 characters.
function inviteMacros(names) {
  const macros = [];
  let cur = "";
  for (const n of names) {
    const line = `/invite ${n}`;
    const next = cur ? `${cur}\n${line}` : line;
    if (next.length > 254 && cur) { macros.push(cur); cur = line; } else cur = next;
  }
  if (cur) macros.push(cur);
  return macros;
}

// Method Raid Tools raid groups: groups of five, tanks then healers then DPS.
function mrtGroups(entries, names) {
  const lines = [];
  const order = { tank: 0, healer: 1, dps: 2 };
  const sorted = entries.map((e, i) => ({ e, n: names[i] }))
    .sort((a, b) => (order[a.e.role] ?? 3) - (order[b.e.role] ?? 3));
  sorted.forEach(({ n }, i) => {
    if (i % 5 === 0) lines.push(i >= 40 ? (i === 40 ? "Extra (no group left)" : null) : `Group ${i / 5 + 1}`);
    lines.push(n);
  });
  return lines.filter((l) => l !== null).join("\n");
}

async function openRaidExport(ev) {
  const data = await tryApi("GET", `/api/events/${ev.id}/export`).catch(() => null);
  if (!data) return;
  const dialog = raidWindow("export-dialog");
  const forever = data.game === "forever" || data.entries.some((e) => e.character?.last_name);
  let format = "lines";
  const bench = el("input", { type: "checkbox", checked: prefs.get("export_bench", false) });
  const maybe = el("input", { type: "checkbox", checked: prefs.get("export_maybe", true) });
  const tabs = el("div", { class: "exp-tabs", role: "tablist" });
  const out = el("div", { class: "exp-out" });
  const summary = el("div", { class: "muted small exp-summary" });
  const flags = el("div", { class: "exp-flags" });
  const FORMATS = [["lines", "Method Raid Tools (Invite by list)"], ["comma", "Comma-separated"], ["mrt", "MRT raid groups"], ["macro", "/invite macros"]];
  const HELP = {
    lines: "One name per line. In game: Method Raid Tools → Invite tools → Invite by list (or /rt invlist 1), paste into a list and invite. Works in VortexInviter, RInvite and most invite addons too.",
    comma: "For addons and sites that want one line of names.",
    mrt: "Groups of five (Group 1 to 8): tanks, then healers, then DPS. Paste into MRT's raid groups.",
    macro: "Each block is one macro (under 255 characters). Make a macro in game (/macro), paste a block in and click it.",
  };
  const block = (text, label) => {
    const area = el("textarea", { class: "exp-text", readonly: true, rows: Math.min(14, Math.max(3, text.split("\n").length)), "aria-label": label }, text);
    area.value = text;
    return el("div", { class: "exp-block" },
      el("div", { class: "exp-block-head" }, el("strong", {}, label),
        el("button", { type: "button", class: "primary exp-copy", onclick: () => copy(text, `${label} copied`) }, "Copy")),
      area);
  };
  const draw = () => {
    const wanted = data.entries.filter((e) => e.status === "yes" || (maybe.checked && (e.status === "late" || e.status === "tentative")) || (bench.checked && e.status === "bench"));
    const picked = wanted.map((e) => exportName(e));
    const names = picked.map((p) => p.name);
    tabs.replaceChildren(...FORMATS.map(([v, l]) => el("button", {
      type: "button", role: "tab", class: `exp-tab${format === v ? " on" : ""}`, "aria-selected": format === v ? "true" : "false",
      onclick: () => { format = v; draw(); },
    }, l)));
    const blocks = !names.length ? [el("p", { class: "muted" }, "Nobody to invite with these options.")]
      : format === "comma" ? [block(names.join(","), "Names")]
        : format === "mrt" ? [block(mrtGroups(wanted, names), "Raid groups")]
          : format === "macro" ? inviteMacros(names).map((m, i, all) => block(m, all.length > 1 ? `Macro ${i + 1}` : "Macro"))
            : [block(names.join("\n"), "Names")];
    out.replaceChildren(el("p", { class: "muted small exp-help" }, HELP[format]), ...blocks);
    const missing = wanted.filter((e, i) => picked[i].flagged);
    summary.textContent = `${names.length} ${names.length === 1 ? "name" : "names"}${missing.length ? ` · ${missing.length} without a character` : ""}`;
    flags.replaceChildren(...(missing.length ? [el("div", { class: "exp-flag" },
      el("strong", {}, "⚠ No character picked: "),
      missing.map((e) => e.display_name).join(", "),
      el("div", { class: "muted small" }, "They're listed by their outhouse name, which may not be their name in game. Use “Change character or role…” on their name in the raid card, or ask them to sign up again with a character."))] : []));
  };
  [bench, maybe].forEach((b) => b.addEventListener("change", () => { prefs.set("export_bench", bench.checked); prefs.set("export_maybe", maybe.checked); draw(); }));

  dialog.append(
    el("div", { class: "att-head" },
      el("div", {}, el("h2", {}, "Invite list"), el("div", { class: "muted small" }, `${ev.title} · ${eventWhen(ev).text}`)),
      closeX(dialog)),
    el("div", { class: "exp-options" },
      el("label", { class: "check-row" }, maybe, "Tentative & running late"),
      el("label", { class: "check-row" }, bench, "Bench")),
    forever ? el("p", { class: "muted small", style: "margin:0" }, "WoW: Forever names go as First Last, the way /invite wants them (e.g. /invite Golgoroth Ironmaw).") : null,
    summary,
    flags,
    tabs,
    out);
  draw();
}
