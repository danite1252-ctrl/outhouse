// Outhouse: audit log, server status, badges (settings, profile cards and
// next to names). Loaded after app.js / features.js and uses their helpers.

// ======================================================================
// Audit log (Outhouse settings → Audit log)
// ======================================================================

const AUDIT_ICONS = {
  channel: "#️⃣", role: "🎭", member: "👤", message: "💬", emoji: "😀", sound: "🔊",
  bot: "🤖", webhook: "🪝", feed: "📰", raid: "📅", badge: "🏅",
};
const AUDIT_DANGER = new Set(["member_kick", "member_ban", "channel_delete", "role_delete", "message_delete", "voice_disconnect", "bot_delete", "raid_delete", "member_timeout"]);

async function drawAuditLog(main) {
  const filters = { action: "", user: "" };
  let entries = [];
  let hasMore = false;
  let actions = [];
  let retention = 90;
  const list = el("div", { class: "audit-list" });
  const more = el("button", { class: "ghost audit-more", hidden: true, onclick: () => load(true) }, "Load older entries");
  const actionSel = el("select", { class: "select", "aria-label": "Filter by action" }, el("option", { value: "" }, "All actions"));
  const userSel = el("select", { class: "select", "aria-label": "Filter by person" }, el("option", { value: "" }, "Everyone"),
    [...state.members].filter((m) => !m.bot).sort((a, b) => a.display_name.localeCompare(b.display_name))
      .map((m) => el("option", { value: m.id }, m.display_name)));
  const intro = el("p", { class: "muted small", style: "margin:0" });
  actionSel.addEventListener("change", () => { filters.action = actionSel.value; load(false); });
  userSel.addEventListener("change", () => { filters.user = userSel.value; load(false); });

  const labelFor = (key) => actions.find((a) => a.key === key)?.label ?? key.replace(/_/g, " ");
  const row = (e) => {
    const who = e.actor_id ? (memberById(e.actor_id)?.display_name ?? e.actor_name ?? "Someone") : "Someone (account gone)";
    const when = new Date(e.created_at);
    return el("div", { class: `audit-row${AUDIT_DANGER.has(e.action) ? " danger" : ""}` },
      el("div", { class: "audit-icon", title: e.target_kind || "" }, AUDIT_ICONS[e.target_kind] ?? "📝"),
      el("div", { class: "audit-main" },
        el("div", { class: "audit-line" },
          e.actor_id ? el("button", { class: "audit-who", onclick: (ev) => openProfile(e.actor_id, ev.currentTarget) }, who) : el("strong", {}, who),
          el("span", { class: "audit-action" }, ` ${labelFor(e.action).replace(/^./, (c) => c.toLowerCase())}`),
          e.target_name ? [el("span", { class: "audit-action" }, ": "), e.target_kind === "member" && e.target_id
            ? el("button", { class: "audit-target", onclick: (ev) => openProfile(e.target_id, ev.currentTarget) }, e.target_name)
            : el("span", { class: "audit-target" }, e.target_name)] : null),
        e.details ? el("div", { class: "audit-details" }, e.details) : null),
      el("time", { class: "audit-time", datetime: e.created_at, title: when.toLocaleString() }, relTime(e.created_at)));
  };
  const paint = () => {
    list.replaceChildren(...(entries.length ? entries.map(row) : [el("div", { class: "sp-empty" },
      el("div", { class: "sp-empty-icon" }, "📜"),
      el("p", { class: "muted" }, filters.action || filters.user ? "Nothing matches those filters." : "Nothing in the log yet. Changes to roles, channels and members show up here."))]));
    more.hidden = !hasMore;
  };
  async function load(older) {
    const q = new URLSearchParams({ limit: "50" });
    if (filters.action) q.set("action", filters.action);
    if (filters.user) q.set("user", filters.user);
    if (older && entries.length) q.set("before", entries[entries.length - 1].id);
    more.disabled = true;
    const page = await tryApi("GET", `/api/guilds/${state.guildId}/audit-log?${q}`).catch(() => null);
    more.disabled = false;
    if (!page) return;
    if (!actions.length) {
      actions = page.actions;
      retention = page.retention_days;
      actionSel.append(...actions.map((a) => el("option", { value: a.key }, a.label)));
      intro.textContent = `Who changed what in ${currentGuild()?.name ?? "this outhouse"}. Entries are kept for ${retention} days.`;
    }
    entries = older ? entries.concat(page.entries) : page.entries;
    hasMore = page.has_more;
    paint();
  }
  main.append(
    el("h2", {}, "Audit log"),
    intro,
    el("div", { class: "audit-filters" }, actionSel, userSel),
    list,
    more);
  await load(false);
}

