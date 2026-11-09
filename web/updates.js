// ======================================================================
// The orange update button (top right of the chat header)
// ======================================================================
//
// Hidden until there's something newer, then an orange download arrow with a
// soft glow (like Discord's green one). Three kinds of "newer":
//   - Windows app: the app checks every hour and remembers what it found; we
//     ask its bridge (http://outhouse.localhost/update-status) every minute,
//     and "Update now" (/update-install) downloads, installs and restarts.
//   - Android app: /updates/android.json is newer than this app; the button
//     opens the APK download (Android apps can't update themselves).
//   - Everywhere: the server has a newer web client (/api/health's
//     web_build changed since this page loaded): "Refresh to update".
// An app update wins over a web refresh when both are out.

const updates = (() => {
  const platform = window.__OUTHOUSE_PLATFORM__ || (window.__OUTHOUSE_DESKTOP__ ? "desktop" : "browser");
  const winApp = !!window.__OUTHOUSE_DESKTOP__ && platform !== "android";
  const droidApp = !!window.__OUTHOUSE_DESKTOP__ && platform === "android";
  const LATER_MS = 6 * 60 * 60 * 1000;
  const WEB_EVERY = 10 * 60 * 1000;
  const APP_EVERY = 60 * 1000;
  const APK_EVERY = 6 * 60 * 60 * 1000;

  const s = {
    app: null, // Windows: the bridge's update-status
    bridgeGone: false, // an older app without the update endpoints
    apk: null, // Android: { version, url }
    webBuild: null, // the web client this page loaded with
    webNew: null, // a newer web client's fingerprint
    installing: false, // "Update now" / "Refresh" clicked
    lastWeb: 0,
    lastApk: 0,
  };

  const ICON = '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 3.5v11.5M6.5 10l5.5 5.5 5.5-5.5M5 20.5h14"/></svg>';

  const btn = el("button", { type: "button", class: "update-btn", id: "update-btn", hidden: true, onclick: () => togglePop() });
  btn.innerHTML = ICON;
  const conn = $("conn-status");
  if (conn) conn.before(btn);
  else document.querySelector(".chat-head")?.append(btn);

  // Is x.y.z newer than a.b.c? ("0.2.10" > "0.2.9")
  function newer(latest, current) {
    const a = String(latest).replace(/^v/, "").split(".").map(Number);
    const b = String(current).replace(/^v/, "").split(".").map(Number);
    if (a.some(isNaN) || b.some(isNaN)) return String(latest) !== String(current);
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      if ((a[i] ?? 0) !== (b[i] ?? 0)) return (a[i] ?? 0) > (b[i] ?? 0);
    }
    return false;
  }

  // What the button offers right now, or null.
  function offer() {
    const app = s.app;
    if (app && (app.available || app.downloading || app.ready)) {
      return { kind: "app", key: `app:${app.version}`, version: app.version, notes: app.notes, date: app.date,
        downloading: !!app.downloading, progress: app.progress, ready: !!app.ready, error: app.error };
    }
    if (s.apk) return { kind: "apk", key: `apk:${s.apk.version}`, version: s.apk.version, notes: s.apk.notes, url: s.apk.url };
    if (s.webNew) return { kind: "web", key: `web:${s.webNew}` };
    return null;
  }

  function snoozed(o) {
    const later = prefs.get("update_later", null);
    return !!later && later.key === o.key && Date.now() < later.until;
  }

  function tip(o) {
    if (o.kind === "app") return `Update available — Outhouse ${o.version}. Click to restart and update`;
    if (o.kind === "apk") return `Update available — Outhouse ${o.version}. Click to download it`;
    return "Update available — Outhouse has been updated. Click to refresh";
  }

  function draw() {
    const o = offer();
    const busy = o && (s.installing || o.downloading || o.ready);
    const show = !!o && (busy || !snoozed(o));
    btn.hidden = !show;
    document.documentElement.classList.toggle("has-update", show);
    if (show) {
      btn.title = busy ? "Updating Outhouse…" : tip(o);
      btn.setAttribute("aria-label", btn.title);
      btn.classList.toggle("busy", !!busy);
      btn.dataset.kind = o.kind;
    }
    if (pop?.isConnected) {
      if (show) pop.replaceChildren(popBody(o));
      else closePopover();
    }
  }

  // ---------- the popover ----------
  let pop = null;
  function togglePop() {
    if (pop?.isConnected) { closePopover(); pop = null; return; }
    const o = offer();
    if (!o) return;
    pop = openPopover(btn, popBody(o), "below");
    pop.classList.add("upd-pop");
    pop.setAttribute("role", "dialog");
    pop.setAttribute("aria-label", "Update Outhouse");
  }

  function niceDate(d) {
    if (!d) return "";
    const t = new Date(String(d).slice(0, 10));
    return isNaN(t) ? "" : t.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
  }

  function popBody(o) {
    const inVoice = !!(state.voice?.room || state.voice?.channelId);
    const body = el("div", { class: "upd-body" });
    const icon = el("span", { class: "upd-icon", "aria-hidden": "true" });
    icon.innerHTML = ICON;
    const title = o.kind === "web" ? "Outhouse has been updated" : `Outhouse ${o.version}`;
    body.append(el("div", { class: "upd-head" }, icon,
      el("div", {},
        el("div", { class: "upd-title" }, "Update available"),
        el("div", { class: "upd-sub muted" }, title, o.date && niceDate(o.date) ? ` · ${niceDate(o.date)}` : ""))));
    if (o.kind === "web") body.append(el("p", { class: "upd-notes" }, "There's a newer version of Outhouse on the server. Refresh to get it."));
    else if (o.notes) body.append(el("p", { class: "upd-notes" }, o.notes));

    const restarting = o.ready || (o.kind === "web" && s.installing);
    if (o.downloading || restarting || (o.kind === "app" && s.installing)) {
      const pct = restarting ? 100 : (typeof o.progress === "number" ? Math.round(o.progress * 100) : null);
      const bar = el("div", { class: `upd-progress${pct === null ? " indeterminate" : ""}`, role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100" },
        el("div", { class: "upd-fill", style: pct === null ? "" : `width:${pct}%` }));
      if (pct !== null) bar.setAttribute("aria-valuenow", String(pct));
      body.append(bar, el("div", { class: "upd-status muted", "aria-live": "polite" },
        restarting ? "Restarting…" : pct === null ? "Downloading…" : `Downloading… ${pct}%`));
      return body;
    }

    if (o.error) body.append(el("p", { class: "upd-error" }, `The last try didn't work (${o.error}). Give it another go.`));
    const leavesVoice = inVoice && o.kind !== "apk";
    if (leavesVoice) body.append(el("p", { class: "upd-warn" }, "This will disconnect you from voice."));
    const label = o.kind === "apk" ? "Download"
      : o.kind === "web" ? (leavesVoice ? "Leave voice and refresh" : "Refresh to update")
      : (leavesVoice ? "Leave voice and update" : "Update now");
    body.append(el("div", { class: "upd-actions" },
      el("button", { type: "button", class: "ghost", id: "update-later", onclick: () => later(o) }, "Later"),
      el("button", { type: "button", class: "primary", id: "update-now", onclick: () => go(o) }, label)));
    return body;
  }

  function later(o) {
    prefs.set("update_later", { key: o.key, until: Date.now() + LATER_MS });
    closePopover();
    pop = null;
    draw();
  }

  async function go(o) {
    if (o.kind === "apk") {
      desktopInvoke("open_external", { url: new URL(o.url || "/updates/Outhouse-Android.apk", location.href).href });
      toast(`Downloading Outhouse ${o.version}. Open it when it's done to install.`);
      closePopover();
      pop = null;
      return;
    }
    if (state.voice?.room || state.voice?.channelId) {
      try { await leaveVoice(true); } catch { /* leaving anyway */ }
    }
    s.installing = true;
    draw();
    if (o.kind === "web") {
      setTimeout(() => location.reload(), 400);
      return;
    }
    const ok = await bridge("update-install");
    if (ok === null) {
      s.installing = false;
      toast("Couldn't start the update. Try again in a moment.");
    }
    pollApp();
  }

  // ---------- checks ----------
  // The Windows app's bridge (not desktopCall: an older app without these
  // endpoints shouldn't mark the desktop connection as broken).
  async function bridge(path) {
    try {
      const res = await fetch(`${BRIDGE}/${path}`, { cache: "no-store" });
      if (res.status === 404) { s.bridgeGone = true; return null; }
      if (!res.ok) return null;
      return await res.text();
    } catch { return null; }
  }

  let appTimer = null;
  async function pollApp() {
    if (!winApp || s.bridgeGone) return;
    clearTimeout(appTimer);
    const text = await bridge("update-status");
    if (text !== null) {
      try { s.app = JSON.parse(text); } catch { /* keep the last one */ }
      if (s.app && !s.app.downloading && !s.app.ready && s.installing && s.app.error) s.installing = false;
      draw();
    }
    if (s.bridgeGone) return;
    const fast = s.installing || s.app?.downloading || s.app?.ready;
    appTimer = setTimeout(pollApp, fast ? 500 : APP_EVERY);
  }

  async function checkApk() {
    if (!droidApp) return;
    const current = window.__OUTHOUSE_APP_VERSION__;
    if (!current) return;
    s.lastApk = Date.now();
    const latest = await fetch("/updates/android.json", { cache: "no-store" }).then((r) => (r.ok ? r.json() : null)).catch(() => null);
    if (latest?.version && newer(latest.version, current)) {
      s.apk = { version: String(latest.version), url: latest.url, notes: typeof latest.notes === "string" ? latest.notes.slice(0, 600) : null };
    } else if (latest) s.apk = null;
    draw();
  }

  async function checkWeb() {
    s.lastWeb = Date.now();
    const health = await fetch("/api/health", { cache: "no-store" }).then((r) => r.json()).catch(() => null);
    const build = health?.web_build;
    if (typeof build !== "string" || !build) return;
    if (!s.webBuild) s.webBuild = build;
    else s.webNew = build !== s.webBuild ? build : null;
    draw();
  }

  function checkAll() {
    return Promise.all([pollApp(), checkApk(), checkWeb()]);
  }

  // Coming back to the window: catch up if a check is due.
  function onReturn() {
    if (document.visibilityState === "hidden") return;
    if (Date.now() - s.lastWeb > WEB_EVERY) checkWeb();
    if (Date.now() - s.lastApk > APK_EVERY) checkApk();
    pollApp();
  }

  checkAll();
  setInterval(checkWeb, WEB_EVERY);
  if (droidApp) setInterval(checkApk, APK_EVERY);
  window.addEventListener("focus", onReturn);
  document.addEventListener("visibilitychange", onReturn);
  // The Windows app nudges us when its hourly (or tray) check finds something.
  window.addEventListener("outhouse-update", () => pollApp());

  return { check: checkAll, state: s, draw, newer };
})();

// For the tests (and the curious): check for updates right now.
window.checkForUpdates = () => updates.check();
