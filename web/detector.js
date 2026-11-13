// Outhouse web client: the image detector. Loaded after safety.js (uses
// its globals and app.js's: el, api, tryApi, state, toast, renderAttachments,
// messageCard, filesView, reportRow, drawServerStatus, scSection, scWhen, …).
//
// The server scans uploaded pictures with a small local model. A picture it
// holds back arrives with `hold` set and no link:
//   checking  still being scanned (pictures to or from teens)
//   held      held for review; only the sender and the site owner can look
//             (never for pictures sent to or from a teen)
//   blur      blurred; anyone can click through
// Avatars, banners, emoji and stickers it holds are swapped for a
// placeholder by the server, so nothing is needed here for those.
//
// It also finds faces and estimates their ages (a preliminary automated
// estimate, not evidence). Explicit pictures with a face estimated clearly
// young are escalated to child safety by the server; the console shows the
// estimates as metadata only.
"use strict";

const DT_AGE_NOTE = "preliminary automated estimate, not evidence";

const dtHeld = (a) => !!a?.hold;
const dtRevealed = new Map(); // attachment ID → blob: URL (only for this page)

function dtShield() {
  return typeof svgIcon === "function"
    ? svgIcon(["M12 3l8 3v6c0 5-3.5 8-8 9-4.5-1-8-4-8-9V6z", "M9.5 12l2 2 3.5-4"], 18)
    : el("span", { "aria-hidden": "true" }, "🛡");
}

async function dtReveal(a) {
  if (dtRevealed.has(a.id)) return dtRevealed.get(a.id);
  const res = await fetch(`/api/attachments/${a.id}/reveal`, { headers: { Authorization: `Bearer ${state.token}` } });
  if (!res.ok) {
    const body = await res.json().catch(() => null);
    throw new Error(body?.error || `Couldn't load it (${res.status})`);
  }
  const url = URL.createObjectURL(await res.blob());
  dtRevealed.set(a.id, url);
  return url;
}

// May I look at a held picture? (The server checks again.)
function dtCanReveal(m, a) {
  if (a.hold?.reveal === "anyone") return true;
  if (a.hold?.reveal !== "sender_owner") return false;
  return m.author_id === state.me?.id || !!state.siteOwner;
}

// One held, checking or blurred picture in a message.
function dtHoldNode(m, a) {
  const h = a.hold;
  if (h.state === "blur") {
    const img = el("img", { src: a.url, alt: a.filename, loading: "lazy" });
    const box = el("div", { class: "dt-blur", "data-attachment": a.id });
    const cover = el("button", { type: "button", class: "dt-cover", title: "Show the picture" },
      dtShield(), el("span", { class: "dt-cover-text" }, "Possibly explicit · click to show"));
    cover.addEventListener("click", (e) => {
      e.stopPropagation();
      box.classList.add("shown");
      cover.remove();
      img.addEventListener("click", () => openLightbox(a.url));
    });
    box.append(img, cover);
    return box;
  }
  const checking = h.state === "checking";
  const mine = m.author_id === state.me?.id;
  const box = el("div", { class: `dt-hold${checking ? " dt-checking" : ""}`, "data-attachment": a.id, role: "note" },
    checking ? el("span", { class: "dt-spinner", "aria-hidden": "true" }) : dtShield(),
    el("div", { class: "dt-hold-text" },
      el("strong", {}, checking ? "Checking…" : "Held for review"),
      el("span", { class: "dt-hold-sub" }, checking
        ? "This picture is being checked. It'll show in a moment."
        : h.reveal === "none"
          ? "This picture isn't shown here."
          : mine ? "Only you and the site owner can see it until it's approved." : "The site owner will look at it.")));
  if (!checking && dtCanReveal(m, a)) {
    const btn = el("button", { type: "button", class: "ghost dt-show" }, mine ? "Show it (only you)" : "Show it (site owner)");
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      btn.disabled = true;
      try {
        const url = await dtReveal(a);
        const img = el("img", { src: url, alt: a.filename, class: "dt-revealed-img" });
        img.addEventListener("click", () => openLightbox(url));
        box.replaceWith(el("div", { class: "dt-revealed", "data-attachment": a.id }, img,
          el("span", { class: "dt-revealed-note" }, "Held for review · only you can see this")));
      } catch (err) {
        btn.disabled = false;
        toast(err.message);
      }
    });
    box.append(btn);
  }
  return box;
}

