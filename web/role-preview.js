// Outhouse: "View outhouse as role" (Outhouse settings → Roles). The server
// works out what someone with only @everyone and the chosen roles could do
// (GET /api/guilds/{id}/preview-permissions), using the same rules as for a
// real member; the client then draws the channel list, composer and menus
// from that. It only hides things: channels you can't see yourself never
// appear, and nothing is changed. Loaded after app.js and guild-info.js.

state.preview = null; // { guildId, roleIds, perms }

const previewRoleName = (id) => (id === state.guildId ? "@everyone" : state.roles.find((r) => r.id === id)?.name ?? "role");

async function fetchPreviewPerms(guildId, roleIds) {
  const q = roleIds.filter((id) => id !== guildId).join(",");
  return api("GET", `/api/guilds/${guildId}/preview-permissions?roles=${encodeURIComponent(q)}`);
}

async function startRolePreview(roleIds) {
  const guildId = state.guildId;
  if (!guildId) return;
  let perms;
  try {
    perms = await fetchPreviewPerms(guildId, roleIds);
  } catch (err) {
    return toast(err.message);
  }
  if (state.guildId !== guildId) return;
  state.preview = { guildId, roleIds: [...new Set(roleIds.filter((id) => id !== guildId))], perms, channels: state.channels };
  document.querySelectorAll("dialog.settings[open]").forEach((d) => d.close());
  applyPreview();
}

async function setPreviewRoles(roleIds) {
  if (!state.preview) return startRolePreview(roleIds);
  const guildId = state.preview.guildId;
  const perms = await fetchPreviewPerms(guildId, roleIds).catch((err) => { toast(err.message); return null; });
  if (!perms || !state.preview || state.guildId !== guildId) return;
  state.preview.roleIds = [...new Set(roleIds.filter((id) => id !== guildId))];
  state.preview.perms = perms;
  applyPreview();
}

function exitRolePreview({ refresh = true } = {}) {
  if (!state.preview) return;
  state.preview = null;
  $("app").classList.remove("previewing");
  document.getElementById("preview-banner")?.remove();
  if (refresh && state.guildId) refreshGuild();
}

// Swaps in the preview's permissions and hides what that role can't see.
function applyPreview() {
  const pv = state.preview;
  if (!pv || pv.guildId !== state.guildId) return;
  const p = pv.perms;
  state.myPerms = { guild: big(p.guild), channels: p.channels, top: p.top_position };
  // From the full list each time (adding a role can bring channels back).
  state.channels = pv.channels.filter((c) => c.id in p.channels && can("view_channel", c.id));
  $("app").classList.add("previewing");
  drawPreviewBanner();
  renderChannels();
  renderMembers();
  if (state.channelId && !state.channels.some((c) => c.id === state.channelId)) {
    const first = state.channels.find((c) => c.kind === TEXT);
    if (first) selectChannel(first.id);
    else {
      state.channelId = null;
      $("channel-name").textContent = "—";
      $("messages").replaceChildren(el("div", { class: "empty" }, el("p", {}, "With these roles, there are no channels to see here.")));
      updateComposer();
    }
  } else {
    updateComposer();
    renderMessages();
  }
}

