// Usernames (api/names.rs): people can't change their own. Staff with
// "Force a username change" (or the site owner) can, when one breaks the
// rules; the person then picks a new one before they can use Outhouse.
// This file has: the staff dialog (People and reports), the blocking
// "choose a new username" dialog, live USER_UPDATE handling, and the site
// owner's Usernames page (blocked words). Loaded after safety.js and staff.js.

const RENAME_REASONS = [
  ["inappropriate", "Inappropriate"],
  ["offensive", "Offensive"],
  ["impersonation", "Impersonation"],
  ["other", "Other"],
];

// ---------- staff: "Force username change…" ----------

function forceRenameDialog(u, act, ctx = null) {
  let reason = "inappropriate";
  const chips = el("div", { class: "rn-reasons", role: "radiogroup", "aria-label": "Reason" });
  const sync = () => chips.querySelectorAll("button").forEach((b) => {
    const on = b.dataset.reason === reason;
    b.classList.toggle("active", on);
    b.setAttribute("aria-checked", String(on));
  });
  for (const [key, label] of RENAME_REASONS) {
    chips.append(el("button", { type: "button", class: "rn-reason", role: "radio", "data-reason": key, onclick: () => { reason = key; sync(); } }, label));
  }
  sync();
  const text = el("textarea", { class: "rn-text", rows: 2, maxlength: 300, placeholder: "More detail for the safety log (needed for “Other”)" });
  // Their display name is usually their username: offer to reset it too.
  const same = (u.display_name || "").trim().toLowerCase() === (u.username || "").toLowerCase();
  const reset = el("input", { type: "checkbox", class: "rn-reset", checked: same });
  return acctDialog(`Force a username change for @${u.username}?`, [
    el("p", { class: "muted small rn-tight" },
      "Their username is replaced straight away with a neutral one (like “renamed-7f3k2”), everywhere. ",
      el("strong", {}, `@${u.username}`), " is reserved for good: nobody can use it again. They must pick a new username before they can use Outhouse, and Outhouse Safety tells them why (never who did it)."),
    el("div", { class: "field" }, "Reason", chips),
    el("label", { class: "field" }, "Details", text),
    el("label", { class: "rp-check rn-reset-row" }, reset,
      el("span", {}, "Also reset their display name ", el("span", { class: "muted" }, `(now “${u.display_name}”)`))),
    el("p", { class: "muted small rn-tight" }, "Their outhouse nicknames aren't touched. This is written to the safety log with your name."),
  ], "Change username", async () => {
    if (reason === "other" && !text.value.trim()) throw new Error("Say why (for “Other”).");
    const body = { action: "force_rename", reason, text: text.value.trim(), reset_display_name: reset.checked };
    const done = "Username changed. They'll be asked to choose a new one.";
    if (!ctx) { await act(body, done); return true; }
    await ctx.post(body);
    toast(done);
    await ctx.refresh();
    return true;
  }, { cls: "rn-dialog", danger: true });
}

// ---------- the person: choose a new username (blocking) ----------

let renameModalOpen = false;

// On app open: are they waiting to pick a username? (true = the dialog is up)
async function checkMustChooseUsername() {
  const me = await api("GET", "/api/users/@me").catch(() => null);
  if (!me?.must_choose_username) return false;
  showChooseUsername(me);
  return true;
}

// A request came back 403 must_choose_username (staff changed it just now).
function onMustChooseUsername() {
  if (renameModalOpen) return;
  api("GET", "/api/users/@me").then((me) => { if (me?.must_choose_username) showChooseUsername(me); }).catch(() => {});
}

function showChooseUsername(me) {
  if (renameModalOpen) return;
  renameModalOpen = true;
  document.querySelectorAll("dialog[open]").forEach((d) => { if (!d.classList.contains("rn-choose")) d.close(); });
  const input = el("input", { type: "text", class: "rn-input", maxlength: 32, autocomplete: "username", autocapitalize: "none", spellcheck: "false", placeholder: "new.username", "aria-label": "New username" });
  const status = el("p", { class: "rn-status small", "aria-live": "polite" }, me.username_rules || "Usernames are 2–32 characters: letters, numbers, dots and underscores.");
  const submit = el("button", { type: "submit", class: "primary rn-save", disabled: true }, "Use this username");
  const error = el("p", { class: "error", role: "alert" });
  let seq = 0;
  let good = null;
  const check = async () => {
    const mine = ++seq;
    const name = input.value.trim().toLowerCase();
    good = null;
    submit.disabled = true;
    status.className = "rn-status small";
    if (!name) { status.textContent = me.username_rules || ""; return; }
    if (!/^[a-z0-9_.]{2,32}$/.test(name)) {
      status.textContent = "Letters, numbers, dots and underscores only, 2–32 characters.";
      status.classList.add("bad");
      return;
    }
    status.textContent = "Checking…";
    const r = await api("GET", `/api/users/@me/username/check?username=${encodeURIComponent(name)}`).catch((e) => ({ ok: false, error: e.message }));
    if (mine !== seq) return;
    if (r.ok) {
      good = r.username;
      status.textContent = `✓ @${r.username} is available.`;
      status.classList.add("ok");
      submit.disabled = false;
    } else {
      status.textContent = r.error || "That username can't be used.";
      status.classList.add("bad");
    }
  };
  let timer = null;
  input.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(check, 250); });
  const reason = me.forced_rename_reason ? ` (reason: ${me.forced_rename_reason})` : "";
  const dialog = el("dialog", { class: "rn-choose", "aria-labelledby": "rn-title" },
    el("form", { method: "dialog" },
      el("div", { class: "rn-icon", "aria-hidden": "true" }, "✏️"),
      el("h2", { id: "rn-title" }, "Choose a new username"),
      el("p", { class: "rn-why" }, `Your username was changed by Outhouse staff because it broke the rules${reason}. Choose a new one.`),
      el("p", { class: "muted small rn-tight" }, `For now you're @${me.username}. You get one pick, so choose something you're happy to keep: usernames can't be changed later. Your display name and outhouse nicknames are separate.`),
      el("label", { class: "field" }, "New username", el("div", { class: "rn-at" }, el("span", { "aria-hidden": "true" }, "@"), input)),
      status,
      error,
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost rn-signout", onclick: () => { dialog.close(); renameModalOpen = false; signOut(); } }, "Sign out"),
        submit)));
  dialog.addEventListener("cancel", (e) => e.preventDefault());
  dialog.querySelector("form").addEventListener("submit", async (e) => {
    e.preventDefault();
    if (!good) return;
    error.textContent = "";
    submit.disabled = true;
    try {
      const user = await api("POST", "/api/users/@me/username", { username: good });
      dialog.close();
      dialog.remove();
      renameModalOpen = false;
      state.me = { ...(state.me || {}), ...user };
      toast(`You're @${user.username} now.`);
      // Signed in just now: start the app. Already running: start it again
      // (anything that was refused while waiting is loaded fresh).
      if (state.ws) location.reload();
      else connect();
    } catch (err) {
      error.textContent = err.message;
      check();
    }
  });
  document.body.append(dialog);
  dialog.showModal();
  input.focus();
}

