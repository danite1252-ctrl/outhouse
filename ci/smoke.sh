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
# Signing out also closes that session's open gateway connection (checked
# in the security section near the end; the server re-checks every 30 s).
LOGOUT_TOKEN=$(TOKEN="" call POST /api/auth/login '{"username":"alice","password":"password123"}' | jq -r .token)
node ci/gateway_probe.mjs "$BASE" "$LOGOUT_TOKEN" /tmp/gw-logout.jsonl 150 &
GW_LOGOUT_PID=$!
for i in $(seq 1 40); do grep -q '"READY"' /tmp/gw-logout.jsonl 2>/dev/null && break; sleep 0.25; done
TOKEN=$LOGOUT_TOKEN call POST /api/auth/logout >/dev/null
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

step "thread mentions and read state"
NTM=$(call POST "/api/channels/$CHAN/messages" '{"content":"Thread for pings"}' | jq -r .id)
NT=$(call POST "/api/channels/$CHAN/threads" '{"name":"Thread pings","message_id":"'"$NTM"'"}' | jq -r .id)
TOKEN=$TOKEN_B
call POST "/api/channels/$CHAN/ack" '{"message_id":"'"$NTM"'"}' >/dev/null
TOKEN=$TOKEN_A
CHLAST=$(call GET "/api/guilds/$GUILD/channels" | jq -r --arg c "$CHAN" '.[] | select(.id == $c) | .last_message_id')
call POST "/api/channels/$CHAN/messages" '{"content":"plain reply","thread_id":"'"$NT"'"}' >/dev/null
TOKEN=$TOKEN_B
# A plain reply: the thread is unread, but the channel isn't.
call GET /api/users/@me/unread | jq -e --arg t "$NT" --arg c "$CHAN" --arg g "$GUILD" '
  (.threads[] | select(.thread_id == $t) | .channel_id == $c and .guild_id == $g and .mention_count == 0 and .last_read_id == null)
  and (.channels[] | select(.channel_id == $c) | .mention_count == 0)' >/dev/null
TOKEN=$TOKEN_A
[ "$(call GET "/api/guilds/$GUILD/channels" | jq -r --arg c "$CHAN" '.[] | select(.id == $c) | .last_message_id')" = "$CHLAST" ]
call POST "/api/channels/$CHAN/messages" '{"content":"<@'"$BOB"'> your turn","thread_id":"'"$NT"'"}' >/dev/null
call POST "/api/channels/$CHAN/messages" '{"content":"@here pull timer","thread_id":"'"$NT"'"}' >/dev/null
NTLAST=$(call POST "/api/channels/$CHAN/messages" '{"content":"<@&'"$ROLE"'> to the thread","thread_id":"'"$NT"'"}' | jq -r .id)
TOKEN=$TOKEN_B
call GET /api/users/@me/unread | jq -e --arg t "$NT" --arg l "$NTLAST" --arg c "$CHAN" '
  (.threads[] | select(.thread_id == $t) | .mention_count == 3 and .last_message_id == $l)
  and (.channels[] | select(.channel_id == $c) | .mention_count == 0)' >/dev/null
# Opening the thread (ack) clears it.
call POST "/api/threads/$NT/ack" '{"message_id":"'"$NTLAST"'"}' >/dev/null
call GET /api/users/@me/unread | jq -e --arg t "$NT" 'all(.threads[]; .thread_id != $t)' >/dev/null
# Your own reply counts as read; someone else's makes it unread again.
call POST "/api/channels/$CHAN/messages" '{"content":"on it","thread_id":"'"$NT"'"}' >/dev/null
call GET /api/users/@me/unread | jq -e --arg t "$NT" 'all(.threads[]; .thread_id != $t)' >/dev/null
TOKEN=$TOKEN_A
call GET /api/users/@me/unread | jq -e --arg t "$NT" '.threads[] | select(.thread_id == $t) | .mention_count == 0' >/dev/null
call POST "/api/threads/$NT/ack" '{"message_id":"'"$NTLAST"'"}' >/dev/null
call GET /api/users/@me/unread | jq -e --arg t "$NT" '.threads[] | select(.thread_id == $t) | .last_read_id != null' >/dev/null
TOKEN=$OUT; fail_call POST "/api/threads/$NT/ack" '{"message_id":"'"$NTLAST"'"}'; TOKEN=$TOKEN_A
fail_call POST "/api/threads/1/ack" '{"message_id":"1"}'

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

step "repeating raids: changing the pattern from a raid on"
PFIRST=$(date -u -d '+1 day' +%Y-%m-%dT20:00:00Z)
PD=$(( $(date -u -d '+1 day' +%u) - 1 ))
PS=$(call POST "/api/channels/$RX/events" '{"title":"Pattern raid","starts_at":"'"$PFIRST"'","post_rule":{"mode":"now"},"repeat":{"weekdays":['"$PD"'],"every_weeks":1,"count":8,"time_zone":"UTC"}}')
PSER=$(jq -r .event.series_id <<<"$PS")
PTO=$(date -u -d '+60 days' +%Y-%m-%dT%H:%M:%SZ); PFROM=$(date -u -d '-1 day' +%Y-%m-%dT%H:%M:%SZ)
pevents() { call GET "/api/guilds/$GUILD/events?from=$PFROM&to=$PTO" | jq --arg s "$1" '[.[] | select(.series_id == $s)]'; }
PN=$(pevents "$PSER" | jq length); [ "$PN" -ge 4 ] || { echo "only $PN raids made"; exit 1; }
P0=$(pevents "$PSER" | jq -r '.[0].id'); P1=$(pevents "$PSER" | jq -r '.[1].id'); P2=$(pevents "$PSER" | jq -r '.[2].id'); P3=$(pevents "$PSER" | jq -r '.[3].id')
P1START=$(pevents "$PSER" | jq -r '.[1].starts_at')
# Sign-ups: Bob on the 2nd and 3rd raids, Carl on the 3rd.
call PUT "/api/events/$P1/signup" '{"status":"yes","role":"tank","class":"Warrior","spec":"Protection","user_id":"'"$BOB"'"}' >/dev/null
call PUT "/api/events/$P2/signup" '{"status":"yes","role":"tank","class":"Warrior","spec":"Protection","user_id":"'"$BOB"'"}' >/dev/null
TOKEN=$TOKEN_C; call PUT "/api/events/$P2/signup" '{"status":"tentative","role":"healer","class":"Priest","spec":"Holy"}' >/dev/null; TOKEN=$TOKEN_A
# The first raid is over, with attendance taken: it must not change.
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = now() - interval '4 hours' WHERE id = $P0;
  INSERT INTO event_attendance (event_id, user_id, status) VALUES ($P0, $BOB, 'present');
  UPDATE events SET attendance_taken = true WHERE id = $P0"
# The preview with no change: the current pattern, and what's left of its 8 raids from here.
call POST "/api/events/$P1/repeat-preview" '{}' | jq -e '.remaining_count == 7 and .removed_raids == 0 and (.next | length == 4) and .label == "Weekly on '"$(date -u -d '+1 day' +%a)"'" and (.first_of_series | not)' >/dev/null
# Every second week from the 2nd raid: the 3rd (Bob, Carl) goes, the 2nd and 4th stay.
NEWPAT='"repeat":{"weekdays":['"$PD"'],"every_weeks":2,"count":3,"time_zone":"UTC"}'
PV=$(call POST "/api/events/$P1/repeat-preview" '{"starts_at":"'"$P1START"'",'"$NEWPAT"'}')
jq -e --arg s "$P1START" '.next[0] == $s and (.next | length == 3) and ((.next[1] | fromdate) - (.next[0] | fromdate) == 14 * 86400)
  and .removed_raids >= 1 and .removed_signups == 2 and .kept_signups == 1 and .kept_raids >= 2 and .label == "Every 2 weeks on '"$(date -u -d '+1 day' +%a)"'"' <<<"$PV" >/dev/null \
  || { echo "odd preview: $PV"; exit 1; }
# Only raid leaders; only "this and future"; sensible patterns only.
TOKEN=$TOKEN_B
fail_call POST "/api/events/$P1/repeat-preview" '{'"$NEWPAT"'}'
fail_call PATCH "/api/events/$P1" '{"scope":"future",'"$NEWPAT"'}'
TOKEN=$TOKEN_A
fail_call PATCH "/api/events/$P1" '{"scope":"one",'"$NEWPAT"'}'
fail_call POST "/api/events/$P1/repeat-preview" '{"repeat":{"weekdays":['"$PD"'],"every_weeks":5,"time_zone":"UTC"}}'
fail_call POST "/api/events/$P1/repeat-preview" '{"repeat":{"weekdays":[],"every_weeks":1,"time_zone":"UTC"}}'
fail_call POST "/api/events/$P0/repeat-preview" '{}' # already over
fail_call POST "/api/events/$EVENT/repeat-preview" '{}' # doesn't repeat
TOKEN=$OUT; fail_call POST "/api/events/$P1/repeat-preview" '{}'; TOKEN=$TOKEN_A
PR=$(call PATCH "/api/events/$P1" '{"scope":"future","starts_at":"'"$P1START"'",'"$NEWPAT"'}')
NSER=$(jq -r .event.series_id <<<"$PR")
jq -e --arg p "$P1" --arg old "$PSER" '.event.id == $p and .event.series_id != $old and .event.repeat.every_weeks == 2 and .event.repeat.count == 3 and (.event.signups | length == 1)' <<<"$PR" >/dev/null || { echo "odd reply: $PR"; exit 1; }
[ "$NSER" != "$PSER" ]
fail_call GET "/api/events/$P2"
call GET "/api/events/$P3" | jq -e --arg n "$NSER" '.series_id == $n' >/dev/null
pevents "$NSER" | jq -e --arg p "$P1" 'length >= 2 and length <= 3 and .[0].id == $p and ((.[1].starts_at | fromdate) - (.[0].starts_at | fromdate) == 14 * 86400)' >/dev/null
# The finished raid is untouched: old series, attendance kept.
call GET "/api/events/$P0" | jq -e --arg s "$PSER" '.series_id == $s and .attendance_taken and .repeat.every_weeks == 1' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM event_attendance WHERE event_id = $P0")" = 1 ]
[ "$(psql "$DATABASE_URL" -tAc "SELECT ended FROM event_series WHERE id = $PSER")" = t ]
# (Tidy up so the attendance section further down starts clean.)
psql "$DATABASE_URL" -qc "DELETE FROM event_attendance WHERE event_id = $P0; UPDATE events SET attendance_taken = false WHERE id = $P0"
# Bob and Carl hear from the Outhouse Helper about the raid that went.
for who in "$TOKEN_B" "$TOKEN_C"; do
  TOKEN=$who
  HD=$(call GET /api/dms | jq -r '[.[] | select(any(.recipients[]; .bot))][0].id')
  call GET "/api/channels/$HD/messages" | jq -e '.[-1].content | contains("Pattern raid") and contains("moved or cancelled") and contains("every 2 weeks")' >/dev/null || { echo "no DM about the moved raid"; exit 1; }
done
TOKEN=$TOKEN_A
call GET "/api/guilds/$GUILD/audit-log?action=raid_repeat_update" | jq -e '.entries[0] | .target_name == "Pattern raid" and (.details | contains("Weekly on") and contains("→ Every 2 weeks on") and contains("2 sign-ups dropped"))' >/dev/null
# Changing from a series' very first raid changes it in place (DST-safe in Sydney).
QF=$(date -u -d '+2 days' +%Y-%m-%dT09:00:00Z); QD=$(( $(date -u -d '+2 days' +%u) - 1 )); QD2=$(( (QD + 2) % 7 ))
QS=$(call POST "/api/channels/$RX/events" '{"title":"In place","starts_at":"'"$QF"'","repeat":{"weekdays":['"$QD"'],"every_weeks":1,"count":4,"time_zone":"Australia/Sydney"}}')
QSER=$(jq -r .event.series_id <<<"$QS"); QE=$(jq -r .event.id <<<"$QS")
call POST "/api/events/$QE/repeat-preview" '{"repeat":{"weekdays":['"$QD"','"$QD2"'],"every_weeks":1,"count":8,"time_zone":"UTC"}}' | jq -e '.first_of_series and .removed_raids == 0 and .repeat.time_zone == "Australia/Sydney"' >/dev/null
call PATCH "/api/events/$QE" '{"scope":"future","repeat":{"weekdays":['"$QD"','"$QD2"'],"every_weeks":1,"count":8,"time_zone":"UTC"}}' | jq -e --arg s "$QSER" --arg e "$QE" '.event.series_id == $s and .event.id == $e and (.event.repeat.weekdays | length == 2)' >/dev/null
pevents "$QSER" | jq -e 'length == 8' >/dev/null
[ "$(pevents "$QSER" | jq -r '.[].starts_at' | while read -r t; do TZ=Australia/Sydney date -d "$t" +%H:%M; done | sort -u | wc -l)" = 1 ]

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
SUNSTART=$(date -u -d "TZ=\"Australia/Sydney\" $SUN 20:00" +%Y-%m-%dT%H:%M:%SZ)
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
  and (.games[0] | .two_names and .max_level == 60 and .rulesets == ["Normal","PvP","Roleplay","Roleplay PvP"] and (.professions | index("Jewelcrafting") | not))
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