// ======================================================================
// Server status (Outhouse settings → Server status, Manage Outhouse)
// ======================================================================

function fmtUptime(seconds) {
  const d = Math.floor(seconds / 86400), h = Math.floor((seconds % 86400) / 3600), m = Math.floor((seconds % 3600) / 60);
  if (d) return `${d} day${d === 1 ? "" : "s"}, ${h} hour${h === 1 ? "" : "s"}`;
  if (h) return `${h} hour${h === 1 ? "" : "s"}, ${m} min`;
  return `${m} min`;
}

async function drawServerStatus(main) {
  const s = await tryApi("GET", `/api/guilds/${state.guildId}/server-status`).catch(() => null);
  if (!s) return;
  const b = s.backup;
  const backupLine = !b ? ["unknown", "No backup status yet (backups run nightly on the server; this shows once backup.sh has run)."]
    : b.ok === false ? ["bad", `Last backup failed ${b.time ? relTime(b.time) : ""}: ${b.message || "see backup.log"}`]
      : [String(b.offsite || "").startsWith("failed") ? "warn" : "good",
        `${b.time ? relTime(b.time) : "?"} · ${b.size || "?"}${b.offsite && b.offsite !== "off" ? ` · off-site copy ${b.offsite.startsWith("ok") ? "✓" : b.offsite}` : " · no off-site copy"}`];
  const item = (label, value, cls = "") => el("div", { class: `status-item ${cls}` }, el("div", { class: "status-label" }, label), el("div", { class: "status-value" }, value));
  main.append(
    el("h2", {}, "Server status"),
    el("div", { class: "status-line" },
      el("span", { class: `push-dot${s.db_ok ? " on" : " bad"}` }),
      el("span", {}, s.db_ok ? `Up ${fmtUptime(s.uptime_seconds)}` : "The database isn't answering"),
      el("span", { class: "muted" }, ` · version ${s.version}`)),
    el("div", { class: "status-grid" },
      item("Version", s.version),
      item("Up since", `${new Date(s.started_at).toLocaleString()} (${fmtUptime(s.uptime_seconds)})`),
      item("Database", s.db_ok ? `OK · ${s.db_size_bytes != null ? fmtSize(s.db_size_bytes) : "?"}` : "Not answering", s.db_ok ? "good" : "bad"),
      item("Last backup", backupLine[1], backupLine[0]),
      item("Online now", `${s.online_users} ${s.online_users === 1 ? "person" : "people"} (whole server)`),
      item("Push notifications", s.push_enabled ? "Available" : "Off"),
      item("Voice", s.voice_configured ? "Set up" : "Not set up", s.voice_configured ? "" : "warn")),
    el("p", { class: "muted small", style: "margin:0" }, "This is the whole Outhouse server (every outhouse on it). Uptime alerts and off-site backups are set up on the server with deploy/setup.sh; see deploy/README.md."),
    el("div", {}, el("button", { class: "ghost", onclick: () => { main.replaceChildren(); drawServerStatus(main); } }, "Refresh")));
}

// ======================================================================
// Badges
// ======================================================================

state.badges = new Map(); // guild ID → [badge]
const BADGES_NEXT_TO_NAMES = ["custom", "raid_leader", "attendance"];

async function loadBadges(guildId = state.guildId) {
  if (!guildId) return [];
  const list = await api("GET", `/api/guilds/${guildId}/badges`).catch(() => null);
  if (!list) return [];
  state.badges.set(guildId, list);
  if (guildId === state.guildId) { renderMembers(); renderMessages(); }
  return list;
}

function badgesFor(userId, guildId = state.guildId) {
  return (state.badges.get(guildId) ?? []).filter((b) => b.holders.includes(userId));
}

