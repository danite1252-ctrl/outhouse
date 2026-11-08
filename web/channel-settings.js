// Edit channel (Overview / Permissions / Delete), channel topics in the
// header, slowmode, spoiler channels and age-restricted (18+) channels.
// Loaded after app.js and friends; wraps their globals like the other extras.

const TOPIC_MAX = 1024;
const DEFAULT_TOPIC_HINT = "Leave it cleaner than you found it.";
const SLOWMODES = [0, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600];
const CONTENT_MODES = [
  { key: "default", icon: "💬", name: "Default", desc: "Messages show as normal." },
  { key: "spoiler", icon: "🙈", name: "Spoiler channel", desc: "Every message is blurred until someone clicks it. Handy for raid strats, new content and film talk." },
  { key: "nsfw", icon: "🔞", name: "Age-restricted (18+)", desc: "Members have to confirm they're 18 or older before they see anything in here, on every device." },
];

function slowmodeLabel(secs, long = false) {
  if (!secs) return "Off";
  if (secs < 60) return long ? `${secs} seconds` : `${secs}s`;
  if (secs < 3600) { const m = secs / 60; return long ? `${m} minute${m === 1 ? "" : "s"}` : `${m}m`; }
  const h = secs / 3600;
  return long ? `${h} hour${h === 1 ? "" : "s"}` : `${h}h`;
}

const chById = (id) => state.channels.find((c) => c.id === id);
const isGuildText = (c) => c && c.kind === TEXT && c.guild_id && c.guild_id !== "0";
// Age-restricted and not accepted yet: nothing from it is shown (the server won't send it anyway).
const isGated = (c) => isGuildText(c) && c.content_mode === "nsfw" && !c.nsfw_accepted;
const canEditChannel = (c) => c && c.guild_id && c.guild_id !== "0" && (c.kind === TEXT || c.kind === VOICE) &&
  (can("manage_channels", c.id) || can("manage_roles"));
const skipsSlowmode = (c) => can("manage_messages", c.id) || can("manage_channels", c.id);

// ======================================================================
// Header: the channel's own topic
// ======================================================================

function renderChannelTopic() {
  const span = document.querySelector(".chat-head .topic");
  if (!span) return;
  const ch = state.home ? null : chById(state.channelId);
  const topic = ch && isGuildText(ch) ? (ch.topic || "") : "";
  span.replaceChildren(...(topic ? renderText(topic.replace(/\s*\n+\s*/g, " ")) : []));
  span.title = topic ? plainMentions(topic) : "";
  span.classList.toggle("has-topic", !!topic);
  document.querySelector(".chat-head .divider")?.toggleAttribute("hidden", !topic);
  const name = $("channel-name");
  name.classList.toggle("has-topic", !!topic);
  name.title = topic ? "Show the channel topic" : "";
  name.querySelector(".nsfw-badge")?.remove();
  if (ch && isGuildText(ch) && ch.content_mode === "nsfw") name.append(el("span", { class: "nsfw-badge", title: "Age-restricted channel" }, "🔞"));
}

function openTopic() {
  const ch = chById(state.channelId);
  if (!ch || state.home || !ch.topic) return;
  const dialog = el("dialog", { class: "topic-dialog" },
    el("h3", {}, `#${ch.name}`),
    el("p", { class: "topic-full" }, renderText(ch.topic)),
    el("div", { class: "topic-actions" },
      can("manage_channels", ch.id) ? el("button", { class: "ghost", onclick: () => { dialog.close(); openEditChannel(ch); } }, "Edit channel") : el("span"),
      el("button", { class: "primary", onclick: () => dialog.close() }, "Done")));
  dialog.addEventListener("close", () => dialog.remove());
  dialog.addEventListener("click", (e) => { if (e.target === dialog) dialog.close(); });
  document.body.append(dialog);
  dialog.showModal();
}

(function topicClicks() {
  const span = document.querySelector(".chat-head .topic");
  if (span) {
    span.id = "channel-topic";
    span.replaceChildren(); // no more hard-coded topic: each channel has its own
    span.addEventListener("click", (e) => { if (!e.target.closest("a, .mention")) openTopic(); });
  }
  $("channel-name").addEventListener("click", () => openTopic());
})();

// ======================================================================
// Age-restricted channels: the gate
// ======================================================================

