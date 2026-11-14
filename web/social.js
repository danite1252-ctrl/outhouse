// Outhouse web client: stickers, forwarding messages and scheduled messages.
// Loaded after the other modules (uses globals from app.js, features.js,
// dms.js, threads.js and voicemsg.js).
"use strict";

// ======================================================================
// Stickers
// ======================================================================

state.stickers = []; // built-in ones, then every outhouse sticker you can use

async function loadStickers() {
  const list = await api("GET", "/api/users/@me/stickers").catch(() => null);
  if (list) state.stickers = list;
}

function stickerImage(s, cls = "msg-sticker") {
  const img = el("img", { class: cls, src: s.url, alt: s.name, title: s.name, loading: "lazy", draggable: "false" });
  img.addEventListener("error", () => img.replaceWith(el("div", { class: `${cls} sticker-gone` }, el("span", {}, "🏷"), el("span", {}, `${s.name}`), el("span", { class: "muted small" }, "Sticker deleted"))));
  return img;
}

async function sendSticker(sticker) {
  const channelId = state.channelId;
  if (!channelId) return;
  const replyId = state.replyTo?.id || null;
  try {
    const msg = await api("POST", `/api/channels/${channelId}/stickers`, { sticker_id: sticker.id, reply_to_id: replyId });
    if (state.replyTo) cancelReply();
    if (channelId === state.channelId && !findMessage(msg.id)) {
      state.messages.push(msg);
      renderMessages.forceBottom = true;
      renderMessages();
    }
  } catch (err) {
    toast(err.message);
  }
}

function stickerPane() {
  const search = el("input", { type: "search", class: "sticker-search", placeholder: "Find a sticker", "aria-label": "Find a sticker" });
  const body = el("div", { class: "sticker-body" });
  const draw = () => {
    const q = search.value.trim().toLowerCase();
    const match = (s) => !q || s.name.toLowerCase().includes(q) || s.emoji.includes(q) || s.description.toLowerCase().includes(q);
    const groups = new Map();
    const here = state.stickers.filter((s) => s.guild_id === state.guildId);
    const rest = state.stickers.filter((s) => s.guild_id !== state.guildId);
    for (const s of [...here, ...rest].filter(match)) {
      if (!groups.has(s.guild_id)) groups.set(s.guild_id, []);
      groups.get(s.guild_id).push(s);
    }
    body.replaceChildren(...(groups.size ? [...groups].map(([guildId, list]) => el("div", { class: "emoji-section" },
      el("div", { class: "emoji-section-title" }, guildId === "0" ? "OUTHOUSE CLASSICS" : (state.guilds.find((g) => g.id === guildId)?.name ?? "Outhouse").toUpperCase()),
      el("div", { class: "sticker-grid" }, list.map((s) => el("button", {
        type: "button", class: "sticker-tile", title: s.description ? `${s.name}: ${s.description}` : s.name, "data-sticker": s.id,
        onclick: () => { closePopover(); sendSticker(s); },
      }, stickerImage(s, "sticker-thumb")))))) : [el("p", { class: "muted small emoji-none" }, q ? "No stickers match." : "No stickers yet.")]));
  };
  search.addEventListener("input", draw);
  draw();
  return el("div", { class: "emoji-pane sticker-pane", hidden: true }, search, body);
}

// The emoji picker's third tab (composer only: stickers are messages).
const stBaseEmojiTabs = customEmojiTabs;
customEmojiTabs = function (standard, onPick, forText) {
  const picker = stBaseEmojiTabs(standard, onPick, forText);
  if (!forText || !state.channelId || !can("send_messages", state.channelId)) return picker;
  const tabs = picker.querySelector(".emoji-tabs");
  if (!tabs) return picker;
  const pane = stickerPane();
  const tab = el("button", { type: "button", class: "sticker-tab", id: "sticker-tab" }, "🏷 Stickers");
  const others = [...tabs.children];
  tab.addEventListener("click", () => {
    picker.querySelectorAll(".emoji-pane").forEach((p) => { p.hidden = p !== pane; });
    others.forEach((b) => b.classList.remove("on"));
    tab.classList.add("on");
    if (!isTouch()) pane.querySelector("input")?.focus({ preventScroll: true });
  });
  others.forEach((b) => b.addEventListener("click", () => { pane.hidden = true; tab.classList.remove("on"); }));
  tabs.append(tab);
  picker.append(pane);
  return picker;
};

