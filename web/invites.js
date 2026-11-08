// Invite links: the "Invite people" dialog (a link to copy, how long it
// lasts and how many people can use it, and your invites so far), the
// Invites page in Outhouse settings, and arriving at /app?invite=<code>
// from an invite page (sign up or in first, then you're joined straight in).
// Loaded after app.js (uses el, api, tryApi, state, toast, can, …).
"use strict";

const INVITE_EXPIRY = [["Never", 0], ["30 minutes", 1800], ["1 hour", 3600], ["6 hours", 21600], ["12 hours", 43200], ["1 day", 86400], ["7 days", 604800]];
const INVITE_USES = [["No limit", 0], ["1 use", 1], ["5 uses", 5], ["10 uses", 10], ["25 uses", 25], ["100 uses", 100]];
const INVITE_DEFAULT = { max_age_secs: 604800, max_uses: 0 };
const PENDING_INVITE = "outhouse_pending_invite";

// "https://oh.example/invite/AbC123", "oh.example/invite/AbC123" or just "AbC123" → "AbC123".
function inviteCodeFrom(text) {
  const t = String(text || "").trim();
  const m = t.match(/\/invite\/([A-Za-z0-9]{1,32})(?:[/?#]|$)/) || t.match(/[?&]invite=([A-Za-z0-9]{1,32})/);
  if (m) return m[1];
  return /^[A-Za-z0-9]{1,32}$/.test(t) ? t : t.replace(/[^A-Za-z0-9]/g, "").slice(0, 32);
}

const inviteLink = (inv) => inv.url || `${location.origin}/invite/${inv.code}`;

function inviteExpiryText(inv) {
  if (!inv.expires_at) return "Never expires";
  const ms = new Date(inv.expires_at) - Date.now();
  if (ms <= 0) return "Expired";
  const min = Math.round(ms / 60000);
  if (min < 60) return `Expires in ${Math.max(1, min)} min`;
  const hours = Math.round(min / 60);
  if (hours < 48) return `Expires in ${hours} h`;
  return `Expires in ${Math.round(hours / 24)} days`;
}

function inviteUsesText(inv) {
  return inv.max_uses ? `${inv.uses} of ${inv.max_uses} uses` : `${inv.uses} ${inv.uses === 1 ? "use" : "uses"}`;
}

// Copies text, falling back to the old select-and-copy way when the
// clipboard API isn't allowed (plain http, older webviews).
async function copyText(text, host = document.body) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch { /* fall back */ }
  const ta = el("textarea", { readonly: true, style: "position:fixed;top:0;left:0;width:1px;height:1px;opacity:0" });
  ta.value = text;
  host.append(ta);
  ta.select();
  let ok = false;
  try { ok = document.execCommand("copy"); } catch { ok = false; }
  ta.remove();
  return ok;
}

// A "Copy link" button that says "Copied!" for a moment.
function copyButton(getText, label = "Copy link", cls = "primary inv-copy") {
  const btn = el("button", { type: "button", class: cls }, label);
  btn.addEventListener("click", async () => {
    const ok = await copyText(getText(), btn.closest("dialog") || document.body);
    btn.textContent = ok ? "Copied!" : "Couldn't copy";
    btn.classList.toggle("copied", ok);
    clearTimeout(btn._t);
    btn._t = setTimeout(() => { btn.textContent = label; btn.classList.remove("copied"); }, 2000);
  });
  return btn;
}

// One invite in a list: link, who made it, uses, expiry, copy and revoke.
function inviteRow(inv, { onRevoked, showCreator = true } = {}) {
  const mine = inv.creator?.id === state.me?.id;
  const canRevoke = mine || can("manage_guild");
  return el("div", { class: "inv-row", "data-code": inv.code },
    el("div", { class: "inv-row-main" },
      el("div", { class: "inv-row-code" }, inviteLink(inv).replace(/^https?:\/\//, "")),
      el("div", { class: "inv-row-meta muted small" },
        showCreator ? el("span", { class: "inv-row-by" }, avatarFor(inv.creator.id, inv.creator.name, 16, inv.creator.avatar_url), mine ? "You" : inv.creator.name) : null,
        el("span", {}, inviteUsesText(inv)),
        el("span", {}, inviteExpiryText(inv)),
      ),
    ),
    el("div", { class: "inv-row-actions" },
      copyButton(() => inviteLink(inv), "Copy", "small-btn inv-row-copy"),
      canRevoke ? el("button", { type: "button", class: "small-btn danger inv-revoke", title: "Stop this link working", onclick: async (e) => {
        const btn = e.currentTarget;
        btn.disabled = true;
        const ok = await tryApi("DELETE", `/api/invites/${encodeURIComponent(inv.code)}`).then(() => true, () => false);
        if (ok) { toast("Invite revoked."); onRevoked?.(inv); } else btn.disabled = false;
      } }, "Revoke") : null,
    ),
  );
}

function inviteSelect(options, value) {
  return el("select", {}, options.map(([label, v]) => el("option", { value: String(v), selected: v === value }, label)));
}

// ---------- the "Invite people" dialog ----------

async function openInviteDialog() {
  const g = currentGuild();
  if (!g) return;
  const guildId = g.id;
  let current = null;
  const linkInput = el("input", { readonly: true, class: "inv-link", "aria-label": "Invite link", value: "Making a link…" });
  linkInput.addEventListener("focus", () => linkInput.select());
  const copy = copyButton(() => (current ? inviteLink(current) : ""));
  copy.disabled = true;
  const meta = el("p", { class: "inv-meta muted small" }, "");
  const expiry = inviteSelect(INVITE_EXPIRY, INVITE_DEFAULT.max_age_secs);
  const uses = inviteSelect(INVITE_USES, INVITE_DEFAULT.max_uses);
  const regen = el("button", { type: "button", class: "ghost inv-new" }, "Make a new link");
  const list = el("div", { class: "inv-list" });
  const listHead = el("div", { class: "inv-list-head" });
  const all = can("manage_guild");

  const dialog = el("dialog", { class: "invite-dialog" },
    el("form", { method: "dialog" },
      el("h2", {}, "Invite people to ", el("span", { class: "inv-guild" }, g.name)),
      el("p", { class: "muted small", style: "margin:0" }, "Send them this link. It opens Outhouse in their app if they've got it, or in the browser if they haven't."),
      el("div", { class: "inv-link-row" }, linkInput, copy),
      meta,
      el("details", { class: "inv-options" },
        el("summary", {}, "Link settings"),
        el("div", { class: "inv-option-grid" },
          el("label", { class: "field" }, "Expire after", expiry),
          el("label", { class: "field" }, "Max number of uses", uses),
        ),
        regen,
      ),
      listHead,
      list,
      el("div", { class: "row" }, el("button", { type: "submit", class: "ghost" }, "Done")),
    ),
  );
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  copy.focus();

  const show = (inv) => {
    current = inv;
    linkInput.value = inviteLink(inv);
    copy.disabled = false;
    meta.textContent = `${inviteExpiryText(inv)} · ${inv.max_uses ? `${inv.max_uses} ${inv.max_uses === 1 ? "use" : "uses"}` : "No limit on uses"}`;
  };

  const drawList = (invites) => {
    listHead.replaceChildren(el("span", {}, all ? "Active invites" : "Your invites"), el("span", { class: "muted" }, String(invites.length)));
    list.replaceChildren(...(invites.length ? invites.map((inv) => inviteRow(inv, { showCreator: all, onRevoked: refresh })) : [el("p", { class: "muted small", style: "margin:0" }, "No other invites going.")]));
  };

  async function refresh(revoked) {
    const invites = await api("GET", `/api/guilds/${guildId}/invites`).catch(() => []);
    if (!dialog.isConnected) return;
    if (revoked && current && revoked.code === current.code) {
      current = null;
      linkInput.value = "That link's been revoked. Make a new one below.";
      copy.disabled = true;
      meta.textContent = "";
    }
    drawList(invites);
    return invites;
  }

  async function make(settings) {
    regen.disabled = true;
    const inv = await tryApi("POST", `/api/guilds/${guildId}/invites`, settings).catch(() => null);
    regen.disabled = false;
    if (!inv || !dialog.isConnected) return;
    show(inv);
    refresh();
  }

  regen.addEventListener("click", () => make({ max_age_secs: Number(expiry.value), max_uses: Number(uses.value) }));

  // Reuse your newest week-long, no-limit link if it has a day or more left;
  // otherwise make one. (Stops a pile of identical links building up.)
  const invites = await refresh() || [];
  if (!dialog.isConnected) return;
  const reusable = invites.find((inv) => inv.creator?.id === state.me?.id && !inv.max_uses
    && inv.expires_at && new Date(inv.expires_at) - Date.now() > 86400000);
  if (reusable) show(reusable);
  else make(INVITE_DEFAULT);
}

// ---------- Outhouse settings → Invites ----------

async function drawInviteSettings(page) {
  page.replaceChildren(el("h2", {}, "Invites"), el("p", { class: "muted" }, "Loading…"));
  const invites = await api("GET", `/api/guilds/${state.guildId}/invites`).catch(() => []);
  page.replaceChildren(
    el("h2", {}, "Invites"),
    el("p", { class: "muted", style: "margin-top:0" }, "Every invite link that still works. Revoke one and it stops working straight away; people who already joined stay in."),
    el("div", { class: "row", style: "justify-content:flex-start;margin-bottom:12px" },
      el("button", { type: "button", class: "primary", style: "padding:0 16px", onclick: () => openInviteDialog() }, "Invite people")),
    el("div", { class: "inv-list settings-inv-list" },
      invites.length ? invites.map((inv) => inviteRow(inv, { onRevoked: () => drawInviteSettings(page) }))
        : el("p", { class: "muted" }, "No invites going. Make one with Invite people.")),
  );
}

// ---------- arriving with ?invite=<code> ----------

(() => {
  const q = new URLSearchParams(location.search);
  if (!q.has("invite")) return;
  const code = inviteCodeFrom(q.get("invite"));
  q.delete("invite");
  const rest = q.toString();
  history.replaceState(null, "", location.pathname + (rest ? `?${rest}` : "") + location.hash);
  if (!code) return;
  try { sessionStorage.setItem(PENDING_INVITE, code); } catch { /* storage blocked */ }
})();

function pendingInvite() {
  try { return sessionStorage.getItem(PENDING_INVITE); } catch { return null; }
}

// On the sign-in / sign-up screen: say where you're headed.
(async () => {
  const code = pendingInvite();
  if (!code || state.token) return;
  const form = $("auth-form");
  const banner = el("div", { class: "auth-invite", role: "status" }, "Checking your invite…");
  form.insertBefore(banner, $("auth-subtitle").nextSibling);
  let preview = null;
  try {
    const res = await fetch(`/api/invites/${encodeURIComponent(code)}`, { cache: "no-store" });
    if (res.ok) preview = await res.json();
  } catch { /* offline */ }
  if (!preview) {
    banner.classList.add("dead");
    banner.replaceChildren(el("b", {}, "That invite has gone down the long drop."), " You can still sign in, just ask for a fresh link.");
    try { sessionStorage.removeItem(PENDING_INVITE); } catch { /* storage blocked */ }
    return;
  }
  const g = preview.guild;
  banner.replaceChildren(
    el("span", { class: "auth-invite-tile" }, g.name.split(/\s+/).filter(Boolean).slice(0, 2).map((w) => [...w][0].toUpperCase()).join("") || "O"),
    el("span", {},
      preview.inviter ? el("span", { class: "muted" }, `${preview.inviter.name} invited you to`) : el("span", { class: "muted" }, "You're invited to"),
      el("b", {}, g.name),
      el("span", { class: "muted small" }, "Sign in or make an account and you'll go straight in.")),
  );
})();

// Signed in (or just signed up) with an invite waiting: join it before the
// app loads, and open that outhouse first.
const inviteBaseStart = start;
start = async function () {
  const code = pendingInvite();
  if (code && state.token) {
    try { sessionStorage.removeItem(PENDING_INVITE); } catch { /* storage blocked */ }
    try {
      const g = await api("POST", `/api/invites/${encodeURIComponent(code)}`);
      localStorage.setItem("outhouse_guild", g.id);
      setTimeout(() => toast(`Welcome to ${g.name}! Pull up a seat.`), 1200);
    } catch (err) {
      if (!state.token) {
        // The saved sign-in had run out: keep the invite for after signing in.
        try { sessionStorage.setItem(PENDING_INVITE, code); } catch { /* storage blocked */ }
        return;
      }
      setTimeout(() => toast(err.message === "Not found." ? "That invite has expired or been revoked." : err.message), 1200);
    }
    document.querySelector(".auth-invite")?.remove();
  }
  return inviteBaseStart.apply(this, arguments);
};
