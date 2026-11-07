#!/usr/bin/env bash
# Starts the real server against Postgres and walks through the main API
# flows, so SQL and routing mistakes fail CI instead of failing in front of
# people. Add a section here for each new feature.
set -euo pipefail
trap 'echo "Smoke test failed at line $LINENO"' ERR

BASE=http://127.0.0.1:8080
LOG=/tmp/outhouse-server.log
# Wowhead and Battle.net stand-ins (no internet needed).
MOCK=http://127.0.0.1:8099
python3 ci/wow_mock.py 8099 >/tmp/wow-mock.log 2>&1 &
MOCK_PID=$!
export WOWHEAD_URL=$MOCK WOWHEAD_TOOLTIP_URL=$MOCK WOWHEAD_ICON_URL=$MOCK/icons LINK_PREVIEWS_OFFLINE=1
export BNET_CLIENT_ID=mock-id BNET_CLIENT_SECRET=mock-secret BNET_REGION=us
export BNET_OAUTH_URL=$MOCK/oauth BNET_API_URL=$MOCK/api PUBLIC_URL=$BASE
# LiveKit RoomService stand-in (checks the JWTs Outhouse signs).
LK=http://127.0.0.1:8098
python3 ci/livekit_mock.py 8098 "${LIVEKIT_API_SECRET:-secret}" >/tmp/livekit-mock.log 2>&1 &
LK_PID=$!
export LIVEKIT_API_URL=$LK
# Web Push service + alert target stand-in (decrypts pushes with http_ece).
python3 -m pip install --quiet cryptography http_ece >/dev/null 2>&1 \
  || python3 -m pip install --quiet --break-system-packages cryptography http_ece >/dev/null 2>&1 \
  || echo "(couldn't install http_ece; the push mock uses its own decoder)"
PUSH=http://127.0.0.1:8097
python3 ci/push_mock.py 8097 >/tmp/push-mock.log 2>&1 &
PUSH_PID=$!
export PUSH_EXTRA_ENDPOINTS=$PUSH/push/
# Where backup.sh would leave its status (read by the server status page).
STATUS_DIR=$(mktemp -d)
export BACKUP_STATUS_FILE=$STATUS_DIR/backup.json OUTHOUSE_VERSION=smoke-test
./target/debug/outhouse-server >"$LOG" 2>&1 &
SERVER=$!
trap 'kill $SERVER $MOCK_PID $LK_PID $PUSH_PID 2>/dev/null || true' EXIT

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

step "push notifications: subscribe"
for i in $(seq 1 20); do curl -fsS "$PUSH/ok" >/dev/null 2>&1 && break; sleep 0.5; done
TOKEN=$TOKEN_B
call GET /api/users/@me/notifications | jq -e '(.vapid_public_key | length) == 87 and .devices == [] and .levels == []' >/dev/null
SUB_B=$(curl -fsS "$PUSH/new/bob")
call POST /api/push/subscriptions "$SUB_B" | jq -e --arg e "$PUSH/push/bob" '.endpoint == $e' >/dev/null
call POST /api/push/subscriptions "$SUB_B" >/dev/null   # same browser again: still one device
call GET /api/users/@me/notifications | jq -e '.devices | length == 1' >/dev/null
# Only real push services (no SSRF), and the keys must be real P-256 keys.
fail_call POST /api/push/subscriptions "$(jq -c '.endpoint = "https://evil.example.com/push"' <<<"$SUB_B")"
fail_call POST /api/push/subscriptions "$(jq -c '.endpoint = "http://169.254.169.254/latest"' <<<"$SUB_B")"
fail_call POST /api/push/subscriptions "$(jq -c '.keys.p256dh = "AAAA"' <<<"$SUB_B")"
call POST /api/push/test | jq -e '.sent == 1 and .failed == 0' >/dev/null
TOKEN=""; fail_call POST /api/push/test; TOKEN=$TOKEN_A
SUB_A=$(curl -fsS "$PUSH/new/alice")
call POST /api/push/subscriptions "$SUB_A" >/dev/null

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
TOKEN=$TOKEN_C; call POST "/api/invites/$CODE" >/dev/null
call POST /api/push/subscriptions "$(curl -fsS "$PUSH/new/carl")" >/dev/null; TOKEN=$TOKEN_A
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

step "repeating raids"
RX=$(call POST "/api/guilds/$GUILD/channels" '{"name":"raid-extras"}' | jq -r .id)
FIRST=$(date -u -d '+1 day' +%Y-%m-%dT09:00:00Z)
D1=$(( $(date -u -d '+1 day' +%u) - 1 )) # 0 = Monday
D2=$(( (D1 + 3) % 7 ))
fail_call POST "/api/channels/$RX/events" '{"title":"No days","starts_at":"'"$FIRST"'","repeat":{"weekdays":[],"every_weeks":1,"time_zone":"UTC"}}'
fail_call POST "/api/channels/$RX/events" '{"title":"Bad zone","starts_at":"'"$FIRST"'","repeat":{"weekdays":[1],"every_weeks":1,"time_zone":"Mars/Olympus"}}'
fail_call POST "/api/channels/$RX/events" '{"title":"Too far apart","starts_at":"'"$FIRST"'","repeat":{"weekdays":[1],"every_weeks":9,"time_zone":"UTC"}}'
SR=$(call POST "/api/channels/$RX/events" '{"title":"BWL","starts_at":"'"$FIRST"'","max_tanks":1,"repeat":{"weekdays":['"$D1"','"$D2"'],"every_weeks":1,"count":6,"time_zone":"Australia/Sydney"}}')
SERIES=$(jq -r .event.series_id <<<"$SR")
jq -e --arg f "$FIRST" '.event.starts_at == $f and .event.repeat.count == 6 and .event.repeat.time_zone == "Australia/Sydney" and (.event.repeat.weekdays | length == 2) and .event.message_id != null' <<<"$SR" >/dev/null
FROM=$(date -u +%Y-%m-%dT%H:%M:%SZ); TO=$(date -u -d '+60 days' +%Y-%m-%dT%H:%M:%SZ)
series() { call GET "/api/guilds/$GUILD/events?from=$FROM&to=$TO" | jq --arg s "$SERIES" '[.[] | select(.series_id == $s)]'; }
series | jq -e 'length == 6' >/dev/null
# Only raids within a week have their chat card yet.
series | jq -e '[.[] | select(.message_id != null)] | length == 2' >/dev/null
call GET "/api/channels/$RX/messages" | jq -e '[.[] | select(.event != null)] | length == 2' >/dev/null
# Same wall-clock time in Sydney every time (daylight saving can't shift it).
[ "$(series | jq -r '.[].starts_at' | while read -r t; do TZ=Australia/Sydney date -d "$t" +%H:%M; done | sort -u | wc -l)" = 1 ]
fail_call GET "/api/guilds/$GUILD/events?from=$FROM&to=$(date -u -d '+100 days' +%Y-%m-%dT%H:%M:%SZ)"
S0=$(series | jq -r '.[0].id'); S1=$(series | jq -r '.[1].id'); S2=$(series | jq -r '.[2].id'); S3=$(series | jq -r '.[3].id')
# Series planned without a posting rule keep the old one: a week before.
series | jq -e '[.[] | select(.message_id == null) | .post_rule.mode == "days" and .post_rule.days == 7 and .signups_open_at != null] | all' >/dev/null
# Sign-ups open when the card goes up (raid leaders can add people before).
TOKEN=$TOKEN_B
fail_call PUT "/api/events/$S2/signup" '{"status":"yes","role":"healer","class":"Priest","spec":"Holy"}'
TOKEN=$TOKEN_A
call PUT "/api/events/$S2/signup" '{"status":"yes","role":"healer","class":"Priest","spec":"Holy","user_id":"'"$BOB"'"}' | jq -e '.event.signups | length == 1' >/dev/null
TOKEN=$TOKEN_B
fail_call DELETE "/api/events/$S2/signups/$BOB"
call GET "/api/events/$S0" | jq -e '.signups | length == 0' >/dev/null
fail_call PATCH "/api/events/$S2" '{"title":"bob edits","scope":"future"}'
fail_call DELETE "/api/events/$S2?scope=future"
TOKEN=$TOKEN_A
# Edit just one.
call PATCH "/api/events/$S1" '{"title":"BWL (just this one)"}' >/dev/null
# Edit this and future: an hour later from the third raid on.
OLD2=$(series | jq -r '.[2].starts_at')
NEW2=$(date -u -d "@$(( $(date -d "$OLD2" +%s) + 3600 ))" +%Y-%m-%dT%H:%M:%SZ)
call PATCH "/api/events/$S2" '{"title":"BWL (late start)","starts_at":"'"$NEW2"'","scope":"future"}' | jq -e --arg t "$NEW2" '.event.title == "BWL (late start)" and .event.starts_at == $t' >/dev/null
series | jq -e '[.[].title] == ["BWL", "BWL (just this one)", "BWL (late start)", "BWL (late start)", "BWL (late start)", "BWL (late start)"]' >/dev/null
series | jq -e '(.[5].starts_at | fromdate) - (.[4].starts_at | fromdate) < 8 * 86400' >/dev/null
call GET "/api/events/$S2" | jq -e '.signups | length == 1' >/dev/null # sign-ups survive the move
# Cancel just one, then this and future.
call DELETE "/api/events/$S1?scope=one" >/dev/null
series | jq -e 'length == 5' >/dev/null
call DELETE "/api/events/$S3?scope=future" >/dev/null
series | jq -e --arg a "$S0" --arg b "$S2" '[.[].id] == [$a, $b]' >/dev/null
fail_call GET "/api/events/$S3"
# Every second week, in UTC.
E2W=$(call POST "/api/channels/$RX/events" '{"title":"Onyxia","starts_at":"'"$FIRST"'","repeat":{"weekdays":['"$D1"'],"every_weeks":2,"count":2,"time_zone":"UTC"}}' | jq -r .event.series_id)
call GET "/api/guilds/$GUILD/events?from=$FROM&to=$TO" | jq -e --arg s "$E2W" '[.[] | select(.series_id == $s)] | length == 2 and ((.[1].starts_at | fromdate) - (.[0].starts_at | fromdate) == 14 * 86400) and .[1].message_id == null' >/dev/null

