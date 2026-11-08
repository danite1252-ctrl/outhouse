// The /invite/<code> page. The server fills in the card (and the link
// preview tags); this decides what the buttons do:
// - signed in: "Join <outhouse>" joins right here and opens it in /app
//   (or "Open <outhouse>" if you're already in);
// - signed out: "Accept invite" goes to sign up / sign in at /app, which
//   joins once you're in (the invite rides along as ?invite=<code>);
// - on Windows and Android: "Open in the Outhouse app" hands the invite to
//   the app (outhouse://invite/<code> on Windows, an Android App Link intent
//   on Android), with a "Download it" nudge if nothing picks it up.
// Inside the apps themselves only the join flow shows.
(() => {
  "use strict";
  const $ = (id) => document.getElementById(id);
  let data = {};
  try { data = JSON.parse($("invite-data").textContent); } catch { /* bad template */ }
  const inApp = !!(window.__OUTHOUSE_DESKTOP__ || window.__OUTHOUSE_PLATFORM__);
  const ua = navigator.userAgent;
  const device = /Android/i.test(ua) ? "android" : /Windows NT/i.test(ua) ? "windows" : "";
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  let token = null;
  try { token = localStorage.getItem("outhouse_token"); } catch { /* storage blocked */ }

  // A few embers drifting up behind the card, like the welcome page.
  const embers = $("embers");
  if (embers && !reduced) {
    const r = (a, b) => (a + Math.random() * (b - a)).toFixed(2);
    for (let i = 0; i < (innerWidth < 560 ? 10 : 16); i++) {
      const s = document.createElement("span");
      s.style.cssText = `--x:${r(8, 92)}%;--s:${r(2.5, 5)}px;--t:${r(7, 13)}s;--d:${r(0, 9)}s;--dx:${r(-60, 60)}px`;
      embers.appendChild(s);
    }
  }

  if (inApp) $("inv-home").href = "/app";
  if (!data.valid) {
    if (inApp) {
      $("inv-dead-home").href = "/app";
      $("inv-dead-home").textContent = "Back to Outhouse";
    }
    return;
  }

  const inv = data.invite;
  const code = inv.code;
  const guild = inv.guild;
  const primary = $("inv-primary");
  const who = $("inv-who");
  const showError = (text) => {
    $("inv-error").textContent = text;
    $("inv-error").hidden = !text;
  };

  // Who's inviting: their picture, or their first letter on a colour.
  const avatar = $("inv-avatar");
  if (inv.inviter) {
    if (inv.inviter.avatar_url && inv.inviter.avatar_url.startsWith("/")) {
      avatar.style.backgroundImage = `url(${JSON.stringify(inv.inviter.avatar_url)})`;
      avatar.textContent = " ";
    } else {
      const colours = ["#ff6b35", "#ffb347", "#fbbf24", "#f2917a", "#4ade80", "#a1a1aa"];
      let h = 0;
      for (const ch of String(inv.inviter.id)) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
      avatar.style.background = colours[h % colours.length];
      avatar.textContent = ([...(inv.inviter.name || "?").trim()][0] || "?").toUpperCase();
    }
  }

  const quips = [
    "The door's unlocked and the seat's still warm.",
    "BYO toilet paper. Everything else is sorted.",
    "Knock twice. Someone's always in.",
    "Best seat in the house. Well, out the back of it.",
    "Mind the redbacks under the seat.",
  ];
  $("inv-quip").textContent = quips[Math.floor(Math.random() * quips.length)];

  // ---------- join / accept ----------
  const signedOut = () => {
    primary.textContent = "Accept invite";
    primary.href = `/app?register&invite=${encodeURIComponent(code)}`;
    primary.onclick = null;
    $("inv-alt").hidden = false;
    who.hidden = true;
  };
  const openUrl = () => `/app?guild=${encodeURIComponent(guild.id)}`;
  const say = (before, name, after) => {
    who.replaceChildren(before, Object.assign(document.createElement("b"), { textContent: name }), after);
    who.hidden = false;
  };

  async function call(method, path) {
    const res = await fetch(path, { method, headers: { Authorization: `Bearer ${token}` }, cache: "no-store" });
    const body = res.status === 204 ? null : await res.json().catch(() => null);
    if (!res.ok) {
      const err = new Error((body && body.error) || `Something went wrong (${res.status}).`);
      err.status = res.status;
      throw err;
    }
    return body;
  }

  async function join(e) {
    e.preventDefault();
    showError("");
    primary.classList.add("busy");
    primary.textContent = "Joining…";
    try {
      const g = await call("POST", `/api/invites/${encodeURIComponent(code)}`);
      primary.textContent = "You're in! Opening…";
      location.href = `/app?guild=${encodeURIComponent(g.id)}`;
    } catch (err) {
      primary.classList.remove("busy");
      if (err.status === 401) return signedOut();
      primary.textContent = `Join ${guild.name}`;
      showError(err.status === 404 ? "This invite has gone down the long drop. Ask for a fresh one." : err.message);
    }
  }

  async function signedIn() {
    $("inv-alt").hidden = true;
    primary.textContent = `Join ${guild.name}`;
    primary.href = `/app?invite=${encodeURIComponent(code)}`;
    primary.onclick = join;
    try {
      const [me, guilds] = await Promise.all([call("GET", "/api/users/@me"), call("GET", "/api/guilds")]);
      if (guilds.some((g) => g.id === guild.id)) {
        primary.textContent = `Open ${guild.name}`;
        primary.href = openUrl();
        primary.onclick = null;
        say("You're already in here as ", me.display_name, ".");
      } else {
        say("Signed in as ", me.display_name, ".");
      }
    } catch (err) {
      if (err.status === 401) signedOut();
    }
  }

  if (token) signedIn();
  else signedOut();

  // ---------- open in the app ----------
  if (!inApp && device) {
    const btn = $("inv-open-app");
    btn.hidden = false;
    if (device === "android" && data.android_package) {
      // Opens the app if it's installed (it handles https://<here>/invite/*),
      // otherwise Chrome goes to the download section instead.
      const fallback = `${location.origin}/#download`;
      btn.href = `intent://${location.host}/invite/${encodeURIComponent(code)}#Intent;scheme=${location.protocol.replace(":", "")};`
        + `package=${data.android_package};S.browser_fallback_url=${encodeURIComponent(fallback)};end`;
    } else {
      const scheme = `outhouse://invite/${encodeURIComponent(code)}`;
      btn.href = scheme;
      btn.addEventListener("click", (e) => {
        e.preventDefault();
        $("inv-noapp").hidden = true;
        let left = false;
        const away = () => { left = true; };
        addEventListener("blur", away, { once: true });
        addEventListener("pagehide", away, { once: true });
        document.addEventListener("visibilitychange", () => { if (document.hidden) left = true; }, { once: true });
        if (/Firefox\//.test(ua)) {
          // Firefox shows an error page for a scheme nothing handles, so try it out of sight.
          const frame = document.createElement("iframe");
          frame.hidden = true;
          frame.src = scheme;
          document.body.appendChild(frame);
          setTimeout(() => frame.remove(), 3000);
        } else {
          location.href = scheme;
        }
        setTimeout(() => {
          removeEventListener("blur", away);
          if (!left && !document.hidden) $("inv-noapp").hidden = false;
        }, 1500);
      });
    }
    $("inv-continue").addEventListener("click", (e) => {
      e.preventDefault();
      $("inv-noapp").hidden = true;
      primary.focus();
    });
  }
})();
