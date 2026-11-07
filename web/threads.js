// Outhouse web client: threads, polls, custom emoji and loading older
// messages as you scroll up. Loaded last (uses globals from the other files).
"use strict";

// ======================================================================
// Loading older messages (scroll to the top)
// ======================================================================

state.historyStart = null; // channel ID whose very first message is loaded

let olderLoading = false;
async function loadOlderMessages() {
  const box = $("messages");
  const channelId = state.channelId;
  const first = state.messages[0];
  if (!channelId || !first || first.channel_id !== channelId || olderLoading || state.historyStart === channelId) return;
  olderLoading = true;
  const loader = el("div", { class: "history-loading" }, "Digging up older messages…");
  box.prepend(loader);
  try {
    const older = await api("GET", `/api/channels/${channelId}/messages?before=${first.id}&limit=50`);
    loader.remove();
    if (state.channelId !== channelId || state.messages[0] !== first) return;
    if (older.length < 50) state.historyStart = channelId;
    const fresh = older.filter((m) => !findMessage(m.id));
    const prevHeight = box.scrollHeight;
    const prevTop = box.scrollTop;
    state.messages = [...fresh, ...state.messages];
    renderMessages();
    // Keep what you were reading in the same place on screen.
    box.scrollTop = box.scrollHeight - prevHeight + prevTop;
  } catch (err) {
    loader.remove();
    toast(err.message);
  } finally {
    olderLoading = false;
  }
}

$("messages").addEventListener("scroll", () => {
  if ($("messages").scrollTop < 300) loadOlderMessages();
}, { passive: true });

// ======================================================================
// Threads
// ======================================================================

const thread = { info: null, messages: [], atStart: false, loadingOlder: false, files: [] };

const repliesLabel = (n) => (n ? `${n} ${n === 1 ? "reply" : "replies"}` : "No replies yet");

// The "🧵 Name · 3 replies" link under a message that has a thread.
function threadLink(t) {
  return el("button", { class: "thread-link", title: "Open thread", onclick: () => openThread(t) },
    el("span", { class: "tl-icon", "aria-hidden": "true" }, "🧵"),
    el("span", { class: "tl-name" }, t.name),
    el("span", { class: "tl-count" }, repliesLabel(t.message_count)),
    el("span", { class: "tl-go", "aria-hidden": "true" }, "›"));
}

async function startThread(m) {
  if (m.thread) return openThread(m.thread);
  const suggested = plainMentions(m.content).replace(/https?:\/\/\S+/g, "").replace(/\s+/g, " ").trim().slice(0, 40) || (m.poll ? m.poll.question.slice(0, 40) : "");
  const name = await ask("Start a thread", {
    value: suggested,
    placeholder: "What's it about?",
    confirm: "Start thread",
    body: el("p", { class: "muted", style: "margin:0" }, "A side chat about this message, so the channel stays tidy."),
  });
  if (!name) return;
  const t = await tryApi("POST", `/api/channels/${m.channel_id}/threads`, { name, message_id: m.id }).catch(() => null);
  if (!t) return;
  const msg = findMessage(m.id);
  if (msg) { msg.thread = t; renderMessages(); }
  openThread(t);
}

async function newChannelThread() {
  const name = await ask("New thread", { placeholder: "Thread name", confirm: "Start thread" });
  if (!name || !state.channelId) return;
  const t = await tryApi("POST", `/api/channels/${state.channelId}/threads`, { name }).catch(() => null);
  if (t) openThread(t);
}

const canManageThread = (t) => t.creator_id === state.me.id || can("manage_messages", t.channel_id);