function badgeChip(b, { small = false } = {}) {
  const colour = hexColour(b.color) || "#a1a1aa";
  return el("span", { class: `badge-chip${small ? " small" : ""}`, style: `--bc:${colour}`, title: b.description ? `${b.name}: ${b.description}` : b.name },
    el("span", { class: "badge-emoji" }, b.emoji), small ? null : el("span", { class: "badge-name" }, b.name));
}

function nameBadges(userId) {
  if (!prefs.get("badges_by_names", true)) return null;
  const shown = badgesFor(userId).filter((b) => (b.auto ? BADGES_NEXT_TO_NAMES.includes(b.key) : true)).slice(0, 3);
  if (!shown.length) return null;
  return el("span", { class: "name-badges" }, shown.map((b) => el("span", { class: "name-badge", title: b.name }, b.emoji)));
}

// Profile cards: a row of badges under the name.
const badgesBaseProfileCard = profileCard;
profileCard = function (p, opts) {
  const card = badgesBaseProfileCard(p, opts);
  const mine = badgesFor(p.id);
  if (!mine.length) return card;
  const user = card.querySelector(".pc-user");
  user?.after(el("div", { class: "pc-badges" }, mine.map((b) => badgeChip(b))));
  return card;
};

// Next to names in the member list and on messages.
const badgesBaseRenderMembers = renderMembers;
renderMembers = function () {
  badgesBaseRenderMembers();
  if (!prefs.get("badges_by_names", true)) return;
  for (const row of document.querySelectorAll("#member-list .member[data-id]")) {
    const name = row.querySelector(".info > div");
    const extra = nameBadges(row.dataset.id);
    if (name && extra) name.append(extra);
  }
};
const badgesBaseRenderMessages = renderMessages;
renderMessages = function (...args) {
  const out = badgesBaseRenderMessages(...args);
  if (prefs.get("badges_by_names", true) && state.badges.get(state.guildId)?.length) {
    for (const article of document.querySelectorAll("#messages article.msg.first[data-id]")) {
      const m = findMessage(article.dataset.id);
      const author = article.querySelector(".msg-meta .msg-author");
      if (!m || !author || author.nextElementSibling?.classList.contains("name-badges")) continue;
      const extra = nameBadges(m.author_id);
      if (extra) author.after(extra);
    }
  }
  return out;
};

const badgesBaseSelectGuild = selectGuild;
selectGuild = async function (id, ...rest) {
  const result = await badgesBaseSelectGuild(id, ...rest);
  if (id && id !== "0" && !state.badges.has(id)) loadBadges(id);
  return result;
};

const badgesBaseHandle = handleEvent;
handleEvent = function (ev) {
  badgesBaseHandle(ev);
  if (ev.t === "BADGES_CHANGED" && state.badges.has(ev.d.guild_id)) loadBadges(ev.d.guild_id);
  if (ev.t === "GUILD_REFRESH" && ev.d.guild_id === state.guildId) loadBadges(ev.d.guild_id);
};

// A switch for badges next to names, on Settings → Profile & picture.
const badgesBaseDrawProfileLook = drawProfileLook;
drawProfileLook = async function (main) {
  await badgesBaseDrawProfileLook(main);
  const box = el("input", { type: "checkbox", checked: prefs.get("badges_by_names", true) });
  box.addEventListener("change", () => { prefs.set("badges_by_names", box.checked); renderMembers(); renderMessages(); });
  const form = main.querySelector(".pl-form");
  (form ?? main).append(el("label", { class: "check-row" }, box, el("span", {}, "Show badges next to people's names (on this device)")));
};

// Outhouse settings → Badges (Manage Outhouse): make custom badges and hand them out.
const BADGE_EMOJI = ["🏅", "🏆", "⭐", "🔥", "💎", "🛡️", "⚔️", "🧙", "🐉", "🍺", "🎯", "👑", "🌱", "🧻", "🚽", "❤️"];