step "raid sign-up posting times, games and channel choice"
PT=$(call POST "/api/guilds/$GUILD/channels" '{"name":"raid-signups"}' | jq -r .id)
VOICE=$(call GET "/api/guilds/$GUILD/channels" | jq -r '[.[] | select(.kind == 2)][0].id')
LATER=$(date -u -d '+10 days' +%Y-%m-%dT20:00:00Z)
fail_call POST "/api/channels/$VOICE/events" '{"title":"In voice","starts_at":"'"$LATER"'"}'
fail_call POST "/api/channels/$PT/events" '{"title":"Bad","starts_at":"'"$LATER"'","post_rule":{"mode":"days","days":15}}'
fail_call POST "/api/channels/$PT/events" '{"title":"Bad","starts_at":"'"$LATER"'","post_rule":{"mode":"weekday","weekday":7}}'
fail_call POST "/api/channels/$PT/events" '{"title":"Bad","starts_at":"'"$LATER"'","post_rule":{"mode":"weekday","weekday":0,"time":"25:00"}}'
fail_call POST "/api/channels/$PT/events" '{"title":"Bad","starts_at":"'"$LATER"'","post_rule":{"mode":"days","days":2,"time_zone":"Mars/Olympus"}}'
fail_call POST "/api/channels/$PT/events" '{"title":"Bad","starts_at":"'"$LATER"'","post_rule":{"mode":"sometime"}}'
fail_call POST "/api/channels/$PT/events" '{"title":"Bad","starts_at":"'"$LATER"'","game":"retail"}'
TOKEN=$OUT; fail_call POST "/api/channels/$PT/events" '{"title":"Outsider","starts_at":"'"$LATER"'"}'; TOKEN=$TOKEN_A
# "Right away" in another channel than the one you're in: the card goes there.
NOW1=$(call POST "/api/channels/$PT/events" '{"title":"Right away","starts_at":"'"$LATER"'","game":"forever","post_rule":{"mode":"now"}}')
jq -e --arg pt "$PT" '.channel_id == $pt and .event.message_id != null and .event.signups_open_at == null and .event.game == "forever"' <<<"$NOW1" >/dev/null
call GET "/api/channels/$PT/messages" | jq -e '[.[] | select(.event != null)] | length == 1' >/dev/null
# Sunday 8pm raid in Sydney on 4 Apr 2027, the day daylight saving ends. Posted "Monday 9am before":
# Mon 29 Mar 9am AEDT = 28 Mar 22:00 UTC (not a plain 6 days before). "7 days before": 28 Mar 8pm AEDT.
DST=$(call POST "/api/channels/$PT/events" '{"title":"Sunday raid","starts_at":"2027-04-04T10:00:00Z","game":"tbc","post_rule":{"mode":"weekday","weekday":0,"time":"09:00","time_zone":"Australia/Sydney"}}')
jq -e --arg pt "$PT" '.event | .message_id == null and .signups_open_at == "2027-03-28T22:00:00Z" and .post_rule.mode == "weekday" and .post_rule.weekday == 0 and .post_rule.time == "09:00" and .game == "tbc" and .channel_id == $pt' <<<"$DST" >/dev/null
DSTID=$(jq -r .event.id <<<"$DST")
call PATCH "/api/events/$DSTID" '{"post_rule":{"mode":"days","days":7,"time_zone":"Australia/Sydney"}}' | jq -e '.event.signups_open_at == "2027-03-28T09:00:00Z" and .event.message_id == null and .event.post_rule.days == 7' >/dev/null
call GET "/api/guilds/$GUILD/events?from=2027-03-20T00:00:00Z&to=2027-04-10T00:00:00Z" | jq -e --arg id "$DSTID" '[.[] | select(.id == $id and .signups_open_at == "2027-03-28T09:00:00Z")] | length == 1' >/dev/null
call GET "/api/channels/$PT/messages" | jq -e '[.[] | select(.event != null)] | length == 1' >/dev/null
# Not open yet: members can't sign up or withdraw; the raid leader can add people.
TOKEN=$TOKEN_B
fail_call PUT "/api/events/$DSTID/signup" '{"status":"yes","role":"dps","class":"Mage","spec":"Fire"}'
fail_call PATCH "/api/events/$DSTID" '{"post_rule":{"mode":"now"}}'
TOKEN=$TOKEN_A
call PUT "/api/events/$DSTID/signup" '{"status":"yes","role":"tank","class":"Warrior","spec":"Protection"}' >/dev/null
# Changing the rule to "now" posts it straight away.
call PATCH "/api/events/$DSTID" '{"post_rule":{"mode":"now"}}' | jq -e '.event.message_id != null and .event.signups_open_at == null' >/dev/null
TOKEN=$TOKEN_B
call PUT "/api/events/$DSTID/signup" '{"status":"yes","role":"dps","class":"Mage","spec":"Fire"}' | jq -e '.event.signups | length == 2' >/dev/null
TOKEN=$TOKEN_A
# The scheduler posts a card when its time comes: a raid tomorrow (+90s), posted 1 day before.
TICK_START=$(date -u -d '+1 day 90 seconds' +%Y-%m-%dT%H:%M:%SZ)
TICK=$(call POST "/api/channels/$PT/events" '{"title":"Posted by the clock","starts_at":"'"$TICK_START"'","post_rule":{"mode":"days","days":1,"time_zone":"UTC"}}')
jq -e '.event.message_id == null and .event.signups_open_at != null' <<<"$TICK" >/dev/null
TICKID=$(jq -r .event.id <<<"$TICK")
TOKEN=$TOKEN_B; fail_call PUT "/api/events/$TICKID/signup" '{"status":"yes","role":"dps","class":"Mage","spec":"Fire"}'; TOKEN=$TOKEN_A
# A weekly Sunday raid in Sydney whose sign-ups go up the Monday before at 9am.
SUN=$(python3 -c 'import datetime as d; t=d.date.today()+d.timedelta(days=1); t+=d.timedelta(days=(6-t.weekday())%7); print(t.isoformat())')
SUNSTART=$(TZ=Australia/Sydney date -u -d "$SUN 20:00" +%Y-%m-%dT%H:%M:%SZ)
WK=$(call POST "/api/channels/$PT/events" '{"title":"Weekly Sunday","starts_at":"'"$SUNSTART"'","game":"forever","repeat":{"weekdays":[6],"every_weeks":1,"count":4,"time_zone":"Australia/Sydney"},"post_rule":{"mode":"weekday","weekday":0,"time":"09:00"}}')
WKS=$(jq -r .event.series_id <<<"$WK")
WFROM=$(date -u +%Y-%m-%dT%H:%M:%SZ); WTO=$(date -u -d '+45 days' +%Y-%m-%dT%H:%M:%SZ)
weekly() { call GET "/api/guilds/$GUILD/events?from=$WFROM&to=$WTO" | jq --arg s "$WKS" '[.[] | select(.series_id == $s)]'; }
weekly | jq -e 'length == 4 and all(.[]; .game == "forever" and .post_rule.mode == "weekday")' >/dev/null
# Each unposted one opens on the Monday before, 9am Sydney time, 6 days ahead of the raid.
weekly | jq -r '.[] | select(.message_id == null) | "\(.starts_at) \(.signups_open_at)"' | while read -r st op; do
  [ "$(TZ=Australia/Sydney date -d "$op" +%u-%H:%M)" = "1-09:00" ] || { echo "opens at $op for $st"; exit 1; }
  [ "$(TZ=Australia/Sydney date -d "$st" +%u-%H:%M)" = "7-20:00" ] || { echo "odd start $st"; exit 1; }
done
[ "$(weekly | jq '[.[] | select(.message_id == null)] | length')" -ge 3 ]
# Edit this and future: post 3 days before instead, from the second raid on.
W1=$(weekly | jq -r '.[1].id')
call PATCH "/api/events/$W1" '{"scope":"future","post_rule":{"mode":"days","days":3}}' >/dev/null
weekly | jq -r '.[1:][] | select(.message_id == null) | "\(.starts_at) \(.signups_open_at) \(.post_rule.days)"' | while read -r st op d; do
  [ "$d" = 3 ] || { echo "rule not changed: $d"; exit 1; }
  [ "$(TZ=Australia/Sydney date -d "$op" +%u-%H:%M)" = "4-20:00" ] || { echo "3 days before $st should be Thursday 8pm, got $op"; exit 1; }
done
weekly | jq -e '.[0].post_rule.mode == "weekday"' >/dev/null
call GET "/api/guilds/$GUILD/events?from=$WFROM&to=$WTO" | jq -e '[.[] | select(.title == "Posted by the clock")] | length == 1' >/dev/null