async function openThread(t) {
  if (typeof t === "string") t = await tryApi("GET", `/api/threads/${t}`).catch(() => null);
  if (!t) return;
  if (t.channel_id !== state.channelId) {
    if (!state.channels.some((c) => c.id === t.channel_id)) return toast("That thread's channel is gone.");
    await selectChannel(t.channel_id);
    if (state.channelId !== t.channel_id) return;
  }
  const more = el("button", { class: "icon-btn", title: "Thread options", "aria-label": "Thread options", onclick: (e) => {
    const r = e.currentTarget.getBoundingClientRect();
    openMenu(r.left, r.bottom + 4, [
      canManageThread(thread.info) ? { label: "Rename thread", onClick: renameThread } : null,
      { label: "Copy thread ID", onClick: () => copy(thread.info.id, "ID copied") },
      can("manage_messages", thread.info.channel_id) ? "sep" : null,
      can("manage_messages", thread.info.channel_id) ? { label: "Delete thread", danger: true, onClick: deleteThread } : null,
    ]);
  } }, "⋯");
  const body = showSidePanel("thread", `🧵 ${t.name}`);
  sidePanel.node.querySelector(".sp-head .icon-btn")?.before(more);
  Object.assign(thread, { info: t, messages: [], atStart: false, loadingOlder: false, files: [] });
  body.classList.add("thread-body");
  body.append(el("p", { class: "sp-note" }, "Loading…"));
  body.addEventListener("scroll", () => { if (body.scrollTop < 200) loadOlderThread(); }, { passive: true });
  sidePanel.node.append(threadComposer());
  const msgs = await api("GET", `/api/channels/${t.channel_id}/messages?thread=${t.id}&limit=50`).catch((err) => { toast(err.message); return null; });
  if (!msgs || sidePanel.kind !== "thread" || thread.info?.id !== t.id) return;
  thread.messages = msgs;
  thread.atStart = msgs.length < 50;
  renderThread(true);
  if (!isPhone()) sidePanel.node.querySelector(".thread-composer textarea")?.focus();
}

async function loadOlderThread() {
  const t = thread.info;
  const first = thread.messages[0];
  if (!t || !first || thread.atStart || thread.loadingOlder) return;
  thread.loadingOlder = true;
  try {
    const older = await api("GET", `/api/channels/${t.channel_id}/messages?thread=${t.id}&before=${first.id}&limit=50`);
    if (thread.info?.id !== t.id || thread.messages[0] !== first) return;
    if (older.length < 50) thread.atStart = true;
    const body = sidePanel.body;
    const prevHeight = body.scrollHeight;
    const prevTop = body.scrollTop;
    thread.messages = [...older.filter((m) => !thread.messages.some((x) => x.id === m.id)), ...thread.messages];
    renderThread();
    body.scrollTop = body.scrollHeight - prevHeight + prevTop;
  } catch (err) {
    toast(err.message);
  } finally {
    thread.loadingOlder = false;
  }
}

function threadMessage(m, prev) {
  const grouped = prev && !m.reply_to && prev.author_id === m.author_id && new Date(m.created_at) - new Date(prev.created_at) < 5 * 60 * 1000;
  const mine = m.author_id === state.me.id;
  const colour = memberColour(m.author_id);
  const imageOnly = m.embeds.length === 1 && m.embeds[0].kind === "image" && m.content.trim() === m.embeds[0].url;
  return el("article", { class: `msg tmsg${grouped ? "" : " first"}`, id: `tmsg-${m.id}`, "data-id": m.id },
    grouped ? el("div", { class: "gutter" }) : messageAvatar(m),
    el("div", { class: "msg-body" },
      grouped ? null : el("div", { class: "msg-meta" },
        el("span", { class: "msg-author", style: colour ? `color:${colour}` : "", onclick: (e) => openProfile(m.author_id, e.currentTarget) }, authorName(m)),
        m.author_bot ? el("span", { class: "bot-tag" }, "BOT") : null,
        el("time", { class: "msg-time", datetime: m.created_at, title: new Date(m.created_at).toLocaleString() }, timeLabel(m.created_at))),
      m.reply_to ? el("div", { class: "tmsg-reply" }, "↱ ", el("strong", {}, m.reply_to.author_display_name), " ", plainMentions(m.reply_to.content) || "(attachment)") : null,
      m.content && !imageOnly ? el("p", { class: "msg-text" }, renderText(m.content), m.edited_at ? el("span", { class: "edited", title: new Date(m.edited_at).toLocaleString() }, "(edited)") : null) : null,
      renderAttachments(m),
      renderEmbeds(m),
      renderReactions(m)),
    el("div", { class: "msg-actions" },
      can("add_reactions", m.channel_id) ? el("button", { title: "Add reaction", onclick: (e) => emojiPicker(e.currentTarget, (emoji) => toggleReaction(m, emoji)) }, "😀") : null,
      mine ? el("button", { title: "Edit", onclick: () => editThreadMessage(m) }, "✎") : null,
      mine || can("manage_messages", m.channel_id) ? el("button", { class: "danger", title: "Delete", onclick: (e) => deleteMessage(m, e.shiftKey) }, "🗑") : null));
}