function renderAgeGate(ch) {
  const nope = () => {
    const other = state.channels.find((c) => c.kind === TEXT && c.id !== ch.id && !isGated(c));
    if (other) selectChannel(other.id);
    else toast("No worries. Pick another channel when you're ready.");
  };
  const go = async (btn) => {
    btn.disabled = true;
    const ok = await tryApi("PUT", `/api/channels/${ch.id}/age-gate`).then(() => true).catch(() => false);
    btn.disabled = false;
    if (!ok) return;
    ch.nsfw_accepted = true;
    renderChannels();
    if (state.channelId === ch.id) selectChannel(ch.id);
  };
  $("messages").replaceChildren(el("div", { class: "age-gate", role: "dialog", "aria-labelledby": "age-gate-title" },
    el("div", { class: "ag-icon", "aria-hidden": "true" }, "🔞"),
    el("h2", { id: "age-gate-title" }, "Age-restricted channel"),
    el("p", { class: "ag-text" }, "This channel is age-restricted. You must be 18+ to view it."),
    el("p", { class: "ag-sub muted" }, `#${ch.name} may contain content that isn't suitable for everyone. Your answer is remembered on all your devices; you can change it from the channel's menu.`),
    el("div", { class: "ag-actions" },
      el("button", { class: "ghost ag-nope", onclick: nope }, "Nope"),
      el("button", { class: "primary ag-continue", onclick: (e) => go(e.currentTarget) }, "Continue"))));
}

async function resetAgeGate(ch) {
  const ok = await tryApi("DELETE", `/api/channels/${ch.id}/age-gate`).then(() => true).catch(() => false);
  if (!ok) return;
  ch.nsfw_accepted = false;
  toast(`We'll ask again before showing #${ch.name}.`);
  renderChannels();
  if (state.channelId === ch.id) selectChannel(ch.id);
}

// ======================================================================
// Wrapping the core: channel switching, composer, messages, channel list
// ======================================================================

const csBaseSelectChannel = selectChannel;
selectChannel = async function (id) {
  const ch = chById(id);
  if (ch && isGated(ch)) {
    if (state.channelId !== id) {
      state.pendingFiles = [];
      renderPendingFiles();
      if (state.replyTo) cancelReply();
      state.editingId = null;
    }
    state.channelId = id;
    localStorage.setItem(`outhouse_channel_${state.guildId}`, id);
    onChannelChanged();
    $("channel-name").textContent = ch.name;
    state.messages = [];
    state.historyStart = null;
    updateComposer();
    renderChannels();
    renderTyping();
    renderMessages();
    renderChannelTopic();
    return;
  }
  const result = await csBaseSelectChannel(id);
  if (state.channelId === id) {
    renderChannelTopic();
    slowmodeFromHistory();
  }
  return result;
};

const csBaseUpdateComposer = updateComposer;
updateComposer = function (...args) {
  const out = csBaseUpdateComposer(...args);
  const ch = state.home ? null : chById(state.channelId);
  const gated = !!(ch && isGated(ch));
  $("composer-wrap").classList.toggle("age-gated", gated);
  $("app").classList.toggle("age-gated", gated);
  if (gated) {
    input.disabled = true;
    input.placeholder = "Age-restricted channel";
  }
  renderSlowmodeNote();
  return out;
};

const csBaseRenderMessages = renderMessages;
renderMessages = function (...args) {
  const ch = state.home ? null : chById(state.channelId);
  if (ch && isGated(ch)) {
    renderMessages.forceBottom = false;
    renderAgeGate(ch);
    return;
  }
  const out = csBaseRenderMessages(...args);
  if (ch && isGuildText(ch) && ch.content_mode === "spoiler") applySpoilers(ch);
  return out;
};
renderMessages.forceBottom = false;

