// Site staff (api/staff.rs): the site owner's Staff page (add, permissions,
// remove, two-person rule and badge settings), Staff activity, the
// Approvals page (two-person rule) and the note staff see when their powers
// are paused. Loaded after safety.js, outhouses.js and profiles.js; uses
// their helpers (sCan, sOwner, scWhen, scPlural, scSection, logView, …).

const STAFF_TIER_TEXT = {
  support: "Account basics in People, password-reset links and removing two-factor for someone locked out, and report status (no evidence). No bans, suspensions or IP addresses.",
  moderator: "Everything Support has, plus the Reports inbox with what was reported, hiding messages, warnings, forced username changes, timeouts, suspensions up to 7 days, a read-only look inside an outhouse with an open report, image-detector holds and same-IP hints (never the addresses).",
  senior: "Everything a Moderator has, plus permanent suspensions, IP addresses and Blocked networks, shutting down outhouses, child-safety reports (never files), family outhouses and staff-assisted ownership transfers.",
  custom: "Tick exactly the permissions this person gets.",
};
const STAFF_OWNER_ONLY = "Always yours alone: police hand-over and evidence downloads, deleting evidence, deleting outhouses for good, managing staff, Server status, the image detector's settings, email tests, backups, Teen protections and the whole safety log.";
const STAFF_ACTION_LABEL = {
  warn: "Warned", force_rename: "Forced a username change", timeout: "Timed out", kick: "Kicked", ban: "Banned", suspend: "Suspended", unsuspend: "Lifted a suspension",
  hide_messages: "Hid messages", unhide_messages: "Showed messages again", delete_message: "Deleted a message", remove_attachment: "Removed files",
  note: "Note", review: "Marked in review", close: "Closed a report", dismiss: "Dismissed a report", reopen: "Reopened a report",
  view_ips: "Looked at IP addresses", view_same_ip: "Checked same-IP hints", ip_block: "Blocked networks", ip_unblock: "Removed a block", ip_block_extend: "Changed a block",
  outhouse_view: "Looked inside an outhouse", outhouse_shut_down: "Shut down an outhouse", outhouse_restore: "Restored an outhouse", family: "Family outhouse",
  reset_link: "Made a reset link", "2fa_reset": "Removed two-factor", approval_requested: "Asked for approval", approval_given: "Approved a request",
  approval_declined: "Declined a request", approval_cancelled: "Cancelled a request", staff_cap: "Hit the hourly limit",
  transfer_started: "Started a transfer", transfer_cancelled: "Cancelled a transfer", approve_file: "Approved a picture", keep_blurred: "Kept a picture blurred", remove_file: "Removed a picture",
};

const staffTierChip = (s) => el("span", { class: `rp-chip staff-tier tier-${s.tier}` }, s.tier_label || s.tier);
const staffYesNo = (ok, yes, no) => el("span", { class: `staff-check ${ok ? "ok" : "bad"}` }, ok ? `✓ ${yes}` : `✗ ${no}`);