function renderThread(forceBottom = false) {
  const body = sidePanel.kind === "thread" ? sidePanel.body : null;
  if (!body || !thread.info) return;
  const near = body.scrollHeight - body.scrollTop - body.clientHeight < 120;
  const nodes = [];
  if (thread.atStart) {
    const starter = thread.info.starter_message_id && state.messages.find((m) => m.id === thread.info.starter_message_id);
    nodes.push(el("div", { class: "thread-start" },
      el("div", { class: "thread-start-icon", "aria-hidden": "true" }, "🧵"),
      el("div", { class: "thread-start-name" }, thread.info.name),
      el("div", { class: "muted small" }, `Started ${timeLabel(thread.info.created_at)}`),
      starter ? messageCard(starter) : null));
  }
  let prev = null;
  for (const m of thread.messages) {
    nodes.push(threadMessage(m, prev));
    prev = m;
  }
  if (!thread.messages.length && thread.atStart) nodes.push(el("p", { class: "sp-note thread-empty" }, "No replies yet. Be the first to chime in."));
  body.replaceChildren(...nodes);
  if (near || forceBottom) body.scrollTop = body.scrollHeight;
}

function threadComposer() {
  const t = thread.info;
  const canSend = can("send_messages", t.channel_id);
  const ta = el("textarea", { rows: 1, placeholder: canSend ? `Reply in ${t.name}` : "You can't post here.", "aria-label": "Reply in thread", disabled: !canSend });
  const chips = el("div", { class: "pending-files", hidden: true });
  const fileInput = el("input", { type: "file", multiple: true, hidden: true });
  const drawChips = () => {
    chips.hidden = !thread.files.length;
    chips.replaceChildren(...thread.files.map((f, i) => el("span", { class: "chip" }, f.type.startsWith("image/") ? "🖼" : "📄", ` ${f.name} `,
      el("button", { type: "button", "aria-label": `Remove ${f.name}`, onclick: () => { thread.files.splice(i, 1); drawChips(); } }, "✕"))));
  };
  fileInput.addEventListener("change", () => {
    for (const f of fileInput.files) {
      if (f.size > 25 * 1024 * 1024) { toast(`${f.name} is over 25 MB.`); continue; }
      if (thread.files.length < 10) thread.files.push(f);
    }
    fileInput.value = "";
    drawChips();
  });
  const resize = () => { ta.style.height = "auto"; ta.style.height = `${Math.min(ta.scrollHeight, 160)}px`; };
  const send = async () => {
    const info = thread.info;
    const raw = ta.value.trim();
    if (!info || (!raw && !thread.files.length)) return;
    const content = encodeMentions(raw);
    const files = thread.files;
    ta.value = "";
    resize();
    try {
      let msg;
      if (files.length) {
        const form = new FormData();
        form.append("content", content);
        form.append("thread_id", info.id);
        for (const f of files) form.append("files", f, f.name);
        msg = await api("POST", `/api/channels/${info.channel_id}/messages/upload`, form);
      } else {
        msg = await api("POST", `/api/channels/${info.channel_id}/messages`, { content, thread_id: info.id });
      }
      thread.files = [];
      drawChips();
      if (thread.info?.id === info.id && !thread.messages.some((x) => x.id === msg.id)) {
        thread.messages.push(msg);
        renderThread(true);
      }
    } catch (err) {
      ta.value = raw;
      resize();
      toast(err.message);
    }
  };
  ta.addEventListener("input", resize);
  ta.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !isTouch()) { e.preventDefault(); send(); }
  });
  ta.addEventListener("paste", (e) => {
    const files = [...(e.clipboardData?.files || [])];
    if (files.length && can("attach_files", t.channel_id)) { e.preventDefault(); thread.files.push(...files.slice(0, 10 - thread.files.length)); drawChips(); }
  });
  return el("div", { class: "thread-composer-wrap" }, chips,
    el("form", { class: "composer thread-composer", onsubmit: (e) => { e.preventDefault(); send(); } },
      can("attach_files", t.channel_id) && canSend ? el("button", { type: "button", class: "tool", title: "Upload files", "aria-label": "Upload files", onclick: () => fileInput.click() }, "+") : null,
      fileInput, ta,
      el("button", { type: "submit", class: "send", "aria-label": "Send", disabled: !canSend },
        el("span", { "aria-hidden": "true" }, "➤"))));
}

