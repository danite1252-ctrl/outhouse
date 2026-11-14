// ======================================================================
// Account security: your email (confirm, add, change), two-factor sign-in
// (authenticator app, recovery codes, trusted devices), the banners that
// suggest them, and the site owner's tools (Server status → Email, People
// → Sign-in & recovery). The sign-in screen's code step and "Forgot
// password?" are in app.js; the pages email links open are email-link.js.
// ======================================================================

const acct = {
  me: null, // /api/users/@me (with email and two-factor flags)
};

const ACCT_DAY = 24 * 60 * 60 * 1000;

function acctSnoozeKey(kind) {
  return `snooze_${kind}_${state.me?.id || acct.me?.id || "?"}`;
}
function acctSnoozed(kind) {
  return Number(prefs.get(acctSnoozeKey(kind), 0)) > Date.now();
}
function acctSnooze(kind, days) {
  prefs.set(acctSnoozeKey(kind), Date.now() + days * ACCT_DAY);
}

async function acctRefresh() {
  const me = await api("GET", "/api/users/@me").catch(() => null);
  if (me) acct.me = me;
  acctBanner();
  return acct.me;
}

// ---------- small building blocks ----------

function acctPassword(label, autocomplete = "current-password") {
  const input = el("input", { type: "password", autocomplete, maxlength: 256 });
  return { input, row: el("label", { class: "field" }, label, input) };
}

function acctCodeInput() {
  return el("input", { type: "text", inputmode: "numeric", autocomplete: "one-time-code", maxlength: 14, class: "acct-code", placeholder: "123456", "aria-label": "Code" });
}

// A dialog with its own fields; `onSubmit` returns true to close (false
// keeps it open, e.g. after a wrong code).
function acctDialog(title, body, submitLabel, onSubmit, { cls = "", danger = false, cancel = "Cancel" } = {}) {
  const error = el("p", { class: "error", role: "alert" });
  const submit = el("button", { type: "submit", class: danger ? "primary danger-fill" : "primary" }, submitLabel);
  const dialog = el("dialog", { class: `acct-dialog ${cls}` },
    el("form", { method: "dialog" },
      el("h2", {}, title),
      body,
      error,
      el("div", { class: "row" },
        cancel ? el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, cancel) : null,
        submitLabel ? submit : null)));
  dialog.querySelector("form").addEventListener("submit", async (e) => {
    e.preventDefault();
    error.textContent = "";
    submit.disabled = true;
    try {
      if (await onSubmit({ dialog, error })) dialog.close();
    } catch (err) {
      error.textContent = err.message;
    } finally {
      submit.disabled = false;
    }
  });
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  dialog.querySelector("input")?.focus();
  return dialog;
}

function acctMask(email) {
  if (!email) return "";
  const [local, domain] = String(email).split("@");
  return domain ? `${local.slice(0, 2)}…@${domain}` : email;
}

// ---------- banners ----------

function acctBannerNode(kind, text, actions, { strong = false, dismiss = null } = {}) {
  return el("div", { class: `acct-banner acct-${kind}${strong ? " strong" : ""}`, role: "status", "data-kind": kind },
    el("span", { class: "acct-banner-icon", "aria-hidden": "true" }, kind === "2fa" ? "🔐" : "✉"),
    el("span", { class: "acct-banner-text" }, text),
    el("span", { class: "acct-banner-actions" }, actions),
    dismiss ? el("button", { type: "button", class: "icon-btn acct-banner-x", "aria-label": "Not now", title: "Not now", onclick: dismiss }, "✕") : null);
}

