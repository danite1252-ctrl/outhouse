// Outhouse web client: reporting, the site owner's safety console, and the
// teen "can't message" state. Loaded after the other feature files (uses
// their globals: el, api, tryApi, state, toast, openMenu, MENUS, settingsShell,
// settingsPage, profileCard, friendMenuItems, threadMessage, …).
//
// Anyone can report a message (channels, DMs, threads), a file, a sticker,
// a custom emoji, a person or an outhouse. Reports go to the site owner only;
// the person reported is never told who reported them.
"use strict";

const REPORT_CATEGORIES = [
  ["child_safety", "Child safety", "Sexual content involving a minor, or someone grooming or soliciting a minor"],
  ["sexual", "Sexual or explicit content", "Nudity or sexual content where it doesn't belong"],
  ["harassment", "Harassment or bullying", "Picking on, threatening or targeting someone"],
  ["hate", "Hate or threats of violence", "Attacks on people for who they are, or threats to hurt someone"],
  ["self_harm", "Self-harm", "Someone talking about hurting themselves, or encouraging it"],
  ["spam", "Spam or scams", "Junk, phishing, fake giveaways, gold sellers"],
  ["illegal", "Illegal activity", "Drugs, weapons, hacking or other crimes"],
  ["impersonation", "Impersonation", "Pretending to be someone else"],
  ["other", "Other", "Something else that breaks the rules"],
];
const REPORT_LABEL = Object.fromEntries(REPORT_CATEGORIES.map(([k, l]) => [k, l]));

// Where suspected child abuse gets reported (shown as plain information).
const AUTHORITY_LINKS = [
  ["Australian Centre to Counter Child Exploitation (ACCCE): report online child sexual exploitation", "https://www.accce.gov.au/report"],
  ["eSafety Commissioner: report illegal and restricted online content", "https://www.esafety.gov.au/report"],
  ["Outside Australia (US): NCMEC CyberTipline", "https://report.cybertip.org"],
];

state.safety = { owner: false, open: 0, urgent: 0, inReview: 0 };

// ======================================================================
// The report dialog
// ======================================================================

// target: { kind: message|attachment|user|sticker|emoji|guild, id, title, userId?, userName? }
function openReportDialog({ kind, id, title, userId = null, userName = "" }) {
  closeMenus();
  closePopover();
  let picked = null;
  const send = el("button", { type: "submit", class: "primary", disabled: true }, "Send report");
  const urgent = el("p", { class: "rp-urgent", hidden: true },
    "Thank you for speaking up. Child-safety reports go straight to the top of the owner's list, and any pictures or files are locked away at once so nobody else sees them. If a child is in danger right now, call 000 (or your local emergency number).");
  const options = REPORT_CATEGORIES.map(([key, label, hint]) => {
    const radio = el("input", { type: "radio", name: "report-category", value: key });
    radio.addEventListener("change", () => {
      picked = key;
      send.disabled = false;
      urgent.hidden = key !== "child_safety";
    });
    return el("label", { class: `rp-option${key === "child_safety" ? " rp-child" : ""}` }, radio,
      el("span", { class: "rp-text" }, el("strong", {}, label), el("span", { class: "rp-hint" }, hint)));
  });
  const details = el("textarea", { class: "rp-details", maxlength: 1000, rows: 3, placeholder: "Anything that would help (optional)", "aria-label": "Details" });
  const counter = el("span", { class: "rp-count" }, "0 / 1000");
  details.addEventListener("input", () => { counter.textContent = `${details.value.length} / 1000`; });
  const canBlock = userId && userId !== state.me?.id && !(typeof isBlocked === "function" && isBlocked(userId));
  const blockBox = canBlock ? el("input", { type: "checkbox", class: "rp-block" }) : null;
  const error = el("p", { class: "error", role: "alert" });
  const dialog = el("dialog", { class: "report-dialog" },
    el("form", { method: "dialog" },
      el("h2", {}, title || "Report"),
      el("p", { class: "muted small rp-intro" }, "Reports go to the person who runs this Outhouse server. Whoever you report isn't told who reported them."),
      el("div", { class: "rp-options", role: "radiogroup", "aria-label": "What's wrong?" }, options),
      urgent,
      el("label", { class: "field" }, "Details", details, counter),
      blockBox ? el("label", { class: "rp-check" }, blockBox, `Block ${userName || "this person"} too`) : null,
      error,
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"),
        send)));
  dialog.querySelector("form").addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!picked) return;
    send.disabled = true;
    error.textContent = "";
    try {
      await api("POST", "/api/reports", { target_kind: kind, target_id: String(id), category: picked, details: details.value.trim() || null });
    } catch (err) {
      error.textContent = err.message;
      send.disabled = false;
      return;
    }
    dialog.close();
    if (blockBox?.checked) {
      const r = await api("PUT", `/api/users/@me/blocks/${userId}`).catch(() => null);
      if (r && state.relationships) {
        state.relationships.set(userId, r);
        if (typeof afterRelationshipChange === "function") afterRelationshipChange();
      }
    }
    toast(blockBox?.checked ? `Thanks. Your report was sent and ${userName || "they"} ${userName ? "is" : "are"} blocked.` : "Thanks. Your report was sent to the server owner.");
  });
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
}

// ---------- where you can report from ----------

const isCustomSticker = (s) => s && s.id && !String(s.id).startsWith("builtin");

function messageReportItems(m, e) {
  if (!m || m.ephemeral || m.author_id === state.me?.id) return [];
  const who = authorName(m);
  const base = { userId: m.author_bot ? null : m.author_id, userName: who };
  const items = [{ label: "Report message", danger: true, onClick: () => openReportDialog({ ...base, kind: "message", id: m.id, title: `Report ${who}'s message` }) }];
  const fileNode = e?.target?.closest?.(".attachments > *");
  if (fileNode && m.attachments?.length) {
    const i = [...fileNode.parentElement.children].indexOf(fileNode);
    const a = m.attachments[i];
    if (a) items.push({ label: "Report this file", danger: true, onClick: () => openReportDialog({ ...base, kind: "attachment", id: a.id, title: `Report ${a.filename}` }) });
  } else if (m.attachments?.length > 1) {
    items.push({ label: "Report a file", submenu: m.attachments.map((a) => ({ label: a.filename, onClick: () => openReportDialog({ ...base, kind: "attachment", id: a.id, title: `Report ${a.filename}` }) })) });
  } else if (m.attachments?.length === 1) {
    const a = m.attachments[0];
    items.push({ label: "Report this file", danger: true, onClick: () => openReportDialog({ ...base, kind: "attachment", id: a.id, title: `Report ${a.filename}` }) });
  }
  if (isCustomSticker(m.sticker)) {
    items.push({ label: "Report sticker", danger: true, onClick: () => openReportDialog({ kind: "sticker", id: m.sticker.id, title: `Report the sticker “${m.sticker.name}”` }) });
  }
  const emoji = e?.target?.closest?.("img")?.getAttribute("src")?.match(/^\/emojis\/(\d+)/);
  if (emoji) items.push({ label: "Report emoji", danger: true, onClick: () => openReportDialog({ kind: "emoji", id: emoji[1], title: "Report this emoji" }) });
  return items;
}