async function editThreadMessage(m) {
  const value = await ask("Edit message", { value: plainMentions(m.content), confirm: "Save", allowEmpty: m.attachments.length > 0 });
  if (value === null || value === plainMentions(m.content)) return;
  await tryApi("PATCH", `/api/channels/${m.channel_id}/messages/${m.id}`, { content: encodeMentions(value) }).catch(() => {});
}

async function renameThread() {
  const t = thread.info;
  if (!t) return;
  const name = await ask("Rename thread", { value: t.name, confirm: "Save" });
  if (!name || name === t.name) return;
  await tryApi("PATCH", `/api/threads/${t.id}`, { name }).catch(() => {});
}

async function deleteThread() {
  const t = thread.info;
  if (!t || !(await confirmBox(`Delete ${t.name}?`, "The thread and all its replies go down the long drop. This can't be undone.", "Delete thread"))) return;
  await tryApi("DELETE", `/api/threads/${t.id}`).catch(() => {});
}

// ---- The threads list for a channel ----

function openThreadsList() {
  if (sidePanel.kind === "threads") return closeSidePanel();
  if (!state.channelId) return;
  showSidePanel("threads", "🧵 Threads",
    can("send_messages", state.channelId)
      ? el("div", { class: "sp-actions" }, el("button", { class: "primary", onclick: newChannelThread }, "＋ New thread"))
      : null);
  drawThreadsList();
}

async function drawThreadsList() {
  const channelId = state.channelId;
  const body = sidePanel.body;
  if (sidePanel.kind !== "threads" || !body || !channelId) return;
  if (!body.childElementCount) body.append(el("p", { class: "sp-note" }, "Loading…"));
  const list = await api("GET", `/api/channels/${channelId}/threads`).catch((err) => { toast(err.message); return null; });
  if (sidePanel.kind !== "threads" || sidePanel.body !== body || state.channelId !== channelId || !list) return;
  body.replaceChildren(...(list.length ? list.map((t) => {
    const card = el("div", { class: "sp-card thread-card", role: "button", tabindex: "0", onclick: () => openThread(t) },
      el("div", { class: "tc-name" }, "🧵 ", t.name),
      el("div", { class: "tc-sub" }, repliesLabel(t.message_count), " · started ", timeLabel(t.created_at)));
    card.addEventListener("keydown", (e) => { if (e.key === "Enter") card.click(); });
    return card;
  }) : [el("div", { class: "sp-empty" },
    el("div", { class: "sp-empty-icon" }, "🧵"),
    el("p", {}, "No threads here yet."),
    el("p", { class: "muted" }, "Start one from a message's 🧵 button to keep side chats tidy."))]));
}

let threadsRedraw = null;
function onThreadUpdate(t) {
  const starter = t.starter_message_id && findMessage(t.starter_message_id);
  if (starter) { starter.thread = t; renderMessages(); }
  if (thread.info?.id === t.id) {
    thread.info = t;
    const title = sidePanel.node?.querySelector(".sp-title");
    if (title) title.textContent = `🧵 ${t.name}`;
  }
  if (sidePanel.kind === "threads" && t.channel_id === state.channelId) {
    clearTimeout(threadsRedraw);
    threadsRedraw = setTimeout(drawThreadsList, 200);
  }
}

function onThreadDelete(d) {
  for (const m of state.messages) if (m.thread?.id === d.id) m.thread = null;
  if (thread.info?.id === d.id) {
    closeSidePanel();
    toast("That thread was deleted.");
  }
  if (sidePanel.kind === "threads" && d.channel_id === state.channelId) drawThreadsList();
  renderMessages();
}

// Keep the thread panel closed when it belongs to another channel.
const baseCloseSidePanel = closeSidePanel;
closeSidePanel = function () {
  thread.info = null;
  thread.messages = [];
  baseCloseSidePanel();
};

const baseOnChannelChanged = onChannelChanged;
onChannelChanged = function () {
  state.historyStart = null;
  if (thread.info && thread.info.channel_id !== state.channelId) closeSidePanel();
  baseOnChannelChanged();
  if (sidePanel.kind === "threads") drawThreadsList();
};

