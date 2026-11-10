// Outhouse web client: profile cards with a message box, the full profile
// page, and DM message requests. Loaded after app.js, features.js, dms.js,
// friends.js, safety.js and the rest (uses their globals: el, api, state,
// profileCard, openProfile, settingsShell, openDmWith, friendMenuItems, …).
//
// - Left-click someone (member list, names and pictures on messages, voice
//   lists, mentions, friends, DMs, raid sign-ups) for their card, with a
//   "Message @name" box at the bottom. Right-click keeps its menu.
// - "View full profile" opens a page like the settings pages: names they've
//   gone by, outhouses and friends in common, roles, WoW characters and
//   everything you can do. Your own page is where you edit your profile.
// - DMs from people who aren't your friend and share no outhouse with you
//   arrive as message requests (the server holds them; see api::requests).
"use strict";

state.msgRequests = { count: 0, list: null };
state.requestsView = false;

const fullProfileUrl = (id) => `/api/users/${id}/profile/full`;
const shortDate = (iso) => (iso ? new Date(iso).toLocaleDateString([], { day: "numeric", month: "short", year: "numeric" }) : "");

// ======================================================================
// Profile cards: the message box, "View full profile", phones
// ======================================================================

// Sends `content` to `userId` (opening the DM first). Resolves to the DM.
async function quickDm(userId, content) {
  const dm = await api("POST", "/api/dms", { user_ids: [userId] });
  upsertDm(dm);
  await api("POST", `/api/channels/${dm.id}/messages`, { content });
  return dm;
}

function openDmById(id) {
  closePopover();
  document.querySelectorAll("dialog.profile-page[open]").forEach((d) => d.close());
  if (state.home) selectDm(id);
  else openHome(id);
}

// The "Message @name" box at the bottom of someone's card.
function cardMessageBox(p, card) {
  const contact = p.contact || null;
  const shownName = memberById(p.id)?.display_name ?? p.display_name;
  const input = el("input", { type: "text", class: "pc-msg-input", maxlength: 2000, placeholder: `Message @${shownName}`, "aria-label": `Message ${shownName}`, autocomplete: "off" });
  const status = el("div", { class: "pc-msg-status", role: "status" });
  const hint = el("p", { class: "pc-msg-hint" });
  const form = el("form", { class: "pc-msg" }, input);
  const paint = (c) => {
    const blocked = typeof isBlocked === "function" && isBlocked(p.id);
    const reason = blocked ? "You've blocked them. Unblock them to send a message." : c && !c.can_message ? c.reason || "You can't message this person right now." : null;
    input.disabled = !!reason;
    form.classList.toggle("off", !!reason);
    const open = card?.querySelector(".pc-message");
    if (open) { open.disabled = !!reason; open.title = reason || ""; }
    hint.className = `pc-msg-hint${reason ? " cant" : ""}`;
    hint.textContent = reason ? `🔒 ${reason}` : c?.request ? "They'll get this as a message request." : "";
    hint.hidden = !hint.textContent;
  };
  paint(contact);
  if (!contact) api("GET", `/api/users/${p.id}/contact`).then(paint).catch(() => {});
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const content = input.value.trim();
    if (!content || input.disabled) return;
    input.disabled = true;
    status.className = "pc-msg-status";
    status.textContent = "Sending…";
    try {
      const dm = await quickDm(p.id, content);
      input.value = "";
      status.replaceChildren(el("span", { class: "pc-sent" }, "✓ Sent"),
        dm.request ? el("span", { class: "muted" }, " as a message request") : null,
        " · ",
        el("button", { type: "button", class: "link pc-open-dm", onclick: () => openDmById(dm.id) }, "Open DM"));
    } catch (err) {
      status.className = "pc-msg-status err";
      status.textContent = err.message;
    } finally {
      input.disabled = false;
      if (!touchFirst()) input.focus();
    }
  });
  input.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.stopPropagation(); closePopover(); } });
  return el("div", { class: "pc-msg-box" }, form, hint, status);
}

