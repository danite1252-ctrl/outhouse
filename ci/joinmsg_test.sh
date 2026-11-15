#!/usr/bin/env bash
# Join and leave messages ("Knock knock — {name} just took the last free
# stall."), end to end against a real server and Postgres. ci/smoke.sh runs
# it; it can also run on its own against a local server:
#
#   BASE=http://127.0.0.1:8080 DATABASE_URL=postgres://... bash ci/joinmsg_test.sh
set -euo pipefail
trap 'echo "Join messages test failed at line $LINENO"' ERR

BASE=${BASE:-http://127.0.0.1:8080}
RUN=${JOINMSG_RUN:-$(date +%s | tail -c 6)}
JB=/tmp/joinmsg-body
jstep() { echo "== join messages: $*"; }

# req TOKEN METHOD PATH [JSON] -> HTTP status (body in $JB)
req() {
  local token=$1 method=$2 path=$3 body=${4:-}
  local args=(-sS -X "$method" -o "$JB" -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  [ -n "$body" ] && args+=(-d "$body")
  curl "${args[@]}" "$BASE$path"
}
ok() {
  local code; code=$(req "$@")
  if [[ "$code" != 2* ]]; then echo "FAILED: $2 $3 -> $code"; cat "$JB"; echo; return 1; fi
  cat "$JB"
}
no() {
  local want=$1; shift
  local code; code=$(req "$@")
  if [ "$code" != "$want" ]; then echo "EXPECTED $want: $2 $3 -> $code"; cat "$JB"; echo; return 1; fi
}
N=0
reg() {
  N=$((N + 1))
  curl -fsS -H "Content-Type: application/json" -H "X-Forwarded-For: 198.18.48.$N" \
    -d '{"username":"'"$1$RUN"'","email":"'"$1$RUN"'@example.test","password":"password123","birth_date":"1990-01-01","display_name":"'"$2"'"}' \
    "$BASE/api/auth/register"
}
sql() { psql "$DATABASE_URL" -tAc "$1"; }
# The system messages in a channel, oldest first.
sys_in() { ok "$1" GET "/api/channels/$2/messages?limit=100" | jq '[.[] | select(.system != null)] | sort_by(.id)'; }
# A fresh invite (unlimited uses).
invite() { ok "$OT" POST "/api/guilds/$G/invites" '{}' | jq -r .code; }

R=$(reg jmowner "Porchlight"); OT=$(jq -r .token <<<"$R")
R=$(reg jmbob "Bob Dunny"); BT=$(jq -r .token <<<"$R"); BU=$(jq -r .user.id <<<"$R")
R=$(reg jmcat "Cat"); CT=$(jq -r .token <<<"$R"); CU=$(jq -r .user.id <<<"$R")
R=$(reg jmdee "Dee"); DT=$(jq -r .token <<<"$R"); DU=$(jq -r .user.id <<<"$R")

G=$(ok "$OT" POST /api/guilds '{"name":"Join Msg '"$RUN"'"}' | jq -r .id)
PORCH=$(ok "$OT" GET "/api/guilds/$G/channels" | jq -r '[.[] | select(.kind == 0)][0].id')

jstep "settings: defaults (on, first channel everyone can see, wave on, leaves off)"
ok "$OT" GET "/api/guilds/$G/welcome" | jq -e --arg p "$PORCH" \
  '.join_messages and .wave and (.leave_messages | not) and .channel_id == null and .effective_channel_id == $p and (.channel_choices | index($p))' >/dev/null
# Members without Manage Outhouse can't see or change them.
CODE=$(invite)
ok "$BT" POST "/api/invites/$CODE" >/dev/null
no 403 "$BT" GET "/api/guilds/$G/welcome"
no 403 "$BT" PUT "/api/guilds/$G/welcome" '{"join_messages":false}'

jstep "joining by invite posts one line in the front porch, with a stored template"
J=$(sys_in "$OT" "$PORCH")
jq -e --arg u "$BU" 'length == 1 and .[0].author_id == $u and .[0].system.kind == "member_join"
  and (.[0].system.template >= 0 and .[0].system.template < 12) and (.[0].system.text | contains("{name}"))
  and .[0].system.wave and (.[0].content | contains("Bob Dunny")) and (.[0].content | contains("<@") | not)' <<<"$J" >/dev/null
JID=$(jq -r '.[0].id' <<<"$J"); TPL=$(jq -r '.[0].system.template' <<<"$J")
# Stored on the message, so it reads the same on reload.
[ "$(sql "SELECT system->>'template' FROM messages WHERE id = $JID")" = "$TPL" ]
[ "$(sql "SELECT system->>'kind' FROM messages WHERE id = $JID")" = "member_join" ]
sys_in "$BT" "$PORCH" | jq -e --arg id "$JID" --argjson t "$TPL" '.[0].id == $id and .[0].system.template == $t' >/dev/null
# The text is one of the 12 templates with the display name in it.
TEXT=$(jq -r '.[0].system.text' <<<"$J"); CONTENT=$(jq -r '.[0].content' <<<"$J")
[ "${TEXT//\{name\}/Bob Dunny}" = "$CONTENT" ]
grep -qF "$TEXT" crates/server/src/api/welcome.rs
# Nobody's mentions inbox gets it.
ok "$OT" GET "/api/users/@me/mentions?limit=50" | jq -e --arg id "$JID" '[.mentions[] | select(.message.id == $id)] | length == 0' >/dev/null

jstep "not editable; only moderators can delete; reactions and a wave reply work"
no 400 "$BT" PATCH "/api/channels/$PORCH/messages/$JID" '{"content":"I rewrote history"}'
no 403 "$BT" DELETE "/api/channels/$PORCH/messages/$JID"
ok "$OT" PUT "/api/channels/$PORCH/messages/$JID/reactions/%F0%9F%91%8B" >/dev/null
ok "$OT" POST "/api/channels/$PORCH/messages" '{"content":"👋","reply_to_id":"'"$JID"'"}' | jq -e --arg id "$JID" '.reply_to.id == $id' >/dev/null
ok "$OT" GET "/api/channels/$PORCH/messages?limit=100" | jq -e --arg id "$JID" '[.[] | select(.id == $id)][0].reactions | length == 1' >/dev/null

jstep "leaving and rejoining within 5 minutes doesn't post again"
ok "$BT" POST "/api/guilds/$G/leave" >/dev/null
ok "$BT" POST "/api/invites/$(invite)" >/dev/null
sys_in "$OT" "$PORCH" | jq -e 'length == 1' >/dev/null
# ...but after 5 minutes it does.
sql "UPDATE welcome_posts SET posted_at = now() - interval '6 minutes' WHERE guild_id = $G AND user_id = $BU" >/dev/null
ok "$BT" POST "/api/guilds/$G/leave" >/dev/null
ok "$BT" POST "/api/invites/$(invite)" >/dev/null
sys_in "$OT" "$PORCH" | jq -e --arg u "$BU" 'length == 2 and .[1].author_id == $u' >/dev/null

jstep "switched off: nothing"
ok "$OT" PUT "/api/guilds/$G/welcome" '{"join_messages":false}' | jq -e '.join_messages == false and .wave' >/dev/null
ok "$CT" POST "/api/invites/$(invite)" >/dev/null
sys_in "$OT" "$PORCH" | jq -e 'length == 2' >/dev/null
ok "$OT" PUT "/api/guilds/$G/welcome" '{"join_messages":true}' >/dev/null
ok "$OT" GET "/api/guilds/$G/audit-log" | jq -e '[.. | objects | select(.action? == "welcome_update")] | length >= 2' >/dev/null

jstep "the channel picker is respected (and must be a channel everyone can see)"
HALL=$(ok "$OT" POST "/api/guilds/$G/channels" '{"name":"welcome-mat"}' | jq -r .id)
SECRET=$(ok "$OT" POST "/api/guilds/$G/channels" '{"name":"staff-dunny"}' | jq -r .id)
ok "$OT" PUT "/api/channels/$SECRET/permissions/$G" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
no 400 "$OT" PUT "/api/guilds/$G/welcome" '{"channel_id":"'"$SECRET"'"}'
no 400 "$OT" PUT "/api/guilds/$G/welcome" '{"channel_id":"12345"}'
ok "$OT" PUT "/api/guilds/$G/welcome" '{"channel_id":"'"$HALL"'"}' | jq -e --arg h "$HALL" '.channel_id == $h and .effective_channel_id == $h' >/dev/null
ok "$DT" POST "/api/invites/$(invite)" >/dev/null
sys_in "$OT" "$HALL" | jq -e --arg u "$DU" 'length == 1 and .[0].author_id == $u' >/dev/null
sys_in "$OT" "$PORCH" | jq -e 'length == 2' >/dev/null

jstep "leave messages (off by default, then on; kicks don't count)"
ok "$DT" POST "/api/guilds/$G/leave" >/dev/null
sys_in "$OT" "$HALL" | jq -e 'length == 1' >/dev/null
ok "$OT" PUT "/api/guilds/$G/welcome" '{"leave_messages":true}' | jq -e '.leave_messages' >/dev/null
ok "$CT" POST "/api/guilds/$G/leave" >/dev/null
L=$(sys_in "$OT" "$HALL")
jq -e --arg u "$CU" 'length == 2 and .[1].author_id == $u and .[1].system.kind == "member_leave"
  and .[1].system.template < 4 and (.[1].system.wave | not)' <<<"$L" >/dev/null
# A kick posts nothing.
ok "$DT" POST "/api/invites/$(invite)" >/dev/null   # Dee's join within 5 minutes: no new line
ok "$OT" DELETE "/api/guilds/$G/members/$DU" >/dev/null
sys_in "$OT" "$HALL" | jq -e 'length == 2' >/dev/null

jstep "bots get their own line"
ok "$OT" POST "/api/guilds/$G/bots" '{"name":"Plunger Bot"}' >/dev/null
sys_in "$OT" "$HALL" | jq -e 'length == 3 and .[2].system.kind == "bot_join" and (.[2].content | startswith("🤖 Plunger Bot"))' >/dev/null

jstep "the chosen channel is deleted (or hidden): nothing is posted"
ok "$OT" DELETE "/api/channels/$HALL" >/dev/null
ok "$OT" GET "/api/guilds/$G/welcome" | jq -e --arg h "$HALL" '.channel_id == $h and .effective_channel_id == null' >/dev/null
sql "DELETE FROM welcome_posts WHERE guild_id = $G" >/dev/null
ok "$CT" POST "/api/invites/$(invite)" >/dev/null
sys_in "$OT" "$PORCH" | jq -e 'length == 2' >/dev/null
# Back to the default: the next join goes in the front porch again.
ok "$OT" PUT "/api/guilds/$G/welcome" '{"channel_id":null}' | jq -e --arg p "$PORCH" '.channel_id == null and .effective_channel_id == $p' >/dev/null
ok "$OT" PUT "/api/channels/$PORCH/permissions/$G" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
ok "$OT" GET "/api/guilds/$G/welcome" | jq -e '.effective_channel_id == null' >/dev/null
ok "$OT" DELETE "/api/channels/$PORCH/permissions/$G" >/dev/null

jstep "a moderator can delete one"
ok "$OT" DELETE "/api/channels/$PORCH/messages/$JID" >/dev/null
sys_in "$OT" "$PORCH" | jq -e 'length == 1' >/dev/null

echo "Join messages test passed."
