// Soundboard: the Discord-style popover above the voice panel (search,
// a rail to jump between sections, favourites, recently used, the built-in
// "Outhouse classics", this outhouse's sounds and sounds from your other
// outhouses), plus the "Manage sounds" dialog. Clips play on each
// listener's own machine (see playSound in app.js); the mic is never touched.
// Loaded after app.js and voice-extras.js.

const SB_ICONS = {
  // A speaker with a couple of musical notes.
  board: '<svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 10v4h3l4 3.5v-11L6 10z"/><path d="M14 9.5a3.5 3.5 0 0 1 0 5"/><path d="M18 4v7.2"/><circle cx="16.4" cy="11.6" r="1.6" fill="currentColor"/><path d="M18 4l3 1"/></svg>',
  speaker: '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M11 5 6 9H2v6h4l5 4z"/><path d="M15.5 8.5a5 5 0 0 1 0 7M19 5a10 10 0 0 1 0 14"/></svg>',
  search: '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M20 20l-4-4"/></svg>',
};
const SB_COOLDOWN_MS = 2000;
const SB_RECENT_MAX = 12;
const sb = { guild: [], all: [], query: "", coolUntil: 0, showVolume: false, guildId: null };

function soundboardButton() {
  const btn = el("button", {
    class: "vp-btn sb-open",
    title: "Soundboard",
    "aria-label": "Soundboard",
    onclick: (e) => {
      if (popover?.querySelector(".sb-board")) return closePopover();
      openSoundboard(e.currentTarget);
    },
  });
  btn.innerHTML = SB_ICONS.board;
  return btn;
}

// The outhouse whose soundboard you're using: where you're in voice.
const sbGuildId = () => state.voice.guildId || state.guildId;

async function sbLoad() {
  const gid = sbGuildId();
  const [guild, all] = await Promise.all([
    api("GET", `/api/guilds/${gid}/sounds`).catch(() => []),
    api("GET", "/api/users/@me/sounds").catch(() => []),
  ]);
  sb.guild = guild;
  sb.all = all;
  sb.guildId = gid;
}

function onSoundsChanged() {
  if (!popover?.querySelector(".sb-board")) return;
  sbLoad().then(() => sbRedraw());
}

function sbRecentIds() {
  const ids = prefs.get("sb_recent", []);
  return Array.isArray(ids) ? ids : [];
}

function sbRemember(id) {
  prefs.set("sb_recent", [id, ...sbRecentIds().filter((x) => x !== id)].slice(0, SB_RECENT_MAX));
}

function sbMatches(snd) {
  const q = sb.query.trim().toLowerCase();
  return !q || snd.name.toLowerCase().includes(q) || (snd.emoji || "").includes(q);
}

// Every playable sound once, by id.
function sbIndex() {
  const byId = new Map();
  for (const snd of [...sb.guild.filter((s) => !s.hidden), ...sb.all]) if (!byId.has(snd.id)) byId.set(snd.id, snd);
  return byId;
}

function sbSections() {
  const byId = sbIndex();
  const gid = sb.guildId;
  const guildName = (id) => state.guilds.find((g) => g.id === id)?.name ?? "Another outhouse";
  const out = [
    { key: "fav", icon: "⭐", title: "Favourites", sounds: [...byId.values()].filter((s) => s.favourite) },
    { key: "recent", icon: "🕑", title: "Recently used", sounds: sbRecentIds().map((id) => byId.get(id)).filter(Boolean) },
    { key: "classics", icon: "🚽", title: "Outhouse classics", sounds: sb.guild.filter((s) => s.builtin && !s.hidden) },
    { key: `g${gid}`, icon: initials(guildName(gid)), title: guildName(gid), guild: true, sounds: sb.guild.filter((s) => !s.builtin) },
  ];
  const others = new Map();
  for (const snd of sb.all) {
    if (snd.guild_id === gid) continue;
    if (!others.has(snd.guild_id)) others.set(snd.guild_id, []);
    others.get(snd.guild_id).push(snd);
  }
  for (const [id, sounds] of others) {
    out.push({ key: `g${id}`, icon: initials(guildName(id)), title: guildName(id), guild: true, sounds });
  }
  return out.map((sec) => ({ ...sec, sounds: sec.sounds.filter(sbMatches) }));
}