step "sign-up deadline"
SOON2=$(date -u -d '+2 hours' +%Y-%m-%dT%H:%M:%SZ)
DL=$(call POST "/api/channels/$RX/events" '{"title":"Locked raid","starts_at":"'"$SOON2"'","signup_deadline_minutes":180}')
DLE=$(jq -r .event.id <<<"$DL")
jq -e '.event.signup_deadline_minutes == 180 and .event.signups_close_at != null' <<<"$DL" >/dev/null
fail_call POST "/api/channels/$RX/events" '{"title":"Too early","starts_at":"'"$SOON2"'","signup_deadline_minutes":20000}'
TOKEN=$TOKEN_B
fail_call PUT "/api/events/$DLE/signup" '{"status":"yes","role":"dps","class":"Rogue","spec":"Combat"}'
TOKEN=$TOKEN_A
call PUT "/api/events/$DLE/signup" '{"status":"yes","role":"tank","class":"Warrior","spec":"Protection"}' >/dev/null # the leader still can
call PUT "/api/events/$DLE/signup" '{"status":"yes","role":"dps","class":"Rogue","spec":"Combat","user_id":"'"$BOB"'"}' >/dev/null
TOKEN=$TOKEN_B; fail_call DELETE "/api/events/$DLE/signups/$BOB"; TOKEN=$TOKEN_A
call PATCH "/api/events/$DLE" '{"signup_deadline_minutes":0}' | jq -e '.event.signups_close_at == null' >/dev/null
TOKEN=$TOKEN_B; call DELETE "/api/events/$DLE/signups/$BOB" >/dev/null; TOKEN=$TOKEN_A

step "auto-bench and promotion"
BE=$(call POST "/api/channels/$RX/events" '{"title":"Bench test","starts_at":"'"$START"'","max_tanks":1,"max_healers":1,"max_dps":1}' | jq -r .event.id)
call PUT "/api/events/$BE/signup" '{"status":"yes","role":"tank","class":"Warrior","spec":"Protection"}' >/dev/null
TOKEN=$TOKEN_B
call PUT "/api/events/$BE/signup" '{"status":"yes","role":"tank","class":"Druid","spec":"Feral (Bear)"}' | jq -e --arg b "$BOB" '.event.signups[] | select(.user_id == $b) | .status == "bench" and .auto_bench' >/dev/null
call PUT "/api/events/$BE/signup" '{"status":"yes","role":"tank","class":"Druid","spec":"Feral (Bear)","note":"keen"}' | jq -e --arg b "$BOB" '.event.signups[] | select(.user_id == $b) | .status == "bench" and .note == "keen"' >/dev/null
TOKEN=$TOKEN_C
call PUT "/api/events/$BE/signup" '{"status":"late","role":"tank","class":"Paladin","spec":"Protection"}' | jq -e --arg c "$CARL" '.event.signups[] | select(.user_id == $c) | .status == "bench" and .auto_bench' >/dev/null
call PUT "/api/events/$BE/signup" '{"status":"yes","role":"healer","class":"Paladin","spec":"Holy"}' | jq -e --arg c "$CARL" '.event.signups[] | select(.user_id == $c) | .status == "yes" and (.auto_bench | not)' >/dev/null
call PUT "/api/events/$BE/signup" '{"status":"late","role":"tank","class":"Paladin","spec":"Protection"}' >/dev/null # back in the tank queue, behind Bob
TOKEN=$TOKEN_A
call GET "/api/events/$BE" | jq -e --arg b "$BOB" --arg c "$CARL" '[.signups[] | select(.status == "bench") | .user_id] == [$b, $c]' >/dev/null
# Alice drops out: Bob (first on the bench) moves up and hears about it.
call DELETE "/api/events/$BE/signups/$ALICE" | jq -e --arg b "$BOB" --arg c "$CARL" '(.event.signups[] | select(.user_id == $b) | .status == "yes" and (.auto_bench | not)) and (.event.signups[] | select(.user_id == $c) | .status == "bench")' >/dev/null
TOKEN=$TOKEN_B
HDM=$(call GET /api/dms | jq -r '[.[] | select(any(.recipients[]; .bot))][0].id')
[ "$HDM" != "null" ]
call GET "/api/channels/$HDM/messages" | jq -e '.[-1].content | contains("off the bench") and contains("Bench test")' >/dev/null
fail_call PUT "/api/events/$BE/signup" '{"status":"bench","role":"tank","user_id":"'"$CARL"'"}' # not the leader
TOKEN=$TOKEN_A
# The leader moves people by hand: Carl onto the roster, Bob to the bench.
call PUT "/api/events/$BE/signup" '{"status":"yes","role":"tank","class":"Paladin","spec":"Protection","user_id":"'"$CARL"'"}' >/dev/null
call PUT "/api/events/$BE/signup" '{"status":"bench","role":"tank","class":"Druid","spec":"Feral (Bear)","user_id":"'"$BOB"'"}' | jq -e --arg b "$BOB" '.event.signups[] | select(.user_id == $b) | .status == "bench" and (.auto_bench | not)' >/dev/null
# Someone the leader benched isn't moved up automatically.
TOKEN=$TOKEN_C; call DELETE "/api/events/$BE/signups/$CARL" >/dev/null; TOKEN=$TOKEN_A
call GET "/api/events/$BE" | jq -e --arg b "$BOB" '.signups[] | select(.user_id == $b) | .status == "bench"' >/dev/null
# More slots let the queue in.
call PUT "/api/events/$BE/signup" '{"status":"yes","role":"healer","class":"Priest","spec":"Holy"}' >/dev/null
TOKEN=$TOKEN_C; call PUT "/api/events/$BE/signup" '{"status":"yes","role":"healer","class":"Paladin","spec":"Holy"}' | jq -e --arg c "$CARL" '.event.signups[] | select(.user_id == $c) | .status == "bench"' >/dev/null; TOKEN=$TOKEN_A
call PATCH "/api/events/$BE" '{"max_healers":2}' | jq -e --arg c "$CARL" '.event.signups[] | select(.user_id == $c) | .status == "yes"' >/dev/null

step "attendance"
PAST=$(date -u -d '-40 minutes' +%Y-%m-%dT%H:%M:%SZ)
AT=$(call POST "/api/channels/$RX/events" '{"title":"Last night","starts_at":"'"$PAST"'"}' | jq -r .event.id)
call PUT "/api/events/$AT/signup" '{"status":"yes","role":"tank","class":"Warrior","spec":"Protection"}' >/dev/null
call PUT "/api/events/$AT/signup" '{"status":"bench","role":"dps","class":"Druid","spec":"Feral (Cat)","user_id":"'"$BOB"'"}' >/dev/null
call PUT "/api/events/$AT/signup" '{"status":"yes","role":"dps","class":"Paladin","spec":"Retribution","user_id":"'"$CARL"'"}' >/dev/null
fail_call PUT "/api/events/$BE/attendance" '{"all_signed_up":true}' # hasn't started yet
fail_call PUT "/api/events/$AT/attendance" '{"records":[{"user_id":"'"$CARL"'","status":"asleep"}]}'
fail_call PUT "/api/events/$AT/attendance" '{"records":[{"user_id":"'"$OUTSIDER"'","status":"present"}]}'
TOKEN=$TOKEN_B; fail_call PUT "/api/events/$AT/attendance" '{"all_signed_up":true}'; TOKEN=$TOKEN_A
call PUT "/api/events/$AT/attendance" '{"all_signed_up":true,"records":[{"user_id":"'"$CARL"'","status":"late"}]}' \
  | jq -e --arg a "$ALICE" --arg b "$BOB" --arg c "$CARL" 'length == 3 and (map({(.user_id): .status}) | add) == {($a): "present", ($b): "benched", ($c): "late"}' >/dev/null
call GET "/api/events/$AT" | jq -e '.attendance_taken' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/events/$AT/attendance" | jq -e 'length == 3' >/dev/null; TOKEN=$TOKEN_A
AT2=$(call POST "/api/channels/$RX/events" '{"title":"The night before","starts_at":"'"$PAST"'"}' | jq -r .event.id)
call PUT "/api/events/$AT2/attendance" '{"records":[{"user_id":"'"$BOB"'","status":"absent"},{"user_id":"'"$CARL"'","status":"present"}]}' >/dev/null
call GET "/api/guilds/$GUILD/attendance?limit=10" | jq -e --arg b "$BOB" --arg c "$CARL" --arg at2 "$AT2" '
  (.raids | length == 2) and .raids[0].id == $at2
  and (.members[] | select(.user_id == $b) | .total == 2 and .percent == 50 and .history == ["absent", "benched"])
  and (.members[] | select(.user_id == $c) | .late == 1 and .present == 1 and .percent == 100)' >/dev/null
call GET "/api/guilds/$GUILD/attendance?limit=1" | jq -e '.raids | length == 1' >/dev/null
TOKEN=$TOKEN_B
call GET "/api/guilds/$GUILD/attendance/$BOB" | jq -e '.total == 2 and .percent == 50 and .recent[0].status == "absent" and .recent[0].title == "The night before"' >/dev/null
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/attendance"; fail_call GET "/api/events/$AT/attendance"; TOKEN=$TOKEN_A
call PUT "/api/events/$AT2/attendance" '{"records":[{"user_id":"'"$BOB"'","status":""},{"user_id":"'"$CARL"'","status":""}]}' | jq -e 'length == 0' >/dev/null
call GET "/api/events/$AT2" | jq -e '.attendance_taken | not' >/dev/null
call GET "/api/guilds/$GUILD/attendance/$BOB" | jq -e '.total == 1 and .percent == 100' >/dev/null