const profBaseProfileCard = profileCard;
profileCard = function (p, opts = {}) {
  const card = profBaseProfileCard(p, opts);
  if (opts.preview) return card;
  const actions = card.querySelector(".pc-actions");
  const body = card.querySelector(".pc-body");
  if (p.level === "suspended") {
    actions?.replaceChildren();
    body?.insertBefore(el("p", { class: "pc-cant pc-suspended" }, "This account is suspended."), actions);
    return card;
  }
  if (p.id === state.me?.id) {
    // Your own card: "Edit profile" opens your profile page (the editor).
    const edit = [...(actions?.querySelectorAll("button") ?? [])].find((b) => b.textContent === "Edit profile");
    if (edit) edit.replaceWith(el("button", { class: "ghost pc-edit", onclick: () => { closePopover(); openFullProfile(p.id, "edit"); } }, "Edit profile"));
    actions?.append(el("button", { class: "ghost pc-full", onclick: () => { closePopover(); openFullProfile(p.id); } }, "View full profile"));
    return card;
  }
  actions?.append(el("button", { class: "ghost pc-full", onclick: () => { closePopover(); openFullProfile(p.id); } }, "View full profile"));
  card.querySelector(".pc-cant")?.remove();
  if (!p.bot) body?.append(cardMessageBox(p, card));
  return card;
};

// Cards come from the full-profile endpoint (what you may see is decided
// there). Phones get a bottom sheet; inside a dialog it stays on top.
const profBaseOpenPopover = openPopover;
openPopover = function (anchor, content, align) {
  const pop = profBaseOpenPopover(anchor, content, align);
  if (content instanceof Element && content.classList.contains("profile-card")) {
    const host = [...document.querySelectorAll("dialog[open]")].pop();
    if (host && !host.contains(pop)) host.append(pop);
    if (isPhoneWidth()) {
      pop.classList.add("pc-sheet");
      pop.prepend(el("div", { class: "pc-grab", "aria-hidden": "true" }));
    }
  }
  return pop;
};

const profBaseOpenProfile = openProfile;
openProfile = async function (userId, anchor) {
  await profBaseOpenProfile(userId, anchor);
  const pop = popover;
  if (!pop?.classList.contains("profile-pop") || pop.classList.contains("pc-sheet")) return;
  const r = pop.getBoundingClientRect();
  if (r.bottom > window.innerHeight - 8) pop.style.top = `${Math.max(8, window.innerHeight - r.height - 8)}px`;
  if (!touchFirst()) pop.querySelector(".pc-msg-input:not(:disabled)")?.focus({ preventScroll: true });
};

// Left-click opens the card in more places (right-click keeps its menu).
document.addEventListener("click", (e) => {
  if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || $("app")?.hidden || openMenus.length) return;
  const t = e.target;
  if (!(t instanceof Element) || t.closest("button.manageable, .su-name.manageable, input, textarea, select, a")) return;
  const mention = t.closest(".mention[data-user]");
  const member = !mention && t.closest("#member-list .member[data-id]");
  const voice = !mention && !member && t.closest("#voice-channels .voice-user[data-id]:not(.hidden-voice)");
  const dmAvatar = !mention && !member && !voice && t.closest("#dm-list .dm-row .dm-avatar");
  const signup = !mention && !member && !voice && !dmAvatar && t.closest(".su-name[data-user]:not(.manageable)");
  if (member && t.closest(".avatar-wrap")) return; // already opens it
  const target = mention || member || voice || dmAvatar || signup;
  if (!target) return;
  // Buttons inside (a stream's LIVE badge, say) do their own thing.
  const inner = t.closest("button, [role=button], .live-badge, .vicon");
  if (inner && inner !== target && target.contains(inner)) return;
  let id = mention?.dataset.user || member?.dataset.id || voice?.dataset.id || signup?.dataset.user;
  if (dmAvatar) {
    const dm = state.dms.find((d) => d.id === dmAvatar.closest(".dm-row")?.dataset.id);
    if (!dm || dm.kind !== DM) return;
    id = dmOthers(dm)[0]?.id;
    e.stopPropagation();
    e.preventDefault();
  }
  if (!id) return;
  openProfile(id, target);
}, true);

// Mentions of people know who they are.
const profBaseMentionPill = mentionPill;
mentionPill = function (kind, id) {
  const pill = profBaseMentionPill(kind, id);
  if (kind === "user") {
    pill.dataset.user = id;
    pill.classList.add("mention-user");
  }
  return pill;
};

