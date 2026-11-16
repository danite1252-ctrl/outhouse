#!/usr/bin/env bash
# Site staff (tiers, permissions, safeguards) and outhouse ownership
# transfers, end to end against a real server and Postgres, with the SMTP
# sink (ci/smtp_sink.py) catching mail. ci/smoke.sh runs it near its end; it
# can also run on its own:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=... MAIL_DIR=/tmp/outhouse-mail \
#   OWNER_TOKEN=<the site owner's token> PUSH=http://127.0.0.1:8097 bash ci/staff_test.sh
#
# The server needs SMTP_* pointing at the sink, PUBLIC_URL=$BASE,
# AUTH_SECRET_KEY, OUTHOUSE_TEST_HOOKS=1, STAFF_ACTIONS_PER_HOUR=8 and
# STAFF_HEAVY_ACTIONS_PER_HOUR=10
# (a small cap so the test can reach it). With PUSH set (ci/push_mock.py as
# the server's ALERT_NTFY_URL target) the owner's cap alert is checked too.
set -euo pipefail
set -E
trap 'echo "Staff test failed at line $LINENO" >&2' ERR

BASE=${BASE:-http://127.0.0.1:8080}
MAIL_DIR=${MAIL_DIR:-/tmp/outhouse-mail}
SB=/tmp/staff-body
RUN=${STAFF_RUN:-$(date +%s | tail -c 6)}
sstep() { echo "== staff: $*"; }

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
body() { cat "$SB"; }
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
# new_user NAME [BIRTH_DATE] [XFF] -> token (email confirmed unless NOCONFIRM=1)
new_user() {
  local dob=${2:-1990-01-01} xff=${3:-198.18.60.$((RANDOM % 200 + 1))}
  local m0; m0=$(mail_n)
  expect 200 POST /api/auth/register "{\"username\":\"$1\",\"email\":\"$(mail_of "$1")\",\"password\":\"password123\",\"birth_date\":\"$dob\",\"display_name\":\"$1\"}" "" "$xff"
  local t; t=$(jq -r .token "$SB")
  if [ "${NOCONFIRM:-0}" != 1 ]; then
    local f; f=$(wait_mail "$(mail_of "$1")" "Confirm your email" "$m0")
    expect 200 POST /api/auth/verify-email "{\"token\":\"$(link_token "$f")\"}" >/dev/null
  fi
  echo "$t"
}
# two_factor TOKEN -> the secret (turned on with the code from one step back,
# so codes for "now" and "next" are still free afterwards)
two_factor() {
  expect 200 POST /api/auth/2fa/setup '{"password":"password123"}' "$1" >/dev/null
  local s; s=$(jq -r .secret "$SB")
  expect 200 POST /api/auth/2fa/confirm "{\"code\":\"$(totp "$s" -1)\"}" "$1" >/dev/null
  echo "$s"
}
dm_from_safety() { # the newest Outhouse Safety DM to this user
  q "SELECT m.content FROM messages m JOIN dm_participants p ON p.channel_id = m.channel_id
     WHERE p.user_id = $1 AND m.author_id = (SELECT safety_bot_id FROM site_settings WHERE id = 1)
     ORDER BY m.id DESC LIMIT 1"
}
join() { # join TOKEN GUILD [INVITER] (via a fresh invite; the site owner's by default)
  expect 200 POST "/api/guilds/$2/invites" "" "${3:-$OWNER_T}" >/dev/null
  local code; code=$(jq -r .code "$SB")
  expect 200 POST "/api/invites/$code" "" "$1" >/dev/null
}
guild() { # guild TOKEN NAME -> id (made by TOKEN, who owns it)
  expect 200 POST /api/guilds "{\"name\":\"$2\"}" "$1" >/dev/null; jq -r .id "$SB"
}
first_channel() { q "SELECT id FROM channels WHERE guild_id = $1 AND kind = 0 ORDER BY position LIMIT 1"; }
say() { # say TOKEN CHANNEL TEXT -> message id
  expect 200 POST "/api/channels/$2/messages" "$(jq -nc --arg c "$3" '{content:$c}')" "$1" >/dev/null; jq -r .id "$SB"
}
file_report() { # file_report TOKEN KIND ID CATEGORY -> report id
  expect 201 POST /api/reports "$(jq -nc --arg k "$2" --arg i "$3" --arg c "$4" '{target_kind:$k,target_id:$i,category:$c,details:"test"}')" "$1" >/dev/null
  q "SELECT id FROM reports WHERE target_kind = '$2' AND target_id = $3 AND status <> 'closed'"
}

if [ -z "${OWNER_TOKEN:-}" ]; then
  OWNER_TOKEN=$(NOCONFIRM=1 new_user "owner$RUN")
fi
OWNER_T=$OWNER_TOKEN
OWNER_ID=$(curl -fsS -H "Authorization: Bearer $OWNER_T" "$BASE/api/users/@me" | jq -r .id)

# ---------------------------------------------------------------------------
sstep "people: support, moderators, seniors (confirmed email + two-factor)"
SUP=$(new_user "sup$RUN"); SUP_ID=$(uid_of "sup$RUN")
MOD=$(new_user "mod$RUN"); MOD_ID=$(uid_of "mod$RUN")
# A sign-in from before two-factor was on (it never gets staff powers).
expect 200 POST /api/auth/login "{\"username\":\"mod$RUN\",\"password\":\"password123\"}"
MOD_OLD=$(jq -r .token "$SB")
MOD2=$(new_user "modb$RUN"); MOD2_ID=$(uid_of "modb$RUN")
SEN=$(new_user "sen$RUN"); SEN_ID=$(uid_of "sen$RUN")
SEN2=$(new_user "senb$RUN"); SEN2_ID=$(uid_of "senb$RUN")
NOFA=$(new_user "nofa$RUN"); NOFA_ID=$(uid_of "nofa$RUN")
NOMAIL=$(NOCONFIRM=1 new_user "nomail$RUN")
SUP_S=$(two_factor "$SUP"); MOD_S=$(two_factor "$MOD"); MOD2_S=$(two_factor "$MOD2")
SEN_S=$(two_factor "$SEN"); SEN2_S=$(two_factor "$SEN2"); NOMAIL_S=$(two_factor "$NOMAIL")
VIC=$(new_user "vic$RUN" 1990-01-01 203.0.113.210); VIC_ID=$(uid_of "vic$RUN")
REP=$(new_user "rep$RUN"); REP_ID=$(uid_of "rep$RUN")
VAL0=$(new_user "member$RUN"); VAL0_ID=$(uid_of "member$RUN")

sstep "only the owner manages staff; refused without a confirmed email or two-factor"
expect 403 GET /api/safety/staff "" "$SEN"
expect 403 POST /api/safety/staff "{\"username\":\"sen$RUN\",\"tier\":\"senior\"}" "$SEN"
expect 200 GET "/api/safety/staff/candidate?username=nofa$RUN" "" "$OWNER_T"
jq -e '.email_verified and (.two_factor | not) and (.ok | not) and (.problem | test("two-factor"))' "$SB" >/dev/null
expect 400 POST /api/safety/staff "{\"username\":\"nofa$RUN\",\"tier\":\"moderator\"}" "$OWNER_T"
grep -q "two-factor" "$SB"
expect 200 GET "/api/safety/staff/candidate?username=nomail$RUN" "" "$OWNER_T"
jq -e '(.email_verified | not) and .two_factor and (.problem | test("confirm their email"))' "$SB" >/dev/null
expect 400 POST /api/safety/staff "{\"username\":\"nomail$RUN\",\"tier\":\"moderator\"}" "$OWNER_T"
expect 400 POST /api/safety/staff "{\"username\":\"sup$RUN\",\"tier\":\"custom\",\"perms\":[]}" "$OWNER_T"
expect 400 POST /api/safety/staff "{\"username\":\"sup$RUN\",\"tier\":\"boss\"}" "$OWNER_T"
M0=$(mail_n)
expect 200 POST /api/safety/staff "{\"username\":\"sup$RUN\",\"tier\":\"support\"}" "$OWNER_T"
wait_mail "$(mail_of "sup$RUN")" "You're now Outhouse staff" "$M0" >/dev/null
expect 200 POST /api/safety/staff "{\"username\":\"mod$RUN\",\"tier\":\"moderator\"}" "$OWNER_T"
expect 200 POST /api/safety/staff "{\"username\":\"modb$RUN\",\"tier\":\"custom\",\"perms\":[\"people\",\"hide_messages\",\"warn\",\"nonsense\"]}" "$OWNER_T"
expect 200 POST /api/safety/staff "{\"username\":\"sen$RUN\",\"tier\":\"senior\"}" "$OWNER_T"
expect 200 POST /api/safety/staff "{\"username\":\"senb$RUN\",\"tier\":\"senior\"}" "$OWNER_T"
expect 400 POST /api/safety/staff "{\"username\":\"sen$RUN\",\"tier\":\"senior\"}" "$OWNER_T"   # already staff
expect 200 GET /api/safety/staff "" "$OWNER_T"
jq -e --arg m "$MOD2_ID" '(.staff | length) >= 5 and (.catalogue.perms | length) == 18
  and any(.staff[]; .id == $m and .tier == "custom" and .perms == ["people","hide_messages","warn"])
  and .settings.two_person_permanent_ip and .settings.two_person_shut_down and .settings.two_person_members == 25
  and (.settings.staff_badge | not)' "$SB" >/dev/null
expect 200 GET /api/users/@me/site-owner "" "$MOD"
jq -e '(.site_owner | not) and .staff.tier == "moderator" and .staff.active and .staff.mfa_session
  and (.staff.perms | index("warn")) and (.staff.perms | index("ips") | not)' "$SB" >/dev/null

sstep "a sign-in from before two-factor can't use staff powers"
expect 403 GET /api/safety/reports "" "$MOD_OLD"
grep -q "used two-factor" "$SB"
expect 200 GET /api/users/@me/site-owner "" "$MOD_OLD"
jq -e '(.staff.active | not) and (.staff.mfa_session | not)' "$SB" >/dev/null
# Signing in with the code gives a session that works.
expect 200 POST /api/auth/login "{\"username\":\"mod$RUN\",\"password\":\"password123\"}"
TICKET=$(jq -r .ticket "$SB")
expect 200 POST /api/auth/login/2fa "{\"ticket\":\"$TICKET\",\"code\":\"$(totp "$MOD_S" 0)\"}"
MOD_NEW=$(jq -r .token "$SB")
expect 200 GET /api/safety/reports "" "$MOD_NEW"
[ "$(q "SELECT mfa FROM sessions WHERE token_hash = encode(sha256('$MOD_NEW'::bytea), 'hex')")" = t ]

# Things to act on: an outhouse with messages and reports.
G=$(guild "$OWNER_T" "Staff Test $RUN"); CH=$(first_channel "$G")
for t in "$VIC" "$REP" "$MOD" "$SEN"; do join "$t" "$G"; done
VM=$(say "$VIC" "$CH" "rude words $RUN")
R_MSG=$(file_report "$REP" message "$VM" harassment)
VM2=$(say "$VIC" "$CH" "very bad $RUN")
R_CHILD=$(file_report "$REP" message "$VM2" child_safety)

sstep "Support: account basics, reset links with a reason, report status only"
expect 200 GET "/api/safety/users?q=vic$RUN" "" "$SUP"
jq -e 'length == 1' "$SB" >/dev/null
expect 200 GET "/api/safety/users/$VIC_ID" "" "$SUP"
jq -e '.username != null and .age_bracket == "adult" and (.outhouses | length) >= 1 and (.sign_in.email | test("•"))
  and (.sign_in | has("enabled")) and (has("log") | not) and (has("reports") | not) and (has("ip_blocks") | not)' "$SB" >/dev/null
expect 200 GET /api/safety/reports "" "$SUP"
jq -e --arg r "$R_MSG" 'any(.reports[]; .id == $r and .locked and .preview == null and .reported_user == null and .status == "open")' "$SB" >/dev/null
expect 403 GET "/api/safety/reports/$R_MSG" "" "$SUP"
expect 403 POST "/api/safety/users/$VIC_ID/actions" '{"action":"warn","text":"hi"}' "$SUP"
expect 403 POST "/api/safety/users/$VIC_ID/actions" '{"action":"suspend","days":1}' "$SUP"
expect 403 GET "/api/safety/users/$VIC_ID/ips" "" "$SUP"
expect 403 GET "/api/safety/users/$VIC_ID/same-ip" "" "$SUP"
expect 403 GET /api/safety/ip-blocks "" "$SUP"
expect 403 GET /api/safety/outhouses "" "$SUP"
expect 400 POST "/api/safety/users/$VIC_ID/reset-link" '{}' "$SUP"
expect 200 POST "/api/safety/users/$VIC_ID/reset-link" '{"reason":"Lost their email, checked on Discord voice"}' "$SUP"
jq -e '.path | startswith("/reset?token=")' "$SB" >/dev/null
expect 403 POST "/api/safety/users/$OWNER_ID/reset-link" '{"reason":"x"}' "$SUP"     # never the site owner
expect 403 POST "/api/safety/users/$MOD_ID/reset-link" '{"reason":"x"}' "$SUP"       # nor higher staff
expect 200 GET /api/safety/summary "" "$SUP"
jq -e '.open == 0' "$SB" >/dev/null                    # nothing Support can act on

sstep "Moderator: reports with snapshots, warn, short suspensions; never child safety, IPs or shut-downs"
expect 200 GET /api/safety/reports "" "$MOD_NEW"
jq -e --arg r "$R_MSG" --arg c "$R_CHILD" 'any(.reports[]; .id == $r and (.locked | not) and (.preview | test("rude")))
  and any(.reports[]; .id == $c and .locked and .child_safety and .preview == null)' "$SB" >/dev/null
expect 200 GET "/api/safety/reports/$R_MSG" "" "$MOD_NEW"
jq -e '.snapshot.message.content != null and (.police_links == []) and (.subject.ip_blocks == null)' "$SB" >/dev/null
expect 403 GET "/api/safety/reports/$R_CHILD" "" "$MOD_NEW"
grep -q "senior moderator" "$SB"
expect 403 POST "/api/safety/reports/$R_CHILD/actions" '{"action":"suspend_pending"}' "$MOD_NEW"
expect 200 POST "/api/safety/reports/$R_MSG/actions" '{"action":"warn","text":"Please be kind."}' "$MOD_NEW"
WARN=$(dm_from_safety "$VIC_ID")
[[ "$WARN" == *"(by Outhouse staff)"* ]] && [[ "$WARN" != *"mod$RUN"* ]]
expect 403 POST "/api/safety/reports/$R_MSG/actions" '{"action":"suspend"}' "$MOD_NEW"        # permanent: seniors
expect 403 POST "/api/safety/reports/$R_MSG/actions" '{"action":"suspend","days":30}' "$MOD_NEW"
expect 200 POST "/api/safety/reports/$R_MSG/actions" '{"action":"suspend","days":3,"text":"Cool off"}' "$MOD_NEW"
expect 403 POST /api/auth/login "{\"username\":\"vic$RUN\",\"password\":\"password123\"}" "" 203.0.113.210
jq -e '.error | test("suspended by Outhouse staff until")' "$SB" >/dev/null
! grep -q "mod$RUN" "$SB"
expect 403 POST "/api/safety/users/$VIC_ID/actions" '{"action":"make_permanent"}' "$MOD_NEW"
expect 200 POST "/api/safety/users/$VIC_ID/actions" '{"action":"unsuspend"}' "$MOD_NEW"
expect 200 POST /api/auth/login "{\"username\":\"vic$RUN\",\"password\":\"password123\"}" "" 203.0.113.210
VIC=$(jq -r .token "$SB")
expect 403 GET "/api/safety/users/$VIC_ID/ips" "" "$MOD_NEW"
expect 200 GET "/api/safety/users/$VIC_ID/same-ip" "" "$MOD_NEW"
jq -e '(.accounts | type) == "array" and (tostring | test("203\\.0\\.113") | not)' "$SB" >/dev/null
expect 403 GET /api/safety/ip-blocks "" "$MOD_NEW"
expect 403 GET "/api/safety/users/$VIC_ID/ip-block-preview" "" "$MOD_NEW"
expect 403 POST "/api/safety/outhouses/$G/actions" '{"action":"shut_down","reason":"x"}' "$MOD_NEW"
expect 403 PUT "/api/safety/guilds/$G/family" '{"family":true}' "$MOD_NEW"
for path in /api/safety/log /api/safety/digest /api/safety/settings /api/safety/detector /api/safety/server-status /api/safety/staff; do
  expect 403 GET "$path" "" "$MOD_NEW"
done
expect 403 POST "/api/safety/reports/$R_MSG/police-links" '{}' "$MOD_NEW"
expect 403 GET "/api/safety/reports/$R_MSG/export" "" "$MOD_NEW"
expect 403 POST "/api/safety/users/$VIC_ID/actions" '{"action":"set_age","bracket":"teen"}' "$MOD_NEW"
# The custom moderator only has what was ticked.
expect 200 POST "/api/safety/users/$VIC_ID/actions" '{"action":"warn","text":"Second warning"}' "$MOD2"
expect 403 GET /api/safety/reports "" "$MOD2"
expect 403 POST "/api/safety/users/$VIC_ID/actions" '{"action":"suspend","days":1}' "$MOD2"

sstep "Moderator: looking inside an outhouse only while it has an open report (logged)"
G2=$(guild "$OWNER_T" "Quiet Shed $RUN"); CH2=$(first_channel "$G2")
expect 200 GET /api/safety/outhouses "" "$MOD_NEW"
expect 403 GET "/api/safety/outhouses/$G2/channels/$CH2/messages" "" "$MOD_NEW"
grep -q "open" "$SB"
expect 200 GET "/api/safety/outhouses/$G/channels/$CH/messages" "" "$MOD_NEW"
[ "$(q "SELECT count(*) FROM safety_log WHERE actor_id = $MOD_ID AND action = 'outhouse_view' AND staff_tier = 'moderator' AND staff_perm = 'outhouse_look'")" -ge 1 ]
expect 200 GET "/api/safety/outhouses/$G" "" "$MOD_NEW"
jq -e '.live_police_links == 0 and (.conflict | not)' "$SB" >/dev/null

sstep "conflicts of interest: about themselves, reported by themselves, or an outhouse they own or moderate"
GM=$(guild "$MOD" "Mods Own $RUN"); CHM=$(first_channel "$GM")
join "$VIC" "$GM" "$MOD"; join "$REP" "$GM" "$MOD"
VMM=$(say "$VIC" "$CHM" "in the mod's outhouse $RUN")
R_OWN=$(file_report "$REP" message "$VMM" spam)
MM=$(say "$MOD" "$CH" "the mod said something $RUN")
R_ABOUT=$(file_report "$REP" message "$MM" spam)
VM3=$(say "$VIC" "$CH" "reported by the mod $RUN")
R_BY=$(file_report "$MOD" message "$VM3" spam)
for r in "$R_OWN" "$R_ABOUT" "$R_BY"; do
  expect 403 GET "/api/safety/reports/$r" "" "$MOD_NEW"
  grep -q "Someone else needs to handle this" "$SB"
  expect 403 POST "/api/safety/reports/$r/actions" '{"action":"dismiss"}' "$MOD_NEW"
done
expect 200 GET /api/safety/reports "" "$MOD_NEW"
jq -e --arg a "$R_OWN" --arg b "$R_ABOUT" --arg c "$R_BY" '[.reports[] | select(.id == $a or .id == $b or .id == $c) | select(.conflict and .locked)] | length == 3' "$SB" >/dev/null
expect 403 POST "/api/safety/users/$VIC_ID/actions" "{\"action\":\"timeout\",\"guild_id\":\"$GM\",\"minutes\":5}" "$MOD_NEW"
expect 403 GET "/api/safety/outhouses/$GM/channels/$CHM/messages" "" "$MOD_NEW"
# The site owner is exempt.
expect 200 GET "/api/safety/reports/$R_OWN" "" "$OWNER_T"

sstep "staff can't act on the site owner or on staff at their level or above"
expect 403 POST "/api/safety/users/$OWNER_ID/actions" '{"action":"warn","text":"x"}' "$MOD_NEW"
expect 403 POST "/api/safety/users/$SEN_ID/actions" '{"action":"warn","text":"x"}' "$MOD_NEW"
expect 403 POST "/api/safety/users/$MOD2_ID/actions" '{"action":"warn","text":"x"}' "$MOD_NEW"   # custom with moderator powers
expect 200 POST "/api/safety/users/$MOD_ID/actions" '{"action":"note","text":"Senior checking in"}' "$SEN"
expect 200 POST "/api/safety/users/$SUP_ID/actions" '{"action":"warn","text":"Test warning"}' "$MOD_NEW"

sstep "Senior: IPs, child-safety reports (Suspend now never waits), shut-downs, family"
expect 200 GET "/api/safety/users/$VIC_ID/ips" "" "$SEN"
jq -e 'any(.ips[]; .ip == "203.0.113.210")' "$SB" >/dev/null
expect 200 GET /api/safety/ip-blocks "" "$SEN"
expect 200 GET "/api/safety/reports/$R_CHILD" "" "$SEN"
jq -e 'all(.files[]; .viewable | not)' "$SB" >/dev/null
expect 200 POST "/api/safety/reports/$R_CHILD/actions" '{"action":"suspend_pending"}' "$SEN"
[ "$(q "SELECT suspension_pending FROM users WHERE id = $VIC_ID")" = t ]
expect 204 PUT "/api/safety/guilds/$G2/family" '{"family":true}' "$SEN"
[ "$(q "SELECT action || '/' || staff_tier || '/' || staff_perm FROM safety_log WHERE actor_id = $SEN_ID AND action = 'family' ORDER BY id DESC LIMIT 1")" = "family/senior/family" ]

sstep "two-person rule: a permanent suspension with an IP block waits for another senior"
expect 200 GET "/api/safety/users/$VIC_ID/ip-block-preview" "" "$SEN"
jq -e '.needs_approval and any(.networks[]; .network == "203.0.113.210/32")' "$SB" >/dev/null
expect 200 POST "/api/safety/users/$VIC_ID/ip-blocks" '{"make_permanent":true,"permanent_reason":"Repeat offender","reason":"Ban evasion"}' "$SEN"
jq -e '.pending_approval' "$SB" >/dev/null
REQ=$(jq -r .request_id "$SB")
[ "$(q "SELECT count(*) FROM ip_blocks WHERE source_user_id = $VIC_ID AND removed_at IS NULL")" = 0 ]
[ "$(q "SELECT suspension_pending FROM users WHERE id = $VIC_ID")" = t ]   # still only 'until reviewed'
expect 200 GET /api/safety/staff-requests "" "$SEN2"
jq -e --arg r "$REQ" 'any(.pending[]; .id == $r and .can_decide and (.summary | test("permanently suspend")))' "$SB" >/dev/null
expect 403 GET /api/safety/staff-requests "" "$MOD_NEW"
expect 403 POST "/api/safety/staff-requests/$REQ/approve" '{}' "$SEN"           # not their own
grep -q "someone else" "$SB"
expect 403 POST "/api/safety/staff-requests/$REQ/approve" '{}' "$MOD_NEW"
expect 200 POST "/api/safety/staff-requests/$REQ/approve" '{"note":"Agreed"}' "$SEN2"
jq -e '.status == "approved" and (.result | test("Blocked 1 network"))' "$SB" >/dev/null
[ "$(q "SELECT suspended_until >= '9999-01-01' AND NOT suspension_pending FROM users WHERE id = $VIC_ID")" = t ]
[ "$(q "SELECT count(*) FROM ip_blocks WHERE source_user_id = $VIC_ID AND removed_at IS NULL AND created_by = $SEN_ID")" = 1 ]
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'approval_given' AND actor_id = $SEN2_ID")" = 1 ]
expect 400 POST "/api/safety/staff-requests/$REQ/decline" '{}' "$SEN2"           # already decided
# The owner's own actions never wait.
expect 200 GET "/api/safety/users/$VIC_ID/ip-block-preview" "" "$OWNER_T"
jq -e '.needs_approval | not' "$SB" >/dev/null
BLOCK=$(q "SELECT id FROM ip_blocks WHERE source_user_id = $VIC_ID AND removed_at IS NULL")
expect 200 POST "/api/safety/ip-blocks/$BLOCK/remove" '' "$SEN"
expect 200 POST /api/safety/ip-blocks/reload '' "$OWNER_T"