// Wraps a menu so extra items go in before `beforeLabel` (or at the end).
function withExtraMenuItems(base, extraFor, beforeLabel) {
  return function (e, node) {
    const extra = extraFor(e, node);
    if (!extra || !extra.length) return base(e, node);
    const realOpen = openMenu;
    openMenu = (x, y, items, depth) => {
      openMenu = realOpen;
      const at = items.findIndex((it) => it && it.label && beforeLabel(it.label));
      if (at >= 0) items.splice(at, 0, ...extra, "sep");
      else items.push("sep", ...extra);
      return realOpen(x, y, items, depth);
    };
    try { return base(e, node); } finally { openMenu = realOpen; }
  };
}

MENUS.message = withExtraMenuItems(MENUS.message,
  (e, node) => messageReportItems(findMessage(node.dataset.id), e),
  (label) => label === "Copy message ID");

MENUS.guild = withExtraMenuItems(MENUS.guild, (e, node) => {
  const g = state.guilds.find((x) => x.id === node.dataset.id);
  if (!g || g.owner_id === state.me?.id) return [];
  return [{ label: "Report outhouse", danger: true, onClick: () => openReportDialog({ kind: "guild", id: g.id, title: `Report ${g.name}` }) }];
}, (label) => label === "Copy outhouse ID");

// The thread panel's right-click menu (parity.js).
if (MENUS.threadMessage) {
  MENUS.threadMessage = withExtraMenuItems(MENUS.threadMessage,
    (e, node) => messageReportItems(typeof threadMsgById === "function" ? threadMsgById(node.dataset.id) : null, e),
    (label) => label === "Copy message ID");
}

// People: member menus, the ⋯ on profile cards and DM menus all use these.
if (typeof friendMenuItems === "function") {
  const safetyBaseFriendItems = friendMenuItems;
  friendMenuItems = function (userId, name, bot = false) {
    const items = safetyBaseFriendItems(userId, name, bot);
    if (!userId || userId === state.me?.id || bot) return items;
    return [...items, { label: "Report…", danger: true, onClick: () => openReportDialog({ kind: "user", id: userId, userId, userName: name, title: `Report ${name}` }) }];
  };
}

// Thread replies get a report button too.
if (typeof threadMessage === "function") {
  const safetyBaseThreadMessage = threadMessage;
  threadMessage = function (m, prev) {
    const node = safetyBaseThreadMessage(m, prev);
    if (m.ephemeral || m.author_id === state.me?.id) return node;
    const bar = node.querySelector(".msg-actions");
    bar?.append(el("button", { class: "report-btn", title: "Report", "aria-label": "Report message", onclick: (e) => {
      const r = e.currentTarget.getBoundingClientRect();
      openMenu(r.left, r.bottom + 4, messageReportItems(m, null));
    } }, "⚑"));
    return node;
  };
}

// Teens: "This person can't receive messages from you" on profile cards.
const safetyBaseProfileCard = profileCard;
profileCard = function (p, opts = {}) {
  const card = safetyBaseProfileCard(p, opts);
  // Full profiles carry it already (profiles.js shows it under the message box).
  if (opts.preview || p.bot || p.id === state.me?.id || "contact" in p) return card;
  const btn = card.querySelector(".pc-message");
  if (!btn) return card;
  api("GET", `/api/users/${p.id}/contact`).then((c) => {
    if (!c || c.can_message) return;
    btn.disabled = true;
    btn.title = c.reason || "";
    if (!card.querySelector(".pc-cant")) card.querySelector(".pc-actions")?.before(el("p", { class: "pc-cant" }, "🔒 ", c.reason));
  }).catch(() => {});
  return card;
};

// ======================================================================
// The site owner: badge, alerts, console
// ======================================================================

function svgIcon(paths, size = 22) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  for (const [k, v] of Object.entries({ width: size, height: size, viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", "stroke-width": 2, "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" })) svg.setAttribute(k, v);
  for (const d of paths) {
    const p = document.createElementNS(ns, "path");
    p.setAttribute("d", d);
    svg.append(p);
  }
  return svg;
}
const shieldIcon = (size) => svgIcon(["M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z", "M12 8v5", "M12 16h.01"], size);

function renderSafetyBadge() {
  let btn = document.getElementById("safety-btn");
  if (!state.safety.owner) { btn?.remove(); return; }
  if (!btn) {
    btn = el("button", { id: "safety-btn", class: "rail-btn safety", title: "Reports", "aria-label": "Reports", onclick: () => openSafetyConsole() }, shieldIcon());
    document.getElementById("join-guild")?.after(btn);
  }
  const { open, urgent } = state.safety;
  btn.classList.toggle("urgent", urgent > 0);
  btn.title = urgent ? `Reports: ${urgent} child-safety report${urgent === 1 ? "" : "s"} waiting` : open ? `Reports: ${open} waiting` : "Reports";
  btn.querySelector(".mention-badge")?.remove();
  if (open || urgent) btn.append(el("span", { class: `mention-badge rail-badge${urgent ? " urgent" : ""}` }, String(Math.max(open, urgent))));
}

async function loadSafetySummary() {
  const owner = await api("GET", "/api/users/@me/site-owner").then((r) => !!r?.site_owner).catch(() => false);
  state.safety.owner = owner;
  if (owner) {
    const c = await api("GET", "/api/safety/summary").catch(() => null);
    if (c) Object.assign(state.safety, { open: c.open, urgent: c.urgent, inReview: c.in_review });
  }
  renderSafetyBadge();
  if (owner && state.openReportsOnReady) {
    state.openReportsOnReady = false;
    openSafetyConsole();
  }
}

function safetyAlert() {
  toast("⚠ Urgent: a child-safety report is waiting for you in Reports.");
  const title = "Urgent safety report";
  const body = "A child-safety report needs your review.";
  if (typeof desktop !== "undefined" && desktop && typeof desktopInvoke === "function") {
    desktopInvoke("notify", { title, body });
  } else if ("Notification" in window && Notification.permission === "granted" && !document.hasFocus()) {
    try { new Notification(title, { body, tag: "safety-report", icon: "/icons/icon-192.png" }); } catch { /* not allowed here */ }
  }
}

const safetyBaseHandle = handleEvent;
handleEvent = function (ev) {
  safetyBaseHandle(ev);
  if (ev.t === "READY") loadSafetySummary();
  if (ev.t === "REPORTS_CHANGED") {
    Object.assign(state.safety, { owner: true, open: ev.d.open, urgent: ev.d.urgent });
    renderSafetyBadge();
    if (ev.d.alert) safetyAlert();
    const open = document.querySelector("dialog.safety-console");
    if (open && open.dataset.view === "list") open.dispatchEvent(new Event("safety-refresh"));
  }
};

// A notification about a report opens the console.
(() => {
  const q = new URLSearchParams(location.search);
  if (!q.has("reports")) return;
  state.openReportsOnReady = true;
  q.delete("reports");
  const rest = q.toString();
  history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : ""));
})();
if (typeof openTarget === "function") {
  const safetyBaseOpenTarget = openTarget;
  openTarget = function (target) {
    if (target?.channel_id === "reports") return openSafetyConsole();
    return safetyBaseOpenTarget(target);
  };
}

