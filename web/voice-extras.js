// Voice extras: moderator tools (server mute/deafen, move, disconnect),
// "mute for me", entrance sounds and screen sharing. Loaded after app.js
// and features.js; app.js calls into these where noted.
"use strict";

// ---------- small icons for the voice list ----------

const VOICE_ICONS = {
  micOff: '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 2l20 20"/><path d="M9 9v3a3 3 0 0 0 5.12 2.12M15 9.34V5a3 3 0 0 0-5.94-.6"/><path d="M17 16.95A7 7 0 0 1 5 12v-2m14 0v2c0 .44-.04.87-.11 1.29"/><path d="M12 19v3"/></svg>',
  deafOff: '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 18v-6a9 9 0 0 1 14.5-7.1M21 12v6"/><path d="M21 19a2 2 0 0 1-2 2h-1v-6h3zM3 19a2 2 0 0 0 2 2h1v-6H3z"/><path d="M2 2l20 20"/></svg>',
  speakerOff: '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M11 5 6 9H2v6h4l5 4z"/><path d="M23 9l-6 6M17 9l6 6"/></svg>',
  screen: '<svg viewBox="0 0 24 24" width="17" height="17" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/></svg>',
};

function iconSpan(name, cls, title) {
  const span = el("span", { class: `vicon ${cls}`, title, "aria-label": title });
  span.innerHTML = VOICE_ICONS[name];
  return span;
}

// The icons after someone's name in the voice list (app.js renderChannels).
function voiceUserIcons(v) {
  const out = [];
  if (v.streaming) {
    out.push(el("button", {
      class: "live-badge",
      title: "Sharing their screen: click to watch",
      onclick: (e) => { e.stopPropagation(); watchScreen(v.user_id, v.channel_id); },
    }, "LIVE"));
  }
  if (v.user_id !== state.me.id && isLocallyMuted(v.user_id)) out.push(iconSpan("speakerOff", "local", "Muted for you"));
  if (v.server_deafened) out.push(iconSpan("deafOff", "server", "Deafened by a moderator"));
  else if (v.server_muted) out.push(iconSpan("micOff", "server", "Muted by a moderator"));
  if (!v.server_deafened && v.deafened) out.push(iconSpan("deafOff", "self", "Deafened"));
  else if (!v.server_muted && !v.server_deafened && v.muted) out.push(iconSpan("micOff", "self", "Muted"));
  return out;
}

// ---------- "Mute for me" ----------

function toggleLocalMute(m) {
  prefs.set(`lmute_${m.id}`, !isLocallyMuted(m.id));
  applyVolume(state.voice.room?.remoteParticipants.get(m.id));
  toast(isLocallyMuted(m.id) ? `${m.display_name} is muted for you.` : `You can hear ${m.display_name} again.`);
  renderChannels();
}

// ---------- moderator tools ----------

// Extra items for the right-click menu on someone in voice.
function voiceModerationItems(m) {
  const vs = state.voiceStates.get(m.id);
  if (!vs || m.id === state.me.id) return [];
  if (m.id === currentGuild()?.owner_id || myTop() <= memberTop(m)) return [];
  const here = vs.channel_id;
  const items = [];
  if (can("mute_members", here)) {
    items.push({ label: "Server mute", checked: !!vs.server_muted, onClick: () => moderateVoice(m, { mute: !vs.server_muted }, vs.server_muted ? `${m.display_name} can talk again.` : `${m.display_name} is server-muted.`) });
  }
  if (can("deafen_members", here)) {
    items.push({ label: "Server deafen", checked: !!vs.server_deafened, onClick: () => moderateVoice(m, { deafen: !vs.server_deafened }, vs.server_deafened ? `${m.display_name} can hear again.` : `${m.display_name} is server-deafened.`) });
  }
  if (can("move_members", here)) {
    const others = state.channels.filter((c) => c.kind === VOICE && c.id !== here);
    if (others.length) {
      items.push({
        label: "Move to",
        submenu: others.map((c) => ({
          label: `🔈 ${c.name}`,
          disabled: !can("move_members", c.id),
          onClick: () => moderateVoice(m, { channel_id: c.id }, `Moved ${m.display_name} to ${c.name}.`),
        })),
      });
    }
    items.push({ label: "Disconnect from voice", danger: true, onClick: async () => {
      const ok = await tryApi("DELETE", `/api/guilds/${state.guildId}/voice/${m.id}`).then(() => true).catch(() => false);
      if (ok) toast(`${m.display_name} was disconnected.`);
    } });
  }
  return items.length ? ["sep", ...items] : [];
}