sstep "two-person rule: requests expire after 72 hours; declining tells the requester"
expect 200 POST "/api/safety/users/$VIC_ID/ip-blocks" '{"reason":"again"}' "$SEN"
REQ2=$(jq -r .request_id "$SB")
q "UPDATE staff_requests SET expires_at = now() - interval '1 minute' WHERE id = $REQ2" >/dev/null
expect 400 POST "/api/safety/staff-requests/$REQ2/approve" '{}' "$SEN2"
grep -q "expired" "$SB"
expect 200 POST "/api/safety/users/$VIC_ID/ip-blocks" '{"reason":"third time"}' "$SEN2"
REQ3=$(jq -r .request_id "$SB")
expect 200 POST "/api/safety/staff-requests/$REQ3/decline" '{"note":"Not enough"}' "$OWNER_T"
[[ "$(dm_from_safety "$SEN2_ID")" == *"declined"* ]]

sstep "two-person rule: shutting down a bigger outhouse waits; staff notices say 'by Outhouse staff'"
expect 200 PUT /api/safety/staff-settings '{"two_person_members":3}' "$OWNER_T"
expect 403 PUT /api/safety/staff-settings '{"two_person_members":1}' "$SEN"
GS=$(guild "$REP" "Big Shed $RUN")
for t in "$VAL0" "$SUP" "$MOD2"; do join "$t" "$GS" "$REP"; done
expect 200 POST "/api/safety/outhouses/$GS/actions" '{"action":"shut_down","reason":"Spam outhouse"}' "$SEN"
jq -e '.pending_approval.pending_approval and (.shut_down | not)' "$SB" >/dev/null
REQS=$(jq -r .pending_approval.request_id "$SB")
expect 200 POST "/api/safety/staff-requests/$REQS/approve" '{}' "$OWNER_T"
jq -e '.status == "approved"' "$SB" >/dev/null
[ "$(q "SELECT shut_down_at IS NOT NULL AND shut_down_by = $SEN_ID FROM guilds WHERE id = $GS")" = t ]
[[ "$(dm_from_safety "$VAL0_ID")" == *"shut down by Outhouse staff"* ]]
# A small one (under the threshold) shuts straight away.
GT=$(guild "$REP" "Tiny Shed $RUN")
expect 200 POST "/api/safety/outhouses/$GT/actions" '{"action":"shut_down","reason":"Test"}' "$SEN"
jq -e '.shut_down and (.pending_approval == null)' "$SB" >/dev/null
expect 200 POST "/api/safety/outhouses/$GT/actions" '{"action":"restore"}' "$SEN"
expect 403 POST "/api/safety/outhouses/$GT/actions" '{"action":"delete","confirm_name":"x"}' "$SEN"
expect 200 PUT /api/safety/staff-settings '{"two_person_members":25}' "$OWNER_T"