// One at a time: add an email > confirm it > set up two-factor.
function acctBanner() {
  document.querySelector(".acct-banner")?.remove();
  const me = acct.me;
  if (!me || me.bot) return;
  let node = null;
  const close = (kind, days) => () => { acctSnooze(kind, days); acctBanner(); };
  if (me.email_missing && !acctSnoozed("add_email")) {
    node = acctBannerNode("add-email", "Add an email so you can reset your password if you forget it.",
      [el("button", { type: "button", class: "acct-banner-btn", onclick: () => acctEmailDialog() }, "Add email")],
      { dismiss: close("add_email", 1) });
  } else if (me.email && (!me.email_verified || me.email_pending) && !acctSnoozed("confirm_email")) {
    const addr = me.email_pending || me.email;
    const text = me.email_available
      ? `Confirm your email — we sent a link to ${addr}.`
      : `Confirm your email (${addr}) — the link comes once this server can send email.`;
    node = acctBannerNode("confirm-email", text, [
      me.email_available ? el("button", { type: "button", class: "acct-banner-btn", onclick: async (e) => {
        e.target.disabled = true;
        const r = await tryApi("POST", "/api/users/@me/email/resend").catch(() => null);
        e.target.disabled = false;
        if (r) toast(`Sent another link to ${r.sent_to}.`);
      } }, "Resend") : null,
      el("button", { type: "button", class: "acct-banner-btn", onclick: () => acctEmailDialog() }, "Change"),
    ], { dismiss: close("confirm_email", 1) });
  } else if (me.two_factor_nudge && !acctSnoozed("2fa")) {
    const strong = me.two_factor_nudge === "strong";
    node = acctBannerNode("2fa", strong
      ? "You can manage or moderate outhouses here — please protect your account with two-factor authentication."
      : "Protect your account with two-factor authentication.", [
      el("button", { type: "button", class: "acct-banner-btn primary-ish", onclick: () => acctSetup2fa() }, "Set up"),
      el("button", { type: "button", class: "acct-banner-btn", onclick: close("2fa", strong ? 3 : 14) }, "Not now"),
    ], { strong });
  }
  if (!node) return;
  const head = document.querySelector("main.chat .chat-head");
  if (head) head.after(node);
}

// After signing in (and on every load while signed in).
const acctBaseStart = start;
start = async function (...args) {
  const out = await acctBaseStart.apply(this, args);
  acctRefresh();
  return out;
};

// ---------- email ----------

function acctEmailDialog() {
  const me = acct.me || {};
  const email = el("input", { type: "email", autocomplete: "email", maxlength: 254, placeholder: "you@example.com", value: me.email_pending || (me.email_verified ? "" : me.email || "") });
  const pw = acctPassword("Your password");
  const adding = !me.email;
  acctDialog(adding ? "Add an email" : "Change your email", [
    el("p", { class: "muted small acct-tight" }, adding
      ? "So you can reset your password if you forget it. We'll send a link to confirm it. Nobody else sees it."
      : me.email_verified
        ? `Your confirmed email (${me.email}) stays until you confirm the new one. We'll let it know about the change.`
        : "We'll send a link to confirm the new address."),
    el("label", { class: "field" }, "Email", email),
    pw.row,
  ], adding ? "Add email" : "Change email", async () => {
    const r = await api("PUT", "/api/users/@me/email", { email: email.value.trim(), password: pw.input.value });
    acct.me = { ...acct.me, ...r };
    acctBanner();
    toast(r.email_available ? "Check your inbox for the confirmation link." : "Saved. The confirmation link comes once this server can send email.");
    document.querySelector("dialog.settings[data-tab=account]")?._draw?.("account");
    return true;
  });
}

function acctEmailSection(me) {
  const verified = me.email && me.email_verified;
  const chip = !me.email ? el("span", { class: "acct-chip warn" }, "Missing")
    : verified ? el("span", { class: "acct-chip good" }, "Confirmed")
      : el("span", { class: "acct-chip warn" }, "Not confirmed");
  return el("section", { class: "acct-section acct-email-section" },
    el("h3", {}, "Email ", chip),
    el("p", { class: "muted small acct-tight" }, "For password resets and security notices. Nobody else sees it."),
    me.email ? el("div", { class: "acct-email-line" }, el("strong", {}, me.email)) : el("p", { class: "acct-tight" }, "You haven't added one yet, so you couldn't reset your password if you forgot it."),
    me.email_pending ? el("p", { class: "muted small acct-tight" }, `Waiting for you to confirm ${me.email_pending}. Until then, ${me.email} stays.`) : null,
    !me.email_available ? el("p", { class: "muted small acct-tight" }, "This server can't send email yet; confirmation links come once the server owner sets it up.") : null,
    el("div", { class: "acct-actions" },
      el("button", { type: "button", class: "ghost", onclick: () => acctEmailDialog() }, me.email ? "Change email…" : "Add email…"),
      me.email_available && me.email && (!verified || me.email_pending) ? el("button", { type: "button", class: "ghost", onclick: async (e) => {
        e.target.disabled = true;
        const r = await tryApi("POST", "/api/users/@me/email/resend").catch(() => null);
        e.target.disabled = false;
        if (r) toast(`Sent another link to ${r.sent_to}.`);
      } }, "Resend link") : null));
}

