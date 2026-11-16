#!/usr/bin/env bash
# Usernames (api/names.rs): nobody changes their own; staff with
# "force_rename" (or the site owner) force a change when one breaks the
# rules; the old name is reserved; the person is blocked until they pick a
# new one (once); the owner's blocked words. ci/smoke.sh runs it after
# ci/staff_test.sh; it can also run on its own:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=... MAIL_DIR=/tmp/outhouse-mail \
#   OWNER_TOKEN=<the site owner's token> bash ci/rename_test.sh
#
# The server needs SMTP_* pointing at ci/smtp_sink.py (MAIL_DIR),
# PUBLIC_URL=$BASE and AUTH_SECRET_KEY (staff need a confirmed email and
# two-factor), and STAFF_ACTIONS_PER_HOUR >= 8.
set -euo pipefail
set -E
trap 'echo "Rename test failed at line $LINENO" >&2' ERR

BASE=${BASE:-http://127.0.0.1:8080}
MAIL_DIR=${MAIL_DIR:-/tmp/outhouse-mail}
SB=/tmp/rename-body
RUN=${RENAME_RUN:-$(date +%s | tail -c 6)}
rstep() { echo "== rename: $*"; }

# sreq METHOD PATH [JSON] [TOKEN] [XFF] -> HTTP status (body in $SB)
sreq() {
  local method=$1 path=$2 body=${3:-} token=${4:-} xff=${5:-}
  local args=(-sS -X "$method" -o "$SB" -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  [ -n "$xff" ] && args+=(-H "X-Forwarded-For: $xff")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$BASE$path"
}
# expect STATUS METHOD PATH [JSON] [TOKEN] [XFF]
expect() {
  local want=$1; shift
  local got; got=$(sreq "$@")
  if [ "$got" != "$want" ]; then echo "EXPECTED $want, GOT $got: $1 $2" >&2; cat "$SB" >&2; echo >&2; return 1; fi
}
q() { psql "$DATABASE_URL" -tAc "$1"; }
mail_n() { find "$MAIL_DIR" -name '*.json' 2>/dev/null | wc -l; }
wait_mail() { # wait_mail TO SUBJECT_PART SINCE -> file
  local to=$1 subj=$2 since=$3 f n
  for _ in $(seq 1 60); do
    for f in $(find "$MAIL_DIR" -name '*.json' 2>/dev/null | sort); do
      n=$(basename "$f" .json); n=$((10#$n))
      [ "$n" -gt "$since" ] || continue
      if jq -e --arg to "$to" --arg s "$subj" '(.to | index($to)) and (.subject | contains($s))' "$f" >/dev/null; then
        echo "$f"; return 0
      fi
    done
    sleep 0.25
  done
  echo "no mail to $to about \"$subj\" (after #$since)" >&2; return 1
}
link_token() { jq -r .text "$1" | grep -oE 'token=[0-9a-f]{64}' | head -1 | cut -d= -f2; }
totp() { python3 ci/totp.py "$1" "${2:-0}"; }
uid_of() { q "SELECT id FROM users WHERE username = '$1'"; }
mail_of() { echo "$1@example.test"; }
xff() { echo "198.18.62.$((RANDOM % 200 + 1))"; }
# new_user NAME [CONFIRM=1] -> token
new_user() {
  local m0; m0=$(mail_n)
  expect 200 POST /api/auth/register "{\"username\":\"$1\",\"email\":\"$(mail_of "$1")\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\",\"display_name\":\"$1\"}" "" "$(xff)"
  local t; t=$(jq -r .token "$SB")
  if [ "${2:-0}" = 1 ]; then
    local f; f=$(wait_mail "$(mail_of "$1")" "Confirm your email" "$m0")
    expect 200 POST /api/auth/verify-email "{\"token\":\"$(link_token "$f")\"}" >/dev/null
  fi
  echo "$t"
}
two_factor() {
  expect 200 POST /api/auth/2fa/setup '{"password":"password123"}' "$1" >/dev/null
  local s; s=$(jq -r .secret "$SB")
  expect 200 POST /api/auth/2fa/confirm "{\"code\":\"$(totp "$s" -1)\"}" "$1" >/dev/null
  echo "$s"
}
staff() { # staff TOKEN NAME TIER [PERMS_JSON] (confirmed email + two-factor first)
  two_factor "$1" >/dev/null
  expect 200 POST /api/safety/staff "{\"username\":\"$2\",\"tier\":\"$3\",\"perms\":${4:-[]}}" "$OWNER_T"
}
dm_from_safety() {
  q "SELECT m.content FROM messages m JOIN dm_participants p ON p.channel_id = m.channel_id
     WHERE p.user_id = $1 AND m.author_id = (SELECT safety_bot_id FROM site_settings WHERE id = 1)
     ORDER BY m.id DESC LIMIT 1"
}
force() { # force STATUS TARGET_ID TOKEN [REASON] [TEXT] [RESET]
  expect "$1" POST "/api/safety/users/$2/actions" \
    "$(jq -nc --arg r "${4:-inappropriate}" --arg t "${5:-}" --argjson d "${6:-false}" '{action:"force_rename",reason:$r,text:$t,reset_display_name:$d}')" "$3"
}

OWNER_T=${OWNER_TOKEN:?OWNER_TOKEN is needed}
OWNER_ID=$(curl -fsS -H "Authorization: Bearer $OWNER_T" "$BASE/api/users/@me" | jq -r .id)

rstep "people: a moderator, Support, a senior, a custom force_rename-only, and some members"
MOD=$(new_user "rnmod$RUN" 1); MOD_ID=$(uid_of "rnmod$RUN"); staff "$MOD" "rnmod$RUN" moderator
SUP=$(new_user "rnsup$RUN" 1); SUP_ID=$(uid_of "rnsup$RUN"); staff "$SUP" "rnsup$RUN" support
SEN=$(new_user "rnsen$RUN" 1); SEN_ID=$(uid_of "rnsen$RUN"); staff "$SEN" "rnsen$RUN" senior
CUS=$(new_user "rncus$RUN" 1); CUS_ID=$(uid_of "rncus$RUN"); staff "$CUS" "rncus$RUN" custom '["people","force_rename"]'
BAD=$(new_user "rude.bad$RUN"); BAD_ID=$(uid_of "rude.bad$RUN")
PAL=$(new_user "rnpal$RUN"); PAL_ID=$(uid_of "rnpal$RUN")
OTHER=$(new_user "rnother$RUN"); OTHER_ID=$(uid_of "rnother$RUN")
OLD=$(new_user "nastyold$RUN"); OLD_ID=$(uid_of "nastyold$RUN")    # before the word is blocked
expect 200 GET /api/safety/staff "" "$OWNER_T"
jq -e '(.catalogue.perms | map(.key) | index("force_rename")) and
  (.catalogue.tiers[] | select(.key == "moderator") | .perms | index("force_rename")) and
  (.catalogue.tiers[] | select(.key == "senior") | .perms | index("force_rename")) and
  ((.catalogue.tiers[] | select(.key == "support") | .perms | index("force_rename")) | not)' "$SB" >/dev/null
# A shared outhouse (the profile check) with BAD and PAL.
expect 200 POST /api/guilds "{\"name\":\"Rename Shed $RUN\"}" "$PAL"; G=$(jq -r .id "$SB")
expect 200 POST "/api/guilds/$G/invites" "" "$PAL"; expect 200 POST "/api/invites/$(jq -r .code "$SB")" "" "$BAD"

rstep "nobody changes their own username"
expect 403 PATCH /api/users/@me '{"username":"newname'"$RUN"'"}' "$BAD"
grep -q "can't be changed" "$SB"
expect 403 PATCH /api/users/@me '{"username":"rnmod2'"$RUN"'","display_name":"x"}' "$MOD"
expect 403 POST /api/users/@me/username '{"username":"newname'"$RUN"'"}' "$BAD"
expect 403 GET "/api/users/@me/username/check?username=newname$RUN" "" "$BAD"
[ "$(q "SELECT username FROM users WHERE id = $BAD_ID")" = "rude.bad$RUN" ]
expect 200 PATCH /api/users/@me '{"display_name":"Rude Display"}' "$BAD"   # display names still change

rstep "Support can't force a change; staff can't act on the owner, equal or higher staff, or themselves"
force 403 "$BAD_ID" "$SUP"
force 403 "$OWNER_ID" "$MOD"
force 403 "$SEN_ID" "$MOD"
grep -q "your level" "$SB"
force 403 "$CUS_ID" "$MOD"            # custom with a moderator permission = same level
force 403 "$MOD_ID" "$MOD"            # themselves (their own level)
expect 200 POST "/api/safety/users/$SUP_ID/actions" '{"action":"note","text":"rename test"}' "$MOD"
force 400 "$BAD_ID" "$MOD" nonsense
force 400 "$BAD_ID" "$MOD" other ""   # "Other" needs a reason
grep -q "Other" "$SB"
[ "$(q "SELECT username FROM users WHERE id = $BAD_ID")" = "rude.bad$RUN" ]

rstep "a moderator forces a change: placeholder, reserved, history, DM without the staff name, safety log"
force 200 "$BAD_ID" "$MOD" offensive "Slur in the name" true
jq -e '.choosing_username and (.username | startswith("renamed-"))' "$SB" >/dev/null
PH=$(q "SELECT username FROM users WHERE id = $BAD_ID")
[[ "$PH" =~ ^renamed-[a-z0-9]{5}$ ]]
[ "$(q "SELECT must_choose_username FROM users WHERE id = $BAD_ID")" = t ]
[ "$(q "SELECT display_name FROM users WHERE id = $BAD_ID")" = "$PH" ]                       # display name reset too
[ "$(q "SELECT count(*) FROM reserved_usernames WHERE name_key = 'rude.bad$RUN' AND user_id = $BAD_ID AND reserved_by = $MOD_ID AND reason = 'Offensive: Slur in the name'")" = 1 ]
[ "$(q "SELECT count(*) FROM name_history WHERE user_id = $BAD_ID AND kind = 'username' AND old_value = 'rude.bad$RUN' AND new_value = '$PH' AND staff_only")" = 1 ]
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'force_rename' AND actor_id = $MOD_ID AND target_user_id = $BAD_ID
        AND staff_tier = 'moderator' AND staff_perm = 'force_rename' AND details LIKE '%@rude.bad$RUN%' AND details LIKE '%Offensive%'")" = 1 ]
DM=$(dm_from_safety "$BAD_ID")
[[ "$DM" == *"Your username was changed by Outhouse staff"* ]] && [[ "$DM" == *"(reason: Offensive)"* ]] && [[ "$DM" == *"choose a new one"* ]]
[[ "$DM" != *"rnmod$RUN"* ]] && [[ "$DM" != *"Slur"* ]]
# Staff and the owner see the old name; the public profile doesn't.
expect 200 GET "/api/safety/users/$BAD_ID" "" "$OWNER_T"
jq -e --arg o "rude.bad$RUN" 'any(.names[]; .kind == "username" and .old == $o)' "$SB" >/dev/null
expect 200 GET "/api/users/$BAD_ID/profile/full" "" "$PAL"
jq -e --arg o "rude.bad$RUN" --arg p "$PH" '(tostring | contains($o) | not) and .username == $p' "$SB" >/dev/null
force 400 "$BAD_ID" "$MOD"             # already choosing
grep -q "choosing" "$SB"

rstep "until they choose: every other request is 403 must_choose_username"
for path in /api/users/@me/unread /api/users/@me/relationships "/api/guilds/$G/members"; do
  expect 403 GET "$path" "" "$BAD"
  jq -e '.code == "must_choose_username" and (.error | test("changed by Outhouse staff"))' "$SB" >/dev/null
done
expect 403 POST "/api/guilds/$G/invites" "" "$BAD"
jq -e '.code == "must_choose_username"' "$SB" >/dev/null
expect 200 GET /api/users/@me "" "$BAD"
jq -e --arg p "$PH" '.must_choose_username and .username == $p and .forced_rename_reason == "Offensive" and (.username_rules | test("2–32"))' "$SB" >/dev/null
# Signing in: the old name (the one they know) still works, so does the placeholder.
expect 200 POST /api/auth/login "{\"username\":\"RUDE.BAD$RUN\",\"password\":\"password123\"}" "" "$(xff)"
BAD2=$(jq -r .token "$SB"); jq -e --arg p "$PH" '.user.username == $p' "$SB" >/dev/null
expect 200 POST /api/auth/login "{\"username\":\"$PH\",\"password\":\"password123\"}" "" "$(xff)"
expect 401 POST /api/auth/login "{\"username\":\"rude.bad$RUN\",\"password\":\"wrong-password\"}" "" "$(xff)"
expect 200 GET /api/users/@me "" "$PAL"
jq -e '.must_choose_username == false' "$SB" >/dev/null

rstep "the owner's blocked words (owner only), normalised"
expect 403 GET /api/safety/username-words "" "$MOD"
expect 403 POST /api/safety/username-words '{"word":"nasty"}' "$MOD"
expect 200 GET /api/safety/username-words "" "$OWNER_T"
expect 200 POST /api/safety/username-words '{"word":"N4$TY"}' "$OWNER_T"     # stored as "nasty"
jq -e 'any(.words[]; .word == "nasty") and .matching_users >= 1' "$SB" >/dev/null
expect 400 POST /api/safety/username-words '{"word":"x"}' "$OWNER_T"
expect 200 POST /api/safety/username-words '{"word":"zzgrot"}' "$OWNER_T"
expect 200 DELETE /api/safety/username-words '{"word":"ZZGROT"}' "$OWNER_T"
jq -e 'all(.words[]; .word != "zzgrot")' "$SB" >/dev/null
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'username_word' AND actor_id = $OWNER_ID AND details LIKE '%nasty%'")" -ge 1 ]

rstep "registration refuses reserved (any case) and blocked usernames"
expect 400 POST /api/auth/register "{\"username\":\"Rude.Bad$RUN\",\"email\":\"rx$RUN@example.test\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\"}" "" "$(xff)"
grep -q "isn't available" "$SB"
for name in "n4sty$RUN" "x_n.a_5_t.y$RUN" "NASTY$RUN"; do
  expect 400 POST /api/auth/register "{\"username\":\"$name\",\"email\":\"ry$RUN@example.test\",\"password\":\"password123\",\"birth_date\":\"1990-01-01\"}" "" "$(xff)"
  grep -q "isn't allowed" "$SB"
done
[ "$(q "SELECT count(*) FROM users WHERE email = 'ry$RUN@example.test' OR email = 'rx$RUN@example.test'")" = 0 ]

rstep "choosing: live check (rules, reserved any case, blocked, taken), then exactly once"
chk() { expect 200 GET "/api/users/@me/username/check?username=$1" "" "$BAD"; }
chk "a"; jq -e '(.ok | not) and (.error | test("2–32"))' "$SB" >/dev/null
chk "R%C3%BCde.bad$RUN"; jq -e '.ok | not' "$SB" >/dev/null                 # accents: not allowed characters
chk "RUDE.BAD$RUN"; jq -e '(.ok | not) and (.error | test("isn.t available"))' "$SB" >/dev/null
chk "s0_n4sty$RUN"; jq -e '(.ok | not) and (.error | test("isn.t allowed"))' "$SB" >/dev/null
chk "rnpal$RUN"; jq -e '(.ok | not) and (.error | test("taken"))' "$SB" >/dev/null
chk "Polite.Name$RUN"; jq -e --arg n "polite.name$RUN" '.ok and .username == $n' "$SB" >/dev/null
expect 400 POST /api/users/@me/username "{\"username\":\"rude.bad$RUN\"}" "$BAD"
expect 400 POST /api/users/@me/username "{\"username\":\"nasty1$RUN\"}" "$BAD"
expect 409 POST /api/users/@me/username "{\"username\":\"rnpal$RUN\"}" "$BAD"
expect 200 POST /api/users/@me/username "{\"username\":\"Polite.Name$RUN\"}" "$BAD"
jq -e --arg n "polite.name$RUN" '.username == $n and .display_name == $n' "$SB" >/dev/null   # the reset display name follows
[ "$(q "SELECT must_choose_username FROM users WHERE id = $BAD_ID")" = f ]
[ "$(q "SELECT count(*) FROM name_history WHERE user_id = $BAD_ID AND old_value = '$PH' AND staff_only")" -ge 1 ]
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'chose_username' AND target_user_id = $BAD_ID")" = 1 ]
expect 403 POST /api/users/@me/username "{\"username\":\"another$RUN\"}" "$BAD"              # once only
expect 403 POST /api/users/@me/username "{\"username\":\"another$RUN\"}" "$BAD2"
expect 200 GET /api/users/@me/unread "" "$BAD"
expect 200 GET /api/users/@me/unread "" "$BAD2"
expect 200 GET "/api/users/$BAD_ID/profile/full" "" "$PAL"
jq -e --arg o "rude.bad$RUN" --arg p "$PH" '(tostring | contains($o) | not) and (tostring | contains($p) | not)' "$SB" >/dev/null
expect 401 POST /api/auth/login "{\"username\":\"rude.bad$RUN\",\"password\":\"password123\"}" "" "$(xff)"   # the old name is gone now
expect 200 POST /api/auth/login "{\"username\":\"polite.name$RUN\",\"password\":\"password123\"}" "" "$(xff)"

rstep "People: the blocked-words filter (existing accounts aren't changed)"
expect 200 GET "/api/safety/users?q=$RUN&filter=blocked" "" "$MOD"
jq -e --arg o "$OLD_ID" --arg p "$PAL_ID" 'any(.[]; .id == $o) and all(.[]; .id != $p)' "$SB" >/dev/null
expect 200 GET "/api/safety/users?q=$RUN" "" "$MOD"
jq -e --arg p "$PAL_ID" 'any(.[]; .id == $p)' "$SB" >/dev/null
[ "$(q "SELECT username FROM users WHERE id = $OLD_ID")" = "nastyold$RUN" ]
expect 200 GET "/api/safety/users/$OLD_ID" "" "$OWNER_T"
jq -e '.username_blocked_word == "nasty" and (.choosing_username | not)' "$SB" >/dev/null
expect 403 GET "/api/safety/users?q=$RUN&filter=blocked" "" "$OTHER"

rstep "from a report: conflicts of interest; a custom force_rename-only staff member; the owner"
expect 200 POST "/api/guilds/$G/invites" "" "$PAL"; INV=$(jq -r .code "$SB")
expect 200 POST "/api/invites/$INV" "" "$OLD"; expect 200 POST "/api/invites/$INV" "" "$OTHER"
expect 201 POST /api/reports "{\"target_kind\":\"user\",\"target_id\":\"$OLD_ID\",\"category\":\"harassment\",\"details\":\"rude name\"}" "$MOD"
R_BY_MOD=$(q "SELECT id FROM reports WHERE target_kind = 'user' AND target_id = $OLD_ID AND status <> 'closed'")
expect 403 POST "/api/safety/reports/$R_BY_MOD/actions" '{"action":"force_rename","reason":"inappropriate"}' "$MOD"
grep -q "Someone else needs to handle this" "$SB"
expect 200 POST "/api/safety/reports/$R_BY_MOD/actions" '{"action":"force_rename","reason":"inappropriate","text":"Seen in the report"}' "$SEN"
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'force_rename' AND report_id = $R_BY_MOD AND staff_tier = 'senior'")" = 1 ]
[[ "$(q "SELECT username FROM users WHERE id = $OLD_ID")" == renamed-* ]]
expect 403 POST "/api/safety/reports/$R_BY_MOD/actions" '{"action":"force_rename","reason":"inappropriate"}' "$CUS"   # no "reports" permission
force 200 "$OTHER_ID" "$CUS" impersonation "Pretending to be the guild master"
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'force_rename' AND actor_id = $CUS_ID AND staff_tier = 'custom' AND staff_perm = 'force_rename'")" = 1 ]
[ "$(q "SELECT display_name FROM users WHERE id = $OTHER_ID")" = "rnother$RUN" ]           # display name kept (not ticked)
force 403 "$PAL_ID" "$SUP"
P2=$(new_user "rnowned$RUN"); P2_ID=$(uid_of "rnowned$RUN")
force 200 "$P2_ID" "$OWNER_T" other "Owner says no"
[[ "$(dm_from_safety "$P2_ID")" == *"Your username was changed by Outhouse staff (reason: Other)"* ]]
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'force_rename' AND actor_id = $OWNER_ID AND staff_tier IS NULL")" -ge 1 ]
expect 403 GET /api/users/@me/unread "" "$P2"
jq -e '.code == "must_choose_username"' "$SB" >/dev/null
expect 204 POST /api/auth/logout "" "$P2"                                                    # signing out still works

echo "Rename test passed."