async function drawBadgeSettings(main) {
  const list = await loadBadges();
  const redraw = () => { main.replaceChildren(); drawBadgeSettings(main); };
  const auto = list.filter((b) => b.auto);
  const custom = list.filter((b) => !b.auto);

  let emoji = BADGE_EMOJI[0];
  const emojiBtn = el("button", { type: "button", class: "badge-emoji-btn", title: "Pick an emoji" }, emoji);
  emojiBtn.addEventListener("click", () => openPopover(emojiBtn, el("div", { class: "emoji-grid badge-emoji-grid" },
    [...BADGE_EMOJI, ...EMOJI.slice(0, 48)].map((e) => el("button", { type: "button", onclick: () => { emoji = e; emojiBtn.textContent = e; closePopover(); } }, e)))));
  const name = el("input", { type: "text", maxlength: 32, placeholder: "Raid MVP" });
  const desc = el("input", { type: "text", maxlength: 100, placeholder: "What it's for (optional)" });
  const colour = el("input", { type: "color", value: "#ffb347" });

  const holderChips = (b) => b.holders.map((uid) => {
    const m = memberById(uid);
    return el("span", { class: "chip holder-chip" }, avatarFor(uid, m?.display_name, 18), m?.display_name ?? "Someone",
      el("button", { title: "Take it away", "aria-label": "Take it away", onclick: async () => {
        await tryApi("DELETE", `/api/badges/${b.id}/members/${uid}`).catch(() => {});
        redraw();
      } }, "✕"));
  });
  const giveSelect = (b) => {
    const sel = el("select", { class: "select give-select", "aria-label": `Give ${b.name} to…` },
      el("option", { value: "" }, "Give to…"),
      state.members.filter((m) => !b.holders.includes(m.id)).sort((x, y) => x.display_name.localeCompare(y.display_name))
        .map((m) => el("option", { value: m.id }, m.display_name)));
    sel.addEventListener("change", async () => {
      if (!sel.value) return;
      await tryApi("PUT", `/api/badges/${b.id}/members/${sel.value}`).catch(() => {});
      redraw();
    });
    return sel;
  };

  main.append(
    el("h2", {}, "Badges"),
    el("p", { class: "muted", style: "margin:0" }, "Badges show on profile cards (and next to names, unless someone turns that off). Some are handed out automatically; make your own for MVPs, officers, birthdays…"),
    el("div", { class: "int-form badge-form" },
      el("div", { class: "inline-row" }, emojiBtn, name, colour),
      desc,
      el("div", {}, el("button", { class: "primary", onclick: async () => {
        if (!name.value.trim()) return name.focus();
        const b = await tryApi("POST", `/api/guilds/${state.guildId}/badges`, {
          name: name.value.trim(), emoji, color: parseInt(colour.value.slice(1), 16), description: desc.value.trim(),
        }).catch(() => null);
        if (b) redraw();
      } }, "Make badge"))),
    el("h3", {}, "Your badges"),
    ...(custom.length ? custom.map((b) => el("div", { class: "int-item badge-item" },
      el("div", { class: "badge-head" },
        badgeChip(b),
        b.description ? el("span", { class: "muted small" }, b.description) : null,
        el("span", { style: "flex:1" }),
        el("button", { class: "ghost small-btn", onclick: async () => {
          const n = await ask("Rename badge", { value: b.name, confirm: "Save" });
          if (n && n.trim() && n.trim() !== b.name) { await tryApi("PATCH", `/api/badges/${b.id}`, { name: n.trim() }).catch(() => {}); redraw(); }
        } }, "Rename"),
        el("button", { class: "ghost small-btn danger", onclick: async () => {
          if (!(await confirmBox(`Delete ${b.emoji} ${b.name}?`, "Everyone who has it loses it.", "Delete"))) return;
          await tryApi("DELETE", `/api/badges/${b.id}`).catch(() => {});
          redraw();
        } }, "Delete")),
      el("div", { class: "holder-row" }, ...holderChips(b), giveSelect(b)))) : [el("p", { class: "muted small", style: "margin:0" }, "None yet.")]),
    el("h3", {}, "Automatic badges"),
    el("div", { class: "auto-badges" }, auto.map((b) => el("div", { class: "auto-badge" },
      badgeChip(b),
      el("span", { class: "muted small" }, b.description),
      el("span", { class: "muted small auto-count" }, `${b.holders.length} ${b.holders.length === 1 ? "person" : "people"}`)))),
  );
}