// The tier picker and the permission checklist (shared by add and edit).
function staffPermPicker(catalogue, { tier = "moderator", perms = null } = {}) {
  const boxes = new Map();
  let current = tier;
  const tiers = [...catalogue.tiers, { key: "custom", label: "Custom", perms: [] }];
  const desc = el("p", { class: "muted small staff-tier-desc" });
  const radios = el("div", { class: "staff-tiers", role: "radiogroup", "aria-label": "Tier" });
  const sync = () => {
    radios.querySelectorAll(".staff-tier-opt").forEach((b) => {
      const on = b.dataset.tier === current;
      b.classList.toggle("active", on);
      b.setAttribute("aria-checked", String(on));
    });
    desc.textContent = STAFF_TIER_TEXT[current] || "";
    const preset = tiers.find((t) => t.key === current)?.perms;
    for (const [key, box] of boxes) {
      if (current !== "custom") box.checked = preset.includes(key);
      box.closest(".staff-perm").classList.toggle("on", box.checked);
    }
  };
  for (const t of tiers) {
    radios.append(el("button", { type: "button", class: "staff-tier-opt", role: "radio", "data-tier": t.key, onclick: () => { current = t.key; sync(); } }, t.label));
  }
  const groups = el("div", { class: "staff-perm-groups" });
  for (const [tierKey, tierLabel] of [["support", "Support"], ["moderator", "Moderator"], ["senior", "Senior moderator"]]) {
    const list = catalogue.perms.filter((p) => p.tier === tierKey);
    groups.append(el("fieldset", { class: "staff-perm-group" },
      el("legend", {}, tierLabel),
      list.map((p) => {
        const box = el("input", { type: "checkbox", value: p.key, checked: !!perms?.includes(p.key) });
        box.addEventListener("change", () => { current = "custom"; sync(); });
        boxes.set(p.key, box);
        return el("label", { class: "staff-perm" }, box, el("span", {}, p.label));
      })));
  }
  const node = el("div", { class: "staff-picker" }, radios, desc, groups,
    el("p", { class: "muted small staff-owner-only" }, STAFF_OWNER_ONLY));
  sync();
  return {
    node,
    value: () => ({ tier: current, perms: [...boxes].filter(([, b]) => b.checked).map(([k]) => k) }),
  };
}

function staffAddDialog(data, done) {
  const name = el("input", { type: "text", class: "staff-username", placeholder: "Their username", autocomplete: "off", maxlength: 40 });
  const card = el("div", { class: "staff-candidate", "aria-live": "polite" },
    el("p", { class: "muted small" }, "Type a username. Staff need a confirmed email and two-factor sign-in."));
  const picker = staffPermPicker(data.catalogue, { tier: "moderator" });
  let candidate = null;
  let seq = 0;
  const submitBtn = () => dialog.querySelector("button[type=submit]");
  const check = async () => {
    const mine = ++seq;
    const u = name.value.trim().replace(/^@/, "");
    candidate = null;
    if (submitBtn()) submitBtn().disabled = true;
    if (!u) { card.replaceChildren(el("p", { class: "muted small" }, "Type a username. Staff need a confirmed email and two-factor sign-in.")); return; }
    const c = await api("GET", `/api/safety/staff/candidate?username=${encodeURIComponent(u)}`).catch((e) => ({ error: e.message }));
    if (mine !== seq || !dialog.isConnected) return;
    if (c.error) { card.replaceChildren(el("p", { class: "muted small" }, c.error)); return; }
    candidate = c;
    card.replaceChildren(
      el("div", { class: "staff-cand-who" }, avatarFor(c.id, c.display_name, 36), el("div", {}, el("strong", {}, c.display_name), el("div", { class: "muted small" }, `@${c.username}`))),
      el("div", { class: "staff-cand-checks" },
        staffYesNo(c.email_verified, "Email confirmed", "Email not confirmed"),
        staffYesNo(c.two_factor, "Two-factor on", "Two-factor off")),
      c.problem ? el("p", { class: "staff-cand-problem" }, c.problem) : el("p", { class: "staff-cand-ok small" }, "They can be staff."));
    if (submitBtn()) submitBtn().disabled = !c.ok;
  };
  let timer = null;
  name.addEventListener("input", () => { clearTimeout(timer); timer = setTimeout(check, 250); });
  const dialog = acctDialog("Add staff", [
    el("label", { class: "field" }, "Username", name),
    card,
    el("h3", { class: "staff-sub" }, "What they can do"),
    picker.node,
  ], "Add to staff", async () => {
    if (!candidate?.ok) throw new Error(candidate?.problem || "Pick someone who can be staff first.");
    const v = picker.value();
    await api("POST", "/api/safety/staff", { username: candidate.username, tier: v.tier, perms: v.perms });
    toast(`${candidate.display_name} is now staff. They've been emailed.`);
    done();
    return true;
  }, { cls: "staff-dialog" });
  submitBtn().disabled = true;
  return dialog;
}