step "wow: info and characters"
call GET /api/wow/info | jq -e '(.classes | length == 9) and (.classes | index("Monk") | not) and (.activities | map(.name) | index("Molten Core")) and .bnet_enabled and (.wowhead_url | endswith("/classic"))
  and (.games | map(.id) == ["forever","tbc","era"])
  and (.games[0] | .two_names and .max_level == 60 and .rulesets == ["Normal","PvP","Roleplay","Hardcore"] and (.professions | index("Jewelcrafting") | not))
  and (.games[1] | (.two_names | not) and .max_level == 70 and (.professions | index("Jewelcrafting")) and (.races | index("Blood Elf")) and (.races | index("Draenei")))
  and (.games[2] | .max_level == 60 and (.races | index("Draenei") | not))' >/dev/null
# WoW: Forever (the default): first and last name, a ruleset, no realm.
CH1=$(call POST /api/users/@me/characters '{"name":"tankadin","last_name":"lightBRINGER","realm":"Forever","ruleset":"pvp","race":"dwarf","class":"paladin","level":60,"professions":["Mining","blacksmithing"],"armory_url":"https://example.com/armory/tankadin"}')
CH1ID=$(jq -r .id <<<"$CH1")
jq -e '.game == "forever" and .name == "Tankadin" and .last_name == "Lightbringer" and .full_name == "Tankadin Lightbringer" and .realm == "" and .ruleset == "PvP" and .race == "Dwarf"
  and .class == "Paladin" and .is_main and .professions == ["Mining","Blacksmithing"] and .source == "manual"' <<<"$CH1" >/dev/null
CH2ID=$(call POST /api/users/@me/characters '{"name":"Frostbolt","last_name":"Fizzle","class":"Mage","level":42}' | jq -r 'select(.is_main | not) | .ruleset as $r | select($r == "Normal") | .id')
fail_call POST /api/users/@me/characters '{"name":"Arthas","last_name":"Menethil","class":"Death Knight"}'
fail_call POST /api/users/@me/characters '{"name":"Chen","last_name":"Stormstout","class":"Monk"}'
fail_call POST /api/users/@me/characters '{"name":"Two Words","last_name":"Smith","class":"Mage"}'
fail_call POST /api/users/@me/characters '{"name":"Nolastname","class":"Mage"}'
fail_call POST /api/users/@me/characters '{"name":"Thirteenchars","last_name":"Smith","class":"Mage"}'
fail_call POST /api/users/@me/characters '{"name":"Toohigh","last_name":"Smith","class":"Mage","level":70}'
fail_call POST /api/users/@me/characters '{"name":"Gemmy","last_name":"Smith","class":"Mage","professions":["Jewelcrafting"]}'
fail_call POST /api/users/@me/characters '{"name":"Belfy","last_name":"Smith","class":"Mage","race":"Blood Elf"}'
fail_call POST /api/users/@me/characters '{"name":"Rules","last_name":"Smith","class":"Mage","ruleset":"Retail"}'
fail_call POST /api/users/@me/characters '{"name":"Tankadin","last_name":"Lightbringer","class":"Paladin"}'
fail_call POST /api/users/@me/characters '{"name":"Linky","last_name":"Smith","class":"Rogue","armory_url":"javascript:alert(1)"}'
fail_call POST /api/users/@me/characters '{"name":"Retail","class":"Mage","game":"retail"}'
# Same first name, different last name: a different character.
call POST /api/users/@me/characters '{"name":"Tankadin","last_name":"Other","class":"Paladin","level":12}' | jq -e '.full_name == "Tankadin Other"' >/dev/null
# TBC: one name and a realm, level 70, Jewelcrafting, Blood Elves and Draenei.
TBC=$(call POST /api/users/@me/characters '{"game":"TBC","name":"kaelthas","last_name":"ignored","realm":"Spineshatter","race":"blood elf","class":"Mage","level":70,"professions":["jewelcrafting","Tailoring"]}')
jq -e '.game == "tbc" and .name == "Kaelthas" and .last_name == "" and .full_name == "Kaelthas" and .realm == "Spineshatter" and .race == "Blood Elf" and .level == 70 and .professions == ["Jewelcrafting","Tailoring"]' <<<"$TBC" >/dev/null
fail_call POST /api/users/@me/characters '{"game":"tbc","name":"Toohigh","class":"Mage","level":71}'
fail_call POST /api/users/@me/characters '{"game":"tbc","name":"Kaelthas","realm":"spineshatter","class":"Mage"}'
# Same name on another realm, or in Classic Era: different characters.
call POST /api/users/@me/characters '{"game":"tbc","name":"Kaelthas","realm":"Thunderstrike","class":"Mage","level":20}' >/dev/null
ERA=$(call POST /api/users/@me/characters '{"game":"era","name":"Kaelthas","realm":"Spineshatter","class":"Mage","level":60}')
jq -e '.game == "era" and .realm == "Spineshatter"' <<<"$ERA" >/dev/null
fail_call POST /api/users/@me/characters '{"game":"era","name":"Gemmy","class":"Mage","professions":["Jewelcrafting"]}'
fail_call POST /api/users/@me/characters '{"game":"era","name":"Dranei","class":"Mage","race":"Draenei"}'
fail_call POST /api/users/@me/characters '{"game":"era","name":"Toohigh","class":"Mage","level":61}'
# Switching a character's game re-checks it.
fail_call PATCH "/api/characters/$(jq -r .id <<<"$TBC")" '{"game":"forever","name":"Kaelthas","class":"Mage","level":70}'
for c in $(call GET /api/users/@me/characters | jq -r '.[] | select(.game != "forever" or .last_name == "Other") | .id'); do call DELETE "/api/characters/$c" >/dev/null; done
call PATCH "/api/characters/$CH2ID" '{"name":"Frostbolt","last_name":"Fizzle","class":"Mage","level":43,"is_main":true}' | jq -e '.is_main and .level == 43' >/dev/null
call GET /api/users/@me/characters | jq -e 'length == 2 and .[0].name == "Frostbolt" and ([.[] | select(.is_main)] | length == 1)' >/dev/null
TOKEN=$TOKEN_B
fail_call PATCH "/api/characters/$CH1ID" '{"name":"Stolen","class":"Mage"}'
fail_call DELETE "/api/characters/$CH1ID"
call GET "/api/profiles/$ALICE" | jq -e '.characters | length == 2 and .[0].is_main' >/dev/null
call POST /api/users/@me/characters '{"game":"tbc","name":"Holybob","realm":"Forever","class":"Priest","level":59,"professions":["Tailoring","Enchanting"]}' >/dev/null
TOKEN=$OUT; fail_call GET "/api/profiles/$ALICE"; TOKEN=$TOKEN_A

step "wow: /char"
call POST "/api/channels/$CHAN/wow/char" '{"query":"tank"}' | jq -e '.interaction.command == "char" and (.embeds[0] | .kind == "wow-char" and .title == "Tankadin Lightbringer" and .color == 16026810 and .url == "https://example.com/armory/tankadin" and (.description | contains("Level 60 Dwarf Paladin") and contains("WoW: Forever") and contains("PvP")) and (.fields | map(.name) | index("Also plays")) and (.fields[] | select(.name == "Also plays") | .value | contains("Frostbolt Fizzle")))' >/dev/null
# WoW: Forever characters are found by first, last or full name.
call POST "/api/channels/$CHAN/wow/char" '{"query":"Lightbringer"}' | jq -e '.embeds[0].title == "Tankadin Lightbringer"' >/dev/null
call POST "/api/channels/$CHAN/wow/char" '{"query":"frostbolt fizzle"}' | jq -e '.embeds[0].title == "Frostbolt Fizzle"' >/dev/null
call POST "/api/channels/$CHAN/wow/char" '{"query":"fizz"}' | jq -e '.embeds[0].title == "Frostbolt Fizzle"' >/dev/null
# TBC / Era: Name-Realm.
call POST "/api/channels/$CHAN/wow/char" '{"query":"holybob-forever"}' | jq -e '.embeds[0].title == "Holybob" and .embeds[0].author_name == "Bob" and (.embeds[0].description | contains("TBC · Forever"))' >/dev/null
fail_call POST "/api/channels/$CHAN/wow/char" '{"query":"Nobodyhere"}'
fail_call POST "/api/channels/$CHAN/wow/char" '{"query":""}'
TOKEN=$OUT; fail_call POST "/api/channels/$CHAN/wow/char" '{"query":"tank"}'; TOKEN=$TOKEN_A

step "wow: /item (mock Wowhead, cache, fallbacks)"
call POST "/api/channels/$CHAN/wow/item" '{"query":"thunderfury"}' | jq -e '.interaction.command == "item" and (.embeds[0] | .kind == "wow-item" and .title == "Thunderfury, Blessed Blade of the Windseeker" and .color == 16744448 and (.description | contains("Item Level 80")) and (.url | endswith("/classic/item=19019")) and (.thumbnail_url | endswith("/inv_sword_39.jpg")) and (.fields[0].value == "Legendary"))' >/dev/null
call POST "/api/channels/$CHAN/wow/item" '{"query":"19019"}' | jq -e '.embeds[0].title == "Thunderfury, Blessed Blade of the Windseeker"' >/dev/null
call POST "/api/channels/$CHAN/wow/item" '{"query":"THUNDERFURY"}' >/dev/null
curl -fsS "$MOCK/__hits" | jq -e '."/tooltip/item/19019" == 1 and ."/classic/search/suggestions-template" == 1' >/dev/null || { echo "item lookups weren't cached"; curl -sS "$MOCK/__hits"; exit 1; }
call POST "/api/channels/$CHAN/wow/item" '{"query":"Leggings of Transcendence"}' | jq -e '.embeds[0].color == 10696174' >/dev/null
call POST "/api/channels/$CHAN/wow/item" '{"query":"no such thing"}' | jq -e '.embeds[0] | .footer == "Lookup unavailable" and (.url | endswith("/classic/search?q=no%20such%20thing")) and (.description | contains("matched"))' >/dev/null
call POST "/api/channels/$CHAN/wow/item" '{"query":"broken"}' | jq -e '.embeds[0] | .footer == "Lookup unavailable" and (.description | contains("reach"))' >/dev/null
call POST "/api/channels/$CHAN/wow/item" '{"query":"777"}' | jq -e '.embeds[0] | .footer == "Lookup unavailable" and (.url | contains("search?q=777"))' >/dev/null
fail_call POST "/api/channels/$CHAN/wow/item" '{"query":"   "}'
TOKEN=$OUT; fail_call POST "/api/channels/$CHAN/wow/item" '{"query":"thunderfury"}'; TOKEN=$TOKEN_A

