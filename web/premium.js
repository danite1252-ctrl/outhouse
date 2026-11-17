// Premium: Golden Throne (you) and Golden Outhouse (an outhouse). See
// crates/server/src/api/premium.rs. There's no way to buy either yet: the
// site owner grants them (User settings → SITE OWNER → Premium, People,
// Outhouses), and Stripe will later switch the same entitlements on.
// Locked features stay visible with a small gold crown and a neutral
// "coming soon" note.
//
// Here: the crown badges (rail, member list, profile card, info panel,
// invite page, Outhouses directory), the Golden Throne page in User
// settings (status, free Golden Outhouse slot, app themes, profile colours,
// personal sounds), the Golden Outhouse page and vanity link in Outhouse
// settings, the owner's Premium page and Grant dialogs, app themes, the
// message-length counter, upload limits, "My sounds" on the soundboard, and
// animated pictures playing on hover in lists.
// Loaded after the other web client files (uses el, api, tryApi, state, …).
"use strict";

const GT_NAME = "Golden Throne";
const GO_NAME = "Golden Outhouse";
const PREMIUM_DEFAULT_PERKS = {
  upload_mb: 100, free_upload_mb: 25, message_chars: 2000, premium_message_chars: 4000, personal_sounds: 10,
  emoji_premium: true, animated_premium: false,
  themes: [
    { key: "ember", name: "Ember", free: true }, { key: "contrast", name: "High contrast", free: true },
    { key: "midnight", name: "Midnight blue", free: false }, { key: "forest", name: "Forest", free: false },
    { key: "royal", name: "Royal purple", free: false }, { key: "throne", name: "Gold throne", free: false },
  ],
};
// What each app theme looks like (for the picker's previews): night, panel, accent, glow.
const THEME_SWATCH = {
  ember: ["#121214", "#1c1c20", "#ff6b35", "#ffb347"],
  contrast: ["#000000", "#0b0b0b", "#ffd400", "#ffffff"],
  midnight: ["#0b1020", "#131a30", "#5b8cff", "#9cc0ff"],
  forest: ["#0d1510", "#152219", "#4caf6e", "#b5e48c"],
  royal: ["#120d1c", "#1c1530", "#a66cff", "#e0b3ff"],
  throne: ["#14110a", "#1f1a10", "#e6b422", "#ffe08a"],
};

state.premium = null; // GET /api/users/@me/premium

const premiumPerks = () => ({ ...PREMIUM_DEFAULT_PERKS, ...(state.premium?.perks || {}) });
const meGolden = () => !!state.me?.premium;
const guildGolden = (id = state.guildId) => !!state.guilds?.find((g) => g.id === id)?.premium;
// Other outhouses' emoji: Golden Throne unless the owner made it free.
function canUseOtherEmoji() { return !premiumPerks().emoji_premium || meGolden(); }

// The gold crown (an SVG, so it doesn't look like the owner's 👑 emoji).
function crownIcon(cls = "") {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  for (const [k, v] of Object.entries({ viewBox: "0 0 24 24", width: "1em", height: "1em", "aria-hidden": "true", class: `gold-crown-svg ${cls}`.trim() })) svg.setAttribute(k, v);
  const path = document.createElementNS(ns, "path");
  path.setAttribute("d", "M3 8.5l4.2 3.6L12 5l4.8 7.1L21 8.5l-1.8 9.5H4.8L3 8.5zM5 19.5h14V21H5z");
  path.setAttribute("fill", "currentColor");
  svg.append(path);
  return svg;
}
const goldBadge = (label, title = label) => el("span", { class: "gold-badge", title }, crownIcon(), label);
// "Golden Throne — coming soon" for things you can't use yet.
const soonLabel = (what) => `${what} — coming soon`;
function lockedChip(what = GT_NAME) {
  return el("span", { class: "gold-locked", title: `${soonLabel(what)}: there's no way to get it yet.` }, crownIcon(), soonLabel(what));
}

function premiumDate(iso) {
  return iso ? new Date(iso).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }) : "";
}
function premiumUntil(s) {
  if (!s?.active) return "";
  return s.forever || !s.until ? "Active, no end date" : `Active until ${premiumDate(s.until)}`;
}

// ---------- loading, themes ----------

function applyAppTheme(key) {
  const k = key && key !== "ember" ? key : "";
  if (k) document.documentElement.dataset.theme = k;
  else delete document.documentElement.dataset.theme;
  try { localStorage.setItem("outhouse_theme", k); } catch { /* storage blocked */ }
}
// Straight away from last time (no flash of Ember), then from the server.
try { const t = localStorage.getItem("outhouse_theme"); if (t) document.documentElement.dataset.theme = t; } catch { /* storage blocked */ }

async function loadPremium() {
  const s = await api("GET", "/api/users/@me/premium").catch(() => null);
  if (!s) return null;
  state.premium = s;
  if (state.me) state.me.premium = s.active;
  applyAppTheme(s.theme_in_use);
  composerCounter();
  return s;
}

// ---------- limits used by the rest of the client ----------

// The biggest file you can send in the channel you're in.
function maxUploadBytes() {
  const p = premiumPerks();
  const dm = !state.guildId;
  const golden = dm ? meGolden() : guildGolden();
  return (golden ? p.upload_mb : p.free_upload_mb) * 1024 * 1024;
}
function uploadTooBig(file) {
  const max = maxUploadBytes();
  if (file.size <= max) return false;
  const p = premiumPerks();
  const mb = Math.round(max / 1024 / 1024);
  const more = max < p.upload_mb * 1024 * 1024 ? (state.guildId ? ` ${GO_NAME}s take up to ${p.upload_mb} MB.` : ` ${GT_NAME} members can send up to ${p.upload_mb} MB in DMs.`) : "";
  toast(`${file.name} is over ${mb} MB.${more}`);
  return true;
}
function messageCharLimit() {
  const p = premiumPerks();
  return meGolden() ? Math.max(p.premium_message_chars, p.message_chars) : p.message_chars;
}