function staffEditDialog(s, data, done) {
  const picker = staffPermPicker(data.catalogue, { tier: s.tier, perms: s.perms });
  return acctDialog(`${s.display_name}'s permissions`, [picker.node], "Save", async () => {
    const v = picker.value();
    await api("PUT", `/api/safety/staff/${s.id}`, v);
    toast("Saved.");
    done();
    return true;
  }, { cls: "staff-dialog" });
}

async function drawStaffPage(page, { stillMine, draw }) {
  const data = await tryApi("GET", "/api/safety/staff").catch(() => null);
  if (!data || !stillMine()) return;
  const redraw = () => draw("staff");
  const st = data.settings;
  const setting = async (body) => {
    const saved = await tryApi("PUT", "/api/safety/staff-settings", body).catch(() => null);
    if (saved) { Object.assign(st, saved); toast("Saved."); }
    return saved;
  };
  const toggle = (key, title, descr, extra = null) => {
    const btn = el("button", { type: "button", class: `toggle${st[key] ? " on" : ""}`, "aria-label": title, "aria-pressed": String(!!st[key]), onclick: async () => {
      const saved = await setting({ [key]: !st[key] });
      if (!saved) return;
      btn.classList.toggle("on", !!st[key]);
      btn.setAttribute("aria-pressed", String(!!st[key]));
    } });
    return el("div", { class: "sc-toggle-row" }, el("div", {}, el("div", { class: "sc-toggle-title" }, title), el("div", { class: "muted small" }, descr), extra), btn);
  };
  const members = el("input", { type: "number", min: 1, max: 100000, class: "staff-members-input", value: st.two_person_members, "aria-label": "Members" });
  members.addEventListener("change", () => setting({ two_person_members: Number(members.value) || 25 }));
  const row = (s) => el("div", { class: `staff-row${s.paused ? " paused" : ""}`, "data-staff": s.id },
    el("div", { class: "staff-row-who" }, avatarFor(s.id, s.display_name, 36),
      el("div", { class: "staff-row-names" },
        el("div", {}, el("strong", {}, s.display_name), el("span", { class: "muted" }, ` @${s.username}`), " ", staffTierChip(s)),
        el("div", { class: "muted small" }, `Last active ${s.last_active_at ? scWhen(s.last_active_at) : "not yet"} · ${scPlural(s.actions_week, "action")} this week · added ${new Date(s.added_at).toLocaleDateString()}`),
        s.paused ? el("div", { class: "staff-paused small" }, `⏸ Powers paused: ${s.paused_reason}`) : null,
        s.tier === "custom" ? el("div", { class: "muted small staff-perm-list" }, s.perms.join(" · ")) : null)),
    el("div", { class: "staff-row-checks" },
      staffYesNo(s.email_verified, "Email", "Email"),
      staffYesNo(s.two_factor, "2FA", "2FA")),
    el("div", { class: "staff-row-btns" },
      el("button", { type: "button", class: "ghost", onclick: () => draw("staffact", { staffId: s.id }) }, "Activity"),
      el("button", { type: "button", class: "ghost staff-edit", onclick: () => staffEditDialog(s, data, redraw) }, "Permissions…"),
      el("button", { type: "button", class: "ghost danger-btn staff-remove", onclick: async () => {
        if (!(await confirmBox(`Remove ${s.display_name} from staff?`, "Straight away. They keep their account and stay signed in as a normal member. They get an email, and it's written to the safety log.", "Remove from staff"))) return;
        if (await tryApi("DELETE", `/api/safety/staff/${s.id}`).catch(() => null)) { toast(`${s.display_name} is no longer staff.`); redraw(); }
      } }, "Remove")));
  page.replaceChildren(
    el("h2", {}, "Staff"),
    el("p", { class: "muted small sc-tight" }, "People you trust to help with safety, separate from outhouse roles. They need a confirmed email and two-factor sign-in, and their powers only work after signing in with their code. Everything they do is in the safety log with their name and the permission they used."),
    el("div", { class: "staff-top" },
      el("button", { type: "button", class: "primary staff-add-btn", onclick: () => staffAddDialog(data, redraw) }, "Add staff…"),
      data.pending_requests ? el("button", { type: "button", class: "ghost staff-pending-btn", onclick: () => draw("approvals") }, `${scPlural(data.pending_requests, "request")} waiting for approval`) : null),
    data.staff.length ? el("div", { class: "staff-list" }, data.staff.map(row))
      : el("div", { class: "sp-empty" }, el("div", { class: "sp-empty-icon" }, "🧻"), el("p", { class: "muted" }, "No staff yet. It's just you minding the dunny.")),
    el("h3", {}, "Tiers"),
    el("div", { class: "staff-tier-cards" }, ["support", "moderator", "senior"].map((t) => el("div", { class: `staff-tier-card tier-${t}` },
      el("strong", {}, ({ support: "Support", moderator: "Moderator", senior: "Senior moderator" })[t]), el("p", { class: "muted small" }, STAFF_TIER_TEXT[t])))),
    el("p", { class: "muted small sc-tight" }, STAFF_OWNER_ONLY),
    el("h3", {}, "Safeguards"),
    el("p", { class: "muted small sc-tight" }, `Hourly limits per staff member: ${data.catalogue.caps.normal} warnings, timeouts, suspensions and hidden messages; ${data.catalogue.caps.heavy} permanent suspensions, IP blocks, shut-downs and transfers. Hitting one blocks more and alerts you. Staff can't act on reports about themselves, reports they made, outhouses they own or moderate, your account, or staff at their level or above.`),
    toggle("two_person_permanent_ip", "Two-person rule: permanent suspension with an IP block", "A senior moderator's block waits until another senior moderator or you approve it (72 hours). “Suspend now” on child-safety reports never waits."),
    toggle("two_person_shut_down", "Two-person rule: shutting down a big outhouse", "Shutting down an outhouse with this many members or more waits for a second person.",
      el("label", { class: "staff-members-row small" }, members, " members or more")),
    toggle("staff_badge", "Show a “Staff” badge on staff profiles", "Off by default, for their privacy and safety. Official notices (warnings, suspensions, shut-downs) always say “Outhouse staff”, never the person."));
}