step "wow: link cards"
wait_embeds() { # MESSAGE_ID COUNT
  for i in $(seq 1 20); do
    call GET "/api/channels/$CHAN/messages?limit=20" | jq -e --arg id "$1" --argjson n "$2" '.[] | select(.id == $id) | .embeds | length == $n' >/dev/null 2>&1 && return 0
    sleep 1
  done
  echo "no link cards on $1"; call GET "/api/channels/$CHAN/messages?limit=20" | jq --arg id "$1" '.[] | select(.id == $id)'; exit 1
}
LK1=$(call POST "/api/channels/$CHAN/messages" '{"content":"bis https://www.wowhead.com/classic/item=19019/thunderfury-blessed-blade-of-the-windseeker and logs https://classic.warcraftlogs.com/reports/aBcD1234#fight=3"}' | jq -r .id)
wait_embeds "$LK1" 2
call GET "/api/channels/$CHAN/messages?limit=20" | jq -e --arg id "$LK1" '.[] | select(.id == $id) | .embeds
  | (.[0] | .kind == "wow-item" and .color == 16744448 and .site_name == "Wowhead Classic" and (.url | startswith("https://www.wowhead.com/classic/item=19019")))
  and (.[1] | .kind == "wow-link" and .site_name == "Warcraft Logs Classic" and .title == "Combat log aBcD1234")' >/dev/null
LK2=$(call POST "/api/channels/$CHAN/messages" '{"content":"https://www.wowhead.com/classic/item=999999/mystery-blade https://raider.io/characters/us/forever/Tankadin https://example.com/nothing-here"}' | jq -r .id)
wait_embeds "$LK2" 2
call GET "/api/channels/$CHAN/messages?limit=20" | jq -e --arg id "$LK2" '.[] | select(.id == $id) | .embeds
  | (.[0] | .kind == "wow-link" and .title == "Mystery Blade" and .site_name == "Wowhead Classic" and (.description | contains("Preview not available")))
  and (.[1] | .kind == "wow-link" and .site_name == "Raider.IO" and .title == "Tankadin – Forever (US)")' >/dev/null

step "wow: Battle.net linking (mock)"
call GET /api/bnet/status | jq -e '.enabled and (.linked | not)' >/dev/null
AUTH_URL=$(call POST /api/bnet/authorize | jq -r .url)
[[ "$AUTH_URL" == "$MOCK/oauth/authorize?client_id=mock-id&redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fapi%2Fbnet%2Fcallback"* ]] || { echo "odd authorize url: $AUTH_URL"; exit 1; }
STATE=$(sed -E 's/.*[?&]state=([^&]+).*/\1/' <<<"$AUTH_URL")
curl -fsS "$BASE/api/bnet/callback?code=bad-code&state=$STATE" | grep -q "Couldn't" || { echo "bad code should fail"; exit 1; }
STATE=$(call POST /api/bnet/authorize | jq -r .url | sed -E 's/.*[?&]state=([^&]+).*/\1/')
curl -fsS "$BASE/api/bnet/callback?code=good-code&state=$STATE" | grep -q "Imported 2 characters" || { echo "Battle.net import failed"; curl -sS "$BASE/api/bnet/callback?code=good-code&state=$STATE"; exit 1; }
curl -fsS "$BASE/api/bnet/callback?code=good-code&state=$STATE" | grep -q "expired" || { echo "state reused"; exit 1; }
call GET /api/bnet/status | jq -e '.linked and .battletag == "Mockadin#1234"' >/dev/null
call GET /api/users/@me/characters | jq -e 'length == 4 and ([.[] | select(.source == "bnet") | .name] | sort == ["Mockadin","Twinky"]) and ([.[] | select(.name == "Deathy")] | length == 0)' >/dev/null
call DELETE /api/bnet/link >/dev/null
call GET /api/bnet/status | jq -e '.linked | not' >/dev/null
call DELETE "/api/characters/$(call GET /api/users/@me/characters | jq -r '.[] | select(.name == "Twinky") | .id')" >/dev/null
call GET /api/users/@me/characters | jq -e 'length == 3 and ([.[] | select(.is_main)] | length == 1)' >/dev/null
TOKEN=""; fail_call POST /api/bnet/authorize; TOKEN=$TOKEN_A

step "wow: group finder"
LF=$(call POST "/api/channels/$CHAN/lfg" '{"activity":"scholomance","note":"Bring a key","leader":{"role":"tank","class":"Paladin","character":"Tankadin","level":60}}')
LFID=$(jq -r .id <<<"$LF")
jq -e --arg a "$ALICE" '.lfg | .activity == "Scholomance" and .activity_kind == "dungeon" and .min_level == 58 and .max_level == 60
  and .max_tanks == 1 and .max_healers == 1 and .max_dps == 3 and (.full | not) and (.closed | not) and (.expired | not)
  and .leader_id == $a and (.members | length == 1) and .members[0].character == "Tankadin"' <<<"$LF" >/dev/null
TOKEN=$TOKEN_B
fail_call PUT "/api/channels/$CHAN/lfg/$LFID/join" '{"role":"tank","class":"Warrior"}'
fail_call PUT "/api/channels/$CHAN/lfg/$LFID/join" '{"role":"dps","class":"Monk"}'
fail_call PUT "/api/channels/$CHAN/lfg/$LFID/join" '{"role":"wizard"}'
call PUT "/api/channels/$CHAN/lfg/$LFID/join" '{"role":"healer","class":"Priest","character":"Holybob","level":59}' | jq -e '.lfg.members | length == 2' >/dev/null
call PUT "/api/channels/$CHAN/lfg/$LFID/join" '{"role":"dps","class":"Priest","character":"Holybob","level":59}' | jq -e '.lfg.members | length == 2 and (map(select(.role == "dps")) | length == 1)' >/dev/null
fail_call POST "/api/channels/$CHAN/lfg/$LFID/close"
fail_call DELETE "/api/channels/$CHAN/lfg/$LFID/members/$ALICE"
call DELETE "/api/channels/$CHAN/lfg/$LFID/members/$BOB" | jq -e '.lfg.members | length == 1' >/dev/null
TOKEN=$OUT; fail_call PUT "/api/channels/$CHAN/lfg/$LFID/join" '{"role":"dps"}'; fail_call POST "/api/channels/$CHAN/lfg" '{"activity":"x"}'; TOKEN=$TOKEN_A
fail_call POST "/api/channels/$CHAN/lfg" '{"activity":"  "}'
fail_call POST "/api/channels/$CHAN/lfg" '{"activity":"Molten Core","max_dps":45}'
fail_call POST "/api/channels/$CHAN/lfg" '{"activity":"Gold run","min_level":50,"max_level":40}'
call POST "/api/channels/$CHAN/lfg" '{"activity":"molten core"}' | jq -e '.lfg | .activity_kind == "raid" and .max_tanks == 5 and .max_healers == 10 and .max_dps == 25' >/dev/null
# Fills up, a kick opens a spot, then the leader closes it.
SM=$(call POST "/api/channels/$CHAN/lfg" '{"activity":"Gold farming in Searing Gorge","max_tanks":0,"max_healers":1,"max_dps":1,"starts_at":"'"$(date -u -d '+1 hour' +%Y-%m-%dT%H:%M:%SZ)"'"}' | jq -r .id)
TOKEN=$TOKEN_B; call PUT "/api/channels/$CHAN/lfg/$SM/join" '{"role":"healer","class":"Priest"}' >/dev/null
TOKEN=$TOKEN_C; call PUT "/api/channels/$CHAN/lfg/$SM/join" '{"role":"dps","class":"Rogue","character":"Stabby"}' | jq -e '.lfg | .full and .activity_kind == "other"' >/dev/null
TOKEN=$TOKEN_A; fail_call PUT "/api/channels/$CHAN/lfg/$SM/join" '{"role":"dps","class":"Mage"}'
call DELETE "/api/channels/$CHAN/lfg/$SM/members/$CARL" | jq -e '.lfg.full | not' >/dev/null
call POST "/api/channels/$CHAN/lfg/$SM/close" | jq -e '.lfg.closed' >/dev/null
TOKEN=$TOKEN_C; fail_call PUT "/api/channels/$CHAN/lfg/$SM/join" '{"role":"dps","class":"Rogue"}'; TOKEN=$TOKEN_A
# Expires on its own (checked at the end).
LX=$(call POST "/api/channels/$CHAN/lfg" '{"activity":"Quick Deadmines","expires_minutes":1}' | jq -r .id)