// "1,850 / 2,000" under the message box as you near the limit.
function composerCounter() {
  const input = $("composer-input");
  if (!input) return;
  let counter = $("composer-count");
  if (!counter) {
    counter = el("span", { id: "composer-count", class: "composer-count", "aria-live": "polite", hidden: true });
    input.parentElement?.append(counter);
    input.addEventListener("input", composerCounter);
  }
  const n = [...input.value].length;
  const max = messageCharLimit();
  counter.hidden = n < max - 300;
  counter.classList.toggle("over", n > max);
  counter.textContent = `${n.toLocaleString()} / ${max.toLocaleString()}`;
  counter.title = n > max ? (meGolden() ? "Too long to send." : `Too long to send. ${GT_NAME} members can send up to ${premiumPerks().premium_message_chars.toLocaleString()} characters.`) : "";
}

// ---------- animated pictures: still in lists until hovered ----------

const reducedMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;
const stillOf = (url) => (url && url.includes("anim=1") ? url.replace("anim=1", "still=1") : url);
// An <img> of a picture that moves: shown still (first frame) and played
// while hovered; `always` plays it unless the person asked for less motion.
function animatedImg(img, always = false) {
  const src = img.getAttribute("src") || "";
  if (!src.includes("anim=1")) return img;
  img.dataset.anim = src;
  img.dataset.still = stillOf(src);
  img.classList.add("anim-pic");
  if (always && !reducedMotion()) return img;
  img.src = img.dataset.still;
  return img;
}
const HOVER_HOSTS = ".guild, .member, .msg, .message, .voice-user, .sc-row, .dir-row, .pc-avatar, .fp-avatar, .gi-avatar, .dm-row, .gold-preview, .fr-row, button, li";
document.addEventListener("mouseover", (e) => {
  const host = e.target.closest?.(HOVER_HOSTS);
  if (!host) return;
  host.querySelectorAll("img[data-anim]").forEach((img) => { if (img.src !== img.dataset.anim) img.src = img.dataset.anim; });
});
document.addEventListener("mouseout", (e) => {
  const host = e.target.closest?.(HOVER_HOSTS);
  if (!host || host.contains(e.relatedTarget)) return;
  host.querySelectorAll("img[data-anim]").forEach((img) => {
    if (img.dataset.always === "1" && !reducedMotion()) return;
    if (img.src !== img.dataset.still) img.src = img.dataset.still;
  });
});

const premiumBaseAvatar = avatarFor;
avatarFor = function (id, name, size = 40, url) {
  const node = premiumBaseAvatar(id, name, size, url);
  return node.tagName === "IMG" ? animatedImg(node) : node;
};
// Always moving (profile cards and pages, the info panel), unless reduced motion.
function playAlways(root) {
  root?.querySelectorAll?.("img[data-anim]").forEach((img) => {
    img.dataset.always = "1";
    if (!reducedMotion()) img.src = img.dataset.anim;
  });
  return root;
}
// Backgrounds (banners) can't be swapped on hover: still when reduced motion.
const bannerUrl = (url) => (url && reducedMotion() ? stillOf(url) : url);

// ---------- the rail: crown on Golden Outhouses, icons play on hover ----------

if (typeof guildFace === "function") {
  const premiumBaseFace = guildFace;
  guildFace = function (g) {
    const face = premiumBaseFace(g);
    return face?.tagName === "IMG" ? animatedImg(face) : face;
  };
}
if (typeof railGuildButton === "function") {
  const premiumBaseRailButton = railGuildButton;
  railGuildButton = function (g, extra = "") {
    const btn = premiumBaseRailButton(g, extra);
    if (g.premium) {
      btn.classList.add("golden");
      btn.title = `${btn.title} · ${GO_NAME}`;
      btn.append(el("span", { class: "rail-crown", "aria-hidden": "true" }, crownIcon()));
    }
    return btn;
  };
}

// ---------- member list: a subtle crown by Golden Throne members ----------

const premiumBaseRenderMembers = renderMembers;
renderMembers = function (...args) {
  const out = premiumBaseRenderMembers(...args);
  document.querySelectorAll("#member-list .member[data-id]").forEach((row) => {
    const m = memberById(row.dataset.id);
    const nameLine = row.querySelector(".info > div");
    if (m?.premium && nameLine && !nameLine.querySelector(".gt-mini")) {
      nameLine.append(el("span", { class: "gt-mini", title: `${GT_NAME} supporter` }, crownIcon()));
    }
  });
  return out;
};

// ---------- profiles: gradient, supporter badge, pictures that move ----------

function gradientCss(p) {
  const g = p?.profile_gradient;
  return g && g.length === 2 ? `linear-gradient(135deg, ${hexColour(g[0]) || "#000"} 0%, ${hexColour(g[1]) || "#000"} 100%)` : null;
}
if (typeof profileCard === "function") {
  const premiumBaseCard = profileCard;
  profileCard = function (p, opts) {
    const card = premiumBaseCard(p, opts);
    const grad = gradientCss(p);
    if (grad) {
      card.classList.add("gt-gradient");
      card.style.setProperty("--gt-grad", grad);
      const banner = card.querySelector(".pc-banner");
      if (banner && !p.banner_url) banner.style.background = grad;
    }
    if (p.banner_url) {
      const banner = card.querySelector(".pc-banner");
      if (banner) banner.style.background = `center / cover no-repeat url("${bannerUrl(p.banner_url)}")`;
    }
    if (p.premium) card.querySelector(".pc-name")?.append(goldBadge(GT_NAME, `${GT_NAME} supporter`));
    return playAlways(card);
  };
}
if (typeof profileHeader === "function") {
  const premiumBaseHeader = profileHeader;
  profileHeader = function (p) {
    const head = premiumBaseHeader(p);
    const grad = gradientCss(p);
    const banner = head.querySelector(".fp-banner");
    if (grad && banner && !p.banner_url) banner.style.background = grad;
    if (grad) head.classList.add("gt-gradient"), head.style.setProperty("--gt-grad", grad);
    if (p.banner_url && banner) banner.style.background = `center / cover no-repeat url("${bannerUrl(p.banner_url)}")`;
    if (p.premium) head.querySelector(".fp-display")?.append(goldBadge(GT_NAME, `${GT_NAME} supporter`));
    return playAlways(head);
  };
}

// ---------- the Outhouse info panel ----------