async function drawStaffActivity(page, { stillMine, openUser, draw, staffId = null }) {
  const staffList = await tryApi("GET", "/api/safety/staff").catch(() => null);
  if (!staffList || !stillMine()) return;
  const who = el("select", { class: "select", "aria-label": "Staff member" }, el("option", { value: "" }, "Everyone"),
    staffList.staff.map((s) => el("option", { value: s.id, selected: s.id === staffId }, `${s.display_name} (@${s.username})`)));
  const what = el("select", { class: "select", "aria-label": "Action" }, el("option", { value: "" }, "Any action"));
  const list = el("div", { class: "staff-activity" });
  const more = el("button", { type: "button", class: "ghost", hidden: true }, "Older");
  let before = null;
  let actionsFilled = false;
  const row = (e) => el("div", { class: "staff-act-row" },
    el("div", { class: "staff-act-top" },
      el("strong", {}, e.actor_name || `@${e.actor_username}`), " ",
      el("span", { class: `rp-chip staff-tier tier-${e.tier}` }, e.tier_label || e.tier),
      el("span", { class: "staff-act-what" }, ` ${STAFF_ACTION_LABEL[e.action] || e.action}`),
      e.perm ? el("code", { class: "staff-perm-code", title: "Permission used" }, e.perm) : null,
      el("span", { class: "sc-when" }, scWhen(e.created_at))),
    e.details ? el("div", { class: "muted small" }, e.details) : null,
    el("div", { class: "staff-act-links small" },
      e.target_user_id ? el("button", { type: "button", class: "link", onclick: () => openUser(e.target_user_id) }, `About ${e.target_name || "someone"}`) : null,
      e.report_id ? el("button", { type: "button", class: "link", onclick: () => draw("open", { reportId: e.report_id }) }, "Open report") : null,
      e.guild_id ? el("button", { type: "button", class: "link", onclick: () => draw("outhouses", { outhouseId: e.guild_id }) }, "Open outhouse") : null));
  const load = async (reset) => {
    if (reset) { before = null; list.replaceChildren(); }
    const q = new URLSearchParams();
    if (who.value) q.set("staff", who.value);
    if (what.value) q.set("action", what.value);
    if (before) q.set("before", before);
    const d = await tryApi("GET", `/api/safety/staff/activity?${q}`).catch(() => null);
    if (!d || !page.isConnected) return;
    if (!actionsFilled) {
      actionsFilled = true;
      what.append(...d.actions.map((a) => el("option", { value: a }, STAFF_ACTION_LABEL[a] || a)));
    }
    list.append(...d.entries.map(row));
    if (!list.children.length) list.append(el("p", { class: "muted" }, "Nothing yet."));
    before = d.entries.at(-1)?.id || null;
    more.hidden = !d.has_more;
  };
  who.addEventListener("change", () => load(true));
  what.addEventListener("change", () => load(true));
  more.addEventListener("click", () => load(false));
  page.replaceChildren(
    el("h2", {}, "Staff activity"),
    el("p", { class: "muted small sc-tight" }, "Everything staff did, newest first: who, as which tier, with which permission. Staff can't change or delete these entries. A weekly summary is in your safety digest."),
    el("div", { class: "dir-tools staff-filters" }, who, what),
    list, more);
  await load(true);
}

