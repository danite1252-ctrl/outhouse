#!/usr/bin/env bash
# Starts the real server against Postgres and walks through the main API
# flows, so SQL and routing mistakes fail CI instead of failing in front of
# people. Add a section here for each new feature.
set -euo pipefail
trap 'echo "Smoke test failed at line $LINENO"' ERR

BASE=http://127.0.0.1:8080
LOG=/tmp/outhouse-server.log
./target/debug/outhouse-server >"$LOG" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT

for i in $(seq 1 60); do
  curl -fsS "$BASE/api/health" >/dev/null 2>&1 && break
  if ! kill -0 $SERVER 2>/dev/null; then echo "server exited:"; cat "$LOG"; exit 1; fi
  sleep 1
done

TOKEN=""
# call METHOD PATH [JSON]  -> prints the body; fails on any non-2xx
call() {
  local method=$1 path=$2 body=${3:-}
  local args=(-sS -X "$method" -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$TOKEN" ] && args+=(-H "Authorization: Bearer $TOKEN")
  [ -n "$body" ] && args+=(-d "$body")
  local code
  code=$(curl "${args[@]}" "$BASE$path")
  if [[ "$code" != 2* ]]; then
    echo "FAILED: $method $path -> $code"; cat /tmp/body; echo; exit 1
  fi
  cat /tmp/body
}
step() { echo "== $*"; }

step "register two users"
A=$(call POST /api/auth/register '{"username":"alice","password":"password123","display_name":"Alice"}')
TOKEN_A=$(jq -r .token <<<"$A"); ALICE=$(jq -r .user.id <<<"$A")
B=$(call POST /api/auth/register '{"username":"bob","password":"password123","display_name":"Bob"}')
TOKEN_B=$(jq -r .token <<<"$B"); BOB=$(jq -r .user.id <<<"$B")
TOKEN=$TOKEN_A
call GET /api/users/@me >/dev/null
call PATCH /api/users/@me '{"display_name":"Alice W"}' >/dev/null

step "guild, channels, invite"
G=$(call POST /api/guilds '{"name":"Smoke Test"}'); GUILD=$(jq -r .id <<<"$G")
C=$(call POST "/api/guilds/$GUILD/channels" '{"name":"general"}'); CHAN=$(jq -r .id <<<"$C")
call POST "/api/guilds/$GUILD/channels" '{"name":"Voice","kind":2}' >/dev/null
call GET "/api/guilds/$GUILD/channels" >/dev/null
INV=$(call POST "/api/guilds/$GUILD/invites"); CODE=$(jq -r .code <<<"$INV")
TOKEN=$TOKEN_B; call POST "/api/invites/$CODE" >/dev/null; TOKEN=$TOKEN_A
call GET "/api/guilds/$GUILD/members" | jq -e 'length == 2' >/dev/null
call PUT "/api/guilds/$GUILD/nickname" '{"nickname":"Ally"}' >/dev/null

step "roles and permissions"
R=$(call POST "/api/guilds/$GUILD/roles" '{"name":"Raider"}'); ROLE=$(jq -r .id <<<"$R")
call PUT "/api/guilds/$GUILD/members/$BOB/roles/$ROLE" >/dev/null
call GET "/api/guilds/$GUILD/permissions" >/dev/null

step "messages, replies, reactions, edits"
M=$(call POST "/api/channels/$CHAN/messages" '{"content":"hello <@'"$BOB"'>"}'); MSG=$(jq -r .id <<<"$M")
call POST "/api/channels/$CHAN/messages" '{"content":"a reply","reply_to_id":"'"$MSG"'"}' | jq -e '.reply_to.author_display_name == "Ally"' >/dev/null
call PUT "/api/channels/$CHAN/messages/$MSG/reactions/%F0%9F%91%8D" >/dev/null
call PATCH "/api/channels/$CHAN/messages/$MSG" '{"content":"hello again"}' >/dev/null
call GET "/api/channels/$CHAN/messages" | jq -e 'length >= 2' >/dev/null
call POST "/api/channels/$CHAN/ack" '{"message_id":"'"$MSG"'"}' >/dev/null

step "webhooks"
W=$(call POST "/api/channels/$CHAN/webhooks" '{"name":"News"}')
WID=$(jq -r .id <<<"$W"); WTOK=$(jq -r .token <<<"$W")
TOKEN=""
call POST "/api/webhooks/$WID/$WTOK" '{"content":"from a webhook"}' >/dev/null
TOKEN=$TOKEN_A

# fail_call METHOD PATH [JSON] -> succeeds only if the request is refused (4xx)
fail_call() {
  local method=$1 path=$2 body=${3:-}
  local args=(-sS -X "$method" -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json")
  [ -n "$TOKEN" ] && args+=(-H "Authorization: Bearer $TOKEN")
  [ -n "$body" ] && args+=(-d "$body")
  local code
  code=$(curl "${args[@]}" "$BASE$path")
  if [[ "$code" != 4* ]]; then
    echo "EXPECTED REFUSAL: $method $path -> $code"; cat /tmp/body; echo; exit 1
  fi
}
enc() { jq -rn --arg v "$1" '$v|@uri'; }

step "pinned messages"
P=$(call POST "/api/guilds/$GUILD/channels" '{"name":"pins"}'); PINS=$(jq -r .id <<<"$P")
P1=$(call POST "/api/channels/$PINS/messages" '{"content":"Raid rules: no ninja looting"}' | jq -r .id)
TOKEN=$TOKEN_B; fail_call PUT "/api/channels/$PINS/pins/$P1"; TOKEN=$TOKEN_A
call PUT "/api/channels/$PINS/pins/$P1" | jq -e '.pinned_at != null' >/dev/null
call PUT "/api/channels/$PINS/pins/$P1" >/dev/null # pinning twice is fine
TOKEN=$TOKEN_B; call GET "/api/channels/$PINS/pins" | jq -e --arg id "$P1" 'length == 1 and .[0].id == $id' >/dev/null; TOKEN=$TOKEN_A
call GET "/api/channels/$PINS/messages" | jq -e --arg id "$P1" '.[] | select(.id == $id) | .pinned_at != null' >/dev/null
fail_call PUT "/api/channels/$CHAN/pins/$P1" # message from another channel
for i in $(seq 2 50); do
  id=$(call POST "/api/channels/$PINS/messages" '{"content":"pin '"$i"'"}' | jq -r .id)
  call PUT "/api/channels/$PINS/pins/$id" >/dev/null
done
call GET "/api/channels/$PINS/pins" | jq -e 'length == 50' >/dev/null
EXTRA=$(call POST "/api/channels/$PINS/messages" '{"content":"one too many"}' | jq -r .id)
fail_call PUT "/api/channels/$PINS/pins/$EXTRA"
TOKEN=$TOKEN_B; fail_call DELETE "/api/channels/$PINS/pins/$P1"; TOKEN=$TOKEN_A
call DELETE "/api/channels/$PINS/pins/$P1" | jq -e '.pinned_at == null' >/dev/null
call GET "/api/channels/$PINS/pins" | jq -e 'length == 49' >/dev/null
call PUT "/api/channels/$PINS/pins/$EXTRA" >/dev/null

step "history around a message"
call GET "/api/channels/$PINS/messages?around=$P1&limit=10" | jq -e --arg id "$P1" 'length >= 5 and (.[0].id == $id) and ([.[] | select(.id == $id)] | length == 1)' >/dev/null
MID=$(call GET "/api/channels/$PINS/messages?limit=30" | jq -r '.[0].id')
call GET "/api/channels/$PINS/messages?around=$MID&limit=10" | jq -e --arg id "$MID" 'length == 10 and .[4].id == $id' >/dev/null

step "message search"
call POST "/api/channels/$CHAN/messages" '{"content":"Who has the Onyxia key? https://wowhead.com/onyxia"}' >/dev/null
TOKEN=$TOKEN_B; call POST "/api/channels/$CHAN/messages" '{"content":"onyxia attunement done"}' >/dev/null; TOKEN=$TOKEN_A
call GET "/api/guilds/$GUILD/search?q=onyx" | jq -e '.total == 2 and (.messages | length == 2) and .terms == ["onyx"]' >/dev/null
call GET "/api/guilds/$GUILD/search?q=$(enc 'onyxia from:@bob')" | jq -e --arg id "$BOB" '.total == 1 and .messages[0].author_id == $id' >/dev/null
call GET "/api/guilds/$GUILD/search?q=$(enc 'onyxia has:link')" | jq -e '.total == 1' >/dev/null
call GET "/api/guilds/$GUILD/search?q=$(enc 'in:#pins ninja')" | jq -e '.total == 1' >/dev/null
call GET "/api/guilds/$GUILD/search?q=$(enc 'onyxia in:#pins')" | jq -e '.total == 0' >/dev/null
call GET "/api/guilds/$GUILD/search?q=$(enc 'from:@nobody-here')" | jq -e '.total == 0' >/dev/null
call GET "/api/guilds/$GUILD/search?q=$(enc 'pin')&limit=5&offset=5" | jq -e '.total >= 49 and (.messages | length == 5)' >/dev/null
call GET "/api/guilds/$GUILD/search?q=$(enc "raid's & (loot) | !:*")" >/dev/null # odd characters can't break the query
fail_call GET "/api/guilds/$GUILD/search?q="
# A private channel's messages stay hidden from people who can't see it.
S=$(call POST "/api/guilds/$GUILD/channels" '{"name":"officers"}'); SECRET=$(jq -r .id <<<"$S")
call PUT "/api/channels/$SECRET/permissions/$GUILD" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
call POST "/api/channels/$SECRET/messages" '{"content":"secret loot council plans"}' >/dev/null
call GET "/api/guilds/$GUILD/search?q=loot+council" | jq -e '.total == 1' >/dev/null
TOKEN=$TOKEN_B
call GET "/api/guilds/$GUILD/search?q=loot+council" | jq -e '.total == 0' >/dev/null
call GET "/api/users/@me/unread" | jq -e --arg id "$SECRET" '[.channels[] | select(.channel_id == $id)] | length == 0' >/dev/null
TOKEN=$TOKEN_A
OUT=$(call POST /api/auth/register '{"username":"outsider","password":"password123"}' | jq -r .token)
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/search?q=onyxia"; TOKEN=$TOKEN_A

step "unread and mention counts"
PG=$(call POST "/api/guilds/$GUILD/channels" '{"name":"pings"}'); PINGS=$(jq -r .id <<<"$PG")
call POST "/api/channels/$PINGS/messages" '{"content":"hey <@'"$BOB"'>"}' >/dev/null
call POST "/api/channels/$PINGS/messages" '{"content":"@everyone raid in 10"}' >/dev/null
call POST "/api/channels/$PINGS/messages" '{"content":"nothing for bob here"}' >/dev/null
LAST=$(call POST "/api/channels/$PINGS/messages" '{"content":"<@&'"$ROLE"'> assemble"}' | jq -r .id)
TOKEN=$TOKEN_B
call GET "/api/users/@me/unread" | jq -e --arg id "$PINGS" --arg last "$LAST" --arg g "$GUILD" --arg r "$ROLE" '
  (.channels[] | select(.channel_id == $id) | .mention_count == 3 and .last_message_id == $last and .last_read_id == null)
  and (.roles[] | select(.guild_id == $g) | .role_ids == [$r])' >/dev/null
call POST "/api/channels/$PINGS/ack" '{"message_id":"'"$LAST"'"}' >/dev/null
call GET "/api/users/@me/unread" | jq -e --arg id "$PINGS" --arg last "$LAST" '.channels[] | select(.channel_id == $id) | .mention_count == 0 and .last_read_id == $last' >/dev/null
TOKEN=$TOKEN_A
call GET "/api/users/@me/unread" | jq -e --arg id "$PINGS" '.channels[] | select(.channel_id == $id) | .mention_count == 0' >/dev/null

step "voice token"
VOICE=$(call GET "/api/guilds/$GUILD/channels" | jq -r '.[] | select(.kind == 2) | .id' | head -1)
call POST "/api/channels/$VOICE/voice/join" >/dev/null
call POST /api/voice/leave >/dev/null

step "profiles"
call PATCH /api/users/@me '{"about_me":"Tank main","profile_theme":"moonwell","accent_color":16737835}' >/dev/null
call GET "/api/profiles/$ALICE" | jq -e '.about_me == "Tank main" and .profile_theme == "moonwell"' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/profiles/$ALICE" >/dev/null; TOKEN=$TOKEN_A
# A 1x1 PNG
printf 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==' | base64 -d > /tmp/pixel.png
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "file=@/tmp/pixel.png;type=image/png" "$BASE/api/users/@me/avatar")
[[ "$code" == 2* ]] || { echo "avatar upload -> $code"; cat /tmp/body; exit 1; }
AVATAR=$(jq -r .avatar_url /tmp/body)
curl -fsS "$BASE$AVATAR" -o /dev/null
call GET "/api/guilds/$GUILD/members" | jq -e --arg id "$ALICE" '.[] | select(.id == $id) | .avatar_url != null' >/dev/null
call GET "/api/channels/$CHAN/messages" | jq -e --arg id "$ALICE" '[.[] | select(.author_id == $id)] | .[-1].author_avatar_url != null' >/dev/null
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "file=@/tmp/pixel.png;type=image/png" "$BASE/api/users/@me/banner")
[[ "$code" == 2* ]] || { echo "banner upload -> $code"; cat /tmp/body; exit 1; }
call DELETE /api/users/@me/avatar | jq -e '.avatar_url == null' >/dev/null

