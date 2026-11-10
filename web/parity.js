// Outhouse web client: odds and ends that bring the side panels in line with
// the main chat. Loaded after social.js (uses its forward dialog and
// scheduled-message helpers) and channel-settings.js (spoilers).
//  - Threads: Forward (↪ and right-click) and Send later (▾ beside Send).
//  - Spoilers: forwarded spoilers stay blurred; pins and search blur too.
//  - Search, pins: automod's "Filtered" messages stay covered.
//  - Voice: people you've blocked fold into "Blocked user"; anyone in the
//    room the server leaves out of your list shows as "Someone (blocked)".
"use strict";

// ======================================================================
// Threads: Forward
// ======================================================================

const threadMsgById = (id) => thread.messages.find((m) => m.id === id);

const parBaseThreadMessage = threadMessage;
threadMessage = function (m, prev) {
  const node = parBaseThreadMessage(m, prev);
  if (m.ephemeral) return node;
  node.dataset.menu = "threadMessage";
  if (canForward(m)) {
    const bar = node.querySelector(".msg-actions");
    const fwd = el("button", { class: "fwd-btn", title: "Forward", "aria-label": "Forward", onclick: () => openForwardDialog(m) }, "↪");
    const react = bar?.querySelector("button[title='Add reaction']");
    if (react) react.after(fwd); else bar?.prepend(fwd);
  }
  return node;
};

MENUS.threadMessage = function (e, node) {
  const m = threadMsgById(node.dataset.id);
  if (!m) return;
  const mine = m.author_id === state.me.id;
  const selection = window.getSelection()?.toString();
  const link = e.target?.closest?.("a");
  const img = e.target?.closest?.("img");
  const canReact = can("add_reactions", m.channel_id);
  const canDelete = mine || can("manage_messages", m.channel_id);
  openMenu(e.clientX, e.clientY, [
    canReact ? { quick: QUICK_REACTIONS, onPick: (emoji) => toggleReaction(m, emoji) } : null,
    canReact ? { label: "Add reaction…", onClick: () => emojiPicker(node.querySelector(".msg-body"), (emoji) => toggleReaction(m, emoji)) } : null,
    canForward(m) ? { label: "Forward", onClick: () => openForwardDialog(m) } : null,
    mine ? { label: "Edit message", onClick: () => editThreadMessage(m) } : null,
    "sep",
    selection ? { label: "Copy selection", onClick: () => copy(selection, "Selection copied") } : null,
    m.content ? { label: "Copy text", onClick: () => copy(plainMentions(m.content), "Message copied") } : null,
    link ? { label: "Copy link", onClick: () => copy(link.href, "Link copied") } : null,
    link ? { label: "Open link", onClick: () => window.open(link.href, "_blank", "noopener") } : null,
    img ? { label: "Open image", onClick: () => window.open(img.src, "_blank", "noopener") } : null,
    "sep",
    { label: "Copy message ID", onClick: () => copy(m.id, "ID copied") },
    canDelete ? "sep" : null,
    canDelete ? { label: "Delete message", danger: true, onClick: () => deleteMessage(m) } : null,
  ]);
};

// ======================================================================
// Threads: Send later
// ======================================================================

const threadScheduled = () => (thread.info ? state.scheduled.filter((s) => s.thread_id === thread.info.id) : []);

async function scheduleFromThread(ta) {
  const info = thread.info;
  if (!info) return;
  const content = ta.value.trim();
  if (!content) return toast("Write your message first, then pick Send later.");
  if (thread.files.length) return toast("Scheduled messages are text only. Send the files now or remove them.");
  const at = await pickSendTime();
  if (!at) return;
  try {
    const s = await api("POST", `/api/channels/${info.channel_id}/scheduled`, { content: encodeMentions(content), send_at: at.toISOString(), thread_id: info.id });
    upsertScheduled(s);
    if (ta.value.trim() === content) { ta.value = ""; ta.dispatchEvent(new Event("input")); }
    toast(`Scheduled for ${whenLabel(s.send_at)}.`);
  } catch (err) {
    toast(err.message);
  }
}