const csBaseRenderChannels = renderChannels;
renderChannels = function (...args) {
  const out = csBaseRenderChannels(...args);
  for (const node of document.querySelectorAll("#text-channels .channel[data-id], #voice-channels .channel[data-id]")) {
    const ch = chById(node.dataset.id);
    if (!ch) continue;
    if (ch.kind === TEXT && ch.content_mode === "nsfw" && !node.querySelector(".nsfw-badge")) {
      const label = node.querySelector(".cname");
      const badge = el("span", { class: "nsfw-badge", title: "Age-restricted channel" }, "🔞");
      if (label) label.after(badge); else node.append(badge);
    }
    if (ch.kind === TEXT && ch.slowmode_secs) node.title = `Slowmode: ${slowmodeLabel(ch.slowmode_secs, true)}`;
    if (can("manage_channels", ch.id) && !node.querySelector(".ch-gear")) {
      const gear = el("span", {
        class: "ch-gear", role: "button", tabindex: "0", title: "Edit channel", "aria-label": `Edit ${ch.name}`,
        onclick: (e) => { e.stopPropagation(); e.preventDefault(); openEditChannel(ch); },
        onkeydown: (e) => { if (e.key === "Enter" || e.key === " ") { e.stopPropagation(); e.preventDefault(); openEditChannel(ch); } },
      }, "⚙");
      node.append(gear);
    }
  }
  return out;
};

// Right-click menu: "Edit channel", and "Ask me again" for 18+ channels you've accepted.
let csMenuChannel = null;
const csBaseOpenMenu = openMenu;
openMenu = function (x, y, items, ...rest) {
  const ch = csMenuChannel;
  csMenuChannel = null;
  if (ch && Array.isArray(items)) {
    items = items.slice();
    const at = items.findIndex((i) => i && typeof i === "object" && (i.label === "Edit permissions" || i.label === "Delete channel"));
    const extra = [];
    if (can("manage_channels", ch.id)) extra.push({ label: "Edit channel", onClick: () => openEditChannel(ch) });
    if (ch.kind === TEXT && ch.content_mode === "nsfw" && ch.nsfw_accepted) extra.push({ label: "Ask me again (18+)", onClick: () => resetAgeGate(ch) });
    if (at >= 0) items.splice(at, 0, ...extra);
    else items.push(...extra);
  }
  return csBaseOpenMenu(x, y, items, ...rest);
};
const csBaseChannelMenu = MENUS.channel;
MENUS.channel = function (e, node) {
  csMenuChannel = chById(node.dataset.id) || null;
  try {
    return csBaseChannelMenu.call(this, e, node);
  } finally {
    csMenuChannel = null;
  }
};

// "Edit permissions" now opens the Permissions tab of Edit channel.
const csBaseChannelPermissions = openChannelPermissions;
openChannelPermissions = function (channel) {
  return openEditChannel(channel, "permissions");
};

// Live updates.
const csBaseHandle = handleEvent;
handleEvent = function (ev) {
  csBaseHandle(ev);
  const { t, d } = ev;
  if (t === "CHANNEL_UPDATE") {
    const ch = chById(d.id);
    if (!ch) return;
    const wasGated = isGated(ch);
    const accepted = ch.nsfw_accepted;
    Object.assign(ch, d, { nsfw_accepted: accepted, last_read_id: ch.last_read_id, last_message_id: ch.last_message_id });
    renderChannels();
    if (state.channelId === ch.id && !state.home) {
      $("channel-name").textContent = ch.name;
      renderChannelTopic();
      if (isGated(ch) !== wasGated) selectChannel(ch.id);
      else { updateComposer(); renderMessages(); }
    }
  } else if (t === "CHANNEL_AGE_GATE") {
    const ch = chById(d.channel_id);
    if (!ch || ch.nsfw_accepted === d.accepted) return;
    ch.nsfw_accepted = d.accepted;
    renderChannels();
    if (state.channelId === ch.id && !state.home) selectChannel(ch.id);
  }
};

// ======================================================================
// Spoiler channels
// ======================================================================

const spoilerRevealed = new Set();
const spoilerShowAll = new Set(JSON.parse(sessionStorage.getItem("outhouse_spoiler_all") || "[]"));

function spoilerShowAllFor(ch, on) {
  if (on) spoilerShowAll.add(ch.id); else spoilerShowAll.delete(ch.id);
  sessionStorage.setItem("outhouse_spoiler_all", JSON.stringify([...spoilerShowAll]));
}

function spoilerize(article, m, ch, redraw) {
  if (!m || m.author_id === state.me.id || spoilerRevealed.has(m.id) || spoilerShowAll.has(ch.id)) return;
  if (!m.content && !m.attachments?.length && !m.embeds?.length) return;
  article.classList.add("spoilered");
  const body = article.querySelector(".msg-body");
  if (!body || body.querySelector(".spoiler-cover")) return;
  body.append(el("button", { type: "button", class: "spoiler-cover", "aria-label": "Show spoiler" }, el("span", {}, "SPOILER")));
  article.addEventListener("click", (e) => {
    if (!article.classList.contains("spoilered")) return;
    if (e.target.closest(".msg-meta, .msg-actions")) return;
    e.preventDefault();
    e.stopPropagation();
    spoilerRevealed.add(m.id);
    article.classList.remove("spoilered");
    article.querySelector(".spoiler-cover")?.remove();
    if (redraw) redraw();
  }, true);
}

