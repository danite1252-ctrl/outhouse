// Outhouse web client: automod settings. Edit channel → Automod (word
// filter, spam limits, who's exempt, alerts), Outhouse settings → Automod
// (the outhouse-wide blocked words channels can use), and the channel's
// "Hide threads after inactivity" default on the Overview tab.
"use strict";

const AUTOMOD_TIMEOUTS = [[0, "Don't time out"], [1, "1 minute"], [5, "5 minutes"], [10, "10 minutes"], [60, "1 hour"], [1440, "1 day"]];

// Blocked words are typed one per line (commas work too).
const parseWordList = (text) => text.split(/[\n,]/).map((w) => w.trim()).filter(Boolean);

function amSwitch(checked, label, onChange) {
  const input = el("input", { type: "checkbox", role: "switch", class: "am-switch-input" });
  input.checked = checked;
  input.addEventListener("change", () => onChange(input.checked));
  return el("label", { class: "am-switch" }, input, el("span", { class: "am-switch-track", "aria-hidden": "true" }), el("span", { class: "am-switch-label" }, label));
}

function amCheck(checked, label, hint, onChange) {
  const input = el("input", { type: "checkbox" });
  input.checked = checked;
  input.addEventListener("change", () => onChange(input.checked));
  return el("label", { class: "am-check" }, input, el("span", {}, el("span", { class: "am-check-label" }, label), hint ? el("span", { class: "am-hint" }, hint) : null));
}

function amNumber(value, { min = 0, max = 50, label, onChange }) {
  const input = el("input", { type: "number", min, max, step: 1, class: "am-num", "aria-label": label, inputmode: "numeric" });
  input.value = String(value);
  input.addEventListener("input", () => {
    const n = Math.max(min, Math.min(max, Math.round(Number(input.value) || 0)));
    onChange(n);
  });
  return input;
}