// Raid sign-up names open the card (unless you manage the raid: then the
// name is a menu of what to do with the sign-up).
if (typeof signupName === "function") {
  const profBaseSignupName = signupName;
  signupName = function (s, ev) {
    const node = profBaseSignupName(s, ev);
    if (s.user_id && !node.classList.contains("manageable")) {
      node.dataset.user = s.user_id;
      node.classList.add("su-person");
    }
    return node;
  };
}

// ======================================================================
// The full profile page
// ======================================================================

const NAME_KIND = { display_name: "Display name", username: "Username", nickname: "Nickname" };

function profileSection(id, title, ...children) {
  return el("section", { class: "fp-section", id: `fp-${id}`, "data-section": id }, el("h3", {}, title), ...children);
}

function nameHistoryList(names, emptyText = "No earlier names recorded.") {
  if (!names?.length) return el("p", { class: "muted small fp-empty" }, emptyText);
  return el("ol", { class: "fp-names" }, names.map((n) => el("li", { class: "fp-name-row" },
    el("div", { class: "fp-name-main" },
      el("span", { class: "fp-name-old" }, n.old ?? (n.kind === "nickname" ? "(no nickname)" : "")),
      el("span", { class: "fp-arrow", "aria-hidden": "true" }, "→"),
      el("span", { class: "fp-name-new" }, n.new ?? (n.kind === "nickname" ? "(no nickname)" : ""))),
    el("div", { class: "fp-name-meta muted small" },
      NAME_KIND[n.kind] || n.kind,
      n.kind === "nickname" ? ` in ${n.guild_name ?? "a deleted outhouse"}` : "",
      ` · ${shortDate(n.changed_at)}`))));
}

function guildIcon(g, size = 36) {
  const node = el("span", { class: "fp-guild-icon", "aria-hidden": "true" }, initials(g.name));
  node.style.width = node.style.height = `${size}px`;
  node.style.background = colourFor(g.id);
  return node;
}

function roleChip(r) {
  const colour = hexColour(r.color) || "#a1a1aa";
  return el("span", { class: "pc-role" }, el("span", { class: "dot", style: `background:${colour}` }), r.name);
}

// What you can do about someone, as buttons.
function profileActions(p, close) {
  const me = state.me?.id;
  const btn = (label, onClick, cls = "ghost") => el("button", { type: "button", class: `${cls} fp-act`, onclick: onClick }, label);
  const row = el("div", { class: "fp-actions" });
  const notes = el("div", { class: "fp-action-notes" });
  if (p.id === me) return el("div", {}, row, notes);
  const rel = typeof relOf === "function" ? relOf(p.id) : p.relationship;
  const blocked = rel === "blocked" || p.relationship === "blocked";
  const contact = p.contact || {};
  if (!p.bot && p.level !== "suspended") {
    const msg = btn(contact.request ? "Send a message request" : "Message", async () => {
      close();
      await openDmWith(p.id);
    }, "primary");
    msg.disabled = !contact.can_message;
    row.append(msg);
    if (!contact.can_message && contact.reason) notes.append(el("p", { class: "pc-cant" }, "🔒 ", contact.reason));
    else if (contact.request) notes.append(el("p", { class: "muted small" }, "They don't share an outhouse with you, so it'll arrive as a message request."));
  }
  const after = () => setTimeout(() => openFullProfile(p.id), 150);
  if (!p.bot && !blocked && p.level !== "suspended") {
    const label = { none: "Add friend", incoming: "Accept friend request", outgoing: "Cancel friend request", friend: "Remove friend" }[rel] ?? "Add friend";
    const friendBtn = btn(label, async () => {
      if (rel === "outgoing") await removeRelationship(p.id);
      else if (rel === "friend") {
        if (!(await confirmBox(`Remove ${p.display_name} as a friend?`, "You can always send another request later.", "Remove friend"))) return;
        await removeRelationship(p.id, "Removed");
      } else await sendFriendRequest(p.id);
      after();
    });
    if ((rel === "none") && p.friend_refusal) {
      friendBtn.disabled = true;
      notes.append(el("p", { class: "pc-cant" }, "🔒 ", p.friend_refusal));
    }
    row.append(friendBtn);
  }
  row.append(blocked
    ? btn("Unblock", async () => { await unblockUser(p.id, p.display_name); after(); })
    : btn("Block", async () => { await blockUser(p.id, p.display_name); after(); }, "ghost danger-btn"));
  if (typeof openReportDialog === "function") {
    row.append(btn("Report", () => openReportDialog({ kind: "user", id: p.id, title: `Report ${p.display_name}`, userId: p.id, userName: p.display_name }), "ghost danger-btn"));
  }
  row.append(btn("Copy user ID", () => copy(p.id, "ID copied")));
  // Moderators: what they can do in the outhouse you're looking at.
  const m = !state.home && state.guildId ? memberById(p.id) : null;
  if (m) {
    const mod = [];
    if (typeof canRenameMember === "function" && canRenameMember(m)) mod.push(btn("Change nickname", () => openNicknameDialog(m, p.display_name)));
    if (canModerate(m, "timeout_members")) mod.push(btn(isTimedOut(m) ? "Remove timeout" : "Time out", (e) => {
      if (isTimedOut(m)) return timeoutMember(m, 0, "");
      const r = e.currentTarget.getBoundingClientRect();
      openMenu(r.left, r.bottom + 4, TIMEOUTS.map(([label, mins]) => ({ label, onClick: () => timeoutMember(m, mins, label) })));
    }));
    if (canModerate(m, "kick_members")) mod.push(btn("Kick", () => { close(); kickMember(m); }, "ghost danger-btn"));
    if (canModerate(m, "ban_members")) mod.push(btn("Ban", () => { close(); banMember(m); }, "ghost danger-btn"));
    if (mod.length) notes.append(el("div", { class: "fp-mod" }, el("h4", {}, `MODERATE IN ${(currentGuild()?.name ?? "this outhouse").toUpperCase()}`), el("div", { class: "fp-actions" }, mod)));
  }
  if (p.site_owner_view && typeof openSafetyConsole === "function") {
    notes.append(el("button", { type: "button", class: "link fp-safety", onclick: () => { close(); openSafetyConsole("people", null, p.id); } }, "Open in Safety console"));
  }
  return el("div", {}, row, notes);
}