step "raid planner"
START=$(date -u -d '+2 days' +%Y-%m-%dT20:00:00Z)
E=$(call POST "/api/channels/$CHAN/events" '{"title":"Molten Core","description":"Heroic, bring flasks","starts_at":"'"$START"'","duration_minutes":180,"max_tanks":2,"max_healers":4,"max_dps":14}')
EVENT=$(jq -r .event.id <<<"$E")
call PUT "/api/events/$EVENT/signup" '{"status":"yes","role":"tank","class":"Paladin","spec":"Protection"}' >/dev/null
TOKEN=$TOKEN_B
call PUT "/api/events/$EVENT/signup" '{"status":"tentative","role":"healer","class":"Priest","spec":"Holy","note":"might be late"}' | jq -e '.event.signups | length == 2' >/dev/null
if (call PATCH "/api/events/$EVENT" '{"title":"nope"}') >/dev/null 2>&1; then echo "non-organiser could edit"; exit 1; fi
TOKEN=$TOKEN_A
call PUT "/api/events/$EVENT/signup" '{"status":"bench","role":"healer","class":"Priest","spec":"Holy","user_id":"'"$BOB"'"}' >/dev/null
call PATCH "/api/events/$EVENT" '{"title":"Molten Core (40)"}' | jq -e '.event.title == "Molten Core (40)"' >/dev/null
call GET "/api/guilds/$GUILD/events" | jq -e 'length == 1' >/dev/null
call GET "/api/channels/$CHAN/messages" | jq -e '[.[] | select(.event != null)] | length == 1' >/dev/null
call DELETE "/api/events/$EVENT/signups/$ALICE" >/dev/null
call GET "/api/events/$EVENT" | jq -e '.signups | length == 1' >/dev/null
# A raid starting now gets a reminder within a minute or so (checked at the end).
SOON=$(date -u -d '+5 minutes' +%Y-%m-%dT%H:%M:%SZ)
R2=$(call POST "/api/channels/$CHAN/events" '{"title":"Reminder test","starts_at":"'"$SOON"'"}')
call PUT "/api/events/$(jq -r .event.id <<<"$R2")/signup" '{"status":"yes","role":"dps","class":"Mage","spec":"Fire"}' >/dev/null

