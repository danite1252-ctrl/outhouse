// Time tags: <t:1767225600> and <t:1767225600:STYLE> (the same syntax as
// Discord, so habits carry over). Each reader sees the time in their own
// time zone. Styles: t 8:30 pm, T 8:30:00 pm, d 1/01/2026, D 1 January 2026,
// f (default) 1 January 2026 at 8:30 pm, F Thursday, 1 January 2026 at
// 8:30 pm, R "in 2 hours". Only Intl does the formatting.
//
// The formatting functions at the top don't touch the page, so
// ci/timetags_check.mjs can run them in Node with a fixed time zone.

const TIME_TAG_RE = /<t:(-?\d{1,13})(?::([tTdDfFR]))?>/g;
const TIME_TAG_STYLES = [
  ["t", "Short time"],
  ["T", "Long time"],
  ["d", "Short date"],
  ["D", "Long date"],
  ["f", "Date and time"],
  ["F", "Day, date and time"],
  ["R", "Relative"],
];

// Seconds since 1970 → a Date, or null when it's out of range.
function timeTagDate(unix) {
  const n = Number(unix);
  if (!Number.isSafeInteger(n)) return null;
  const d = new Date(n * 1000);
  return Number.isNaN(d.getTime()) ? null : d;
}

const TIME_TAG_OPTS = {
  t: { hour: "numeric", minute: "2-digit" },
  T: { hour: "numeric", minute: "2-digit", second: "2-digit" },
  d: { day: "numeric", month: "numeric", year: "numeric" },
  D: { day: "numeric", month: "long", year: "numeric" },
  f: { day: "numeric", month: "long", year: "numeric", hour: "numeric", minute: "2-digit" },
  F: { weekday: "long", day: "numeric", month: "long", year: "numeric", hour: "numeric", minute: "2-digit" },
};

// "in 2 hours", "3 days ago".
function relativeTimeText(date, { locale, now = Date.now() } = {}) {
  const secs = Math.round((date.getTime() - now) / 1000);
  const abs = Math.abs(secs);
  const steps = [[60, "second", 1], [3600, "minute", 60], [86400, "hour", 3600], [86400 * 30, "day", 86400], [86400 * 365, "month", 86400 * 30]];
  let unit = "year";
  let size = 86400 * 365;
  for (const [limit, u, s] of steps) {
    if (abs < limit) { unit = u; size = s; break; }
  }
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "always" });
  const n = Math.round(abs / size) * Math.sign(secs);
  return rtf.format(n || 0, unit);
}

// The text for one tag. opts: { locale, timeZone, now } (all optional; the
// defaults are the reader's own).
function formatTimeTag(unix, style = "f", opts = {}) {
  const date = timeTagDate(unix);
  if (!date) return null;
  if (style === "R") return relativeTimeText(date, opts);
  const o = TIME_TAG_OPTS[style] ?? TIME_TAG_OPTS.f;
  try {
    return new Intl.DateTimeFormat(opts.locale, { ...o, timeZone: opts.timeZone }).format(date);
  } catch {
    return null;
  }
}

function readerTimeZone() {
  try { return Intl.DateTimeFormat().resolvedOptions().timeZone || "your time zone"; } catch { return "your time zone"; }
}