async function moderateVoice(m, body, done) {
  const ok = await tryApi("PATCH", `/api/guilds/${state.guildId}/voice/${m.id}`, body).then(() => true).catch(() => false);
  if (ok && done) toast(done);
}

// Our own voice state changed on the server (a moderator muted us, say).
function voiceSelfUpdate(d) {
  const v = state.voice;
  if (!v.room || d.guild_id !== v.guildId || !d.channel_id) return;
  const muted = !!d.server_muted, deafened = !!d.server_deafened;
  if (muted === v.serverMuted && deafened === v.serverDeafened) return;
  const wasDeafened = v.serverDeafened;
  if (deafened !== wasDeafened) toast(deafened ? "A moderator deafened you." : "A moderator undeafened you.");
  else toast(muted ? "A moderator muted you." : "A moderator unmuted you. Your own mute button works again.");
  v.serverMuted = muted;
  v.serverDeafened = deafened;
  syncMic();
  applyAllVolumes();
  if (wasDeafened && !deafened) {
    // Hearing was switched off on the server too: ask for everyone's audio again.
    for (const p of v.room.remoteParticipants.values()) {
      for (const pub of p.trackPublications.values()) {
        try { pub.setSubscribed(true); } catch { /* gone */ }
      }
    }
  }
  renderVoicePanel();
}

// A moderator moved us to another voice channel.
async function onVoiceMove(d) {
  const v = state.voice;
  if (!v.room || v.channelId === d.channel_id) return;
  const name = (d.guild_id === state.guildId ? state.channels.find((c) => c.id === d.channel_id)?.name : null) ?? "another channel";
  await leaveVoice(false);
  toast(`A moderator moved you to ${name}.`);
  await joinVoice(d.channel_id, { guildId: d.guild_id, name: name === "another channel" ? "Voice" : name });
}

// A moderator disconnected us.
function onVoiceKicked(d) {
  if (state.voice.channelId !== d.channel_id) return;
  leaveVoice(false);
  toast("A moderator disconnected you from voice.");
}

// ---------- entrance sounds ----------

function onEntranceSound(d) {
  if (d.channel_id !== state.voice.channelId || d.user_id === state.me.id) return;
  state.voice.lastEntrance = { user_id: d.user_id, url: d.url, played: false };
  if (!prefs.get("entrance_sounds", true) || isDeafened() || isLocallyMuted(d.user_id)) return;
  playEntrance(d.url);
  state.voice.lastEntrance.played = true;
}

function playEntrance(url) {
  const audio = new Audio(url);
  audio.volume = Math.max(0, Math.min(1, prefs.get("entrance_volume", 0.6) * outputVolume()));
  const out = prefs.get("output_device", "");
  if (out && audio.setSinkId) audio.setSinkId(out).catch(() => {});
  audio.play().catch(() => {});
}

// Length of a sound file in seconds (0 if the browser can't tell).
function audioDuration(file) {
  return new Promise((resolve) => {
    const audio = new Audio();
    const url = URL.createObjectURL(file);
    const done = (secs) => { URL.revokeObjectURL(url); resolve(secs); };
    const timer = setTimeout(() => done(0), 3000);
    audio.preload = "metadata";
    audio.addEventListener("loadedmetadata", () => { clearTimeout(timer); done(Number.isFinite(audio.duration) ? audio.duration : 0); });
    audio.addEventListener("error", () => { clearTimeout(timer); done(0); });
    audio.src = url;
  });
}