step "bots and slash commands"
BOTRES=$(call POST "/api/guilds/$GUILD/bots" '{"name":"Dice"}')
BOT_TOKEN=$(jq -r .token <<<"$BOTRES"); BOT_ID=$(jq -r .bot.id <<<"$BOTRES")
TOKEN=$BOT_TOKEN
call GET /api/users/@me | jq -e '.bot == true' >/dev/null
TOKEN=$TOKEN_A
OUTHOUSE_URL=$BASE OUTHOUSE_BOT_TOKEN=$BOT_TOKEN node web/bots/dice-bot.mjs >/tmp/bot.log 2>&1 &
BOT_PID=$!
trap 'kill $SERVER $BOT_PID 2>/dev/null || true' EXIT
for i in $(seq 1 20); do
  call GET "/api/guilds/$GUILD/commands" | jq -e 'length == 3' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/guilds/$GUILD/bots" | jq -e '.[0].online == true and (.[0].commands | length == 3)' >/dev/null || { cat /tmp/bot.log; exit 1; }
ROLL=$(call GET "/api/guilds/$GUILD/commands" | jq -r '.[] | select(.name == "roll") | .id')
call POST "/api/channels/$CHAN/interactions" '{"command_id":"'"$ROLL"'","options":{"dice":"2d6"}}' >/dev/null
for i in $(seq 1 10); do
  call GET "/api/channels/$CHAN/messages" | jq -e '.[-1].interaction.command == "roll"' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/channels/$CHAN/messages" | jq -e '.[-1].interaction.command == "roll" and .[-1].author_bot' >/dev/null || { echo "bot never replied"; cat /tmp/bot.log; exit 1; }