// ---------- small helpers ----------

const scWhen = (iso) => (iso ? new Date(iso).toLocaleString([], { day: "numeric", month: "short", year: "numeric", hour: "numeric", minute: "2-digit" }) : "");
const scPlural = (n, word) => `${n} ${word}${n === 1 ? "" : "s"}`;
const SC_STATUS = { open: "Open", review: "In review", closed: "Closed" };

function catChip(r) {
  return el("span", { class: `rp-chip cat-${r.category}` }, r.child_safety ? "⚠ " : "", r.category_label || REPORT_LABEL[r.category] || r.category);
}

function scSection(title, ...children) {
  return el("section", { class: "sc-section" }, el("h3", {}, title), ...children);
}

// A small form in a dialog. fields: [{ name, label, type: text|textarea|select|checkbox, options: [[value, label]], value, placeholder }]
function formDialog(title, fields, confirm, { intro = null, danger = false } = {}) {
  return new Promise((resolve) => {
    const inputs = {};
    const rows = fields.map((f) => {
      let input;
      if (f.type === "select") input = el("select", {}, f.options.map(([v, l]) => el("option", { value: v, selected: v === f.value }, l)));
      else if (f.type === "textarea") input = el("textarea", { rows: 3, maxlength: f.max || 1500, placeholder: f.placeholder || "" });
      else if (f.type === "checkbox") input = el("input", { type: "checkbox", checked: !!f.value });
      else input = el("input", { type: "text", maxlength: f.max || 500, placeholder: f.placeholder || "", value: f.value || "" });
      inputs[f.name] = input;
      return f.type === "checkbox" ? el("label", { class: "rp-check" }, input, f.label) : el("label", { class: "field" }, f.label, input);
    });
    const dialog = el("dialog", { class: "report-dialog sc-form" },
      el("form", { method: "dialog" },
        el("h2", {}, title),
        intro ? el("p", { class: "muted small", style: "margin:0" }, intro) : null,
        rows,
        el("div", { class: "row" },
          el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
          el("button", { type: "submit", class: danger ? "primary danger-fill" : "primary", value: "ok" }, confirm))));
    dialog.addEventListener("close", () => {
      const ok = dialog.returnValue === "ok";
      const out = {};
      for (const [k, input] of Object.entries(inputs)) out[k] = input.type === "checkbox" ? input.checked : input.value.trim();
      dialog.remove();
      resolve(ok ? out : null);
    });
    document.body.append(dialog);
    dialog.showModal();
  });
}

// ---------- the console ----------

async function openSafetyConsole(tab = "open", reportId = null, openPerson = null) {
  closeMenus();
  closePopover();
  document.querySelectorAll("dialog.settings").forEach((d) => d.close());
  const { dialog, nav, main } = settingsShell("Safety");
  dialog.classList.add("safety-console");
  let drawing = 0;
  let personId = openPerson; // People opens on this person
  const navBtn = (key, label, current) => el("button", { class: current === key ? "active" : "", "data-tab": key, onclick: () => draw(key) }, label);
  const draw = async (key, id = null) => {
    const mine = ++drawing;
    dialog.dataset.view = id ? "detail" : ["open", "review", "closed"].includes(key) ? "list" : key;
    nav.replaceChildren(
      el("h3", {}, "REPORTS"),
      navBtn("open", "Open", id ? null : key), navBtn("review", "In review", id ? null : key), navBtn("closed", "Closed", id ? null : key),
      el("h3", {}, "SAFETY"),
      navBtn("people", "People", key), navBtn("teens", "Teen protections", key),
      typeof drawDetectorSettings === "function" ? navBtn("detector", "Image detector", key) : null,
      navBtn("digest", "This week", key),
      navBtn("log", "Safety log", key), navBtn("police", "Reporting to police", key));
    const page = settingsPage(main);
    const stillMine = () => mine === drawing;
    if (id) return drawReportDetail(page, id, { back: () => draw(key), stillMine, openUser: (uid) => { personId = uid; draw("people"); } });
    if (key === "people") {
      const userId = personId;
      personId = null;
      return drawPeople(page, { stillMine, openReport: (rid) => draw("open", rid), userId });
    }
    if (key === "teens") return drawTeenSettings(page, stillMine);
    if (key === "detector" && typeof drawDetectorSettings === "function") return drawDetectorSettings(page, stillMine);
    if (key === "digest") return drawDigest(page, stillMine);
    if (key === "log") return drawSafetyLog(page, stillMine);
    if (key === "police") return drawPoliceGuide(page);
    return drawReportList(page, key, { stillMine, openReport: (rid) => draw(key, rid) });
  };
  dialog.addEventListener("safety-refresh", () => { if (dialog.dataset.view === "list") draw(nav.querySelector("button.active")?.dataset.tab || "open"); });
  draw(tab, reportId);
  return dialog;
}

function reportRow(r, onOpen) {
  return el("button", { class: `sc-row${r.child_safety ? " child" : ""}`, "data-report": r.id, onclick: () => onOpen(r.id) },
    el("div", { class: "sc-row-top" }, catChip(r),
      r.report_count > 1 ? el("span", { class: "sc-count", title: "Reports of this" }, `×${r.report_count}`) : null,
      el("span", { class: "sc-when" }, scWhen(r.created_at))),
    el("div", { class: "sc-row-who" },
      r.reported_user ? el("strong", {}, r.reported_user.display_name || "Deleted account") : null,
      r.reported_user?.username ? el("span", { class: "muted" }, ` @${r.reported_user.username}`) : null,
      r.place ? el("span", { class: "muted" }, ` · ${r.place}`) : null),
    r.preview ? el("div", { class: "sc-row-preview" }, r.preview) : null,
    r.status === "closed" && r.resolution ? el("div", { class: "sc-row-res" }, `✓ ${r.resolution}`) : null);
}