function openThreadScheduledList() {
  const info = thread.info;
  if (!info) return;
  const dialog = el("dialog", { class: "sched-dialog sched-list-dialog" }, el("form", { method: "dialog" },
    el("h2", {}, `Scheduled in 🧵 ${info.name}`),
    el("p", { class: "muted small", style: "margin:0" }, `Times are in ${myTimeZone()}. They're sent with your permissions at that moment.`),
    el("div", { id: "sched-list", class: "sched-list", "data-channel": info.channel_id, "data-thread": info.id }),
    el("div", { class: "row" }, el("button", { type: "submit", class: "primary", value: "ok" }, "Done"))));
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  drawScheduledList();
  dialog.showModal();
}

// The list dialog shows only that thread's when opened from the thread.
const parBaseDrawScheduledList = drawScheduledList;
drawScheduledList = function (...args) {
  const out = parBaseDrawScheduledList(...args);
  const box = $("sched-list");
  const threadId = box?.dataset.thread;
  if (!threadId) return out;
  const items = [...box.querySelectorAll(".sched-item[data-id]")];
  let shown = 0;
  for (const item of items) {
    const s = state.scheduled.find((x) => x.id === item.dataset.id);
    if (s?.thread_id === threadId) { shown++; item.querySelector(".si-when .muted")?.remove(); } else item.remove();
  }
  if (!shown) box.replaceChildren(el("p", { class: "muted" }, "Nothing scheduled in this thread."));
  return out;
};

// "You have 1 scheduled message in this thread" above the thread's reply box.
function renderThreadSchedBar() {
  const wrap = sidePanel.kind === "thread" ? sidePanel.node?.querySelector(".thread-composer-wrap") : null;
  if (!wrap) return;
  let bar = wrap.querySelector(".thread-sched-bar");
  const here = threadScheduled();
  if (!here.length) { bar?.remove(); return; }
  if (!bar) {
    bar = el("div", { class: "sched-bar thread-sched-bar", role: "button", tabindex: "0", onclick: openThreadScheduledList,
      onkeydown: (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openThreadScheduledList(); } } });
    wrap.prepend(bar);
  }
  const failed = here.filter((s) => s.status === "failed").length;
  const pending = here.length - failed;
  bar.classList.toggle("has-failed", failed > 0);
  bar.replaceChildren(
    el("span", { class: "sb-icon", "aria-hidden": "true" }, failed ? "⚠" : "🕒"),
    el("span", { class: "sb-text" },
      pending ? `${pending} scheduled in this thread` : "",
      pending && failed ? " · " : "",
      failed ? el("span", { class: "sb-failed" }, `${failed} couldn't be sent`) : "",
      pending ? el("span", { class: "muted" }, ` · next ${whenLabel(here.find((s) => s.status !== "failed").send_at)}`) : ""),
    el("span", { class: "sb-view" }, "View"));
}

const parBaseRenderScheduledBar = renderScheduledBar;
renderScheduledBar = function (...args) {
  const out = parBaseRenderScheduledBar(...args);
  renderThreadSchedBar();
  return out;
};