step "voice moderation (mock LiveKit API)"
jwt_claims() { python3 -c 'import sys,base64;p=sys.argv[1].split(".")[1];print(base64.urlsafe_b64decode(p+"="*(-len(p)%4)).decode())' "$1"; }
lk_calls() { curl -fsS "$LK/__calls"; }
VOICE2=$(call POST "/api/guilds/$GUILD/channels" '{"name":"Raid comms","kind":2}' | jq -r .id)
curl -fsS -X POST "$LK/__reset" >/dev/null
TOKEN=$TOKEN_B
J=$(call POST "/api/channels/$VOICE/voice/join")
jq -e '.can_speak and .can_stream and (.server_muted | not)' <<<"$J" >/dev/null
jwt_claims "$(jq -r .token <<<"$J")" | jq -e '.video.canPublish and .video.canSubscribe and .video.canPublishSources == ["microphone","screen_share","screen_share_audio"]' >/dev/null
call POST /api/voice/state '{"muted":false,"deafened":false,"streaming":true}' >/dev/null
call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" '.[] | select(.user_id == $b) | .streaming' >/dev/null
fail_call PATCH "/api/guilds/$GUILD/voice/$ALICE" '{"mute":true}'   # Bob can't mute the owner
TOKEN=$TOKEN_C; fail_call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"mute":true}'; fail_call DELETE "/api/guilds/$GUILD/voice/$BOB"; TOKEN=$TOKEN_A
fail_call PATCH "/api/guilds/$GUILD/voice/$ALICE" '{"mute":true}'   # not yourself
call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"mute":true}' >/dev/null
call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" '.[] | select(.user_id == $b) | .server_muted and (.server_deafened | not)' >/dev/null
lk_calls | jq -e --arg room "voice-$VOICE" --arg b "$BOB" '
  (length == 3) and all(.[]; .jwt_ok and .claims.video.roomAdmin and .claims.video.room == $room and .content_type == "application/json")
  and .[0].method == "GetParticipant" and .[0].body == {room: $room, identity: $b}
  and .[1].method == "MutePublishedTrack" and .[1].body == {room: $room, identity: $b, track_sid: "TR_mic", muted: true}
  and .[2].method == "UpdateParticipant" and .[2].body.identity == $b
  and .[2].body.permission == {can_subscribe: true, can_publish: true, can_publish_data: true, can_publish_sources: ["SCREEN_SHARE","SCREEN_SHARE_AUDIO"]}' >/dev/null \
  || { echo "unexpected LiveKit calls:"; lk_calls | jq .; exit 1; }
# The mute sticks when Bob leaves and comes back.
TOKEN=$TOKEN_B; call POST /api/voice/leave >/dev/null
J=$(call POST "/api/channels/$VOICE/voice/join")
jq -e '.server_muted and .can_speak' <<<"$J" >/dev/null
jwt_claims "$(jq -r .token <<<"$J")" | jq -e '.video.canPublishSources == ["screen_share","screen_share_audio"]' >/dev/null
TOKEN=$TOKEN_A
curl -fsS -X POST "$LK/__reset" >/dev/null
call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"mute":false,"deafen":true}' >/dev/null
lk_calls | jq -e '[.[] | select(.method == "UpdateParticipant")][-1].body.permission | (.can_subscribe | not) and .can_publish_sources == ["SCREEN_SHARE","SCREEN_SHARE_AUDIO"]' >/dev/null
call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"deafen":false}' >/dev/null
lk_calls | jq -e '.[-1].method == "UpdateParticipant" and .[-1].body.permission.can_subscribe and .[-1].body.permission.can_publish_sources == ["MICROPHONE","SCREEN_SHARE","SCREEN_SHARE_AUDIO"]' >/dev/null
call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" '.[] | select(.user_id == $b) | (.server_muted | not) and (.server_deafened | not)' >/dev/null
# Move: refused into a text channel, fine into another voice channel; the old room drops him a few seconds later.
fail_call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"channel_id":"'"$CHAN"'"}'
curl -fsS -X POST "$LK/__reset" >/dev/null
call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"channel_id":"'"$VOICE2"'"}' >/dev/null
call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" --arg c "$VOICE2" '.[] | select(.user_id == $b) | .channel_id == $c and (.streaming | not)' >/dev/null
sleep 5
lk_calls | jq -e --arg room "voice-$VOICE" --arg b "$BOB" '.[-1].method == "RemoveParticipant" and .[-1].body == {room: $room, identity: $b} and .[-1].jwt_ok' >/dev/null \
  || { echo "no RemoveParticipant after the move:"; lk_calls | jq .; exit 1; }
# Disconnect.
call DELETE "/api/guilds/$GUILD/voice/$BOB" >/dev/null
call GET "/api/guilds/$GUILD/voice" | jq -e --arg b "$BOB" '[.[] | select(.user_id == $b)] | length == 0' >/dev/null
lk_calls | jq -e --arg room "voice-$VOICE2" '.[-1].method == "RemoveParticipant" and .[-1].body.room == $room' >/dev/null
fail_call DELETE "/api/guilds/$GUILD/voice/$BOB"                      # not in voice any more
fail_call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"channel_id":"'"$VOICE"'"}'
# Muting someone who isn't in voice is saved for when they join.
call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"mute":true}' >/dev/null
TOKEN=$TOKEN_B; call POST "/api/channels/$VOICE2/voice/join" | jq -e '.server_muted' >/dev/null; call POST /api/voice/leave >/dev/null; TOKEN=$TOKEN_A
call PATCH "/api/guilds/$GUILD/voice/$BOB" '{"mute":false}' >/dev/null
call GET /api/permissions | jq -e '[.[] | select(.key == "stream")] | length == 1' >/dev/null

step "entrance sounds and soundboard"
python3 - <<'PY'
import struct
def wav(path, seconds, rate=8000, bits=8):
    n = int(rate * seconds) * bits // 8
    data = bytes([128]) * n if bits == 8 else bytes(n)
    hdr = b"RIFF" + struct.pack("<I", 36 + n) + b"WAVEfmt " + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * bits // 8, bits // 8, bits) + b"data" + struct.pack("<I", n)
    open(path, "wb").write(hdr + data)
wav("/tmp/short.wav", 1)
wav("/tmp/six.wav", 6)
wav("/tmp/eleven.wav", 11)
wav("/tmp/big.wav", 3, rate=44100, bits=16)   # 3 s but 264 KB
open("/tmp/junk.mp3", "wb").write(b"this is not really an mp3 file" * 10)
PY
upload() { # upload PATH FILE [extra -F args...] -> body in /tmp/body, status in $code
  local path=$1 file=$2; shift 2
  code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN" -F "file=@$file" "$@" "$BASE$path")
}
TOKEN=$TOKEN_B
upload /api/users/@me/entrance-sound /tmp/short.wav
[[ "$code" == 200 ]] || { echo "entrance upload -> $code"; cat /tmp/body; exit 1; }
jq -e '.duration_ms == 1000 and (.url | startswith("/files/"))' /tmp/body >/dev/null
ENTRANCE=$(call GET /api/users/@me/entrance-sound | jq -r .url)
curl -fsS -D /tmp/headers "$BASE$ENTRANCE" -o /dev/null; grep -qi "content-type: audio/wav" /tmp/headers
upload /api/users/@me/entrance-sound /tmp/six.wav; [[ "$code" == 400 ]] || { echo "6 s entrance sound -> $code"; exit 1; }
upload /api/users/@me/entrance-sound /tmp/big.wav; [[ "$code" == 400 ]] || { echo "264 KB entrance sound -> $code"; exit 1; }
upload /api/users/@me/entrance-sound /tmp/junk.mp3; [[ "$code" == 400 ]] || { echo "junk entrance sound -> $code"; exit 1; }
upload /api/users/@me/entrance-sound /tmp/short.wav   # replacing removes the old file
[[ "$code" == 200 ]] || { echo "entrance re-upload -> $code"; exit 1; }
curl -sS -o /dev/null -w "%{http_code}" "$BASE$ENTRANCE" | grep -q 404
call DELETE /api/users/@me/entrance-sound >/dev/null
call GET /api/users/@me/entrance-sound | jq -e '. == null' >/dev/null
# Soundboard: only Manage Sounds may add, clips up to 10 s.
upload "/api/guilds/$GUILD/sounds" /tmp/short.wav -F name=Bonk; [[ "$code" == 403 ]] || { echo "bob added a sound -> $code"; exit 1; }
TOKEN=$TOKEN_A
upload "/api/guilds/$GUILD/sounds" /tmp/short.wav -F name=Bonk -F "emoji=🔨"
[[ "$code" == 200 ]] || { echo "sound upload -> $code"; cat /tmp/body; exit 1; }
SND=$(jq -r .id /tmp/body)
upload "/api/guilds/$GUILD/sounds" /tmp/eleven.wav -F name=Long; [[ "$code" == 400 ]] || { echo "11 s clip -> $code"; exit 1; }
grep -q "10 seconds" /tmp/body
call PUT "/api/sounds/$SND/favourite" >/dev/null
call GET "/api/guilds/$GUILD/sounds" | jq -e --arg s "$SND" '.[] | select(.id == $s) | .favourite and .emoji == "🔨"' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/sounds" | jq -e --arg s "$SND" '.[] | select(.id == $s) | .favourite | not' >/dev/null; TOKEN=$TOKEN_A
TOKEN=$OUT; fail_call PUT "/api/sounds/$SND/favourite"; TOKEN=$TOKEN_A
call DELETE "/api/sounds/$SND/favourite" >/dev/null
call GET "/api/guilds/$GUILD/sounds" | jq -e --arg s "$SND" '.[] | select(.id == $s) | .favourite | not' >/dev/null
# Playing needs voice; then a cooldown stops spam.
fail_call POST "/api/sounds/$SND/play"
call POST "/api/channels/$VOICE/voice/join" >/dev/null
call POST "/api/sounds/$SND/play" >/dev/null
fail_call POST "/api/sounds/$SND/play"
grep -q "Easy on the soundboard" /tmp/body
sleep 2.2
call POST "/api/sounds/$SND/play" >/dev/null
call POST /api/voice/leave >/dev/null