// Search results and pins inside a thread open the thread.
const baseJumpToMessage = jumpToMessage;
jumpToMessage = async function (channelId, messageId) {
  const hit = document.querySelector(`.sp-card[data-id="${messageId}"]`);
  const threadId = hit?.dataset.thread;
  if (threadId) return openThread(threadId);
  return baseJumpToMessage(channelId, messageId);
};
const baseMessageCard = messageCard;
messageCard = function (m, opts) {
  const node = baseMessageCard(m, opts);
  if (m.thread_id) {
    node.dataset.thread = m.thread_id;
    node.querySelector(".sp-meta")?.append(el("span", { class: "sp-chan" }, "🧵 thread"));
  }
  return node;
};

// ======================================================================
// Polls
// ======================================================================

const POLL_COMMAND = {
  id: "poll", bot_id: "0", bot_name: "Outhouse", name: "poll", builtin: true,
  description: "Ask a question with up to 10 options",
  options: [{ name: "question", description: "What are you asking?", required: false }],
};

const POLL_DURATIONS = [["No end time", 0], ["1 hour", 60], ["4 hours", 240], ["1 day", 1440], ["3 days", 4320], ["1 week", 10080]];

function pollVoters(p) {
  return new Set(p.options.flatMap((o) => o.user_ids)).size;
}

function pollEnded(p) {
  return p.closed || (p.ends_at && new Date(p.ends_at) <= new Date());
}

function renderPollCard(m) {
  const p = m.poll;
  const voters = pollVoters(p);
  const ended = pollEnded(p);
  const canVote = !ended && can("add_reactions", m.channel_id);
  const top = Math.max(0, ...p.options.map((o) => o.user_ids.length));
  const nameOf = (id) => memberById(id)?.display_name ?? (id === state.me.id ? state.me.display_name : "someone");
  return el("div", { class: `poll-card${ended ? " ended" : ""}` },
    el("div", { class: "poll-kicker" }, ended ? "📊 POLL · CLOSED" : "📊 POLL"),
    el("div", { class: "poll-q" }, p.question),
    el("div", { class: "poll-hint" }, ended ? "Voting's closed." : p.multi ? "Pick as many as you like." : "Pick one."),
    el("div", { class: "poll-options", role: p.multi ? "group" : "radiogroup" }, p.options.map((o, i) => {
      const mine = o.user_ids.includes(state.me.id);
      const pct = voters ? Math.round((o.user_ids.length / voters) * 100) : 0;
      const names = o.user_ids.map(nameOf);
      return el("button", {
        type: "button",
        class: `poll-opt${mine ? " mine" : ""}${ended && top > 0 && o.user_ids.length === top ? " winner" : ""}`,
        disabled: !canVote,
        role: p.multi ? "checkbox" : "radio",
        "aria-checked": mine ? "true" : "false",
        title: names.length ? `${names.join(", ")}` : "No votes yet",
        onclick: () => votePoll(m, i),
      },
      el("span", { class: "poll-fill", style: `width:${pct}%` }),
      el("span", { class: `poll-check${p.multi ? " square" : ""}` }, mine ? "✓" : ""),
      el("span", { class: "poll-text" }, o.text),
      el("span", { class: "poll-count" }, `${o.user_ids.length} · ${pct}%`));
    })),
    el("div", { class: "poll-foot" },
      el("span", {}, `${voters} ${voters === 1 ? "vote" : "votes"}`),
      el("span", {}, ended ? "Final results" : p.ends_at ? `Ends ${relativeTime(new Date(p.ends_at))}` : "No end time"),
      voters ? el("button", { type: "button", class: "link poll-link", onclick: (e) => showVoters(e.currentTarget, m) }, "See votes") : null,
      !ended && (m.author_id === state.me.id || can("manage_messages", m.channel_id))
        ? el("button", { type: "button", class: "link poll-link", onclick: () => endPoll(m) }, "End poll") : null));
}

function showVoters(anchor, m) {
  const p = m.poll;
  const nameOf = (id) => memberById(id)?.display_name ?? (id === state.me.id ? state.me.display_name : "someone");
  openPopover(anchor, el("div", { class: "poll-voters" },
    el("div", { class: "pv-title" }, p.question),
    ...p.options.map((o) => el("div", { class: "pv-opt" },
      el("div", { class: "pv-name" }, o.text, el("span", { class: "muted" }, ` · ${o.user_ids.length}`)),
      el("div", { class: "pv-people" }, o.user_ids.length
        ? o.user_ids.map((id) => el("span", { class: "pv-person" }, avatarFor(id, nameOf(id), 20), nameOf(id)))
        : el("span", { class: "muted small" }, "Nobody"))))), "above");
}

