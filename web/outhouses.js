// Outhouse: the site owner's Outhouses directory (User settings → SITE OWNER
// → Outhouses): every outhouse on the server, searchable, with a read-only
// look inside, family outhouses, and shutting one down / restoring /
// deleting it for good. Also the notice members see in an outhouse that was
// shut down. Loaded after safety.js; uses its helpers (scWhen, scPlural,
// scSection, formDialog, logView, catChip) and app.js globals.

const dirState = { q: "", sort: "name", page: 1 };
const SHUT_DOWN_TEXT = "This outhouse has been shut down by the server owner.";
const dirDate = (iso) => (iso ? new Date(iso).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" }) : "");

function dirIcon(o, size = 40) {
  const box = el("span", { class: "dir-icon", style: `width:${size}px;height:${size}px;flex-basis:${size}px` });
  if (o.icon_url) box.append(el("img", { src: o.icon_url, alt: "" }));
  else box.append(initials(o.name));
  return box;
}

function dirChips(o) {
  return [
    o.shut_down ? el("span", { class: "rp-chip dir-shut" }, "Shut down") : null,
    o.family ? el("span", { class: "rp-chip dir-family" }, "Family") : null,
    o.open_reports ? el("span", { class: "rp-chip dir-reports" }, scPlural(o.open_reports, "open report")) : null,
    o.owner?.suspended ? el("span", { class: "rp-chip dir-susp" }, "Owner suspended") : null,
  ];
}

async function drawOuthousesDirectory(page, ctx) {
  if (ctx.outhouseId) return drawOuthouseDetail(page, ctx.outhouseId, ctx);
  const search = el("input", { type: "search", class: "dir-search", placeholder: "Search by name, old name, ID or owner's username", value: dirState.q, "aria-label": "Search outhouses" });
  const sort = el("select", { class: "select dir-sort", "aria-label": "Sort by" },
    [["name", "Name"], ["members", "Most members"], ["created", "Newest"], ["activity", "Most active"], ["reports", "Open reports"]]
      .map(([v, l]) => el("option", { value: v, selected: dirState.sort === v }, l)));
  const summary = el("p", { class: "dir-summary" });
  const list = el("div", { class: "sc-list dir-list" });
  const pager = el("div", { class: "dir-pager" });
  page.append(
    el("h2", {}, "Outhouses"),
    summary,
    el("div", { class: "dir-tools" }, search, sort),
    list, pager);
  let seq = 0;
  const load = async () => {
    const mine = ++seq;
    const q = new URLSearchParams({ q: dirState.q, sort: dirState.sort, page: String(dirState.page) });
    const d = await tryApi("GET", `/api/safety/outhouses?${q}`).catch(() => null);
    if (!d || mine !== seq || !ctx.stillMine() || !page.isConnected) return;
    summary.replaceChildren(
      el("strong", {}, scPlural(d.total_outhouses, "outhouse")), " · ",
      el("strong", {}, scPlural(d.total_people, "person").replace(/persons$/, "people")),
      ...(d.total_shut_down ? [" · ", el("span", { class: "dir-shut-count" }, `${d.total_shut_down} shut down`)] : []),
      dirState.q ? el("span", { class: "muted" }, ` · ${d.matching} matching “${dirState.q}”`) : null);
    list.replaceChildren(...(d.outhouses.length ? d.outhouses.map((o) => el("button", {
      type: "button", class: `sc-row dir-row${o.shut_down ? " shut" : ""}`, "data-outhouse": o.id,
      onclick: () => ctx.draw("outhouses", { outhouseId: o.id }),
    },
    el("div", { class: "dir-row-main" }, dirIcon(o),
      el("div", { class: "dir-row-text" },
        el("div", { class: "dir-row-name" }, el("strong", {}, o.name), ...dirChips(o)),
        el("div", { class: "muted small" },
          `@${o.owner.username} · ${scPlural(o.members, "member")} · ${o.online} online · ${scPlural(o.messages_7d, "message")} this week · built ${dirDate(o.created_at)}`),
        o.previous_names.length ? el("div", { class: "muted small dir-prev" }, `Previously: ${o.previous_names.join(", ")}`) : null)))) : [el("p", { class: "muted" }, "No outhouses match.")]));
    pager.replaceChildren(
      el("button", { type: "button", class: "ghost", disabled: d.page <= 1, onclick: () => { dirState.page -= 1; load(); } }, "← Previous"),
      el("span", { class: "muted small" }, `Page ${d.page} of ${d.pages}`),
      el("button", { type: "button", class: "ghost", disabled: d.page >= d.pages, onclick: () => { dirState.page += 1; load(); } }, "Next →"));
  };
  let timer = null;
  search.addEventListener("input", () => {
    clearTimeout(timer);
    timer = setTimeout(() => { dirState.q = search.value.trim(); dirState.page = 1; load(); }, 250);
  });
  sort.addEventListener("change", () => { dirState.sort = sort.value; dirState.page = 1; load(); });
  await load();
}

async function outhouseAction(id, body) {
  return tryApi("POST", `/api/safety/outhouses/${id}/actions`, body).catch(() => null);
}

async function drawOuthouseDetail(page, id, ctx) {
  const d = await tryApi("GET", `/api/safety/outhouses/${id}`).catch(() => null);
  if (!ctx.stillMine() || !page.isConnected) return;
  const back = el("div", { class: "sc-back" }, el("button", { type: "button", class: "link", onclick: () => ctx.draw("outhouses") }, "← All outhouses"));
  if (!d) { page.replaceChildren(back, el("p", { class: "muted" }, "That outhouse is gone.")); return; }
  const redraw = () => ctx.draw("outhouses", { outhouseId: id });
  const item = (label, value, cls = "") => el("div", { class: `status-item ${cls}` }, el("div", { class: "status-label" }, label), el("div", { class: "status-value" }, value));
  const shutDown = async () => {
    const out = await formDialog(`Shut down ${d.name}?`, [
      { name: "reason", label: "Why (only you see this; it's kept in the safety log)", type: "textarea", max: 500, placeholder: "e.g. Spam outhouse, reported by several people" },
      d.owner.id !== state.me.id ? { name: "suspend", label: `Also suspend the owner, @${d.owner.username} (until you review it)`, type: "checkbox" } : null,
    ].filter(Boolean), "Shut down", {
      danger: true,
      intro: `Nobody can post, join, talk or make invites in it. It disappears from the outhouse list of everyone in it (${scPlural(d.members, "member")}), and they're told “${SHUT_DOWN_TEXT}” Everything in it is kept as evidence until you delete it for good. You can restore it.`,
    });
    if (!out) return;
    if (!out.reason) return toast("Write why it's being shut down.");
    const r = await outhouseAction(id, { action: "shut_down", reason: out.reason, suspend_owner: !!out.suspend });
    if (r) { toast(`${d.name} is shut down.`); redraw(); }
  };
  const deleteForGood = async () => {
    const out = await formDialog(`Delete ${d.name} for good?`, [
      { name: "name", label: `Type the outhouse's name (${d.name}) to confirm`, type: "text", max: 120 },
    ], "Delete permanently", {
      danger: true,
      intro: "Its channels, messages, files and members are deleted and can't be brought back. Not possible while a report about it is open or a police download link is live.",
    });
    if (!out) return;
    const r = await outhouseAction(id, { action: "delete", confirm_name: out.name });
    if (r?.deleted) { toast(`${d.name} was deleted.`); ctx.draw("outhouses"); }
  };
  const blockers = d.open_reports || d.live_police_links;
  const actions = el("div", { class: "fp-actions dir-actions" },
    el("button", { type: "button", class: "primary", onclick: () => page.querySelector("#dir-channels")?.scrollIntoView({ behavior: "smooth", block: "start" }) }, "Open as owner"),
    el("button", { type: "button", class: "ghost", onclick: async () => { if (await outhouseAction(id, { action: "family", family: !d.family })) redraw(); } }, d.family ? "Unmark family" : "Mark as family"),
    d.shut_down
      ? [el("button", { type: "button", class: "ghost", onclick: async () => {
        let body = { action: "restore" };
        if (d.members_removed_at) {
          const n = d.member_snapshot.length;
          const out = await formDialog(`Restore ${d.name}?`, [
            { name: "readd", label: `Re-add the ${scPlural(n, "previous member")} (not anyone suspended or banned since)`, type: "checkbox", value: true },
          ], "Restore", { intro: `Its members were removed on ${scWhen(d.members_removed_at)}.` });
          if (!out) return;
          body = { action: "restore", readd: !!out.readd };
        }
        if (await outhouseAction(id, body)) { toast(`${d.name} is open again.`); redraw(); }
      } }, "Restore"),
        el("button", { type: "button", class: "danger-btn", disabled: !!blockers, title: blockers ? "Close its open reports (and wait for police links) first" : "", onclick: deleteForGood }, "Delete permanently")]
      : el("button", { type: "button", class: "danger-btn dir-shut-btn", onclick: shutDown }, "Shut down outhouse"));
  page.replaceChildren(
    back,
    el("div", { class: "dir-head" }, dirIcon(d, 64),
      el("div", { class: "dir-head-text" },
        el("h2", {}, d.name),
        el("div", { class: "dir-row-name" }, ...dirChips(d)),
        el("div", { class: "muted small" }, `ID ${d.id} · built ${dirDate(d.created_at)}`),
        d.previous_names.length ? el("div", { class: "muted small" }, `Previously: ${d.previous_names.join(", ")}`) : null)),
    d.shut_down ? el("div", { class: "sc-urgent-banner dir-shut-banner", role: "status" },
      el("strong", {}, `Shut down ${scWhen(d.shut_down_at)}.`), ` Reason: ${d.shut_down_reason}`,
      el("div", { class: "small" }, d.members_removed_at
        ? `Members removed on ${scWhen(d.members_removed_at)} (who they were is listed below).`
        : `Members keep it on their list (closed) until ${scWhen(d.members_remove_at)}, then they're removed.`),
      blockers ? el("div", { class: "small" }, `Can't be deleted yet: ${d.open_reports ? scPlural(d.open_reports, "open report") : ""}${d.open_reports && d.live_police_links ? " and " : ""}${d.live_police_links ? "a live police download link" : ""}.`) : null) : null,
    actions,
    el("div", { class: "status-grid" },
      item("Owner", el("button", { type: "button", class: "link dir-owner", onclick: () => ctx.openUser(d.owner.id) }, `${d.owner.display_name} (@${d.owner.username})`), d.owner.suspended ? "bad" : ""),
      item("Members", `${d.members} · ${d.online} online now`),
      item("Messages", `${d.messages_7d} this week · ${d.messages_30d} in 30 days`),
      item("Channels", String(d.channels)),
      item("Open reports", String(d.open_reports), d.open_reports ? "warn" : "good"),
      item("Image detector", d.detector_enabled ? "On" : "Off", d.detector_enabled ? "good" : "warn")),
    scSection("Channels (read-only, as the site owner)",
      el("p", { class: "muted small", style: "margin:0" }, "Look without joining. Every look goes in the safety log. Messages held for a safety review stay hidden, and quarantined files are never shown."),
      el("div", { class: "sc-list", id: "dir-channels" }, d.channels_list.map((c) => c.kind === 2
        ? el("div", { class: "sc-row static dir-ch" }, el("span", {}, `🔈 ${c.name}`), el("span", { class: "muted small" }, "voice"))
        : el("button", { type: "button", class: "sc-row dir-ch", "data-channel": c.id, onclick: () => drawOwnerChannel(page, d, c, ctx) },
          el("span", {}, `# ${c.name}`), el("span", { class: "muted small" }, `${scPlural(c.messages, "message")}${c.last_message_at ? ` · last ${scWhen(c.last_message_at)}` : ""}`))))),
    scSection(`Members (${d.members_list.length})`,
      el("div", { class: "sc-list" }, d.members_list.map((m) => el("button", { type: "button", class: "sc-row dir-member", onclick: () => (m.bot ? null : ctx.openUser(m.id)) },
        el("div", { class: "sc-row-who" },
          el("strong", {}, m.display_name), el("span", { class: "muted" }, ` @${m.username}`),
          m.owner ? el("span", { class: "rp-chip" }, "Owner") : null,
          m.bot ? el("span", { class: "rp-chip" }, "Bot") : m.age_bracket === "teen" ? el("span", { class: "rp-chip dir-teen" }, "Teen") : el("span", { class: "rp-chip dir-adult" }, "Adult"),
          m.suspended ? el("span", { class: "rp-chip dir-susp" }, "Suspended") : null),
        m.roles.length ? el("div", { class: "muted small" }, m.roles.join(", ")) : null)))),
    d.members_removed_at && d.member_snapshot.length ? scSection(`Members when they were removed (${d.member_snapshot.length})`,
      el("div", { class: "sc-list dir-snapshot" }, d.member_snapshot.map((m) => el("button", { type: "button", class: "sc-row dir-member", onclick: () => (m.bot ? null : ctx.openUser(m.user_id)) },
        el("div", { class: "sc-row-who" },
          el("strong", {}, m.display_name), el("span", { class: "muted" }, ` @${m.username}`),
          m.bot ? el("span", { class: "rp-chip" }, "Bot") : m.age_bracket === "teen" ? el("span", { class: "rp-chip dir-teen" }, "Teen") : el("span", { class: "rp-chip dir-adult" }, "Adult")),
        el("div", { class: "muted small" }, [`Joined ${dirDate(m.joined_at)}`, m.roles?.length ? m.roles.join(", ") : null].filter(Boolean).join(" · ")))))) : null,
    scSection("Reports involving it",
      d.reports.length ? el("div", { class: "sc-list" }, d.reports.map((r) => el("button", { type: "button", class: "sc-row", onclick: () => ctx.openReport(r.id) },
        el("div", { class: "sc-row-top" }, catChip(r), el("span", { class: "rp-chip" }, SC_STATUS[r.status] || r.status), el("span", { class: "sc-when" }, scWhen(r.created_at)))))) : el("p", { class: "muted small", style: "margin:0" }, "None.")),
    scSection("Safety log", logView(d.log)));
}

// A channel's history, read-only, as the site owner (the look is logged).
async function drawOwnerChannel(page, d, c, ctx) {
  const list = el("div", { class: "dir-msgs" });
  const more = el("button", { type: "button", class: "ghost", hidden: true }, "Load older");
  page.replaceChildren(
    el("div", { class: "sc-back" }, el("button", { type: "button", class: "link", onclick: () => ctx.draw("outhouses", { outhouseId: d.id }) }, `← ${d.name}`)),
    el("h2", {}, `#${c.name}`),
    el("p", { class: "muted small dir-ro-note", style: "margin:0" }, `Read-only, as the site owner. You aren't a member of ${d.name}; this look is in the safety log.`),
    more, list);
  let oldest = null;
  const msgRow = (m) => el("div", { class: `dir-msg${m.held ? " held" : ""}` },
    el("div", { class: "dir-msg-head" }, el("strong", {}, m.author_name), el("span", { class: "muted small" }, ` @${m.author_username} · ${scWhen(m.created_at)}${m.edited_at ? " · edited" : ""}`)),
    m.held ? el("div", { class: "muted small" }, "🔒 Held for a safety review (open it from Reports).")
      : el("div", { class: "dir-msg-text" }, m.content),
    m.files.length ? el("div", { class: "dir-msg-files" }, m.files.map((f) => el("a", { href: f.url, target: "_blank", rel: "noopener noreferrer" }, `📎 ${f.filename}`))) : null);
  const load = async () => {
    const q = oldest ? `?before=${oldest}` : "";
    const r = await tryApi("GET", `/api/safety/outhouses/${d.id}/channels/${c.id}/messages${q}`).catch(() => null);
    if (!r || !page.isConnected) return;
    if (r.messages.length) oldest = r.messages[0].id;
    list.prepend(...r.messages.map(msgRow));
    if (!oldest) list.replaceChildren(el("p", { class: "muted" }, "No messages."));
    more.hidden = !r.has_more;
  };
  more.addEventListener("click", load);
  await load();
}

// ---------- members' side: a shut-down outhouse ----------
// It stays on their rail, closed, until members are removed (`closed_until`).

const closedGuild = (id) => state.guilds.find((g) => g.id === id && g.closed_until);
const dirDay = (iso) => new Date(iso).toLocaleDateString([], { weekday: "long", day: "numeric", month: "long", year: "numeric" });

function showShutDownNotice(name, removeAt = null, guildId = null) {
  $("app").classList.add("tombstone-mode");
  state.channelId = null;
  state.channels = [];
  $("guild-name").textContent = name || "Outhouse";
  for (const id of ["text-channels", "voice-channels", "member-list"]) $(id)?.replaceChildren();
  $("channel-name").textContent = name || "Closed";
  $("channel-topic").textContent = "";
  $("composer-input").disabled = true;
  const leave = guildId ? el("button", { type: "button", class: "primary tombstone-leave", onclick: async () => {
    leave.disabled = true;
    const ok = await tryApi("POST", `/api/guilds/${guildId}/leave`).then(() => true, () => false);
    leave.disabled = false;
    if (!ok) return;
    state.guilds = state.guilds.filter((g) => g.id !== guildId);
    state.guildId = null;
    $("app").classList.remove("tombstone-mode");
    toast(`You left ${name}.`);
    renderGuilds();
    if (state.guilds[0]) selectGuild(state.guilds[0].id);
    else if (typeof openHome === "function") openHome();
  } }, "Leave now") : null;
  $("messages").replaceChildren(el("div", { class: "empty tombstone" },
    el("div", { class: "tombstone-icon", "aria-hidden": "true" }, "🚧"),
    el("h2", {}, name ? `${name} is closed` : "This outhouse is closed"),
    el("p", {}, SHUT_DOWN_TEXT),
    removeAt ? el("p", { class: "muted tombstone-date" }, `It will be removed from your list on ${dirDay(removeAt)}.`) : el("p", { class: "muted small" }, "Its channels can't be used any more."),
    leave));
}

// No unread dots or counts from a closed outhouse: a small lock instead.
if (typeof guildBadge === "function") {
  const dirBaseGuildBadge = guildBadge;
  guildBadge = function (guildId) {
    if (closedGuild(guildId)) return el("span", { class: "closed-badge", title: "Closed by the server owner", "aria-label": "Closed" }, "🔒");
    return dirBaseGuildBadge(guildId);
  };
}
if (typeof guildUnread === "function") {
  const dirBaseGuildUnread = guildUnread;
  guildUnread = function (guildId) { return closedGuild(guildId) ? false : dirBaseGuildUnread(guildId); };
}

const dirBaseHandle = handleEvent;
handleEvent = function (ev) {
  if (ev.t === "GUILD_SHUT_DOWN") {
    const g = state.guilds.find((x) => x.id === ev.d.guild_id);
    if (g) g.closed_until = ev.d.remove_at;
    if (state.guildId === ev.d.guild_id) {
      if (typeof exitRolePreview === "function") exitRolePreview({ refresh: false });
      if (state.voice?.channelId && typeof leaveVoice === "function" && state.channels.some((c) => c.id === state.voice.channelId)) leaveVoice(true);
      showShutDownNotice(ev.d.name, ev.d.remove_at, ev.d.guild_id);
    }
    renderGuilds();
    return;
  }
  const wasClosed = ev.t === "GUILD_UPDATE" && !!closedGuild(ev.d.id);
  dirBaseHandle(ev);
  // Restored: open it properly again.
  if (wasClosed && !ev.d.closed_until && state.guildId === ev.d.id) selectGuild(ev.d.id);
};

// Opening a closed outhouse shows the notice (with "Leave now"), not its channels.
const dirBaseSelectGuild = selectGuild;
selectGuild = function (id, ...rest) {
  const g = closedGuild(id);
  if (g) {
    state.guildId = id;
    localStorage.setItem("outhouse_guild", id);
    renderGuilds();
    showShutDownNotice(g.name, g.closed_until, id);
    return Promise.resolve();
  }
  $("app").classList.remove("tombstone-mode");
  return dirBaseSelectGuild(id, ...rest);
};
if (typeof openHome === "function") {
  const dirBaseOpenHome = openHome;
  openHome = function (...args) {
    $("app").classList.remove("tombstone-mode");
    return dirBaseOpenHome(...args);
  };
}