call POST "/api/bots/$BOT_ID/token" >/dev/null
call DELETE "/api/bots/$BOT_ID" >/dev/null
call GET "/api/guilds/$GUILD/commands" | jq -e 'length == 0' >/dev/null

step "scroll-back (before= cursor)"
call GET "/api/channels/$PINS/messages?limit=10&before=$MID" | jq -e --arg m "$MID" 'length == 10 and all(.[]; .id < $m)' >/dev/null
OLDEST=$(call GET "/api/channels/$PINS/messages?limit=10&before=$MID" | jq -r '.[0].id')
call GET "/api/channels/$PINS/messages?limit=10&before=$OLDEST" | jq -e --arg m "$OLDEST" 'length == 10 and all(.[]; .id < $m)' >/dev/null
fail_call GET "/api/channels/$PINS/messages?before=notanid"

step "direct messages"
TOKEN=$OUT; OUTSIDER=$(call GET /api/users/@me | jq -r .id); TOKEN=$TOKEN_A
C3=$(call POST /api/auth/register '{"username":"carl","password":"password123","display_name":"Carl"}')
TOKEN_C=$(jq -r .token <<<"$C3"); CARL=$(jq -r .user.id <<<"$C3")
TOKEN=$TOKEN_C; call POST "/api/invites/$CODE" >/dev/null; TOKEN=$TOKEN_A
call GET /api/users/@me/contacts | jq -e --arg b "$BOB" --arg o "$OUTSIDER" '([.[] | select(.id == $b)] | length == 1) and ([.[] | select(.id == $o)] | length == 0)' >/dev/null
DM=$(call POST /api/dms '{"user_ids":["'"$BOB"'"]}'); DMID=$(jq -r .id <<<"$DM")
jq -e '.kind == 1 and (.recipients | length == 2)' <<<"$DM" >/dev/null
call POST /api/dms '{"user_ids":["'"$BOB"'"]}' | jq -e --arg id "$DMID" '.id == $id' >/dev/null # same DM again
TOKEN=$TOKEN_B; call POST /api/dms '{"user_ids":["'"$ALICE"'"]}' | jq -e --arg id "$DMID" '.id == $id' >/dev/null; TOKEN=$TOKEN_A
DM1=$(call POST "/api/channels/$DMID/messages" '{"content":"psst bob"}' | jq -r .id)
TOKEN=$TOKEN_B
call GET /api/dms | jq -e --arg id "$DMID" --arg m "$DM1" '.[] | select(.id == $id) | .last_message_id == $m' >/dev/null
call GET /api/users/@me/unread | jq -e --arg id "$DMID" '.channels[] | select(.channel_id == $id) | .guild_id == "0" and .mention_count == 1' >/dev/null
call GET "/api/channels/$DMID/messages" | jq -e 'length == 1 and .[0].content == "psst bob"' >/dev/null
call PUT "/api/channels/$DMID/messages/$DM1/reactions/%F0%9F%91%8D" >/dev/null
call POST "/api/channels/$DMID/typing" >/dev/null
call POST "/api/channels/$DMID/ack" '{"message_id":"'"$DM1"'"}' >/dev/null
call GET /api/users/@me/unread | jq -e --arg id "$DMID" '.channels[] | select(.channel_id == $id) | .mention_count == 0' >/dev/null
fail_call DELETE "/api/channels/$DMID/messages/$DM1" # not Bob's message
fail_call PATCH "/api/channels/$DMID/messages/$DM1" '{"content":"edited by bob"}'
fail_call PUT "/api/channels/$DMID/pins/$DM1"
fail_call DELETE "/api/channels/$DMID"
TOKEN=$TOKEN_A
call PATCH "/api/channels/$DMID/messages/$DM1" '{"content":"psst bob (edited)"}' >/dev/null
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "content=a picture" -F "files=@/tmp/pixel.png;type=image/png" "$BASE/api/channels/$DMID/messages/upload")
[[ "$code" == 2* ]] || { echo "DM upload -> $code"; cat /tmp/body; exit 1; }
jq -e '.attachments | length == 1' /tmp/body >/dev/null
START2=$(date -u -d '+2 days' +%Y-%m-%dT20:00:00Z)
fail_call POST "/api/channels/$DMID/events" '{"title":"Nope","starts_at":"'"$START2"'"}'
TOKEN=$OUT
fail_call GET "/api/channels/$DMID/messages"
fail_call POST "/api/channels/$DMID/messages" '{"content":"let me in"}'
fail_call POST /api/dms '{"user_ids":["'"$ALICE"'"]}' # shares no outhouse
TOKEN=$TOKEN_A
fail_call POST /api/dms '{"user_ids":["'"$BOB"'","'"$OUTSIDER"'"]}'
fail_call POST /api/dms '{"user_ids":[]}'
GDM=$(call POST /api/dms '{"user_ids":["'"$BOB"'","'"$CARL"'"],"name":"Raid leads"}'); GDMID=$(jq -r .id <<<"$GDM")
jq -e --arg a "$ALICE" '.kind == 3 and .name == "Raid leads" and (.recipients | length == 3) and .owner_id == $a' <<<"$GDM" >/dev/null
TOKEN=$TOKEN_B; call PATCH "/api/dms/$GDMID" '{"name":"Loot council"}' | jq -e '.name == "Loot council"' >/dev/null; TOKEN=$TOKEN_A
fail_call PATCH "/api/dms/$DMID" '{"name":"no names for one-to-one"}'
call POST "/api/channels/$GDMID/messages" '{"content":"group hello"}' >/dev/null
TOKEN=$TOKEN_C
call GET "/api/channels/$GDMID/messages" | jq -e 'length == 1' >/dev/null
call DELETE "/api/dms/$GDMID/recipients/@me" >/dev/null
fail_call GET "/api/channels/$GDMID/messages"
call GET /api/dms | jq -e --arg id "$GDMID" '[.[] | select(.id == $id)] | length == 0' >/dev/null
TOKEN=$TOKEN_A
call GET /api/dms | jq -e --arg id "$GDMID" '.[] | select(.id == $id) | .recipients | length == 2' >/dev/null
call POST "/api/dms/$GDMID/recipients" '{"user_id":"'"$CARL"'"}' | jq -e '.recipients | length == 3' >/dev/null
fail_call POST "/api/dms/$GDMID/recipients" '{"user_id":"'"$OUTSIDER"'"}'
fail_call DELETE "/api/dms/$DMID/recipients/@me" # can't leave a one-to-one