async function drawReportList(page, status, { stillMine, openReport }) {
  const data = await tryApi("GET", `/api/safety/reports?status=${status}`).catch(() => null);
  if (!data || !stillMine()) return;
  Object.assign(state.safety, { open: data.counts.open, urgent: data.counts.urgent, inReview: data.counts.in_review });
  renderSafetyBadge();
  const titles = { open: "Reports", review: "In review", closed: "Closed reports" };
  const empty = { open: "Nothing waiting. All quiet out the back.", review: "Nothing in review.", closed: "No closed reports yet." };
  page.replaceChildren(
    el("h2", {}, titles[status]),
    el("p", { class: "muted small", style: "margin:0" },
      status === "closed" ? "The most recently closed first." : "Child-safety reports first, then the most serious, newest first. Only you (the site owner) can see these."),
    status !== "closed" ? el("div", { class: "sc-counts" },
      el("span", {}, el("strong", {}, String(data.counts.open)), " open"),
      el("span", {}, el("strong", {}, String(data.counts.in_review)), " in review"),
      data.counts.urgent ? el("span", { class: "sc-urgent" }, el("strong", {}, String(data.counts.urgent)), " child safety") : null) : null,
    data.reports.length
      ? el("div", { class: "sc-list" }, data.reports.map((r) => reportRow(r, openReport)))
      : el("div", { class: "sp-empty" }, el("div", { class: "sp-empty-icon" }, "🛡"), el("p", { class: "muted" }, empty[status])));
}

// The warning shown on every child-safety report.
function childSafetyNotice() {
  return el("div", { class: "sc-child-warning", role: "note" },
    el("strong", {}, "Suspected child sexual abuse material: don't look, don't share."),
    el("p", {}, "Pictures and videos from this report are locked away and are never shown here. Do not view, download, copy or forward suspected child sexual abuse material, not even to report it. Preserve it (Outhouse keeps the files unchanged on the server) and report it to the authorities:"),
    el("ul", {},
      AUTHORITY_LINKS.slice(0, 2).map(([label, href]) => el("li", {}, label, " — ", el("a", { href, target: "_blank", rel: "noopener noreferrer" }, href))),
      el("li", {}, el("strong", {}, "If a child is in immediate danger, call 000.")),
      el("li", {}, AUTHORITY_LINKS[2][0], " — ", el("a", { href: AUTHORITY_LINKS[2][1], target: "_blank", rel: "noopener noreferrer" }, AUTHORITY_LINKS[2][1]), ", or your local police.")),
    el("p", {}, "“Export evidence package” gives police the details and SHA-256 hashes without the files. See “Reporting to police” for how they can get the files."));
}

