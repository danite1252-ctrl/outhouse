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
    // Pinned to the bottom of the rail, below everything that scrolls.
    document.getElementById("rail-foot")?.append(btn);
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
    openSafetyConsole("open", state.openReportOnReady || null);
    state.openReportOnReady = null;
  }
}

// What child-safety alerts say (police advice: suspend straight away).
const SUSPEND_NUDGE = "Review and suspend the account now if in doubt.";

// Opens the console on a report (from a notification).
function openReportFromAlert(reportId) {
  state.safety.pendingReport = null;
  openSafetyConsole("open", reportId || null);
}

function safetyAlert(reportId = null) {
  toast(`⚠ Urgent: a child-safety report is waiting in Reports. ${SUSPEND_NUDGE}`);
  const title = "Urgent safety report";
  const body = `A child-safety report needs your review. ${SUSPEND_NUDGE}`;
  if (typeof desktop !== "undefined" && desktop && typeof desktopInvoke === "function") {
    desktopInvoke("notify", { title, body });
    // Clicking the app's notification brings the window forward: open the report then.
    if (!document.hasFocus()) state.safety.pendingReport = reportId || "list";
  } else if ("Notification" in window && Notification.permission === "granted" && !document.hasFocus()) {
    try {
      const n = new Notification(title, { body, tag: "safety-report", icon: "/icons/icon-192.png" });
      n.onclick = () => { window.focus(); n.close(); openReportFromAlert(reportId); };
    } catch { /* not allowed here */ }
  }
}
window.addEventListener("focus", () => {
  const pending = state.safety.pendingReport;
  if (pending) openReportFromAlert(pending === "list" ? null : pending);
});

const safetyBaseHandle = handleEvent;
handleEvent = function (ev) {
  safetyBaseHandle(ev);
  if (ev.t === "READY") loadSafetySummary();
  if (ev.t === "REPORTS_CHANGED") {
    Object.assign(state.safety, { owner: true, open: ev.d.open, urgent: ev.d.urgent });
    renderSafetyBadge();
    if (ev.d.alert) safetyAlert(ev.d.report_id || null);
    const open = document.querySelector("dialog.safety-console");
    if (open && open.dataset.view === "list") open.dispatchEvent(new Event("safety-refresh"));
  }
};

// A notification about a report opens the console.
(() => {
  const q = new URLSearchParams(location.search);
  if (!q.has("reports")) return;
  state.openReportsOnReady = true;
  state.openReportOnReady = /^\d+$/.test(q.get("report") || "") ? q.get("report") : null;
  q.delete("reports");
  q.delete("report");
  const rest = q.toString();
  history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : ""));
})();
if (typeof openTarget === "function") {
  const safetyBaseOpenTarget = openTarget;
  openTarget = function (target) {
    if (target?.channel_id === "reports") return openSafetyConsole("open", target.report_id || null);
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
      navBtn("people", "People", key), navBtn("blocks", "Blocked networks", key), navBtn("teens", "Teen protections", key),
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
    if (key === "blocks") return drawBlockedNetworks(page, { stillMine, openUser: (uid) => { personId = uid; draw("people"); }, redraw: () => draw("blocks") });
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
  return el("button", { class: `sc-row${r.child_safety ? " child" : ""}${r.source === "ip-match" ? " ipm" : ""}`, "data-report": r.id, "data-source": r.source || "user", onclick: () => onOpen(r.id) },
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
    status !== "closed" && data.counts.urgent ? el("div", { class: "sc-urgent-banner", role: "alert" },
      el("strong", {}, `⚠ ${scPlural(data.counts.urgent, "child-safety report")} waiting. ${SUSPEND_NUDGE}`)) : null,
    status !== "closed" ? el("div", { class: "sc-counts" },
      el("span", {}, el("strong", {}, String(data.counts.open)), " open"),
      el("span", {}, el("strong", {}, String(data.counts.in_review)), " in review"),
      data.counts.urgent ? el("span", { class: "sc-urgent" }, el("strong", {}, String(data.counts.urgent)), " child safety") : null,
      data.counts.ip_matches ? el("span", { class: "sc-ipm-count", title: "Accounts that used the same IP address as a suspended or banned account. Hints, not proof." }, el("strong", {}, String(data.counts.ip_matches)), " same-IP hint", data.counts.ip_matches === 1 ? "" : "s") : null) : null,
    data.reports.length
      ? el("div", { class: "sc-list" }, data.reports.map((r) => reportRow(r, openReport)))
      : el("div", { class: "sp-empty" }, el("div", { class: "sp-empty-icon" }, "🛡"), el("p", { class: "muted" }, empty[status])));
}

// The warning shown on every child-safety report.
function childSafetyNotice() {
  return el("div", { class: "sc-child-warning", role: "note" },
    el("strong", {}, "Suspected child sexual abuse material: don't look, don't share."),
    el("p", { class: "sc-nudge" }, el("strong", {}, SUSPEND_NUDGE), " Police advice: suspend the reported account straight away; leaving it active once you know could be seen as helping. You can lift it after you've reviewed."),
    el("p", {}, "Pictures and videos from this report are locked away and are never shown here. Do not view, download, copy or forward suspected child sexual abuse material, not even to report it. Preserve it (Outhouse keeps the files unchanged on the server) and report it to the authorities:"),
    el("ul", {},
      AUTHORITY_LINKS.slice(0, 2).map(([label, href]) => el("li", {}, label, " — ", el("a", { href, target: "_blank", rel: "noopener noreferrer" }, href))),
      el("li", {}, el("strong", {}, "If a child is in immediate danger, call 000.")),
      el("li", {}, AUTHORITY_LINKS[2][0], " — ", el("a", { href: AUTHORITY_LINKS[2][1], target: "_blank", rel: "noopener noreferrer" }, AUTHORITY_LINKS[2][1]), ", or your local police.")),
    el("p", {}, "When police ask for the evidence, use “Hand over to police” below: it gives them a one-time download link for the sealed package (the files, unchanged, and all the details), so the files never touch your device."));
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
    el("div", { class: "audit-icon" }, { note: "📝", quarantine: "🔒", suspend: "⛔", warn: "⚠", ban: "🚫", kick: "👢", timeout: "⏳", dismiss: "✖", close: "✓", export: "📦", view_ips: "👁", ip_match: "🔗", handover: "📦", handover_download: "⬇", handover_revoke: "✖", handover_expired: "⌛", ip_block: "⛔", ip_unblock: "↩", ip_block_extend: "⏳", ip_exempt: "✓" }[e.action] || "•"),
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
        u.message_requests.ignored + u.message_requests.blocked >= 3 ? "warn" : "") : null),
    el("div", { class: "muted small" }, "Outhouses: ", u.outhouses.length ? u.outhouses.map((o) => `${o.name}${o.owner ? " (owner)" : ""}${o.family ? " (family)" : ""}${o.timeout_until ? " (timed out)" : ""}`).join(", ") : "none"),
    others.length ? el("div", { class: "muted small" }, "Reports: ", others.slice(0, 10).map((o) => `${o.category_label} (${SC_STATUS[o.status] || o.status}, ${new Date(o.created_at).toLocaleDateString()})`).join(" · ")) : null,
    u.names ? el("details", { class: "sc-names" }, el("summary", {}, `Names they've gone by (${u.names.length})`),
      typeof nameHistoryList === "function" ? nameHistoryList(u.names, "No name changes recorded.") : null) : null);
}

