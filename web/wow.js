// Outhouse: WoW tools. Group finder (LFG) cards, Wowhead item cards (/item),
// WoW link cards (Wowhead, Warcraft Logs, Raider.IO), characters on profiles
// (/char, Settings → Profile & picture) and Battle.net linking.
// Loaded after raids.js; uses the raid planner's WOW_CLASSES and ROLE_INFO.

state.wowInfo = null;
state.myCharacters = [];

async function loadWow() {
  const [info, chars] = await Promise.all([
    api("GET", "/api/wow/info").catch(() => null),
    api("GET", "/api/users/@me/characters").catch(() => null),
  ]);
  if (info) state.wowInfo = info;
  if (chars) state.myCharacters = chars;
}

const wowBaseOnReady = onReady;
onReady = function () {
  wowBaseOnReady();
  loadWow();
};

const ROLES = ["tank", "healer", "dps"];
const ROLE_ONE = { tank: "Tank", healer: "Healer", dps: "DPS" };
const canPlay = (cls, role) => !!WOW_CLASSES.find((c) => c.name === cls)?.specs.some(([, r]) => r === role);
const mainCharacter = () => state.myCharacters.find((c) => c.is_main) ?? state.myCharacters[0] ?? null;
const charName = (c) => c.full_name || c.name;
// Characters you could bring to something for this game ("" = any).
const charsFor = (game) => state.myCharacters.filter((c) => !game || (c.game || "forever") === game);
const gameInfo = (id) => state.wowInfo?.games?.find((g) => g.id === id) ?? null;
const nameOfUser = (id, fallback) => memberById(id)?.display_name ?? (id === state.me.id ? state.me.display_name : fallback);

function classChip(c, { onclick, on = false, small = false } = {}) {
  const colour = classColour(c.class) ?? "var(--text)";
  return el(onclick ? "button" : "span", {
    type: onclick ? "button" : null,
    class: `char-chip${on ? " on" : ""}${small ? " small" : ""}`,
    style: `--cc:${colour}`,
    title: [GAME_LABEL[c.game], c.game === "forever" ? c.ruleset : c.realm, c.race, c.professions?.length ? c.professions.join(", ") : null].filter(Boolean).join(" · ") || null,
    onclick,
  },
  c.is_main ? el("span", { class: "char-star", title: "Main" }, "★") : null,
  el("span", { class: "char-name" }, charName(c)),
  el("span", { class: "char-meta" }, `${c.level} ${c.class}`),
  c.game ? el("span", { class: `char-game g-${c.game}` }, GAME_SHORT[c.game] ?? c.game) : null);
}

// ======================================================================
// Group finder cards
// ======================================================================

function lfgStatus(g) {
  if (g.closed) return "closed";
  if (g.expired || new Date(g.expires_at) <= new Date()) return "expired";
  return g.full ? "full" : "open";
}

function lfgLevels(g) {
  if (g.min_level && g.max_level) return g.min_level === g.max_level ? `Level ${g.min_level}` : `Levels ${g.min_level}–${g.max_level}`;
  if (g.min_level) return `Level ${g.min_level}+`;
  if (g.max_level) return `Up to level ${g.max_level}`;
  return null;
}

function canManageLfg(m) {
  return m.lfg.leader_id === state.me.id || can("manage_messages", m.channel_id);
}