if (typeof openGuildInfo === "function") {
  const premiumBaseInfo = openGuildInfo;
  openGuildInfo = async function (guildId = state.guildId) {
    const dialog = await premiumBaseInfo(guildId);
    const g = state.guilds.find((x) => x.id === (dialog?.dataset.guild || guildId));
    if (dialog && g?.premium) dialog.querySelector(".gi-name")?.append(goldBadge(GO_NAME));
    if (dialog) {
      dialog.querySelectorAll(".gi-icon img").forEach((img) => { animatedImg(img, true); });
      playAlways(dialog);
      const banner = dialog.querySelector(".gi-banner.img");
      const url = g?.banner_url;
      if (banner && url) banner.style.backgroundImage = `url("${bannerUrl(url)}")`;
    }
    return dialog;
  };
}

// ---------- User settings → Golden Throne ----------

const GT_PERKS = (p) => [
  ["🖼️", "Profile colours", "Pick two colours for a gradient on your profile card and page, and a gold supporter badge."],
  ["😀", "Emoji everywhere", p.emoji_premium ? "Use custom emoji from any of your outhouses in any outhouse or DM." : "Custom emoji from all your outhouses (free for everyone on this server right now)."],
  ["📎", `${p.upload_mb} MB files in DMs`, `Send files up to ${p.upload_mb} MB in DMs and group DMs (${p.free_upload_mb} MB otherwise).`],
  ["🔊", "Personal soundboard", `Up to ${p.personal_sounds} sounds of your own, usable in any voice channel, and one as your entrance sound everywhere.`],
  ["🎨", "App themes", "Midnight blue, Forest, Royal purple and Gold throne (High contrast is free for everyone)."],
  ["✍️", "Longer messages", `${p.premium_message_chars.toLocaleString()} characters a message (${p.message_chars.toLocaleString()} otherwise).`],
  ["🏰", `A free ${GO_NAME}`, `Put one ${GO_NAME} on any outhouse you're in (move it once a week).`],
];
const GO_PERKS = (p) => [
  ["🔗", "Vanity invite link", "A short link of your choosing, like /invite/the-long-drop."],
  ["📦", `${p.upload_mb} MB uploads`, `Files up to ${p.upload_mb} MB in every channel and thread (${p.free_upload_mb} MB otherwise).`],
  ["✨", "Animated icon and banner", p.animated_premium ? "GIF or animated WebP icon and banner." : "GIF or animated WebP icon and banner (free for everyone on this server right now)."],
  ["👑", "A gold badge", "On the outhouse in the rail, its info panel, invite page and the directory."],
];
function perkList(rows) {
  return el("ul", { class: "gold-perks" }, rows.map(([icon, title, text]) => el("li", {},
    el("span", { class: "gold-perk-icon", "aria-hidden": "true" }, icon),
    el("div", {}, el("strong", {}, title), el("div", { class: "muted small" }, text)))));
}
function goldHero(kind, s, extra = null) {
  const active = !!s?.active;
  return el("div", { class: `gold-hero${active ? " active" : ""}` },
    el("div", { class: "gold-hero-crown", "aria-hidden": "true" }, crownIcon()),
    el("div", { class: "gold-hero-text" },
      el("h3", {}, kind),
      el("p", { class: "gold-status" }, active ? premiumUntil(s) : soonLabel(kind)),
      active
        ? el("p", { class: "muted small" }, (s.entitlements || s.sources || []).map((e) => e.source === "slot" ? `Free slot from ${e.sponsor || "a member"}` : e.source === "grant" ? `Granted${e.note ? ` (${e.note})` : ""}` : "Supporter").join(" · "))
        : el("p", { class: "muted small" }, `There's no way to get ${kind} yet. When there is, it'll switch on everything below.`),
      extra));
}