step "raid managers by role, sign-up channel, characters on sign-ups, invite list"
call GET /api/wow/info | jq -e '(.games[0].races | index("Skyborne (Alliance)") and index("Skyborne (Horde)")) and (.games[1].races | index("Skyborne (Horde)") | not) and (.games[2].races | index("Skyborne (Alliance)") | not) and (.classes | length == 9)' >/dev/null
# Settings: anyone in the outhouse can read them, only Manage Outhouse can change them.
call GET "/api/guilds/$GUILD/raid-settings" | jq -e '.manager_role_ids == [] and .default_channel_id == null' >/dev/null
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/raid-settings"; TOKEN=$TOKEN_A
TOKEN=$TOKEN_B; fail_call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[],"default_channel_id":null}'; TOKEN=$TOKEN_A
RMV=$(call POST "/api/guilds/$GUILD/channels" '{"name":"rm-voice","kind":2}' | jq -r .id)
fail_call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":["'"$GUILD"'"]}'
fail_call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":["123"]}'
fail_call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[],"default_channel_id":"'"$RMV"'"}'
call DELETE "/api/channels/$RMV" >/dev/null
RL=$(call POST "/api/guilds/$GUILD/roles" '{"name":"Raid leader"}' | jq -r .id)
call PUT "/api/guilds/$GUILD/members/$CARL/roles/$RL" >/dev/null
# Skyborne pick a faction; any of the nine classes goes (Forever's combinations aren't enforced).
TOKEN=$TOKEN_B
SKY=$(call POST /api/users/@me/characters '{"name":"Shieldy","last_name":"Mcshield","race":"skyborne (horde)","class":"Warrior","level":60}')
jq -e '.race == "Skyborne (Horde)" and .faction == "Horde" and .full_name == "Shieldy Mcshield"' <<<"$SKY" >/dev/null
SKYID=$(jq -r .id <<<"$SKY")
call POST /api/users/@me/characters '{"name":"Sparky","last_name":"Mcshield","race":"Skyborne (Alliance)","class":"Shaman"}' | jq -e '.faction == "Alliance"' >/dev/null
fail_call POST /api/users/@me/characters '{"game":"era","name":"Skyguy","race":"Skyborne (Alliance)","class":"Mage"}'
TOKEN=$TOKEN_A
RM=$(call POST "/api/channels/$RX/events" '{"title":"Manager test","starts_at":"'"$START"'","game":"forever","max_tanks":1,"max_healers":1,"max_dps":2}' | jq -r .event.id)
# Bob signs up with his own character (its class fills in); not someone else's, nor one from another game.
TOKEN=$TOKEN_B
fail_call PUT "/api/events/$RM/signup" '{"status":"yes","role":"tank","class":"Paladin","character_id":"'"$CH1ID"'"}'
HOLY=$(call GET /api/users/@me/characters | jq -r '[.[] | select(.game == "tbc")][0].id')
fail_call PUT "/api/events/$RM/signup" '{"status":"yes","role":"healer","class":"Priest","character_id":"'"$HOLY"'"}'
call PUT "/api/events/$RM/signup" '{"status":"yes","role":"tank","spec":"Protection","character_id":"'"$SKYID"'"}' \
  | jq -e --arg b "$BOB" --arg c "$SKYID" '.event.signups[] | select(.user_id == $b) | .class == "Warrior" and .display_name == "Bob"
      and .character.id == $c and .character.name == "Shieldy" and .character.last_name == "Mcshield" and .character.full_name == "Shieldy Mcshield" and .character.game == "forever"' >/dev/null
# Carl has no say yet.
TOKEN=$TOKEN_C
call PUT "/api/events/$RM/signup" '{"status":"yes","role":"dps","class":"Rogue","spec":"Combat"}' | jq -e --arg c "$CARL" '.event.signups[] | select(.user_id == $c) | .character == null' >/dev/null
fail_call PUT "/api/events/$RM/signup" '{"status":"bench","role":"tank","user_id":"'"$BOB"'","keep_character":true}'
fail_call DELETE "/api/events/$RM/signups/$BOB"
fail_call GET "/api/events/$RM/export"
fail_call PATCH "/api/events/$RM" '{"description":"Hijacked"}'
TOKEN=$TOKEN_A
# Raid leader becomes a raid manager role; #raid-extras the sign-up channel.
call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":["'"$RL"'"],"default_channel_id":"'"$RX"'"}' \
  | jq -e --arg r "$RL" --arg c "$RX" '.manager_role_ids == [$r] and .default_channel_id == $c' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/raid-settings" | jq -e --arg r "$RL" --arg c "$RX" '.manager_role_ids == [$r] and .default_channel_id == $c' >/dev/null; TOKEN=$TOKEN_A
call GET "/api/guilds/$GUILD/audit-log?action=raid_settings_update" | jq -e '.entries | length == 1 and (.[0].details | contains("Raid managers: nobody → Raid leader") and contains("#raid-extras"))' >/dev/null
TOKEN=$TOKEN_C
# Moving Bob to the bench keeps his character.
call PUT "/api/events/$RM/signup" '{"status":"bench","role":"tank","class":"Warrior","spec":"Protection","user_id":"'"$BOB"'","keep_character":true}' \
  | jq -e --arg b "$BOB" '.event.signups[] | select(.user_id == $b) | .status == "bench" and .character.full_name == "Shieldy Mcshield"' >/dev/null
# Adding Alice with one of her characters (not one of Bob's).
fail_call PUT "/api/events/$RM/signup" '{"status":"yes","role":"tank","class":"Paladin","user_id":"'"$ALICE"'","character_id":"'"$SKYID"'"}'
call PUT "/api/events/$RM/signup" '{"status":"yes","role":"tank","class":"Paladin","spec":"Protection","user_id":"'"$ALICE"'","character_id":"'"$CH1ID"'"}' \
  | jq -e --arg a "$ALICE" '.event.signups[] | select(.user_id == $a) | .status == "yes" and .character.full_name == "Tankadin Lightbringer"' >/dev/null
call PATCH "/api/events/$RM" '{"description":"Run by Carl"}' | jq -e '.event.description == "Run by Carl"' >/dev/null
# The invite list: roster (tanks, healers, DPS), then tentative, then the bench; nobody who can't make it.
call GET "/api/events/$RM/export" | jq -e --arg a "$ALICE" --arg b "$BOB" --arg c "$CARL" --arg e "$RM" '
  .event_id == $e and .title == "Manager test" and .game == "forever"
  and (.entries | map(.user_id)) == [$a, $c, $b]
  and (.entries[0] | .role == "tank" and .character.name == "Tankadin" and .character.last_name == "Lightbringer")
  and (.entries[1] | .display_name == "Carl" and .character == null and .role == "dps")
  and (.entries[2] | .status == "bench" and .character.full_name == "Shieldy Mcshield")' >/dev/null
# A removed character stays on old sign-ups.
TOKEN=$TOKEN_B; call DELETE "/api/characters/$SKYID" >/dev/null; TOKEN=$TOKEN_C
call GET "/api/events/$RM" | jq -e --arg b "$BOB" '.signups[] | select(.user_id == $b) | .character.id == null and .character.full_name == "Shieldy Mcshield"' >/dev/null
call DELETE "/api/events/$RM/signups/$BOB" | jq -e --arg b "$BOB" 'all(.event.signups[]; .user_id != $b)' >/dev/null
# Attendance by a raid manager.
TOKEN=$TOKEN_A; RMP=$(call POST "/api/channels/$RX/events" '{"title":"Manager attendance","starts_at":"'"$PAST"'"}' | jq -r .event.id); TOKEN=$TOKEN_C
call PUT "/api/events/$RMP/attendance" '{"records":[{"user_id":"'"$CARL"'","status":"present"}]}' | jq -e 'length == 1' >/dev/null
# Taking the role away (or the setting) ends it.
TOKEN=$TOKEN_A; call DELETE "/api/guilds/$GUILD/members/$CARL/roles/$RL" >/dev/null; TOKEN=$TOKEN_C
fail_call GET "/api/events/$RM/export"
TOKEN=$TOKEN_A; call PUT "/api/guilds/$GUILD/members/$CARL/roles/$RL" >/dev/null
call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[],"default_channel_id":null}' | jq -e '.manager_role_ids == [] and .default_channel_id == null' >/dev/null
TOKEN=$TOKEN_C; fail_call GET "/api/events/$RM/export"; fail_call DELETE "/api/events/$RM/signups/$ALICE"; TOKEN=$TOKEN_A
call GET "/api/events/$RM/export" | jq -e '.entries | length == 2' >/dev/null
call DELETE "/api/events/$RM" >/dev/null; call DELETE "/api/events/$RMP" >/dev/null

step "raid chat cards removed after the raid"
CC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"raid-cleanup"}' | jq -r .id)
call GET "/api/guilds/$GUILD/raid-settings" | jq -e '.card_cleanup_series == "next" and .card_cleanup_single == "off"' >/dev/null
fail_call POST "/api/channels/$CC/events" '{"title":"Bad","starts_at":"'"$LATER"'","card_cleanup":"next"}' # one-off raids have no next raid
fail_call POST "/api/channels/$CC/events" '{"title":"Bad","starts_at":"'"$LATER"'","card_cleanup":"sometime"}'
cards_in() { call GET "/api/channels/$1/messages?limit=100" | jq '[.[] | select(.event != null) | .event.id]'; }
# A weekly raid (the default: "when the next raid's sign-ups go up"), on now; every card goes up
# straight away. Its card stays while it's on.
WSTART=$(date -u -d '-10 minutes' +%Y-%m-%dT%H:%M:%SZ)
WD=$(( $(date -u -d "$WSTART" +%u) - 1 ))
CW=$(call POST "/api/channels/$CC/events" '{"title":"Weekly tidy","starts_at":"'"$WSTART"'","duration_minutes":15,"post_rule":{"mode":"now"},"repeat":{"weekdays":['"$WD"'],"every_weeks":1,"count":3,"time_zone":"UTC"}}')
jq -e '.event.card_cleanup == "next" and (.event.card_removed | not) and .event.message_id != null' <<<"$CW" >/dev/null
CWS=$(jq -r .event.series_id <<<"$CW"); C1=$(jq -r .event.id <<<"$CW"); C1M=$(jq -r .id <<<"$CW")
CFROM=$(date -u -d '-1 day' +%Y-%m-%dT%H:%M:%SZ); CTO=$(date -u -d '+30 days' +%Y-%m-%dT%H:%M:%SZ)
tidy() { call GET "/api/guilds/$GUILD/events?from=$CFROM&to=$CTO" | jq --arg s "$CWS" '[.[] | select(.series_id == $s)]'; }
tidy | jq -e 'length == 3 and all(.[]; .card_cleanup == "next" and .message_id != null)' >/dev/null
C2=$(tidy | jq -r '.[1].id')
call PATCH "/api/events/$C2" '{"description":"Next week"}' >/dev/null # runs the clean-up: nothing to do yet
call GET "/api/events/$C1" | jq -e '.message_id != null' >/dev/null
# It finishes (ended 20 minutes ago, after the next card went up): its card goes.
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = starts_at - interval '25 minutes' WHERE id = $C1"
call PATCH "/api/events/$C2" '{"description":"Next week!"}' >/dev/null
tidy | jq -e '(.[0] | .card_removed and .message_id == null and .signups_open_at == null) and ([.[1:][] | select(.message_id != null and (.card_removed | not))] | length == 2)' >/dev/null
cards_in "$CC" | jq -e --arg a "$C1" 'length == 2 and (index($a) | not)' >/dev/null
call GET "/api/channels/$CC/messages?limit=100" | jq -e --arg m "$C1M" 'all(.[]; .id != $m)' >/dev/null
# The raid is all still there: a raid leader adds people, takes attendance and exports it.
call PUT "/api/events/$C1/signup" '{"status":"yes","role":"tank","class":"Warrior","spec":"Protection"}' | jq -e '(.event.signups | length == 1) and .event.message_id == null' >/dev/null
call PUT "/api/events/$C1/signup" '{"status":"yes","role":"dps","class":"Mage","spec":"Frost","user_id":"'"$BOB"'"}' >/dev/null
call PUT "/api/events/$C1/attendance" '{"all_signed_up":true}' | jq -e 'length == 2' >/dev/null
call GET "/api/events/$C1" | jq -e '.attendance_taken and .card_removed and (.signups | length == 2)' >/dev/null
call GET "/api/events/$C1/export" | jq -e '.entries | length == 2' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/events/$C1" | jq -e '.card_removed' >/dev/null
fail_call PATCH "/api/events/$C2" '{"card_cleanup":"off"}'; TOKEN=$TOKEN_A
# Running it again changes nothing (and doesn't post a new card).
call PATCH "/api/events/$C1" '{"description":"Tidied"}' | jq -e '.event.message_id == null and .event.card_removed and .event.description == "Tidied"' >/dev/null
cards_in "$CC" | jq -e 'length == 2' >/dev/null
# "Edit this and future" changes the choice for the rest of the series.
call PATCH "/api/events/$C2" '{"scope":"future","card_cleanup":"after"}' | jq -e '.event.card_cleanup == "after"' >/dev/null
tidy | jq -e '[.[].card_cleanup] == ["next", "after", "after"]' >/dev/null
call PATCH "/api/events/$C2" '{"scope":"future","card_cleanup":"next"}' >/dev/null
fail_call PATCH "/api/events/$C2" '{"card_cleanup":"sometime"}'

# A raid still on when the next card goes up keeps its card until 3 hours after it ends.
OSTART=$(date -u -d '-10 minutes' +%Y-%m-%dT%H:%M:%SZ)
OD=$(( $(date -u -d "$OSTART" +%u) - 1 ))
OW=$(call POST "/api/channels/$CC/events" '{"title":"Still on","starts_at":"'"$OSTART"'","duration_minutes":60,"post_rule":{"mode":"now"},"repeat":{"weekdays":['"$OD"'],"every_weeks":1,"count":2,"time_zone":"UTC"}}')
O1=$(jq -r .id <<<"$OW"); O1E=$(jq -r .event.id <<<"$OW")
jq -e '.event.message_id != null and (.event.card_removed | not)' <<<"$OW" >/dev/null
OWS=$(jq -r .event.series_id <<<"$OW")
# It ended an hour ago, and the next card went up before it ended: not yet.
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = now() - interval '2 hours' WHERE id = $O1E;
  UPDATE events SET card_posted_at = now() - interval '90 minutes' WHERE series_id = $OWS AND id <> $O1E"
