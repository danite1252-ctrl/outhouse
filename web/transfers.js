// Outhouse ownership transfers (api/transfers.rs): the owner's "Transfer
// ownership…" (member menu and the profile ⋯), the banners for offers,
// waiting transfers and "I'm still here", and staff-assisted transfers from
// the Outhouses directory. Loaded after staff.js; uses acctDialog
// (account-security.js), sCan/sOwner (safety.js) and app.js globals.

const xfer = { incoming: [], outgoing: [], objections: [], inactiveDays: 90 };
const xferDay = (iso) => (iso ? new Date(iso).toLocaleDateString([], { day: "numeric", month: "long", year: "numeric" }) : "");

// ---------- the owner hands it over ----------

async function openTransferDialog(m) {
  const g = currentGuild();
  if (!g) return;
  const me = await api("GET", "/api/users/@me").catch(() => null);
  const name = m.display_name || m.nickname || m.username;
  const confirm = el("input", { type: "text", class: "xfer-confirm", autocomplete: "off", spellcheck: "false", placeholder: m.username });
  const password = el("input", { type: "password", class: "xfer-password", autocomplete: "current-password" });
  const code = me?.two_factor ? el("input", { type: "text", class: "xfer-code", inputmode: "numeric", autocomplete: "one-time-code", maxlength: 14 }) : null;
  const keep = el("input", { type: "checkbox", class: "xfer-keep" });
  acctDialog(`Transfer ownership of ${g.name}?`, [
    el("div", { class: "xfer-who" }, avatarFor(m.id, name, 40), el("div", {}, el("strong", {}, name), el("div", { class: "muted small" }, `@${m.username}`))),
    el("ul", { class: "xfer-changes" },
      el("li", {}, `${name} becomes the owner: full control of its settings, roles, channels and members, and they can delete it.`),
      el("li", {}, "You lose your owner powers. You stay a member."),
      el("li", {}, "They have 7 days to accept. You can cancel until then, and you'll hear if they decline.")),
    el("label", { class: "rp-check xfer-keep-row" }, keep, el("span", {}, el("strong", {}, "Keep me as an admin"), el("span", { class: "muted small" }, " (gives you an Administrator role, made if there isn't one)"))),
    el("label", { class: "field" }, `Type their username (${m.username}) to confirm`, confirm),
    el("label", { class: "field" }, "Your password", password),
    code ? el("label", { class: "field" }, "Code from your authenticator app", code) : null,
  ], "Send the offer", async () => {
    await api("POST", `/api/guilds/${g.id}/transfer`, {
      user_id: m.id, confirm_username: confirm.value.trim(), password: password.value,
      code: code ? code.value.trim() : "", keep_admin: keep.checked,
    });
    toast(`Offer sent. ${name} has 7 days to accept.`);
    loadTransfers();
    return true;
  }, { danger: true, cls: "xfer-dialog" });
}

// The member menu and the ⋯ on profile cards (only the owner sees it).
if (typeof friendMenuItems === "function") {
  const xferBaseFriendItems = friendMenuItems;
  friendMenuItems = function (userId, name, bot = false) {
    const items = xferBaseFriendItems(userId, name, bot);
    const g = !state.home ? currentGuild() : null;
    const m = g ? memberById(userId) : null;
    if (!g || !m || bot || m.bot || userId === state.me?.id || g.owner_id !== state.me?.id || g.closed_until) return items;
    return [...items, "sep", { label: "Transfer ownership…", danger: true, onClick: () => openTransferDialog(m) }];
  };
}

// ---------- banners ----------

function xferBanner(kind, icon, text, actions) {
  return el("div", { class: `xfer-banner xfer-${kind}`, role: "status", "data-kind": kind },
    el("span", { class: "xfer-banner-icon", "aria-hidden": "true" }, icon),
    el("span", { class: "xfer-banner-text" }, text),
    el("span", { class: "xfer-banner-actions" }, actions));
}