function profileHeader(p) {
  const theme = themeCss(p.profile_theme);
  const banner = el("div", { class: `fp-banner${p.banner_url ? " img" : ""}` });
  banner.style.background = p.level === "suspended" ? "var(--raised)" : profileBanner(p);
  const badges = typeof badgesFor === "function" && memberById(p.id) ? badgesFor(p.id) : [];
  const head = el("div", { class: `fp-head${theme ? " themed" : ""}` },
    banner,
    el("div", { class: "fp-id" },
      el("div", { class: "fp-avatar" }, avatarFor(p.id, p.display_name, 96, p.avatar_url || undefined),
        p.online ? el("span", { class: "status-dot", title: "Online" }) : null),
      el("div", { class: "fp-names-top" },
        el("h2", { class: "fp-display" }, p.display_name, p.bot ? el("span", { class: "bot-tag" }, "BOT") : null),
        el("div", { class: "fp-user muted" }, `@${p.username}`,
          p.level === "full" || p.level === "limited" ? ` · ${p.online ? "Online" : "Offline"}` : ""),
        badges.length ? el("div", { class: "pc-badges" }, badges.map((b) => badgeChip(b))) : null)));
  if (theme) head.style.setProperty("--theme", theme);
  return head;
}

// Your own page's editing tabs reuse the settings pages.
async function drawMyNames(page, p) {
  const name = el("input", { type: "text", maxlength: 32, value: state.me.display_name });
  const save = el("button", { class: "primary", onclick: async () => {
    const user = await tryApi("PATCH", "/api/users/@me", { display_name: name.value }).catch(() => null);
    if (!user) return;
    state.me = user;
    renderMe();
    toast("Name saved.");
    const fresh = await api("GET", fullProfileUrl(state.me.id)).catch(() => null);
    if (fresh) history.replaceChildren(nameHistoryList(fresh.names));
  } }, "Save");
  const history = el("div", {}, nameHistoryList(p.names));
  page.append(
    el("h2", {}, "Names"),
    el("label", { class: "field" }, "Display name", el("div", { class: "inline-row" }, name, save)),
    el("p", { class: "muted small fp-tight" }, "What everyone sees, unless you set a nickname in an outhouse. Your username (@" + state.me.username + ") is used to sign in and can't be changed."),
    el("h3", {}, "Names you've gone by"),
    el("p", { class: "muted small fp-tight" }, "Friends and people in your outhouses can see these (nicknames only from outhouses you share with them). They're kept, so people can tell who's who."),
    history);
  const nicks = el("div", { class: "fp-nicks" });
  page.append(nicks);
  await drawProfiles(nicks);
}