// Buttons for the account actions; `act(body, done)` posts one.
function accountActions(u, act, guildId = null, ctx = null) {
  if (u.site_owner) return el("p", { class: "muted small", style: "margin:0" }, "Site owners can't be warned, timed out or suspended here.");
  const lift = () => el("button", { class: "ghost", onclick: async () => {
    if (await confirmBox(`Lift ${u.display_name}'s suspension?`, "They can sign in again. Their bots' tokens stay revoked until they reset them, and hidden messages stay hidden unless you show them again.", "Lift suspension")) act({ action: "unsuspend" }, "Suspension lifted.");
  } }, "Lift suspension");
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
    u.suspended && u.suspension_pending && ctx ? el("button", { class: "danger-btn sc-make-permanent", onclick: () => permanentDialog(u, ctx, { makePermanent: true }) }, "Make permanent…") : null,
    u.suspended && u.suspended_permanently && ctx ? el("button", { class: "ghost sc-block-ips", onclick: () => permanentDialog(u, ctx, { makePermanent: false }) }, "Block IPs…") : null,
    u.suspended
      ? lift()
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
      } }, "Hide all their messages"),
    exemptToggle(u, act));
}

// "Allow from blocked networks": the owner's way to let someone he knows is
// innocent (family on the same connection) past a block.
function exemptToggle(u, act) {
  const btn = el("button", { type: "button", class: `toggle${u.ip_block_exempt ? " on" : ""}`, "aria-label": "Allow from blocked networks", "aria-pressed": String(!!u.ip_block_exempt), onclick: async () => {
    const next = !u.ip_block_exempt;
    if (next && !(await confirmBox(`Allow ${u.display_name} from blocked networks?`, "They can sign in and use Outhouse from a blocked address (e.g. family sharing a blocked connection). It's written to the safety log.", "Allow"))) return;
    act({ action: "set_ip_exempt", exempt: next }, next ? "Allowed from blocked networks." : "No longer allowed from blocked networks.");
  } });
  return el("div", { class: "sc-toggle-row sc-exempt" },
    el("div", {}, el("div", { class: "sc-toggle-title" }, "Allow from blocked networks"),
      el("div", { class: "muted small" }, "For someone you know is innocent who shares a blocked connection. Off by default.")), btn);
}

// The suspension at the top of a child-safety report: "Suspend now" first.
function suspendNowPanel(d, act, ctx) {
  const u = d.subject;
  if (!u || u.site_owner || u.bot) return null;
  const guild = d.report.target_kind === "guild";
  if (!u.suspended) {
    return el("section", { class: "sc-section sc-suspend-now" },
      el("h3", {}, guild ? "Suspend the outhouse owner now" : "Suspend now"),
      el("p", { class: "sc-tight" }, `${SUSPEND_NUDGE} This suspends ${u.display_name} (@${u.username}) until you've reviewed it: they're signed out everywhere at once, can't sign in, their bots stop and their profile is hidden. Nothing else is hidden.`),
      el("p", { class: "muted small sc-tight" }, "They only see “Suspended pending review of a report”: never what it's about or who reported it."),
      el("div", { class: "sc-actions" }, el("button", { class: "primary danger-fill sc-suspend-now-btn", onclick: async () => {
        if (await confirmBox(`Suspend ${u.display_name} now?`, "Until you've reviewed this report. They see only “Suspended pending review of a report”. You can lift it or make it permanent afterwards.", "Suspend now")) act({ action: "suspend_pending" }, "Suspended until you've reviewed it.");
      } }, "Suspend now")));
  }
  const by = u.suspended_by && u.suspended_by === state.me?.id ? "you" : u.suspended_by_name || "the site owner";
  return el("section", { class: "sc-section sc-suspended-panel" },
    el("h3", {}, u.suspension_pending ? "Suspended until reviewed" : `Suspended ${u.suspended_label || ""}`),
    el("p", { class: "sc-tight" }, u.suspended_at ? `Suspended at ${scWhen(u.suspended_at)} by ${by}.` : "Suspended.",
      u.suspension_pending ? " They see only “Suspended pending review of a report”." : ""),
    el("div", { class: "sc-actions" },
      u.suspension_pending ? el("button", { class: "danger-btn sc-make-permanent", onclick: () => permanentDialog(u, ctx, { makePermanent: true }) }, "Make permanent…") : null,
      u.suspended_permanently ? el("button", { class: "ghost sc-block-ips", onclick: () => permanentDialog(u, ctx, { makePermanent: false }) }, "Block IPs…") : null,
      el("button", { class: "ghost", onclick: async () => {
        if (await confirmBox(`Lift ${u.display_name}'s suspension?`, "They can sign in again. Their bots' tokens stay revoked until they reset them.", "Lift suspension")) act({ action: "unsuspend" }, "Suspension lifted.");
      } }, "Lift suspension")));
}