// The "Entrance sound" part of User settings → Voice & audio.
async function entranceSoundSection() {
  const box = el("div", { class: "entrance-box" });
  const draw = async () => {
    const [mine, classics] = await Promise.all([
      api("GET", "/api/users/@me/entrance-sound").catch(() => null),
      api("GET", "/api/builtin-sounds").catch(() => []),
    ]);
    const file = el("input", { type: "file", accept: ".mp3,.ogg,.oga,.opus,.wav,audio/mpeg,audio/ogg,audio/wav", hidden: true, "aria-label": "Entrance sound file" });
    file.addEventListener("change", async () => {
      const f = file.files[0];
      if (!f) return;
      if (f.size > 200 * 1024) return toast("Entrance sounds can be up to 200 KB.");
      const secs = await audioDuration(f);
      if (secs > 5.25) return toast(`That sound is ${secs.toFixed(1)} seconds long; the limit is 5 seconds.`);
      const form = new FormData();
      form.append("file", f, f.name);
      const ok = await tryApi("POST", "/api/users/@me/entrance-sound", form).catch(() => null);
      if (ok) { toast("Entrance sound saved."); draw(); }
    });
    const on = el("input", { type: "checkbox", checked: prefs.get("entrance_sounds", true) });
    on.addEventListener("change", () => prefs.set("entrance_sounds", on.checked));
    const vol = el("input", { type: "range", min: 0, max: 100, step: 5, value: Math.round(prefs.get("entrance_volume", 0.6) * 100), "aria-label": "Entrance sound volume" });
    const volValue = el("span", { class: "vol-value" }, `${vol.value}%`);
    vol.addEventListener("input", () => { prefs.set("entrance_volume", vol.value / 100); volValue.textContent = `${vol.value}%`; });
    box.replaceChildren(
      el("h3", {}, "Entrance sound"),
      el("p", { class: "muted small", style: "margin:0" }, "Plays for everyone in a voice channel when you join it. MP3, OGG or WAV, up to 5 seconds and 200 KB."),
      el("div", { class: "inline-row entrance-row" },
        mine
          ? el("button", { class: "ghost", onclick: () => playEntrance(mine.url) }, `▶ Play ${mine.builtin ? `${classics.find((c) => c.id === `builtin:${mine.builtin}`)?.name ?? "classic"} ` : ""}(${(mine.duration_ms / 1000).toFixed(1)} s)`)
          : el("span", { class: "muted" }, "No entrance sound yet."),
        el("button", { class: "ghost", onclick: () => file.click() }, mine ? "Change…" : "Upload…"),
        mine ? el("button", { class: "ghost danger", onclick: async () => {
          const ok = await tryApi("DELETE", "/api/users/@me/entrance-sound").then(() => true).catch(() => false);
          if (ok) { toast("Entrance sound removed."); draw(); }
        } }, "Remove") : null,
        file),
      el("div", { class: "entrance-classics" },
        el("span", { class: "muted small" }, "Or pick an Outhouse classic:"),
        classics.filter((c) => (c.duration_ms ?? 0) <= 5000).map((c) => {
          const key = c.id.replace(/^builtin:/, "");
          const chosen = mine?.builtin === key;
          return el("button", {
            type: "button",
            class: chosen ? "on" : "",
            "data-key": key,
            "aria-pressed": String(chosen),
            title: chosen ? "Your entrance sound" : `Use ${c.name} as your entrance sound`,
            onclick: async () => {
              playEntrance(c.url);
              if (chosen) return;
              const ok = await tryApi("PUT", "/api/users/@me/entrance-sound", { builtin: key }).catch(() => null);
              if (ok) { toast(`${c.name} is your entrance sound.`); draw(); }
            },
          }, `${c.emoji} ${c.name}`);
        })),
      el("label", { class: "check-row" }, on, "Play other people's entrance sounds"),
      el("div", { class: "vol-row" }, el("span", {}, "Entrance sound volume"), vol, volValue),
    );
  };
  await draw();
  return box;
}

