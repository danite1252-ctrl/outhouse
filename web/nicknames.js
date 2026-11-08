// Manage Nicknames: change someone else's nickname in an outhouse (from
// the member right-click / long-press menu or their profile card), plus
// your own from the same places. Loaded after app.js and features.js.
"use strict";

// May I change this member's nickname here? (Hierarchy-aware, like kicks.)
function canRenameMember(m) {
  if (!m || !state.guildId) return false;
  if (m.id === state.me.id) return can("change_nickname") || can("manage_nicknames");
  return canModerate(m, "manage_nicknames");
}

// Right-click menu entry (app.js adds whatever this returns).
function nicknameMenuItems(m) {
  return canRenameMember(m) ? [{ label: "Change nickname", onClick: () => openNicknameDialog(m) }] : [];
}

// The little dialog: their nickname, their display name as the placeholder.
async function openNicknameDialog(m, accountName) {
  const guildId = state.guildId;
  if (!accountName) {
    const p = await api("GET", `/api/profiles/${m.id}`).catch(() => null);
    accountName = p?.display_name ?? m.display_name;
  }
  const self = m.id === state.me.id;
  const current = m.display_name !== accountName ? m.display_name : "";
  const input = el("input", { type: "text", maxlength: 32, value: current, placeholder: accountName, "aria-label": "Nickname" });
  const save = async (nickname) => {
    const path = self ? `/api/guilds/${guildId}/nickname` : `/api/guilds/${guildId}/members/${m.id}/nickname`;
    const member = await tryApi("PUT", path, { nickname }).catch(() => null);
    if (!member) return false;
    toast(nickname && nickname.trim()
      ? `${self ? "You're" : `${accountName} is`} now "${member.display_name}" here.`
      : `${self ? "Your" : `${accountName}'s`} nickname is cleared.`);
    if (state.guildId === guildId) refreshGuild();
    return true;
  };
  const dialog = el("dialog", { class: "nick-dialog" },
    el("form", { method: "dialog", onsubmit: async (e) => { e.preventDefault(); if (await save(input.value)) dialog.close(); } },
      el("h2", {}, self ? "Change your nickname" : `Change ${accountName}'s nickname`),
      el("p", { class: "muted small", style: "margin:0" }, self
        ? "Only in this outhouse. Leave it blank to use your display name."
        : "Only in this outhouse. Leave it blank to go back to their display name."),
      input,
      el("div", { class: "row" },
        el("button", { type: "button", class: "ghost nick-reset", disabled: !current, onclick: async () => { if (await save(null)) dialog.close(); } }, "Reset nickname"),
        el("button", { type: "button", class: "ghost", onclick: () => dialog.close() }, "Cancel"),
        el("button", { type: "submit", class: "primary nick-save" }, "Save"),
      ),
    ),
  );
  dialog.addEventListener("close", () => dialog.remove());
  document.body.append(dialog);
  dialog.showModal();
  input.focus();
  input.select();
}

// A "Change nickname" button on profile cards (when you're allowed).
const nickBaseProfileCard = profileCard;
profileCard = function (p, opts = {}) {
  const card = nickBaseProfileCard(p, opts);
  if (opts.preview) return card;
  const m = memberById(p.id);
  if (!canRenameMember(m)) return card;
  card.querySelector(".pc-actions")?.append(
    el("button", { class: "ghost pc-nick", onclick: () => { closePopover(); openNicknameDialog(m, p.display_name); } }, "Change nickname"));
  return card;
};