step "audit log"
TOKEN=$TOKEN_A
AUD=$(call GET "/api/guilds/$GUILD/audit-log?limit=100")
jq -e '(.entries | length) > 50 and (.actions | length) > 30 and .retention_days == 90' <<<"$AUD" >/dev/null
for action in channel_create channel_permissions role_create member_role_add message_pin message_unpin emoji_create sound_create voice_moderate voice_move voice_disconnect bot_create webhook_create; do
  call GET "/api/guilds/$GUILD/audit-log?action=$action&limit=1" | jq -e '.entries | length == 1' >/dev/null || { echo "no $action in the audit log"; exit 1; }
done
call GET "/api/guilds/$GUILD/audit-log?action=role_create" | jq -e --arg a "$ALICE" 'all(.entries[]; .actor_id == $a and .target_kind == "role" and (.target_name | startswith("@")))' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=channel_permissions&limit=100" | jq -e 'any(.entries[]; .details | contains("Denied: +View channels"))' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=message_pin" | jq -e '(.entries | length) >= 1 and all(.entries[]; .action == "message_pin")' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?user=$BOB" | jq -e --arg b "$BOB" '(.entries | length) >= 1 and all(.entries[]; .actor_id == $b or .target_id == $b)' >/dev/null
P1=$(call GET "/api/guilds/$GUILD/audit-log?limit=3")
jq -e '(.entries | length) == 3 and .has_more' <<<"$P1" >/dev/null
P2=$(call GET "/api/guilds/$GUILD/audit-log?limit=3&before=$(jq -r '.entries[-1].id' <<<"$P1")")
jq -n --argjson a "$P1" --argjson b "$P2" '($a.entries | map(.id)) as $x | ($b.entries | map(.id)) as $y | ($y | length) == 3 and ([$x[], $y[]] | unique | length) == 6' >/dev/null
fail_call GET "/api/guilds/$GUILD/audit-log?before=nope"
# Moderation actions land in the log too.
call PATCH "/api/guilds/$GUILD/roles/$ROLE" '{"name":"Core raider","color":16711680}' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=role_update" | jq -e '.entries[0].details | contains("Name: Raider → Core raider") and contains("Colour")' >/dev/null
call POST "/api/guilds/$GUILD/members/$CARL/timeout" '{"minutes":60}' >/dev/null
call POST "/api/guilds/$GUILD/members/$CARL/timeout" '{"minutes":0}' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?user=$CARL" | jq -e '[.entries[].action] | index("member_timeout") != null and index("member_timeout_remove") != null' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=member_timeout" | jq -e '.entries[0].details == "For 1 hour"' >/dev/null
DEL=$(call POST "/api/channels/$CHAN/messages" '{"content":"rude words"}' | jq -r .id)
TOKEN=$TOKEN_B; DELB=$(call POST "/api/channels/$CHAN/messages" '{"content":"bob says rude words"}' | jq -r .id); TOKEN=$TOKEN_A
call DELETE "/api/channels/$CHAN/messages/$DEL" >/dev/null    # your own: not logged
call DELETE "/api/channels/$CHAN/messages/$DELB" >/dev/null   # someone else's: logged
call GET "/api/guilds/$GUILD/audit-log?action=message_delete" | jq -e --arg b "$BOB" '.entries[0].target_id == $b and (.entries[0].details | contains("bob says rude words")) and all(.entries[]; .details | contains("“rude words”") | not)' >/dev/null
KR=$(call POST /api/auth/register '{"username":"kickme","password":"password123"}')
TOKEN=$(jq -r .token <<<"$KR"); call POST "/api/invites/$CODE" >/dev/null; TOKEN=$TOKEN_A
KICKED=$(jq -r .user.id <<<"$KR")
call DELETE "/api/guilds/$GUILD/members/$KICKED" >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=member_kick" | jq -e --arg k "$KICKED" '.entries[0].target_id == $k and .entries[0].target_name != ""' >/dev/null
RD=$(call POST "/api/channels/$CHAN/events" '{"title":"Cancelled raid","starts_at":"'"$START"'"}' | jq -r .event.id)
call DELETE "/api/events/$RD" >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=raid_delete" | jq -e '.entries[0].target_name == "Cancelled raid"' >/dev/null
# Needs View Audit Log (or Manage Outhouse); strangers don't even see it exists.
TOKEN=$TOKEN_B; fail_call GET "/api/guilds/$GUILD/audit-log"; TOKEN=$TOKEN_A
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/audit-log"; TOKEN=$TOKEN_A
AUDR=$(call POST "/api/guilds/$GUILD/roles" '{"name":"Auditor","permissions":"8589934592"}' | jq -r .id)
call PUT "/api/guilds/$GUILD/members/$BOB/roles/$AUDR" >/dev/null
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/audit-log" | jq -e '.entries | length > 0' >/dev/null; TOKEN=$TOKEN_A
call DELETE "/api/guilds/$GUILD/members/$BOB/roles/$AUDR" >/dev/null

step "server status and health"
call GET /api/health | jq -e '.status == "ok" and .db == true and .version == "smoke-test" and .uptime_seconds >= 0' >/dev/null
call GET "/api/guilds/$GUILD/server-status" | jq -e '.version == "smoke-test" and .db_ok and .db_size_bytes > 0 and .push_enabled and .backup == null' >/dev/null
printf '{"time":"%s","ok":true,"file":"outhouse-test.tgz","size":"1.2M","offsite":"ok: smb","message":""}\n' "$(date -Iseconds)" > "$BACKUP_STATUS_FILE"
call GET "/api/guilds/$GUILD/server-status" | jq -e '.backup.ok and .backup.file == "outhouse-test.tgz" and .backup.offsite == "ok: smb"' >/dev/null
TOKEN=$TOKEN_B; fail_call GET "/api/guilds/$GUILD/server-status"; TOKEN=$TOKEN_A

step "badges"
BOT_ID=$(call POST "/api/guilds/$GUILD/bots" '{"name":"Badger"}' | jq -r .bot.id)
BL=$(call GET "/api/guilds/$GUILD/badges")
jq -e --arg a "$ALICE" --arg b "$BOB" --arg bot "$BOT_ID" '
  (.[] | select(.key == "owner") | .holders == [$a] and .auto)
  and (.[] | select(.key == "founder") | (.holders | index($a)) != null and (.holders | index($b)) != null)
  and (.[] | select(.key == "raid_leader") | (.holders | index($a)) != null)
  and (.[] | select(.key == "bot") | (.holders | index($bot)) != null)
  and (.[] | select(.key == "early") | (.holders | index($a)) != null)' <<<"$BL" >/dev/null || { echo "unexpected badges:"; jq -c '.[] | {key, holders}' <<<"$BL"; exit 1; }
TOKEN=$TOKEN_B; fail_call POST "/api/guilds/$GUILD/badges" '{"name":"Nope","emoji":"🚫"}'; TOKEN=$TOKEN_A
fail_call POST "/api/guilds/$GUILD/badges" '{"name":"","emoji":"🏆"}'
fail_call POST "/api/guilds/$GUILD/badges" '{"name":"Too long emoji","emoji":"not an emoji at all"}'
BADGE=$(call POST "/api/guilds/$GUILD/badges" '{"name":"MVP","emoji":"🏆","color":16753920,"description":"Carried the raid"}' | jq -r .id)
call PUT "/api/badges/$BADGE/members/$BOB" >/dev/null
call PUT "/api/badges/$BADGE/members/$BOB" >/dev/null   # twice is fine
fail_call PUT "/api/badges/$BADGE/members/$OUTSIDER"    # not a member
TOKEN=$TOKEN_B; fail_call PUT "/api/badges/$BADGE/members/$BOB"; fail_call DELETE "/api/badges/$BADGE"; TOKEN=$TOKEN_A
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/badges" | jq -e --arg id "$BADGE" --arg b "$BOB" '.[] | select(.id == $id) | .holders == [$b] and .emoji == "🏆" and .color == 16753920 and (.auto | not)' >/dev/null; TOKEN=$TOKEN_A
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/badges"; TOKEN=$TOKEN_A
call PATCH "/api/badges/$BADGE" '{"name":"Raid MVP"}' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=badge_update" | jq -e '.entries[0].details == "Name: MVP → Raid MVP"' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=badge_award" | jq -e --arg b "$BOB" '(.entries | length) == 1 and .entries[0].target_id == $b' >/dev/null
call DELETE "/api/badges/$BADGE/members/$BOB" >/dev/null
call GET "/api/guilds/$GUILD/badges" | jq -e --arg id "$BADGE" '.[] | select(.id == $id) | .holders == []' >/dev/null
call DELETE "/api/badges/$BADGE" >/dev/null
call GET "/api/guilds/$GUILD/badges" | jq -e --arg id "$BADGE" '[.[] | select(.id == $id)] | length == 0' >/dev/null

