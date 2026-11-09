// Outhouse web client: buttons and select menus on bot messages (Discord's
// message components), ephemeral bot replies ("Only you can see this") and
// messages automod hid behind a "Filtered" reveal. Loaded after the other
// chat files; app.js and threads.js call renderComponents() and friends.
"use strict";

const COMPONENT_TIMEOUT_MS = 3000;
const BUTTON_STYLES = { 1: "primary", 2: "secondary", 3: "success", 4: "danger", 5: "link" };

// Interaction ID → resolve(), for clicks waiting on the bot.
const waitingInteractions = new Map();
// Answers that arrived before the click's own request finished.
const answeredInteractions = new Set();
// "messageId:customId" → "loading" | "failed", so redraws keep the state.
const componentState = new Map();
// "messageId:customId" → values picked in a select menu (until the bot answers).
const componentPicks = new Map();
// Filtered messages someone chose to look at.
const filteredRevealed = new Set();
// Ephemeral messages (never saved on the server), kept while the page is open.
const ephemeralKept = [];

const compKey = (m, c) => `${m.id}:${c.custom_id}`;

function componentEmoji(e) {
  if (!e) return null;
  if (e.id && /^\d+$/.test(e.id)) return el("img", { class: "mc-emoji custom-emoji", src: `/emojis/${e.id}`, alt: e.name ? `:${e.name}:` : "", loading: "lazy" });
  return e.name ? el("span", { class: "mc-emoji", "aria-hidden": "true" }, e.name) : null;
}

function renderComponents(m) {
  if (!m.components?.length) return null;
  const failed = m.components.some((row) => row.components.some((c) => componentState.get(compKey(m, c)) === "failed"));
  return el("div", { class: "msg-components" },
    m.components.map((row) => el("div", { class: "mc-row", role: "group" },
      row.components.map((c) => (c.type === 3 ? renderSelectMenu(m, c) : renderComponentButton(m, c))))),
    failed ? el("div", { class: "mc-failed", role: "alert" }, "⚠ This interaction failed") : null);
}

function renderComponentButton(m, c) {
  const style = BUTTON_STYLES[c.style] || "secondary";
  if (c.style === 5) {
    const href = safeHref(c.url);
    return el(href && !c.disabled ? "a" : "span", {
      class: `mc-btn mc-${style}${c.disabled ? " disabled" : ""}`,
      href: href && !c.disabled ? href : null,
      target: "_blank",
      rel: "noopener noreferrer",
      title: href || "",
    }, componentEmoji(c.emoji), c.label ? el("span", { class: "mc-label" }, c.label) : null,
    el("span", { class: "mc-ext", "aria-hidden": "true" }, "↗"));
  }
  const loading = componentState.get(compKey(m, c)) === "loading";
  return el("button", {
    type: "button",
    class: `mc-btn mc-${style}${loading ? " loading" : ""}`,
    disabled: c.disabled || loading,
    "aria-busy": loading ? "true" : null,
    "data-custom-id": c.custom_id,
    onclick: () => useComponent(m, c, []),
  }, loading ? el("span", { class: "mc-spinner", "aria-hidden": "true" }) : null,
  componentEmoji(c.emoji), c.label ? el("span", { class: "mc-label" }, c.label) : null);
}

function selectedValues(m, c) {
  const picked = componentPicks.get(compKey(m, c));
  if (picked) return picked;
  return c.options.filter((o) => o.default).map((o) => o.value);
}

function renderSelectMenu(m, c) {
  const key = compKey(m, c);
  const loading = componentState.get(key) === "loading";
  const values = selectedValues(m, c);
  const chosen = c.options.filter((o) => values.includes(o.value));
  const label = chosen.length
    ? el("span", { class: "mc-select-value" }, chosen.map((o, i) => [i ? ", " : null, componentEmoji(o.emoji), o.label]))
    : el("span", { class: "mc-select-placeholder" }, c.placeholder || (c.max_values > 1 ? "Pick some options" : "Make a selection"));
  const trigger = el("button", {
    type: "button",
    class: `mc-select${loading ? " loading" : ""}`,
    disabled: c.disabled || loading,
    "aria-haspopup": "listbox",
    "aria-expanded": "false",
    "aria-busy": loading ? "true" : null,
    "data-custom-id": c.custom_id,
    onmousedown: (e) => e.stopPropagation(),
    onclick: (e) => toggleSelectMenu(e.currentTarget, m, c),
  }, label, loading ? el("span", { class: "mc-spinner", "aria-hidden": "true" }) : el("span", { class: "mc-caret", "aria-hidden": "true" }, "▾"));
  return trigger;
}

let openSelect = null; // { key, trigger }