// Messages, threads, forwards: held pictures get a cover instead.
const dtBaseRenderAttachments = renderAttachments;
renderAttachments = function (m) {
  const held = (m.attachments || []).filter(dtHeld);
  if (!held.length) return dtBaseRenderAttachments(m);
  const node = dtBaseRenderAttachments({ ...m, attachments: m.attachments.filter((a) => !dtHeld(a)) });
  const covers = held.map((a) => dtHoldNode(m, a));
  // Inside a forward's box when it's one; otherwise under the other files.
  const target = node?.matches?.(".attachments") ? node
    : node?.querySelector?.(".fwd-body") || node?.querySelector?.(".attachments") || null;
  if (target) {
    target.append(...covers);
    return node;
  }
  return el("div", { class: "attachments dt-attachments" }, node, covers);
};

// Pins and search results: no thumbnails of held pictures.
if (typeof messageCard === "function") {
  const dtBaseMessageCard = messageCard;
  messageCard = function (m, opts) {
    const held = (m.attachments || []).filter((a) => dtHeld(a) && a.hold.state !== "blur");
    const blurred = (m.attachments || []).some((a) => a.hold?.state === "blur");
    const node = dtBaseMessageCard(held.length ? { ...m, attachments: m.attachments.filter((a) => !held.includes(a)) } : m, opts);
    if (blurred) node.querySelector(".sp-thumb")?.classList.add("dt-blur-thumb");
    if (held.length) {
      const checking = held.every((a) => a.hold.state === "checking");
      node.append(el("div", { class: "sp-files dt-card-chip" }, dtShield(),
        checking ? " Checking a picture…" : ` ${held.length} picture${held.length === 1 ? "" : "s"} held for review`));
    }
    return node;
  };
}

// Reporting a file: with held pictures in a message the files aren't in
// their usual order on screen, so offer them by name instead.
if (typeof messageReportItems === "function") {
  const dtBaseReportItems = messageReportItems;
  messageReportItems = function (m, e) {
    return dtBaseReportItems(m, m?.attachments?.some(dtHeld) ? null : e);
  };
}

// ======================================================================
// The site owner's console
// ======================================================================

if (typeof reportRow === "function") {
  const dtBaseReportRow = reportRow;
  reportRow = function (r, onOpen) {
    const node = dtBaseReportRow(r, onOpen);
    if (r.source === "detector") {
      node.classList.add("dt-report");
      node.querySelector(".sc-count")?.remove();
      node.querySelector(".sc-row-top")?.prepend(el("span", { class: "rp-chip dt-chip" }, dtShield(), "Image detector"));
    }
    return node;
  };
}

// The detector's line under each reported file (metadata only: the
// picture itself is never shown for child-safety reports).
function dtScoreLine(f, child) {
  const d = f.detector;
  if (!d) return null;
  const box = el("div", { class: `dt-score${d.near_zero ? " low" : ""}${d.status === "flagged" ? " high" : ""}`, "data-key": f.key },
    el("div", { class: "dt-score-line" }, dtShield(), el("span", { class: "dt-score-text" }, d.summary)),
    d.age_line ? el("div", { class: "dt-age-line" }, d.age_line) : null,
    d.age_escalated && !d.age_cleared ? el("div", { class: "dt-age-flag" }, "Escalated to child safety automatically: explicit content with a face estimated young") : null,
    d.age_cleared ? el("div", { class: "muted small" }, "An automatic age escalation for this file was dismissed as a mistake.") : null);
  if (d.near_zero) {
    box.append(el("div", { class: "dt-guide" }, child
      ? `A score this close to zero suggests a false report: you can dismiss it without viewing the file. (Any age shown is a ${DT_AGE_NOTE}, so it's an aid, not proof either way.)`
      : "A score this close to zero suggests nothing explicit: you can decide without viewing the file."));
  } else if (child && d.score != null) {
    box.append(el("div", { class: "dt-guide" }, `The scores are an aid, not proof. Any age shown is a guess from the face alone (a ${DT_AGE_NOTE}): often wrong by years, and drawings can look like faces. Don't open the file; report it to the authorities if you think it's real.`));
  }
  if (f.teen_hold) box.append(el("div", { class: "dt-guide" }, "🔒 Sent to or from a teen: held, and not shown to anyone (you included)."));
  return box;
}