const REQ_STATUS = { pending: "Waiting", approved: "Approved", declined: "Declined", expired: "Expired", cancelled: "Cancelled", failed: "Couldn't be done" };

async function drawApprovals(page, { stillMine, openUser, draw }) {
  const d = await tryApi("GET", "/api/safety/staff-requests").catch(() => null);
  if (!d || !stillMine()) return;
  const redraw = () => draw("approvals");
  const decide = async (r, approve) => {
    const note = el("input", { type: "text", maxlength: 500, placeholder: approve ? "Optional note" : "Why (optional, they'll see it in the log)" });
    acctDialog(`${approve ? "Approve" : "Decline"}: ${r.summary}?`, [
      el("p", { class: "muted small acct-tight" }, approve ? `It's carried out now, as ${r.requester_name}'s action, and your approval is logged.` : "Nothing happens; they're told it was declined."),
      el("label", { class: "field" }, "Note", note),
    ], approve ? "Approve" : "Decline", async () => {
      await api("POST", `/api/safety/staff-requests/${r.id}/${approve ? "approve" : "decline"}`, { note: note.value.trim() });
      toast(approve ? "Approved and done." : "Declined.");
      redraw();
      return true;
    }, { danger: approve });
  };
  const card = (r) => el("div", { class: `staff-req st-${r.status}`, "data-request": r.id },
    el("div", { class: "staff-req-top" },
      el("span", { class: "rp-chip" }, r.kind_label),
      el("span", { class: `rp-chip req-${r.status}` }, REQ_STATUS[r.status] || r.status),
      el("span", { class: "sc-when" }, r.status === "pending" ? `ends ${scWhen(r.expires_at)}` : scWhen(r.decided_at || r.created_at))),
    el("div", { class: "staff-req-what" }, el("strong", {}, r.requester_name), ` asks to ${r.summary}.`),
    r.reason ? el("div", { class: "muted small" }, `Reason: ${r.reason}`) : null,
    el("div", { class: "staff-act-links small" },
      r.target_user_id && sCan("people") ? el("button", { type: "button", class: "link", onclick: () => openUser(r.target_user_id) }, `Open ${r.target_name || "them"} in People`) : null,
      r.report_id ? el("button", { type: "button", class: "link", onclick: () => draw("open", { reportId: r.report_id }) }, "Open report") : null,
      r.guild_id ? el("button", { type: "button", class: "link", onclick: () => draw("outhouses", { outhouseId: r.guild_id }) }, `Open ${r.guild_name || "outhouse"}`) : null),
    r.status !== "pending" ? el("div", { class: "muted small" }, [r.decided_by_name ? `By ${r.decided_by_name}` : null, r.decision_note ? `“${r.decision_note}”` : null, r.result || null].filter(Boolean).join(" · ")) : null,
    r.status === "pending" ? el("div", { class: "sc-actions" },
      r.can_decide ? el("button", { type: "button", class: "primary danger-fill staff-approve", onclick: () => decide(r, true) }, "Approve") : null,
      r.can_decide ? el("button", { type: "button", class: "ghost staff-decline", onclick: () => decide(r, false) }, "Decline") : null,
      r.mine ? el("span", { class: "muted small" }, "Waiting for another senior moderator or the site owner.") : null,
      r.mine || sOwner() ? el("button", { type: "button", class: "ghost", onclick: async () => {
        if (await tryApi("POST", `/api/safety/staff-requests/${r.id}/cancel`).catch(() => null)) { toast("Cancelled."); redraw(); }
      } }, "Cancel request") : null) : null);
  page.replaceChildren(
    el("h2", {}, "Approvals"),
    el("p", { class: "muted small sc-tight" }, `The two-person rule: a senior moderator's permanent suspension with an IP block, or shutting down a big outhouse, waits here until someone else approves it. Nobody can approve their own. Requests run out after ${d.hours} hours.`),
    d.pending.length ? el("div", { class: "staff-reqs" }, d.pending.map(card)) : el("div", { class: "sp-empty" }, el("p", { class: "muted" }, "Nothing waiting for approval.")),
    d.done.length ? el("h3", {}, "Decided") : null,
    d.done.length ? el("div", { class: "staff-reqs done" }, d.done.map(card)) : null);
}