function renderTransferBanners() {
  document.querySelectorAll(".xfer-banner").forEach((n) => n.remove());
  const head = document.querySelector("main.chat .chat-head");
  if (!head) return;
  const nodes = [];
  for (const t of xfer.objections) {
    nodes.push(xferBanner("objection", "⏳", `Outhouse staff are handing ${t.guild_name} to @${t.to.username} because you've been away. It moves on ${xferDay(t.objection_until)} unless you say you're still here.`, [
      el("button", { type: "button", class: "xfer-btn primary-ish xfer-still-here", onclick: () => stillHere(t) }, "I'm still here"),
      el("button", { type: "button", class: "xfer-btn", onclick: () => openObjectionDialog(t) }, "Details"),
    ]));
  }
  for (const t of xfer.incoming) {
    nodes.push(xferBanner("incoming", "🔑", t.route === "owner"
      ? `${t.from.display_name} wants to hand you ownership of ${t.guild_name}.`
      : `Outhouse staff are handing you ownership of ${t.guild_name}.`, [
      el("button", { type: "button", class: "xfer-btn primary-ish xfer-review", onclick: () => openAcceptDialog(t) }, "Review"),
    ]));
  }
  for (const t of xfer.outgoing) {
    nodes.push(xferBanner("outgoing", "🔑", `Waiting for @${t.to.username} to accept ownership of ${t.guild_name} (until ${xferDay(t.deadline)}).`, [
      el("button", { type: "button", class: "xfer-btn", onclick: async () => {
        if (!(await confirmBox(`Cancel the transfer of ${t.guild_name}?`, `@${t.to.username} is told it was cancelled. Nothing changes.`, "Cancel transfer"))) return;
        if (await tryApi("POST", `/api/transfers/${t.id}/cancel`).catch(() => null)) { toast("Transfer cancelled."); loadTransfers(); }
      } }, "Cancel"),
    ]));
  }
  head.after(...nodes);
}

async function stillHere(t) {
  if (!(await confirmBox(`Keep ${t.guild_name}?`, "The transfer stops and nothing changes. Outhouse staff and the admin who asked are told you're still around.", "I'm still here"))) return;
  if (await tryApi("POST", `/api/transfers/${t.id}/still-here`).catch(() => null)) { toast(`${t.guild_name} stays yours.`); loadTransfers(); }
}

function openObjectionDialog(t) {
  acctDialog(`Is ${t.guild_name} still yours?`, [
    el("p", { class: "acct-tight" }, `You haven't used Outhouse for a while, and an admin of ${t.guild_name} asked for ownership so it can keep running. Outhouse staff started handing it to ${t.to.display_name} (@${t.to.username}).`),
    t.reason ? el("p", { class: "muted small acct-tight" }, `What they told staff: “${t.reason}”`) : null,
    el("p", { class: "acct-tight" }, `It moves to them on ${xferDay(t.objection_until)} unless you press “I'm still here”.`),
  ], "I'm still here", async () => {
    await api("POST", `/api/transfers/${t.id}/still-here`);
    toast(`${t.guild_name} stays yours.`);
    loadTransfers();
    return true;
  }, { cancel: "Close", cls: "xfer-dialog" });
}

function openAcceptDialog(t) {
  const waitingOnOwner = t.status === "objection";
  const d = acctDialog(`Take over ${t.guild_name}?`, [
    el("p", { class: "acct-tight" }, t.route === "owner"
      ? `${t.from.display_name} (@${t.from.username}) wants you to be the owner of ${t.guild_name}.`
      : `Outhouse staff are handing you ${t.guild_name}${t.route === "inactive" ? " because its owner has been away and you asked for it" : " at its owner's request (they confirmed by email)"}.`),
    el("ul", { class: "xfer-changes" },
      el("li", {}, "You get full control: its settings, roles, channels and members."),
      el("li", {}, t.keep_admin ? `${t.from.display_name} keeps an Administrator role.` : `${t.from.display_name} stays a member without owner powers.`),
      waitingOnOwner ? el("li", {}, `Accepting confirms you asked for it. It becomes yours on ${xferDay(t.objection_until)}, unless the owner says they're still here.`) : el("li", {}, `This offer ends on ${xferDay(t.deadline)}.`)),
  ], waitingOnOwner ? "Accept (confirms my request)" : "Accept ownership", async () => {
    const r = await api("POST", `/api/transfers/${t.id}/accept`);
    toast(r.status === "completed" ? `You now own ${t.guild_name}.` : `Accepted. It becomes yours on ${xferDay(t.objection_until)} unless the owner objects.`);
    loadTransfers();
    return true;
  }, { cancel: "Not now", cls: "xfer-dialog" });
  d.querySelector(".row")?.prepend(el("button", { type: "button", class: "ghost danger-btn xfer-decline", onclick: async () => {
    if (await tryApi("POST", `/api/transfers/${t.id}/decline`).catch(() => null)) {
      toast("Declined. They've been told.");
      d.close();
      loadTransfers();
    }
  } }, "Decline"));
}