async function drawGoldenThronePage(page) {
  page.replaceChildren(el("h2", {}, GT_NAME), el("p", { class: "muted" }, "Loading…"));
  const s = await loadPremium();
  if (!s) return page.replaceChildren(el("h2", {}, GT_NAME), el("p", { class: "muted" }, "Couldn't load your status."));
  const p = premiumPerks();
  const active = s.active;
  const redraw = () => drawGoldenThronePage(page);

  // The free Golden Outhouse slot.
  const slotGuild = s.slot?.guild_id ? state.guilds.find((g) => g.id === s.slot.guild_id) : null;
  const pick = el("select", { class: "select", "aria-label": "Outhouse for your free slot", disabled: !active },
    el("option", { value: "" }, "Choose an outhouse…"),
    state.guilds.filter((g) => !g.closed_until).map((g) => el("option", { value: g.id, selected: g.id === s.slot?.guild_id }, g.name)));
  const slot = el("section", { class: "gold-section", id: "gt-slot" },
    el("h3", {}, `Your free ${GO_NAME}`),
    el("p", { class: "muted small" }, `${GT_NAME} comes with one ${GO_NAME} you can put on any outhouse you're in. You can move it once every 7 days.`),
    el("div", { class: "inline-row" }, pick,
      el("button", { class: "primary", disabled: !active, onclick: async () => {
        if (!pick.value) return toast("Choose an outhouse first.");
        const r = await tryApi("PUT", "/api/users/@me/premium/slot", { guild_id: pick.value }).catch(() => null);
        if (r) { toast(`${GO_NAME} is on ${state.guilds.find((g) => g.id === pick.value)?.name ?? "that outhouse"}.`); redraw(); }
      } }, "Use it here"),
      slotGuild ? el("button", { class: "ghost", onclick: async () => {
        if (!(await confirmBox(`Take it off ${slotGuild.name}?`, `Its ${GO_NAME} perks stop (unless someone else sponsors it). Moving it somewhere else still waits for the 7 days.`, "Take it off"))) return;
        if (await tryApi("DELETE", "/api/users/@me/premium/slot").catch(() => null)) redraw();
      } }, "Take it off") : null),
    el("p", { class: "muted small gold-slot-now" }, slotGuild ? `On ${slotGuild.name} since ${premiumDate(s.slot.applied_at)}.${s.slot.can_move_at ? ` You can move it from ${premiumDate(s.slot.can_move_at)}.` : ""}` : active ? "Not used yet." : ""),
    active ? null : lockedChip());

  // App themes.
  const themes = el("div", { class: "gold-themes", role: "radiogroup", "aria-label": "App theme" },
    p.themes.map((t) => {
      const usable = t.free || active;
      const [night, panel, accent, glow] = THEME_SWATCH[t.key] || THEME_SWATCH.ember;
      return el("button", {
        type: "button", role: "radio", "aria-checked": String(s.theme_in_use === t.key),
        class: `gold-theme${s.theme_in_use === t.key ? " on" : ""}${usable ? "" : " locked"}`, "data-theme": t.key,
        title: usable ? t.name : `${t.name}: ${soonLabel(GT_NAME)}`,
        onclick: async () => {
          if (!usable) return toast(`${t.name} comes with ${GT_NAME} (coming soon).`);
          const r = await tryApi("PUT", "/api/users/@me/theme", { theme: t.key }).catch(() => null);
          if (r) { state.premium = r; applyAppTheme(r.theme_in_use); redraw(); }
        },
      },
      el("span", { class: "gold-theme-preview", style: `--n:${night};--p:${panel};--a:${accent};--g:${glow}` },
        el("i", { class: "tp-rail" }), el("i", { class: "tp-side" }), el("i", { class: "tp-msg" }), el("i", { class: "tp-btn" })),
      el("span", { class: "gold-theme-name" }, t.name,
        t.key === "contrast" ? el("span", { class: "gold-free" }, "Free · accessibility") : t.free ? null : el("span", { class: "gold-theme-lock" }, crownIcon())));
    }));

  // Profile colours.
  const g = s.profile_gradient || [0xff6b35, 0x6b2bd9];
  const c1 = el("input", { type: "color", value: hexColour(g[0]) || "#ff6b35", disabled: !active, "aria-label": "First colour" });
  const c2 = el("input", { type: "color", value: hexColour(g[1]) || "#6b2bd9", disabled: !active, "aria-label": "Second colour" });
  const sample = el("div", { class: "gold-grad-sample" });
  const paintSample = () => { sample.style.background = `linear-gradient(135deg, ${c1.value}, ${c2.value})`; };
  c1.addEventListener("input", paintSample); c2.addEventListener("input", paintSample); paintSample();
  const colours = el("section", { class: "gold-section", id: "gt-colours" },
    el("h3", {}, "Profile colours"),
    el("p", { class: "muted small" }, "Two colours make the gradient on your profile card and page (a banner picture still wins)."),
    el("div", { class: "inline-row" }, sample, c1, c2,
      el("button", { class: "primary", disabled: !active, onclick: async () => {
        const colors = [parseInt(c1.value.slice(1), 16), parseInt(c2.value.slice(1), 16)];
        if (await tryApi("PUT", "/api/users/@me/profile-gradient", { colors }).catch(() => null)) toast("Profile colours saved.");
      } }, "Save"),
      s.profile_gradient ? el("button", { class: "ghost", onclick: async () => {
        if (await tryApi("PUT", "/api/users/@me/profile-gradient", { colors: null }).catch(() => null)) redraw();
      } }, "Remove") : null),
    active ? null : lockedChip());

  const sounds = el("section", { class: "gold-section", id: "gt-sounds" }, el("h3", {}, "My sounds"), el("p", { class: "muted small" }, "Loading…"));
  page.replaceChildren(
    el("h2", {}, GT_NAME),
    goldHero(GT_NAME, s),
    el("section", { class: "gold-section" }, el("h3", {}, "What's included"), perkList(GT_PERKS(p)),
      el("p", { class: "muted small" }, "Safety tools, reporting, blocking, two-factor, teen protections, chat, voice and screen share stay free for everyone.")),
    el("section", { class: "gold-section", id: "gt-themes" }, el("h3", {}, "App theme"), themes),
    colours, slot, sounds);
  drawPersonalSounds(sounds, active);
}

function premiumPickFile(accept, onFile) {
  const f = el("input", { type: "file", accept, hidden: true });
  f.addEventListener("change", () => { if (f.files[0]) onFile(f.files[0]); f.remove(); });
  document.body.append(f);
  f.click();
}

async function drawPersonalSounds(box, active) {
  const d = await api("GET", "/api/users/@me/personal-sounds").catch(() => null);
  if (!d || !box.isConnected) return;
  const redraw = () => drawPersonalSounds(box, active);
  const add = () => premiumPickFile("audio/mpeg,audio/ogg,audio/wav,.mp3,.ogg,.wav", async (file) => {
    const name = await ask("Name this sound", { placeholder: "Victory fanfare", value: file.name.replace(/\.[^.]+$/, "").slice(0, 32), confirm: "Add" });
    if (!name) return;
    const form = new FormData();
    form.append("name", name);
    form.append("file", file, file.name);
    if (await tryApi("POST", "/api/users/@me/personal-sounds", form).catch(() => null)) { toast("Sound added."); redraw(); }
  });
  const entrance = async (snd) => {
    const on = d.entrance_id === snd.id;
    const r = await tryApi(on ? "DELETE" : "PUT", "/api/users/@me/premium-entrance", on ? undefined : { sound_id: snd.id }).catch(() => null);
    if (r) { toast(on ? "Back to your usual entrance sound." : `${snd.name} is your entrance sound.`); redraw(); }
  };
  box.replaceChildren(
    el("h3", {}, "My sounds ", el("span", { class: "muted small" }, `${d.sounds.length} / ${d.max}`)),
    el("p", { class: "muted small" }, "Your own soundboard sounds, in every voice channel's soundboard under My sounds (outhouses can turn them off). Pick one as your entrance sound everywhere. Same limits as outhouse sounds: up to 10 seconds and 1 MB."),
    d.sounds.length ? el("div", { class: "gold-sounds" }, d.sounds.map((snd) => el("div", { class: `gold-sound${snd.locked ? " locked" : ""}`, "data-id": snd.id },
      el("button", { class: "ghost", title: "Preview (just you)", onclick: () => (typeof playSound === "function" ? playSound(snd.url, snd.volume, true) : null) }, "▶"),
      el("span", { class: "gold-sound-name" }, snd.emoji || "🔊", " ", snd.name),
      d.entrance_id === snd.id ? el("span", { class: "rp-chip" }, "Entrance sound") : null,
      el("div", { style: "flex:1" }),
      snd.locked ? lockedChip() : el("button", { class: "ghost", onclick: () => entrance(snd) }, d.entrance_id === snd.id ? "Stop using as entrance" : "Use as entrance"),
      el("button", { class: "ghost danger", onclick: async () => {
        if (!(await confirmBox(`Delete ${snd.name}?`, "It's gone for good.", "Delete"))) return;
        if (await tryApi("DELETE", `/api/users/@me/personal-sounds/${encodeURIComponent(snd.id)}`).catch(() => null)) redraw();
      } }, "Delete"))))
      : el("p", { class: "muted small" }, active ? "No sounds yet." : "Your sounds would show here."),
    el("div", { class: "inline-row" },
      el("button", { class: "primary", disabled: !d.premium || d.sounds.length >= d.max, onclick: add }, "Add a sound"),
      d.premium ? null : lockedChip()));
}