if (typeof filesView === "function") {
  const dtBaseFilesView = filesView;
  filesView = function (d, reportId) {
    const node = dtBaseFilesView(d, reportId);
    if (!node) return node;
    const rows = node.querySelectorAll(".sc-file");
    d.files.forEach((f, i) => {
      const line = dtScoreLine(f, d.report.child_safety);
      if (!line || !rows[i]) return;
      rows[i].querySelector(".sc-file-status")?.after(line);
      if (f.detector?.status === "pending") {
        // Scanning it now: look again in a moment.
        const poll = async (tries) => {
          await new Promise((r) => setTimeout(r, 2500));
          if (!line.isConnected) return;
          const fresh = await api("GET", `/api/safety/reports/${reportId}`).catch(() => null);
          const ff = fresh?.files?.find((x) => x.key === f.key);
          if (ff?.detector && ff.detector.status !== "pending") line.replaceWith(dtScoreLine(ff, d.report.child_safety));
          else if (tries > 0) poll(tries - 1);
        };
        poll(20);
      }
    });
    return node;
  };
}

const DT_WHERE = {
  attachment: "a message",
  avatar: "a profile picture",
  banner: "a profile banner",
  emoji: "a custom emoji",
  sticker: "a sticker",
  guild_icon: "an outhouse picture",
  guild_banner: "an outhouse banner",
};

// An automatic child-safety escalation (the age rule): why, in numbers.
function dtAutoPanel(d) {
  const a = d.snapshot?.auto_escalation;
  if (!a || !d.report.child_safety) return null;
  const facts = [
    a.score != null ? `nudity ${Number(a.score).toFixed(2)}` : null,
    a.faces != null ? `${a.faces === 1 ? "face" : "faces"} ${a.faces}` : null,
    a.min_age != null ? `youngest estimated ~${Math.round(a.min_age)}` : null,
  ].filter(Boolean).join(" · ");
  return scSection("Automatic escalation",
    el("div", { class: "dt-score high dt-auto" }, el("div", { class: "dt-score-line" }, dtShield(), el("span", { class: "dt-score-text" }, a.line))),
    el("div", { class: "muted small" }, `${facts} (${DT_AGE_NOTE}) · rule: nudity ${Number(a.escalation_score).toFixed(2)} or more with a face estimated ${Math.round(a.age_threshold)} or under · ${scWhen(a.at)}`),
    el("div", { class: "dt-guide" }, "The image detector made this a child-safety report on its own, whatever category anyone picked. The age is a guess from the face alone: it's often wrong by years and drawings can look like faces. Judge it by the scores (you can't view the file). If you're sure it's a mistake, dismiss it: the file and message come back and the detector handles the picture as usual."));
}