sstep "hourly caps: blocked at the cap, and the owner is alerted (no content)"
CAP=${STAFF_ACTIONS_PER_HOUR:-8}
USED=$(q "SELECT count(*) FROM staff_actions WHERE user_id = $MOD2_ID AND NOT heavy AND at > now() - interval '1 hour'")
for _ in $(seq $((USED + 1)) "$CAP"); do
  expect 200 POST "/api/safety/users/$REP_ID/actions" '{"action":"hide_messages"}' "$MOD2"
done
expect 429 POST "/api/safety/users/$REP_ID/actions" '{"action":"hide_messages"}' "$MOD2"
grep -q "hourly limit" "$SB"
expect 429 POST "/api/safety/users/$REP_ID/actions" '{"action":"warn","text":"x"}' "$MOD2"
[ "$(q "SELECT count(*) FROM safety_log WHERE actor_id = $MOD2_ID AND action = 'staff_cap'")" = 1 ]
if [ -n "${PUSH:-}" ]; then
  curl -fsS "$PUSH/alerts/safety-ntfy" | jq -e 'any(.[]; (.title // "" | test("limit")) and (.body | test("hide|rude") | not))' >/dev/null
fi
# Notes aren't capped.
expect 200 POST "/api/safety/users/$REP_ID/actions" '{"action":"note","text":"fine"}' "$MOD2"
expect 200 POST "/api/safety/users/$REP_ID/actions" '{"action":"unhide_messages"}' "$OWNER_T"