function drawProfileOverview(page, p, close) {
  const full = p.level === "full";
  const self = p.id === state.me?.id;
  const parts = [profileHeader(p)];
  if (p.level === "suspended") {
    parts.push(el("p", { class: "fp-notice" }, "This account is suspended."));
    return page.append(...parts);
  }
  parts.push(profileActions(p, close));
  if (p.level === "minimal") {
    parts.push(el("p", { class: "muted fp-notice" }, p.relationship === "blocked" ? "You've blocked them." : "There's nothing more to show here."));
    return page.append(...parts);
  }
  if (p.about_me) parts.push(profileSection("about", "About me", el("p", { class: "fp-about" }, p.about_me)));
  if (p.created_at || p.outhouses?.some((o) => o.joined_at)) {
    parts.push(profileSection("since", "Member since",
      el("ul", { class: "fp-since" },
        p.created_at ? el("li", {}, el("strong", {}, "Outhouse"), ` · ${shortDate(p.created_at)}`) : null,
        (p.outhouses ?? []).filter((o) => o.joined_at).map((o) => el("li", {}, el("strong", {}, o.name), ` · ${shortDate(o.joined_at)}`)))));
  }
  if (p.level !== "teen") {
    parts.push(profileSection("names", self ? "Names you've gone by" : "Previous names",
      p.names == null
        ? el("p", { class: "muted small fp-empty" }, "Only friends and people in an outhouse with them can see the names they've gone by.")
        : nameHistoryList(p.names)));
  }
  const outhouses = p.outhouses ?? [];
  parts.push(profileSection("outhouses", `${self ? "Your outhouses" : "Outhouses in common"} (${outhouses.length})`,
    outhouses.length
      ? el("div", { class: "fp-guilds" }, outhouses.map((o) => el("button", { type: "button", class: "fp-guild", title: `Go to ${o.name}`, onclick: () => { close(); selectGuild(o.id); } },
        guildIcon(o),
        el("span", { class: "fp-guild-text" },
          el("span", { class: "fp-guild-name" }, o.name, o.owner ? el("span", { class: "crown", title: "Owner" }, " 👑") : null),
          o.nickname ? el("span", { class: "muted small" }, `Nickname: ${o.nickname}`) : null,
          o.roles?.length ? el("span", { class: "pc-roles" }, o.roles.map(roleChip)) : null))))
      : el("p", { class: "muted small fp-empty" }, "None.")));
  if (p.level !== "teen" && !self) {
    const common = p.friends_in_common ?? [];
    parts.push(profileSection("friends", `Friends in common (${common.length})`,
      common.length
        ? el("div", { class: "fp-friends" }, common.map((u) => el("button", { type: "button", class: "fp-friend", title: `${u.display_name} (@${u.username})`, onclick: (e) => openProfile(u.id, e.currentTarget) },
          avatarFor(u.id, u.display_name, 40, u.avatar_url || undefined), el("span", { class: "fp-friend-name" }, u.display_name))))
        : el("p", { class: "muted small fp-empty" }, "None.")));
  }
  const chars = p.characters ?? [];
  if (full && chars.length && typeof classChip === "function") {
    parts.push(profileSection("wow", "WoW characters",
      el("div", { class: "fp-chars" }, chars.map((c) => {
        const colour = classColour(c.class) ?? "var(--text)";
        const href = charHref(c);
        const card = el("div", { class: "fp-char", style: `--cc:${colour}` },
          charAvatar(c, 44),
          el("div", { class: "fp-char-text" },
            el("div", { class: "fp-char-name" }, c.is_main ? el("span", { class: "char-star", title: "Main" }, "★ ") : null,
              href ? el("a", { href, target: "_blank", rel: "noopener noreferrer" }, charName(c)) : charName(c)),
            el("div", { class: "muted small" }, [`Level ${c.level}`, [c.race, c.class].filter(Boolean).join(" "), c.item_level ? `ilvl ${c.item_level}` : null].filter(Boolean).join(" · ")),
            c.guild_name ? el("div", { class: "pc-armory-guild" }, `<${c.guild_name}>`) : null,
            linkRow(c)));
        return card;
      }))));
  }
  if (p.level === "teen") parts.push(el("p", { class: "muted small fp-notice" }, "Some of this profile is only shown to their friends."));
  page.append(...parts);
}