// Outhouse settings → Stickers.
async function drawStickerSettings(main) {
  const guildId = state.guildId;
  const list = await api("GET", `/api/guilds/${guildId}/stickers`).catch((err) => { toast(err.message); return []; });
  const redraw = () => { main.replaceChildren(); drawStickerSettings(main); loadStickers(); };
  const name = el("input", { id: "sticker-name", placeholder: "Name, like Big Pull", maxlength: 30, "aria-label": "Sticker name" });
  const emoji = el("input", { id: "sticker-emoji", placeholder: "🔥", maxlength: 16, class: "sticker-emoji-input", "aria-label": "Related emoji" });
  const desc = el("input", { id: "sticker-desc", placeholder: "Description (optional)", maxlength: 100, "aria-label": "Description" });
  const file = el("input", { type: "file", id: "sticker-file", accept: "image/png,image/apng,image/gif,image/webp", hidden: true });
  const preview = el("div", { class: "emoji-preview sticker-preview" }, "?");
  let picked = null;
  file.addEventListener("change", () => {
    picked = file.files[0] || null;
    if (!picked) return;
    if (picked.size > 512 * 1024) { toast("That picture is over 512 KB."); picked = null; return; }
    if (!name.value.trim()) name.value = picked.name.replace(/\.[^.]+$/, "").replace(/[_-]+/g, " ").slice(0, 30);
    preview.replaceChildren(el("img", { src: URL.createObjectURL(picked), alt: "" }));
  });
  const full = list.length >= 60;
  const upload = el("button", { class: "primary", id: "sticker-upload", disabled: full, onclick: async () => {
    if (!picked) return file.click();
    const form = new FormData();
    form.append("name", name.value.trim());
    form.append("emoji", emoji.value.trim());
    form.append("description", desc.value.trim());
    form.append("file", picked, picked.name);
    const res = await tryApi("POST", `/api/guilds/${guildId}/stickers`, form).catch(() => null);
    if (res) { toast(`${res.name} is ready.`); redraw(); }
  } }, "Upload");
  const edit = async (s) => {
    const n = el("input", { value: s.name, maxlength: 30 });
    const em = el("input", { value: s.emoji, maxlength: 16 });
    const d = el("input", { value: s.description, maxlength: 100 });
    const dialog = el("dialog", {}, el("form", { method: "dialog" },
      el("h2", {}, "Edit sticker"),
      el("div", { class: "sticker-edit-preview" }, stickerImage(s, "sticker-thumb")),
      el("label", { class: "field" }, "Name", n),
      el("label", { class: "field" }, "Related emoji", em),
      el("label", { class: "field" }, "Description", d),
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
        el("button", { type: "submit", class: "primary", value: "ok" }, "Save"))));
    dialog.addEventListener("close", async () => {
      const ok = dialog.returnValue === "ok";
      dialog.remove();
      if (!ok) return;
      if (await tryApi("PATCH", `/api/stickers/${s.id}`, { name: n.value.trim(), emoji: em.value.trim(), description: d.value.trim() }).catch(() => null)) redraw();
    });
    document.body.append(dialog);
    dialog.showModal();
  };
  main.append(
    el("h2", {}, "Stickers"),
    el("p", { class: "muted", style: "margin:0" }, "Stickers are big pictures people send on their own: PNG, APNG, GIF or WebP up to 512 KB (320 × 320 looks best). Everyone here can send them, in DMs too, from the 🏷 Stickers tab in the emoji picker."),
    el("div", { class: "emoji-upload sticker-upload" }, preview,
      el("div", { class: "emoji-upload-fields" },
        el("div", { class: "inline-row" }, name, emoji),
        desc,
        el("div", { class: "inline-row" }, el("button", { type: "button", class: "ghost", onclick: () => file.click() }, "Choose picture"), upload)),
      file),
    el("p", { class: "muted small", style: "margin:0" }, full ? "60 of 60 used. Delete one to add another." : `${list.length} of 60 used`),
    list.length ? el("div", { class: "sticker-admin-grid" }, list.map((s) => el("div", { class: "sticker-card", "data-sticker": s.id },
      stickerImage(s, "sticker-thumb"),
      el("div", { class: "sc-name" }, s.emoji ? `${s.emoji} ` : "", s.name),
      s.description ? el("div", { class: "muted small sc-desc" }, s.description) : null,
      el("div", { class: "inline-row" },
        el("button", { class: "ghost", onclick: () => edit(s) }, "Edit"),
        el("button", { class: "ghost danger", onclick: async () => {
          if (!(await confirmBox(`Delete ${s.name}?`, "Messages that used it will say the sticker was deleted.", "Delete"))) return;
          await tryApi("DELETE", `/api/stickers/${s.id}`).catch(() => {});
          redraw();
        } }, "Delete"))))) : el("p", { class: "muted" }, "No stickers yet."),
  );
}