// ---------- screen sharing ----------

const screens = { tiles: new Map(), focus: null, hidden: false };

const screenShareSupported = () => !!navigator.mediaDevices?.getDisplayMedia
  && window.__OUTHOUSE_PLATFORM__ !== "android"
  && !(typeof isTouch === "function" && isTouch())
  && !/Android|iPhone|iPad|iPod/i.test(navigator.userAgent);

// The 🖥 button in the voice panel (null where screen sharing can't work).
function screenShareButton() {
  const v = state.voice;
  if (!v.room || !v.canStream || !screenShareSupported()) return null;
  const btn = el("button", {
    class: `vp-btn share${v.sharing ? " on" : ""}`,
    title: v.sharing ? "Stop sharing your screen" : "Share your screen",
    "aria-label": v.sharing ? "Stop sharing your screen" : "Share your screen",
    onclick: (e) => {
      if (state.voice.sharing) return stopScreenShare();
      openSharePicker(e.currentTarget);
    },
  });
  btn.innerHTML = VOICE_ICONS.screen;
  return btn;
}

// Screen share quality: resolution × frame rate, with bitrates to match.
const SHARE_RESOLUTIONS = {
  "480p": { width: 854, height: 480 },
  "720p": { width: 1280, height: 720 },
  "1080p": { width: 1920, height: 1080 },
};
// Max bitrate (bits/s) by resolution and frame rate.
const SHARE_BITRATES = {
  "480p": { 15: 800_000, 30: 1_200_000, 60: 1_800_000 },
  "720p": { 15: 1_500_000, 30: 2_500_000, 60: 3_500_000 },
  "1080p": { 15: 3_000_000, 30: 4_500_000, 60: 6_000_000 },
};
const SHARE_FPS = [15, 30, 60];

function shareSettings() {
  let saved = null;
  try { saved = JSON.parse(localStorage.getItem("outhouse_share_quality") || "null"); } catch { /* storage unavailable */ }
  const res = SHARE_RESOLUTIONS[saved?.res] ? saved.res : "1080p";
  const fps = SHARE_FPS.includes(saved?.fps) ? saved.fps : 15;
  return { res, fps, sound: !!saved?.sound };
}

function saveShareSettings(q) {
  try { localStorage.setItem("outhouse_share_quality", JSON.stringify(q)); } catch { /* storage unavailable */ }
}

const shareLabel = (q) => `${q.res} · ${q.fps} fps`;

// The picker the 🖥 button opens: quality, frame rate, sound, Share.
function openSharePicker(anchor) {
  const q = shareSettings();
  const box = el("div", { class: "share-picker" });
  const seg = (options, current, onPick, label) => el("div", { class: "share-seg", role: "radiogroup", "aria-label": label },
    options.map(([value, text]) => el("button", {
      type: "button", role: "radio", "aria-checked": String(value === current),
      class: value === current ? "on" : "", onclick: () => { onPick(value); draw(); },
    }, text)));
  const draw = () => {
    const sound = el("input", { type: "checkbox", checked: q.sound });
    sound.addEventListener("change", () => { q.sound = sound.checked; });
    box.replaceChildren(
      el("div", { class: "share-title" }, "Share your screen"),
      el("div", { class: "share-label" }, "Quality"),
      seg([["480p", "480p"], ["720p", "720p"], ["1080p", "1080p"]], q.res, (v) => { q.res = v; }, "Quality"),
      el("div", { class: "share-label" }, "Frame rate"),
      seg(SHARE_FPS.map((f) => [f, `${f} fps`]), q.fps, (v) => { q.fps = v; }, "Frame rate"),
      el("div", { class: "share-hint muted small" }, q.fps >= 30
        ? "Smooth motion: good for games and videos. Uses more upload."
        : "Sharp detail: good for text, code and spreadsheets."),
      el("label", { class: "check-row" }, sound, el("span", {}, "Share sound too")),
      el("button", { class: "primary share-go", onclick: () => {
        saveShareSettings(q);
        closePopover();
        startScreenShare(q.sound, { res: q.res, fps: q.fps });
      } }, "Share"));
  };
  draw();
  openPopover(anchor, box, "above");
}