// The panel on a detector report: what it found and the decisions.
function detectorReportPanel(d, act) {
  const auto = dtAutoPanel(d);
  const x = d.detector;
  if (!x || d.report.source !== "detector") return auto;
  if (d.report.child_safety) return auto;
  const profile = ["avatar", "banner", "emoji", "sticker", "guild_icon", "guild_banner"].includes(x.kind);
  const decided = x.review === "approved" ? "Approved: shown as normal."
    : x.review === "blurred" ? (profile ? "Kept hidden." : x.teen ? "Kept held." : "Kept blurred: people can click through.")
      : null;
  const holdNow = { none: "Shown as normal", held: "Held for review", hidden: "Hidden (placeholder shown)", blur: "Blurred (people can click through)", checking: "Checking…" }[x.hold] || x.hold;
  const labels = (x.labels || []).map((l) => `${String(l.label).toLowerCase().replace(/_/g, " ")} ${Number(l.score).toFixed(2)}`).join(", ");
  return scSection("Image detector",
    el("div", { class: "dt-score high" }, el("div", { class: "dt-score-line" }, dtShield(), el("span", { class: "dt-score-text" }, x.summary))),
    el("div", { class: "muted small" },
      `Found in ${DT_WHERE[x.kind] || "an upload"}`,
      x.teen ? " sent to or from a teen" : "",
      x.adult_only ? " in an adult-only 18+ channel" : "",
      ` · now: ${holdNow}`,
      x.model ? ` · model ${x.model}` : "",
      x.scanned_at ? ` · ${scWhen(x.scanned_at)}` : ""),
    labels ? el("div", { class: "muted small" }, `Labels: ${labels}`) : null,
    x.age_line ? el("div", { class: "dt-age-line" }, x.age_line) : null,
    x.teen ? el("div", { class: "dt-guide" }, "🔒 This was sent to or from a teen, so it stays held and isn't shown to anyone, you included. You can still approve it if you're sure it's harmless, or remove it.") : null,
    el("div", { class: "dt-guide" }, "The detector is an aid, not a guarantee: it misses things and sometimes flags harmless pictures (swimwear, art, skin-coloured objects)."),
    decided ? el("div", { class: "sc-row-res" }, `✓ ${decided}`) : null,
    el("div", { class: "sc-actions dt-actions" },
      x.review !== "approved" ? el("button", { class: "primary dt-approve", onclick: async () => {
        if (await confirmBox("Approve this picture?", "It's shown to everyone who can see it again, straight away. The report is closed.", "Approve")) act({ action: "approve_file" }, "Approved: the picture is shown again.");
      } }, "Approve (release)") : null,
      x.review !== "blurred" ? el("button", { class: "ghost dt-keep", onclick: () => act({ action: "keep_blurred" }, profile ? "Kept hidden." : "Kept blurred.") },
        profile ? "Keep hidden" : x.teen ? "Keep held" : "Keep blurred") : null,
      el("button", { class: "ghost dt-remove", onclick: async () => {
        if (await confirmBox("Remove this picture?", profile ? "It's taken off for good (the report keeps a copy until it's closed)." : "It's taken out of every message it's in (the text stays; the report keeps a copy until it's closed).", "Remove")) act({ action: "remove_file" }, "Picture removed.");
      } }, "Remove file")));
}

// ---------- settings ----------

const DT_SENS = [["low", "Low (only very likely nudity)"], ["medium", "Medium (recommended)"], ["high", "High (catches more, more false alarms)"]];
const DT_ACTION = [["hold", "Hold for approval"], ["blur", "Blur only (viewers can click through)"]];

function dtToggle(on, title, desc, save) {
  const btn = el("button", { type: "button", class: `toggle${on ? " on" : ""}`, "aria-label": title, "aria-pressed": String(!!on) });
  btn.addEventListener("click", async () => {
    const next = !btn.classList.contains("on");
    if (!(await save(next))) return;
    btn.classList.toggle("on", next);
    btn.setAttribute("aria-pressed", String(next));
  });
  return el("div", { class: "sc-toggle-row" }, el("div", {}, el("div", { class: "sc-toggle-title" }, title), el("div", { class: "muted small" }, desc)), btn);
}

function dtSelect(value, options, title, desc, save) {
  const sel = el("select", { "aria-label": title, class: "dt-select" }, options.map(([v, l]) => el("option", { value: v, selected: v === value }, l)));
  sel.addEventListener("change", () => save(sel.value));
  return el("div", { class: "sc-toggle-row" }, el("div", {}, el("div", { class: "sc-toggle-title" }, title), el("div", { class: "muted small" }, desc)), sel);
}