// ---------- Outhouse settings → Golden Outhouse, and the vanity link ----------

async function drawGoldenOuthousePage(page) {
  const gid = state.guildId;
  page.replaceChildren(el("h2", {}, GO_NAME), el("p", { class: "muted" }, "Loading…"));
  const s = await api("GET", `/api/guilds/${gid}/premium`).catch(() => null);
  if (!s || !page.isConnected) return;
  const p = { ...premiumPerks(), ...(s.perks || {}) };
  const redraw = () => drawGoldenOuthousePage(page);
  const mine = state.premium?.slot;
  const myActive = meGolden();
  const allow = el("input", { type: "checkbox", checked: s.allow_personal_sounds, disabled: !s.can_manage });
  allow.addEventListener("change", async () => {
    const r = await tryApi("PUT", `/api/guilds/${gid}/premium-settings`, { allow_personal_sounds: allow.checked }).catch(() => null);
    if (!r) allow.checked = !allow.checked;
    else toast(allow.checked ? "Personal sounds allowed." : "Personal sounds turned off here.");
  });
  page.replaceChildren(
    el("h2", {}, GO_NAME),
    goldHero(GO_NAME, s, s.vanity ? el("p", { class: "small" }, "Vanity link: ", el("code", {}, s.vanity.url), s.vanity.active ? "" : " (paused)") : null),
    el("section", { class: "gold-section" }, el("h3", {}, "What's included"), perkList(GO_PERKS(p)),
      el("p", { class: "muted small" }, `Any member will be able to sponsor ${GO_NAME} for an outhouse. Members with ${GT_NAME} also get one free ${GO_NAME} to put on any outhouse they're in.`)),
    el("section", { class: "gold-section" }, el("h3", {}, "Your free slot"),
      myActive
        ? el("div", { class: "inline-row" },
          el("span", { class: "muted small" }, s.my_slot_here ? "Your free slot is on this outhouse." : mine?.guild_id ? `Your free slot is on ${mine.guild_name || "another outhouse"}.` : "You haven't used your free slot yet."),
          s.my_slot_here ? null : el("button", { class: "primary", onclick: async () => {
            const r = await tryApi("PUT", "/api/users/@me/premium/slot", { guild_id: gid }).catch(() => null);
            if (r) { state.premium = r; toast(`This is a ${GO_NAME} now.`); redraw(); }
          } }, "Use my free slot here"))
        : el("p", { class: "muted small" }, `${GT_NAME} members get one free ${GO_NAME}. `, lockedChip(GT_NAME))),
    el("section", { class: "gold-section" }, el("h3", {}, "Personal soundboard sounds"),
      el("label", { class: "rp-check" }, allow, `Let ${GT_NAME} members play their own sounds here (and use them as entrance sounds)`),
      el("p", { class: "muted small" }, "On by default. Blocked people's sounds never play for whoever blocked them.")),
  );
}

// The vanity link box at the top of Outhouse settings → Invites.
async function vanityBox(gid) {
  const s = await api("GET", `/api/guilds/${gid}/premium`).catch(() => null);
  if (!s) return null;
  const box = el("section", { class: `gold-section gold-vanity${s.active ? "" : " locked"}` });
  const input = el("input", { type: "text", maxlength: 32, placeholder: "the-long-drop", value: s.vanity?.code || "", disabled: !s.active || !s.can_manage, "aria-label": "Vanity link", spellcheck: "false", autocapitalize: "off" });
  const err = el("p", { class: "error small" });
  const live = el("span", { class: "gold-vanity-prefix" }, `${location.host}/invite/`);
  input.addEventListener("input", () => {
    const v = input.value.toLowerCase();
    if (v !== input.value) input.value = v;
    err.textContent = v && !/^[a-z0-9](?:[a-z0-9]|-(?=[a-z0-9])){2,31}$/.test(v) ? "3–32 letters, numbers and single hyphens (not at either end)." : "";
  });
  const paused = s.vanity && !s.vanity.active;
  box.append(
    el("h3", {}, crownIcon(), " Vanity invite link"),
    el("p", { class: "muted small" }, `A permanent link that's easy to remember. It works like any invite (bans, join messages and the app links all apply) and counts its uses.${s.active ? "" : ` Comes with ${GO_NAME}.`}`),
    el("div", { class: "inline-row gold-vanity-row" }, live, input,
      el("button", { class: "primary", disabled: !s.active || !s.can_manage, onclick: async () => {
        if (err.textContent) return;
        const r = await tryApi("PUT", `/api/guilds/${gid}/vanity`, { code: input.value.trim() }).catch((e) => { err.textContent = e.message; return null; });
        if (r) { toast("Vanity link saved."); box.replaceWith(await vanityBox(gid)); }
      } }, "Save"),
      s.vanity && s.can_manage ? el("button", { class: "ghost", onclick: async () => {
        if (!(await confirmBox("Remove the vanity link?", "It stops working straight away and someone else could take the name.", "Remove"))) return;
        if (await tryApi("DELETE", `/api/guilds/${gid}/vanity`).catch(() => null)) box.replaceWith(await vanityBox(gid));
      } }, "Remove") : null),
    err,
    s.vanity ? el("p", { class: "small gold-vanity-now" },
      paused ? el("span", { class: "rp-chip" }, "Paused") : el("span", { class: "rp-chip gold-chip" }, "Live"), " ",
      el("code", {}, s.vanity.url), ` · ${s.vanity.uses} ${s.vanity.uses === 1 ? "use" : "uses"}`,
      paused ? el("span", { class: "muted" }, ` · ${GO_NAME} ended: the link isn't active right now and the name is held for this outhouse until ${premiumDate(s.vanity.release_at)}.`) : null,
      s.vanity.active ? el("button", { class: "link", onclick: () => (typeof copyText === "function" ? copyText(s.vanity.url).then(() => toast("Link copied.")) : null) }, " Copy") : null) : null,
    s.active ? null : lockedChip(GO_NAME));
  return box;
}