step "threads"
TM=$(call POST "/api/channels/$CHAN/messages" '{"content":"Who is tanking Rag tonight?"}' | jq -r .id)
T=$(call POST "/api/channels/$CHAN/threads" '{"name":"","message_id":"'"$TM"'"}'); THREAD=$(jq -r .id <<<"$T")
jq -e --arg m "$TM" '.name == "Who is tanking Rag tonight?" and .message_count == 0 and .starter_message_id == $m' <<<"$T" >/dev/null
call POST "/api/channels/$CHAN/threads" '{"name":"again","message_id":"'"$TM"'"}' | jq -e --arg id "$THREAD" '.id == $id' >/dev/null
TOKEN=$TOKEN_B
R1=$(call POST "/api/channels/$CHAN/messages" '{"content":"me! <@'"$ALICE"'>","thread_id":"'"$THREAD"'"}')
jq -e --arg t "$THREAD" '.thread_id == $t' <<<"$R1" >/dev/null; R1=$(jq -r .id <<<"$R1")
fail_call POST "/api/channels/$CHAN/threads" '{"name":"nested","message_id":"'"$R1"'"}'
fail_call POST "/api/channels/$PINS/messages" '{"content":"wrong channel","thread_id":"'"$THREAD"'"}'
TOKEN=$TOKEN_A
R2=$(call POST "/api/channels/$CHAN/messages" '{"content":"thanks","thread_id":"'"$THREAD"'","reply_to_id":"'"$R1"'"}' | jq -r .id)
call PUT "/api/channels/$CHAN/messages/$R1/reactions/%F0%9F%91%8D" >/dev/null
call PATCH "/api/channels/$CHAN/messages/$R2" '{"content":"thanks mate"}' >/dev/null
call GET "/api/channels/$CHAN/messages?thread=$THREAD" | jq -e --arg t "$THREAD" 'length == 2 and all(.[]; .thread_id == $t) and .[1].content == "thanks mate" and (.[0].reactions | length == 1)' >/dev/null
call GET "/api/channels/$CHAN/messages?limit=100" | jq -e '[.[] | select(.thread_id != null)] | length == 0' >/dev/null
call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg id "$TM" '.[] | select(.id == $id) | .thread.message_count == 2' >/dev/null
call GET "/api/channels/$CHAN/threads" | jq -e --arg id "$THREAD" '.[0].id == $id and .[0].message_count == 2' >/dev/null
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -F "content=boss strat" -F "thread_id=$THREAD" -F "files=@/tmp/pixel.png;type=image/png" "$BASE/api/channels/$CHAN/messages/upload")
[[ "$code" == 2* ]] || { echo "thread upload -> $code"; cat /tmp/body; exit 1; }
jq -e --arg t "$THREAD" '.thread_id == $t and (.attachments | length == 1)' /tmp/body >/dev/null
TOKEN=$TOKEN_B
call DELETE "/api/channels/$CHAN/messages/$R1" >/dev/null
fail_call PATCH "/api/threads/$THREAD" '{"name":"bob was here"}'
fail_call DELETE "/api/threads/$THREAD"
TOKEN=$TOKEN_A
call GET "/api/threads/$THREAD" | jq -e '.message_count == 2' >/dev/null
call PATCH "/api/threads/$THREAD" '{"name":"Rag tanks"}' | jq -e '.name == "Rag tanks"' >/dev/null
CT=$(call POST "/api/channels/$CHAN/threads" '{"name":"Loot talk"}' | jq -r .id)
call GET "/api/threads/$CT" | jq -e '.starter_message_id == null' >/dev/null
fail_call POST "/api/channels/$CHAN/threads" '{"name":"   "}'
DT=$(call POST "/api/channels/$DMID/threads" '{"name":"DM side chat"}' | jq -r .id)
call POST "/api/channels/$DMID/messages" '{"content":"in a DM thread","thread_id":"'"$DT"'"}' >/dev/null
TOKEN=$OUT
fail_call GET "/api/channels/$CHAN/threads"
fail_call GET "/api/threads/$THREAD"
fail_call GET "/api/channels/$CHAN/messages?thread=$THREAD"
TOKEN=$TOKEN_A
call DELETE "/api/threads/$CT" >/dev/null
fail_call GET "/api/threads/$CT"