async function drawChannelAutomod(main, ch) {
  main.replaceChildren(el("div", { class: "ec-page" }, el("h2", {}, "Automod"), el("p", { class: "muted" }, "Loading…")));
  const [cfg, defaults] = await Promise.all([
    api("GET", `/api/channels/${ch.id}/automod`).catch((err) => { toast(err.message); return null; }),
    api("GET", `/api/guilds/${ch.guild_id}/automod`).catch(() => ({ words: [] })),
  ]);
  if (!cfg) return;
  if (!state.roles?.some((r) => r.id === ch.guild_id)) {
    state.roles = await api("GET", `/api/guilds/${ch.guild_id}/roles`).catch(() => state.roles || []);
  }
  const saved = JSON.stringify(cfg);
  const words = el("textarea", { rows: 5, class: "am-words", placeholder: "One per line, like:\nbad word\ndang*", "aria-label": "Blocked words", spellcheck: "false" });
  words.value = cfg.words.join("\n");
  words.addEventListener("input", () => { cfg.words = parseWordList(words.value); paint(); });

  const wordBody = el("div", { class: "am-body" });
  const spamBody = el("div", { class: "am-body" });
  const actionCards = [["block", "🚫", "Block the message", "They see “Your message was blocked by automod” and keep their text to fix it."],
    ["hide", "🚧", "Let it through, but hide it", "Everyone else sees a “Filtered” notice they can click to read it."]].map(([key, icon, name, desc]) => {
    const radio = el("input", { type: "radio", name: `am-action-${ch.id}`, value: key });
    radio.checked = cfg.word_action === key;
    radio.addEventListener("change", () => { if (radio.checked) { cfg.word_action = key; paint(); } });
    return el("label", { class: "ec-mode am-action" }, radio, el("span", { class: "ec-mode-icon", "aria-hidden": "true" }, icon),
      el("span", { class: "ec-mode-text" }, el("span", { class: "ec-mode-name" }, name), el("span", { class: "ec-mode-desc" }, desc)));
  });
  wordBody.append(
    el("label", { class: "field" }, "Blocked words and phrases", words,
      el("span", { class: "ec-hint" }, "Not case sensitive. Use * as a wildcard: ", el("code", {}, "dang*"), " catches dang, danged and dangit.")),
    amCheck(cfg.use_default_words, `Also use the outhouse-wide list (${defaults.words.length} ${defaults.words.length === 1 ? "word" : "words"})`,
      can("manage_guild") ? "Edit it in Outhouse settings → Automod." : "People who can manage the outhouse look after it.", (v) => { cfg.use_default_words = v; paint(); }),
    amCheck(cfg.whole_word, "Whole words only", "“ass” won't catch “class”.", (v) => { cfg.whole_word = v; paint(); }),
    amCheck(cfg.leetspeak, "Catch leetspeak", "Reads n00b as noob, $hit as shit, and so on.", (v) => { cfg.leetspeak = v; paint(); }),
    el("div", { class: "field" }, "When a message has a blocked word", el("div", { class: "ec-modes" }, actionCards)));

  const row = (...kids) => el("div", { class: "am-row" }, ...kids);
  const timeout = el("select", { class: "select am-select", "aria-label": "Time out for" },
    AUTOMOD_TIMEOUTS.map(([m, label]) => el("option", { value: String(m), selected: m === cfg.timeout_minutes }, label)));
  if (!AUTOMOD_TIMEOUTS.some(([m]) => m === cfg.timeout_minutes)) timeout.append(el("option", { value: String(cfg.timeout_minutes), selected: true }, `${cfg.timeout_minutes} minutes`));
  timeout.addEventListener("change", () => { cfg.timeout_minutes = Number(timeout.value); paint(); });
  spamBody.append(
    row(el("span", { class: "am-row-label" }, "Most messages"), amNumber(cfg.max_messages, { max: 50, label: "Most messages", onChange: (n) => { cfg.max_messages = n; paint(); } }),
      el("span", {}, "every"), amNumber(cfg.per_seconds, { min: 1, max: 300, label: "Seconds", onChange: (n) => { cfg.per_seconds = n; paint(); } }), el("span", {}, "seconds")),
    amCheck(cfg.block_duplicates, "Block repeats", "The same message three times in a minute.", (v) => { cfg.block_duplicates = v; paint(); }),
    row(el("span", { class: "am-row-label" }, "Most @mentions per message"), amNumber(cfg.max_mentions, { label: "Most mentions", onChange: (n) => { cfg.max_mentions = n; paint(); } })),
    row(el("span", { class: "am-row-label" }, "Most links per message"), amNumber(cfg.max_links, { label: "Most links", onChange: (n) => { cfg.max_links = n; paint(); } })),
    row(el("span", { class: "am-row-label" }, "Most attachments per message"), amNumber(cfg.max_attachments, { max: 10, label: "Most attachments", onChange: (n) => { cfg.max_attachments = n; paint(); } })),
    el("p", { class: "ec-hint" }, "0 means no limit."),
    row(el("span", { class: "am-row-label" }, "After"), amNumber(cfg.timeout_after, { min: 1, max: 20, label: "Blocked messages", onChange: (n) => { cfg.timeout_after = n; paint(); } }),
      el("span", {}, "blocked messages in 10 minutes:"), timeout));

  const roles = (state.roles || []).filter((r) => r.id !== ch.guild_id);
  const roleChips = el("div", { class: "am-roles" }, roles.length ? roles.map((r) => {
    const input = el("input", { type: "checkbox", value: r.id });
    input.checked = cfg.exempt_roles.includes(r.id);
    input.addEventListener("change", () => {
      cfg.exempt_roles = input.checked ? [...new Set([...cfg.exempt_roles, r.id])] : cfg.exempt_roles.filter((x) => x !== r.id);
      paint();
    });
    return el("label", { class: "am-role" }, input, el("span", { class: "swatch", style: `background:${hexColour(r.color) || "#a1a1aa"}` }), r.name);
  }) : el("span", { class: "muted small" }, "No roles yet."));

  const alert = el("select", { class: "select am-select", "aria-label": "Alert channel" },
    el("option", { value: "" }, "Don't post alerts"),
    state.channels.filter((c) => c.kind === TEXT && c.guild_id === ch.guild_id).map((c) => el("option", { value: c.id, selected: c.id === cfg.alert_channel_id }, `#${c.name}`)));
  alert.addEventListener("change", () => { cfg.alert_channel_id = alert.value || null; paint(); });

  const wordSwitch = amSwitch(cfg.word_filter, "Word filter", (v) => { cfg.word_filter = v; paint(); });
  const spamSwitch = amSwitch(cfg.spam_filter, "Spam limits", (v) => { cfg.spam_filter = v; paint(); });
  const status = el("span", { class: "am-status" });

  const save = el("button", { class: "primary", onclick: async () => {
    save.disabled = true;
    const out = await tryApi("PUT", `/api/channels/${ch.id}/automod`, cfg).catch(() => null);
    save.disabled = false;
    if (!out) return;
    Object.assign(cfg, out);
    words.value = cfg.words.join("\n");
    lastSaved = JSON.stringify(cfg);
    toast("Automod saved.");
    paint();
  } }, "Save changes");
  const reset = el("button", { class: "link", onclick: () => drawChannelAutomod(main, ch) }, "Reset");
  const bar = el("div", { class: "ec-savebar" }, el("span", { class: "ec-savebar-text" }, "Careful, you've got unsaved changes!"), el("div", { class: "ec-savebar-btns" }, reset, save));
  let lastSaved = saved;

  function paint() {
    wordBody.classList.toggle("off", !cfg.word_filter);
    spamBody.classList.toggle("off", !cfg.spam_filter);
    const on = [cfg.word_filter && "word filter", cfg.spam_filter && "spam limits"].filter(Boolean);
    status.textContent = on.length ? `On: ${on.join(" and ")}` : "Off in this channel";
    status.classList.toggle("on", !!on.length);
    bar.classList.toggle("show", JSON.stringify(cfg) !== lastSaved);
  }

  main.replaceChildren(el("div", { class: "ec-page am-page" },
    el("div", { class: "am-head" }, el("h2", {}, "Automod"), status),
    el("p", { class: "muted am-intro" }, `Keep #${ch.name} tidy automatically. Blocked messages go in the audit log (just a short snippet).`),
    el("section", { class: "am-card" }, wordSwitch, wordBody),
    el("section", { class: "am-card" }, spamSwitch, spamBody),
    el("section", { class: "am-card" },
      el("h3", { class: "am-title" }, "Who skips automod"),
      amCheck(cfg.exempt_moderators, "People who can manage messages", null, (v) => { cfg.exempt_moderators = v; paint(); }),
      el("div", { class: "field" }, "Roles", roleChips),
      amCheck(cfg.check_bots, "Check bots too", "Bots skip automod unless this is on.", (v) => { cfg.check_bots = v; paint(); })),
    el("section", { class: "am-card" },
      el("h3", { class: "am-title" }, "Alerts"),
      el("label", { class: "field" }, "Post an alert when something's blocked", alert))),
  bar);
  paint();
  return () => JSON.stringify(cfg) !== lastSaved;
}