async function sbPlay(snd, tile) {
  if (Date.now() < sb.coolUntil) return toast("Easy on the soundboard!");
  const ok = await tryApi("POST", `/api/sounds/${encodeURIComponent(snd.id)}/play`).then(() => true).catch(() => false);
  if (!ok) return;
  sbRemember(snd.id);
  sb.coolUntil = Date.now() + SB_COOLDOWN_MS;
  const board = tile?.closest(".sb-board") || popover?.querySelector(".sb-board");
  board?.classList.remove("cooling");
  void board?.offsetWidth; // restart the sweep animation
  board?.classList.add("cooling");
  setTimeout(() => {
    if (Date.now() >= sb.coolUntil) document.querySelector(".sb-board")?.classList.remove("cooling");
  }, SB_COOLDOWN_MS);
}

async function sbFavourite(snd) {
  const on = !snd.favourite;
  const ok = await tryApi(on ? "PUT" : "DELETE", `/api/sounds/${encodeURIComponent(snd.id)}/favourite`).then(() => true).catch(() => false);
  if (!ok) return;
  for (const s of [...sb.guild, ...sb.all]) if (s.id === snd.id) s.favourite = on;
  sbRedraw();
}

function sbTile(snd) {
  const tile = el("div", { class: `sb-tile${snd.favourite ? " fav" : ""}`, "data-id": snd.id },
    el("button", {
      class: "sb-play",
      title: snd.name,
      onclick: (e) => sbPlay(snd, e.currentTarget),
      oncontextmenu: (e) => {
        e.preventDefault();
        e.stopPropagation();
        openMenu(e.clientX, e.clientY, [
          { label: "Preview (just you)", onClick: () => playSound(snd.url, snd.volume, true) },
          { label: snd.favourite ? "Remove from favourites" : "Add to favourites", onClick: () => sbFavourite(snd) },
        ]);
      },
    }, el("span", { class: "sb-emoji" }, snd.emoji || "🔊"), el("span", { class: "sb-name" }, snd.name)),
    el("button", { class: "sb-prev", title: "Preview (just you)", "aria-label": `Preview ${snd.name}`, tabindex: -1, onclick: () => playSound(snd.url, snd.volume, true) }, "▶"),
    el("button", { class: "sb-star", title: snd.favourite ? "Remove from favourites" : "Add to favourites", "aria-label": snd.favourite ? `Remove ${snd.name} from favourites` : `Add ${snd.name} to favourites`, tabindex: -1, onclick: () => sbFavourite(snd) }, snd.favourite ? "★" : "☆"));
  return tile;
}

function sbBody() {
  const collapsed = new Set(prefs.get("sb_collapsed", []));
  const sections = sbSections();
  const searching = !!sb.query.trim();
  const list = el("div", { class: "sb-list" });
  let shown = 0;
  for (const sec of sections) {
    if (!sec.sounds.length && (searching || sec.key === "recent" || (sec.guild && sec.key !== `g${sb.guildId}`))) continue;
    const isClosed = collapsed.has(sec.key) && !searching;
    shown += sec.sounds.length;
    list.append(el("section", { class: `sb-sec${isClosed ? " closed" : ""}`, "data-sec": sec.key },
      el("button", {
        class: "sb-sec-head",
        "aria-expanded": String(!isClosed),
        onclick: () => {
          const now = new Set(prefs.get("sb_collapsed", []));
          if (now.has(sec.key)) now.delete(sec.key); else now.add(sec.key);
          prefs.set("sb_collapsed", [...now]);
          sbRedraw();
        },
      }, el("span", { class: "sb-caret" }, "▾"), el("span", {}, sec.title)),
      isClosed ? null : sec.sounds.length
        ? el("div", { class: "sb-grid" }, sec.sounds.map(sbTile))
        : el("p", { class: "muted small sb-empty" }, sec.key === "fav" ? "Tap ☆ on a sound to keep it here." : "No sounds here yet.")));
  }
  if (searching && !shown) list.append(el("p", { class: "muted sb-none" }, `No sounds match “${sb.query.trim()}”.`));
  const rail = el("nav", { class: "sb-rail", "aria-label": "Soundboard sections" },
    sections.filter((sec) => list.querySelector(`[data-sec="${sec.key}"]`)).map((sec) =>
      el("button", {
        class: `sb-rail-btn${sec.guild ? " guild" : ""}`,
        title: sec.title,
        "aria-label": sec.title,
        tabindex: -1,
        onclick: () => list.querySelector(`[data-sec="${sec.key}"]`)?.scrollIntoView({ block: "start", behavior: "smooth" }),
      }, sec.icon)));
  return el("div", { class: "sb-body" }, rail, list);
}