// ======================================================================
// Forwarded messages: rendering
// ======================================================================

function forwardOrigin(f) {
  const where = f.guild_name ? `${f.channel_name} (${f.guild_name})` : f.channel_name;
  return `${where} · ${f.author_display_name}`;
}

async function openForwardSource(f) {
  if (f.guild_id === "0") {
    if (!state.dms.some((d) => d.id === f.channel_id)) return toast("You're not in that conversation.");
    await openHome(f.channel_id);
  } else {
    if (!state.guilds.some((g) => g.id === f.guild_id)) return toast("You're not in that outhouse.");
    if (state.guildId !== f.guild_id) await selectGuild(f.guild_id, f.channel_id);
  }
  jumpToMessage(f.channel_id, f.message_id);
}

const fwBaseRenderAttachments = renderAttachments;
renderAttachments = function (m) {
  const sticker = m.sticker ? el("div", { class: "sticker-wrap" }, stickerImage(m.sticker)) : null;
  if (!m.forward) {
    const rest = fwBaseRenderAttachments(m);
    return sticker ? el("div", {}, sticker, rest) : rest;
  }
  const f = m.forward;
  const embeds = f.embeds?.length ? renderEmbeds({ ...m, content: f.content, embeds: f.embeds }) : null;
  return el("div", { class: "fwd" },
    el("button", { type: "button", class: "fwd-head", title: "Go to the original message", onclick: () => openForwardSource(f) },
      el("span", { class: "fwd-arrow", "aria-hidden": "true" }, "↪"), el("em", {}, "Forwarded from "), el("span", { class: "fwd-from" }, forwardOrigin(f))),
    el("div", { class: "fwd-body" },
      f.content ? el("p", { class: "msg-text" }, renderText(f.content)) : null,
      f.sticker ? el("div", { class: "sticker-wrap" }, stickerImage(f.sticker)) : null,
      fwBaseRenderAttachments(m),
      embeds,
      el("time", { class: "fwd-time", datetime: f.created_at }, new Date(f.created_at).toLocaleString([], { dateStyle: "medium", timeStyle: "short" }))),
    sticker);
};

// ======================================================================
// Forwarding: the dialog
// ======================================================================

const FWD_MAX = 5;