if (typeof drawInviteSettings === "function") {
  const premiumBaseInvites = drawInviteSettings;
  drawInviteSettings = async function (page) {
    await premiumBaseInvites(page);
    if (!can("manage_guild")) return;
    const box = await vanityBox(state.guildId);
    if (box && page.isConnected) page.querySelector("h2")?.after(box);
  };
}

// ---------- the owner's Premium page, Grant dialogs ----------

const GRANT_LENGTHS = [["month", "1 month"], ["year", "1 year"], ["forever", "Forever"], ["custom", "Until a date…"]];
// grantDialog("user" | "outhouse", id, name) -> true when granted
async function grantDialog(kind, id, name) {
  const what = kind === "user" ? GT_NAME : GO_NAME;
  const length = el("select", { class: "select" }, GRANT_LENGTHS.map(([v, l]) => el("option", { value: v }, l)));
  const date = el("input", { type: "date", hidden: true, min: new Date(Date.now() + 86400000).toISOString().slice(0, 10) });
  const note = el("input", { type: "text", maxlength: 200, placeholder: "e.g. Thanks for running raids" });
  length.addEventListener("change", () => { date.hidden = length.value !== "custom"; });
  return new Promise((resolve) => {
    const dialog = el("dialog", { class: "report-dialog sc-form gold-grant" },
      el("form", { method: "dialog" },
        el("h2", {}, crownIcon(), ` Grant ${what}`),
        el("p", { class: "muted small", style: "margin:0" }, kind === "user"
          ? `@${name} gets every ${GT_NAME} perk, including one free ${GO_NAME} slot. It's written to the safety log.`
          : `${name} gets every ${GO_NAME} perk. It's written to the safety log and the outhouse's audit log.`),
        el("label", { class: "field" }, "How long", length, date),
        el("label", { class: "field" }, "Note (optional, only staff see it)", note),
        el("div", { class: "row" },
          el("button", { type: "button", class: "ghost", onclick: () => dialog.close("") }, "Cancel"),
          el("button", { type: "submit", class: "primary", value: "ok" }, "Grant"))));
    dialog.addEventListener("close", async () => {
      const ok = dialog.returnValue === "ok";
      dialog.remove();
      if (!ok) return resolve(false);
      const body = { duration: length.value, note: note.value.trim() };
      if (length.value === "custom") {
        if (!date.value) { toast("Pick a date."); return resolve(false); }
        body.ends_at = new Date(`${date.value}T23:59:59`).toISOString();
      }
      const path = kind === "user" ? `/api/safety/users/${id}/premium` : `/api/safety/outhouses/${id}/premium`;
      const r = await tryApi("POST", path, body).catch(() => null);
      if (r) toast(`${what} granted.`);
      resolve(!!r);
    });
    document.body.append(dialog);
    dialog.showModal();
  });
}
async function revokePremium(kind, id, name, entId = null) {
  const what = kind === "user" ? GT_NAME : GO_NAME;
  if (!(await confirmBox(`Revoke ${what} from ${name}?`, `Granted ${what} ${entId ? "(this one)" : "(every active grant)"} ends now. Things they made stay; perks lock.`, "Revoke"))) return false;
  const path = kind === "user" ? `/api/safety/users/${id}/premium/revoke` : `/api/safety/outhouses/${id}/premium/revoke`;
  const r = await tryApi("POST", path, entId ? { id: entId } : {}).catch(() => null);
  if (r) toast(`${what} revoked.`);
  return !!r;
}

// People → someone: their Golden Throne and the buttons.
function goldenPersonSection(u, refresh) {
  if (!u.golden || u.bot) return null;
  const s = u.golden;
  const canGrant = typeof sCan === "function" && sCan("grant_premium");
  return el("section", { class: "gold-section sc-golden" },
    el("h3", {}, crownIcon(), ` ${GT_NAME}`),
    el("p", { class: "small" }, s.active ? premiumUntil(s) : "Not active.",
      s.entitlements?.length ? el("span", { class: "muted" }, ` · ${s.entitlements.map((e) => `${e.source}${e.ends_at ? ` until ${premiumDate(e.ends_at)}` : ", forever"}${e.note ? ` (${e.note})` : ""}`).join("; ")}`) : null),
    canGrant ? el("div", { class: "sc-actions" },
      el("button", { class: "ghost gold-btn", onclick: async () => { if (await grantDialog("user", u.id, u.username)) refresh(); } }, `Grant ${GT_NAME}…`),
      s.active ? el("button", { class: "ghost", onclick: async () => { if (await revokePremium("user", u.id, `@${u.username}`)) refresh(); } }, "Revoke") : null) : null);
}

// Outhouses directory → an outhouse.
function goldenOuthouseSection(d, refresh) {
  const s = d.golden;
  if (!s) return null;
  const canGrant = typeof sCan === "function" && sCan("grant_premium");
  return el("section", { class: "gold-section dir-golden" },
    el("h3", {}, crownIcon(), ` ${GO_NAME}`),
    el("p", { class: "small" }, s.active ? premiumUntil(s) : "Not active.",
      s.sources?.length ? el("span", { class: "muted" }, ` · ${s.sources.map((e) => e.source === "slot" ? `free slot from ${e.sponsor}` : `${e.source}${e.ends_at ? ` until ${premiumDate(e.ends_at)}` : ", forever"}${e.note ? ` (${e.note})` : ""}`).join("; ")}`) : null,
      s.vanity ? el("span", { class: "muted" }, ` · vanity link /invite/${s.vanity}`) : null),
    canGrant ? el("div", { class: "sc-actions" },
      el("button", { class: "ghost gold-btn", onclick: async () => { if (await grantDialog("outhouse", d.id, d.name)) refresh(); } }, `Grant ${GO_NAME}…`),
      s.sources?.some((e) => e.source === "grant") ? el("button", { class: "ghost", onclick: async () => { if (await revokePremium("outhouse", d.id, d.name)) refresh(); } }, "Revoke") : null) : null);
}