// ---------- two-factor ----------

function acctRecoveryCodesView(codes) {
  const text = `Outhouse recovery codes for @${state.me?.username || acct.me?.username || ""}\nEach code works once. Keep them somewhere safe.\n\n${codes.join("\n")}\n`;
  const copy = el("button", { type: "button", class: "ghost", onclick: async () => {
    try { await navigator.clipboard.writeText(codes.join("\n")); toast("Copied."); } catch { toast("Couldn't copy. Select them and copy by hand."); }
  } }, "Copy");
  const download = el("button", { type: "button", class: "ghost", onclick: () => {
    const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    const a = el("a", { href: url, download: "outhouse-recovery-codes.txt" });
    document.body.append(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 2000);
  } }, "Download .txt");
  return el("div", { class: "acct-codes-wrap" },
    el("p", { class: "acct-tight" }, el("strong", {}, "Save these recovery codes now."), " If you lose your phone, each one gets you in once. You won't see them again."),
    el("ol", { class: "acct-codes" }, codes.map((c) => el("li", {}, el("code", {}, c)))),
    el("div", { class: "acct-actions" }, copy, download));
}

function acctShowCodes(title, codes, after) {
  acctDialog(title, [acctRecoveryCodesView(codes)], "I've saved them", async () => { after?.(); return true; }, { cls: "acct-wide", cancel: null });
}

// Set up: password → QR code and a code from the app → recovery codes.
function acctSetup2fa() {
  const pw = acctPassword("Your password");
  acctDialog("Set up two-factor", [
    el("p", { class: "muted small acct-tight" }, "Signing in will need your password and a 6-digit code from an authenticator app on your phone (Google Authenticator, Microsoft Authenticator, 1Password, Aegis or any other)."),
    pw.row,
  ], "Next", async ({ dialog }) => {
    const s = await api("POST", "/api/auth/2fa/setup", { password: pw.input.value });
    dialog.close();
    acctScanStep(s);
    return false;
  }, { cls: "acct-setup" });
}

function acctScanStep(s) {
  const code = acctCodeInput();
  const qr = s.qr_svg
    ? el("img", { class: "acct-qr", alt: "QR code for your authenticator app", src: `data:image/svg+xml;base64,${btoa(s.qr_svg)}` })
    : null;
  acctDialog("Scan this with your app", [
    el("div", { class: "acct-scan" },
      qr,
      el("div", { class: "acct-scan-side" },
        el("ol", { class: "acct-steps" },
          el("li", {}, "Open your authenticator app and add an account (often a + button)."),
          el("li", {}, "Scan the QR code. Can't scan? Type this key instead:"),
        ),
        el("code", { class: "acct-secret", title: "Setup key" }, s.secret_grouped),
        el("div", { class: "muted small" }, `Account: ${s.issuer} (${s.account}) · time-based`),
      )),
    el("label", { class: "field" }, "Then enter the 6-digit code it shows", code),
  ], "Turn on two-factor", async ({ dialog }) => {
    const r = await api("POST", "/api/auth/2fa/confirm", { code: code.value.trim() });
    dialog.close();
    acctShowCodes("Two-factor is on", r.recovery_codes, () => {
      acctRefresh();
      document.querySelector("dialog.settings[data-tab=account]")?._draw?.("account");
    });
    toast("Two-factor sign-in is on.");
    return false;
  }, { cls: "acct-wide acct-setup" });
}