// The hover/long-press text: the whole thing in your zone, then UTC.
function timeTagDetails(unix, opts = {}) {
  const date = timeTagDate(unix);
  if (!date) return null;
  const zone = opts.timeZone || readerTimeZone();
  const local = new Intl.DateTimeFormat(opts.locale, { ...TIME_TAG_OPTS.F, timeZone: opts.timeZone, timeZoneName: "short" }).format(date);
  const utc = new Intl.DateTimeFormat(opts.locale, { weekday: "short", day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23", timeZone: "UTC" }).format(date);
  return { local, zone, utc: `${utc} UTC`, relative: relativeTimeText(date, opts) };
}

// Message text with every tag written out (notifications, reply and DM
// previews): "R" tags get the date and time, since they'd go stale.
function plainTimeTags(text, opts = {}) {
  return String(text ?? "").replace(TIME_TAG_RE, (whole, unix, style) =>
    formatTimeTag(unix, style === "R" ? "f" : style || "f", opts) ?? whole);
}

if (typeof globalThis !== "undefined") {
  Object.assign(globalThis, { TIME_TAG_RE, TIME_TAG_STYLES, formatTimeTag, timeTagDetails, plainTimeTags, relativeTimeText });
}

// ---------------------------------------------------------------------------
// In the page.
// ---------------------------------------------------------------------------
// The clock button's picker (set below when there's a page).
var openTimeTagPicker = () => null;

if (typeof document !== "undefined" && typeof renderText === "function") {
  // One chip: <time class="timetag">.
  const timeChip = (whole, unix, style) => {
    const s = style || "f";
    const text = formatTimeTag(unix, s);
    if (text == null) return whole;
    const d = timeTagDate(unix);
    return el("time", { class: `timetag${s === "R" ? " tt-rel" : ""}`, datetime: d.toISOString(), "data-ts": String(unix), "data-style": s, tabindex: "0" }, text);
  };

  // Tags in a bit of plain text → text and chips.
  const tagify = (str) => {
    if (!str.includes("<t:")) return [str];
    const out = [];
    let last = 0;
    for (const m of str.matchAll(TIME_TAG_RE)) {
      if (m.index > last) out.push(str.slice(last, m.index));
      out.push(timeChip(m[0], m[1], m[2]));
      last = m.index + m[0].length;
    }
    if (last < str.length) out.push(str.slice(last));
    return out;
  };
  const tagifyParts = (parts) => parts.flatMap((p) => (typeof p === "string" ? tagify(p) : [p]));

  // renderText: tags outside `code` (and ``` blocks) become chips. Code is
  // already a <code> node by now, so only the loose text is looked at.
  const baseRenderText = renderText;
  renderText = function (text) {
    const s = String(text ?? "");
    if (!s.includes("<t:")) return baseRenderText(s);
    if (!s.includes("```")) return tagifyParts(baseRenderText(s));
    const out = [];
    for (const [i, seg] of s.split(/(```[\s\S]*?```)/).entries()) {
      if (!seg) continue;
      out.push(...(i % 2 ? baseRenderText(seg) : tagifyParts(baseRenderText(seg))));
    }
    return out;
  };

  // Search results with the words marked: chips there too.
  if (typeof highlighted === "function") {
    const baseHighlighted = highlighted;
    highlighted = function (text, terms) {
      const r = baseHighlighted(text, terms);
      return Array.isArray(r) ? tagifyParts(r) : tagify(r);
    };
  }

  // ---- the hover / long-press card ----
  let tip = null;
  let tipFor = null;
  const hideTip = () => { tip?.remove(); tip = null; tipFor = null; };
  const showTip = (chip) => {
    if (tipFor === chip && tip) return;
    hideTip();
    const info = timeTagDetails(chip.dataset.ts);
    if (!info) return;
    tipFor = chip;
    tip = el("div", { class: "tt-tip", role: "tooltip" },
      el("div", { class: "tt-tip-local" }, info.local),
      el("div", { class: "tt-tip-zone" }, `🌏 ${info.zone}`),
      el("div", { class: "tt-tip-utc" }, info.utc),
      el("div", { class: "tt-tip-rel" }, info.relative));
    document.body.append(tip);
    placeTip();
  };
  // Above the chip (below it when there's no room); gone if the chip is.
  const placeTip = () => {
    if (!tip || !tipFor) return;
    const r = tipFor.getBoundingClientRect();
    if (!tipFor.isConnected || r.bottom < 0 || r.top > window.innerHeight) { hideTip(); return; }
    const t = tip.getBoundingClientRect();
    const x = Math.max(8, Math.min(r.left + r.width / 2 - t.width / 2, window.innerWidth - t.width - 8));
    const above = r.top - t.height - 8;
    tip.style.left = `${x}px`;
    tip.style.top = `${above >= 8 ? above : r.bottom + 8}px`;
  };
  const chipFrom = (e) => e.target?.closest?.(".timetag");
  // On touch screens a tap also sends mouseover and focus: only the tap counts there.
  const coarse = () => matchMedia("(pointer: coarse)").matches;
  document.addEventListener("mouseover", (e) => {
    if (coarse()) return;
    const c = chipFrom(e);
    if (c) showTip(c);
    else if (tip && !e.target.closest?.(".tt-tip")) hideTip();
  });
  document.addEventListener("focusin", (e) => { const c = !coarse() && chipFrom(e); if (c) showTip(c); });
  document.addEventListener("focusout", (e) => { if (chipFrom(e) === tipFor) hideTip(); });
  // Tap (phones) toggles it; long-press shows it instead of the message menu.
  document.addEventListener("click", (e) => {
    const c = chipFrom(e);
    if (!c) { if (tip) hideTip(); return; }
    if (coarse()) {
      e.stopPropagation();
      if (tipFor === c) hideTip(); else showTip(c);
    }
  }, true);
  document.addEventListener("contextmenu", (e) => {
    const c = chipFrom(e);
    if (!c) return;
    e.preventDefault();
    e.stopPropagation();
    showTip(c);
  }, true);
  // The list can scroll under it (new messages, pictures loading): follow the chip.
  document.addEventListener("scroll", () => placeTip(), true);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") hideTip(); });

  // "in 2 hours" keeps up (every 30 s, only while you can see it).
  const refreshRelative = () => {
    if (document.hidden) return;
    for (const c of document.querySelectorAll(".timetag.tt-rel")) {
      const t = formatTimeTag(c.dataset.ts, "R");
      if (t && c.textContent !== t) c.textContent = t;
    }
    if (tip && tipFor) {
      const rel = tip.querySelector(".tt-tip-rel");
      const info = timeTagDetails(tipFor.dataset.ts);
      if (rel && info) rel.textContent = info.relative;
    }
  };
  setInterval(refreshRelative, 30_000);
  document.addEventListener("visibilitychange", refreshRelative);

  // ---- the composer's clock button and picker ----
  const pad = (n) => String(n).padStart(2, "0");
  const dateValue = (d) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  const timeValue = (d) => `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  // The next round half-hour from now (local time).
  const nextHalfHour = (now = new Date()) => {
    const d = new Date(now);
    d.setSeconds(0, 0);
    d.setMinutes(d.getMinutes() < 30 ? 30 : 60);
    return d;
  };

  const CLOCK_SVG = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/></svg>';

  const insertAtCaret = (ta, text) => {
    const start = ta.selectionStart ?? ta.value.length;
    const end = ta.selectionEnd ?? start;
    const before = ta.value.slice(0, start);
    const after = ta.value.slice(end);
    const lead = before && !/\s$/.test(before) ? " " : "";
    const trail = after && !/^\s/.test(after) ? " " : "";
    const piece = lead + text + trail;
    ta.value = before + piece + after;
    const pos = start + piece.length;
    ta.focus();
    ta.setSelectionRange(pos, pos);
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  };

  // Opens the picker for a text box. Returns the popover.
  openTimeTagPicker = function (anchor, ta, initial = null) {
    if (popover?.classList.contains("tt-pop")) { closePopover(); return null; }
    const start = initial ?? nextHalfHour();
    const date = el("input", { type: "date", class: "tt-date", value: dateValue(start), required: true, "aria-label": "Date" });
    const time = el("input", { type: "time", class: "tt-time", value: timeValue(start), step: "60", required: true, "aria-label": "Time" });
    const style = el("select", { class: "select tt-style", "aria-label": "Style" });
    const preview = el("div", { class: "tt-preview" });
    const raw = el("code", { class: "tt-raw" });
    const insert = el("button", { type: "submit", class: "primary tt-insert" }, "Insert");
    const chosen = () => {
      if (!date.value || !time.value) return null;
      const d = new Date(`${date.value}T${time.value}`);
      return Number.isNaN(d.getTime()) ? null : Math.floor(d.getTime() / 1000);
    };
    const paint = () => {
      const unix = chosen();
      const keep = style.value || "f";
      style.replaceChildren(...TIME_TAG_STYLES.map(([s, label]) =>
        el("option", { value: s, selected: s === keep }, unix == null ? label : `${label}: ${formatTimeTag(unix, s)}`)));
      style.value = keep;
      insert.disabled = unix == null;
      if (unix == null) { preview.replaceChildren("Pick a date and time."); raw.textContent = ""; return; }
      const tag = `<t:${unix}:${style.value}>`;
      preview.replaceChildren(...renderText(tag));
      raw.textContent = tag;
    };
    for (const i of [date, time]) { i.addEventListener("input", paint); i.addEventListener("change", paint); }
    style.addEventListener("change", paint);
    style.value = "f";
    const form = el("form", { class: "tt-form", onsubmit: (e) => {
      e.preventDefault();
      const unix = chosen();
      if (unix == null) return;
      closePopover();
      insertAtCaret(ta, `<t:${unix}:${style.value || "f"}>`);
    } },
      el("div", { class: "tt-grab", "aria-hidden": "true" }),
      el("div", { class: "tt-head" }, el("strong", {}, "Insert a time"),
        el("span", { class: "muted small" }, "Everyone sees it in their own time zone.")),
      el("div", { class: "tt-row" },
        el("label", { class: "tt-field" }, el("span", {}, "Date"), date),
        el("label", { class: "tt-field" }, el("span", {}, "Time"), time)),
      el("label", { class: "tt-field" }, el("span", {}, "Style"), style),
      el("div", { class: "tt-show" }, preview, raw),
      el("p", { class: "muted small tt-hint" }, "You can type them too: <t:seconds since 1970:style>, like Discord. Styles t T d D f F R."),
      el("div", { class: "tt-actions" },
        el("button", { type: "button", class: "ghost", onclick: () => closePopover() }, "Cancel"),
        insert));
    paint();
    const pop = openPopover(anchor, form, "above");
    pop.classList.add("tt-pop");
    if (matchMedia("(max-width: 760px)").matches) {
      pop.classList.add("tt-sheet");
      pop.style.left = "";
      pop.style.top = "";
    }
    if (!matchMedia("(pointer: coarse)").matches) date.focus();
    return pop;
  };

  const clockButton = (cls, getTa) => {
    const b = el("button", { type: "button", class: `tool tt-btn ${cls}`, title: "Insert a time (shows in each reader's time zone)", "aria-label": "Insert a time" });
    b.innerHTML = CLOCK_SVG;
    b.addEventListener("click", (e) => {
      e.preventDefault();
      const ta = getTa(b);
      if (ta && !ta.disabled) openTimeTagPicker(b, ta);
    });
    return b;
  };

  // The channel composer: next to the emoji button.
  (function composerClock() {
    const emoji = document.getElementById("emoji-btn");
    if (!emoji || document.getElementById("time-btn")) return;
    const b = clockButton("", () => document.getElementById("composer-input"));
    b.id = "time-btn";
    emoji.before(b);
  })();

  // The thread composer.
  if (typeof threadComposer === "function") {
    const baseThreadComposer = threadComposer;
    threadComposer = function () {
      const node = baseThreadComposer();
      const form = node?.querySelector?.(".thread-composer");
      const send = form?.querySelector(".send");
      if (form && send && !send.disabled) {
        // Before the voice message mic (if there is one), like the channel composer.
        (form.querySelector(".vm-mic") || send).before(clockButton("thread-time-btn", (btn) => btn.closest("form")?.querySelector("textarea")));
      }
      return node;
    };
  }

  // Clicking inside the picker mustn't close it (the clock buttons toggle it).
  document.addEventListener("mousedown", (e) => {
    if (e.target.closest?.(".tt-btn") && popover?.classList.contains("tt-pop")) e.stopPropagation();
  }, true);
}