function snapshotView(d) {
  const s = d.snapshot || {};
  const r = d.report;
  if (s.kind === "message") {
    const m = s.message || {};
    const focus = s.focus_attachment_id;
    return [
      el("div", { class: "sc-meta" },
        el("strong", {}, m.author?.display_name || "?"), el("span", { class: "muted" }, ` @${m.author?.username || "?"}`),
        m.author?.bot ? el("span", { class: "bot-tag" }, "BOT") : null,
        el("span", { class: "muted" }, ` · ${scWhen(m.created_at)}`),
        m.edited_at ? el("span", { class: "muted" }, ` · edited ${scWhen(m.edited_at)}`) : null),
      el("div", { class: "muted small" }, r.place || "",
        s.place?.dm && s.place.participants?.length ? ` (${s.place.participants.join(", ")})` : ""),
      m.content ? el("div", { class: "sc-content" }, m.content) : el("div", { class: "muted small" }, "(no text)"),
      m.sticker ? el("div", { class: "muted small" }, `Sticker: ${m.sticker.name || ""}`) : null,
      m.forward ? el("div", { class: "muted small" }, `Forwarded from ${m.forward.author_display_name || "someone"}: ${m.forward.content || ""}`) : null,
      focus ? el("div", { class: "muted small" }, "This report is about one of its files.") : null,
      s.edits?.length ? el("div", { class: "sc-edits" }, el("strong", {}, "Edited after it was reported:"),
        s.edits.map((e) => el("div", { class: "sc-edit" }, el("span", { class: "muted small" }, scWhen(e.at)), el("div", {}, el("del", {}, e.before)), el("div", {}, e.after)))) : null,
      s.deleted ? el("div", { class: "sc-deleted" }, `Deleted ${scWhen(s.deleted.at)}${s.deleted.by === m.author?.id ? " by its author" : ""}. The snapshot above is what it said.`) : null,
      !d.message_live && !s.deleted ? el("div", { class: "sc-deleted" }, "The message is gone now. The snapshot above is what it said.") : null,
      d.message_hidden ? el("div", { class: "sc-hidden-note" }, "🔒 Hidden from everyone pending your review.") : null,
    ];
  }
  if (s.kind === "user") {
    const u = s.user || {};
    return [el("div", { class: "sc-meta" }, el("strong", {}, u.display_name), el("span", { class: "muted" }, ` @${u.username}`), el("span", { class: "muted" }, ` · joined ${scWhen(u.created_at)}`)),
      u.about_me ? el("div", { class: "sc-content" }, u.about_me) : el("div", { class: "muted small" }, "(no “about me”)")];
  }
  if (s.kind === "sticker" || s.kind === "emoji") {
    return [el("div", { class: "sc-meta" }, el("strong", {}, `${s.kind === "sticker" ? "Sticker" : "Emoji"} :${s.art?.name || ""}:`), el("span", { class: "muted" }, ` in ${s.place?.guild_name || "?"} · added ${scWhen(s.art?.created_at)}`))];
  }
  if (s.kind === "guild") {
    const g = s.guild || {};
    return [el("div", { class: "sc-meta" }, el("strong", {}, g.name), el("span", { class: "muted" }, ` · ${scPlural(g.members || 0, "member")} · made ${scWhen(g.created_at)}`)),
      g.channels?.length ? el("div", { class: "muted small" }, `Channels: ${g.channels.map((c) => `#${c}`).join(", ")}`) : null];
  }
  return [];
}

function filesView(d, reportId) {
  if (!d.files.length) return null;
  const child = d.report.child_safety;
  return el("div", { class: "sc-files" }, d.files.map((f) => {
    const preview = el("div", { class: "sc-file-preview" });
    const media = /^(image|video)\//.test(f.content_type);
    const status = f.quarantined ? "Quarantined: nobody can download it" : f.on_disk ? "Kept on the server" : "Gone from the server";
    if (child && media) preview.append(el("div", { class: "sc-locked" }, "🔒 Not shown: child-safety evidence is never displayed here."));
    const show = f.viewable ? el("button", { class: "ghost", onclick: async (e) => {
      e.currentTarget.disabled = true;
      try {
        const res = await fetch(`/api/safety/reports/${reportId}/files/${f.key}`, { headers: { Authorization: `Bearer ${state.token}` } });
        if (!res.ok) throw new Error(`Couldn't load it (${res.status})`);
        const url = URL.createObjectURL(await res.blob());
        if (f.content_type.startsWith("image/")) preview.replaceChildren(el("img", { src: url, alt: f.filename }));
        else if (f.content_type.startsWith("video/")) preview.replaceChildren(el("video", { src: url, controls: true }));
        else preview.replaceChildren(el("a", { href: url, download: f.filename }, `Save ${f.filename}`));
      } catch (err) { toast(err.message); }
    } }, "Show") : null;
    return el("div", { class: "sc-file" },
      el("div", { class: "sc-file-head" }, el("strong", {}, f.filename), el("span", { class: "muted small" }, ` ${f.content_type} · ${fmtSize(f.size)}`), show),
      el("div", { class: "sc-hash" }, "SHA-256 ", el("code", {}, f.sha256 || "unknown")),
      el("div", { class: `sc-file-status${f.quarantined ? " q" : ""}` }, status),
      preview);
  }));
}

function contextView(d) {
  const ctx = d.snapshot?.context;
  if (!ctx?.length) return null;
  const m = d.snapshot.message;
  const line = (c, reported) => el("div", { class: `sc-ctx${reported ? " reported" : ""}` },
    el("span", { class: "sc-ctx-time" }, scWhen(c.created_at)),
    el("strong", {}, c.author_name || c.author?.display_name || "?"),
    el("span", { class: "sc-ctx-text" }, c.content || (c.files ? `(${scPlural(c.files, "file")})` : ""), c.files && c.content ? ` (+${scPlural(c.files, "file")})` : ""));
  return el("div", { class: "sc-context" },
    ctx.filter((c) => c.before).map((c) => line(c, false)),
    line({ ...m, author_name: m.author?.display_name }, true),
    ctx.filter((c) => !c.before).map((c) => line(c, false)));
}

function logView(entries) {
  if (!entries?.length) return el("p", { class: "muted small", style: "margin:0" }, "Nothing yet.");
  return el("div", { class: "audit-list" }, entries.map((e) => el("div", { class: `audit-row${e.action === "note" ? " note" : ""}` },
    el("div", { class: "audit-icon" }, { note: "📝", quarantine: "🔒", suspend: "⛔", warn: "⚠", ban: "🚫", kick: "👢", timeout: "⏳", dismiss: "✖", close: "✓", export: "📦" }[e.action] || "•"),
    el("div", { class: "audit-main" },
      el("div", { class: "audit-line" }, el("strong", {}, e.actor_name), el("span", { class: "audit-action" }, ` ${e.action.replace(/_/g, " ")}`), e.target_name ? el("span", { class: "audit-action" }, ` · ${e.target_name}`) : null),
      e.details ? el("div", { class: "audit-details" }, e.details) : null),
    el("time", { class: "audit-time", title: new Date(e.created_at).toLocaleString() }, scWhen(e.created_at)))));
}

// Someone's history and the account actions (used on reports and People).
function subjectView(u, act) {
  const others = (u.reports || []);
  const ageSel = el("select", { "aria-label": "Age group" }, el("option", { value: "adult", selected: u.age_bracket === "adult" }, "Adult (18+)"), el("option", { value: "teen", selected: u.age_bracket === "teen" }, "Teen (13–17)"));
  ageSel.addEventListener("change", () => act({ action: "set_age", bracket: ageSel.value }, `Age group set to ${ageSel.value}.`));
  const item = (label, value, cls = "") => el("div", { class: `status-item ${cls}` }, el("div", { class: "status-label" }, label), el("div", { class: "status-value" }, value));
  return el("div", { class: "sc-subject" },
    el("div", { class: "sc-meta" }, el("strong", {}, u.display_name), el("span", { class: "muted" }, ` @${u.username}`),
      u.site_owner ? el("span", { class: "rp-chip" }, "Site owner") : null,
      u.suspended ? el("span", { class: "rp-chip cat-child_safety" }, `Suspended ${u.suspended_label}`) : null),
    u.suspended && u.suspended_reason ? el("div", { class: "muted small" }, `Reason: ${u.suspended_reason}`) : null,
    el("div", { class: "status-grid" },
      item("Account age", `${scPlural(u.account_age_days, "day")} (since ${new Date(u.created_at).toLocaleDateString()})`),
      item("Age group", ageSel),
      item("Messages (30 days)", String(u.messages_30d)),
      item("Hidden messages", String(u.hidden_messages), u.hidden_messages ? "warn" : ""),
      item("Reports about them", String(others.length), others.length > 1 ? "warn" : ""),
      item("Reports they made", String(u.reports_made)),
      item("Bots they own", String(u.bots)),
      u.message_requests ? item("Message requests sent",
        `${u.message_requests.sent} (${u.message_requests.ignored} ignored, ${u.message_requests.blocked} blocked, ${u.message_requests.accepted} accepted; ${u.message_requests.last_day} today)`,
        u.message_requests.ignored + u.message_requests.blocked >= 3 ? "warn" : "") : null,
      item("IP addresses", u.ip_note, "unknown")),
    el("div", { class: "muted small" }, "Outhouses: ", u.outhouses.length ? u.outhouses.map((o) => `${o.name}${o.owner ? " (owner)" : ""}${o.family ? " (family)" : ""}${o.timeout_until ? " (timed out)" : ""}`).join(", ") : "none"),
    others.length ? el("div", { class: "muted small" }, "Reports: ", others.slice(0, 10).map((o) => `${o.category_label} (${SC_STATUS[o.status] || o.status}, ${new Date(o.created_at).toLocaleDateString()})`).join(" · ")) : null,
    u.names ? el("details", { class: "sc-names" }, el("summary", {}, `Names they've gone by (${u.names.length})`),
      typeof nameHistoryList === "function" ? nameHistoryList(u.names, "No name changes recorded.") : null) : null);
}

// Buttons for the account actions; `act(body, done)` posts one.
function accountActions(u, act, guildId = null) {
  if (u.site_owner) return el("p", { class: "muted small", style: "margin:0" }, "Site owners can't be warned, timed out or suspended here.");
  const outhouses = u.outhouses.filter((o) => !o.owner);
  const pickOuthouse = () => ({ name: "guild_id", label: "Outhouse", type: "select", value: guildId, options: outhouses.map((o) => [o.id, o.name]) });
  return el("div", { class: "sc-actions" },
    el("button", { class: "ghost", onclick: async () => {
      const f = await formDialog(`Warn ${u.display_name}`, [{ name: "text", label: "Warning (sent as a DM from Outhouse Safety; it doesn't say who reported them)", type: "textarea", placeholder: "Please don't post that here." }], "Send warning");
      if (f?.text) act({ action: "warn", text: f.text }, "Warning sent.");
    } }, "Warn…"),
    outhouses.length ? el("button", { class: "ghost", onclick: async () => {
      const f = await formDialog(`Time out ${u.display_name}`, [pickOuthouse(), { name: "minutes", label: "For", type: "select", value: "60", options: [["10", "10 minutes"], ["60", "1 hour"], ["1440", "1 day"], ["10080", "1 week"]] }], "Time out");
      if (f) act({ action: "timeout", guild_id: f.guild_id, minutes: Number(f.minutes) }, "Timed out.");
    } }, "Time out…") : null,
    outhouses.length ? el("button", { class: "ghost", onclick: async () => {
      const f = await formDialog(`Kick ${u.display_name}`, [pickOuthouse()], "Kick", { danger: true });
      if (f) act({ action: "kick", guild_id: f.guild_id }, "Kicked.");
    } }, "Kick…") : null,
    outhouses.length ? el("button", { class: "ghost", onclick: async () => {
      const f = await formDialog(`Ban ${u.display_name}`, [pickOuthouse(), { name: "text", label: "Reason (shown in the outhouse's ban list)", type: "text" }], "Ban", { danger: true });
      if (f) act({ action: "ban", guild_id: f.guild_id, text: f.text }, "Banned.");
    } }, "Ban…") : null,
    u.suspended
      ? el("button", { class: "ghost", onclick: async () => {
        if (await confirmBox(`Lift ${u.display_name}'s suspension?`, "They can sign in again. Their bots' tokens stay revoked until they reset them, and hidden messages stay hidden unless you show them again.", "Lift suspension")) act({ action: "unsuspend" }, "Suspension lifted.");
      } }, "Lift suspension")
      : el("button", { class: "danger-btn sc-suspend", onclick: async () => {
        const f = await formDialog(`Suspend ${u.display_name}`, [
          { name: "days", label: "For", type: "select", value: "", options: [["1", "1 day"], ["3", "3 days"], ["7", "1 week"], ["30", "30 days"], ["90", "90 days"], ["", "Permanently"]] },
          { name: "text", label: "Reason (they see it when they try to sign in)", type: "text" },
          { name: "hide", label: "Also hide all their messages from everyone (kept for evidence)", type: "checkbox" },
        ], "Suspend account", { danger: true, intro: "They're signed out everywhere at once, can't sign in, their bots stop working and their profile is hidden. Their messages stay (for evidence) unless you hide them." });
        if (f) act({ action: "suspend", days: f.days ? Number(f.days) : null, text: f.text, hide_messages: f.hide }, "Account suspended.");
      } }, "Suspend account…"),
    u.hidden_messages
      ? el("button", { class: "ghost", onclick: () => act({ action: "unhide_messages" }, "Their messages are back (child-safety evidence stays hidden).") }, "Show their messages again")
      : el("button", { class: "ghost", onclick: async () => {
        if (await confirmBox(`Hide all of ${u.display_name}'s messages?`, "Everyone stops seeing them, everywhere. They're kept for evidence and you can show them again.", "Hide messages")) act({ action: "hide_messages" }, "Their messages are hidden.");
      } }, "Hide all their messages"));
}

function noteBox(act) {
  const text = el("textarea", { rows: 2, maxlength: 2000, placeholder: "A private note (only site owners see these)", "aria-label": "Private note" });
  return el("div", { class: "sc-note" }, text, el("button", { class: "ghost", onclick: () => { if (text.value.trim()) act({ action: "note", text: text.value.trim() }, "Note added."); } }, "Add note"));
}

async function exportEvidence(id) {
  const ok = await confirmBox("Export the evidence package?",
    "This downloads a JSON file with the report, the snapshot, who reported it, the account's history and the files' names and SHA-256 hashes. It never contains the files themselves. Keep it private and only give it to police or the eSafety Commissioner.", "Download");
  if (!ok) return;
  const res = await fetch(`/api/safety/reports/${id}/export`, { headers: { Authorization: `Bearer ${state.token}` } }).catch(() => null);
  if (!res?.ok) return toast("Couldn't export it.");
  const url = URL.createObjectURL(await res.blob());
  const a = el("a", { href: url, download: `outhouse-report-${id}.json` });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
  toast("Evidence package downloaded.");
}

async function drawReportDetail(page, id, { back, stillMine, openUser }) {
  const d = await tryApi("GET", `/api/safety/reports/${id}`).catch(() => null);
  if (!d || !stillMine()) return;
  const r = d.report;
  const redraw = (fresh) => { if (fresh) render(fresh); };
  const act = async (body, done) => {
    const fresh = await tryApi("POST", `/api/safety/reports/${id}/actions`, body).catch(() => null);
    if (fresh) { if (done) toast(done); redraw(fresh); }
  };
  const render = (d) => {
    const r = d.report;
    const closed = r.status === "closed";
    const snapMsg = d.snapshot?.message;
    page.replaceChildren(
      el("div", { class: "sc-back" }, el("button", { class: "ghost", onclick: back }, "← All reports")),
      el("h2", {}, `Report: ${r.category_label}`),
      el("div", { class: "sc-head" }, catChip(r), el("span", { class: `rp-chip st-${r.status}` }, SC_STATUS[r.status] || r.status),
        el("span", { class: "muted small" }, `${r.source === "detector" ? "Flagged by the image detector" : scPlural(r.report_count, "report")} · first ${scWhen(r.created_at)}${closed && r.closed_at ? ` · closed ${scWhen(r.closed_at)}${r.closed_by_name ? ` by ${r.closed_by_name}` : ""}` : ""}`)),
      closed && r.resolution ? el("div", { class: "sc-row-res" }, `✓ ${r.resolution}`) : null,
      r.child_safety ? childSafetyNotice() : null,
      // Pictures the image detector flagged (detector.js).
      typeof detectorReportPanel === "function" ? detectorReportPanel(d, act) : null,
      scSection("What was reported", ...snapshotView(d)),
      d.files.length ? scSection("Files", filesView(d, r.id)) : null,
      d.snapshot?.context?.length ? scSection("Around it (10 messages before and after)", contextView(d)) : null,
      r.source === "detector" && !d.reporters.length ? null : scSection(`Reported by (${d.reporters.length})`,
        el("p", { class: "muted small", style: "margin:0" }, "Never shown to the person reported."),
        el("div", { class: "sc-reporters" }, d.reporters.map((p) => el("div", { class: "sc-reporter" },
          el("strong", {}, p.display_name || "Deleted account"), p.username ? el("span", { class: "muted" }, ` @${p.username}`) : null,
          el("span", { class: "muted" }, ` · ${p.category_label} · ${scWhen(p.created_at)}`),
          p.details ? el("div", { class: "sc-content small" }, p.details) : null)))),
      d.subject ? scSection(`About ${d.subject.display_name}`, subjectView(d.subject, act),
        el("div", {}, el("button", { class: "link", onclick: () => openUser(d.subject.id) }, "Open their page in People"))) : null,
      scSection("Actions",
        el("div", { class: "sc-actions" },
          r.status === "open" ? el("button", { class: "ghost", onclick: () => act({ action: "review" }, "Marked as in review.") }, "Mark in review") : null,
          !closed ? el("button", { class: "primary sc-close", onclick: async () => {
            const f = await formDialog("Close this report", [{ name: "text", label: "What you did (for your records)", type: "text", placeholder: "Warned them and deleted the message" }], "Close report");
            if (f) act({ action: "close", text: f.text }, "Report closed.");
          } }, "Close…") : null,
          !closed ? el("button", { class: "ghost", onclick: async () => {
            const f = await formDialog("Dismiss this report", [{ name: "text", label: "Why (optional)", type: "text", placeholder: "Not against the rules" }], "Dismiss",
              { intro: r.child_safety ? "Dismissing a child-safety report unlocks its files and shows the message again. Only do this if you're sure it was a mistake." : null });
            if (f) act({ action: "dismiss", text: f.text }, "Report dismissed.");
          } }, "Dismiss…") : null,
          closed ? el("button", { class: "ghost", onclick: () => act({ action: "reopen" }, "Reopened.") }, "Reopen") : null,
          d.message_live ? el("button", { class: "ghost", onclick: async () => {
            if (await confirmBox("Delete the reported message?", "It's removed for everyone. The snapshot and any files stay with this report.", "Delete message")) act({ action: "delete_message" }, "Message deleted.");
          } }, "Delete message") : null,
          d.message_live && snapMsg?.attachments?.length ? el("button", { class: "ghost", onclick: async () => {
            if (await confirmBox("Remove the files from the message?", "The text stays; the files disappear from chat but are kept with this report.", "Remove files")) act({ action: "remove_attachment" }, "Files removed.");
          } }, "Remove files") : null,
          el("button", { class: "ghost", onclick: () => exportEvidence(r.id) }, "Export evidence package"),
          closed && r.child_safety && !r.evidence_released ? el("button", { class: "danger-btn", onclick: async () => {
            if (await confirmBox("Delete the evidence files?", "Only do this once police (or the eSafety Commissioner) have what they need or have told you it's not needed. The files are deleted from the server for good; their names and SHA-256 hashes stay in the report.", "Delete files for good")) act({ action: "delete_evidence" }, "Evidence files deleted.");
          } }, "Delete evidence files…") : null),
        d.subject ? accountActions(d.subject, act, r.guild_id) : null),
      scSection("Notes and history", noteBox(act), logView(d.log)));
  };
  render(d);
}

async function drawPeople(page, { stillMine, openReport, userId }) {
  const search = el("input", { type: "text", placeholder: "Find someone by name or username", "aria-label": "Find someone" });
  const results = el("div", { class: "sc-list" });
  const detail = el("div", { class: "sc-person" });
  const showPerson = async (uid) => {
    const u = await tryApi("GET", `/api/safety/users/${uid}`).catch(() => null);
    if (!u || !stillMine()) return;
    const act = async (body, done) => {
      const fresh = await tryApi("POST", `/api/safety/users/${uid}/actions`, body).catch(() => null);
      if (fresh) { if (done) toast(done); paint(fresh); }
    };
    const paint = (u) => detail.replaceChildren(
      el("h3", {}, u.display_name),
      subjectView(u, act),
      accountActions(u, act),
      u.reports.length ? el("div", { class: "sc-list" }, u.reports.map((r) => el("button", { class: "sc-row", onclick: () => openReport(r.id) },
        el("div", { class: "sc-row-top" }, el("span", { class: `rp-chip cat-${r.category}` }, r.category_label), el("span", { class: "sc-when" }, `${SC_STATUS[r.status] || r.status} · ${scWhen(r.created_at)}`))))) : null,
      noteBox(act),
      logView(u.log));
    paint(u);
    detail.scrollIntoView?.({ block: "start" });
  };
  let seq = 0;
  const find = async () => {
    const mine = ++seq;
    const list = await tryApi("GET", `/api/safety/users?q=${encodeURIComponent(search.value.trim())}`).catch(() => []);
    if (mine !== seq || !stillMine()) return;
    results.replaceChildren(...(list.length ? list.map((u) => el("button", { class: "sc-row", "data-user": u.id, onclick: () => showPerson(u.id) },
      el("div", { class: "sc-row-who" }, el("strong", {}, u.display_name), el("span", { class: "muted" }, ` @${u.username}`),
        u.age_bracket === "teen" ? el("span", { class: "rp-chip" }, "Teen") : null,
        u.suspended ? el("span", { class: "rp-chip cat-child_safety" }, "Suspended") : null,
        el("span", { class: "sc-when" }, `joined ${new Date(u.created_at).toLocaleDateString()}`)))) : [el("p", { class: "muted" }, "Nobody by that name.")]));
  };
  let timer = null;
  search.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(find, 250); });
  page.replaceChildren(el("h2", {}, "People"),
    el("p", { class: "muted small", style: "margin:0" }, "Look anyone up to see their history, set their age group (teen or adult), or warn, time out or suspend them without a report. Newest accounts first."),
    search, results, detail);
  await find();
  if (userId) showPerson(userId);
}