function spoilerBar(ch, redraw) {
  const all = spoilerShowAll.has(ch.id);
  return el("div", { class: "spoiler-bar" },
    el("span", {}, "🙈 ", el("strong", {}, "Spoiler channel."), all ? " Showing everything for now." : " Messages stay hidden until you click them."),
    el("button", { class: "ghost", onclick: () => {
      spoilerShowAllFor(ch, !all);
      if (all) spoilerRevealed.clear();
      redraw();
    } }, all ? "Hide again" : "Show all"));
}

function applySpoilers(ch) {
  const box = $("messages");
  for (const article of box.querySelectorAll("article.msg[data-id]")) {
    spoilerize(article, findMessage(article.dataset.id), ch, null);
  }
  if (state.messages.length) box.prepend(spoilerBar(ch, () => renderMessages()));
}

if (typeof renderThread === "function") {
  const csBaseRenderThread = renderThread;
  renderThread = function (...args) {
    const out = csBaseRenderThread(...args);
    const info = typeof thread !== "undefined" ? thread.info : null;
    const ch = info && chById(info.channel_id);
    const body = sidePanel.kind === "thread" ? sidePanel.body : null;
    if (ch && body && isGuildText(ch) && ch.content_mode === "spoiler") {
      for (const article of body.querySelectorAll("article.tmsg[data-id]")) {
        spoilerize(article, thread.messages.find((m) => m.id === article.dataset.id), ch, null);
      }
    }
    return out;
  };
}

// ======================================================================
// Slowmode: countdown under the composer
// ======================================================================

const slowUntil = new Map(); // channel ID → time (ms) you can post again
let slowTimer = null;

function slowmodeFromHistory() {
  const ch = chById(state.channelId);
  if (!ch || !ch.slowmode_secs) return renderSlowmodeNote();
  const mine = [...state.messages].reverse().find((m) => m.author_id === state.me.id);
  if (mine) {
    const until = new Date(mine.created_at).getTime() + ch.slowmode_secs * 1000;
    if (until > Date.now() && until > (slowUntil.get(ch.id) || 0)) slowUntil.set(ch.id, until);
  }
  renderSlowmodeNote();
}

function renderSlowmodeNote() {
  let note = $("slowmode-note");
  const ch = state.home ? null : chById(state.channelId);
  const on = ch && isGuildText(ch) && ch.slowmode_secs > 0 && !isGated(ch);
  if (!on) {
    note?.remove();
    clearInterval(slowTimer);
    slowTimer = null;
    return;
  }
  if (!note) {
    note = el("div", { id: "slowmode-note", class: "slowmode-note", "aria-live": "polite" });
    $("composer-wrap").append(note);
  }
  const left = Math.ceil(((slowUntil.get(ch.id) || 0) - Date.now()) / 1000);
  const exempt = skipsSlowmode(ch);
  note.classList.toggle("cooling", left > 0 && !exempt);
  if (exempt) note.textContent = `🐢 Slowmode ${slowmodeLabel(ch.slowmode_secs)} · you're exempt`;
  else if (left > 0) note.textContent = `🐢 Slowmode: ${left >= 60 ? `${Math.floor(left / 60)}:${String(left % 60).padStart(2, "0")}` : `${left}s`} left`;
  else note.textContent = `🐢 Slowmode is on (${slowmodeLabel(ch.slowmode_secs, true)})`;
  note.title = `Everyone can post once every ${slowmodeLabel(ch.slowmode_secs, true)}.`;
  if (left > 0 && !exempt && !slowTimer) {
    slowTimer = setInterval(renderSlowmodeNote, 1000);
  } else if ((left <= 0 || exempt) && slowTimer) {
    clearInterval(slowTimer);
    slowTimer = null;
  }
}