sstep "staff activity (owner only), digest section, and the log can't be changed"
expect 200 GET "/api/safety/staff/activity?staff=$MOD_ID" "" "$OWNER_T"
jq -e 'any(.entries[]; .action == "warn" and .tier == "moderator" and .perm == "warn") and all(.entries[]; .actor_username | test("^mod"))' "$SB" >/dev/null
expect 200 GET "/api/safety/staff/activity?action=suspend" "" "$OWNER_T"
jq -e 'all(.entries[]; .action == "suspend") and (.entries | length) >= 1' "$SB" >/dev/null
expect 403 GET /api/safety/staff/activity "" "$SEN"
expect 200 GET /api/safety/digest "" "$OWNER_T"
jq -e --arg u "mod$RUN" 'any(.staff_activity[]; .username == $u and .actions >= 1)' "$SB" >/dev/null
! q "UPDATE safety_log SET details = 'edited' WHERE actor_id = $MOD_ID AND staff_tier IS NOT NULL" 2>/dev/null
! q "DELETE FROM safety_log WHERE actor_id = $MOD_ID AND staff_tier IS NOT NULL" 2>/dev/null
[ "$(q "SELECT count(*) FROM safety_log WHERE actor_id = $MOD_ID AND details = 'edited'")" = 0 ]