function acctTrustedDevices(s, redraw) {
  if (!s.trusted_devices?.length) return el("p", { class: "muted small acct-tight" }, "No trusted devices. When you sign in with a code you can tick “Trust this device” to skip codes there for a while.");
  return el("div", { class: "acct-devices" },
    s.trusted_devices.map((d) => el("div", { class: "acct-device" },
      el("div", { class: "acct-device-main" },
        el("div", {}, el("strong", {}, d.label || "A device"), d.current ? el("span", { class: "acct-chip good" }, "This device") : null),
        el("div", { class: "muted small" }, `Trusted ${new Date(d.created_at).toLocaleDateString()} · last used ${relTime(d.last_used_at)} · until ${new Date(d.expires_at).toLocaleDateString()}`)),
      el("button", { type: "button", class: "ghost small-btn", onclick: async () => {
        if (await tryApi("DELETE", `/api/auth/2fa/devices/${d.id}`).catch(() => null)) { toast("Forgotten. It'll ask for a code next time."); redraw(); }
      } }, "Forget"))),
    s.trusted_devices.length > 1 ? el("div", {}, el("button", { type: "button", class: "ghost", onclick: async () => {
      if (!(await confirmBox("Forget all trusted devices?", "Every device will ask for a code next time you sign in.", "Forget all"))) return;
      if (await tryApi("DELETE", "/api/auth/2fa/devices").catch(() => null)) { toast("All forgotten."); redraw(); }
    } }, "Forget all")) : null);
}

async function acctTwoFactorSection() {
  const box = el("section", { class: "acct-section acct-2fa-section" }, el("h3", {}, "Two-factor authentication"));
  const draw = async () => {
    const s = await api("GET", "/api/auth/2fa").catch(() => null);
    if (!s) return;
    const chip = s.enabled ? el("span", { class: "acct-chip good" }, "On") : el("span", { class: "acct-chip warn" }, "Off — recommended");
    box.replaceChildren(el("h3", {}, "Two-factor authentication ", chip));
    if (!s.enabled) {
      box.append(
        el("p", { class: "acct-tight" }, "Even if someone learns your password, they can't sign in without a code from your phone. It takes a minute."),
        !s.available ? el("p", { class: "muted small acct-tight" }, "It can't be turned on until the server owner sets AUTH_SECRET_KEY.") : null,
        el("div", { class: "acct-actions" }, el("button", { type: "button", class: "primary", disabled: !s.available, onclick: () => acctSetup2fa() }, "Set up two-factor")));
      return;
    }
    box.append(
      el("p", { class: "muted small acct-tight" }, `On since ${new Date(s.enabled_at).toLocaleDateString()}. Signing in needs a code from your authenticator app. ${s.recovery_codes_left} recovery code${s.recovery_codes_left === 1 ? "" : "s"} left.`),
      s.recovery_codes_left <= 3 ? el("p", { class: "acct-warn small acct-tight" }, "You're running low on recovery codes: make new ones.") : null,
      el("div", { class: "acct-actions" },
        el("button", { type: "button", class: "ghost", onclick: () => {
          const code = acctCodeInput();
          acctDialog("New recovery codes", [
            el("p", { class: "muted small acct-tight" }, "Your old recovery codes stop working. Enter a code from your app to go on."),
            el("label", { class: "field" }, "Code from your app", code),
          ], "Make new codes", async ({ dialog }) => {
            const r = await api("POST", "/api/auth/2fa/recovery-codes", { code: code.value.trim() });
            dialog.close();
            acctShowCodes("New recovery codes", r.recovery_codes, draw);
            return false;
          });
        } }, "New recovery codes…"),
        el("button", { type: "button", class: "danger-btn", onclick: () => {
          const pw = acctPassword("Your password");
          const code = acctCodeInput();
          acctDialog("Turn off two-factor?", [
            el("p", { class: "muted small acct-tight" }, "Signing in will need only your password. Your trusted devices are forgotten."),
            pw.row,
            el("label", { class: "field" }, "Code from your app (or a recovery code)", code),
          ], "Turn off", async () => {
            await api("POST", "/api/auth/2fa/disable", { password: pw.input.value, code: code.value.trim() });
            toast("Two-factor is off.");
            draw();
            acctRefresh();
            return true;
          }, { danger: true });
        } }, "Turn off…")),
      el("h4", { class: "acct-sub" }, "Trusted devices"),
      acctTrustedDevices(s, draw));
  };
  await draw();
  return box;
}

