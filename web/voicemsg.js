// Outhouse web client: voice messages.
// Loaded after app.js, features.js and threads.js (uses their globals).
//
// - Desktop: click the mic in the composer to start, then ➤ to send or 🗑
//   to bin it. Phones: press and hold the mic, let go to send, slide left
//   to cancel. Up to 5 minutes.
// - Recorded with MediaRecorder (Opus in WebM on Chrome, Edge/WebView2 and
//   Android; Ogg on Firefox; AAC in MP4 on Safari) from its own microphone
//   stream, so the voice channel's mic, noise gate and push to talk are
//   never touched.
// - The loudness is sampled while recording and sent as a little waveform.
"use strict";

const VM_MAX_MS = 5 * 60 * 1000;
const VM_BARS = 48;
const VM_SPEEDS = [1, 1.5, 2];

function vmFormat(ms) {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

// Recording format: the best this browser's MediaRecorder offers.
function vmMime() {
  if (typeof MediaRecorder === "undefined") return null;
  const options = [
    ["audio/webm;codecs=opus", "webm"],
    ["audio/ogg;codecs=opus", "ogg"],
    ["audio/webm", "webm"],
    ["audio/mp4;codecs=mp4a.40.2", "m4a"],
    ["audio/mp4", "m4a"],
  ];
  for (const [type, ext] of options) {
    try { if (MediaRecorder.isTypeSupported(type)) return { type, ext }; } catch { /* keep looking */ }
  }
  return { type: "", ext: "webm" }; // let the browser pick
}

// Base64 waveform → numbers 0–1, stretched or squeezed to `n` bars.
function vmBars(waveform, n = VM_BARS) {
  let raw = [];
  try { raw = waveform ? [...atob(waveform)].map((c) => c.charCodeAt(0) / 255) : []; } catch { raw = []; }
  if (!raw.length) return Array.from({ length: n }, (_, i) => 0.25 + 0.15 * Math.sin(i / 2));
  return Array.from({ length: n }, (_, i) => raw[Math.min(raw.length - 1, Math.floor((i * raw.length) / n))]);
}

// Loudness samples → up to 64 bytes, loudest = 255, as base64.
function vmWaveform(levels) {
  if (!levels.length) return null;
  const n = Math.min(64, levels.length);
  const out = [];
  for (let i = 0; i < n; i++) {
    const from = Math.floor((i * levels.length) / n);
    const to = Math.max(from + 1, Math.floor(((i + 1) * levels.length) / n));
    let peak = 0;
    for (let j = from; j < to; j++) peak = Math.max(peak, levels[j]);
    out.push(peak);
  }
  const max = Math.max(...out) || 1;
  // A gentle curve so quiet speech still shows as more than a flat line.
  return btoa(String.fromCharCode(...out.map((v) => Math.max(8, Math.round(Math.pow(v / max, 0.6) * 255)))));
}

// ---------- the player ----------

let vmPlaying = null; // the <audio> playing now (one at a time)

function voicePlayer(a) {
  const total = a.duration_ms || 0;
  const bars = vmBars(a.waveform);
  const audio = new Audio();
  audio.preload = "none";
  let speed = 0;
  const play = el("button", { type: "button", class: "vm-play", "aria-label": "Play voice message", title: "Play" });
  const wave = el("div", { class: "vm-wave", role: "slider", "aria-label": "Seek", "aria-valuemin": "0", "aria-valuemax": String(Math.round(total / 1000)), "aria-valuenow": "0", tabindex: "0" },
    bars.map((v) => el("span", { style: `height:${Math.round(12 + v * 88)}%` })));
  const time = el("span", { class: "vm-time" }, vmFormat(total));
  const rate = el("button", { type: "button", class: "vm-rate", title: "Playback speed", "aria-label": "Playback speed 1×" }, "1×");
  const node = el("div", { class: "voice-msg", "data-url": a.url }, play, wave, time, rate);

  const length = () => (Number.isFinite(audio.duration) && audio.duration > 0 ? audio.duration * 1000 : total);
  const paint = () => {
    const pos = audio.currentTime * 1000;
    const frac = length() ? Math.min(1, pos / length()) : 0;
    const lit = Math.round(frac * bars.length);
    [...wave.children].forEach((b, i) => b.classList.toggle("on", i < lit));
    time.textContent = audio.paused && !pos ? vmFormat(total) : vmFormat(pos);
    wave.setAttribute("aria-valuenow", String(Math.round(pos / 1000)));
    node.classList.toggle("playing", !audio.paused);
    play.title = audio.paused ? "Play" : "Pause";
    play.setAttribute("aria-label", audio.paused ? "Play voice message" : "Pause voice message");
  };
  const ensureSrc = () => { if (!audio.src) { audio.src = a.url; audio.load(); } };
  const toggle = async () => {
    ensureSrc();
    if (audio.paused) {
      if (vmPlaying && vmPlaying !== audio) vmPlaying.pause();
      vmPlaying = audio;
      audio.playbackRate = VM_SPEEDS[speed];
      await audio.play().catch((err) => toast(`Couldn't play that: ${err.message}`));
    } else {
      audio.pause();
    }
    paint();
  };
  const seekTo = (frac) => {
    ensureSrc();
    const target = (Math.max(0, Math.min(1, frac)) * length()) / 1000;
    try { audio.currentTime = target; } catch { /* not seekable yet */ }
    paint();
  };
  play.addEventListener("click", toggle);
  wave.addEventListener("click", (e) => {
    const r = wave.getBoundingClientRect();
    seekTo((e.clientX - r.left) / r.width);
  });
  wave.addEventListener("keydown", (e) => {
    const step = 5000 / (length() || 1);
    const now = (audio.currentTime * 1000) / (length() || 1);
    if (e.key === "ArrowRight") { e.preventDefault(); seekTo(now + step); }
    if (e.key === "ArrowLeft") { e.preventDefault(); seekTo(now - step); }
    if (e.key === " " || e.key === "Enter") { e.preventDefault(); toggle(); }
  });
  rate.addEventListener("click", () => {
    speed = (speed + 1) % VM_SPEEDS.length;
    audio.playbackRate = VM_SPEEDS[speed];
    rate.textContent = `${VM_SPEEDS[speed]}×`;
    rate.setAttribute("aria-label", `Playback speed ${VM_SPEEDS[speed]}×`);
    rate.classList.toggle("fast", speed > 0);
  });
  audio.addEventListener("timeupdate", paint);
  audio.addEventListener("pause", paint);
  audio.addEventListener("play", paint);
  audio.addEventListener("ended", () => { audio.currentTime = 0; paint(); });
  return node;
}

// Voice messages show as players; everything else as before.
const vmBaseRenderAttachments = renderAttachments;
renderAttachments = function (m) {
  const voice = (m.attachments || []).filter((a) => a.voice);
  if (!voice.length) return vmBaseRenderAttachments(m);
  const rest = vmBaseRenderAttachments({ ...m, attachments: m.attachments.filter((a) => !a.voice) });
  return el("div", { class: "voice-msgs" }, voice.map(voicePlayer), rest);
};

// ---------- recording ----------

const vmRec = {
  active: false,
  sending: false,
  ctx: null,       // { channelId, threadId, replyId, form, onSent }
  stream: null,
  recorder: null,
  chunks: [],
  mime: null,
  started: 0,
  levels: [],
  audioCtx: null,
  analyser: null,
  timer: null,
  bar: null,
  cancelArmed: false,
};

function vmCleanup() {
  clearInterval(vmRec.timer);
  vmRec.timer = null;
  vmRec.stream?.getTracks().forEach((t) => t.stop());
  vmRec.stream = null;
  vmRec.audioCtx?.close().catch(() => {});
  vmRec.audioCtx = null;
  vmRec.analyser = null;
  vmRec.recorder = null;
  vmRec.bar?.remove();
  vmRec.bar = null;
  vmRec.ctx?.form?.classList.remove("vm-recording");
  vmRec.active = false;
  vmRec.cancelArmed = false;
  document.querySelectorAll(".vm-mic.on").forEach((b) => b.classList.remove("on"));
}

function vmRecBar(holdMode) {
  const dot = el("span", { class: "vm-dot", "aria-hidden": "true" });
  const clock = el("span", { class: "vm-clock" }, "0:00");
  const live = el("div", { class: "vm-live", "aria-hidden": "true" }, Array.from({ length: 28 }, () => el("span")));
  const hint = el("span", { class: "vm-hint" }, holdMode ? "◀ Slide to cancel" : "");
  const bar = el("div", { class: `vm-recbar${holdMode ? " hold" : ""}`, role: "status", "aria-live": "polite" },
    holdMode ? null : el("button", { type: "button", class: "vm-cancel", title: "Cancel", "aria-label": "Cancel recording", onclick: () => vmStop(false) }, "🗑"),
    dot, clock, live, hint,
    holdMode ? null : el("button", { type: "button", class: "vm-send", title: "Send voice message", "aria-label": "Send voice message", onclick: () => vmStop(true) }, "➤"));
  bar.update = () => {
    const ms = Date.now() - vmRec.started;
    clock.textContent = vmFormat(ms);
    const recent = vmRec.levels.slice(-live.children.length);
    const max = Math.max(0.02, ...vmRec.levels.slice(-200));
    [...live.children].forEach((s, i) => {
      const v = recent[i - (live.children.length - recent.length)] ?? 0;
      s.style.height = `${Math.round(10 + Math.min(1, v / max) * 90)}%`;
    });
    if (VM_MAX_MS - ms < 15000) clock.classList.add("ending");
  };
  bar.setCancel = (on) => { bar.classList.toggle("cancel", on); hint.textContent = on ? "Let go to cancel" : "◀ Slide to cancel"; };
  return bar;
}

async function vmStart(ctx, holdMode = false) {
  if (vmRec.active || vmRec.sending) return false;
  if (!can("send_messages", ctx.channelId) || !can("attach_files", ctx.channelId)) {
    toast("You can't send voice messages here.");
    return false;
  }
  const mime = vmMime();
  if (!mime || !navigator.mediaDevices?.getUserMedia) {
    toast("This browser can't record voice messages.");
    return false;
  }
  vmRec.active = true;
  vmRec.ctx = ctx;
  let stream;
  try {
    // Its own stream (same microphone you picked for voice), so a voice
    // channel's mic and push to talk carry on untouched.
    stream = await navigator.mediaDevices.getUserMedia({ audio: micOptions() })
      .catch((err) => (err?.name === "OverconstrainedError" ? navigator.mediaDevices.getUserMedia({ audio: true }) : Promise.reject(err)));
  } catch (err) {
    vmRec.active = false;
    toast(err?.name === "NotAllowedError" ? "Outhouse isn't allowed to use your microphone. Allow it in your browser's site settings." : `Couldn't start the microphone: ${err?.message || err}`);
    return false;
  }
  if (!vmRec.active) { stream.getTracks().forEach((t) => t.stop()); return false; } // cancelled meanwhile
  vmRec.stream = stream;
  vmRec.mime = mime;
  vmRec.chunks = [];
  vmRec.levels = [];
  try {
    vmRec.recorder = new MediaRecorder(stream, { ...(mime.type ? { mimeType: mime.type } : {}), audioBitsPerSecond: 48000 });
  } catch {
    vmRec.recorder = new MediaRecorder(stream);
  }
  vmRec.recorder.addEventListener("dataavailable", (e) => { if (e.data?.size) vmRec.chunks.push(e.data); });
  try {
    const AC = window.AudioContext || window.webkitAudioContext;
    vmRec.audioCtx = new AC();
    vmRec.analyser = vmRec.audioCtx.createAnalyser();
    vmRec.analyser.fftSize = 1024;
    vmRec.audioCtx.createMediaStreamSource(stream).connect(vmRec.analyser);
  } catch { vmRec.analyser = null; }
  const buf = new Float32Array(1024);
  vmRec.recorder.start(250);
  vmRec.started = Date.now();
  vmRec.bar = vmRecBar(holdMode);
  ctx.form.classList.add("vm-recording");
  ctx.form.append(vmRec.bar);
  ctx.mic?.classList.add("on");
  vmRec.timer = setInterval(() => {
    if (vmRec.analyser) {
      vmRec.analyser.getFloatTimeDomainData(buf);
      let sum = 0;
      for (const v of buf) sum += v * v;
      vmRec.levels.push(Math.sqrt(sum / buf.length));
    } else {
      vmRec.levels.push(0.2);
    }
    vmRec.bar?.update();
    if (Date.now() - vmRec.started >= VM_MAX_MS) {
      toast("That's the 5-minute limit. Sending it.");
      vmStop(true);
    }
  }, 60);
  return true;
}

async function vmStop(send) {
  if (!vmRec.active || !vmRec.recorder) { vmCleanup(); return; }
  const rec = vmRec.recorder;
  const ctx = vmRec.ctx;
  const elapsed = Date.now() - vmRec.started;
  const levels = vmRec.levels.slice();
  const mime = vmRec.mime;
  const stopped = new Promise((resolve) => rec.addEventListener("stop", resolve, { once: true }));
  try { rec.stop(); } catch { /* already stopped */ }
  await Promise.race([stopped, new Promise((r) => setTimeout(r, 1500))]);
  const chunks = vmRec.chunks;
  vmCleanup();
  if (!send) return;
  if (elapsed < 700 || !chunks.length) return toast(isTouch() ? "Hold the mic to record a voice message." : "That was too short to send.");
  const type = (rec.mimeType || mime.type || "audio/webm").split(";")[0];
  const blob = new Blob(chunks, { type });
  if (blob.size > 10 * 1024 * 1024) return toast("That voice message is over 10 MB.");
  const ext = type.includes("ogg") ? "ogg" : type.includes("mp4") ? "m4a" : "webm";
  const form = new FormData();
  form.append("content", "");
  if (ctx.threadId) form.append("thread_id", ctx.threadId);
  if (ctx.replyId) form.append("reply_to_id", ctx.replyId);
  form.append("voice", JSON.stringify({ duration_ms: Math.min(elapsed, VM_MAX_MS), waveform: vmWaveform(levels) }));
  form.append("files", blob, `voice-message.${ext}`);
  vmRec.sending = true;
  try {
    const msg = await api("POST", `/api/channels/${ctx.channelId}/messages/upload`, form);
    ctx.onSent?.(msg);
  } catch (err) {
    toast(err.message);
  } finally {
    vmRec.sending = false;
  }
}

const MIC_SVG = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="9" y="2" width="6" height="12" rx="3"/><path d="M5 10a7 7 0 0 0 14 0M12 17v4M8 21h8"/></svg>';

// A mic button: click to start/stop (mouse), press and hold (touch).
function vmMicButton(getCtx) {
  const btn = el("button", { type: "button", class: "tool vm-mic", title: "Record a voice message", "aria-label": "Record a voice message" });
  btn.innerHTML = MIC_SVG;
  let hold = null; // { x, pointerId } while a finger is down
  btn.addEventListener("pointerdown", async (e) => {
    if (e.pointerType !== "touch") return;
    e.preventDefault();
    const ctx = getCtx();
    if (!ctx) return;
    hold = { x: e.clientX, id: e.pointerId, started: false };
    try { btn.setPointerCapture(e.pointerId); } catch { /* fine */ }
    const ok = await vmStart({ ...ctx, mic: btn }, true);
    if (!hold) { if (ok) vmStop(false); return; } // let go already
    hold.started = ok;
  });
  btn.addEventListener("pointermove", (e) => {
    if (!hold || e.pointerId !== hold.id || !vmRec.active) return;
    const armed = hold.x - e.clientX > 70;
    if (armed !== vmRec.cancelArmed) { vmRec.cancelArmed = armed; vmRec.bar?.setCancel(armed); }
  });
  const release = (e) => {
    if (!hold || e.pointerId !== hold.id) return;
    const started = hold.started;
    hold = null;
    if (started) vmStop(!vmRec.cancelArmed && e.type !== "pointercancel");
  };
  btn.addEventListener("pointerup", release);
  btn.addEventListener("pointercancel", release);
  btn.addEventListener("contextmenu", (e) => e.preventDefault());
  btn.addEventListener("click", (e) => {
    if (e.pointerType === "touch" || (e.detail === 0 && isTouch() && vmRec.active)) return;
    if (vmRec.active) return vmStop(true);
    const ctx = getCtx();
    if (ctx) vmStart({ ...ctx, mic: btn }, false);
  });
  return btn;
}

// The main composer.
(function composerMic() {
  const form = $("composer");
  const send = form?.querySelector(".send");
  if (!form || !send) return;
  const btn = vmMicButton(() => {
    if (!state.channelId || input.disabled) return null;
    return {
      channelId: state.channelId,
      replyId: state.replyTo?.id || null,
      form,
      onSent: (msg) => {
        if (state.replyTo) cancelReply();
        if (msg.channel_id === state.channelId && !msg.thread_id && !findMessage(msg.id)) {
          state.messages.push(msg);
          renderMessages.forceBottom = true;
          renderMessages();
        }
      },
    };
  });
  btn.id = "mic-btn";
  send.before(btn);
  // Phones: the mic stands in for Send until there's something to send.
  const syncMode = () => form.classList.toggle("has-text", !!input.value.trim() || state.pendingFiles.length > 0);
  input.addEventListener("input", syncMode);
  const vmBaseAutosize = autosize;
  autosize = function (...args) { vmBaseAutosize(...args); syncMode(); };
  const vmBaseRenderPending = renderPendingFiles;
  renderPendingFiles = function (...args) { vmBaseRenderPending(...args); syncMode(); };
  syncMode();
  const sync = () => {
    const ch = state.channels.find((c) => c.id === state.channelId);
    btn.hidden = !(ch && can("send_messages", ch.id) && can("attach_files", ch.id)) || !vmMime();
  };
  const vmBaseUpdateComposer = updateComposer;
  updateComposer = function (...args) {
    vmBaseUpdateComposer(...args);
    sync();
  };
  sync();
})();

// The thread side panel's composer.
const vmBaseThreadComposer = threadComposer;
threadComposer = function (...args) {
  const wrap = vmBaseThreadComposer(...args);
  const form = wrap.querySelector("form");
  const send = form?.querySelector(".send");
  const t = thread.info;
  if (form && send && t && can("send_messages", t.channel_id) && can("attach_files", t.channel_id) && vmMime()) {
    send.before(vmMicButton(() => (thread.info ? {
      channelId: thread.info.channel_id,
      threadId: thread.info.id,
      form,
      onSent: (msg) => {
        if (thread.info?.id === msg.thread_id && !thread.messages.some((x) => x.id === msg.id)) {
          thread.messages.push(msg);
          renderThread(true);
        }
      },
    } : null)));
  }
  return wrap;
};

// Leaving the channel (or the page) bins a recording in progress.
const vmBaseSelectChannel = selectChannel;
selectChannel = function (...args) {
  if (vmRec.active) vmStop(false);
  return vmBaseSelectChannel(...args);
};
window.addEventListener("pagehide", () => { if (vmRec.active) vmStop(false); });
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && vmRec.active) vmStop(false);
});