const csBaseSendMessage = sendMessage;
sendMessage = async function (content, ...rest) {
  const channelId = state.channelId;
  const ch = chById(channelId);
  const slow = ch && isGuildText(ch) && ch.slowmode_secs > 0 && !skipsSlowmode(ch);
  if (slow) {
    const left = Math.ceil(((slowUntil.get(channelId) || 0) - Date.now()) / 1000);
    if (left > 0) throw new Error(`Slowmode is on. You can post again in ${slowmodeLabel(left, true)}.`);
  }
  try {
    const out = await csBaseSendMessage(content, ...rest);
    if (slow) { slowUntil.set(channelId, Date.now() + ch.slowmode_secs * 1000); renderSlowmodeNote(); }
    return out;
  } catch (err) {
    const m = /post again in (\d+)s/.exec(err?.message || "");
    if (m) { slowUntil.set(channelId, Date.now() + Number(m[1]) * 1000); renderSlowmodeNote(); }
    throw err;
  }
};

// ======================================================================
// Edit channel page
// ======================================================================

function openEditChannel(channel, tab = "overview") {
  const ch = chById(channel.id) || channel;
  if (!canEditChannel(ch)) return toast("You can't edit this channel.");
  const shell = settingsShell(`Edit #${ch.name}`);
  const { dialog, nav, main } = shell;
  dialog.classList.add("edit-channel");
  let dirty = null; // the Overview page's "unsaved changes" check
  const tabs = [
    can("manage_channels", ch.id) ? ["overview", "Overview"] : null,
    can("manage_roles") ? ["permissions", "Permissions"] : null,
  ].filter(Boolean);
  const draw = async (active) => {
    if (dirty && dirty() && !(await confirmBox("Unsaved changes", "You've changed this channel's settings. Throw the changes away?", "Discard"))) return;
    dirty = null;
    const current = chById(ch.id) || ch;
    nav.replaceChildren(
      el("h3", {}, `${current.kind === VOICE ? "🔈 " : "#"}${current.name.toUpperCase()}`),
      el("h3", { class: "ec-kind" }, current.kind === VOICE ? "VOICE CHANNEL" : "TEXT CHANNEL"),
      ...tabs.map(([key, label]) => el("button", { class: key === active ? "active" : "", "data-tab": key, onclick: () => draw(key) }, label)),
      can("manage_channels", ch.id) ? el("div", { class: "ec-sep" }) : null,
      can("manage_channels", ch.id) ? el("button", { class: `ec-delete-tab${active === "delete" ? " active" : ""}`, "data-tab": "delete", onclick: () => draw("delete") }, "Delete", el("span", { class: "ec-wide" }, " channel")) : null);
    if (active === "overview") dirty = drawChannelOverview(main, current, dialog);
    else if (active === "permissions") drawChannelPermissions(main, current, dialog);
    else drawChannelDelete(main, current, dialog);
  };
  dialog.addEventListener("cancel", async (e) => {
    if (dirty && dirty()) {
      e.preventDefault();
      if (await confirmBox("Unsaved changes", "You've changed this channel's settings. Throw the changes away?", "Discard")) { dirty = null; dialog.close(); }
    }
  });
  const start = tabs.some(([k]) => k === tab) ? tab : tab === "delete" && can("manage_channels", ch.id) ? "delete" : tabs[0][0];
  draw(start);
  return shell;
}