async function startScreenShare(withSound, quality = shareSettings()) {
  const room = state.voice.room;
  if (!room) return;
  const res = SHARE_RESOLUTIONS[quality.res] ?? SHARE_RESOLUTIONS["1080p"];
  const fps = SHARE_FPS.includes(quality.fps) ? quality.fps : 15;
  const maxBitrate = SHARE_BITRATES[quality.res]?.[fps] ?? 3_000_000;
  state.voice.shareQuality = { res: quality.res, fps };
  try {
    await room.localParticipant.setScreenShareEnabled(true, {
      audio: withSound ? { echoCancellation: false, noiseSuppression: false, autoGainControl: false } : false,
      systemAudio: withSound ? "include" : "exclude",
      selfBrowserSurface: "exclude",
      surfaceSwitching: "include",
      resolution: { width: res.width, height: res.height, frameRate: fps },
      contentHint: fps >= 30 ? "motion" : "detail",
    }, {
      // Viewers read the quality from the track's name (shown on the tile).
      name: `screen ${quality.res} ${fps}fps`,
      screenShareEncoding: { maxBitrate, maxFramerate: fps },
      screenShareSimulcastLayers: [],
    });
  } catch (err) {
    // Cancelling the picker isn't worth a message.
    if (err?.name !== "NotAllowedError" && err?.name !== "AbortError") toast(`Couldn't share your screen: ${err.message}`);
  }
  onLocalTracksChanged();
}

// "1080p · 15 fps" for a share, from its track name ("screen 1080p 15fps").
function shareQualityOf(pub, local) {
  if (local && state.voice.shareQuality) return shareLabel(state.voice.shareQuality);
  const m = /(\d{3,4}p) (\d+)fps/.exec(pub?.trackName ?? "");
  return m ? `${m[1]} · ${m[2]} fps` : "";
}

async function stopScreenShare() {
  const room = state.voice.room;
  if (!room) return;
  try { await room.localParticipant.setScreenShareEnabled(false); } catch { /* already stopped */ }
  onLocalTracksChanged();
}

// Called by app.js when we publish/unpublish anything (the browser's own
// "Stop sharing" bar ends a share too).
function onLocalTracksChanged() {
  const room = state.voice.room;
  if (!room) return;
  const sharing = !!room.localParticipant.isScreenShareEnabled;
  if (sharing !== state.voice.sharing) {
    state.voice.sharing = sharing;
    pushVoiceState();
    renderVoicePanel();
  }
  renderScreens();
}

function screenTracks() {
  const room = state.voice.room;
  if (!room || !LK()) return [];
  const { Track } = LK();
  const out = [];
  const mine = room.localParticipant.getTrackPublication(Track.Source.ScreenShare);
  if (mine?.track) out.push({ id: room.localParticipant.identity, track: mine.track, local: true, quality: shareQualityOf(mine, true) });
  for (const p of room.remoteParticipants.values()) {
    const pub = p.getTrackPublication(Track.Source.ScreenShare);
    if (pub?.track) out.push({ id: p.identity, track: pub.track, local: false, quality: shareQualityOf(pub, false) });
  }
  return out;
}