async function drawTeenSettings(page, stillMine) {
  const data = await tryApi("GET", "/api/safety/settings").catch(() => null);
  if (!data || !stillMine()) return;
  const s = data.settings;
  const toggle = (key, title, desc) => {
    const btn = el("button", { type: "button", class: `toggle${s[key] ? " on" : ""}`, "aria-label": title, "aria-pressed": String(!!s[key]), onclick: async () => {
      const next = !s[key];
      const saved = await tryApi("PUT", "/api/safety/settings", { [key]: next }).catch(() => null);
      if (!saved) return;
      Object.assign(s, saved);
      btn.classList.toggle("on", !!s[key]);
      btn.setAttribute("aria-pressed", String(!!s[key]));
      toast("Saved.");
    } });
    return el("div", { class: "sc-toggle-row" }, el("div", {}, el("div", { class: "sc-toggle-title" }, title), el("div", { class: "muted small" }, desc)), btn);
  };
  page.replaceChildren(
    el("h2", {}, "Teen protections"),
    el("p", { class: "muted small", style: "margin:0" }, `Sign-up asks for a date of birth: under 13s can't make an account, 13–17 are teens, 18+ adults (only the age group is kept). Accounts made before this count as adults; set someone's age group in People. There ${data.teens === 1 ? "is 1 teen" : `are ${data.teens} teens`} here now. Teens can always report and block.`),
    toggle("teen_contact_limits", "Limit who can contact teens",
      "Teens only get friend requests from people they share an outhouse with who are friends of friends or in a family outhouse. Adults can't start a DM with a teen they aren't friends with (they see “This person can't receive messages from you”), and teens' DMs are friends-only to start with."),
    toggle("teen_block_adult_channels", "Keep teens out of 18+ channels",
      "Age-restricted channels stay closed to teens, even if they click “I'm 18+”."),
    toggle("require_birth_date", "Ask for a date of birth at sign-up",
      "Switch off only if every new account is someone you know is an adult."),
    el("h3", {}, "Family outhouses"),
    el("p", { class: "muted small", style: "margin:0" }, "People in a family outhouse with a teen can send them friend requests (once they're friends they can DM)."),
    el("div", { class: "sc-list" }, data.outhouses.map((o) => {
      const btn = el("button", { type: "button", class: `toggle${o.family ? " on" : ""}`, "aria-label": `${o.name} is a family outhouse`, onclick: async () => {
        const ok = await tryApi("PUT", `/api/safety/guilds/${o.id}/family`, { family: !o.family }).then(() => true, () => false);
        if (!ok) return;
        o.family = !o.family;
        btn.classList.toggle("on", o.family);
      } });
      return el("div", { class: "sc-toggle-row" }, el("div", {}, el("div", { class: "sc-toggle-title" }, o.name), el("div", { class: "muted small" }, scPlural(o.members, "member"))), btn);
    })));
}