call PATCH "/api/events/$C1" '{"description":"Tidied again"}' >/dev/null # runs the clean-up
call GET "/api/events/$O1E" | jq -e '.message_id != null' >/dev/null
# Ended over 3 hours ago: now it goes.
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = now() - interval '5 hours' WHERE id = $O1E;
  UPDATE events SET card_posted_at = now() - interval '270 minutes' WHERE series_id = $OWS AND id <> $O1E"
call PATCH "/api/events/$C1" '{"description":"Tidied once more"}' >/dev/null
call GET "/api/events/$O1E" | jq -e '.message_id == null and .card_removed' >/dev/null
cards_in "$CC" | jq -e --arg a "$O1E" 'index($a) | not' >/dev/null
call GET "/api/channels/$CC/messages?limit=100" | jq -e --arg m "$O1" 'all(.[]; .id != $m)' >/dev/null

# One-off raids: "3 hours after it ends" (left to the once-a-minute job, checked further down) and Off.
PSTART=$(date -u -d '-40 minutes' +%Y-%m-%dT%H:%M:%SZ)
CA=$(call POST "/api/channels/$CC/events" '{"title":"Tidy after","starts_at":"'"$PSTART"'","duration_minutes":15,"card_cleanup":"after"}')
CAE=$(jq -r .event.id <<<"$CA"); CAM=$(jq -r .id <<<"$CA")
jq -e '.event.card_cleanup == "after" and .event.message_id != null' <<<"$CA" >/dev/null
call PUT "/api/events/$CAE/signup" '{"status":"yes","role":"healer","class":"Priest","spec":"Holy"}' >/dev/null
call PUT "/api/events/$CAE/attendance" '{"all_signed_up":true}' >/dev/null
CO=$(call POST "/api/channels/$CC/events" '{"title":"Keep me","starts_at":"'"$PSTART"'","duration_minutes":15}')
COE=$(jq -r .event.id <<<"$CO")
jq -e '.event.card_cleanup == "off"' <<<"$CO" >/dev/null
fail_call PATCH "/api/events/$COE" '{"card_cleanup":"next"}'
# Ended well over 3 hours ago (both of them).
psql "$DATABASE_URL" -qc "UPDATE events SET starts_at = now() - interval '4 hours' WHERE id IN ($CAE, $COE)"

# Outhouse settings → Raids: the defaults for new raids.
TOKEN=$TOKEN_B; fail_call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[],"card_cleanup_single":"after"}'; TOKEN=$TOKEN_A
fail_call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[],"card_cleanup_single":"next"}'
fail_call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[],"card_cleanup_series":"never"}'
call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[],"card_cleanup_series":"off","card_cleanup_single":"after"}' \
  | jq -e '.card_cleanup_series == "off" and .card_cleanup_single == "after"' >/dev/null
