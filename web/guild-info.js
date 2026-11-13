// Outhouse: an outhouse's "more info". The Outhouse info panel any member
// can open (right-click an outhouse, or the menu on its name), and Outhouse
// settings → Overview for people with Manage Outhouse: name, picture,
// banner, tagline, About, rules and links. Loaded after app.js and
// folders.js; uses their globals.

const GI_LIMITS = { name: 100, tagline: 80, description: 1000, rules: 2000, links: 5, label: 40 };

const giDate = (iso) => (iso ? new Date(iso).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }) : "");

function giIcon(info, size = 96) {
  const box = el("div", { class: "gi-icon", style: `width:${size}px;height:${size}px` });
  if (info.icon_url) box.append(el("img", { src: info.icon_url, alt: "" }));
  else box.append(el("span", {}, initials(info.name)));
  return box;
}

// ---------- the info panel ----------

async function openGuildInfo(guildId = state.guildId) {
  closePopover();
  closeMenus();
  let info;
  let roles = [];
  try {
    [info, roles] = await Promise.all([
      api("GET", `/api/guilds/${guildId}/info`),
      guildId === state.guildId ? Promise.resolve(state.roles) : api("GET", `/api/guilds/${guildId}/roles`).catch(() => []),
    ]);
  } catch (err) {
    return toast(err.message);
  }
  document.querySelectorAll("dialog.guild-info").forEach((d) => d.close());
  const { dialog, nav, main } = settingsShell("Outhouse info");
  dialog.classList.add("profile-page", "guild-info");
  dialog.dataset.guild = guildId;
  const close = () => dialog.close();
  const here = guildId === state.guildId;
  const sections = [["overview", "Overview"],
    info.rules ? ["rules", "Rules"] : null,
    info.links.length ? ["links", "Links"] : null,
    ["you", "You here"],
    info.name_history.length ? ["names", "Previous names"] : null].filter(Boolean);
  nav.replaceChildren(
    el("button", { class: "fp-back", onclick: close }, "← Back"),
    el("h3", {}, info.name.toUpperCase()),
    ...sections.map(([id, label], i) => el("button", { class: i === 0 ? "active" : "", "data-section": id, onclick: () => {
      nav.querySelectorAll("button.active").forEach((b) => b.classList.remove("active"));
      nav.querySelector(`button[data-section="${id}"]`)?.classList.add("active");
      if (id === "overview") main.scrollTo({ top: 0, behavior: "smooth" });
      else main.querySelector(`#gi-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
    } }, label)));

  const page = settingsPage(main);
  page.classList.add("fp-page", "gi-page");
  const banner = el("div", { class: `fp-banner gi-banner${info.banner_url ? " img" : ""}` });
  if (info.banner_url) banner.style.backgroundImage = `url("${info.banner_url}")`;
  const owner = info.owner;
  const ownerBtn = owner ? el("button", { type: "button", class: "gi-owner", onclick: (e) => openProfile(owner.id, e.currentTarget) },
    avatarFor(owner.id, owner.display_name, 22, owner.avatar_url || undefined), owner.display_name) : el("span", {}, "—");
  const section = (id, title, ...children) => el("section", { class: "fp-section", id: `gi-${id}`, "data-section": id }, el("h3", {}, title), ...children);
  const myRoles = info.my_role_ids.map((id) => roles.find((r) => r.id === id)).filter(Boolean).sort((a, b) => b.position - a.position);
  const canInvite = here && can("create_invite");
  page.append(
    el("div", { class: "fp-head gi-head" },
      banner,
      el("div", { class: "fp-id" },
        el("div", { class: "fp-avatar gi-avatar" }, giIcon(info)),
        el("div", { class: "fp-names-top" },
          el("h2", { class: "fp-display gi-name" }, info.name),
          info.tagline ? el("div", { class: "gi-tagline" }, info.tagline) : null))),
    el("div", { class: "fp-actions" },
      canInvite ? el("button", { class: "primary", onclick: () => { close(); invitePeople(); } }, "Invite people") : null,
      here && info.can_manage ? el("button", { class: "ghost", onclick: () => { close(); openSettings("overview"); } }, "Edit outhouse") : null,
      !here ? el("button", { class: "ghost", onclick: () => { close(); selectGuild(guildId); } }, "Open outhouse") : null),
    el("div", { class: "gi-stats" },
      el("div", { class: "gi-stat" }, el("strong", {}, String(info.member_count)), el("span", { class: "muted" }, info.member_count === 1 ? "member" : "members")),
      el("div", { class: "gi-stat" }, el("strong", {}, el("span", { class: "gi-online-dot" }), String(info.online_count)), el("span", { class: "muted" }, "online")),
      el("div", { class: "gi-stat" }, el("strong", {}, giDate(info.created_at)), el("span", { class: "muted" }, "built")),
      el("div", { class: "gi-stat gi-stat-owner" }, ownerBtn, el("span", { class: "muted" }, "owner"))),
    section("about", "About", info.description
      ? el("div", { class: "gi-text" }, renderText(info.description))
      : el("p", { class: "muted fp-empty" }, info.can_manage ? "Nothing here yet. Add an About section in Outhouse settings → Overview." : "Nothing here yet.")),
    info.rules ? section("rules", "Rules", el("div", { class: "gi-text gi-rules" }, renderText(info.rules))) : null,
    info.links.length ? section("links", "Links", el("div", { class: "gi-links" }, info.links.map((l) =>
      el("a", { class: "gi-link", href: l.url, target: "_blank", rel: "noopener noreferrer nofollow" },
        el("span", { class: "gi-link-label" }, l.label), el("span", { class: "gi-link-host muted small" }, (() => { try { return new URL(l.url).host; } catch { return ""; } })()))))) : null,
    section("you", "You here",
      el("ul", { class: "fp-since" },
        el("li", {}, "Joined ", el("strong", {}, giDate(info.joined_at)))),
      myRoles.length
        ? el("div", { class: "gi-roles" }, myRoles.map((r) => el("span", { class: "gi-role" }, el("span", { class: "swatch", style: `background:${hexColour(r.color) || "#a1a1aa"}` }), r.name)))
        : el("p", { class: "muted small fp-empty" }, "No roles here yet, just @everyone.")),
    info.name_history.length ? section("names", "Previously called",
      el("ul", { class: "fp-names" }, info.name_history.map((h) => el("li", { class: "fp-name-row" },
        el("div", { class: "fp-name-main" }, el("span", { class: "fp-name-old" }, h.old), el("span", { class: "fp-arrow" }, "→"), el("span", {}, h.new)),
        el("div", { class: "muted small" }, giDate(h.changed_at)))))) : null,
  );
  return dialog;
}

// "Outhouse info" first in an outhouse's menus (rail icons and the name at the top).
const giBaseGuildMenu = MENUS.guild;
MENUS.guild = function (e, node) {
  const id = node.dataset.id;
  const realOpen = openMenu;
  openMenu = (x, y, items, depth) => {
    openMenu = realOpen;
    return realOpen(x, y, [{ label: "Outhouse info", onClick: () => openGuildInfo(id) }, "sep", ...items], depth);
  };
  try { return giBaseGuildMenu(e, node); } finally { openMenu = realOpen; }
};

// ---------- Outhouse settings → Overview ----------

function giCounter(input, max) {
  const n = el("span", { class: "gi-count muted small" });
  const paint = () => { const len = [...input.value].length; n.textContent = `${len} / ${max}`; n.classList.toggle("over", len > max); };
  input.addEventListener("input", paint);
  paint();
  return n;
}

async function drawGuildOverview(page) {
  const gid = state.guildId;
  let info;
  try {
    info = await api("GET", `/api/guilds/${gid}/info`);
  } catch (err) {
    page.append(el("h2", {}, "Overview"), el("p", { class: "error" }, err.message));
    return;
  }
  if (state.guildId !== gid || !page.isConnected) return;
  const name = el("input", { type: "text", maxlength: GI_LIMITS.name, value: info.name, required: true });
  const tagline = el("input", { type: "text", maxlength: GI_LIMITS.tagline, value: info.tagline, placeholder: "Classic+ raiding, Tuesdays and Thursdays" });
  const about = el("textarea", { rows: 6, maxlength: GI_LIMITS.description, placeholder: "What this outhouse is about. **Bold**, `code`, links and @mentions work like in messages." });
  about.value = info.description;
  const rules = el("textarea", { rows: 6, maxlength: GI_LIMITS.rules, placeholder: "1. Be nice.\n2. No loot drama." });
  rules.value = info.rules;
  const linkRows = el("div", { class: "gi-link-rows" });
  const addLinkRow = (l = { label: "", url: "" }) => {
    const row = el("div", { class: "gi-link-row" },
      el("input", { type: "text", class: "gi-link-label-in", maxlength: GI_LIMITS.label, placeholder: "Label", value: l.label, "aria-label": "Link label" }),
      el("input", { type: "url", class: "gi-link-url-in", placeholder: "https://", value: l.url, "aria-label": "Link address" }),
      el("button", { type: "button", class: "icon-btn", title: "Remove link", "aria-label": "Remove link", onclick: () => { row.remove(); paintAdd(); } }, "✕"));
    linkRows.append(row);
    paintAdd();
  };
  const addLink = el("button", { type: "button", class: "ghost gi-add-link", onclick: () => addLinkRow() }, "＋ Add a link");
  const paintAdd = () => { addLink.hidden = linkRows.children.length >= GI_LIMITS.links; };
  info.links.forEach(addLinkRow);
  paintAdd();

  const picture = (kind) => {
    const url = kind === "icon" ? info.icon_url : info.banner_url;
    const file = el("input", { type: "file", accept: "image/png,image/jpeg,image/gif,image/webp", hidden: true });
    file.addEventListener("change", async () => {
      const f = file.files[0];
      if (!f) return;
      const body = new FormData();
      body.append("file", f);
      const saved = await tryApi("POST", `/api/guilds/${gid}/${kind}`, body).catch(() => null);
      if (saved) { toast(kind === "icon" ? "Picture saved." : "Banner saved."); redraw(); }
    });
    const preview = kind === "icon"
      ? giIcon(info, 72)
      : el("div", { class: `gi-banner-prev${url ? " img" : ""}` });
    if (kind !== "icon" && url) preview.style.backgroundImage = `url("${url}")`;
    return el("div", { class: `gi-pic gi-pic-${kind}` }, preview,
      el("div", { class: "gi-pic-btns" },
        el("button", { type: "button", class: "ghost", onclick: () => file.click() }, url ? "Change" : "Upload"),
        url ? el("button", { type: "button", class: "ghost danger", onclick: async () => {
          const saved = await tryApi("DELETE", `/api/guilds/${gid}/${kind}`).catch(() => null);
          if (saved) { toast(kind === "icon" ? "Picture removed." : "Banner removed."); redraw(); }
        } }, "Remove") : null), file);
  };
  const redraw = () => { const p = settingsPage(page.parentElement); drawGuildOverview(p); };

  const save = el("button", { class: "primary", type: "button", onclick: async () => {
    const links = [...linkRows.querySelectorAll(".gi-link-row")].map((r) => ({
      label: r.querySelector(".gi-link-label-in").value.trim(),
      url: r.querySelector(".gi-link-url-in").value.trim(),
    })).filter((l) => l.label || l.url);
    save.disabled = true;
    const saved = await tryApi("PATCH", `/api/guilds/${gid}`, {
      name: name.value, tagline: tagline.value, description: about.value, rules: rules.value, links,
    }).catch(() => null);
    save.disabled = false;
    if (saved) { toast("Outhouse saved."); redraw(); }
  } }, "Save changes");

  page.append(
    el("h2", {}, "Overview"),
    el("p", { class: "muted" }, "How your outhouse looks to its members. Pictures are checked by the image detector like profile pictures. Name changes are listed in Outhouse info, so members can see what it used to be called."),
    el("div", { class: "gi-pics" },
      el("div", { class: "field" }, "Picture", picture("icon")),
      el("div", { class: "field gi-field-banner" }, "Banner", picture("banner"))),
    el("label", { class: "field" }, el("span", { class: "gi-label-row" }, "Outhouse name", giCounter(name, GI_LIMITS.name)), name),
    el("label", { class: "field" }, el("span", { class: "gi-label-row" }, "Tagline", giCounter(tagline, GI_LIMITS.tagline)), tagline),
    el("label", { class: "field" }, el("span", { class: "gi-label-row" }, "About", giCounter(about, GI_LIMITS.description)), about),
    el("label", { class: "field" }, el("span", { class: "gi-label-row" }, "Rules", giCounter(rules, GI_LIMITS.rules)), rules),
    el("div", { class: "field" }, el("span", { class: "gi-label-row" }, "Links", el("span", { class: "muted small" }, `up to ${GI_LIMITS.links}, https only`)), linkRows, addLink),
    el("div", { class: "settings-actions" },
      el("button", { class: "ghost", type: "button", onclick: () => openGuildInfo(gid) }, "Preview info panel"),
      save),
  );
}