// Make a suspension permanent (and block their networks), or just block a
// permanently suspended person's networks. ctx: { post(body), refresh(), reportId }
async function permanentDialog(u, ctx, { makePermanent }) {
  const p = await tryApi("GET", `/api/safety/users/${u.id}/ip-block-preview`).catch(() => null);
  if (!p) return;
  const blockable = p.networks.filter((n) => !n.skip && !n.already_blocked);
  const blockBox = el("input", { type: "checkbox", class: "sc-block-box", checked: makePermanent ? !!p.child_safety && blockable.length > 0 : blockable.length > 0, disabled: !blockable.length });
  const ownBox = el("input", { type: "checkbox", class: "sc-own-box" });
  const rows = p.networks.map((n) => {
    const box = el("input", { type: "checkbox", value: n.network, checked: !n.skip && !n.already_blocked && !n.own, disabled: !!n.skip || n.already_blocked });
    const why = n.skip ? `Skipped: ${n.skip}` : n.already_blocked ? "Already blocked" : n.own ? "⚠ One of your own addresses" : `Last seen ${scWhen(n.last_seen)}`;
    return { n, box, row: el("label", { class: `sc-net${n.skip || n.already_blocked ? " skipped" : ""}${n.own ? " own" : ""}` }, box,
      el("span", {}, el("code", {}, n.network), el("span", { class: "muted small" }, ` ${why}`),
        n.ips.length > 1 || n.network.includes(":") ? el("span", { class: "muted small" }, ` (${n.ips.join(", ")})`) : null)) };
  });
  const anyOwn = p.networks.some((n) => n.own && !n.skip);
  ownBox.addEventListener("change", () => rows.filter((r) => r.n.own && !r.n.skip).forEach((r) => { r.box.checked = ownBox.checked; }));
  const duration = el("select", { "aria-label": "How long" }, el("option", { value: "" }, "Until I remove it"), el("option", { value: "365" }, "1 year"), el("option", { value: "90" }, "90 days"));
  const reason = el("input", { type: "text", maxlength: 300, placeholder: makePermanent ? "Shown when they try to sign in (optional)" : "For your records (optional)" });
  const netList = el("div", { class: "sc-nets" }, rows.length ? rows.map((r) => r.row) : el("p", { class: "muted small sc-tight" }, "No IP addresses recorded for them in the retention period."));
  const blockPart = el("div", { class: "sc-block-part" },
    el("label", { class: "rp-check" }, blockBox, el("span", {}, el("strong", {}, "Also block their IP addresses"), p.child_safety ? " (ticked because they have a child-safety report)" : "")),
    el("div", { class: "sc-child-warning sc-block-note" }, el("strong", {}, "Everyone on these networks will be locked out."),
      el("p", {}, "Nobody can sign up, sign in or use Outhouse from a blocked address: their household, and possibly a whole mobile network, school or workplace that shares it. People already signed in from it are signed out now. That's your choice under your safety policy. Changing networks or using a VPN can still get around it."),
      el("p", {}, "Your own account always works, and in People you can “Allow from blocked networks” for someone you know is innocent. Private, home-network and proxy addresses are never blocked.")),
    netList,
    anyOwn ? el("label", { class: "rp-check sc-own-warning" }, ownBox, "Block my own address too (it locks out everyone at home except my account)") : null,
    el("label", { class: "field" }, "For", duration));
  const sync = () => { blockPart.querySelectorAll(".sc-nets input, select, .sc-own-box").forEach((x) => { x.disabled = !blockBox.checked || (x.closest(".sc-net")?.classList.contains("skipped") ?? false); }); };
  blockBox.addEventListener("change", sync);
  const dialog = el("dialog", { class: "report-dialog sc-form sc-permanent-dialog" },
    el("form", { method: "dialog" },
      el("h2", {}, makePermanent ? `Make ${u.display_name}'s suspension permanent` : `Block ${u.display_name}'s IP addresses`),
      makePermanent ? el("p", { class: "muted small", style: "margin:0" }, "They stay signed out for good. They're never told what it was about or who reported them.") : null,
      makePermanent ? el("label", { class: "field" }, "Reason", reason) : null,
      blockPart,
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
        el("button", { type: "submit", class: "primary danger-fill", value: "ok" }, makePermanent ? "Make permanent" : "Block"))));
  sync();
  dialog.addEventListener("close", async () => {
    const ok = dialog.returnValue === "ok";
    const chosen = rows.filter((r) => r.box.checked && !r.box.disabled).map((r) => r.n.network);
    const includeOwn = anyOwn && ownBox.checked;
    dialog.remove();
    if (!ok) return;
    if (makePermanent) {
      const done = await ctx.post({ action: "make_permanent", text: reason.value.trim() }).catch(() => null);
      if (!done) return;
    }
    let msg = makePermanent ? "Suspension made permanent." : "";
    if (blockBox.checked && chosen.length) {
      const r = await tryApi("POST", `/api/safety/users/${u.id}/ip-blocks`, { networks: chosen, days: duration.value ? Number(duration.value) : null, include_own: includeOwn, report_id: ctx.reportId || null, reason: reason.value.trim() || (p.child_safety ? "Child-safety report" : "Permanent suspension") }).catch(() => null);
      if (r) msg = `${msg} Blocked ${scPlural(r.blocked.length, "network")}${r.signed_out ? `; ${scPlural(r.signed_out, "account")} signed out` : ""}.`.trim();
    }
    if (msg) toast(msg);
    ctx.refresh();
  });
  document.body.append(dialog);
  dialog.showModal();
}