function dtHealthGrid(h) {
  const item = (label, value, cls = "") => el("div", { class: `status-item ${cls}` }, el("div", { class: "status-label" }, label), el("div", { class: "status-value" }, value));
  const now = !h.configured ? ["Not set up (DETECTOR_URL isn't set)", "warn"]
    : !h.enabled ? ["Switched off", "warn"]
      : h.online ? ["Online", "good"] : ["Detector offline: uploads still work; pictures wait in the queue", "bad"];
  return el("div", { class: "status-grid dt-health" },
    item("Image detector", now[0], now[1]),
    item("Models", h.models ? [h.models.nudity, h.models.faces, h.models.age].filter(Boolean).join(" · ") : (h.model || "unknown")),
    item("Waiting to be checked", String(h.queue ?? 0), h.queue > 20 ? "warn" : ""),
    item("Held now", String(h.held ?? 0)),
    h.last_scan_ms != null ? item("Last scan took", `${h.last_scan_ms} ms`) : null,
    h.threshold_override != null ? item("Test threshold", `DETECTOR_THRESHOLD=${h.threshold_override} is set: remove it from .env when you're done`, "bad") : null,
    h.escalation_age_override != null ? item("Test age rule", `DETECTOR_ESCALATION_AGE=${h.escalation_age_override} is set: remove it from .env when you're done`, "bad") : null,
    h.escalation_score_override != null ? item("Test age rule", `DETECTOR_ESCALATION_SCORE=${h.escalation_score_override} is set: remove it from .env when you're done`, "bad") : null,
    h.last_error ? item("Last problem", `${h.last_error}${h.last_error_at ? ` (${relTime(h.last_error_at)})` : ""}`, "bad") : null);
}

async function drawDetectorSettings(page, stillMine = () => true) {
  const data = await tryApi("GET", "/api/safety/detector").catch(() => null);
  if (!data || !stillMine()) return;
  const s = data.settings;
  const save = async (patch) => {
    const saved = await tryApi("PUT", "/api/safety/detector", patch).catch(() => null);
    if (saved) { Object.assign(s, saved); toast("Saved."); }
    return !!saved;
  };
  page.replaceChildren(
    el("h2", {}, "Image detector"),
    el("p", { class: "muted small dt-intro" }, "Uploaded pictures (attachments, avatars, banners, emoji, stickers) are checked for nudity, and faces in them for age, by small models running on this server. No picture ever leaves the server, and only the scores are kept. Checking happens in the background, so chat never waits for it. It's an aid, not a guarantee: it misses some things and flags some harmless pictures."),
    dtHealthGrid(data.health),
    dtToggle(s.enabled, "Check uploaded pictures", "Off: nothing new is checked and nothing is held (reported files are still checked for the review screen).", (v) => save({ enabled: v })),
    dtSelect(s.sensitivity, DT_SENS, "Sensitivity", `How sure the detector must be to flag a picture (scores: low ${data.thresholds.low}, medium ${data.thresholds.medium}, high ${data.thresholds.high}).`, (v) => save({ sensitivity: v })),
    dtSelect(s.action, DT_ACTION, "Outside 18+ channels", "What happens to a flagged picture in an ordinary channel. In an 18+ channel only adults can see it's posted as normal.", (v) => save({ action: v })),
    dtToggle(s.hold_teens, "Hold DM pictures to and from teens until checked", "Shows “Checking…” for a moment instead of the picture. Teen rules always apply: a flagged picture to or from a teen is always held, and teens never receive it.", (v) => save({ hold_teens: v })),
    dtAgeSettings(data.age, save),
    el("h3", {}, "Outhouses"),
    el("p", { class: "muted small dt-intro" }, "Each outhouse can switch the detector off or use its own settings (Outhouse settings → Image detector)."),
    el("div", { class: "sc-list" }, data.outhouses.map((o) => el("div", { class: "sc-toggle-row" },
      el("div", {}, el("div", { class: "sc-toggle-title" }, o.name),
        el("div", { class: "muted small" }, o.enabled
          ? `On · ${o.sensitivity ? `sensitivity ${o.sensitivity}` : "site sensitivity"} · ${o.action ? (o.action === "blur" ? "blur only" : "hold for approval") : "site action"} · ${o.hold_teens ? "holds pictures for teens until checked" : "doesn't hold pictures for teens while checking"}`
          : "Off in this outhouse")),
      el("span", { class: `rp-chip ${o.enabled ? "st-closed" : "st-open"}` }, o.enabled ? "On" : "Off")))));
}