async function drawDigest(page, stillMine) {
  const d = await tryApi("GET", "/api/safety/digest").catch(() => null);
  if (!d || !stillMine()) return;
  const item = (label, value, cls = "") => el("div", { class: `status-item ${cls}` }, el("div", { class: "status-label" }, label), el("div", { class: "status-value" }, value));
  page.replaceChildren(
    el("h2", {}, "This week"),
    el("p", { class: "muted small", style: "margin:0" }, "The last 7 days. Outhouse Safety also DMs you this once a week."),
    el("div", { class: "status-grid" },
      item("New accounts", String(d.new_accounts.length)),
      item("Reports", String(d.reports_total)),
      item("Waiting for review", String(d.waiting), d.waiting ? "warn" : "good"),
      item("Child safety open", String(d.urgent), d.urgent ? "bad" : "good")),
    d.reports.length ? el("div", { class: "muted small" }, d.reports.map((r) => `${r.category_label}: ${r.count}`).join(" · ")) : null,
    el("h3", {}, "New accounts"),
    d.new_accounts.length ? el("div", { class: "sc-list" }, d.new_accounts.map((a) => el("div", { class: "sc-row static" },
      el("div", { class: "sc-row-who" }, el("strong", {}, a.display_name), el("span", { class: "muted" }, ` @${a.username}`),
        a.age_bracket === "teen" ? el("span", { class: "rp-chip" }, "Teen") : null,
        el("span", { class: "sc-when" }, scWhen(a.created_at)))))) : el("p", { class: "muted" }, "Nobody new this week."));
}