function sbDraw() {
  const search = el("input", { class: "sb-search", type: "search", placeholder: "Find the perfect sound", value: sb.query, "aria-label": "Find a sound" });
  search.addEventListener("input", () => {
    sb.query = search.value;
    const body = popover?.querySelector(".sb-body");
    body?.replaceWith(sbBody());
  });
  const volBtn = el("button", { class: `sb-vol-btn${sb.showVolume ? " on" : ""}${prefs.get("soundboard_muted", false) ? " muted" : ""}`, title: "Soundboard volume", "aria-label": "Soundboard volume", onclick: () => { sb.showVolume = !sb.showVolume; sbRedraw(); } });
  volBtn.innerHTML = prefs.get("soundboard_muted", false) ? VOICE_ICONS.speakerOff : SB_ICONS.speaker;
  const vol = el("input", { type: "range", min: 0, max: 100, step: 5, value: Math.round(prefs.get("soundboard_volume", 0.7) * 100), "aria-label": "Soundboard volume" });
  const volValue = el("span", { class: "vol-value" }, `${vol.value}%`);
  vol.addEventListener("input", () => { prefs.set("soundboard_volume", vol.value / 100); volValue.textContent = `${vol.value}%`; });
  const mute = el("input", { type: "checkbox", checked: prefs.get("soundboard_muted", false) });
  mute.addEventListener("change", () => { prefs.set("soundboard_muted", mute.checked); sbRedraw(); });
  const canManage = can("manage_sounds");
  const inVoice = !!state.voice.room;
  const board = el("div", { class: `sb-board${Date.now() < sb.coolUntil ? " cooling" : ""}`, role: "dialog", "aria-label": "Soundboard" },
    el("div", { class: "sb-top" },
      el("label", { class: "sb-search-wrap" }, el("span", { class: "sb-search-icon" }), search),
      volBtn),
    sb.showVolume ? el("div", { class: "sb-volume" },
      el("span", {}, "Volume"), vol, volValue,
      el("label", { class: "check-row" }, mute, "Mute")) : null,
    inVoice ? null : el("p", { class: "muted small sb-note" }, "Join a voice channel to play sounds. ▶ previews just for you."),
    sbBody(),
    el("div", { class: "sb-foot" },
      el("span", { class: "muted small" }, "Click to play · ▶ preview · ☆ favourite"),
      canManage ? el("button", { class: "sb-manage-link", onclick: () => { closePopover(); openManageSounds(); } }, "Manage sounds") : null));
  board.querySelector(".sb-search-icon").innerHTML = SB_ICONS.search;
  return board;
}

function sbRedraw() {
  if (!popover?.querySelector(".sb-board")) return;
  const scroll = popover.querySelector(".sb-list")?.scrollTop ?? 0;
  const focusedId = document.activeElement?.closest?.(".sb-tile")?.dataset.id;
  const searchFocused = document.activeElement?.classList?.contains("sb-search");
  popover.replaceChildren(sbDraw());
  const list = popover.querySelector(".sb-list");
  if (list) list.scrollTop = scroll;
  if (searchFocused) popover.querySelector(".sb-search")?.focus();
  else if (focusedId) popover.querySelector(`.sb-tile[data-id="${CSS.escape(focusedId)}"] .sb-play`)?.focus();
}