// Refused attempts from blocked networks (theirs, or on networks blocked because of them).
function attemptsView(list) {
  if (!list?.length) return el("p", { class: "muted small sc-tight" }, "None.");
  return el("div", { class: "sc-table-wrap" }, el("table", { class: "sc-table sc-attempts" },
    el("thead", {}, el("tr", {}, ["When", "IP address", "Network", "What", "Account"].map((h) => el("th", {}, h)))),
    el("tbody", {}, list.map((a) => el("tr", {},
      el("td", {}, scWhen(a.created_at)), el("td", {}, el("code", {}, a.ip)), el("td", {}, el("code", {}, a.network)),
      el("td", {}, ATTEMPT_KIND[a.kind] || a.kind),
      el("td", {}, a.account_name ? `${a.account_name} (@${a.username})` : a.username ? `@${a.username} (tried)` : "—"))))));
}
const ATTEMPT_KIND = { register: "Sign-up", login: "Sign-in", request: "Using Outhouse", gateway: "Live connection" };

function noteBox(act) {
  const text = el("textarea", { rows: 2, maxlength: 2000, placeholder: "A private note (only site owners see these)", "aria-label": "Private note" });
  return el("div", { class: "sc-note" }, text, el("button", { class: "ghost", onclick: () => { if (text.value.trim()) act({ action: "note", text: text.value.trim() }, "Note added."); } }, "Add note"));
}

async function exportEvidence(id) {
  const ok = await confirmBox("Export the details only?",
    "This downloads a JSON file with the report, the snapshot, who reported it, the account's history and IP addresses, and the files' names and SHA-256 hashes. It never contains the files themselves (for those, use Hand over to police). Keep it private and only give it to police or the eSafety Commissioner.", "Download");
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

// ---------- Hand over to police ----------
// The sealed package (original files byte for byte, the evidence package,
// HANDOVER.txt, MANIFEST.sha256) is built on the server. Police get it from a
// one-time link on their own device; "Download here" only if they ask.

const PL_STATUS = { active: "Active", used: "Used", expired: "Expired", revoked: "Revoked" };
const scBytes = (n) => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : n >= 1024 ? `${Math.round(n / 1024)} KB` : `${n} bytes`);

// The confirm dialog: police present, officer / reference, (link options), password.
// submit(values) runs while the dialog is open; it throws to show an error.
function handoverDialog(mode, d, submit) {
  const link = mode === "link";
  const files = d.files.length;
  const present = el("input", { type: "checkbox", class: "ho-present" });
  const reference = el("input", { type: "text", class: "ho-reference", maxlength: 200, placeholder: "e.g. Det. Sgt Smith, ref 1234/26", autocomplete: "off" });
  const hours = el("select", { class: "ho-hours" }, [["1", "1 hour"], ["24", "24 hours"], ["168", "7 days"]].map(([v, l]) => el("option", { value: v, selected: v === "24" }, l)));
  const downloads = el("select", { class: "ho-downloads" }, [["1", "Once (recommended)"], ["2", "Twice"], ["3", "3 times"]].map(([v, l]) => el("option", { value: v }, l)));
  const password = el("input", { type: "password", class: "ho-password", autocomplete: "current-password", required: true });
  const error = el("p", { class: "error ho-error", role: "alert" });
  const go = el("button", { type: "submit", class: "primary", disabled: true }, link ? "Create the link" : "Download the package");
  const sync = () => { go.disabled = !(present.checked && password.value); };
  present.addEventListener("change", sync);
  password.addEventListener("input", sync);
  const what = `${scPlural(files, "original file")}${files ? " (byte for byte, never opened)" : ""}, everything Outhouse knows about this report (JSON, with IP addresses and blocked-network attempts), HANDOVER.txt and MANIFEST.sha256`;
  const dialog = el("dialog", { class: "report-dialog sc-form ho-dialog" },
    el("form", { method: "dialog" },
      el("h2", {}, link ? "Create a download link for police" : "Download the package here"),
      link
        ? el("p", { class: "muted small ho-intro" }, `Outhouse packs ${what} into one sealed file on the server, and gives you a link for police to open on their own device. You never see the files.`)
        : el("div", { class: "ho-warning", role: "note" }, el("strong", {}, "This saves the files onto this device."), " Don't open or extract it; hand it straight to police and delete it after.",
            el("span", { class: "ho-warning-sub" }, `It has ${what}.`)),
      el("label", { class: "rp-check ho-check" }, present, link ? "Police are with me, or police have asked me for this" : "Police are with me and have asked for it on this device"),
      el("label", { class: "field" }, "Officer / reference (optional)", reference,
        el("span", { class: "muted small" }, "Goes in HANDOVER.txt and the safety log.")),
      link ? el("div", { class: "ho-options" },
        el("label", { class: "field" }, "Link works for", hours),
        el("label", { class: "field" }, "Can be downloaded", downloads)) : null,
      el("label", { class: "field" }, "Your password (to confirm it's you)", password),
      error,
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"),
        go)));
  dialog.querySelector("form").addEventListener("submit", async (e) => {
    e.preventDefault();
    if (go.disabled) return;
    go.disabled = true;
    error.textContent = "";
    const label = go.textContent;
    go.textContent = "Packing…";
    try {
      await submit({
        password: password.value,
        police_present: present.checked,
        reference: reference.value.trim() || null,
        hours: Number(hours.value),
        max_downloads: Number(downloads.value),
      });
      dialog.close();
    } catch (err) {
      error.textContent = err.message;
      go.textContent = label;
      password.value = "";
      sync();
      password.focus();
    }
  });
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
}