function toggleSelectMenu(trigger, m, c) {
  const key = compKey(m, c);
  if (openSelect?.key === key && popover) {
    closePopover();
    openSelect = null;
    return;
  }
  const multi = (c.max_values || 1) > 1;
  const min = c.min_values ?? 1;
  const max = c.max_values ?? 1;
  let picks = new Set(selectedValues(m, c));
  let active = Math.max(0, c.options.findIndex((o) => picks.has(o.value)));
  const listId = `mc-list-${m.id}-${Math.random().toString(36).slice(2, 8)}`;
  const items = c.options.map((o, i) => el("div", {
    class: "mc-option",
    role: "option",
    id: `${listId}-${i}`,
    "aria-selected": "false",
    onclick: () => { active = i; choose(i); },
    onmousemove: () => { if (active !== i) { active = i; paint(); } },
  }, el("span", { class: "mc-check", "aria-hidden": "true" }),
  componentEmoji(o.emoji),
  el("span", { class: "mc-option-text" }, el("span", { class: "mc-option-label" }, o.label),
    o.description ? el("span", { class: "mc-option-desc" }, o.description) : null)));
  const count = el("span", { class: "mc-count muted small" });
  const done = multi ? el("button", { type: "button", class: "primary small mc-done", onclick: () => submit() }, "Done") : null;
  const list = el("div", {
    class: `mc-listbox${multi ? " multi" : ""}`,
    role: "listbox",
    tabindex: "0",
    id: listId,
    "aria-multiselectable": multi ? "true" : "false",
    "aria-label": c.placeholder || "Options",
  }, items);
  const panel = el("div", { class: "mc-menu" }, list, multi ? el("div", { class: "mc-menu-foot" }, count, done) : null);

  function paint() {
    items.forEach((node, i) => {
      const on = picks.has(c.options[i].value);
      node.setAttribute("aria-selected", on ? "true" : "false");
      node.classList.toggle("on", on);
      node.classList.toggle("active", i === active);
    });
    list.setAttribute("aria-activedescendant", `${listId}-${active}`);
    items[active]?.scrollIntoView({ block: "nearest" });
    if (multi) {
      count.textContent = min === max ? `Pick ${min}` : `Pick ${min ? `${min}–` : "up to "}${max}`;
      if (done) done.disabled = picks.size < min || picks.size > max;
    }
  }
  function choose(i) {
    const v = c.options[i].value;
    if (!multi) { picks = new Set([v]); return submit(); }
    if (picks.has(v)) picks.delete(v);
    else if (picks.size < max) picks.add(v);
    paint();
  }
  function submit() {
    const values = c.options.map((o) => o.value).filter((v) => picks.has(v));
    if (values.length < min || values.length > max) return;
    closePopover();
    openSelect = null;
    trigger.setAttribute("aria-expanded", "false");
    componentPicks.set(key, values);
    useComponent(m, c, values);
  }
  list.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { active = Math.min(items.length - 1, active + 1); paint(); e.preventDefault(); }
    else if (e.key === "ArrowUp") { active = Math.max(0, active - 1); paint(); e.preventDefault(); }
    else if (e.key === "Home") { active = 0; paint(); e.preventDefault(); }
    else if (e.key === "End") { active = items.length - 1; paint(); e.preventDefault(); }
    else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      if (multi && e.key === "Enter" && e.ctrlKey) submit();
      else choose(active);
    } else if (e.key === "Tab" && multi && done) { e.preventDefault(); done.focus(); }
    else if (e.key === "Escape") { e.stopPropagation(); closePopover(); openSelect = null; trigger.setAttribute("aria-expanded", "false"); trigger.focus(); }
  });
  const pop = openPopover(trigger, panel, "below");
  pop.classList.add("mc-pop");
  pop.addEventListener("mousedown", (e) => e.stopPropagation());
  // Under the menu if it fits, otherwise above it; as wide as the menu button.
  const r = trigger.getBoundingClientRect();
  pop.style.minWidth = `${Math.min(r.width, window.innerWidth - 16)}px`;
  const pr = pop.getBoundingClientRect();
  const below = r.bottom + 6;
  const y = below + pr.height <= window.innerHeight - 8 ? below : Math.max(8, r.top - pr.height - 6);
  placeFixed(pop, Math.max(8, Math.min(r.left, window.innerWidth - pr.width - 8)), y);
  openSelect = { key, trigger };
  trigger.setAttribute("aria-expanded", "true");
  paint();
  list.focus();
}

function redrawMessage(m) {
  if (m.thread_id && typeof thread !== "undefined" && thread.messages?.some((x) => x.id === m.id)) renderThread();
  else renderMessages();
}

async function useComponent(m, c, values) {
  const key = compKey(m, c);
  if (componentState.get(key) === "loading") return;
  componentState.set(key, "loading");
  redrawMessage(m);
  let id = null;
  let failed = false;
  try {
    const res = await api("POST", `/api/channels/${m.channel_id}/messages/${m.id}/interactions`, { custom_id: c.custom_id, values });
    id = res?.id;
  } catch (err) {
    failed = true;
    toast(err.message);
  }
  if (!failed && id && !answeredInteractions.has(id)) {
    failed = !(await new Promise((resolve) => {
      const timer = setTimeout(() => { waitingInteractions.delete(id); resolve(false); }, COMPONENT_TIMEOUT_MS);
      waitingInteractions.set(id, () => { clearTimeout(timer); resolve(true); });
    }));
  }
  answeredInteractions.delete(id);
  if (failed) {
    componentState.set(key, "failed");
    componentPicks.delete(key);
    setTimeout(() => {
      if (componentState.get(key) === "failed") { componentState.delete(key); redrawMessage(m); }
    }, 6000);
  } else {
    componentState.delete(key);
  }
  redrawMessage(m);
}