// Draws the strip of screen shares above the chat (or the enlarged one).
function renderScreens() {
  const list = screenTracks();
  if (!list.length) return clearScreens();
  let stage = $("screen-stage");
  if (!stage) {
    stage = el("section", { id: "screen-stage", class: "screen-stage", "aria-label": "Screen shares" });
    $("messages").before(stage);
  }
  for (const [id, t] of screens.tiles) {
    if (!list.some((s) => s.id === id && s.track === t.track)) {
      try { t.track.detach(t.video); } catch { /* gone */ }
      t.node.remove();
      screens.tiles.delete(id);
    }
  }
  for (const s of list) {
    if (screens.tiles.has(s.id)) continue;
    const video = el("video", { autoplay: true, playsinline: true });
    video.muted = true; // sound comes through the voice mix
    s.track.attach(video);
    const name = s.local ? "You're sharing" : memberById(s.id)?.display_name ?? "Someone";
    const tools = el("div", { class: "st-tools" },
      el("button", { class: "st-btn", title: "Full screen", onclick: (e) => { e.stopPropagation(); node.requestFullscreen?.().catch(() => {}); } }, "⛶ Full screen"),
      el("button", { class: "st-btn", title: "Back to the chat", onclick: (e) => { e.stopPropagation(); focusScreen(null); } }, "✕"));
    const node = el("div", {
      class: `screen-tile${s.local ? " mine" : ""}`,
      "data-id": s.id,
      tabindex: 0,
      role: "button",
      title: "Click to enlarge, double-click for full screen",
      onclick: () => focusScreen(s.id),
      ondblclick: () => { if (document.fullscreenElement) document.exitFullscreen().catch(() => {}); else node.requestFullscreen?.().catch(() => {}); },
    }, video, el("div", { class: "st-label" }, el("span", { class: "live-badge" }, "LIVE"), el("span", { class: "st-name" }, name), s.quality ? el("span", { class: "st-quality" }, s.quality) : null), tools);
    screens.tiles.set(s.id, { node, video, track: s.track });
    stage.append(node);
  }
  if (screens.focus && !screens.tiles.has(screens.focus)) screens.focus = null;
  let hide = stage.querySelector(".st-hide");
  if (!hide) {
    hide = el("button", { class: "st-hide", title: "Hide screen shares (click LIVE in the voice list to watch again)", onclick: () => { screens.hidden = true; renderScreens(); } }, "Hide");
    stage.append(hide);
  } else {
    stage.append(hide); // keep it last
  }
  stage.classList.toggle("focused", !!screens.focus);
  stage.classList.toggle("solo", screens.tiles.size === 1);
  stage.hidden = screens.hidden && !screens.focus;
  for (const [id, t] of screens.tiles) t.node.classList.toggle("big", id === screens.focus);
}

function focusScreen(id) {
  if (screens.focus === id) return;
  screens.focus = id;
  if (id) screens.hidden = false;
  renderScreens();
}

function clearScreens() {
  for (const t of screens.tiles.values()) {
    try { t.track.detach(t.video); } catch { /* gone */ }
  }
  screens.tiles.clear();
  screens.focus = null;
  screens.hidden = false;
  $("screen-stage")?.remove();
}

// Clicking LIVE in the voice list.
function watchScreen(userId, channelId) {
  if (state.voice.channelId !== channelId) {
    const ch = state.channels.find((c) => c.id === channelId);
    return toast(`Join ${ch?.name ?? "their voice channel"} to watch.`);
  }
  if (!screens.tiles.has(userId)) return toast("Their screen is still loading…");
  $("app").classList.remove("show-nav");
  focusScreen(userId);
}

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && screens.focus && !document.fullscreenElement) focusScreen(null);
});

// ---------- long-press on someone in voice (touch screens) ----------

(function voiceLongPress() {
  let timer = null;
  document.addEventListener("pointerdown", (e) => {
    if (e.pointerType !== "touch") return;
    const row = e.target.closest?.("#voice-channels .voice-user");
    if (!row) return;
    clearTimeout(timer);
    const { clientX, clientY } = e;
    timer = setTimeout(() => {
      timer = null;
      if (row.isConnected) MENUS.member({ clientX, clientY, preventDefault() {} }, row);
    }, 550);
  });
  const cancel = () => { clearTimeout(timer); timer = null; };
  document.addEventListener("pointerup", cancel);
  document.addEventListener("pointercancel", cancel);
  document.addEventListener("pointermove", (e) => { if (timer && (Math.abs(e.movementX) > 6 || Math.abs(e.movementY) > 6)) cancel(); });
})();