// A staff member whose powers can't be used right now.
function drawStaffNote(page) {
  const s = state.safety.staff || {};
  page.replaceChildren(
    el("h2", {}, "Staff"),
    el("div", { class: "sc-urgent-banner staff-note", role: "status" },
      el("strong", {}, s.paused_reason ? "Your staff powers are paused." : "Sign in with your code to use staff powers."),
      el("p", { class: "sc-tight" }, s.paused_reason
        ? `Because ${s.paused_reason}. They come back on their own once that's fixed (User settings → My account). The site owner has been told.`
        : "This sign-in is from before two-factor was on, so staff tools stay locked. Sign out and sign in again with the code from your authenticator app.")),
    s.paused_reason ? null : el("button", { type: "button", class: "primary", onclick: () => signOut() }, "Sign out now"),
    el("p", { class: "muted small sc-tight" }, `Your staff tier: ${s.tier_label || "Custom"}. Everything staff do is logged.`));
}

// The small "Staff" badge on profiles (only when the site owner turned it on).
if (typeof profileHeader === "function") {
  const staffBaseProfileHeader = profileHeader;
  profileHeader = function (p, ...rest) {
    const head = staffBaseProfileHeader(p, ...rest);
    if (p?.staff_badge) head.querySelector(".fp-display")?.append(el("span", { class: "staff-badge", title: "Outhouse staff" }, "Staff"));
    return head;
  };
}

// The Approvals page refreshes itself when a request changes.
const staffBaseHandle = handleEvent;
handleEvent = function (ev) {
  staffBaseHandle(ev);
  if (ev.t === "STAFF_CHANGED") {
    const open = document.querySelector("dialog.user-settings[open]");
    if (open?._draw && open.dataset.tab === "approvals") open._draw("approvals");
  }
};