function renderLfgCard(m) {
  const g = m.lfg;
  const status = lfgStatus(g);
  const live = status === "open" || status === "full";
  const manage = canManageLfg(m);
  const mine = g.members.find((x) => x.user_id === state.me.id);
  const slots = g.max_tanks + g.max_healers + g.max_dps;
  const maxOf = (role) => ({ tank: g.max_tanks, healer: g.max_healers, dps: g.max_dps })[role];
  const start = g.starts_at ? new Date(g.starts_at) : null;
  const kind = { dungeon: "DUNGEON", raid: "RAID" }[g.activity_kind] ?? "GROUP";
  const pill = { full: "FULL", closed: "CLOSED", expired: "EXPIRED" }[status];

  const person = (x) => {
    const colour = classColour(x.class);
    const node = el("div", { class: `lfg-person${x.user_id === state.me.id ? " me" : ""}` },
      el("span", { class: "lfg-pname", style: colour ? `color:${colour}` : "" },
        x.user_id === g.leader_id ? el("span", { class: "lfg-crown", title: "Group leader" }, "👑") : null,
        x.character || nameOfUser(x.user_id, x.display_name)),
      el("span", { class: "lfg-pmeta" }, [x.character ? nameOfUser(x.user_id, x.display_name) : null, x.level ? `${x.level}` : null, x.class && !x.character ? x.class : null].filter(Boolean).join(" · ")));
    node.title = [x.character && `${x.character}${x.level ? ` (level ${x.level})` : ""}`, x.class, x.character ? `${nameOfUser(x.user_id, x.display_name)} in the outhouse` : null].filter(Boolean).join(" — ");
    if (manage && x.user_id !== state.me.id) {
      node.classList.add("manageable");
      node.addEventListener("click", (e) => {
        const r = e.currentTarget.getBoundingClientRect();
        openMenu(r.left, r.bottom + 4, [
          { label: "Remove from group", danger: true, onClick: () => lfgRemove(m, x.user_id) },
        ]);
      });
    }
    return node;
  };
  const column = (role) => {
    const max = maxOf(role);
    if (!max) return null;
    const people = g.members.filter((x) => x.role === role);
    const open = Math.max(0, max - people.length);
    return el("div", { class: `lfg-col ${role}` },
      el("div", { class: "lfg-col-head" }, `${ROLE_INFO[role].icon} ${ROLE_INFO[role].label}`,
        el("span", { class: `lfg-count${people.length >= max ? " full" : ""}` }, `${people.length}/${max}`)),
      people.map(person),
      open && live ? el("div", { class: "lfg-open" }, open === 1 ? "1 spot open" : `${open} spots open`) : null);
  };
  const joinBtn = (role) => {
    const max = maxOf(role);
    if (!max) return null;
    const taken = g.members.filter((x) => x.role === role && x.user_id !== state.me.id).length;
    const here = mine?.role === role;
    return el("button", {
      class: `ev-btn lfg-join${here ? " on" : ""}`,
      disabled: !here && taken >= max,
      title: !here && taken >= max ? `No more ${ROLE_INFO[role].label.toLowerCase()} needed` : null,
      onclick: (e) => openLfgJoin(m, role, e.currentTarget),
    }, `${ROLE_INFO[role].icon} ${here ? `Joined as ${ROLE_ONE[role]}` : `Join as ${ROLE_ONE[role]}`}`);
  };
  const canJoin = live && can("send_messages", m.channel_id);
  const expires = new Date(g.expires_at);

  return el("div", { class: `lfg-card ${status} kind-${g.activity_kind}`, "data-lfg": m.id },
    el("div", { class: "lfg-head" },
      el("div", { class: "lfg-head-main" },
        el("div", { class: "lfg-kicker" }, `🔎 LOOKING FOR GROUP · ${kind}`,
          g.game ? el("span", { class: `ev-game g-${g.game}`, title: GAME_LABEL[g.game] ?? g.game }, GAME_SHORT[g.game] ?? g.game) : null,
          pill ? el("span", { class: `lfg-pill ${status}` }, pill) : null),
        el("div", { class: "lfg-title" }, g.activity),
        el("div", { class: "lfg-when" },
          [lfgLevels(g),
            start ? `🕗 ${start.toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" })} (${relativeTime(start)})` : "🕗 Now",
          ].filter(Boolean).join(" · "))),
      el("div", { class: "ev-total", title: "In the group / spots" }, el("strong", {}, g.members.length), `/${slots}`),
      manage && live ? el("button", { class: "icon-btn small", title: "Group options", "aria-label": "Group options", onclick: (e) => {
        const r = e.currentTarget.getBoundingClientRect();
        openMenu(r.left, r.bottom + 4, [
          { label: "Copy group list", onClick: () => copy(lfgText(g), "Group list copied") },
          "sep",
          { label: "Close group", danger: true, onClick: () => lfgClose(m) },
        ]);
      } }, "⋯") : null),
    g.note ? el("p", { class: "lfg-note" }, renderText(g.note)) : null,
    el("div", { class: "lfg-cols" }, ROLES.map(column)),
    canJoin ? el("div", { class: "ev-actions lfg-actions" },
      ROLES.map(joinBtn),
      mine ? el("button", { class: "ev-btn ghost", onclick: () => lfgRemove(m, state.me.id) }, "Leave") : null) : null,
    el("div", { class: "lfg-foot" },
      status === "closed" ? "The leader closed this group."
        : status === "expired" ? "This listing has ended."
          : `${status === "full" ? "Group's full · " : ""}Listing ends ${relativeTime(expires)}`),
  );
}

function lfgText(g) {
  const lines = [`${g.activity}${lfgLevels(g) ? ` (${lfgLevels(g)})` : ""}`];
  for (const role of ROLES) {
    const people = g.members.filter((x) => x.role === role);
    if (!people.length) continue;
    lines.push(`${ROLE_INFO[role].label}: ${people.map((x) => `${x.character || nameOfUser(x.user_id, x.display_name)}${x.class ? ` (${x.class})` : ""}`).join(", ")}`);
  }
  return lines.join("\n");
}

async function lfgRemove(m, userId) {
  if (userId !== state.me.id && !(await confirmBox("Remove from group?", `${nameOfUser(userId, "They")} will be taken out of the group. They can join again while it's open.`, "Remove"))) return;
  const msg = await tryApi("DELETE", `/api/channels/${m.channel_id}/lfg/${m.id}/members/${userId}`).catch(() => null);
  if (msg) applyMessageUpdate(msg);
}

async function lfgClose(m) {
  if (!(await confirmBox("Close the group?", "Nobody else can join. The card stays up with who's in.", "Close group"))) return;
  const msg = await tryApi("POST", `/api/channels/${m.channel_id}/lfg/${m.id}/close`).catch(() => null);
  if (msg) applyMessageUpdate(msg);
}