function drawChannelOverview(main, ch, dialog) {
  const text = ch.kind === TEXT;
  const name = el("input", { type: "text", maxlength: 50, "aria-label": "Channel name", autocomplete: "off" });
  name.value = ch.name;
  const nameHint = el("div", { class: "ec-hint" });
  const showNameHint = () => {
    if (!text) return;
    const norm = name.value.trim().toLowerCase().split(/\s+/).join("-").replace(/[^\p{L}\p{N}_-]/gu, "").slice(0, 50);
    nameHint.textContent = norm && norm !== name.value.trim() ? `Will be saved as #${norm}` : "Lower case, dashes for spaces.";
  };
  showNameHint();

  const topic = el("textarea", { rows: 4, maxlength: TOPIC_MAX, placeholder: DEFAULT_TOPIC_HINT, "aria-label": "Channel topic" });
  // Mentions show as @names while editing (stored as <@id> so renames don't break them).
  const decodeTopic = (t) => (t || "").replace(/<@(&?)(\d+)>/g, (_, amp, id) => mentionLabel(amp ? "role" : "user", id));
  const encodeTopic = (t) => encodeMentions(t.trim());
  topic.value = decodeTopic(ch.topic);
  const count = el("span", { class: "ec-count" });
  const preview = el("div", { class: "ec-topic-preview" });
  const showTopic = () => {
    const n = [...encodeTopic(topic.value)].length;
    count.textContent = `${n} / ${TOPIC_MAX}`;
    count.classList.toggle("near", n > TOPIC_MAX - 50);
    const v = encodeTopic(topic.value);
    preview.hidden = !v;
    preview.replaceChildren(el("span", { class: "ec-preview-label" }, "Preview"), el("div", { class: "ec-preview-text" }, renderText(v)));
  };
  showTopic();

  let slowIdx = Math.max(0, SLOWMODES.indexOf(ch.slowmode_secs));
  if (SLOWMODES.indexOf(ch.slowmode_secs) < 0 && ch.slowmode_secs) slowIdx = SLOWMODES.findIndex((s) => s >= ch.slowmode_secs);
  const slider = el("input", { type: "range", min: 0, max: SLOWMODES.length - 1, step: 1, value: slowIdx, "aria-label": "Slowmode" });
  const slowValue = el("strong", { class: "ec-slow-value" });
  const ticks = el("div", { class: "ec-ticks", "aria-hidden": "true" }, SLOWMODES.map((s, i) =>
    el("span", { style: `left:${(i / (SLOWMODES.length - 1)) * 100}%` }, i % 2 === 0 || i === SLOWMODES.length - 1 ? slowmodeLabel(s) : "")));
  const showSlow = () => {
    slowValue.textContent = slowmodeLabel(SLOWMODES[slider.value], true);
    slider.style.setProperty("--fill", `${(slider.value / (SLOWMODES.length - 1)) * 100}%`);
  };
  showSlow();

  let mode = ch.content_mode || "default";
  const modeCards = CONTENT_MODES.map((m) => {
    const radio = el("input", { type: "radio", name: `content-${ch.id}`, value: m.key });
    radio.checked = m.key === mode;
    return el("label", { class: `ec-mode ec-mode-${m.key}` }, radio,
      el("span", { class: "ec-mode-icon", "aria-hidden": "true" }, m.icon),
      el("span", { class: "ec-mode-text" }, el("span", { class: "ec-mode-name" }, m.name), el("span", { class: "ec-mode-desc" }, m.desc)));
  });
  const modeBox = el("div", { class: "ec-modes", role: "radiogroup", "aria-label": "Content visibility" }, modeCards);

  const values = () => ({
    name: name.value.trim(),
    topic: encodeTopic(topic.value),
    slowmode_secs: SLOWMODES[slider.value],
    content_mode: mode,
  });
  const original = () => ({ name: ch.name, topic: ch.topic || "", slowmode_secs: ch.slowmode_secs || 0, content_mode: ch.content_mode || "default" });
  const changed = () => {
    const now = values();
    const was = original();
    return text ? Object.keys(now).some((k) => now[k] !== was[k]) : now.name !== was.name;
  };
  const save = el("button", { class: "primary", onclick: async () => {
    const v = values();
    if (!v.name) return toast("Give the channel a name.");
    if ([...v.topic].length > TOPIC_MAX) return toast(`Topics can be up to ${TOPIC_MAX} characters.`);
    save.disabled = true;
    const body = text ? v : { name: v.name };
    const updated = await tryApi("PATCH", `/api/channels/${ch.id}`, body).catch(() => null);
    save.disabled = false;
    if (!updated) return;
    const local = chById(ch.id);
    const lastRead = local ? local.last_read_id : ch.last_read_id;
    if (local && local !== ch) Object.assign(local, updated, { last_read_id: lastRead });
    Object.assign(ch, updated, { last_read_id: lastRead });
    name.value = updated.name;
    topic.value = decodeTopic(updated.topic);
    showNameHint(); showTopic();
    toast("Channel saved.");
    renderChannels();
    if (state.channelId === ch.id && !state.home) selectChannel(ch.id);
    paintBar();
  } }, "Save changes");
  const reset = el("button", { class: "link", onclick: () => {
    name.value = ch.name;
    topic.value = decodeTopic(ch.topic);
    slider.value = Math.max(0, SLOWMODES.indexOf(ch.slowmode_secs || 0));
    mode = ch.content_mode || "default";
    for (const r of modeBox.querySelectorAll("input")) r.checked = r.value === mode;
    showNameHint(); showTopic(); showSlow(); paintBar();
  } }, "Reset");
  const bar = el("div", { class: "ec-savebar" }, el("span", { class: "ec-savebar-text" }, "Careful, you've got unsaved changes!"), el("div", { class: "ec-savebar-btns" }, reset, save));
  const paintBar = () => { bar.classList.toggle("show", changed()); };
  name.addEventListener("input", () => { showNameHint(); paintBar(); });
  topic.addEventListener("input", () => { showTopic(); paintBar(); });
  slider.addEventListener("input", () => { showSlow(); paintBar(); });
  modeBox.addEventListener("change", (e) => { if (e.target.name === `content-${ch.id}`) { mode = e.target.value; paintBar(); } });

  main.replaceChildren(el("div", { class: "ec-page" },
    el("h2", {}, "Overview"),
    el("label", { class: "field" }, "Channel name", name, text ? nameHint : null),
    text ? el("div", { class: "field ec-topic" },
      el("div", { class: "ec-row" }, el("label", { for: `topic-${ch.id}` }, "Channel topic"), count),
      topic,
      el("div", { class: "ec-hint" }, "Shown at the top of the channel. Links, mentions, `code` and emoji work like in messages."),
      preview) : null,
    text ? el("div", { class: "ec-divider" }) : null,
    text ? el("div", { class: "field ec-slow" },
      el("div", { class: "ec-row" }, el("span", {}, "Slowmode"), slowValue),
      el("div", { class: "ec-slider" }, slider, ticks),
      el("div", { class: "ec-hint" }, "Members can only send one message per this interval. People who can manage messages or channels skip it.")) : null,
    text ? el("div", { class: "ec-divider" }) : null,
    text ? el("div", { class: "field" }, el("span", {}, "Content visibility"), modeBox) : null,
    !text ? el("p", { class: "muted", style: "margin:0" }, "Voice channels just have a name. Use the Permissions tab to decide who can join and speak.") : null),
    bar);
  topic.id = `topic-${ch.id}`;
  paintBar();
  return changed;
}