step "polls"
PM=$(call POST "/api/channels/$CHAN/polls" '{"question":"Which raid this week?","options":["MC","BWL","AQ40"],"duration_minutes":60}')
POLL=$(jq -r .id <<<"$PM")
jq -e '.poll.question == "Which raid this week?" and (.poll.options | length == 3) and .poll.closed == false and .poll.ends_at != null and .poll.multi == false' <<<"$PM" >/dev/null
call PUT "/api/channels/$CHAN/polls/$POLL/votes" '{"options":[1]}' | jq -e --arg a "$ALICE" '.poll.options[1].user_ids == [$a]' >/dev/null
fail_call PUT "/api/channels/$CHAN/polls/$POLL/votes" '{"options":[0,1]}' # one choice only
fail_call PUT "/api/channels/$CHAN/polls/$POLL/votes" '{"options":[7]}'
fail_call PUT "/api/channels/$PINS/polls/$POLL/votes" '{"options":[0]}' # wrong channel
TOKEN=$TOKEN_B
call PUT "/api/channels/$CHAN/polls/$POLL/votes" '{"options":[1]}' | jq -e '.poll.options[1].user_ids | length == 2' >/dev/null
call PUT "/api/channels/$CHAN/polls/$POLL/votes" '{"options":[2]}' | jq -e '(.poll.options[1].user_ids | length == 1) and (.poll.options[2].user_ids | length == 1)' >/dev/null
fail_call POST "/api/channels/$CHAN/polls/$POLL/close"
TOKEN=$TOKEN_A
call PUT "/api/channels/$CHAN/polls/$POLL/votes" '{"options":[]}' | jq -e '.poll.options[1].user_ids | length == 0' >/dev/null
call POST "/api/channels/$CHAN/polls/$POLL/close" | jq -e '.poll.closed == true' >/dev/null
fail_call PUT "/api/channels/$CHAN/polls/$POLL/votes" '{"options":[0]}'
fail_call POST "/api/channels/$CHAN/polls" '{"question":"Lonely?","options":["just one"]}'
fail_call POST "/api/channels/$CHAN/polls" '{"question":"Too many?","options":["1","2","3","4","5","6","7","8","9","10","11"]}'
fail_call POST "/api/channels/$CHAN/polls" '{"question":"Same?","options":["Yes","yes"]}'
fail_call POST "/api/channels/$CHAN/polls" '{"question":"Forever?","options":["a","b"],"duration_minutes":20000}'
fail_call POST "/api/channels/$CHAN/polls" '{"question":"  ","options":["a","b"]}'
MP=$(call POST "/api/channels/$CHAN/polls" '{"question":"Snacks?","options":["Chips","Lollies","Fruit"],"multi":true,"duration_minutes":1}' | jq -r .id)
call PUT "/api/channels/$CHAN/polls/$MP/votes" '{"options":[0,2,2]}' | jq -e '(.poll.options[0].user_ids | length == 1) and (.poll.options[2].user_ids | length == 1)' >/dev/null
call POST "/api/channels/$DMID/polls" '{"question":"Pub or BBQ?","options":["Pub","BBQ"]}' | jq -e '.poll.ends_at == null' >/dev/null
TOKEN=$OUT; fail_call PUT "/api/channels/$CHAN/polls/$MP/votes" '{"options":[0]}'; TOKEN=$TOKEN_A