// Settings → My account: email and two-factor around the password part.
const acctBaseDrawAccount = drawAccount;
drawAccount = function (main) {
  acctBaseDrawAccount(main);
  const pwHead = [...main.querySelectorAll("h3")].find((h) => h.textContent === "Password");
  const me = acct.me;
  (async () => {
    const fresh = await acctRefresh();
    if (!main.isConnected) return;
    const emailBox = acctEmailSection(fresh || me || {});
    if (pwHead) pwHead.before(emailBox); else main.append(emailBox);
    if (!(fresh || me)?.bot) main.append(await acctTwoFactorSection());
  })();
};

// ---------- site owner: Server status → Email ----------

if (typeof drawServerStatus === "function") {
  const acctBaseDrawServerStatus = drawServerStatus;
  drawServerStatus = async function (main) {
    await acctBaseDrawServerStatus(main);
    const s = await api("GET", "/api/safety/server-status").catch(() => null);
    if (!s?.email || !main.isConnected) return;
    const m = s.email;
    const to = el("input", { type: "email", maxlength: 254, placeholder: acct.me?.email || "you@example.com", "aria-label": "Send the test to" });
    const result = el("p", { class: "muted small acct-tight", role: "status" });
    const test = el("button", { type: "button", class: "ghost", disabled: !m.configured, onclick: async () => {
      test.disabled = true;
      result.textContent = "Sending…";
      try {
        const r = await api("POST", "/api/safety/email-test", { to: to.value.trim() || null });
        result.textContent = `Sent to ${r.to}. Check that inbox (and its spam folder).`;
      } catch (err) {
        result.textContent = err.message;
      } finally {
        test.disabled = false;
      }
    } }, "Send a test email");
    const line = m.configured
      ? `Set up · ${m.server} (${m.tls}) · from ${m.from}`
      : `Email not set up: ${m.problem || "SMTP settings missing"}.`;
    const last = m.last_error && (!m.last_ok_at || m.last_error_at > m.last_ok_at)
      ? `Last error ${relTime(m.last_error_at)}: ${m.last_error}`
      : m.last_ok_at ? `Last sent ${relTime(m.last_ok_at)} · ${m.sent} sent since start` : "Nothing sent since start.";
    const block = el("div", { class: "acct-status-block" },
      el("h3", {}, "Email & sign-in"),
      el("div", { class: "status-grid" },
        el("div", { class: `status-item ${m.configured ? "good" : "warn"}` }, el("div", { class: "status-label" }, "Email"), el("div", { class: "status-value" }, line)),
        m.configured ? el("div", { class: `status-item ${m.last_error && (!m.last_ok_at || m.last_error_at > m.last_ok_at) ? "bad" : ""}` }, el("div", { class: "status-label" }, "Delivery"), el("div", { class: "status-value" }, last)) : null,
        el("div", { class: `status-item ${s.auth_key === "env" ? "good" : "warn"}` }, el("div", { class: "status-label" }, "Two-factor key"),
          el("div", { class: "status-value" }, s.auth_key === "env" ? "AUTH_SECRET_KEY is set" : s.auth_key === "file" ? "Using a key file in the uploads folder: set AUTH_SECRET_KEY" : "Missing: two-factor can't be turned on"))),
      m.configured ? null : el("p", { class: "muted small acct-tight" }, "Without email, sign-up still asks for an address (confirmation links go out once email works) and “Forgot password?” tells people to ask you: make them a reset link in People. Set SMTP_HOST, MAIL_FROM and friends in deploy/.env (deploy/README.md, “Email”)."),
      el("div", { class: "acct-test-row" }, to, test),
      result);
    const refresh = main.querySelector("button.ghost:last-of-type")?.parentElement;
    if (refresh && refresh.parentElement === main) refresh.before(block); else main.append(block);
  };
}

// ---------- site owner: People → Sign-in & recovery ----------

