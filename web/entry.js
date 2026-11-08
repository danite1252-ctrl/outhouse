// How you arrived at the web client (/app). The welcome page at / links here
// with ?register (open the sign-up form) or ?login. Loaded last, after app.js.
(() => {
  const q = new URLSearchParams(location.search);
  const inApp = !!(window.__OUTHOUSE_DESKTOP__ || window.__OUTHOUSE_PLATFORM__);
  if (q.has("register") || q.has("login")) {
    if (!state.token && q.has("register")) setAuthMode(true);
    q.delete("register");
    q.delete("login");
    const rest = q.toString();
    history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : "") + location.hash);
  }
  // A way back to the welcome page from the sign-in screen (not inside the apps).
  const form = document.getElementById("auth-form");
  if (!inApp && form && !document.getElementById("auth-home")) {
    const link = document.createElement("a");
    link.id = "auth-home";
    link.className = "auth-home";
    link.href = "/";
    link.textContent = "What's Outhouse?";
    link.style.cssText = "align-self:center;font-size:13px;color:var(--muted);text-underline-offset:3px";
    form.appendChild(link);
  }
})();