// After a link is made (shown once) or the package was downloaded here.
function handoverResultDialog({ url = null, link }) {
  const row = (label, value, what) => el("div", { class: "ho-result-row" },
    el("span", { class: "ho-result-label" }, label),
    el("code", { class: "ho-result-value" }, value),
    el("button", { type: "button", class: "ghost sc-copy", onclick: () => copy(value, what) }, "Copy"));
  const dialog = el("dialog", { class: "report-dialog sc-form ho-result" },
    el("form", { method: "dialog" },
      el("h2", {}, url ? "Link for police" : "Package downloaded"),
      url
        ? el("p", { class: "ho-once" }, el("strong", {}, "This is the only time the link is shown."), " Give it to police to open on their device, or paste it into their portal. Opening it doesn't use it up; downloading does.")
        : el("p", { class: "ho-once" }, el("strong", {}, "Don't open or extract it."), " Hand it straight to police, then delete it from this device."),
      url ? row("Link", url, "Link copied") : null,
      row("SHA-256", link.package_sha256, "SHA-256 copied"),
      el("dl", { class: "ho-facts" },
        el("dt", {}, "File"), el("dd", {}, `${link.package_name} (${scBytes(link.package_size)}, ${scPlural(link.file_count, "file")})`),
        url ? [el("dt", {}, "Works until"), el("dd", {}, scWhen(link.expires_at))] : null,
        url ? [el("dt", {}, "Downloads"), el("dd", {}, link.max_downloads === 1 ? "Once, then it's deleted from the server" : `${link.max_downloads} times, then it's deleted from the server`)] : null,
        link.reference ? [el("dt", {}, "Reference"), el("dd", {}, link.reference)] : null),
      el("p", { class: "muted small" }, "Police can check the file with sha256sum (it should match the SHA-256 above, which is also in the safety log)."),
      el("div", { class: "row" }, el("button", { type: "submit", class: "primary" }, "Done"))));
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
}

async function downloadHere(reportId, values) {
  const res = await fetch(`/api/safety/reports/${reportId}/handover-download`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${state.token}` },
    body: JSON.stringify(values),
  }).catch(() => null);
  if (!res) throw new Error("Couldn't reach the server.");
  if (!res.ok) {
    const data = await res.json().catch(() => null);
    throw new Error(data?.error || `Couldn't make the package (${res.status}).`);
  }
  const name = (res.headers.get("Content-Disposition") || "").match(/filename="([^"]+)"/)?.[1] || `evidence-${reportId}.tar.gz`;
  const sha = res.headers.get("X-Package-SHA256") || "";
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = el("a", { href: url, download: name });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
  return { package_name: name, package_sha256: sha, package_size: blob.size, file_count: Number(res.headers.get("X-Package-Files") || 0), reference: values.reference || "" };
}

function policeLinksView(d, ctx) {
  const links = d.police_links || [];
  if (!links.length) return null;
  const revoke = async (l) => {
    if (!await confirmBox("Revoke this link?", "It stops working at once and the package is deleted from the server.", "Revoke link")) return;
    const r = await tryApi("POST", `/api/safety/reports/${d.report.id}/police-links/${l.id}/revoke`).catch(() => null);
    if (r) { toast("Link revoked; the package was deleted."); ctx.refresh(); }
  };
  const rowFor = (l) => el("tr", { class: `ho-${l.status}` },
    el("td", { "data-label": "Made" }, scWhen(l.created_at), el("div", { class: "muted small" }, el("code", {}, `${l.token_prefix}…`))),
    el("td", { "data-label": "Reference" }, l.reference || "—"),
    el("td", { "data-label": "Works until" }, scWhen(l.expires_at)),
    el("td", { "data-label": "Downloads" }, `${l.downloads} of ${l.max_downloads}`, l.last_download_at ? el("div", { class: "muted small" }, `last ${scWhen(l.last_download_at)}`) : null),
    el("td", { "data-label": "Status" }, el("span", { class: `rp-chip ho-chip ho-chip-${l.status}` }, PL_STATUS[l.status] || l.status),
      l.package_deleted && l.status !== "active" ? el("div", { class: "muted small" }, "package deleted") : null),
    el("td", { class: "ho-act" }, l.status === "active" ? el("button", { class: "ghost ho-revoke", onclick: () => revoke(l) }, "Revoke") : null));
  const table = el("table", { class: "sc-table ho-links-table" },
    el("thead", {}, el("tr", {}, ["Made", "Reference", "Works until", "Downloads", "Status", ""].map((h) => el("th", {}, h)))),
    el("tbody", {}, links.map(rowFor)));
  return el("div", { class: "ho-links" },
    el("h4", {}, "Links made for this report"),
    el("div", { class: "sc-table-wrap" }, table),
    el("p", { class: "muted small sc-tight" }, "Each download is written to the safety log with the downloader's IP address and browser."));
}