// The age check: explicit + a face estimated clearly young → child safety.
function dtAgeSettings(age, save) {
  if (!age) return null;
  const scores = [...new Set([...age.scores, age.escalation_score])].sort((a, b) => a - b);
  return el("div", { class: "dt-age-settings" },
    el("h3", {}, "Age check"),
    el("p", { class: "muted small dt-intro" }, `Faces in every checked picture get an age estimate (a ${DT_AGE_NOTE}: a small model's guess from the face alone; only the numbers are kept, never the faces). If a picture looks explicit and a face in it is estimated clearly young, it's escalated to child safety straight away, everywhere (18+ channels too): the file is locked for everyone, you included, the message is hidden and you get an urgent alert. A young face without nudity does nothing, so family photos are fine. The age model struggles around 16–20 and with drawings; a wrong escalation is just a locked file you dismiss.`),
    dtToggle(age.enabled, "Escalate explicit pictures with a young-looking face", "Off: ages are still estimated and shown on reports, but nothing is escalated automatically.", (v) => save({ age_check: v })),
    dtSelect(String(age.threshold), age.thresholds.map((t) => [String(t), `${t} or under${t === 13 ? " (recommended)" : ""}`]), "Counts as young", "The youngest face's estimate at or below this, with explicit content, escalates the picture.", (v) => save({ age_threshold: Number(v) })),
    dtSelect(String(age.escalation_score), scores.map((v) => [String(v), `${Number(v).toFixed(2)}${v === 0.4 ? " (recommended)" : ""}`]), "Counts as explicit (for this check)", "The nudity score that counts here. Lower than the normal sensitivity on purpose: it's the combination that matters.", (v) => save({ escalation_score: Number(v) })));
}

// Outhouse settings → Image detector (site owner only).
async function drawDetectorGuildSettings(page) {
  const gid = state.guildId;
  const g = await tryApi("GET", `/api/safety/guilds/${gid}/detector`).catch(() => null);
  if (!g) return;
  const save = async (patch) => {
    const saved = await tryApi("PUT", `/api/safety/guilds/${gid}/detector`, patch).catch(() => null);
    if (saved) toast("Saved.");
    return !!saved;
  };
  const siteSens = DT_SENS.find(([v]) => v === g.site.sensitivity)?.[0] || "medium";
  const siteAction = g.site.action === "blur" ? "blur only" : "hold for approval";
  page.replaceChildren(
    el("h2", {}, "Image detector"),
    el("p", { class: "muted small dt-intro" }, `Pictures uploaded in ${g.name} are checked on this server (they never leave it). Only you, the site owner, can change this. Site-wide settings are in Reports → Image detector.`),
    g.site.enabled ? null : el("p", { class: "dt-guide" }, "The detector is switched off for the whole site, so these settings don't do anything right now."),
    dtToggle(g.enabled, "Check pictures in this outhouse", "Off: nothing posted here is checked or held.", (v) => save({ enabled: v })),
    dtSelect(g.sensitivity || "default", [["default", `Site setting (${siteSens})`], ...DT_SENS], "Sensitivity", "How sure the detector must be to flag a picture.", (v) => save({ sensitivity: v })),
    dtSelect(g.action || "default", [["default", `Site setting (${siteAction})`], ...DT_ACTION], "Outside 18+ channels", "Flagged pictures in an 18+ channel only adults can see are posted as normal.", (v) => save({ action: v })),
    dtToggle(g.hold_teens, "Hold images for teens until checked", "Pictures in channels a teen can see (and from teens) show “Checking…” for a moment until they've been checked.", (v) => save({ hold_teens: v })));
}

// Server status: the detector's health and queue.
if (typeof drawServerStatus === "function") {
  const dtBaseDrawServerStatus = drawServerStatus;
  drawServerStatus = async function (main) {
    await dtBaseDrawServerStatus(main);
    const s = await api("GET", "/api/safety/server-status").catch(() => null);
    if (!s?.detector || !main.isConnected) return;
    const block = el("div", { class: "dt-status" }, el("h3", {}, "Image detector"), dtHealthGrid(s.detector));
    const refresh = main.querySelector("button.ghost:last-of-type")?.parentElement;
    if (refresh) refresh.before(block); else main.append(block);
  };
}