async function drawSafetyLog(page, stillMine) {
  const data = await tryApi("GET", "/api/safety/log").catch(() => null);
  if (!data || !stillMine()) return;
  page.replaceChildren(el("h2", {}, "Safety log"),
    el("p", { class: "muted small", style: "margin:0" }, "Everything done about reports and accounts, newest first, with your private notes. Kept for good."),
    logView(data.entries));
}

function drawPoliceGuide(page) {
  const link = ([label, href]) => el("li", {}, label, " — ", el("a", { href, target: "_blank", rel: "noopener noreferrer" }, href));
  page.replaceChildren(
    el("h2", {}, "Reporting to police"),
    el("p", { class: "muted small", style: "margin:0" }, "General information, not legal advice."),
    el("ul", { class: "sc-guide" },
      el("li", {}, el("strong", {}, "A child in immediate danger: call 000.")),
      link(AUTHORITY_LINKS[0]), link(AUTHORITY_LINKS[1]),
      el("li", {}, "Outside Australia: ", el("a", { href: AUTHORITY_LINKS[2][1], target: "_blank", rel: "noopener noreferrer" }, "NCMEC CyberTipline (US)"), ", or your local police.")),
    el("h3", {}, "Suspected child sexual abuse material"),
    el("p", {}, "Don't view, download, copy, screenshot or forward it, not even to show police. Outhouse has already locked the files (nobody can download them) and keeps them unchanged on the server. Report it to the ACCCE or eSafety and tell them you run the server."),
    el("h3", {}, "What to give them"),
    el("p", {}, "Open the report and press “Export evidence package”. It's a JSON file with the report, what the message said (and the messages around it), who sent it and when, which accounts reported it, the account's history and each file's name, size and SHA-256 hash. It never contains the files."),
    el("p", {}, "If police ask for the files themselves, follow their instructions. They're in the server's upload folder, named by the storage key in the package; the SHA-256 proves they haven't changed. deploy/README.md (“Safety & reports”) explains where that is."),
    el("h3", {}, "After it's dealt with"),
    el("p", {}, "Suspend the account if you haven't. Once police have what they need (or say they don't need it), close the report and use “Delete evidence files” to remove the files from the server for good."));
}

// ---------- Outhouse settings → Reports (site owner) ----------

async function drawReportsTab(page) {
  const data = await tryApi("GET", "/api/safety/reports?status=open").catch(() => null);
  if (!data || !page.isConnected) return;
  const c = data.counts;
  page.replaceChildren(
    el("h2", {}, "Reports"),
    el("p", { class: "muted small", style: "margin:0" }, "Reports from everyone on this server (every outhouse) come to you, the site owner. Nobody else sees them."),
    el("div", { class: "sc-counts" },
      el("span", {}, el("strong", {}, String(c.open)), " open"),
      el("span", {}, el("strong", {}, String(c.in_review)), " in review"),
      c.urgent ? el("span", { class: "sc-urgent" }, el("strong", {}, String(c.urgent)), " child safety") : null),
    data.reports.length
      ? el("div", { class: "sc-list" }, data.reports.slice(0, 10).map((r) => reportRow(r, (id) => openSafetyConsole("open", id))))
      : el("p", { class: "muted" }, "Nothing waiting. All quiet out the back."),
    el("div", {}, el("button", { class: "primary", onclick: () => openSafetyConsole() }, "Open the safety console")));
}