function handoverPanel(d, ctx) {
  const r = d.report;
  const any = !r.child_safety && !d.files.some((f) => f.quarantined);
  const cmd = `cd ~/outhouse && ./deploy/evidence-handover.sh ${any ? "--any " : ""}${r.id}`;
  const files = d.files.length;
  return el("section", { class: "sc-section sc-handover" },
    el("h3", {}, "Hand over to police"),
    el("p", { class: "sc-tight" }, `Police get ${files ? `the ${scPlural(files, "original file")}, unchanged, ` : ""}in one sealed package with everything Outhouse knows about this report. You never need to open, view or download the files, and you shouldn't.`),
    el("div", { class: "ho-routes" },
      el("div", { class: "ho-route ho-route-link" },
        el("div", { class: "ho-route-head" }, el("strong", {}, "One-time download link for police"), el("span", { class: "rp-chip ho-recommended" }, "Recommended")),
        el("p", { class: "muted small" }, "Packed on the server; police open the link on their own device (or you paste it into their portal). It works once and expires, then the package is deleted. The files never touch your device."),
        el("button", { class: "primary ho-create", onclick: () => handoverDialog("link", d, async (values) => {
          const res = await api("POST", `/api/safety/reports/${r.id}/police-links`, values);
          handoverResultDialog({ url: res.url, link: res.link });
          ctx.refresh();
        }) }, "Create a download link for police")),
      el("div", { class: "ho-route ho-route-here" },
        el("div", { class: "ho-route-head" }, el("strong", {}, "Download here"), el("span", { class: "muted small" }, "only if police ask")),
        el("p", { class: "muted small" }, "Saves the same package onto this device, for police to copy under supervision."),
        el("button", { class: "ghost ho-here", onclick: () => handoverDialog("here", d, async (values) => {
          const done = await downloadHere(r.id, values);
          handoverResultDialog({ link: done });
          ctx.refresh();
        }) }, "Download here…"))),
    policeLinksView(d, ctx),
    el("details", { class: "ho-advanced" },
      el("summary", {}, "Advanced: from the server over SSH"),
      el("p", { class: "muted small sc-tight" }, "The same package, made on the server if the app can't be used. It's saved in ~/outhouse-evidence on the server and logged."),
      el("div", { class: "sc-cmd" }, el("code", { class: "sc-cmd-text" }, cmd),
        el("button", { class: "ghost sc-copy", onclick: () => copy(cmd, "Command copied") }, "Copy"))));
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
  const ctx = {
    reportId: id,
    post: (body) => tryApi("POST", `/api/safety/reports/${id}/actions`, body),
    refresh: async () => redraw(await tryApi("GET", `/api/safety/reports/${id}`).catch(() => null)),
  };
  const render = (d) => {
    const r = d.report;
    const closed = r.status === "closed";
    const snapMsg = d.snapshot?.message;
    page.replaceChildren(
      el("div", { class: "sc-back" }, el("button", { class: "ghost", onclick: back }, "← All reports")),
      el("h2", {}, r.source === "ip-match" ? "Same-IP hint" : `Report: ${r.category_label}`),
      el("div", { class: "sc-head" }, catChip(r), el("span", { class: `rp-chip st-${r.status}` }, SC_STATUS[r.status] || r.status),
        el("span", { class: "muted small" }, `${r.source === "detector" ? "Flagged by the image detector" : r.source === "ip-match" ? "Same-IP hint (not a report)" : scPlural(r.report_count, "report")} · first ${scWhen(r.created_at)}${closed && r.closed_at ? ` · closed ${scWhen(r.closed_at)}${r.closed_by_name ? ` by ${r.closed_by_name}` : ""}` : ""}`)),
      closed && r.resolution ? el("div", { class: "sc-row-res" }, `✓ ${r.resolution}`) : null,
      r.child_safety ? childSafetyNotice() : null,
      r.child_safety ? suspendNowPanel(d, act, ctx) : null,
      // Pictures the image detector flagged (detector.js).
      typeof detectorReportPanel === "function" ? detectorReportPanel(d, act) : null,
      r.source === "ip-match" ? scSection("Same IP address", ipMatchView(d, openUser)) : scSection("What was reported", ...snapshotView(d)),
      d.files.length ? scSection("Files", filesView(d, r.id)) : null,
      r.source === "ip-match" ? null : handoverPanel(d, ctx),
      d.snapshot?.context?.length ? scSection("Around it (10 messages before and after)", contextView(d)) : null,
      (r.source === "detector" || r.source === "ip-match") && !d.reporters.length ? null : scSection(`Reported by (${d.reporters.length})`,
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
          el("button", { class: "ghost", onclick: () => exportEvidence(r.id) }, "Export details only (JSON, no files)"),
          closed && r.child_safety && !r.evidence_released ? el("button", { class: "danger-btn", onclick: async () => {
            if (await confirmBox("Delete the evidence files?", "Only do this once police (or the eSafety Commissioner) have what they need or have told you it's not needed. The files are deleted from the server for good; their names and SHA-256 hashes stay in the report.", "Delete files for good")) act({ action: "delete_evidence" }, "Evidence files deleted.");
          } }, "Delete evidence files…") : null),
        d.subject ? accountActions(d.subject, act, r.guild_id, ctx) : null),
      scSection("Notes and history", noteBox(act), logView(d.log)));
  };
  render(d);
}

// ---------- IP addresses (site owner only; every look is in the safety log) ----------

const IP_KIND = { register: "Sign-up", login: "Sign-in", session: "Signed-in activity" };

