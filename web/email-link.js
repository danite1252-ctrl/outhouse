// The page reset and confirmation emails link to:
//   /reset?token=…         choose a new password (and, with two-factor on,
//                          give a code from the app or a recovery code)
//   /verify-email?token=…  confirm an email address
// It works in any browser and in the Windows/Android apps' webview. The
// token is taken out of the address bar straight away (so it isn't left in
// history), and the page sends no Referer anywhere.
"use strict";

(() => {
  const params = new URLSearchParams(location.search);
  const token = (params.get("token") || "").trim();
  const reset = location.pathname.replace(/\/+$/, "") === "/reset";
  history.replaceState(null, "", location.pathname);

  const card = document.getElementById("card");
  const status = document.getElementById("status");

  function el(tag, attrs = {}, ...children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === "class") node.className = v;
      else if (k.startsWith("on")) node.addEventListener(k.slice(2), v);
      else if (v === true) node.setAttribute(k, "");
      else if (v !== false && v != null) node.setAttribute(k, v);
    }
    for (const c of children.flat()) {
      if (c == null || c === false) continue;
      node.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return node;
  }

  async function post(path, body) {
    const res = await fetch(path, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      credentials: "same-origin",
    });
    const data = await res.json().catch(() => null);
    if (!res.ok) throw new Error((data && data.error) || `Something went wrong (${res.status}).`);
    return data;
  }

  // Keep the logo and wordmark; swap what's under them.
  function show(...nodes) {
    while (card.children.length > 2) card.lastElementChild.remove();
    card.append(...nodes.flat().filter(Boolean));
  }

  const signInLink = (label = "Sign in") => el("a", { class: "primary", href: "/login" }, label);

  // "Open Outhouse": the installed app if there is one (the Windows app via
  // its outhouse:// link, the Android app by its package), otherwise the
  // web app here. Already inside an app's window: just go to the app page.
  const ua = navigator.userAgent;
  const inApp = !!(window.__OUTHOUSE_DESKTOP__ || window.__OUTHOUSE_PLATFORM__);
  const device = /Android/i.test(ua) ? "android" : /Windows NT/i.test(ua) ? "windows" : "";
  let androidPackage = "";
  fetch("/api/auth/registration", { credentials: "same-origin" })
    .then((r) => (r.ok ? r.json() : null))
    .then((d) => { androidPackage = (d && d.android_package) || ""; })
    .catch(() => {});

  function openOuthouse(e) {
    const web = `${location.origin}/app`;
    if (inApp || !device) return; // the plain link to /app does it
    if (device === "android") {
      if (!androidPackage) return;
      e.preventDefault();
      location.href = `intent:#Intent;action=android.intent.action.MAIN;category=android.intent.category.LAUNCHER;`
        + `package=${androidPackage};S.browser_fallback_url=${encodeURIComponent(web)};end`;
      return;
    }
    // Windows: try the app; if nothing takes the link, carry on in the browser.
    e.preventDefault();
    const scheme = "outhouse://open";
    let left = false;
    const away = () => { left = true; };
    addEventListener("blur", away, { once: true });
    addEventListener("pagehide", away, { once: true });
    document.addEventListener("visibilitychange", () => { if (document.hidden) left = true; }, { once: true });
    if (/Firefox\//.test(ua)) {
      // Firefox shows an error page for a link nothing handles; try it out of sight.
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
      if (!left && !document.hidden) location.href = web;
    }, 1500);
  }
  const openLink = (label = "Open Outhouse") => el("a", { class: "primary", href: "/app", onclick: openOuthouse }, label);

  function failed(message) {
    document.title = "Link not working · Outhouse";
    show(el("h2", { class: "auth-step-title" }, "This link isn't working"),
      el("p", { class: "muted" }, message),
      reset ? el("p", { class: "muted small" }, "On the sign-in screen, use “Forgot password?” to get a new one.") : null,
      signInLink("Go to Outhouse"));
  }

  if (!/^[0-9a-f]{64}$/i.test(token)) {
    failed("The link is incomplete. Open it straight from the email, or copy the whole link.");
    return;
  }

  // ---------- confirming an email ----------
  if (!reset) {
    document.title = "Confirm your email · Outhouse";
    status.textContent = "Confirming your email…";
    post("/api/auth/verify-email", { token })
      .then((r) => {
        document.title = "Email confirmed · Outhouse";
        show(el("div", { class: "done-icon", "aria-hidden": "true" }, "✉️"),
          el("h2", { class: "auth-step-title" }, "Email confirmed"),
          el("p", { class: "muted" }, `Thanks! ${r.email} is confirmed. If you ever forget your password, we can send you a reset link there.`),
          openLink(),
          inApp ? null : el("p", { class: "muted small" }, "Opens the Outhouse app if it's installed on this device, otherwise Outhouse in your browser."));
      })
      .catch((err) => failed(err.message));
    return;
  }

  // ---------- resetting a password ----------
  document.title = "Choose a new password · Outhouse";
  post("/api/auth/reset/check", { token })
    .then((info) => drawResetForm(info))
    .catch((err) => failed(err.message));

  function drawResetForm(info) {
    const pw = el("input", { type: "password", id: "password", autocomplete: "new-password", minlength: 8, maxlength: 256, required: true });
    const again = el("input", { type: "password", id: "password2", autocomplete: "new-password", minlength: 8, maxlength: 256, required: true });
    const code = info.two_factor ? el("input", { type: "text", id: "code", inputmode: "numeric", autocomplete: "one-time-code", maxlength: 14, required: true }) : null;
    const error = el("p", { class: "error", role: "alert" });
    const submit = el("button", { type: "submit", class: "primary" }, "Change password");
    const form = el("form", { class: "reset-form", method: "post" },
      el("label", {}, "New password", pw),
      el("label", {}, "New password again", again),
      code ? el("label", {}, "Code from your authenticator app (or a recovery code)", code) : null,
      error,
      submit);
    form.style.display = "flex";
    form.style.flexDirection = "column";
    form.style.gap = "14px";
    form.addEventListener("submit", async (e) => {
      e.preventDefault();
      error.textContent = "";
      if (pw.value.length < 8) { error.textContent = "Passwords need at least 8 characters."; return; }
      if (pw.value !== again.value) { error.textContent = "The two passwords don't match."; return; }
      submit.disabled = true;
      try {
        const r = await post("/api/auth/reset", { token, password: pw.value, code: code ? code.value.trim() : null });
        // Any session in this browser has just been signed out.
        try { localStorage.removeItem("outhouse_token"); } catch { /* storage blocked */ }
        document.title = "Password changed · Outhouse";
        show(el("div", { class: "done-icon", "aria-hidden": "true" }, "🔑"),
          el("h2", { class: "auth-step-title" }, "Password changed"),
          el("p", { class: "muted" }, `You're signed out everywhere. Sign in as @${r.username} with your new password.`),
          r.suspended ? el("p", { class: "error" }, "This account is suspended, so signing in still won't work until the suspension ends.") : null,
          openLink("Sign in"),
          inApp ? null : el("p", { class: "muted small" }, "Opens the Outhouse app if it's installed on this device, otherwise Outhouse in your browser."));
      } catch (err) {
        error.textContent = err.message;
        if (/expired|already used|new reset link/i.test(err.message)) setTimeout(() => failed(err.message), 1200);
      } finally {
        submit.disabled = false;
      }
    });
    show(el("h2", { class: "auth-step-title" }, "Choose a new password"),
      el("p", { class: "who" }, `@${info.username}`),
      info.two_factor ? el("p", { class: "muted small" }, "Your account has two-factor sign-in, so we need a code from your app too.") : null,
      form);
    pw.focus();
  }
})();