async function forwardTargets() {
  const out = [];
  if (!state.dmsLoaded) loadDms();
  const dms = await state.dmsLoaded.catch(() => state.dms);
  const sendBit = state.P.send_messages ?? 0n;
  const attachBit = state.P.attach_files ?? 0n;
  for (const dm of dms || state.dms) {
    out.push({ id: dm.id, guild: "Direct messages", guildId: "0", label: dmName(dm), icon: dm.kind === 1 ? "@" : "👥", nsfw: false, attach: true, dm });
  }
  const per = await Promise.all(state.guilds.map(async (g) => {
    const [channels, perms] = await Promise.all([
      api("GET", `/api/guilds/${g.id}/channels`).catch(() => []),
      api("GET", `/api/guilds/${g.id}/permissions`).catch(() => null),
    ]);
    if (!perms) return [];
    return channels.filter((c) => c.kind === TEXT).map((c) => {
      const bits = big(perms.channels[c.id]);
      return (bits & sendBit) === sendBit ? { id: c.id, guild: g.name, guildId: g.id, label: `#${c.name}`, icon: "#", nsfw: c.content_mode === "nsfw", attach: (bits & attachBit) === attachBit } : null;
    }).filter(Boolean);
  }));
  for (const list of per) out.push(...list);
  return out;
}