sstep "turning two-factor off pauses staff powers (owner told); turning it back on restores them"
expect 200 POST /api/auth/2fa/disable "{\"password\":\"password123\",\"code\":\"$(totp "$SEN2_S" 0)\"}" "$SEN2"
expect 403 GET /api/safety/reports "" "$SEN2"
grep -q "paused" "$SB"
[[ "$(dm_from_safety "$OWNER_ID")" == *"Staff powers paused"* ]] || [ "$(q "SELECT count(*) FROM safety_log WHERE action = 'staff_paused' AND target_user_id = $SEN2_ID")" = 1 ]
expect 200 GET /api/safety/staff "" "$OWNER_T"
jq -e --arg s "$SEN2_ID" 'any(.staff[]; .id == $s and .paused and (.two_factor | not))' "$SB" >/dev/null
SEN2_S=$(two_factor "$SEN2")
expect 200 GET /api/safety/reports "" "$SEN2"
[ "$(q "SELECT lapsed_at IS NULL FROM staff WHERE user_id = $SEN2_ID")" = t ]

sstep "staff badge: off by default; the owner can show it"
expect 200 GET "/api/users/$SEN_ID/profile/full" "" "$REP"
jq -e '.staff_badge == false' "$SB" >/dev/null
expect 200 PUT /api/safety/staff-settings '{"staff_badge":true}' "$OWNER_T"
expect 200 GET "/api/users/$SEN_ID/profile/full" "" "$REP"
jq -e '.staff_badge == true' "$SB" >/dev/null
expect 200 PUT /api/safety/staff-settings '{"staff_badge":false}' "$OWNER_T"