// ---------- live name changes ----------

const namesBaseHandle = handleEvent;
handleEvent = function (ev) {
  namesBaseHandle(ev);
  if (ev.t !== "USER_UPDATE" || !ev.d) return;
  const u = ev.d;
  if (state.me && u.id === state.me.id) {
    Object.assign(state.me, { username: u.username, display_name: u.display_name });
    if (typeof renderMe === "function") renderMe();
    if (u.username.startsWith("renamed-")) onMustChooseUsername();
  }
  let changed = false;
  for (const m of state.members || []) {
    if (m.id === u.id && m.username !== u.username) { m.username = u.username; changed = true; }
  }
  for (const m of state.messages || []) {
    if (m.author_id === u.id && m.author_username !== u.username) { m.author_username = u.username; changed = true; }
  }
  if (changed) {
    if (typeof renderMembers === "function") renderMembers();
    if (typeof renderMessages === "function") renderMessages();
  }
};

// ---------- site owner: Usernames (blocked words) ----------

async function drawUsernameWords(page, { stillMine, draw }) {
  const d = await tryApi("GET", "/api/safety/username-words").catch(() => null);
  if (!d || !stillMine()) return;
  const input = el("input", { type: "text", class: "rn-word-input", maxlength: 40, placeholder: "A word or part of one", "aria-label": "Blocked word" });
  const list = el("div", { class: "rn-words" });
  const count = el("p", { class: "muted small sc-tight rn-matching" });
  const paint = (data) => {
    list.replaceChildren(...(data.words.length
      ? data.words.map((w) => el("span", { class: "rn-word", "data-word": w.word }, w.word,
        el("button", { type: "button", class: "rn-word-x", "aria-label": `Unblock ${w.word}`, title: "Remove", onclick: async () => {
          const fresh = await tryApi("DELETE", "/api/safety/username-words", { word: w.word }).catch(() => null);
          if (fresh) { paint(fresh); toast(`“${w.word}” removed.`); }
        } }, "×")))
      : [el("p", { class: "muted small sc-tight" }, "No blocked words yet. Add words you never want in a username.")]));
    count.replaceChildren(data.matching_users
      ? el("span", {}, `${scPlural(data.matching_users, "existing account")} ${data.matching_users === 1 ? "has" : "have"} a username matching these. `,
        el("button", { type: "button", class: "link rn-review", onclick: () => draw("people", { peopleFilter: "blocked" }) }, "Review them in People"))
      : "No existing usernames match.");
  };
  const add = async () => {
    const word = input.value.trim();
    if (!word) return;
    const fresh = await tryApi("POST", "/api/safety/username-words", { word }).catch(() => null);
    if (fresh) { input.value = ""; paint(fresh); toast("Blocked word added."); }
  };
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); add(); } });
  page.replaceChildren(
    el("h2", {}, "Usernames"),
    el("p", { class: "muted small sc-tight" }, "People can't change their own username. If one breaks the rules, staff (or you) can force a change from People or a report: it's swapped for a neutral one at once, the old one is reserved for good, and they pick a new one."),
    el("h3", {}, "Blocked words"),
    el("p", { class: "muted small sc-tight" }, "New usernames containing any of these are refused, at sign-up and when someone picks a new one. Case, accents, dots and underscores don't matter, and simple swaps count (0→o, 1→i, 3→e, 4→a, 5→s, @→a, $→s). Existing accounts aren't changed."),
    el("div", { class: "inline-row rn-add-row" }, input, el("button", { type: "button", class: "primary rn-add", onclick: add }, "Add")),
    list,
    count);
  paint(d);
}