// ---- Outhouse settings → Automod ----

async function drawAutomodDefaults(page) {
  const guildId = state.guildId;
  const cfg = await api("GET", `/api/guilds/${guildId}/automod`).catch((err) => { toast(err.message); return null; });
  if (!cfg) return;
  const words = el("textarea", { rows: 10, class: "am-words", placeholder: "One per line", "aria-label": "Outhouse-wide blocked words", spellcheck: "false" });
  words.value = cfg.words.join("\n");
  const count = el("span", { class: "ec-count" });
  const paint = () => { const n = parseWordList(words.value).length; count.textContent = `${n} ${n === 1 ? "word" : "words"}`; };
  words.addEventListener("input", paint);
  paint();
  const save = el("button", { class: "primary", onclick: async () => {
    save.disabled = true;
    const out = await tryApi("PUT", `/api/guilds/${guildId}/automod`, { words: parseWordList(words.value) }).catch(() => null);
    save.disabled = false;
    if (!out) return;
    words.value = out.words.join("\n");
    paint();
    toast("Outhouse-wide word list saved.");
  } }, "Save");
  page.append(el("div", { class: "am-page am-defaults" },
    el("h2", {}, "Automod"),
    el("p", { class: "muted" }, "Automod is switched on per channel: open a channel's settings (⚙) → Automod. Channels can use this outhouse-wide list of blocked words as well as their own."),
    el("label", { class: "field" }, el("span", { class: "ec-row" }, "Outhouse-wide blocked words", count), words,
      el("span", { class: "ec-hint" }, "One per line. Not case sensitive; * is a wildcard (dang* catches dangit).")),
    el("div", {}, save)));
}

// ---- Overview: "Hide threads after inactivity" ----

function threadArchiveField(ch) {
  const select = el("select", { class: "select am-select", "aria-label": "Hide threads after inactivity", disabled: true },
    [[60, "1 hour"], [1440, "24 hours"], [4320, "3 days"], [10080, "1 week"]].map(([m, label]) => el("option", { value: String(m) }, label)));
  api("GET", `/api/channels/${ch.id}/thread-defaults`).then((d) => {
    select.value = String(d.auto_archive_duration);
    select.disabled = false;
  }).catch(() => {});
  select.addEventListener("change", async () => {
    select.disabled = true;
    const out = await tryApi("PUT", `/api/channels/${ch.id}/thread-defaults`, { auto_archive_duration: Number(select.value) }).catch(() => null);
    select.disabled = false;
    if (out) {
      if (typeof threadDefaults !== "undefined") threadDefaults.set(ch.id, out.auto_archive_duration);
      toast("New threads will use that.");
    }
  });
  return el("div", { class: "field am-thread-default" },
    el("span", {}, "Hide threads after inactivity"), select,
    el("span", { class: "ec-hint" }, "New threads in this channel tidy themselves away after this long without a reply (people can pick their own when they start one). Saved straight away."));
}