sstep "removing staff: immediate, their sign-in keeps working as a normal account, emailed"
M1=$(mail_n)
expect 200 DELETE "/api/safety/staff/$SUP_ID" "" "$OWNER_T"
wait_mail "$(mail_of "sup$RUN")" "no longer Outhouse staff" "$M1" >/dev/null
expect 403 GET "/api/safety/users?q=x" "" "$SUP"
expect 200 GET /api/users/@me "" "$SUP"
expect 200 GET /api/users/@me/site-owner "" "$SUP"
jq -e '.staff == null' "$SB" >/dev/null
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'staff_removed' AND target_user_id = $SUP_ID")" = 1 ]
expect 200 PUT "/api/safety/staff/$MOD2_ID" '{"tier":"support"}' "$OWNER_T"
expect 403 POST "/api/safety/users/$REP_ID/actions" '{"action":"warn","text":"x"}' "$MOD2"   # support now
expect 200 GET "/api/safety/users/$REP_ID" "" "$MOD2"

# ===========================================================================
# Ownership transfers
# ===========================================================================
sstep "transfer by the owner: checks on the recipient, username + password to confirm"
OLI=$(new_user "oli$RUN"); OLI_ID=$(uid_of "oli$RUN")
RAY=$(new_user "ray$RUN"); RAY_ID=$(uid_of "ray$RUN")
TEEN=$(new_user "teen$RUN" "$(date -u -d '15 years ago' +%F)"); TEEN_ID=$(uid_of "teen$RUN")
UNC=$(NOCONFIRM=1 new_user "unc$RUN"); UNC_ID=$(uid_of "unc$RUN")
OUT=$(new_user "out$RUN"); OUT_ID=$(uid_of "out$RUN")
GO=$(guild "$OLI" "Olis Shed $RUN")
oinv() { expect 200 POST "/api/guilds/$GO/invites" "" "$OLI" >/dev/null; jq -r .code "$SB"; }
for t in "$RAY" "$TEEN" "$UNC"; do expect 200 POST "/api/invites/$(oinv)" "" "$t" >/dev/null; done
BOT_ID=$(q "SELECT safety_bot_id FROM site_settings WHERE id = 1")
q "INSERT INTO guild_members (guild_id, user_id) VALUES ($GO, $BOT_ID) ON CONFLICT DO NOTHING" >/dev/null
tbody() { jq -nc --arg u "$1" --arg c "$2" --arg p "${3:-password123}" --argjson k "${4:-false}" '{user_id:$u,confirm_username:$c,password:$p,keep_admin:$k}'; }
expect 403 POST "/api/guilds/$GO/transfer" "$(tbody "$OLI_ID" "oli$RUN")" "$RAY"           # only the owner
expect 400 POST "/api/guilds/$GO/transfer" "$(tbody "$RAY_ID" "someone")" "$OLI"
grep -q "exactly" "$SB"
expect 400 POST "/api/guilds/$GO/transfer" "$(tbody "$RAY_ID" "ray$RUN" wrongpass)" "$OLI"
expect 400 POST "/api/guilds/$GO/transfer" "$(tbody "$TEEN_ID" "teen$RUN")" "$OLI"
grep -q "Teen" "$SB"
expect 400 POST "/api/guilds/$GO/transfer" "$(tbody "$UNC_ID" "unc$RUN")" "$OLI"
grep -q "confirmed email" "$SB"
expect 400 POST "/api/guilds/$GO/transfer" "$(tbody "$BOT_ID" "x")" "$OLI"
grep -q "Bots" "$SB"
expect 400 POST "/api/guilds/$GO/transfer" "$(tbody "$OUT_ID" "out$RUN")" "$OLI"
grep -q "member" "$SB"
expect 200 POST "/api/safety/users/$RAY_ID/actions" '{"action":"suspend","days":1}' "$OWNER_T"
expect 400 POST "/api/guilds/$GO/transfer" "$(tbody "$RAY_ID" "ray$RUN")" "$OLI"
grep -q "suspended" "$SB"
expect 200 POST "/api/safety/users/$RAY_ID/actions" '{"action":"unsuspend"}' "$OWNER_T"
expect 200 POST /api/auth/login "{\"username\":\"ray$RUN\",\"password\":\"password123\"}"
RAY=$(jq -r .token "$SB")
q "DELETE FROM guild_members WHERE guild_id = $GO AND user_id = $BOT_ID" >/dev/null
# A family outhouse can go to a teen (offered, then cancelled).
q "UPDATE guilds SET family = true WHERE id = $GO" >/dev/null
expect 200 POST "/api/guilds/$GO/transfer" "$(tbody "$TEEN_ID" "teen$RUN")" "$OLI"
T0=$(jq -r .id "$SB")
expect 200 POST "/api/transfers/$T0/cancel" '' "$OLI"
q "UPDATE guilds SET family = false WHERE id = $GO" >/dev/null

sstep "transfer by the owner: decline tells the owner; cancel; one at a time"
M2=$(mail_n)
expect 200 POST "/api/guilds/$GO/transfer" "$(tbody "$RAY_ID" "ray$RUN")" "$OLI"
T1=$(jq -r .id "$SB"); jq -e '.status == "awaiting_recipient" and .route == "owner"' "$SB" >/dev/null
F=$(wait_mail "$(mail_of "ray$RUN")" "wants you to own" "$M2")
jq -e --arg t "$T1" '.text | contains("/app?transfer=" + $t)' "$F" >/dev/null
[[ "$(dm_from_safety "$RAY_ID")" == *"wants you to own"* ]]
expect 200 GET /api/users/@me/transfers "" "$RAY"
jq -e --arg t "$T1" '.incoming[0].id == $t and .incoming[0].from.username != null' "$SB" >/dev/null
expect 200 GET /api/users/@me/transfers "" "$OLI"
jq -e --arg t "$T1" '.outgoing[0].id == $t' "$SB" >/dev/null
expect 409 POST "/api/guilds/$GO/transfer" "$(tbody "$RAY_ID" "ray$RUN")" "$OLI"         # one at a time
expect 404 POST "/api/transfers/$T1/accept" '' "$TEEN"
expect 200 POST "/api/transfers/$T1/decline" '' "$RAY"
wait_mail "$(mail_of "oli$RUN")" "declined" "$M2" >/dev/null
expect 400 POST "/api/transfers/$T1/accept" '' "$RAY"
expect 200 POST "/api/guilds/$GO/transfer" "$(tbody "$RAY_ID" "ray$RUN")" "$OLI"
T2=$(jq -r .id "$SB")
expect 404 POST "/api/transfers/$T2/cancel" '' "$RAY"
expect 200 POST "/api/transfers/$T2/cancel" '' "$OLI"
jq -e '.status == "cancelled"' "$SB" >/dev/null