document.addEventListener("keydown", (e) => {
  if (e.defaultPrevented || !popover?.querySelector(".sb-board")) return;
  const active = document.activeElement;
  if (popover.contains(active) || !active || active === document.body) sbKeys(e);
});

// Arrow keys move between tiles, Enter plays (it's a button), typing searches.
function sbKeys(e) {
  const search = popover?.querySelector(".sb-search");
  const tiles = [...(popover?.querySelectorAll(".sb-play") ?? [])];
  const cur = tiles.indexOf(document.activeElement);
  const arrows = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"];
  if (arrows.includes(e.key)) {
    if (!tiles.length) return;
    if (document.activeElement === search) {
      if (e.key !== "ArrowDown") return;
      e.preventDefault();
      tiles[0].focus();
      return;
    }
    e.preventDefault();
    if (cur < 0) return tiles[0].focus();
    if (e.key === "ArrowLeft") return tiles[Math.max(0, cur - 1)].focus();
    if (e.key === "ArrowRight") return tiles[Math.min(tiles.length - 1, cur + 1)].focus();
    // Up/down: the nearest tile in the row above/below.
    const r = tiles[cur].getBoundingClientRect();
    const cx = r.left + r.width / 2;
    const down = e.key === "ArrowDown";
    let best = null;
    let bestScore = Infinity;
    for (const t of tiles) {
      const tr = t.getBoundingClientRect();
      const dy = down ? tr.top - r.top : r.top - tr.top;
      if (dy < 4) continue;
      const score = dy * 4 + Math.abs(tr.left + tr.width / 2 - cx);
      if (score < bestScore) { best = t; bestScore = score; }
    }
    if (best) best.focus();
    else if (!down) search?.focus();
    best?.scrollIntoView({ block: "nearest" });
    return;
  }
  // Start typing anywhere in the popover to search.
  if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey && document.activeElement !== search
      && !document.activeElement?.matches?.("input, textarea")) {
    e.preventDefault();
    search.focus();
    search.value += e.key;
    search.dispatchEvent(new Event("input"));
  }
}

async function openSoundboard(anchor) {
  await sbLoad();
  sb.query = "";
  sb.showVolume = false;
  const pop = openPopover(anchor, sbDraw());
  pop.classList.add("sb-pop");
  // Sit above the whole voice panel, lined up with its left edge (like Discord).
  const panel = document.getElementById("voice-panel");
  if (panel && !panel.hidden && window.innerWidth > 760) {
    const pr = panel.getBoundingClientRect();
    const r = pop.getBoundingClientRect();
    pop.style.left = `${Math.max(8, Math.min(pr.left, window.innerWidth - r.width - 8))}px`;
    pop.style.top = `${Math.max(8, pr.top - r.height - 8)}px`;
  }
  if (!matchMedia("(pointer: coarse)").matches) pop.querySelector(".sb-search")?.focus();
}

// ---------- Manage sounds (upload, rename, delete, hide built-ins) ----------