step "custom emoji"
upload_emoji() { # TOKEN NAME FILE -> http code, body in /tmp/body
  curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $1" -F "name=$2" -F "file=@$3" "$BASE/api/guilds/$GUILD/emojis"
}
code=$(upload_emoji "$TOKEN_A" pog /tmp/pixel.png); [[ "$code" == 2* ]] || { echo "emoji upload -> $code"; cat /tmp/body; exit 1; }
EMOJI=$(jq -r .id /tmp/body)
jq -e '.name == "pog" and .url != null and .animated == false' /tmp/body >/dev/null
curl -fsS "$BASE/emojis/$EMOJI" -o /tmp/emoji.png && cmp -s /tmp/emoji.png /tmp/pixel.png
call GET "/api/guilds/$GUILD/emojis" | jq -e 'length == 1 and .[0].name == "pog"' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/emojis" | jq -e 'length == 1' >/dev/null; TOKEN=$TOKEN_A
code=$(upload_emoji "$TOKEN_B" bobs /tmp/pixel.png); [[ "$code" == 403 ]] || { echo "Bob uploaded an emoji -> $code"; exit 1; }
printf 'definitely not a picture' > /tmp/fake.png
code=$(upload_emoji "$TOKEN_A" fake /tmp/fake.png); [[ "$code" == 400 ]] || { echo "fake emoji -> $code"; exit 1; }
{ head -c 8 /tmp/pixel.png; head -c 300000 /dev/zero; } > /tmp/big.png
code=$(upload_emoji "$TOKEN_A" huge /tmp/big.png); [[ "$code" == 4* ]] || { echo "huge emoji -> $code"; exit 1; }
code=$(upload_emoji "$TOKEN_A" "Bad-Name" /tmp/pixel.png); [[ "$code" == 400 ]] || { echo "bad emoji name -> $code"; exit 1; }
code=$(upload_emoji "$TOKEN_A" pog /tmp/pixel.png); [[ "$code" == 409 ]] || { echo "duplicate emoji -> $code"; exit 1; }
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/emojis"; TOKEN=$TOKEN_A
TOK="<:pog:$EMOJI>"
call POST "/api/channels/$CHAN/messages" '{"content":"gg :pog: :nope: `:pog:`"}' | jq -e --arg e "$TOK" '.content == ("gg " + $e + " :nope: `:pog:`")' >/dev/null
call POST "/api/channels/$DMID/messages" '{"content":":pog:"}' | jq -e --arg e "$TOK" '.content == $e' >/dev/null
EM=$(call POST "/api/channels/$CHAN/messages" '{"content":"plain"}' | jq -r .id)
call PATCH "/api/channels/$CHAN/messages/$EM" '{"content":"edited :pog:"}' | jq -e --arg e "$TOK" '.content == ("edited " + $e)' >/dev/null
call PUT "/api/channels/$CHAN/messages/$EM/reactions/$(enc "$TOK")" >/dev/null
call GET "/api/channels/$CHAN/messages?limit=5" | jq -e --arg id "$EM" --arg e "$TOK" '.[] | select(.id == $id) | .reactions[0].emoji == $e' >/dev/null
fail_call PUT "/api/channels/$CHAN/messages/$EM/reactions/$(enc '<:pog:123>')"
call GET /api/users/@me/emojis | jq -e --arg id "$EMOJI" '[.[] | select(.id == $id)] | length == 1' >/dev/null
call PATCH "/api/emojis/$EMOJI" '{"name":"poggers"}' | jq -e '.name == "poggers"' >/dev/null
TOKEN=$TOKEN_B; fail_call DELETE "/api/emojis/$EMOJI"; fail_call PATCH "/api/emojis/$EMOJI" '{"name":"bobmoji"}'; TOKEN=$TOKEN_A
call DELETE "/api/emojis/$EMOJI" >/dev/null
code=$(curl -sS -o /dev/null -w "%{http_code}" "$BASE/emojis/$EMOJI"); [[ "$code" == 404 ]] || { echo "deleted emoji still served -> $code"; exit 1; }
# Leave one behind for the UI test.
code=$(upload_emoji "$TOKEN_A" partyparrot /tmp/pixel.png); [[ "$code" == 2* ]] || { echo "emoji upload -> $code"; cat /tmp/body; exit 1; }

step "raid reminder"
for i in $(seq 1 75); do
  call GET "/api/channels/$CHAN/messages" | jq -e '[.[] | select(.content | startswith("⏰"))] | length == 1' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/channels/$CHAN/messages" | jq -e '[.[] | select(.content | startswith("⏰"))] | length == 1' >/dev/null || { echo "no raid reminder"; exit 1; }

step "poll closes at its end time"
for i in $(seq 1 90); do
  call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg id "$MP" '.[] | select(.id == $id) | .poll.closed' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg id "$MP" '.[] | select(.id == $id) | .poll.closed' >/dev/null || { echo "poll never closed"; exit 1; }
fail_call PUT "/api/channels/$CHAN/polls/$MP/votes" '{"options":[1]}'

echo "Smoke test passed."