async function loadTransfers() {
  const d = await api("GET", "/api/users/@me/transfers").catch(() => null);
  if (!d) return;
  xfer.incoming = d.incoming;
  xfer.outgoing = d.outgoing;
  xfer.objections = d.objections;
  xfer.inactiveDays = d.inactive_days;
  renderTransferBanners();
  if (state.openTransferOnReady) {
    const id = state.openTransferOnReady;
    state.openTransferOnReady = null;
    const t = xfer.incoming.find((x) => x.id === id);
    const o = xfer.objections.find((x) => x.id === id);
    if (t) openAcceptDialog(t);
    else if (o) openObjectionDialog(o);
  }
}

// The email's "Open Outhouse" link: /app?transfer=<id>.
(() => {
  const q = new URLSearchParams(location.search);
  if (!/^\d+$/.test(q.get("transfer") || "")) return;
  state.openTransferOnReady = q.get("transfer");
  q.delete("transfer");
  const rest = q.toString();
  history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : ""));
})();

const xferBaseHandle = handleEvent;
handleEvent = function (ev) {
  xferBaseHandle(ev);
  if (ev.t === "READY" || ev.t === "TRANSFERS_CHANGED") loadTransfers();
};
// Switching channels redraws the header: keep the banners under it.
if (typeof selectChannel === "function") {
  const xferBaseSelectChannel = selectChannel;
  selectChannel = function (...args) {
    const out = xferBaseSelectChannel.apply(this, args);
    Promise.resolve(out).then(() => { if (!document.querySelector(".xfer-banner")) renderTransferBanners(); });
    return out;
  };
}

// ---------- staff-assisted (Outhouses directory) ----------

const XFER_STATUS = {
  awaiting_owner: (t) => `Waiting for the owner to confirm by email (the link works until ${scWhen(t.deadline)}).`,
  objection: (t) => `The owner can say they're still here until ${scWhen(t.objection_until)}. @${t.to.username} ${t.recipient_accepted ? "has accepted" : "hasn't accepted yet"}.`,
  awaiting_recipient: (t) => `Waiting for @${t.to.username} to accept (until ${scWhen(t.deadline)}).`,
};

// The transfer under way, in the directory.
function transferStatusPanel(t, redraw) {
  return el("section", { class: "sc-section xfer-panel" },
    el("h3", {}, "Ownership transfer under way"),
    el("p", { class: "sc-tight" }, `${t.route_label}: @${t.from.username} → @${t.to.username}.`),
    el("p", { class: "muted small sc-tight" }, (XFER_STATUS[t.status] || (() => t.status))(t)),
    t.reason ? el("p", { class: "muted small sc-tight" }, `Reason: ${t.reason}`) : null,
    sCan("transfers") ? el("div", { class: "sc-actions" }, el("button", { type: "button", class: "ghost", onclick: async () => {
      if (!(await confirmBox("Cancel this transfer?", "Nothing changes, and the people involved are told.", "Cancel transfer"))) return;
      if (await tryApi("POST", `/api/transfers/${t.id}/cancel`).catch(() => null)) { toast("Transfer cancelled."); redraw(); }
    } }, "Cancel transfer")) : null);
}