call POST "/api/channels/$CC/events" '{"title":"Default after","starts_at":"'"$LATER"'"}' | jq -e '.event.card_cleanup == "after"' >/dev/null
call POST "/api/channels/$CC/events" '{"title":"Default off","starts_at":"'"$LATER"'","repeat":{"weekdays":[1],"every_weeks":1,"count":1,"time_zone":"UTC"}}' | jq -e '.event.card_cleanup == "off"' >/dev/null
# Leaving them out keeps them.
call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[]}' | jq -e '.card_cleanup_series == "off" and .card_cleanup_single == "after"' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=raid_settings_update" | jq -e 'any(.entries[]; .details | contains("Old repeating raid cards: when the next raid'"'"'s sign-ups go up → keep it"))' >/dev/null
call PUT "/api/guilds/$GUILD/raid-settings" '{"manager_role_ids":[],"card_cleanup_series":"next","card_cleanup_single":"off"}' >/dev/null

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

step "wow: Battle.net linking (mock): WoW: Forever first, re-import updates"
call GET /api/bnet/status | jq -e '.enabled and (.linked | not)' >/dev/null
call GET /api/wow/info | jq -e '.armory_enabled and .armory_region == "us" and .regions == ["us","eu","kr","tw"]
  and (.armory_links | map(.label) == ["Armory","AoTC.gg","Armory","Armory"]) and (.armory_links | map(.game) == ["forever","forever","tbc","era"])
  and (.armory_links[1].template == "https://www.aotc.gg/forever/character/{region}/{slug}")
  and (.armory_links[2].template == "https://worldofwarcraft.blizzard.com/en-us/classicann/{region}/armory/character/{realm}/{name}")
  and (.armory_links[3].template == "https://worldofwarcraft.blizzard.com/en-us/classic1x/{region}/armory/character/{realm}/{name}")' >/dev/null
# Mockadin Lightforge was added by hand before linking: the import finds him instead of adding him twice.
MOCKID=$(call POST /api/users/@me/characters '{"name":"Mockadin","last_name":"Lightforge","class":"Paladin","level":58}' | jq -r 'select(.last_synced_at == null and .blizzard_id == null) | .id')
BEFORE=$(call GET /api/users/@me/characters | jq length)
AUTH_URL=$(call POST /api/bnet/authorize | jq -r .url)
[[ "$AUTH_URL" == "$MOCK/oauth/authorize?client_id=mock-id&redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fapi%2Fbnet%2Fcallback"* ]] || { echo "odd authorize url: $AUTH_URL"; exit 1; }
STATE=$(sed -E 's/.*[?&]state=([^&]+).*/\1/' <<<"$AUTH_URL")
curl -fsS "$BASE/api/bnet/callback?code=bad-code&state=$STATE" | grep -q "Couldn't" || { echo "bad code should fail"; exit 1; }
STATE=$(call POST /api/bnet/authorize | jq -r .url | sed -E 's/.*[?&]state=([^&]+).*/\1/')
# Forever: Mockadin + Breezy (Deathy the Death Knight is turned away); Classic Era: Twinky (Deathy again, no).
curl -fsS "$BASE/api/bnet/callback?code=good-code&state=$STATE" | grep -q "Imported 3 characters" || { echo "Battle.net import failed"; curl -sS "$BASE/api/bnet/callback?code=good-code&state=$STATE"; exit 1; }
curl -fsS "$BASE/api/bnet/callback?code=good-code&state=$STATE" | grep -q "expired" || { echo "state reused"; exit 1; }
call GET /api/bnet/status | jq -e '.linked and .battletag == "Mockadin#1234"' >/dev/null
call GET /api/users/@me/characters | jq -e --argjson n "$((BEFORE + 2))" --arg m "$MOCKID" 'length == $n
  and ([.[] | select(.name == "Deathy")] | length == 0)
  and (.[] | select(.name == "Mockadin") | .id == $m and .game == "forever" and .last_name == "Lightforge" and .blizzard_id == "1001" and .level == 60
       and .race == "Human" and .faction == "Alliance" and .region == "us" and .source == "manual")
  and (.[] | select(.name == "Breezy") | .game == "forever" and .last_name == "Cloudsong" and .full_name == "Breezy Cloudsong" and .realm == ""
       and .race == "Skyborne (Horde)" and .faction == "Horde" and .class == "Shaman" and .ruleset == "Roleplay PvP" and .blizzard_id == "1002"
       and .source == "bnet" and .level == 37)
  and (.[] | select(.name == "Twinky") | .game == "era" and .realm == "Nightslayer" and .last_name == "")' >/dev/null \
  || { echo "import mapping:"; call GET /api/users/@me/characters | jq -c '.[] | {name,last_name,game,realm,race,faction,class,level,ruleset,blizzard_id,source}'; exit 1; }
# The Armory is read for WoW: Forever characters in the background: portrait, guild, item level.
wait_chars() { # JQ_FILTER WHAT
  for i in $(seq 1 30); do call GET /api/users/@me/characters | jq -e "$1" >/dev/null 2>&1 && return 0; sleep 0.5; done
  echo "never got: $2"; call GET /api/users/@me/characters | jq -c '.[] | {name,level,guild_name,item_level,avatar,last_synced_at}'; exit 1
}
wait_chars '(.[] | select(.name == "Breezy") | .guild_name == "Throne Room" and .item_level == 42 and (.avatar | startswith("/api/wow/avatars/us-breezy-cloudsong?v=")) and .last_synced_at != null)
  and (.[] | select(.name == "Mockadin") | .guild_name == "Forever Flush" and .item_level == 63 and (.avatar | startswith("/api/wow/avatars/us-mockadin-lightforge")))' "Armory data after import"
BREEZY=$(call GET /api/users/@me/characters | jq -r '.[] | select(.name == "Breezy") | .id')
AV=$(call GET /api/users/@me/characters | jq -r '.[] | select(.name == "Breezy") | .avatar')
curl -fsS -D /tmp/av-headers -o /tmp/av.png "$BASE$AV"
grep -qi "^content-type: image/png" /tmp/av-headers && grep -qi "^x-content-type-options: nosniff" /tmp/av-headers
head -c 8 /tmp/av.png | od -An -c | grep -q "P   N   G" || { echo "avatar isn't a PNG"; exit 1; }
code=$(curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/wow/avatars/..%2Fetc%2Fpasswd"); [ "$code" = 404 ] || { echo "bad avatar key -> $code"; exit 1; }
code=$(curl -sS -o /dev/null -w "%{http_code}" "$BASE/api/wow/avatars/us-nobody-here"); [ "$code" = 404 ]
# Linking again updates (Breezy levelled) instead of adding them again.
STATE=$(call POST /api/bnet/authorize | jq -r .url | sed -E 's/.*[?&]state=([^&]+).*/\1/')
curl -fsS "$BASE/api/bnet/callback?code=good-code&state=$STATE" | grep -q "Imported 3 characters"
call GET /api/users/@me/characters | jq -e --argjson n "$((BEFORE + 2))" --arg b "$BREEZY" 'length == $n and ([.[] | select(.name == "Breezy")] | length == 1) and (.[] | select(.name == "Breezy") | .id == $b)' >/dev/null
wait_chars '.[] | select(.name == "Breezy") | .level == 38' "Breezy's new level after re-import"
call DELETE /api/bnet/link >/dev/null
call GET /api/bnet/status | jq -e '.linked | not' >/dev/null
call DELETE "/api/characters/$(call GET /api/users/@me/characters | jq -r '.[] | select(.name == "Twinky") | .id')" >/dev/null
call GET /api/users/@me/characters | jq -e --argjson n "$((BEFORE + 1))" 'length == $n and ([.[] | select(.is_main)] | length == 1)' >/dev/null
TOKEN=""; fail_call POST /api/bnet/authorize; TOKEN=$TOKEN_A

step "wow: Armory look-ups, refresh, links (mock)"
TOKEN=$TOKEN_B
L=$(call POST /api/wow/armory/lookup '{"region":"US","name":"sunny","last_name":"SKYWARD"}')
jq -e '.found and .region == "us" and .blizzard_id == "1005" and .name == "Sunny" and .last_name == "Skyward" and .class == "Mage"
  and .race == "Skyborne (Alliance)" and .faction == "Alliance" and .level == 52 and .ruleset == "Roleplay" and .guild_name == "Cloud Nine"
  and .item_level == 48 and (.avatar | startswith("/api/wow/avatars/us-sunny-skyward?v="))' <<<"$L" >/dev/null || { echo "lookup: $L"; exit 1; }
curl -fsS -o /dev/null "$BASE$(jq -r .avatar <<<"$L")"
# Saving what the look-up found keeps the Armory data (portrait, guild, item level, Blizzard's id).
SUNNY=$(call POST /api/users/@me/characters "$(jq -c '{name, last_name, class, race, level, ruleset, region, game: "forever"}' <<<"$L")")
jq -e '.blizzard_id == "1005" and .guild_name == "Cloud Nine" and .item_level == 48 and .region == "us" and .faction == "Alliance"
  and (.avatar | startswith("/api/wow/avatars/us-sunny-skyward")) and .last_synced_at != null and .armory_status == ""' <<<"$SUNNY" >/dev/null || { echo "saved: $SUNNY"; exit 1; }
SUNNYID=$(jq -r .id <<<"$SUNNY")
# Not on the Armory (yet): a friendly answer, not an error. EU doesn't know Mockadin.
call POST /api/wow/armory/lookup '{"region":"us","name":"Ghosty","last_name":"Nobody"}' | jq -e '(.found | not) and .reason == "not_found" and (.message | contains("doesn'"'"'t know Ghosty Nobody on US"))' >/dev/null
call POST /api/wow/armory/lookup '{"region":"eu","name":"Mockadin","last_name":"Lightforge"}' | jq -e '(.found | not) and .reason == "not_found" and .region == "eu"' >/dev/null
fail_call POST /api/wow/armory/lookup '{"region":"xx","name":"Sunny","last_name":"Skyward"}'
fail_call POST /api/wow/armory/lookup '{"region":"us","name":"Sunny"}'
fail_call POST /api/wow/armory/lookup '{"region":"us","name":"Su nny","last_name":"Skyward"}'
fail_call POST /api/users/@me/characters '{"name":"Region","last_name":"Smith","class":"Mage","region":"mars"}'
TOKEN=""; fail_call POST /api/wow/armory/lookup '{"region":"us","name":"Sunny","last_name":"Skyward"}'; TOKEN=$TOKEN_B
# Battle.net says slow down (429): we wait as long as it asks, and say so.
call POST /api/wow/armory/lookup '{"region":"us","name":"Busy","last_name":"Bee"}' | jq -e '(.found | not) and .reason == "busy" and (.message | contains("busy"))' >/dev/null
call POST /api/wow/armory/lookup '{"region":"us","name":"Ghosty","last_name":"Nobody"}' | jq -e '.reason == "busy"' >/dev/null
sleep 2.5
call POST /api/wow/armory/lookup '{"region":"us","name":"Ghosty","last_name":"Nobody"}' | jq -e '.reason == "not_found"' >/dev/null
# Refresh: Bob can't refresh Alice's characters; TBC ones aren't on the Armory.
fail_call POST "/api/characters/$BREEZY/refresh"
TOKEN=$TOKEN_A
R=$(call POST "/api/characters/$BREEZY/refresh")
jq -e '.found and .message == "" and .character.level == 39 and .character.guild_name == "Throne Room"' <<<"$R" >/dev/null || { echo "refresh: $R"; exit 1; }
call POST "/api/characters/$BREEZY/refresh" | jq -e '.found and (.message | contains("moment ago")) and .character.level == 39' >/dev/null
TBCX=$(call POST /api/users/@me/characters '{"game":"tbc","name":"Refreshme","realm":"Spineshatter","class":"Mage","level":70}' | jq -r .id)
fail_call POST "/api/characters/$TBCX/refresh"
call DELETE "/api/characters/$TBCX" >/dev/null
# The app token was fetched once and reused.
curl -fsS "$MOCK/__hits" | jq -e '."app-token" == 1' >/dev/null || { echo "app token not cached:"; curl -sS "$MOCK/__hits"; exit 1; }
# A new name is a different character to the Armory: the old data goes; not found → noted, nothing breaks.
TOKEN=$TOKEN_B
call PATCH "/api/characters/$SUNNYID" "$(jq -c '.last_name = "Elsewhere"' <<<"$SUNNY")" | jq -e '.blizzard_id == null and .avatar == "" and .guild_name == "" and .item_level == null and .armory_status == "not_found" and .region == "us"' >/dev/null
call POST "/api/characters/$SUNNYID/refresh" | jq -e '(.found | not) and (.message | contains("doesn'"'"'t know Sunny Elsewhere")) and .character.armory_status == "not_found" and .character.level == 52' >/dev/null
call PATCH "/api/characters/$SUNNYID" "$(jq -c '.last_name = "Skyward"' <<<"$SUNNY")" | jq -e '.armory_status == "" and .guild_name == "Cloud Nine" and .blizzard_id == "1005"' >/dev/null
TOKEN=$TOKEN_A
# /char shows the portrait, guild, item level and Armory links; the profile has them too.
call POST "/api/channels/$CHAN/wow/char" '{"query":"Mockadin"}' | jq -e '.embeds[0] | .title == "Mockadin Lightforge"
  and (.thumbnail_url | startswith("/api/wow/avatars/us-mockadin-lightforge"))
  and (.fields[] | select(.name == "Guild") | .value == "<Forever Flush>") and (.fields[] | select(.name == "Item level") | .value == "63")
  and (.fields[] | select(.name == "Armory") | .value | contains("Armory: https://worldofwarcraft.blizzard.com/en-us/forever/us/armory/character/mockadin-lightforge") and contains("AoTC.gg: https://www.aotc.gg/forever/character/us/mockadin-lightforge"))
  and .url == "https://worldofwarcraft.blizzard.com/en-us/forever/us/armory/character/mockadin-lightforge" and (.footer | startswith("From the Armory"))' >/dev/null
call GET "/api/profiles/$ALICE" | jq -e '.characters[] | select(.name == "Breezy") | .item_level == 42 and .guild_name == "Throne Room" and (.avatar | length > 0)' >/dev/null
# Raid rosters show what the Armory says about the character someone brings.
AR=$(call POST "/api/channels/$CHAN/events" '{"title":"Armory raid","starts_at":"'"$START"'","game":"forever"}' | jq -r .event.id)
call PUT "/api/events/$AR/signup" '{"status":"yes","role":"healer","spec":"Restoration","character_id":"'"$BREEZY"'"}' \
  | jq -e '.event.signups[0].character | .full_name == "Breezy Cloudsong" and .level == 39 and .item_level == 42 and .guild_name == "Throne Room" and (.avatar | startswith("/api/wow/avatars/"))' >/dev/null
call DELETE "/api/events/$AR?scope=one" >/dev/null
# Item cards from WoW: Forever's own item data when Wowhead doesn't have the item.
call POST "/api/channels/$CHAN/wow/item" '{"query":"990001"}' | jq -e '.embeds[0] | .kind == "wow-item" and .title == "Skyborne Sash" and .color == 10696174
  and (.description | contains("Item Level 62")) and (.footer | contains("Battle.net")) and (.thumbnail_url | endswith("/icons/inv_belt_13.jpg"))' >/dev/null
call POST "/api/channels/$CHAN/wow/item" '{"query":"Skyborne Sash"}' | jq -e '.embeds[0].title == "Skyborne Sash"' >/dev/null

step "wow: Armory links for TBC Anniversary and Classic Era characters"
TBCC=$(call POST /api/users/@me/characters '{"game":"tbc","name":"Kaelthas","realm":"Pyrewood Village","region":"EU","race":"Blood Elf","class":"Mage","level":70}')
jq -e '.game == "tbc" and .region == "eu" and .realm == "Pyrewood Village"' <<<"$TBCC" >/dev/null || { echo "odd TBC character: $TBCC"; exit 1; }
call POST "/api/channels/$CHAN/wow/char" '{"query":"Kaelthas-Pyrewood Village"}' | jq -e '.embeds[0]
  | .url == "https://worldofwarcraft.blizzard.com/en-us/classicann/eu/armory/character/pyrewood-village/kaelthas"
  and (.fields[] | select(.name == "Armory") | .value == "Armory: https://worldofwarcraft.blizzard.com/en-us/classicann/eu/armory/character/pyrewood-village/kaelthas")' >/dev/null \
  || { echo "no TBC armory link"; cat /tmp/body; exit 1; }
# Classic Era, no region set: the server's (us).
call POST /api/users/@me/characters '{"game":"era","name":"Grimbold","realm":"Old Blanchy","class":"Warrior","level":60}' >/dev/null
call POST "/api/channels/$CHAN/wow/char" '{"query":"Grimbold-Old Blanchy"}' | jq -e '.embeds[0]
  | .url == "https://worldofwarcraft.blizzard.com/en-us/classic1x/us/armory/character/old-blanchy/grimbold"
  and (.fields[] | select(.name == "Armory") | .value | contains("classic1x/us/armory/character/old-blanchy/grimbold"))' >/dev/null \
  || { echo "no Era armory link"; cat /tmp/body; exit 1; }

step "wow: without Battle.net keys"
BIND_ADDR=127.0.0.1:8081 BNET_CLIENT_ID= BNET_CLIENT_SECRET= ./target/debug/outhouse-server >/tmp/outhouse-nokeys.log 2>&1 &
NOKEYS=$!
for i in $(seq 1 60); do curl -fsS "http://127.0.0.1:8081/api/health" >/dev/null 2>&1 && break; sleep 0.5; done
B8081() { curl -sS -o /tmp/nk -w "%{http_code}" -H "Authorization: Bearer $TOKEN_A" -H "Content-Type: application/json" "$@"; }
[ "$(B8081 http://127.0.0.1:8081/api/wow/info)" = 200 ]
jq -e '(.bnet_enabled | not) and (.armory_enabled | not) and (.armory_links | length == 4)' /tmp/nk >/dev/null || { cat /tmp/nk; exit 1; }
[ "$(B8081 -X POST -d '{"region":"us","name":"Sunny","last_name":"Skyward"}' http://127.0.0.1:8081/api/wow/armory/lookup)" = 400 ]
grep -q "deploy/README.md" /tmp/nk
[ "$(B8081 -X POST http://127.0.0.1:8081/api/characters/$BREEZY/refresh)" = 400 ]
[ "$(B8081 http://127.0.0.1:8081/api/bnet/status)" = 200 ] && jq -e '(.enabled | not)' /tmp/nk >/dev/null
# Saving a character with a region still works (no Armory call).
[ "$(B8081 -X POST -d '{"name":"Offline","last_name":"Keys","class":"Mage","region":"eu"}' http://127.0.0.1:8081/api/users/@me/characters)" = 201 ]
jq -e '.region == "eu" and .last_synced_at == null' /tmp/nk >/dev/null
call DELETE "/api/characters/$(jq -r .id /tmp/nk)" >/dev/null
kill $NOKEYS 2>/dev/null || true
if grep -qi "mock-secret\|mock-app-token\|mock-token" "$LOG" /tmp/outhouse-nokeys.log; then echo "a secret or token got into the server log"; exit 1; fi
if grep -q "Armory refresh failed" "$LOG"; then echo "the Armory refresher failed:"; grep "Armory refresh failed" "$LOG"; exit 1; fi

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

step "built-in soundboard classics"
TOKEN=$TOKEN_A
call GET /api/builtin-sounds | jq -e '[.[].id] == ["builtin:flush","builtin:strain","builtin:plop","builtin:door","builtin:fly"]' >/dev/null
# Every outhouse has them (new ones too), listed first.
G2=$(call POST /api/guilds '{"name":"Second Outhouse"}' | jq -r .id)
for g in "$GUILD" "$G2"; do
  call GET "/api/guilds/$g/sounds" | jq -e '([.[] | select(.builtin)] | length) == 5 and .[0].id == "builtin:flush" and .[0].emoji == "🚽"
    and all(.[] | select(.builtin); (.url | startswith("/sounds/builtin/")) and (.hidden | not) and .duration_ms <= 10000)' >/dev/null \
    || { echo "built-ins missing in $g"; cat /tmp/body; exit 1; }
done
# The files are served as MP3 and decode to the right sort of length.
for k in flush strain plop door fly; do
  curl -fsS -D /tmp/headers "$BASE/sounds/builtin/$k.mp3" -o "/tmp/builtin-$k.mp3"
  grep -qi "content-type: audio/mpeg" /tmp/headers || { echo "$k.mp3 content type"; cat /tmp/headers; exit 1; }
  if command -v ffmpeg >/dev/null; then
    d=$(ffmpeg -v error -i "/tmp/builtin-$k.mp3" -f s16le -ac 1 -ar 8000 - | wc -c | awk '{print $1 / 16000}')
    awk -v d="$d" 'BEGIN { exit !(d > 0.3 && d <= 10) }' || { echo "$k.mp3 decodes to $d s"; exit 1; }
    echo "  $k.mp3 decodes to $d s"
  fi
done
# They can't be deleted or renamed; anyone can favourite them.
fail_call DELETE /api/sounds/builtin:flush; grep -q "hide" /tmp/body
fail_call PATCH /api/sounds/builtin:flush '{"name":"Nope"}'
fail_call PUT /api/sounds/builtin:nope/favourite
call PUT /api/sounds/builtin:plop/favourite >/dev/null
call GET "/api/guilds/$G2/sounds" | jq -e '.[] | select(.id == "builtin:plop") | .favourite' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/sounds" | jq -e '.[] | select(.id == "builtin:plop") | .favourite | not' >/dev/null
# Hiding one: managers only. Members stop seeing it; managers see it marked hidden.
fail_call PUT "/api/guilds/$GUILD/hidden-sounds/strain"
TOKEN=$OUT; fail_call GET "/api/guilds/$GUILD/sounds"; fail_call DELETE "/api/guilds/$GUILD/hidden-sounds/strain"; TOKEN=$TOKEN_A
call PUT "/api/guilds/$GUILD/hidden-sounds/strain" >/dev/null
fail_call PUT "/api/guilds/$GUILD/hidden-sounds/nope"
call GET "/api/guilds/$GUILD/sounds" | jq -e '.[] | select(.id == "builtin:strain") | .hidden' >/dev/null
call GET "/api/guilds/$G2/sounds" | jq -e '.[] | select(.id == "builtin:strain") | .hidden | not' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/sounds" | jq -e 'all(.[]; .id != "builtin:strain") and any(.[]; .id == "builtin:door")' >/dev/null
# Renaming an uploaded clip.
TOKEN=$TOKEN_B; fail_call PATCH "/api/sounds/$SND" '{"name":"Bob was here"}'
TOKEN=$TOKEN_A; call PATCH "/api/sounds/$SND" '{"name":"Bonk!","emoji":"🔨"}' | jq -e '.name == "Bonk!"' >/dev/null
# Built-in entrance sounds.
call PUT /api/users/@me/entrance-sound '{"builtin":"door"}' | jq -e '.builtin == "door" and .url == "/sounds/builtin/door.mp3"' >/dev/null
call GET /api/users/@me/entrance-sound | jq -e '.builtin == "door"' >/dev/null
fail_call PUT /api/users/@me/entrance-sound '{"builtin":"nope"}'
# Playing: Bob listens in voice while Alice plays.
TOKEN=$TOKEN_B; call POST "/api/channels/$VOICE/voice/join" >/dev/null
node ci/gateway_probe.mjs "$BASE" "$TOKEN_B" /tmp/gw-sb.jsonl 25 &
PROBE=$!
for i in $(seq 1 40); do grep -q '"READY"' /tmp/gw-sb.jsonl 2>/dev/null && break; sleep 0.25; done
TOKEN=$TOKEN_A
call POST "/api/channels/$VOICE/voice/join" >/dev/null   # door creak entrance for Bob
sleep 2.2
call POST /api/sounds/builtin:flush/play >/dev/null
fail_call POST /api/sounds/builtin:plop/play; grep -q "Easy on the soundboard" /tmp/body   # same cooldown as uploads
sleep 2.2
fail_call POST /api/sounds/builtin:strain/play; grep -q "hidden" /tmp/body
fail_call POST /api/sounds/builtin:nope/play
# Sounds from your other outhouses play here too (Discord-style)...
upload "/api/guilds/$G2/sounds" /tmp/short.wav -F name=Faraway -F "emoji=🛸"
[[ "$code" == 200 ]] || { echo "second outhouse sound -> $code"; cat /tmp/body; exit 1; }
SND2=$(jq -r .id /tmp/body)
call GET /api/users/@me/sounds | jq -e --arg s "$SND2" --arg g "$G2" --arg s1 "$SND" 'any(.[]; .id == $s and .guild_id == $g) and any(.[]; .id == $s1) and all(.[]; .builtin | not)' >/dev/null
call POST "/api/sounds/$SND2/play" >/dev/null
# ...but not ones from an outhouse you aren't in.
TOKEN=$TOKEN_B
call GET /api/users/@me/sounds | jq -e --arg s "$SND2" 'all(.[]; .id != $s)' >/dev/null
fail_call POST "/api/sounds/$SND2/play"
call POST /api/sounds/builtin:door/play >/dev/null
TOKEN=$TOKEN_A
call DELETE "/api/guilds/$GUILD/hidden-sounds/strain" >/dev/null
sleep 2.2
call POST /api/sounds/builtin:strain/play >/dev/null
for i in $(seq 1 40); do grep -q 'builtin:strain' /tmp/gw-sb.jsonl && break; sleep 0.25; done
kill $PROBE 2>/dev/null || true
jq -s -e --arg g "$GUILD" --arg v "$VOICE" --arg s2 "$SND2" '
  any(.[]; .t == "SOUNDBOARD_PLAY" and .d.sound_id == "builtin:flush" and .d.url == "/sounds/builtin/flush.mp3" and .d.channel_id == $v and .d.guild_id == $g)
  and any(.[]; .t == "SOUNDBOARD_PLAY" and .d.sound_id == $s2 and .d.guild_id == $g and (.d.url | startswith("/files/")))
  and any(.[]; .t == "SOUNDBOARD_PLAY" and .d.sound_id == "builtin:strain")
  and any(.[]; .t == "ENTRANCE_SOUND" and .d.url == "/sounds/builtin/door.mp3")
  and any(.[]; .t == "SOUNDS_CHANGED" and .d.guild_id == $g)
  and all(.[]; .t == "SOUNDBOARD_PLAY" and .d.sound_id == "builtin:plop" | not)' /tmp/gw-sb.jsonl >/dev/null \
  || { echo "soundboard events wrong:"; cat /tmp/gw-sb.jsonl; exit 1; }
TOKEN=$TOKEN_B; call POST /api/voice/leave >/dev/null
TOKEN=$TOKEN_A; call POST /api/voice/leave >/dev/null
# Swapping a built-in entrance for an upload and back cleans up the file.
upload /api/users/@me/entrance-sound /tmp/short.wav
[[ "$code" == 200 ]] || { echo "entrance upload after built-in -> $code"; cat /tmp/body; exit 1; }
jq -e '.builtin == null and (.url | startswith("/files/"))' /tmp/body >/dev/null
ENT=$(jq -r .url /tmp/body)
call PUT /api/users/@me/entrance-sound '{"builtin":"plop"}' | jq -e '.builtin == "plop"' >/dev/null
curl -sS -o /dev/null -w "%{http_code}" "$BASE$ENT" | grep -q 404
call DELETE /api/users/@me/entrance-sound >/dev/null
call GET /api/users/@me/entrance-sound | jq -e '. == null' >/dev/null

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

step "manage nicknames"
TOKEN=$TOKEN_A
call GET /api/permissions | jq -e 'any(.[]; .key == "manage_nicknames" and .bit == "1024") and ([.[].key] | index("manage_nicknames")) == ([.[].key] | index("change_nickname")) + 1' >/dev/null
# @everyone doesn't get it.
call GET "/api/guilds/$GUILD/roles" | jq -e --arg g "$GUILD" '.[] | select(.id == $g) | ((.permissions | tonumber) / 1024 | floor) % 2 == 0' >/dev/null
TOKEN=$TOKEN_B
fail_call PUT "/api/guilds/$GUILD/members/$CARL/nickname" '{"nickname":"Not allowed"}'
# You can still do your own through the new path (Change nickname).
call PUT "/api/guilds/$GUILD/members/$BOB/nickname" '{"nickname":"  Bobby  "}' | jq -e '.display_name == "Bobby"' >/dev/null
call PUT "/api/guilds/$GUILD/nickname" '{"nickname":null}' | jq -e '.display_name == "Bob"' >/dev/null
TOKEN=$TOKEN_A
NICKR=$(call POST "/api/guilds/$GUILD/roles" '{"name":"Nick mod","permissions":"1024"}' | jq -r .id)
call PATCH "/api/guilds/$GUILD/roles/$NICKR" '{"position":500}' >/dev/null
call PUT "/api/guilds/$GUILD/members/$BOB/roles/$NICKR" >/dev/null
TOKEN=$TOKEN_B; call GET "/api/guilds/$GUILD/permissions" | jq -e '((.guild | tonumber) / 1024 | floor) % 2 == 1' >/dev/null; TOKEN=$TOKEN_A
# Carl's other clients hear about it live.
node ci/gateway_probe.mjs "$BASE" "$TOKEN_C" /tmp/gw-nick.jsonl 15 &
NPROBE=$!
for i in $(seq 1 40); do grep -q '"READY"' /tmp/gw-nick.jsonl 2>/dev/null && break; sleep 0.25; done
grep -q '"READY"' /tmp/gw-nick.jsonl || { echo "gateway probe never got READY"; cat /tmp/gw-nick.jsonl; exit 1; }
TOKEN=$TOKEN_B
call PUT "/api/guilds/$GUILD/members/$CARL/nickname" '{"nickname":"Carl the Brave and Extremely Long Named Warrior"}' \
  | jq -e --arg c "$CARL" '.id == $c and .display_name == "Carl the Brave and Extremely Lon"' >/dev/null
for i in $(seq 1 40); do grep -q '"GUILD_REFRESH"' /tmp/gw-nick.jsonl && break; sleep 0.25; done
jq -s -e --arg g "$GUILD" 'any(.[]; .t == "GUILD_REFRESH" and .d.guild_id == $g)' /tmp/gw-nick.jsonl >/dev/null \
  || { echo "no GUILD_REFRESH after a nickname change:"; cat /tmp/gw-nick.jsonl; exit 1; }
kill $NPROBE 2>/dev/null || true
call GET "/api/guilds/$GUILD/members" | jq -e --arg c "$CARL" '.[] | select(.id == $c) | .display_name == "Carl the Brave and Extremely Lon"' >/dev/null
# The owner is off limits; so is anyone ranked the same or higher.
fail_call PUT "/api/guilds/$GUILD/members/$ALICE/nickname" '{"nickname":"Boss"}'
TOKEN=$TOKEN_A; call PUT "/api/guilds/$GUILD/members/$CARL/roles/$NICKR" >/dev/null; TOKEN=$TOKEN_B
fail_call PUT "/api/guilds/$GUILD/members/$CARL/nickname" '{"nickname":"Equal rank"}'
TOKEN=$TOKEN_A; call DELETE "/api/guilds/$GUILD/members/$CARL/roles/$NICKR" >/dev/null; TOKEN=$TOKEN_B
fail_call PUT "/api/guilds/$GUILD/members/$OUTSIDER/nickname" '{"nickname":"Who"}'
# Clearing it (null or blank) puts their display name back.
call PUT "/api/guilds/$GUILD/members/$CARL/nickname" '{"nickname":"   "}' | jq -e '.display_name == "Carl"' >/dev/null
call PUT "/api/guilds/$GUILD/members/$CARL/nickname" '{"nickname":"Carlos"}' >/dev/null
call PUT "/api/guilds/$GUILD/members/$CARL/nickname" '{"nickname":null}' | jq -e '.display_name == "Carl"' >/dev/null
TOKEN=$TOKEN_A
# The owner can rename anyone (even a moderator); all of it lands in the audit log.
call PUT "/api/guilds/$GUILD/members/$BOB/nickname" '{"nickname":"Bobsled"}' | jq -e '.display_name == "Bobsled"' >/dev/null
NLOG=$(call GET "/api/guilds/$GUILD/audit-log?action=member_nickname&limit=20")
jq -e --arg a "$ALICE" --arg b "$BOB" --arg c "$CARL" '
  .entries[0].actor_id == $a and .entries[0].target_id == $b and .entries[0].target_kind == "member"
    and .entries[0].target_name == "Bob" and .entries[0].details == "(none) → “Bobsled”"
  and any(.entries[]; .actor_id == $b and .target_id == $c and .details == "(none) → “Carl the Brave and Extremely Lon”")
  and any(.entries[]; .details == "“Carlos” → (none)")
  and all(.entries[]; .target_id != .actor_id)
  and any(.actions[]; .key == "member_nickname" and .label == "Changed nickname")' <<<"$NLOG" >/dev/null \
  || { echo "bad nickname audit entries:"; echo "$NLOG" | jq '.entries'; exit 1; }
# Setting the same nickname again isn't logged twice.
call PUT "/api/guilds/$GUILD/members/$BOB/nickname" '{"nickname":"Bobsled"}' >/dev/null
call GET "/api/guilds/$GUILD/audit-log?action=member_nickname&user=$BOB&limit=50" | jq -e '[.entries[] | select(.details == "(none) → “Bobsled”")] | length == 1' >/dev/null
call PUT "/api/guilds/$GUILD/members/$BOB/nickname" '{"nickname":null}' >/dev/null
call DELETE "/api/guilds/$GUILD/members/$BOB/roles/$NICKR" >/dev/null
TOKEN=$TOKEN_B; fail_call PUT "/api/guilds/$GUILD/members/$CARL/nickname" '{"nickname":"Not any more"}'; TOKEN=$TOKEN_A
call DELETE "/api/guilds/$GUILD/roles/$NICKR" >/dev/null

step "server status and health"
call GET /api/health | jq -e '.status == "ok" and .db == true and .version == "smoke-test" and .uptime_seconds >= 0' >/dev/null
call GET "/api/guilds/$GUILD/server-status" | jq -e '.version == "smoke-test" and .db_ok and .db_size_bytes > 0 and .push_enabled and .backup == null' >/dev/null
printf '{"time":"%s","ok":true,"file":"outhouse-test.tgz","size":"1.2M","offsite":"ok: smb","message":""}\n' "$(date -Iseconds)" > "$BACKUP_STATUS_FILE"
call GET "/api/guilds/$GUILD/server-status" | jq -e '.backup.ok and .backup.file == "outhouse-test.tgz" and .backup.offsite == "ok: smb"' >/dev/null
TOKEN=$TOKEN_B; fail_call GET "/api/guilds/$GUILD/server-status"; TOKEN=$TOKEN_A
# Only the site owner (first account, Alice) gets server status, not other outhouse admins.
call GET /api/users/@me/site-owner | jq -e '.site_owner == true' >/dev/null
TOKEN=$TOKEN_B; call GET /api/users/@me/site-owner | jq -e '.site_owner == false' >/dev/null; TOKEN=$TOKEN_A
BOBG=$(TOKEN=$TOKEN_B call POST /api/guilds '{"name":"Bob place"}' | jq -r .id)
TOKEN=$TOKEN_B; fail_call GET "/api/guilds/$BOBG/server-status"; TOKEN=$TOKEN_A

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

step "muting a DM (for a while or for good)"
TOKEN=$TOKEN_B
fail_call PUT "/api/users/@me/notifications/channels/$DMID" '{"muted":true,"minutes":0}'
fail_call PUT "/api/users/@me/notifications/channels/$DMID" '{"muted":true,"minutes":999999}'
call PUT "/api/users/@me/notifications/channels/$DMID" '{"muted":true,"minutes":15}' | jq -e '.muted and .until != null' >/dev/null
call GET /api/users/@me/notifications | jq -e --arg d "$DMID" '(.muted[] | select(.channel_id == $d) | .guild_id == "0" and .kind == 1 and .until != null) and (.muted_channels | index($d) != null)' >/dev/null
call POST "/api/channels/$DMID/ack" '{"message_id":"'"$(call GET "/api/channels/$DMID/messages?limit=1" | jq -r '.[0].id')"'"}' >/dev/null
TOKEN=$TOKEN_A
call POST "/api/channels/$DMID/messages" '{"content":"muted dm hello"}' >/dev/null
call POST "/api/channels/$DMID/messages" '{"content":"<@'"$BOB"'> muted dm ping"}' >/dev/null
TOKEN=$TOKEN_B
# Both are unread; only the @mention still counts while it's muted.
call GET /api/users/@me/unread | jq -e --arg d "$DMID" '.channels[] | select(.channel_id == $d) | .mention_count == 2 and .direct_count == 1' >/dev/null
# Until you turn it back on, then off again.
call PUT "/api/users/@me/notifications/channels/$DMID" '{"muted":true}' | jq -e '.until == null' >/dev/null
call GET /api/users/@me/notifications | jq -e --arg d "$DMID" '.muted[] | select(.channel_id == $d) | .until == null' >/dev/null
call PUT "/api/users/@me/notifications/channels/$DMID" '{"muted":false}' >/dev/null
call GET /api/users/@me/notifications | jq -e --arg d "$DMID" 'all(.muted[]; .channel_id != $d)' >/dev/null
# A timed mute wears off by itself.
call PUT "/api/users/@me/notifications/channels/$DMID" '{"muted":true,"minutes":60}' >/dev/null
TOKEN=$TOKEN_A
call POST "/api/channels/$DMID/messages" '{"content":"still muted dm"}' >/dev/null
sleep 1.5
psql "$DATABASE_URL" -qc "UPDATE channel_mutes SET muted_until = now() - interval '1 minute' WHERE channel_id = $DMID AND user_id = $BOB"
call POST "/api/channels/$DMID/messages" '{"content":"dm after the mute wore off"}' >/dev/null
TOKEN=$TOKEN_B
call GET /api/users/@me/notifications | jq -e --arg d "$DMID" 'all(.muted[]; .channel_id != $d) and (.muted_channels | index($d) == null)' >/dev/null
[ "$(psql "$DATABASE_URL" -tAc "SELECT count(*) FROM channel_mutes WHERE channel_id = $DMID")" = 0 ]
# Channels take timed mutes too.
call PUT "/api/users/@me/notifications/channels/$PINS" '{"muted":true,"minutes":480}' >/dev/null
call GET /api/users/@me/notifications | jq -e --arg p "$PINS" '.muted[] | select(.channel_id == $p) | .until != null and .guild_id != "0"' >/dev/null
call PUT "/api/users/@me/notifications/channels/$PINS" '{"muted":false}' >/dev/null
TOKEN=$OUT; fail_call PUT "/api/users/@me/notifications/channels/$DMID" '{"muted":true,"minutes":15}'; TOKEN=$TOKEN_A

step "raid sign-up card posted by the scheduler"
for i in $(seq 1 120); do
  call GET "/api/events/$TICKID" | jq -e '.message_id != null' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/events/$TICKID" | jq -e '.message_id != null and .signups_open_at == null' >/dev/null || { echo "card never posted"; exit 1; }
call GET "/api/channels/$PT/messages" | jq -e --arg id "$TICKID" '[.[] | select(.event.id == $id)] | length == 1' >/dev/null
TOKEN=$TOKEN_B; call PUT "/api/events/$TICKID/signup" '{"status":"yes","role":"dps","class":"Mage","spec":"Fire"}' >/dev/null; TOKEN=$TOKEN_A

step "old raid card removed by the once-a-minute job (3 hours after the raid)"
for i in $(seq 1 90); do
  call GET "/api/events/$CAE" | jq -e '.message_id == null' >/dev/null 2>&1 && break
  sleep 1
done
call GET "/api/events/$CAE" | jq -e '.message_id == null and .card_removed and .attendance_taken and (.signups | length == 1)' >/dev/null || { echo "old raid card never removed"; exit 1; }
call GET "/api/channels/$CC/messages?limit=100" | jq -e --arg m "$CAM" 'all(.[]; .id != $m)' >/dev/null
call GET "/api/events/$COE" | jq -e '.message_id != null and (.card_removed | not)' >/dev/null # Off keeps it
call GET "/api/channels/$CC/messages?limit=100" | jq -e --arg e "$COE" 'any(.[]; .event.id == $e)' >/dev/null
call GET "/api/guilds/$GUILD/attendance?limit=50" | jq -e --arg e "$CAE" --arg c "$C1" 'any(.raids[]; .id == $e) and any(.raids[]; .id == $c)' >/dev/null
call GET "/api/guilds/$GUILD/events?from=$CFROM&to=$CTO" | jq -e --arg e "$CAE" '[.[] | select(.id == $e and .card_removed)] | length == 1' >/dev/null

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
  curl -fsS "$PUSH/received/bob" | jq -e 'any(.[]; .payload.body == "last dm for bob") and any(.[]; .payload.body == "dm after the mute wore off")' >/dev/null 2>&1 && break
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
check_push "$RB" 'any(.[]; .payload.kind == "mention" and (.payload.title | contains("🧵 Thread pings")) and (.payload.url | contains("&thread=")) and .payload.thread_id != null)' "bob: mention in a thread"
check_push "$RB" 'all(.[]; (.payload.body // "") | contains("muted dm") | not)' "bob: nothing from a muted DM (not even an @mention)"
check_push "$RB" 'any(.[]; .payload.kind == "dm" and .payload.body == "dm after the mute wore off")' "bob: DMs again once the mute wore off"
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

step "edit channel: name, topic and limits"
TOKEN=$TOKEN_A
# New outhouses: only the front porch gets the old default topic.
NG=$(call POST /api/guilds '{"name":"Topic Test"}' | jq -r .id)
call GET "/api/guilds/$NG/channels" | jq -e '(.[] | select(.name == "front-porch") | .topic == "Leave it cleaner than you found it.") and (.[] | select(.kind == 2) | .topic == "")' >/dev/null
call DELETE "/api/guilds/$NG" >/dev/null
EC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"Edit Me"}' | jq -r .id)
call GET "/api/guilds/$GUILD/channels" | jq -e --arg c "$EC" '.[] | select(.id == $c) | .topic == "" and .slowmode_secs == 0 and .content_mode == "default" and (.nsfw_accepted | not)' >/dev/null
call PATCH "/api/channels/$EC" '{"name":"Raid Talk","topic":"  Strats and `loot`: https://example.com <@'"$BOB"'>  "}' \
  | jq -e --arg b "$BOB" '.name == "raid-talk" and .topic == "Strats and `loot`: https://example.com <@\($b)>"' >/dev/null
LONG=$(python3 -c 'print("x" * 1024)')
call PATCH "/api/channels/$EC" "{\"topic\":\"$LONG\"}" | jq -e '.topic | length == 1024' >/dev/null
fail_call PATCH "/api/channels/$EC" "{\"topic\":\"${LONG}y\"}"
grep -q "1024" /tmp/body
call PATCH "/api/channels/$EC" '{"topic":""}' | jq -e '.topic == ""' >/dev/null
call PATCH "/api/channels/$EC" '{"topic":"Raid strats only"}' >/dev/null
call GET "/api/guilds/$GUILD/channels" | jq -e --arg c "$EC" '.[] | select(.id == $c) | .topic == "Raid strats only"' >/dev/null
fail_call PATCH "/api/channels/$EC" '{"name":"   "}'
fail_call PATCH "/api/channels/$EC" '{"slowmode_secs":-1}'
fail_call PATCH "/api/channels/$EC" '{"slowmode_secs":21601}'
fail_call PATCH "/api/channels/$EC" '{"content_mode":"spicy"}'
VC=$(call POST "/api/guilds/$GUILD/channels" '{"name":"Edit Voice","kind":2}' | jq -r .id)
call PATCH "/api/channels/$VC" '{"name":"Raid Comms"}' | jq -e '.name == "Raid Comms" and .kind == 2' >/dev/null
fail_call PATCH "/api/channels/$VC" '{"topic":"voice has no topic"}'
fail_call PATCH "/api/channels/$VC" '{"content_mode":"nsfw"}'
fail_call PATCH "/api/channels/$VC" '{"slowmode_secs":5}'
# Needs Manage Channels; strangers don't even see the channel.
TOKEN=$TOKEN_B; fail_call PATCH "/api/channels/$EC" '{"topic":"bob was here"}'; TOKEN=$TOKEN_A
TOKEN=$OUT; fail_call PATCH "/api/channels/$EC" '{"topic":"stranger"}'; TOKEN=$TOKEN_A
call GET "/api/guilds/$GUILD/channels" | jq -e --arg c "$EC" '.[] | select(.id == $c) | .topic == "Raid strats only"' >/dev/null

step "slowmode"
call PATCH "/api/channels/$EC" '{"slowmode_secs":5}' | jq -e '.slowmode_secs == 5' >/dev/null
TOKEN=$TOKEN_B
SLOW1=$(call POST "/api/channels/$EC/messages" '{"content":"first!"}' | jq -r .id)
fail_call POST "/api/channels/$EC/messages" '{"content":"second, too soon"}'
grep -q "Slowmode is on. You can post again in [1-5]s." /tmp/body || { echo "wrong slowmode reply:"; cat /tmp/body; exit 1; }
# Deleting your message doesn't reset the wait; uploads wait too.
call DELETE "/api/channels/$EC/messages/$SLOW1" >/dev/null
fail_call POST "/api/channels/$EC/messages" '{"content":"deleted it, try again"}'
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Authorization: Bearer $TOKEN" -F "content=upload too soon" -F "files=@/tmp/short.wav" "$BASE/api/channels/$EC/messages/upload")
[[ "$code" == 400 ]] && grep -q "Slowmode" /tmp/body || { echo "upload skipped slowmode -> $code"; cat /tmp/body; exit 1; }
TOKEN=$TOKEN_A
# Manage Channels (Alice owns the place) skips it...
call POST "/api/channels/$EC/messages" '{"content":"mods skip slowmode 1"}' >/dev/null
call POST "/api/channels/$EC/messages" '{"content":"mods skip slowmode 2"}' >/dev/null
# ...and so does Manage Messages in just this channel.
call PUT "/api/channels/$EC/permissions/$BOB" '{"kind":"member","allow":"128","deny":"0"}' >/dev/null
TOKEN=$TOKEN_B
call POST "/api/channels/$EC/messages" '{"content":"bob can manage messages here 1"}' >/dev/null
call POST "/api/channels/$EC/messages" '{"content":"bob can manage messages here 2"}' >/dev/null
TOKEN=$TOKEN_A
call DELETE "/api/channels/$EC/permissions/$BOB" >/dev/null
sleep 5.3
TOKEN=$TOKEN_B; call POST "/api/channels/$EC/messages" '{"content":"after the wait"}' >/dev/null; TOKEN=$TOKEN_A
call PATCH "/api/channels/$EC" '{"slowmode_secs":0}' >/dev/null
TOKEN=$TOKEN_B; call POST "/api/channels/$EC/messages" '{"content":"slowmode off again"}' >/dev/null; TOKEN=$TOKEN_A

step "spoiler and age-restricted (18+) channels"
call PATCH "/api/channels/$EC" '{"content_mode":"spoiler"}' | jq -e '.content_mode == "spoiler"' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/channels/$EC/messages" | jq -e 'length > 0' >/dev/null; TOKEN=$TOKEN_A
NS=$(call POST "/api/guilds/$GUILD/channels" '{"name":"after-dark"}' | jq -r .id)
call POST "/api/channels/$NS/messages" '{"content":"before it was restricted"}' >/dev/null
call PATCH "/api/channels/$NS" '{"content_mode":"nsfw","topic":"Grown-ups only"}' | jq -e '.content_mode == "nsfw" and (.nsfw_accepted | not)' >/dev/null
# Everyone answers the gate, owner included.
fail_call GET "/api/channels/$NS/messages"
jq -e '.code == "age_restricted" and (.error | contains("18+"))' /tmp/body >/dev/null
call PUT "/api/channels/$NS/age-gate" >/dev/null
call PUT "/api/channels/$NS/age-gate" >/dev/null   # twice is fine
call GET "/api/guilds/$GUILD/channels" | jq -e --arg c "$NS" '.[] | select(.id == $c) | .nsfw_accepted' >/dev/null
# Bob is offline: his push says something arrived, but not what.
NM=$(call POST "/api/channels/$NS/messages" '{"content":"zebrafinch secret for <@'"$BOB"'>"}' | jq -r .id)
call PUT "/api/channels/$NS/pins/$NM" >/dev/null
call GET "/api/guilds/$GUILD/search?q=zebrafinch" | jq -e '.total == 1' >/dev/null
for i in $(seq 1 20); do
  curl -fsS "$PUSH/received/bob" | jq -e 'any(.[]; .payload.title == "New message in #after-dark")' >/dev/null 2>&1 && break
  sleep 0.5
done
RB=$(curl -fsS "$PUSH/received/bob")
check_push "$RB" 'any(.[]; .payload.title == "New message in #after-dark" and (.payload.body | contains("age-restricted")))' "bob: content-free push from an 18+ channel"
check_push "$RB" 'all(.[]; ((.payload.body // "") + (.payload.title // "")) | contains("zebrafinch") | not)' "bob: no 18+ content in pushes"
# Bob hasn't said he's 18+: no messages, pins, threads, search results, posting or reacting.
TOKEN=$TOKEN_B
call GET "/api/guilds/$GUILD/channels" | jq -e --arg c "$NS" '.[] | select(.id == $c) | .content_mode == "nsfw" and .topic == "Grown-ups only" and (.nsfw_accepted | not)' >/dev/null
fail_call GET "/api/channels/$NS/messages"; jq -e '.code == "age_restricted"' /tmp/body >/dev/null
fail_call GET "/api/channels/$NS/messages?around=$NM"
fail_call GET "/api/channels/$NS/pins"
fail_call GET "/api/channels/$NS/threads"
fail_call POST "/api/channels/$NS/messages" '{"content":"let me in"}'
fail_call PUT "/api/channels/$NS/messages/$NM/reactions/%F0%9F%91%8D"
fail_call POST "/api/channels/$NS/polls" '{"question":"in?","options":["yes","no"]}'
call GET "/api/guilds/$GUILD/search?q=zebrafinch" | jq -e '.total == 0 and .messages == []' >/dev/null
call GET "/api/guilds/$GUILD/search?q=$(enc 'in:#after-dark')" | jq -e '.total == 0' >/dev/null
# The gateway holds back its messages too (but still says the channel changed).
node ci/gateway_probe.mjs "$BASE" "$TOKEN_B" /tmp/gw-bob.jsonl 25 &
PROBE=$!
for i in $(seq 1 40); do grep -q '"READY"' /tmp/gw-bob.jsonl 2>/dev/null && break; sleep 0.25; done
grep -q '"READY"' /tmp/gw-bob.jsonl || { echo "gateway probe never got READY"; cat /tmp/gw-bob.jsonl; exit 1; }
TOKEN=$TOKEN_A
call POST "/api/channels/$NS/messages" '{"content":"gatewaycheck hidden quokka"}' >/dev/null
call PUT "/api/channels/$NS/messages/$NM/reactions/%F0%9F%94%A5" >/dev/null
call PATCH "/api/channels/$NS" '{"topic":"Grown-ups only, mind"}' >/dev/null
call POST "/api/channels/$CHAN/messages" '{"content":"gatewaycheck visible wombat"}' >/dev/null
for i in $(seq 1 40); do grep -q "visible wombat" /tmp/gw-bob.jsonl && break; sleep 0.25; done
sleep 0.5
jq -s -e --arg ns "$NS" '
  any(.[]; .t == "MESSAGE_CREATE" and (.d.content | contains("visible wombat")))
  and any(.[]; .t == "CHANNEL_UPDATE" and .d.id == $ns and .d.topic == "Grown-ups only, mind")
  and all(.[]; (.t | IN("MESSAGE_CREATE", "MESSAGE_UPDATE", "REACTION_ADD", "TYPING_START", "THREAD_UPDATE")) and .d.channel_id == $ns | not)
  and all(.[]; tostring | contains("quokka") | not)' /tmp/gw-bob.jsonl >/dev/null \
  || { echo "gateway leaked an 18+ channel (or missed events):"; cat /tmp/gw-bob.jsonl; exit 1; }
# Bob says he's 18+: everything shows up, on every device (and live).
TOKEN=$TOKEN_B
call PUT "/api/channels/$NS/age-gate" >/dev/null
call GET "/api/channels/$NS/messages" | jq -e 'any(.[]; .content | contains("zebrafinch")) and any(.[]; .content | contains("quokka"))' >/dev/null
call GET "/api/channels/$NS/pins" | jq -e 'length == 1' >/dev/null
call GET "/api/guilds/$GUILD/search?q=zebrafinch" | jq -e '.total == 1' >/dev/null
call GET "/api/guilds/$GUILD/channels" | jq -e --arg c "$NS" '.[] | select(.id == $c) | .nsfw_accepted' >/dev/null
TOKEN=$TOKEN_A
call POST "/api/channels/$NS/messages" '{"content":"gatewaycheck now visible platypus"}' >/dev/null
for i in $(seq 1 40); do grep -q "platypus" /tmp/gw-bob.jsonl && break; sleep 0.25; done
kill $PROBE 2>/dev/null || true
jq -s -e --arg ns "$NS" '
  any(.[]; .t == "CHANNEL_AGE_GATE" and .d.channel_id == $ns and .d.accepted)
  and any(.[]; .t == "MESSAGE_CREATE" and .d.channel_id == $ns and (.d.content | contains("platypus")))' /tmp/gw-bob.jsonl >/dev/null \
  || { echo "no 18+ messages after accepting:"; cat /tmp/gw-bob.jsonl; exit 1; }
# Taking it back closes the gate again.
TOKEN=$TOKEN_B
call DELETE "/api/channels/$NS/age-gate" >/dev/null
fail_call GET "/api/channels/$NS/messages"
call GET "/api/guilds/$GUILD/channels" | jq -e --arg c "$NS" '.[] | select(.id == $c) | .nsfw_accepted | not' >/dev/null
TOKEN=$OUT; fail_call PUT "/api/channels/$NS/age-gate"; TOKEN=$TOKEN_A
# Back to default: open to all again.
call PATCH "/api/channels/$NS" '{"content_mode":"default"}' >/dev/null
TOKEN=$TOKEN_B; call GET "/api/channels/$NS/messages" | jq -e 'length >= 3' >/dev/null; TOKEN=$TOKEN_A
# Every change is in the audit log.
AUDC=$(call GET "/api/guilds/$GUILD/audit-log?action=channel_update&limit=100")
jq -e '
  any(.entries[]; .details | contains("Name: edit-me → raid-talk"))
  and any(.entries[]; .details | contains("Topic: (none) → “Raid strats only”"))
  and any(.entries[]; .details | contains("Slowmode: Off → 5s"))
  and any(.entries[]; .details | contains("Content: Default → Spoiler channel"))
  and any(.entries[]; .details | contains("Content: Default → Age-restricted"))
  and any(.entries[]; .details | contains("Name: Edit Voice → Raid Comms"))
  and all(.entries[]; .target_kind == "channel")' <<<"$AUDC" >/dev/null || { echo "channel changes missing from the audit log:"; jq '.entries[] | .details' <<<"$AUDC"; exit 1; }

step "invite links: expiry, max uses, list, revoke, preview, the /invite page, assetlinks.json"
TOKEN=$TOKEN_A
IG=$(call POST /api/guilds '{"name":"Invite <Test> & Co"}'); IGUILD=$(jq -r .id <<<"$IG")
ANAME=$(call GET /api/users/@me | jq -r .display_name)
# An empty body still makes a week-long, unlimited invite (older clients).
I0=$(call POST "/api/guilds/$IGUILD/invites")
jq -e --arg base "$BASE" '(.code | test("^[A-Za-z0-9]{8}$")) and .url == ($base + "/invite/" + .code)
  and .max_uses == null and .uses == 0' <<<"$I0" >/dev/null \
  || { echo "bad default invite: $I0"; exit 1; }
jq -e --arg n "$ANAME" '.creator.name == $n' <<<"$I0" >/dev/null || { echo "creator: $I0"; exit 1; }
jq -e '((.expires_at | sub("\\.[0-9]+"; "") | fromdateiso8601) - now) | (. > 6.9*86400 and . < 7.1*86400)' <<<"$I0" >/dev/null \
  || { echo "default invite doesn't last 7 days: $I0"; exit 1; }
# One hour, two uses.
I1=$(call POST "/api/guilds/$IGUILD/invites" '{"max_age_secs":3600,"max_uses":2}'); C1=$(jq -r .code <<<"$I1")
jq -e '.max_uses == 2 and (((.expires_at | sub("\\.[0-9]+"; "") | fromdateiso8601) - now) | (. > 3500 and . < 3700))' <<<"$I1" >/dev/null \
  || { echo "bad 1h/2-use invite: $I1"; exit 1; }
# Never expires.
I2=$(call POST "/api/guilds/$IGUILD/invites" '{"max_age_secs":0,"max_uses":0}'); C2=$(jq -r .code <<<"$I2")
jq -e '.expires_at == null and .max_uses == null' <<<"$I2" >/dev/null || { echo "bad forever invite: $I2"; exit 1; }
fail_call POST "/api/guilds/$IGUILD/invites" '{"max_age_secs":5}'
fail_call POST "/api/guilds/$IGUILD/invites" '{"max_age_secs":99999999}'
fail_call POST "/api/guilds/$IGUILD/invites" '{"max_uses":-1}'
fail_call POST "/api/guilds/$IGUILD/invites" 'not json'
# Preview without signing in.
TOKEN=""
PV=$(call GET "/api/invites/$C1")
jq -e --arg g "$IGUILD" '.guild.id == $g and .guild.name == "Invite <Test> & Co" and .member_count == 1
  and (.online_count | type == "number") and (.inviter.name | length > 0)' <<<"$PV" >/dev/null || { echo "bad preview: $PV"; exit 1; }
fail_call GET "/api/invites/nope1234"
fail_call GET "/api/invites/..%2Fetc"
# Joining counts uses; a 2-use invite lets two people in, not three.
fail_call POST "/api/invites/$C1"
IU=()
for n in 1 2 3; do
  R=$(call POST /api/auth/register "{\"username\":\"invitee$n\",\"password\":\"password123\",\"display_name\":\"Invitee $n\"}")
  IU+=("$(jq -r .token <<<"$R")")
done
TOKEN=${IU[0]}; call POST "/api/invites/$C1" | jq -e --arg g "$IGUILD" '.id == $g' >/dev/null
# Already in: you just get the outhouse back and no use is counted.
call POST "/api/invites/$C1" | jq -e --arg g "$IGUILD" '.id == $g' >/dev/null
TOKEN=${IU[1]}; call POST "/api/invites/$C1" >/dev/null
TOKEN=${IU[2]}; fail_call POST "/api/invites/$C1"
grep -q "expired or been used up" /tmp/body || { echo "wrong used-up message:"; cat /tmp/body; exit 1; }
TOKEN=""; fail_call GET "/api/invites/$C1"
TOKEN=$TOKEN_A
call GET "/api/guilds/$IGUILD/members" | jq -e 'length == 3' >/dev/null
# The list shows invites that still work, with their creator.
LIST=$(call GET "/api/guilds/$IGUILD/invites")
jq -e --arg c1 "$C1" --arg c2 "$C2" '(map(.code) | index($c1) == null) and (map(.code) | index($c2) != null)
  and all(.[]; .creator.id != null and (.url | contains("/invite/")))' <<<"$LIST" >/dev/null || { echo "bad invite list: $LIST"; exit 1; }
# Members see only their own invites; they can't revoke someone else's.
TOKEN=${IU[0]}
MI=$(call POST "/api/guilds/$IGUILD/invites" '{"max_age_secs":1800,"max_uses":1}'); MC=$(jq -r .code <<<"$MI")
call GET "/api/guilds/$IGUILD/invites" | jq -e --arg m "$MC" 'length == 1 and .[0].code == $m' >/dev/null
fail_call DELETE "/api/invites/$C2"
call DELETE "/api/invites/$MC" >/dev/null
TOKEN=${IU[2]}; fail_call GET "/api/guilds/$IGUILD/invites"
fail_call POST "/api/guilds/$IGUILD/invites"
# Expired invites are refused.
TOKEN=$TOKEN_A
I3=$(call POST "/api/guilds/$IGUILD/invites" '{"max_age_secs":1800}'); C3=$(jq -r .code <<<"$I3")
psql "$DATABASE_URL" -qc "UPDATE invites SET expires_at = now() - interval '1 minute' WHERE code = '$C3'"
TOKEN=${IU[2]}; fail_call POST "/api/invites/$C3"
TOKEN=""; fail_call GET "/api/invites/$C3"
# Revoked (by the owner, who has Manage Outhouse) = gone.
TOKEN=$TOKEN_A; call DELETE "/api/invites/$C2" >/dev/null
TOKEN=${IU[2]}; fail_call POST "/api/invites/$C2"
TOKEN=$TOKEN_A
# Someone else's invite revoked by a moderator lands in the audit log.
TOKEN=${IU[0]}; MI2=$(call POST "/api/guilds/$IGUILD/invites"); MC2=$(jq -r .code <<<"$MI2"); TOKEN=$TOKEN_A
call DELETE "/api/invites/$MC2" >/dev/null
call GET "/api/guilds/$IGUILD/audit-log?action=invite_delete" | jq -e 'any(.entries[]; .details | contains("Revoked invite"))' >/dev/null

# The /invite/<code> page: works signed out, never cached, link-preview tags with the name filled in (escaped).
I4=$(call POST "/api/guilds/$IGUILD/invites"); C4=$(jq -r .code <<<"$I4")
curl -sS -D /tmp/inv-headers -o /tmp/inv.html "$BASE/invite/$C4"
grep -qi "^HTTP/1.1 200" /tmp/inv-headers || { echo "invite page status:"; cat /tmp/inv-headers; exit 1; }
grep -qi "^cache-control: no-cache" /tmp/inv-headers || { echo "invite page is cacheable:"; cat /tmp/inv-headers; exit 1; }
grep -qi "^content-type: text/html" /tmp/inv-headers
grep -q '<meta property="og:title" content="You&#39;ve been invited to Invite &lt;Test&gt; &amp; Co on Outhouse">' /tmp/inv.html \
  || { echo "og:title missing:"; grep -n "og:" /tmp/inv.html; exit 1; }
grep -q "<meta property=\"og:image\" content=\"$BASE/icons/icon-512.png\">" /tmp/inv.html
grep -q "<meta property=\"og:url\" content=\"$BASE/invite/$C4\">" /tmp/inv.html
grep -q 'name="twitter:card" content="summary"' /tmp/inv.html
grep -q "og:description\" content=\"$ANAME saved you a seat in Invite &lt;Test&gt; &amp; Co. 3 members · 0 online" /tmp/inv.html \
  || { echo "og:description:"; grep -n "description" /tmp/inv.html; exit 1; }
grep -q 'data-state="valid"' /tmp/inv.html
if grep -q "<Test>" /tmp/inv.html; then echo "invite page didn't escape the name"; exit 1; fi
grep -q '{{' /tmp/inv.html && { echo "unfilled template bits:"; grep -n '{{' /tmp/inv.html; exit 1; }
sed -n 's:.*<script type="application/json" id="invite-data">\(.*\)</script>.*:\1:p' /tmp/inv.html \
  | jq -e --arg c "$C4" '.valid and .invite.code == $c and .invite.guild.name == "Invite <Test> & Co" and .android_package == "cc.hartspace.outhouse"' >/dev/null \
  || { echo "bad invite data:"; grep -n "invite-data" /tmp/inv.html; exit 1; }
# Expired, revoked and made-up codes: a friendly 404 page.
for bad in "$C3" "$C2" "zzzzzzzz" "%3Cscript%3E"; do
  code=$(curl -sS -o /tmp/inv-bad.html -w "%{http_code}" "$BASE/invite/$bad")
  [ "$code" = 404 ] || { echo "/invite/$bad -> $code"; exit 1; }
  grep -q "gone down the long drop" /tmp/inv-bad.html
  grep -q 'data-state="invalid"' /tmp/inv-bad.html
  grep -q '<meta property="og:title" content="This invite has gone down the long drop">' /tmp/inv-bad.html
  grep -q '"code":""\|"code":"[A-Za-z0-9]*"' /tmp/inv-bad.html || { echo "bad code in the page:"; grep -n invite-data /tmp/inv-bad.html; exit 1; }
  if grep -q "%3C\|&lt;script" /tmp/inv-bad.html; then echo "made-up code echoed into the page"; exit 1; fi
done
# Android App Links file.
curl -sS -D /tmp/al-headers -o /tmp/assetlinks.json "$BASE/.well-known/assetlinks.json"
grep -qi "^content-type: application/json" /tmp/al-headers
jq -e '.[0].relation == ["delegate_permission/common.handle_all_urls"]
  and .[0].target.namespace == "android_app"
  and .[0].target.package_name == "cc.hartspace.outhouse"
  and (.[0].target.sha256_cert_fingerprints[0] | test("^([0-9A-F]{2}:){31}[0-9A-F]{2}$"))
  and .[0].target.sha256_cert_fingerprints[0] == "D6:31:45:91:E5:FA:44:C0:82:28:27:92:C5:28:31:88:3B:FE:0C:FD:85:63:2B:51:8B:E5:04:FE:83:0F:8A:E3"' /tmp/assetlinks.json >/dev/null \
  || { echo "bad assetlinks.json:"; cat /tmp/assetlinks.json; exit 1; }
TOKEN=$TOKEN_A

step "security: headers (CSP, HSTS, framing, sniffing) on pages, API and files"
for path in / /app /invite/zzzzzzzz /api/health /style.css; do
  curl -sS -D /tmp/sec-h -o /tmp/sec-b "$BASE$path"
  for want in "x-content-type-options: nosniff" "x-frame-options: DENY" "referrer-policy: strict-origin-when-cross-origin" \
              "strict-transport-security: max-age=31536000" "cross-origin-opener-policy: same-origin"; do
    grep -qi "^$want" /tmp/sec-h || { echo "$path is missing '$want':"; cat /tmp/sec-h; exit 1; }
  done
  grep -qi "^permissions-policy: .*microphone=(self).*display-capture=(self)" /tmp/sec-h || { echo "$path permissions-policy:"; cat /tmp/sec-h; exit 1; }
  CSP=$(grep -i "^content-security-policy:" /tmp/sec-h | cut -d' ' -f2- | tr -d '\r')
  for want in "default-src 'self'" "script-src 'self' blob: 'sha256-" "object-src 'none'" "frame-ancestors 'none'" "base-uri 'self'" \
              "connect-src 'self' http://outhouse.localhost"; do
    grep -qF "$want" <<<"$CSP" || { echo "$path CSP lacks \"$want\": $CSP"; exit 1; }
  done
  grep -qF "'unsafe-inline' https://fonts" <<<"$CSP"   # styles only
  if grep -q "script-src[^;]*unsafe" <<<"$CSP"; then echo "scripts allowed inline/eval: $CSP"; exit 1; fi
done
# The landing and invite pages' inline scripts are allowed by their exact hashes.
for page in landing.html invite.html; do
  HASH=$(python3 -c 'import re,sys,hashlib,base64
html=open(sys.argv[1]).read()
for attrs, body in re.findall(r"<script([^>]*)>(.*?)</script>", html, re.S | re.I):
    if "src=" not in attrs and "application/json" not in attrs:
        print(base64.b64encode(hashlib.sha256(body.encode()).digest()).decode())' "web/$page")
  [ -n "$HASH" ] || { echo "no inline script found in $page"; exit 1; }
  for h in $HASH; do grep -qF "'sha256-$h'" <<<"$CSP" || { echo "CSP doesn't allow $page's inline script ($h)"; exit 1; }; done
done
# Uploaded files keep their own locked-down policy (no scripts, sandboxed).
UP=$(curl -fsS -H "Authorization: Bearer $TOKEN_A" -F "content=header check" -F "files=@/tmp/pixel.png;type=image/png" "$BASE/api/channels/$CHAN/messages/upload")
FURL=$(jq -r '.attachments[0].url' <<<"$UP")
curl -sS -D /tmp/sec-f -o /dev/null "$BASE$FURL"
grep -qi "^content-security-policy: default-src 'none'; sandbox" /tmp/sec-f || { echo "file CSP:"; cat /tmp/sec-f; exit 1; }
grep -qi "^x-content-type-options: nosniff" /tmp/sec-f
# The web client loads LiveKit from this server, not a CDN.
grep -q 'src="vendor/livekit-client.umd.js"' web/index.html
curl -fsS -o /dev/null "$BASE/vendor/livekit-client.umd.js"
if grep -rq "cdn.jsdelivr" web/*.html; then echo "a page still loads scripts from a CDN"; exit 1; fi

step "security: NUL characters get a 400, not a server error"
TOKEN=$TOKEN_A
fail_call POST "/api/channels/$CHAN/messages" '{"content":"nul \u0000 here"}'
grep -q "NUL" /tmp/body
fail_call PATCH /api/users/@me '{"display_name":"a\u0000b"}'
fail_call GET "/api/guilds/$GUILD/search?q=a%00b"
call POST "/api/channels/$CHAN/messages" '{"content":"an escaped backslash: \\u0000 is fine"}' | jq -e '.content == "an escaped backslash: \\u0000 is fine"' >/dev/null

step "security: who's in a hidden voice channel stays hidden"
TOKEN=$TOKEN_A
call POST /api/voice/leave >/dev/null
HV=$(call POST "/api/guilds/$GUILD/channels" '{"name":"Officers","kind":2}' | jq -r .id)
call PUT "/api/channels/$HV/permissions/$GUILD" '{"kind":"role","allow":"0","deny":"1"}' >/dev/null
call POST "/api/channels/$HV/voice/join" >/dev/null
call GET "/api/guilds/$GUILD/voice" | jq -e --arg a "$ALICE" --arg c "$HV" 'any(.[]; .user_id == $a and .channel_id == $c)' >/dev/null
TOKEN=$TOKEN_B
call GET "/api/guilds/$GUILD/voice" | jq -e --arg a "$ALICE" '[.[] | select(.user_id == $a)] | length == 0' >/dev/null \
  || { echo "Bob can see who's in a voice channel he can't see"; exit 1; }
TOKEN=$TOKEN_A
call POST /api/voice/leave >/dev/null
call DELETE "/api/channels/$HV" >/dev/null

step "security: no CORS, no cookies"
curl -sS -D /tmp/sec-c -o /dev/null -H "Origin: https://evil.example" -H "Authorization: Bearer $TOKEN_A" "$BASE/api/users/@me"
if grep -qi "^access-control-allow-origin" /tmp/sec-c; then echo "API answers cross-origin reads:"; cat /tmp/sec-c; exit 1; fi
if grep -qi "^set-cookie" /tmp/sec-c; then echo "API sets cookies:"; cat /tmp/sec-c; exit 1; fi

step "security: javascript: links in bot embeds are dropped"
TOKEN=$TOKEN_A
XB=$(call POST "/api/guilds/$GUILD/bots" '{"name":"Linky"}'); XB_TOKEN=$(jq -r .token <<<"$XB"); XB_ID=$(jq -r .bot.id <<<"$XB")
TOKEN=$XB_TOKEN
call PUT "/api/guilds/$GUILD/commands" '[{"name":"linky","description":"posts a link"}]' >/dev/null
XCMD=$(call GET "/api/guilds/$GUILD/commands" | jq -r '.[] | select(.name == "linky") | .id')
node ci/gateway_probe.mjs "$BASE" "$XB_TOKEN" /tmp/gw-linky.jsonl 20 &
XB_PROBE=$!
for i in $(seq 1 20); do grep -q '"READY"' /tmp/gw-linky.jsonl 2>/dev/null && break; sleep 0.5; done
TOKEN=$TOKEN_A
call POST "/api/channels/$CHAN/interactions" '{"command_id":"'"$XCMD"'","options":{}}' >/dev/null
for i in $(seq 1 20); do grep -q '"INTERACTION_CREATE"' /tmp/gw-linky.jsonl && break; sleep 0.5; done
XI=$(jq -r 'select(.t == "INTERACTION_CREATE") | .d.id' /tmp/gw-linky.jsonl | head -1)
TOKEN=$XB_TOKEN
call POST "/api/interactions/$XI/reply" '{"content":"","embeds":[{"url":"javascript:alert(document.domain)","kind":"rich","title":"click me","author_name":"me","author_url":" JavaScript:alert(1)","thumbnail_url":"data:text/html,x","image_url":"https://img.example/a.png","fields":[{"name":"a","value":"b"}]}]}' \
  | jq -e '.embeds[0].url == "" and .embeds[0].author_url == null and .embeds[0].thumbnail_url == null
           and .embeds[0].image_url == "https://img.example/a.png" and .embeds[0].title == "click me"' >/dev/null
TOKEN=$TOKEN_A
call GET "/api/channels/$CHAN/messages" | jq -e '[.[] | .embeds[] | .url, .author_url, .image_url, .thumbnail_url | select(. != null and . != "") | select(test("^(https?://|/[^/])") | not)] | length == 0' >/dev/null \
  || { echo "a non-web link is stored in an embed"; exit 1; }
kill $XB_PROBE 2>/dev/null || true
call DELETE "/api/bots/$XB_ID" >/dev/null

step "security: gateway refuses huge messages and closes signed-out sessions"
node -e '
const ws = new WebSocket(process.argv[1].replace(/^http/, "ws") + "/api/gateway");
const t = setTimeout(() => { console.error("still open after a 100 KB message"); process.exit(1); }, 8000);
ws.addEventListener("open", () => {
  ws.send(JSON.stringify({ op: "IDENTIFY", d: { token: process.argv[2] } }));
  setTimeout(() => ws.send("x".repeat(100 * 1024)), 500);
});
ws.addEventListener("close", () => { clearTimeout(t); process.exit(0); });
ws.addEventListener("error", () => {});' "$BASE" "$TOKEN_A"
for i in $(seq 1 90); do grep -q '"INVALID_SESSION"' /tmp/gw-logout.jsonl && break; sleep 1; done
grep -q '"READY"' /tmp/gw-logout.jsonl || { echo "logout probe never connected:"; cat /tmp/gw-logout.jsonl; exit 1; }
grep -q '"INVALID_SESSION"' /tmp/gw-logout.jsonl || { echo "signed-out session's gateway stayed open"; tail -3 /tmp/gw-logout.jsonl; exit 1; }
kill $GW_LOGOUT_PID 2>/dev/null || true

step "security: sign-in brute-force limits (per account and per address)"
TOKEN=""
call POST /api/auth/register '{"username":"lockme","password":"password123"}' >/dev/null
xff_login() { # xff_login IP USER PASSWORD -> HTTP status
  curl -sS -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json" -H "X-Forwarded-For: $1" \
    -d '{"username":"'"$2"'","password":"'"$3"'"}' "$BASE/api/auth/login"
}
# Ten wrong passwords for one account (from ten addresses) lock it, even for the right password.
for i in $(seq 1 10); do [ "$(xff_login "198.51.100.$i" lockme wrongpass$i)" = 401 ]; done
[ "$(xff_login 198.51.100.99 lockme password123)" = 429 ] || { echo "account wasn't locked:"; cat /tmp/body; exit 1; }
grep -q "Wait 15 minutes" /tmp/body
# Twenty wrong guesses from one address block that address (for any account)...
for i in $(seq 1 20); do [ "$(xff_login 203.0.113.50 "nobody$i" wrongpass)" = 401 ]; done
[ "$(xff_login 203.0.113.50 alice password123)" = 429 ]
# ...but not anyone else. A client's own X-Forwarded-For entries before the
# proxy's are ignored (see security::client_ip unit tests).
[ "$(xff_login 203.0.113.51 alice password123)" = 200 ]
[ "$(xff_login "203.0.113.50, 203.0.113.52" alice password123)" = 200 ]
# Absurdly long passwords are refused without hashing them.
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json" \
  -d "{\"username\":\"longpw\",\"password\":\"$(head -c 400 /dev/zero | tr '\0' a)\"}" "$BASE/api/auth/register")
[ "$code" = 400 ] && grep -q "up to 256" /tmp/body

step "security: sign-up and invite look-up limits"
for i in $(seq 1 20); do
  code=$(curl -sS -o /dev/null -w "%{http_code}" -H "Content-Type: application/json" -H "X-Forwarded-For: 203.0.113.60" \
    -d '{"username":"!","password":"password123"}' "$BASE/api/auth/register")
  [ "$code" = 400 ]
done
code=$(curl -sS -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json" -H "X-Forwarded-For: 203.0.113.60" \
  -d '{"username":"toomany","password":"password123"}' "$BASE/api/auth/register")
[ "$code" = 429 ] || { echo "sign-up limit: $code"; cat /tmp/body; exit 1; }
for i in $(seq 1 120); do
  code=$(curl -sS -o /dev/null -w "%{http_code}" -H "X-Forwarded-For: 203.0.113.70" "$BASE/api/invites/zzzz$i")
  [ "$code" = 404 ] || { echo "invite look-up $i -> $code"; exit 1; }
done
[ "$(curl -sS -o /dev/null -w "%{http_code}" -H "X-Forwarded-For: 203.0.113.70" "$BASE/api/invites/$CODE")" = 429 ]
[ "$(curl -sS -o /dev/null -w "%{http_code}" -H "X-Forwarded-For: 203.0.113.70" "$BASE/invite/$CODE")" = 429 ]
[ "$(curl -sS -o /dev/null -w "%{http_code}" -H "X-Forwarded-For: 203.0.113.71" "$BASE/api/invites/$CODE")" = 200 ]
call GET /api/auth/registration | jq -e '.mode == "open"' >/dev/null

step "security: REGISTRATION=invite and REGISTRATION=closed"
TOKEN=$TOKEN_A
REG_INV=$(call POST "/api/guilds/$GUILD/invites" | jq -r .code)
TOKEN=""
for mode in invite closed; do
  PORT=$([ "$mode" = invite ] && echo 8181 || echo 8182)
  BIND_ADDR=127.0.0.1:$PORT REGISTRATION=$mode ./target/debug/outhouse-server >"/tmp/outhouse-$mode.log" 2>&1 &
  REG_PID=$!
  for i in $(seq 1 30); do curl -fsS "http://127.0.0.1:$PORT/api/health" >/dev/null 2>&1 && break; sleep 0.5; done
  R="http://127.0.0.1:$PORT"
  curl -fsS "$R/api/auth/registration" | jq -e --arg m "$mode" '.mode == $m' >/dev/null
  reg() { curl -sS -o /tmp/body -w "%{http_code}" -H "Content-Type: application/json" -d "$1" "$R/api/auth/register"; }
  [ "$(reg '{"username":"reg'"$mode"'1","password":"password123"}')" = 403 ] || { echo "$mode: signed up without an invite"; cat /tmp/body; exit 1; }
  [ "$(reg '{"username":"reg'"$mode"'2","password":"password123","invite":"nope1234"}')" = 403 ]
  if [ "$mode" = invite ]; then
    grep -q "need an invite" /tmp/body
    [ "$(reg '{"username":"reginvite3","password":"password123","invite":"'"$REG_INV"'"}')" = 200 ] || { cat /tmp/body; exit 1; }
    T3=$(jq -r .token /tmp/body)
    curl -fsS -X POST -H "Authorization: Bearer $T3" "$R/api/invites/$REG_INV" | jq -e --arg g "$GUILD" '.id == $g' >/dev/null
  else
    grep -q "closed" /tmp/body
    [ "$(reg '{"username":"regclosed3","password":"password123","invite":"'"$REG_INV"'"}')" = 403 ]
  fi
  # Signing in still works.
  curl -fsS -H "Content-Type: application/json" -d '{"username":"bob","password":"password123"}' "$R/api/auth/login" >/dev/null
  kill $REG_PID; wait $REG_PID 2>/dev/null || true
done
TOKEN=$TOKEN_A

echo "Smoke test passed."