// Pick a character (or class) and join in a role.
function openLfgJoin(m, role, anchor) {
  const g = m.lfg;
  const mine = g.members.find((x) => x.user_id === state.me.id);
  const chars = charsFor(g.game).filter((c) => canPlay(c.class, role));
  const main = mainCharacter();
  const fits = main && canPlay(main.class, role) && (!g.game || main.game === g.game);
  const start = mine?.role === role ? mine
    : (fits ? { class: main.class, character: charName(main), level: main.level } : chars[0] ? { class: chars[0].class, character: charName(chars[0]), level: chars[0].level } : null);
  let cls = start?.class ?? (canPlay(prefs.get("raid_class", ""), role) ? prefs.get("raid_class", "") : "");
  const charInput = el("input", { placeholder: "Character name (optional)", maxlength: 25, value: start?.character ?? "", "aria-label": "Character name" });
  const level = el("input", { type: "number", min: 1, max: gameInfo(g.game)?.max_level ?? 70, placeholder: "Level", value: start?.level ?? "", "aria-label": "Level" });
  const charRow = el("div", { class: "char-chips" });
  const classes = el("div", { class: "su-classes" });
  const go = el("button", { class: "primary" }, `${ROLE_INFO[role].icon} Join as ${ROLE_ONE[role]}`);
  const lv = lfgLevels(g);
  const levelHint = el("div", { class: "muted small" });
  const paint = () => {
    charRow.replaceChildren(...chars.map((c) => classChip(c, {
      on: charInput.value === charName(c) && cls === c.class,
      onclick: () => { cls = c.class; charInput.value = charName(c); level.value = c.level; paint(); },
    })));
    charRow.hidden = !chars.length;
    classes.replaceChildren(...WOW_CLASSES.filter((c) => canPlay(c.name, role)).map((c) => el("button", {
      type: "button", class: `su-class${cls === c.name ? " on" : ""}`, style: `--cc:${c.colour}`, title: c.name,
      onclick: () => { cls = c.name; paint(); },
    }, c.name)));
    go.disabled = !cls;
    const lvl = Number(level.value);
    levelHint.textContent = lvl && ((g.min_level && lvl < g.min_level) || (g.max_level && lvl > g.max_level)) ? `Heads up: they're after ${lv?.toLowerCase()}.` : "";
  };
  level.addEventListener("input", paint);
  charInput.addEventListener("input", paint);
  go.addEventListener("click", async () => {
    closePopover();
    prefs.set("raid_class", cls);
    const msg = await tryApi("PUT", `/api/channels/${m.channel_id}/lfg/${m.id}/join`, {
      role, class: cls, character: charInput.value.trim(), level: level.value ? Number(level.value) : null,
    }).catch(() => null);
    if (msg) applyMessageUpdate(msg);
  });
  paint();
  const pop = openPopover(anchor, el("div", { class: "signup-pop lfg-join-pop" },
    el("h3", {}, `${g.activity}${lv ? ` · ${lv}` : ""}`),
    chars.length ? el("div", { class: "muted small" }, "Your characters") : null, charRow,
    el("div", { class: "muted small" }, "Class"), classes,
    el("div", { class: "lfg-join-row" }, charInput, level),
    levelHint,
    state.myCharacters.length ? null : el("div", { class: "muted small" }, "Tip: add your characters in Settings → Profile & picture to fill this in with one tap."),
    go), "above");
  const r = anchor.getBoundingClientRect();
  const above = r.top - pop.offsetHeight - 8;
  placeFixed(pop, r.left, above >= 8 ? above : r.bottom + 8);
}

// ---------- Posting a group ----------

const LFG_EXPIRY = [[30, "30 minutes"], [60, "1 hour"], [120, "2 hours"], [240, "4 hours"], [480, "8 hours"], [1440, "1 day"]];