async function openManageSounds() {
  const gid = state.guildId;
  const dialog = el("dialog", { class: "sb-manage" });
  const draw = async () => {
    const sounds = await api("GET", `/api/guilds/${gid}/sounds`).catch(() => []);
    const classics = sounds.filter((s) => s.builtin);
    const mine = sounds.filter((s) => !s.builtin);
    const row = (snd) => {
      const name = el("input", { value: snd.name, maxlength: 32, "aria-label": "Name" });
      const emoji = el("input", { value: snd.emoji, maxlength: 8, class: "sbm-emoji", "aria-label": "Emoji" });
      return el("div", { class: "sbm-row" },
        emoji, name,
        el("button", { type: "button", class: "ghost", title: "Preview", onclick: () => playSound(snd.url, snd.volume, true) }, "▶"),
        el("button", { type: "button", class: "ghost", onclick: async () => {
          const ok = await tryApi("PATCH", `/api/sounds/${snd.id}`, { name: name.value, emoji: emoji.value }).catch(() => null);
          if (ok) { toast("Sound saved."); draw(); }
        } }, "Save"),
        el("button", { type: "button", class: "ghost danger", onclick: async () => {
          if (!(await confirmBox(`Delete ${snd.name}?`, "It's gone for everyone in this outhouse.", "Delete"))) return;
          const ok = await tryApi("DELETE", `/api/sounds/${snd.id}`).then(() => true).catch(() => false);
          if (ok) draw();
        } }, "Delete"));
    };
    dialog.replaceChildren(
      el("div", { class: "sbm-head" },
        el("h2", {}, "Manage sounds"),
        el("button", { type: "button", class: "icon-btn", title: "Close", "aria-label": "Close", onclick: () => dialog.close() }, "✕")),
      el("h3", {}, "Outhouse classics"),
      el("p", { class: "muted small" }, "Built into every outhouse. You can't delete them, but you can hide any you'd rather not hear."),
      el("div", { class: "sbm-classics" }, classics.map((snd) =>
        el("div", { class: `sbm-row classic${snd.hidden ? " is-hidden" : ""}`, "data-id": snd.id },
          el("span", { class: "sbm-emoji-show" }, snd.emoji), el("span", { class: "sbm-name" }, snd.name),
          el("button", { type: "button", class: "ghost", title: "Preview", onclick: () => playSound(snd.url, snd.volume, true) }, "▶"),
          el("button", { type: "button", class: `ghost sbm-toggle${snd.hidden ? " off" : ""}`, onclick: async () => {
            const key = snd.id.replace(/^builtin:/, "");
            const ok = await tryApi(snd.hidden ? "DELETE" : "PUT", `/api/guilds/${gid}/hidden-sounds/${key}`).then(() => true).catch(() => false);
            if (ok) draw();
          } }, snd.hidden ? "Hidden · Show" : "Shown · Hide")))),
      el("div", { class: "sbm-sub" },
        el("h3", {}, `Uploaded sounds (${mine.length})`),
        el("button", { type: "button", class: "primary sbm-add", onclick: () => addSoundDialog(gid, draw) }, "＋ Add sound")),
      mine.length ? el("div", { class: "sbm-list" }, mine.map(row)) : el("p", { class: "muted" }, "No uploaded sounds yet."),
    );
  };
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  await draw();
  dialog.showModal();
}

function addSoundDialog(gid, after) {
  const name = el("input", { placeholder: "Leeroy", maxlength: 32 });
  const emoji = el("input", { placeholder: "🐔", maxlength: 8, style: "width:80px" });
  const volume = el("input", { type: "range", min: 10, max: 100, value: 100 });
  const file = el("input", { type: "file", accept: ".mp3,.ogg,.oga,.opus,.wav,audio/*" });
  const dialog = el("dialog", {},
    el("form", { method: "dialog" },
      el("h2", {}, "Add a sound"),
      el("p", { class: "muted", style: "margin:0" }, "MP3, OGG or WAV, up to 10 seconds and 1 MB. Short clips work best."),
      el("label", { class: "field" }, "Name", name),
      el("label", { class: "field" }, "Emoji", emoji),
      el("label", { class: "field" }, "Volume", volume),
      file,
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"),
        el("button", { type: "submit", class: "primary", value: "ok" }, "Upload")),
    ));
  dialog.addEventListener("close", async () => {
    dialog.remove();
    if (dialog.returnValue !== "ok") return;
    if (!file.files[0]) return toast("Choose a sound file.");
    const form = new FormData();
    form.append("name", name.value.trim() || file.files[0].name.replace(/\.[^.]+$/, ""));
    form.append("emoji", emoji.value.trim());
    form.append("volume", String(volume.value / 100));
    form.append("file", file.files[0], file.files[0].name);
    const ok = await tryApi("POST", `/api/guilds/${gid}/sounds`, form).catch(() => null);
    if (ok) {
      toast(`${ok.name} added to the soundboard.`);
      await loadSounds();
      after?.();
    }
  });
  document.body.append(dialog);
  dialog.showModal();
}