sstep "transfer by the owner: accepted, the old owner keeps an Administrator role, everyone told"
M3=$(mail_n)
expect 200 POST "/api/guilds/$GO/transfer" "$(tbody "$RAY_ID" "ray$RUN" password123 true)" "$OLI"
T3=$(jq -r .id "$SB")
expect 200 POST "/api/transfers/$T3/accept" '' "$RAY"
jq -e '.status == "completed"' "$SB" >/dev/null
[ "$(q "SELECT owner_id FROM guilds WHERE id = $GO")" = "$RAY_ID" ]
[ "$(q "SELECT count(*) FROM member_roles mr JOIN roles r ON r.id = mr.role_id WHERE mr.guild_id = $GO AND mr.user_id = $OLI_ID AND r.name = 'Administrator' AND (r.permissions & 1099511627776) <> 0")" = 1 ]
wait_mail "$(mail_of "ray$RUN")" "You now own" "$M3" >/dev/null
wait_mail "$(mail_of "oli$RUN")" "has a new owner" "$M3" >/dev/null
[ "$(q "SELECT count(*) FROM audit_log WHERE guild_id = $GO AND action = 'owner_transfer'")" = 1 ]
[ "$(q "SELECT count(*) FROM safety_log WHERE guild_id = $GO AND action = 'transfer_completed'")" = 1 ]
expect 403 POST "/api/guilds/$GO/transfer" "$(tbody "$TEEN_ID" "teen$RUN")" "$OLI"     # not the owner now

sstep "transfer by the owner: two-factor code needed; blocked while shut down or with an open child-safety report"
TESS=$(new_user "tess$RUN"); TESS_S=$(two_factor "$TESS")
GTS=$(guild "$TESS" "Tess Shed $RUN"); CHT=$(first_channel "$GTS")
expect 200 POST "/api/guilds/$GTS/invites" "" "$TESS"; expect 200 POST "/api/invites/$(jq -r .code "$SB")" "" "$RAY"
expect 400 POST "/api/guilds/$GTS/transfer" "$(tbody "$RAY_ID" "ray$RUN")" "$TESS"
grep -q "authenticator" "$SB"
expect 200 POST "/api/guilds/$GTS/transfer" "$(jq -nc --arg u "$RAY_ID" --arg c "ray$RUN" --arg k "$(totp "$TESS_S" 0)" '{user_id:$u,confirm_username:$c,password:"password123",code:$k}')" "$TESS"
expect 200 POST "/api/transfers/$(jq -r .id "$SB")/cancel" '' "$TESS"
RM=$(say "$RAY" "$CHT" "child report $RUN")
RC=$(file_report "$TESS" message "$RM" child_safety)
expect 409 POST "/api/guilds/$GTS/transfer" "$(jq -nc --arg u "$RAY_ID" --arg c "ray$RUN" --arg k "$(totp "$TESS_S" 1)" '{user_id:$u,confirm_username:$c,password:"password123",code:$k}')" "$TESS"
grep -q "child-safety" "$SB"
expect 200 POST "/api/safety/reports/$RC/actions" '{"action":"dismiss","text":"test"}' "$OWNER_T"
expect 200 POST "/api/safety/outhouses/$GTS/actions" '{"action":"shut_down","reason":"test"}' "$OWNER_T"
expect 400 POST "/api/safety/outhouses/$GTS/transfer" "{\"route\":\"verified\",\"username\":\"ray$RUN\",\"reason\":\"x\"}" "$OWNER_T"
grep -q "shut down" "$SB"
expect 200 POST "/api/safety/outhouses/$GTS/actions" '{"action":"restore"}' "$OWNER_T"

sstep "staff transfer, inactive owner: 90+ days away, an admin who asked; 'I'm still here' stops it"
OLAF=$(new_user "olaf$RUN"); OLAF_ID=$(uid_of "olaf$RUN")
ADA=$(new_user "ada$RUN"); ADA_ID=$(uid_of "ada$RUN")
GI=$(guild "$OLAF" "Old Shed $RUN")
for t in "$ADA" "$RAY"; do expect 200 POST "/api/guilds/$GI/invites" "" "$OLAF"; expect 200 POST "/api/invites/$(jq -r .code "$SB")" "" "$t"; done
expect 200 POST "/api/guilds/$GI/roles" '{"name":"Admin","permissions":"1099511627776"}' "$OLAF"
AROLE=$(jq -r .id "$SB")
expect 204 PUT "/api/guilds/$GI/members/$ADA_ID/roles/$AROLE" "" "$OLAF"
ib() { jq -nc --arg u "$1" --arg r "${2:-inactive}" '{route:$r,username:$u,reason:"Ada asked in #help: the owner left the game a year ago"}'; }
expect 403 POST "/api/safety/outhouses/$GI/transfer" "$(ib "ada$RUN")" "$MOD_NEW"      # moderators can't
expect 400 POST "/api/safety/outhouses/$GI/transfer" "$(ib "ada$RUN")" "$SEN"
grep -q "days" "$SB"
q "UPDATE users SET last_active_at = now() - interval '100 days' WHERE id = $OLAF_ID" >/dev/null
expect 400 POST "/api/safety/outhouses/$GI/transfer" "$(ib "ray$RUN")" "$SEN"          # not an admin
grep -q "admin" "$SB"
expect 400 POST "/api/safety/outhouses/$GI/transfer" '{"route":"inactive","username":"ada'"$RUN"'","reason":""}' "$SEN"
M4=$(mail_n)
expect 200 POST "/api/safety/outhouses/$GI/transfer" "$(ib "ada$RUN")" "$SEN"
TI=$(jq -r .id "$SB"); jq -e '.status == "objection" and .route == "inactive" and .objection_until != null' "$SB" >/dev/null
F=$(wait_mail "$(mail_of "olaf$RUN")" "still yours" "$M4")
jq -e '.text | test("I.m still here")' "$F" >/dev/null
expect 200 GET /api/users/@me/transfers "" "$OLAF"
jq -e --arg t "$TI" '.objections[0].id == $t' "$SB" >/dev/null
expect 200 GET "/api/safety/outhouses/$GI" "" "$SEN"
jq -e --arg t "$TI" '.transfer.id == $t and .owner.inactive_days >= 90' "$SB" >/dev/null
expect 404 POST "/api/transfers/$TI/still-here" '' "$ADA"
expect 200 POST "/api/transfers/$TI/still-here" '' "$OLAF"
jq -e '.status == "objected"' "$SB" >/dev/null
[ "$(q "SELECT owner_id FROM guilds WHERE id = $GI")" = "$OLAF_ID" ]