function openStaffTransferDialog(d, redraw) {
  const days = d.transfer_rules?.inactive_days ?? 90;
  const window = d.transfer_rules?.objection_days ?? 14;
  const away = d.owner.inactive_days ?? 0;
  const inactiveOk = !d.owner.online && away >= days;
  const emailOk = !!d.owner.email_confirmed;
  let route = inactiveOk ? "inactive" : emailOk ? "verified" : "inactive";
  const admins = d.members_list.filter((m) => !m.bot && !m.owner);
  const username = el("input", { type: "text", class: "xfer-new-owner", list: "xfer-members", autocomplete: "off", placeholder: "Their username" });
  const reason = el("textarea", { rows: 3, maxlength: 500, class: "xfer-reason", placeholder: "Who asked, and what they said (e.g. Ada asked in #help; the owner left the game last year)" });
  const keep = el("input", { type: "checkbox" });
  const teen = sOwner() ? el("input", { type: "checkbox" }) : null;
  const opt = (key, title, body, ok, why) => el("button", { type: "button", class: "xfer-route", "data-route": key, disabled: !ok, onclick: () => { route = key; sync(); } },
    el("div", { class: "xfer-route-head" }, el("span", { class: "xfer-radio", "aria-hidden": "true" }), el("strong", {}, title)),
    el("p", { class: "muted small" }, body),
    ok ? null : el("p", { class: "xfer-route-why small" }, why));
  const routes = el("div", { class: "xfer-routes", role: "radiogroup" },
    opt("inactive", "The owner has been inactive",
      `For an owner who's been away ${days}+ days, when one of the outhouse's admins (Administrator or Manage outhouse) asks for it. The owner gets an email and a notice with “I'm still here”, and has ${window} days to stop it. The new owner confirms by accepting; it completes when the window ends.`,
      inactiveOk, d.owner.online ? "The owner is online right now." : `The owner was last active ${scPlural(away, "day")} ago (needs ${days}+).`),
    opt("verified", "The owner asked (check by email)",
      "For an owner who lost access to their account. We send a single-use link (24 hours) to the owner's confirmed email: “Confirm you want to transfer it”. Once they confirm, the new owner accepts.",
      emailOk, "The owner has no confirmed email: use the inactive route."));
  const submit = () => dialog.querySelector("button[type=submit]");
  const sync = () => {
    routes.querySelectorAll(".xfer-route").forEach((b) => {
      b.classList.toggle("active", b.dataset.route === route);
      b.setAttribute("aria-checked", String(b.dataset.route === route));
    });
    if (submit()) {
      submit().textContent = route === "verified" ? "Send verification to the owner's email" : "Start the transfer";
      submit().disabled = !(route === "inactive" ? inactiveOk : emailOk);
    }
  };
  const dialog = acctDialog(`Transfer ownership of ${d.name}`, [
    el("p", { class: "muted small acct-tight" }, `Owner: ${d.owner.display_name} (@${d.owner.username}) · last active ${d.owner.online ? "now" : d.owner.last_active_at ? scWhen(d.owner.last_active_at) : "unknown"}. Everything here is written to the safety log.`),
    routes,
    el("label", { class: "field" }, route === "inactive" ? "New owner (must be an admin of the outhouse who asked)" : "New owner", username,
      el("datalist", { id: "xfer-members" }, admins.map((m) => el("option", { value: m.username }, `${m.display_name}${m.roles.length ? ` · ${m.roles.join(", ")}` : ""}`)))),
    el("label", { class: "field" }, "Reason (required)", reason),
    el("label", { class: "rp-check" }, keep, "Keep the old owner as an admin (an Administrator role)"),
    teen ? el("label", { class: "rp-check" }, teen, "Allow a teen to own it (site owner only; family outhouses always can)") : null,
  ], "Start the transfer", async () => {
    if (!username.value.trim()) throw new Error("Type the new owner's username.");
    if (!reason.value.trim()) throw new Error("Write why: it goes in the safety log.");
    await api("POST", `/api/safety/outhouses/${d.id}/transfer`, {
      route, username: username.value.trim().replace(/^@/, ""), reason: reason.value.trim(),
      keep_admin: keep.checked, allow_teen: !!teen?.checked,
    });
    toast(route === "verified" ? "Sent. The owner has 24 hours to confirm by email." : `Started. The owner has ${window} days to object; the new owner has to accept.`);
    redraw();
    return true;
  }, { cls: "xfer-dialog xfer-staff-dialog" });
  sync();
  return dialog;
}