function acctOwnerSection(u, ctx) {
  const si = u.sign_in;
  if (!si || u.bot) return null;
  const item = (label, value, cls = "") => el("div", { class: `status-item ${cls}` }, el("div", { class: "status-label" }, label), el("div", { class: "status-value" }, value));
  return el("section", { class: "acct-section acct-owner-section" },
    el("h3", {}, "Sign-in & recovery"),
    el("div", { class: "status-grid" },
      item("Email", si.email ? `${si.email} · ${si.email_verified ? "confirmed" : "not confirmed"}${si.email_pending ? ` (changing to ${si.email_pending})` : ""}` : "None", si.email && si.email_verified ? "good" : "warn"),
      item("Two-factor", si.enabled ? `On since ${new Date(si.enabled_at).toLocaleDateString()} · ${si.recovery_codes_left} recovery codes left · ${si.trusted_devices} trusted device${si.trusted_devices === 1 ? "" : "s"}` : "Off", si.enabled ? "good" : "")),
    el("div", { class: "acct-actions" },
      el("button", { type: "button", class: "ghost acct-reset-link-btn", onclick: async () => {
        if (!(await confirmBox(`Make a password reset link for ${u.display_name}?`, "For someone who can't use their email. It works once, for 24 hours, and any older reset link stops working. You'll see it once: send it to them privately. It's written to the safety log.", "Make link"))) return;
        const r = await tryApi("POST", `/api/safety/users/${u.id}/reset-link`).catch(() => null);
        if (!r) return;
        const link = r.url || `${location.origin}${r.path}`;
        const field = el("input", { type: "text", readonly: true, value: link, class: "acct-link-field", "aria-label": "Reset link" });
        acctDialog("Password reset link", [
          el("p", { class: "muted small acct-tight" }, `Works once, until ${new Date(r.expires_at).toLocaleString()}. Shown only now.`),
          field,
          r.two_factor ? el("p", { class: "acct-warn small acct-tight" }, "They have two-factor on: the reset page also asks for a code from their app or a recovery code. If they lost those too, remove their two-factor below first.") : null,
        ], "Copy link", async () => {
          try { await navigator.clipboard.writeText(link); toast("Copied."); } catch { field.select(); toast("Select it and copy by hand."); return false; }
          return true;
        }, { cancel: "Close", cls: "acct-wide" });
        ctx?.refresh?.();
      } }, "Create password reset link…"),
      si.enabled ? el("button", { type: "button", class: "danger-btn acct-remove-2fa-btn", onclick: () => {
        const reason = el("input", { type: "text", maxlength: 300, placeholder: "Lost their phone and recovery codes; checked on Discord" });
        acctDialog(`Remove ${u.display_name}'s two-factor?`, [
          el("p", { class: "muted small acct-tight" }, "Only when you're sure it's really them (they lost their phone and their recovery codes). They can then sign in with just their password. They get an email, and it's written to the safety log."),
          el("label", { class: "field" }, "Why", reason),
        ], "Remove two-factor", async () => {
          await api("POST", `/api/safety/users/${u.id}/remove-2fa`, { reason: reason.value.trim() });
          toast("Two-factor removed.");
          ctx?.refresh?.();
          return true;
        }, { danger: true });
      } }, "Remove two-factor…") : null));
}

if (typeof accountActions === "function") {
  const acctBaseAccountActions = accountActions;
  accountActions = function (u, act, guildId = null, ctx = null) {
    const base = acctBaseAccountActions(u, act, guildId, ctx);
    // People (not a report): the sign-in tools too.
    if (!ctx || ctx.reportId) return base;
    const extra = acctOwnerSection(u, ctx);
    if (!extra) return base;
    return el("div", { class: "acct-owner-wrap" }, base, extra);
  };
}

// Safety log icons for the new entries.
if (typeof logView === "function") {
  const ACCT_LOG_ICONS = { password_reset: "🔑", reset_link: "🔗", email_changed: "✉", "2fa_enabled": "🔐", "2fa_disabled": "🔓", "2fa_reset": "🔓", "2fa_recovery_codes": "🔐", recovery_code_used: "🗝" };
  const acctBaseLogView = logView;
  logView = function (entries) {
    const out = acctBaseLogView(entries);
    if (entries?.length) {
      out.querySelectorAll?.(".audit-row").forEach((row, i) => {
        const icon = ACCT_LOG_ICONS[entries[i]?.action];
        if (icon) row.querySelector(".audit-icon").textContent = icon;
      });
    }
    return out;
  };
}