sstep "staff transfer, inactive owner: the new owner accepts; it completes when the window ends"
q "UPDATE users SET last_active_at = now() - interval '120 days' WHERE id = $OLAF_ID" >/dev/null
expect 200 POST "/api/safety/outhouses/$GI/transfer" "$(ib "ada$RUN")" "$OWNER_T"
TI2=$(jq -r .id "$SB")
expect 200 POST "/api/transfers/sweep" '' "$OWNER_T"
[ "$(q "SELECT status FROM ownership_transfers WHERE id = $TI2")" = objection ]     # window still open
expect 200 POST "/api/transfers/$TI2/accept" '' "$ADA"
jq -e '.status == "objection" and .recipient_accepted' "$SB" >/dev/null
[ "$(q "SELECT owner_id FROM guilds WHERE id = $GI")" = "$OLAF_ID" ]
q "UPDATE ownership_transfers SET objection_until = now() - interval '1 minute' WHERE id = $TI2" >/dev/null
expect 403 POST "/api/transfers/sweep" '' "$SEN"
expect 200 POST "/api/transfers/sweep" '' "$OWNER_T"
jq -e '.completed == 1' "$SB" >/dev/null
[ "$(q "SELECT owner_id FROM guilds WHERE id = $GI")" = "$ADA_ID" ]

sstep "staff transfer, email check: a single-use link to the owner's confirmed email"
VAL=$(new_user "val$RUN"); VAL_ID=$(uid_of "val$RUN")
VERN=$(new_user "vern$RUN")
GV=$(guild "$VERN" "Vern Shed $RUN")
expect 200 POST "/api/guilds/$GV/invites" "" "$VERN"; expect 200 POST "/api/invites/$(jq -r .code "$SB")" "" "$VAL"
M5=$(mail_n)
expect 200 POST "/api/safety/outhouses/$GV/transfer" "$(ib "val$RUN" verified)" "$SEN2"
TV=$(jq -r .id "$SB"); jq -e '.status == "awaiting_owner"' "$SB" >/dev/null
F=$(wait_mail "$(mail_of "vern$RUN")" "Confirm the transfer" "$M5")
jq -e --arg u "val$RUN" '.text | contains("to @" + $u)' "$F" >/dev/null
TOK=$(link_token "$F"); [ "${#TOK}" = 64 ]
[ "$(q "SELECT count(*) FROM ownership_transfers WHERE owner_token_hash = '$TOK'")" = 0 ]   # only the hash is kept
curl -fsS "$BASE/confirm-transfer?token=$TOK" -o /tmp/staff-page && grep -q 'src="/email-link.js"' /tmp/staff-page
expect 200 POST /api/transfers/confirm-owner/check "{\"token\":\"$TOK\"}"
jq -e --arg u "val$RUN" '.to_username == $u and (.guild_name | test("Vern"))' "$SB" >/dev/null
expect 200 POST /api/transfers/confirm-owner "{\"token\":\"$TOK\"}"
expect 400 POST /api/transfers/confirm-owner "{\"token\":\"$TOK\"}"                   # single use
grep -q "expired or was already used" "$SB"
expect 200 POST "/api/transfers/$TV/accept" '' "$VAL"
[ "$(q "SELECT owner_id FROM guilds WHERE id = $GV")" = "$VAL_ID" ]

sstep "staff transfer, email check: links expire; no confirmed email → unavailable"
expect 200 POST "/api/guilds/$GV/invites" "" "$VAL"; expect 200 POST "/api/invites/$(jq -r .code "$SB")" "" "$VERN"
M6=$(mail_n)
expect 200 POST "/api/safety/outhouses/$GV/transfer" "$(ib "vern$RUN" verified)" "$OWNER_T"
TV2=$(jq -r .id "$SB")
TOK2=$(link_token "$(wait_mail "$(mail_of "val$RUN")" "Confirm the transfer" "$M6")")
q "UPDATE ownership_transfers SET deadline = now() - interval '1 minute' WHERE id = $TV2" >/dev/null
expect 400 POST /api/transfers/confirm-owner "{\"token\":\"$TOK2\"}"
expect 200 POST /api/transfers/sweep '' "$OWNER_T"
[ "$(q "SELECT status FROM ownership_transfers WHERE id = $TV2")" = expired ]
q "UPDATE users SET email_verified_at = NULL WHERE id = $VAL_ID" >/dev/null
expect 400 POST "/api/safety/outhouses/$GV/transfer" "$(ib "vern$RUN" verified)" "$OWNER_T"
grep -q "no confirmed email" "$SB"
q "UPDATE users SET email_verified_at = now() WHERE id = $VAL_ID" >/dev/null

sstep "staff transfers: conflicts of interest and the site owner's outhouses refused; staff can cancel"
GSEN=$(guild "$SEN2" "Senior Shed $RUN")
expect 200 POST "/api/guilds/$GSEN/invites" "" "$SEN2"; expect 200 POST "/api/invites/$(jq -r .code "$SB")" "" "$RAY"
expect 403 POST "/api/safety/outhouses/$GSEN/transfer" "$(ib "ray$RUN" verified)" "$SEN2"   # their own outhouse
grep -q "Someone else" "$SB"
expect 403 POST "/api/safety/outhouses/$G2/transfer" "$(ib "rep$RUN" verified)" "$SEN2"     # the site owner's outhouse
expect 403 POST "/api/safety/outhouses/$GSEN/transfer" "$(ib "ray$RUN" verified)" "$SEN"     # staff of the same tier
grep -q "your level" "$SB"
expect 200 POST "/api/safety/outhouses/$GV/transfer" "$(ib "vern$RUN" verified)" "$SEN"
TC=$(jq -r .id "$SB")
expect 404 POST "/api/transfers/$TC/cancel" '' "$RAY"
expect 404 POST "/api/transfers/$TC/cancel" '' "$VERN"   # only staff (or the old owner on their own offers) cancel
expect 200 POST "/api/transfers/$TC/cancel" '' "$SEN"
jq -e '.status == "cancelled"' "$SB" >/dev/null
[ "$(q "SELECT count(*) FROM safety_log WHERE action = 'transfer_started' AND staff_perm = 'transfers' AND actor_id = $SEN_ID")" -ge 2 ]

echo "Staff test passed."