async function votePoll(m, i) {
  const p = m.poll;
  const mine = p.options.map((o, k) => (o.user_ids.includes(state.me.id) ? k : -1)).filter((k) => k >= 0);
  let picks;
  if (p.multi) picks = mine.includes(i) ? mine.filter((k) => k !== i) : [...mine, i];
  else picks = mine.includes(i) ? [] : [i];
  const msg = await tryApi("PUT", `/api/channels/${m.channel_id}/polls/${m.id}/votes`, { options: picks }).catch(() => null);
  if (msg) applyMessageUpdate(msg);
}

async function endPoll(m) {
  if (!(await confirmBox("End the poll now?", "Nobody can vote after this. The results stay up.", "End poll"))) return;
  const msg = await tryApi("POST", `/api/channels/${m.channel_id}/polls/${m.id}/close`).catch(() => null);
  if (msg) applyMessageUpdate(msg);
}

function openPollDialog(question = "") {
  if (!state.channelId) return;
  const channelId = state.channelId;
  const q = el("input", { placeholder: "What should we raid this week?", maxlength: 300, value: question, "aria-label": "Question" });
  const list = el("div", { class: "poll-edit-options" });
  const addBtn = el("button", { type: "button", class: "ghost", onclick: () => { addOption(""); } }, "＋ Add option");
  const multi = el("input", { type: "checkbox" });
  const duration = el("select", { "aria-label": "How long" }, POLL_DURATIONS.map(([label, mins]) => el("option", { value: String(mins), selected: mins === 1440 }, label)));
  const error = el("p", { class: "error" });
  const sync = () => {
    const rows = list.querySelectorAll(".poll-edit-row");
    addBtn.hidden = rows.length >= 10;
    rows.forEach((row, i) => {
      row.querySelector("input").placeholder = `Option ${i + 1}`;
      row.querySelector("button").disabled = rows.length <= 2;
    });
  };
  function addOption(value) {
    const inputEl = el("input", { maxlength: 100, value, "aria-label": "Option" });
    const row = el("div", { class: "poll-edit-row" }, inputEl,
      el("button", { type: "button", class: "icon-btn", "aria-label": "Remove option", onclick: () => { row.remove(); sync(); } }, "✕"));
    list.append(row);
    sync();
    if (!value) inputEl.focus();
  }
  const dialog = el("dialog", { class: "poll-dialog" },
    el("form", { method: "dialog" },
      el("h2", {}, "📊 Make a poll"),
      el("label", { class: "field" }, "Question", q),
      el("div", { class: "field" }, "Options", list, addBtn),
      el("label", { class: "check-row" }, multi, "Let people pick more than one"),
      el("label", { class: "field" }, "Voting closes", duration),
      error,
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"),
        el("button", { type: "submit", class: "primary" }, "Post poll"))));
  addOption("");
  addOption("");
  dialog.querySelector("form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const options = [...list.querySelectorAll("input")].map((x) => x.value.trim()).filter(Boolean);
    if (!q.value.trim()) { error.textContent = "Ask a question."; return q.focus(); }
    if (options.length < 2) { error.textContent = "Give people at least two options."; return; }
    error.textContent = "";
    try {
      const msg = await api("POST", `/api/channels/${channelId}/polls`, { question: q.value.trim(), options, multi: multi.checked, duration_minutes: Number(duration.value) || null });
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
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  (question ? list.querySelector("input") : q).focus();
}

// The composer button and /poll.
(function pollButton() {
  const raid = $("raid-btn");
  const btn = el("button", { type: "button", class: "tool", id: "poll-btn", title: "Make a poll", "aria-label": "Make a poll", onclick: () => openPollDialog() });
  btn.innerHTML = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M4 20h16M7 16V9M12 16V5M17 16v-4"/></svg>';
  raid?.after(btn);
})();

const baseLoadCommands = loadCommands;
loadCommands = async function () {
  await baseLoadCommands();
  if (!state.commands.some((c) => c.name === "poll")) state.commands = [...state.commands, POLL_COMMAND];
};
const baseRunCommand = runCommand;
runCommand = async function (command, text) {
  if (command.builtin && command.name === "poll") return openPollDialog(text.replace(/^\/poll\s*/, "").replace(/^question:\s*/, ""));
  return baseRunCommand(command, text);
};

// "Ends in 3 hours" stays fresh.
setInterval(() => {
  if (state.messages.some((m) => m.poll && !m.poll.closed) && !document.hidden && !state.editingId) renderMessages();
}, 60_000);

// ======================================================================
// Custom emoji
// ======================================================================

state.emojis = []; // every custom emoji in your outhouses

async function loadEmojis() {
  const list = await api("GET", "/api/users/@me/emojis").catch(() => null);
  if (list) state.emojis = list;
}

// The emoji picker gets a second tab with outhouse emoji.
function customEmojiTabs(standard, onPick, forText) {
  const here = state.emojis.filter((e) => e.guild_id === state.guildId);
  const others = state.emojis.filter((e) => e.guild_id !== state.guildId);
  const groups = new Map();
  for (const e of [...here, ...others]) {
    if (!groups.has(e.guild_id)) groups.set(e.guild_id, []);
    groups.get(e.guild_id).push(e);
  }
  const custom = el("div", { class: "emoji-pane custom-pane", hidden: true },
    groups.size ? [...groups].map(([guildId, list]) => el("div", { class: "emoji-section" },
      el("div", { class: "emoji-section-title" }, (state.guilds.find((g) => g.id === guildId)?.name ?? "Outhouse").toUpperCase()),
      el("div", { class: "emoji-grid" }, list.map((e) => el("button", {
        type: "button",
        title: `:${e.name}:`,
        onclick: () => { closePopover(); onPick(forText ? `:${e.name}:` : `<:${e.name}:${e.id}>`); },
      }, el("img", { class: "custom-emoji", src: e.url, alt: `:${e.name}:`, loading: "lazy" }))))))
      : el("p", { class: "muted small emoji-none" }, "No outhouse emoji yet. People who can manage an outhouse add them in Outhouse settings → Emoji."));
  const stdPane = el("div", { class: "emoji-pane" }, standard);
  const tabStd = el("button", { type: "button", class: "on", onclick: () => show(false) }, "😀 Emoji");
  const tabCustom = el("button", { type: "button", class: "custom-tab", onclick: () => show(true) }, "⭐ Outhouse");
  function show(customOn) {
    custom.hidden = !customOn;
    stdPane.hidden = customOn;
    tabStd.classList.toggle("on", !customOn);
    tabCustom.classList.toggle("on", customOn);
  }
  return el("div", { class: "emoji-picker" }, el("div", { class: "emoji-tabs" }, tabStd, tabCustom), stdPane, custom);
}

const emojiFileName = (name) => name.replace(/\.[^.]+$/, "").toLowerCase().replace(/[^a-z0-9_]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 32);

async function drawEmojiSettings(main) {
  const guildId = state.guildId;
  const list = await api("GET", `/api/guilds/${guildId}/emojis`).catch((err) => { toast(err.message); return []; });
  const redraw = () => { main.replaceChildren(); drawEmojiSettings(main); loadEmojis(); };
  const name = el("input", { placeholder: "name, like raid_boss", maxlength: 32, "aria-label": "Emoji name" });
  const file = el("input", { type: "file", accept: "image/png,image/gif,image/webp", hidden: true });
  const preview = el("div", { class: "emoji-preview" }, "?");
  let picked = null;
  const choose = el("button", { type: "button", class: "ghost", onclick: () => file.click() }, "Choose picture");
  file.addEventListener("change", () => {
    picked = file.files[0] || null;
    if (!picked) return;
    if (picked.size > 256 * 1024) { toast("That picture is over 256 KB."); picked = null; return; }
    if (!name.value.trim()) name.value = emojiFileName(picked.name);
    preview.replaceChildren(el("img", { src: URL.createObjectURL(picked), alt: "" }));
  });
  const upload = el("button", { class: "primary", onclick: async () => {
    if (!picked) return file.click();
    const n = name.value.trim().replace(/^:|:$/g, "").toLowerCase();
    if (!/^[a-z0-9_]{2,32}$/.test(n) || /^\d+$/.test(n)) return toast("Names are 2–32 letters, numbers or _ (like raid_boss).");
    const form = new FormData();
    form.append("name", n);
    form.append("file", picked, picked.name);
    const res = await tryApi("POST", `/api/guilds/${guildId}/emojis`, form).catch(() => null);
    if (res) { toast(`:${res.name}: is ready.`); redraw(); }
  } }, "Upload");
  main.append(
    el("h2", {}, "Emoji"),
    el("p", { class: "muted", style: "margin:0" }, "Add your own emoji: PNG, GIF or WebP up to 256 KB. Everyone here can use them as ", el("code", {}, ":name:"), " in messages and as reactions, and in their DMs too."),
    el("div", { class: "emoji-upload" }, preview, el("div", { class: "emoji-upload-fields" }, name, el("div", { class: "inline-row" }, choose, upload)), file),
    el("p", { class: "muted small", style: "margin:0" }, `${list.length} of 100 used`),
    ...(list.length ? list.map((e) => el("div", { class: "int-item emoji-item" },
      el("div", { class: "inline-row" }, el("img", { class: "emoji-big", src: e.url, alt: "" }), el("strong", {}, `:${e.name}:`), e.animated ? el("span", { class: "muted small" }, "animated") : null),
      el("div", { class: "inline-row" },
        el("button", { class: "ghost", onclick: async () => {
          const n = await ask("Rename emoji", { value: e.name, confirm: "Save" });
          if (!n || n === e.name) return;
          if (await tryApi("PATCH", `/api/emojis/${e.id}`, { name: n }).catch(() => null)) redraw();
        } }, "Rename"),
        el("button", { class: "ghost danger", onclick: async () => {
          if (!(await confirmBox(`Delete :${e.name}:?`, "Old messages show its name instead of the picture.", "Delete"))) return;
          await tryApi("DELETE", `/api/emojis/${e.id}`).catch(() => {});
          redraw();
        } }, "Delete")))) : [el("p", { class: "muted" }, "No emoji yet.")]),
  );
}

// ======================================================================
// Header button and live updates
// ======================================================================

(function threadsButton() {
  const pins = $("pins-btn");
  pins?.before(el("button", { class: "tool-btn", id: "threads-btn", "data-panel": "threads", title: "Threads", "aria-label": "Threads", onclick: openThreadsList },
    "🧵", el("span", { class: "tool-label" }, "Threads")));
})();

function updateThreadReactions(d, add) {
  const m = thread.messages.find((x) => x.id === d.message_id);
  if (!m) return;
  let r = m.reactions.find((x) => x.emoji === d.emoji);
  if (add) {
    if (!r) m.reactions.push((r = { emoji: d.emoji, user_ids: [] }));
    if (!r.user_ids.includes(d.user_id)) r.user_ids.push(d.user_id);
  } else if (r) {
    r.user_ids = r.user_ids.filter((u) => u !== d.user_id);
    if (!r.user_ids.length) m.reactions = m.reactions.filter((x) => x !== r);
  }
  renderThread();
}

const threadsBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  switch (t) {
    case "MESSAGE_CREATE":
      if (d.thread_id) {
        // Thread replies stay out of the channel itself.
        clearTyping(d.channel_id, d.author_id);
        desktopNotifyMessage(d, state.channels.find((c) => c.id === d.channel_id));
        if (thread.info?.id === d.thread_id && !thread.messages.some((m) => m.id === d.id)) {
          thread.messages.push(d);
          renderThread();
        }
        return;
      }
      break;
    case "MESSAGE_UPDATE": {
      const i = thread.messages.findIndex((m) => m.id === d.id);
      if (i >= 0) { thread.messages[i] = d; renderThread(); }
      break;
    }
    case "MESSAGE_DELETE":
      if (thread.messages.some((m) => m.id === d.id)) {
        thread.messages = thread.messages.filter((m) => m.id !== d.id);
        renderThread();
      }
      break;
    case "REACTION_ADD":
    case "REACTION_REMOVE":
      if (thread.info) updateThreadReactions(d, t === "REACTION_ADD");
      break;
    case "THREAD_UPDATE":
      onThreadUpdate(d);
      break;
    case "THREAD_DELETE":
      onThreadDelete(d);
      break;
    case "EMOJIS_CHANGED":
      loadEmojis();
      break;
  }
  threadsBaseHandle(ev);
};