function openLfgDialog(activity = "") {
  if (!state.channelId) return toast("Open a channel first.");
  const channelId = state.channelId;
  const list = state.wowInfo?.activities ?? [];
  const listId = `lfg-activities-${Date.now()}`;
  const datalist = el("datalist", { id: listId }, list.map((a) => el("option", { value: a.name }, `${a.kind === "raid" ? "Raid" : "Dungeon"} · ${a.min_level === a.max_level ? a.min_level : `${a.min_level}–${a.max_level}`}`)));
  const act = el("input", { maxlength: 80, value: activity, placeholder: "Scholomance, Molten Core, gold farming…", list: listId, required: true, "aria-label": "Activity" });
  const quick = el("div", { class: "lfg-quick" });
  const minL = el("input", { type: "number", min: 1, max: 70, placeholder: "Any", "aria-label": "Lowest level" });
  const maxL = el("input", { type: "number", min: 1, max: 70, placeholder: "Any", "aria-label": "Highest level" });
  const game = el("select", { class: "select", "aria-label": "Game" },
    el("option", { value: "" }, "Any game"),
    Object.entries(GAME_LABEL).map(([id, label]) => el("option", { value: id }, label)));
  game.value = prefs.get("lfg_game", mainCharacter()?.game ?? "");
  const tanks = el("input", { type: "number", min: 0, max: 40, value: 1, "aria-label": "Tanks" });
  const healers = el("input", { type: "number", min: 0, max: 40, value: 1, "aria-label": "Healers" });
  const dps = el("input", { type: "number", min: 0, max: 40, value: 3, "aria-label": "DPS" });
  const timed = el("input", { type: "checkbox" });
  const later = new Date(Date.now() + 60 * 60000); later.setMinutes(Math.ceil(later.getMinutes() / 15) * 15, 0, 0);
  const when = el("input", { type: "datetime-local", value: localInputValue(later), disabled: true, "aria-label": "Start time" });
  const expiry = el("select", { class: "select", "aria-label": "Listing ends after" }, LFG_EXPIRY.map(([mins, label]) => el("option", { value: mins, selected: mins === 120 }, label)));
  timed.addEventListener("change", () => { when.disabled = !timed.checked; expiry.disabled = timed.checked; });
  const note = el("textarea", { rows: 2, maxlength: 300, placeholder: "Need someone with the key, summons at the stone, quest share…" });
  const myRole = el("select", { class: "select", "aria-label": "Your role" },
    el("option", { value: "" }, "Not joining myself"),
    ROLES.map((r) => el("option", { value: r }, `${ROLE_INFO[r].icon} ${ROLE_ONE[r]}`)));
  const myChar = el("select", { class: "select", "aria-label": "Your character" });
  const paintChars = () => {
    const list = charsFor(game.value);
    const keep = myChar.value;
    myChar.replaceChildren(...(list.length ? list.map((c) => el("option", { value: c.id }, `${c.is_main ? "★ " : ""}${charName(c)} · ${c.level} ${c.class}`)) : [el("option", { value: "" }, "No characters for this game")]));
    if (list.some((c) => c.id === keep)) myChar.value = keep;
    const top = gameInfo(game.value)?.max_level ?? 70;
    minL.max = maxL.max = top;
  };
  game.addEventListener("change", paintChars);
  paintChars();
  const error = el("p", { class: "error" });

  const fill = () => {
    const a = list.find((x) => x.name.toLowerCase() === act.value.trim().toLowerCase());
    if (!a) return;
    minL.value = a.min_level; maxL.value = a.max_level;
    const [t, h, d] = a.size <= 5 ? [1, 1, 3] : a.size <= 10 ? [2, 3, 5] : a.size <= 20 ? [3, 5, 12] : [5, 10, 25];
    tanks.value = t; healers.value = h; dps.value = d;
  };
  act.addEventListener("input", fill);
  // A few handy picks for people's level.
  const main = mainCharacter();
  const suggestions = (main ? list.filter((a) => a.kind === "dungeon" && main.level >= a.min_level - 2 && main.level <= a.max_level + 1) : list.filter((a) => ["Scholomance", "Stratholme: Undead", "Upper Blackrock Spire", "Molten Core"].includes(a.name))).slice(0, 5);
  quick.replaceChildren(...suggestions.map((a) => el("button", { type: "button", class: "lfg-quick-btn", onclick: () => { act.value = a.name; fill(); } }, a.name)));
  myRole.value = prefs.get("lfg_role", "");
  if (activity) fill();

  const form = el("form", { method: "dialog" },
    el("h2", {}, "🔎 Looking for group"),
    el("div", { class: "raid-row lfg-what" },
      el("label", { class: "field" }, "What for", act, datalist),
      el("label", { class: "field" }, "Game", game)),
    suggestions.length ? quick : null,
    el("div", { class: "raid-row three" },
      el("label", { class: "field" }, "🛡️ Tanks", tanks),
      el("label", { class: "field" }, "✚ Healers", healers),
      el("label", { class: "field" }, "⚔️ DPS", dps)),
    el("div", { class: "raid-row even" },
      el("label", { class: "field" }, "Lowest level", minL),
      el("label", { class: "field" }, "Highest level", maxL)),
    el("label", { class: "check-row" }, timed, "Set a start time (otherwise: now)"),
    el("div", { class: "raid-row even stack" },
      el("label", { class: "field" }, "Starts (your time)", when),
      el("label", { class: "field" }, "Listing ends after", expiry)),
    el("label", { class: "field" }, "Note", note),
    el("div", { class: "raid-row even" },
      el("label", { class: "field" }, "You're going as", myRole),
      el("label", { class: "field" }, "On", myChar)),
    error,
    el("div", { class: "row" },
      el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"),
      el("button", { type: "submit", class: "primary" }, "Post group")));
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!act.value.trim()) { error.textContent = "Say what the group's for."; return act.focus(); }
    const num = (x) => (x.value === "" ? null : Number(x.value));
    const body = {
      activity: act.value.trim(),
      min_level: num(minL), max_level: num(maxL),
      max_tanks: num(tanks) ?? 0, max_healers: num(healers) ?? 0, max_dps: num(dps) ?? 0,
      note: note.value.trim(),
      game: game.value,
    };
    prefs.set("lfg_game", game.value);
    if (timed.checked && when.value) body.starts_at = new Date(when.value).toISOString();
    else body.expires_minutes = Number(expiry.value);
    prefs.set("lfg_role", myRole.value);
    if (myRole.value) {
      const c = state.myCharacters.find((x) => x.id === myChar.value);
      body.leader = { role: myRole.value, class: c?.class ?? "", character: c ? charName(c) : "", level: c?.level ?? null };
      if (c && !canPlay(c.class, myRole.value)) { error.textContent = `${charName(c)} can't play ${ROLE_ONE[myRole.value]}.`; return; }
    }
    error.textContent = "";
    try {
      const msg = await api("POST", `/api/channels/${channelId}/lfg`, body);
      dialog.close();
      if (state.channelId === channelId && !findMessage(msg.id)) {
        state.messages.push(msg);
        renderMessages.forceBottom = true;
        renderMessages();
      }
    } catch (err) {
      error.textContent = err.message;
    }
  });
  const dialog = el("dialog", { class: "raid-dialog lfg-dialog" }, form);
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  act.focus();
}

// The composer button.
(function lfgButton() {
  const after = $("poll-btn") ?? $("raid-btn");
  const btn = el("button", { type: "button", class: "tool", id: "lfg-btn", title: "Looking for group", "aria-label": "Looking for group", onclick: () => openLfgDialog() });
  btn.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6"/><path d="M15 15l5 5M8 10.5h5M10.5 8v5"/></svg>';
  after?.after(btn);
})();

// ======================================================================
// Item, link and character cards
// ======================================================================

