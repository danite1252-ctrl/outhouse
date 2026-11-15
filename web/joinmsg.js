// Join and leave messages ("Knock knock — @Bob just took the last free stall.")
// and their settings (Outhouse settings → Overview → Welcome).
//
// The server posts them (crates/server/src/api/welcome.rs) with `system` set:
// { kind, template, text, wave }. `text` has {name} where the member goes; the
// message's author is that member. They're drawn as a one-liner with no avatar
// or author header, can't be edited, and only moderators can delete them.

const JM_ICONS = { member_join: "🚽", member_leave: "🚪", bot_join: "🔌" };

// Has the person already waved at this join message (a 👋 reply from them)?
function jmWaved(m) {
  return state.messages.some((r) => r.reply_to?.id === m.id && r.author_id === state.me.id);
}

async function jmWave(m, btn) {
  btn.disabled = true;
  try {
    await api("POST", `/api/channels/${m.channel_id}/messages`, { content: "👋", reply_to_id: m.id });
  } catch (err) {
    toast(err.message);
    btn.disabled = false;
  }
}

function renderSystemMessage(m) {
  const sys = m.system;
  const name = authorName(m);
  const colour = memberColour(m.author_id);
  const [before, after = ""] = sys.text.split("{name}");
  const mention = el("button", {
    type: "button", class: "mention sys-name", style: colour ? `color:${colour}` : "",
    onclick: (e) => openProfile(m.author_id, e.currentTarget),
  }, `@${name}`);
  const wave = sys.wave && m.author_id !== state.me.id && can("send_messages", m.channel_id) && !jmWaved(m)
    ? el("button", { type: "button", class: "sys-wave", onclick: (e) => jmWave(m, e.currentTarget) }, el("span", { "aria-hidden": "true" }, "👋"), ` Wave to ${name}`)
    : null;
  const body = el("div", { class: "msg-body" },
    el("p", { class: "sys-text" }, before, mention, after,
      el("time", { class: "msg-time", datetime: m.created_at, title: new Date(m.created_at).toLocaleString() }, timeLabel(m.created_at))),
    wave,
    renderReactions(m));
  return el("article", {
    class: `msg first sys-msg sys-${sys.kind}${state.flashId === m.id ? " flash" : ""}${m.pinned_at ? " pinned" : ""}`,
    "data-menu": "message",
    "data-id": m.id,
    id: `msg-${m.id}`,
  }, el("div", { class: "sys-icon", "aria-hidden": "true" }, JM_ICONS[sys.kind] || "🚽"), body, messageActions(m));
}

// ---------- Outhouse settings → Overview → Welcome ----------

function jmSwitch(checked, label, onChange) {
  if (typeof amSwitch === "function") return amSwitch(checked, label, onChange);
  const input = el("input", { type: "checkbox", checked });
  input.addEventListener("change", () => onChange(input.checked));
  return el("label", { class: "check-row" }, input, label);
}

function drawWelcomeSettings(gid) {
  const box = el("section", { class: "jm-settings", id: "welcome-settings" }, el("h3", {}, "Welcome"), el("p", { class: "muted" }, "Loading…"));
  const channelName = (id) => state.channels.find((c) => c.id === id)?.name;
  const paint = (s) => {
    const save = async (patch) => {
      const out = await tryApi("PUT", `/api/guilds/${gid}/welcome`, patch).catch(() => null);
      if (out) { toast("Saved."); paint(out); } else paint(s);
    };
    const firstName = s.channel_id == null ? channelName(s.effective_channel_id) : channelName(s.channel_choices[0]);
    const picker = el("select", { class: "select jm-channel", "aria-label": "Post join messages in", disabled: !s.join_messages && !s.leave_messages },
      el("option", { value: "" }, firstName ? `First channel everyone can see (#${firstName})` : "First channel everyone can see"),
      s.channel_choices.map((id) => el("option", { value: id }, `#${channelName(id) || "unknown-channel"}`)));
    const gone = s.channel_id != null && !s.channel_choices.includes(s.channel_id);
    if (gone) picker.prepend(el("option", { value: s.channel_id }, "⚠ A channel that's gone or hidden"));
    picker.value = s.channel_id ?? "";
    picker.addEventListener("change", () => save({ channel_id: picker.value || null }));
    const where = s.effective_channel_id
      ? el("span", { class: "ec-hint" }, `They go in #${channelName(s.effective_channel_id) || "?"}. Nobody gets pinged.`)
      : el("span", { class: "ec-hint jm-warn" }, gone
        ? "That channel was deleted or everyone can't see it any more, so nothing is posted. Pick another."
        : "There's no text channel everyone can see, so nothing is posted.");
    box.replaceChildren(
      el("h3", {}, "Welcome"),
      el("p", { class: "muted" }, "A one-line hello when someone joins, picked from a dozen outhouse-themed ones (“Knock knock — @Bob just took the last free stall.”). Leaving and coming back within 5 minutes doesn't post again. Saved straight away."),
      el("div", { class: "jm-row" }, jmSwitch(s.join_messages, "Join messages", (v) => save({ join_messages: v }))),
      el("div", { class: "jm-row jm-sub" }, jmSwitch(s.wave, "Show a “Wave 👋” button under them", (v) => save({ wave: v }))),
      el("div", { class: "jm-row" }, jmSwitch(s.leave_messages, "Leave messages", (v) => save({ leave_messages: v })),
        el("span", { class: "ec-hint" }, "“@Bob wandered off into the bushes.” Only when people leave by themselves, not kicks or bans.")),
      el("label", { class: "field" }, el("span", {}, "Post in"), picker, where),
    );
  };
  api("GET", `/api/guilds/${gid}/welcome`).then((s) => { if (state.guildId === gid) paint(s); })
    .catch((err) => box.replaceChildren(el("h3", {}, "Welcome"), el("p", { class: "error" }, err.message)));
  return box;
}

if (typeof drawGuildOverview === "function") {
  const jmBaseOverview = drawGuildOverview;
  drawGuildOverview = async function (page) {
    const gid = state.guildId;
    await jmBaseOverview(page);
    if (!page.isConnected || state.guildId !== gid || page.querySelector("#welcome-settings")) return;
    const actions = page.querySelector(".settings-actions");
    const section = drawWelcomeSettings(gid);
    if (actions) actions.after(section); else page.append(section);
  };
}