const premiumPage = { kind: "all", status: "active", q: "" };
async function drawPremiumPage(page, { stillMine, draw }) {
  const search = el("input", { type: "search", class: "dir-search", placeholder: "Search by username or outhouse", value: premiumPage.q, "aria-label": "Search" });
  const kind = el("select", { class: "select", "aria-label": "Show" }, [["all", "People and outhouses"], ["people", `${GT_NAME} (people)`], ["outhouses", `${GO_NAME} (outhouses)`]].map(([v, l]) => el("option", { value: v, selected: premiumPage.kind === v }, l)));
  const status = el("select", { class: "select", "aria-label": "Status" }, [["active", "Active"], ["ended", "Ended or revoked"], ["all", "All"]].map(([v, l]) => el("option", { value: v, selected: premiumPage.status === v }, l)));
  const list = el("div", { class: "sc-table-wrap" });
  const switches = el("section", { class: "gold-section gold-switches" });
  page.replaceChildren(
    el("h2", {}, crownIcon(), " Premium"),
    el("p", { class: "muted small", style: "margin:0" }, `${GT_NAME} is per person (and comes with one free ${GO_NAME} slot); ${GO_NAME} is per outhouse. There's no way to buy either yet, so they're granted here, from People, or from the Outhouses directory. Everything is written to the safety log; grants that run out end by themselves.`),
    el("div", { class: "gold-tools" }, search, kind, status),
    list, switches);
  const load = async () => {
    const q = new URLSearchParams({ kind: premiumPage.kind, status: premiumPage.status, q: premiumPage.q });
    const d = await tryApi("GET", `/api/safety/premium?${q}`).catch(() => null);
    if (!d || !stillMine() || !page.isConnected) return;
    const rows = d.entitlements;
    list.replaceChildren(rows.length ? el("table", { class: "sc-table gold-table" },
      el("thead", {}, el("tr", {}, ["", "Who", "How", "Since", "Ends", "Note", ""].map((h) => el("th", {}, h)))),
      el("tbody", {}, rows.map((r) => el("tr", { class: r.active ? "" : "expired" },
        el("td", { class: "gt-kind" }, r.kind === "user" ? el("span", { title: GT_NAME }, "🪑") : el("span", { title: GO_NAME }, "🏠")),
        el("td", { class: "gt-who" }, el("button", { class: "link", onclick: () => (r.kind === "user" ? draw("people", { openPerson: r.target_id }) : draw("outhouses", { outhouseId: r.target_id })) }, r.kind === "user" ? `@${r.target_name}` : r.target_name)),
        el("td", {}, r.source === "slot" ? `Free slot (@${r.sponsor})` : r.source === "grant" ? `Grant${r.granted_by ? ` by @${r.granted_by}` : ""}` : r.source),
        el("td", {}, premiumDate(r.starts_at)),
        el("td", {}, r.revoked_at ? `Revoked ${premiumDate(r.revoked_at)}` : r.ends_at ? (r.active ? premiumDate(r.ends_at) : `Ended ${premiumDate(r.ends_at)}`) : r.source === "slot" ? (r.active ? "While they have it" : "Inactive") : "Never"),
        el("td", { class: "muted small" }, r.note || ""),
        el("td", {}, r.active && r.source === "grant" ? el("button", { class: "ghost", onclick: async () => {
          if (await revokePremium(r.kind === "user" ? "user" : "outhouse", r.target_id, r.kind === "user" ? `@${r.target_name}` : r.target_name, r.id)) load();
        } }, "Revoke") : null)))))
      : el("p", { class: "muted" }, "Nothing here."));
    const st = d.settings;
    const sw = (key, label, help) => {
      const box = el("input", { type: "checkbox", checked: !!st[key], disabled: !d.owner });
      box.addEventListener("change", async () => {
        const r = await tryApi("PUT", "/api/safety/premium-settings", { [key]: box.checked }).catch(() => null);
        if (!r) box.checked = !box.checked; else { toast("Saved."); loadPremium(); }
      });
      return el("label", { class: "rp-check" }, box, el("span", {}, el("strong", {}, label), el("span", { class: "muted small" }, ` ${help}`)));
    };
    const free = el("input", { type: "number", min: 100, max: 20000, value: st.message_chars, disabled: !d.owner, "aria-label": "Message length for everyone" });
    const prem = el("input", { type: "number", min: 100, max: 20000, value: st.premium_message_chars, disabled: !d.owner, "aria-label": `Message length with ${GT_NAME}` });
    switches.replaceChildren(
      el("h3", {}, "Which perks are premium"),
      el("p", { class: "muted small" }, `Your switches. Defaults come from the server's environment (${st.defaults.message_chars} / ${st.defaults.premium_message_chars} characters, emoji ${st.defaults.emoji_premium ? "premium" : "free"}, animated pictures ${st.defaults.animated_premium ? "premium" : "free"}).${d.owner ? "" : " Only the site owner can change them."}`),
      sw("emoji_premium", "Other outhouses' emoji need Golden Throne", "Off: anyone can use emoji from any outhouse they're in."),
      sw("animated_premium", "Animated pictures need premium", "Off: animated avatars, banners and outhouse pictures are free (as now). On: without premium only the first frame shows (files are kept)."),
      el("div", { class: "inline-row" },
        el("label", { class: "field" }, "Message length (everyone)", free),
        el("label", { class: "field" }, `With ${GT_NAME}`, prem),
        d.owner ? el("button", { class: "primary", onclick: async () => {
          const r = await tryApi("PUT", "/api/safety/premium-settings", { message_chars: Number(free.value), premium_message_chars: Number(prem.value) }).catch(() => null);
          if (r) { toast("Saved."); loadPremium(); }
        } }, "Save") : null,
        d.owner ? el("button", { class: "ghost", onclick: async () => {
          if (await tryApi("PUT", "/api/safety/premium-settings", { reset: true }).catch(() => null)) { toast("Back to the defaults."); load(); loadPremium(); }
        } }, "Use the defaults") : null));
  };
  let timer = null;
  search.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(() => { premiumPage.q = search.value.trim(); load(); }, 250); });
  kind.addEventListener("change", () => { premiumPage.kind = kind.value; load(); });
  status.addEventListener("change", () => { premiumPage.status = status.value; load(); });
  await load();
}