const WOW_SITE = (e) => {
  const s = (e.site_name ?? "").toLowerCase();
  if (s.startsWith("wowhead")) return "wowhead";
  if (s.startsWith("warcraft logs")) return "wcl";
  if (s.startsWith("raider")) return "rio";
  return "other";
};

function wowLink(text, url, cls, style = "") {
  // Only web links (a bot could send any embed).
  if (url && !/^https?:\/\//i.test(url)) url = null;
  return url ? el("a", { class: cls, href: url, target: "_blank", rel: "noopener noreferrer", style }, text) : el("span", { class: cls, style }, text);
}

function hideBroken(img) {
  img.addEventListener("error", () => img.remove());
  return img;
}

function renderWowEmbed(e) {
  const colour = e.color != null ? hexColour(e.color) || "#ffffff" : null;
  if (e.kind === "wow-item") {
    const found = e.color != null;
    return el("div", { class: `wow-embed wow-item${found ? "" : " missing"}`, style: colour ? `--q:${colour}` : "" },
      e.thumbnail_url ? hideBroken(el("img", { class: "wow-icon", src: e.thumbnail_url, alt: "", loading: "lazy" })) : el("div", { class: "wow-icon blank" }, found ? "?" : "🔎"),
      el("div", { class: "wow-main" },
        wowLink(e.title ?? "Item", e.url, "wow-title"),
        e.description ? el("div", { class: "wow-tooltip" }, e.description) : null,
        el("div", { class: "wow-foot" }, [e.site_name, ...(e.fields ?? []).map((f) => `${f.name}: ${f.value}`), e.footer].filter(Boolean).join(" · "))));
  }
  if (e.kind === "wow-char") {
    return el("div", { class: "wow-embed wow-char", style: colour ? `--q:${colour}` : "" },
      el("div", { class: "wow-main" },
        el("div", { class: "wow-site" }, "⚔️ CHARACTER"),
        wowLink(e.title, e.url, "wow-title"),
        e.description ? el("div", { class: "wow-desc" }, e.description) : null,
        e.fields?.length ? el("div", { class: "rich-fields" }, e.fields.map((f) =>
          el("div", { class: `rich-field${f.inline ? " inline" : ""}` }, el("div", { class: "fname" }, f.name), el("div", { class: "fvalue" }, f.value)))) : null,
        e.footer ? el("div", { class: "wow-foot" }, e.footer) : null));
  }
  // wow-link: Wowhead, Warcraft Logs, Raider.IO pages.
  const site = WOW_SITE(e);
  const badge = { wowhead: "WH", wcl: "WCL", rio: "RIO" }[site] ?? "🔗";
  return el("div", { class: `wow-embed wow-link site-${site}`, style: colour ? `--q:${colour}` : "" },
    el("div", { class: "wow-badge", "aria-hidden": "true" }, badge),
    el("div", { class: "wow-main" },
      el("div", { class: "wow-site" }, e.site_name ?? "Link"),
      wowLink(e.title ?? e.url, e.url, "wow-title"),
      e.description ? el("div", { class: "wow-desc" }, e.description) : null),
    e.thumbnail_url ? hideBroken(el("img", { class: "wow-thumb", src: e.thumbnail_url, alt: "", loading: "lazy" })) : null);
}

const wowBaseRenderEmbeds = renderEmbeds;
renderEmbeds = function (m) {
  const lfg = m.lfg ? renderLfgCard(m) : null;
  const wow = m.embeds.filter((e) => e.kind?.startsWith("wow-"));
  if (!wow.length) {
    const rest = wowBaseRenderEmbeds(m);
    return lfg ? [lfg, ...(rest ?? [])] : rest;
  }
  const out = lfg ? [lfg] : [];
  const others = m.embeds.filter((e) => !e.kind?.startsWith("wow-"));
  const rendered = others.length ? wowBaseRenderEmbeds({ ...m, embeds: others }) ?? [] : [];
  let k = 0;
  for (const e of m.embeds) out.push(e.kind?.startsWith("wow-") ? renderWowEmbed(e) : rendered[k++]);
  return out;
};

// ======================================================================
// Slash commands: /lfg, /item, /char
// ======================================================================

const WOW_COMMANDS = [
  { name: "lfg", description: "Post a looking-for-group card", options: [{ name: "activity", description: "Dungeon, raid or anything else", required: false }] },
  { name: "item", description: "Show a WoW item from Wowhead", options: [{ name: "item", description: "Item name or ID", required: true }] },
  { name: "char", description: "Show a WoW character from someone here", options: [{ name: "name", description: "First, last or full name (Name-Realm for TBC and Era)", required: true }] },
].map((c) => ({ ...c, id: c.name, bot_id: "0", bot_name: "Outhouse", builtin: true }));

const wowBaseLoadCommands = loadCommands;
loadCommands = async function () {
  await wowBaseLoadCommands();
  const extra = WOW_COMMANDS.filter((c) => !state.commands.some((x) => x.name === c.name));
  if (extra.length) state.commands = [...state.commands, ...extra];
};

async function wowLookup(kind, query) {
  const channelId = state.channelId;
  if (!query) throw new Error(kind === "item" ? "Type an item name or ID after /item." : "Type a character name after /char.");
  if (kind === "item") toast("Looking that up on Wowhead…");
  const msg = await api("POST", `/api/channels/${channelId}/wow/${kind}`, { query });
  if (state.channelId === channelId && !findMessage(msg.id)) {
    state.messages.push(msg);
    renderMessages.forceBottom = true;
    renderMessages();
  }
}

const wowBaseRunCommand = runCommand;
runCommand = async function (command, text) {
  if (command.builtin && WOW_COMMANDS.some((c) => c.name === command.name)) {
    const rest = text.replace(/^\/[a-z0-9_-]+\s*/, "").replace(/^(activity|item|name):\s*/, "").trim();
    if (command.name === "lfg") return openLfgDialog(rest);
    return wowLookup(command.name, rest);
  }
  return wowBaseRunCommand(command, text);
};

// "Listing ends in 20 minutes" stays fresh.
setInterval(() => {
  if (state.messages.some((m) => m.lfg && !m.lfg.closed && !m.lfg.expired) && !document.hidden && !state.editingId) renderMessages();
}, 60_000);

// ======================================================================
// Characters: profile cards, raid sign-ups, settings
// ======================================================================

const wowBaseProfileCard = profileCard;
profileCard = function (p, opts) {
  const card = wowBaseProfileCard(p, opts);
  const chars = p.characters ?? [];
  if (!chars.length) return card;
  const shown = chars.slice(0, 6);
  const section = el("div", { class: "pc-section pc-chars" },
    el("h4", {}, "WOW CHARACTERS"),
    el("div", { class: "char-chips" }, shown.map((c) => c.armory_url
      ? el("a", { href: c.armory_url, target: "_blank", rel: "noopener noreferrer", class: "char-link" }, classChip(c))
      : classChip(c))),
    chars.length > shown.length ? el("div", { class: "muted small" }, `+${chars.length - shown.length} more`) : null);
  const body = card.querySelector(".pc-body");
  const since = [...(body?.querySelectorAll(".pc-section") ?? [])].find((x) => x.querySelector("h4")?.textContent === "MEMBER SINCE");
  body?.insertBefore(section, since ?? body.querySelector(".pc-actions"));
  return card;
};

// Raid sign-ups: one tap on a character picks its class, and the roster
// shows that character's name instead of yours.
const wowBaseOpenSignup = openSignup;
openSignup = function (ev, anchor, status) {
  wowBaseOpenSignup(ev, anchor, status);
  const pop = popover;
  const box = pop?.querySelector(".signup-pop");
  const mineChars = charsFor(ev.game);
  if (!box || !mineChars.length) return;
  const mine = ev.signups.find((s) => s.user_id === state.me.id);
  let picked = null;
  const mark = () => {
    row.querySelectorAll(".char-chip").forEach((b) => b.classList.toggle("on", b.dataset.id === picked?.id));
    if (picked) box.dataset.characterId = picked.id;
    else delete box.dataset.characterId;
    hint.textContent = picked ? `The roster shows you as ${charName(picked)}.` : "Not bringing one of these? The roster shows your name.";
  };
  const pick = (c) => {
    picked = picked?.id === c.id ? null : c;
    if (picked) box.querySelector(`.su-class[title="${c.class}"]`)?.click();
    mark();
  };
  const row = el("div", { class: "char-chips" }, mineChars.slice(0, 8).map((c) => {
    const b = classChip(c, { onclick: () => pick(c), small: true });
    b.dataset.id = c.id;
    return b;
  }));
  const hint = el("div", { class: "muted small su-char-hint" });
  // Picking a different class by hand means it isn't that character.
  box.querySelector(".su-classes")?.addEventListener("click", (e) => {
    const b = e.target.closest(".su-class");
    if (b && picked && b.title !== picked.class) { picked = null; mark(); }
  });
  const label = el("div", { class: "muted small" }, "Your characters");
  box.insertBefore(row, box.children[1]);
  box.insertBefore(label, row);
  box.insertBefore(hint, row.nextSibling);
  const already = mine?.character?.id ? mineChars.find((c) => c.id === mine.character.id) : null;
  const main = mainCharacter();
  if (already) pick(already);
  else if (!mine && !prefs.get("raid_class", "") && main && mineChars.includes(main)) pick(main);
  else mark();
  const r = anchor.getBoundingClientRect();
  const above = r.top - pop.offsetHeight - 8;
  placeFixed(pop, r.left, above >= 8 ? above : r.bottom + 8);
};

// Settings → Profile & picture gets a characters section.
const wowBaseDrawProfileLook = drawProfileLook;
drawProfileLook = async function (main) {
  await wowBaseDrawProfileLook(main);
  const section = el("section", { class: "chars-settings" });
  main.append(section);
  drawCharacters(section);
};

async function drawCharacters(section) {
  const [chars, bnet] = await Promise.all([
    api("GET", "/api/users/@me/characters").catch(() => state.myCharacters),
    api("GET", "/api/bnet/status").catch(() => ({ enabled: false })),
  ]);
  state.myCharacters = chars;
  const redraw = () => drawCharacters(section);
  const rows = chars.map((c) => el("div", { class: `char-row${c.is_main ? " main" : ""}`, style: `--cc:${classColour(c.class) ?? "var(--text)"}` },
    el("div", { class: "char-row-main" },
      el("div", { class: "char-row-name" }, c.is_main ? el("span", { class: "char-star", title: "Main" }, "★ ") : null, charName(c),
        el("span", { class: `char-game g-${c.game}` }, GAME_SHORT[c.game] ?? c.game),
        c.source === "bnet" ? el("span", { class: "char-src", title: "Imported from Battle.net" }, "Battle.net") : null,
        c.game === "forever" && !c.last_name ? el("button", { type: "button", class: "char-fix", onclick: () => openCharacterDialog(c, redraw) }, "Add last name") : null),
      el("div", { class: "muted small" }, [`Level ${c.level} ${[c.race, c.class].filter(Boolean).join(" ")}`, c.faction && !c.race.includes(c.faction) ? c.faction : null, c.game === "forever" ? c.ruleset : c.realm, c.professions.join(", ")].filter(Boolean).join(" · ")),
      c.armory_url ? el("a", { class: "small", href: c.armory_url, target: "_blank", rel: "noopener noreferrer" }, "Armory ↗") : null),
    el("div", { class: "char-row-actions" },
      c.is_main ? null : el("button", { class: "ghost small-btn", onclick: async () => {
        const ok = await tryApi("PATCH", `/api/characters/${c.id}`, { ...c, is_main: true }).then(() => true, () => false);
        if (ok) redraw();
      } }, "Make main"),
      el("button", { class: "ghost small-btn", onclick: () => openCharacterDialog(c, redraw) }, "Edit"),
      el("button", { class: "ghost small-btn danger", "aria-label": `Remove ${charName(c)}`, onclick: async () => {
        if (!(await confirmBox(`Remove ${charName(c)}?`, "They'll come off your profile. You can add them again any time.", "Remove"))) return;
        const ok = await tryApi("DELETE", `/api/characters/${c.id}`).then(() => true, () => false);
        if (ok) redraw();
      } }, "Remove"))));

  const bnetBox = bnet.enabled ? el("div", { class: "bnet-box" },
    el("div", {},
      el("strong", {}, "Battle.net"),
      el("div", { class: "muted small" }, bnet.linked
        ? `Linked${bnet.battletag ? ` as ${bnet.battletag}` : ""}. Link again to refresh your character list.`
        : "Sign in with Battle.net to bring in your Classic characters. You can still edit them here afterwards.")),
    el("div", { class: "inline-row" },
      el("button", { class: "ghost bnet-link", onclick: () => linkBattleNet(redraw) }, bnet.linked ? "Refresh from Battle.net" : "Link Battle.net"),
      bnet.linked ? el("button", { class: "ghost danger", onclick: async () => {
        const ok = await tryApi("DELETE", "/api/bnet/link").then(() => true, () => false);
        if (ok) { toast("Battle.net unlinked. Your characters stay."); redraw(); }
      } }, "Unlink") : null)) : null;

  section.replaceChildren(
    el("h2", {}, "WoW characters"),
    el("p", { class: "muted small" }, "Your characters show on your profile card, fill in your class when you sign up for raids or groups, and anyone can look them up with /char."),
    rows.length ? el("div", { class: "char-list" }, rows) : el("p", { class: "muted" }, "No characters yet."),
    el("div", { class: "inline-row" },
      el("button", { class: "primary add-char", disabled: chars.length >= 20, onclick: () => openCharacterDialog(null, redraw) }, "＋ Add character")),
    bnetBox,
  );
}

async function linkBattleNet(onDone) {
  const before = await api("GET", "/api/bnet/status").catch(() => null);
  const reply = await tryApi("POST", "/api/bnet/authorize").catch(() => null);
  if (!reply) return;
  window.open(reply.url, "_blank", "noopener");
  toast("Finish signing in to Battle.net in the new window.");
  // Watch for the link to land (works even when it opens in another browser).
  const started = Date.now();
  const check = async () => {
    const now = await api("GET", "/api/bnet/status").catch(() => null);
    const chars = await api("GET", "/api/users/@me/characters").catch(() => null);
    const changed = now && (now.linked !== before?.linked || now.battletag !== before?.battletag || (chars && chars.length !== state.myCharacters.length));
    if (changed) { toast("Battle.net linked."); onDone(); return; }
    if (Date.now() - started < 180_000 && document.querySelector(".chars-settings")) setTimeout(check, 3000);
  };
  setTimeout(check, 3000);
}

function openCharacterDialog(existing, onSaved) {
  const c = existing;
  const games = state.wowInfo?.games?.length ? state.wowInfo.games : [
    { id: "forever", name: "WoW: Forever", max_level: 60, two_names: true, rulesets: ["Normal", "PvP", "Roleplay", "Hardcore"], professions: state.wowInfo?.professions ?? [], races: [] },
  ];
  let g = games.find((x) => x.id === (c?.game || prefs.get("char_game", "forever"))) ?? games[0];
  const professions = new Set(c?.professions ?? []);
  const game = el("select", { class: "select", "aria-label": "Game" }, games.map((x) => el("option", { value: x.id, selected: x.id === g.id }, x.name)));
  const name = el("input", { maxlength: 12, value: c?.name ?? "", placeholder: "Thrall", required: true, "aria-label": "Name" });
  const lastName = el("input", { maxlength: 12, value: c?.last_name ?? "", placeholder: "Stormborn", "aria-label": "Last name" });
  const realm = el("input", { maxlength: 40, value: c?.game !== "forever" ? c?.realm ?? "" : "", placeholder: "Your realm", "aria-label": "Realm" });
  const ruleset = el("select", { class: "select", "aria-label": "Ruleset" });
  const race = el("select", { class: "select", "aria-label": "Race" });
  const cls = el("select", { class: "select", "aria-label": "Class" },
    el("option", { value: "" }, "Pick a class…"),
    WOW_CLASSES.map((k) => el("option", { value: k.name, selected: c?.class === k.name }, k.name)));
  const level = el("input", { type: "number", min: 1, max: g.max_level, value: c?.level ?? g.max_level, "aria-label": "Level" });
  const profBox = el("div", { class: "prof-chips", role: "group", "aria-label": "Professions" });
  const paintProfs = () => profBox.replaceChildren(...g.professions.map((p) => el("button", {
    type: "button", class: `day-chip prof-chip${professions.has(p) ? " on" : ""}`, "aria-pressed": professions.has(p) ? "true" : "false",
    onclick: () => { professions.has(p) ? professions.delete(p) : professions.size < 5 && professions.add(p); paintProfs(); },
  }, p)));
  const nameRow = el("div", { class: "raid-row even" });
  const placeRow = el("div", { class: "raid-row even char-place" });
  const nameHint = el("p", { class: "muted small", style: "margin:0" });
  // The form follows the game: names, realm or ruleset, races, level cap, professions.
  const paintGame = () => {
    nameRow.replaceChildren(
      el("label", { class: "field" }, g.two_names ? "First name" : "Name", name),
      g.two_names ? el("label", { class: "field" }, "Last name", lastName) : el("label", { class: "field" }, "Realm", realm));
    const keepRuleset = ruleset.value || c?.ruleset || "Normal";
    ruleset.replaceChildren(...g.rulesets.map((r) => el("option", { value: r, selected: r === keepRuleset }, r)));
    const keepRace = race.value || c?.race || "";
    race.replaceChildren(el("option", { value: "" }, "Race (optional)"), ...g.races.map((r) => el("option", { value: r, selected: r === keepRace }, r)));
    placeRow.replaceChildren(...[
      g.two_names ? el("label", { class: "field" }, "Ruleset", ruleset) : null,
      el("label", { class: "field" }, "Race", race)].filter(Boolean));
    placeRow.classList.toggle("even", g.two_names);
    // A new character starts at the game's level cap.
    if (!c && (!level.value || Number(level.value) === Number(level.max))) level.value = g.max_level;
    level.max = g.max_level;
    if (Number(level.value) > g.max_level) level.value = g.max_level;
    for (const p of [...professions]) if (!g.professions.includes(p)) professions.delete(p);
    nameHint.textContent = g.two_names
      ? "WoW: Forever has no realms: characters have a first and a last name (2 to 12 letters each) and a ruleset. Level cap 60. Skyborne pick Alliance or Horde when they're made."
      : `${g.name}: one name (2 to 12 letters) and a realm. Level cap ${g.max_level}.`;
    paintProfs();
  };
  game.addEventListener("change", () => { g = games.find((x) => x.id === game.value) ?? games[0]; paintGame(); });
  paintGame();
  const armory = el("input", { type: "url", maxlength: 300, value: c?.armory_url ?? "", placeholder: "https://… (optional)", "aria-label": "Armory link" });
  const isMain = el("input", { type: "checkbox", checked: c ? c.is_main : !state.myCharacters.length });
  const error = el("p", { class: "error" });
  // WoW: Forever changed which races can be which classes, so nothing is
  // refused; only the Skyborne's faction-specific classes get a heads-up.
  const comboWarn = el("p", { class: "muted small combo-warn", style: "margin:0" });
  const paintWarn = () => {
    const r = race.value, k = cls.value;
    comboWarn.textContent = g.id === "forever" && r === "Skyborne (Alliance)" && k === "Shaman"
      ? "Heads up: from what we've read, only Horde Skyborne can be Shamans. Saving anyway is fine if Forever says otherwise."
      : g.id === "forever" && r === "Skyborne (Horde)" && k === "Mage"
        ? "Heads up: from what we've read, only Alliance Skyborne can be Mages. Saving anyway is fine if Forever says otherwise."
        : "";
    comboWarn.hidden = !comboWarn.textContent;
  };
  race.addEventListener("change", paintWarn);
  cls.addEventListener("change", paintWarn);
  game.addEventListener("change", paintWarn);
  paintWarn();
  const form = el("form", { method: "dialog" },
    el("h2", {}, c ? `Edit ${charName(c)}` : "Add a character"),
    el("label", { class: "field" }, "Game", game),
    nameRow,
    nameHint,
    placeRow,
    el("div", { class: "raid-row even" },
      el("label", { class: "field" }, "Class", cls),
      el("label", { class: "field" }, "Level", level)),
    comboWarn,
    el("div", { class: "field" }, "Professions", profBox),
    el("label", { class: "field" }, "Armory or profile link", armory),
    el("label", { class: "check-row" }, isMain, "This is my main"),
    error,
    el("div", { class: "row" },
      el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"),
      el("button", { type: "submit", class: "primary" }, c ? "Save" : "Add character")));
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!cls.value) { error.textContent = "Pick a class."; return; }
    const body = {
      game: g.id, name: name.value.trim(),
      last_name: g.two_names ? lastName.value.trim() : "",
      realm: g.two_names ? "" : realm.value.trim(),
      ruleset: g.two_names ? ruleset.value : "",
      race: race.value,
      class: cls.value, level: Number(level.value) || g.max_level, professions: [...professions], armory_url: armory.value.trim(), is_main: isMain.checked,
    };
    try {
      const saved = await api(c ? "PATCH" : "POST", c ? `/api/characters/${c.id}` : "/api/users/@me/characters", body);
      prefs.set("char_game", g.id);
      dialog.close();
      toast(c ? "Character saved." : `${charName(saved)} added.`);
      onSaved();
    } catch (err) {
      error.textContent = err.message;
    }
  });
  const dialog = el("dialog", { class: "raid-dialog char-dialog" }, form);
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  name.focus();
}