// "Banned in X · Suspended until Y" chips, or "Normal".
function accountStatusChips(a) {
  const chips = [];
  if (a.suspended) chips.push(el("span", { class: "rp-chip ip-flag" }, `Suspended ${a.suspended_label || ""}`.trim()));
  if (a.banned_in?.length) chips.push(el("span", { class: "rp-chip ip-flag" }, `Banned in ${a.banned_in.join(", ")}`));
  if (!chips.length) chips.push(el("span", { class: "rp-chip" }, "Normal"));
  return chips;
}

function ipSection(u, openPerson) {
  const body = el("div", { class: "sc-ips-body" });
  const show = el("button", { class: "ghost", "data-show-ips": u.id, onclick: async () => {
    show.disabled = true;
    const d = await tryApi("GET", `/api/safety/users/${u.id}/ips`).catch(() => null);
    show.disabled = false;
    if (!d) return;
    show.remove();
    body.replaceChildren(
      el("h4", {}, `Recent IPs (${d.ips.length})`),
      d.ips.length ? el("div", { class: "sc-table-wrap" }, el("table", { class: "sc-table sc-ip-table" },
        el("thead", {}, el("tr", {}, ["IP address", "First seen", "Last seen", "First seen at", "Times"].map((h) => el("th", {}, h)))),
        el("tbody", {}, d.ips.map((r) => el("tr", {},
          el("td", {}, el("code", {}, r.ip), r.local ? el("span", { class: "muted small", title: "A home-network (LAN) or proxy address: says little about who it is." }, " LAN") : null),
          el("td", {}, scWhen(r.first_seen)), el("td", {}, scWhen(r.last_seen)),
          el("td", {}, IP_KIND[r.kind] || r.kind), el("td", {}, String(r.count)))))))
        : el("p", { class: "muted small sc-tight" }, "None recorded in the retention period."),
      el("h4", {}, `Other accounts seen on the same IP (${d.same_ip_accounts.length})`),
      d.same_ip_accounts.length ? el("div", { class: "sc-list" }, d.same_ip_accounts.map((a) => el("div", { class: `sc-row static sc-same-ip${a.flagged ? " flagged" : ""}`, "data-user": a.id },
        el("div", { class: "sc-row-who" },
          el("button", { class: "link", onclick: () => openPerson(a.id) }, a.display_name), el("span", { class: "muted" }, ` @${a.username}`),
          el("span", { class: "rp-chip" }, a.age_bracket === "teen" ? "Teen" : "Adult"),
          ...accountStatusChips(a),
          el("span", { class: "sc-when" }, `last seen ${scWhen(a.last_seen)}`)),
        el("div", { class: "muted small" }, `Shared: ${a.shared_ips.join(", ")}${a.local_only ? " (LAN or proxy addresses only: a very weak hint)" : ""}`))))
        : el("p", { class: "muted small sc-tight" }, "Nobody else."));
  } }, "Show IP addresses");
  return el("section", { class: "sc-ips", "data-ips-for": u.id },
    el("h3", {}, "IP addresses"),
    el("p", { class: "muted small sc-tight" }, u.ip_note || ""),
    el("p", { class: "muted small sc-tight" }, "Only you can see these. Each time you look, it's written to the safety log."),
    show, body);
}