function drawChannelPermissions(main, ch, dialog) {
  const targets = el("nav", { class: "settings-nav ec-perm-targets", "aria-label": "Roles and members" });
  const page = el("div", { class: "ec-perm-page" });
  main.replaceChildren(el("div", { class: "ec-perms" }, targets, page));
  // The existing channel permission editor, drawn inside this tab.
  const shellOverride = { dialog, nav: targets, main: page, title: `#${ch.name}` };
  const realShell = settingsShell;
  settingsShell = () => shellOverride;
  try {
    csBaseChannelPermissions(ch);
  } finally {
    settingsShell = realShell;
  }
}

function drawChannelDelete(main, ch, dialog) {
  const isVoice = ch.kind === VOICE;
  main.replaceChildren(el("div", { class: "ec-page" },
    el("h2", {}, "Delete channel"),
    el("div", { class: "ec-danger" },
      el("div", { class: "ec-danger-icon", "aria-hidden": "true" }, "🚽"),
      el("div", {},
        el("p", { class: "ec-danger-title" }, `Flush ${isVoice ? "🔈 " : "#"}${ch.name}?`),
        el("p", { class: "muted" }, isVoice
          ? "The voice channel goes away and anyone in it is disconnected. This can't be undone."
          : "Every message, pin, thread and file in it goes too. This can't be undone."))),
    el("div", {},
      el("button", { class: "danger-btn ec-delete-btn", onclick: async () => {
        if (!(await confirmBox(`Delete ${isVoice ? "" : "#"}${ch.name}?`, "This can't be undone.", "Delete channel"))) return;
        const ok = await tryApi("DELETE", `/api/channels/${ch.id}`).then(() => true).catch(() => false);
        if (ok) { toast(`${isVoice ? ch.name : `#${ch.name}`} has been flushed.`); dialog.close(); }
      } }, "Delete channel"))));
}

// The channel list and header may already be drawn (this file loads last).
if (state.channels?.length) { renderChannels(); renderChannelTopic(); }

// Direct messages have no topic, slowmode or gate.
if (typeof openHome === "function") {
  const csBaseOpenHome = openHome;
  openHome = function (...args) {
    const out = csBaseOpenHome(...args);
    renderChannelTopic();
    renderSlowmodeNote();
    return Promise.resolve(out).finally(() => { renderChannelTopic(); renderSlowmodeNote(); });
  };
}