// ---------- soundboard: "My sounds" ----------

if (typeof sbLoad === "function") {
  const premiumBaseSbLoad = sbLoad;
  sbLoad = async function () {
    const [, mine] = await Promise.all([premiumBaseSbLoad(), api("GET", "/api/users/@me/personal-sounds").catch(() => null)]);
    sb.personal = mine;
  };
  const premiumBaseSections = sbSections;
  sbSections = function () {
    const out = premiumBaseSections();
    const mine = sb.personal;
    if (!mine?.sounds?.length) return out;
    const q = sb.query.trim().toLowerCase();
    const sounds = mine.sounds.filter((s) => !q || s.name.toLowerCase().includes(q) || (s.emoji || "").includes(q));
    const at = out.findIndex((s) => s.key === "classics");
    out.splice(at < 0 ? out.length : at + 1, 0, { key: "mine", icon: "👑", title: mine.premium ? "My sounds" : `My sounds · ${soonLabel(GT_NAME)}`, sounds });
    return out;
  };
  const premiumBaseTile = sbTile;
  sbTile = function (snd) {
    const tile = premiumBaseTile(snd);
    if (!snd.personal) return tile;
    tile.classList.add("sb-personal");
    tile.querySelector(".sb-star")?.remove();
    if (snd.locked) {
      tile.classList.add("locked");
      const play = tile.querySelector(".sb-play");
      if (play) {
        play.onclick = () => toast(`Personal sounds come with ${GT_NAME} (coming soon).`);
        play.title = `${snd.name}: ${soonLabel(GT_NAME)}`;
      }
    }
    return tile;
  };
}

// ---------- settings pages: where they go ----------

const premiumBaseUserSettings = openUserSettings;
openUserSettings = async function (tab = "account", opts = {}) {
  const dialog = await premiumBaseUserSettings(tab === "golden" ? "account" : tab, opts);
  if (!dialog?._draw) return dialog;
  const baseDraw = dialog._draw;
  const nav = dialog.querySelector(".settings-nav");
  // The nav is redrawn on every page change: put our button back each time.
  const addNav = () => {
    if (!nav || nav.querySelector('[data-tab="golden"]')) return;
    const btn = el("button", { class: `gold-nav${dialog.dataset.tab === "golden" ? " active" : ""}`, "data-tab": "golden", onclick: () => draw("golden") }, crownIcon(), ` ${GT_NAME}`);
    const anchor = nav.querySelector('[data-tab="look"]');
    if (anchor) anchor.after(btn); else nav.prepend(btn);
  };
  const draw = (active, extra = {}) => {
    if (active !== "golden") return baseDraw(active, extra);
    baseDraw("account");
    dialog.dataset.tab = "golden";
    dialog.dataset.view = "golden";
    nav?.querySelectorAll("button.active").forEach((b) => b.classList.remove("active"));
    addNav();
    nav?.querySelector('[data-tab="golden"]')?.classList.add("active");
    return drawGoldenThronePage(settingsPage(dialog.querySelector(".settings-main")));
  };
  dialog._draw = draw;
  if (nav) new MutationObserver(addNav).observe(nav, { childList: true });
  addNav();
  if (tab === "golden") await draw("golden");
  return dialog;
};

const premiumBaseSettings = openSettings;
openSettings = async function (tab = "roles", selectedRoleId = null) {
  const wantGolden = tab === "golden";
  await premiumBaseSettings(wantGolden ? (can("manage_guild") ? "overview" : "roles") : tab, selectedRoleId);
  const dialog = [...document.querySelectorAll("dialog.settings")].pop();
  const nav = dialog?.querySelector(".settings-nav");
  if (!nav) return;
  const show = () => {
    nav.querySelectorAll("button.active").forEach((b) => b.classList.remove("active"));
    nav.querySelector('[data-tab="golden"]')?.classList.add("active");
    drawGoldenOuthousePage(settingsPage(dialog.querySelector(".settings-main")));
  };
  const add = () => {
    if (nav.querySelector('[data-tab="golden"]')) return;
    const btn = el("button", { class: "gold-nav", "data-tab": "golden", onclick: show }, crownIcon(), ` ${GO_NAME}`);
    nav.querySelector("h3")?.after(btn);
  };
  // The nav is redrawn on every tab change (after the page is drawn).
  let pending = wantGolden;
  const sync = () => {
    if (!nav.querySelector("h3")) return;
    add();
    if (pending) { pending = false; show(); }
  };
  new MutationObserver(sync).observe(nav, { childList: true });
  sync();
};

if (typeof SITE_OWNER_TABS !== "undefined") {
  SITE_OWNER_TABS.splice(SITE_OWNER_TABS.findIndex(([id]) => id === "names") + 1, 0, ["premium", "Premium"]);
  STAFF_TAB_PERMS.premium = ["grant_premium"];
  const premiumBaseOwnerTab = drawSiteOwnerTab;
  drawSiteOwnerTab = function (page, key, ctx) {
    if (key === "premium") return drawPremiumPage(page, ctx);
    return premiumBaseOwnerTab(page, key, ctx);
  };
}

// ---------- live updates ----------

const premiumBaseHandle = handleEvent;
handleEvent = function (ev) {
  premiumBaseHandle(ev);
  if (ev.t === "READY") {
    if (ev.d?.user && state.me) state.me.premium = !!ev.d.user.premium;
    loadPremium();
  }
  if (ev.t === "USER_UPDATE" && ev.d && state.me && ev.d.id === state.me.id) {
    const was = state.me.premium;
    state.me.premium = !!ev.d.premium;
    if (ev.d.avatar_url !== undefined) state.me.avatar_url = ev.d.avatar_url;
    loadPremium().then(() => {
      if (was !== state.me.premium) document.querySelectorAll('dialog.user-settings[data-tab="golden"]').forEach((d) => d._draw?.("golden"));
    });
  }
  if (ev.t === "GUILD_UPDATE" || ev.t === "GUILD_CREATE") composerCounter();
  if (ev.t === "MESSAGE_CREATE" && ev.d?.author_id === state.me?.id) composerCounter();
};