// A same-IP hint: the new account and the suspended or banned one.
function ipMatchView(d, openUser) {
  const s = d.snapshot || {};
  const card = (title, a, extra) => el("div", { class: "sc-ipm-card" },
    el("div", { class: "muted small" }, title),
    el("div", { class: "sc-row-who" }, el("strong", {}, a?.display_name || "?"), el("span", { class: "muted" }, ` @${a?.username || "?"}`),
      el("span", { class: "rp-chip" }, a?.age_bracket === "teen" ? "Teen" : "Adult"), ...(extra || [])),
    el("div", { class: "muted small" }, `Joined ${scWhen(a?.created_at)}`),
    a?.id ? el("button", { class: "link", onclick: () => openUser(a.id) }, "Open in People") : null);
  return el("div", { class: "sc-ipm" },
    el("p", { class: "sc-tight" }, d.report.preview),
    el("div", { class: "sc-ipm-cards" },
      card(s.trigger === "register" ? "New account" : "Signed in", s.account),
      card("Seen on the same IP", s.matched, accountStatusChips(s.matched || {}))),
    el("p", { class: "muted small sc-tight" }, `${s.note || ""} Nothing has been done to either account. Open them in People to compare their IP addresses (each look is logged).`));
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
    const ctx = {
      reportId: null,
      post: (body) => tryApi("POST", `/api/safety/users/${uid}/actions`, body),
      refresh: async () => { const f = await tryApi("GET", `/api/safety/users/${uid}`).catch(() => null); if (f && stillMine()) paint(f); },
    };
    const paint = (u) => detail.replaceChildren(
      el("h3", {}, u.display_name),
      subjectView(u, act),
      ipSection(u, showPerson),
      el("section", { class: "sc-attempts-section" }, el("h3", {}, `Attempts from blocked networks (${u.blocked_attempts?.length || 0})`),
        u.ip_blocks ? el("p", { class: "muted small sc-tight" }, `${scPlural(u.ip_blocks, "network")} blocked because of them (Safety → Blocked networks).`) : null,
        attemptsView(u.blocked_attempts)),
      accountActions(u, act, null, ctx),
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

async function drawBlockedNetworks(page, { stillMine, openUser, redraw }) {
  const data = await tryApi("GET", "/api/safety/ip-blocks").catch(() => null);
  if (!data || !stillMine()) return;
  const until = (b) => (b.expires_at ? (b.active ? `until ${scWhen(b.expires_at)}` : `ended ${scWhen(b.expires_at)}`) : "until you remove it");
  const extend = async (b) => {
    const f = await formDialog(`Extend the block on ${b.network}`, [{ name: "days", label: "Block it", type: "select", value: "", options: [["", "Until I remove it"], ["365", "For 1 year from now"], ["90", "For 90 days from now"]] }], "Save");
    if (!f) return;
    if (await tryApi("POST", `/api/safety/ip-blocks/${b.id}/extend`, { days: f.days ? Number(f.days) : null }).catch(() => null)) { toast("Block updated."); redraw(); }
  };
  const remove = async (b) => {
    if (!(await confirmBox(`Remove the block on ${b.network}?`, "People can sign up and sign in from it again. The record of the block and its attempts is kept.", "Remove block"))) return;
    if (await tryApi("POST", `/api/safety/ip-blocks/${b.id}/remove`).catch(() => null)) { toast("Block removed."); redraw(); }
  };
  page.replaceChildren(
    el("h2", {}, "Blocked networks"),
    el("p", { class: "muted small", style: "margin:0" }, "IP addresses you blocked when you suspended someone permanently (IPv4: that address; IPv6: its /64). Nobody can sign up, sign in or use Outhouse from them, except you and people you allow (People → Allow from blocked networks). Every refused attempt is kept and goes in that person's evidence package."),
    el("p", { class: "muted small", style: "margin:0" }, data.note),
    data.blocks.length ? el("div", { class: "sc-table-wrap" }, el("table", { class: "sc-table sc-blocks" },
      el("thead", {}, el("tr", {}, ["Network", "Account", "Reason", "Blocked", "Lasts", "Attempts", ""].map((h) => el("th", {}, h)))),
      el("tbody", {}, data.blocks.map((b) => el("tr", { class: b.active ? "" : "expired", "data-block": b.id },
        el("td", {}, el("code", {}, b.network)),
        el("td", {}, b.account ? el("button", { class: "link", onclick: () => openUser(b.account.id) }, b.account.display_name || `@${b.account.username}`) : "—"),
        el("td", {}, b.reason || "—"),
        el("td", {}, scWhen(b.created_at)),
        el("td", {}, until(b)),
        el("td", {}, String(b.attempts), b.last_attempt ? el("div", { class: "muted small" }, `last ${scWhen(b.last_attempt)}`) : null),
        el("td", { class: "sc-block-btns" },
          el("button", { class: "ghost", onclick: () => extend(b) }, b.active ? "Extend" : "Block again"),
          el("button", { class: "ghost", onclick: () => remove(b) }, "Remove"))))))) 
      : el("div", { class: "sp-empty" }, el("p", { class: "muted" }, "No blocked networks. You can block a permanently suspended person's addresses from their report or from People.")),
    el("h3", {}, `Latest refused attempts (${data.attempts.length})`),
    attemptsView(data.attempts));
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
      item("Child safety open", String(d.urgent), d.urgent ? "bad" : "good"),
      item("Same-IP hints", String(d.ip_matches ?? 0), d.ip_matches ? "warn" : "")),
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
    el("p", {}, "Open the report and use “Hand over to police”. Best with police there (or when they've asked): press “Create a download link for police”, enter your password, and give them the link to open on their own device (or paste it into their portal). Outhouse packs everything into one sealed file on the server: the original files, unchanged and never opened; a JSON file with the report, what the message said (and the messages around it), who sent it and when, who reported it, the account's history and suspension, the IP addresses it used (UTC and Sydney time) and attempts from blocked networks; the safety log; HANDOVER.txt; and SHA-256 hashes of it all. The link works once and expires; then the package is deleted from the server. Every step, and every download (with its IP address), goes in the safety log."),
    el("h3", {}, "The files themselves"),
    el("p", {}, "Never open them, and don't download them yourself. Only use “Download here” if police ask you to, and then hand the file straight over and delete it. If the app can't be used, “Advanced: from the server over SSH” on the report makes the same package on the server. “Export details only” is the JSON alone, without files."),
    el("h3", {}, "Suspend straight away"),
    el("p", {}, "Police advice: suspend the reported account as soon as you see a child-safety report (“Suspend now” on the report). They're only told it's pending review of a report. Lift it if it was a mistake, or make it permanent, and you can block their IP addresses too."),
    el("h3", {}, "After it's dealt with"),
    el("p", {}, "Once police have what they need (or say they don't need it), close the report and use “Delete evidence files” to remove the files from the server for good."));
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
      c.urgent ? el("span", { class: "sc-urgent" }, el("strong", {}, String(c.urgent)), " child safety") : null,
      c.ip_matches ? el("span", { class: "sc-ipm-count" }, el("strong", {}, String(c.ip_matches)), " same-IP hint", c.ip_matches === 1 ? "" : "s") : null),
    data.reports.length
      ? el("div", { class: "sc-list" }, data.reports.slice(0, 10).map((r) => reportRow(r, (id) => openSafetyConsole("open", id))))
      : el("p", { class: "muted" }, "Nothing waiting. All quiet out the back."),
    el("div", {}, el("button", { class: "primary", onclick: () => openSafetyConsole() }, "Open the safety console")));
}

// ---------- Settings → Privacy: the IP address line ----------

if (typeof drawPrivacyPage === "function") {
  const ipBasePrivacy = drawPrivacyPage;
  drawPrivacyPage = async function (main, ...rest) {
    const out = await ipBasePrivacy(main, ...rest);
    const days = await ipRetentionDays();
    if (main.isConnected) {
      main.append(el("section", { class: "notify-box", id: "privacy-ips" },
        el("h3", {}, "IP addresses"),
        el("p", { class: "muted small privacy-ip-note" }, ipRetentionLine(days))));
    }
    return out;
  };
}