async function openForwardDialog(m) {
  const source = state.channels.find((c) => c.id === m.channel_id);
  const fromGated = !!(source && source.kind === TEXT && source.content_mode === "nsfw");
  const hasFiles = (m.attachments || []).length > 0;
  const picked = new Set();
  const filter = el("input", { type: "search", placeholder: "Search channels and DMs", "aria-label": "Search channels and DMs" });
  const list = el("div", { class: "people-list fwd-list" }, el("p", { class: "muted small" }, "Loading…"));
  const note = el("textarea", { rows: 2, maxlength: 2000, placeholder: "Add a note (optional)", "aria-label": "Note", class: "fwd-note" });
  const count = el("span", { class: "muted small" });
  const ok = el("button", { type: "submit", class: "primary", value: "ok", disabled: true }, "Forward");
  const preview = el("div", { class: "fwd-preview" },
    el("div", { class: "fwd-preview-head" }, avatarFor(m.author_id, authorName(m), 20, m.author_avatar_url || undefined), el("strong", {}, authorName(m))),
    el("div", { class: "fwd-preview-text" },
      m.forward ? `↪ ${plainTimeTags(plainMentions(m.forward.content || ""))}` : plainTimeTags(plainMentions(m.content || "")),
      m.sticker || m.forward?.sticker ? " 🏷 Sticker" : "",
      hasFiles ? ` 📎 ${m.attachments.length} attachment${m.attachments.length === 1 ? "" : "s"}` : ""));
  let targets = [];
  const sync = () => {
    count.textContent = `${picked.size} of ${FWD_MAX}`;
    ok.disabled = !picked.size;
  };
  const draw = () => {
    const q = filter.value.trim().toLowerCase();
    const shown = targets.filter((t) => !q || t.label.toLowerCase().includes(q) || t.guild.toLowerCase().includes(q));
    if (!shown.length) return list.replaceChildren(el("p", { class: "muted small" }, targets.length ? "Nothing by that name." : "Nowhere you can post."));
    let lastGuild = null;
    const rows = [];
    for (const t of shown) {
      if (t.guild !== lastGuild) { rows.push(el("div", { class: "fwd-group" }, t.guild.toUpperCase())); lastGuild = t.guild; }
      const blockedReason = fromGated && !t.nsfw ? "Only to 18+ channels" : hasFiles && !t.attach ? "Can't post files there" : null;
      const box = el("input", { type: "checkbox", checked: picked.has(t.id), disabled: !!blockedReason, "data-target": t.id });
      box.addEventListener("change", () => {
        if (box.checked && picked.size >= FWD_MAX) { box.checked = false; return toast(`You can forward to up to ${FWD_MAX} places at once.`); }
        box.checked ? picked.add(t.id) : picked.delete(t.id);
        sync();
      });
      rows.push(el("label", { class: `person-row fwd-row${blockedReason ? " off" : ""}` }, box,
        t.dm ? dmIcon(t.dm, 24) : el("span", { class: "fwd-hash", "aria-hidden": "true" }, t.icon),
        el("span", { class: "person-name" }, t.label, t.nsfw ? el("span", { class: "nsfw-badge" }, "🔞") : null),
        blockedReason ? el("span", { class: "muted small fwd-why" }, blockedReason) : null));
    }
    list.replaceChildren(...rows);
  };
  filter.addEventListener("input", draw);
  const dialog = el("dialog", { class: "people-dialog fwd-dialog" },
    el("form", { method: "dialog" },
      el("h2", {}, "Forward message"),
      preview,
      fromGated ? el("p", { class: "muted small fwd-gated" }, "🔞 This is from an age-restricted channel, so it can only go to other age-restricted channels.") : null,
      filter, list, note,
      el("div", { class: "row" }, count, el("span", { style: "flex:1" }),
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"), ok)));
  dialog.querySelector("form").addEventListener("submit", async (e) => {
    e.preventDefault();
    ok.disabled = true;
    try {
      const sent = await api("POST", `/api/channels/${m.channel_id}/messages/${m.id}/forward`, { destinations: [...picked], note: note.value.trim() || null });
      dialog.close();
      toast(`Forwarded to ${sent.length} place${sent.length === 1 ? "" : "s"}.`);
      for (const msg of sent) {
        if (msg.channel_id === state.channelId && !findMessage(msg.id)) { state.messages.push(msg); renderMessages.forceBottom = true; renderMessages(); }
      }
    } catch (err) {
      toast(err.message);
      ok.disabled = false;
    }
  });
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  if (!isTouch()) filter.focus({ preventScroll: true });
  sync();
  targets = await forwardTargets();
  if (dialog.isConnected) draw();
}

const canForward = (m) => !!(m.content?.trim() || m.attachments?.length || m.embeds?.length || m.sticker || m.forward);

const fwBaseMessageActions = messageActions;
messageActions = function (m) {
  const bar = fwBaseMessageActions(m);
  if (canForward(m)) {
    const more = bar.lastElementChild;
    more?.before(el("button", { class: "fwd-btn", title: "Forward", "aria-label": "Forward", onclick: () => openForwardDialog(m) }, "↪"));
  }
  return bar;
};

const fwBaseMessageMenu = MENUS.message;
MENUS.message = function (e, node) {
  const m = findMessage(node.dataset.id);
  if (!m || !canForward(m)) return fwBaseMessageMenu(e, node);
  const realOpen = openMenu;
  openMenu = (x, y, items, depth) => {
    openMenu = realOpen;
    const at = items.findIndex((it) => it === "sep");
    items.splice(at >= 0 ? at : items.length, 0, { label: "Forward", onClick: () => openForwardDialog(m) });
    return realOpen(x, y, items, depth);
  };
  try { fwBaseMessageMenu(e, node); } finally { openMenu = realOpen; }
};

// ======================================================================
// Scheduled messages
// ======================================================================

state.scheduled = []; // yours, everywhere (pending and failed)

async function loadScheduled() {
  const list = await api("GET", "/api/users/@me/scheduled").catch(() => null);
  if (!list) return;
  state.scheduled = list;
  renderScheduledBar();
  const failed = list.filter((s) => s.status === "failed");
  if (failed.length && !loadScheduled.told) {
    loadScheduled.told = true;
    toast(`⚠ ${failed.length} scheduled message${failed.length === 1 ? "" : "s"} couldn't be sent. Open the channel to see why.`);
  }
}

// (myTimeZone comes from raids.js, localInputValue from features.js.)

function whenLabel(iso) {
  const d = new Date(iso);
  const now = new Date();
  const tomorrow = new Date(now); tomorrow.setDate(now.getDate() + 1);
  const time = d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return `today at ${time}`;
  if (d.toDateString() === tomorrow.toDateString()) return `tomorrow at ${time}`;
  return `${d.toLocaleDateString([], { weekday: "short", day: "numeric", month: "short", year: d.getFullYear() === now.getFullYear() ? undefined : "numeric" })} at ${time}`;
}

function schedulePresets() {
  const now = new Date();
  const inHour = new Date(now.getTime() + 60 * 60 * 1000);
  inHour.setSeconds(0, 0);
  const tonight = new Date(now); tonight.setHours(19, 0, 0, 0);
  const tonightLabel = tonight > new Date(now.getTime() + 60_000) ? "Tonight at 7 pm" : "Tomorrow at 7 pm";
  if (tonight <= new Date(now.getTime() + 60_000)) tonight.setDate(tonight.getDate() + 1);
  const morning = new Date(now); morning.setDate(now.getDate() + 1); morning.setHours(9, 0, 0, 0);
  return [["hour", "In 1 hour", inHour], ["tonight", tonightLabel, tonight], ["morning", "Tomorrow at 9 am", morning]];
}

// Pick a time. Resolves to a Date or null.
function pickSendTime({ title = "Send later", initial = null, confirm = "Schedule" } = {}) {
  return new Promise((resolve) => {
    const presets = schedulePresets();
    let choice = initial ? "custom" : "hour";
    const custom = el("input", { type: "datetime-local", class: "sched-custom", "aria-label": "Date and time", value: localInputValue(initial ? new Date(initial) : presets[0][2]), min: localInputValue(new Date()) });
    const summary = el("p", { class: "muted small sched-summary" });
    const chosen = () => (choice === "custom" ? (custom.value ? new Date(custom.value) : null) : presets.find((p) => p[0] === choice)[2]);
    const sync = () => {
      buttons.forEach((b) => b.classList.toggle("on", b.dataset.choice === choice));
      const d = chosen();
      summary.textContent = d && !Number.isNaN(d.getTime()) ? `Sends ${whenLabel(d.toISOString())} (${myTimeZone()}).` : "Pick a date and time.";
    };
    const buttons = [...presets.map(([key, label, d]) => el("button", { type: "button", class: "sched-preset", "data-choice": key, onclick: () => { choice = key; sync(); } },
      el("strong", {}, label), el("span", { class: "muted small" }, d.toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" })))),
    el("button", { type: "button", class: "sched-preset", "data-choice": "custom", onclick: () => { choice = "custom"; sync(); custom.focus(); } }, el("strong", {}, "Pick a time"), el("span", { class: "muted small" }, "Any date and time"))];
    custom.addEventListener("input", () => { choice = "custom"; sync(); });
    const dialog = el("dialog", { class: "sched-dialog" }, el("form", { method: "dialog" },
      el("h2", {}, title),
      el("div", { class: "sched-presets" }, buttons),
      el("label", { class: "field" }, "Date and time", custom),
      summary,
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
        el("button", { type: "submit", class: "primary", value: "ok" }, confirm))));
    dialog.addEventListener("close", () => {
      const d = dialog.returnValue === "ok" ? chosen() : null;
      dialog.remove();
      resolve(d && !Number.isNaN(d.getTime()) ? d : null);
    });
    document.body.append(dialog);
    sync();
    dialog.showModal();
  });
}

async function scheduleFromComposer() {
  const channelId = state.channelId;
  const content = input.value.trim();
  if (!channelId) return;
  if (!content) return toast("Write your message first, then pick Send later.");
  if (state.pendingFiles.length) return toast("Scheduled messages are text only. Send the files now or remove them.");
  const at = await pickSendTime();
  if (!at) return;
  try {
    const s = await api("POST", `/api/channels/${channelId}/scheduled`, { content: encodeMentions(content), send_at: at.toISOString() });
    upsertScheduled(s);
    if (input.value.trim() === content) { input.value = ""; autosize(); }
    toast(`Scheduled for ${whenLabel(s.send_at)}.`);
  } catch (err) {
    toast(err.message);
  }
}

function upsertScheduled(s) {
  const i = state.scheduled.findIndex((x) => x.id === s.id);
  if (i >= 0) state.scheduled[i] = s; else state.scheduled.push(s);
  state.scheduled.sort((a, b) => new Date(a.send_at) - new Date(b.send_at));
  renderScheduledBar();
  if ($("sched-list")) drawScheduledList();
}

function removeScheduled(id) {
  state.scheduled = state.scheduled.filter((x) => x.id !== id);
  renderScheduledBar();
  if ($("sched-list")) drawScheduledList();
}

function renderScheduledBar() {
  const wrap = $("composer-wrap");
  if (!wrap) return;
  let bar = $("sched-bar");
  const here = state.scheduled.filter((s) => s.channel_id === state.channelId && !s.thread_id);
  if (!here.length || !state.channelId) { bar?.remove(); return; }
  if (!bar) {
    bar = el("div", { id: "sched-bar", class: "sched-bar", role: "button", tabindex: "0", onclick: openScheduledList,
      onkeydown: (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openScheduledList(); } } });
    $("composer").before(bar);
  }
  const failed = here.filter((s) => s.status === "failed").length;
  const pending = here.length - failed;
  bar.classList.toggle("has-failed", failed > 0);
  bar.replaceChildren(
    el("span", { class: "sb-icon", "aria-hidden": "true" }, failed ? "⚠" : "🕒"),
    el("span", { class: "sb-text" },
      pending ? `You have ${pending} scheduled message${pending === 1 ? "" : "s"} here` : "",
      pending && failed ? " · " : "",
      failed ? el("span", { class: "sb-failed" }, `${failed} couldn't be sent`) : "",
      pending ? el("span", { class: "muted" }, ` · next ${whenLabel(here.find((s) => s.status !== "failed").send_at)}`) : ""),
    el("span", { class: "sb-view" }, "View"));
}

async function editScheduled(s) {
  const text = el("textarea", { rows: 4, maxlength: 4000, class: "sched-edit-text" }, plainMentions(s.content));
  const when = el("input", { type: "datetime-local", value: localInputValue(new Date(s.send_at)), min: localInputValue(new Date()) });
  const dialog = el("dialog", { class: "sched-dialog" }, el("form", { method: "dialog" },
    el("h2", {}, s.status === "failed" ? "Try again" : "Edit scheduled message"),
    s.status === "failed" ? el("p", { class: "sched-error" }, `⚠ ${s.error || "It couldn't be sent."}`) : null,
    el("label", { class: "field" }, "Message", text),
    el("label", { class: "field" }, `Send at (${myTimeZone()})`, when),
    el("div", { class: "row" },
      el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
      el("button", { type: "submit", class: "primary", value: "ok" }, "Save"))));
  dialog.addEventListener("close", async () => {
    const ok = dialog.returnValue === "ok";
    dialog.remove();
    if (!ok) return;
    const at = new Date(when.value);
    const patch = { content: encodeMentions(text.value.trim()) };
    if (!Number.isNaN(at.getTime()) && (s.status === "failed" || at.toISOString() !== new Date(s.send_at).toISOString())) patch.send_at = at.toISOString();
    const updated = await tryApi("PATCH", `/api/scheduled/${s.id}`, patch).catch(() => null);
    if (updated) upsertScheduled(updated);
  });
  document.body.append(dialog);
  dialog.showModal();
}

function drawScheduledList() {
  const box = $("sched-list");
  if (!box) return;
  const channelId = box.dataset.channel;
  const here = state.scheduled.filter((s) => s.channel_id === channelId);
  box.replaceChildren(...(here.length ? here.map((s) => el("div", { class: `sched-item${s.status === "failed" ? " failed" : ""}`, "data-id": s.id },
    el("div", { class: "si-when" }, s.status === "failed" ? "⚠ Couldn't send" : `🕒 ${whenLabel(s.send_at)}`, s.thread_id ? el("span", { class: "muted small" }, " · in a thread") : null),
    s.status === "failed" ? el("div", { class: "sched-error small" }, s.error || "") : null,
    el("div", { class: "si-text" }, renderText(s.content)),
    el("div", { class: "inline-row si-actions" },
      el("button", { type: "button", class: "ghost", onclick: () => editScheduled(s) }, s.status === "failed" ? "Try again" : "Edit"),
      el("button", { type: "button", class: "ghost", onclick: async () => {
        const msg = await tryApi("POST", `/api/scheduled/${s.id}/send`).catch(() => null);
        if (!msg) return;
        removeScheduled(s.id);
        toast("Sent.");
      } }, "Send now"),
      el("button", { type: "button", class: "ghost danger", onclick: async () => {
        const ok = await tryApi("DELETE", `/api/scheduled/${s.id}`).then(() => true, () => false);
        if (ok) removeScheduled(s.id);
      } }, s.status === "failed" ? "Delete" : "Cancel")))) : [el("p", { class: "muted" }, "Nothing scheduled here.")]));
}

function openScheduledList() {
  const channelId = state.channelId;
  if (!channelId) return;
  const ch = state.channels.find((c) => c.id === channelId);
  const dialog = el("dialog", { class: "sched-dialog sched-list-dialog" }, el("form", { method: "dialog" },
    el("h2", {}, `Scheduled in ${ch ? (ch.kind === TEXT ? `#${ch.name}` : ch.name) : "this channel"}`),
    el("p", { class: "muted small", style: "margin:0" }, `Times are in ${myTimeZone()}. They're sent with your permissions at that moment.`),
    el("div", { id: "sched-list", class: "sched-list", "data-channel": channelId }),
    el("div", { class: "row" }, el("button", { type: "submit", class: "primary", value: "ok" }, "Done"))));
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  drawScheduledList();
  dialog.showModal();
}

// "Send later": the ▾ beside Send (or press and hold Send on a phone).
(function sendLaterButton() {
  const form = $("composer");
  const send = form?.querySelector(".send");
  if (!form || !send) return;
  const menu = (x, y) => openMenu(x, y, [
    { label: "Send now", onClick: () => form.requestSubmit() },
    { label: "Send later…", onClick: scheduleFromComposer },
    state.scheduled.some((s) => s.channel_id === state.channelId) ? { label: "Scheduled messages", onClick: openScheduledList } : null,
  ]);
  const more = el("button", { type: "button", class: "send-more", id: "send-later", title: "Send later", "aria-label": "Send later", onclick: (e) => {
    const r = e.currentTarget.getBoundingClientRect();
    menu(r.right - 180, r.top - 110);
  } }, "▾");
  send.after(more);
  // Long-press Send on touch screens.
  let timer = null;
  let longPressed = false;
  send.addEventListener("pointerdown", (e) => {
    if (e.pointerType !== "touch") return;
    longPressed = false;
    timer = setTimeout(() => {
      longPressed = true;
      const r = send.getBoundingClientRect();
      if (navigator.vibrate) navigator.vibrate(15);
      menu(r.right - 180, r.top - 110);
    }, 450);
  });
  const clear = () => { clearTimeout(timer); timer = null; };
  send.addEventListener("pointerup", clear);
  send.addEventListener("pointercancel", clear);
  send.addEventListener("pointerleave", clear);
  send.addEventListener("contextmenu", (e) => { if (isTouch()) e.preventDefault(); });
  send.addEventListener("click", (e) => {
    if (longPressed) { e.preventDefault(); e.stopPropagation(); longPressed = false; }
  }, true);
  const sync = () => { more.hidden = input.disabled || !state.channelId; };
  const scBaseUpdateComposer = updateComposer;
  updateComposer = function (...args) {
    scBaseUpdateComposer(...args);
    sync();
    renderScheduledBar();
  };
  sync();
})();

// ======================================================================
// Live updates
// ======================================================================

const socialBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  switch (t) {
    case "READY":
      loadStickers();
      loadScheduled();
      break;
    case "GUILD_CREATE":
    case "GUILD_DELETE":
    case "STICKERS_CHANGED":
      loadStickers();
      break;
    case "SCHEDULED_UPDATE": {
      const before = state.scheduled.find((s) => s.id === d.id);
      upsertScheduled(d);
      if (d.status === "failed" && before?.status !== "failed") toast(`⚠ A scheduled message couldn't be sent: ${d.error || "unknown reason"}`);
      break;
    }
    case "SCHEDULED_DELETE":
      removeScheduled(d.id);
      break;
  }
  socialBaseHandle(ev);
};