function drawPreviewBanner() {
  const pv = state.preview;
  let bar = document.getElementById("preview-banner");
  if (!bar) {
    bar = el("div", { id: "preview-banner", class: "preview-banner", role: "status" });
    document.querySelector("main.chat")?.prepend(bar);
  }
  const others = state.roles
    .filter((r) => r.id !== state.guildId && !pv.roleIds.includes(r.id))
    .sort((a, b) => b.position - a.position);
  const chips = pv.roleIds.map((id) => {
    const r = state.roles.find((x) => x.id === id);
    return el("span", { class: "pv-chip", style: `--rc:${hexColour(r?.color) || "#a1a1aa"}` },
      el("span", { class: "swatch" }), previewRoleName(id),
      el("button", { type: "button", class: "pv-chip-x", title: `Stop viewing as ${previewRoleName(id)}`, "aria-label": `Remove ${previewRoleName(id)}`,
        onclick: () => setPreviewRoles(pv.roleIds.filter((x) => x !== id)) }, "✕"));
  });
  const add = others.length ? el("button", { type: "button", class: "pv-add", title: "Add a role", "aria-label": "Add a role to the preview", onclick: (e) => {
    const r = e.currentTarget.getBoundingClientRect();
    openMenu(r.left, r.bottom + 4, others.map((o) => ({ label: o.name, swatch: hexColour(o.color) || "#a1a1aa", onClick: () => setPreviewRoles([...pv.roleIds, o.id]) })));
  } }, "＋") : null;
  bar.replaceChildren(
    el("span", { class: "pv-eye", "aria-hidden": "true" }, "👁"),
    el("span", { class: "pv-label" }, "Viewing as"),
    el("span", { class: "pv-chips" }, pv.roleIds.length ? chips : el("span", { class: "pv-chip plain" }, "@everyone"), add),
    el("button", { type: "button", class: "pv-exit", onclick: () => exitRolePreview() }, "Exit preview"),
  );
}

// Keep the preview through refreshes; leave it when you switch outhouse.
const pvBaseRefresh = refreshGuild;
refreshGuild = async function (...args) {
  const r = await pvBaseRefresh(...args);
  const pv = state.preview;
  if (pv && pv.guildId === state.guildId) {
    pv.channels = state.channels; // fresh from the server, before hiding any
    const perms = await fetchPreviewPerms(pv.guildId, pv.roleIds).catch(() => null);
    if (perms && state.preview === pv) { pv.perms = perms; applyPreview(); }
  }
  return r;
};

const pvBaseSelectGuild = selectGuild;
selectGuild = function (id, ...rest) {
  if (state.preview && state.preview.guildId !== id) exitRolePreview({ refresh: false });
  return pvBaseSelectGuild(id, ...rest);
};

// Messages show only where the preview could read them (and you really can).
const pvBaseRenderMessages = renderMessages;
renderMessages = function (...args) {
  const pv = state.preview;
  if (pv && pv.guildId === state.guildId && state.channelId && !can("read_history", state.channelId)) {
    $("messages").replaceChildren(el("div", { class: "empty pv-empty" },
      el("p", {}, "With these roles, the message history here is hidden."),
      el("p", { class: "muted small" }, "New messages would still show up while they're online.")));
    return;
  }
  return pvBaseRenderMessages(...args);
};

const pvBaseHandle = handleEvent;
handleEvent = function (ev) {
  pvBaseHandle(ev);
  if (ev.t === "GUILD_DELETE" && state.preview?.guildId === ev.d.guild_id) exitRolePreview({ refresh: false });
};

// The composer shows what the role could do, but nothing is sent while previewing.
const pvBaseUpdateComposer = updateComposer;
updateComposer = function (...args) {
  const r = pvBaseUpdateComposer(...args);
  const pv = state.preview;
  if (pv && pv.guildId === state.guildId) {
    const ch = state.channels.find((c) => c.id === state.channelId);
    const who = pv.roleIds.length ? pv.roleIds.map(previewRoleName).join(" + ") : "@everyone";
    const canSend = ch && can("send_messages", ch.id);
    input.disabled = true;
    input.placeholder = !ch ? "—" : canSend ? `Preview: ${who} could post here` : `Preview: ${who} can't post in #${ch.name}`;
    $("composer-wrap").classList.toggle("pv-readonly", !canSend);
    $("composer-wrap").classList.add("pv-off");
    $("composer-wrap").querySelectorAll(".composer button").forEach((b) => b.setAttribute("aria-disabled", "true"));
  } else {
    $("composer-wrap").classList.remove("pv-readonly", "pv-off");
    $("composer-wrap").querySelectorAll(".composer button[aria-disabled]").forEach((b) => b.removeAttribute("aria-disabled"));
  }
  return r;
};

// Belt and braces: nothing is sent while previewing.
$("composer").addEventListener("submit", (e) => {
  if (state.preview) { e.preventDefault(); e.stopImmediatePropagation(); }
}, true);