step "notification levels and channel mutes"
TOKEN=$TOKEN_B
fail_call PUT "/api/users/@me/notifications/guilds/$GUILD" '{"level":"loud"}'
fail_call PUT "/api/users/@me/notifications/guilds/$OUTSIDER" '{"level":"all"}'   # not an outhouse of Bob's
fail_call PUT "/api/users/@me/notifications/channels/$SECRET" '{"muted":true}'     # can't see it
call PUT "/api/users/@me/notifications/guilds/$GUILD" '{"level":"all"}' | jq -e '.level == "all"' >/dev/null
call PUT "/api/users/@me/notifications/channels/$PINS" '{"muted":true}' >/dev/null
call GET /api/users/@me/notifications | jq -e --arg g "$GUILD" --arg p "$PINS" '(.levels[] | select(.guild_id == $g) | .level == "all") and .muted_channels == [$p]' >/dev/null
TOKEN=$TOKEN_A
call POST "/api/channels/$CHAN/messages" '{"content":"plain chatter for level all"}' >/dev/null
call POST "/api/channels/$PINS/messages" '{"content":"<@'"$BOB"'> in a muted channel"}' >/dev/null
call POST "/api/channels/$SECRET/messages" '{"content":"officers only, level all"}' >/dev/null
sleep 1.5 # pushes go out in the background; let them read the settings first
TOKEN=$TOKEN_B
call PUT "/api/users/@me/notifications/guilds/$GUILD" '{"level":"nothing"}' >/dev/null
call PUT "/api/users/@me/notifications/channels/$PINS" '{"muted":false}' >/dev/null
TOKEN=$TOKEN_A
call POST "/api/channels/$CHAN/messages" '{"content":"<@'"$BOB"'> you set nothing"}' >/dev/null
TOKEN=$TOKEN_B
call PUT "/api/users/@me/notifications/guilds/0" '{"level":"nothing"}' >/dev/null
TOKEN=$TOKEN_A
call POST "/api/channels/$DMID/messages" '{"content":"dm while dms are off"}' >/dev/null
sleep 1.5
TOKEN=$TOKEN_B
call PUT "/api/users/@me/notifications/guilds/0" '{"level":"all"}' >/dev/null
call PUT "/api/users/@me/notifications/guilds/$GUILD" '{"level":"mentions"}' >/dev/null
TOKEN=$TOKEN_A
call POST "/api/channels/$DMID/messages" '{"content":"last dm for bob"}' >/dev/null
# A push service saying "gone" drops that device.
call POST /api/push/subscriptions '{"endpoint":"'"$PUSH"'/push/gone","keys":'"$(jq -c .keys <<<"$SUB_A")"'}' >/dev/null
call POST /api/push/test | jq -e '.sent == 1 and .removed == 1' >/dev/null
call GET /api/users/@me/notifications | jq -e '.devices | length == 1' >/dev/null

step "raid sign-up card posted by the scheduler"
for i in $(seq 1 120); do
  call GET "/api/events/$TICKID" | jq -e '.message_id != null' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/events/$TICKID" | jq -e '.message_id != null and .signups_open_at == null' >/dev/null || { echo "card never posted"; exit 1; }
call GET "/api/channels/$PT/messages" | jq -e --arg id "$TICKID" '[.[] | select(.event.id == $id)] | length == 1' >/dev/null
TOKEN=$TOKEN_B; call PUT "/api/events/$TICKID/signup" '{"status":"yes","role":"dps","class":"Mage","spec":"Fire"}' >/dev/null; TOKEN=$TOKEN_A

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

step "group post expires"
for i in $(seq 1 60); do
  call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg id "$LX" '.[] | select(.id == $id) | .lfg.expired' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/channels/$CHAN/messages?limit=100" | jq -e --arg id "$LX" '.[] | select(.id == $id) | .lfg.expired' >/dev/null || { echo "group post never expired"; exit 1; }
fail_call PUT "/api/channels/$CHAN/lfg/$LX/join" '{"role":"dps"}'

step "push notifications arrived (VAPID checked, payload decrypted)"
for i in $(seq 1 20); do
  curl -fsS "$PUSH/received/bob" | jq -e 'any(.[]; .payload.body == "last dm for bob")' >/dev/null 2>&1 && break
  sleep 0.5
done
RB=$(curl -fsS "$PUSH/received/bob"); RA=$(curl -fsS "$PUSH/received/alice"); RC=$(curl -fsS "$PUSH/received/carl")
jq -e 'all(.[]; .ok)' <<<"$RB" >/dev/null || { echo "bad pushes for bob:"; jq . <<<"$RB"; exit 1; }
jq -e 'all(.[]; .ok)' <<<"$RA" >/dev/null || { echo "bad pushes for alice:"; jq . <<<"$RA"; exit 1; }
jq -e 'all(.[]; .ok)' <<<"$RC" >/dev/null || { echo "bad pushes for carl:"; jq . <<<"$RC"; exit 1; }
echo "push payloads decrypted by: $(jq -r '[.[].decrypted_by] | unique | join(", ")' <<<"$RB")"
check_push() { # check_push JSON jq-filter description
  jq -e "$2" <<<"$1" >/dev/null || { echo "missing push: $3"; jq -c '[.[].payload | {kind, title, body}]' <<<"$1"; exit 1; }
}
check_push "$RB" 'any(.[]; .payload.kind == "test")' "bob: test notification"
check_push "$RB" 'any(.[]; .payload.kind == "mention" and .payload.body == "hello @Bob" and (.payload.title | contains("#general")))' "bob: mention"
check_push "$RB" 'any(.[]; .payload.kind == "mention" and (.payload.body | contains("@Core raider") or contains("@Raider")))' "bob: role mention"
check_push "$RB" 'any(.[]; .payload.kind == "dm" and .payload.body == "psst bob" and .payload.guild_id == "0")' "bob: DM"
check_push "$RB" 'any(.[]; .payload.kind == "raid" and (.payload.title | contains("off the bench")))' "bob: bench promotion"
check_push "$RB" 'any(.[]; .payload.kind == "message" and .payload.body == "plain chatter for level all")' "bob: level all"
check_push "$RB" 'all(.[]; (.payload.body // "") | contains("muted channel") | not)' "bob: nothing from a muted channel"
check_push "$RB" 'all(.[]; (.payload.body // "") | contains("officers only") | not)' "bob: nothing from a channel he can't see"
check_push "$RB" 'all(.[]; (.payload.body // "") | contains("you set nothing") | not)' "bob: nothing when the level is nothing"
check_push "$RB" 'all(.[]; (.payload.body // "") | contains("dms are off") | not)' "bob: no DMs when DMs are off"
check_push "$RB" 'all(.[]; .payload.url | startswith("/"))' "bob: urls"
check_push "$RC" 'any(.[]; .payload.kind == "dm")' "carl: DM"
check_push "$RA" 'any(.[]; .payload.kind == "raid" and .payload.title == "⏰ Raid reminder" and (.payload.body | contains("Reminder test")))' "alice: raid reminder"

step "uptime alerts (deploy/healthcheck.sh)"
HC_DIR=$(mktemp -d)
hc() { # hc SERVER_URL LIVEKIT_URL
  HEALTH_URL=$1 LIVEKIT_HEALTH_URL=$2 STATUS_DIR=$HC_DIR BACKUP_STATUS_FILE=$BACKUP_STATUS_FILE \
    ALERT_NAME="Smoke Outhouse" ALERT_NTFY_URL=$PUSH/alert/ntfy ALERT_WEBHOOK_URL=$PUSH/alert/hook \
    bash deploy/healthcheck.sh
}
alert_count() { curl -fsS "$PUSH/alerts/$1" | jq length; }
hc "$BASE/api/health" "$PUSH/ok"
[ "$(alert_count ntfy)" = 0 ] || { echo "alerted on a healthy first run"; curl -fsS "$PUSH/alerts/ntfy"; exit 1; }
grep -q '^server=up$' "$HC_DIR/health.state"
hc "http://127.0.0.1:1/api/health" "$PUSH/ok"
[ "$(alert_count ntfy)" = 1 ] && [ "$(alert_count hook)" = 1 ] || { echo "no 'down' alert"; exit 1; }
curl -fsS "$PUSH/alerts/ntfy" | jq -e '.[0].title | contains("Smoke Outhouse") and contains("down")' >/dev/null
curl -fsS "$PUSH/alerts/hook" | jq -e '.[0].content_type == "application/json" and (.[0].body | fromjson | .content | contains("isn'"'"'t answering"))' >/dev/null
hc "http://127.0.0.1:1/api/health" "$PUSH/ok"   # still down: no repeat
[ "$(alert_count ntfy)" = 1 ] || { echo "repeated a 'down' alert"; exit 1; }
hc "$BASE/api/health" "http://127.0.0.1:1"      # server back, LiveKit down: two alerts
[ "$(alert_count ntfy)" = 3 ] || { echo "expected back-up + LiveKit alerts"; curl -fsS "$PUSH/alerts/ntfy"; exit 1; }
curl -fsS "$PUSH/alerts/ntfy" | jq -e 'any(.[]; .title | contains("back up")) and any(.[]; .body | contains("LiveKit"))' >/dev/null
printf '{"time":"%s","ok":false,"file":"","size":"","offsite":"off","message":"pg_dump failed"}\n' "$(date -Iseconds)" > "$BACKUP_STATUS_FILE"
hc "$BASE/api/health" "$PUSH/ok"
curl -fsS "$PUSH/alerts/ntfy" | jq -e 'any(.[]; .body | contains("pg_dump failed"))' >/dev/null || { echo "no backup alert"; curl -fsS "$PUSH/alerts/ntfy"; exit 1; }
printf '{"time":"%s","ok":true,"file":"x.tgz","size":"1M","offsite":"failed: //nas/backups isn'"'"'t mounted","message":""}\n' "$(date -Iseconds)" > "$BACKUP_STATUS_FILE"
hc "$BASE/api/health" "$PUSH/ok"
curl -fsS "$PUSH/alerts/ntfy" | jq -e 'any(.[]; .body | contains("mounted"))' >/dev/null || { echo "no off-site copy alert"; exit 1; }
bash -n deploy/backup.sh deploy/setup.sh deploy/healthcheck.sh deploy/restore.sh

echo "Smoke test passed."