// ---- Ephemeral messages ----------------------------------------------------

function ephemeralNote(m) {
  return el("div", { class: "ephemeral-note" },
    el("span", { class: "eph-eye", "aria-hidden": "true" }, "👁"),
    " Only you can see this · ",
    el("button", { type: "button", class: "link eph-dismiss", onclick: () => dismissEphemeral(m) }, "Dismiss"),
    " message");
}

function dismissEphemeral(m) {
  const drop = (list) => { const i = list.findIndex((x) => x.id === m.id); if (i >= 0) list.splice(i, 1); };
  drop(ephemeralKept);
  drop(state.messages);
  if (typeof thread !== "undefined" && thread.messages) drop(thread.messages);
  if (m.thread_id && typeof renderThread === "function") renderThread();
  renderMessages();
}

function addEphemeral(d) {
  const kept = ephemeralKept.findIndex((x) => x.id === d.id);
  if (kept >= 0) ephemeralKept[kept] = d; else ephemeralKept.push(d);
  while (ephemeralKept.length > 50) ephemeralKept.shift();
  if (d.thread_id) {
    if (typeof thread !== "undefined" && thread.info?.id === d.thread_id && !thread.messages.some((x) => x.id === d.id)) {
      thread.messages.push(d);
      renderThread(true);
    }
    return;
  }
  if (d.channel_id === state.channelId && !state.home && !findMessage(d.id)) {
    state.messages.push(d);
    renderMessages.forceBottom = true;
    renderMessages();
  }
}

// After a channel loads, put back your ephemeral messages for it (in order).
function mergeEphemerals() {
  const mine = ephemeralKept.filter((x) => !x.thread_id && x.channel_id === state.channelId && !findMessage(x.id));
  if (!mine.length || !state.messages.length) return;
  const first = state.messages[0].id;
  const fresh = mine.filter((x) => idGt(x.id, first));
  if (!fresh.length) return;
  state.messages.push(...fresh);
  state.messages.sort((a, b) => (idGt(a.id, b.id) ? 1 : idGt(b.id, a.id) ? -1 : 0));
  renderMessages();
}

const compBaseSelectChannel = selectChannel;
selectChannel = async function (id, ...rest) {
  const out = await compBaseSelectChannel(id, ...rest);
  if (state.channelId === id) mergeEphemerals();
  return out;
};

if (typeof openThread === "function") {
  const compBaseRenderThread = renderThread;
  renderThread = function (...args) {
    // Your ephemeral replies in this thread come back when it's reopened.
    if (typeof thread !== "undefined" && thread.info && thread.messages.length) {
      for (const x of ephemeralKept) {
        if (x.thread_id === thread.info.id && !thread.messages.some((y) => y.id === x.id) && idGt(x.id, thread.messages[0].id)) {
          thread.messages.push(x);
          thread.messages.sort((a, b) => (idGt(a.id, b.id) ? 1 : idGt(b.id, a.id) ? -1 : 0));
        }
      }
    }
    return compBaseRenderThread(...args);
  };
}

// ---- Filtered messages (automod) -------------------------------------------

// Hidden behind "Filtered" for everyone but the sender, until clicked.
function isFilteredHidden(m) {
  return !!m.filtered && m.author_id !== state.me?.id && !filteredRevealed.has(m.id);
}

function filteredCover(m) {
  return el("div", { class: "filtered-cover" },
    el("span", { class: "fc-icon", "aria-hidden": "true" }, "🚧"),
    el("span", { class: "fc-text" }, "Automod hid this message."),
    el("button", { type: "button", class: "link fc-show", onclick: () => { filteredRevealed.add(m.id); redrawMessage(m); } }, "Show it"));
}

// The sender sees a small note on their own filtered message.
function filteredNote(m) {
  if (!m.filtered || m.author_id !== state.me?.id) return null;
  return el("div", { class: "filtered-note" }, "🚧 Automod hid this from others behind a “Filtered” notice.");
}

// ---- Live events ---------------------------------------------------------------

const compBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  if (t === "INTERACTION_SUCCESS") {
    const done = waitingInteractions.get(d.id);
    if (done) { waitingInteractions.delete(d.id); done(); } else { answeredInteractions.add(d.id); setTimeout(() => answeredInteractions.delete(d.id), 10000); }
    return;
  }
  if (t === "MESSAGE_CREATE" && d.ephemeral) {
    addEphemeral(d);
    return;
  }
  if (t === "MESSAGE_UPDATE") {
    // The bot changed the message: menus show what it says now.
    for (const key of [...componentPicks.keys()]) if (key.startsWith(`${d.id}:`)) componentPicks.delete(key);
    if (d.ephemeral) {
      const i = ephemeralKept.findIndex((x) => x.id === d.id);
      if (i >= 0) ephemeralKept[i] = d;
    }
  }
  compBaseHandle(ev);
};