// Opens someone's full profile (yours: `tab` "edit" or "names" to edit).
async function openFullProfile(userId, tab = "profile") {
  closePopover();
  closeMenus();
  let p;
  try {
    p = await api("GET", fullProfileUrl(userId));
  } catch (err) {
    return toast(err.message);
  }
  document.querySelectorAll("dialog.profile-page").forEach((d) => d.close());
  const { dialog, nav, main } = settingsShell("Profile");
  dialog.classList.add("profile-page");
  dialog.dataset.user = userId;
  const close = () => dialog.close();
  const self = p.id === state.me?.id;
  const sections = self
    ? [["profile", "My profile"], ["edit", "Picture & look"], ["names", "Names"]]
    : [["profile", "Overview"], ...(p.level === "full" || p.level === "limited" ? [["names", "Previous names"], ["outhouses", "Outhouses"], ["friends", "Friends in common"]] : [])]
      .concat(p.characters?.length && p.level === "full" ? [["wow", "WoW characters"]] : []);
  const draw = async (active) => {
    nav.replaceChildren(
      el("button", { class: "fp-back", onclick: close }, "← Back"),
      el("h3", {}, (self ? "YOUR PROFILE" : p.display_name.toUpperCase())),
      ...sections.map(([id, label]) => el("button", { class: active === id ? "active" : "", "data-section": id, onclick: () => {
        if (self) return draw(id);
        nav.querySelectorAll("button.active").forEach((b) => b.classList.remove("active"));
        nav.querySelector(`button[data-section="${id}"]`)?.classList.add("active");
        if (id === "profile") main.scrollTo({ top: 0, behavior: "smooth" });
        else main.querySelector(`#fp-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
      } }, label)));
    const page = settingsPage(main);
    page.classList.add("fp-page");
    if (self && active === "edit") return drawProfileLook(page);
    if (self && active === "names") return drawMyNames(page, p);
    drawProfileOverview(page, p, close);
    if (self) {
      page.append(el("div", { class: "fp-self-edit" },
        el("button", { class: "primary", onclick: () => draw("edit") }, "Edit picture & look"),
        el("button", { class: "ghost", onclick: () => draw("names") }, "Change your name")));
    }
  };
  draw(self && sections.some(([id]) => id === tab) ? tab : "profile");
  return dialog;
}

// User settings → "Profile & picture" points at your profile page.
const profBaseDrawProfileLook = drawProfileLook;
drawProfileLook = async function (main) {
  await profBaseDrawProfileLook(main);
  if (main.closest("dialog.profile-page")) return;
  main.querySelector("h2")?.after(el("p", { class: "muted small fp-tight" }, "This is also on ",
    el("button", { type: "button", class: "link", onclick: () => openFullProfile(state.me.id, "edit") }, "your profile page"),
    ", along with the names you've gone by."));
};

// ======================================================================
// Message requests
// ======================================================================

async function loadMessageRequests() {
  const r = await api("GET", "/api/dms/requests").catch(() => null);
  if (!r) return;
  state.msgRequests = { count: r.count, list: r.requests };
  if (state.home) renderDmList();
  if (state.requestsView) drawRequestsPage();
}

function attachmentLabel(m) {
  const bits = [];
  const add = (n, one, many) => { if (n) bits.push(`${n} ${n === 1 ? one : many}`); };
  add(m.pictures, "picture", "pictures");
  add(m.videos, "video", "videos");
  add(m.voice, "voice message", "voice messages");
  add(m.files, "file", "files");
  return bits.length ? `📎 ${bits.join(", ")}` : null;
}

function requestRow(r) {
  const u = r.user;
  const busy = (fn) => async (e) => {
    const b = e.currentTarget;
    b.disabled = true;
    try { await fn(); } finally { b.disabled = false; }
  };
  const accept = busy(async () => {
    const dm = await tryApi("POST", `/api/dms/requests/${r.channel_id}/accept`).catch(() => null);
    if (!dm) return;
    state.msgRequests.list = (state.msgRequests.list ?? []).filter((x) => x.channel_id !== r.channel_id);
    state.msgRequests.count = state.msgRequests.list.length;
    upsertDm(dm);
    leaveRequestsView();
    selectDm(dm.id);
    toast(`You can now chat with ${u.display_name}.`);
  });
  const ignore = busy(async () => {
    const ok = await tryApi("POST", `/api/dms/requests/${r.channel_id}/ignore`).then(() => true, () => false);
    if (ok) { toast(`Ignored. ${u.display_name} isn't told.`); loadMessageRequests(); }
  });
  const block = busy(async () => { await blockUser(u.id, u.display_name); loadMessageRequests(); });
  const report = () => openReportDialog({ kind: "user", id: u.id, title: `Report ${u.display_name}`, userId: u.id, userName: u.display_name });
  return el("article", { class: "mr-row", "data-channel": r.channel_id, "data-user": u.id },
    el("div", { class: "mr-who" },
      el("button", { type: "button", class: "mr-avatar", title: "View profile", onclick: (e) => openProfile(u.id, e.currentTarget) }, avatarFor(u.id, u.display_name, 44, u.avatar_url || undefined)),
      el("div", { class: "mr-names" },
        el("button", { type: "button", class: "mr-name link", onclick: (e) => openProfile(u.id, e.currentTarget) }, u.display_name),
        el("span", { class: "muted small" }, `@${u.username} · ${relTime(r.sent_at || r.created_at)}`))),
    el("div", { class: "mr-preview" }, r.messages.map((m) => el("div", { class: "mr-msg" },
      m.content ? el("p", {}, m.content) : null,
      m.sticker ? el("p", { class: "muted small" }, "🏷️ A sticker") : null,
      attachmentLabel(m) ? el("p", { class: "mr-files muted small" }, attachmentLabel(m)) : null))),
    el("p", { class: "muted small mr-note" }, "Pictures and files show once you accept."),
    el("div", { class: "mr-actions" },
      el("button", { type: "button", class: "primary mr-accept", onclick: accept }, "Accept"),
      el("button", { type: "button", class: "ghost mr-ignore", onclick: ignore }, "Ignore"),
      el("button", { type: "button", class: "ghost danger-btn mr-block", onclick: block }, "Block"),
      typeof openReportDialog === "function" ? el("button", { type: "button", class: "ghost danger-btn mr-report", onclick: report }, "Report") : null));
}

function drawRequestsPage() {
  const box = $("messages");
  if (!state.requestsView || !box) return;
  const list = state.msgRequests.list;
  box.replaceChildren(el("div", { class: "friends-page requests-page" },
    el("div", { class: "mr-intro" },
      el("p", {}, "Messages from people who aren't your friends and don't share an outhouse with you wait here. They can't see whether you've read them, and they aren't told if you ignore them."),
      el("p", { class: "muted small" }, "Accept to chat as normal. Ignore hides it quietly. You can choose to refuse these entirely in User settings → Privacy & friends.")),
    list == null
      ? el("p", { class: "muted" }, "Loading…")
      : list.length
        ? el("div", { class: "mr-list" }, list.map(requestRow))
        : el("p", { class: "fr-empty muted" }, "No message requests. All quiet on the porch.")));
}

function showRequests() {
  if (typeof showFriends !== "function") return;
  showFriends();
  state.requestsView = true;
  $("app").classList.add("requests-mode");
  setChatIcon("✉️");
  $("channel-name").textContent = "Message requests";
  renderDmList();
  drawRequestsPage();
  loadMessageRequests();
}

function leaveRequestsView() {
  if (!state.requestsView) return;
  state.requestsView = false;
  $("app").classList.remove("requests-mode");
}

async function openRequests() {
  if (state.home) return showRequests();
  if (typeof openFriends === "function") await openFriends();
  if (state.home) showRequests();
}

const profBaseDrawFriendsPage = drawFriendsPage;
drawFriendsPage = function (...args) {
  if (state.requestsView) return drawRequestsPage();
  return profBaseDrawFriendsPage(...args);
};

const profBaseLeaveFriendsView = leaveFriendsView;
leaveFriendsView = function (...args) {
  leaveRequestsView();
  return profBaseLeaveFriendsView(...args);
};

const profBaseShowFriends = showFriends;
showFriends = function (...args) {
  leaveRequestsView();
  return profBaseShowFriends(...args);
};

// "Message requests (N)" at the top of the DM list, under Friends. A quiet
// count (not the red unread badge), and only when there are some.
const profBaseRenderDmList = renderDmList;
renderDmList = function () {
  profBaseRenderDmList();
  const box = $("dm-list");
  if (!box || box.hidden) return;
  const n = state.msgRequests.count;
  if (state.requestsView) $("friends-row")?.classList.remove("active");
  if (!n && !state.requestsView) return;
  const row = el("button", {
    class: `channel dm-row requests-row${state.requestsView ? " active" : ""}`,
    id: "requests-row",
    title: n ? `Message requests (${n})` : "Message requests",
    "aria-label": n ? `Message requests (${n})` : "Message requests",
    onclick: () => openRequests(),
  }, el("span", { class: "group-icon requests-icon", "aria-hidden": "true" }, "✉️"),
  el("span", { class: "dm-text" }, el("span", { class: "cname" }, "Message requests")),
  n ? el("span", { class: "quiet-badge", title: `${n} message request${n === 1 ? "" : "s"}` }, n) : null);
  const friends = $("friends-row");
  if (friends) friends.after(row);
  else box.prepend(row);
};

// The sender's side: a note while their messages wait as a request.
function syncRequestNote() {
  const wrap = $("composer-wrap");
  if (!wrap) return;
  let note = $("dm-request-note");
  const dm = typeof currentDm === "function" ? currentDm() : null;
  const show = !!(dm && dm.request === "outgoing" && !state.friendsView);
  if (!show) { if (note) note.hidden = true; return; }
  if (!note) {
    note = el("div", { id: "dm-request-note", class: "dm-request-note", role: "note" });
    wrap.before(note);
  }
  note.replaceChildren("✉️ ", el("span", {}, "Sent as a message request — they'll see it when they accept."));
  note.hidden = false;
}

const profBaseUpsertDm = upsertDm;
upsertDm = function (dm) {
  profBaseUpsertDm(dm);
  syncRequestNote();
};

const profBaseSelectDm = selectDm;
selectDm = async function (id) {
  const out = await profBaseSelectDm(id);
  syncRequestNote();
  return out;
};

const profBaseSelectChannel = selectChannel;
selectChannel = async function (...args) {
  const out = await profBaseSelectChannel(...args);
  syncRequestNote();
  return out;
};

const profBaseHandle = handleEvent;
handleEvent = function (ev) {
  const { t, d } = ev;
  if (t === "MESSAGE_REQUESTS") {
    const before = state.msgRequests.count;
    state.msgRequests.count = d.count;
    if (state.requestsView || d.count !== (state.msgRequests.list?.length ?? before)) loadMessageRequests();
    else if (state.home) renderDmList();
    return;
  }
  profBaseHandle(ev);
  if (t === "READY") loadMessageRequests();
  if (t === "DM_DELETE" || t === "DM_UPDATE") syncRequestNote();
};

// ======================================================================
// Settings → Privacy: message requests (adults; teens have stricter rules)
// ======================================================================

if (typeof drawPrivacyPage === "function") {
  const profBaseDrawPrivacyPage = drawPrivacyPage;
  drawPrivacyPage = async function (main) {
    await profBaseDrawPrivacyPage(main);
    const p = await api("GET", "/api/users/@me/privacy").catch(() => null);
    if (!p || !main.isConnected) return;
    // Teens' DMs already only come from friends (and teens they know).
    if (p.teen) return;
    const pick = (v) => tryApi("PUT", "/api/users/@me/privacy", { message_requests: v }).then(() => toast("Saved."), () => {});
    const options = [
      ["allow", "Allow as message requests", "They wait under Message requests until you accept. You aren't notified."],
      ["block", "Block entirely", "Only friends and people in your outhouses can message you."],
    ];
    const box = el("section", { class: "notify-box", id: "privacy-requests" },
      el("h3", {}, "Message requests from people you don't share an outhouse with"),
      el("div", { class: "radio-list", role: "radiogroup" }, options.map(([v, label, hint]) => {
        const input = el("input", { type: "radio", name: "message_requests", value: v, checked: v === (p.message_requests || "allow") });
        input.addEventListener("change", () => input.checked && pick(v));
        return el("label", { class: "radio-row" }, input, el("span", {}, el("strong", {}, label), el("span", { class: "muted small" }, hint)));
      })));
    const sections = main.querySelectorAll("section.notify-box");
    (sections[1] ?? sections[0])?.after(box);
  };
}
