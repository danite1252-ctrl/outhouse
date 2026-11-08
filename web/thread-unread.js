// Outhouse web client: unread replies and @mentions in threads. Threads
// with new replies get a dot (or a red count when someone @mentions you,
// your roles, @everyone or @here) in the channel's thread list, on the
// "N replies" link under a message and on the 🧵 Threads button; mention
// counts add up into the channel, the outhouse icon and the tab title.
// Plain replies don't make the channel itself unread. Opening a thread marks
// it read (on all your devices). Loaded after threads.js and notify.js.
"use strict";

function threadIsUnread(id) {
  const t = state.threadUnread.get(id);
  return !!t && !!t.last_message_id && (!t.last_read_id || idGt(t.last_message_id, t.last_read_id));
}

// The red count (mentions) or the dot (new replies) for a thread.
function threadBadge(id) {
  const t = state.threadUnread.get(id);
  if (t?.mentions) return el("span", { class: "mention-badge thread-badge", title: mentionsTitle(t.mentions) }, countLabel(t.mentions));
  return threadIsUnread(id) ? el("span", { class: "thread-dot", title: "New replies", "aria-label": "New replies" }) : null;
}

function channelThreadState(channelId) {
  let unread = false;
  let mentions = 0;
  for (const [id, t] of state.threadUnread) {
    if (t.channel_id !== channelId) continue;
    if (threadIsUnread(id)) unread = true;
    mentions += t.mentions;
  }
  return { unread, mentions };
}

// Repaints the badges in place (thread links, the thread list, the button).
function paintThreadUnread() {
  for (const node of document.querySelectorAll(".thread-link[data-thread], .thread-card[data-thread]")) {
    const id = node.dataset.thread;
    node.querySelector(".thread-badge, .thread-dot")?.remove();
    node.classList.toggle("unread", threadIsUnread(id));
    const badge = threadBadge(id);
    if (!badge) continue;
    if (node.classList.contains("thread-card")) node.querySelector(".tc-name")?.append(badge);
    else node.querySelector(".tl-go")?.before(badge);
  }
  const btn = $("threads-btn");
  if (btn) {
    btn.querySelector(".tool-badge, .tool-dot")?.remove();
    const { unread, mentions } = state.channelId ? channelThreadState(state.channelId) : { unread: false, mentions: 0 };
    btn.classList.toggle("has-unread", unread);
    if (mentions) btn.append(el("span", { class: "mention-badge tool-badge", title: mentionsTitle(mentions) }, countLabel(mentions)));
    else if (unread) btn.append(el("span", { class: "tool-dot", title: "Threads with new replies" }));
  }
}

const tuBaseRefreshUnreadUi = refreshUnreadUi;
refreshUnreadUi = function () {
  tuBaseRefreshUnreadUi();
  paintThreadUnread();
};

// Messages (and thread lists) are redrawn often: put the badges back.
const tuBaseRenderMessages = renderMessages;
renderMessages = function (...args) {
  const out = tuBaseRenderMessages.apply(this, args);
  paintThreadUnread();
  return out;
};
// You've read a thread up to a reply (here, and the server tells your other devices).
function ackThread(id, lastId) {
  if (!id || !lastId) return;
  const t = state.threadUnread.get(id);
  const wasUnread = !!t;
  if (t) {
    if (!t.last_read_id || idGt(lastId, t.last_read_id)) t.last_read_id = lastId;
    if (!t.last_message_id || !idGt(t.last_message_id, t.last_read_id)) state.threadUnread.delete(id);
  }
  if (wasUnread) refreshUnreadUi();
  api("POST", `/api/threads/${id}/ack`, { message_id: lastId }).catch(() => {});
}

const lastThreadReply = () => (thread.messages.length ? thread.messages[thread.messages.length - 1].id : thread.info?.last_message_id);

// Opening a thread marks it read.
const tuBaseOpenThread = openThread;
openThread = async function (t) {
  await tuBaseOpenThread(t);
  if (thread.info) ackThread(thread.info.id, lastThreadReply());
};

// Coming back to a window with an unread thread open.
window.addEventListener("focus", () => {
  if (thread.info && threadIsUnread(thread.info.id)) ackThread(thread.info.id, lastThreadReply());
});

// A new reply in a thread (not in the channel itself).
function noteThreadMessage(m) {
  const known = state.unread.get(m.channel_id)?.guild_id;
  const guildId = known ?? (state.channels.some((c) => c.id === m.channel_id) ? state.guildId : null);
  if (!guildId || guildId === "0") return;
  if (m.author_id === state.me.id) {
    if (state.threadUnread.delete(m.thread_id)) refreshUnreadUi();
    return;
  }
  // Reading along: it's read straight away.
  if (thread.info?.id === m.thread_id && document.hasFocus() && !document.hidden) {
    ackThread(m.thread_id, m.id);
    return;
  }
  let t = state.threadUnread.get(m.thread_id);
  if (!t) {
    t = { channel_id: m.channel_id, guild_id: guildId, last_message_id: null, last_read_id: null, mentions: 0 };
    state.threadUnread.set(m.thread_id, t);
  }
  t.last_message_id = m.id;
  if (pingsMe(m, guildId)) t.mentions += 1;
  refreshUnreadUi();
}

function onThreadAck(d) {
  const t = state.threadUnread.get(d.thread_id);
  if (!t) return;
  if (!t.last_read_id || idGt(d.last_read_id, t.last_read_id)) t.last_read_id = d.last_read_id;
  if (!t.last_message_id || !idGt(t.last_message_id, t.last_read_id)) state.threadUnread.delete(d.thread_id);
  else if (t.mentions) loadUnread(); // only partly read: recount
  refreshUnreadUi();
}

const tuBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  if (t === "MESSAGE_CREATE" && d.thread_id && state.me) noteThreadMessage(d);
  else if (t === "THREAD_ACK") onThreadAck(d);
  else if (t === "THREAD_DELETE" && state.threadUnread.delete(d.id)) setTimeout(refreshUnreadUi, 0);
  tuBaseHandle(ev);
  if (t === "READY" && state.pendingThread) {
    // Opened from a thread reply's notification.
    const id = state.pendingThread;
    state.pendingThread = null;
    setTimeout(() => openThread(id), 1200);
  }
  if (t === "THREAD_UPDATE") setTimeout(paintThreadUnread, 250);
};
