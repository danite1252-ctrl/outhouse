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

step "raid reminder"
for i in $(seq 1 75); do
  call GET "/api/channels/$CHAN/messages" | jq -e '[.[] | select(.content | startswith("⏰"))] | length == 1' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/channels/$CHAN/messages" | jq -e '[.[] | select(.content | startswith("⏰"))] | length == 1' >/dev/null || { echo "no raid reminder"; exit 1; }

echo "Smoke test passed."