const parBaseThreadComposer = threadComposer;
threadComposer = function (...args) {
  const wrap = parBaseThreadComposer(...args);
  const form = wrap.querySelector("form.thread-composer");
  const send = form?.querySelector("button.send");
  const ta = form?.querySelector("textarea");
  if (!send || !ta) return wrap;
  const menu = (x, y) => openMenu(x, y, [
    { label: "Send now", onClick: () => form.requestSubmit() },
    { label: "Send later…", onClick: () => scheduleFromThread(ta) },
    threadScheduled().length ? { label: "Scheduled messages", onClick: openThreadScheduledList } : null,
  ]);
  const more = el("button", { type: "button", class: "send-more thread-send-later", title: "Send later", "aria-label": "Send later", disabled: send.disabled, onclick: (e) => {
    const r = e.currentTarget.getBoundingClientRect();
    menu(r.right - 180, r.top - 110);
  } }, "▾");
  send.after(more);
  // Press and hold Send on touch screens.
  let timer = null;
  let longPressed = false;
  send.addEventListener("pointerdown", (e) => {
    if (e.pointerType !== "touch" || send.disabled) return;
    longPressed = false;
    timer = setTimeout(() => {
      longPressed = true;
      const r = send.getBoundingClientRect();
      if (navigator.vibrate) navigator.vibrate(15);
      menu(Math.max(8, r.right - 180), r.top - 110);
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
  return wrap;
};

// Locked/archived threads disable Send: the ▾ follows it. The scheduled bar
// goes back on whenever the panel's composer is redrawn.
const parBasePaintThreadArchive = paintThreadArchive;
paintThreadArchive = function (...args) {
  const out = parBasePaintThreadArchive(...args);
  const wrap = sidePanel.kind === "thread" ? sidePanel.node?.querySelector(".thread-composer-wrap") : null;
  const more = wrap?.querySelector(".thread-send-later");
  if (more) more.disabled = !!wrap.querySelector("button.send")?.disabled;
  renderThreadSchedBar();
  return out;
};

// ======================================================================
// Spoilers: forwarded ones, pins and search
// ======================================================================

// Blurs a forwarded spoiler wherever it lands (until it's clicked).
function blurForwarded(article, m) {
  if (!article || !m?.forward?.source_spoiler) return;
  const ch = chById(m.channel_id) || { id: m.channel_id };
  // spoilerize skips messages with nothing to hide; the snapshot counts.
  spoilerize(article, { ...m, content: m.content || m.forward.content || " " }, ch, null);
}

const parBaseRenderMessages = renderMessages;
renderMessages = function (...args) {
  const out = parBaseRenderMessages(...args);
  if (!state.friendsView) {
    for (const article of document.querySelectorAll("#messages article.msg[data-id]")) {
      const m = findMessage(article.dataset.id);
      if (m?.forward?.source_spoiler) blurForwarded(article, m);
    }
  }
  return out;
};
renderMessages.forceBottom = false;

const isSpoilerChannel = (ch) => !!ch && isGuildText(ch) && ch.content_mode === "spoiler";

// A card in the pins or search list: automod's Filtered cover, and spoiler blur.
const parBaseMessageCard = messageCard;
messageCard = function (m, opts) {
  const hidden = typeof isFilteredHidden === "function" && isFilteredHidden(m);
  if (hidden) {
    // Nothing of the message (not even matched search words) until "Show it".
    const node = parBaseMessageCard({ ...m, content: "", attachments: [], embeds: [], event: null, poll: null }, { ...opts, terms: null });
    node.classList.add("sp-filtered");
    node.append(el("div", { class: "filtered-cover" },
      el("span", { class: "fc-icon", "aria-hidden": "true" }, "🚧"),
      el("span", { class: "fc-text" }, "Automod hid this message."),
      el("button", { type: "button", class: "link fc-show", onclick: (e) => {
        e.stopPropagation();
        filteredRevealed.add(m.id);
        node.replaceWith(messageCard(m, opts));
        if (typeof redrawMessage === "function" && findMessage(m.id)) redrawMessage(m);
      } }, "Show it")));
    return node;
  }
  const node = parBaseMessageCard(m, opts);
  const ch = chById(m.channel_id);
  const spoiler = (isSpoilerChannel(ch) && !spoilerShowAll.has(ch.id)) || !!m.forward?.source_spoiler;
  const hasStuff = node.querySelector(".sp-text, .sp-thumb, .sp-files");
  if (spoiler && hasStuff && m.author_id !== state.me.id && !spoilerRevealed.has(m.id)) {
    node.classList.add("sp-spoilered");
    node.append(el("span", { class: "sp-spoiler-tag" }, "SPOILER · click to show"));
    node.addEventListener("click", (e) => {
      if (!node.classList.contains("sp-spoilered") || e.target.closest(".sp-x")) return;
      e.preventDefault();
      e.stopImmediatePropagation(); // not a jump yet: this click only reveals
      spoilerRevealed.add(m.id);
      node.classList.remove("sp-spoilered");
      node.querySelector(".sp-spoiler-tag")?.remove();
    }, true);
  }
  return node;
};

// ======================================================================
// Voice: blocked people
// ======================================================================

const voiceBlockedShown = new Set(); // people you've blocked, shown for now anyway
const voiceLeftAt = new Map(); // user ID → when they left your channel (ms)
const foldBlockedInVoice = () => prefs.get("voice_fold_blocked", true);
const iBlocked = (id) => typeof isBlocked === "function" && isBlocked(id);
let parHiddenKey = ""; // who's shown as "Someone (blocked)" right now

// People in your LiveKit room the server leaves out of your list (they've
// blocked you). Not someone who only just left (LiveKit catches up later).
function hiddenVoiceParticipants() {
  const v = state.voice;
  if (!v.room || !v.channelId || v.guildId !== state.guildId) return [];
  const now = Date.now();
  const out = [];
  for (const p of v.room.remoteParticipants.values()) {
    const id = p.identity;
    if (state.voiceStates.get(id)?.channel_id === v.channelId) continue;
    if (now - (voiceLeftAt.get(id) || 0) < 8000) continue;
    out.push(id);
  }
  return out;
}

// How a voice participant is labelled (screen shares): null for their name.
function voiceParticipantLabel(id) {
  if (iBlocked(id) && foldBlockedInVoice() && !voiceBlockedShown.has(id)) return "Someone (blocked)";
  if (state.voice.room && state.voice.room.remoteParticipants.has(id) && hiddenVoiceParticipants().includes(id)) return "Someone (blocked)";
  return null;
}

const parBaseRenderChannels = renderChannels;
renderChannels = function (...args) {
  const out = parBaseRenderChannels(...args);
  const list = $("voice-channels");
  if (!list) return out;
  const speaking = state.voice.speaking || new Set();
  // Fold people you've blocked into a placeholder.
  if (foldBlockedInVoice()) {
    for (const row of list.querySelectorAll(".voice-user[data-id]")) {
      const id = row.dataset.id;
      if (!iBlocked(id) || voiceBlockedShown.has(id)) continue;
      row.classList.add("blocked-voice");
      row.replaceChildren(
        el("span", { class: "bv-icon", "aria-hidden": "true" }, "🚫"),
        el("span", { class: "vname" }, "Blocked user"),
        el("button", { type: "button", class: "link bv-show", title: "Show who it is", onclick: (e) => { e.stopPropagation(); voiceBlockedShown.add(id); renderChannels(); } }, "Show"));
    }
  }
  // Anyone else in your room you can hear but aren't told about.
  const hidden = hiddenVoiceParticipants();
  const mine = list.querySelector(`.channel.voice[data-id="${state.voice.channelId}"]`);
  if (hidden.length && mine) {
    let last = mine;
    while (last.nextElementSibling?.classList.contains("voice-user")) last = last.nextElementSibling;
    for (const id of hidden) {
      const row = el("div", { class: `voice-user blocked-voice hidden-voice${speaking.has(id) ? " speaking" : ""}`, "data-id": id, title: "Someone in this channel who has blocked you, or whom you've blocked" },
        el("span", { class: "bv-icon", "aria-hidden": "true" }, "🚫"),
        el("span", { class: "vname" }, "Someone (blocked)"));
      last.after(row);
      last = row;
    }
  }
  parHiddenKey = hidden.join(",");
  return out;
};

// LiveKit people come and go without a gateway event: check now and then.
setInterval(() => {
  if (!state.voice.room) { if (parHiddenKey) { parHiddenKey = ""; renderChannels(); } return; }
  if (hiddenVoiceParticipants().join(",") !== parHiddenKey) renderChannels();
}, 1500);

// No entrance sounds or soundboard clips from people you've blocked.
if (typeof onEntranceSound === "function") {
  const parBaseEntrance = onEntranceSound;
  onEntranceSound = function (d) {
    if (iBlocked(d.user_id)) return;
    return parBaseEntrance(d);
  };
}

const parBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  if (t === "SOUNDBOARD_PLAY" && iBlocked(d.user_id)) return;
  if (t === "VOICE_STATE_UPDATE" && d.user_id !== state.me?.id && d.channel_id !== state.voice.channelId) {
    voiceLeftAt.set(d.user_id, Date.now());
  }
  parBaseHandle(ev);
  // Blocking or unblocking someone changes how they're shown.
  if (t === "RELATIONSHIP_UPDATE") renderChannels();
};

// Privacy settings: the fold switch.
if (typeof drawPrivacyPage === "function") {
  const parBasePrivacy = drawPrivacyPage;
  drawPrivacyPage = async function (main, ...rest) {
    const out = await parBasePrivacy(main, ...rest);
    const box = el("input", { type: "checkbox", id: "voice-fold-blocked", checked: foldBlockedInVoice() });
    box.addEventListener("change", () => {
      prefs.set("voice_fold_blocked", box.checked);
      voiceBlockedShown.clear();
      renderChannels();
      toast("Saved.");
    });
    main.append(el("section", { class: "notify-box" },
      el("h3", {}, "Blocked people in voice"),
      el("label", { class: "check-row" }, box, "Fold people you've blocked into “Blocked user” in voice channel lists"),
      el("p", { class: "muted small" }, "You can still click Show to see who it is. People who've blocked you don't see you in their voice lists.")));
    return out;
  };
}
